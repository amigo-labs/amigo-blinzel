//! App-level tests of viewing one image: view modes, zoom, pan, rotation,
//! full screen, the Esc chain, messages, opening, deleting, the menu and
//! the editor (`docs/specs/viewing.md`).

use super::*;
use crate::testkit::{Harness, IMAGE_SIZE};
use Key::*;
use blinzel_core::decode::DecodeError;
use blinzel_core::layout::ViewOptions;

const MODES: [ViewMode; 3] = [
    ViewMode::FitWindowToImage,
    ViewMode::FitImageToWindow,
    ViewMode::ActualSize,
];

/// A 1600 × 1200 image open in the 800 × 600 window: it fits at 50 %.
fn big() -> Harness {
    let mut h = Harness::new();
    h.write_png("big.png", 1600, 1200);
    h.open("big.png");
    h
}

fn scale(h: &Harness) -> f64 {
    h.layout().unwrap().scale
}

fn origin(h: &Harness) -> (f64, f64) {
    h.layout().unwrap().origin
}

/// Client pixels between `a` and `b` on the longer axis.
fn off(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

/// Drags a selection from image point `a` to `b` (the image must fit).
fn select(h: &mut Harness, a: (f64, f64), b: (f64, f64)) -> Rect {
    let (from, to) = (h.at(a), h.at(b));
    h.drag(from, to);
    h.app.selection().unwrap()
}

/// Selects part of the image and cuts it (Ctrl+X): the view crop.
fn cut(h: &mut Harness) -> PixelRect {
    select(h, (4.0, 4.0), (40.0, 30.0));
    h.ctrl(Letter('X'));
    h.view().crop.expect("cut")
}

/// User zoom and pan on whatever is shown.
fn zoom_and_pan(h: &mut Harness) {
    h.wheel((400.0, 300.0), 240);
    h.middle_drag((400.0, 300.0), (380.0, 290.0));
    assert!(h.view().zoom.is_some() && h.view().center.is_some());
}

// ------------------------------------------------------------ view modes

/// AC-02.
#[test]
fn keys_and_the_menu_switch_the_mode_save_it_and_drop_zoom_and_pan() {
    let default = ViewOptions {
        mode: ViewMode::FitImageToWindow,
        no_upscale: true,
    };
    assert_eq!(Settings::parse("").view, default, "without a saved choice");
    let mut h = Harness::with_images(&["a.png"]);
    assert_eq!(h.app.settings.view, default);
    assert!(h.menu_checked(Cmd::View(ViewMode::FitImageToWindow)));
    assert!(h.menu_checked(Cmd::ToggleNoUpscale));

    let keys = [
        (Digit(1), ViewMode::FitWindowToImage),
        (Digit(3), ViewMode::ActualSize),
        (Digit(2), ViewMode::FitImageToWindow),
    ];
    for (key, mode) in keys {
        zoom_and_pan(&mut h);
        h.key(key);
        assert_eq!(h.mode(), mode);
        assert_eq!(h.saved_settings().unwrap().view.mode, mode, "saved at once");
        assert_eq!(h.view(), ViewState::default(), "zoom and pan dropped");
        for m in MODES {
            assert_eq!(h.menu_checked(Cmd::View(m)), m == mode, "{m:?}");
        }
    }
    for mode in [
        ViewMode::ActualSize,
        ViewMode::FitWindowToImage,
        ViewMode::FitImageToWindow,
    ] {
        zoom_and_pan(&mut h);
        h.command(Cmd::View(mode));
        assert_eq!(h.mode(), mode);
        assert_eq!(h.saved_settings().unwrap().view.mode, mode);
        assert_eq!(h.view(), ViewState::default());
        for m in MODES {
            assert_eq!(h.menu_checked(Cmd::View(m)), m == mode, "{m:?}");
        }
    }

    // A view crop stays.
    let crop = cut(&mut h);
    zoom_and_pan(&mut h);
    h.key(Digit(3));
    assert_eq!(
        h.view(),
        ViewState {
            crop: Some(crop),
            ..Default::default()
        }
    );

    // Ctrl+1 … Ctrl+3 are not view modes.
    for d in [1, 2, 3] {
        assert!(h.press(Digit(d), true, false).is_none(), "Ctrl+{d}");
    }
    assert_eq!(h.mode(), ViewMode::ActualSize);
}

// ------------------------------------------------------------------ zoom

/// AC-06: the wheel.
#[test]
fn the_wheel_zooms_by_1_2_per_notch_around_the_cursor_within_the_limits() {
    let mut h = big();
    assert_eq!(scale(&h), 0.5);
    let cursor = (200.0, 150.0);
    let p = h.layout().unwrap().client_to_image(cursor);
    let mut expected = 0.5;
    for (delta, factor) in [
        (120, 1.2),
        (240, 1.44),
        (60, 1.2f64.sqrt()),
        (-120, 1.0 / 1.2),
    ] {
        h.wheel(cursor, delta);
        expected *= factor;
        assert!(
            (scale(&h) - expected).abs() < 1e-9,
            "{delta}: {}",
            scale(&h)
        );
        assert!(h.layout().unwrap().overflows);
        let q = h.at(p);
        assert!(off(q, cursor) <= 0.5, "{delta}: {q:?}");
    }
    h.wheel(cursor, 120 * 40);
    assert_eq!(scale(&h), 64.0, "6400 % at most");
    h.wheel(cursor, -120 * 60);
    assert_eq!(scale(&h), 0.01, "1 % at least");

    // Over the filmstrip and the grid the wheel does not zoom.
    drop(h);
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    h.key(Letter('T'));
    h.wheel((400.0, 595.0), -120);
    assert_eq!(h.view().zoom, None, "over the filmstrip");
    assert_eq!(h.current_name().as_deref(), Some("b.png"), "it browses");
    h.key(Letter('T'));
    h.key(Letter('G'));
    h.wheel((400.0, 300.0), 120);
    h.key(Letter('G'));
    assert_eq!(h.view().zoom, None, "over the grid");
}

/// AC-06: + and −.
#[test]
fn plus_and_minus_step_through_the_levels_around_the_window_centre() {
    let mut h = big();
    h.wheel((400.0, 300.0), -120 * 60);
    assert_eq!(scale(&h), 0.01);
    let steps = [
        0.05, 0.1, 0.125, 0.167, 0.25, 0.333, 0.5, 0.667, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0,
        12.0, 16.0, 24.0, 32.0, 64.0,
    ];
    for s in steps {
        h.key(Plus);
        assert_eq!(scale(&h), s);
    }
    h.key(Plus);
    assert_eq!(scale(&h), 64.0, "the highest step stays");
    for &s in steps.iter().rev().skip(1) {
        h.key(Minus);
        assert_eq!(scale(&h), s);
    }

    // The image point at the window centre stays there, wherever the
    // pointer is (within a pixel: the panned origin is fractional).
    h.key(Digit(0));
    h.key(Plus);
    assert_eq!(scale(&h), 0.667);
    h.key(Right);
    h.key(Down);
    h.mouse_move((10.0, 590.0));
    let centre = (400.0, 300.0);
    let p = h.layout().unwrap().client_to_image(centre);
    assert!(off(p, (800.0, 600.0)) > 50.0, "panned away from the middle");
    for (key, s) in [(Plus, 0.75), (Plus, 1.0), (Minus, 0.75)] {
        h.key(key);
        assert_eq!(scale(&h), s);
        assert!(off(h.at(p), centre) <= 1.0, "{key:?} {s}: {:?}", h.at(p));
    }
}

/// AC-06: 0.
#[test]
fn zero_returns_to_the_modes_scale_centred() {
    let mut h = big();
    h.key(Digit(3));
    h.key(Right);
    h.key(Down);
    h.wheel((100.0, 100.0), 120);
    h.key(Digit(0));
    assert_eq!(h.view(), ViewState::default());
    assert_eq!(scale(&h), 1.0);
    assert_eq!(origin(&h), (-400.0, -300.0), "centred");
    // Ctrl+0 too.
    h.key(Right);
    h.wheel((100.0, 100.0), -120);
    h.press(Digit(0), true, false).unwrap();
    assert_eq!((scale(&h), origin(&h)), (1.0, (-400.0, -300.0)));
    // In "fit image to window" the fitted scale.
    h.key(Digit(2));
    h.wheel((100.0, 100.0), 360);
    h.key(Right);
    h.key(Digit(0));
    assert_eq!((scale(&h), origin(&h)), (0.5, (0.0, 0.0)));
}

// ------------------------------------------------------------------- pan

/// AC-08.
#[test]
fn a_middle_drag_pans_whatever_the_size_and_arrows_pan_only_an_overflowing_image() {
    // An image that fits: the middle button pans (it never selects), the
    // arrows browse.
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    assert!(!h.layout().unwrap().overflows);
    h.middle_drag((390.0, 290.0), (420.0, 310.0));
    assert!(h.app.selection().is_none(), "a middle drag never selects");
    assert!(h.view().center.is_some(), "it panned");
    for (key, name) in [
        (Right, "b.png"),
        (Left, "a.png"),
        (Down, "b.png"),
        (Up, "a.png"),
    ] {
        h.key(key);
        assert_eq!(h.current_name().as_deref(), Some(name), "{key:?}");
    }

    // An image larger than the window, at 100 %.
    drop(h);
    let mut h = Harness::new();
    h.write_png("a.png", 40, 30);
    h.write_png("big.png", 1600, 1200);
    h.write_png("c.png", 40, 30);
    h.open("big.png");
    h.key(Digit(3));
    assert_eq!(origin(&h), (-400.0, -300.0));
    h.middle_drag((400.0, 300.0), (370.0, 280.0));
    assert_eq!(origin(&h), (-430.0, -320.0), "the image follows the mouse");
    h.key(Digit(0));
    // 10 % of the shorter side (600 px) per press.
    for (key, o) in [
        (Right, (-460.0, -300.0)),
        (Left, (-400.0, -300.0)),
        (Down, (-400.0, -360.0)),
        (Up, (-400.0, -300.0)),
    ] {
        h.key(key);
        assert_eq!(origin(&h), o, "{key:?}");
    }
    h.resize((1000, 500));
    h.key(Digit(0));
    let o = origin(&h);
    h.key(Right);
    assert_eq!(origin(&h), (o.0 - 50.0, o.1), "10 % of 500 px");
    h.press(Up, true, false).unwrap();
    assert_eq!(origin(&h), (o.0 - 50.0, o.1 + 50.0), "Ctrl+Up pans too");
    assert_eq!(h.current_name().as_deref(), Some("big.png"));
    // With Ctrl, Left and Right browse.
    h.press(Right, true, false).unwrap();
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
    h.open("big.png");
    assert!(h.layout().unwrap().overflows);
    h.press(Left, true, false).unwrap();
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
}

// -------------------------------------------------------------- rotation

/// AC-09.
#[test]
fn r_and_l_turn_the_view_and_drop_selection_zoom_pan_and_crop() {
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    let file = h.images.join("a.png");
    let bytes = std::fs::read(&file).unwrap();
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
    let (w, ht) = (IMAGE_SIZE.0 as f64, IMAGE_SIZE.1 as f64);
    let cw = Orientation::NORMAL.rotated_cw();
    let ccw = Orientation::NORMAL.rotated_ccw();
    assert_eq!(cw.quarter_turns, 1);
    assert_eq!(ccw.quarter_turns, 3);

    h.key(Letter('R'));
    assert_eq!(h.app.user_rotation(), cw);
    assert_eq!(h.layout().unwrap().content, (ht, w), "turned on its side");
    h.key(Letter('L'));
    assert_eq!(h.app.user_rotation(), Orientation::NORMAL);
    h.key(Letter('L'));
    assert_eq!(h.app.user_rotation(), ccw);
    h.press(Letter('R'), false, true).unwrap();
    assert_eq!(h.app.user_rotation(), ccw.rotated_ccw(), "Shift+R");
    h.command(Cmd::RotateRight);
    h.command(Cmd::RotateRight);
    assert_eq!(h.app.user_rotation(), Orientation::NORMAL);
    h.command(Cmd::RotateLeft);
    assert_eq!(h.app.user_rotation(), ccw);
    h.key(Letter('R'));

    // Selection, zoom and pan go.
    select(&mut h, (4.0, 4.0), (40.0, 30.0));
    zoom_and_pan(&mut h);
    h.key(Letter('R'));
    assert!(h.app.selection().is_none());
    assert_eq!(h.view(), ViewState::default());
    // So does a view crop: the whole turned image shows.
    h.key(Letter('L'));
    cut(&mut h);
    h.key(Letter('L'));
    assert_eq!(h.view(), ViewState::default());
    assert_eq!(h.layout().unwrap().content, (ht, w));

    // Ctrl+R is not a rotation.
    assert!(h.press(Letter('R'), true, false).is_none());
    assert_eq!(h.app.user_rotation(), ccw);

    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "the file is not written"
    );
    let now = std::fs::metadata(&file).unwrap().modified().unwrap();
    assert_eq!(now, modified);
    // The next image starts unrotated, and so does this one again.
    h.key(Right);
    assert_eq!(h.app.user_rotation(), Orientation::NORMAL);
    h.key(Letter('R'));
    h.key(Left);
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
    assert_eq!(h.app.user_rotation(), Orientation::NORMAL);
    assert_eq!(h.layout().unwrap().content, (w, ht));
}

// --------------------------------------------------------- window sizing

/// The `FitWindow` requests in `a`: client size and `grow_only`.
fn fits(a: &[Action]) -> Vec<((u32, u32), bool)> {
    a.iter()
        .filter_map(|a| match a {
            Action::FitWindow { client, grow_only } => Some((*client, *grow_only)),
            _ => None,
        })
        .collect()
}

/// A JPEG of `w` × `h` in the images' folder (it decodes at a reduced
/// size for the screen).
fn write_jpeg(h: &Harness, name: &str, w: u32, ht: u32) {
    let opts = blinzel_core::decode::jpeg::EncodeOptions {
        quality: 90,
        subsampling: 420,
        ..Default::default()
    };
    let rgb = blinzel_core::testimg::rgb(w, ht);
    h.write_file(
        name,
        &blinzel_core::decode::jpeg::encode_rgb(&rgb, w, ht, opts).unwrap(),
    );
}

/// Nothing open yet, view mode `m`.
fn in_mode(m: ViewMode) -> Harness {
    let mut s = Settings::default();
    s.view.mode = m;
    Harness::with_settings(s)
}

/// AC-05: "fit window to image".
#[test]
fn the_window_takes_each_images_size_once_per_image_and_rotation() {
    let mut h = in_mode(ViewMode::FitWindowToImage);
    h.write_png("a.png", 64, 48);
    // Larger than the 1920 × 1080 screen box: shown first from a smaller
    // stage, then from the full resolution once zoomed in.
    write_jpeg(&h, "b.jpg", 2400, 1800);
    h.write_png("c.png", 30, 90);
    // No resize before the decoded image brings the size.
    let a = h.app.open(h.images.join("a.png"));
    assert_eq!(fits(&a), []);
    assert_eq!(fits(&h.pump()), [((64, 48), false)]);

    // Once per image: a later, sharper result of the same image does not
    // resize again.
    h.clear_actions();
    let a = h.key(Right);
    assert_eq!(fits(&a), [((2400, 1800), false)]);
    let id = h.app.current.as_ref().unwrap().cache_id;
    assert!(!h.app.has_stage(id, Stage::Full));
    h.wheel((400.0, 300.0), 120 * 8);
    h.wait_for(|h| h.app.has_stage(id, Stage::Full));
    assert_eq!(fits(h.actions()), [((2400, 1800), false)], "only once");

    // Rotating: the turned size, again once per rotation.
    h.key(Digit(0));
    let a = h.key(Right);
    assert_eq!(fits(&a), [((30, 90), false)]);
    assert_eq!(fits(&h.key(Letter('R'))), [((90, 30), false)]);
    assert_eq!(fits(&h.key(Letter('L'))), [((30, 90), false)]);
    // Switching the view mode and the filmstrip refit it.
    assert_eq!(fits(&h.key(Digit(1))), [((30, 90), false)]);
    let strip = strip_height(1.0) as u32;
    assert_eq!(fits(&h.key(Letter('T'))), [((30, 90 + strip), false)]);
    assert_eq!(fits(&h.key(Letter('T'))), [((30, 90), false)]);

    // Cutting fits the window to the crop, also after switching the
    // mode; returning to the original fits the whole image again.
    h.open("a.png");
    let crop = cut(&mut h);
    let crop_size = ((crop.w, crop.h), false);
    assert_eq!(fits(&h.actions()[h.actions().len() - 1..]), [crop_size]);
    assert_eq!(fits(&h.key(Digit(1))), [crop_size]);
    assert_eq!(fits(&h.key(Escape)), [((64, 48), false)]);

    // Never while maximized or full screen.
    for (maximized, fullscreen) in [(true, false), (false, true)] {
        h.app.maximized = maximized;
        h.app.fullscreen = fullscreen;
        assert_eq!(fits(&h.key(Letter('R'))), [], "rotated");
        assert_eq!(fits(&h.key(Right)), [], "next image");
        assert_eq!(fits(&h.key(Digit(1))), [], "mode");
        h.key(Left);
    }
    h.app.maximized = false;
    h.app.fullscreen = false;
    // Never in "100 %".
    h.key(Digit(3));
    h.clear_actions();
    h.key(Letter('R'));
    h.key(Right);
    h.key(Letter('T'));
    h.open("b.jpg");
    assert_eq!(fits(h.actions()), [], "100 %");
}

/// AC-05: "fit image to window" only lets the window grow.
#[test]
fn in_fit_image_to_window_the_window_may_grow_for_each_image() {
    let mut h = in_mode(ViewMode::FitImageToWindow);
    h.write_png("a.png", 64, 48);
    h.write_png("b.png", 1600, 1200);
    h.open("a.png");
    assert_eq!(fits(h.actions()), [((64, 48), true)]);
    assert_eq!(fits(&h.key(Right)), [((1600, 1200), true)]);
    assert_eq!(fits(&h.key(Letter('R'))), [((1200, 1600), true)]);
    let strip = strip_height(1.0) as u32;
    assert_eq!(fits(&h.key(Letter('T'))), [((1200, 1600 + strip), true)]);
}

// ----------------------------------------------------------- full screen

fn centre(r: Rect) -> (f64, f64) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// AC-11: the keys.
#[test]
fn f11_and_enter_toggle_full_screen_which_hides_the_buttons() {
    let mut h = Harness::with_images(&["a.png"]);
    let buttons = h.app.chrome().shapes(None, None, false).len();
    assert!(buttons > 0);
    assert_eq!(h.frame().shapes.len(), buttons, "the buttons are drawn");
    let menu = centre(h.app.chrome().rect(Button::Menu));
    for key in [F11, Enter] {
        let a = h.key(key);
        assert_eq!(a, [Action::ToggleFullscreen], "{key:?}");
        h.shell(a);
        assert!(h.app.fullscreen);
        assert!(h.frame().shapes.is_empty(), "no buttons drawn");
        h.mouse_move(menu);
        assert_eq!(h.app.hover, None, "none hovered");
        assert_eq!(h.click(menu), [], "none clicked");
        let a = h.key(key);
        assert_eq!(a, [Action::ToggleFullscreen]);
        h.shell(a);
        assert!(!h.app.fullscreen);
        assert_eq!(h.frame().shapes.len(), buttons);
        assert!(matches!(h.click(menu)[..], [Action::ShowMenu { .. }]));
        h.app.popup = None;
    }

    // Not in the grid, the menu or a dialog.
    h.key(Letter('G'));
    let a = h.key(Enter);
    assert!(!a.contains(&Action::ToggleFullscreen), "{a:?}");
    assert_eq!(h.grid_selected(), None, "Enter opened the tile");
    h.app.open_menu((300.0, 200.0));
    assert!(!h.key(F11).contains(&Action::ToggleFullscreen));
    assert!(h.app.popup.is_some());
    h.key(Escape);
    h.ctrl(Letter('B'));
    for key in [F11, Enter] {
        assert!(!h.key(key).contains(&Action::ToggleFullscreen), "{key:?}");
    }
    assert!(h.app.dialog.is_some());
}

/// AC-11: the double click.
#[test]
fn a_double_click_below_the_caption_strip_toggles_full_screen() {
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    let chrome = h.app.chrome();
    let caption = chrome.caption_height();
    let toggle = Some(vec![Action::ToggleFullscreen]);
    // On the image or beside it.
    assert_eq!(h.double_click((400.0, 300.0)), toggle);
    assert_eq!(h.double_click((20.0, caption + 1.0)), toggle);
    // The caption strip and its buttons are the window's (maximize).
    for b in [
        Button::Menu,
        Button::Minimize,
        Button::Maximize,
        Button::Close,
    ] {
        assert_eq!(h.double_click(centre(chrome.rect(b))), None, "{b:?}");
    }
    assert_eq!(
        h.double_click((400.0, caption / 2.0)),
        None,
        "caption strip"
    );
    // Full screen has no buttons and no caption strip.
    h.shell(vec![Action::ToggleFullscreen]);
    assert_eq!(h.double_click(centre(chrome.rect(Button::Close))), toggle);
    assert_eq!(h.double_click((400.0, caption / 2.0)), toggle);
    h.shell(vec![Action::ToggleFullscreen]);
    // Not on the filmstrip or the grid.
    h.key(Letter('T'));
    assert_eq!(h.double_click((400.0, 595.0)), Some(vec![]), "filmstrip");
    h.key(Letter('G'));
    let tile = centre(h.grid_cell(1).unwrap());
    let a = h.double_click(tile).unwrap();
    assert!(!a.contains(&Action::ToggleFullscreen), "{a:?}");
    assert_eq!(
        h.current_name().as_deref(),
        Some("b.png"),
        "it opened the tile"
    );
}

// --------------------------------------------------------------- Escape

/// AC-12.
#[test]
fn escape_ends_a_drag_clears_the_selection_shows_the_original_leaves_full_screen_and_closes() {
    let mut h = Harness::with_images(&["a.png"]);
    let crop = cut(&mut h);
    let a = h.key(F11);
    h.shell(a);
    assert!(h.app.fullscreen);
    // A selection drag in progress.
    let (from, to) = (h.at((8.0, 8.0)), h.at((24.0, 20.0)));
    h.mouse_down(from);
    h.mouse_move(((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0));
    h.mouse_move(to);
    assert!(h.app.is_dragging());
    let sel = h.app.selection().unwrap();
    assert_eq!(h.key(Escape), []);
    assert!(!h.app.is_dragging(), "the drag ended");
    h.mouse_move((to.0 + 10.0, to.1 + 10.0));
    h.mouse_up();
    assert_eq!(h.app.selection(), Some(sel), "the selection keeps its size");

    assert_eq!(h.key(Escape), []);
    assert_eq!(h.app.selection(), None, "the selection");
    assert_eq!(h.view().crop, Some(crop));
    assert_eq!(fits(&h.key(Escape)), []);
    assert_eq!(h.view().crop, None, "the crop");
    assert!(h.app.fullscreen);
    let a = h.key(Escape);
    assert_eq!(a, [Action::ToggleFullscreen], "full screen");
    h.shell(a);
    assert_eq!(h.key(Escape), [Action::Close], "the window");

    // The grid, the menu and the dialog take it first.
    h.key(Letter('G'));
    assert_eq!(h.key(Escape), []);
    assert_eq!(h.grid_selected(), None);
    h.app.open_menu((300.0, 200.0));
    assert_eq!(h.key(Escape), []);
    assert!(h.app.popup.is_none());
    h.ctrl(Letter('B'));
    assert_eq!(h.key(Escape), []);
    assert!(h.app.dialog.is_none());
    assert_eq!(h.key(Escape), [Action::Close]);
}

// -------------------------------------------------------------- messages

/// `text` alone in the frame, centred (7 px per character, 16 px high).
fn assert_centred_message(h: &Harness, text: &str) {
    assert_eq!(h.frame().texts, [text]);
    let w = 7.0 * text.chars().count() as f64;
    let at = (
        ((800.0 - w) / 2.0).round() as f32,
        ((600.0f64 - 16.0) / 2.0).round() as f32,
    );
    assert_eq!(h.frame().text_positions, [at]);
}

/// AC-13: nothing open.
#[test]
fn without_an_image_the_window_shows_the_hint_or_why_the_open_failed() {
    let mut h = Harness::new();
    h.render();
    let hint = "Drop an image here or press Ctrl+O";
    assert_centred_message(&h, hint);
    h.command(Cmd::Language(blinzel_i18n::lang_from_code("de")));
    let german = tr(Msg::HintOpen);
    assert_ne!(german, hint);
    assert_centred_message(&h, german);
    h.command(Cmd::Language(Some(blinzel_i18n::FALLBACK)));

    let missing = h.images.join("missing.png");
    let reason = std::fs::metadata(&missing).unwrap_err().to_string();
    let a = h.app.open(missing);
    assert_eq!(a, []);
    h.pump();
    assert_centred_message(&h, &format!("Could not open “missing.png”: {reason}"));
}

/// AC-13: an image that cannot be shown, and one still loading.
#[test]
fn a_broken_image_shows_why_instead_of_the_previous_one() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_file("b.png", b"not an image");
    h.write_file("c.png", &crate::testkit::png(64, 48)[..60]);
    h.open("a.png");
    assert_eq!(h.frame().images.len(), 1);
    // No decoder for it.
    h.key(Right);
    assert!(h.frame().images.is_empty(), "not the previous image");
    assert_centred_message(
        &h,
        "Could not open “b.png”: unsupported or damaged image format",
    );
    // The decoder's own message.
    h.key(Right);
    let id = h.app.current.as_ref().unwrap().cache_id;
    let e = h.app.error_of(id).unwrap().to_string();
    assert!(e.starts_with("corrupt image"), "{e}");
    assert!(h.frame().images.is_empty());
    assert_centred_message(&h, &format!("Could not open “c.png”: {e}"));
    // Back to one that works: no message.
    h.key(Home);
    assert_eq!(h.frame().images.len(), 1);
    assert!(h.frame().texts.is_empty());

    // An error from WIC.
    drop(h);
    let mut h = Harness::new();
    h.write_png("d.png", 64, 48);
    h.set_decode_error(Some(DecodeError::Corrupt(
        "WIC: The component cannot be found.".into(),
    )));
    h.open("d.png");
    assert_centred_message(
        &h,
        "Could not open “d.png”: unsupported or damaged image format",
    );

    // Still loading: no message.
    h.set_decode_error(None);
    h.write_png("e.png", 64, 48);
    let _ = h.app.open(h.images.join("e.png"));
    h.render();
    assert!(h.frame().texts.is_empty(), "{:?}", h.frame().texts);
    h.pump();
    assert_eq!(h.frame().images.len(), 1);
}

// ------------------------------------------------------ opening, deleting

/// AC-14.
#[test]
fn ctrl_o_and_open_ask_for_the_dialog_and_a_drop_opens_its_first_file() {
    let mut h = Harness::new();
    assert_eq!(
        h.ctrl(Letter('O')),
        [Action::OpenDialog],
        "at an empty start"
    );
    assert_eq!(h.command(Cmd::Open), [Action::OpenDialog]);
    std::fs::create_dir_all(h.images.join("trip")).unwrap();
    h.write_png("a.png", 64, 48);
    h.write_png("trip/b.png", 64, 48);
    h.write_png("trip/c.png", 64, 48);
    let at = |h: &Harness, f: &str| h.images.join(f);
    let drop = vec![at(&h, "trip/c.png"), at(&h, "a.png"), at(&h, "trip/b.png")];
    let a = h.app.files_dropped(drop);
    h.shell(a);
    assert_eq!(h.current_path(), Some(at(&h, "trip/c.png")), "the first");
    assert_eq!(h.names(), ["b.png", "c.png"], "its folder");
    assert_eq!(
        h.ctrl(Letter('O')),
        [Action::OpenDialog],
        "with an image open"
    );
    // With the batch dialog open they go into its list instead.
    h.ctrl(Letter('B'));
    let a = h.app.files_dropped(vec![at(&h, "a.png")]);
    h.shell(a);
    assert_eq!(h.current_path(), Some(at(&h, "trip/c.png")));
    assert_eq!(h.batch_files(), Some(vec![at(&h, "a.png")]));
}

/// AC-15 and the edge case of deleting the folder's last file.
#[test]
fn delete_moves_the_file_away_and_shows_the_one_that_took_its_place() {
    let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
    h.open("b.png");
    let b = h.images.join("b.png");
    let a = h.key(Delete);
    assert_eq!(a, [Action::Recycle(b.clone())]);
    // Until the platform reports the move the image stays: a failed move
    // ends here.
    assert!(b.exists());
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
    assert_eq!(h.frame().images.len(), 1);
    h.shell(a);
    assert!(!b.exists());
    assert_eq!(
        h.current_name().as_deref(),
        Some("c.png"),
        "it took b's place"
    );
    assert_eq!(h.names(), ["a.png", "c.png"]);
    // The menu does the same; the last file gives way to the one before.
    let a = h.command(Cmd::Recycle);
    assert_eq!(a, [Action::Recycle(h.images.join("c.png"))]);
    h.shell(a);
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
    // The folder's last file: the empty hint and the app's name.
    let a = h.key(Delete);
    h.shell(a);
    assert_eq!(h.current_name(), None);
    assert_eq!(h.host().title, tr(Msg::AppName));
    assert!(h.frame().images.is_empty());
    assert_eq!(h.frame().texts, [tr(Msg::HintOpen)]);
    assert_eq!(h.key(Delete), [], "nothing to delete");
}

// ------------------------------------------------------------------ menu

/// AC-18.
#[test]
fn the_menu_opens_below_its_button_and_at_a_right_click() {
    let mut h = Harness::with_images(&["a.png"]);
    let r = h.app.chrome().rect(Button::Menu);
    let below = Action::ShowMenu {
        x: r.x as i32,
        y: r.bottom() as i32,
    };
    assert_eq!(h.click(centre(r)), std::slice::from_ref(&below), "☰");
    assert_eq!(h.key(F10), std::slice::from_ref(&below), "F10");
    assert_eq!(h.key(Menu), [below], "the context-menu key");
    // Right to left: from the button's right corner.
    assert_eq!(menu_corner(r, false), (r.x, r.bottom()));
    assert_eq!(menu_corner(r, true), (r.right(), r.bottom()));
    // A right click: at the pointer, where the drawn menu opens.
    let a = h.app.right_click((300.0, 200.0));
    assert_eq!(a, [Action::ShowMenu { x: 300, y: 200 }]);
    h.shell(a);
    assert!(h.app.popup.is_some());
}

/// AC-19.
#[test]
fn the_menu_enables_and_checks_its_entries_by_state() {
    let image = [
        Cmd::ZoomIn,
        Cmd::ZoomOut,
        Cmd::ZoomReset,
        Cmd::RotateRight,
        Cmd::RotateLeft,
        Cmd::Copy,
        Cmd::SelectAll,
        Cmd::OpenWith,
        Cmd::EditWith,
        Cmd::Recycle,
    ];
    let browse = [Cmd::Next, Cmd::Prev, Cmd::First, Cmd::Last];
    let cut_cmds = [Cmd::Cut, Cmd::ShowOriginal, Cmd::SaveCutout];
    let on = |h: &Harness, cmds: &[Cmd]| -> Vec<bool> {
        cmds.iter().map(|&c| h.menu_enabled(c)).collect()
    };
    let mut h = Harness::new();
    assert_eq!(on(&h, &image), [false; 10], "no image");
    assert_eq!(on(&h, &cut_cmds), [false; 3]);
    assert_eq!(on(&h, &browse), [false; 4]);
    h.write_png("a.png", 64, 48);
    h.open("a.png");
    assert_eq!(on(&h, &image), [true; 10], "an image");
    assert_eq!(on(&h, &browse), [false; 4], "the folder's only file");
    assert_eq!(on(&h, &cut_cmds), [false; 3]);
    select(&mut h, (4.0, 4.0), (40.0, 30.0));
    assert_eq!(on(&h, &cut_cmds), [true, false, true], "a selection");
    h.ctrl(Letter('X'));
    assert_eq!(on(&h, &cut_cmds), [false, true, true], "a crop");
    h.key(Escape);
    assert_eq!(on(&h, &cut_cmds), [false; 3]);
    h.write_png("b.png", 64, 48);
    h.app.on_dir_changed();
    h.pump();
    assert_eq!(h.names(), ["a.png", "b.png"]);
    assert_eq!(on(&h, &browse), [true; 4], "two files");

    for m in MODES {
        h.command(Cmd::View(m));
        let fit = m == ViewMode::FitImageToWindow;
        assert_eq!(h.menu_enabled(Cmd::ToggleNoUpscale), fit, "{m:?}");
        for n in MODES {
            assert_eq!(h.menu_checked(Cmd::View(n)), n == m, "{m:?} {n:?}");
        }
    }
    assert!(!h.menu_checked(Cmd::Fullscreen));
    let a = h.key(F11);
    h.shell(a);
    assert!(h.menu_checked(Cmd::Fullscreen));
}

// ---------------------------------------------------------------- editor

/// AC-21 and the edge case of an editor path with a quote or a line break.
#[test]
fn e_opens_the_image_in_the_editor_asking_for_one_the_first_time() {
    let mut h = Harness::new();
    assert_eq!(h.key(Letter('E')), [], "nothing open");
    h.write_png("a.png", 64, 48);
    h.open("a.png");
    let file = h.images.join("a.png");
    let ask = Action::ChooseEditor {
        then: Some(file.clone()),
    };
    assert_eq!(h.key(Letter('E')), std::slice::from_ref(&ask));
    assert_eq!(h.command(Cmd::EditWith), [ask]);
    for bad in ["/opt/my \"editor\"", "/opt/edit\nor", "/opt/edit\ror"] {
        let a = h.app.editor_chosen(PathBuf::from(bad), Some(file.clone()));
        assert_eq!(a, [], "{bad:?}: nothing launched");
        assert_eq!(h.app.settings.editor, None, "{bad:?}: not saved");
    }

    let editor = PathBuf::from("/usr/bin/gimp");
    let edit = Action::EditWith {
        editor: editor.clone(),
        file: file.clone(),
    };
    let a = h.app.editor_chosen(editor, Some(file.clone()));
    assert_eq!(a, std::slice::from_ref(&edit), "the image opens with it");
    let saved = h.saved_settings().unwrap().editor;
    assert_eq!(saved.as_deref(), Some("/usr/bin/gimp"), "remembered");
    assert_eq!(h.key(Letter('E')), std::slice::from_ref(&edit));
    assert_eq!(h.command(Cmd::EditWith), std::slice::from_ref(&edit));
    h.restart();
    h.open("a.png");
    assert_eq!(h.key(Letter('E')), [edit], "also in the next session");
    assert!(h.press(Letter('E'), true, false).is_none(), "Ctrl+E");
}

/// AC-22's request to the platform (its chooser is a manual check).
#[test]
fn open_with_hands_the_current_file_to_the_platform() {
    let mut h = Harness::new();
    assert_eq!(h.command(Cmd::OpenWith), [], "nothing open");
    h.write_png("a.png", 64, 48);
    h.open("a.png");
    let a = h.command(Cmd::OpenWith);
    assert_eq!(a, [Action::OpenWith(h.images.join("a.png"))]);
}
