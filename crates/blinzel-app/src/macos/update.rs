//! The macOS side of the self-updater ([`crate::selfupdate`]): the zipped,
//! universal app bundle `amigo-blinzel-macos.zip` replaces the running
//! bundle, which stays next to it (hidden) for a rollback. Only a copy
//! running from an `.app` bundle updates itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use blinzel_core::update as core;

use crate::selfupdate::{self, Installation};

struct Installed {
    /// `…/amigo-blinzel.app`
    bundle: PathBuf,
}

/// The installation when this process runs from an app bundle.
fn installed() -> Option<Installed> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let macos = exe.parent()?;
    let bundle = macos.parent()?.parent()?;
    (macos.file_name()? == "MacOS" && bundle.extension()? == "app").then(|| Installed {
        bundle: bundle.to_path_buf(),
    })
}

impl Installed {
    fn sibling(&self, name: &str) -> PathBuf {
        self.bundle.with_file_name(name)
    }

    fn previous(&self) -> PathBuf {
        self.sibling(".amigo-blinzel-previous.app")
    }

    fn bad(&self) -> PathBuf {
        self.sibling(".amigo-blinzel-bad.app")
    }
}

fn remove(p: &Path) {
    let _ = std::fs::remove_dir_all(p);
}

const NO_BUNDLE: &str = "the download holds no app bundle";

/// Puts the `.app` found in `unpacked` in place of `bundle` and keeps the
/// running bundle as `previous` (updates AC-18). Without an `.app` in
/// `unpacked`, or if the new bundle cannot take its name, `bundle` stays
/// as it was.
fn put_in_place(unpacked: &Path, bundle: &Path, previous: &Path) -> Result<(), String> {
    let new = std::fs::read_dir(unpacked)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "app"))
        .ok_or(NO_BUNDLE)?;
    remove(previous);
    std::fs::rename(bundle, previous).map_err(|e| format!("keep previous app: {e}"))?;
    std::fs::rename(&new, bundle).map_err(|e| {
        let _ = std::fs::rename(previous, bundle);
        format!("move new app: {e}")
    })
}

impl Installation for Installed {
    fn asset(&self) -> String {
        core::macos_asset_name()
    }

    fn state_file(&self) -> PathBuf {
        blinzel_ui::dirs::cache()
            .unwrap_or_else(std::env::temp_dir)
            .join("update.state")
    }

    fn install(&self, download: &[u8]) -> Result<(), String> {
        // Unpacked next to the bundle: the swap is a rename on one volume.
        let tmp = self.sibling(".amigo-blinzel-update");
        remove(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let zip = tmp.join("update.zip");
        std::fs::write(&zip, download).map_err(|e| format!("{}: {e}", zip.display()))?;
        let unpacked = tmp.join("unpacked");
        let ok = Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(&unpacked)
            .status()
            .map_err(|e| format!("ditto: {e}"))?
            .success();
        let r = if ok {
            put_in_place(&unpacked, &self.bundle, &self.previous())
        } else {
            Err(NO_BUNDLE.into())
        };
        remove(&tmp);
        r
    }

    fn rollback(&self) -> bool {
        let (previous, bad) = (self.previous(), self.bad());
        remove(&bad);
        previous.exists()
            && std::fs::rename(&self.bundle, &bad).is_ok()
            && std::fs::rename(&previous, &self.bundle).is_ok()
    }

    fn forget_previous(&self) {
        remove(&self.previous());
        remove(&self.bad());
    }

    fn program(&self) -> PathBuf {
        self.bundle.join("Contents/MacOS/blinzel")
    }
}

pub fn on_start() -> bool {
    selfupdate::on_start(installed())
}

pub fn on_first_frame() {
    selfupdate::on_first_frame(installed());
}

pub fn schedule() {
    selfupdate::schedule(installed());
}

/// The program inside the app bundle this process runs from, if any (for
/// the login item).
pub fn bundle_program() -> Option<PathBuf> {
    installed().map(|i| i.program())
}

/// An update waits in the bundle while a resident instance keeps running
/// the previous version.
pub fn restart_pending() -> bool {
    selfupdate::restart_pending(installed())
}

/// Starts the installed (updated) bundle as the new resident instance.
pub fn start_installed_resident() {
    if let Some(i) = installed() {
        let _ = resident_start(&i.bundle).spawn();
    }
}

/// `open -n -g <bundle> --args --resident`: a new instance of `bundle`,
/// in the background, started hidden.
fn resident_start(bundle: &Path) -> Command {
    let mut c = Command::new("/usr/bin/open");
    c.args(["-n", "-g"])
        .arg(bundle)
        .args(["--args", "--resident"]);
    c
}

/// "Check for updates" from the menu; the result goes to `notice`.
pub fn check_now(notice: impl FnOnce(String) + Send + 'static) {
    let i = installed().ok_or_else(|| "not running from an app bundle".to_string());
    selfupdate::check_now(i, notice);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-14: the test executable runs outside an app bundle, so it is no
    /// installation and "Check for updates" says so.
    #[test]
    fn a_copy_outside_a_bundle_does_not_update() {
        assert!(installed().is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        check_now(move |text| tx.send(text).unwrap());
        let text = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        assert_eq!(text, "Update failed: not running from an app bundle");
    }

    /// AC-04: the macOS updater fetches the zipped universal bundle.
    #[test]
    fn fetches_the_zipped_bundle() {
        let i = Installed {
            bundle: PathBuf::from("/Applications/amigo-blinzel.app"),
        };
        assert_eq!(i.asset(), "amigo-blinzel-macos.zip");
    }

    /// `docs/specs/resident-mode.md` AC-21: the installed update starts as
    /// a new, hidden instance of the bundle.
    #[test]
    fn an_installed_update_starts_as_a_new_hidden_instance() {
        let c = resident_start(Path::new("/Applications/amigo-blinzel.app"));
        assert_eq!(c.get_program(), "/usr/bin/open");
        let args: Vec<_> = c.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "-n",
                "-g",
                "/Applications/amigo-blinzel.app",
                "--args",
                "--resident"
            ]
        );
    }

    /// A bundle `amigo-blinzel.app` holding "v1" in a fresh temporary
    /// folder (removed by the caller).
    fn sandbox(name: &str) -> (PathBuf, Installed) {
        let dir = std::env::temp_dir().join(format!("blinzel-{name}-{}", std::process::id()));
        remove(&dir);
        let i = Installed {
            bundle: dir.join("amigo-blinzel.app"),
        };
        bundle_with(&i.bundle, "v1");
        (dir, i)
    }

    fn bundle_with(app: &Path, version: &str) {
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/blinzel"), version).unwrap();
    }

    fn version_in(app: &Path) -> String {
        std::fs::read_to_string(app.join("Contents/MacOS/blinzel")).unwrap()
    }

    /// AC-18: the unpacked `.app` takes the running bundle's name, which
    /// moves to `.amigo-blinzel-previous.app` in the same folder; without
    /// an `.app` nothing moves.
    #[test]
    fn the_unpacked_bundle_takes_the_running_bundles_name() {
        let (dir, i) = sandbox("put-in-place");
        let unpacked = dir.join("unpacked");
        bundle_with(&unpacked.join("amigo-blinzel.app"), "v2");
        put_in_place(&unpacked, &i.bundle, &i.previous()).unwrap();
        assert_eq!(version_in(&i.bundle), "v2");
        assert_eq!(i.previous(), dir.join(".amigo-blinzel-previous.app"));
        assert_eq!(version_in(&i.previous()), "v1");
        let empty = dir.join("empty");
        std::fs::create_dir_all(empty.join("Contents")).unwrap();
        assert_eq!(
            put_in_place(&empty, &i.bundle, &i.previous()),
            Err(NO_BUNDLE.into())
        );
        assert_eq!(version_in(&i.bundle), "v2");
        assert_eq!(version_in(&i.previous()), "v1");
        remove(&dir);
    }

    /// AC-18, AC-19: the downloaded zip is unpacked next to the bundle and
    /// put in place, so the next start runs the new program; a download
    /// that is no zip or holds no `.app` leaves the bundle untouched.
    #[test]
    fn a_downloaded_zip_is_unpacked_next_to_the_bundle() {
        let (dir, i) = sandbox("install");
        let zip = |src: &Path| {
            let zip = dir.join("download.zip");
            let ok = Command::new("/usr/bin/ditto")
                .args(["-c", "-k", "--keepParent"])
                .arg(src)
                .arg(&zip)
                .status()
                .unwrap()
                .success();
            assert!(ok, "ditto packs {}", src.display());
            std::fs::read(&zip).unwrap()
        };
        let new = dir.join("release/amigo-blinzel.app");
        bundle_with(&new, "v2");
        i.install(&zip(&new)).unwrap();
        assert_eq!(std::fs::read_to_string(i.program()).unwrap(), "v2");
        assert_eq!(version_in(&i.previous()), "v1");
        assert!(!dir.join(".amigo-blinzel-update").exists());
        let plain = dir.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("readme.txt"), "no app").unwrap();
        for download in [zip(&plain), b"no zip".to_vec()] {
            assert_eq!(i.install(&download), Err(NO_BUNDLE.into()));
            assert_eq!(version_in(&i.bundle), "v2");
            assert_eq!(version_in(&i.previous()), "v1");
        }
        remove(&dir);
    }

    /// AC-20: a rollback puts `.amigo-blinzel-previous.app` back and keeps
    /// the failed bundle as `.amigo-blinzel-bad.app`; without a previous
    /// bundle nothing moves.
    #[test]
    fn a_rollback_restores_the_previous_bundle() {
        let (dir, i) = sandbox("rollback");
        assert!(!i.rollback(), "nothing to roll back to");
        assert_eq!(version_in(&i.bundle), "v1");
        std::fs::rename(&i.bundle, i.previous()).unwrap();
        bundle_with(&i.bundle, "v2");
        assert!(i.rollback());
        assert_eq!(version_in(&i.bundle), "v1");
        assert_eq!(i.bad(), dir.join(".amigo-blinzel-bad.app"));
        assert_eq!(version_in(&i.bad()), "v2");
        assert!(!i.previous().exists());
        assert_eq!(i.program(), i.bundle.join("Contents/MacOS/blinzel"));
        remove(&dir);
    }

    /// AC-21: dropping the previous version deletes the previous and the
    /// failed bundle, never the running one.
    #[test]
    fn forgetting_deletes_the_previous_and_bad_bundles() {
        let (dir, i) = sandbox("forget");
        bundle_with(&i.previous(), "v0");
        bundle_with(&i.bad(), "v2");
        i.forget_previous();
        assert!(!i.previous().exists() && !i.bad().exists());
        assert_eq!(version_in(&i.bundle), "v1");
        remove(&dir);
    }
}
