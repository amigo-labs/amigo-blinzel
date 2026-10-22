//! Thin Windows helpers: strings, files, folders, threads.

pub mod enumerate;
pub mod io;
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

/// Writes to the debugger output (visible in DebugView / WPA), never blocks.
pub fn debug_log(msg: &str) {
    let w = wide(format!("[blinzel] {msg}\n"));
    unsafe {
        windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(windows::core::PCWSTR(
            w.as_ptr(),
        ))
    };
}
