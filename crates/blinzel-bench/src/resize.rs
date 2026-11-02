//! Gate for the batch resampler: our Lanczos3 in linear light against
//! `fast_image_resize` doing the same (sRGB→linear U16 mapping, Lanczos3,
//! alpha premultiplied), one thread each.

use std::time::Instant;

use blinzel_core::batch::resample;
use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::{flag, option};

fn photo(w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let mut s = 0x9e37_79b9u32;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = (s & 15) as u8;
            out.extend([
                (x * 255 / w) as u8 ^ n,
                (y * 255 / h) as u8 ^ n,
                ((x + y) & 255) as u8,
                if alpha { (x & 255) as u8 } else { 255 },
            ]);
        }
    }
    out
}

fn fir(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mapper = fast_image_resize::create_srgb_mapper();
    let s8 = Image::from_vec_u8(sw, sh, src.to_vec(), PixelType::U8x4).unwrap();
    let mut s16 = Image::new(sw, sh, PixelType::U16x4);
    mapper.forward_map(&s8, &mut s16).unwrap();
    let mut d16 = Image::new(dw, dh, PixelType::U16x4);
    Resizer::new()
        .resize(
            &s16,
            &mut d16,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
        )
        .unwrap();
    let mut d8 = Image::new(dw, dh, PixelType::U8x4);
    mapper.backward_map(&d16, &mut d8).unwrap();
    d8.into_vec()
}

/// Name, source and target size, alpha: at a quarter of every dimension
/// with `quick`.
type Case = (&'static str, u32, u32, u32, u32, bool);

fn cases(quick: bool) -> Vec<Case> {
    let cases: [Case; 4] = [
        ("24 MP -> 1600", 6000, 4000, 1600, 1067, false),
        ("24 MP -> 400", 6000, 4000, 400, 267, false),
        ("12 MP -> 2048 alpha", 4000, 3000, 2048, 1536, true),
        ("2 MP -> 4 MP up", 1600, 1200, 2400, 1800, false),
    ];
    let q = if quick { 4 } else { 1 };
    cases
        .map(|(name, sw, sh, dw, dh, alpha)| (name, sw / q, sh / q, dw / q, dh / q, alpha))
        .into()
}

/// The largest difference of one channel between two RGBA images; alpha
/// counts only where the image has alpha.
fn max_diff(a: &[u8], b: &[u8], alpha: bool) -> u8 {
    a.iter()
        .zip(b)
        .enumerate()
        .filter(|(i, _)| alpha || i % 4 != 3)
        .map(|(_, (x, y))| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// `--runs` (default 5).
fn runs(args: &[String]) -> Result<usize, String> {
    option(args, "--runs")
        .map_or(Ok(5), str::parse)
        .map_err(|e| format!("{e}"))
}

/// `blinzel-bench resize [--runs N] [--quick]`
pub fn resize(args: &[String]) -> Result<(), String> {
    let runs = runs(args)?;
    let quick = flag(args, "--quick");
    println!(
        "{:<22} {:>10} {:>10} {:>8}",
        "case", "own ms", "fir ms", "max diff"
    );
    for (name, sw, sh, dw, dh, alpha) in cases(quick) {
        let src = photo(sw, sh, alpha);
        let mut own_t = Vec::new();
        let mut fir_t = Vec::new();
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for _ in 0..runs {
            let t = Instant::now();
            a = resample::resize(&src, sw, sh, dw, dh);
            own_t.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            b = fir(&src, sw, sh, dw, dh);
            fir_t.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let diff = max_diff(&a, &b, alpha);
        println!(
            "{name:<22} {:>10.1} {:>10.1} {diff:>8}",
            median(own_t),
            median(fir_t)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-13: the four cases, a quarter of every dimension with `--quick`,
    /// and 5 runs by default.
    #[test]
    fn resize_has_four_cases_quartered_when_quick() {
        let full = cases(false);
        assert_eq!(
            full,
            [
                ("24 MP -> 1600", 6000, 4000, 1600, 1067, false),
                ("24 MP -> 400", 6000, 4000, 400, 267, false),
                ("12 MP -> 2048 alpha", 4000, 3000, 2048, 1536, true),
                ("2 MP -> 4 MP up", 1600, 1200, 2400, 1800, false),
            ]
        );
        for (f, q) in full.iter().zip(cases(true)) {
            assert_eq!(q, (f.0, f.1 / 4, f.2 / 4, f.3 / 4, f.4 / 4, f.5));
        }
        assert_eq!(runs(&[]).unwrap(), 5);
        assert_eq!(runs(&["--runs".into(), "2".into()]).unwrap(), 2);
    }

    /// AC-13: the largest channel difference, alpha only with alpha.
    #[test]
    fn the_difference_counts_alpha_only_where_the_image_has_it() {
        let a = [10, 20, 30, 0, 10, 20, 30, 255];
        let b = [12, 20, 25, 200, 10, 21, 30, 255];
        assert_eq!(max_diff(&a, &b, false), 5);
        assert_eq!(max_diff(&a, &b, true), 200);
    }
}
