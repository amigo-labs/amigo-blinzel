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
    let tell = |text: &str| ui::message(text, true);
    match mode {
        Mode::Install { silent } => install_mode(
            silent,
            crate::platform::os::refusal(crate::platform::os::build_number()),
            || install_now(&layout, &exe),
            || ui::run_install(&layout, &exe),
            tell,
        ),
        Mode::Uninstall { silent } => uninstall(&layout, &exe, silent),
        Mode::UninstallFromTemp { silent, parent } => finish_from_temp(
            parent,
            silent,
            wait_for_exit,
            stop_running_instance,
            || install::uninstall(&layout),
            tell,
            || delete_self_later(&exe),
        ),
    }
}

/// `--install`: on a Windows build the viewer refuses (`refusal`, its
/// message), nothing is installed and the exit code is 2, with the message
/// unless `silent`; otherwise `silent` installs without any window (0
/// installed, 1 failed) and an interactive install runs the setup window.
fn install_mode(
    silent: bool,
    refusal: Option<&str>,
    install: impl FnOnce() -> Result<(), String>,
    interactive: impl FnOnce() -> i32,
    tell: impl FnOnce(&str),
) -> i32 {
    if let Some(text) = refusal {
        if !silent {
            tell(text);
        }
        return 2;
    }
    if silent {
        i32::from(install().is_err())
    } else {
        interactive()
    }
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn install_now(layout: &Layout, exe: &Path) -> Result<(), String> {
    install::install(layout, exe, version(), tr(Msg::AppDescription))
}

/// `--uninstall`. The installed executable cannot delete its own folder:
/// it hands the job to a copy in `%TEMP%`; any other copy (the downloaded
/// setup) uninstalls directly.
fn uninstall(layout: &Layout, exe: &Path, silent: bool) -> i32 {
    if !layout.is_installed_exe(exe) {
        return remove(
            silent,
            stop_running_instance,
            || install::uninstall(layout),
            |text| ui::message(text, true),
        );
    }
    hand_to_temp(
        exe,
        &std::env::temp_dir(),
        std::process::id(),
        silent,
        |copy, args| {
            std::process::Command::new(copy)
                .args(args)
                .spawn()
                .map(drop)
        },
    )
}

/// Copies `exe` into `temp_dir` and starts the copy (`spawn`) to finish
/// the uninstallation after this process (`pid`) has exited; 0 once the
/// copy runs, 1 if it could not be copied or started.
fn hand_to_temp(
    exe: &Path,
    temp_dir: &Path,
    pid: u32,
    silent: bool,
    spawn: impl FnOnce(&Path, &[String]) -> std::io::Result<()>,
) -> i32 {
    let copy: PathBuf = temp_dir.join(format!("{}-uninstall-{pid}.exe", install::PRODUCT));
    if std::fs::copy(exe, &copy).is_err() {
        return 1;
    }
    let mut args = vec!["--uninstall-from-temp".to_string(), pid.to_string()];
    if silent {
        args.push("--silent".into());
    }
    i32::from(spawn(&copy, &args).is_err())
}

/// The `%TEMP%` copy: waits for the installed executable (`parent`) to
/// exit, removes the installation and deletes itself afterwards.
fn finish_from_temp(
    parent: u32,
    silent: bool,
    wait: impl FnOnce(u32, u32),
    stop: impl FnOnce(),
    uninstall: impl FnOnce() -> Result<(), String>,
    tell: impl FnOnce(&str),
    delete_self: impl FnOnce(),
) -> i32 {
    wait(parent, QUIT_WAIT_MS);
    let code = remove(silent, stop, uninstall, tell);
    delete_self();
    code
}

/// Asks running instances to quit (`stop`), then removes the installation;
/// a failure is shown (`tell`) unless `silent` and exits with 1.
fn remove(
    silent: bool,
    stop: impl FnOnce(),
    uninstall: impl FnOnce() -> Result<(), String>,
    tell: impl FnOnce(&str),
) -> i32 {
    stop();
    match uninstall() {
        Ok(()) => 0,
        Err(e) => {
            if !silent {
                tell(&e);
            }
            1
        }
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
        .raw_arg(delete_command(exe))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// `cmd.exe`'s arguments that delete `exe` about two seconds later.
fn delete_command(exe: &Path) -> String {
    format!(
        "/d /c ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"",
        exe.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/specs/platform-shells.md` AC-01, `installation.md` AC-09: the
    /// setup on an unsupported build shows the viewer's message, not with
    /// `--silent`, installs nothing and exits with 2.
    #[test]
    fn an_unsupported_windows_refuses_the_install_with_2() {
        use crate::platform::os::refusal;
        let nothing = || -> Result<(), String> { panic!("installed") };
        let mut told = vec![];
        let tell = |t: &str| told.push(t.to_string());
        assert_eq!(
            install_mode(false, refusal(22631), nothing, || panic!(), tell),
            2
        );
        assert_eq!(told, [tr(Msg::ErrorOs)]);
        let quiet = |_: &str| panic!("silent");
        assert_eq!(
            install_mode(true, refusal(22631), nothing, || panic!(), quiet),
            2
        );
        assert_eq!(
            blinzel_i18n::tr_in(blinzel_i18n::FALLBACK, Msg::ErrorOs),
            "amigo-blinzel requires Windows 11 version 24H2 or newer."
        );
    }

    /// AC-09: the viewer refuses an old build with the same message as the
    /// setup, and starts on a supported one.
    #[test]
    fn the_viewer_refuses_an_old_windows_with_the_setups_message() {
        use crate::platform::os::refusal;
        assert_eq!(refusal(26099), Some(tr(Msg::ErrorOs)));
        assert_eq!(refusal(0), Some(tr(Msg::ErrorOs)));
        assert_eq!(refusal(26100), None);
    }

    /// AC-08: `--silent` installs without a window or a message box and
    /// exits with 0 installed, 1 failed; without it the setup window runs.
    #[test]
    fn a_silent_install_shows_nothing_and_exits_0_or_1() {
        let quiet = |_: &str| panic!("message box");
        let no_window = || panic!("window");
        assert_eq!(install_mode(true, None, || Ok(()), no_window, quiet), 0);
        let failed = || Err("copy to x: denied".to_string());
        assert_eq!(install_mode(true, None, failed, no_window, quiet), 1);
        let not_here = || -> Result<(), String> { panic!("the window installs") };
        assert_eq!(install_mode(false, None, not_here, || 3, quiet), 3);
    }

    /// AC-07, AC-08: the installed executable copies itself to `%TEMP%`,
    /// starts the copy with its own process id (and `--silent`) and exits
    /// with 0 at once; 1 if the copy cannot be made or started.
    #[test]
    fn the_installed_exe_hands_the_uninstall_to_a_temp_copy_and_exits_0() {
        let dir = std::env::temp_dir().join(format!("blinzel-hand-{}", std::process::id()));
        let temp = dir.join("temp");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&temp).unwrap();
        let exe = dir.join("blinzel.exe");
        std::fs::write(&exe, b"exe").unwrap();
        for silent in [false, true] {
            let mut started = None;
            let code = hand_to_temp(&exe, &temp, 42, silent, |copy, args| {
                started = Some((copy.to_path_buf(), args.to_vec()));
                Ok(())
            });
            assert_eq!(code, 0);
            let (copy, args) = started.expect("the copy was not started");
            assert_eq!(copy, temp.join("amigo-blinzel-uninstall-42.exe"));
            assert_eq!(std::fs::read(&copy).unwrap(), b"exe");
            assert_eq!(
                mode(&args, &copy),
                Some(Mode::UninstallFromTemp { silent, parent: 42 })
            );
        }
        let refused = |_: &Path, _: &[String]| Err(std::io::Error::other("refused"));
        assert_eq!(hand_to_temp(&exe, &temp, 42, false, refused), 1);
        let gone = dir.join("gone.exe");
        assert_eq!(hand_to_temp(&gone, &temp, 42, false, |_, _| Ok(())), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AC-07: the `%TEMP%` copy waits up to 30 s for the installed
    /// executable, asks running instances to quit, removes the installation
    /// and then deletes itself; a failure is shown unless silent and exits
    /// with 1 (AC-08).
    #[test]
    fn the_temp_copy_waits_stops_removes_and_then_deletes_itself() {
        use std::cell::RefCell;
        let steps = RefCell::new(Vec::<String>::new());
        let step = |s: &str| steps.borrow_mut().push(s.to_string());
        let code = finish_from_temp(
            42,
            false,
            |pid, ms| step(&format!("wait {pid} {ms}")),
            || step("stop"),
            || {
                step("remove");
                Ok(())
            },
            |_| panic!("no error to show"),
            || step("delete self"),
        );
        assert_eq!(code, 0);
        assert_eq!(
            *steps.borrow(),
            ["wait 42 30000", "stop", "remove", "delete self"]
        );

        for silent in [false, true] {
            let mut told = vec![];
            let mut deleted = false;
            let code = finish_from_temp(
                42,
                silent,
                |_, _| {},
                || {},
                || Err("a: denied; b: denied".into()),
                |t| told.push(t.to_string()),
                || deleted = true,
            );
            assert_eq!(code, 1);
            assert!(deleted, "deletes itself after a failure too");
            let shown: &[&str] = if silent {
                &[]
            } else {
                &["a: denied; b: denied"]
            };
            assert_eq!(told, shown);
        }
    }

    /// AC-07: the copy deletes itself through a hidden `cmd` a moment after
    /// it has exited.
    #[test]
    fn the_temp_copy_is_deleted_after_it_exits() {
        let exe = Path::new(r"C:\Users\a b\AppData\Local\Temp\amigo-blinzel-uninstall-42.exe");
        assert_eq!(
            delete_command(exe),
            r#"/d /c ping -n 3 127.0.0.1 >nul & del /f /q "C:\Users\a b\AppData\Local\Temp\amigo-blinzel-uninstall-42.exe""#
        );
    }

    /// Edge cases: uninstalling from another copy (the downloaded setup)
    /// asks running instances to quit first, then removes; a failure is
    /// shown unless silent and exits with 1.
    #[test]
    fn a_direct_uninstall_shows_its_error_unless_silent() {
        let mut stopped = false;
        assert_eq!(remove(false, || stopped = true, || Ok(()), |_| panic!()), 0);
        assert!(stopped);
        let mut told = vec![];
        let failed = || Err("x: denied".to_string());
        assert_eq!(
            remove(false, || {}, failed, |t| told.push(t.to_string())),
            1
        );
        assert_eq!(told, ["x: denied"]);
        assert_eq!(remove(true, || {}, failed, |_| panic!("silent")), 1);
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
