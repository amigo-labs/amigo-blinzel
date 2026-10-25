//! The self-updater on Linux, the same flow as on Windows: at most daily,
//! ten seconds after start and on a background thread, it asks GitHub for
//! the latest release, downloads the executable for this architecture and
//! verifies it before swapping it in (active from the next start):
//!
//! 1. Ed25519 signature of `SHA256SUMS` with our own release key,
//! 2. SHA-256 of the executable against `SHA256SUMS`.
//!
//! Downloads go through `curl` (on every desktop distribution): the
//! signature, not the transport, proves origin, so the viewer carries no
//! TLS stack of its own. A new version that fails to reach its first frame
//! three times is replaced by the previous one again. Only an installed
//! copy (`--install`) updates itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blinzel_core::update::{self as core, Release, StartAction, State, Version};
use blinzel_i18n::{Msg, trf};

use super::install::Layout;

/// Largest download accepted (the executable is about 20 MB).
const MAX_DOWNLOAD: usize = 128 << 20;

fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

fn running_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is x.y.z")
}

/// The layout when this process is the installed executable.
fn installed_layout() -> Option<Layout> {
    let layout = Layout::user()?;
    let exe = std::env::current_exe().ok()?;
    layout.is_installed_exe(&exe).then_some(layout)
}

fn state_path(layout: &Layout) -> PathBuf {
    layout.cache.join("update.state")
}

fn load_state(layout: &Layout) -> State {
    std::fs::read_to_string(state_path(layout))
        .map(|t| State::parse(&t))
        .unwrap_or_default()
}

fn save_state(layout: &Layout, state: &State) {
    let _ = std::fs::create_dir_all(&layout.cache);
    let _ = std::fs::write(state_path(layout), state.to_text());
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn old_exe(layout: &Layout) -> PathBuf {
    layout.exe().with_extension("old")
}

fn bad_exe(layout: &Layout) -> PathBuf {
    layout.exe().with_extension("bad")
}

/// Called early at every start. Returns `true` when this process should
/// exit because it rolled back and started the previous version instead.
pub fn on_start() -> bool {
    let Some(layout) = installed_layout() else {
        return false;
    };
    let mut state = load_state(&layout);
    let before = state.clone();
    if core::on_start(&mut state, running_version()) == StartAction::Rollback {
        let (exe, old, bad) = (layout.exe(), old_exe(&layout), bad_exe(&layout));
        let _ = std::fs::remove_file(&bad);
        state.pending = None;
        state.failed_starts = 0;
        if old.exists()
            && std::fs::rename(&exe, &bad).is_ok()
            && std::fs::rename(&old, &exe).is_ok()
        {
            save_state(&layout, &state);
            let _ = Command::new(&exe).args(std::env::args_os().skip(1)).spawn();
            return true;
        }
        // Nothing to roll back to: keep going with this version.
    }
    if state.pending.is_none() {
        // The previous executable is no longer needed.
        let _ = std::fs::remove_file(old_exe(&layout));
        let _ = std::fs::remove_file(bad_exe(&layout));
    }
    if state != before {
        save_state(&layout, &state);
    }
    false
}

/// Called once after the first frame: the running version works.
pub fn on_first_frame() {
    let _ = std::thread::Builder::new()
        .name("blinzel-update-ok".into())
        .spawn(|| {
            let Some(layout) = installed_layout() else {
                return;
            };
            let mut state = load_state(&layout);
            if state.pending.is_some() {
                core::on_first_frame(&mut state);
                save_state(&layout, &state);
            }
        });
}

/// Starts the daily background check.
pub fn schedule() {
    if core::RELEASE_KEY.is_none() || installed_layout().is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("blinzel-update".into())
        .spawn(|| {
            std::thread::sleep(Duration::from_secs(10));
            if blinzel_ui::settings_store::load().auto_update {
                check_if_due();
            }
        });
}

fn check_if_due() {
    let Some(layout) = installed_layout() else {
        return;
    };
    let mut state = load_state(&layout);
    if !core::check_due(state.last_check, now()) {
        return;
    }
    state.last_check = Some(now());
    save_state(&layout, &state);
    if let Err(e) = check_and_install(&layout) {
        super::shell::log(&format!("update: {e}"));
    }
}

/// What a check found.
enum Outcome {
    Current(Version),
    Installed(Version),
}

/// "Check for updates" from the menu: runs in the background and hands the
/// result to `notice` as text.
pub fn check_now(notice: impl FnOnce(String) + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("blinzel-update-now".into())
        .spawn(move || {
            let text = match installed_layout()
                .ok_or_else(|| "not installed (blinzel --install)".to_string())
                .and_then(|l| check_and_install(&l))
            {
                Ok(Outcome::Current(v)) => trf(Msg::UpdateCurrent, &[("version", &v)]),
                Ok(Outcome::Installed(v)) => trf(Msg::UpdateReady, &[("version", &v)]),
                Err(e) => trf(Msg::UpdateFailed, &[("reason", &e)]),
            };
            notice(text);
        });
}

fn check_and_install(layout: &Layout) -> Result<Outcome, String> {
    let key = core::RELEASE_KEY.ok_or("no release signing key in this build")?;
    let api = format!(
        "https://api.github.com/repos/{}/releases/latest",
        core::REPO
    );
    let json = http_get(&api, 1 << 20)?;
    let json = String::from_utf8(json).map_err(|_| "release info is not UTF-8")?;
    let asset = core::linux_asset_name(arch());
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

    let sums = http_get(&sums_url, 64 << 10)?;
    let sig = http_get(&sig_url, 4 << 10)?;
    if !core::verify_signature(&sums, &sig, &key) {
        return Err("checksums are not signed with the release key".into());
    }
    let sums = String::from_utf8(sums).map_err(|_| "checksums are not UTF-8")?;
    let want = core::sum_for(&sums, &asset).ok_or("no checksum for us")?;
    let exe = http_get(&exe_url, MAX_DOWNLOAD)?;
    if core::sha256(&exe) != want {
        return Err("download does not match its checksum".into());
    }

    let new = layout.exe().with_extension("new");
    write_executable(&new, &exe)?;
    swap(&layout.exe(), &new, &old_exe(layout))?;
    let mut state = load_state(layout);
    state.pending = Some(version);
    state.failed_starts = 0;
    save_state(layout, &state);
    Ok(Outcome::Installed(version))
}

fn write_executable(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(data)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Puts `new` in place of the `installed` executable and keeps the
/// previous one as `old` for a rollback. The running process keeps its
/// file open; the new one is active from the next start.
fn swap(installed: &Path, new: &Path, old: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(old);
    std::fs::hard_link(installed, old).map_err(|e| format!("keep previous exe: {e}"))?;
    std::fs::rename(new, installed).map_err(|e| format!("move new exe: {e}"))
}

/// GET `url` (HTTPS only, redirects allowed) with at most `limit` bytes.
fn http_get(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let out = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-time", "300", "--max-filesize", &limit.to_string()])
        .arg("--user-agent")
        .arg(format!("amigo-blinzel/{}", env!("CARGO_PKG_VERSION")))
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("curl: {}", err.trim()));
    }
    if out.stdout.len() > limit {
        return Err("download too large".into());
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_keeps_the_previous_executable() {
        let dir = std::env::temp_dir().join(format!("blinzel-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (installed, new, old) = (
            dir.join("blinzel"),
            dir.join("blinzel.new"),
            dir.join("blinzel.old"),
        );
        std::fs::write(&installed, b"v1").unwrap();
        write_executable(&new, b"v2").unwrap();
        swap(&installed, &new, &old).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), b"v2");
        assert_eq!(std::fs::read(&old).unwrap(), b"v1");
        assert!(!new.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
