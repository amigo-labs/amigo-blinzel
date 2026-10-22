//! Settings file location: `%APPDATA%\amigo-blinzel\settings.toml`.

use std::path::PathBuf;

use blinzel_core::settings::Settings;

fn path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("amigo-blinzel").join("settings.toml"))
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
