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

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
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
    /// HTTPS GET of at most `limit` bytes.
    fn fetch(&self, url: &str, limit: usize) -> Result<Vec<u8>, String> {
        crate::curl::get(url, limit)
    }
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
    start(
        &i,
        running_version(),
        std::env::args_os().skip(1),
        |mut c| {
            let _ = c.spawn();
        },
    )
}

/// The start of `running` from installation `i`: counts a start of an
/// update on trial, rolls a failed update back (AC-20) and drops the
/// previous version once no update is on trial (AC-21). After a rollback
/// the restored program goes to `spawn` with `args`, and `true` says this
/// process should exit.
pub(crate) fn start(
    i: &impl Installation,
    running: Version,
    args: impl IntoIterator<Item = OsString>,
    spawn: impl FnOnce(Command),
) -> bool {
    let mut state = load_state(i);
    let before = state.clone();
    if core::on_start(&mut state, running) == StartAction::Rollback {
        state.pending = None;
        state.failed_starts = 0;
        if i.rollback() {
            save_state(i, &state);
            let mut restart = Command::new(i.program());
            restart.args(args);
            spawn(restart);
            return true;
        }
        // Nothing to roll back to: keep going with this version.
    }
    if state.pending.is_none() {
        i.forget_previous();
    }
    if state != before {
        save_state(i, &state);
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

/// Starts the daily background check; `false` when this is no installed
/// copy (or the build has no release key) and nothing was started.
pub fn schedule(i: Option<impl Installation + Send + 'static>) -> bool {
    let Some(i) = i else {
        return false;
    };
    if core::RELEASE_KEY.is_none() {
        return false;
    }
    std::thread::Builder::new()
        .name("blinzel-update".into())
        .spawn(move || auto_checks(&mut Auto(i)))
        .is_ok()
}

/// A Linux or macOS process looks once whether a check is due; it does
/// not keep running for days like a resident Windows instance.
fn auto_checks(shell: &mut impl core::AutoCheck) {
    core::auto_checks(shell, false);
}

/// The automatic check of an installed copy.
struct Auto<I>(I);

impl<I: Installation> core::AutoCheck for Auto<I> {
    fn sleep(&mut self, d: Duration) -> bool {
        std::thread::sleep(d);
        true
    }
    fn auto_update(&mut self) -> bool {
        blinzel_ui::settings_store::load().auto_update
    }
    fn now(&mut self) -> u64 {
        now()
    }
    fn load(&mut self) -> State {
        load_state(&self.0)
    }
    fn save(&mut self, state: &State) {
        save_state(&self.0, state);
    }
    fn check(&mut self) {
        if let Err(e) = check_and_install(&self.0)
            && std::env::var_os("BLINZEL_DEBUG").is_some()
        {
            eprintln!("blinzel: update: {e}");
        }
    }
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
        |url, limit| i.fetch(url, limit),
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

    /// An installed copy (state in `dir`) whose downloads come from a
    /// release of the running version; it records the URLs asked for.
    struct Served {
        dir: PathBuf,
        asked: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Installation for Served {
        fn asset(&self) -> String {
            "blinzel-test".into()
        }
        fn state_file(&self) -> PathBuf {
            self.dir.join("update-state")
        }
        fn install(&self, _: &[u8]) -> Result<(), String> {
            Err("nothing to install".into())
        }
        fn rollback(&self) -> bool {
            false
        }
        fn forget_previous(&self) {}
        fn program(&self) -> PathBuf {
            self.dir.join("blinzel")
        }
        fn fetch(&self, url: &str, _: usize) -> Result<Vec<u8>, String> {
            self.asked.lock().unwrap().push(url.into());
            let asset = |name: &str| {
                format!(r#"{{"name": "{name}", "browser_download_url": "https://x/{name}"}}"#)
            };
            let json = format!(
                r#"{{"tag_name": "v{}", "draft": false, "prerelease": false, "assets": [{}, {}, {}]}}"#,
                running_version(),
                asset("blinzel-test"),
                asset(core::SUMS),
                asset(core::SUMS_SIG)
            );
            Ok(json.into_bytes())
        }
    }

    /// The notice a manual check hands over, waited for.
    fn manual_check(i: Result<impl Installation + Send + 'static, String>) -> String {
        let (tx, rx) = std::sync::mpsc::channel();
        check_now(i, move |text| tx.send(text).unwrap());
        rx.recv_timeout(Duration::from_secs(20)).unwrap()
    }

    /// AC-15: "Check for updates" checks at once, though the last check is
    /// only seconds old, and leaves the schedule alone.
    #[test]
    fn a_manual_check_ignores_the_schedule() {
        let dir = std::env::temp_dir().join(format!("blinzel-manual-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let asked = std::sync::Arc::default();
        let served = Served {
            dir: dir.clone(),
            asked: std::sync::Arc::clone(&asked),
        };
        let recent = State {
            last_check: Some(now()),
            ..State::default()
        };
        save_state(&served, &recent);
        assert_eq!(
            manual_check(Ok(served)),
            format!("Blinzel {} is up to date.", running_version())
        );
        assert_eq!(*asked.lock().unwrap(), [core::latest_release_url()]);
        assert_eq!(load_state(&Fake(dir.clone())), recent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AC-14: a copy that is not installed starts no automatic check, and a
    /// manual check fails with the reason.
    #[test]
    fn a_copy_that_is_not_installed_never_checks() {
        assert!(!schedule(None::<Fake>));
        assert_eq!(
            manual_check(Err::<Fake, _>("not installed".into())),
            "Update failed: not installed"
        );
    }

    /// A shell for [`auto_checks`] that counts its looks and never waits.
    #[derive(Default)]
    struct Looks {
        sleeps: Vec<u64>,
        looks: u32,
    }

    impl core::AutoCheck for Looks {
        fn sleep(&mut self, d: Duration) -> bool {
            self.sleeps.push(d.as_secs());
            self.sleeps.len() < 100
        }
        fn auto_update(&mut self) -> bool {
            self.looks += 1;
            false
        }
        fn now(&mut self) -> u64 {
            0
        }
        fn load(&mut self) -> State {
            State::default()
        }
        fn save(&mut self, _: &State) {}
        fn check(&mut self) {}
    }

    /// AC-12: Linux and macOS look once, 10 s after start.
    #[test]
    fn a_process_looks_once_ten_seconds_after_start() {
        let mut l = Looks::default();
        auto_checks(&mut l);
        assert_eq!((l.sleeps, l.looks), (vec![10], 1));
    }

    /// AC-05, AC-15: what "Check for updates" says for each result.
    #[test]
    fn a_manual_check_reports_its_result() {
        let v = Version::parse("1.3.0").unwrap();
        assert_eq!(
            notice_text(Ok(Outcome::Current(v))),
            "Blinzel 1.3.0 is up to date."
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
