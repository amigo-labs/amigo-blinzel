//! App-level tests of browsing a folder: the listing, navigation, folder
//! watching and prefetch (`docs/specs/browsing.md`).

use super::*;
use crate::testkit::Harness;
use Key::*;

fn index(h: &Harness) -> Option<usize> {
    h.app.current.as_ref().and_then(|c| c.index)
}

// --------------------------------------------------------------- listing

/// AC-03.
#[test]
fn an_opened_file_shows_before_its_folder_is_listed_and_keys_wait_for_the_listing() {
    let mut h = Harness::new();
    for n in ["a.png", "B.png", "b.png", "c.png"] {
        h.write_png(n, 64, 48);
    }
    let _ = h.app.open(h.images.join("b.png"));
    h.pump_holding_listing();
    assert!(h.app.is_listing());
    assert_eq!(h.frame().images.len(), 1, "shown without the listing");
    assert_eq!(index(&h), None);
    for k in [Right, Left, Home, End] {
        h.app.key(k, false, false);
        assert_eq!(h.current_name().as_deref(), Some("b.png"), "{k:?}");
    }
    h.release_listing();
    h.pump();
    assert_eq!(h.names(), ["a.png", "B.png", "b.png", "c.png"]);
    assert_eq!(index(&h), Some(2), "the exact name, not B.png");
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
}

// ------------------------------------------------------------ navigation

/// AC-04, AC-05.
#[test]
fn navigation_keys_step_jump_wrap_and_pan_a_zoomed_image() {
    let mut h = Harness::new();
    for n in ["a.png", "b.png", "c.png", "d.png"] {
        h.write_png(n, 64, 48);
    }
    h.write_png("e.png", 1600, 1200);
    h.open("a.png");
    let steps = [
        (Right, "b"),
        (PageDown, "c"),
        (Space, "d"),
        (Down, "e"),
        (Right, "a"),
        (Left, "e"),
        (PageUp, "d"),
        (Backspace, "c"),
        (Up, "b"),
        (End, "e"),
        (Home, "a"),
    ];
    for (k, want) in steps {
        h.key(k);
        assert_eq!(h.current_name(), Some(format!("{want}.png")), "{k:?}");
    }
    // Right-to-left languages swap Right and Left only.
    assert_eq!(nav_cmd(Right, true), Some(Cmd::Prev));
    assert_eq!(nav_cmd(Left, true), Some(Cmd::Next));
    assert_eq!(nav_cmd(Right, false), Some(Cmd::Next));
    assert_eq!(nav_cmd(PageDown, true), Some(Cmd::Next));
    assert_eq!(nav_cmd(Up, true), Some(Cmd::Prev));

    // At 100 % e.png overflows the window: the arrows pan, Page Down
    // still pages.
    h.key(End);
    h.key(Digit(3));
    assert!(h.layout().unwrap().overflows);
    for k in [Right, Down, Left, Up] {
        let before = h.layout().unwrap().origin;
        h.key(k);
        assert_eq!(h.current_name().as_deref(), Some("e.png"), "{k:?}");
        assert_ne!(h.layout().unwrap().origin, before, "{k:?} pans");
    }
    h.key(PageDown);
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
}
