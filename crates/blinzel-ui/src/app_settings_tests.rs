//! App-level tests of the settings file: what the menu offers and writes,
//! when the file is written and what happens when it cannot be
//! (`docs/specs/settings.md`).

use super::*;
use crate::menu::{self, MenuItem, Platform};
use crate::testkit::Harness;
use Cmd::*;

const WINDOWS: Platform = Platform {
    resident: true,
    screenshots: true,
    mac: false,
};
const MACOS: Platform = Platform {
    resident: true,
    screenshots: true,
    mac: true,
};
const LINUX: Platform = Platform {
    resident: false,
    screenshots: false,
    mac: false,
};

/// Settings that differ from the defaults, file-only keys included.
fn custom() -> Settings {
    Settings {
        language: Some("fr".into()),
        prefetch_ahead: 7,
        prefetch_behind: 3,
        background: [0x12, 0x34, 0x56],
        filmstrip: true,
        screenshot_dir: Some("/home/a/Shots #1".into()),
        editor: Some("/usr/bin/gimp".into()),
        ..Settings::default()
    }
}

/// Menu → Settings as platform `p` shows it for the App's state.
fn settings_menu(h: &Harness, p: Platform) -> Vec<MenuItem> {
    let title = tr(Msg::MenuSettings);
    menu::items_for(&h.menu_state(), p)
        .into_iter()
        .find_map(|i| match i {
            MenuItem::Sub(t, items) if t == title => Some(items),
            _ => None,
        })
        .expect("a Settings submenu")
}

/// The commands of `items` (`None`: a separator).
fn cmds(items: &[MenuItem]) -> Vec<Option<Cmd>> {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Item { cmd, .. } => Some(*cmd),
            _ => None,
        })
        .collect()
}

/// The checked entries of `items`.
fn checked(items: &[MenuItem]) -> Vec<Cmd> {
    items
        .iter()
        .filter_map(|i| match i {
            MenuItem::Item {
                cmd, checked: true, ..
            } => Some(*cmd),
            _ => None,
        })
        .collect()
}

/// Every setting the Settings menu shows as a check item, all off.
fn all_off(s: &mut Settings) {
    s.resident = false;
    s.single_instance = false;
    s.color_management = false;
    s.auto_update = false;
    s.screenshot_hotkey = false;
    s.screenshot_save = false;
    s.screenshot_open = false;
    s.screenshot_edit = false;
}

#[test]
fn menu_settings_offers_each_platforms_entries() {
    let h = Harness::new();
    let common = [
        (ToggleSingleInstance, "Open images in the same window"),
        (ToggleColorManagement, "Use colour profiles (ICC)"),
        (ToggleAutoUpdate, "Install updates automatically"),
        (ChooseEditor, "Choose editor…"),
    ];
    let screenshots = |key: &'static str| {
        [
            (ToggleScreenshotHotkey, key),
            (ToggleScreenshotSave, "Save screenshots in the folder"),
            (ToggleScreenshotOpen, "Open screenshots in the viewer"),
            (ToggleScreenshotEdit, "Open screenshots in the editor"),
            (ScreenshotFolder, "Screenshot folder…"),
        ]
    };
    let resident = (ToggleResident, "Keep ready in the background");
    let with_shots = |key| {
        let mut e: Vec<Option<(Cmd, &str)>> = vec![Some(resident)];
        e.extend(common.map(Some));
        e.push(None);
        e.extend(screenshots(key).map(Some));
        e
    };
    for (p, expected) in [
        (WINDOWS, with_shots("Print key takes screenshots")),
        (MACOS, with_shots("⌘⇧1 takes screenshots")),
        (LINUX, common.map(Some).to_vec()),
    ] {
        let items = settings_menu(&h, p);
        let got: Vec<Option<(Cmd, String)>> = items
            .iter()
            .map(|i| match i {
                MenuItem::Item { cmd, label, .. } => Some((*cmd, label.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(got.len(), expected.len(), "{p:?}: {got:?}");
        for (g, e) in got.iter().zip(&expected) {
            match (g, e) {
                (Some((gc, gl)), Some((ec, el))) => {
                    assert_eq!(gc, ec, "{p:?}");
                    assert!(gl.starts_with(el), "{p:?}: {gl:?} for {el:?}");
                }
                (None, None) => {}
                _ => panic!("{p:?}: {got:?}"),
            }
        }
    }
    // This build shows its own platform's menu.
    let this = if cfg!(windows) {
        WINDOWS
    } else if cfg!(target_os = "macos") {
        MACOS
    } else {
        LINUX
    };
    let own = menu::items(&h.menu_state());
    assert_eq!(own, menu::items_for(&h.menu_state(), this));
}

/// Turns one setting on.
type TurnOn = fn(&mut Settings);

#[test]
fn each_check_item_is_checked_when_its_setting_is_on() {
    let mut h = Harness::new();
    let setters: [(Cmd, TurnOn); 8] = [
        (ToggleResident, |s| s.resident = true),
        (ToggleSingleInstance, |s| s.single_instance = true),
        (ToggleColorManagement, |s| s.color_management = true),
        (ToggleAutoUpdate, |s| s.auto_update = true),
        (ToggleScreenshotHotkey, |s| s.screenshot_hotkey = true),
        (ToggleScreenshotSave, |s| s.screenshot_save = true),
        (ToggleScreenshotOpen, |s| s.screenshot_open = true),
        (ToggleScreenshotEdit, |s| s.screenshot_edit = true),
    ];
    for p in [WINDOWS, MACOS] {
        all_off(&mut h.app.settings);
        assert_eq!(checked(&settings_menu(&h, p)), [], "{p:?}");
        for (cmd, on) in setters {
            all_off(&mut h.app.settings);
            on(&mut h.app.settings);
            // The screenshot key only while resident mode is on too.
            let expected: &[Cmd] = if cmd == ToggleScreenshotHotkey {
                &[]
            } else {
                &[cmd]
            };
            assert_eq!(checked(&settings_menu(&h, p)), expected, "{p:?} {cmd:?}");
        }
        all_off(&mut h.app.settings);
        h.app.settings.resident = true;
        h.app.settings.screenshot_hotkey = true;
        assert_eq!(
            checked(&settings_menu(&h, p)),
            [ToggleResident, ToggleScreenshotHotkey],
            "{p:?}"
        );
        for (_, on) in setters {
            on(&mut h.app.settings);
        }
        let all: Vec<Cmd> = setters.iter().map(|(c, _)| *c).collect();
        assert_eq!(checked(&settings_menu(&h, p)), all, "{p:?}");
    }
    all_off(&mut h.app.settings);
    h.app.settings.color_management = true;
    assert_eq!(checked(&settings_menu(&h, LINUX)), [ToggleColorManagement]);
}

#[test]
fn prefetch_and_background_can_only_be_changed_in_the_file() {
    let mut h = Harness::with_settings(custom());
    h.write_png("a.png", 64, 48);
    h.write_png("b.png", 64, 48);
    h.open("a.png");
    let languages =
        std::iter::once(Language(None)).chain(blinzel_i18n::Lang::ALL.map(|l| Language(Some(l))));
    let file_only = |s: &Settings| (s.prefetch_ahead, s.prefetch_behind, s.background);
    let expected = file_only(&custom());
    for cmd in menu::ALL.into_iter().chain(languages) {
        h.command(cmd);
        assert_eq!(file_only(&h.app.settings), expected, "{cmd:?}");
    }
    h.app.editor_chosen(PathBuf::from("/opt/edit"), None);
    h.app
        .screenshot_folder_picked(PathBuf::from("/home/a/Shots"));
    assert_eq!(file_only(&h.app.settings), expected);
    // No menu entry stands for them.
    for p in [WINDOWS, MACOS, LINUX] {
        for cmd in cmds(&settings_menu(&h, p)).into_iter().flatten() {
            assert!(menu::ALL.contains(&cmd), "{cmd:?}");
        }
    }
}

#[test]
fn a_setting_changed_in_the_menu_writes_the_file_at_once_with_all_settings() {
    let mut h = Harness::with_settings(custom());
    let de = blinzel_i18n::lang_from_code("de").unwrap();
    let mut changes = vec![
        View(ViewMode::FitWindowToImage),
        View(ViewMode::ActualSize),
        View(ViewMode::FitImageToWindow),
        ToggleNoUpscale,
        ToggleFilmstrip,
    ];
    // Every check item under Menu → Settings, on any platform.
    for p in [WINDOWS, MACOS, LINUX] {
        for cmd in cmds(&settings_menu(&h, p)).into_iter().flatten() {
            if !matches!(cmd, ChooseEditor | ScreenshotFolder) && !changes.contains(&cmd) {
                changes.push(cmd);
            }
        }
    }
    changes.extend([Language(Some(de)), Language(None)]);
    assert_eq!(changes.len(), 5 + 8 + 2);
    for cmd in changes {
        let _ = std::fs::remove_file(h.settings_file());
        let before = h.app.settings.clone();
        h.command(cmd);
        assert_ne!(h.app.settings, before, "{cmd:?} changes a setting");
        assert_eq!(
            h.saved_settings().as_ref(),
            Some(&h.app.settings),
            "{cmd:?}"
        );
    }
    // The file-only and the path settings were written along.
    let saved = h.saved_settings().unwrap();
    assert_eq!(
        (saved.prefetch_ahead, saved.background),
        (7, [0x12, 0x34, 0x56])
    );
    assert_eq!(saved.editor.as_deref(), Some("/usr/bin/gimp"));

    // Picking an editor or a screenshot folder: the entries ask, the
    // answer is written.
    assert_eq!(
        h.command(ChooseEditor),
        [Action::ChooseEditor { then: None }]
    );
    let _ = std::fs::remove_file(h.settings_file());
    h.app
        .editor_chosen(PathBuf::from("/opt/my editor/bin"), None);
    assert_eq!(h.app.settings.editor.as_deref(), Some("/opt/my editor/bin"));
    assert_eq!(h.saved_settings().as_ref(), Some(&h.app.settings));
    assert_eq!(h.command(ScreenshotFolder), [Action::PickScreenshotFolder]);
    let _ = std::fs::remove_file(h.settings_file());
    h.app
        .screenshot_folder_picked(PathBuf::from("/home/a/Shots #2"));
    assert_eq!(
        h.app.settings.screenshot_dir.as_deref(),
        Some("/home/a/Shots #2")
    );
    assert_eq!(h.saved_settings().as_ref(), Some(&h.app.settings));
}

#[test]
fn a_picked_path_with_a_quote_or_line_break_is_not_stored() {
    let mut h = Harness::with_images(&["a.png"]);
    h.app.settings = custom();
    let file = h.images.join("a.png");
    for bad in ["/opt/a\"b", "/opt/a\nb", "/opt/a\rb"] {
        let actions = h.app.editor_chosen(PathBuf::from(bad), Some(file.clone()));
        assert_eq!(actions, [], "{bad:?} is not started either");
        h.app.screenshot_folder_picked(PathBuf::from(bad));
        assert_eq!(h.app.settings, custom(), "{bad:?}");
        assert_eq!(h.saved_text(), None, "{bad:?}");
    }
    // A good one is stored (the checks above are not vacuous).
    h.app.screenshot_folder_picked(PathBuf::from("/opt/ok"));
    assert_eq!(
        h.saved_settings().unwrap().screenshot_dir.as_deref(),
        Some("/opt/ok")
    );
}

#[test]
fn an_unwritable_file_keeps_the_change_in_memory_without_a_message() {
    let mut h = Harness::with_images(&["a.png"]);
    // A folder where the file should go: every write fails.
    std::fs::create_dir_all(h.settings_file()).unwrap();
    h.clear_actions();
    let a = h.command(ToggleFilmstrip);
    h.shell(a);
    let a = h.command(View(ViewMode::ActualSize));
    h.shell(a);
    h.command(ToggleAutoUpdate);
    h.app.editor_chosen(PathBuf::from("/opt/edit"), None);
    h.app.save_settings_on_exit();
    h.render();
    assert!(h.app.settings.filmstrip && !h.app.settings.auto_update);
    assert_eq!(h.mode(), ViewMode::ActualSize);
    assert_eq!(h.app.settings.editor.as_deref(), Some("/opt/edit"));
    assert!(h.settings_file().is_dir() && h.saved_text().is_none());
    assert_eq!(h.app.notice, None, "no message");
    assert!(
        !h.actions()
            .iter()
            .any(|a| matches!(a, Action::Close | Action::Exit)),
        "{:?}",
        h.actions()
    );
    // The viewer keeps working: the next image still opens.
    h.write_png("b.png", 64, 48);
    h.open("b.png");
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
}

#[test]
fn an_unknown_language_shows_the_system_language_and_stays_in_the_file() {
    let mut h = Harness::new();
    std::fs::create_dir_all(h.settings_file().parent().unwrap()).unwrap();
    std::fs::write(h.settings_file(), "language = \"xx\"\nfilmstrip = true\n").unwrap();
    // A hidden instance waking reads the file (the shells' start does the
    // same with the system language as the fallback).
    let de = blinzel_i18n::lang_from_code("de").unwrap();
    blinzel_i18n::set_lang(de);
    h.app.reload_settings();
    assert_eq!(blinzel_i18n::lang(), h.app.services.system_language());
    assert_eq!(h.app.settings.language.as_deref(), Some("xx"));
    assert!(h.app.settings.filmstrip);
    assert_eq!(
        h.menu_state().language,
        None,
        "the menu shows the system language"
    );
    // Other changes keep it in the file.
    h.command(ToggleFilmstrip);
    assert!(h.saved_text().unwrap().contains("language = \"xx\""));
    h.command(ToggleColorManagement);
    assert_eq!(h.saved_settings().unwrap().language.as_deref(), Some("xx"));
    // Until the user picks a language.
    h.command(Language(Some(de)));
    assert_eq!(h.saved_settings().unwrap().language.as_deref(), Some("de"));
    h.command(Language(None));
    assert_eq!(h.saved_settings().unwrap().language, None);
    assert!(h.saved_text().unwrap().contains("language = \"auto\""));
}

#[test]
fn exiting_writes_the_settings_once_more() {
    let mut h = Harness::new();
    h.app.settings = custom();
    assert_eq!(h.saved_text(), None);
    h.app.save_settings_on_exit();
    assert_eq!(h.saved_settings(), Some(custom()));
    // A hidden instance too, except on Windows.
    let _ = std::fs::remove_file(h.settings_file());
    h.app.hidden = true;
    h.app.save_settings_on_exit();
    assert_eq!(h.saved_settings().is_some(), !cfg!(windows));
}

#[test]
fn only_a_hidden_instance_on_windows_skips_the_write_on_exit() {
    assert_eq!(HIDDEN_EXIT_KEEPS_FILE, cfg!(windows));
    // Windows.
    assert!(writes_settings_on_exit(false, true));
    assert!(!writes_settings_on_exit(true, true));
    // Linux and macOS.
    assert!(writes_settings_on_exit(false, false));
    assert!(writes_settings_on_exit(true, false));
}
