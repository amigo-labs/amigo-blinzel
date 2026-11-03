//! App-level tests of resident mode: the toggle, what a hidden instance
//! drops and keeps, waking, and the tray menu
//! (`docs/specs/resident-mode.md`).

use super::*;
use crate::menu::{self, MenuItem};
use crate::testkit::{Harness, LUT_SOURCES};
use Cmd::*;
use blinzel_core::bufpool;
use blinzel_core::icc::{Profile, Trc, colorants, matrix_profile};

/// AC-02 (the App half): the toggle saves the setting and tells the shell
/// to add or remove the tray icon and sign-in entry at once; the screenshot
/// key follows, registered only while both settings are on.
#[test]
fn toggling_resident_mode_saves_it_and_switches_the_icon_and_the_key() {
    let mut h = Harness::new();
    assert!(!h.app.settings.resident, "off by default");
    assert!(h.app.settings.screenshot_hotkey);
    let a = h.command(ToggleResident);
    assert_eq!(
        a,
        [Action::SetResident(true), Action::SetScreenshotHotkey(true)]
    );
    assert!(h.saved_settings().unwrap().resident);
    let a = h.command(ToggleResident);
    assert_eq!(
        a,
        [
            Action::SetResident(false),
            Action::SetScreenshotHotkey(false)
        ]
    );
    assert!(!h.saved_settings().unwrap().resident);
    // The key setting off: resident mode alone registers no key.
    h.app.settings.screenshot_hotkey = false;
    let a = h.command(ToggleResident);
    assert_eq!(
        a,
        [
            Action::SetResident(true),
            Action::SetScreenshotHotkey(false)
        ]
    );
}

/// AC-05: hiding drops the session (image, jobs, folder and its watch,
/// textures, thumbnails, animations, notice, view) and the pool's idle
/// buffers; the device and the loader stay and open the next image.
#[test]
fn hiding_drops_the_session_and_keeps_device_and_loader() {
    let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
    h.open("a.png");
    h.key(Key::Right);
    h.app.view.zoom = Some(3.0);
    h.app.show_notice("note".into());
    assert!(h.app.current.is_some() && !h.app.textures.is_empty());
    assert!(!h.app.list.is_empty() && h.app.watcher.is_some());
    let uploads = h.decodes().len();
    let pool = bufpool::global();
    pool.set_max_retained(bufpool::DEFAULT_MAX_RETAINED);
    drop(pool.take(4 << 20));
    assert!(pool.retained_bytes() > 0);

    h.app.hide();
    assert!(h.app.hidden && h.app.cloaked);
    assert!(h.app.current.is_none() && h.app.shown.is_none());
    assert_eq!(h.app.loader.pending(), 0, "no decode jobs left");
    assert!(h.app.list.is_empty() && h.app.watcher.is_none());
    assert!(h.app.textures.is_empty() && h.app.animations.is_empty());
    assert!(h.app.notice.is_none());
    assert_eq!(h.app.view, ViewState::default());
    assert_eq!(bufpool::global().retained_bytes(), 0);

    // Woken with a file: the same device and loader decode and show it.
    h.app.wake(true);
    h.open("c.png");
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
    assert!(h.decodes().len() > uploads);
}

/// AC-09 (the App half): waking a hidden instance leaves it cloaked until
/// its first frame, which uncloaks it and asks for the foreground; a
/// visible instance is only activated.
#[test]
fn a_woken_instance_stays_cloaked_until_its_first_frame_then_takes_the_foreground() {
    let mut h = Harness::with_images(&["a.png"]);
    assert!(!h.app.wake(false), "visible: only activated");
    h.app.hide();
    assert!(h.app.wake(true));
    assert!(!h.app.hidden && h.app.cloaked && h.app.activate_on_uncloak);
    // The pool keeps idle buffers again.
    drop(bufpool::global().take(4 << 20));
    assert!(bufpool::global().retained_bytes() > 0);
    h.open("a.png");
    assert!(
        h.actions().contains(&Action::Uncloak),
        "the first frame shows it"
    );
    // Without a file the shell draws the hint and uncloaks at once.
    h.app.hide();
    assert!(h.app.wake(false));
    assert!(h.app.cloaked && h.app.activate_on_uncloak);
}

/// AC-10: a hidden instance woken with or without a file reloads the
/// settings another window saved meanwhile, language and colour
/// management included.
#[test]
fn a_woken_instance_reloads_the_settings_from_disk() {
    let mut h = Harness::with_images(&["a.png"]);
    let p3 = Profile::new(matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3"));
    assert!(h.app.color.lut(Some(&p3)).is_some(), "converts on");
    for opened in [false, true] {
        h.app.hide();
        let saved = Settings {
            language: Some("de".into()),
            color_management: false,
            resident: true,
            ..Settings::default()
        };
        settings_store::save(&saved);
        blinzel_i18n::set_lang(blinzel_i18n::FALLBACK);
        if opened {
            h.open("a.png");
        }
        h.app.wake(opened);
        assert_eq!(h.app.settings.language.as_deref(), Some("de"), "{opened}");
        assert!(!h.app.settings.color_management, "{opened}");
        assert!(h.app.color.lut(Some(&p3)).is_none(), "converts nothing");
        assert_eq!(blinzel_i18n::lang().code(), "de", "{opened}");
        h.app.settings = Settings::default();
        h.app.color.set_enabled(true);
    }
    // A visible window keeps what it has.
    settings_store::save(&Settings::default());
    h.app.settings.prefetch_ahead = 9;
    h.app.wake(false);
    assert_eq!(h.app.settings.prefetch_ahead, 9);
}

/// AC-12: the tray menu's entries in order; "Exit" ends the process also
/// in resident mode, "Open…" asks for a file and the chosen one wakes
/// the window.
#[test]
fn the_tray_menu_shows_opens_takes_screenshots_and_exits() {
    let entries = |screenshots| -> Vec<Option<(Cmd, String)>> {
        menu::tray_items_with(screenshots)
            .into_iter()
            .map(|i| match i {
                MenuItem::Item { cmd, label, .. } => Some((cmd, label)),
                _ => None,
            })
            .collect()
    };
    let mut h = Harness::with_images(&["a.png"]);
    let item = |c, l: &str| Some((c, l.to_string()));
    assert_eq!(
        entries(true),
        [
            item(ShowWindow, "Show amigo-blinzel"),
            item(Open, "Open…"),
            None,
            item(Capture, "Take screenshot…"),
            item(CaptureScreen, "Screenshot of the whole screen"),
            None,
            item(Quit, "Exit"),
        ]
    );
    // Linux has no screenshots; this build shows its own platform's menu.
    assert_eq!(
        entries(false),
        [entries(true)[..2].to_vec(), entries(true)[5..].to_vec()].concat()
    );
    assert_eq!(menu::tray_items(), menu::tray_items_with(menu::SCREENSHOTS));
    h.app.settings.resident = true;
    assert_eq!(h.command(Quit), [Action::Exit]);
    assert_eq!(h.command(ShowWindow), [Action::Wake { opened: false }]);
    assert_eq!(h.command(Open), [Action::OpenDialog]);
    h.app.hide();
    let a = h.app.open_chosen(h.images.join("a.png"));
    assert_eq!(a.last(), Some(&Action::Wake { opened: true }));
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
}

/// AC-20 (the App half): the `--resident` warm-up decodes, uploads and
/// presents a generated 3072 × 2048 JPEG once and, with colour management
/// on, builds exactly one transform, from Display P3; with it off, none.
#[test]
fn warming_up_presents_a_photo_once_and_builds_one_p3_transform() {
    let p3 = Profile::new(matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3"));
    for color_management in [true, false] {
        let mut h = Harness::with_settings(Settings {
            color_management,
            ..Settings::default()
        });
        let presented = h.app.canvas.presented;
        let luts = LUT_SOURCES.lock().unwrap().len();
        h.app.warm_up(2);
        assert_eq!(*h.app.device.uploads.lock().unwrap(), [(3072, 2048)]);
        assert_eq!(h.app.canvas.presented, presented + 1, "presented once");
        let drawn: Vec<_> = h.frame().images.iter().map(|i| i.full_size).collect();
        assert_eq!(drawn, [(3072, 2048)]);
        let built = LUT_SOURCES.lock().unwrap()[luts..].to_vec();
        let want = if color_management {
            vec![Some(p3.clone())]
        } else {
            vec![]
        };
        assert_eq!(built, want, "colour management {color_management}");
    }
}

/// AC-22 (the App half): a message for the user is the window's notice,
/// and while the window is hidden it goes back to the shell for the tray
/// balloon or the menu bar title instead.
#[test]
fn a_message_while_hidden_goes_to_the_tray_instead_of_the_notice() {
    let mut h = Harness::with_images(&["a.png"]);
    assert_eq!(h.app.tell("copied".into()), None);
    assert_eq!(h.app.notice.as_deref(), Some("copied"));
    h.app.hide();
    assert_eq!(h.app.tell("taken".into()).as_deref(), Some("taken"));
    assert!(h.app.notice.is_none(), "no notice on a hidden window");
}
