//! App-level tests of the grid, the filmstrip and the thumbnails they show
//! (`docs/specs/thumbnails.md`).

use super::*;
use crate::testkit::Harness;
use Key::*;

fn centre(r: Rect) -> (f64, f64) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// `names` in a folder, the first open, the grid open on it.
fn grid_on(names: &[&str]) -> Harness {
    let mut h = Harness::with_images(names);
    h.key(Letter('G'));
    assert_eq!(h.grid_selected(), Some(0));
    h
}

#[test]
fn delete_in_the_grid_recycles_the_selected_tile_not_the_open_image() {
    let mut h = grid_on(&["a.png", "b.png", "c.png"]);
    h.key(Right);
    h.key(Right);
    assert_eq!(h.grid_selected(), Some(2));
    let a = h.key(Delete);
    assert_eq!(a, [Action::Recycle(h.images.join("c.png"))]);
    h.shell(a);
    assert!(h.images.join("a.png").exists(), "the open image stays");
    assert_eq!(h.names(), ["a.png", "b.png"]);
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
    assert_eq!(h.grid_selected(), Some(1), "clamped to the images left");
    // The menu's command takes the selected tile too.
    let a = h.command(Cmd::Recycle);
    assert_eq!(a, [Action::Recycle(h.images.join("b.png"))]);
    h.shell(a);
    assert_eq!(h.names(), ["a.png"]);
    assert_eq!(h.grid_selected(), Some(0));
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
}

#[test]
fn deleting_a_marked_tile_drops_its_mark() {
    let mut h = grid_on(&["a.png", "b.png", "c.png"]);
    h.ctrl(Letter('A'));
    h.key(Right);
    let a = h.key(Delete);
    h.shell(a);
    let marked: Vec<&str> = h
        .app
        .browse
        .grid
        .as_ref()
        .unwrap()
        .marked
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(marked, ["a.png", "c.png"]);
}

#[test]
fn image_keys_do_nothing_to_the_open_image_while_the_grid_is_open() {
    let mut h = grid_on(&["a.png", "b.png"]);
    h.key(Right);
    let view = h.view();
    h.key(Plus);
    h.key(Minus);
    h.key(Letter('R'));
    h.press(Letter('R'), false, true).unwrap();
    h.key(Letter('L'));
    h.ctrl(Letter('C'));
    h.ctrl(Letter('X'));
    h.ctrl(Letter('S'));
    assert_eq!(h.host().clipboard_offers, 0, "nothing copied");
    assert!(h.app.clip.is_none());
    assert_eq!(h.app.user_rotation(), Orientation::NORMAL, "not rotated");
    assert_eq!(h.view(), view, "not zoomed or cropped");
    assert!(
        !h.actions()
            .iter()
            .any(|a| matches!(a, Action::SaveCutout(_))),
        "{:?}",
        h.actions()
    );
    assert_eq!(h.grid_selected(), Some(1), "the grid stays open");
    // Closed again, they act on the open image.
    h.key(Letter('G'));
    h.ctrl(Letter('C'));
    assert_eq!(h.host().clipboard_offers, 1);
    assert_eq!(h.app.clip.as_ref().unwrap().path, h.images.join("a.png"));
}

#[test]
fn edit_and_open_with_in_the_grid_take_the_selected_tile() {
    let mut h = grid_on(&["a.png", "b.png"]);
    h.key(Right);
    let b = h.images.join("b.png");
    assert_eq!(h.command(Cmd::OpenWith), [Action::OpenWith(b.clone())]);
    match h.key(Letter('E')).as_slice() {
        [Action::ChooseEditor { then }] => assert_eq!(then.as_ref(), Some(&b)),
        a => panic!("{a:?}"),
    }
}

// ------------------------------------------------------- marks and batch

#[test]
fn resize_images_starts_with_the_marked_files_in_folder_order() {
    let mut h = grid_on(&["a.png", "b.png", "c.png", "d.png"]);
    for i in [3, 1, 2] {
        let p = centre(h.grid_cell(i).unwrap());
        h.ctrl_click(p);
    }
    assert_eq!(h.grid_marked(), ["b.png", "c.png", "d.png"]);
    // Gone from the disk since it was marked.
    std::fs::remove_file(h.images.join("c.png")).unwrap();
    h.ctrl(Letter('B'));
    let want = vec![h.images.join("b.png"), h.images.join("d.png")];
    assert_eq!(h.batch_files(), Some(want.clone()));
    h.key(Escape);
    assert_eq!(h.batch_files(), None);
    // The menu does the same; the marks are still there.
    h.command(Cmd::Batch);
    assert_eq!(h.batch_files(), Some(want));
    h.key(Escape);
    // Without marks the list starts empty.
    h.key(Escape);
    assert!(h.grid_marked().is_empty());
    assert_eq!(h.grid_selected(), Some(2), "the grid stays open");
    h.ctrl(Letter('B'));
    assert_eq!(h.batch_files(), Some(vec![]));
}

#[test]
fn resize_images_outside_the_grid_starts_empty() {
    let mut h = grid_on(&["a.png", "b.png"]);
    h.ctrl(Letter('A'));
    // Closing the grid drops its marks.
    h.key(Letter('G'));
    h.ctrl(Letter('B'));
    assert_eq!(h.batch_files(), Some(vec![]));
}

/// `docs/specs/batch-resize.md` AC-22: Ctrl+B or the menu opens the modal
/// dialog with the defaults, then with the settings last used.
#[test]
fn resize_images_opens_a_modal_dialog_that_keeps_its_settings() {
    use blinzel_core::batch::{Job, Output, Size};
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    h.ctrl(Letter('B'));
    let title = blinzel_i18n::tr(blinzel_i18n::Msg::BatchTitle);
    assert!(h.frame().texts.iter().any(|t| t == title), "drawn");
    assert_eq!(h.frame().images.len(), 1, "over the viewer");
    let job = h.app.dialog.as_ref().unwrap().job().unwrap();
    assert_eq!(job.size, Size::LongEdge(1920));
    assert!(!job.enlarge, "don't enlarge");
    assert_eq!(job.output, Output::Same, "format as source");
    assert_eq!(job.quality.jpeg, 85);
    assert!(job.keep_exif && job.keep_gps && job.keep_icc);
    assert_eq!(job.pattern, "{name}");
    assert_eq!(job.out_dir, Job::default().out_dir);
    assert_eq!(job.out_dir, PathBuf::from("resized"));
    // Modal: keys and clicks go to the dialog, not to the viewer.
    h.key(Right);
    h.key(Letter('G'));
    h.drag((380.0, 285.0), (420.0, 315.0));
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
    assert_eq!(h.grid_selected(), None);
    assert!(h.app.selection().is_none());
    // A new long edge, then closed and opened again from the menu.
    h.key(Tab);
    for _ in 0..4 {
        h.key(Backspace);
    }
    h.app.text_input("1280");
    h.key(Escape);
    assert!(h.app.dialog.is_none());
    h.command(Cmd::Batch);
    let job = h.app.dialog.as_ref().unwrap().job().unwrap();
    assert_eq!(job.size, Size::LongEdge(1280), "kept in this session");
    assert_eq!(h.batch_files(), Some(vec![]));
}
