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
        let new = std::fs::read_dir(&unpacked)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "app"));
        let Some(new) = new.filter(|_| ok) else {
            remove(&tmp);
            return Err("the download holds no app bundle".into());
        };
        let previous = self.previous();
        remove(&previous);
        let r = std::fs::rename(&self.bundle, &previous)
            .map_err(|e| format!("keep previous app: {e}"))
            .and_then(|()| {
                std::fs::rename(&new, &self.bundle).map_err(|e| {
                    let _ = std::fs::rename(&previous, &self.bundle);
                    format!("move new app: {e}")
                })
            });
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
        let _ = Command::new("/usr/bin/open")
            .args(["-n", "-g"])
            .arg(&i.bundle)
            .args(["--args", "--resident"])
            .spawn();
    }
}

/// "Check for updates" from the menu; the result goes to `notice`.
pub fn check_now(notice: impl FnOnce(String) + Send + 'static) {
    let i = installed().ok_or_else(|| "not running from an app bundle".to_string());
    selfupdate::check_now(i, notice);
}
