//! Installation and uninstallation in a sandbox: a temporary folder and a
//! private HKCU branch, so the user's real registration is never touched.

#![cfg(windows)]

use blinzel_app::DISPLAY_NAME;
use blinzel_app::setup::{Layout, install, registry};
use blinzel_i18n::{Msg, tr};

#[test]
fn installs_updates_and_uninstalls_in_a_sandbox() {
    let id = std::process::id();
    let root = std::env::temp_dir().join(format!("blinzel-setup-test-{id}"));
    let reg = format!(r"Software\amigo-blinzel-test-{id}");
    let layout = Layout::under(&root, &reg);
    let src = std::env::current_exe().unwrap();

    install::install(&layout, &src, "9.9.9", "test").unwrap();
    assert!(layout.exe().is_file());
    assert!(layout.shortcut().is_file());
    // The name people see is "Blinzel"; folders and keys keep amigo-blinzel.
    assert_eq!(tr(Msg::AppName), DISPLAY_NAME);
    assert_eq!(
        layout.shortcut().file_name(),
        Some(std::ffi::OsStr::new("Blinzel.lnk"))
    );
    assert_eq!(
        registry::get_str(&layout.uninstall, "DisplayName").as_deref(),
        Some(DISPLAY_NAME)
    );
    let caps = format!(r"{}\Capabilities", layout.app_key);
    assert_eq!(
        registry::get_str(&caps, "ApplicationName").as_deref(),
        Some(DISPLAY_NAME)
    );
    assert!(layout.dir.ends_with("amigo-blinzel"));
    assert_eq!(layout.installed_version().as_deref(), Some("9.9.9"));
    let command = format!(
        r"{}\{}\shell\open\command",
        layout.classes,
        install::PROG_ID
    );
    assert_eq!(
        registry::get_str(&command, ""),
        Some(format!("\"{}\" \"%1\"", layout.exe().display()))
    );
    let open_with = format!(r"{}\.jpg\OpenWithProgids", layout.classes);
    assert!(registry::value_exists(&open_with, install::PROG_ID));
    assert_eq!(
        registry::get_str(&layout.registered_apps, install::PRODUCT),
        Some(format!(r"{}\Capabilities", layout.app_key))
    );
    let uninstall = registry::get_str(&layout.uninstall, "UninstallString").unwrap();
    assert!(uninstall.ends_with("\" --uninstall"));
    // AC-04: the entries are below HKEY_CURRENT_USER, as reg.exe sees them,
    // and nothing went to HKEY_LOCAL_MACHINE.
    assert!(reg_query("HKCU", &layout.uninstall).contains("9.9.9"));
    assert!(reg_query("HKCU", &command).contains("%1"));
    assert_eq!(reg_query("HKLM", &layout.uninstall), "");

    // Installing over an existing installation updates it in place, and
    // removes the shortcut versions up to 1.5 created as amigo-blinzel.lnk.
    let legacy = layout.start_menu.join("amigo-blinzel.lnk");
    std::fs::copy(layout.shortcut(), &legacy).unwrap();
    install::install(&layout, &src, "9.9.10", "test").unwrap();
    assert_eq!(layout.installed_version().as_deref(), Some("9.9.10"));
    assert!(layout.shortcut().is_file());
    assert!(!legacy.exists());

    // Resident mode starts the app hidden at sign-in.
    install::set_autostart(&layout, Some(&layout.exe())).unwrap();
    assert_eq!(
        install::autostart(&layout),
        Some(format!("\"{}\" --resident", layout.exe().display()))
    );
    install::set_autostart(&layout, None).unwrap();
    assert_eq!(install::autostart(&layout), None);
    // resident-mode.md AC-16: a value left by a moved or reinstalled copy
    // is rewritten to the running executable, a current one left alone.
    let moved = root.join("old").join("blinzel.exe");
    install::set_autostart(&layout, Some(&moved)).unwrap();
    assert_eq!(install::refresh_autostart(&layout, &layout.exe()), Ok(true));
    assert_eq!(
        install::autostart(&layout),
        Some(install::autostart_command(&layout.exe()))
    );
    assert_eq!(
        install::refresh_autostart(&layout, &layout.exe()),
        Ok(false)
    );

    // AC-07: the viewer's cache folder goes with the installation.
    std::fs::create_dir_all(&layout.cache).unwrap();
    std::fs::write(layout.cache.join("update.exe"), b"x").unwrap();
    install::uninstall(&layout).unwrap();
    assert!(!layout.dir.exists());
    assert!(!layout.cache.exists());
    assert!(!layout.shortcut().exists());
    assert_eq!(layout.installed_version(), None);
    assert!(!registry::value_exists(&open_with, install::PROG_ID));
    assert_eq!(registry::get_str(&command, ""), None);
    assert_eq!(install::autostart(&layout), None);

    registry::delete_tree(&reg).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// What `reg query <root>\<key>` prints, or "" if the key does not exist.
fn reg_query(root: &str, key: &str) -> String {
    let out = std::process::Command::new("reg")
        .args(["query", &format!(r"{root}\{key}")])
        .output()
        .unwrap();
    if out.status.success() {
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        String::new()
    }
}
