//! Thin Windows helpers: strings, files, folders, threads.

pub mod enumerate;
pub mod io;
pub mod os;
pub mod threads;

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// NUL-terminated UTF-16.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// UTF-16 up to the first NUL.
pub fn from_wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Now as FILETIME ticks (100 ns since 1601, UTC).
pub fn now_filetime() -> i64 {
    let t = unsafe { windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime() };
    ((t.dwHighDateTime as i64) << 32) | t.dwLowDateTime as i64
}

/// When this process was created, as FILETIME ticks.
pub fn process_created() -> Option<i64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut t = [FILETIME::default(); 4];
    let [c, e, k, u] = &mut t;
    unsafe { GetProcessTimes(GetCurrentProcess(), c, e, k, u) }.ok()?;
    Some(((t[0].dwHighDateTime as i64) << 32) | t[0].dwLowDateTime as i64)
}

/// Message box flags for the UI language: right-aligned, right-to-left
/// reading for right-to-left languages.
pub fn message_box_direction() -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE {
    message_box_style(blinzel_i18n::is_rtl())
}

fn message_box_style(rtl: bool) -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE {
    use windows::Win32::UI::WindowsAndMessaging::{MB_RIGHT, MB_RTLREADING};
    if rtl {
        MB_RIGHT | MB_RTLREADING
    } else {
        Default::default()
    }
}

/// Writes to the debugger output (visible in DebugView / WPA), never blocks.
pub fn debug_log(msg: &str) {
    let w = wide(format!("[blinzel] {msg}\n"));
    unsafe {
        windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(windows::core::PCWSTR(
            w.as_ptr(),
        ))
    };
}

#[cfg(test)]
mod tests {
    use super::message_box_style;
    use windows::Win32::UI::WindowsAndMessaging::{MB_RIGHT, MB_RTLREADING, MESSAGEBOX_STYLE};

    /// `docs/specs/localization.md` AC-16: message boxes are right-aligned
    /// with right-to-left reading while the language is right to left.
    #[test]
    fn right_to_left_message_boxes_are_right_aligned_and_read_right_to_left() {
        assert_eq!(message_box_style(true), MB_RIGHT | MB_RTLREADING);
        assert_eq!(message_box_style(false), MESSAGEBOX_STYLE::default());
    }
}
