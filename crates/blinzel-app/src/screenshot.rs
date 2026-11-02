//! Screenshots: the Print key (in resident mode) or the tray menu freezes
//! the desktop with GDI, shows the picture in a topmost overlay where
//! [`blinzel_ui::snip`] picks a region, a window or a monitor. That part
//! goes to the clipboard, and as the settings say into the screenshot
//! folder, the viewer and the editor ([`blinzel_core::screenshot`]). There
//! is no editor of our own: "Edit with…" hands the file on.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use blinzel_core::geom::{Orientation, Rect};
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};
use blinzel_core::screenshot as core_shot;
use blinzel_core::settings::Settings;
use blinzel_i18n::{Lang, Msg, fill, tr, tr_in, trf};
use blinzel_ui::app::Action;
use blinzel_ui::backend::{Canvas, Device};
use blinzel_ui::gfx::Rgba;
use blinzel_ui::snip::{self, Outcome, Snip};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmFlush, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetMonitorInfoW, HDC,
    HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ROP_CODE, ReleaseDC,
    SRCCOPY, SelectObject, ValidateRect,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
    ReleaseCapture, SetCapture, SetFocus, UnregisterHotKey, VK_SHIFT, VK_SNAPSHOT,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Pictures, FOLDERID_Screenshots, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, GUID, PCWSTR, w};

use crate::backend::{D3dCanvas, D3dDevice};
use crate::gfx::{Renderer, SwapChain};
use crate::platform::{debug_log, wide};
use crate::window::{WM_NOTICE, set_cloak, with_app};

/// A finished screenshot for the main window; LPARAM is a
/// `Box<Result<Taken, String>>`.
pub const WM_SCREENSHOT: u32 = WM_APP + 11;

/// The picked part of a screenshot, for the clipboard, and the file it
/// went into (none when the settings ask for none).
pub struct Taken {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
    pub file: Result<Option<PathBuf>, String>,
}

const HOTKEY_PRINT: i32 = 1;
const HOTKEY_CTRL_SHIFT_PRINT: i32 = 2;

/// Windows kept the Print key at start; the window says so when it next
/// shows.
static HOTKEY_HINT: AtomicBool = AtomicBool::new(false);

/// Registers (or drops) the screenshot keys on `hwnd`: Print, and
/// Ctrl+Shift+Print, which Windows never takes. Returns whether Print
/// itself was free (Windows 11 gives it to the Snipping Tool by default).
pub fn set_hotkey(hwnd: HWND, on: bool) -> bool {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_PRINT);
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_CTRL_SHIFT_PRINT);
        if !on {
            return true;
        }
        let [(ctrl_shift, ctrl_shift_mods), (print, print_mods)] = hotkeys();
        let _ = RegisterHotKey(
            Some(hwnd),
            ctrl_shift,
            ctrl_shift_mods,
            VK_SNAPSHOT.0 as u32,
        );
        RegisterHotKey(Some(hwnd), print, print_mods, VK_SNAPSHOT.0 as u32).is_ok()
    }
}

/// The screenshot keys on `VK_SNAPSHOT`, by id: Ctrl+Shift+Print, then
/// Print.
fn hotkeys() -> [(i32, HOT_KEY_MODIFIERS); 2] {
    [
        (
            HOTKEY_CTRL_SHIFT_PRINT,
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
        ),
        (HOTKEY_PRINT, MOD_NOREPEAT),
    ]
}

/// Whether the screenshot keys belong registered: resident mode and the
/// key setting both on.
pub fn hotkey_wanted(settings: &Settings) -> bool {
    settings.resident && settings.screenshot_hotkey
}

/// What one of our keys does, with the window shown or hidden.
pub const ON_HOTKEY: Action = Action::Capture {
    whole_screen: false,
};

/// How often, and how far apart, Print is tried again at start (a minute).
pub const RETRY_TRIES: u32 = 12;
pub const RETRY_MS: u32 = 5000;

/// What the retry timer does after a try.
#[derive(Debug, PartialEq, Eq)]
pub enum Retry {
    /// Print is still taken; try again.
    Again,
    /// Print is ours, or no longer wanted.
    Stop,
    /// The last try failed: say that Windows keeps the key.
    GiveUp,
}

/// One retry of the Print key with `left` tries left after it; `free`
/// registers it and says whether that worked.
pub fn retry(wanted: bool, free: impl FnOnce() -> bool, left: u32) -> Retry {
    if !wanted || free() {
        Retry::Stop
    } else if left == 0 {
        Retry::GiveUp
    } else {
        Retry::Again
    }
}

/// How long the [`hotkey_taken`] notice stays in the window.
pub const HINT_MS: u32 = 9000;

/// Where the [`hotkey_taken`] notice goes after [`Retry::GiveUp`].
#[derive(Debug, PartialEq, Eq)]
pub enum HintRoute {
    /// The window is hidden: a tray balloon with this text, and the notice
    /// again in the window at the next wake (a notification may be off).
    Balloon(String),
    /// In the window for this many milliseconds.
    Window(u32),
}

/// Routes the notice from `told`, `App::tell`'s answer (the text back
/// while the window is hidden); a balloon also marks it for the next wake.
pub fn route_hint(told: Option<String>) -> HintRoute {
    match told {
        Some(text) => {
            hint_pending(true);
            HintRoute::Balloon(text)
        }
        None => HintRoute::Window(HINT_MS),
    }
}

/// Marks (or clears) the [`hotkey_taken`] notice for the next wake.
pub fn hint_pending(on: bool) {
    HOTKEY_HINT.store(on, Ordering::Relaxed);
}

/// Whether the [`hotkey_taken`] notice is due; it is shown once.
pub fn take_hint() -> bool {
    HOTKEY_HINT.swap(false, Ordering::Relaxed)
}

/// Whether a `WM_HOTKEY` is ours.
pub fn is_hotkey(wp: WPARAM) -> bool {
    matches!(wp.0 as i32, HOTKEY_PRINT | HOTKEY_CTRL_SHIFT_PRINT)
}

/// Lets a just-closed menu fade out before the screen is taken.
pub fn let_menu_fade() {
    unsafe {
        let _ = DwmFlush();
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
}

/// The frozen desktop: BGRA pixels of the virtual screen, with monitors
/// and windows in its coordinates.
struct Shot {
    bgra: Vec<u8>,
    size: (u32, u32),
    origin: (i32, i32),
    monitors: Vec<Rect>,
    windows: Vec<Rect>,
}

impl Shot {
    /// The pixels of `r` (whole pixels inside the picture), tight rows.
    fn crop(&self, r: Rect) -> (Vec<u8>, u32, u32) {
        let (x0, y0) = (r.x.max(0.0) as usize, r.y.max(0.0) as usize);
        let w = (r.w as usize).min(self.size.0 as usize - x0);
        let h = (r.h as usize).min(self.size.1 as usize - y0);
        let stride = self.size.0 as usize * 4;
        let mut out = Vec::with_capacity(w * h * 4);
        for y in y0..y0 + h {
            let row = y * stride + x0 * 4;
            out.extend_from_slice(&self.bgra[row..row + w * 4]);
        }
        (out, w as u32, h as u32)
    }

    fn cursor(&self) -> (f64, f64) {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        ((p.x - self.origin.0) as f64, (p.y - self.origin.1) as f64)
    }
}

fn virtual_screen() -> RECT {
    unsafe {
        let (x, y) = (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        );
        RECT {
            left: x,
            top: y,
            right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn to_rect(r: &RECT, origin: (i32, i32)) -> Rect {
    Rect::new(
        (r.left - origin.0) as f64,
        (r.top - origin.1) as f64,
        (r.right - r.left) as f64,
        (r.bottom - r.top) as f64,
    )
}

/// A plain copy that includes layered windows.
fn capture_rop() -> ROP_CODE {
    SRCCOPY | CAPTUREBLT
}

/// Copies the whole virtual screen (layered windows included).
fn grab() -> Result<Shot, String> {
    let vs = virtual_screen();
    let (w, h) = (vs.right - vs.left, vs.bottom - vs.top);
    if w <= 0 || h <= 0 {
        return Err("no screen".into());
    }
    let origin = (vs.left, vs.top);
    // Windows first: the list must not change while the picture is taken.
    let windows = windows(origin);
    let monitors = monitors(origin);
    let mut bgra = vec![0u8; w as usize * h as usize * 4];
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| e.message())
            .and_then(|bmp| {
                let old = SelectObject(mem, bmp.into());
                let r = BitBlt(
                    mem,
                    0,
                    0,
                    w,
                    h,
                    Some(screen),
                    vs.left,
                    vs.top,
                    capture_rop(),
                )
                .map_err(|e| e.message());
                if r.is_ok() {
                    // SAFETY: the section holds w × h × 4 bytes.
                    std::ptr::copy_nonoverlapping(bits.cast::<u8>(), bgra.as_mut_ptr(), bgra.len());
                }
                SelectObject(mem, old);
                let _ = DeleteObject(bmp.into());
                r
            });
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result?;
    }
    // GDI leaves alpha undefined.
    for p in bgra.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    Ok(Shot {
        bgra,
        size: (w as u32, h as u32),
        origin,
        monitors,
        windows,
    })
}

fn monitors(origin: (i32, i32)) -> Vec<Rect> {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Vec<RECT>) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(m, &mut info) }.as_bool() {
            out.push(info.rcMonitor);
        }
        BOOL(1)
    }
    let mut rects: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(each),
            LPARAM(std::ptr::from_mut(&mut rects) as isize),
        );
    }
    rects.iter().map(|r| to_rect(r, origin)).collect()
}

/// Visible top-level windows front to back (the desktop itself, cloaked
/// and minimized windows left out), by their visible frame.
fn windows(origin: (i32, i32)) -> Vec<Rect> {
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Vec<RECT>) };
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return BOOL(1);
            }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                std::ptr::from_mut(&mut cloaked).cast(),
                4,
            );
            if cloaked != 0 {
                return BOOL(1);
            }
            let mut class = [0u16; 32];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            if class == "Progman" || class == "WorkerW" {
                return BOOL(1);
            }
            let mut r = RECT::default();
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                std::ptr::from_mut(&mut r).cast(),
                size_of::<RECT>() as u32,
            )
            .is_err()
                && GetWindowRect(hwnd, &mut r).is_err()
            {
                return BOOL(1);
            }
            if r.right - r.left >= 8 && r.bottom - r.top >= 8 {
                out.push(r);
            }
        }
        BOOL(1)
    }
    let mut rects: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(std::ptr::from_mut(&mut rects) as isize));
    }
    rects.iter().map(|r| to_rect(r, origin)).collect()
}

/// Takes a screenshot: the monitor under the pointer at once, or the
/// overlay to pick one. Errors become a notice on the viewer.
pub fn start(main: HWND, whole_screen: bool) {
    let shot = match grab() {
        Ok(s) => s,
        Err(e) => return post(main, Err(e)),
    };
    if whole_screen {
        let r = snip::monitor_under(&shot.monitors, shot.cursor()).map_or(
            Rect::new(0.0, 0.0, shot.size.0 as f64, shot.size.1 as f64),
            |i| shot.monitors[i],
        );
        return save(main, &shot, r);
    }
    if let Err(e) = open_overlay(main, shot) {
        debug_log(&format!("screenshot overlay: {e}"));
        post(main, Err(e));
    }
}

/// The whole desktop into `out` as PNG, at once (CI): its size.
pub fn capture_test(out: &Path) -> Result<(u32, u32), String> {
    let shot = grab()?;
    let (w, h) = shot.size;
    if shot.monitors.is_empty() {
        return Err("no monitor".into());
    }
    let png = blinzel_core::export::encode_png_bgra(&shot.bgra, w, h)?;
    std::fs::write(out, png).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok((w, h))
}

/// Hands the result to the main window (or a message for a failure).
fn post(main: HWND, result: Result<Taken, String>) {
    let ptr = Box::into_raw(Box::new(result)) as isize;
    unsafe {
        if PostMessageW(Some(main), WM_SCREENSHOT, WPARAM(0), LPARAM(ptr)).is_err() {
            drop(Box::from_raw(ptr as *mut Result<Taken, String>));
        }
    }
}

/// Text for a failed screenshot, for `WM_NOTICE`.
pub fn failure(reason: &str) -> String {
    trf(Msg::ScreenshotFailed, &[("reason", &reason)])
}

/// Encodes part `r` of the picture and writes the file the settings ask
/// for in the background, then tells `main`.
fn save(main: HWND, shot: &Shot, r: Rect) {
    let (bgra, w, h) = shot.crop(r);
    let settings = with_app(|a| a.settings.clone()).unwrap_or_default();
    let cache =
        core_shot::cache_folder(&blinzel_ui::dirs::cache().unwrap_or_else(std::env::temp_dir));
    let folder = folder(&settings);
    let raw = main.0 as isize;
    let _ = std::thread::Builder::new()
        .name("blinzel-screenshot".into())
        .spawn(move || {
            let result = blinzel_core::export::encode_png_bgra(&bgra, w, h).map(|png| {
                let file = core_shot::store(&settings, &folder, &cache, &file_stem(), &png);
                Taken {
                    bgra,
                    width: w,
                    height: h,
                    png,
                    file,
                }
            });
            post(HWND(raw as *mut _), result);
        });
}

/// The folder from the settings, else Windows' screenshot folder.
fn folder(settings: &Settings) -> PathBuf {
    folder_in(settings, |id| unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    })
}

/// The folder from the settings, else the Screenshots known folder, else
/// Pictures\Screenshots, as `known` finds them.
fn folder_in(settings: &Settings, known: impl Fn(&GUID) -> Option<PathBuf>) -> PathBuf {
    if let Some(d) = &settings.screenshot_dir {
        return PathBuf::from(d);
    }
    known(&FOLDERID_Screenshots)
        .or_else(|| known(&FOLDERID_Pictures).map(|p| p.join("Screenshots")))
        .unwrap_or_else(|| std::env::temp_dir().join("Screenshots"))
}

fn file_stem() -> String {
    file_stem_at(blinzel_i18n::lang(), &unsafe { GetLocalTime() })
}

/// "Screenshot 2026-10-26 071530" in `lang`, for the local time `t`.
fn file_stem_at(lang: Lang, t: &SYSTEMTIME) -> String {
    let date = format!(
        "{:04}-{:02}-{:02} {:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    fill(tr_in(lang, Msg::ScreenshotName), &[("date", &date)])
}

// ------------------------------------------------------------------ overlay

struct Overlay {
    main: HWND,
    hwnd: HWND,
    canvas: D3dCanvas,
    picture: <D3dDevice as Device>::Image,
    snip: Snip,
    shot: Shot,
    scale: f64,
}

thread_local! {
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

const CLASS: PCWSTR = w!("BlinzelSnip");

fn register_class() -> Result<(), String> {
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let ok = *REGISTERED.get_or_init(|| unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return false;
        };
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc) != 0
    });
    if ok {
        Ok(())
    } else {
        Err("cannot register the overlay window class".into())
    }
}

/// Shows the frozen picture over every monitor; it stays cloaked until
/// its first frame is on screen, so nothing flickers.
fn open_overlay(main: HWND, shot: Shot) -> Result<(), String> {
    close_overlay();
    let (gfx, device) =
        with_app(|a| (a.canvas.gfx.clone(), Arc::clone(&a.device))).ok_or("viewer busy")?;
    let (w, h) = shot.size;
    let mut plane = Plane::new(w, h, 4);
    plane.data.copy_from_slice(&shot.bgra);
    let image = Image {
        pixels: Pixels::Packed {
            plane,
            format: PackedFormat::Bgra8,
            alpha: Alpha::Opaque,
        },
        width: w,
        height: h,
        full_width: w,
        full_height: h,
        region: None,
    };
    let picture = device.upload(&image, Orientation::NORMAL)?;
    drop(image);
    let cursor = shot.cursor();
    let scale = unsafe {
        let m = MonitorFromPoint(
            POINT {
                x: cursor.0 as i32 + shot.origin.0,
                y: cursor.1 as i32 + shot.origin.1,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        dx.max(96) as f64 / 96.0
    };
    let hwnd = overlay_window(shot.origin, shot.size)?;
    let swap = match SwapChain::new(&gfx, hwnd, (w, h)) {
        Ok(s) => s,
        Err(e) => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err(e.message());
        }
    };
    let renderer = Renderer::new(&gfx.device).map_err(|e| e.message())?;
    let snip = Snip::new(
        shot.size,
        shot.monitors.clone(),
        shot.windows.clone(),
        cursor,
    );
    OVERLAY.with(|cell| {
        *cell.borrow_mut() = Some(Overlay {
            main,
            hwnd,
            canvas: D3dCanvas::new(gfx, renderer, swap),
            picture,
            snip,
            shot,
            scale,
        })
    });
    render();
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        let _ = DwmFlush();
    }
    set_cloak(hwnd, false);
    Ok(())
}

/// The overlay's window over the virtual screen at `origin`, 1:1:
/// topmost, without a taskbar button, cloaked until its first frame.
fn overlay_window(origin: (i32, i32), (w, h): (u32, u32)) -> Result<HWND, String> {
    register_class()?;
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.message())?;
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            w!("amigo-blinzel"),
            WS_POPUP,
            origin.0,
            origin.1,
            w as i32,
            h as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.message())?
    };
    set_cloak(hwnd, true);
    Ok(hwnd)
}

fn render() {
    OVERLAY.with(|cell| {
        let Ok(mut g) = cell.try_borrow_mut() else {
            return;
        };
        let Some(o) = g.as_mut() else { return };
        if o.canvas.begin(Rgba(0.0, 0.0, 0.0, 1.0)) {
            o.snip.draw(&mut o.canvas, &o.picture, o.scale);
            o.canvas.present();
        }
        unsafe {
            let _ = ValidateRect(Some(o.hwnd), None);
        }
    });
}

/// Closes the overlay, if open; returns it (for its picture).
fn close_overlay() -> Option<Overlay> {
    let o = OVERLAY.with(|cell| cell.try_borrow_mut().ok()?.take())?;
    unsafe {
        let _ = ReleaseCapture();
        let _ = DestroyWindow(o.hwnd);
    }
    Some(o)
}

fn outcome(o: Outcome) {
    match o {
        Outcome::Unchanged => {}
        Outcome::Redraw => render(),
        Outcome::Cancel => {
            close_overlay();
        }
        Outcome::Done(r) => {
            if let Some(o) = close_overlay() {
                save(o.main, &o.shot, r);
            }
        }
    }
}

/// Feeds the overlay; `None` if it is not open (or busy).
fn with_snip(f: impl FnOnce(&mut Snip) -> Outcome) -> Option<Outcome> {
    OVERLAY.with(|cell| Some(f(&mut cell.try_borrow_mut().ok()?.as_mut()?.snip)))
}

fn point(lp: LPARAM) -> (f64, f64) {
    (
        (lp.0 & 0xFFFF) as i16 as f64,
        ((lp.0 >> 16) & 0xFFFF) as i16 as f64,
    )
}

/// Whether `msg` closes the overlay without a shot: a right click, or
/// losing the foreground (Alt+Tab, the Win key, another app).
fn cancels(msg: u32, wp: WPARAM) -> bool {
    msg == WM_RBUTTONUP || (msg == WM_ACTIVATE && (wp.0 & 0xFFFF) as u32 == WA_INACTIVE)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let o = match msg {
        WM_PAINT => {
            render();
            return LRESULT(0);
        }
        WM_MOUSEMOVE => with_snip(|s| s.mouse_move(point(lp))),
        WM_LBUTTONDOWN => {
            unsafe {
                SetCapture(hwnd);
            }
            with_snip(|s| s.mouse_down(point(lp)))
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            with_snip(|s| s.mouse_up(point(lp)))
        }
        m if cancels(m, wp) => Some(Outcome::Cancel),
        WM_KEYDOWN => {
            let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            match crate::window::key_of(wp.0 as u16) {
                Some(k) => with_snip(|s| s.key(k, shift)),
                None => None,
            }
        }
        // It spans monitors with different scaling: stay as created.
        WM_DPICHANGED => return LRESULT(0),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    };
    if let Some(o) = o {
        outcome(o);
    }
    LRESULT(0)
}

/// Opens the system's "Open with" dialog for `file`.
pub fn open_with(hwnd: HWND, file: &Path) {
    use windows::Win32::UI::Shell::{
        OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OPENASINFO, SHOpenWithDialog,
    };
    let f = wide(file);
    let info = OPENASINFO {
        pcszFile: PCWSTR(f.as_ptr()),
        pcszClass: PCWSTR::null(),
        oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_EXEC,
    };
    unsafe {
        let _ = SHOpenWithDialog(Some(hwnd), &info);
    }
}

/// Starts `editor` with `file`; a failure becomes a notice.
pub fn edit_with(hwnd: HWND, editor: &Path, file: &Path) {
    if let Err(e) = std::process::Command::new(editor).arg(file).spawn() {
        let text = trf(Msg::EditorFailed, &[("reason", &e.to_string())]);
        notice(hwnd, text);
    }
}

/// A toast on the viewer.
pub fn notice(hwnd: HWND, text: String) {
    let ptr = Box::into_raw(Box::new(text)) as isize;
    unsafe {
        if PostMessageW(Some(hwnd), WM_NOTICE, WPARAM(0), LPARAM(ptr)).is_err() {
            drop(Box::from_raw(ptr as *mut String));
        }
    }
}

/// The notice when Windows keeps the Print key.
pub fn hotkey_taken() -> String {
    tr(Msg::HotkeyTaken).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-02: Print and Ctrl+Shift+Print (never repeating) open the
    /// overlay, shown or hidden, where resident mode and the key are on.
    #[test]
    fn print_and_ctrl_shift_print_open_the_overlay_in_resident_mode() {
        assert_eq!(
            hotkeys(),
            [
                (2, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT),
                (1, MOD_NOREPEAT)
            ]
        );
        for (id, _) in hotkeys() {
            assert!(is_hotkey(WPARAM(id as usize)));
        }
        assert!(!is_hotkey(WPARAM(3)));
        assert_eq!(
            ON_HOTKEY,
            Action::Capture {
                whole_screen: false
            }
        );
        for (resident, key) in [(true, true), (true, false), (false, true), (false, false)] {
            let s = Settings {
                resident,
                screenshot_hotkey: key,
                ..Settings::default()
            };
            assert_eq!(hotkey_wanted(&s), resident && key);
        }
    }

    /// AC-03: Print is tried every 5 s for a minute, then the notice is
    /// due once; a try that gets it, or the setting turned off, stops.
    #[test]
    fn a_taken_print_key_is_retried_for_a_minute_then_reported() {
        assert_eq!(RETRY_TRIES * RETRY_MS, 60_000);
        assert_eq!(RETRY_MS, 5000);
        let mut left = RETRY_TRIES;
        let mut tries = vec![];
        loop {
            left -= 1;
            let r = retry(true, || false, left);
            tries.push(left);
            if r != Retry::Again {
                assert_eq!(r, Retry::GiveUp);
                break;
            }
        }
        assert_eq!(tries.len() as u32, RETRY_TRIES);
        assert_eq!(retry(true, || true, 0), Retry::Stop, "the last try got it");
        assert_eq!(retry(true, || true, 5), Retry::Stop);
        assert_eq!(retry(false, || panic!("not tried"), 0), Retry::Stop);
    }

    /// AC-03: while hidden the notice is a balloon and due once at the
    /// next wake; otherwise it shows 9 s in the window.
    #[test]
    fn the_print_key_notice_is_a_balloon_while_hidden_and_9_s_in_the_window() {
        hint_pending(false);
        assert_eq!(route_hint(None), HintRoute::Window(9000));
        assert!(!take_hint(), "nothing due for the next wake");
        let text = hotkey_taken();
        assert_eq!(route_hint(Some(text.clone())), HintRoute::Balloon(text));
        assert!(take_hint(), "due at the next wake");
        assert!(!take_hint(), "once");
    }

    /// AC-07: the whole virtual desktop is frozen (layered windows
    /// included) and the overlay covers it 1:1, topmost, without a taskbar
    /// button, cloaked until its first frame.
    #[test]
    fn a_screenshot_freezes_the_virtual_desktop_under_a_cloaked_overlay() {
        assert_eq!(capture_rop(), SRCCOPY | CAPTUREBLT);
        let vs = virtual_screen();
        let shot = grab().expect("the CI machine has a desktop");
        let size = ((vs.right - vs.left) as u32, (vs.bottom - vs.top) as u32);
        assert_eq!((shot.size, shot.origin), (size, (vs.left, vs.top)));
        assert_eq!(shot.bgra.len(), size.0 as usize * size.1 as usize * 4);
        assert!(!shot.monitors.is_empty());
        let hwnd = overlay_window(shot.origin, shot.size).unwrap();
        let mut r = RECT::default();
        let mut cloaked = 0u32;
        unsafe {
            GetWindowRect(hwnd, &mut r).unwrap();
            DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4).unwrap();
        }
        assert_eq!(
            (r.left, r.top, r.right, r.bottom),
            (vs.left, vs.top, vs.right, vs.bottom)
        );
        assert_ne!(cloaked, 0, "cloaked");
        let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        let want = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP;
        assert_eq!(ex & want.0, want.0);
        unsafe { DestroyWindow(hwnd).unwrap() };
    }

    /// AC-14: a right click or losing the foreground closes the overlay
    /// without a shot; becoming active or a left click does not.
    #[test]
    fn a_right_click_or_losing_the_foreground_cancels_the_overlay() {
        assert!(cancels(WM_RBUTTONUP, WPARAM(0)));
        assert!(cancels(WM_ACTIVATE, WPARAM(WA_INACTIVE as usize)));
        // Minimized while losing it: the high word is set.
        assert!(cancels(WM_ACTIVATE, WPARAM(0x1_0000)));
        assert!(!cancels(WM_ACTIVATE, WPARAM(WA_ACTIVE as usize)));
        assert!(!cancels(WM_ACTIVATE, WPARAM(WA_CLICKACTIVE as usize)));
        assert!(!cancels(WM_LBUTTONUP, WPARAM(0)));
    }

    /// AC-18: the folder chosen in the menu, else the Screenshots known
    /// folder, else Pictures\Screenshots; this machine's folder is a real
    /// one.
    #[test]
    fn the_folder_is_the_chosen_one_else_the_systems() {
        let chosen = Settings {
            screenshot_dir: Some("D:\\Shots".into()),
            ..Settings::default()
        };
        assert_eq!(
            folder_in(&chosen, |_| panic!("not asked")),
            Path::new("D:\\Shots")
        );
        let both = |id: &GUID| {
            Some(PathBuf::from(if *id == FOLDERID_Screenshots {
                "C:\\S"
            } else {
                "C:\\P"
            }))
        };
        let none = Settings::default();
        assert_eq!(folder_in(&none, both), Path::new("C:\\S"));
        let pictures = |id: &GUID| (*id == FOLDERID_Pictures).then(|| PathBuf::from("C:\\P"));
        assert_eq!(folder_in(&none, pictures), Path::new("C:\\P\\Screenshots"));
        let real = folder(&none);
        assert!(real.is_absolute(), "{real:?}");
        assert!(real.ends_with("Screenshots"), "{real:?}");
    }

    /// AC-18: the file is named after the local time in the UI language.
    #[test]
    fn the_file_is_named_after_the_time_in_the_ui_language() {
        let t = SYSTEMTIME {
            wYear: 2026,
            wMonth: 1,
            wDay: 9,
            wHour: 7,
            wMinute: 5,
            wSecond: 3,
            ..Default::default()
        };
        assert_eq!(file_stem_at(Lang::En, &t), "Screenshot 2026-01-09 070503");
        assert_eq!(
            file_stem_at(Lang::De, &t),
            "Bildschirmfoto 2026-01-09 070503"
        );
    }
}
