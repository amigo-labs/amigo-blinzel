//! The macOS shell: an AppKit window with a Metal layer, the menu bar, and
//! the system's services (ImageIO, Core Text, colour profiles) around
//! `blinzel_ui`.

pub mod color;
pub mod gpu;
pub mod hotkey;
pub mod imageio;
pub mod resident;
pub mod screenshot;
pub mod shell;
pub mod text;
pub mod update;

pub use shell::run;
