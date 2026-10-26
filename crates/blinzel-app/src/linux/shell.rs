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

/// The desktop's language preference (`LANGUAGE`, `LC_ALL`, `LANG`).
pub fn system_language() -> Lang {
    let vars: Vec<String> = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .flat_map(|v| {
            v.split(':')
                .map(|tag| tag.split(['.', '@']).next().unwrap_or("").replace('_', "-"))
                .collect::<Vec<_>>()
        })
        .filter(|t| !t.is_empty() && t != "C" && t != "POSIX")
        .collect();
    blinzel_i18n::negotiate(vars.iter().map(String::as_str))
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
        decode::probe(data)
    }

    fn decode(&self, data: &[u8], req: Request, threads: usize) -> Result<Image, DecodeError> {
        decode::decode_with(data, req, threads)
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
        use notify::Watcher;
        let proxy = Mutex::new(self.proxy.clone());
        let mut w = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
            if r.is_ok_and(|e| !e.kind.is_access()) {
                let _ = proxy
                    .lock()
                    .unwrap()
                    .send_event(UserEvent::Wake(Wake::DirChanged));
            }
        })
        .ok()?;
        w.watch(dir, notify::RecursiveMode::NonRecursive).ok()?;
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
    exiting: bool,
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
        if self.args.exit_after_first_frame {
            app.exit_after_first_frame = Some(
                self.args
                    .json
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("startup.json")),
            );
        }
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
        match a {
            Action::ShowMenu { x, y } => {
                app.open_menu((x as f64, y as f64));
                vec![]
            }
            Action::FitWindow { client } => {
                if !app.fullscreen && !window.is_maximized() {
                    let _ = window.request_inner_size(PhysicalSize::new(client.0, client.1));
                }
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
            Action::Recycle(path) => match trash::delete(&path) {
                Ok(()) => app.removed(&path),
                Err(e) => {
                    log(&format!("trash: {e}"));
                    vec![]
                }
            },
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
            Action::Close | Action::Exit | Action::Quit => {
                self.exiting = true;
                el.exit();
                vec![]
            }
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
        let _ = std::thread::Builder::new()
            .name("blinzel-save".into())
            .spawn(move || {
                let text = blinzel_ui::app::save_cutout(&*services, &job, &target);
                services.send(UserEvent::Notice(text));
            });
    }

    /// Work that waits for the end of an event: a copy's pixels.
    fn after_event(&mut self) {
        let copy = std::mem::take(&mut self.host.borrow_mut().copy);
        if copy && let Some(src) = self.app.as_ref().and_then(|a| a.clip.clone()) {
            let services = self.services.clone();
            let _ = std::thread::Builder::new()
                .name("blinzel-copy".into())
                .spawn(move || {
                    let r = (|| -> Result<_, String> {
                        let data = services.read_file(&src.path).map_err(|e| e.to_string())?;
                        let img = services
                            .decode(&data, Request::Region(src.stored_rect), 1)
                            .map_err(|e| e.to_string())?;
                        let bgra = img.to_bgra();
                        let (px, w, h) = blinzel_core::selection::orient_bgra(
                            &bgra,
                            img.width,
                            img.height,
                            src.orientation,
                        );
                        let rgba = px
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|p| [p[2], p[1], p[0], p[3]])
                            .collect();
                        Ok((rgba, w, h))
                    })();
                    services.send(UserEvent::Clipboard(r));
                });
        }
    }

    /// The resize direction for a client point near the window's edges.
    fn edge(&self, p: (f64, f64)) -> Option<ResizeDirection> {
        let (app, window) = (self.app.as_ref()?, self.window.as_ref()?);
        if app.fullscreen || window.is_maximized() {
            return None;
        }
        let e = EDGE * self.scale();
        let (w, h) = (app.client.0 as f64, app.client.1 as f64);
        let (l, r, t, b) = (p.0 < e, p.0 >= w - e, p.1 < e, p.1 >= h - e);
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

    fn update_cursor(&self) {
        let (Some(app), Some(window)) = (self.app.as_ref(), self.window.as_ref()) else {
            return;
        };
        let p = self.cursor;
        let icon = match self.edge(p) {
            Some(ResizeDirection::North | ResizeDirection::South) => CursorIcon::NsResize,
            Some(ResizeDirection::East | ResizeDirection::West) => CursorIcon::EwResize,
            Some(ResizeDirection::NorthWest | ResizeDirection::SouthEast) => CursorIcon::NwseResize,
            Some(_) => CursorIcon::NeswResize,
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
        let (Some(app), Some(window)) = (self.app.as_mut(), self.window.clone()) else {
            return;
        };
        if app.popup.is_none() {
            if let Some(dir) = self.edge(p) {
                let _ = window.drag_resize_window(dir);
                return;
            }
            let app = self.app.as_mut().unwrap();
            let chrome = app.chrome();
            let button = (!app.fullscreen).then(|| chrome.hit(p)).flatten();
            if let Some(b @ (Button::Minimize | Button::Maximize | Button::Close)) = button {
                app.pressed = Some(b);
                self.chrome_pressed = Some(b);
                app.invalidate();
                return;
            }
            if double {
                let actions = match app.double_click(p) {
                    Some(a) => a,
                    None if !app.fullscreen && p.1 < chrome.caption_height() => {
                        vec![Action::ToggleMaximize]
                    }
                    None => vec![Action::ToggleFullscreen],
                };
                self.run(el, actions);
                return;
            }
            if !app.fullscreen
                && button.is_none()
                && p.1 < chrome.caption_height()
                && !app.is_dragging()
            {
                let _ = window.drag_window();
                return;
            }
        }
        let app = self.app.as_mut().unwrap();
        let ctrl = self.modifiers.control_key();
        let actions = app.mouse_down(p, shift, ctrl, false);
        self.run(el, actions);
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
        match event {
            WindowEvent::CloseRequested => {
                self.exiting = true;
                el.exit();
            }
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
                        let (x, y) = (self.cursor.0 as i32, self.cursor.1 as i32);
                        self.run(el, vec![Action::ShowMenu { x, y }]);
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
            WindowEvent::DroppedFile(path) => {
                let actions = self.app.as_mut().unwrap().files_dropped(vec![path]);
                self.run(el, actions);
            }
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
            UserEvent::Open(file) => {
                let mut a = match file {
                    Some(f) => app.open(f),
                    None => vec![],
                };
                a.push(Action::Activate);
                a
            }
            UserEvent::Notice(text) => {
                app.show_notice(text);
                self.notice_until = Some(Instant::now() + NOTICE);
                vec![]
            }
            UserEvent::Clipboard(r) => {
                let set = r.and_then(|(rgba, w, h)| {
                    let cb = match &mut self.clipboard {
                        Some(c) => c,
                        None => self
                            .clipboard
                            .insert(arboard::Clipboard::new().map_err(|e| e.to_string())?),
                    };
                    cb.set_image(arboard::ImageData {
                        width: w as usize,
                        height: h as usize,
                        bytes: rgba.into(),
                    })
                    .map_err(|e| e.to_string())
                });
                if let Err(e) = set {
                    log(&format!("clipboard: {e}"));
                }
                vec![]
            }
        };
        self.run(el, actions);
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.timers(el);
    }
}

/// Runs the viewer; returns the process exit code.
pub fn run() -> i32 {
    timing::init(0);
    timing::mark("main");
    let args = parse_args();
    let settings = settings_store::load();
    let lang = settings
        .language
        .as_deref()
        .and_then(blinzel_i18n::lang_from_code)
        .unwrap_or_else(system_language);
    blinzel_i18n::set_lang(lang);
    if args.install || args.uninstall {
        return super::install::run(args.uninstall);
    }
    if super::update::on_start() {
        return 0;
    }
    if settings.single_instance
        && !args.exit_after_first_frame
        && super::instance::hand_off(args.file.as_deref())
    {
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
    if settings.single_instance && !args.exit_after_first_frame {
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
        exiting: false,
    };
    if let Err(e) = event_loop.run_app(&mut shell) {
        eprintln!("amigo-blinzel: {e}");
        return 1;
    }
    if let Some(app) = &shell.app {
        settings_store::save(&app.settings);
    }
    0
}
