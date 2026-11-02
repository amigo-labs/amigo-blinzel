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

// ------------------------------------------------------- folder watching

/// What the shell does about 150 ms after a change in the folder.
fn folder_changed(h: &mut Harness) {
    h.app.on_dir_changed();
    h.pump();
}

/// AC-14.
#[test]
fn a_changed_folder_is_listed_again_and_the_current_image_stays() {
    let mut h = Harness::with_images(&["b.png", "a.png", "c.png"]);
    assert_eq!(h.host().watched, [h.images.clone()]);
    h.write_png("d.png", 64, 48);
    std::fs::rename(h.images.join("c.png"), h.images.join("e.png")).unwrap();
    std::fs::remove_file(h.images.join("a.png")).unwrap();
    folder_changed(&mut h);
    assert_eq!(h.names(), ["b.png", "d.png", "e.png"]);
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
    assert_eq!(index(&h), Some(0));
    assert_eq!(h.frame().images.len(), 1);
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("d.png"), "added");
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("e.png"), "renamed");
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("b.png"), "a.png is gone");
}

/// AC-14: the current file deleted or renamed outside the viewer stays on
/// screen, and Next and Previous go on from its place in the folder.
#[test]
fn after_the_current_file_left_the_folder_next_and_previous_go_on_from_its_place() {
    for (key, want) in [(Right, "c.png"), (Left, "a.png")] {
        let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
        h.open("b.png");
        std::fs::remove_file(h.images.join("b.png")).unwrap();
        folder_changed(&mut h);
        assert_eq!(h.names(), ["a.png", "c.png"]);
        assert_eq!(h.current_name().as_deref(), Some("b.png"));
        assert_eq!(h.frame().images.len(), 1, "still on screen");
        h.key(key);
        assert_eq!(h.current_name().as_deref(), Some(want), "{key:?}");
        drop(h);
    }
    // Renamed: from where the old name sorted.
    let mut h = Harness::with_images(&["a.png", "b.png", "c.png", "d.png"]);
    h.open("b.png");
    std::fs::rename(h.images.join("b.png"), h.images.join("z.png")).unwrap();
    folder_changed(&mut h);
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("d.png"));
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("z.png"));
}

/// AC-14: the current file rewritten in place (new size or write time)
/// loads anew.
#[test]
fn the_current_file_rewritten_in_place_is_loaded_anew() {
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    let old = h.app.current.as_ref().unwrap().cache_id;
    h.write_png("a.png", 30, 20);
    folder_changed(&mut h);
    let cur = h.app.current.as_ref().unwrap();
    assert_ne!(cur.cache_id, old);
    assert_eq!(cur.index, Some(0));
    assert_eq!(h.frame().images[0].full_size, (30, 20));
}

/// AC-14: a change while the folder is still being listed is listed once
/// that listing is in.
#[test]
fn a_change_during_the_listing_is_listed_afterwards() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_png("b.png", 64, 48);
    let _ = h.app.open(h.images.join("a.png"));
    h.pump_holding_listing();
    h.write_png("c.png", 64, 48);
    h.app.on_dir_changed();
    h.release_listing();
    h.pump();
    assert_eq!(h.names(), ["a.png", "b.png", "c.png"]);
}
