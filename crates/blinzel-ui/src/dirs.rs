//! Where settings and caches live on each platform.
//!
//! - Windows: `%APPDATA%\amigo-blinzel`, `%LOCALAPPDATA%\amigo-blinzel`
//! - macOS: `~/Library/Application Support/amigo-blinzel`,
//!   `~/Library/Caches/amigo-blinzel`
//! - Linux: `$XDG_CONFIG_HOME/amigo-blinzel` (`~/.config`),
//!   `$XDG_CACHE_HOME/amigo-blinzel` (`~/.cache`)

use std::ffi::OsString;
use std::path::PathBuf;

const APP: &str = "amigo-blinzel";

/// The platform whose conventions apply (all three are testable on any).
#[derive(Clone, Copy)]
enum Platform {
    Windows,
    Mac,
    Linux,
}

const OS: Platform = if cfg!(windows) {
    Platform::Windows
} else if cfg!(target_os = "macos") {
    Platform::Mac
} else {
    Platform::Linux
};

/// A path from the environment variable `name` as `var` reads it; unset
/// and empty are the same.
fn path_var(var: &impl Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    var(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn config_in(os: Platform, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let home = || path_var(&var, "HOME");
    let base = match os {
        Platform::Windows => path_var(&var, "APPDATA"),
        Platform::Mac => home().map(|h| h.join("Library/Application Support")),
        Platform::Linux => {
            path_var(&var, "XDG_CONFIG_HOME").or_else(|| home().map(|h| h.join(".config")))
        }
    };
    base.map(|b| b.join(APP))
}

fn cache_in(os: Platform, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let home = || path_var(&var, "HOME");
    let base = match os {
        Platform::Windows => path_var(&var, "LOCALAPPDATA"),
        Platform::Mac => home().map(|h| h.join("Library/Caches")),
        Platform::Linux => {
            path_var(&var, "XDG_CACHE_HOME").or_else(|| home().map(|h| h.join(".cache")))
        }
    };
    base.map(|b| b.join(APP))
}

/// Settings.
pub fn config() -> Option<PathBuf> {
    config_in(OS, |n| std::env::var_os(n))
}

/// Caches (thumbnails, the remembered display profile).
pub fn cache() -> Option<PathBuf> {
    cache_in(OS, |n| std::env::var_os(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment with exactly `vars`.
    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    fn settings_file(os: Platform, vars: &[(&str, &str)]) -> Option<PathBuf> {
        config_in(os, env(vars)).map(|d| d.join("settings.toml"))
    }

    #[test]
    fn the_settings_folder_follows_each_platforms_convention() {
        let appdata = r"C:\Users\a\AppData\Roaming";
        assert_eq!(
            settings_file(
                Platform::Windows,
                &[("APPDATA", appdata), ("HOME", "/home/a")]
            ),
            Some(
                PathBuf::from(appdata)
                    .join("amigo-blinzel")
                    .join("settings.toml")
            )
        );
        assert_eq!(
            settings_file(Platform::Mac, &[("HOME", "/Users/a")]),
            Some(PathBuf::from(
                "/Users/a/Library/Application Support/amigo-blinzel/settings.toml"
            ))
        );
        assert_eq!(
            settings_file(
                Platform::Linux,
                &[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/a")]
            ),
            Some(PathBuf::from("/xdg/amigo-blinzel/settings.toml"))
        );
        // XDG_CONFIG_HOME unset or empty: ~/.config.
        let home = PathBuf::from("/home/a/.config/amigo-blinzel/settings.toml");
        for vars in [
            &[("HOME", "/home/a")][..],
            &[("XDG_CONFIG_HOME", ""), ("HOME", "/home/a")][..],
        ] {
            assert_eq!(settings_file(Platform::Linux, vars), Some(home.clone()));
        }
        assert_eq!(
            settings_file(Platform::Windows, &[("HOME", "/home/a")]),
            None
        );
    }

    #[test]
    fn this_platforms_settings_folder_is_the_one_tested() {
        let os = |n: &str| std::env::var_os(n);
        assert_eq!(config(), config_in(OS, os));
        assert_eq!(cache(), cache_in(OS, os));
        if cfg!(not(any(windows, target_os = "macos"))) {
            assert!(matches!(OS, Platform::Linux));
        }
    }

    #[test]
    fn caches_follow_each_platforms_convention() {
        let vars = [
            ("LOCALAPPDATA", "/local"),
            ("XDG_CACHE_HOME", ""),
            ("HOME", "/h"),
        ];
        assert_eq!(
            cache_in(Platform::Windows, env(&vars)),
            Some(PathBuf::from("/local").join("amigo-blinzel"))
        );
        assert_eq!(
            cache_in(Platform::Mac, env(&vars)),
            Some(PathBuf::from("/h/Library/Caches/amigo-blinzel"))
        );
        assert_eq!(
            cache_in(Platform::Linux, env(&vars)),
            Some(PathBuf::from("/h/.cache/amigo-blinzel"))
        );
    }
}
