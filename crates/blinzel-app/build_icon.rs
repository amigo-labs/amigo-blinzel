//! The application icon, drawn from the website favicon's geometry (blue
//! rounded square, white eye, blue pupil in a 32-unit design space) and
//! packed as an ICO. Used by build.rs and its test.

const BLUE: [f64; 3] = [0.0, 120.0, 212.0];
const WHITE: [f64; 3] = [255.0, 255.0, 255.0];
/// Samples per pixel and axis (anti-aliasing).
const SS: usize = 4;
const CORNER: f64 = 7.0;
const PUPIL: f64 = 4.5;

/// The upper eyelid: two cubic Béziers from (5,16) over (16,8) to (27,16),
/// flattened. Monotonic in x; the lower lid mirrors it at y = 16.
fn lid() -> Vec<(f64, f64)> {
    let segments = [
        [(5.0, 16.0), (8.0, 10.5), (12.0, 8.0), (16.0, 8.0)],
        [(16.0, 8.0), (20.0, 8.0), (24.0, 10.5), (27.0, 16.0)],
    ];
    let mut pts = Vec::new();
    for s in segments {
        for i in 0..=256 {
            let t = i as f64 / 256.0;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            let x = (0..4).map(|k| w[k] * s[k].0).sum();
            let y = (0..4).map(|k| w[k] * s[k].1).sum();
            pts.push((x, y));
        }
    }
    pts
}

/// The upper lid's y at `x` (5 ≤ x ≤ 27).
fn lid_y(lid: &[(f64, f64)], x: f64) -> f64 {
    let i = lid.partition_point(|p| p.0 < x).clamp(1, lid.len() - 1);
    let (a, b) = (lid[i - 1], lid[i]);
    if b.0 <= a.0 {
        return a.1;
    }
    a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0)
}

fn in_square(x: f64, y: f64) -> bool {
    if !(0.0..32.0).contains(&x) || !(0.0..32.0).contains(&y) {
        return false;
    }
    let (cx, cy) = (
        x.clamp(CORNER, 32.0 - CORNER),
        y.clamp(CORNER, 32.0 - CORNER),
    );
    (x - cx).powi(2) + (y - cy).powi(2) <= CORNER * CORNER
}

/// Colour at design point (x, y), `None` outside the icon.
fn colour(lid: &[(f64, f64)], x: f64, y: f64) -> Option<[f64; 3]> {
    if !in_square(x, y) {
        return None;
    }
    let eye = (5.0..=27.0).contains(&x) && {
        let top = lid_y(lid, x);
        y > top && y < 32.0 - top
    };
    let pupil = (x - 16.0).powi(2) + (y - 16.0).powi(2) < PUPIL * PUPIL;
    Some(if eye && !pupil { WHITE } else { BLUE })
}

/// Straight-alpha RGBA pixels at `size` × `size`.
pub fn rgba(size: u32) -> Vec<u8> {
    let lid = lid();
    let n = size as usize;
    let step = 32.0 / (n * SS) as f64;
    let mut out = vec![0u8; n * n * 4];
    for py in 0..n {
        for px in 0..n {
            let mut sum = [0.0f64; 3];
            let mut covered = 0usize;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = ((px * SS + sx) as f64 + 0.5) * step;
                    let y = ((py * SS + sy) as f64 + 0.5) * step;
                    if let Some(c) = colour(&lid, x, y) {
                        for k in 0..3 {
                            sum[k] += c[k];
                        }
                        covered += 1;
                    }
                }
            }
            if covered > 0 {
                let p = &mut out[(py * n + px) * 4..][..4];
                for k in 0..3 {
                    p[k] = (sum[k] / covered as f64).round() as u8;
                }
                p[3] = (covered as f64 * 255.0 / (SS * SS) as f64).round() as u8;
            }
        }
    }
    out
}

fn png(size: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = png::Encoder::new(&mut buf, size, size);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    let mut w = enc.write_header().expect("png header");
    w.write_image_data(&rgba(size)).expect("png data");
    w.finish().expect("png end");
    buf
}

/// A 32-bit DIB icon image: the header (height covers both bitmaps), the
/// colour bitmap bottom-up in BGRA, then the 1-bit AND mask (set where
/// fully transparent).
pub fn dib(size: u32) -> Vec<u8> {
    let px = rgba(size);
    let n = size as usize;
    let mask_stride = n.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + n * n * 4 + mask_stride * n);
    out.extend(40u32.to_le_bytes());
    out.extend((size as i32).to_le_bytes());
    out.extend((2 * size as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes()); // planes
    out.extend(32u16.to_le_bytes()); // bits per pixel
    out.extend(0u32.to_le_bytes()); // BI_RGB
    out.extend(((n * n * 4 + mask_stride * n) as u32).to_le_bytes());
    out.extend([0u8; 16]); // resolution, palette
    for y in (0..n).rev() {
        for p in px[y * n * 4..][..n * 4].as_chunks::<4>().0 {
            out.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    for y in (0..n).rev() {
        let mut row = vec![0u8; mask_stride];
        for x in 0..n {
            if px[(y * n + x) * 4 + 3] == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend(row);
    }
    out
}

/// An ICO file: uncompressed DIBs up to 128 px (no decoding when Windows
/// loads the small sizes at startup), PNG for 256 px.
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&s| if s >= 256 { png(s) } else { dib(s) })
        .collect();
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes()); // reserved
    out.extend(1u16.to_le_bytes()); // type: icon
    out.extend((sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&size, image) in sizes.iter().zip(&images) {
        // 0 means 256.
        let dim = if size >= 256 { 0 } else { size as u8 };
        out.extend([dim, dim, 0, 0]); // width, height, colours, reserved
        out.extend(1u16.to_le_bytes()); // planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((image.len() as u32).to_le_bytes());
        out.extend((offset as u32).to_le_bytes());
        offset += image.len();
    }
    for image in images {
        out.extend(image);
    }
    out
}
