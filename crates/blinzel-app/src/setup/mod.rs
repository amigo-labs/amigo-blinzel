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
                if !silent {
                    ui::message(tr(Msg::ErrorOs), true);
                }
                return 2;
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
            wait_for_exit(parent);
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
        return match install::uninstall(layout) {
            Ok(()) => 0,
            Err(_) => 1,
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

fn wait_for_exit(pid: u32) {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, 30_000);
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

#[cfg(test)]
mod tests {
    use super::*;

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
