//! Startup. Everything that can run in parallel does:
//!
//! 1. single-instance hand-off (before anything expensive),
//! 2. decode worker reads + decodes the file (screen-sized),
//! 3. a second worker loads d3d11.dll and creates the device,
//! 4. meanwhile the main thread creates the window — cloaked, with the DWM
//!    open animation disabled — and the folder is listed in the background.
//!
//! The window becomes visible (uncloaked) with the first image on it.

use std::path::PathBuf;
use std::sync::mpsc::channel;

use blinzel_core::timing;
use blinzel_i18n::{Lang, Msg, negotiate, set_lang, tr};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
use windows::Win32::Graphics::Dwm::{
    DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Shell::DragAcceptFiles;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR};

use crate::app::{Action, App};
use crate::gfx::{Gfx, Renderer, SwapChain};
use crate::loader::{Job, Loader, Stage, WM_LOADED};
use crate::platform::{io, threads, wide};
use crate::window::{self, APP, run_actions, set_cloak, start_uncloak_timer};
use crate::{bench, instance, resident, settings_store, setup, update};

/// The main window for the device thread's wake-up message (0 until it
/// exists).
static MAIN_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[derive(Debug, Default)]
pub struct Args {
    pub file: Option<PathBuf>,
    pub bench: Option<PathBuf>,
    pub warp: bool,
    pub json: Option<PathBuf>,
    pub exit_after_first_frame: bool,
    /// Started at sign-in: hidden, tray icon only (resident mode).
    pub resident: bool,
}

pub fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--bench") => a.bench = it.next().map(PathBuf::from),
            Some("--warp") => a.warp = true,
            Some("--json") => a.json = it.next().map(PathBuf::from),
            Some("--exit-after-first-frame") => a.exit_after_first_frame = true,
            Some("--resident") => a.resident = true,
            _ => a.file = Some(PathBuf::from(arg)),
        }
    }
    a
}

/// Microseconds since the process was created (loader, Defender scan and
/// CRT init happened before `main`).
fn process_age_us() -> i64 {
    crate::platform::process_created().map_or(0, |c| (crate::platform::now_filetime() - c) / 10)
}

/// The best shipped language for the user's Windows UI languages.
pub fn system_language() -> Lang {
    let mut count = 0u32;
    let mut len = 0u32;
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut len).is_err()
            || len == 0
        {
            return negotiate([]);
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .is_err()
        {
            return negotiate([]);
        }
        let all = String::from_utf16_lossy(&buf);
        negotiate(all.split('\0').filter(|s| !s.is_empty()))
    }
}

fn monitor_under_cursor() -> (HMONITOR, RECT, RECT) {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    };
    unsafe {
        let mut pt = windows::Win32::Foundation::POINT::default();
        let _ = GetCursorPos(&mut pt);
        let m = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(m, &mut mi);
        (m, mi.rcMonitor, mi.rcWork)
    }
}

fn fatal(msg: &str) -> i32 {
    let text = wide(msg);
    let caption = wide(tr(Msg::AppName));
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_ICONERROR | MB_OK,
        );
    }
    1
}

pub fn run() -> i32 {
    timing::init(process_age_us());
    timing::mark("main");
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args = parse_args();
    if let Some(dir) = &args.bench {
        return bench::run(dir, args.warp, args.json.as_deref());
    }
    let settings = settings_store::load();
    let lang = settings
        .language
        .as_deref()
        .and_then(blinzel_i18n::lang_from_code)
        .unwrap_or_else(system_language);
    set_lang(lang);
    // The executable is its own installer (setup file name, --install,
    // --uninstall).
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let exe = std::env::current_exe().unwrap_or_default();
    if let Some(mode) = setup::mode(&raw, &exe) {
        return setup::run(mode);
    }
    if !crate::platform::os::is_supported() {
        return fatal(tr(Msg::ErrorOs));
    }
    // A freshly updated version that keeps failing hands over to the
    // previous one.
    if update::on_start() {
        return 0;
    }
    unsafe {
        // Groups the window with the Start menu shortcut on the taskbar.
        let id = wide(setup::shortcut::APP_ID);
        let _ =
            windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr()));
    }

    if args.resident {
        // Started at sign-in: only if still wanted, and only once.
        if !settings.resident {
            resident::set_autostart(false);
            return 0;
        }
        if instance::running().is_some() {
            return 0;
        }
    } else if !args.exit_after_first_frame
        && let Some(target) = instance::target(settings.single_instance, settings.resident)
    {
        let handed_off = match &args.file {
            Some(file) => instance::hand_off(target, file),
            // Start menu: a resident instance shows its window.
            None => settings.resident && instance::show(target),
        };
        if handed_off {
            return 0;
        }
    }
    let file = args.file.clone().filter(|_| !args.resident);
    timing::mark("instance_checked");

    // Decode workers: P-cores only (at most what the prefetch window can
    // use); the visible image decodes on all P-cores. One I/O thread on
    // rotating disks.
    let cpu = threads::cpu_info();
    let drive = file.as_deref().map(io::drive_info).unwrap_or_default();
    let io_threads = if drive.seek_penalty { 1 } else { 2 };
    let cpu_threads = cpu.performance.clamp(2, 6);
    let loader = Loader::new(io_threads, cpu_threads, cpu.performance);
    let (hmonitor, monitor, work) = monitor_under_cursor();
    // The display's colour target as remembered from the last start (see
    // color); Windows is asked again after the first frame.
    let color = crate::color::ColorManager::new(settings.color_management);
    if settings.color_management {
        let c = color.clone();
        let m = hmonitor.0 as isize;
        let _ = std::thread::Builder::new()
            .name("blinzel-color".into())
            .spawn(move || {
                let remembered = crate::color::remembered_target(HMONITOR(m as *mut _));
                c.set_target(remembered.flatten());
            });
    }
    loader.set_color(color.clone());
    loader.set_screen_box((
        (monitor.right - monitor.left) as u32,
        (monitor.bottom - monitor.top) as u32,
    ));

    // Start decoding the file before anything else.
    let first = file
        .as_ref()
        .and_then(|f| io::entry(f).ok().map(|e| (f.clone(), e)));
    if let Some((path, entry)) = &first {
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

    // Device on a worker (loads d3d11.dll there).
    let (dev_tx, dev_rx) = channel();
    let warp = args.warp;
    let _ = std::thread::Builder::new()
        .name("blinzel-gfx-init".into())
        .spawn(move || {
            let r = Gfx::create(warp);
            timing::mark("device_ready");
            let _ = dev_tx.send(r);
            // Wake the UI thread's message loop (see below).
            let hwnd = MAIN_HWND.load(std::sync::atomic::Ordering::SeqCst);
            if hwnd != 0 {
                unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_NULL, WPARAM(0), LPARAM(0));
                }
            }
        });
    crate::first_frame::configure(settings.view, settings.background);

    // Window, cloaked until the first image is on it.
    let instance = unsafe { GetModuleHandleW(None) }
        .map(Into::into)
        .unwrap_or_default();
    if let Err(e) = window::register_class(instance) {
        return fatal(&e.message());
    }
    let (ww, wh) = (
        (work.right - work.left) * 7 / 10,
        (work.bottom - work.top) * 7 / 10,
    );
    let (wx, wy) = (
        work.left + ((work.right - work.left) - ww) / 2,
        work.top + ((work.bottom - work.top) - wh) / 2,
    );
    let title = wide(tr(Msg::AppName));
    let hwnd = match unsafe {
        CreateWindowExW(
            // With a redirection surface: the first frame may come through
            // GDI before Direct3D is ready.
            WINDOW_EX_STYLE(0),
            instance::WINDOW_CLASS,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            wx,
            wy,
            ww,
            wh,
            None,
            None,
            Some(instance),
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => return fatal(&e.message()),
    };
    prepare_window(hwnd);
    let mut cr = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut cr);
    }
    loader.set_first_frame_target(crate::first_frame::Target {
        client: (
            (cr.right - cr.left).max(1) as u32,
            (cr.bottom - cr.top).max(1) as u32,
        ),
        view: settings.view,
        background: settings.background,
    });
    // The loader may post the first decoded image from now on.
    loader.set_hwnd(hwnd);
    MAIN_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
    if !args.resident {
        unsafe {
            // Still cloaked, so nothing shows yet; but a shown window gets
            // painted. Not activated: activation costs ~20 ms and waits
            // until an image is on screen.
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
    }
    timing::mark("window_created");

    // Pump messages until the device exists: the first image may arrive
    // first and is then shown through GDI (first_frame).
    let gfx = loop {
        match dev_rx.try_recv() {
            Ok(Ok(g)) => break g,
            Ok(Err(e)) => return fatal(&format!("Direct3D 11: {}", e.message())),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return fatal("Direct3D 11 initialization failed");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        let mut msg = MSG::default();
        unsafe {
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                return 0;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    };
    loader.set_device(gfx.device.clone(), gfx.max_texture_size());
    let renderer = match Renderer::new(&gfx.device) {
        Ok(r) => r,
        Err(e) => return fatal(&e.message()),
    };
    let mut cr = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut cr);
    }
    let client = (
        (cr.right - cr.left).max(1) as u32,
        (cr.bottom - cr.top).max(1) as u32,
    );
    let swap = match SwapChain::new(&gfx, hwnd, client) {
        Ok(s) => s,
        Err(e) => return fatal(&e.message()),
    };
    timing::mark("swapchain_ready");

    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let resident_mode = settings.resident;
    let mut app = App::new(
        hwnd, gfx, renderer, swap, settings, loader, color, dpi, client,
    );
    // The colour target was chosen for this monitor.
    app.monitor = hmonitor.0 as isize;
    // The GDI first frame already made the window visible.
    app.cloaked = !crate::first_frame::shown();
    if args.exit_after_first_frame {
        app.exit_after_first_frame = args
            .json
            .clone()
            .or_else(|| Some(PathBuf::from("startup.json")));
    }
    if args.resident {
        app.hidden = true;
        app.wake_report = args.json.clone();
    }
    let has_file = first.is_some();
    let mut actions = match first {
        Some((path, _)) => app.open(path),
        None => Vec::new(),
    };
    app.update_screen_box();
    app.loader.set_hwnd(hwnd);
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    crate::first_frame::release();
    update::schedule();
    if resident_mode {
        resident::add_tray(hwnd);
        resident::refresh_autostart();
    }
    if args.resident {
        // Nothing to show. This version starts fine (see update::on_start);
        // the hint frame warms the renderer and text pipeline.
        update::on_first_frame();
        // Shown once while cloaked (never on screen): the first present of a
        // window that was never visible costs extra in DWM. As a tool window
        // meanwhile, so the taskbar does not add a button.
        let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_TOOLWINDOW.0 as isize);
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
        #[cfg(feature = "turbojpeg")]
        window::with_app(|a| a.warm_up(cpu.performance));
        run_actions(hwnd, window::with_app(|a| a.render()).unwrap_or_default());
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style);
        }
        blinzel_core::bufpool::global().set_max_retained(0);
        message_loop();
        return finish();
    }

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        // Results that arrived before the window existed.
        let _ = PostMessageW(Some(hwnd), WM_LOADED, WPARAM(0), LPARAM(0));
    }
    if has_file {
        // Never stay invisible: a slow or broken file still shows the window.
        start_uncloak_timer(hwnd, 400);
    } else {
        // Nothing to wait for: draw the hint, then show the window.
        actions.extend(window::with_app(|a| a.render()).unwrap_or_default());
        actions.push(Action::Uncloak);
    }
    run_actions(hwnd, actions);

    message_loop();
    finish()
}

fn finish() -> i32 {
    APP.with(|cell| cell.borrow_mut().take());
    if window::restart_requested() {
        update::start_installed_resident();
    }
    0
}

/// DWM attributes that must be set before the window is shown.
fn prepare_window(hwnd: HWND) {
    let on: i32 = 1;
    unsafe {
        // The open/close animation alone costs 150–250 ms.
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            std::ptr::from_ref(&on).cast(),
            4,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&on).cast(),
            4,
        );
        // A 1 px top margin keeps the DWM shadow and border on all sides.
        let m = MARGINS {
            cxLeftWidth: 0,
            cxRightWidth: 0,
            cyTopHeight: 1,
            cyBottomHeight: 0,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
        DragAcceptFiles(hwnd, true);
    }
    set_cloak(hwnd, true);
    unsafe {
        // Apply our WM_NCCALCSIZE (caption removed).
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn message_loop() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
