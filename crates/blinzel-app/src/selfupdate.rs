//! The self-updater on Linux and macOS, the same flow as on Windows: at
//! most daily, ten seconds after start and on a background thread, it asks
//! GitHub for the latest release, downloads this platform's build and
//! verifies it before putting it in place (active from the next start):
//!
//! 1. Ed25519 signature of `SHA256SUMS` with our own release key,
//! 2. SHA-256 of the download against `SHA256SUMS`.
//!
//! Downloads go through `curl` ([`crate::curl`]). A new version that fails
//! to reach its first frame three times is replaced by the previous one
//! again. Only an installed copy updates itself; the platform says what
//! that is and how a download is put in place ([`Installation`]).

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blinzel_core::update::{self as core, Found, StartAction, State, Version};
use blinzel_i18n::{Msg, trf};

/// Largest download accepted (the Linux executable is about 20 MB, the
/// macOS universal app bundle about twice that, zipped).
const MAX_DOWNLOAD: usize = 128 << 20;

/// An installed copy of the viewer that can update itself.
pub trait Installation {
    /// The release asset with this platform's build.
    fn asset(&self) -> String;
    /// Where the update state lives.
    fn state_file(&self) -> PathBuf;
    /// Puts the verified download in place and keeps the running version
    /// as the previous one (for a rollback).
    fn install(&self, download: &[u8]) -> Result<(), String>;
    /// Puts the previous version back; `false` if there is none.
    fn rollback(&self) -> bool;
    /// Drops the previous version: the running one works.
    fn forget_previous(&self);
    /// The installed program (started again after a rollback).
    fn program(&self) -> PathBuf;
}

pub fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

fn running_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is x.y.z")
}

fn load_state(i: &impl Installation) -> State {
    std::fs::read_to_string(i.state_file())
        .map(|t| State::parse(&t))
        .unwrap_or_default()
}

fn save_state(i: &impl Installation, state: &State) {
    let path = i.state_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, state.to_text());
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Called early at every start. Returns `true` when this process should
/// exit because it rolled back and started the previous version instead.
pub fn on_start(i: Option<impl Installation>) -> bool {
    let Some(i) = i else {
        return false;
    };
    let mut state = load_state(&i);
    let before = state.clone();
    if core::on_start(&mut state, running_version()) == StartAction::Rollback {
        state.pending = None;
        state.failed_starts = 0;
        if i.rollback() {
            save_state(&i, &state);
            let _ = std::process::Command::new(i.program())
                .args(std::env::args_os().skip(1))
                .spawn();
            return true;
        }
        // Nothing to roll back to: keep going with this version.
    }
    if state.pending.is_none() {
        i.forget_previous();
    }
    if state != before {
        save_state(&i, &state);
    }
    false
}

/// An update was installed while this process kept running the previous
/// version (resident mode): it should hand over to the new program.
pub fn restart_pending(i: Option<impl Installation>) -> bool {
    i.is_some_and(|i| load_state(&i).restart_pending(running_version()))
}

/// Called once after the first frame: the running version works.
pub fn on_first_frame(i: Option<impl Installation + Send + 'static>) {
    let Some(i) = i else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("blinzel-update-ok".into())
        .spawn(move || {
            let mut state = load_state(&i);
            if state.pending.is_some() {
                core::on_first_frame(&mut state);
                save_state(&i, &state);
            }
        });
}

/// Starts the daily background check.
pub fn schedule(i: Option<impl Installation + Send + 'static>) {
    let Some(i) = i else {
        return;
    };
    if core::RELEASE_KEY.is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("blinzel-update".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            if !blinzel_ui::settings_store::load().auto_update {
                return;
            }
            let mut state = load_state(&i);
            if !core::check_due(state.last_check, now()) {
                return;
            }
            state.last_check = Some(now());
            save_state(&i, &state);
            if let Err(e) = check_and_install(&i)
                && std::env::var_os("BLINZEL_DEBUG").is_some()
            {
                eprintln!("blinzel: update: {e}");
            }
        });
}

/// What a check found.
enum Outcome {
    Current(Version),
    Installed(Version),
}

/// "Check for updates" from the menu: runs in the background and hands the
/// result to `notice` as text. `Err`: not an installed copy, and why.
pub fn check_now(
    i: Result<impl Installation + Send + 'static, String>,
    notice: impl FnOnce(String) + Send + 'static,
) {
    let _ = std::thread::Builder::new()
        .name("blinzel-update-now".into())
        .spawn(move || {
            notice(notice_text(i.and_then(|i| check_and_install(&i))));
        });
}

/// The notice "Check for updates" shows for a check's result.
fn notice_text(result: Result<Outcome, String>) -> String {
    match result {
        Ok(Outcome::Current(v)) => trf(Msg::UpdateCurrent, &[("version", &v)]),
        Ok(Outcome::Installed(v)) => trf(Msg::UpdateReady, &[("version", &v)]),
        Err(e) => trf(Msg::UpdateFailed, &[("reason", &e)]),
    }
}

fn check_and_install(i: &impl Installation) -> Result<Outcome, String> {
    let found = core::check(
        &i.asset(),
        running_version(),
        core::RELEASE_KEY,
        MAX_DOWNLOAD,
        crate::curl::get,
        |d| Ok(core::sha256(d)),
    )?;
    let (version, download) = match found {
        Found::Current(v) => return Ok(Outcome::Current(v)),
        Found::New { version, download } => (version, download),
    };
    i.install(&download)?;
    let mut state = load_state(i);
    state.pending = Some(version);
    state.failed_starts = 0;
    save_state(i, &state);
    Ok(Outcome::Installed(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An installed copy whose update state lives in `0`.
    struct Fake(PathBuf);

    impl Installation for Fake {
        fn asset(&self) -> String {
            String::new()
        }
        fn state_file(&self) -> PathBuf {
            self.0.join("update-state")
        }
        fn install(&self, _: &[u8]) -> Result<(), String> {
            Ok(())
        }
        fn rollback(&self) -> bool {
            false
        }
        fn forget_previous(&self) {}
        fn program(&self) -> PathBuf {
            self.0.join("blinzel")
        }
    }

    /// AC-05, AC-15: what "Check for updates" says for each result.
    #[test]
    fn a_manual_check_reports_its_result() {
        let v = Version::parse("1.3.0").unwrap();
        assert_eq!(
            notice_text(Ok(Outcome::Current(v))),
            "amigo-blinzel 1.3.0 is up to date."
        );
        assert_eq!(
            notice_text(Ok(Outcome::Installed(v))),
            "Version 1.3.0 is downloaded and active from the next start."
        );
        assert_eq!(
            notice_text(Err("no checksum for us".into())),
            "Update failed: no checksum for us"
        );
    }

    /// `docs/specs/resident-mode.md` AC-21: a hiding window hands over only
    /// when an installed update's version differs from the running one;
    /// no installation, no state or the running version pending: it stays.
    #[test]
    fn a_hiding_window_restarts_only_for_another_installed_version() {
        let dir = std::env::temp_dir().join(format!("blinzel-selfupdate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fake = || Some(Fake(dir.clone()));
        assert!(!restart_pending(None::<Fake>), "not an installed copy");
        assert!(!restart_pending(fake()), "nothing installed");
        let pending = |v: Version| State {
            pending: Some(v),
            ..State::default()
        };
        save_state(&Fake(dir.clone()), &pending(running_version()));
        assert!(!restart_pending(fake()), "the running version");
        let other = Version::parse("999.0.0").unwrap();
        save_state(&Fake(dir.clone()), &pending(other));
        assert!(restart_pending(fake()), "another version installed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
