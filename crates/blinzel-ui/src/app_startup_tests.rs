//! App-level tests of the cold start: when the cloaked window becomes
//! visible (`docs/specs/startup.md`).

use super::*;
use crate::testkit::Harness;

fn uncloaked(h: &Harness) -> bool {
    h.actions().contains(&Action::Uncloak) && !h.app.cloaked
}

// ------------------------------------------------------- the cloaked window

/// AC-03: without a file the window is shown at once, with the hint.
#[test]
fn without_a_file_the_window_shows_the_open_hint_at_once() {
    let mut h = Harness::new();
    assert!(h.app.cloaked, "a new window starts cloaked");
    h.render();
    assert!(uncloaked(&h));
    assert_eq!(h.frame().texts, [tr(Msg::HintOpen)]);
}

/// Defect "no 400 ms fallback at a cold start" (Linux): a missing first
/// file shows the window with its error instead of leaving it invisible.
#[test]
fn a_missing_first_file_shows_the_window_with_its_error() {
    let mut h = Harness::new();
    h.open("missing.png");
    assert!(uncloaked(&h));
    let text = h.frame().texts.join(" ");
    assert!(text.contains("missing.png"), "{text}");
}

/// Defect "no 400 ms fallback at a cold start" (Linux, macOS): a broken
/// first file shows the window with its error instead of leaving it
/// invisible.
#[test]
fn a_broken_first_file_shows_the_window_with_its_error() {
    let mut h = Harness::new();
    h.write_file("broken.png", b"not an image");
    h.open("broken.png");
    assert!(
        h.app
            .error_of(h.app.current.as_ref().unwrap().cache_id)
            .is_some()
    );
    assert!(uncloaked(&h));
    let text = h.frame().texts.join(" ");
    assert!(text.contains("broken.png"), "{text}");
}

/// AC-06 (the platform-independent half): while the first image is still
/// being decoded nothing is drawn and the window stays cloaked.
#[test]
fn the_window_stays_cloaked_while_the_first_image_decodes() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    // Opened, but the loader's result is not taken yet.
    let a = h.app.open(h.images.join("a.png"));
    assert!(!a.contains(&Action::Uncloak));
    let a = h.render();
    assert!(!a.contains(&Action::Uncloak), "{a:?}");
    assert!(h.app.cloaked);
    assert!(h.frame().images.is_empty() && h.frame().texts.is_empty());
    h.pump();
    assert!(uncloaked(&h), "the decoded image shows the window");
}

/// A closed resident window has nothing open and stays hidden when it is
/// drawn (warm-up frames).
#[test]
fn a_hidden_resident_window_is_not_shown_by_a_frame() {
    let mut h = Harness::new();
    h.app.hide();
    h.clear_actions();
    let a = h.render();
    assert!(!a.contains(&Action::Uncloak), "{a:?}");
    assert!(h.app.cloaked);
}
