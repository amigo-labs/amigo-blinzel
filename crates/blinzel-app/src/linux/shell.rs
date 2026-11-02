//! The window and its event loop: winit events become `App` calls, the
//! app's actions become window operations, dialogs and desktop services.
//!
//! The window is frameless like on Windows: the app draws the menu and
//! caption buttons, the free top strip drags the window, the edges resize
//! it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::decode::{self, DecodeError, Probe, Request};
use blinzel_core::filelist::Entry;
use blinzel_core::icc::Profile;
use blinzel_core::pixel::Image;
use blinzel_core::settings::Settings;
use blinzel_core::timing;
use blinzel_i18n::{Lang, Msg, tr, trf};
use blinzel_ui::app::{Action, App, SaveJob};
use blinzel_ui::backend::{Backend, CursorKind, Host, Key, Services, Timer, Wake};
use blinzel_ui::chrome::Button;
use blinzel_ui::color::ColorManager;
use blinzel_ui::loader::{Job, Loader, Stage};
use blinzel_ui::menu::Cmd;
use blinzel_ui::{dirs, files, settings_store};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{CursorIcon, Fullscreen, ResizeDirection, Window, WindowId};

use super::clipboard;
use super::gpu::{Gpu, Target, WgpuCanvas, WgpuDevice, instance};
use super::install::APP_ID;

/// Events from other threads to the event loop.
#[derive(Debug)]
pub enum UserEvent {
    Wake(Wake),
    /// Another instance handed over a file (`None`: show the window).
    Open(Option<PathBuf>),
    /// A message for the notice toast.
    Notice(String),
    /// Pixels for the clipboard (RGBA), decoded after a copy.
    Clipboard(Result<(Vec<u8>, u32, u32), String>),
}

/// The Linux shell's backend types.
pub struct Lx;

impl Backend for Lx {
    type Device = WgpuDevice;
    type Canvas = WgpuCanvas;
    type Host = LinuxHost;
    type Services = LinuxServices;
}

// ---------------------------------------------------------------- services

pub struct LinuxServices {
    proxy: Mutex<EventLoopProxy<UserEvent>>,
}

impl LinuxServices {
    fn send(&self, e: UserEvent) {
        let _ = self.proxy.lock().unwrap().send_event(e);
    }
}

/// Debug output when `BLINZEL_DEBUG` is set.
pub(super) fn log(msg: &str) {
    if std::env::var_os("BLINZEL_DEBUG").is_some() {
        eprintln!("blinzel: {msg}");
    }
}

/// The desktop's language preference (`LANGUAGE`, `LC_ALL`,
/// `LC_MESSAGES`, `LANG`).
pub fn system_language() -> Lang {
    language_from_env(|v| std::env::var(v).ok())
}

/// The language the variables `var` reads prefer, in the order
/// `LANGUAGE` (colon-separated), `LC_ALL`, `LC_MESSAGES`, `LANG`; `C`,
/// `POSIX` and `.encoding` / `@modifier` suffixes are ignored.
fn language_from_env(var: impl Fn(&str) -> Option<String>) -> Lang {
    let vars: Vec<String> = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| var(v))
        .flat_map(|v| {
            v.split(':')
                .map(|tag| tag.split(['.', '@']).next().unwrap_or("").replace('_', "-"))
                .collect::<Vec<_>>()
        })
        .filter(|t| !t.is_empty() && t != "C" && t != "POSIX")
        .collect();
    blinzel_i18n::negotiate(vars.iter().map(String::as_str))
}

/// Probes a file. Linux has no system decoder: the native probe is the
/// whole answer (`docs/specs/image-formats.md` AC-24).
fn probe(data: &[u8]) -> Result<Probe, DecodeError> {
    decode::probe(data)
}

/// Decodes a file with the native decoders only; what they do not handle
/// (a raw's sensor data, ICO, PSD, …) stays an error, and a raw shows its
/// embedded previews instead.
fn decode_with(data: &[u8], req: Request, threads: usize) -> Result<Image, DecodeError> {
    decode::decode_with(data, req, threads)
}

impl Services for LinuxServices {
    fn wake(&self, w: Wake) {
        self.send(UserEvent::Wake(w));
    }

    fn read_file(&self, path: &Path) -> std::io::Result<PoolBuf> {
        files::read_file(path)
    }

    fn list_dir(&self, dir: &Path) -> std::io::Result<Vec<Entry>> {
        files::list_dir(dir)
    }

    fn entry(&self, path: &Path) -> std::io::Result<Entry> {
        files::entry(path)
    }

    fn probe(&self, data: &[u8]) -> Result<Probe, DecodeError> {
        probe(data)
    }

    fn decode(&self, data: &[u8], req: Request, threads: usize) -> Result<Image, DecodeError> {
        decode_with(data, req, threads)
    }

    fn log(&self, msg: &str) {
        log(msg);
    }

    fn display_target(&self, monitor: isize) -> Option<Profile> {
        super::color::display_target(monitor)
    }

    fn thumb_threads(&self) -> usize {
        std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(2))
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        dirs::cache()
    }

    fn system_language(&self) -> Lang {
        system_language()
    }
}

// -------------------------------------------------------------------- host

#[derive(Default)]
struct HostState {
    timers: HashMap<Timer, Instant>,
    /// A copy asked for the clipboard; the shell produces the pixels.
    copy: bool,
}

pub struct LinuxHost {
    window: Arc<Window>,
    state: Rc<RefCell<HostState>>,
    proxy: EventLoopProxy<UserEvent>,
}

impl Host for LinuxHost {
    fn invalidate(&self) {
        self.window.request_redraw();
    }

    fn first_frame_shown(&self) {
        super::update::on_first_frame();
    }

    fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    fn set_timer(&self, timer: Timer, ms: u32) {
        let due = Instant::now() + Duration::from_millis(ms as u64);
        self.state.borrow_mut().timers.insert(timer, due);
    }

    fn kill_timer(&self, timer: Timer) {
        self.state.borrow_mut().timers.remove(&timer);
    }

    fn to_screen(&self, p: (f64, f64)) -> (i32, i32) {
        // Menus are drawn inside the window: client coordinates.
        (p.0 as i32, p.1 as i32)
    }

    fn monitor(&self) -> Option<(isize, (u32, u32))> {
        let m = self.window.current_monitor()?;
        let id = m.name().map_or(0, |n| super::color::monitor_id(&n));
        let s = m.size();
        Some((id, (s.width, s.height)))
    }

    fn offer_clipboard(&self) -> bool {
        self.state.borrow_mut().copy = true;
        true
    }

    fn watch_dir(&self, dir: &Path) -> Option<Box<dyn std::any::Any>> {
        let proxy = Mutex::new(self.proxy.clone());
        let w = crate::watch::folder(dir, move || {
            let _ = proxy
                .lock()
                .unwrap()
                .send_event(UserEvent::Wake(Wake::DirChanged));
        })?;
        Some(Box::new(w))
    }
}

// ------------------------------------------------------------------- shell

#[derive(Default)]
pub struct Args {
    pub file: Option<PathBuf>,
    pub json: Option<PathBuf>,
    pub exit_after_first_frame: bool,
    pub install: bool,
    pub uninstall: bool,
}

pub fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--json") => a.json = it.next().map(PathBuf::from),
            Some("--exit-after-first-frame") => a.exit_after_first_frame = true,
            Some("--install") => a.install = true,
            Some("--uninstall") => a.uninstall = true,
            _ => a.file = Some(PathBuf::from(arg)),
        }
    }
    a
}

/// Edge width (logical pixels) that resizes the frameless window.
const EDGE: f64 = 6.0;
/// The notice toast stays this long.
const NOTICE: Duration = Duration::from_millis(3500);
/// Folder changes are collected this long before relisting.
const DIR_SETTLE: Duration = Duration::from_millis(150);

struct Shell {
    args: Args,
    proxy: EventLoopProxy<UserEvent>,
    services: Arc<LinuxServices>,
    settings: Option<Settings>,
    loader: Option<Loader<WgpuDevice, LinuxServices>>,
    color: Arc<ColorManager>,
    app: Option<App<Lx>>,
    window: Option<Arc<Window>>,
    host: Rc<RefCell<HostState>>,
    cursor: (f64, f64),
    modifiers: ModifiersState,
    last_click: Option<(Instant, (f64, f64))>,
    /// A caption button pressed and not yet released.
    chrome_pressed: Option<Button>,
    clipboard: Option<arboard::Clipboard>,
    notice_until: Option<Instant>,
    dir_due: Option<Instant>,
    /// Fractional wheel steps of touchpads (in 1/120).
    wheel_rest: f64,
    /// The files of a drop still being reported.
    dropped: Dropped,
    exiting: bool,
}

/// The files of one drop. winit reports a drop of several files as one
/// `DroppedFile` event per file, in a row; they are collected until another
/// event or the end of the event batch, so that the App gets the whole
/// drop at once (and opens its first file when no dialog is open).
#[derive(Default)]
struct Dropped(Vec<PathBuf>);

impl Dropped {
    /// Keeps `event` when it reports a dropped file. Otherwise returns the
    /// drop collected so far, which comes before `event`.
    fn feed(&mut self, event: WindowEvent) -> (Option<Vec<PathBuf>>, Option<WindowEvent>) {
        match event {
            WindowEvent::DroppedFile(path) => {
                self.0.push(path);
                (None, None)
            }
            event => (self.take(), Some(event)),
        }
    }

    /// The drop collected so far, if any.
    fn take(&mut self) -> Option<Vec<PathBuf>> {
        (!self.0.is_empty()).then(|| std::mem::take(&mut self.0))
    }
}

impl Shell {
    fn scale(&self) -> f64 {
        self.window.as_ref().map_or(1.0, |w| w.scale_factor())
    }

    fn create(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let settings = self.settings.take().unwrap_or_default();
        let monitor = el.primary_monitor().map(|m| m.size());
        let (w, h) = monitor.map_or((1280, 800), |s| (s.width * 7 / 10, s.height * 7 / 10));
        // The app id (Wayland) / WM_CLASS (X11, the same field) ties the
        // window to the desktop entry `--install` writes.
        let attrs = Window::default_attributes()
            .with_title(tr(Msg::AppName))
            .with_decorations(false)
            .with_visible(self.args.file.is_none())
            .with_inner_size(PhysicalSize::new(w.max(320), h.max(240)))
            .with_min_inner_size(PhysicalSize::new(200, 150));
        let attrs = WindowAttributesExtWayland::with_name(attrs, APP_ID, APP_ID);
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        timing::mark("window_created");
        let instance = instance();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let gpu = Arc::new(Gpu::new(&instance, Some(&surface), None)?);
        timing::mark("device_ready");
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: gpu.format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        let client = (size.width.max(1), size.height.max(1));
        let canvas = WgpuCanvas::new(gpu.clone(), Target::Surface { surface, config }, client);
        let device = Arc::new(WgpuDevice::new(gpu));
        let loader = self.loader.take().ok_or("no loader")?;
        loader.set_device(device.clone());
        let host = LinuxHost {
            window: window.clone(),
            state: self.host.clone(),
            proxy: self.proxy.clone(),
        };
        let dpi = (window.scale_factor() * 96.0).round() as u32;
        let mut app = App::new(
            host,
            device,
            canvas,
            self.services.clone(),
            settings,
            loader,
            self.color.clone(),
            dpi,
            client,
        );
        app.cloaked = self.args.file.is_some();
        app.exit_after_first_frame = report_path(&self.args);
        let actions = match self.args.file.clone() {
            Some(f) => app.open(f),
            None => Vec::new(),
        };
        app.update_screen_box();
        app.check_monitor(true);
        self.window = Some(window);
        self.app = Some(app);
        self.run(el, actions);
        Ok(())
    }

    /// Runs `actions` and whatever they lead to.
    fn run(&mut self, el: &ActiveEventLoop, mut actions: Vec<Action>) {
        while !actions.is_empty() {
            let next: Vec<Action> = actions.drain(..).flat_map(|a| self.action(el, a)).collect();
            actions = next;
        }
        self.after_event();
    }

    fn action(&mut self, el: &ActiveEventLoop, a: Action) -> Vec<Action> {
        let (Some(app), Some(window)) = (self.app.as_mut(), self.window.clone()) else {
            return vec![];
        };
        if ends_process(&a) {
            self.exiting = true;
            el.exit();
            return vec![];
        }
        match a {
            Action::ShowMenu { x, y } => {
                app.open_menu((x as f64, y as f64));
                vec![]
            }
            Action::FitWindow { client, grow_only } => {
                if app.fullscreen || window.is_maximized() {
                    return vec![];
                }
                let client = if grow_only {
                    // winit knows no work area; the monitor size is the
                    // limit and the window manager keeps panels free.
                    let Some(monitor) = window.current_monitor() else {
                        return vec![];
                    };
                    let k = window.scale_factor();
                    let max = (monitor.size().width, monitor.size().height);
                    let min = ((320.0 * k) as u32, (220.0 * k) as u32);
                    match blinzel_core::layout::grown_client_size(client, app.client, max, min) {
                        Some(size) => size,
                        None => return vec![],
                    }
                } else {
                    client
                };
                let _ = window.request_inner_size(PhysicalSize::new(client.0, client.1));
                vec![]
            }
            Action::ToggleFullscreen => {
                app.fullscreen = !app.fullscreen;
                window.set_fullscreen(app.fullscreen.then_some(Fullscreen::Borderless(None)));
                app.invalidate();
                vec![]
            }
            Action::ToggleMaximize => {
                window.set_maximized(!window.is_maximized());
                vec![]
            }
            Action::Minimize => {
                window.set_minimized(true);
                vec![]
            }
            Action::OpenDialog => {
                let dir = app
                    .current
                    .as_ref()
                    .and_then(|c| c.path.parent().map(Path::to_path_buf));
                let mut d = rfd::FileDialog::new()
                    .set_title(tr(Msg::MenuOpen))
                    .add_filter(
                        tr(Msg::OpenFilterImages),
                        blinzel_core::format::IMAGE_EXTENSIONS,
                    );
                if let Some(dir) = dir {
                    d = d.set_directory(dir);
                }
                match d.pick_file() {
                    Some(p) => {
                        window.set_visible(true);
                        app.open(p)
                    }
                    None => vec![],
                }
            }
            Action::Recycle(path) => recycle(&path, |p| app.removed(p)),
            Action::SaveCutout(job) => {
                self.save(job);
                vec![]
            }
            Action::CheckUpdates => {
                let proxy = self.proxy.clone();
                super::update::check_now(move |text| {
                    let _ = proxy.send_event(UserEvent::Notice(text));
                });
                vec![]
            }
            Action::About => {
                let text = trf(
                    Msg::AboutBody,
                    &[("version", &env!("CARGO_PKG_VERSION")), ("url", &URL)],
                );
                app.show_notice(text);
                self.notice_until = Some(Instant::now() + NOTICE * 2);
                vec![]
            }
            Action::Notices => {
                if let Err(e) = show_notices() {
                    log(&format!("notices: {e}"));
                }
                vec![]
            }
            Action::PickFolder => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    app.batch_folder_picked(dir);
                }
                vec![]
            }
            Action::OpenFolder(dir) => {
                open_url(&dir.to_string_lossy());
                vec![]
            }
            Action::SetResident(_) | Action::TrayMenu { .. } => vec![],
            Action::Wake { .. } | Action::Activate => {
                window.set_visible(true);
                window.focus_window();
                vec![]
            }
            Action::Uncloak => {
                app.cloaked = false;
                window.set_visible(true);
                vec![]
            }
            // Ended above.
            Action::Close | Action::Exit | Action::Quit => vec![],
            Action::OpenWith(file) => {
                open_with(&file);
                vec![]
            }
            Action::EditWith { editor, file } => {
                if let Err(e) = std::process::Command::new(&editor).arg(&file).spawn() {
                    app.show_notice(trf(Msg::EditorFailed, &[("reason", &e.to_string())]));
                }
                vec![]
            }
            Action::ChooseEditor { then } => {
                match rfd::FileDialog::new()
                    .set_title(tr(Msg::MenuChooseEditor))
                    .pick_file()
                {
                    Some(editor) => app.editor_chosen(editor, then),
                    None => vec![],
                }
            }
            // Screenshots are not on Linux yet (the menu does not offer them).
            Action::Capture { .. }
            | Action::SetScreenshotHotkey(_)
            | Action::PickScreenshotFolder => vec![],
        }
    }

    fn save(&mut self, job: SaveJob) {
        let folder = job
            .source
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let Some(target) = rfd::FileDialog::new()
            .set_directory(folder)
            .set_file_name(&job.default_name)
            .add_filter("JPEG", &["jpg", "jpeg"])
            .add_filter("PNG", &["png"])
            .add_filter("WebP", &["webp"])
            .add_filter("AVIF", &["avif"])
            .save_file()
        else {
            return;
        };
        let services = self.services.clone();
        let _ =
            blinzel_ui::app::save_in_background(self.services.clone(), job, target, move |text| {
                services.send(UserEvent::Notice(text))
            });
    }

    /// Work that waits for the end of an event: a copy's pixels.
    /// Every file of one drop, to the App.
    fn files_dropped(&mut self, el: &ActiveEventLoop, paths: Vec<PathBuf>) {
        let actions = self.app.as_mut().unwrap().files_dropped(paths);
        self.run(el, actions);
    }

    fn after_event(&mut self) {
        let copy = std::mem::take(&mut self.host.borrow_mut().copy);
        if copy && let Some(src) = self.app.as_ref().and_then(|a| a.clip.clone()) {
            let services = self.services.clone();
            let _ = blinzel_ui::app::copy_in_background(self.services.clone(), src, move |r| {
                let r = r.map(|(px, w, h)| (clipboard::rgba(&px), w, h));
                services.send(UserEvent::Clipboard(r));
            });
        }
    }

    /// The resize direction for a client point near the window's edges.
    fn edge(&self, p: (f64, f64)) -> Option<ResizeDirection> {
        let (app, window) = (self.app.as_ref()?, self.window.as_ref()?);
        let plain = !app.fullscreen && !window.is_maximized();
        resize_edge(p, app.client, EDGE * self.scale(), plain)
    }

    fn update_cursor(&self) {
        let (Some(app), Some(window)) = (self.app.as_ref(), self.window.as_ref()) else {
            return;
        };
        let p = self.cursor;
        let icon = match self.edge(p) {
            Some(dir) => resize_cursor(dir),
            None if app.popup.is_some() => CursorIcon::Default,
            None => match app.cursor_at(p, self.modifiers.shift_key()) {
                CursorKind::Arrow => CursorIcon::Default,
                CursorKind::Cross => CursorIcon::Crosshair,
                CursorKind::Hand => CursorIcon::Grab,
                CursorKind::Move => CursorIcon::Move,
                CursorKind::SizeNS => CursorIcon::NsResize,
                CursorKind::SizeWE => CursorIcon::EwResize,
                CursorKind::SizeNWSE => CursorIcon::NwseResize,
                CursorKind::SizeNESW => CursorIcon::NeswResize,
            },
        };
        window.set_cursor(icon);
    }

    fn left_down(&mut self, el: &ActiveEventLoop) {
        let p = self.cursor;
        let shift = self.modifiers.shift_key();
        let double = self.last_click.is_some_and(|(t, q)| {
            t.elapsed() < Duration::from_millis(400)
                && (q.0 - p.0).abs() < 4.0
                && (q.1 - p.1).abs() < 4.0
        });
        self.last_click = (!double).then(|| (Instant::now(), p));
        let edge = self.edge(p);
        let (Some(app), Some(window)) = (self.app.as_mut(), self.window.clone()) else {
            return;
        };
        let what = if app.popup.is_some() {
            Press::App
        } else {
            let chrome = app.chrome();
            let button = (!app.fullscreen).then(|| chrome.hit(p)).flatten();
            let caption = !app.fullscreen && p.1 < chrome.caption_height() && !app.is_dragging();
            press(edge, button, double, caption)
        };
        match what {
            Press::Resize(dir) => {
                let _ = window.drag_resize_window(dir);
            }
            Press::Button(b) => {
                app.pressed = Some(b);
                self.chrome_pressed = Some(b);
                app.invalidate();
            }
            Press::DoubleClick => {
                let actions = double_clicked(app.double_click(p));
                self.run(el, actions);
            }
            Press::MoveWindow => {
                let _ = window.drag_window();
            }
            Press::App => {
                let ctrl = self.modifiers.control_key();
                let actions = app.mouse_down(p, shift, ctrl, false);
                self.run(el, actions);
            }
        }
    }

    fn left_up(&mut self, el: &ActiveEventLoop) {
        let p = self.cursor;
        let Some(app) = self.app.as_mut() else { return };
        if let Some(b) = self.chrome_pressed.take() {
            app.pressed = None;
            app.invalidate();
            if app.chrome().hit(p) == Some(b) {
                let a = match b {
                    Button::Minimize => Action::Minimize,
                    Button::Maximize => Action::ToggleMaximize,
                    _ => Action::Close,
                };
                self.run(el, vec![a]);
            }
            return;
        }
        app.mouse_up();
    }

    fn key(&mut self, el: &ActiveEventLoop, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let (ctrl, shift) = (self.modifiers.control_key(), self.modifiers.shift_key());
        let Some(app) = self.app.as_mut() else { return };
        let actions = key_of(&event.key_without_modifiers()).and_then(|k| app.key(k, ctrl, shift));
        // Characters for the batch dialog's text fields.
        if app.dialog.is_some()
            && !ctrl
            && let Some(text) = &event.text
        {
            app.text_input(text);
        }
        if let Some(actions) = actions {
            self.run(el, actions);
        }
    }

    /// Timers that are due, and when the next one is.
    fn timers(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let anim_due = {
            let mut s = self.host.borrow_mut();
            let due = s.timers.get(&Timer::Anim).is_some_and(|&t| t <= now);
            if due {
                s.timers.remove(&Timer::Anim);
            }
            due
        };
        if let Some(app) = self.app.as_mut() {
            if anim_due {
                app.on_anim_timer();
            }
            if self.notice_until.is_some_and(|t| t <= now) {
                self.notice_until = None;
                app.notice = None;
                app.invalidate();
            }
            if self.dir_due.is_some_and(|t| t <= now) {
                self.dir_due = None;
                app.on_dir_changed();
            }
        }
        let next = self
            .host
            .borrow()
            .timers
            .values()
            .copied()
            .chain(self.notice_until)
            .chain(self.dir_due)
            .min();
        el.set_control_flow(match next {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

const URL: &str = "https://github.com/amigo-labs/amigo-blinzel";

/// The resize direction for client point `p` within `edge` physical pixels
/// of the edges of a `client`-sized window (diagonal in the corners); none
/// while the window is maximized or full screen (`plain` false).
fn resize_edge(
    p: (f64, f64),
    client: (u32, u32),
    edge: f64,
    plain: bool,
) -> Option<ResizeDirection> {
    if !plain {
        return None;
    }
    let (w, h) = (client.0 as f64, client.1 as f64);
    let (l, r, t, b) = (p.0 < edge, p.0 >= w - edge, p.1 < edge, p.1 >= h - edge);
    Some(match (l, r, t, b) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (_, true, true, _) => ResizeDirection::NorthEast,
        (true, _, _, true) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::West,
        (_, true, ..) => ResizeDirection::East,
        (_, _, true, _) => ResizeDirection::North,
        (_, _, _, true) => ResizeDirection::South,
        _ => return None,
    })
}

/// The pointer over a resize edge.
fn resize_cursor(dir: ResizeDirection) -> CursorIcon {
    match dir {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
    }
}

/// What a left press does before the App sees it.
#[derive(Debug, PartialEq)]
enum Press {
    /// Resize the window from this edge.
    Resize(ResizeDirection),
    /// A caption button, acting on release.
    Button(Button),
    /// The second press of a double click.
    DoubleClick,
    /// Move the window.
    MoveWindow,
    /// The App's own press (image, selection, ☰).
    App,
}

/// A left press at a resize `edge`, on a caption `button`, as the second of
/// a `double` click, or on the free `caption` strip (not full screen, no
/// selection drag running).
fn press(
    edge: Option<ResizeDirection>,
    button: Option<Button>,
    double: bool,
    caption: bool,
) -> Press {
    match (edge, button) {
        (Some(dir), _) => Press::Resize(dir),
        (_, Some(b @ (Button::Minimize | Button::Maximize | Button::Close))) => Press::Button(b),
        _ if double => Press::DoubleClick,
        (_, None) if caption => Press::MoveWindow,
        _ => Press::App,
    }
}

/// A double click: the App's `answer`, or on the free caption strip (where
/// the App answers none) maximize or restore.
fn double_clicked(answer: Option<Vec<Action>>) -> Vec<Action> {
    answer.unwrap_or_else(|| vec![Action::ToggleMaximize])
}

/// Moves `path` to the desktop's trash; only then does `removed` take it
/// out of the file list. A refused move leaves the file and the list alone.
fn recycle(path: &Path, removed: impl FnOnce(&Path) -> Vec<Action>) -> Vec<Action> {
    match trash::delete(path) {
        Ok(()) => removed(path),
        Err(e) => {
            log(&format!("trash: {e}"));
            vec![]
        }
    }
}

fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// The desktop's "Open with" choice through the OpenURI portal (`ask`);
/// without a portal, the default application.
fn open_with(file: &Path) {
    let uri = format!(
        "file://{}",
        file.to_string_lossy()
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    );
    let asked = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.OpenURI.OpenURI",
            "",
            &uri,
            "{'ask': <true>}",
        ])
        .status()
        .is_ok_and(|s| s.success());
    if !asked {
        open_url(&file.to_string_lossy());
    }
}

/// Writes the licence texts next to the cache and opens them.
fn show_notices() -> Result<(), String> {
    let dir = dirs::cache().ok_or("no cache folder")?;
    let path = dir.join("THIRD-PARTY-NOTICES.txt");
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, crate::notices_text::TEXT))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    open_url(&path.to_string_lossy());
    Ok(())
}

fn key_of(k: &WKey) -> Option<Key> {
    Some(match k {
        WKey::Named(n) => match n {
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::Space => Key::Space,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Tab => Key::Tab,
            NamedKey::Delete => Key::Delete,
            NamedKey::F10 => Key::F10,
            NamedKey::F11 => Key::F11,
            NamedKey::ContextMenu => Key::Menu,
            _ => return None,
        },
        WKey::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            match c {
                '+' | '=' => Key::Plus,
                '-' => Key::Minus,
                '0'..='9' => Key::Digit(c as u8 - b'0'),
                c if c.is_ascii_alphabetic() => Key::Letter(c.to_ascii_uppercase()),
                _ => return None,
            }
        }
        _ => return None,
    })
}

impl ApplicationHandler<UserEvent> for Shell {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.app.is_none()
            && let Err(e) = self.create(el)
        {
            eprintln!("amigo-blinzel: {e}");
            el.exit();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if self.app.is_none() || self.exiting {
            return;
        }
        let (dropped, event) = self.dropped.feed(event);
        if let Some(paths) = dropped {
            self.files_dropped(el, paths);
        }
        let Some(event) = event else { return };
        match event {
            WindowEvent::CloseRequested => self.run(el, vec![Action::Close]),
            WindowEvent::Resized(size) => {
                let app = self.app.as_mut().unwrap();
                app.maximized = self.window.as_ref().is_some_and(|w| w.is_maximized());
                app.resized((size.width, size.height));
                app.update_screen_box();
                app.invalidate();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let app = self.app.as_mut().unwrap();
                app.dpi = (scale_factor * 96.0).round() as u32;
                app.invalidate();
            }
            WindowEvent::Moved(_) => {
                let app = self.app.as_mut().unwrap();
                app.check_monitor(false);
                app.update_screen_box();
            }
            WindowEvent::RedrawRequested => {
                let actions = self.app.as_mut().unwrap().render();
                self.run(el, actions);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                let p = self.cursor;
                let app = self.app.as_mut().unwrap();
                // Caption buttons light up on hover (the menu button is the
                // app's own business).
                let over = (!app.fullscreen && app.popup.is_none())
                    .then(|| app.chrome().hit(p))
                    .flatten()
                    .filter(|b| *b != Button::Menu);
                if over != app.hover
                    && (over.is_some() || app.hover.is_some_and(|b| b != Button::Menu))
                {
                    app.hover = over;
                    app.invalidate();
                }
                app.mouse_move(p);
                self.update_cursor();
            }
            WindowEvent::CursorLeft { .. } => {
                let app = self.app.as_mut().unwrap();
                if app.hover.take().is_some() {
                    app.invalidate();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                let shift = self.modifiers.shift_key();
                match (button, pressed) {
                    (MouseButton::Left, true) => self.left_down(el),
                    (MouseButton::Left, false) => self.left_up(el),
                    (MouseButton::Middle, true) => {
                        let p = self.cursor;
                        let ctrl = self.modifiers.control_key();
                        let actions = self.app.as_mut().unwrap().mouse_down(p, shift, ctrl, true);
                        self.run(el, actions);
                    }
                    (MouseButton::Middle, false) => self.app.as_mut().unwrap().mouse_up(),
                    (MouseButton::Right, false) => {
                        let actions = self.app.as_mut().unwrap().right_click(self.cursor);
                        self.run(el, actions);
                    }
                    (MouseButton::Back, true) => {
                        let actions = self.app.as_mut().unwrap().command(Cmd::Prev);
                        self.run(el, actions);
                    }
                    (MouseButton::Forward, true) => {
                        let actions = self.app.as_mut().unwrap().command(Cmd::Next);
                        self.run(el, actions);
                    }
                    _ => {}
                }
                self.update_cursor();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64 * 120.0,
                    // A wheel notch is about 40 px of touchpad scrolling.
                    MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => y * 3.0,
                };
                self.wheel_rest += steps;
                let whole = self.wheel_rest.trunc().clamp(-32_000.0, 32_000.0);
                if whole.abs() >= 1.0 {
                    self.wheel_rest -= whole;
                    let p = self.cursor;
                    let actions = self.app.as_mut().unwrap().wheel(p, whole as i16);
                    self.run(el, actions);
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
                self.update_cursor();
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(el, &event),
            _ => {}
        }
        self.after_event();
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        let Some(app) = self.app.as_mut() else { return };
        let actions = match event {
            UserEvent::Wake(Wake::Loaded) => app.on_loaded(),
            UserEvent::Wake(Wake::List) => app.on_list(),
            UserEvent::Wake(Wake::DirChanged) => {
                self.dir_due = Some(Instant::now() + DIR_SETTLE);
                vec![]
            }
            UserEvent::Wake(Wake::Thumbs) => {
                app.on_thumbs();
                vec![]
            }
            UserEvent::Wake(Wake::Batch) => {
                app.on_batch();
                vec![]
            }
            UserEvent::Wake(Wake::ColorTarget { monitor, target }) => {
                app.on_color_target(monitor, target);
                vec![]
            }
            UserEvent::Open(file) => handed_over(file, |f| app.open(f)),
            UserEvent::Notice(text) => {
                app.show_notice(text);
                self.notice_until = Some(Instant::now() + NOTICE);
                vec![]
            }
            UserEvent::Clipboard(r) => {
                let set = r
                    .and_then(|(rgba, w, h)| clipboard::put_image(&mut self.clipboard, rgba, w, h));
                if let Err(e) = set {
                    log(&format!("clipboard: {e}"));
                }
                vec![]
            }
        };
        self.run(el, actions);
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(paths) = self.dropped.take()
            && self.app.is_some()
            && !self.exiting
        {
            self.files_dropped(el, paths);
        }
        self.timers(el);
    }
}

/// Runs the viewer; returns the process exit code.
/// Whether this launch takes part in "Open images in the same window": it
/// hands over to a running window or listens for later launches. Not while
/// measuring the first frame.
fn same_window(settings: &Settings, args: &Args) -> bool {
    settings.single_instance && !args.exit_after_first_frame
}

/// Where `--exit-after-first-frame` writes the startup report: the
/// `--json` path, else `startup.json` in the working directory.
fn report_path(args: &Args) -> Option<PathBuf> {
    let json = || args.json.clone().unwrap_or_else(|| "startup.json".into());
    args.exit_after_first_frame.then(json)
}

/// What a later launch's hand-over does: the window opens its file, if it
/// sent one, and comes to the foreground.
fn handed_over(file: Option<PathBuf>, open: impl FnOnce(PathBuf) -> Vec<Action>) -> Vec<Action> {
    let mut a = file.map(open).unwrap_or_default();
    a.push(Action::Activate);
    a
}

pub fn run() -> i32 {
    timing::init(0);
    timing::mark("main");
    let args = parse_args();
    let settings = settings_store::load();
    blinzel_i18n::set_lang(blinzel_i18n::choose(
        settings.language.as_deref(),
        system_language,
    ));
    if args.install || args.uninstall {
        return super::install::run(args.uninstall);
    }
    if super::update::on_start() {
        return 0;
    }
    if same_window(&settings, &args) && super::instance::hand_off(args.file.as_deref()) {
        return 0;
    }
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("amigo-blinzel: no display ({e})");
            return 1;
        }
    };
    let proxy = event_loop.create_proxy();
    let services = Arc::new(LinuxServices {
        proxy: Mutex::new(proxy.clone()),
    });
    if same_window(&settings, &args) {
        let p = Mutex::new(proxy.clone());
        super::instance::listen(move |file| {
            let _ = p.lock().unwrap().send_event(UserEvent::Open(file));
        });
    }
    let cpu = std::thread::available_parallelism().map_or(4, |n| n.get());
    let loader = Loader::new(services.clone(), 2, cpu.clamp(2, 6), cpu);
    let color = ColorManager::new(settings.color_management, super::color::build_lut);
    color.set_target(None);
    loader.set_color(color.clone());
    // Start decoding the file before the window and the GPU exist.
    if let Some(path) = &args.file
        && let Ok(entry) = files::entry(path)
    {
        let stage = if settings.view.mode == blinzel_core::layout::ViewMode::ActualSize {
            Stage::Full
        } else {
            Stage::Screen
        };
        loader.set_wanted(vec![Job {
            path: path.clone(),
            cache_id: entry.cache_id(),
            stage,
            with_preview: true,
            priority: 0,
        }]);
    }
    timing::mark("first_job_queued");
    super::update::schedule();
    let mut shell = Shell {
        args,
        proxy,
        services,
        settings: Some(settings),
        loader: Some(loader),
        color,
        app: None,
        window: None,
        host: Rc::new(RefCell::new(HostState::default())),
        cursor: (0.0, 0.0),
        modifiers: ModifiersState::default(),
        last_click: None,
        chrome_pressed: None,
        clipboard: None,
        notice_until: None,
        dir_due: None,
        wheel_rest: 0.0,
        dropped: Dropped::default(),
        exiting: false,
    };
    if let Err(e) = event_loop.run_app(&mut shell) {
        eprintln!("amigo-blinzel: {e}");
        return 1;
    }
    let Some(app) = &shell.app else { return 0 };
    app.save_settings_on_exit();
    app.exit_code()
}

/// Whether `a` ends the process. Every way of closing the window does
/// (close button, menu "Exit"), in resident mode too: the Linux shell has
/// no tray to hide the window in (`docs/specs/resident-mode.md` AC-01).
fn ends_process(a: &Action) -> bool {
    matches!(a, Action::Close | Action::Exit | Action::Quit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/specs/resident-mode.md` AC-01: on Linux closing the window
    /// ends the process even with resident mode on in the settings.
    #[test]
    fn closing_the_window_always_ends_the_process() {
        const { assert!(!blinzel_ui::menu::RESIDENT) };
        for a in [Action::Close, Action::Exit, Action::Quit] {
            assert!(ends_process(&a), "{a:?}");
        }
        assert!(!ends_process(&Action::Wake { opened: false }));
        assert!(!ends_process(&Action::Minimize));
    }

    /// `docs/specs/localization.md` AC-10: the UI language from the
    /// environment, in the order the variables are read.
    #[test]
    fn the_system_language_comes_from_the_locale_variables_in_order() {
        let code = |vars: &[(&str, &str)]| {
            language_from_env(|name| {
                vars.iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.to_string())
            })
            .code()
        };
        assert_eq!(code(&[]), "en");
        assert_eq!(code(&[("LANG", "C.UTF-8")]), "en");
        assert_eq!(code(&[("LANG", "POSIX")]), "en");
        assert_eq!(code(&[("LANG", "de_DE.UTF-8")]), "de");
        assert_eq!(code(&[("LANG", "de_DE"), ("LC_MESSAGES", "fr_FR")]), "fr");
        assert_eq!(
            code(&[("LC_MESSAGES", "fr_FR"), ("LC_ALL", "it_IT.UTF-8")]),
            "it"
        );
        assert_eq!(code(&[("LC_ALL", "it_IT"), ("LANGUAGE", "ja_JP")]), "ja");
        // `LANGUAGE` is a list; tags that map to nothing are skipped.
        assert_eq!(
            code(&[("LANGUAGE", "xx:C:ru_RU@euro"), ("LANG", "de_DE")]),
            "ru"
        );
        assert_eq!(code(&[("LANGUAGE", "pt_PT.UTF-8:de")]), "pt-br");
        assert_eq!(code(&[("LC_ALL", "C"), ("LANG", "es_ES.UTF-8")]), "es");
        assert_eq!(code(&[("LANG", "sr_RS@latin")]), "en");
    }

    /// `docs/specs/batch-resize.md` AC-23: the files of one drop reach the
    /// App together, in order, before the next event.
    #[test]
    fn the_files_of_one_drop_are_handed_over_together() {
        let mut d = Dropped::default();
        let (a, b, c) = (
            PathBuf::from("/a.png"),
            PathBuf::from("/b.png"),
            PathBuf::from("/c"),
        );
        for p in [&a, &b, &c] {
            let (dropped, event) = d.feed(WindowEvent::DroppedFile(p.clone()));
            assert!(dropped.is_none() && event.is_none(), "still collecting");
        }
        let (dropped, event) = d.feed(WindowEvent::Focused(true));
        assert_eq!(dropped, Some(vec![a.clone(), b.clone(), c]));
        assert!(matches!(event, Some(WindowEvent::Focused(true))));
        let (dropped, event) = d.feed(WindowEvent::Focused(false));
        assert!(
            dropped.is_none() && event.is_some(),
            "nothing dropped since"
        );

        // The end of the event batch ends a drop too.
        d.feed(WindowEvent::DroppedFile(b.clone()));
        d.feed(WindowEvent::DroppedFile(a.clone()));
        assert_eq!(d.take(), Some(vec![b, a]));
        assert_eq!(d.take(), None);
    }

    /// `docs/specs/browsing.md` AC-15: on by default, off opens every
    /// launch in its own window.
    #[test]
    fn same_window_follows_the_setting() {
        let mut settings = Settings::default();
        let args = Args::default();
        assert!(same_window(&settings, &args), "on by default");
        settings.single_instance = false;
        assert!(!same_window(&settings, &args));
        let measuring = Args {
            exit_after_first_frame: true,
            ..Args::default()
        };
        assert!(!same_window(&Settings::default(), &measuring));
    }

    /// `docs/specs/startup.md` AC-10: measuring writes `--json`'s path, or
    /// `startup.json` without it; without the mode there is no report.
    #[test]
    fn the_startup_report_goes_to_json_or_startup_json() {
        let mut args = Args::default();
        assert_eq!(report_path(&args), None);
        args.json = Some(PathBuf::from("out.json"));
        assert_eq!(report_path(&args), None, "--json alone measures nothing");
        args.exit_after_first_frame = true;
        assert_eq!(report_path(&args), Some(PathBuf::from("out.json")));
        args.json = None;
        assert_eq!(report_path(&args), Some(PathBuf::from("startup.json")));
    }

    /// `docs/specs/browsing.md` AC-15: the running window shows the handed
    /// file and comes to the foreground; without a file it only comes up.
    #[test]
    fn a_handed_over_file_opens_and_the_window_comes_to_the_front() {
        let file = PathBuf::from("/pics/a.png");
        let a = handed_over(Some(file.clone()), |f| {
            assert_eq!(f, file);
            vec![Action::Uncloak]
        });
        assert_eq!(a, [Action::Uncloak, Action::Activate]);
        let a = handed_over(None, |_| panic!("nothing to open"));
        assert_eq!(a, [Action::Activate]);
    }

    /// `docs/specs/image-formats.md` AC-24: there is no system decoder: a
    /// raw keeps its embedded previews, and what the native decoders do
    /// not handle fails as not decoded natively.
    #[test]
    fn without_a_system_decoder_raws_keep_their_previews_and_other_files_fail() {
        // A Fujifilm RAF pointing at a 600 × 400 JPEG (header and an empty
        // scan) and saying 6240 × 4160 in its cropped-size record.
        let jpeg: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 1, 0x90, 2, 0x58, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11,
            1, 0xFF, 0xDA, 0, 12, 3, 1, 0, 2, 0x11, 3, 0x11, 0, 63, 0, 0xFF, 0xD9,
        ];
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(200, 0);
        raf[84..88].copy_from_slice(&200u32.to_be_bytes());
        raf[88..92].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
        raf[92..96].copy_from_slice(&(200 + jpeg.len() as u32).to_be_bytes());
        raf.extend_from_slice(jpeg);
        raf.extend([0, 0, 0, 1, 0x01, 0x11, 0, 4, 0x10, 0x40, 0x18, 0x60]);

        let p = probe(&raf).unwrap();
        assert!(p.is_raw());
        assert_eq!((p.width, p.height), (6240, 4160));
        let sizes: Vec<_> = p.previews.iter().map(|p| (p.width, p.height)).collect();
        assert_eq!(sizes, [(600, 400)]);
        let e = decode_with(&raf, Request::Full, 2).unwrap_err();
        assert_eq!(e.to_string(), "Raw is not decoded natively");

        // Recognized, but not decoded natively: probe and decode say so.
        let ico: &[u8] = &[0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0];
        let psd: &[u8] = b"8BPS\0\x01\0\0\0\0\0\0\0\x03\0\0\0\x10\0\0\0\x10\0\x08\0\x03";
        // A raw without an embedded preview.
        let orf: &[u8] = b"IIRO\x08\0\0\0\0\0\0\0\0\0\0\0";
        for (data, name) in [(ico, "Ico"), (psd, "Psd"), (orf, "Raw")] {
            let msg = format!("{name} is not decoded natively");
            assert_eq!(probe(data).unwrap_err().to_string(), msg);
            let e = decode_with(data, Request::Fit { w: 64, h: 64 }, 1).unwrap_err();
            assert_eq!(e.to_string(), msg);
        }
    }

    /// `docs/specs/platform-shells.md` AC-11: within 6 logical px of an
    /// edge a press resizes in that direction, diagonally in the corners,
    /// under the matching cursor; never while maximized or full screen.
    #[test]
    fn the_edges_resize_the_plain_window_only() {
        use ResizeDirection::*;
        let client = (800, 600);
        let at = |p, scale: f64| resize_edge(p, client, EDGE * scale, true);
        assert_eq!(at((0.0, 0.0), 1.0), Some(NorthWest));
        assert_eq!(at((799.0, 3.0), 1.0), Some(NorthEast));
        assert_eq!(at((5.9, 599.0), 1.0), Some(SouthWest));
        assert_eq!(at((794.0, 594.0), 1.0), Some(SouthEast));
        assert_eq!(at((2.0, 300.0), 1.0), Some(West));
        assert_eq!(at((797.0, 300.0), 1.0), Some(East));
        assert_eq!(at((400.0, 1.0), 1.0), Some(North));
        assert_eq!(at((400.0, 598.0), 1.0), Some(South));
        assert_eq!(at((6.0, 300.0), 1.0), None, "6 px in: the image");
        assert_eq!(at((400.0, 593.0), 1.0), None);
        // Logical pixels: twice as wide at 200 %.
        assert_eq!(at((11.0, 300.0), 2.0), Some(West));
        assert_eq!(at((12.0, 300.0), 2.0), None);
        assert_eq!(resize_edge((0.0, 0.0), client, EDGE, false), None);
        assert_eq!(resize_edge((400.0, 599.0), client, EDGE, false), None);

        assert_eq!(resize_cursor(North), CursorIcon::NsResize);
        assert_eq!(resize_cursor(South), CursorIcon::NsResize);
        assert_eq!(resize_cursor(West), CursorIcon::EwResize);
        assert_eq!(resize_cursor(East), CursorIcon::EwResize);
        assert_eq!(resize_cursor(NorthWest), CursorIcon::NwseResize);
        assert_eq!(resize_cursor(SouthEast), CursorIcon::NwseResize);
        assert_eq!(resize_cursor(NorthEast), CursorIcon::NeswResize);
        assert_eq!(resize_cursor(SouthWest), CursorIcon::NeswResize);
    }

    /// `docs/specs/platform-shells.md` AC-11: a press on the free caption
    /// strip moves the window and a double click there is the App's (which
    /// answers none there, so the window maximizes or restores); edges come
    /// before everything, caption buttons act on release.
    #[test]
    fn a_press_resizes_moves_or_goes_to_the_app() {
        let north = Some(ResizeDirection::North);
        assert_eq!(
            press(north, Some(Button::Close), true, true),
            Press::Resize(ResizeDirection::North)
        );
        for b in [Button::Minimize, Button::Maximize, Button::Close] {
            assert_eq!(press(None, Some(b), true, true), Press::Button(b));
        }
        assert_eq!(press(None, None, false, true), Press::MoveWindow);
        assert_eq!(press(None, None, true, true), Press::DoubleClick);
        assert_eq!(press(None, None, true, false), Press::DoubleClick);
        assert_eq!(press(None, Some(Button::Menu), false, true), Press::App);
        assert_eq!(
            press(None, Some(Button::Menu), true, true),
            Press::DoubleClick
        );
        // Below the strip, full screen or while a selection drag runs.
        assert_eq!(press(None, None, false, false), Press::App);
        assert_eq!(double_clicked(None), [Action::ToggleMaximize]);
        let full = vec![Action::ToggleFullscreen];
        assert_eq!(double_clicked(Some(full.clone())), full);
    }

    /// `docs/specs/platform-shells.md` AC-14: a deleted image goes to the
    /// desktop's trash and only then leaves the list; a refused move
    /// leaves the list alone.
    #[test]
    fn a_deleted_image_goes_to_the_trash_and_a_refused_one_stays_listed() {
        let dir = std::env::temp_dir().join(format!("blinzel-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let file = dir.join("a.png");
        std::fs::write(&file, b"png").unwrap();
        let mut removed = vec![];
        let a = recycle(&file, |p| {
            removed.push(p.to_path_buf());
            vec![Action::Uncloak]
        });
        assert_eq!(a, [Action::Uncloak]);
        assert_eq!(removed, std::slice::from_ref(&file));
        assert!(!file.exists());
        let trashed: Vec<_> = trash::os_limited::list()
            .unwrap()
            .into_iter()
            .filter(|i| i.original_parent == dir && i.name == "a.png")
            .collect();
        assert_eq!(trashed.len(), 1, "in the trash");
        trash::os_limited::purge_all(trashed).unwrap();

        let missing = dir.join("b.png");
        let a = recycle(&missing, |_| panic!("the list keeps it"));
        assert!(a.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `docs/specs/browsing.md` AC-14: changes in the folder are collected
    /// for about 150 ms before it is listed again.
    #[test]
    fn folder_changes_settle_for_150_ms_before_relisting() {
        assert_eq!(DIR_SETTLE, Duration::from_millis(150));
    }
}
