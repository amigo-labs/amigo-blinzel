//! App-level tests of the info overlay (`docs/specs/info-overlay.md`).

use super::*;
use crate::testkit::{Harness, jpeg};
use Key::*;
use blinzel_core::testimg::{ExifValue as V, exif_block, jpeg_with_exif};

fn ascii(s: &str) -> V {
    V::Ascii(s.as_bytes().to_vec())
}

/// A 64 × 48 JPEG whose EXIF says it was taken with a Canon EOS R6.
fn canon_jpeg() -> Vec<u8> {
    let tiff = exif_block(
        &[(0x010F, ascii("Canon")), (0x0110, ascii("Canon EOS R6"))],
        &[
            (0x9003, ascii("2026:11:20 14:32:09")),
            (0x829A, V::Rational(10, 2500)),
            (0x829D, V::Rational(28, 10)),
            (0x8827, V::Short(400)),
            (0x920A, V::Rational(850, 10)),
            (0xA434, ascii("RF24-105mm F4 L IS USM")),
        ],
    );
    jpeg_with_exif(&jpeg(64, 48), &tiff)
}

/// The file size line the overlay shows for `name` in English.
fn kb(h: &Harness, name: &str) -> String {
    let len = std::fs::metadata(h.images.join(name)).unwrap().len();
    assert!((1000..1_000_000).contains(&len), "test files are KB sized");
    let tenths = (len as f64 / 100.0).round() as u64;
    format!("{}.{} KB", tenths / 10, tenths % 10)
}

/// Turns the overlay on and waits until it shows `lines` lines.
fn show_info(h: &mut Harness, lines: usize) {
    h.key(Letter('I'));
    h.wait_for(|h| h.info_lines().len() == lines);
}

/// AC-01, AC-11
#[test]
fn i_toggles_the_overlay_the_menu_follows_and_the_choice_is_kept() {
    let mut h = Harness::with_images(&["a.png"]);
    h.pump();
    assert!(!h.menu_checked(Cmd::ToggleInfo), "off by default");
    assert!(h.info_lines().is_empty());
    h.key(Letter('I'));
    h.pump();
    assert!(h.menu_checked(Cmd::ToggleInfo));
    assert!(!h.info_lines().is_empty());
    assert_eq!(h.saved_settings().map(|s| s.info), Some(true));
    assert!(h.saved_text().unwrap().contains("info = true"));
    // Shift+I toggles too, Ctrl+I does not.
    h.press(Letter('I'), true, false);
    assert!(h.menu_checked(Cmd::ToggleInfo));
    h.press(Letter('I'), false, true);
    h.pump();
    assert!(!h.menu_checked(Cmd::ToggleInfo));
    assert!(h.info_lines().is_empty());
    assert_eq!(h.saved_settings().map(|s| s.info), Some(false));
    // The menu's entry does the same, and a restart keeps the choice.
    h.command(Cmd::ToggleInfo);
    h.restart();
    h.open("a.png");
    h.pump();
    assert!(h.menu_checked(Cmd::ToggleInfo));
}

/// AC-01: the entry sits with "Filmstrip" and "Grid".
#[test]
fn show_info_is_in_the_view_menu_next_to_filmstrip_and_grid() {
    let h = Harness::with_images(&["a.png"]);
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
    let at = |c| cmds.iter().position(|&x| x == c).unwrap();
    assert_eq!(at(Cmd::ToggleInfo) + 1, at(Cmd::ToggleFilmstrip));
    assert_eq!(at(Cmd::ToggleFilmstrip) + 1, at(Cmd::ToggleGrid));
}

/// AC-02, AC-04, AC-05
#[test]
fn the_overlay_shows_every_fact_in_order() {
    let mut h = Harness::new();
    h.write_file("a.jpg", &canon_jpeg());
    h.write_png("b.png", 64, 48);
    h.open("a.jpg");
    show_info(&mut h, 8);
    assert_eq!(
        h.info_lines(),
        [
            "a.jpg".to_string(),
            "1 / 2".into(),
            "64 × 48 · JPEG".into(),
            kb(&h, "a.jpg"),
            "2026-11-20 14:32".into(),
            "Canon EOS R6".into(),
            "RF24-105mm F4 L IS USM".into(),
            "f/2.8 · 1/250 s · ISO 400 · 85 mm".into(),
        ]
    );
}

/// AC-03
#[test]
fn an_image_without_exif_shows_name_position_size_and_file_size() {
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    h.key(Right);
    show_info(&mut h, 4);
    assert_eq!(
        h.info_lines(),
        ["b.png", "2 / 2", "64 × 48 · PNG", &kb(&h, "b.png")]
    );
}

/// AC-02: the size is the oriented one.
#[test]
fn a_portrait_shot_shows_its_size_as_displayed() {
    let mut h = Harness::new();
    let tiff = exif_block(&[(0x0112, V::Short(6))], &[]);
    h.write_file("p.jpg", &jpeg_with_exif(&jpeg(64, 48), &tiff));
    h.open("p.jpg");
    show_info(&mut h, 4);
    assert_eq!(h.info_lines()[2], "48 × 64 · JPEG");
}

/// AC-07
#[test]
fn dates_and_numbers_follow_the_ui_language() {
    let mut h = Harness::new();
    h.write_file("a.jpg", &canon_jpeg());
    h.open("a.jpg");
    h.command(Cmd::Language(Some(
        blinzel_i18n::lang_from_code("de").unwrap(),
    )));
    show_info(&mut h, 8);
    let lines = h.info_lines();
    assert_eq!(lines[3], kb(&h, "a.jpg").replace('.', ","));
    assert_eq!(lines[4], "20.11.2026 14:32");
    assert_eq!(lines[7], "f/2,8 · 1/250 s · ISO 400 · 85 mm");
}

/// AC-08, AC-12
#[test]
fn going_on_never_shows_the_previous_images_facts_and_never_waits_for_them() {
    let mut h = Harness::new();
    h.write_file("a.jpg", &canon_jpeg());
    h.write_png("b.png", 64, 48);
    h.open("a.jpg");
    show_info(&mut h, 8);

    let services = h.app.services.clone();
    let held = services.info_gate.lock().unwrap();
    h.key(Right);
    h.pump();
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
    assert_eq!(h.frame().images.len(), 1, "the image is drawn meanwhile");
    assert_eq!(h.info_lines(), ["b.png", "2 / 2", &kb(&h, "b.png")]);
    // Keys go on while the facts are read.
    h.key(Left);
    h.pump();
    assert_eq!(h.current_name().as_deref(), Some("a.jpg"));
    assert_eq!(h.info_lines()[0], "a.jpg");
    assert!(!h.info_lines().iter().any(|l| l.contains("PNG")));
    drop(held);
    h.wait_for(|h| h.info_lines().len() == 8);
    assert_eq!(h.info_lines()[5], "Canon EOS R6");
}

/// AC-12: the first frame does not wait for the facts.
#[test]
fn the_first_frame_does_not_wait_for_the_facts() {
    let mut h = Harness::with_settings(Settings {
        info: true,
        ..Settings::default()
    });
    h.write_file("a.jpg", &canon_jpeg());
    let services = h.app.services.clone();
    let held = services.info_gate.lock().unwrap();
    h.open("a.jpg");
    h.pump();
    assert_eq!(h.frame().images.len(), 1);
    assert!(h.host().first_frame_shown);
    assert_eq!(h.info_lines()[0], "a.jpg");
    drop(held);
    h.wait_for(|h| h.info_lines().len() == 8);
}

/// AC-09
#[test]
fn in_the_grid_the_overlay_follows_the_selected_tile() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_file("b.jpg", &canon_jpeg());
    h.open("a.png");
    h.pump();
    h.key(Letter('G'));
    show_info(&mut h, 4);
    assert_eq!(h.info_lines()[0], "a.png");
    h.key(Right);
    h.wait_for(|h| h.info_lines().len() == 8);
    assert_eq!(h.info_lines()[0], "b.jpg");
    assert_eq!(h.info_lines()[1], "2 / 2");
    assert_eq!(h.info_lines()[5], "Canon EOS R6");
}

/// AC-10
#[test]
fn the_box_sits_bottom_left_above_the_filmstrip_and_shortens_long_names() {
    let long = format!("{}.png", "x".repeat(200));
    let mut h = Harness::with_images(&[&long]);
    show_info(&mut h, 4);
    let r = h.app.info_rect.unwrap();
    let (cw, ch) = (h.app.client.0 as f64, h.app.client.1 as f64);
    assert!(r.x < 40.0 && r.y + r.h > ch - 40.0, "bottom left: {r:?}");
    assert!(
        r.w <= cw * 0.4 + 40.0,
        "at most 40 % wide plus padding: {r:?}"
    );
    let name = &h.info_lines()[0];
    assert!(name.contains('…') && name.ends_with("x.png"), "{name}");
    assert!(name.starts_with("xxx"));

    h.key(Letter('T'));
    h.wait_for(|h| h.app.info_rect.is_some_and(|r| r.y + r.h < ch - 60.0));
    let r = h.app.info_rect.unwrap();
    assert!(
        r.y + r.h <= ch - crate::browse::strip_height(h.app.scale()),
        "above the strip"
    );
}

/// AC-10: a notice is drawn over the box.
#[test]
fn a_notice_is_drawn_after_the_box() {
    let mut h = Harness::with_images(&["a.png"]);
    show_info(&mut h, 4);
    h.app.show_notice("Hello".into());
    h.render();
    let texts = &h.frame().texts;
    let name = texts.iter().position(|t| t == "a.png").unwrap();
    let notice = texts.iter().position(|t| t == "Hello").unwrap();
    assert!(notice > name);
}

/// AC-13
#[test]
fn a_broken_exif_block_leaves_out_its_facts_and_the_image_shows() {
    let mut tiff = exif_block(&[(0x0110, ascii("Cam"))], &[(0x8827, V::Short(200))]);
    // The Exif IFD pointer far outside the block.
    let n = u16::from_le_bytes([tiff[8], tiff[9]]) as usize;
    for i in 0..n {
        let e = 10 + 12 * i;
        if u16::from_le_bytes([tiff[e], tiff[e + 1]]) == 0x8769 {
            tiff[e + 8..e + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        }
    }
    let mut h = Harness::new();
    h.write_file("a.jpg", &jpeg_with_exif(&jpeg(64, 48), &tiff));
    h.open("a.jpg");
    show_info(&mut h, 5);
    assert_eq!(h.frame().images.len(), 1);
    assert_eq!(h.info_lines()[4], "Cam");
}

/// AC-14
#[test]
fn the_position_follows_the_folder_list() {
    let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
    h.key(Right);
    show_info(&mut h, 4);
    assert_eq!(h.info_lines()[1], "2 / 3");
    std::fs::remove_file(h.images.join("a.png")).unwrap();
    h.app.on_dir_changed();
    h.wait_for(|h| h.info_lines().get(1).map(String::as_str) == Some("1 / 2"));
    assert_eq!(h.info_lines()[0], "b.png");
}

/// The error image shows no guessed facts.
#[test]
fn a_broken_image_shows_name_position_and_file_size_only() {
    let mut h = Harness::new();
    h.write_file("bad.jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 1, 2, 3, 4]);
    h.open("bad.jpg");
    h.key(Letter('I'));
    h.settle();
    h.render();
    assert_eq!(h.info_lines(), ["bad.jpg", "1 / 1", "10 B"]);
}

/// Edge case "2 GB TIFF": a big file is read only up to its start.
#[test]
fn a_big_file_is_read_only_at_its_start() {
    let mut h = Harness::new();
    let mut big = canon_jpeg();
    big.resize(crate::info::HEAD as usize * 2, 0);
    h.write_file("big.jpg", &big);
    h.open("big.jpg");
    show_info(&mut h, 8);
    assert_eq!(h.info_lines()[5], "Canon EOS R6");
    let whole = h
        .app
        .services
        .reads
        .lock()
        .unwrap()
        .iter()
        .any(|(t, p)| t == crate::info::THREAD && p.ends_with("big.jpg"));
    assert!(!whole, "the reader did not read the whole file");
}

/// AC-09: a tile whose file cannot be read, and one whose size only a
/// decoder could tell.
#[test]
fn in_the_grid_an_unreadable_tile_shows_what_is_known() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_file("b.jpg", &canon_jpeg());
    // A JPEG header without a frame: the format, no size.
    h.write_file("c.jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0]);
    h.open("a.png");
    h.pump();
    let b_size = kb(&h, "b.jpg");
    std::fs::remove_file(h.images.join("b.jpg")).unwrap();
    h.key(Letter('G'));
    show_info(&mut h, 4);
    h.key(Right);
    h.wait_for(|h| h.info_lines().first().map(String::as_str) == Some("b.jpg"));
    h.wait_for(|h| h.info_lines().len() == 3);
    assert_eq!(h.info_lines(), ["b.jpg", "2 / 3", &b_size]);
    h.key(Right);
    h.wait_for(|h| h.info_lines().len() == 4);
    assert_eq!(h.info_lines(), ["c.jpg", "3 / 3", "JPEG", "8 B"]);
}

/// AC-14: the shown file replaced (read again) or gone (no position).
#[test]
fn a_replaced_file_is_read_again_and_a_gone_one_loses_its_position() {
    let mut h = Harness::new();
    h.write_file("a.jpg", &canon_jpeg());
    h.write_png("b.png", 64, 48);
    h.open("a.jpg");
    show_info(&mut h, 8);
    let tiff = exif_block(&[(0x0110, ascii("Other Cam"))], &[]);
    let mut other = jpeg_with_exif(&jpeg(64, 48), &tiff);
    other.extend([0; 7]);
    h.write_file("a.jpg", &other);
    h.app.on_dir_changed();
    h.wait_for(|h| h.info_lines().get(4).map(String::as_str) == Some("Other Cam"));
    std::fs::remove_file(h.images.join("a.jpg")).unwrap();
    h.app.on_dir_changed();
    h.wait_for(|h| h.names().len() == 1);
    h.render();
    let lines = h.info_lines();
    assert_eq!(lines[0], "a.jpg");
    assert!(!lines.iter().any(|l| l.contains(" / ")), "{lines:?}");
}
