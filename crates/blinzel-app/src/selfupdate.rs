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

use blinzel_core::update::{self as core, Release, StartAction, State, Version};
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
            let text = match i.and_then(|i| check_and_install(&i)) {
                Ok(Outcome::Current(v)) => trf(Msg::UpdateCurrent, &[("version", &v)]),
                Ok(Outcome::Installed(v)) => trf(Msg::UpdateReady, &[("version", &v)]),
                Err(e) => trf(Msg::UpdateFailed, &[("reason", &e)]),
            };
            notice(text);
        });
}

fn check_and_install(i: &impl Installation) -> Result<Outcome, String> {
    let key = core::RELEASE_KEY.ok_or("no release signing key in this build")?;
    let api = format!(
        "https://api.github.com/repos/{}/releases/latest",
        core::REPO
    );
    let json = crate::curl::get(&api, 1 << 20)?;
    let json = String::from_utf8(json).map_err(|_| "release info is not UTF-8")?;
    let asset = i.asset();
    let Release {
        version,
        exe_url,
        sums_url,
        sig_url,
    } = core::parse_release(&json, &asset).ok_or("no usable release")?;
    let running = running_version();
    if version <= running {
        return Ok(Outcome::Current(running));
    }

    let sums = crate::curl::get(&sums_url, 64 << 10)?;
    let sig = crate::curl::get(&sig_url, 4 << 10)?;
    if !core::verify_signature(&sums, &sig, &key) {
        return Err("checksums are not signed with the release key".into());
    }
    let sums = String::from_utf8(sums).map_err(|_| "checksums are not UTF-8")?;
    let want = core::sum_for(&sums, &asset).ok_or("no checksum for us")?;
    let download = crate::curl::get(&exe_url, MAX_DOWNLOAD)?;
    if core::sha256(&download) != want {
        return Err("download does not match its checksum".into());
    }
    i.install(&download)?;
    let mut state = load_state(i);
    state.pending = Some(version);
    state.failed_starts = 0;
    save_state(i, &state);
    Ok(Outcome::Installed(version))
}
