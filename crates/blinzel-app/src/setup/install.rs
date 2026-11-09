//! Per-user installation without admin rights: files, Start menu shortcut,
//! file type registration and the "Installed apps" entry, all below
//! `%LOCALAPPDATA%`, `%APPDATA%` and HKCU.

use std::path::{Path, PathBuf};

use super::{registry, shortcut};
use crate::DISPLAY_NAME;

/// Folders, registry keys and the sign-in value; the name people see is
/// [`DISPLAY_NAME`].
pub const PRODUCT: &str = "amigo-blinzel";
pub const PUBLISHER: &str = "amigo-labs";
pub const EXE_NAME: &str = "blinzel.exe";
pub const PROG_ID: &str = "amigo-blinzel.Image";
pub const URL: &str = "https://github.com/amigo-labs/amigo-blinzel";

/// File types offered under "Open with" and in the default apps settings:
/// the native decoders plus what Windows' codecs (WIC) add.
pub const EXTENSIONS: &[&str] = &[
    ".jpg", ".jpeg", ".jpe", ".jfif", ".png", ".gif", ".bmp", ".dib", ".tif", ".tiff", ".ico",
    ".webp", ".heic", ".heif", ".avif", ".jxl",
];

/// Where an installation lives. [`Layout::user`] is the real one; tests use
/// [`Layout::under`] so they never touch the user's registration.
#[derive(Clone, Debug)]
pub struct Layout {
    pub dir: PathBuf,
    pub start_menu: PathBuf,
    /// Cache that belongs to the installation (downloaded updates).
    pub cache: PathBuf,
    /// Registry paths below HKCU.
    pub classes: String,
    pub app_key: String,
    pub registered_apps: String,
    pub uninstall: String,
    pub app_paths: String,
    /// Programs started at sign-in (resident mode).
    pub run: String,
    /// Tell the shell about new associations (off in tests).
    pub notify_shell: bool,
}

impl Layout {
    pub fn user() -> Option<Layout> {
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
        let roaming = PathBuf::from(std::env::var_os("APPDATA")?);
        Some(Layout::in_profile(&local, &roaming))
    }

    /// The real installation of the user whose `%LOCALAPPDATA%` is `local`
    /// and `%APPDATA%` is `roaming`; every registry path is below HKCU.
    pub fn in_profile(local: &Path, roaming: &Path) -> Layout {
        Layout {
            dir: local.join("Programs").join(PRODUCT),
            start_menu: roaming.join(r"Microsoft\Windows\Start Menu\Programs"),
            cache: local.join(PRODUCT),
            classes: r"Software\Classes".into(),
            app_key: format!(r"Software\{PRODUCT}"),
            registered_apps: r"Software\RegisteredApplications".into(),
            uninstall: format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{PRODUCT}"),
            app_paths: format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{EXE_NAME}"),
            run: r"Software\Microsoft\Windows\CurrentVersion\Run".into(),
            notify_shell: true,
        }
    }

    /// Everything below `root` (files) and `HKCU\<reg_root>` (registry).
    pub fn under(root: &Path, reg_root: &str) -> Layout {
        Layout {
            dir: root.join("Programs").join(PRODUCT),
            start_menu: root.join("Start Menu"),
            cache: root.join("cache"),
            classes: format!(r"{reg_root}\Classes"),
            app_key: format!(r"{reg_root}\App"),
            registered_apps: format!(r"{reg_root}\RegisteredApplications"),
            uninstall: format!(r"{reg_root}\Uninstall"),
            app_paths: format!(r"{reg_root}\AppPaths"),
            run: format!(r"{reg_root}\Run"),
            notify_shell: false,
        }
    }

    pub fn exe(&self) -> PathBuf {
        self.dir.join(EXE_NAME)
    }

    pub fn shortcut(&self) -> PathBuf {
        self.start_menu.join(format!("{DISPLAY_NAME}.lnk"))
    }

    /// The shortcut as versions up to 1.5 named it.
    fn legacy_shortcut(&self) -> PathBuf {
        self.start_menu.join(format!("{PRODUCT}.lnk"))
    }

    /// Whether `exe` is (or would be) the installed executable.
    pub fn is_installed_exe(&self, exe: &Path) -> bool {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(exe) == canon(&self.exe())
    }

    pub fn is_installed(&self) -> bool {
        registry::get_str(&self.uninstall, "DisplayVersion").is_some()
    }

    pub fn installed_version(&self) -> Option<String> {
        registry::get_str(&self.uninstall, "DisplayVersion")
    }
}

/// Installs (or updates in place) `source`, the running setup executable.
pub fn install(
    layout: &Layout,
    source: &Path,
    version: &str,
    description: &str,
) -> Result<(), String> {
    let io = |what: &str, p: &Path| {
        let what = what.to_string();
        let p = p.display().to_string();
        move |e: std::io::Error| format!("{what} {p}: {e}")
    };
    std::fs::create_dir_all(&layout.dir).map_err(io("create", &layout.dir))?;
    let exe = layout.exe();
    if !layout.is_installed_exe(source) {
        place_exe(source, &exe)?;
    }
    std::fs::create_dir_all(&layout.start_menu).map_err(io("create", &layout.start_menu))?;
    shortcut::create(&layout.shortcut(), &exe, description)?;
    let _ = std::fs::remove_file(layout.legacy_shortcut());
    register(layout, &exe, version, description)?;
    if layout.notify_shell {
        notify_associations_changed();
    }
    Ok(())
}

/// Copies `source` to `exe`. An installed older version may be running:
/// Windows does not let it be overwritten, but renamed, so it moves to
/// `blinzel.exe.old` first (replacing an earlier one).
fn place_exe(source: &Path, exe: &Path) -> Result<(), String> {
    if exe.exists() {
        let old = exe.with_extension("exe.old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|e| format!("rename {}: {e}", exe.display()))?;
    }
    std::fs::copy(source, exe)
        .map(drop)
        .map_err(|e| format!("copy to {}: {e}", exe.display()))
}

fn register(layout: &Layout, exe: &Path, version: &str, description: &str) -> Result<(), String> {
    let exe_s = exe.display().to_string();
    let dir_s = layout.dir.display().to_string();
    let open = format!("\"{exe_s}\" \"%1\"");
    let icon = format!("\"{exe_s}\",0");
    let classes = &layout.classes;

    let prog = registry::Key::create(&format!(r"{classes}\{PROG_ID}"))?;
    prog.set_str("", DISPLAY_NAME)?;
    registry::Key::create(&format!(r"{classes}\{PROG_ID}\DefaultIcon"))?.set_str("", &icon)?;
    registry::Key::create(&format!(r"{classes}\{PROG_ID}\shell\open\command"))?
        .set_str("", &open)?;

    let app = format!(r"{classes}\Applications\{EXE_NAME}");
    registry::Key::create(&format!(r"{app}\shell\open\command"))?.set_str("", &open)?;
    let supported = registry::Key::create(&format!(r"{app}\SupportedTypes"))?;
    let caps = format!(r"{}\Capabilities", layout.app_key);
    let caps_key = registry::Key::create(&caps)?;
    caps_key.set_str("ApplicationName", DISPLAY_NAME)?;
    caps_key.set_str("ApplicationDescription", description)?;
    caps_key.set_str("ApplicationIcon", &icon)?;
    let assoc = registry::Key::create(&format!(r"{caps}\FileAssociations"))?;
    for ext in EXTENSIONS {
        supported.set_str(ext, "")?;
        assoc.set_str(ext, PROG_ID)?;
        registry::Key::create(&format!(r"{classes}\{ext}\OpenWithProgids"))?.set_none(PROG_ID)?;
    }
    registry::Key::create(&layout.registered_apps)?.set_str(PRODUCT, &caps)?;

    let paths = registry::Key::create(&layout.app_paths)?;
    paths.set_str("", &exe_s)?;
    paths.set_str("Path", &dir_s)?;

    let size_kb = std::fs::metadata(exe).map(|m| m.len() / 1024).unwrap_or(0) as u32;
    let u = registry::Key::create(&layout.uninstall)?;
    u.set_str("DisplayName", DISPLAY_NAME)?;
    u.set_str("DisplayVersion", version)?;
    u.set_str("Publisher", PUBLISHER)?;
    u.set_str("DisplayIcon", &icon)?;
    u.set_str("InstallLocation", &dir_s)?;
    u.set_str("UninstallString", &format!("\"{exe_s}\" --uninstall"))?;
    u.set_str(
        "QuietUninstallString",
        &format!("\"{exe_s}\" --uninstall --silent"),
    )?;
    u.set_str("URLInfoAbout", URL)?;
    u.set_dword("NoModify", 1)?;
    u.set_dword("NoRepair", 1)?;
    u.set_dword("EstimatedSize", size_kb)?;
    Ok(())
}

/// Removes everything [`install`] created. Must not run from the installed
/// executable (it deletes the folder); see `setup::uninstall`.
pub fn uninstall(layout: &Layout) -> Result<(), String> {
    let classes = &layout.classes;
    let mut errors = Vec::new();
    let mut note = |r: Result<(), String>| {
        if let Err(e) = r {
            errors.push(e);
        }
    };
    for ext in EXTENSIONS {
        note(registry::delete_value(
            &format!(r"{classes}\{ext}\OpenWithProgids"),
            PROG_ID,
        ));
    }
    note(registry::delete_tree(&format!(r"{classes}\{PROG_ID}")));
    note(registry::delete_tree(&format!(
        r"{classes}\Applications\{EXE_NAME}"
    )));
    note(registry::delete_value(&layout.registered_apps, PRODUCT));
    note(registry::delete_tree(&layout.app_key));
    note(registry::delete_tree(&layout.app_paths));
    note(registry::delete_tree(&layout.uninstall));
    note(registry::delete_value(&layout.run, PRODUCT));
    let _ = std::fs::remove_file(layout.shortcut());
    let _ = std::fs::remove_file(layout.legacy_shortcut());
    for dir in [&layout.dir, &layout.cache] {
        if dir.exists() {
            note(std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display())));
        }
    }
    if layout.notify_shell {
        notify_associations_changed();
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// The sign-in command line of the resident instance.
pub fn autostart_command(exe: &Path) -> String {
    format!("\"{}\" --resident", exe.display())
}

/// Starts `exe` hidden at sign-in (resident mode), or with `None` no
/// longer.
pub fn set_autostart(layout: &Layout, exe: Option<&Path>) -> Result<(), String> {
    match exe {
        Some(exe) => registry::Key::create(&layout.run)?.set_str(PRODUCT, &autostart_command(exe)),
        None => registry::delete_value(&layout.run, PRODUCT),
    }
}

pub fn autostart(layout: &Layout) -> Option<String> {
    registry::get_str(&layout.run, PRODUCT)
}

/// Points the sign-in value at `exe` when it names another executable
/// (moved or reinstalled); `true` if it was rewritten.
pub fn refresh_autostart(layout: &Layout, exe: &Path) -> Result<bool, String> {
    if autostart(layout).as_deref() == Some(&autostart_command(exe)) {
        return Ok(false);
    }
    set_autostart(layout, Some(exe)).map(|()| true)
}

fn notify_associations_changed() {
    use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-04: the real installation is per user: the program below
    /// `%LOCALAPPDATA%\Programs`, the shortcut in the user's Start menu, the
    /// cache in `%LOCALAPPDATA%\amigo-blinzel`, every registry path relative
    /// to HKCU (`registry` opens nothing else).
    #[test]
    fn the_real_installation_is_per_user_below_hkcu() {
        let local = Path::new(r"C:\Users\a\AppData\Local");
        let roaming = Path::new(r"C:\Users\a\AppData\Roaming");
        let l = Layout::in_profile(local, roaming);
        assert_eq!(
            l.exe(),
            Path::new(r"C:\Users\a\AppData\Local\Programs\amigo-blinzel\blinzel.exe")
        );
        assert_eq!(
            l.shortcut(),
            Path::new(
                r"C:\Users\a\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Blinzel.lnk"
            )
        );
        assert_eq!(
            l.cache,
            Path::new(r"C:\Users\a\AppData\Local\amigo-blinzel")
        );
        assert_eq!(l.classes, r"Software\Classes");
        assert_eq!(l.app_key, r"Software\amigo-blinzel");
        assert_eq!(l.registered_apps, r"Software\RegisteredApplications");
        assert_eq!(
            l.uninstall,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\amigo-blinzel"
        );
        assert_eq!(
            l.app_paths,
            r"Software\Microsoft\Windows\CurrentVersion\App Paths\blinzel.exe"
        );
        assert_eq!(l.run, r"Software\Microsoft\Windows\CurrentVersion\Run");
        assert!(l.notify_shell);

        let real = Layout::user().expect("LOCALAPPDATA and APPDATA");
        let local = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
        assert_eq!(real.dir, local.join("Programs").join(PRODUCT));
    }

    /// AC-05: an installed older version that is running cannot be
    /// overwritten; it is renamed to `blinzel.exe.old` and the new
    /// executable is copied in its place.
    #[test]
    fn a_running_old_version_is_renamed_to_old() {
        let dir = std::env::temp_dir().join(format!("blinzel-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(EXE_NAME);
        let windir = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        let ping = windir.join("System32").join("PING.EXE");
        std::fs::copy(&ping, &exe).unwrap();
        let mut running = std::process::Command::new(&exe)
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        assert!(
            std::fs::OpenOptions::new().write(true).open(&exe).is_err(),
            "a running executable cannot be overwritten"
        );
        let source = std::env::current_exe().unwrap();
        let placed = place_exe(&source, &exe);
        let _ = running.kill();
        let _ = running.wait();
        placed.unwrap();
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            std::fs::read(&source).unwrap()
        );
        let old = dir.join("blinzel.exe.old");
        assert_eq!(std::fs::read(&old).unwrap(), std::fs::read(&ping).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
