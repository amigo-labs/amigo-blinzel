//! The executable is its own installer (M7): started as
//! `amigo-blinzel-setup-*.exe` or with `--install` it installs itself for
//! the current user; `--uninstall` (the "Installed apps" entry) removes it.
//! `--silent` skips all UI (winget).

pub mod install;
pub mod registry;
pub mod shortcut;
pub mod ui;

use std::path::{Path, PathBuf};

use blinzel_i18n::{Msg, tr};

pub use install::Layout;

/// What the process was started to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Install {
        silent: bool,
    },
    Uninstall {
        silent: bool,
    },
    /// The temporary copy that removes the installation after the
    /// installed executable (process `parent`) has exited.
    UninstallFromTemp {
        silent: bool,
        parent: u32,
    },
}

/// The setup mode for these arguments and executable name, if any.
pub fn mode(args: &[String], exe: &Path) -> Option<Mode> {
    let has = |f: &str| args.iter().any(|a| a == f);
    let silent = has("--silent");
    if let Some(i) = args.iter().position(|a| a == "--uninstall-from-temp") {
        let parent = args.get(i + 1)?.parse().ok()?;
        return Some(Mode::UninstallFromTemp { silent, parent });
    }
    if has("--uninstall") {
        return Some(Mode::Uninstall { silent });
    }
    let named_setup = exe
        .file_name()
        .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().contains("setup"));
    (has("--install") || named_setup).then_some(Mode::Install { silent })
}

/// Runs `mode`; returns the process exit code.
pub fn run(mode: Mode) -> i32 {
    let Some(layout) = Layout::user() else {
        return 1;
    };
    let exe = std::env::current_exe().unwrap_or_default();
    match mode {
        Mode::Install { silent } => {
            if !crate::platform::os::is_supported() {
                return refuse_install(silent, |text| ui::message(text, true));
            }
            if silent {
                match install_now(&layout, &exe) {
                    Ok(()) => 0,
                    Err(_) => 1,
                }
            } else {
                ui::run_install(&layout, &exe)
            }
        }
        Mode::Uninstall { silent } => uninstall(&layout, &exe, silent),
        Mode::UninstallFromTemp { silent, parent } => {
            wait_for_exit(parent, QUIT_WAIT_MS);
            stop_running_instance();
            let r = install::uninstall(&layout);
            if let Err(e) = &r
                && !silent
            {
                ui::message(&e.to_string(), true);
            }
            delete_self_later(&exe);
            i32::from(r.is_err())
        }
    }
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn install_now(layout: &Layout, exe: &Path) -> Result<(), String> {
    install::install(layout, exe, version(), tr(Msg::AppDescription))
}

/// The installed executable cannot delete its own folder: copy it to
/// `%TEMP%`, let the copy finish the job after this process exits.
fn uninstall(layout: &Layout, exe: &Path, silent: bool) -> i32 {
    if !layout.is_installed_exe(exe) {
        stop_running_instance();
        return match install::uninstall(layout) {
            Ok(()) => 0,
            Err(e) => {
                if !silent {
                    ui::message(&e, true);
                }
                1
            }
        };
    }
    let temp: PathBuf = std::env::temp_dir().join(format!(
        "{}-uninstall-{}.exe",
        install::PRODUCT,
        std::process::id()
    ));
    if std::fs::copy(exe, &temp).is_err() {
        return 1;
    }
    let mut cmd = std::process::Command::new(&temp);
    cmd.arg("--uninstall-from-temp")
        .arg(std::process::id().to_string());
    if silent {
        cmd.arg("--silent");
    }
    match cmd.spawn() {
        Ok(_) => 0,
        Err(_) => 1,
    }
}

/// How long the uninstaller waits for each instance it asked to quit.
const QUIT_WAIT_MS: u32 = 30_000;

/// A resident instance keeps the executable open: ask it to exit first.
fn stop_running_instance() {
    stop_instances(crate::instance::quit_all, wait_for_exit);
}

/// Asks every running instance to quit (`quit_all`), then waits for each
/// in turn, up to [`QUIT_WAIT_MS`] (`docs/specs/resident-mode.md` AC-16).
fn stop_instances(quit_all: impl FnOnce() -> Vec<u32>, mut wait: impl FnMut(u32, u32)) {
    for pid in quit_all() {
        wait(pid, QUIT_WAIT_MS);
    }
}

fn wait_for_exit(pid: u32, ms: u32) {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, ms);
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
    }
}

/// A running executable cannot delete itself; a hidden `cmd` does it a
/// moment after this process has exited.
fn delete_self_later(exe: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // raw_arg: cmd.exe has its own quoting rules.
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(format!(
            "/d /c ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"",
            exe.display()
        ))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// An install on a Windows build the viewer does not support: tells the
/// user (the viewer's message) unless `silent`, installs nothing and exits
/// with 2.
fn refuse_install(silent: bool, tell: impl FnOnce(&str)) -> i32 {
    if !silent {
        tell(tr(Msg::ErrorOs));
    }
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/specs/platform-shells.md` AC-01: the setup on an unsupported
    /// build shows the viewer's message, not with `--silent`, and exits
    /// with 2.
    #[test]
    fn an_unsupported_windows_refuses_the_install_with_2() {
        let mut told = vec![];
        assert_eq!(refuse_install(false, |t| told.push(t.to_string())), 2);
        assert_eq!(told, [tr(Msg::ErrorOs)]);
        assert_eq!(refuse_install(true, |_| panic!("silent")), 2);
        assert_eq!(
            blinzel_i18n::tr_in(blinzel_i18n::FALLBACK, Msg::ErrorOs),
            "amigo-blinzel requires Windows 11 version 24H2 or newer."
        );
    }

    /// `docs/specs/resident-mode.md` AC-16: before uninstalling, every
    /// running instance is asked to quit, then each is awaited for 30 s.
    #[test]
    fn uninstalling_asks_every_instance_to_quit_and_waits_30_s_each() {
        let mut waited = vec![];
        stop_instances(|| vec![7, 9], |pid, ms| waited.push((pid, ms)));
        assert_eq!(waited, [(7, 30_000), (9, 30_000)]);
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_modes() {
        let viewer = Path::new(r"C:\x\blinzel.exe");
        let setup = Path::new(r"C:\Downloads\amigo-blinzel-setup-x64.exe");
        assert_eq!(mode(&args(&["photo.jpg"]), viewer), None);
        assert_eq!(
            mode(&args(&[]), setup),
            Some(Mode::Install { silent: false })
        );
        assert_eq!(
            mode(&args(&["--install", "--silent"]), viewer),
            Some(Mode::Install { silent: true })
        );
        assert_eq!(
            mode(&args(&["--uninstall"]), viewer),
            Some(Mode::Uninstall { silent: false })
        );
        assert_eq!(
            mode(&args(&["--uninstall-from-temp", "42", "--silent"]), viewer),
            Some(Mode::UninstallFromTemp {
                silent: true,
                parent: 42
            })
        );
    }
}
