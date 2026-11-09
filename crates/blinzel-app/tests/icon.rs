//! The application icon (build_icon.rs, embedded by build.rs): the pixel
//! monogram generated into `assets/brand/`.

#[path = "../build_icon.rs"]
mod icon;

const MONOGRAM_JSON: &str = include_str!("../../../assets/brand/monogram.json");

fn pixel(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

/// The `accent` colour recorded in monogram.json, as RGB.
fn accent() -> [u8; 3] {
    let key = "\"accent\": \"#";
    let at = MONOGRAM_JSON.find(key).expect("accent in monogram.json") + key.len();
    let hex = &MONOGRAM_JSON[at..at + 6];
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex accent");
    [byte(0), byte(2), byte(4)]
}

#[test]
fn uses_the_generated_monogram() {
    let [r, g, b] = accent();
    for (size, _) in icon::BRAND {
        let px = icon::rgba(size);
        assert_eq!(px.len(), (size * size * 4) as usize, "{size} px");
        // The letters are drawn in the accent colour.
        assert!(
            px.as_chunks::<4>().0.contains(&[r, g, b, 255]),
            "{size} px has no accent pixel"
        );
        // Not the old eye: no pixel in its blue.
        assert!(
            !px.as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] == [0, 120, 212] && p[3] > 0),
            "{size} px still shows the eye"
        );
    }
}

/// Windows' 20, 24 and 40 px (the tray at 125 % and 150 %) have no PNG:
/// the mark fills them like every other size instead of sitting in a
/// transparent border, and stays crisp (nearest neighbour, no blended
/// colours).
#[test]
fn sizes_without_a_png_scale_the_small_mark_to_fill() {
    for (size, inner) in [(20, 16), (24, 16), (40, 32)] {
        let px = icon::rgba(size);
        let src = icon::rgba(inner);
        // The plate reaches all four edges at their middle, as in `inner`.
        let mid = size / 2;
        for (x, y) in [(mid, 0), (mid, size - 1), (0, mid), (size - 1, mid)] {
            assert_eq!(pixel(&px, size, x, y)[3], 255, "{size} px at ({x}, {y})");
        }
        // Every pixel is the source pixel under its centre.
        for y in 0..size {
            for x in 0..size {
                let (sx, sy) = (
                    (2 * x + 1) * inner / (2 * size),
                    (2 * y + 1) * inner / (2 * size),
                );
                assert_eq!(
                    pixel(&px, size, x, y),
                    pixel(&src, inner, sx, sy),
                    "{size} px at ({x}, {y})"
                );
            }
        }
    }
}

/// Straight RGBA from an icon DIB (bottom-up BGRA after a 40-byte header).
fn from_dib(data: &[u8], size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut out = vec![0; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let s = &data[40 + ((n - 1 - y) * n + x) * 4..][..4];
            out[(y * n + x) * 4..][..4].copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
    }
    out
}

#[test]
fn packs_dib_and_png_entries() {
    let sizes = [16, 20, 32, 256];
    let ico = icon::ico(&sizes);
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(ico[i..i + 4].try_into().unwrap());
    assert_eq!((u16_at(0), u16_at(2), u16_at(4)), (0, 1, 4));
    for (k, &size) in sizes.iter().enumerate() {
        let entry = 6 + 16 * k;
        let dim = if size == 256 { 0 } else { size as u8 };
        assert_eq!(&ico[entry..entry + 2], &[dim, dim]);
        let (len, offset) = (u32_at(entry + 8) as usize, u32_at(entry + 12) as usize);
        let data = &ico[offset..offset + len];
        if size < 256 {
            assert_eq!(u32::from_le_bytes(data[..4].try_into().unwrap()), 40);
            assert_eq!(from_dib(data, size), icon::rgba(size));
            continue;
        }
        // The 256 px entry is the generated PNG itself.
        assert_eq!(Some(data), icon::png(256));
    }
}
