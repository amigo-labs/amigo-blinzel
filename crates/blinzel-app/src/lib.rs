//! amigo-blinzel for Windows: a frameless Win32 window, a Direct3D 11
//! renderer and a prefetching decode pipeline around `blinzel-core`.

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
