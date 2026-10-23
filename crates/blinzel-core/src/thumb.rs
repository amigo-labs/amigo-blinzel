//! Thumbnails: the smallest decode that covers the tile (an embedded
//! preview, or JPEG DCT scaling down to 1/8), reduced by exact area
//! averaging. The EXIF orientation is kept as a tag and applied when the
//! thumbnail is drawn.

use crate::decode::{self, Probe, Request, Result};
use crate::geom::Orientation;
use crate::pixel::{Image, Pixels, Plane, ycc_to_rgb_u8};

/// Longest edge of a thumbnail, in pixels (Explorer's large thumbnails).
pub const EDGE: u32 = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA in stored (not oriented) layout, straight alpha.
    pub bgra: Vec<u8>,
    pub orientation: Orientation,
    /// The image's full-resolution size (stored orientation).
    pub full: (u32, u32),
}

impl Thumb {
    pub fn has_alpha(&self) -> bool {
        self.bgra.as_chunks::<4>().0.iter().any(|p| p[3] != 255)
    }
}

/// A thumbnail as the GPU atlas takes it: Y at full resolution and Cb/Cr
/// interleaved at half resolution both ways (4:2:0), opaque. 1.5 bytes
/// per pixel to upload instead of 4; the shader converts to RGB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planes {
    pub width: u32,
    pub height: u32,
    /// `width × height`.
    pub y: Vec<u8>,
    /// `chroma_size()` pixels of (Cb, Cr).
    pub cbcr: Vec<u8>,
    pub orientation: Orientation,
    pub full: (u32, u32),
}

impl Planes {
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }

    /// From a BGRA thumbnail; alpha is composited over `background` (RGB).
    pub fn from_thumb(t: &Thumb, background: [u8; 3]) -> Planes {
        let (w, h) = (t.width as usize, t.height as usize);
        let rgb: Vec<[i32; 3]> = t
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let a = p[3] as i32;
                let mix = |c: u8, b: u8| (c as i32 * a + b as i32 * (255 - a) + 127) / 255;
                [
                    mix(p[2], background[0]),
                    mix(p[1], background[1]),
                    mix(p[0], background[2]),
                ]
            })
            .collect();
        let y = rgb
            .iter()
            .map(|&[r, g, b]| ((19_595 * r + 38_470 * g + 7_471 * b + 32_768) >> 16) as u8)
            .collect();
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut cbcr = Vec::with_capacity(cw * ch * 2);
        for cy in 0..ch {
            for cx in 0..cw {
                // The 2 × 2 block's mean colour (fewer pixels at odd edges).
                let mut sum = [0i32; 3];
                let mut n = 0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (x, yy) = (2 * cx + dx, 2 * cy + dy);
                    if x < w && yy < h {
                        let p = rgb[yy * w + x];
                        (0..3).for_each(|k| sum[k] += p[k]);
                        n += 1;
                    }
                }
                let [r, g, b] = sum.map(|v| (v + n / 2) / n);
                let cb = (-11_059 * r - 21_709 * g + 32_768 * b + (128 << 16) + 32_768) >> 16;
                let cr = (32_768 * r - 27_439 * g - 5_329 * b + (128 << 16) + 32_768) >> 16;
                cbcr.extend([cb.clamp(0, 255) as u8, cr.clamp(0, 255) as u8]);
            }
        }
        Planes {
            width: t.width,
            height: t.height,
            y,
            cbcr,
            orientation: t.orientation,
            full: t.full,
        }
    }

    /// From a decoded thumbnail: 4:2:0 YCbCr (the cache's JPEGs) is taken
    /// as it is, anything else goes through BGRA.
    pub fn from_image(
        img: &Image,
        orientation: Orientation,
        full: (u32, u32),
        background: [u8; 3],
    ) -> Planes {
        let (w, h) = (img.width, img.height);
        if let Pixels::Ycc { y, cb, cr, sub } = &img.pixels
            && (sub.h, sub.v) == (2, 2)
        {
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            let mut luma = Vec::with_capacity((w * h) as usize);
            for row in 0..h {
                luma.extend(&y.row(row)[..w as usize]);
            }
            let mut cbcr = Vec::with_capacity((cw * ch * 2) as usize);
            for row in 0..ch {
                let (b, r) = (cb.row(row), cr.row(row));
                for x in 0..cw as usize {
                    cbcr.extend([b[x], r[x]]);
                }
            }
            return Planes {
                width: w,
                height: h,
                y: luma,
                cbcr,
                orientation,
                full,
            };
        }
        let t = Thumb {
            width: w,
            height: h,
            bgra: to_bgra(img, (w, h)),
            orientation,
            full,
        };
        Planes::from_thumb(&t, background)
    }
}

/// `full` shrunk to fit `edge` × `edge`, keeping the aspect ratio; never
/// enlarged, at least 1 px per side.
pub fn fit(full: (u32, u32), edge: u32) -> (u32, u32) {
    let (w, h) = full;
    if w <= edge && h <= edge {
        return (w.max(1), h.max(1));
    }
    let s = edge as f64 / w.max(h) as f64;
    (
        ((w as f64 * s).round() as u32).clamp(1, edge),
        ((h as f64 * s).round() as u32).clamp(1, edge),
    )
}

/// Decodes `data` (a whole file) as asked; the app passes one that falls
/// back to Windows' codecs.
pub type DecodeFn<'a> = &'a dyn Fn(&[u8], Request) -> Result<Image>;

/// The thumbnail of a whole image file, using `probe` (already read).
pub fn make(data: &[u8], probe: &Probe, edge: u32, decode: DecodeFn) -> Result<Thumb> {
    let full = (probe.width, probe.height);
    let target = fit(full, edge);
    let req = Request::Fit {
        w: target.0,
        h: target.1,
    };
    let img = match preview_for(probe, target).and_then(|r| data.get(r)) {
        Some(bytes) => decode(bytes, req).or_else(|_| decode(data, req))?,
        None => decode(data, req)?,
    };
    Ok(Thumb {
        width: target.0,
        height: target.1,
        bgra: to_bgra(&img, target),
        orientation: probe.orientation,
        full,
    })
}

/// `img` as BGRA at `size`. YCbCr is reduced per plane and converted once
/// at the target size: far cheaper than converting the whole decode, and
/// the same up to clipping, since the conversion is linear.
pub fn to_bgra(img: &Image, size: (u32, u32)) -> Vec<u8> {
    match &img.pixels {
        // A cached thumbnail: no reduction, chroma from the nearest sample.
        Pixels::Ycc { y, cb, cr, sub } if size == (img.width, img.height) => {
            let (w, h) = (img.width as usize, img.height as usize);
            let (sh, sv) = (sub.h as usize, sub.v as usize);
            let mut out = Vec::with_capacity(w * h * 4);
            for row in 0..h {
                let (ly, crow) = (
                    y.row(row as u32),
                    (row / sv).min(cb.height as usize - 1) as u32,
                );
                let (lb, lr) = (cb.row(crow), cr.row(crow));
                for (x, &l) in ly[..w].iter().enumerate() {
                    let c = (x / sh).min(cb.width as usize - 1);
                    let [r, g, b] = ycc_to_rgb_u8(l as i32, lb[c] as i32, lr[c] as i32);
                    out.extend([b, g, r, 255]);
                }
            }
            out
        }
        Pixels::Ycc { y, cb, cr, .. } => {
            let (ly, lb, lr) = (
                reduce_plane(y, (img.width, img.height), size),
                reduce_plane(cb, (cb.width, cb.height), size),
                reduce_plane(cr, (cr.width, cr.height), size),
            );
            let mut out = Vec::with_capacity(ly.len() * 4);
            for i in 0..ly.len() {
                let [r, g, b] = ycc_to_rgb_u8(ly[i] as i32, lb[i] as i32, lr[i] as i32);
                out.extend([b, g, r, 255]);
            }
            out
        }
        Pixels::Packed { .. } if size == (img.width, img.height) => img.to_bgra(),
        Pixels::Packed { .. } => reduce(&img.to_bgra(), (img.width, img.height), size),
    }
}

/// One 8-bit plane (its top-left `src` area) by exact area averaging.
fn reduce_plane(p: &Plane, src: (u32, u32), dst: (u32, u32)) -> Vec<u8> {
    let (dw, dh) = (dst.0 as usize, dst.1 as usize);
    let (hx, vy) = (weights(src.0, dst.0), weights(src.1, dst.1));
    let mut rows = vec![0f32; dw * src.1 as usize];
    for y in 0..src.1 as usize {
        let line = p.row(y as u32);
        for (x, (first, w)) in hx.iter().enumerate() {
            rows[y * dw + x] = w
                .iter()
                .enumerate()
                .map(|(k, wk)| line[first + k] as f32 * wk)
                .sum();
        }
    }
    let mut out = vec![0u8; dw * dh];
    for (y, (first, w)) in vy.iter().enumerate() {
        for x in 0..dw {
            let v: f32 = w
                .iter()
                .enumerate()
                .map(|(k, wk)| rows[(first + k) * dw + x] * wk)
                .sum();
            out[y * dw + x] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// The smallest embedded preview that covers `target` with the image's
/// aspect ratio (EXIF thumbnails are often 4:3 with black bars).
fn preview_for(probe: &Probe, target: (u32, u32)) -> Option<std::ops::Range<usize>> {
    let aspect = probe.width as f64 / probe.height as f64;
    probe
        .previews
        .iter()
        .filter(|p| p.width >= target.0 && p.height >= target.1)
        .filter(|p| ((p.width as f64 / p.height as f64) / aspect - 1.0).abs() < 0.02)
        .min_by_key(|p| p.width as u64 * p.height as u64)
        .map(|p| p.range.clone())
}

/// For each output index: the first source index and the weights of the
/// source cells the output cell covers (summing to 1).
fn weights(src: u32, dst: u32) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            let (lo, hi) = (i as f64 * scale, (i + 1) as f64 * scale);
            let first = lo.floor() as usize;
            let last = (hi.ceil() as usize).min(src as usize);
            let w = (first..last)
                .map(|j| {
                    let overlap = hi.min((j + 1) as f64) - lo.max(j as f64);
                    (overlap / scale) as f32
                })
                .collect();
            (first, w)
        })
        .collect()
}

/// Shrinks a BGRA image by exact area averaging (separable; colours are
/// averaged premultiplied, so transparent pixels do not bleed in).
pub fn reduce(bgra: &[u8], src: (u32, u32), dst: (u32, u32)) -> Vec<u8> {
    let (sw, sh) = (src.0 as usize, src.1 as usize);
    let (dw, dh) = (dst.0 as usize, dst.1 as usize);
    let (hx, vy) = (weights(src.0, dst.0), weights(src.1, dst.1));
    // Horizontal pass: sh rows of dw premultiplied RGBA in f32.
    let mut rows = vec![[0f32; 4]; dw * sh];
    for y in 0..sh {
        let line = bgra[y * sw * 4..][..sw * 4].as_chunks::<4>().0;
        for (x, (first, w)) in hx.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, &wk) in w.iter().enumerate() {
                let p = line[first + k];
                let a = p[3] as f32 * wk;
                acc[0] += p[0] as f32 * a;
                acc[1] += p[1] as f32 * a;
                acc[2] += p[2] as f32 * a;
                acc[3] += a;
            }
            rows[y * dw + x] = acc;
        }
    }
    // Vertical pass, then back to straight alpha.
    let mut out = vec![0u8; dw * dh * 4];
    for (y, (first, w)) in vy.iter().enumerate() {
        for x in 0..dw {
            let mut acc = [0f32; 4];
            for (k, &wk) in w.iter().enumerate() {
                let p = rows[(first + k) * dw + x];
                for c in 0..4 {
                    acc[c] += p[c] * wk;
                }
            }
            let o = &mut out[(y * dw + x) * 4..][..4];
            let a = acc[3];
            if a > 0.0 {
                for c in 0..3 {
                    o[c] = (acc[c] / a).round().clamp(0.0, 255.0) as u8;
                }
            }
            o[3] = a.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// [`make`] with the native decoders only (tests, benchmarks, Linux).
pub fn make_native(data: &[u8], edge: u32) -> Result<Thumb> {
    let probe = decode::probe(data)?;
    make(data, &probe, edge, &decode::decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_the_tile() {
        assert_eq!(fit((6000, 4000), 256), (256, 171));
        assert_eq!(fit((4000, 6000), 256), (171, 256));
        assert_eq!(fit((100, 50), 256), (100, 50));
        assert_eq!(fit((100_000, 10), 256), (256, 1));
        assert_eq!(fit((0, 0), 256), (1, 1));
    }

    #[test]
    fn area_average_is_exact() {
        // 3 → 2: the middle source pixel is shared half/half.
        let src = [[0u8, 0, 0, 255], [90, 90, 90, 255], [180, 180, 180, 255]].concat();
        assert_eq!(
            reduce(&src, (3, 1), (2, 1)),
            [[30u8, 30, 30, 255], [150, 150, 150, 255]].concat()
        );
        // A flat colour stays flat at any ratio.
        let flat = [10u8, 200, 30, 255].repeat(97 * 61);
        let out = reduce(&flat, (97, 61), (40, 25));
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [10, 200, 30, 255])
        );
    }

    #[test]
    fn planes_round_trip() {
        let (w, h) = (97, 61);
        let rgb = crate::testimg::rgb(w, h);
        let bgra: Vec<u8> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        let t = Thumb {
            width: w,
            height: h,
            bgra: bgra.clone(),
            orientation: Orientation::NORMAL,
            full: (w, h),
        };
        let p = Planes::from_thumb(&t, [0, 0, 0]);
        assert_eq!(p.chroma_size(), (49, 31));
        assert_eq!((p.y.len(), p.cbcr.len()), (97 * 61, 49 * 31 * 2));
        // Back through the 4:2:0 decode path: close to the original.
        let mut y = Plane::new(w, h, 1);
        y.data.copy_from_slice(&p.y);
        let (mut cb, mut cr) = (Plane::new(49, 31, 1), Plane::new(49, 31, 1));
        for (i, c) in p.cbcr.as_chunks::<2>().0.iter().enumerate() {
            cb.data[i] = c[0];
            cr.data[i] = c[1];
        }
        let img = Image {
            pixels: Pixels::Ycc {
                y,
                cb,
                cr,
                sub: crate::pixel::ChromaSub { h: 2, v: 2 },
            },
            width: w,
            height: h,
            full_width: w,
            full_height: h,
            region: None,
        };
        let back = to_bgra(&img, (w, h));
        let psnr = crate::cpuref::psnr(&back, &bgra);
        assert!(psnr > 30.0, "{psnr:.1} dB");
        // Taken as they are from a 4:2:0 image.
        assert_eq!(
            Planes::from_image(&img, Orientation::NORMAL, (w, h), [0; 3]),
            p
        );
        // Transparent pixels show the background.
        let clear = Thumb {
            width: 2,
            height: 2,
            bgra: [0u8, 0, 0, 0].repeat(4),
            orientation: Orientation::NORMAL,
            full: (2, 2),
        };
        let p = Planes::from_thumb(&clear, [255, 255, 255]);
        assert_eq!((p.y[0], p.cbcr[0], p.cbcr[1]), (255, 128, 128));
    }

    #[test]
    fn transparent_pixels_do_not_bleed() {
        // Opaque red next to fully transparent "black".
        let src = [[0u8, 0, 255, 255], [0, 0, 0, 0]].concat();
        assert_eq!(reduce(&src, (2, 1), (1, 1)), [0, 0, 255, 128]);
    }

    #[cfg(feature = "turbojpeg")]
    #[test]
    fn thumbnails_a_jpeg_with_dct_scaling() {
        use crate::decode::jpeg::{EncodeOptions, encode_rgb};
        let (w, h) = (2400, 1600);
        let rgb = crate::testimg::rgb(w, h);
        let data = encode_rgb(&rgb, w, h, EncodeOptions::default()).unwrap();
        let t = make_native(&data, EDGE).unwrap();
        assert_eq!((t.width, t.height, t.full), (256, 171, (w, h)));
        assert_eq!(t.bgra.len(), 256 * 171 * 4);
        // Close to reducing the full decode directly.
        let full = decode::decode(&data, Request::Full).unwrap().to_bgra();
        let want = reduce(&full, (w, h), (256, 171));
        let psnr = crate::cpuref::psnr(&t.bgra, &want);
        assert!(psnr > 30.0, "{psnr:.1} dB");
    }
}
