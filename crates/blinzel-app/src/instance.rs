//! Single instance: a second launch hands its file (or, for a resident
//! instance, a "show" request) to the running window and exits before
//! doing anything expensive.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowExW, GetWindowThreadProcessId, IsWindowVisible,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::{PCWSTR, w};

use crate::platform::wide;

pub const WINDOW_CLASS: PCWSTR = w!("AmigoBlinzelViewer");
/// `dwData` tags of our WM_COPYDATA ("blinzel open / show / quit").
pub const COPYDATA_OPEN: usize = 0xB1_1E;
pub const COPYDATA_SHOW: usize = 0xB1_1F;
pub const COPYDATA_QUIT: usize = 0xB1_20;

/// What another process asks the running instance to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Open {
        path: PathBuf,
        /// When the sending process was created (FILETIME ticks): the
        /// origin of the hand-off's timing marks.
        sender_created: Option<i64>,
    },
    /// Show the window (a resident instance may be hidden).
    Show,
    /// Exit for good (uninstall).
    Quit,
}

/// Top-level windows of our class, hidden ones included.
fn windows() -> impl Iterator<Item = HWND> {
    let mut prev: Option<HWND> = None;
    std::iter::from_fn(move || {
        let hwnd = unsafe { FindWindowExW(None, prev, WINDOW_CLASS, PCWSTR::null()) }.ok()?;
        if hwnd.is_invalid() {
            return None;
        }
        prev = Some(hwnd);
        Some(hwnd)
    })
}

/// A running instance's window, if any (hidden ones included).
pub fn running() -> Option<HWND> {
    windows().next()
}

/// A hidden resident instance other than `except`.
pub fn hidden(except: Option<HWND>) -> Option<HWND> {
    windows().find(|&w| Some(w) != except && !unsafe { IsWindowVisible(w) }.as_bool())
}

/// The instance a launch should hand over to: any with one window per
/// app, else only a hidden resident one.
pub fn target(single_instance: bool, resident: bool) -> Option<HWND> {
    if single_instance {
        running()
    } else if resident {
        hidden(None)
    } else {
        None
    }
}

/// Sends `path` to `target`. True if it took over.
pub fn hand_off(target: HWND, path: &Path) -> bool {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let data = open_payload(&abs, crate::platform::process_created());
    send(target, COPYDATA_OPEN, &data, true)
}

/// The NUL-terminated path, then the sender's creation time (four UTF-16
/// units, little-endian), which older receivers ignore.
fn open_payload(path: &Path, created: Option<i64>) -> Vec<u16> {
    let mut data = wide(path.as_os_str());
    if let Some(t) = created {
        data.extend((0..4).map(|k| (t >> (16 * k)) as u16));
    }
    data
}

/// Asks `target` to show its window. True if it did.
pub fn show(target: HWND) -> bool {
    send(target, COPYDATA_SHOW, &[0], true)
}

/// Asks every running instance to exit; returns their process ids, so the
/// caller can wait for them.
pub fn quit_all() -> Vec<u32> {
    let mut pids = Vec::new();
    for hwnd in windows().collect::<Vec<_>>() {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if send(hwnd, COPYDATA_QUIT, &[0], false) {
            pids.push(pid);
        }
    }
    pids
}

fn send(hwnd: HWND, tag: usize, data: &[u16], foreground: bool) -> bool {
    unsafe {
        if foreground {
            // Without this the running window may only flash in the taskbar.
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let _ = AllowSetForegroundWindow(pid);
        }
        let cds = COPYDATASTRUCT {
            dwData: tag,
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

/// Decodes a request received via WM_COPYDATA.
pub fn received(cds: &COPYDATASTRUCT) -> Option<Request> {
    match cds.dwData {
        COPYDATA_SHOW => Some(Request::Show),
        COPYDATA_QUIT => Some(Request::Quit),
        COPYDATA_OPEN if !cds.lpData.is_null() => {
            let units = unsafe {
                std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2)
            };
            let end = units.iter().position(|&c| c == 0)?;
            let path = String::from_utf16_lossy(&units[..end]);
            let sender_created = units
                .get(end + 1..end + 5)
                .map(|t| t.iter().rev().fold(0i64, |acc, &u| (acc << 16) | u as i64));
            (!path.is_empty()).then(|| Request::Open {
                path: PathBuf::from(path),
                sender_created,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cds(tag: usize, data: &[u16]) -> COPYDATASTRUCT {
        COPYDATASTRUCT {
            dwData: tag,
            cbData: (data.len() * 2) as u32,
            lpData: data.as_ptr() as *mut _,
        }
    }

    #[test]
    fn decodes_requests() {
        let file = PathBuf::from(r"C:\Bilder\a.jpg");
        let path = wide(&file);
        assert_eq!(
            received(&cds(COPYDATA_OPEN, &path)),
            Some(Request::Open {
                path: file.clone(),
                sender_created: None
            })
        );
        let t = 0x01DC_2345_6789_ABCD;
        assert_eq!(
            received(&cds(COPYDATA_OPEN, &open_payload(&file, Some(t)))),
            Some(Request::Open {
                path: file,
                sender_created: Some(t)
            })
        );
        assert_eq!(received(&cds(COPYDATA_OPEN, &[0])), None);
        assert_eq!(received(&cds(COPYDATA_SHOW, &[0])), Some(Request::Show));
        assert_eq!(received(&cds(COPYDATA_QUIT, &[0])), Some(Request::Quit));
        assert_eq!(received(&cds(0x4242, &path)), None);
    }
}
