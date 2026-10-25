//! The viewer's UI without a platform: what is shown and why, navigation,
//! prefetching, the thumbnail browser, menus and colour management. The
//! platform shells in `blinzel-app` (Windows, macOS, Linux) implement the
//! traits in [`backend`] and drive [`app::App`] from their event loops.

pub mod app;
pub mod backend;
pub mod batch_dialog;
pub mod browse;
pub mod chrome;
pub mod color;
pub mod dirs;
pub mod files;
pub mod gfx;
pub mod loader;
pub mod menu;
pub mod popup;
pub mod settings_store;
pub mod thumbs;
pub mod widgets;
