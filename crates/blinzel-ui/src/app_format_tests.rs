//! App-level tests of the decoder hand-off: embedded previews of camera
//! raws and JPEGs as stages, and the system decoder's part
//! (`docs/specs/image-formats.md`).

use super::*;
use crate::testkit::{Harness, jpeg, raf, tiff_raw};

/// The size every raw here develops to in the system decoder; its EXIF
/// (cropped-size record) says 6240 × 4160, close enough to be trusted.
const DEVELOPED: (u32, u32) = (6000, 4000);

/// A harness with a system decoder and `name` written as a RAF of 6240 ×
/// 4160 whose only embedded preview is a JPEG of `preview`.
fn with_raw(name: &str, preview: (u32, u32)) -> Harness {
    let h = Harness::new();
    h.set_system_decoder(Some(DEVELOPED));
    h.write_file(name, &raf(&jpeg(preview.0, preview.1), (6240, 4160)));
    h
}

fn current_id(h: &Harness) -> u64 {
    h.app.current.as_ref().unwrap().cache_id
}

/// The stored size the App knows for the current image.
fn probed_size(h: &Harness) -> (u32, u32) {
    let p = &h.app.probes[&current_id(h)];
    (p.width, p.height)
}

/// Zooms the current image to 100 % and loads what that needs, every job
/// to its end ([`Harness::settle`]).
fn actual_size(h: &mut Harness) {
    h.app.view.zoom = Some(1.0);
    h.app.after_zoom();
    h.settle();
}

/// AC-20: a raw preview that covers the screen box is the Screen stage, one
/// that covers the whole image also the Full stage.
#[test]
fn a_raw_preview_that_covers_the_stage_is_the_stage_and_the_system_decoder_is_not_asked() {
    // The screen box (1920 × 1080) needs 1620 × 1080 of 6240 × 4160; a
    // 1616 × 1080 preview is within 1 %.
    let mut h = with_raw("a.raf", (1616, 1080));
    // Settled: with a plain pump the App cancels the job once the stage
    // arrives, before it could wrongly go on to the system decoder.
    h.open_settled("a.raf");
    let id = current_id(&h);
    assert!(h.app.has_stage(id, Stage::Screen));
    assert!(!h.app.has_stage(id, Stage::Preview));
    assert_eq!(h.decodes(), [], "the system decoder is not asked");
    let drawn = &h.frame().images[0];
    assert_eq!((drawn.size, drawn.full_size), ((1616, 1080), (6240, 4160)));
    // 100 % needs the whole image, which only the system decoder has.
    actual_size(&mut h);
    assert!(h.app.has_stage(id, Stage::Full));
    assert_eq!(h.decodes(), [((6240, 4160), 2)]);
    drop(h);

    // A raw of 1500 × 1000 whose preview has every pixel: the preview is
    // the Full stage too.
    let mut h = Harness::new();
    h.set_system_decoder(Some(DEVELOPED));
    h.write_file("b.raf", &raf(&jpeg(1500, 1000), (1500, 1000)));
    h.open_settled("b.raf");
    let id = current_id(&h);
    actual_size(&mut h);
    assert!(h.app.has_stage(id, Stage::Screen));
    assert_eq!(h.frame().images[0].size, (1500, 1000));
    assert_eq!(h.decodes(), [], "the system decoder is not asked");
    assert_eq!(probed_size(&h), (1500, 1000));
}

/// AC-20: of a raw's several previews the largest is the one that must
/// cover the stage: a TIFF raw with a small thumbnail in IFD0 and a
/// covering preview in its sub-IFD.
#[test]
fn a_raw_with_several_previews_takes_its_largest_as_the_stage() {
    let mut h = Harness::new();
    h.set_system_decoder(Some(DEVELOPED));
    let data = tiff_raw(&jpeg(160, 120), &jpeg(1616, 1080), (6240, 4160));
    h.write_file("a.nef", &data);
    h.open_settled("a.nef");
    let id = current_id(&h);
    let sizes: Vec<_> = h.app.probes[&id]
        .previews
        .iter()
        .map(|p| (p.width, p.height))
        .collect();
    assert_eq!(sizes, [(1616, 1080), (160, 120)]);
    assert!(h.app.has_stage(id, Stage::Screen));
    assert!(!h.app.has_stage(id, Stage::Preview));
    assert_eq!(h.decodes(), [], "the system decoder is not asked");
    let drawn = &h.frame().images[0];
    assert_eq!((drawn.size, drawn.full_size), ((1616, 1080), (6240, 4160)));
}

/// AC-20: a raw preview that has every pixel of the image is the Full
/// stage at its own size, even where it is larger than the screen box.
#[test]
fn a_raw_preview_with_every_pixel_is_the_full_stage_at_its_full_size() {
    let mut h = Harness::new();
    h.set_system_decoder(Some(DEVELOPED));
    h.write_file("a.raf", &raf(&jpeg(4000, 2667), (4000, 2667)));
    h.open_settled("a.raf");
    let id = current_id(&h);
    assert_eq!(h.frame().images[0].size, (2000, 1334), "the Screen stage");
    actual_size(&mut h);
    assert!(h.app.has_stage(id, Stage::Full));
    assert_eq!(h.decodes(), [], "the system decoder is not asked");
    let drawn = &h.frame().images[0];
    assert_eq!((drawn.size, drawn.full_size), ((4000, 2667), (4000, 2667)));
}

/// AC-20: a preview just over 1 % short of the screen box is not the stage.
#[test]
fn a_raw_preview_just_over_one_percent_short_shows_first_and_the_system_decoder_is_asked() {
    // 1600 × 1066 against the 1620 × 1080 the screen box needs: 1.2 % and
    // 1.3 % short.
    // Then short in one side only: 1620 × 1060 and 1590 × 1080 (both
    // within 3 % of the EXIF aspect, so the image stays 6240 × 4160).
    for preview in [(1600, 1066), (1620, 1060), (1590, 1080)] {
        let mut h = with_raw("a.raf", preview);
        h.open("a.raf");
        let id = current_id(&h);
        assert!(h.app.has_stage(id, Stage::Preview), "{preview:?}");
        assert!(h.app.has_stage(id, Stage::Screen));
        assert_eq!(
            h.decodes(),
            [((6240, 4160), 2)],
            "{preview:?}: the system decoder is asked"
        );
    }
}

/// AC-20: a raw's preview shows first however many of the image's pixels
/// it has; only a JPEG's must have under a third.
#[test]
fn a_raw_preview_shows_first_with_a_third_of_the_pixels_or_more() {
    // 1000 × 667 of 1500 × 1000 is 44 %, and short of the 1485 × 990
    // that would make it the stage.
    let mut h = Harness::new();
    h.set_system_decoder(Some((1500, 1000)));
    h.write_file("a.raf", &raf(&jpeg(1000, 667), (1500, 1000)));
    h.open("a.raf");
    let id = current_id(&h);
    assert!(h.app.has_stage(id, Stage::Preview));
    assert!(h.app.has_stage(id, Stage::Screen));
    assert_eq!(h.decodes(), [((1500, 1000), 2)]);
}

/// AC-20: a smaller raw preview shows first, then the system decoder
/// develops the image and says how large it is.
#[test]
fn a_raw_preview_that_covers_half_the_screen_box_shows_first_and_the_developed_size_wins() {
    // 1000 wide covers half of the 1620 the screen box needs.
    let mut h = with_raw("a.raf", (1000, 667));
    h.open("a.raf");
    let id = current_id(&h);
    assert!(h.app.has_stage(id, Stage::Preview));
    assert!(h.app.has_stage(id, Stage::Screen));
    assert_eq!(h.decodes(), [((6240, 4160), 2)]);
    assert_eq!(probed_size(&h), DEVELOPED, "the system decoder's size");
    assert_eq!(h.frame().images[0].full_size, DEVELOPED);
    drop(h);

    // 640 × 427 covers neither half the width nor half the height.
    let mut h = with_raw("b.raf", (640, 427));
    h.open("b.raf");
    let id = current_id(&h);
    assert!(!h.app.has_stage(id, Stage::Preview));
    assert!(h.app.has_stage(id, Stage::Screen));
    assert_eq!(h.decodes(), [((6240, 4160), 2)]);
}

/// A 3600 × 2400 JPEG named `name` followed by a JPEG of `preview`, which
/// the probe reports as its embedded preview.
fn jpeg_with_preview(h: &Harness, name: &str, preview: (u32, u32)) {
    let mut data = jpeg(3600, 2400);
    let at = data.len();
    data.extend_from_slice(&jpeg(preview.0, preview.1));
    h.set_preview(data.len(), at..data.len(), preview);
    h.write_file(name, &data);
}

/// AC-20: a JPEG's preview shows first only while it has under a third of
/// the main image's pixels (and covers half the screen box).
#[test]
fn a_jpeg_preview_shows_first_only_with_under_a_third_of_the_pixels() {
    // 3600 × 2400 has 8.64 MP; a third is 2.88 MP.
    for (preview, shown) in [
        ((1980, 1320), true),  // 2.61 MP
        ((2100, 1400), false), // 2.94 MP
        ((800, 533), false),   // under half of 1620 × 1080
        ((810, 540), true),    // exactly half
        ((900, 300), true),    // half the width only
        ((2400, 1200), false), // exactly a third
    ] {
        let mut h = Harness::new();
        jpeg_with_preview(&h, "a.jpg", preview);
        h.open("a.jpg");
        let id = current_id(&h);
        assert_eq!(h.app.has_stage(id, Stage::Preview), shown, "{preview:?}");
        assert!(h.app.has_stage(id, Stage::Screen), "{preview:?}");
        assert_eq!(h.decodes(), [((3600, 2400), 2)], "the main decode follows");
    }
}

/// `jpeg` with a marker segment inserted after SOI.
fn with_segment(jpeg: &[u8], marker: u8, payload: &[u8]) -> Vec<u8> {
    let len = (payload.len() + 2) as u16;
    [
        &jpeg[..2],
        &[0xFF, marker],
        &len.to_be_bytes(),
        payload,
        &jpeg[2..],
    ]
    .concat()
}

/// A 3600 × 2400 JPEG whose EXIF thumbnail (IFD1) is a JPEG of `thumb` and
/// whose MPF index lists a JPEG of `large` after the image.
fn jpeg_with_exif_and_mpf(thumb: (u32, u32), large: (u32, u32)) -> Vec<u8> {
    let (main, thumb, large) = (
        jpeg(3600, 2400),
        jpeg(thumb.0, thumb.1),
        jpeg(large.0, large.1),
    );
    // TIFF: an empty IFD0 at 8, IFD1 at 14 with the thumbnail right after
    // it (at 44).
    let mut app1 = b"Exif\0\0II*\0".to_vec();
    app1.extend(8u32.to_le_bytes());
    app1.extend(0u16.to_le_bytes());
    app1.extend(14u32.to_le_bytes());
    app1.extend(2u16.to_le_bytes());
    for (tag, v) in [(0x0201u16, 44u32), (0x0202, thumb.len() as u32)] {
        app1.extend(tag.to_le_bytes());
        app1.extend(4u16.to_le_bytes());
        app1.extend(1u32.to_le_bytes());
        app1.extend(v.to_le_bytes());
    }
    app1.extend(0u32.to_le_bytes());
    app1.extend(&thumb);
    // MPF: the index IFD at 8, its two entries at 26 (primary, preview);
    // offsets count from the MPF TIFF header.
    let mpf_len = 4 + 26 + 32;
    let mpf_at = 2 + 4 + app1.len() + 4 + 4;
    let large_at = 2 + 4 + app1.len() + 4 + mpf_len + main.len() - 2;
    let mut mpf = b"MPF\0II*\0".to_vec();
    mpf.extend(8u32.to_le_bytes());
    mpf.extend(1u16.to_le_bytes());
    mpf.extend(0xB002u16.to_le_bytes());
    mpf.extend(7u16.to_le_bytes());
    mpf.extend(32u32.to_le_bytes());
    mpf.extend(26u32.to_le_bytes());
    mpf.extend(0u32.to_le_bytes());
    for (attr, size, off) in [
        (0x03_0000u32, main.len() as u32, 0u32),
        (0x01_0002, large.len() as u32, (large_at - mpf_at) as u32),
    ] {
        mpf.extend([attr, size, off, 0].iter().flat_map(|v| v.to_le_bytes()));
    }
    let data = with_segment(&with_segment(&main, 0xE2, &mpf), 0xE1, &app1);
    [data, large].concat()
}

/// AC-20: of a JPEG's previews the largest shows first, an MPF preview
/// over a small EXIF thumbnail.
#[test]
fn a_jpegs_largest_preview_shows_first() {
    let mut h = Harness::new();
    h.write_file("a.jpg", &jpeg_with_exif_and_mpf((160, 120), (1000, 667)));
    h.open("a.jpg");
    let id = current_id(&h);
    let sizes: Vec<_> = h.app.probes[&id]
        .previews
        .iter()
        .map(|p| (p.width, p.height))
        .collect();
    assert_eq!(sizes, [(1000, 667), (160, 120)]);
    let shown = h.app.textures.peek(&loader::key(id, Stage::Preview));
    assert_eq!(shown.map(|t| t.size), Some((1000, 667)));
    assert!(h.app.has_stage(id, Stage::Screen));
}

/// AC-20: a raw preview larger than the screen box is the Screen stage at
/// the reduced size that still covers it, not at its own size.
#[test]
fn a_raw_preview_larger_than_the_screen_box_decodes_reduced() {
    // 4000 × 2667 at 1/2 still covers the 1620 × 1080 the screen box needs.
    let mut h = with_raw("a.raf", (4000, 2667));
    h.open_settled("a.raf");
    let id = current_id(&h);
    assert!(h.app.has_stage(id, Stage::Screen));
    assert_eq!(h.decodes(), [], "the system decoder is not asked");
    assert_eq!(h.frame().images[0].size, (2000, 1334));
}

/// AC-20: the image after the current one in browsing direction gets its
/// preview first too, the ones further ahead only the Screen stage.
#[test]
fn the_next_image_gets_its_preview_first_and_later_ones_do_not() {
    let mut h = Harness::new();
    for n in ["a.jpg", "b.jpg", "c.jpg", "d.jpg", "e.jpg", "f.jpg"] {
        jpeg_with_preview(&h, n, (960, 640));
    }
    h.open("c.jpg");
    h.pump();
    let id = |h: &Harness, n: &str| {
        let i = h.names().iter().position(|m| m == n).unwrap();
        h.app.list.get(i).unwrap().cache_id()
    };
    let (next, later) = (id(&h, "d.jpg"), id(&h, "e.jpg"));
    assert!(h.app.has_stage(next, Stage::Screen) && h.app.has_stage(later, Stage::Screen));
    assert!(h.app.has_stage(next, Stage::Preview), "the next image");
    assert!(!h.app.has_stage(later, Stage::Preview), "two ahead");
}

/// AC-24: without a system decoder (Linux, whose shell hands the loader
/// the native decoders only) a raw shows its embedded preview, and a file
/// the native decoders do not handle says why.
#[test]
fn without_a_system_decoder_a_raw_shows_its_preview_and_other_files_fail() {
    let mut h = Harness::new();
    h.write_file("a.raf", &raf(&jpeg(1000, 667), (6240, 4160)));
    // An ICO header: recognized, not decoded natively.
    h.write_file("b.ico", &[0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0]);
    h.open("a.raf");
    let drawn = &h.frame().images;
    assert_eq!(drawn.len(), 1);
    assert_eq!(
        (drawn[0].size, drawn[0].full_size),
        ((1000, 667), (6240, 4160))
    );
    assert!(h.frame().texts.is_empty(), "no error over the preview");
    assert_eq!(
        h.decodes(),
        [((6240, 4160), 2)],
        "the sensor data was tried"
    );

    h.open("b.ico");
    let id = current_id(&h);
    assert_eq!(h.app.error_of(id), Some("Ico is not decoded natively"));
    assert!(h.frame().images.is_empty());
    // The window shows it in words of its own (viewing.md).
    assert!(
        h.frame()
            .texts
            .iter()
            .any(|t| t.starts_with("Could not open “b.ico”"))
    );
}
