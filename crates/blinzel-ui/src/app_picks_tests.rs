//! App-level tests of marking images and keeping the marks
//! (`docs/specs/picks.md`).

use super::*;
use crate::testkit::Harness;
use Key::*;
use blinzel_core::picks;

/// Where the harness's App keeps the marks.
fn store(h: &Harness) -> PathBuf {
    h.settings_file().parent().unwrap().join("picks")
}

/// `names` in a folder, the first open, the marks read.
fn folder(names: &[&str]) -> Harness {
    let mut h = Harness::with_images(names);
    h.pump();
    h
}

fn marked(h: &Harness) -> Vec<String> {
    h.app.browse.marks.iter().cloned().collect()
}

fn badges(h: &Harness) -> usize {
    // Each check mark is two lines.
    h.frame()
        .shapes
        .iter()
        .filter(|s| matches!(s, Shape::Line { .. }))
        .count()
        / 2
}

/// AC-01, AC-05
#[test]
fn f_marks_the_shown_image_and_a_badge_shows_it() {
    let mut h = folder(&["a.png", "b.png"]);
    h.render();
    // The window buttons are lines too.
    let base = badges(&h);
    let badges = |h: &Harness| badges(h) - base;
    h.key(Letter('F'));
    h.render();
    assert_eq!(marked(&h), ["a.png"]);
    assert_eq!(badges(&h), 1, "a badge on the shown image");
    // Shift+F too; Ctrl+F is not F.
    h.press(Letter('F'), true, false);
    assert_eq!(marked(&h), ["a.png"]);
    h.press(Letter('F'), false, true);
    h.render();
    assert!(marked(&h).is_empty());
    assert_eq!(badges(&h), 0);
    // Browsing on, marking the next one.
    h.key(Right);
    h.pump();
    h.key(Letter('F'));
    assert_eq!(marked(&h), ["b.png"]);
    // The filmstrip shows the mark on the tile.
    h.key(Letter('T'));
    h.wait_for(|h| badges(h) == 2);
}

/// AC-01: in the grid F takes the selected tile.
#[test]
fn f_in_the_grid_marks_the_selected_tile() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    h.key(Letter('G'));
    h.key(Right);
    h.key(Letter('F'));
    assert_eq!(marked(&h), ["b.png"]);
    h.key(Letter('F'));
    assert!(marked(&h).is_empty());
}

/// AC-04
#[test]
fn remove_all_marks_asks_first_and_only_remove_clears() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    assert!(!h.menu_enabled(Cmd::ClearMarks), "nothing to remove");
    h.key(Letter('G'));
    h.ctrl(Letter('A'));
    assert!(h.menu_enabled(Cmd::ClearMarks));
    h.command(Cmd::ClearMarks);
    h.render();
    assert!(
        h.frame()
            .texts
            .iter()
            .any(|t| t == "Remove the marks of 3 images?"),
        "{:?}",
        h.frame().texts
    );
    // Modal: other keys do nothing; Esc cancels.
    h.key(Letter('F'));
    h.key(Escape);
    assert_eq!(marked(&h).len(), 3);
    assert!(h.app.confirm.is_none());
    // The button removes them.
    h.command(Cmd::ClearMarks);
    h.render();
    let yes = h.app.confirm.as_ref().unwrap().yes_rect().unwrap();
    h.click((yes.x + 2.0, yes.y + 2.0));
    assert!(marked(&h).is_empty());
    h.pump();
    assert!(picks::load(&store(&h), &h.images).unwrap().is_empty());
}

/// AC-06
#[test]
fn the_grid_and_the_menu_name_the_count() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    h.key(Letter('G'));
    h.ctrl(Letter('A'));
    h.render();
    assert!(h.frame().texts.iter().any(|t| t == "3 images marked"));
    let label = crate::menu::items(&h.menu_state())
        .into_iter()
        .flat_map(|m| match m {
            crate::menu::MenuItem::Sub(_, items) => items,
            other => vec![other],
        })
        .find_map(|m| match m {
            crate::menu::MenuItem::Item {
                cmd: Cmd::ClearMarks,
                label,
                ..
            } => Some(label),
            _ => None,
        });
    assert_eq!(label.as_deref(), Some("Remove all marks (3 images)"));
}

/// AC-07
#[test]
fn the_info_overlay_shows_the_mark_and_the_count() {
    let mut h = folder(&["a.png", "b.png"]);
    h.key(Letter('F'));
    h.key(Letter('I'));
    h.wait_for(|h| h.info_lines().len() == 6);
    let lines = h.info_lines();
    assert_eq!(lines[1], "1 / 2");
    assert_eq!(lines[2], "✓ Marked");
    assert_eq!(lines[5], "1 image marked in this folder");
    h.key(Right);
    h.wait_for(|h| h.info_lines().len() == 5);
    assert!(!h.info_lines().contains(&"✓ Marked".to_string()));
}

/// AC-08, AC-09
#[test]
fn marks_last_across_the_grid_folders_and_a_restart_and_never_touch_the_folder() {
    let mut h = folder(&["a.png", "b.png"]);
    let before: Vec<_> = std::fs::read_dir(&h.images)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    h.key(Letter('G'));
    h.ctrl(Letter('A'));
    h.key(Letter('G'));
    h.key(Letter('G'));
    assert_eq!(marked(&h), ["a.png", "b.png"]);
    h.key(Escape);
    // Another folder has its own marks (none) …
    let other = h.images.join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("x.png"), crate::testkit::png(8, 8)).unwrap();
    h.app.open(other.join("x.png"));
    h.pump();
    assert!(marked(&h).is_empty());
    h.key(Letter('F'));
    // … and coming back brings these back.
    h.open("a.png");
    h.pump();
    assert_eq!(marked(&h), ["a.png", "b.png"]);
    h.restart();
    h.open("b.png");
    h.pump();
    assert_eq!(marked(&h), ["a.png", "b.png"]);
    h.app.open(other.join("x.png"));
    h.pump();
    assert_eq!(marked(&h), ["x.png"]);
    let mut after: Vec<_> = std::fs::read_dir(&h.images)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    after.retain(|n| n != "other");
    assert_eq!(after.len(), before.len(), "nothing new next to the images");
    assert!(std::fs::read_dir(store(&h)).unwrap().count() >= 2);
}

/// AC-08: another window's marks survive this one's change.
#[test]
fn a_change_keeps_what_another_window_marked() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    h.key(Letter('F'));
    h.pump();
    // The other window marks c meanwhile.
    picks::apply(
        &store(&h),
        &h.images,
        &picks::Change::Mark(vec!["c.png".into()]),
    )
    .unwrap();
    h.key(Right);
    h.pump();
    h.key(Letter('F'));
    h.pump();
    assert_eq!(marked(&h), ["a.png", "b.png", "c.png"]);
    assert_eq!(picks::load(&store(&h), &h.images).unwrap().len(), 3);
}

/// AC-10, AC-13
#[test]
fn reading_and_storing_marks_never_hold_up_frames_or_keys() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_png("b.png", 64, 48);
    picks::apply(
        &store(&h),
        &h.images,
        &picks::Change::Mark(vec!["b.png".into()]),
    )
    .unwrap();
    let held = crate::marks::HOLD.lock().unwrap();
    let _ = h.app.open(h.images.join("a.png"));
    // The loader and the listing finish; the marks are held back.
    h.pump_holding_marks();
    assert_eq!(h.frame().images.len(), 1, "the first frame is there");
    assert!(!h.menu_enabled(Cmd::ClearMarks), "not before they are read");
    // F waits for the read and then applies, in order.
    let _ = h.app.key(Letter('F'), false, false);
    let _ = h.app.key(Right, false, false);
    h.pump_holding_marks();
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
    drop(held);
    h.pump();
    assert_eq!(marked(&h), ["a.png", "b.png"]);
    // A write held back does not hold up keys either.
    let held = crate::marks::HOLD.lock().unwrap();
    let _ = h.app.key(Letter('F'), false, false);
    let _ = h.app.key(Left, false, false);
    h.pump_holding_marks();
    assert_eq!(marked(&h), ["a.png"]);
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
    drop(held);
    h.pump();
    assert_eq!(picks::load(&store(&h), &h.images).unwrap().len(), 1);
}

/// AC-11
#[test]
fn a_missing_file_keeps_its_mark_until_unmarked_and_the_recycle_bin_drops_it() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    h.key(Letter('G'));
    h.ctrl(Letter('A'));
    h.pump();
    // b goes away outside the viewer: not counted, still stored.
    let b = std::fs::read(h.images.join("b.png")).unwrap();
    std::fs::remove_file(h.images.join("b.png")).unwrap();
    h.app.on_dir_changed();
    h.wait_for(|h| h.names().len() == 2);
    assert_eq!(h.app.marked_count(), 2);
    assert!(marked(&h).contains(&"b.png".to_string()));
    // It comes back marked.
    std::fs::write(h.images.join("b.png"), b).unwrap();
    h.app.on_dir_changed();
    h.wait_for(|h| h.names().len() == 3);
    assert_eq!(h.app.marked_count(), 3);
    // The recycle bin from the viewer drops the mark for good.
    let a = h.command(Cmd::Recycle);
    h.shell(a);
    h.pump();
    assert!(!marked(&h).contains(&"a.png".to_string()));
    assert!(
        !picks::load(&store(&h), &h.images)
            .unwrap()
            .contains("a.png")
    );
}

/// AC-12
#[test]
fn a_broken_marks_file_counts_its_valid_names_and_a_failed_write_says_so_once() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_png("b.png", 64, 48);
    let file = picks::file_of(&store(&h), &h.images);
    std::fs::create_dir_all(store(&h)).unwrap();
    let mut text = picks::to_text(&h.images, &Default::default()).into_bytes();
    text.extend(b"a.png\n\xff\xfe\nsub/b.png\n\n");
    std::fs::write(&file, text).unwrap();
    h.open("a.png");
    h.pump();
    assert_eq!(marked(&h), ["a.png"]);
    // The store turns into a file: writing fails.
    std::fs::remove_dir_all(store(&h)).unwrap();
    std::fs::write(store(&h), "not a folder").unwrap();
    h.key(Right);
    h.pump();
    h.key(Letter('F'));
    h.pump();
    assert_eq!(marked(&h), ["a.png", "b.png"], "marking still works");
    let notice = h.app.notice.clone().unwrap_or_default();
    assert!(notice.starts_with("Marks cannot be saved:"), "{notice}");
    h.app.notice = None;
    h.key(Letter('F'));
    h.pump();
    assert_eq!(h.app.notice, None, "once per session");
}
