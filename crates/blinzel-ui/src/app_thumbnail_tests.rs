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

// ------------------------------------------------------------------ grid

/// `n` images `00.png`, `01.png`, … with the first open.
fn folder_of(n: usize) -> Harness {
    let names: Vec<String> = (0..n).map(|i| format!("{i:02}.png")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    Harness::with_images(&refs)
}

fn scroll(h: &Harness) -> f64 {
    h.app.browse.grid.as_ref().unwrap().scroll
}

/// Tile `i` is fully on screen, and with `snug` its bottom gap touches
/// the window's bottom edge (scrolled just enough).
fn assert_shown(h: &Harness, i: usize, snug: bool) {
    let (gap, height) = (12.0, crate::testkit::CLIENT.1 as f64);
    let r = h.grid_cell(i).unwrap();
    assert!(
        r.y >= gap - 1e-9 && r.bottom() + gap <= height + 1e-9,
        "{i}: {r:?}"
    );
    if snug {
        assert!((r.bottom() + gap - height).abs() < 1e-9, "{i}: {r:?}");
    }
}

#[test]
fn grid_keys_move_the_selection_clamped_and_scrolled_just_enough() {
    let mut h = folder_of(30);
    h.open("25.png");
    let g = grid_layout(30, (800.0, 600.0), 1.0);
    let (cols, page) = (g.cols, g.page_rows(600.0));
    assert_eq!((cols, page), (4, 3));
    h.key(Letter('G'));
    assert_eq!(h.grid_selected(), Some(25), "the current image");
    assert_shown(&h, 25, true);
    assert!(h.frame().images.is_empty(), "over the whole window");
    assert!(h.menu_state().grid);
    let steps = [
        (Right, 26),
        (Left, 25),
        (Up, 25 - cols),
        (Down, 25),
        (Down, 29),
        (Down, 29),
        (Right, 29),
    ];
    for (key, want) in steps {
        h.key(key);
        assert_eq!(h.grid_selected(), Some(want), "{key:?}");
        assert_shown(&h, want, false);
    }
    h.key(Home);
    assert_eq!(h.grid_selected(), Some(0));
    assert_eq!(scroll(&h), 0.0);
    h.key(Up);
    h.key(Left);
    assert_eq!(h.grid_selected(), Some(0), "clamped");
    h.key(PageDown);
    assert_eq!(h.grid_selected(), Some(page * cols));
    assert_shown(&h, page * cols, true);
    h.key(PageUp);
    assert_eq!(h.grid_selected(), Some(0));
    assert_shown(&h, 0, false);
    h.key(End);
    assert_eq!(h.grid_selected(), Some(29));
    assert_shown(&h, 29, true);
    h.key(PageDown);
    assert_eq!(h.grid_selected(), Some(29), "clamped");
}

#[test]
fn grid_wheel_scrolls_a_row_per_notch_and_clicks_select() {
    let mut h = folder_of(30);
    h.key(Letter('G'));
    let pitch = 176.0 + 12.0;
    h.wheel((400.0, 300.0), -120);
    assert_eq!(scroll(&h), pitch, "one row down");
    h.wheel((400.0, 300.0), -240);
    assert_eq!(scroll(&h), 3.0 * pitch);
    h.wheel((400.0, 300.0), 120);
    assert_eq!(scroll(&h), 2.0 * pitch, "one row up");
    // A click selects without opening.
    let c = centre(h.grid_cell(9).unwrap());
    assert_eq!(h.click(c), []);
    assert_eq!(h.grid_selected(), Some(9));
    assert_eq!(h.current_name().as_deref(), Some("00.png"));
    // At this scroll offset the same point is tile 9, not tile 1.
    assert!(h.grid_cell(1).unwrap().bottom() < 0.0);
}

#[test]
fn grid_opens_with_enter_space_or_a_double_click_and_closes_with_g_or_escape() {
    let mut h = folder_of(6);
    h.key(Letter('G'));
    h.key(Right);
    h.key(Enter);
    assert_eq!(h.grid_selected(), None, "closed");
    assert_eq!(h.current_name().as_deref(), Some("01.png"));
    h.key(Letter('G'));
    assert_eq!(h.grid_selected(), Some(1), "opens on the current image");
    h.key(Right);
    h.key(Space);
    assert_eq!(h.grid_selected(), None);
    assert_eq!(h.current_name().as_deref(), Some("02.png"));
    h.command(Cmd::ToggleGrid);
    let c = centre(h.grid_cell(4).unwrap());
    h.double_click(c).unwrap();
    assert_eq!(h.grid_selected(), None);
    assert_eq!(h.current_name().as_deref(), Some("04.png"));
    h.key(Letter('G'));
    h.key(Right);
    h.key(Letter('G'));
    assert_eq!(h.grid_selected(), None);
    assert_eq!(
        h.current_name().as_deref(),
        Some("04.png"),
        "G opens nothing"
    );
    h.key(Letter('G'));
    h.key(Left);
    assert_eq!(h.key(Escape), [], "Escape closes the grid, not the window");
    assert_eq!(h.grid_selected(), None);
    assert_eq!(h.current_name().as_deref(), Some("04.png"));
}

#[test]
fn an_empty_grid_opens_nothing() {
    let mut h = Harness::new();
    h.key(Letter('G'));
    assert_eq!(h.grid_selected(), Some(0));
    assert_eq!(h.double_click((100.0, 100.0)), Some(vec![]));
    assert_eq!(h.grid_selected(), Some(0));
    h.key(Enter);
    assert_eq!(h.grid_selected(), None);
    assert_eq!(h.current_name(), None);
}

/// Lines in the last frame: a check mark is two (the menu button has
/// some too).
fn lines(h: &Harness) -> usize {
    h.frame()
        .shapes
        .iter()
        .filter(|s| matches!(s, Shape::Line { .. }))
        .count()
}

#[test]
fn grid_marks_with_ctrl_shift_and_ctrl_a_and_escape_clears_them() {
    let mut h = folder_of(6);
    h.key(Letter('G'));
    let at = |h: &Harness, i: usize| centre(h.grid_cell(i).unwrap());
    let base = lines(&h);
    let check_marks = |h: &Harness| (lines(h) - base) / 2;
    let p = at(&h, 1);
    h.ctrl_click(p);
    assert_eq!(h.grid_marked(), ["01.png"]);
    assert_eq!(h.grid_selected(), Some(1));
    assert_eq!(check_marks(&h), 1, "a check mark on the marked tile");
    h.ctrl_click(p);
    assert!(h.grid_marked().is_empty(), "toggled off");
    h.ctrl_click(p);
    let p = at(&h, 4);
    h.shift_down(p);
    h.mouse_up();
    assert_eq!(h.grid_marked(), ["01.png", "02.png", "03.png", "04.png"]);
    assert_eq!(check_marks(&h), 4);
    let p = at(&h, 0);
    h.click(p);
    assert!(h.grid_marked().is_empty(), "a plain click clears the marks");
    assert_eq!(check_marks(&h), 0);
    h.ctrl(Letter('A'));
    assert_eq!(h.grid_marked().len(), 6);
    assert_eq!(check_marks(&h), 6);
    assert_eq!(h.key(Escape), []);
    assert!(h.grid_marked().is_empty());
    assert_eq!(h.grid_selected(), Some(0), "still open");
    h.key(Escape);
    assert_eq!(h.grid_selected(), None);
}

// ------------------------------------------------------------- filmstrip

const STRIP: f64 = 84.0;

fn strip_band(h: &Harness) -> bool {
    let band = Rect::new(0.0, 600.0 - STRIP, 800.0, STRIP);
    h.frame()
        .shapes
        .iter()
        .any(|s| matches!(s, Shape::Fill { rect, .. } if *rect == band))
}

#[test]
fn filmstrip_toggles_saves_and_lays_out_the_image_above_it() {
    let mut h = folder_of(5);
    assert!(!h.app.settings.filmstrip, "off by default");
    assert!(!strip_band(&h));
    assert_eq!(h.layout().unwrap().client, (800.0, 600.0));
    h.key(Letter('T'));
    assert!(h.menu_state().filmstrip);
    assert!(h.saved_settings().unwrap().filmstrip, "saved");
    assert!(strip_band(&h));
    assert_eq!(h.layout().unwrap().client, (800.0, 600.0 - STRIP));
    h.command(Cmd::ToggleFilmstrip);
    assert!(!h.saved_settings().unwrap().filmstrip);
    assert_eq!(h.layout().unwrap().client, (800.0, 600.0));
    // "Fit window to image": the window grows by the strip.
    h.key(Digit(1));
    let a = h.key(Letter('T'));
    let (w, ht) = crate::testkit::IMAGE_SIZE;
    assert!(
        a.contains(&Action::FitWindow {
            client: (w, ht + STRIP as u32),
            grow_only: false
        }),
        "{a:?}"
    );
}

#[test]
fn filmstrip_centres_the_current_tile_and_opens_clicked_ones() {
    let mut h = folder_of(5);
    h.key(Letter('T'));
    h.open("02.png");
    h.wait_for(|h| h.frame().thumbs.len() == 5);
    let top = 600.0 - STRIP;
    let thumbs = h.frame().thumbs.clone();
    assert!(thumbs.iter().all(|r| r.y >= top), "{thumbs:?}");
    let centred = |r: &Rect| (r.x + r.w / 2.0 - 400.0).abs() < 1.0;
    assert_eq!(thumbs.iter().filter(|r| centred(r)).count(), 1);
    assert!(
        h.frame().shapes.iter().any(|s| matches!(s,
            Shape::Outline { rect, .. } if centred(rect) && rect.y >= top - 6.0)),
        "the current tile is outlined"
    );
    // A click on the next tile opens it; on the current one nothing.
    let y = top + 6.0 + 36.0;
    h.click((400.0 + 78.0, y));
    assert_eq!(h.current_name().as_deref(), Some("03.png"));
    assert_eq!(h.click((400.0, y)), []);
    assert_eq!(h.current_name().as_deref(), Some("03.png"));
    assert_eq!(
        h.double_click((400.0 - 78.0, y)),
        Some(vec![]),
        "no full screen"
    );
    assert_eq!(h.current_name().as_deref(), Some("03.png"));
    // The wheel over the strip pages.
    h.wheel((400.0, y), 120);
    assert_eq!(h.current_name().as_deref(), Some("02.png"));
    h.wheel((400.0, y), -120);
    assert_eq!(h.current_name().as_deref(), Some("03.png"));
    // The grid hides the strip; T there only toggles the setting.
    h.key(Letter('G'));
    assert!(!strip_band(&h));
    h.key(Letter('T'));
    assert!(!h.saved_settings().unwrap().filmstrip);
    h.key(Letter('T'));
    assert!(!strip_band(&h));
    h.key(Letter('G'));
    assert!(strip_band(&h), "back once the grid closes");
}
