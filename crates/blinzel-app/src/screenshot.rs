//! Screenshots: the Print key (in resident mode) or the tray menu freezes
//! the desktop with GDI, shows the picture in a topmost overlay where
//! [`blinzel_ui::snip`] picks a region, a window or a monitor, saves that
//! part as PNG in the screenshot folder and opens it in the viewer. There
//! is no editor: "Edit with…" hands the file on.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blinzel_core::geom::{Orientation, Rect};
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};
use blinzel_i18n::{Msg, tr, trf};
use blinzel_ui::backend::{Canvas, Device};
use blinzel_ui::gfx::Rgba;
use blinzel_ui::snip::{Outcome, Snip};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmFlush, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetMonitorInfoW, HDC,
    HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC, SRCCOPY,
    SelectObject, ValidateRect,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, ReleaseCapture, SetCapture,
    SetFocus, UnregisterHotKey, VK_SHIFT, VK_SNAPSHOT,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Pictures, FOLDERID_Screenshots, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

use crate::backend::{D3dCanvas, D3dDevice};
use crate::gfx::{Renderer, SwapChain};
use crate::platform::{debug_log, wide};
use crate::window::{WM_NOTICE, set_cloak, with_app};

/// A finished screenshot for the main window; LPARAM is a
/// `Box<Result<PathBuf, String>>`.
pub const WM_SCREENSHOT: u32 = WM_APP + 11;

const HOTKEY_PRINT: i32 = 1;
const HOTKEY_CTRL_SHIFT_PRINT: i32 = 2;

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
        let _ = RegisterHotKey(
            Some(hwnd),
            HOTKEY_CTRL_SHIFT_PRINT,
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
            VK_SNAPSHOT.0 as u32,
        );
        RegisterHotKey(Some(hwnd), HOTKEY_PRINT, MOD_NOREPEAT, VK_SNAPSHOT.0 as u32).is_ok()
    }
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
                    SRCCOPY | CAPTUREBLT,
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
        let c = shot.cursor();
        let r = shot
            .monitors
            .iter()
            .find(|m| m.contains(c))
            .copied()
            .unwrap_or(Rect::new(0.0, 0.0, shot.size.0 as f64, shot.size.1 as f64));
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
fn post(main: HWND, result: Result<PathBuf, String>) {
    let ptr = Box::into_raw(Box::new(result)) as isize;
    unsafe {
        if PostMessageW(Some(main), WM_SCREENSHOT, WPARAM(0), LPARAM(ptr)).is_err() {
            drop(Box::from_raw(ptr as *mut Result<PathBuf, String>));
        }
    }
}

/// Text for a failed screenshot, for `WM_NOTICE`.
pub fn failure(reason: &str) -> String {
    trf(Msg::ScreenshotFailed, &[("reason", &reason)])
}

/// Saves part `r` of the picture in the background, then tells `main`.
fn save(main: HWND, shot: &Shot, r: Rect) {
    let (bgra, w, h) = shot.crop(r);
    let folder = folder();
    let raw = main.0 as isize;
    let _ = std::thread::Builder::new()
        .name("blinzel-screenshot".into())
        .spawn(move || {
            let result = (|| {
                std::fs::create_dir_all(&folder)
                    .map_err(|e| format!("{}: {e}", folder.display()))?;
                let png = blinzel_core::export::encode_png_bgra(&bgra, w, h)?;
                let path = free_name(&folder, &file_stem());
                let tmp = path.with_extension("blinzel-tmp");
                std::fs::write(&tmp, &png)
                    .and_then(|()| std::fs::rename(&tmp, &path))
                    .map_err(|e| {
                        let _ = std::fs::remove_file(&tmp);
                        format!("{}: {e}", path.display())
                    })?;
                Ok(path)
            })();
            post(HWND(raw as *mut _), result);
        });
}

/// The folder from the settings, else Windows' screenshot folder
/// (Pictures\Screenshots).
fn folder() -> PathBuf {
    if let Some(d) = with_app(|a| a.settings.screenshot_dir.clone()).flatten() {
        return PathBuf::from(d);
    }
    let known = |id| unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    };
    known(&FOLDERID_Screenshots)
        .or_else(|| known(&FOLDERID_Pictures).map(|p| p.join("Screenshots")))
        .unwrap_or_else(|| std::env::temp_dir().join("Screenshots"))
}

/// "Screenshot 2026-10-26 071530" in the UI language.
fn file_stem() -> String {
    let t = unsafe { GetLocalTime() };
    let date = format!(
        "{:04}-{:02}-{:02} {:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    trf(Msg::ScreenshotName, &[("date", &date)])
}

/// `stem.png`, or `stem (2).png` … if that exists.
fn free_name(folder: &Path, stem: &str) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => folder.join(format!("{stem}.png")),
            n => folder.join(format!("{stem} ({n}).png")),
        })
        .find(|p| !p.exists())
        .expect("a free name")
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
    register_class()?;
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
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.message())?;
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            w!("amigo-blinzel"),
            WS_POPUP,
            shot.origin.0,
            shot.origin.1,
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
        WM_RBUTTONUP => Some(Outcome::Cancel),
        WM_KEYDOWN => {
            let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            match crate::window::key_of(wp.0 as u16) {
                Some(k) => with_snip(|s| s.key(k, shift)),
                None => None,
            }
        }
        // Switching away (Alt+Tab, Win key) cancels.
        WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => Some(Outcome::Cancel),
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
