//! Installation and uninstallation in a sandbox: a temporary folder and a
//! private HKCU branch, so the user's real registration is never touched.

#![cfg(windows)]

use blinzel_app::setup::{Layout, install, registry};

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

    // Installing over an existing installation updates it in place.
    install::install(&layout, &src, "9.9.10", "test").unwrap();
    assert_eq!(layout.installed_version().as_deref(), Some("9.9.10"));

    install::uninstall(&layout).unwrap();
    assert!(!layout.dir.exists());
    assert!(!layout.shortcut().exists());
    assert_eq!(layout.installed_version(), None);
    assert!(!registry::value_exists(&open_with, install::PROG_ID));
    assert_eq!(registry::get_str(&command, ""), None);

    registry::delete_tree(&reg).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
