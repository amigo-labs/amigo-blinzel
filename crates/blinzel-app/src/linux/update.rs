//! The Linux side of the self-updater ([`crate::selfupdate`]): the bare
//! executable `amigo-blinzel-linux-<arch>` replaces the copy `--install`
//! put in `~/.local/share/amigo-blinzel/`, which keeps the previous one as
//! `blinzel.old` for a rollback.

use std::path::{Path, PathBuf};

use blinzel_core::update as core;

use super::install::Layout;
use crate::selfupdate::{self, Installation};

struct Installed(Layout);

/// The installation when this process is the installed executable.
fn installed() -> Option<Installed> {
    let layout = Layout::user()?;
    let exe = std::env::current_exe().ok()?;
    layout.is_installed_exe(&exe).then_some(Installed(layout))
}

impl Installed {
    fn old(&self) -> PathBuf {
        self.0.exe().with_extension("old")
    }

    fn bad(&self) -> PathBuf {
        self.0.exe().with_extension("bad")
    }
}

impl Installation for Installed {
    fn asset(&self) -> String {
        core::linux_asset_name(selfupdate::arch())
    }

    fn state_file(&self) -> PathBuf {
        self.0.cache.join("update.state")
    }

    fn install(&self, download: &[u8]) -> Result<(), String> {
        let exe = self.0.exe();
        let new = exe.with_extension("new");
        write_executable(&new, download)?;
        swap(&exe, &new, &self.old())
    }

    fn rollback(&self) -> bool {
        let (exe, old, bad) = (self.0.exe(), self.old(), self.bad());
        let _ = std::fs::remove_file(&bad);
        old.exists() && std::fs::rename(&exe, &bad).is_ok() && std::fs::rename(&old, &exe).is_ok()
    }

    fn forget_previous(&self) {
        let _ = std::fs::remove_file(self.old());
        let _ = std::fs::remove_file(self.bad());
    }

    fn program(&self) -> PathBuf {
        self.0.exe()
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

/// "Check for updates" from the menu; the result goes to `notice`.
pub fn check_now(notice: impl FnOnce(String) + Send + 'static) {
    let i = installed().ok_or_else(|| "not installed (blinzel --install)".to_string());
    selfupdate::check_now(i, notice);
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
