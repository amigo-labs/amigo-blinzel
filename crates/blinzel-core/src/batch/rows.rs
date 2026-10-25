//! Rows of a decoded image as BGRA8, converted as the resampler asks for
//! them: no full-size BGRA copy, and YCbCr in fixed point instead of the
//! shader-exact float path of [`Image::to_bgra`] (at most one level
//! apart, a tenth of the time).

use crate::kernels::{self, Level};
use crate::pixel::{Alpha, ChromaSub, Image, PackedFormat, Pixels, Plane};

/// Chroma sample position in 1/256 texels: left texel and weight of the
/// right one, clamped at the edges like the GPU sampler.
fn positions(n: u32, luma: u32, scale: f32, plane: u32) -> Vec<(u32, u32, u32)> {
    (0..n)
        .map(|i| {
            let t = (i as f32 + 0.5) / luma as f32 * scale * plane as f32 - 0.5;
            let t0 = t.floor();
            let f = ((t - t0) * 256.0).round() as u32;
            let last = plane as f32 - 1.0;
            let a = t0.clamp(0.0, last) as u32;
            let b = (t0 + 1.0).clamp(0.0, last) as u32;
            (a, b, f.min(256))
        })
        .collect()
}

pub struct Rows<'a> {
    img: &'a Image,
    /// For YCbCr: per luma column, per luma row.
    cols: Vec<(u32, u32, u32)>,
    lines: Vec<(u32, u32, u32)>,
}

impl<'a> Rows<'a> {
    pub fn new(img: &'a Image) -> Rows<'a> {
        let (mut cols, mut lines) = (Vec::new(), Vec::new());
        if let Pixels::Ycc { cb, sub, .. } = &img.pixels {
            let scale = sub.uv_scale((img.width, img.height), (cb.width, cb.height));
            cols = positions(img.width, img.width, scale.0, cb.width);
            lines = positions(img.height, img.height, scale.1, cb.height);
        }
        Rows { img, cols, lines }
    }

    /// Whether every pixel is opaque without looking at them.
    pub fn opaque(&self) -> bool {
        match &self.img.pixels {
            Pixels::Packed { format, alpha, .. } => {
                *alpha == Alpha::Opaque || *format == PackedFormat::Gray8
            }
            Pixels::Ycc { .. } => true,
        }
    }

    /// Pixels `x0..x0 + dst.len() / 4` of row `y` as BGRA.
    pub fn row(&self, y: u32, x0: u32, dst: &mut [u8]) {
        let n = dst.len() / 4;
        let x0 = x0 as usize;
        match &self.img.pixels {
            Pixels::Packed {
                plane,
                format,
                alpha,
            } => {
                let src = plane.row(y);
                match format {
                    PackedFormat::Bgra8 => dst.copy_from_slice(&src[x0 * 4..(x0 + n) * 4]),
                    PackedFormat::Rgba8 => {
                        kernels::rgba_to_bgra(Level::Baseline, &src[x0 * 4..(x0 + n) * 4], dst)
                    }
                    PackedFormat::Gray8 => {
                        for (d, &g) in dst.as_chunks_mut::<4>().0.iter_mut().zip(&src[x0..x0 + n]) {
                            *d = [g, g, g, 255];
                        }
                    }
                }
                if *alpha == Alpha::Opaque {
                    dst.as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .for_each(|d| d[3] = 255);
                }
            }
            Pixels::Ycc {
                y: luma,
                cb,
                cr,
                sub,
            } => {
                self.ycc_row([luma, cb, cr], *sub, y, x0, dst);
            }
        }
    }

    fn ycc_row(
        &self,
        [luma, cb, cr]: [&Plane; 3],
        _sub: ChromaSub,
        y: u32,
        x0: usize,
        dst: &mut [u8],
    ) {
        let (r0, r1, fy) = self.lines[y as usize];
        let blend = |p: &Plane| -> Vec<u16> {
            let (a, b) = (p.row(r0), p.row(r1));
            a.iter()
                .zip(b)
                .take(p.width as usize)
                .map(|(&a, &b)| (a as u32 * (256 - fy) + b as u32 * fy) as u16)
                .collect()
        };
        // Vertically blended chroma rows, in 1/256 levels.
        let (bc, rc) = (blend(cb), blend(cr));
        let ys = &luma.row(y)[x0..];
        for ((d, &l), &(a, b, fx)) in dst
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(ys)
            .zip(&self.cols[x0..])
        {
            // Chroma in 1/256 levels, centred on zero; the BT.601 full-range
            // factors are 16.16 fixed point, so the sums are in 2^-24.
            let h = |c: &[u16]| {
                ((c[a as usize] as i64 * (256 - fx) as i64
                    + c[b as usize] as i64 * fx as i64
                    + 128)
                    >> 8)
                    - 128 * 256
            };
            let (cb, cr) = (h(&bc), h(&rc));
            let base = ((l as i64) << 24) + (1 << 23);
            let c = |v: i64| (v >> 24).clamp(0, 255) as u8;
            let r = c(base + 91_881 * cr);
            let g = c(base - 22_554 * cb - 46_802 * cr);
            let bl = c(base + 116_130 * cb);
            *d = [bl, g, r, 255];
        }
    }
}

#[cfg(all(test, feature = "turbojpeg"))]
mod tests {
    use super::*;
    use crate::decode::{self, Request};

    #[test]
    fn ycc_rows_match_the_exact_path() {
        // 4:2:0 and 4:4:4 JPEGs with odd sizes.
        for sub in [420, 444, 422] {
            let (w, h) = (101u32, 67u32);
            let mut px = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    px.extend([(x * 2) as u8, (y * 3) as u8, ((x * y) % 251) as u8, 255]);
                }
            }
            let jpeg = decode::jpeg::encode_bgra(&px, w, h, 90, sub, None).unwrap();
            let img = decode::decode(&jpeg, Request::Full).unwrap();
            assert!(matches!(img.pixels, Pixels::Ycc { .. }));
            let exact = img.to_bgra();
            let rows = Rows::new(&img);
            assert!(rows.opaque());
            let mut row = vec![0u8; (w - 7) as usize * 4];
            for y in 0..h {
                rows.row(y, 7, &mut row);
                let want = &exact[((y * w + 7) * 4) as usize..((y + 1) * w * 4) as usize];
                for (i, (a, b)) in row.iter().zip(want).enumerate() {
                    assert!(
                        a.abs_diff(*b) <= 1,
                        "{sub}: ({}, {y}) c{}: {a} vs {b}",
                        i / 4 + 7,
                        i % 4
                    );
                }
            }
        }
    }
}
