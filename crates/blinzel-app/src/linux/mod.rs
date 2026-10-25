//! The Linux shell: a winit window (Wayland, X11), wgpu rendering and the
//! desktop's services around `blinzel_ui`.

pub mod color;
pub mod gpu;
pub mod install;
pub mod instance;
pub mod shell;
pub mod text;
pub mod update;

pub use shell::run;
