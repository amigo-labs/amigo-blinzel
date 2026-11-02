//! Resident mode: closing hides the window instead of exiting, with the
//! Direct3D device, shaders and decoder threads kept warm. A tray icon
//! brings the window back or quits; at sign-in the app starts hidden
//! (`--resident`). Opening an image then costs no process start and no
//! device creation: the second launch only hands its file over.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use blinzel_i18n::{Msg, tr};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_LARGE_ICON, NIIF_USER, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_SELECT, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW, LoadImageW,
    RegisterWindowMessageW, SM_CXICON, SM_CXSMICON, WM_APP, WM_CONTEXTMENU,
};
use windows::core::{PCWSTR, w};

use crate::platform::debug_log;
use crate::setup::install::{self, Layout};
use blinzel_ui::app::Action;

/// The tray icon's callback message.
pub const WM_TRAY: u32 = WM_APP + 7;
const TRAY_ID: u32 = 1;
/// `NIN_SELECT | NINF_KEY`: the icon chosen with the keyboard.
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

static TRAY_SHOWN: AtomicBool = AtomicBool::new(false);

/// Explorer broadcasts this after it (re)starts; the tray icon must then be
/// added again.
pub fn taskbar_created() -> u32 {
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// The application icon (resource 1) at the system's small or large icon
/// size; Windows' default icon if the build has no resources.
pub fn app_icon(small: bool) -> HICON {
    static ICONS: OnceLock<[isize; 2]> = OnceLock::new();
    let icons = ICONS.get_or_init(|| {
        let load = |metric| unsafe {
            let size = GetSystemMetricsForDpi(metric, GetDpiForSystem());
            let module = GetModuleHandleW(None).ok();
            LoadImageW(
                module.map(Into::into),
                // MAKEINTRESOURCE(1)
                PCWSTR(std::ptr::without_provenance(1)),
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
            .map(|h| h.0 as isize)
            .or_else(|_| LoadIconW(None, IDI_APPLICATION).map(|h| h.0 as isize))
            .unwrap_or(0)
        };
        [load(SM_CXSMICON), load(SM_CXICON)]
    });
    HICON(icons[usize::from(!small)] as *mut _)
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: app_icon(true),
        ..Default::default()
    };
    copy_wide(&mut d.szTip, tr(Msg::AppName));
    d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    d
}

/// `s` into a fixed, NUL-terminated field, cut to fit.
fn copy_wide(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().collect();
    let n = w.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&w[..n]);
}

/// A balloon on the tray icon (a notification on Windows 10 and 11);
/// clicking it shows the window.
pub fn balloon(hwnd: HWND, text: &str) {
    if !TRAY_SHOWN.load(Ordering::Relaxed) {
        return;
    }
    let mut d = tray_data(hwnd);
    d.uFlags = NIF_INFO;
    copy_wide(&mut d.szInfoTitle, tr(Msg::AppName));
    copy_wide(&mut d.szInfo, text);
    d.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON;
    d.hBalloonIcon = app_icon(false);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
    }
}

/// Shows the tray icon (again, after an Explorer restart).
pub fn add_tray(hwnd: HWND) {
    let d = tray_data(hwnd);
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &d).as_bool() {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
    }
    TRAY_SHOWN.store(true, Ordering::Relaxed);
}

pub fn remove_tray(hwnd: HWND) {
    if TRAY_SHOWN.swap(false, Ordering::Relaxed) {
        let d = tray_data(hwnd);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &d);
        }
    }
}

/// Explorer restarted: the icon again, if it was shown.
pub fn on_taskbar_created(hwnd: HWND) {
    readd_tray(TRAY_SHOWN.load(Ordering::Relaxed), hwnd, add_tray);
}

/// After `TaskbarCreated`, `add` the icon again where it was `shown`.
fn readd_tray(shown: bool, hwnd: HWND, add: impl FnOnce(HWND)) {
    if shown {
        add(hwnd);
    }
}

/// Resident mode on or off, at once: the tray icon and the sign-in entry
/// (`sign_in`: [`set_autostart`]; a test passes its own).
pub fn set_resident(hwnd: HWND, on: bool, sign_in: impl FnOnce(bool)) {
    sign_in(on);
    if on {
        add_tray(hwnd);
    } else {
        remove_tray(hwnd);
    }
}

/// A tray icon event (NOTIFYICON_VERSION_4: the event in the low word of
/// `lp`, the anchor point in `wp`).
pub fn on_tray(wp: WPARAM, lp: LPARAM) -> Option<Action> {
    let event = (lp.0 & 0xFFFF) as u32;
    let (x, y) = (
        (wp.0 & 0xFFFF) as i16 as i32,
        ((wp.0 >> 16) & 0xFFFF) as i16 as i32,
    );
    match event {
        NIN_SELECT | NIN_KEYSELECT | NIN_BALLOONUSERCLICK => Some(Action::Wake { opened: false }),
        WM_CONTEXTMENU => Some(Action::TrayMenu { x, y }),
        _ => None,
    }
}

/// Starts the app hidden at sign-in, or no longer.
pub fn set_autostart(on: bool) {
    let Some(layout) = Layout::user() else {
        return;
    };
    let exe = std::env::current_exe().ok();
    let r = install::set_autostart(&layout, exe.as_deref().filter(|_| on));
    if let Err(e) = r {
        debug_log(&format!("autostart: {e}"));
    }
}

/// Keeps the sign-in entry pointing at this executable (moved or
/// reinstalled elsewhere).
pub fn refresh_autostart() {
    let Some(layout) = Layout::user() else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Err(e) = install::refresh_autostart(&layout, &exe) {
        debug_log(&format!("autostart: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE;

    /// `docs/specs/resident-mode.md` AC-02 (the Windows shell): turning
    /// resident mode on adds the tray icon and writes the sign-in entry,
    /// off removes both (the entry here is a fake, never the real value).
    #[test]
    fn toggling_resident_mode_adds_or_removes_the_icon_and_the_sign_in_entry() {
        let mut sign_in = vec![];
        set_resident(HWND::default(), true, |on| sign_in.push(on));
        assert!(TRAY_SHOWN.load(Ordering::Relaxed), "icon shown");
        set_resident(HWND::default(), false, |on| sign_in.push(on));
        assert!(!TRAY_SHOWN.load(Ordering::Relaxed), "icon removed");
        assert_eq!(sign_in, [true, false]);
    }

    /// `docs/specs/resident-mode.md` AC-13: Explorer's `TaskbarCreated`
    /// broadcast adds a shown icon again, and only a shown one.
    #[test]
    fn the_icon_is_added_again_after_explorer_restarts() {
        let id = taskbar_created();
        assert!(
            (0xC000..=0xFFFF).contains(&id),
            "a registered message: {id:#x}"
        );
        // SAFETY: a static wide string; registering again returns the same id.
        assert_eq!(id, unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });
        let mut added = 0;
        readd_tray(false, HWND::default(), |_| added += 1);
        assert_eq!(added, 0, "no icon while not resident");
        readd_tray(true, HWND::default(), |_| added += 1);
        assert_eq!(added, 1);
    }

    /// `docs/specs/resident-mode.md` AC-13: the icon has the app name as
    /// tooltip and reports to `WM_TRAY` in the version 4 format; a click,
    /// the keyboard or its balloon show the window, a right click opens
    /// the tray menu where it happened.
    #[test]
    fn the_tray_icon_shows_the_window_or_opens_its_menu() {
        let d = tray_data(HWND::default());
        let end = d.szTip.iter().position(|&c| c == 0).unwrap();
        assert_eq!(String::from_utf16_lossy(&d.szTip[..end]), tr(Msg::AppName));
        assert_eq!(d.uCallbackMessage, WM_TRAY);
        assert!(
            d.uFlags
                .contains(NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP)
        );
        // SAFETY: `tray_data` wrote the union's `uVersion` field.
        assert_eq!(unsafe { d.Anonymous.uVersion }, NOTIFYICON_VERSION_4);

        let at = |x: i16, y: i16| WPARAM(x as u16 as usize | (y as u16 as usize) << 16);
        let event = |e: u32| LPARAM(e as isize);
        for e in [NIN_SELECT, NIN_KEYSELECT, NIN_BALLOONUSERCLICK] {
            let a = on_tray(at(5, 5), event(e));
            assert_eq!(a, Some(Action::Wake { opened: false }), "{e:#x}");
        }
        let menu = on_tray(at(-20, 1060), event(WM_CONTEXTMENU));
        assert_eq!(menu, Some(Action::TrayMenu { x: -20, y: 1060 }));
        assert_eq!(on_tray(at(5, 5), event(WM_MOUSEMOVE)), None);
    }
}
