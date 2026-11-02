//! App-level tests of viewing one image: view modes, zoom, pan, rotation,
//! full screen, the Esc chain, messages, opening, deleting, the menu and
//! the editor (`docs/specs/viewing.md`).

use super::*;
use crate::testkit::{Harness, IMAGE_SIZE};
use Key::*;
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
