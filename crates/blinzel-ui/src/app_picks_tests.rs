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

// ------------------------------------------------------------ handing over

fn mark_all(h: &mut Harness, names: &[&str]) {
    for n in names {
        h.open(n);
        h.key(Letter('F'));
    }
    h.pump();
}

/// AC-14
#[test]
fn ctrl_shift_c_puts_the_marked_names_on_the_clipboard_in_folder_order() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    mark_all(&mut h, &["c.png", "a.png"]);
    let a = h.ctrl_shift(Letter('C'));
    let eol = if cfg!(windows) { "\r\n" } else { "\n" };
    assert_eq!(a, [Action::CopyText(format!("a.png{eol}c.png{eol}"))]);
    assert_eq!(h.host().clipboard_offers, 0, "not the image");
    h.app.text_copied(Ok(()));
    assert_eq!(h.app.notice.as_deref(), Some("File names copied: 2 images"));
    // The menu does the same; a clipboard that refuses says why.
    let a = h.command(Cmd::CopyMarkedNames);
    assert!(matches!(&a[..], [Action::CopyText(_)]));
    h.app.text_copied(Err("busy".into()));
    assert_eq!(
        h.app.notice.as_deref(),
        Some("Could not copy to the clipboard: busy")
    );
    // A key clears the App's own notice.
    h.key(Right);
    assert_eq!(h.app.notice, None);
}

/// AC-15
#[test]
fn without_marks_the_handover_is_disabled() {
    let mut h = folder(&["a.png", "b.png"]);
    assert!(!h.menu_enabled(Cmd::CopyMarkedNames));
    assert!(!h.menu_enabled(Cmd::CopyMarkedFiles));
    assert!(!h.menu_enabled(Cmd::ToggleOnlyMarked));
    assert_eq!(h.ctrl_shift(Letter('C')), []);
    assert_eq!(h.app.notice.as_deref(), Some("No images marked"));
    h.key(Letter('F'));
    assert!(h.menu_enabled(Cmd::CopyMarkedNames));
    assert!(h.menu_enabled(Cmd::CopyMarkedFiles));
}

/// AC-16, AC-17, AC-18
#[test]
fn copying_marked_files_makes_byte_identical_copies_and_never_replaces() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    mark_all(&mut h, &["a.png", "c.png"]);
    let out = h.images.parent().unwrap().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("a.png"), b"someone else's").unwrap();
    assert_eq!(h.command(Cmd::CopyMarkedFiles), [Action::PickFolder]);
    h.app.batch_folder_picked(out.clone());
    assert_eq!(h.app.notice.as_deref(), Some("Copying 0 / 2 …"));
    assert!(!h.menu_enabled(Cmd::CopyMarkedFiles), "one copy at a time");
    // Browsing goes on meanwhile.
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
    h.wait_for(|h| {
        h.app
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("copied to"))
    });
    assert_eq!(h.app.notice.as_deref(), Some("2 images copied to “out”"));
    let bytes = |p: PathBuf| std::fs::read(p).unwrap();
    assert_eq!(bytes(out.join("a (2).png")), bytes(h.images.join("a.png")));
    assert_eq!(bytes(out.join("c.png")), bytes(h.images.join("c.png")));
    assert_eq!(bytes(out.join("a.png")), b"someone else's");
    assert!(!out.join("b.png").exists());
    assert!(h.menu_enabled(Cmd::CopyMarkedFiles));
    // Cancelling the dialog: the shell calls nothing back, nothing happens.
    h.command(Cmd::CopyMarkedFiles);
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 3);
}

/// AC-19
#[test]
fn copying_reports_failures_and_refuses_the_images_own_folder() {
    let mut h = folder(&["a.png", "b.png"]);
    mark_all(&mut h, &["a.png", "b.png"]);
    h.command(Cmd::CopyMarkedFiles);
    let images = h.images.clone();
    h.app.batch_folder_picked(images.join(".").join(""));
    assert_eq!(
        h.app.notice.as_deref(),
        Some("Choose a different folder than the images' own")
    );
    // One source vanished after it was listed.
    std::fs::remove_file(h.images.join("b.png")).unwrap();
    let out = h.images.parent().unwrap().join("out");
    std::fs::create_dir(&out).unwrap();
    h.command(Cmd::CopyMarkedFiles);
    h.app.batch_folder_picked(out.clone());
    h.wait_for(|h| {
        h.app
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("failed"))
    });
    let n = h.app.notice.clone().unwrap();
    assert!(n.starts_with("1 of 2 copied; 1 failed: "), "{n}");
    assert!(out.join("a.png").exists());
}

// ------------------------------------------------------------- only marked

/// AC-21, AC-22
#[test]
fn only_marked_browses_the_marked_images_and_counts_them() {
    let mut h = folder(&["a.png", "b.png", "c.png", "d.png"]);
    mark_all(&mut h, &["b.png", "d.png"]);
    h.open("a.png");
    h.command(Cmd::ToggleOnlyMarked);
    h.pump();
    assert!(h.menu_checked(Cmd::ToggleOnlyMarked));
    assert_eq!(
        h.current_name().as_deref(),
        Some("b.png"),
        "the next marked one"
    );
    assert_eq!(h.names(), ["b.png", "d.png"]);
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("d.png"));
    h.key(Right);
    assert_eq!(
        h.current_name().as_deref(),
        Some("b.png"),
        "round to the first marked"
    );
    h.key(Letter('I'));
    h.wait_for(|h| h.info_lines().get(1).map(String::as_str) == Some("1 / 2"));
    h.key(Letter('G'));
    h.render();
    assert!(
        h.frame()
            .texts
            .iter()
            .any(|t| t == "2 images marked · Only marked"),
        "{:?}",
        h.frame().texts
    );
}

/// AC-21: no marked image after the shown one: the previous marked one.
#[test]
fn only_marked_falls_back_to_the_previous_marked_image() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    mark_all(&mut h, &["a.png"]);
    h.open("c.png");
    h.command(Cmd::ToggleOnlyMarked);
    h.pump();
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
}

/// AC-23
#[test]
fn unmarking_under_only_marked_keeps_the_image_until_moving_on() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    mark_all(&mut h, &["a.png", "b.png", "c.png"]);
    h.open("b.png");
    h.command(Cmd::ToggleOnlyMarked);
    h.pump();
    h.key(Letter('F'));
    assert_eq!(h.current_name().as_deref(), Some("b.png"), "stays shown");
    assert_eq!(h.names(), ["a.png", "c.png"]);
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
    h.key(Left);
    assert_eq!(
        h.current_name().as_deref(),
        Some("a.png"),
        "b is not reached"
    );
    // The last marks go: "Only marked" turns off by itself.
    h.key(Letter('F'));
    h.key(Right);
    h.key(Letter('F'));
    assert!(!h.menu_checked(Cmd::ToggleOnlyMarked));
    assert_eq!(h.names().len(), 3);
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
}

/// AC-24
#[test]
fn only_marked_ends_when_turned_off_or_in_another_folder() {
    let mut h = folder(&["a.png", "b.png", "c.png"]);
    mark_all(&mut h, &["b.png"]);
    h.command(Cmd::ToggleOnlyMarked);
    h.pump();
    h.command(Cmd::ToggleOnlyMarked);
    assert!(!h.menu_checked(Cmd::ToggleOnlyMarked));
    assert_eq!(h.names().len(), 3);
    assert_eq!(
        h.current_name().as_deref(),
        Some("b.png"),
        "from the image shown"
    );
    h.command(Cmd::ToggleOnlyMarked);
    let other = h.images.join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("x.png"), crate::testkit::png(8, 8)).unwrap();
    std::fs::write(other.join("y.png"), crate::testkit::png(8, 8)).unwrap();
    h.app.open(other.join("x.png"));
    h.pump();
    assert!(!h.menu_checked(Cmd::ToggleOnlyMarked));
    assert_eq!(h.names(), ["x.png", "y.png"]);
}

/// AC-25
#[test]
fn the_marks_entries_sit_with_filmstrip_and_grid() {
    let h = folder(&["a.png"]);
    let cmd = |i: &crate::menu::MenuItem| match i {
        crate::menu::MenuItem::Item { cmd, .. } => Some(*cmd),
        _ => None,
    };
    let view = crate::menu::items(&h.menu_state())
        .into_iter()
        .find_map(|m| match m {
            crate::menu::MenuItem::Sub(_, items)
                if items.iter().any(|i| cmd(i) == Some(Cmd::ToggleGrid)) =>
            {
                Some(items)
            }
            _ => None,
        })
        .unwrap();
    let cmds: Vec<Cmd> = view.iter().filter_map(cmd).collect();
    for c in [
        Cmd::ToggleOnlyMarked,
        Cmd::CopyMarkedNames,
        Cmd::CopyMarkedFiles,
        Cmd::ClearMarks,
    ] {
        assert!(cmds.contains(&c), "{c:?}");
    }
}
