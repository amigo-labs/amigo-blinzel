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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::TempDir;

    fn in_config_dir<T>(dir: Option<PathBuf>, f: impl FnOnce() -> T) -> T {
        TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = dir);
        let r = f();
        TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = None);
        r
    }

    #[test]
    fn a_missing_or_unreadable_file_loads_the_defaults() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("config");
        // No folder at all, an empty folder, no file.
        assert_eq!(in_config_dir(None, load), Settings::default());
        assert_eq!(in_config_dir(Some(dir.clone()), load), Settings::default());
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(in_config_dir(Some(dir.clone()), load), Settings::default());
        // Not UTF-8, and a folder where the file should be.
        std::fs::write(dir.join("settings.toml"), b"filmstrip = true\n\xff\xfe").unwrap();
        assert_eq!(in_config_dir(Some(dir.clone()), load), Settings::default());
        std::fs::remove_file(dir.join("settings.toml")).unwrap();
        std::fs::create_dir(dir.join("settings.toml")).unwrap();
        assert_eq!(in_config_dir(Some(dir.clone()), load), Settings::default());
        // A readable file does apply (the checks above are not vacuous).
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("settings.toml"), "filmstrip = true\n").unwrap();
        assert!(in_config_dir(Some(other), load).filmstrip);
    }
}
