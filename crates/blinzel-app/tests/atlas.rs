//! Thumbnail atlas on WARP: slots, orientation, scaling, no bleeding from
//! neighbouring slots, slot reuse.

#![cfg(windows)]

use blinzel_app::gfx::atlas::Atlas;
use blinzel_app::gfx::{Gfx, Offscreen, Renderer, Rgba};
use blinzel_core::geom::{Orientation, Rect};
use blinzel_core::thumb::{Planes, Thumb};

const RED: [u8; 4] = [0, 0, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [255, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// Quadrants (stored layout): red, green / blue, white.
fn quadrants(w: u32, h: u32, orientation: Orientation) -> Thumb {
    let mut bgra = Vec::new();
    for y in 0..h {
        for x in 0..w {
            bgra.extend(match (x < w / 2, y < h / 2) {
                (true, true) => RED,
                (false, true) => GREEN,
                (true, false) => BLUE,
                (false, false) => WHITE,
            });
        }
    }
    Thumb {
        width: w,
        height: h,
        bgra,
        orientation,
        full: (w, h),
    }
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> Thumb {
    Thumb {
        width: w,
        height: h,
        bgra: c.repeat((w * h) as usize),
        orientation: Orientation::NORMAL,
        full: (w, h),
    }
}

fn pixel(bgra: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [bgra[i], bgra[i + 1], bgra[i + 2], bgra[i + 3]]
}

/// Equal up to the YCbCr round trip.
fn near(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 3)
}

fn planes(t: &Thumb) -> Planes {
    Planes::from_thumb(t, [0, 0, 0])
}

#[test]
fn draws_oriented_and_scaled_without_bleeding() {
    let gfx = Gfx::create(true).expect("WARP device");
    let mut r = Renderer::new(&gfx.device).expect("renderer");
    let mut atlas = Atlas::new(1);
    // Slot 0: the quadrants, rotated (EXIF 6: 90° clockwise); slot 1, its
    // right neighbour: solid green, which must not bleed into slot 0.
    let t = quadrants(200, 100, Orientation::from_exif(6));
    atlas
        .insert(&gfx.device, &gfx.context, 1, &planes(&t))
        .unwrap();
    atlas
        .insert(
            &gfx.device,
            &gfx.context,
            2,
            &planes(&solid(256, 256, GREEN)),
        )
        .unwrap();

    let size = (300, 300);
    let target = Offscreen::new(&gfx.device, size).unwrap();
    let black = Rgba(0.0, 0.0, 0.0, 1.0);
    r.begin(&gfx.context, &target.rtv, size, black);
    // Oriented 100 × 200, drawn at half size into 50 × 100 at (10, 20).
    let placed = atlas.get(1).unwrap();
    assert_eq!(placed.size, (200, 100));
    let dest = Rect::new(10.0, 20.0, 50.0, 100.0);
    r.draw_thumb(&gfx.context, &placed, &dest, 1.0);
    let got = target.read_bgra(&gfx.context).unwrap();
    // 90° clockwise: stored top-left (red) goes to the top right, stored
    // bottom-left (blue) to the top left.
    assert!(near(pixel(&got, 300, 20, 30), BLUE));
    assert!(near(pixel(&got, 300, 50, 30), RED));
    assert!(near(pixel(&got, 300, 20, 110), WHITE));
    assert!(near(pixel(&got, 300, 50, 110), GREEN));
    // Outside the destination: untouched.
    assert_eq!(pixel(&got, 300, 70, 30), [0, 0, 0, 255]);
    // The right edge of the stored image (green / white) borders slot 1
    // (green): the white quadrant's last column must stay white.
    let edge = pixel(&got, 300, 15, 119);
    assert!(edge[0] > 240 && edge[2] > 240, "{edge:?}");
}

#[test]
fn reuses_the_least_recently_drawn_slot() {
    let gfx = Gfx::create(true).expect("WARP device");
    let mut atlas = Atlas::new(1);
    let t = planes(&solid(16, 16, RED));
    for id in 0..64 {
        assert_eq!(
            atlas.insert(&gfx.device, &gfx.context, id, &t).unwrap(),
            None
        );
    }
    atlas.begin_frame();
    // Everything but id 5 is drawn in this frame.
    for id in (0..64).filter(|&id| id != 5) {
        assert!(atlas.get(id).is_some());
    }
    assert_eq!(
        atlas.insert(&gfx.device, &gfx.context, 100, &t).unwrap(),
        Some(5)
    );
    assert!(!atlas.contains(5));
    assert!(atlas.contains(100));
    // Re-inserting an id keeps its slot.
    assert_eq!(
        atlas.insert(&gfx.device, &gfx.context, 100, &t).unwrap(),
        None
    );
}
