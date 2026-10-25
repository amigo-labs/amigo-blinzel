//! `blinzel.exe`

// No console window for the viewer.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    std::process::exit(blinzel_app::run::run());
}

#[cfg(all(unix, not(target_os = "macos")))]
fn main() {
    std::process::exit(blinzel_app::linux::run());
}
