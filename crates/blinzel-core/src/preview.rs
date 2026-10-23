//! The first frame on the CPU: shown through GDI while the Direct3D device
//! is still being created (115–150 ms on a real GPU, longer than reading
//! and decoding the image). Same geometry as the GPU renderer
//! ([`cpuref::Frame`]); sampling is nearest-neighbour, or a 2 × 2 box when
//! minifying, and colour conversion is integer. Good enough for the moment
//! until the GPU frame replaces it, and about 100× faster than the exact
//! reference renderer.

use crate::cpuref::Frame;
use crate::geom::Rect;
use crate::layout::texels_per_pixel;
use crate::lut::Lut3d;
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

/// Runs `f(y, row)` for every BGRA row of `out`; with a colour transform
/// (about 4× the work) on several threads.
fn rows(out: &mut [u8], width: usize, parallel: bool, f: impl Fn(usize, &mut [u8]) + Sync) {
    let stride = width * 4;
    let height = out.len() / stride.max(1);
    let threads = if parallel {
        std::thread::available_parallelism().map_or(1, |n| n.get().min(8))
    } else {
        1
    };
    if threads <= 1 || height < 2 * threads {
        for (y, row) in out.chunks_exact_mut(stride).enumerate() {
            f(y, row);
        }
        return;
    }
    let band = height.div_ceil(threads);
    std::thread::scope(|s| {
        for (b, chunk) in out.chunks_mut(band * stride).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, row) in chunk.chunks_exact_mut(stride).enumerate() {
                    f(b * band + i, row);
                }
            });
        }
    });
}

/// Full-range BT.601 (JFIF) to RGB in 16.16 fixed point.
#[inline(always)]
fn ycc(y: i32, cb: i32, cr: i32) -> [u8; 3] {
    let (cb, cr) = (cb - 128, cr - 128);
    let c = |v: i32| v.clamp(0, 255) as u8;
    [
        c(y + ((91_881 * cr + 32_768) >> 16)),
        c(y - ((22_554 * cb + 46_802 * cr - 32_768) >> 16)),
        c(y + ((116_130 * cb + 32_768) >> 16)),
    ]
}

#[inline(always)]
fn texel(p: &Plane, bpp: usize, x: i64, y: i64) -> &[u8] {
    let xi = x.clamp(0, p.width as i64 - 1) as usize;
    let yi = y.clamp(0, p.height as i64 - 1) as u32;
    &p.row(yi)[xi * bpp..xi * bpp + bpp]
}

/// RGBA at stage texel coordinates, nearest texel.
#[inline(always)]
fn sample(img: &Image, x: f64, y: f64) -> [u8; 4] {
    let (xi, yi) = (x.floor() as i64, y.floor() as i64);
    match &img.pixels {
        Pixels::Packed {
            plane,
            format,
            alpha,
        } => {
            let a = |v: u8| if *alpha == Alpha::Opaque { 255 } else { v };
            match format {
                PackedFormat::Rgba8 => {
                    let t = texel(plane, 4, xi, yi);
                    [t[0], t[1], t[2], a(t[3])]
                }
                PackedFormat::Bgra8 => {
                    let t = texel(plane, 4, xi, yi);
                    [t[2], t[1], t[0], a(t[3])]
                }
                PackedFormat::Gray8 => {
                    let g = texel(plane, 1, xi, yi)[0];
                    [g, g, g, 255]
                }
            }
        }
        Pixels::Ycc { y: yp, cb, cr, sub } => {
            let l = texel(yp, 1, xi, yi)[0] as i32;
            let (cx, cy) = (
                (x / sub.h as f64).floor() as i64,
                (y / sub.v as f64).floor() as i64,
            );
            let [r, g, b] = ycc(
                l,
                texel(cb, 1, cx, cy)[0] as i32,
                texel(cr, 1, cx, cy)[0] as i32,
            );
            [r, g, b, 255]
        }
    }
}

/// Sample positions along one output axis: for each output pixel the two
/// stage texels (luma and chroma) of the 2 × 2 box, or `None` outside the
/// content.
#[derive(Clone, Copy)]
struct Taps {
    luma: [usize; 2],
    chroma: [usize; 2],
}

/// Output pixel `i` samples stage coordinate `scale · (i + ½) + offset`.
fn axis(
    n: usize,
    (scale, offset): (f64, f64),
    (lo, hi): (f64, f64),
    len: u32,
    sub: u8,
    q: f64,
) -> Vec<Option<Taps>> {
    let last = len.max(1) as f64 - 1.0;
    let clen = len.div_ceil(sub as u32).max(1) as f64 - 1.0;
    (0..n)
        .map(|i| {
            let s = scale * (i as f64 + 0.5) + offset;
            if s < lo || s >= hi {
                return None;
            }
            let at = |v: f64, max: f64| v.floor().clamp(0.0, max) as usize;
            let (s0, s1) = (s - q, s + q);
            Some(Taps {
                luma: [at(s0, last), at(s1, last)],
                chroma: [at(s0 / sub as f64, clen), at(s1 / sub as f64, clen)],
            })
        })
        .collect()
}

/// Renders `img` into a tightly packed, top-down BGRA frame.
///
/// EXIF orientations and view transforms are axis-aligned, so the stage
/// texels of every output column and row are computed once; the inner loop
/// is table lookups and integer arithmetic.
pub fn render(img: &Image, f: &Frame) -> Vec<u8> {
    let (cw, ch) = (f.client.0 as usize, f.client.1 as usize);
    let bg = f.background;
    let mut out: Vec<u8> = [bg[2], bg[1], bg[0], 255].repeat(cw * ch);
    let Some(inv) = f.stage_to_client.inverse() else {
        return out;
    };
    let footprint = texels_per_pixel(&f.stage_to_client);
    // A 2 × 2 box when minifying keeps edges from shimmering.
    let q = if footprint > 1.5 {
        footprint / 4.0
    } else {
        0.0
    };
    let Some(c) = f
        .content
        .intersect(&Rect::new(0.0, 0.0, img.width as f64, img.height as f64))
    else {
        return out;
    };
    let (sub_h, sub_v) = match &img.pixels {
        Pixels::Ycc { sub, .. } => (sub.h, sub.v),
        Pixels::Packed { .. } => (1, 1),
    };
    let x_range = (c.x, c.right());
    let y_range = (c.y, c.bottom());
    // Output columns drive stage x (or stage y when rotated by 90°).
    let swap = if inv.b == 0.0 && inv.d == 0.0 {
        false
    } else if inv.a == 0.0 && inv.e == 0.0 {
        true
    } else {
        return render_general(img, f);
    };
    let (cols, row_taps) = if swap {
        (
            axis(cw, (inv.d, inv.f), y_range, img.height, sub_v, q),
            axis(ch, (inv.b, inv.c), x_range, img.width, sub_h, q),
        )
    } else {
        (
            axis(cw, (inv.a, inv.c), x_range, img.width, sub_h, q),
            axis(ch, (inv.e, inv.f), y_range, img.height, sub_v, q),
        )
    };
    let lut = f.lut.as_deref();
    rows(&mut out, cw, lut.is_some(), |y, row| {
        let Some(ry) = row_taps[y] else { return };
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let Some(cx) = cols[x] else { continue };
            let (tx, ty) = if swap { (ry, cx) } else { (cx, ry) };
            *px = shade(img, tx, ty, bg, lut);
        }
    });
    out
}

/// The 2 × 2 box at stage texels (`tx`, `ty`), colour transformed, over
/// the background, as BGRA.
#[inline(always)]
fn shade(img: &Image, tx: Taps, ty: Taps, bg: [u8; 3], lut: Option<&Lut3d>) -> [u8; 4] {
    let map = |c: [u8; 3]| lut.map_or(c, |l| l.apply_u8(c));
    let sum = |p: &Plane, xs: [usize; 2], ys: [usize; 2], bpp: usize, k: usize| -> u32 {
        let at = |x: usize, y: usize| p.data[y * p.stride + x * bpp + k] as u32;
        at(xs[0], ys[0]) + at(xs[1], ys[0]) + at(xs[0], ys[1]) + at(xs[1], ys[1])
    };
    match &img.pixels {
        Pixels::Ycc { y, cb, cr, .. } => {
            let l = (sum(y, tx.luma, ty.luma, 1, 0) + 2) >> 2;
            let b = (sum(cb, tx.chroma, ty.chroma, 1, 0) + 2) >> 2;
            let r = (sum(cr, tx.chroma, ty.chroma, 1, 0) + 2) >> 2;
            let [r, g, b] = map(ycc(l as i32, b as i32, r as i32));
            [b, g, r, 255]
        }
        Pixels::Packed {
            plane,
            format,
            alpha,
        } => {
            let (tx, ty) = (tx.luma, ty.luma);
            let rgba = match format {
                PackedFormat::Gray8 => {
                    let g = ((sum(plane, tx, ty, 1, 0) + 2) >> 2) as u8;
                    let [r, g, b] = map([g, g, g]);
                    return [b, g, r, 255];
                }
                PackedFormat::Rgba8 => [0, 1, 2, 3].map(|k| (sum(plane, tx, ty, 4, k) + 2) >> 2),
                PackedFormat::Bgra8 => [2, 1, 0, 3].map(|k| (sum(plane, tx, ty, 4, k) + 2) >> 2),
            };
            let [r, g, b] = map([rgba[0] as u8, rgba[1] as u8, rgba[2] as u8]);
            let rgba = [r as u32, g as u32, b as u32, rgba[3]];
            let a = if *alpha == Alpha::Opaque {
                255
            } else {
                rgba[3]
            };
            let mix = |v: u32, b: u8| ((v * a + b as u32 * (255 - a) + 127) / 255) as u8;
            [
                mix(rgba[2], bg[2]),
                mix(rgba[1], bg[1]),
                mix(rgba[0], bg[0]),
                255,
            ]
        }
    }
}

/// Any affine transform, one inverse mapping per pixel (not needed by
/// the viewer's axis-aligned views; kept so `render` is total).
fn render_general(img: &Image, f: &Frame) -> Vec<u8> {
    let (cw, ch) = (f.client.0 as usize, f.client.1 as usize);
    let mut out = vec![0u8; cw * ch * 4];
    let Some(inv) = f.stage_to_client.inverse() else {
        return out;
    };
    let bg = f.background;
    let footprint = texels_per_pixel(&f.stage_to_client);
    let offs: &[(f64, f64)] = if footprint > 1.5 {
        let q = footprint / 4.0;
        &[(-q, -q), (q, -q), (-q, q), (q, q)]
    } else {
        &[(0.0, 0.0)]
    };
    let content = f
        .content
        .intersect(&Rect::new(0.0, 0.0, img.width as f64, img.height as f64));
    for (y, row) in out.chunks_exact_mut(cw * 4).enumerate() {
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let l = inv.apply((x as f64 + 0.5, y as f64 + 0.5));
            let mut rgb = bg;
            if let Some(c) = content
                && c.contains(l)
            {
                let mut acc = [0u32; 4];
                for (ox, oy) in offs {
                    let s = sample(img, l.0 + ox, l.1 + oy);
                    for k in 0..4 {
                        acc[k] += s[k] as u32;
                    }
                }
                let n = offs.len() as u32;
                let a = acc[3] / n;
                let c = [0, 1, 2].map(|k| (acc[k] / n) as u8);
                let c = f.lut.as_ref().map_or(c, |l| l.apply_u8(c));
                for k in 0..3 {
                    rgb[k] = ((c[k] as u32 * a + bg[k] as u32 * (255 - a) + 127) / 255) as u8;
                }
            }
            *px = [rgb[2], rgb[1], rgb[0], 255];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuref;
    use crate::geom::Orientation;
    use crate::layout::{
        ViewMode, ViewOptions, ViewState, compute, content_in_stage, stage_to_client,
    };
    use crate::pixel::ChromaSub;
    use crate::testimg;

    fn frame(img: &Image, client: (u32, u32), o: Orientation) -> Frame {
        let full = (img.full_width, img.full_height);
        let opts = ViewOptions {
            mode: ViewMode::FitImageToWindow,
            no_upscale: true,
        };
        let l = compute(
            o.oriented_size(full.0, full.1),
            (client.0 as f64, client.1 as f64),
            &opts,
            &ViewState::default(),
        );
        Frame {
            client,
            stage_to_client: stage_to_client(&l, o, full, (img.width, img.height)),
            content: content_in_stage(&l, o, full, (img.width, img.height)),
            background: [10, 20, 30],
            lut: None,
        }
    }

    fn ycc_image(w: u32, h: u32) -> Image {
        let rgb = testimg::rgb(w, h);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut y = Plane::new(w, h, 1);
        let mut cb = Plane::new(cw, ch, 1);
        let mut cr = Plane::new(cw, ch, 1);
        for (i, p) in rgb.as_chunks::<3>().0.iter().enumerate() {
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            y.data[i] = (0.299 * r + 0.587 * g + 0.114 * b).round() as u8;
            let (x, yy) = (i as u32 % w, i as u32 / w);
            if x % 2 == 0 && yy % 2 == 0 {
                let c = (yy / 2 * cw + x / 2) as usize;
                cb.data[c] = (128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b).round() as u8;
                cr.data[c] = (128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b).round() as u8;
            }
        }
        Image {
            pixels: Pixels::Ycc {
                y,
                cb,
                cr,
                sub: ChromaSub { h: 2, v: 2 },
            },
            width: w,
            height: h,
            full_width: w,
            full_height: h,
            region: None,
        }
    }

    /// The table-driven path renders what the general one does.
    #[test]
    fn fast_path_matches_the_general_path() {
        let img = ycc_image(600, 400);
        for exif in 1..=8 {
            let f = frame(&img, (320, 240), Orientation::from_exif(exif));
            // The fast path averages Y, Cb, Cr before converting, the
            // general one after: equal up to clipping at saturated colours.
            let psnr = cpuref::psnr(&render(&img, &f), &render_general(&img, &f));
            assert!(psnr > 35.0, "orientation {exif}: {psnr:.1} dB");
        }
    }

    /// Close to the exact renderer (the GPU's twin) for every orientation;
    /// background and letterboxing identical.
    #[test]
    fn tracks_the_reference_renderer() {
        let img = ycc_image(600, 400);
        for exif in 1..=8 {
            let o = Orientation::from_exif(exif);
            let f = frame(&img, (320, 240), o);
            let fast = render(&img, &f);
            let exact = cpuref::render(&img, &f);
            let psnr = cpuref::psnr(&fast, &exact);
            assert!(psnr > 26.0, "orientation {exif}: {psnr:.1} dB");
            // Corners are background (letterbox) in both.
            assert_eq!(&fast[..4], &exact[..4]);
        }
    }

    /// A colour transform changes the image, not the letterbox, and the
    /// parallel path renders what the exact renderer does.
    #[test]
    fn applies_the_colour_transform_to_the_image_only() {
        let img = ycc_image(600, 400);
        let lut = std::sync::Arc::new(crate::lut::Lut3d::from_fn(crate::lut::SIZE, |c| {
            [c[1], c[2] * c[2], (c[0] + 0.2).min(1.0)]
        }));
        for exif in [1, 6] {
            let plain = frame(&img, (320, 240), Orientation::from_exif(exif));
            let f = Frame {
                lut: Some(lut.clone()),
                ..plain
            };
            let fast = render(&img, &f);
            let psnr = cpuref::psnr(&fast, &cpuref::render(&img, &f));
            assert!(psnr > 26.0, "orientation {exif}: {psnr:.1} dB");
            let unchanged = render(&img, &frame(&img, (320, 240), Orientation::from_exif(exif)));
            assert!(cpuref::psnr(&fast, &unchanged) < 20.0);
            assert_eq!(&fast[..4], &unchanged[..4]);
            let general = render_general(&img, &f);
            assert!(cpuref::psnr(&fast, &general) > 30.0);
        }
    }

    #[test]
    fn integer_colour_conversion_matches_float() {
        for (y, cb, cr) in [(0, 128, 128), (255, 128, 128), (76, 85, 255), (150, 44, 21)] {
            let f =
                crate::pixel::ycc_to_rgb(y as f32 / 255.0, cb as f32 / 255.0, cr as f32 / 255.0);
            let i = ycc(y, cb, cr);
            for k in 0..3 {
                assert!((crate::pixel::to_u8(f[k]) as i32 - i[k] as i32).abs() <= 1);
            }
        }
    }

    #[test]
    fn straight_alpha_blends_over_the_background() {
        let mut plane = Plane::new(4, 4, 4);
        for px in plane.data.as_chunks_mut::<4>().0 {
            *px = [200, 100, 0, 128];
        }
        let img = Image {
            pixels: Pixels::Packed {
                plane,
                format: PackedFormat::Rgba8,
                alpha: Alpha::Straight,
            },
            width: 4,
            height: 4,
            full_width: 4,
            full_height: 4,
            region: None,
        };
        let f = frame(&img, (4, 4), Orientation::default());
        let out = render(&img, &f);
        let exact = cpuref::render(&img, &f);
        assert!(cpuref::max_diff(&out, &exact) <= 2);
    }
}
