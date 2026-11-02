//! Headless harness for [`App`]: fake backend implementations that record
//! what the UI asks of the platform, a temp folder with real image files,
//! and a driver that runs the loader until it is idle, so App-level
//! behaviour (keys, commands, menu state, mouse, loader results, grid,
//! filmstrip, batch dialog) can be tested on Linux.
//!
//! ```text
//! let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
//! h.press(Key::Right, false, false);
//! assert_eq!(h.current_name().as_deref(), Some("b.png"));
//! ```
//!
//! Each [`Harness`] holds a process-wide lock while it lives: the UI
//! language is a global, so harness tests run one after another.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::decode::{self, DecodeError, Probe, Request};
use blinzel_core::filelist::Entry;
use blinzel_core::geom::{Affine, Orientation, Rect};
use blinzel_core::icc::Profile;
use blinzel_core::layout::{Layout, ViewMode, ViewState};
use blinzel_core::lut::Lut3d;
use blinzel_core::pixel::Image;
use blinzel_core::settings::Settings;
use blinzel_i18n::Lang;

use crate::app::{Action, App};
use crate::backend::{Backend, Canvas, Device, Host, Key, Services, Timer, Wake};
use crate::browse::grid_layout;
use crate::color::ColorManager;
use crate::gfx::{AtlasPages, ImageInfo, Placed, Rgba, Shape};
use crate::loader::Loader;
use crate::menu::{Cmd, MenuItem, MenuState};

// ------------------------------------------------------------------ device

/// An image stage "on the GPU": only its sizes.
#[derive(Debug)]
pub(crate) struct FakeImage {
    pub size: (u32, u32),
    pub full_size: (u32, u32),
    pub orientation: Orientation,
}

impl ImageInfo for FakeImage {
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn full_size(&self) -> (u32, u32) {
        self.full_size
    }
    fn orientation(&self) -> Orientation {
        self.orientation
    }
    fn bytes(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize * 4
    }
}

/// A device that keeps nothing but a log of the uploads.
#[derive(Default)]
pub(crate) struct FakeDevice {
    /// Stage size of every image uploaded, in order.
    pub uploads: Mutex<Vec<(u32, u32)>>,
}

impl Device for FakeDevice {
    type Image = FakeImage;
    type Lut = ();

    fn upload(&self, img: &Image, orientation: Orientation) -> Result<FakeImage, String> {
        let size = (img.width, img.height);
        let full_size = (img.full_width, img.full_height);
        self.uploads.lock().unwrap().push(size);
        Ok(FakeImage {
            size,
            full_size,
            orientation,
        })
    }
    fn upload_lut(&self, _: &Lut3d) -> Result<(), String> {
        Ok(())
    }
    fn texture_budget(&self) -> usize {
        1 << 30
    }
    fn adapter_name(&self) -> String {
        "test".into()
    }
    fn is_software(&self) -> bool {
        true
    }
}

// ------------------------------------------------------------------ canvas

/// An image drawn in a frame.
#[derive(Clone, Debug)]
pub(crate) struct DrawnImage {
    /// The stage's size (texels).
    pub size: (u32, u32),
    pub full_size: (u32, u32),
}

/// What one [`App::render`] drew.
#[derive(Clone, Debug, Default)]
pub(crate) struct Frame {
    pub images: Vec<DrawnImage>,
    pub shapes: Vec<Shape>,
    /// Thumbnails, by destination rectangle.
    pub thumbs: Vec<Rect>,
    pub texts: Vec<String>,
    /// The colour of each of [`Frame::texts`].
    pub text_colors: Vec<Rgba>,
    /// The top-left corner of each of [`Frame::texts`].
    pub text_positions: Vec<(f32, f32)>,
}

/// A canvas that measures text (7 px per character, 16 px high) and
/// records each frame.
#[derive(Default)]
pub(crate) struct FakeCanvas {
    /// The frame being drawn, then the last one presented.
    pub frame: Frame,
    /// Frames presented so far.
    pub presented: usize,
}

impl AtlasPages for FakeCanvas {
    fn add_page(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn upload(
        &mut self,
        _: usize,
        _: (u32, u32),
        _: (u32, u32),
        _: (u32, u32),
        _: &blinzel_core::thumb::Planes,
    ) {
    }
}

impl Canvas for FakeCanvas {
    type Device = FakeDevice;

    fn begin(&mut self, _: Rgba) -> bool {
        self.frame = Frame::default();
        true
    }
    fn draw_image(&mut self, img: &FakeImage, _: &Affine, _: &Rect, _: Rgba, _: Option<&()>) {
        self.frame.images.push(DrawnImage {
            size: img.size,
            full_size: img.full_size,
        });
    }
    fn draw_shapes(&mut self, shapes: &[Shape]) {
        self.frame.shapes.extend_from_slice(shapes);
    }
    fn draw_thumb(&mut self, _: &Placed, dest: &Rect, _: f32) {
        self.frame.thumbs.push(*dest);
    }
    fn text_size(&mut self, text: &str, _: f32, _: f32) -> Option<(u32, u32)> {
        Some((7 * text.chars().count() as u32, 16))
    }
    fn draw_text(&mut self, text: &str, _: f32, _: f32, at: (f32, f32), color: Rgba) {
        self.frame.texts.push(text.to_string());
        self.frame.text_colors.push(color);
        self.frame.text_positions.push(at);
    }
    fn present(&mut self) {
        self.presented += 1;
    }
    fn resize(&mut self, _: (u32, u32)) {}
}

// -------------------------------------------------------------------- host

/// Everything the UI asked of its window.
#[derive(Debug, Default)]
pub(crate) struct HostLog {
    pub invalidations: usize,
    pub title: String,
    /// Timers set and not killed, with their interval.
    pub timers: HashMap<Timer, u32>,
    pub watched: Vec<PathBuf>,
    pub clipboard_offers: usize,
    pub first_frame_shown: bool,
}

/// A window on a 1920 × 1080 monitor (id 1); client points are screen
/// points.
#[derive(Default)]
pub(crate) struct FakeHost {
    pub log: RefCell<HostLog>,
}

impl Host for FakeHost {
    fn invalidate(&self) {
        self.log.borrow_mut().invalidations += 1;
    }
    fn set_title(&self, title: &str) {
        self.log.borrow_mut().title = title.to_string();
    }
    fn set_timer(&self, timer: Timer, ms: u32) {
        self.log.borrow_mut().timers.insert(timer, ms);
    }
    fn kill_timer(&self, timer: Timer) {
        self.log.borrow_mut().timers.remove(&timer);
    }
    fn to_screen(&self, p: (f64, f64)) -> (i32, i32) {
        (p.0 as i32, p.1 as i32)
    }
    fn monitor(&self) -> Option<(isize, (u32, u32))> {
        Some((1, (1920, 1080)))
    }
    fn offer_clipboard(&self) -> bool {
        self.log.borrow_mut().clipboard_offers += 1;
        true
    }
    fn watch_dir(&self, dir: &Path) -> Option<Box<dyn std::any::Any>> {
        self.log.borrow_mut().watched.push(dir.to_path_buf());
        Some(Box::new(()))
    }
    fn first_frame_shown(&self) {
        self.log.borrow_mut().first_frame_shown = true;
    }
}

// ---------------------------------------------------------------- services

/// The real file system and the crate's own decoders; wakes go into a
/// channel the [`Harness`] drains. English, UTC unless a test sets an
/// offset ([`Harness::set_utc_offset`]), sRGB monitor; log lines go to the
/// test's output.
pub(crate) struct FakeServices {
    wakes: Mutex<Sender<Wake>>,
    cache: PathBuf,
    /// Every file read, with the reading thread's name.
    pub reads: Mutex<Vec<(String, PathBuf)>>,
    /// File attributes the listing reports, by name (cloud placeholders).
    pub attributes: Mutex<HashMap<String, u32>>,
    /// Seconds the local clock is ahead of UTC, at every moment.
    pub utc_offset: AtomicI32,
    /// When set, every decode fails with this error (e.g. one from WIC).
    pub decode_error: Mutex<Option<DecodeError>>,
}

impl FakeServices {
    fn with_attributes(&self, mut e: Entry) -> Entry {
        if let Some(a) = self.attributes.lock().unwrap().get(&e.name) {
            e.attributes |= a;
        }
        e
    }
}

impl Services for FakeServices {
    fn wake(&self, w: Wake) {
        let _ = self.wakes.lock().unwrap().send(w);
    }
    fn read_file(&self, path: &Path) -> std::io::Result<PoolBuf> {
        let thread = std::thread::current().name().unwrap_or("").to_string();
        self.reads
            .lock()
            .unwrap()
            .push((thread, path.to_path_buf()));
        crate::files::read_file(path)
    }
    fn list_dir(&self, dir: &Path) -> std::io::Result<Vec<Entry>> {
        let list = crate::files::list_dir(dir)?;
        Ok(list.into_iter().map(|e| self.with_attributes(e)).collect())
    }
    fn entry(&self, path: &Path) -> std::io::Result<Entry> {
        crate::files::entry(path).map(|e| self.with_attributes(e))
    }
    fn probe(&self, data: &[u8]) -> Result<Probe, DecodeError> {
        decode::probe(data)
    }
    fn decode(&self, data: &[u8], req: Request, threads: usize) -> Result<Image, DecodeError> {
        if let Some(e) = self.decode_error.lock().unwrap().clone() {
            return Err(e);
        }
        decode::decode_with(data, req, threads)
    }
    fn log(&self, msg: &str) {
        eprintln!("blinzel: {msg}");
    }
    fn display_target(&self, _: isize) -> Option<Profile> {
        None
    }
    fn thumb_threads(&self) -> usize {
        1
    }
    fn cache_dir(&self) -> Option<PathBuf> {
        Some(self.cache.clone())
    }
    fn system_language(&self) -> Lang {
        blinzel_i18n::FALLBACK
    }
    fn utc_offset(&self, _t: std::time::SystemTime) -> i32 {
        self.utc_offset.load(Ordering::Relaxed)
    }
}

/// The fake platform.
pub(crate) struct TestBackend;

impl Backend for TestBackend {
    type Device = FakeDevice;
    type Canvas = FakeCanvas;
    type Host = FakeHost;
    type Services = FakeServices;
}

// ------------------------------------------------------------- temp folder

/// A folder under the system's temp folder, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> TempDir {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("blinzel-ui-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An opaque PNG of `w` × `h` with the test pattern.
pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
    let bgra: Vec<u8> = blinzel_core::testimg::rgb(w, h)
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], 255])
        .collect();
    blinzel_core::export::encode_png_bgra(&bgra, w, h).unwrap()
}

// ------------------------------------------------------------------ driver

/// Size of the images [`Harness::with_images`] writes.
pub(crate) const IMAGE_SIZE: (u32, u32) = (64, 48);
/// Client area of the window.
pub(crate) const CLIENT: (u32, u32) = (800, 600);

/// How long [`Harness::pump`] waits for the loader before failing.
const PUMP_LIMIT: Duration = Duration::from_secs(30);

/// Colour transforms are not built: every image shows as sRGB.
fn no_lut(_: Option<&Profile>, _: Option<&Profile>) -> Option<Lut3d> {
    None
}

/// Harness tests share globals (the UI language): one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

/// An [`App`] over the fake backend and a temp folder, at 96 dpi with an
/// 800 × 600 client area. Settings are saved to `<temp>/config`.
pub(crate) struct Harness {
    pub app: App<TestBackend>,
    /// The images' folder (`<temp>/images`).
    pub images: PathBuf,
    wakes: Receiver<Wake>,
    actions: Vec<Action>,
    rendered_at: usize,
    // Dropped after the app (its threads may still read files).
    tmp: TempDir,
    _serial: MutexGuard<'static, ()>,
}

impl Harness {
    /// An empty folder, nothing open, default settings.
    pub fn new() -> Harness {
        Harness::with_settings(Settings::default())
    }

    /// An empty folder, nothing open.
    pub fn with_settings(settings: Settings) -> Harness {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        blinzel_i18n::set_lang(blinzel_i18n::FALLBACK);
        let tmp = TempDir::new();
        let images = tmp.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        let config = tmp.path().join("config");
        crate::settings_store::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = Some(config));
        let (app, wakes) = Harness::start(tmp.path(), settings);
        Harness {
            app,
            images,
            wakes,
            actions: Vec::new(),
            rendered_at: 0,
            tmp,
            _serial: serial,
        }
    }

    /// A new session over the same folders (images, settings file,
    /// thumbnail cache), with the settings as saved; nothing open.
    pub fn restart(&mut self) {
        let settings = self.saved_settings().unwrap_or_default();
        let (app, wakes) = Harness::start(self.tmp.path(), settings);
        self.app = app;
        self.wakes = wakes;
        self.actions.clear();
        self.rendered_at = 0;
    }

    /// A new App over the folders under `tmp`.
    fn start(tmp: &Path, settings: Settings) -> (App<TestBackend>, Receiver<Wake>) {
        let (tx, wakes) = channel();
        let services = Arc::new(FakeServices {
            wakes: Mutex::new(tx),
            cache: tmp.join("cache"),
            reads: Mutex::new(Vec::new()),
            attributes: Mutex::new(HashMap::new()),
            utc_offset: AtomicI32::new(0),
            decode_error: Mutex::new(None),
        });
        let device = Arc::new(FakeDevice::default());
        let loader = Loader::new(services.clone(), 1, 2, 2);
        loader.set_device(device.clone());
        let color = ColorManager::new(settings.color_management, no_lut);
        color.set_target(None);
        loader.set_color(color.clone());
        let app = App::new(
            FakeHost::default(),
            device,
            FakeCanvas::default(),
            services,
            settings,
            loader,
            color,
            96,
            CLIENT,
        );
        app.update_screen_box();
        (app, wakes)
    }

    /// Writes [`IMAGE_SIZE`] PNGs named `names` and opens the first.
    pub fn with_images(names: &[&str]) -> Harness {
        let mut h = Harness::new();
        for n in names {
            h.write_png(n, IMAGE_SIZE.0, IMAGE_SIZE.1);
        }
        if let Some(first) = names.first() {
            h.open(first);
        }
        h
    }

    /// A PNG of `w` × `h` in the images' folder.
    pub fn write_png(&self, name: &str, w: u32, h: u32) -> PathBuf {
        self.write_file(name, &png(w, h))
    }

    /// Any file in the images' folder (e.g. a broken image).
    pub fn write_file(&self, name: &str, data: &[u8]) -> PathBuf {
        let p = self.images.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    /// Opens `name` from the images' folder and pumps until idle.
    pub fn open(&mut self, name: &str) -> Vec<Action> {
        let a = self.app.open(self.images.join(name));
        self.record(a)
    }

    // ------------------------------------------------------------- input

    /// [`App::key`] and pump; `None` when the App does not handle the key.
    pub fn press(&mut self, key: Key, ctrl: bool, shift: bool) -> Option<Vec<Action>> {
        let a = self.app.key(key, ctrl, shift)?;
        Some(self.record(a))
    }

    /// A key without modifiers that the App must handle.
    pub fn key(&mut self, key: Key) -> Vec<Action> {
        self.press(key, false, false)
            .unwrap_or_else(|| panic!("{key:?} not handled"))
    }

    /// A key with Ctrl that the App must handle.
    pub fn ctrl(&mut self, key: Key) -> Vec<Action> {
        self.press(key, true, false)
            .unwrap_or_else(|| panic!("Ctrl+{key:?} not handled"))
    }

    /// [`App::command`] (as from the menu) and pump.
    pub fn command(&mut self, cmd: Cmd) -> Vec<Action> {
        let a = self.app.command(cmd);
        self.record(a)
    }

    pub fn mouse_down(&mut self, p: (f64, f64)) -> Vec<Action> {
        let a = self.app.mouse_down(p, false, false, false);
        self.record(a)
    }

    /// Left button down with Shift held.
    pub fn shift_down(&mut self, p: (f64, f64)) -> Vec<Action> {
        let a = self.app.mouse_down(p, true, false, false);
        self.record(a)
    }

    /// Left click at `p` with Ctrl held.
    pub fn ctrl_click(&mut self, p: (f64, f64)) -> Vec<Action> {
        let a = self.app.mouse_down(p, false, true, false);
        let a = self.record(a);
        self.mouse_up();
        a
    }

    pub fn mouse_move(&mut self, p: (f64, f64)) {
        self.app.mouse_move(p);
        self.pump();
    }

    pub fn mouse_up(&mut self) {
        self.app.mouse_up();
        self.pump();
    }

    /// Left click (down and up) at `p`.
    pub fn click(&mut self, p: (f64, f64)) -> Vec<Action> {
        let a = self.mouse_down(p);
        self.mouse_up();
        a
    }

    /// Left drag from `from` to `to` in a few steps.
    pub fn drag(&mut self, from: (f64, f64), to: (f64, f64)) -> Vec<Action> {
        let a = self.mouse_down(from);
        for k in 1..=4 {
            let t = k as f64 / 4.0;
            self.mouse_move((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t));
        }
        self.mouse_up();
        a
    }

    /// Middle-button drag from `from` to `to` in a few steps.
    pub fn middle_drag(&mut self, from: (f64, f64), to: (f64, f64)) -> Vec<Action> {
        let a = self.app.mouse_down(from, false, false, true);
        let a = self.record(a);
        for k in 1..=4 {
            let t = k as f64 / 4.0;
            self.mouse_move((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t));
        }
        self.mouse_up();
        a
    }

    pub fn double_click(&mut self, p: (f64, f64)) -> Option<Vec<Action>> {
        let a = self.app.double_click(p)?;
        Some(self.record(a))
    }

    pub fn wheel(&mut self, p: (f64, f64), delta: i16) -> Vec<Action> {
        let a = self.app.wheel(p, delta);
        self.record(a)
    }

    /// The window's client area changed.
    pub fn resize(&mut self, client: (u32, u32)) {
        self.app.resized(client);
        self.pump();
    }

    // -------------------------------------------------------------- loop

    /// Delivers wakes and renders when invalidated, until the loader has
    /// nothing queued or running and the folder is listed. Returns the
    /// actions that came up meanwhile (they are also in
    /// [`Harness::actions`]). Thumbnails and the colour-target check may
    /// still arrive later.
    pub fn pump(&mut self) -> Vec<Action> {
        let start = self.actions.len();
        let t = Instant::now();
        loop {
            while let Ok(w) = self.wakes.try_recv() {
                self.deliver(w);
            }
            // Results whose wake was already taken.
            let a = self.app.on_loaded();
            self.actions.extend(a);
            if self.app.host.log.borrow().invalidations != self.rendered_at {
                self.render();
            }
            // A result is sent, then woken, then its job leaves the
            // scheduler; the listing is woken after it is sent. So with no
            // job left and no wake waiting, nothing more can arrive.
            if self.app.loader.pending() == 0 && !self.app.is_listing() {
                match self.wakes.try_recv() {
                    Ok(w) => {
                        self.deliver(w);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            assert!(t.elapsed() < PUMP_LIMIT, "the loader did not get idle");
            if let Ok(w) = self.wakes.recv_timeout(Duration::from_millis(5)) {
                self.deliver(w);
            }
        }
        self.actions[start..].iter().map(Action::clone).collect()
    }

    /// Pumps, then keeps delivering wakes (thumbnails, the colour-target
    /// check) and rendering until `done` holds; fails after a while.
    pub fn wait_for(&mut self, done: impl Fn(&Harness) -> bool) {
        let t = Instant::now();
        self.pump();
        while !done(self) {
            assert!(t.elapsed() < PUMP_LIMIT, "waited in vain");
            if let Ok(w) = self.wakes.recv_timeout(Duration::from_millis(5)) {
                self.deliver(w);
            }
            self.pump();
        }
    }

    fn deliver(&mut self, w: Wake) {
        let a = match w {
            Wake::Loaded => self.app.on_loaded(),
            Wake::List => self.app.on_list(),
            Wake::DirChanged => {
                self.app.on_dir_changed();
                vec![]
            }
            Wake::Thumbs => {
                self.app.on_thumbs();
                vec![]
            }
            Wake::Batch => {
                self.app.on_batch();
                vec![]
            }
            Wake::ColorTarget { monitor, target } => {
                self.app.on_color_target(monitor, target);
                vec![]
            }
        };
        self.actions.extend(a);
    }

    /// Draws a frame ([`Harness::frame`]). Like every shell, an
    /// [`Action::Uncloak`] it returns uncovers the window at once.
    pub fn render(&mut self) -> Vec<Action> {
        self.rendered_at = self.app.host.log.borrow().invalidations;
        let a = self.app.render();
        if a.contains(&Action::Uncloak) {
            self.app.cloaked = false;
        }
        self.actions.extend(a.iter().map(Action::clone));
        a
    }

    /// Records `a` as returned by the App, pumps, and returns `a` plus
    /// whatever the pump brought.
    fn record(&mut self, a: Vec<Action>) -> Vec<Action> {
        let mut out: Vec<Action> = a.iter().map(Action::clone).collect();
        self.actions.extend(a);
        out.extend(self.pump());
        out
    }

    /// Does what the Linux shell does with the window-state actions in
    /// `actions` (full screen, maximise, uncloak, the menu drawn in the
    /// window, a resize to the asked size, recycling = deleting the temp
    /// file); returns the actions it did not handle.
    pub fn shell(&mut self, actions: Vec<Action>) -> Vec<Action> {
        let mut rest = Vec::new();
        for a in actions {
            let more = match a {
                Action::ToggleFullscreen => {
                    self.app.fullscreen = !self.app.fullscreen;
                    self.app.invalidate();
                    vec![]
                }
                Action::ToggleMaximize => {
                    self.app.maximized = !self.app.maximized;
                    self.app.invalidate();
                    vec![]
                }
                Action::Uncloak => {
                    self.app.cloaked = false;
                    vec![]
                }
                Action::ShowMenu { x, y } => {
                    self.app.open_menu((x as f64, y as f64));
                    vec![]
                }
                Action::FitWindow { client, .. } => {
                    self.app.resized(client);
                    self.app.invalidate();
                    vec![]
                }
                Action::Recycle(path) => {
                    std::fs::remove_file(&path).unwrap();
                    self.app.removed(&path)
                }
                a => {
                    rest.push(a);
                    vec![]
                }
            };
            self.actions.extend(more.iter().map(Action::clone));
            rest.extend(self.shell(more));
        }
        self.pump();
        rest
    }

    // ----------------------------------------------------------- observe

    /// Every action the App returned so far, in order.
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// Forgets the recorded actions.
    pub fn clear_actions(&mut self) {
        self.actions.clear();
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.app.current.as_ref().map(|c| c.path.clone())
    }

    pub fn current_name(&self) -> Option<String> {
        self.app.current.as_ref().map(|c| c.entry.name.clone())
    }

    /// The folder's file names in browsing order.
    pub fn names(&self) -> Vec<String> {
        self.app
            .list()
            .entries()
            .iter()
            .map(|e| e.name.clone())
            .collect()
    }

    /// Per-image view state: zoom, centre, crop.
    pub fn view(&self) -> ViewState {
        self.app.view
    }

    pub fn mode(&self) -> ViewMode {
        self.app.settings.view.mode
    }

    /// Layout of the image on screen (scale = client px per image px).
    pub fn layout(&self) -> Option<Layout> {
        self.app.current_layout()
    }

    pub fn menu_state(&self) -> MenuState {
        self.app.menu_state()
    }

    /// Whether the window menu offers `cmd` enabled (it must be in it).
    pub fn menu_enabled(&self, cmd: Cmd) -> bool {
        self.menu_entry(cmd).1
    }

    /// Whether the window menu shows `cmd` with a check mark.
    pub fn menu_checked(&self, cmd: Cmd) -> bool {
        self.menu_entry(cmd).0
    }

    /// `(checked, enabled)` of `cmd` in the window menu.
    fn menu_entry(&self, cmd: Cmd) -> (bool, bool) {
        fn find(items: &[MenuItem], cmd: Cmd) -> Option<(bool, bool)> {
            items.iter().find_map(|i| match i {
                MenuItem::Item {
                    cmd: c,
                    checked,
                    enabled,
                    ..
                } if *c == cmd => Some((*checked, *enabled)),
                MenuItem::Sub(_, sub) => find(sub, cmd),
                _ => None,
            })
        }
        find(&crate::menu::items(&self.menu_state()), cmd)
            .unwrap_or_else(|| panic!("{cmd:?} not in the menu"))
    }

    /// The client point where image point `p` (oriented full-resolution
    /// pixels) is shown.
    pub fn at(&self, p: (f64, f64)) -> (f64, f64) {
        self.layout().unwrap().image_to_client().apply(p)
    }

    /// The selected tile while the grid is open.
    pub fn grid_selected(&self) -> Option<usize> {
        self.app.browse.grid.as_ref().map(|g| g.selected)
    }

    /// Files the thumbnail pool read, in order.
    pub fn thumb_reads(&self) -> Vec<PathBuf> {
        let reads = self.app.services.reads.lock().unwrap();
        reads
            .iter()
            .filter(|(t, _)| t.starts_with("blinzel-thumb"))
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// The listing reports `name` with `attributes` (e.g. a cloud
    /// placeholder) from now on.
    pub fn set_attributes(&self, name: &str, attributes: u32) {
        let mut a = self.app.services.attributes.lock().unwrap();
        a.insert(name.to_string(), attributes);
    }

    /// Every decode fails with `e` from now on (`None`: decode again).
    pub fn set_decode_error(&self, e: Option<DecodeError>) {
        *self.app.services.decode_error.lock().unwrap() = e;
    }

    /// The platform's clock is `secs` ahead of UTC from now on.
    pub fn set_utc_offset(&self, secs: i32) {
        self.app.services.utc_offset.store(secs, Ordering::Relaxed);
    }

    /// The marked tiles' names while the grid is open.
    pub fn grid_marked(&self) -> Vec<String> {
        let g = self.app.browse.grid.as_ref();
        g.map(|g| g.marked.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The files in the open batch dialog's list (`None`: it is closed).
    pub fn batch_files(&self) -> Option<Vec<PathBuf>> {
        self.app.dialog.as_ref().map(|d| d.file_paths())
    }

    /// Client rectangle of grid tile `i` while the grid is open.
    pub fn grid_cell(&self, i: usize) -> Option<Rect> {
        let g = self.app.browse.grid.as_ref()?;
        let (w, h) = self.app.client;
        let layout = grid_layout(
            self.app.list().len(),
            (w as f64, h as f64),
            self.app.scale(),
        );
        Some(layout.cell_rect(i, g.scroll))
    }

    /// What the UI asked of its window.
    pub fn host(&self) -> Ref<'_, HostLog> {
        self.app.host.log.borrow()
    }

    /// The last frame drawn.
    pub fn frame(&self) -> &Frame {
        &self.app.canvas.frame
    }

    /// Where the App saves its settings (`<temp>/config/settings.toml`).
    pub fn settings_file(&self) -> PathBuf {
        self.tmp.path().join("config").join("settings.toml")
    }

    /// The settings file's text as saved (`None`: never saved).
    pub fn saved_text(&self) -> Option<String> {
        std::fs::read_to_string(self.settings_file()).ok()
    }

    /// The settings file as saved (`None`: never saved).
    pub fn saved_settings(&self) -> Option<Settings> {
        self.saved_text().map(|t| Settings::parse(&t))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        crate::settings_store::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Key::*;

    fn centre(r: Rect) -> (f64, f64) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn the_fakes_record_what_the_app_asked_for() {
        let mut h = Harness::with_images(&["a.png", "b.png"]);
        assert_eq!(h.host().watched, [h.images.clone()]);
        assert!(h.host().first_frame_shown);
        assert!(h.host().timers.is_empty(), "a still image sets no timer");
        assert!(h.app.canvas.presented >= 1);
        assert!(h.app.device.uploads.lock().unwrap().contains(&IMAGE_SIZE));
        assert_eq!(h.frame().images[0].size, IMAGE_SIZE);
        h.ctrl(Letter('C'));
        assert_eq!(h.host().clipboard_offers, 1);
        assert_eq!(h.app.clip.as_ref().unwrap().path, h.images.join("a.png"));
        h.clear_actions();
        h.command(Cmd::Next);
        assert_eq!(h.current_name().as_deref(), Some("b.png"));
        assert!(!h.actions().contains(&Action::Uncloak), "cleared");
    }

    #[test]
    fn mouse_drag_selects_and_escape_clears_it() {
        let mut h = Harness::with_images(&["a.png"]);
        let shapes = h.frame().shapes.len();
        // The 64 × 48 image sits at 100 % in the middle of 800 × 600.
        h.drag((380.0, 285.0), (420.0, 315.0));
        let sel = h.app.selection().unwrap();
        assert!(sel.w > 30.0 && sel.h > 20.0, "{sel:?}");
        assert!(h.frame().shapes.len() > shapes, "selection drawn");
        assert_eq!(h.key(Escape), []);
        assert!(h.app.selection().is_none());
        // A plain click on the image keeps it clear.
        h.click((400.0, 300.0));
        assert!(h.app.selection().is_none());
    }

    #[test]
    fn wheel_keys_and_resize_change_the_layout() {
        let mut h = Harness::with_images(&["a.png"]);
        assert_eq!(h.layout().unwrap().scale, 1.0);
        h.wheel((400.0, 300.0), 120);
        assert!((h.layout().unwrap().scale - 1.2).abs() < 1e-9);
        assert!(h.view().zoom.is_some());
        h.key(Plus);
        assert_eq!(h.layout().unwrap().scale, 1.5);
        h.key(Digit(0));
        assert_eq!(h.view(), ViewState::default());
        h.resize((400, 300));
        assert_eq!(h.layout().unwrap().client, (400.0, 300.0));
    }

    #[test]
    fn shell_feedback_toggles_full_screen_and_rotation_resets_on_navigation() {
        let mut h = Harness::with_images(&["a.png", "b.png"]);
        let a = h.key(Enter);
        assert_eq!(a, [Action::ToggleFullscreen]);
        assert_eq!(h.shell(a), []);
        assert!(h.app.fullscreen && h.menu_state().fullscreen);
        h.key(Letter('R'));
        assert_eq!(h.app.user_rotation(), Orientation::NORMAL.rotated_cw());
        h.key(Right);
        assert_eq!(h.app.user_rotation(), Orientation::NORMAL);
        let a = h.key(Escape);
        assert_eq!(a, [Action::ToggleFullscreen]);
        h.shell(a);
        assert!(!h.app.fullscreen);
    }

    #[test]
    fn grid_tiles_are_clicked_and_opened() {
        let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
        h.key(Letter('G'));
        assert_eq!(h.grid_selected(), Some(0));
        assert!(h.menu_state().grid);
        h.wait_for(|h| h.frame().thumbs.len() == 3);
        let c = centre(h.grid_cell(2).unwrap());
        assert_eq!(h.click(c), []);
        assert_eq!(h.grid_selected(), Some(2));
        h.double_click(c).unwrap();
        assert_eq!(h.grid_selected(), None);
        assert_eq!(h.current_name().as_deref(), Some("c.png"));
    }

    #[test]
    fn a_broken_file_shows_its_error() {
        let mut h = Harness::new();
        h.write_file("bad.png", b"not an image");
        h.open("bad.png");
        let id = h.app.current.as_ref().unwrap().cache_id;
        assert!(h.app.error_of(id).is_some());
        assert!(h.frame().images.is_empty());
        assert!(
            h.frame().texts.iter().any(|t| t.contains("bad.png")),
            "{:?}",
            h.frame().texts
        );
    }
}
