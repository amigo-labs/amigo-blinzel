//! Where settings and caches live on each platform.
//!
//! - Windows: `%APPDATA%\amigo-blinzel`, `%LOCALAPPDATA%\amigo-blinzel`
//! - macOS: `~/Library/Application Support/amigo-blinzel`,
//!   `~/Library/Caches/amigo-blinzel`
//! - Linux: `$XDG_CONFIG_HOME/amigo-blinzel` (`~/.config`),
//!   `$XDG_CACHE_HOME/amigo-blinzel` (`~/.cache`)

use std::path::PathBuf;

const APP: &str = "amigo-blinzel";

fn env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(windows))]
fn home() -> Option<PathBuf> {
    env("HOME")
}

/// Settings.
pub fn config() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = env("APPDATA");
    #[cfg(target_os = "macos")]
    let base = home().map(|h| h.join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = env("XDG_CONFIG_HOME").or_else(|| home().map(|h| h.join(".config")));
    base.map(|b| b.join(APP))
}

/// Caches (thumbnails, the remembered display profile).
pub fn cache() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = env("LOCALAPPDATA");
    #[cfg(target_os = "macos")]
    let base = home().map(|h| h.join("Library/Caches"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = env("XDG_CACHE_HOME").or_else(|| home().map(|h| h.join(".cache")));
    base.map(|b| b.join(APP))
}
