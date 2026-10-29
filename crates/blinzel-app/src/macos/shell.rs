//! The macOS shell: an AppKit window whose content view hosts a
//! `CAMetalLayer`, the menu bar and context menu built from the UI's menu
//! tree, and the system's dialogs, clipboard, trash and folder watching.
//!
//! All UI state lives in one [`Shell`] on the main thread. AppKit calls
//! (view events, menu actions, delegate callbacks) and wake-ups from other
//! threads (posted to the main dispatch queue) reach it through
//! [`with_shell`]; while it is busy (a context menu or a panel runs its own
//! loop inside a handler) events wait in a queue and run right after.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::decode::{DecodeError, Probe, Request};
use blinzel_core::filelist::Entry;
use blinzel_core::icc::Profile;
use blinzel_core::pixel::Image;
use blinzel_core::settings::Settings;
use blinzel_core::timing;
use blinzel_i18n::{Lang, Msg, tr, trf};
use blinzel_ui::app::{Action, App, SaveJob};
use blinzel_ui::backend::{Backend, CursorKind, Host, Key, Services, Timer, Wake};
use blinzel_ui::color::ColorManager;
use blinzel_ui::loader::{Job, Loader, Stage};
use blinzel_ui::menu::{Cmd, MenuItem, MenuState};
use blinzel_ui::{dirs, files, settings_store};
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSBackingStoreType, NSControlStateValueOff, NSControlStateValueOn,
    NSCursor, NSDragOperation, NSDraggingInfo, NSEvent, NSEventModifierFlags, NSMenu,
    NSMenuDelegate, NSMenuItem, NSModalResponseOK, NSOpenPanel, NSPasteboard,
    NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSSavePanel, NSScreen, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSData, NSFileManager, NSNotification, NSNumber, NSObject,
    NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL, ns_string,
};
use objc2_quartz_core::CAMetalLayer;

use super::gpu::{Gpu, MetalCanvas, MetalDevice, Target};

/// Events that reach the shell from outside AppKit's event handlers.
#[derive(Debug)]
pub enum UserEvent {
    Wake(Wake),
    /// Finder, the Dock or `open` handed over a file.
    Open(PathBuf),
    /// A message for the notice toast.
    Notice(String),
    /// A copy's pixels as PNG, encoded in the background.
    Clipboard(Result<Vec<u8>, String>),
    /// A menu command (its id).
    Menu(u32),
    /// Draw a frame (coalesced invalidations).
    Render,
    /// A timer may be due.
    Tick,
}

/// The macOS shell's backend types.
pub struct Mac;

impl Backend for Mac {
    type Device = MetalDevice;
    type Canvas = MetalCanvas;
    type Host = MacHost;
    type Services = MacServices;
}

// -------------------------------------------------------------- event queue

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    static PENDING: RefCell<VecDeque<UserEvent>> = const { RefCell::new(VecDeque::new()) };
    /// What `run` prepares before AppKit starts, for the window's creation.
    static START: RefCell<Option<Start>> = const { RefCell::new(None) };
}

/// Hands `e` to the main thread (from any thread).
pub fn post(e: UserEvent) {
    DispatchQueue::main().exec_async(move || deliver(e));
}

fn deliver(e: UserEvent) {
    PENDING.with(|p| p.borrow_mut().push_back(e));
    drain();
}

/// Runs queued events while the shell is free.
fn drain() {
    loop {
        let handled = SHELL.with(|s| {
            let Ok(mut s) = s.try_borrow_mut() else {
                return false;
            };
            let Some(shell) = s.as_mut() else {
                return false;
            };
            let Some(e) = PENDING.with(|p| p.borrow_mut().pop_front()) else {
                return false;
            };
            shell.user_event(e);
            true
        });
        if !handled {
            break;
        }
    }
}

/// Runs `f` on the shell unless it is busy (or not created yet), then the
/// events that queued up meanwhile.
fn with_shell<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    let r = SHELL.with(|s| {
        let mut s = s.try_borrow_mut().ok()?;
        Some(f(s.as_mut()?))
    });
    drain();
    r
}

// ---------------------------------------------------------------- services

pub struct MacServices;

/// Debug output when `BLINZEL_DEBUG` is set.
pub(super) fn log(msg: &str) {
    if std::env::var_os("BLINZEL_DEBUG").is_some() {
        eprintln!("blinzel: {msg}");
    }
}

/// The user's preferred languages (System Settings → Language & Region).
pub fn system_language() -> Lang {
    let langs = objc2_foundation::NSLocale::preferredLanguages();
    let tags: Vec<String> = langs.iter().map(|l| l.to_string()).collect();
    blinzel_i18n::negotiate(tags.iter().map(String::as_str))
}

impl Services for MacServices {
    fn wake(&self, w: Wake) {
        post(UserEvent::Wake(w));
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
        super::imageio::probe_any(data)
    }

    fn decode(&self, data: &[u8], req: Request, threads: usize) -> Result<Image, DecodeError> {
        super::imageio::decode_any_with(data, req, threads)
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
    /// A `Render` event is on its way.
    render_queued: bool,
}

pub struct MacHost {
    window: Retained<NSWindow>,
    state: Rc<RefCell<HostState>>,
}

impl Host for MacHost {
    fn invalidate(&self) {
        let mut s = self.state.borrow_mut();
        if !s.render_queued {
            s.render_queued = true;
            post(UserEvent::Render);
        }
    }

    fn first_frame_shown(&self) {
        super::update::on_first_frame();
    }

    fn set_title(&self, title: &str) {
        self.window.setTitle(&NSString::from_str(title));
    }

    fn set_timer(&self, timer: Timer, ms: u32) {
        let due = Instant::now() + Duration::from_millis(ms as u64);
        self.state.borrow_mut().timers.insert(timer, due);
    }

    fn kill_timer(&self, timer: Timer) {
        self.state.borrow_mut().timers.remove(&timer);
    }

    fn to_screen(&self, p: (f64, f64)) -> (i32, i32) {
        // Context menus open at view coordinates.
        (p.0 as i32, p.1 as i32)
    }

    fn monitor(&self) -> Option<(isize, (u32, u32))> {
        let screen = self.window.screen()?;
        let id = screen_id(&screen)?;
        let f = screen.frame();
        let s = screen.backingScaleFactor();
        Some((
            id,
            (
                (f.size.width * s).round() as u32,
                (f.size.height * s).round() as u32,
            ),
        ))
    }

    fn offer_clipboard(&self) -> bool {
        self.state.borrow_mut().copy = true;
        true
    }

    fn watch_dir(&self, dir: &Path) -> Option<Box<dyn std::any::Any>> {
        use notify::Watcher;
        let mut w = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
            if r.is_ok_and(|e| !e.kind.is_access()) {
                post(UserEvent::Wake(Wake::DirChanged));
            }
        })
        .ok()?;
        w.watch(dir, notify::RecursiveMode::NonRecursive).ok()?;
        Some(Box::new(w))
    }
}

/// The display id (`CGDirectDisplayID`) of a screen.
fn screen_id(screen: &NSScreen) -> Option<isize> {
    let desc = screen.deviceDescription();
    let n = desc.objectForKey(ns_string!("NSScreenNumber"))?;
    let n = n.downcast::<NSNumber>().ok()?;
    Some(n.unsignedIntValue() as isize)
}

// ------------------------------------------------------------------- shell

#[derive(Default)]
pub struct Args {
    pub file: Option<PathBuf>,
    pub json: Option<PathBuf>,
    pub exit_after_first_frame: bool,
}

pub fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--json") => a.json = it.next().map(PathBuf::from),
            Some("--exit-after-first-frame") => a.exit_after_first_frame = true,
            // Process serial number from old Finder launches.
            Some(s) if s.starts_with("-psn_") => {}
            _ => a.file = Some(PathBuf::from(arg)),
        }
    }
    a
}

/// The notice toast stays this long.
const NOTICE: Duration = Duration::from_millis(3500);
/// Folder changes are collected this long before relisting.
const DIR_SETTLE: Duration = Duration::from_millis(150);

/// Prepared in [`run`] before AppKit starts: decoding of the file on the
/// command line begins before the window exists.
struct Start {
    args: Args,
    settings: Settings,
    services: Arc<MacServices>,
    loader: Loader<MetalDevice, MacServices>,
    color: Arc<ColorManager>,
}

struct Shell {
    window: Retained<NSWindow>,
    view: Retained<BlinzelView>,
    layer: Retained<CAMetalLayer>,
    services: Arc<MacServices>,
    app: App<Mac>,
    host: Rc<RefCell<HostState>>,
    cursor: (f64, f64),
    modifiers: NSEventModifierFlags,
    notice_until: Option<Instant>,
    dir_due: Option<Instant>,
    /// When the next `Tick` is scheduled.
    tick_at: Option<Instant>,
    /// Fractional wheel steps of touchpads (in 1/120).
    wheel_rest: f64,
    /// A file was opened before the window was shown (it stays cloaked
    /// until that file's first frame).
    opened: bool,
}

impl Shell {
    fn scale(&self) -> f64 {
        self.window.backingScaleFactor()
    }

    /// The view's size in physical pixels.
    fn client(&self) -> (u32, u32) {
        let s = self.view.frame().size;
        let k = self.scale();
        (
            (s.width * k).round().max(1.0) as u32,
            (s.height * k).round().max(1.0) as u32,
        )
    }

    /// A view point (from an event) in physical pixels.
    fn point(&self, event: &NSEvent) -> (f64, f64) {
        let p = self
            .view
            .convertPoint_fromView(event.locationInWindow(), None);
        let k = self.scale();
        (p.x * k, p.y * k)
    }

    fn ctrl(&self) -> bool {
        self.modifiers
            .intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Control)
    }

    fn shift(&self) -> bool {
        self.modifiers.contains(NSEventModifierFlags::Shift)
    }

    /// Runs `actions` and whatever they lead to.
    fn run(&mut self, mut actions: Vec<Action>) {
        while !actions.is_empty() {
            let next: Vec<Action> = actions.drain(..).flat_map(|a| self.action(a)).collect();
            actions = next;
        }
        self.after_event();
    }

    fn action(&mut self, a: Action) -> Vec<Action> {
        let mtm = MainThreadMarker::from(&*self.window);
        match a {
            Action::ShowMenu { x, y } => {
                let state = self.app.menu_state();
                let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!(""));
                menu.setAutoenablesItems(false);
                fill_menu(&menu, &blinzel_ui::menu::items(&state), mtm);
                let k = self.scale();
                // Runs the menu's own loop; a choice arrives as `Menu`.
                menu.popUpMenuPositioningItem_atLocation_inView(
                    None,
                    NSPoint::new(x as f64 / k, y as f64 / k),
                    Some(&self.view),
                );
                vec![]
            }
            Action::FitWindow { client, grow_only } => {
                if !self.app.fullscreen && !self.window.isZoomed() {
                    let k = self.scale();
                    let client = if grow_only {
                        let Some(screen) = self.window.screen() else {
                            return vec![];
                        };
                        let work = screen.visibleFrame().size;
                        let title = self.window.frame().size.height
                            - self
                                .window
                                .contentRectForFrameRect(self.window.frame())
                                .size
                                .height;
                        let max = ((work.width * k) as u32, ((work.height - title) * k) as u32);
                        let min = ((320.0 * k) as u32, (220.0 * k) as u32);
                        match blinzel_core::layout::grown_client_size(
                            client,
                            self.app.client,
                            max,
                            min,
                        ) {
                            Some(size) => size,
                            None => return vec![],
                        }
                    } else {
                        client
                    };
                    let size = NSSize::new(client.0 as f64 / k, client.1 as f64 / k);
                    let old = self.window.frame();
                    let content = NSRect::new(NSPoint::new(0.0, 0.0), size);
                    let mut f = self.window.frameRectForContentRect(content);
                    f.origin.x = old.origin.x + (old.size.width - f.size.width) / 2.0;
                    f.origin.y = old.origin.y + (old.size.height - f.size.height) / 2.0;
                    self.window.setFrame_display(f, true);
                }
                vec![]
            }
            Action::ToggleFullscreen => {
                self.window.toggleFullScreen(None);
                vec![]
            }
            Action::ToggleMaximize => {
                self.window.zoom(None);
                vec![]
            }
            Action::Minimize => {
                self.window.miniaturize(None);
                vec![]
            }
            Action::OpenDialog => {
                let panel = NSOpenPanel::openPanel(mtm);
                panel.setCanChooseFiles(true);
                panel.setCanChooseDirectories(false);
                panel.setAllowsMultipleSelection(false);
                panel.setTitle(Some(&NSString::from_str(tr(Msg::MenuOpen))));
                if let Some(dir) = self
                    .app
                    .current
                    .as_ref()
                    .and_then(|c| c.path.parent().map(Path::to_path_buf))
                {
                    panel.setDirectoryURL(Some(&file_url(&dir)));
                }
                if panel.runModal() == NSModalResponseOK
                    && let Some(path) = panel.URL().and_then(|u| url_path(&u))
                {
                    self.show();
                    self.app.open(path)
                } else {
                    vec![]
                }
            }
            Action::Recycle(path) => {
                let url = file_url(&path);
                match NSFileManager::defaultManager()
                    .trashItemAtURL_resultingItemURL_error(&url, None)
                {
                    Ok(()) => self.app.removed(&path),
                    Err(e) => {
                        log(&format!("trash: {}", e.localizedDescription()));
                        vec![]
                    }
                }
            }
            Action::SaveCutout(job) => {
                self.save(job, mtm);
                vec![]
            }
            Action::CheckUpdates => {
                super::update::check_now(|text| post(UserEvent::Notice(text)));
                vec![]
            }
            Action::About => {
                let text = trf(
                    Msg::AboutBody,
                    &[("version", &env!("CARGO_PKG_VERSION")), ("url", &URL)],
                );
                self.app.show_notice(text);
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
                let panel = NSOpenPanel::openPanel(mtm);
                panel.setCanChooseFiles(false);
                panel.setCanChooseDirectories(true);
                panel.setCanCreateDirectories(true);
                panel.setAllowsMultipleSelection(false);
                if panel.runModal() == NSModalResponseOK
                    && let Some(dir) = panel.URL().and_then(|u| url_path(&u))
                {
                    self.app.batch_folder_picked(dir);
                }
                vec![]
            }
            Action::OpenFolder(dir) => {
                open_path(&dir);
                vec![]
            }
            Action::SetResident(_) | Action::TrayMenu { .. } => vec![],
            Action::Wake { .. } | Action::Activate => {
                self.show();
                vec![]
            }
            Action::Uncloak => {
                self.app.cloaked = false;
                self.show();
                vec![]
            }
            Action::Close | Action::Exit | Action::Quit => {
                NSApplication::sharedApplication(mtm).terminate(None);
                vec![]
            }
            Action::OpenWith(file) => {
                if let Some(app) = pick_app(mtm, tr(Msg::MenuOpenWith)) {
                    let _ = open_with_app(&app, &file);
                }
                vec![]
            }
            Action::EditWith { editor, file } => {
                if let Err(e) = open_with_app(&editor, &file) {
                    self.app
                        .show_notice(trf(Msg::EditorFailed, &[("reason", &e)]));
                }
                vec![]
            }
            Action::ChooseEditor { then } => match pick_app(mtm, tr(Msg::MenuChooseEditor)) {
                Some(editor) => self.app.editor_chosen(editor, then),
                None => vec![],
            },
            // Screenshots are not on macOS yet (the menu does not offer them).
            Action::Capture { .. }
            | Action::SetScreenshotHotkey(_)
            | Action::PickScreenshotFolder => vec![],
        }
    }

    /// Makes the window visible and active.
    fn show(&self) {
        self.window.setAlphaValue(1.0);
        self.window.makeKeyAndOrderFront(None);
        let mtm = MainThreadMarker::from(&*self.window);
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }

    fn save(&mut self, job: SaveJob, mtm: MainThreadMarker) {
        let panel = NSSavePanel::savePanel(mtm);
        panel.setCanCreateDirectories(true);
        panel.setNameFieldStringValue(&NSString::from_str(&job.default_name));
        if let Some(dir) = job.source.parent() {
            panel.setDirectoryURL(Some(&file_url(dir)));
        }
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(target) = panel.URL().and_then(|u| url_path(&u)) else {
            return;
        };
        let services = self.services.clone();
        let _ = std::thread::Builder::new()
            .name("blinzel-save".into())
            .spawn(move || {
                let text = blinzel_ui::app::save_cutout(&*services, &job, &target);
                post(UserEvent::Notice(text));
            });
    }

    /// Work that waits for the end of an event: a copy's pixels, the next
    /// timer.
    fn after_event(&mut self) {
        let copy = std::mem::take(&mut self.host.borrow_mut().copy);
        if copy && let Some(src) = self.app.clip.clone() {
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
                        blinzel_core::batch::encode::encode(
                            &px,
                            w,
                            h,
                            blinzel_core::batch::encode::Codec::Png,
                            &Default::default(),
                            None,
                            None,
                        )
                    })();
                    post(UserEvent::Clipboard(r));
                });
        }
        self.schedule_tick();
    }

    /// Asks for a `Tick` when the next timer is due.
    fn schedule_tick(&mut self) {
        let next = self
            .host
            .borrow()
            .timers
            .values()
            .copied()
            .chain(self.notice_until)
            .chain(self.dir_due)
            .min();
        let Some(next) = next else {
            return;
        };
        if self.tick_at.is_some_and(|t| t <= next) {
            return;
        }
        self.tick_at = Some(next);
        let ns = next.saturating_duration_since(Instant::now()).as_nanos() as i64;
        let _ =
            DispatchQueue::main().after(DispatchTime::NOW.time(ns), || deliver(UserEvent::Tick));
    }

    /// Timers that are due.
    fn timers(&mut self) {
        let now = Instant::now();
        if self.tick_at.is_some_and(|t| t <= now) {
            self.tick_at = None;
        }
        let anim_due = {
            let mut s = self.host.borrow_mut();
            let due = s.timers.get(&Timer::Anim).is_some_and(|&t| t <= now);
            if due {
                s.timers.remove(&Timer::Anim);
            }
            due
        };
        if anim_due {
            self.app.on_anim_timer();
        }
        if self.notice_until.is_some_and(|t| t <= now) {
            self.notice_until = None;
            self.app.notice = None;
            self.app.invalidate();
        }
        if self.dir_due.is_some_and(|t| t <= now) {
            self.dir_due = None;
            self.app.on_dir_changed();
        }
    }

    fn user_event(&mut self, event: UserEvent) {
        let app = &mut self.app;
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
                self.opened = true;
                let mut a = app.open(file);
                if !app.cloaked {
                    a.push(Action::Activate);
                }
                a
            }
            UserEvent::Notice(text) => {
                app.show_notice(text);
                self.notice_until = Some(Instant::now() + NOTICE);
                vec![]
            }
            UserEvent::Clipboard(r) => {
                match r {
                    Ok(png) => {
                        let pb = NSPasteboard::generalPasteboard();
                        pb.clearContents();
                        let data = NSData::with_bytes(&png);
                        // SAFETY: a constant type name.
                        let ok = pb.setData_forType(Some(&data), unsafe { NSPasteboardTypePNG });
                        if !ok {
                            log("clipboard: refused");
                        }
                    }
                    Err(e) => log(&format!("clipboard: {e}")),
                }
                vec![]
            }
            UserEvent::Menu(id) => match Cmd::from_id(id) {
                Some(cmd) => app.command(cmd),
                None => vec![],
            },
            UserEvent::Render => {
                self.host.borrow_mut().render_queued = false;
                app.render()
            }
            UserEvent::Tick => {
                self.timers();
                vec![]
            }
        };
        self.run(actions);
    }

    // ------------------------------------------------------------- input

    // The resize cursors are deprecated in favour of ones macOS 14 lacks.
    #[allow(deprecated)]
    fn update_cursor(&self) {
        let p = self.cursor;
        let kind = if self.app.popup.is_some() {
            CursorKind::Arrow
        } else {
            self.app.cursor_at(p, self.shift())
        };
        let c = match kind {
            CursorKind::Arrow => NSCursor::arrowCursor(),
            CursorKind::Cross => NSCursor::crosshairCursor(),
            CursorKind::Hand => NSCursor::openHandCursor(),
            CursorKind::Move => NSCursor::closedHandCursor(),
            CursorKind::SizeNS => NSCursor::resizeUpDownCursor(),
            CursorKind::SizeWE => NSCursor::resizeLeftRightCursor(),
            CursorKind::SizeNWSE | CursorKind::SizeNESW => NSCursor::crosshairCursor(),
        };
        c.set();
    }

    fn mouse_moved(&mut self, event: &NSEvent) {
        self.cursor = self.point(event);
        let p = self.cursor;
        let app = &mut self.app;
        // No caption buttons to light up: the system draws them on a Mac
        // (the menu button is the app's own business).
        app.mouse_move(p);
        self.update_cursor();
        self.after_event();
    }

    fn left_down(&mut self, event: &NSEvent) {
        self.modifiers = event.modifierFlags();
        let p = self.point(event);
        self.cursor = p;
        // Control-click is the context menu on a Mac.
        if self.modifiers.contains(NSEventModifierFlags::Control)
            && !self.modifiers.contains(NSEventModifierFlags::Command)
        {
            self.run(vec![Action::ShowMenu {
                x: p.0 as i32,
                y: p.1 as i32,
            }]);
            return;
        }
        let double = event.clickCount() == 2;
        let (shift, ctrl) = (self.shift(), self.ctrl());
        if self.app.popup.is_none() && self.app.dialog.is_none() {
            let chrome = self.app.chrome();
            let button = (!self.app.fullscreen).then(|| chrome.hit(p)).flatten();
            if double {
                let actions = match self.app.double_click(p) {
                    Some(a) => a,
                    None if !self.app.fullscreen && p.1 < chrome.caption_height() => {
                        vec![Action::ToggleMaximize]
                    }
                    None => vec![Action::ToggleFullscreen],
                };
                self.run(actions);
                return;
            }
            if !self.app.fullscreen
                && button.is_none()
                && p.1 < chrome.caption_height()
                && !self.app.is_dragging()
            {
                self.window.performWindowDragWithEvent(event);
                return;
            }
        }
        let actions = self.app.mouse_down(p, shift, ctrl, false);
        self.run(actions);
    }

    fn left_up(&mut self) {
        self.app.mouse_up();
        self.update_cursor();
        self.after_event();
    }

    fn other_down(&mut self, event: &NSEvent) {
        let p = self.point(event);
        let actions = match event.buttonNumber() {
            2 => self.app.mouse_down(p, self.shift(), self.ctrl(), true),
            3 => self.app.command(Cmd::Prev),
            4 => self.app.command(Cmd::Next),
            _ => vec![],
        };
        self.run(actions);
    }

    fn scroll(&mut self, event: &NSEvent) {
        let y = event.scrollingDeltaY();
        // A wheel notch is about 40 points of touchpad scrolling.
        let steps = if event.hasPreciseScrollingDeltas() {
            y * 3.0
        } else {
            y * 120.0
        };
        self.wheel_rest += steps;
        let whole = self.wheel_rest.trunc().clamp(-32_000.0, 32_000.0);
        if whole.abs() >= 1.0 {
            self.wheel_rest -= whole;
            let p = self.point(event);
            let actions = self.app.wheel(p, whole as i16);
            self.run(actions);
        }
    }

    fn key_down(&mut self, event: &NSEvent) {
        self.modifiers = event.modifierFlags();
        let (ctrl, shift) = (self.ctrl(), self.shift());
        let chars = event
            .charactersIgnoringModifiers()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let actions = key_of(event.keyCode(), &chars).and_then(|k| self.app.key(k, ctrl, shift));
        // Characters for the batch dialog's text fields.
        if self.app.dialog.is_some()
            && !ctrl
            && let Some(text) = event.characters()
        {
            let text: String = text
                .to_string()
                .chars()
                .filter(|c| !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(c))
                .collect();
            if !text.is_empty() {
                self.app.text_input(&text);
            }
        }
        match actions {
            Some(a) => self.run(a),
            None => self.after_event(),
        }
    }

    fn resized(&mut self) {
        let client = self.client();
        self.layer.setContentsScale(self.scale());
        self.app.dpi = (self.scale() * 96.0).round() as u32;
        self.app.maximized = self.window.isZoomed();
        self.app.resized(client);
        self.app.update_screen_box();
        // Draw now: AppKit stretches the old frame meanwhile.
        let actions = self.app.render();
        self.run(actions);
    }
}

const URL: &str = "https://github.com/amigo-labs/amigo-blinzel";

fn file_url(p: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()))
}

fn url_path(u: &NSURL) -> Option<PathBuf> {
    u.path().map(|p| PathBuf::from(p.to_string()))
}

fn open_path(p: &Path) {
    let _ = std::process::Command::new("/usr/bin/open").arg(p).spawn();
}

/// Asks for an application (in /Applications).
fn pick_app(mtm: MainThreadMarker, title: &str) -> Option<PathBuf> {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setTitle(Some(&NSString::from_str(title)));
    panel.setDirectoryURL(Some(&file_url(Path::new("/Applications"))));
    (panel.runModal() == NSModalResponseOK)
        .then(|| panel.URL().and_then(|u| url_path(&u)))
        .flatten()
}

/// Opens `file` with an app bundle (through Launch Services) or a program.
fn open_with_app(app: &Path, file: &Path) -> Result<(), String> {
    let status = if app.extension().is_some_and(|e| e == "app") {
        std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg(app)
            .arg(file)
            .status()
    } else {
        std::process::Command::new(app)
            .arg(file)
            .spawn()
            .map(|_| Default::default())
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!("{} cannot open the file", app.display())),
        Err(e) => Err(e.to_string()),
    }
}

/// Writes the licence texts next to the cache and opens them.
fn show_notices() -> Result<(), String> {
    let dir = dirs::cache().ok_or("no cache folder")?;
    let path = dir.join("THIRD-PARTY-NOTICES.txt");
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, crate::notices_text::TEXT))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    open_path(&path);
    Ok(())
}

/// macOS virtual key codes (`kVK_*`) and the typed character.
fn key_of(code: u16, chars: &str) -> Option<Key> {
    Some(match code {
        123 => Key::Left,
        124 => Key::Right,
        125 => Key::Down,
        126 => Key::Up,
        116 => Key::PageUp,
        121 => Key::PageDown,
        115 => Key::Home,
        119 => Key::End,
        49 => Key::Space,
        51 => Key::Backspace,
        117 => Key::Delete,
        36 | 76 => Key::Enter,
        53 => Key::Escape,
        48 => Key::Tab,
        109 => Key::F10,
        103 => Key::F11,
        110 => Key::Menu,
        _ => {
            let mut it = chars.chars();
            let c = it.next()?;
            if it.next().is_some() {
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
    })
}

// ------------------------------------------------------------------- menus

/// Fills `menu` from the UI's menu tree; items send `menuCommand:` with
/// their command id as tag.
fn fill_menu(menu: &NSMenu, items: &[MenuItem], mtm: MainThreadMarker) {
    menu.removeAllItems();
    let target = DELEGATE.with(|d| d.get().cloned());
    for it in items {
        match it {
            MenuItem::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
            MenuItem::Item {
                cmd,
                label,
                checked,
                enabled,
                ..
            } => {
                let quit = *cmd == Cmd::Exit;
                // SAFETY: `menuCommand:` exists on the delegate (target).
                let mi = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(label),
                        Some(sel!(menuCommand:)),
                        if quit {
                            ns_string!("q")
                        } else {
                            ns_string!("")
                        },
                    )
                };
                if let Some(t) = &target {
                    // SAFETY: the delegate lives as long as the app.
                    unsafe { mi.setTarget(Some(t)) };
                }
                mi.setTag(cmd.id() as isize);
                mi.setState(if *checked {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                mi.setEnabled(*enabled);
                menu.addItem(&mi);
            }
            MenuItem::Sub(label, children) => {
                let title = NSString::from_str(label);
                let sub = NSMenu::initWithTitle(NSMenu::alloc(mtm), &title);
                sub.setAutoenablesItems(false);
                fill_menu(&sub, children, mtm);
                let mi = NSMenuItem::new(mtm);
                mi.setTitle(&title);
                mi.setSubmenu(Some(&sub));
                menu.addItem(&mi);
            }
        }
    }
}

/// The menu tree split for the menu bar: the application menu (about,
/// updates, settings, language, quit) and the rest.
fn menu_bar_trees(state: &MenuState) -> (Vec<MenuItem>, Vec<MenuItem>) {
    let app_cmds = [Cmd::About, Cmd::CheckUpdates, Cmd::Notices];
    let app_subs = [tr(Msg::MenuSettings), tr(Msg::MenuLanguage)];
    let mut app_menu = Vec::new();
    let mut subs = Vec::new();
    let mut quit = None;
    let mut rest = Vec::new();
    for it in blinzel_ui::menu::items(state) {
        match &it {
            MenuItem::Item { cmd, .. } if app_cmds.contains(cmd) => app_menu.push(it),
            MenuItem::Item { cmd: Cmd::Exit, .. } => quit = Some(it),
            MenuItem::Sub(label, _) if app_subs.contains(&label.as_str()) => subs.push(it),
            _ => rest.push(it),
        }
    }
    app_menu.push(MenuItem::Separator);
    app_menu.extend(subs);
    app_menu.push(MenuItem::Separator);
    app_menu.extend(quit);
    (tidy(app_menu), tidy(rest))
}

/// Drops leading, trailing and doubled separators.
fn tidy(items: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut out: Vec<MenuItem> = Vec::with_capacity(items.len());
    for it in items {
        let sep = it == MenuItem::Separator;
        if sep && out.last().is_none_or(|l| *l == MenuItem::Separator) {
            continue;
        }
        out.push(it);
    }
    while out.last() == Some(&MenuItem::Separator) {
        out.pop();
    }
    out
}

// ------------------------------------------------------------------ classes

thread_local! {
    static DELEGATE: OnceCell<Retained<Delegate>> = const { OnceCell::new() };
}

#[derive(Default)]
pub struct DelegateIvars {
    app_menu: OnceCell<Retained<NSMenu>>,
    image_menu: OnceCell<Retained<NSMenu>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BlinzelDelegate"]
    #[ivars = DelegateIvars]
    pub struct Delegate;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: the signatures match AppKit's.
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_finish_launching(&self, _n: &NSNotification) {
            let mtm = self.mtm();
            self.build_menu_bar(mtm);
            if let Err(e) = create_shell(mtm) {
                eprintln!("amigo-blinzel: {e}");
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }

        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            with_shell(|s| {
                if !s.opened {
                    s.app.cloaked = false;
                    s.show();
                }
                s.app.invalidate();
            });
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            if let Some(path) = urls.iter().find_map(|u| url_path(&u)) {
                deliver(UserEvent::Open(path));
            }
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last_window(&self, _app: &NSApplication) -> bool {
            true
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            with_shell(|s| settings_store::save(&s.app.settings));
            NSApplicationTerminateReply::TerminateNow
        }
    }

    // SAFETY: the signatures match AppKit's.
    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _n: &NSNotification) {
            with_shell(Shell::resized);
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing(&self, _n: &NSNotification) {
            with_shell(Shell::resized);
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn window_did_change_screen(&self, _n: &NSNotification) {
            with_shell(|s| {
                s.app.check_monitor(false);
                s.app.update_screen_box();
                s.after_event();
            });
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn did_enter_full_screen(&self, _n: &NSNotification) {
            with_shell(|s| {
                s.app.fullscreen = true;
                s.app.invalidate();
                s.after_event();
            });
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn did_exit_full_screen(&self, _n: &NSNotification) {
            with_shell(|s| {
                s.app.fullscreen = false;
                s.app.invalidate();
                s.after_event();
            });
        }
    }

    // SAFETY: the signature matches AppKit's.
    unsafe impl NSMenuDelegate for Delegate {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            let Some(state) = with_shell(|s| s.app.menu_state()) else {
                return;
            };
            let (app_items, image_items) = menu_bar_trees(&state);
            let mtm = self.mtm();
            let iv = self.ivars();
            if iv.app_menu.get().is_some_and(|m| std::ptr::eq(&**m, menu)) {
                fill_menu(menu, &app_items, mtm);
            } else if iv.image_menu.get().is_some_and(|m| std::ptr::eq(&**m, menu)) {
                fill_menu(menu, &image_items, mtm);
            }
        }
    }

    impl Delegate {
        /// Menu items' action: the command id is the item's tag.
        #[unsafe(method(menuCommand:))]
        fn menu_command(&self, sender: &NSMenuItem) {
            deliver(UserEvent::Menu(sender.tag() as u32));
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars::default());
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }

    /// The menu bar: the application menu and "Image"; both are filled
    /// when they open (`menuNeedsUpdate:`), so checks and states are
    /// current.
    fn build_menu_bar(&self, mtm: MainThreadMarker) {
        let bar = NSMenu::new(mtm);
        let delegate = ProtocolObject::from_ref(self);
        let add = |title: &str| {
            let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
            menu.setAutoenablesItems(false);
            menu.setDelegate(Some(delegate));
            // One placeholder so AppKit shows the menu at all.
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            let item = NSMenuItem::new(mtm);
            item.setSubmenu(Some(&menu));
            bar.addItem(&item);
            menu
        };
        let app_menu = add(tr(Msg::AppName));
        let image_menu = add(tr(Msg::MenuImage));
        let _ = self.ivars().app_menu.set(app_menu);
        let _ = self.ivars().image_menu.set(image_menu);
        NSApplication::sharedApplication(mtm).setMainMenu(Some(&bar));
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "BlinzelView"]
    pub struct BlinzelView;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for BlinzelView {}

    // SAFETY: the signatures match AppKit's.
    impl BlinzelView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        /// The app drags the window itself (caption strip only).
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            with_shell(|s| s.left_down(e));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _e: &NSEvent) {
            with_shell(Shell::left_up);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            with_shell(|s| s.mouse_moved(e));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            with_shell(|s| s.mouse_moved(e));
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, e: &NSEvent) {
            with_shell(|s| {
                let p = s.point(e);
                s.run(vec![Action::ShowMenu {
                    x: p.0 as i32,
                    y: p.1 as i32,
                }]);
            });
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, e: &NSEvent) {
            with_shell(|s| s.other_down(e));
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, _e: &NSEvent) {
            with_shell(|s| {
                s.app.mouse_up();
                s.after_event();
            });
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, e: &NSEvent) {
            with_shell(|s| s.mouse_moved(e));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, e: &NSEvent) {
            with_shell(|s| s.scroll(e));
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            with_shell(|s| s.key_down(e));
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, e: &NSEvent) {
            with_shell(|s| {
                s.modifiers = e.modifierFlags();
                s.update_cursor();
            });
        }

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let pb = sender.draggingPasteboard();
            let urls = {
                let class = <NSURL as objc2::ClassType>::class();
                let classes = NSArray::from_slice(&[class]);
                // SAFETY: NSURL is a valid class to read.
                unsafe { pb.readObjectsForClasses_options(&classes, None) }
            };
            let path = urls.and_then(|a| {
                a.iter()
                    .find_map(|o| o.downcast::<NSURL>().ok().and_then(|u| url_path(&u)))
            });
            match path {
                Some(p) => {
                    deliver(UserEvent::Open(p));
                    true
                }
                None => false,
            }
        }
    }
);

impl BlinzelView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

// --------------------------------------------------------------------- run

fn create_shell(mtm: MainThreadMarker) -> Result<(), String> {
    let Start {
        args,
        settings,
        services,
        loader,
        color,
    } = START
        .with(|s| s.borrow_mut().take())
        .ok_or("started twice")?;
    let screen = NSScreen::mainScreen(mtm);
    let visible = screen.as_ref().map(|s| s.visibleFrame());
    let (w, h) = visible.map_or((1100.0, 720.0), |f| {
        (f.size.width * 0.7, f.size.height * 0.7)
    });
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    let rect = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(w.max(320.0), h.max(240.0)),
    );
    // SAFETY: released-when-closed is turned off right below.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the shell keeps the window.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(tr(Msg::AppName)));
    window.setTitlebarAppearsTransparent(true);
    window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    window.setAcceptsMouseMovedEvents(true);
    window.setContentMinSize(NSSize::new(200.0, 150.0));
    window.center();
    // Cloaked until the first frame of a file opened at launch.
    window.setAlphaValue(0.0);
    if let Some(d) = DELEGATE.with(|d| d.get().cloned()) {
        window.setDelegate(Some(ProtocolObject::from_ref(&*d)));
    }

    let view = BlinzelView::new(mtm, rect);
    let layer = CAMetalLayer::new();
    let scale = window.backingScaleFactor();
    layer.setContentsScale(scale);
    // A layer-hosting view: the layer first, then wantsLayer.
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    // SAFETY: a constant type name.
    let types = NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]);
    view.registerForDraggedTypes(&types);
    window.setContentView(Some(&view));
    window.makeFirstResponder(Some(&view));
    timing::mark("window_created");

    let gpu = Arc::new(Gpu::new()?);
    timing::mark("device_ready");
    let size = view.frame().size;
    let client = (
        (size.width * scale).round().max(1.0) as u32,
        (size.height * scale).round().max(1.0) as u32,
    );
    let canvas = MetalCanvas::new(gpu.clone(), Target::Layer(layer.clone()), client);
    let device = Arc::new(MetalDevice::new(gpu));
    loader.set_device(device.clone());
    let state = Rc::new(RefCell::new(HostState::default()));
    let host = MacHost {
        window: window.clone(),
        state: state.clone(),
    };
    let dpi = (scale * 96.0).round() as u32;
    let mut app = App::new(
        host,
        device,
        canvas,
        services.clone(),
        settings,
        loader,
        color,
        dpi,
        client,
    );
    app.cloaked = true;
    if args.exit_after_first_frame {
        app.exit_after_first_frame = Some(
            args.json
                .clone()
                .unwrap_or_else(|| PathBuf::from("startup.json")),
        );
    }
    let shell = Shell {
        window,
        view,
        layer,
        services,
        app,
        host: state,
        cursor: (0.0, 0.0),
        modifiers: NSEventModifierFlags::empty(),
        notice_until: None,
        dir_due: None,
        tick_at: None,
        wheel_rest: 0.0,
        opened: false,
    };
    SHELL.with(|s| *s.borrow_mut() = Some(shell));
    with_shell(|s| {
        s.app.update_screen_box();
        s.app.check_monitor(true);
        s.after_event();
    });
    if let Some(f) = args.file {
        deliver(UserEvent::Open(f));
    }
    Ok(())
}

/// Runs the viewer; returns the process exit code.
pub fn run() -> i32 {
    timing::init(0);
    timing::mark("main");
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("amigo-blinzel: not on the main thread");
        return 1;
    };
    let args = parse_args();
    let settings = settings_store::load();
    let lang = settings
        .language
        .as_deref()
        .and_then(blinzel_i18n::lang_from_code)
        .unwrap_or_else(system_language);
    blinzel_i18n::set_lang(lang);
    if super::update::on_start() {
        return 0;
    }
    let services = Arc::new(MacServices);
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
    START.with(|s| {
        *s.borrow_mut() = Some(Start {
            args,
            settings,
            services,
            loader,
            color,
        })
    });

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|d| {
        let _ = d.set(delegate);
    });
    app.run();
    0
}
