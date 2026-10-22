//! CPU reference renderer. It implements exactly what the image pixel
//! shader does (bilinear taps, box-filtered minification, YCbCr→RGB,
//! straight-alpha compositing over the background) so the GPU output can be
//! checked pixel by pixel in the WARP golden tests.

use crate::geom::{Affine, Rect};
use crate::layout::texels_per_pixel;
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane, to_u8, ycc_to_rgb};

/// Maximum box-filter taps per axis; must match `MAX_TAPS` in the shader.
pub const MAX_TAPS: u32 = 4;

/// Taps per axis for a footprint of `texels` stage texels per pixel.
pub fn taps(texels: f64) -> u32 {
    ((texels - 0.01).ceil() as u32).clamp(1, MAX_TAPS)
}

/// Parameters of one rendered frame.
pub struct Frame {
    pub client: (u32, u32),
    pub stage_to_client: Affine,
    /// Visible content in stage texel coordinates.
    pub content: Rect,
    pub background: [u8; 3],
}

/// Renders `img` into a tightly packed BGRA frame.
pub fn render(img: &Image, f: &Frame) -> Vec<u8> {
    let (cw, ch) = (f.client.0 as usize, f.client.1 as usize);
    let mut out = vec![0u8; cw * ch * 4];
    let bg = f.background.map(|c| c as f32 / 255.0);
    let inv = f
        .stage_to_client
        .inverse()
        .expect("invertible view transform");
    let footprint = texels_per_pixel(&f.stage_to_client);
    let n = taps(footprint);
    let (sw, sh) = (img.width as f64, img.height as f64);
    let content = f.content.intersect(&Rect::new(0.0, 0.0, sw, sh));
    for y in 0..ch {
        for x in 0..cw {
            let l = inv.apply((x as f64 + 0.5, y as f64 + 0.5));
            let mut rgb = bg;
            if let Some(c) = content
                && c.contains(l)
            {
                let mut acc = [0f32; 4];
                for j in 0..n {
                    for i in 0..n {
                        let ox = ((i as f64 + 0.5) / n as f64 - 0.5) * footprint;
                        let oy = ((j as f64 + 0.5) / n as f64 - 0.5) * footprint;
                        let s = sample(img, (l.0 + ox) as f32, (l.1 + oy) as f32);
                        for k in 0..4 {
                            acc[k] += s[k];
                        }
                    }
                }
                let nn = (n * n) as f32;
                let a = acc[3] / nn;
                for k in 0..3 {
                    rgb[k] = acc[k] / nn * a + bg[k] * (1.0 - a);
                }
            }
            let o = (y * cw + x) * 4;
            out[o] = to_u8(rgb[2]);
            out[o + 1] = to_u8(rgb[1]);
            out[o + 2] = to_u8(rgb[0]);
            out[o + 3] = 255;
        }
    }
    out
}

/// Bilinear sample of channel `c` of a plane with `bpp` interleaved
/// channels at texel coordinates (`x`, `y`) of that plane; texel centres
/// at `i + 0.5`, clamped at the edges like the GPU sampler.
fn bilinear(p: &Plane, bpp: usize, c: usize, x: f32, y: f32) -> f32 {
    let (w, h) = (p.width as f32, p.height as f32);
    let tx = x - 0.5;
    let ty = y - 0.5;
    let x0 = tx.floor();
    let y0 = ty.floor();
    let (fx, fy) = (tx - x0, ty - y0);
    let get = |xi: f32, yi: f32| {
        let xi = xi.clamp(0.0, w - 1.0) as usize;
        let yi = yi.clamp(0.0, h - 1.0) as u32;
        p.row(yi)[xi * bpp + c] as f32 / 255.0
    };
    let top = get(x0, y0) * (1.0 - fx) + get(x0 + 1.0, y0) * fx;
    let bot = get(x0, y0 + 1.0) * (1.0 - fx) + get(x0 + 1.0, y0 + 1.0) * fx;
    top * (1.0 - fy) + bot * fy
}

/// RGBA (0..1) at stage texel coordinates.
fn sample(img: &Image, x: f32, y: f32) -> [f32; 4] {
    match &img.pixels {
        Pixels::Packed {
            plane,
            format,
            alpha,
        } => {
            let a = |v: f32| if *alpha == Alpha::Opaque { 1.0 } else { v };
            match format {
                PackedFormat::Rgba8 => [
                    bilinear(plane, 4, 0, x, y),
                    bilinear(plane, 4, 1, x, y),
                    bilinear(plane, 4, 2, x, y),
                    a(bilinear(plane, 4, 3, x, y)),
                ],
                PackedFormat::Bgra8 => [
                    bilinear(plane, 4, 2, x, y),
                    bilinear(plane, 4, 1, x, y),
                    bilinear(plane, 4, 0, x, y),
                    a(bilinear(plane, 4, 3, x, y)),
                ],
                PackedFormat::Gray8 => {
                    let g = bilinear(plane, 1, 0, x, y);
                    [g, g, g, 1.0]
                }
            }
        }
        Pixels::Ycc { y: yp, cb, cr, sub } => {
            let luma = bilinear(yp, 1, 0, x, y);
            let (cx, cy) = (x / sub.h as f32, y / sub.v as f32);
            let rgb = ycc_to_rgb(luma, bilinear(cb, 1, 0, cx, cy), bilinear(cr, 1, 0, cx, cy));
            [
                rgb[0].clamp(0.0, 1.0),
                rgb[1].clamp(0.0, 1.0),
                rgb[2].clamp(0.0, 1.0),
                1.0,
            ]
        }
    }
}

/// Peak signal-to-noise ratio of two equally sized byte buffers (dB).
pub fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mse = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as f64 - *y as f64).powi(2))
        .sum::<f64>()
        / a.len().max(1) as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// Largest absolute per-byte difference.
pub fn max_diff(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Orientation;
    use crate::layout::{
        ViewMode, ViewOptions, ViewState, compute, content_in_stage, stage_to_client,
    };

    fn solid(w: u32, h: u32, bgra: [u8; 4], alpha: Alpha) -> Image {
        let mut plane = Plane::new(w, h, 4);
        for px in plane.data.as_chunks_mut::<4>().0.iter_mut() {
            px.copy_from_slice(&bgra);
        }
        Image {
            pixels: Pixels::Packed {
                plane,
                format: PackedFormat::Bgra8,
                alpha,
            },
            width: w,
            height: h,
            full_width: w,
            full_height: h,
            region: None,
        }
    }

    fn frame_for(img: &Image, client: (u32, u32), o: Orientation, opts: ViewOptions) -> Frame {
        let full = (img.full_width, img.full_height);
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
        }
    }

    #[test]
    fn letterboxing_and_colors() {
        let img = solid(4, 2, [200, 100, 50, 255], Alpha::Opaque);
        let opts = ViewOptions {
            mode: ViewMode::FitImageToWindow,
            no_upscale: false,
        };
        let out = render(&img, &frame_for(&img, (8, 8), Orientation::NORMAL, opts));
        // Image occupies rows 2..6 (scaled 2×), background elsewhere.
        assert_eq!(&out[..4], &[30, 20, 10, 255]);
        let mid = (4 * 8 + 4) * 4;
        assert_eq!(&out[mid..mid + 4], &[200, 100, 50, 255]);
    }

    #[test]
    fn rotation_swaps_axes() {
        let img = solid(4, 2, [0, 0, 255, 255], Alpha::Opaque);
        let opts = ViewOptions {
            mode: ViewMode::ActualSize,
            no_upscale: true,
        };
        let out = render(
            &img,
            &frame_for(&img, (4, 4), Orientation::from_exif(6), opts),
        );
        // Rotated image is 2 wide, 4 tall, centred horizontally.
        let px = |x: usize, y: usize| out[(y * 4 + x) * 4 + 2];
        assert_eq!((px(0, 0), px(1, 0), px(2, 3), px(3, 3)), (10, 255, 255, 10));
    }

    #[test]
    fn alpha_composites_over_background() {
        let img = solid(2, 2, [255, 255, 255, 128], Alpha::Straight);
        let opts = ViewOptions {
            mode: ViewMode::ActualSize,
            no_upscale: true,
        };
        let out = render(&img, &frame_for(&img, (2, 2), Orientation::NORMAL, opts));
        let a = 128.0 / 255.0;
        assert_eq!(out[2], to_u8(a + 10.0 / 255.0 * (1.0 - a)));
    }

    #[test]
    fn taps_follow_minification() {
        assert_eq!(taps(0.5), 1);
        assert_eq!(taps(1.0), 1);
        assert_eq!(taps(1.5), 2);
        assert_eq!(taps(3.2), 4);
        assert_eq!(taps(10.0), 4);
        assert_eq!(psnr(&[1, 2], &[1, 2]), f64::INFINITY);
        assert_eq!(max_diff(&[1, 9], &[3, 2]), 7);
    }
}
