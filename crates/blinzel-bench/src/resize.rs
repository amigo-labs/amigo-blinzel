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

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// `blinzel-bench resize [--runs N] [--quick]`
pub fn resize(args: &[String]) -> Result<(), String> {
    let runs: usize = option(args, "--runs")
        .map_or(Ok(5), str::parse)
        .map_err(|e| format!("{e}"))?;
    let quick = flag(args, "--quick");
    let cases: &[(&str, u32, u32, u32, u32, bool)] = &[
        ("24 MP -> 1600", 6000, 4000, 1600, 1067, false),
        ("24 MP -> 400", 6000, 4000, 400, 267, false),
        ("12 MP -> 2048 alpha", 4000, 3000, 2048, 1536, true),
        ("2 MP -> 4 MP up", 1600, 1200, 2400, 1800, false),
    ];
    println!(
        "{:<22} {:>10} {:>10} {:>8}",
        "case", "own ms", "fir ms", "max diff"
    );
    for &(name, sw, sh, dw, dh, alpha) in cases {
        let (sw, sh, dw, dh) = if quick {
            (sw / 4, sh / 4, dw / 4, dh / 4)
        } else {
            (sw, sh, dw, dh)
        };
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
        let diff = a
            .iter()
            .zip(&b)
            .enumerate()
            .filter(|(i, _)| alpha || i % 4 != 3)
            .map(|(_, (x, y))| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        println!(
            "{name:<22} {:>10.1} {:>10.1} {diff:>8}",
            median(own_t),
            median(fir_t)
        );
    }
    Ok(())
}
