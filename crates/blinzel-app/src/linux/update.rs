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
    use std::ffi::OsString;
    use std::io::Read as _;
    use std::process::Command;

    use blinzel_core::update::{State, Version};

    use super::*;

    /// An installation in a fresh temporary folder (removed by the caller).
    fn sandbox(name: &str) -> (PathBuf, Installed) {
        let dir = std::env::temp_dir().join(format!("blinzel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let i = Installed(Layout::under(&dir));
        std::fs::create_dir_all(&i.0.dir).unwrap();
        std::fs::create_dir_all(&i.0.cache).unwrap();
        (dir, i)
    }

    fn set_state(i: &Installed, state: &State) {
        std::fs::write(i.state_file(), state.to_text()).unwrap();
    }

    fn state(i: &Installed) -> State {
        State::parse(&std::fs::read_to_string(i.state_file()).unwrap_or_default())
    }

    fn read(p: &Path) -> Vec<u8> {
        std::fs::read(p).unwrap()
    }

    fn no_restart(_: Command) {
        panic!("restarted although nothing was rolled back");
    }

    /// AC-14: the test executable is not the one `--install` placed, so
    /// it is no installation and "Check for updates" says so.
    #[test]
    fn a_copy_outside_the_installation_does_not_update() {
        assert!(installed().is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        check_now(move |text| tx.send(text).unwrap());
        let text = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        assert_eq!(text, "Update failed: not installed (blinzel --install)");
    }

    /// AC-04: the Linux updater fetches the bare executable of its own
    /// architecture.
    #[test]
    fn fetches_the_bare_executable() {
        let i = Installed(Layout::under(Path::new("/nonexistent")));
        let want = if cfg!(target_arch = "aarch64") {
            "amigo-blinzel-linux-arm64"
        } else {
            "amigo-blinzel-linux-x64"
        };
        assert_eq!(i.asset(), want);
    }

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

    /// AC-19: the running process keeps the file it was started from (the
    /// previous version), and the installed update is what the next start
    /// runs.
    #[test]
    fn the_next_start_runs_the_installed_update() {
        let (dir, i) = sandbox("next-start");
        std::fs::write(i.0.exe(), b"v1").unwrap();
        let mut running = std::fs::File::open(i.0.exe()).unwrap();
        i.install(b"v2").unwrap();
        let mut kept = Vec::new();
        running.read_to_end(&mut kept).unwrap();
        assert_eq!(
            kept, b"v1",
            "the running process keeps the previous version"
        );
        assert_eq!(read(&i.program()), b"v2", "the next start runs the update");
        assert_eq!(read(&i.old()), b"v1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AC-20: three starts of an update on trial are counted; the fourth
    /// puts the previous executable back, keeps the failed one as
    /// `blinzel.bad`, starts the previous one with the same arguments and
    /// tells this process to exit.
    #[test]
    fn a_failing_update_is_rolled_back_on_the_fourth_start() {
        let (dir, i) = sandbox("rollback");
        let new = Version::parse("9.9.9").unwrap();
        std::fs::write(i.0.exe(), b"v2").unwrap();
        std::fs::write(i.old(), b"v1").unwrap();
        set_state(
            &i,
            &State {
                pending: Some(new),
                ..State::default()
            },
        );
        for n in 1..=3 {
            assert!(!selfupdate::start(&i, new, [], no_restart));
            assert_eq!(state(&i).failed_starts, n);
            assert_eq!(
                (read(&i.0.exe()), read(&i.old())),
                (b"v2".into(), b"v1".into())
            );
        }
        let args = ["photo.jpg", "--resident"].map(OsString::from);
        let mut restarted = None;
        assert!(selfupdate::start(&i, new, args.clone(), |c| restarted = Some(c)));
        let c = restarted.expect("the previous version is started");
        assert_eq!(c.get_program(), i.program());
        assert_eq!(c.get_args().collect::<Vec<_>>(), args);
        assert_eq!(read(&i.0.exe()), b"v1");
        assert_eq!(read(&i.bad()), b"v2");
        assert!(!i.old().exists());
        assert_eq!(state(&i), State::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AC-20: with no previous executable to restore, the trial ends and
    /// this version keeps running.
    #[test]
    fn without_a_previous_executable_the_update_keeps_running() {
        let (dir, i) = sandbox("no-previous");
        let new = Version::parse("9.9.9").unwrap();
        std::fs::write(i.0.exe(), b"v2").unwrap();
        set_state(
            &i,
            &State {
                pending: Some(new),
                failed_starts: 3,
                ..State::default()
            },
        );
        assert!(!selfupdate::start(&i, new, [], no_restart));
        assert_eq!(read(&i.0.exe()), b"v2");
        assert_eq!(state(&i), State::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AC-21: while an update is on trial `blinzel.old` stays; a start
    /// with no update on trial deletes it and `blinzel.bad`.
    #[test]
    fn a_start_without_a_trial_deletes_the_previous_copies() {
        let (dir, i) = sandbox("forget");
        let new = Version::parse("9.9.9").unwrap();
        std::fs::write(i.0.exe(), b"v2").unwrap();
        std::fs::write(i.old(), b"v1").unwrap();
        std::fs::write(i.bad(), b"v0").unwrap();
        set_state(
            &i,
            &State {
                pending: Some(new),
                ..State::default()
            },
        );
        assert!(!selfupdate::start(&i, new, [], no_restart));
        assert!(
            i.old().exists() && i.bad().exists(),
            "kept during the trial"
        );
        set_state(&i, &State::default());
        assert!(!selfupdate::start(&i, new, [], no_restart));
        assert!(!i.old().exists() && !i.bad().exists());
        assert_eq!(read(&i.0.exe()), b"v2");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
