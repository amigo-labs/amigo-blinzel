//! `blinzel.exe`

// No console window for the viewer.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    std::process::exit(blinzel_app::run::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("amigo-blinzel is a Windows application.");
    std::process::exit(1);
}
