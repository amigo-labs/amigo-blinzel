//! App-level tests of screenshots: the commands, the key setting and what
//! happens to a taken shot (`docs/specs/screenshots.md`).

use super::*;
use crate::menu::{self, MenuItem, Platform};
use crate::testkit::Harness;
use Cmd::*;

/// Whether the Settings menu of a platform with screenshots shows the
/// screenshot key as checked.
fn key_checked(h: &Harness) -> bool {
    let p = Platform {
        resident: true,
        screenshots: true,
        mac: false,
    };
    fn find(items: &[MenuItem]) -> Option<bool> {
        items.iter().find_map(|i| match i {
            MenuItem::Item { cmd, checked, .. } if *cmd == ToggleScreenshotHotkey => Some(*checked),
            MenuItem::Sub(_, items) => find(items),
            _ => None,
        })
    }
    find(&menu::items_for(&h.menu_state(), p)).expect("the screenshot key entry")
}

/// AC-05: turning the screenshot key on while resident mode is off turns
/// resident mode on too; the menu checks the key only while both are on.
#[test]
fn turning_the_screenshot_key_on_turns_resident_mode_on_too() {
    let mut h = Harness::new();
    assert!(!h.app.settings.resident && h.app.settings.screenshot_hotkey);
    assert!(!key_checked(&h), "not checked while resident mode is off");
    assert_eq!(
        h.command(ToggleScreenshotHotkey),
        [Action::SetResident(true), Action::SetScreenshotHotkey(true)]
    );
    let saved = h.saved_settings().unwrap();
    assert!(saved.resident && saved.screenshot_hotkey);
    assert!(key_checked(&h));
    // Off again: resident mode stays on.
    assert_eq!(
        h.command(ToggleScreenshotHotkey),
        [Action::SetScreenshotHotkey(false)]
    );
    assert!(h.app.settings.resident && !h.app.settings.screenshot_hotkey);
    assert!(!key_checked(&h));
    // On with resident mode already on, then resident mode off: the key
    // stays set but is no longer checked.
    assert_eq!(
        h.command(ToggleScreenshotHotkey),
        [Action::SetScreenshotHotkey(true)]
    );
    h.command(ToggleResident);
    assert!(h.app.settings.screenshot_hotkey && !key_checked(&h));
}

/// AC-06 (the App half): "Screenshot of the whole screen" takes the
/// monitor at once, "Take screenshot…" opens the overlay.
#[test]
fn the_tray_entries_take_the_screen_or_open_the_overlay() {
    let mut h = Harness::new();
    assert_eq!(
        h.command(CaptureScreen),
        [Action::Capture { whole_screen: true }]
    );
    assert_eq!(
        h.command(Capture),
        [Action::Capture {
            whole_screen: false
        }]
    );
}

/// AC-20: with "open in the viewer" on, the shot opens and the window
/// comes up (also from hidden) without a "copied" notice; with it off, the
/// notice says the shot was copied. With "open in the editor" on, the file
/// then goes to the editor, or to the question for one first.
#[test]
fn a_shot_opens_in_the_viewer_or_says_it_was_copied_then_goes_to_the_editor() {
    let mut h = Harness::with_images(&["shot.png", "other.png"]);
    let file = h.images.join("shot.png");
    h.app.hide();
    let (a, text) = h.app.on_screenshot(true, Ok(Some(file.clone())));
    assert_eq!(text, None, "no copied notice");
    assert_eq!(a.last(), Some(&Action::Wake { opened: true }));
    assert_eq!(h.current_name().as_deref(), Some("shot.png"));
    // The shell's answer to the Wake.
    assert!(h.app.wake(true));

    h.app.settings.screenshot_open = false;
    let other = h.images.join("other.png");
    for f in [Some(other.clone()), None] {
        let (a, text) = h.app.on_screenshot(true, Ok(f));
        assert_eq!(a, [], "neither opened nor woken");
        assert_eq!(text.as_deref(), Some("Screenshot copied to the clipboard"));
    }
    assert_eq!(h.current_name().as_deref(), Some("shot.png"));

    h.app.settings.screenshot_edit = true;
    let (a, _) = h.app.on_screenshot(true, Ok(Some(other.clone())));
    assert_eq!(
        a,
        [Action::ChooseEditor {
            then: Some(other.clone())
        }]
    );
    h.app.settings.editor = Some("/usr/bin/gimp".into());
    h.app.settings.screenshot_open = true;
    let (a, text) = h.app.on_screenshot(true, Ok(Some(other.clone())));
    assert_eq!(text, None);
    let n = a.len();
    assert!(n >= 2, "{a:?}");
    assert_eq!(a[n - 2], Action::Wake { opened: true });
    assert_eq!(
        a[n - 1],
        Action::EditWith {
            editor: PathBuf::from("/usr/bin/gimp"),
            file: other
        },
        "the editor after the viewer, on top"
    );
    assert_eq!(h.current_name().as_deref(), Some("other.png"));
}

/// AC-15 (the notice): a shot that did not reach the clipboard says so;
/// a file that could not be written says why.
#[test]
fn a_shot_that_misses_the_clipboard_or_its_file_says_why() {
    let mut h = Harness::with_images(&["shot.png"]);
    let file = h.images.join("shot.png");
    for open in [false, true] {
        h.app.settings.screenshot_open = open;
        let (_, text) = h.app.on_screenshot(false, Ok(Some(file.clone())));
        assert_eq!(
            text.as_deref(),
            Some("Screenshot failed: clipboard"),
            "open {open}"
        );
    }
    let (a, text) = h.app.on_screenshot(true, Err("/shots: disk full".into()));
    assert_eq!(a, []);
    assert_eq!(
        text.as_deref(),
        Some("Screenshot failed: /shots: disk full")
    );
}
