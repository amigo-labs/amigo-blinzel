//! App-level tests of selecting, copying, cutting and saving a cutout
//! (`docs/specs/selection-and-export.md`).

use super::*;
use crate::testkit::{Harness, IMAGE_SIZE};
use Key::*;

const W: u32 = IMAGE_SIZE.0;
const H: u32 = IMAGE_SIZE.1;

/// A JPEG of `w` × `h` (4:2:0, quality 90) in the images' folder.
fn write_jpeg(h: &Harness, name: &str, w: u32, ht: u32) -> PathBuf {
    let opts = blinzel_core::decode::jpeg::EncodeOptions {
        quality: 90,
        subsampling: 420,
        ..Default::default()
    };
    let rgb = blinzel_core::testimg::rgb(w, ht);
    h.write_file(
        name,
        &blinzel_core::decode::jpeg::encode_rgb(&rgb, w, ht, opts).unwrap(),
    )
}

// A cut's `FitWindow` is not applied: the harness has no minimum window
// size, so the window would shrink to the crop under the menu button.

/// Drags a selection from image point `a` to `b`.
fn select(h: &mut Harness, a: (f64, f64), b: (f64, f64)) -> PixelRect {
    let (from, to) = (h.at(a), h.at(b));
    h.drag(from, to);
    h.app.selection().unwrap().to_pixels(W, H).unwrap()
}

fn fit_window(a: &[Action]) -> bool {
    a.iter().any(|a| matches!(a, Action::FitWindow { .. }))
}

#[test]
fn a_short_click_outside_the_selection_clears_it() {
    let mut h = Harness::with_images(&["a.png"]);
    select(&mut h, (4.0, 4.0), (24.0, 20.0));
    // Pressed outside it, released after moving 2 logical px.
    let p = h.at((50.0, 40.0));
    h.mouse_down(p);
    h.mouse_move((p.0 + 2.0, p.1 + 1.0));
    h.mouse_up();
    assert!(h.app.selection().is_none(), "cleared, no new one");
    // Moving more than 3 px makes a new selection instead.
    select(&mut h, (4.0, 4.0), (24.0, 20.0));
    h.mouse_down(p);
    h.mouse_move((p.0 + 6.0, p.1 + 6.0));
    h.mouse_up();
    let s = h.app.selection().unwrap();
    assert!(s.x > 40.0 && s.w > 3.0, "{s:?}");
}

#[test]
fn a_short_click_clears_the_selection_while_left_drag_pans() {
    let mut h = Harness::new();
    h.write_png("big.png", 1600, 1200);
    h.open("big.png");
    h.key(Digit(3));
    assert!(h.layout().unwrap().overflows);
    // Shift+drag selects in a larger image.
    let (a, b) = (h.at((700.0, 500.0)), h.at((900.0, 650.0)));
    h.shift_down(a);
    for k in 1..=4 {
        let t = k as f64 / 4.0;
        h.mouse_move((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
    }
    h.mouse_up();
    assert!(h.app.selection().is_some());
    // A plain drag pans and keeps it.
    let view = h.view();
    h.drag((600.0, 500.0), (560.0, 470.0));
    assert_ne!(h.view(), view, "panned");
    assert!(h.app.selection().is_some());
    // A click outside it clears it.
    h.mouse_down((600.0, 500.0));
    h.mouse_move((601.0, 502.0));
    h.mouse_up();
    assert!(h.app.selection().is_none());
}

#[test]
fn select_all_takes_the_whole_image_or_the_cropped_region() {
    let mut h = Harness::with_images(&["a.png"]);
    h.ctrl(Letter('A'));
    assert_eq!(
        h.app.selection(),
        Some(Rect::new(0.0, 0.0, W as f64, H as f64))
    );
    h.key(Escape);
    h.command(Cmd::SelectAll);
    assert_eq!(
        h.app.selection(),
        Some(Rect::new(0.0, 0.0, W as f64, H as f64))
    );
    let r = select(&mut h, (8.0, 6.0), (40.0, 30.0));
    h.ctrl(Letter('X'));
    assert_eq!(h.view().crop, Some(r));
    h.ctrl(Letter('A'));
    assert_eq!(h.app.selection(), Some(r.to_rect()));
}

#[test]
fn escape_ends_a_drag_then_clears_then_uncrops_then_leaves_full_screen_then_closes() {
    let mut h = Harness::with_images(&["a.png"]);
    let (a, b) = (h.at((4.0, 4.0)), h.at((30.0, 30.0)));
    h.mouse_down(a);
    h.mouse_move(b);
    assert!(h.app.is_dragging());
    assert_eq!(h.key(Escape), []);
    assert!(!h.app.is_dragging());
    assert!(
        h.app.selection().is_some(),
        "the drag ended, the selection stays"
    );
    // The pointer moving on does not change it any more.
    let sel = h.app.selection();
    h.mouse_move(h.at((50.0, 40.0)));
    assert_eq!(h.app.selection(), sel);
    h.mouse_up();
    assert!(h.app.selection().is_some(), "releasing is no click");
    assert_eq!(h.key(Escape), []);
    assert!(h.app.selection().is_none());
    // A cut view goes back to the original.
    select(&mut h, (4.0, 4.0), (30.0, 30.0));
    h.ctrl(Letter('X'));
    assert!(h.view().crop.is_some());
    h.key(Escape);
    assert!(h.view().crop.is_none());
    // Then full screen, then the window.
    let a = h.key(Enter);
    h.shell(a);
    assert!(h.app.fullscreen);
    let a = h.key(Escape);
    assert_eq!(a, [Action::ToggleFullscreen]);
    h.shell(a);
    assert_eq!(h.key(Escape), [Action::Close]);
}

#[test]
fn copy_announces_the_selection_else_the_crop_else_the_whole_image() {
    let mut h = Harness::with_images(&["a.png"]);
    let path = h.images.join("a.png");
    h.ctrl(Letter('C'));
    let clip = h.app.clip.clone().unwrap();
    assert_eq!(clip.path, path, "decoded from the file");
    assert_eq!(clip.stored_rect, PixelRect::new(0, 0, W, H));
    assert_eq!(clip.orientation, Orientation::NORMAL);
    // The selection.
    let r = select(&mut h, (8.0, 6.0), (40.0, 30.0));
    h.command(Cmd::Copy);
    assert_eq!(h.app.clip.as_ref().unwrap().stored_rect, r);
    assert_eq!(h.host().clipboard_offers, 2);
    // The cropped region after a cut (which copies too).
    h.ctrl(Letter('X'));
    assert_eq!(h.host().clipboard_offers, 3);
    h.app.clip = None;
    h.ctrl(Letter('C'));
    assert_eq!(h.app.clip.as_ref().unwrap().stored_rect, r);
}

#[test]
fn copy_maps_a_rotated_view_back_to_the_file_at_full_resolution() {
    let mut h = Harness::new();
    h.write_png("big.png", 1600, 1200);
    h.open("big.png");
    assert!(
        h.layout().unwrap().scale < 1.0,
        "shown smaller than the file"
    );
    h.ctrl(Letter('A'));
    h.ctrl(Letter('C'));
    let clip = h.app.clip.clone().unwrap();
    assert_eq!(clip.stored_rect, PixelRect::new(0, 0, 1600, 1200));
    // Rotated: the selection is in the orientation shown (1200 × 1600),
    // the clip in the file's, with the rotation to apply.
    let a = h.key(Letter('R'));
    h.shell(a);
    let cw = Orientation::NORMAL.rotated_cw();
    let shown = Rect::new(100.0, 200.0, 300.0, 400.0);
    h.app.selection = Some(shown);
    h.ctrl(Letter('C'));
    let clip = h.app.clip.clone().unwrap();
    assert_eq!(clip.orientation, cw);
    let want = selection::to_stored(&shown.to_pixels(1200, 1600).unwrap(), cw, (1600, 1200));
    assert_eq!(clip.stored_rect, want);
    assert_eq!((want.w, want.h), (400, 300));
}

#[test]
fn clip_pixels_are_the_files_region_in_the_orientation_shown() {
    let mut h = Harness::with_images(&["a.png"]);
    h.key(Letter('R'));
    h.ctrl(Letter('C'));
    let clip = h.app.clip.clone().unwrap();
    let (px, w, ht) = clip.pixels(&*h.app.services).unwrap();
    assert_eq!((w, ht), (H, W), "turned upright");
    let rgb = blinzel_core::testimg::rgb(W, H);
    // Turned clockwise: the top-left pixel is the file's bottom-left one.
    let src = (H as usize - 1) * W as usize * 3;
    assert_eq!(
        [px[2], px[1], px[0], px[3]],
        [rgb[src], rgb[src + 1], rgb[src + 2], 255]
    );
}

#[test]
fn cut_crops_the_view_to_the_selection_and_needs_one() {
    let mut h = Harness::with_images(&["a.png"]);
    // Without a selection: nothing, and the menu entry is off.
    assert!(!h.menu_enabled(Cmd::Cut));
    assert_eq!(h.ctrl(Letter('X')), []);
    assert_eq!(h.view().crop, None);
    assert_eq!(h.host().clipboard_offers, 0);
    h.key(Plus);
    let r = select(&mut h, (8.0, 6.0), (40.0, 30.0));
    assert!(h.menu_enabled(Cmd::Cut));
    let a = h.ctrl(Letter('X'));
    assert_eq!(h.host().clipboard_offers, 1, "copied");
    assert_eq!(h.app.clip.as_ref().unwrap().stored_rect, r);
    assert_eq!(h.view().crop, Some(r));
    assert_eq!(h.view().zoom, None);
    assert_eq!(h.view().center, None);
    assert!(h.app.selection().is_none());
    assert!(fit_window(&a), "fitted as for a new image: {a:?}");
    let l = h.layout().unwrap();
    assert_eq!(l.content, (r.w as f64, r.h as f64));
    // A cut within the crop replaces it.
    let inner = select(&mut h, (10.0, 8.0), (20.0, 20.0));
    h.ctrl(Letter('X'));
    assert_eq!(h.view().crop, Some(inner));
}

#[test]
fn show_original_navigation_and_rotation_drop_the_crop() {
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    assert!(!h.menu_enabled(Cmd::ShowOriginal));
    let cut = |h: &mut Harness| {
        select(h, (8.0, 6.0), (40.0, 30.0));
        h.ctrl(Letter('X'));
        assert!(h.view().crop.is_some());
    };
    cut(&mut h);
    assert!(h.menu_enabled(Cmd::ShowOriginal));
    let a = h.command(Cmd::ShowOriginal);
    assert!(fit_window(&a), "{a:?}");
    assert_eq!(h.view(), ViewState::default());
    assert!(!h.menu_enabled(Cmd::ShowOriginal));
    // Another image.
    cut(&mut h);
    select(&mut h, (10.0, 8.0), (20.0, 20.0));
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
    assert_eq!(h.view().crop, None);
    assert!(h.app.selection().is_none());
    // Rotating.
    cut(&mut h);
    select(&mut h, (10.0, 8.0), (20.0, 20.0));
    h.key(Letter('R'));
    assert_eq!(h.view().crop, None);
    assert!(h.app.selection().is_none());
}

#[test]
fn save_asks_with_a_name_by_source_type_and_prefers_the_selection() {
    let mut h = Harness::new();
    write_jpeg(&h, "photo.jpg", 64, 48);
    h.write_png("pic.png", 64, 48);
    h.open("photo.jpg");
    // Nothing to save: no question, the menu entry is off.
    assert!(!h.menu_enabled(Cmd::SaveCutout));
    assert_eq!(h.ctrl(Letter('S')), []);
    let r = select(&mut h, (8.0, 6.0), (40.0, 30.0));
    assert!(h.menu_enabled(Cmd::SaveCutout));
    let job = match &h.ctrl(Letter('S'))[..] {
        [Action::SaveCutout(job)] => job.clone(),
        a => panic!("{a:?}"),
    };
    assert_eq!(
        job.source,
        h.images.join("photo.jpg"),
        "in the source's folder"
    );
    assert_eq!(job.default_name, "photo_cutout.jpg");
    assert!(!job.prefer_png);
    assert_eq!(job.cut.stored_rect, r);
    // A crop alone is saved too; a selection in it wins.
    h.ctrl(Letter('X'));
    assert!(h.menu_enabled(Cmd::SaveCutout));
    match &h.command(Cmd::SaveCutout)[..] {
        [Action::SaveCutout(job)] => assert_eq!(job.cut.stored_rect, r),
        a => panic!("{a:?}"),
    }
    let inner = select(&mut h, (10.0, 8.0), (20.0, 20.0));
    match &h.ctrl(Letter('S'))[..] {
        [Action::SaveCutout(job)] => assert_eq!(job.cut.stored_rect, inner),
        a => panic!("{a:?}"),
    }
    // Any other source proposes PNG, with the suffix translated.
    h.open("pic.png");
    h.command(Cmd::Language(blinzel_i18n::lang_from_code("de")));
    h.ctrl(Letter('A'));
    match &h.ctrl(Letter('S'))[..] {
        [Action::SaveCutout(job)] => {
            assert_eq!(job.default_name, "pic_ausschnitt.png");
            assert!(job.prefer_png);
        }
        a => panic!("{a:?}"),
    }
}

#[test]
fn nothing_happens_before_the_header_is_known() {
    let mut h = Harness::new();
    h.write_file("bad.png", b"not an image");
    h.open("bad.png");
    assert_eq!(h.ctrl(Letter('C')), []);
    assert_eq!(h.ctrl(Letter('S')), []);
    assert!(h.app.clip.is_none());
    assert_eq!(h.host().clipboard_offers, 0);
}
