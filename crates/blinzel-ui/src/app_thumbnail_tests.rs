//! App-level tests of the grid, the filmstrip and the thumbnails they show
//! (`docs/specs/thumbnails.md`).

use super::*;
use crate::testkit::Harness;
use Key::*;

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
