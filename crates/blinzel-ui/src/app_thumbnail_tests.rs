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

// ------------------------------------------------------ thumbnail requests

/// Paths of list items `items`.
fn paths(h: &Harness, items: impl IntoIterator<Item = usize>) -> Vec<PathBuf> {
    items
        .into_iter()
        .map(|i| h.app.list().path(i).unwrap())
        .collect()
}

/// Waits until the pool has read `n` files and every tile on screen has
/// its thumbnail, then renders a few more frames (each asks again).
fn settle(h: &mut Harness, n: usize, drawn: usize) {
    h.wait_for(|h| h.thumb_reads().len() >= n && h.frame().thumbs.len() >= drawn);
    for _ in 0..3 {
        h.app.invalidate();
        h.pump();
    }
}

#[test]
fn the_grid_asks_for_the_visible_tiles_then_the_next_then_the_previous_screenful() {
    let mut h = folder_of(60);
    h.open("40.png");
    h.key(Letter('G'));
    let g = grid_layout(60, (800.0, 600.0), 1.0);
    let visible = g.visible(scroll(&h), 600.0);
    assert!(visible.start >= 16, "{visible:?}: scrolled down");
    let page = visible.len();
    let next = visible.end..(visible.end + page).min(60);
    let prev = (visible.start - page..visible.start).rev();
    let want = paths(&h, visible.clone().chain(next).chain(prev));
    settle(&mut h, want.len(), page);
    // One pool thread: requests are served in priority order, each once.
    assert_eq!(h.thumb_reads(), want);
}

#[test]
fn the_filmstrip_asks_nearest_first_then_alternating_further_out() {
    let mut h = folder_of(40);
    h.open("20.png");
    h.key(Letter('T'));
    // 11 tiles fit 800 px (72 + 6 each), the current one in the middle.
    let visible = [20, 19, 21, 18, 22, 17, 23, 16, 24, 15, 25];
    let further = (6..=15).flat_map(|k| [20 + k, 20 - k]);
    let want = paths(&h, visible.into_iter().chain(further));
    settle(&mut h, want.len(), visible.len());
    assert_eq!(h.frame().thumbs.len(), visible.len());
    assert_eq!(h.thumb_reads(), want);
}

#[test]
fn thumbnails_are_made_stored_and_taken_from_the_cache_next_session() {
    use blinzel_core::thumbcache::{Key, ThumbCache};
    let mut h = folder_of(3);
    h.key(Letter('G'));
    settle(&mut h, 3, 3);
    assert_eq!(h.thumb_reads(), paths(&h, 0..3), "made from the files");
    let dir = h.app.services.cache_dir().unwrap().join("thumbs");
    let key = |h: &Harness, i: usize| {
        let e = h.app.list().get(i).unwrap();
        Key::new(&h.app.list().path(i).unwrap(), e.size, e.mtime)
    };
    let cache = ThumbCache::open(&dir, 1 << 30).unwrap();
    for i in 0..3 {
        assert!(cache.get(&key(&h, i)).is_some(), "{i} stored");
    }
    // Next session: from the cache, no file read.
    h.restart();
    h.open("00.png");
    h.key(Letter('G'));
    h.wait_for(|h| h.frame().thumbs.len() == 3);
    assert_eq!(h.thumb_reads(), [] as [PathBuf; 0]);
    // A file changed since: its entry is stale, it is made again.
    h.write_png("01.png", 30, 20);
    h.restart();
    h.open("00.png");
    h.key(Letter('G'));
    settle(&mut h, 1, 3);
    assert_eq!(h.thumb_reads(), paths(&h, [1]));
}

/// The fill drawn exactly over grid tile `i`: its placeholder.
fn placeholder_at(h: &Harness, i: usize) -> bool {
    let cell = h.grid_cell(i).unwrap();
    h.frame()
        .shapes
        .iter()
        .any(|s| matches!(s, Shape::Fill { rect, .. } if *rect == cell))
}

#[test]
fn a_broken_file_keeps_its_placeholder_and_is_not_asked_for_again() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_file("b.png", b"not an image");
    h.write_png("c.png", 64, 48);
    h.open("a.png");
    h.key(Letter('G'));
    settle(&mut h, 3, 2);
    assert!(placeholder_at(&h, 1));
    assert!(!placeholder_at(&h, 0) && !placeholder_at(&h, 2));
    // Closed and opened again: still not asked for.
    h.key(Letter('G'));
    h.key(Letter('G'));
    settle(&mut h, 3, 2);
    assert_eq!(h.thumb_reads(), paths(&h, 0..3), "each read once");
    assert!(placeholder_at(&h, 1));
}

#[test]
fn cloud_placeholders_are_never_read_for_a_thumbnail() {
    use blinzel_core::filelist::attr;
    let mut h = Harness::new();
    let cloud = [
        ("b.png", attr::OFFLINE),
        ("c.png", attr::RECALL_ON_OPEN),
        ("d.png", attr::RECALL_ON_DATA_ACCESS),
    ];
    for name in ["a.png", "b.png", "c.png", "d.png", "e.png"] {
        h.write_png(name, 64, 48);
    }
    for (name, a) in cloud {
        h.set_attributes(name, a);
    }
    h.open("a.png");
    h.key(Letter('G'));
    settle(&mut h, 2, 2);
    assert_eq!(h.thumb_reads(), paths(&h, [0, 4]));
    assert!((1..4).all(|i| placeholder_at(&h, i)));
}

#[test]
fn an_evicted_thumbnail_is_asked_for_again_when_it_is_needed() {
    // 8 atlas pages of 64 slots: 600 thumbnails do not fit.
    let mut h = folder_of(600);
    h.key(Letter('G'));
    let id = h.app.list().get(0).unwrap().cache_id();
    h.wait_for(|h| h.app.browse.has_thumb(id));
    while h.grid_selected() != Some(599) {
        h.wait_for(|h| h.frame().thumbs.len() >= 16);
        h.key(PageDown);
    }
    h.wait_for(|h| h.thumb_reads().len() >= 600);
    // Tile 0 was drawn least recently: its slot went to another one.
    assert!(!h.app.browse.has_thumb(id), "evicted");
    h.key(Home);
    h.wait_for(|h| h.app.browse.has_thumb(id));
    h.wait_for(|h| h.frame().thumbs.len() >= 16);
    let cell = h.grid_cell(0).unwrap();
    assert!(
        h.frame()
            .thumbs
            .iter()
            .any(|r| cell.contains((r.x + 1.0, r.y + 1.0)))
    );
}

#[test]
fn transparent_thumbnails_are_composited_over_the_background_colour() {
    // AC-11: a fully transparent PNG over a white (not the default) background.
    let settings = Settings {
        background: [255, 255, 255],
        ..Settings::default()
    };
    let mut h = Harness::with_settings(settings.clone());
    let clear = vec![0u8; 64 * 48 * 4];
    let png = blinzel_core::export::encode_png_bgra(&clear, 64, 48).unwrap();
    h.write_file("a.png", &png);
    let white = |h: &Harness| {
        let planes = &h.app.canvas.thumb_planes;
        assert_eq!(planes.len(), 1);
        let p = &planes[0];
        assert!(p.y.iter().all(|&y| y == 255), "Y of white: {:?}", &p.y[..4]);
        assert!(p.cbcr.iter().all(|&c| c == 128), "no colour");
    };
    h.open("a.png");
    h.key(Letter('G'));
    settle(&mut h, 1, 1);
    white(&h);
    // Next session, the same background: the same from the cache.
    let config = h.settings_file();
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, settings.to_text()).unwrap();
    h.restart();
    h.open("a.png");
    h.key(Letter('G'));
    h.wait_for(|h| h.frame().thumbs.len() == 1);
    assert_eq!(h.thumb_reads(), [] as [PathBuf; 0]);
    white(&h);
}

/// The pack file's length after the viewer made one thumbnail into a
/// cache whose pack was `len` bytes long (a sparse file) before.
fn pack_after_one_thumbnail(len: u64) -> u64 {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    let dir = h.app.services.cache_dir().unwrap().join("thumbs");
    std::fs::create_dir_all(&dir).unwrap();
    let pack = dir.join("thumbs.pack");
    std::fs::File::create(&pack).unwrap().set_len(len).unwrap();
    h.open("a.png");
    h.key(Letter('G'));
    settle(&mut h, 1, 1);
    std::fs::metadata(&pack).unwrap().len()
}

#[test]
fn the_viewer_opens_its_thumbnail_cache_with_a_one_gib_limit() {
    // AC-08: 1 MiB below 1 GiB, a thumbnail is appended; 16 bytes below,
    // it starts a new generation.
    let gib = 1u64 << 30;
    let below = gib - (1 << 20);
    let appended = pack_after_one_thumbnail(below);
    assert!(appended > below && appended <= gib, "appended: {appended}");
    let renewed = pack_after_one_thumbnail(gib - 16);
    assert!(renewed < 1 << 20, "new generation: {renewed}");
}
