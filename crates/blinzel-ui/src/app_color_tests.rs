//! App-level tests of colour management: the setting, and the target
//! following the window's monitor (`docs/specs/color-management.md`).

use super::*;
use crate::testkit::{self, Harness, LUTS_BUILT};
use Cmd::*;
use blinzel_core::icc::{Profile, Trc, colorants, embed_in_jpeg, matrix_profile};
use std::sync::atomic::Ordering;

fn profile(c: [[f64; 3]; 3], name: &str) -> Profile {
    Profile::new(matrix_profile(c, Trc::Srgb, name))
}

/// Writes a JPEG tagged Display P3 and opens it.
fn open_p3(h: &mut Harness) {
    let p3 = matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3 test");
    h.write_file("p3.jpg", &embed_in_jpeg(&testkit::jpeg(64, 48), &p3));
    h.open("p3.jpg");
}

fn converted(h: &mut Harness) -> bool {
    h.render();
    let images = &h.frame().images;
    assert_eq!(images.len(), 1, "one image drawn");
    images[0].converted
}

/// AC-17: on by default, off converts nothing, the toggle takes effect on
/// the next frame and is saved.
#[test]
fn the_setting_switches_conversion_on_the_next_frame() {
    assert!(Settings::default().color_management);
    let mut h = Harness::new();
    open_p3(&mut h);
    assert!(converted(&mut h), "a P3 image on an sRGB display");

    let before = h.host().invalidations;
    h.command(ToggleColorManagement);
    assert!(h.host().invalidations > before, "redraws");
    assert!(!converted(&mut h), "off: drawn unconverted");
    assert!(!h.saved_settings().unwrap().color_management);

    h.command(ToggleColorManagement);
    assert!(converted(&mut h), "on again without a restart");
    drop(h);

    let off = Settings {
        color_management: false,
        ..Settings::default()
    };
    let mut h = Harness::with_settings(off);
    open_p3(&mut h);
    assert!(!converted(&mut h), "off from the start");
}

/// AC-16: the target follows the window's monitor; a change redraws with
/// the new monitor's transform, the same monitor does nothing unless the
/// displays changed.
#[test]
fn a_new_monitor_retargets_and_redraws() {
    let mut h = Harness::with_images(&["a.png"]);
    {
        let mut t = h.app.services.targets.lock().unwrap();
        t.insert(2, profile(colorants::DISPLAY_P3, "P3 display"));
        t.insert(3, profile(colorants::ADOBE_RGB, "Adobe display"));
    }
    h.app.check_monitor(true);
    assert!(!converted(&mut h), "sRGB image on an sRGB monitor");

    let before = h.host().invalidations;
    h.app.host.monitor.set(2);
    h.app.check_monitor(false);
    assert!(h.host().invalidations > before, "redraws");
    let built = LUTS_BUILT.load(Ordering::Relaxed);
    assert!(converted(&mut h), "converted for the P3 monitor");
    assert_eq!(LUTS_BUILT.load(Ordering::Relaxed), built + 1);

    // Moving within the monitor: nothing to do.
    let before = h.host().invalidations;
    h.app.check_monitor(false);
    assert_eq!(h.host().invalidations, before);

    h.app.host.monitor.set(3);
    h.app.check_monitor(false);
    assert!(converted(&mut h), "converted for the Adobe RGB monitor");
    assert_eq!(
        LUTS_BUILT.load(Ordering::Relaxed),
        built + 2,
        "a new transform"
    );

    // The display configuration changed: the monitor's profile is gone.
    h.app.services.targets.lock().unwrap().remove(&3);
    let before = h.host().invalidations;
    h.app.check_monitor(true);
    assert!(h.host().invalidations > before, "redraws");
    assert!(!converted(&mut h), "sRGB again");
}
