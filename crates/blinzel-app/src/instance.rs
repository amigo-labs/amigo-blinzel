//! Single instance: a second launch hands its file to the running window
//! and exits before doing anything expensive.

use std::path::Path;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::{PCWSTR, w};

use crate::platform::wide;

pub const WINDOW_CLASS: PCWSTR = w!("AmigoBlinzelViewer");
/// `dwData` tag of our WM_COPYDATA ("blinzel open").
pub const COPYDATA_OPEN: usize = 0xB1_1E;

/// Sends `path` to a running instance. True if it took over.
pub fn hand_off(path: &Path) -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(WINDOW_CLASS, PCWSTR::null()) else {
            return false;
        };
        if hwnd.is_invalid() {
            return false;
        }
        // Without this the running window may only flash in the taskbar.
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let _ = AllowSetForegroundWindow(pid);
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let data = wide(abs.as_os_str());
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_OPEN,
            cbData: (data.len() * 2) as u32,
            lpData: data.as_ptr() as *mut _,
        };
        let mut result = 0usize;
        let r = SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut result),
        );
        r.0 != 0 && result == 1
    }
}

/// Decodes a path received via WM_COPYDATA.
pub fn received_path(cds: &COPYDATASTRUCT) -> Option<std::path::PathBuf> {
    if cds.dwData != COPYDATA_OPEN || cds.lpData.is_null() {
        return None;
    }
    let units =
        unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) };
    let s = crate::platform::from_wide(units);
    (!s.is_empty()).then(|| std::path::PathBuf::from(s))
}
