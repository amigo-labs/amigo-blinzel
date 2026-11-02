//! App-level tests of colour management: the setting, and the target
//! following the window's monitor (`docs/specs/color-management.md`).

use super::*;
use crate::testkit::{self, Harness, LUT_SOURCES, LUTS_BUILT};
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

fn queries(h: &Harness) -> usize {
    h.app.services.target_queries.load(Ordering::Relaxed)
}

/// AC-11: the start draws with the remembered target (the shell sets it
/// before the window) without asking the platform, asks once after the
/// first frame and redraws with the answer when it differs.
#[test]
fn the_start_draws_with_the_remembered_target_and_asks_after_the_first_frame() {
    let mut h = Harness::new();
    // As the Windows shell starts: the monitor under the cursor, the target
    // remembered for it.
    h.app.monitor = 1;
    h.app
        .color
        .set_target(Some(profile(colorants::DISPLAY_P3, "P3 display")));
    let adobe = profile(colorants::ADOBE_RGB, "Adobe display");
    h.app
        .services
        .targets
        .lock()
        .unwrap()
        .insert(1, adobe.clone());
    h.write_png("a.png", 64, 48);
    assert_eq!(queries(&h), 0, "nothing asked before the window");

    let services = h.app.services.clone();
    let held = services.target_gate.lock().unwrap();
    h.open("a.png");
    let built = LUT_SOURCES.lock().unwrap().len();
    assert!(
        converted(&mut h),
        "the first frames use the remembered target"
    );

    drop(held);
    let before = h.host().invalidations;
    h.wait_for(|h| h.color_answers > 0);
    assert_eq!(queries(&h), 1, "asked after the first frame");
    assert!(h.host().invalidations > before, "another answer redraws");
    assert!(converted(&mut h));
    assert_eq!(
        LUT_SOURCES.lock().unwrap().len(),
        built + 1,
        "a transform for the answer"
    );
    for _ in 0..3 {
        h.render();
    }
    assert_eq!(queries(&h), 1, "asked once");
    drop(h);

    // The answer is what was remembered: nothing to redraw.
    let mut h = Harness::new();
    h.render();
    let before = h.host().invalidations;
    h.wait_for(|h| h.color_answers > 0);
    assert_eq!(queries(&h), 1);
    assert_eq!(h.host().invalidations, before, "the same answer");
}

/// The rXYZ, gXYZ and bXYZ colorants of an ICC profile.
fn colorants_of(icc: &[u8]) -> [[f64; 3]; 3] {
    let be = |at: usize| u32::from_be_bytes(icc[at..at + 4].try_into().unwrap());
    let tag = |sig: &[u8; 4]| {
        let entry = (0..be(128) as usize)
            .map(|i| 132 + 12 * i)
            .find(|&e| &icc[e..e + 4] == sig)
            .expect("colorant tag");
        let at = be(entry + 4) as usize + 8;
        [0, 4, 8].map(|k| f64::from(be(at + k) as i32) / 65536.0)
    };
    [tag(b"rXYZ"), tag(b"gXYZ"), tag(b"bXYZ")]
}

/// AC-19: the resident start's warm-up loads the colour engine once, with
/// a Display P3 profile, while no image is open; with colour management
/// off it does not.
#[cfg(feature = "turbojpeg")]
#[test]
fn the_warm_up_builds_one_display_p3_transform() {
    let mut h = Harness::new();
    let before = LUT_SOURCES.lock().unwrap().len();
    h.app.warm_up(1);
    let built = LUT_SOURCES.lock().unwrap()[before..].to_vec();
    assert_eq!(built.len(), 1, "one transform");
    let src = built[0].as_ref().expect("from a profile");
    assert!(!src.srgb);
    for (got, want) in colorants_of(&src.bytes).iter().zip(&colorants::DISPLAY_P3) {
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-4, "Display P3: {got:?} vs {want:?}");
        }
    }
    drop(h);

    let off = Settings {
        color_management: false,
        ..Settings::default()
    };
    let mut h = Harness::with_settings(off);
    let before = LUT_SOURCES.lock().unwrap().len();
    h.app.warm_up(1);
    assert_eq!(
        LUT_SOURCES.lock().unwrap().len(),
        before,
        "off: no transform"
    );
}
