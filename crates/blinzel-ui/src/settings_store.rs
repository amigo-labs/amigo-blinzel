//! The settings file, `settings.toml` in the platform's settings folder
//! (see [`crate::dirs`]).

use std::path::PathBuf;

use blinzel_core::settings::Settings;

pub fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.toml"))
}

#[cfg(not(test))]
fn config_dir() -> Option<PathBuf> {
    crate::dirs::config()
}

/// Unit tests never touch the real settings file: the folder is the one
/// this thread set in [`TEST_CONFIG_DIR`] (the test harness's temp folder),
/// or none.
#[cfg(test)]
fn config_dir() -> Option<PathBuf> {
    TEST_CONFIG_DIR.with(|d| d.borrow().clone())
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CONFIG_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Loads settings (defaults if missing or unreadable; never fails).
pub fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| Settings::parse(&t))
        .unwrap_or_default()
}

pub fn save(s: &Settings) {
    if let Some(p) = path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, s.to_text());
    }
}
