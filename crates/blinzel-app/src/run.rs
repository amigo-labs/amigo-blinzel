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
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
use windows::Win32::Graphics::Dwm::{
    DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
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
use crate::{bench, instance, settings_store};

#[derive(Debug, Default)]
pub struct Args {
    pub file: Option<PathBuf>,
    pub bench: Option<PathBuf>,
    pub warp: bool,
    pub json: Option<PathBuf>,
    pub exit_after_first_frame: bool,
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
            _ => a.file = Some(PathBuf::from(arg)),
        }
    }
    a
}

/// Microseconds since the process was created (loader, Defender scan and
/// CRT init happened before `main`).
fn process_age_us() -> i64 {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return 0;
        }
        let now = GetSystemTimePreciseAsFileTime();
        let ticks = |f: FILETIME| ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64;
        (ticks(now) - ticks(c)) / 10
    }
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

fn monitor_under_cursor() -> (RECT, RECT) {
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
        (mi.rcMonitor, mi.rcWork)
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
    if !crate::platform::os::is_supported() {
        return fatal(tr(Msg::ErrorOs));
    }

    if let Some(file) = &args.file
        && settings.single_instance
        && !args.exit_after_first_frame
        && instance::hand_off(file)
    {
        return 0;
    }
    timing::mark("instance_checked");

    // Decode workers: P-cores only (at most what the prefetch window can
    // use); the visible image decodes on all P-cores. One I/O thread on
    // rotating disks.
    let cpu = threads::cpu_info();
    let drive = args.file.as_deref().map(io::drive_info).unwrap_or_default();
    let io_threads = if drive.seek_penalty { 1 } else { 2 };
    let cpu_threads = cpu.performance.clamp(2, 6);
    let loader = Loader::new(io_threads, cpu_threads, cpu.performance);
    let (monitor, work) = monitor_under_cursor();
    loader.set_screen_box((
        (monitor.right - monitor.left) as u32,
        (monitor.bottom - monitor.top) as u32,
    ));

    // Start decoding the file before anything else.
    let first = args
        .file
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
        });

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
            WS_EX_NOREDIRECTIONBITMAP,
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
    timing::mark("window_created");

    let gfx = match dev_rx.recv() {
        Ok(Ok(g)) => g,
        Ok(Err(e)) => return fatal(&format!("Direct3D 11: {}", e.message())),
        Err(_) => return fatal("Direct3D 11 initialization failed"),
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
    let mut app = App::new(hwnd, gfx, renderer, swap, settings, loader, dpi, client);
    if args.exit_after_first_frame {
        app.exit_after_first_frame = args
            .json
            .clone()
            .or_else(|| Some(PathBuf::from("startup.json")));
    }
    let has_file = first.is_some();
    let mut actions = match first {
        Some((path, _)) => app.open(path),
        None => Vec::new(),
    };
    app.update_screen_box();
    app.loader.set_hwnd(hwnd);
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
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
    APP.with(|cell| cell.borrow_mut().take());
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
