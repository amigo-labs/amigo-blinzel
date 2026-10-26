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
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
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
    let tip: Vec<u16> = tr(Msg::AppName).encode_utf16().collect();
    let n = tip.len().min(d.szTip.len() - 1);
    d.szTip[..n].copy_from_slice(&tip[..n]);
    d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    d
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

pub fn on_taskbar_created(hwnd: HWND) {
    if TRAY_SHOWN.load(Ordering::Relaxed) {
        add_tray(hwnd);
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
        NIN_SELECT | NIN_KEYSELECT => Some(Action::Wake { opened: false }),
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
    if install::autostart(&layout).as_deref() != Some(&install::autostart_command(&exe)) {
        set_autostart(true);
    }
}
