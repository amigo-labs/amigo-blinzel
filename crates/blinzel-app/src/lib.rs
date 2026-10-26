//! amigo-blinzel's platform shells around `blinzel-ui`: Windows (Win32,
//! Direct3D 11), Linux (winit, wgpu) and macOS (AppKit, Metal).

#[cfg(unix)]
pub mod curl;
#[cfg(unix)]
pub mod lcms;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod notices_text;
#[cfg(unix)]
pub mod selfupdate;

#[cfg(windows)]
pub mod backend;
#[cfg(windows)]
pub mod bench;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod color;
#[cfg(windows)]
pub mod dialogs;
#[cfg(windows)]
pub mod first_frame;
#[cfg(windows)]
pub mod gfx;
#[cfg(windows)]
pub mod instance;
#[cfg(windows)]
pub mod menu;
#[cfg(windows)]
pub mod notices;
#[cfg(windows)]
pub mod platform;
#[cfg(windows)]
pub mod resident;
#[cfg(windows)]
pub mod run;
#[cfg(windows)]
pub mod setup;
#[cfg(windows)]
pub mod update;
#[cfg(windows)]
pub mod wic;
#[cfg(windows)]
pub mod window;
