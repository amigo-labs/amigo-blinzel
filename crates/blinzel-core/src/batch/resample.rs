//! Lanczos3 resampling of BGRA8 (straight alpha) in linear light.
//!
//! Colours are converted to linear light and premultiplied before
//! filtering, so dark edges do not darken and transparent pixels do not
//! bleed into their neighbours. The filter is separable: each band of
//! output rows first filters the source rows it needs horizontally, then
//! filters those vertically, so the intermediate stays small even for
//! 100-megapixel sources.

use std::sync::OnceLock;

/// Output rows per band: bounds the intermediate buffer.
const BAND: usize = 64;

/// Lanczos window (a = 3).
const A: f64 = 3.0;

fn lanczos(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    if x.abs() >= A {
        return 0.0;
    }
    let px = std::f64::consts::PI * x;
    A * px.sin() * (px / A).sin() / (px * px)
}

/// Filter taps of one output coordinate.
struct Taps {
    first: usize,
    weights: Vec<f32>,
}

/// Taps for every output coordinate when `src` samples become `dst`.
fn taps(src: usize, dst: usize) -> Vec<Taps> {
    let scale = src as f64 / dst as f64;
    // Downscaling widens the filter to the source footprint; upscaling
    // keeps it at one source pixel.
    let stretch = scale.max(1.0);
    let radius = A * stretch;
    (0..dst)
        .map(|i| {
            let center = (i as f64 + 0.5) * scale;
            let lo = (center - radius).floor().max(0.0) as usize;
            let hi = ((center + radius).ceil() as usize).min(src);
            let mut w: Vec<f64> = (lo..hi)
                .map(|j| lanczos((j as f64 + 0.5 - center) / stretch))
                .collect();
            let sum: f64 = w.iter().sum();
            if sum.abs() > 1e-12 {
                w.iter_mut().for_each(|v| *v /= sum);
            }
            // Trim zero weights at both ends.
            let start = w.iter().position(|v| *v != 0.0).unwrap_or(0);
            let end = w.iter().rposition(|v| *v != 0.0).map_or(start, |e| e + 1);
            Taps {
                first: lo + start,
                weights: w[start..end].iter().map(|&v| v as f32).collect(),
            }
        })
        .collect()
}

fn srgb_to_linear() -> &'static [f32; 256] {
    static T: OnceLock<[f32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f64 / 255.0;
            (if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }) as f32
        })
    })
}

/// Linear light in steps of 1/LIN_STEPS back to sRGB bytes.
const LIN_STEPS: usize = 1 << 16;

fn linear_to_srgb() -> &'static [u8] {
    static T: OnceLock<Vec<u8>> = OnceLock::new();
    T.get_or_init(|| {
        (0..=LIN_STEPS)
            .map(|i| {
                let l = i as f64 / LIN_STEPS as f64;
                let c = if l <= 0.003_130_8 {
                    l * 12.92
                } else {
                    1.055 * l.powf(1.0 / 2.4) - 0.055
                };
                (c * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect()
    })
}

/// Resamples `src` (`sw`×`sh`, BGRA8, straight alpha) to `dw`×`dh`.
pub fn resize(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    assert_eq!(src.len(), sw as usize * sh as usize * 4);
    if (sw, sh) == (dw, dh) {
        return src.to_vec();
    }
    let opaque = src.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
    let stride = sw as usize * 4;
    resize_rows(sw, sh, dw, dh, opaque, &mut |y, row| {
        row.copy_from_slice(&src[y * stride..(y + 1) * stride])
    })
}

/// Resamples an `sw`×`sh` source whose rows (BGRA8, straight alpha)
/// `source(y, out)` fills on demand, to `dw`×`dh`. `opaque` skips the alpha
/// handling when the caller knows there is none.
pub fn resize_rows(
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    opaque: bool,
    source: &mut dyn FnMut(usize, &mut [u8]),
) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    assert!(sw > 0 && sh > 0 && dw > 0 && dh > 0);
    let to_lin = srgb_to_linear();
    let to_srgb = linear_to_srgb();
    let mut row = vec![0u8; sw * 4];
    let hx = taps(sw, dw);
    let vy = taps(sh, dh);
    let mut out = vec![0u8; dw * dh * 4];
    let mut line = vec![[0f32; 4]; sw];
    let mut band: Vec<[f32; 4]> = Vec::new();

    for (b, out_rows) in out.chunks_mut(BAND * dw * 4).enumerate() {
        let y0 = b * BAND;
        let rows = &vy[y0..y0 + out_rows.len() / (dw * 4)];
        let first = rows.iter().map(|t| t.first).min().unwrap_or(0);
        let last = rows
            .iter()
            .map(|t| t.first + t.weights.len())
            .max()
            .unwrap_or(first);
        band.clear();
        band.resize((last - first) * dw, [0.0; 4]);

        // Horizontal pass over the source rows this band needs.
        for (r, sy) in (first..last).enumerate() {
            source(sy, &mut row);
            for (l, p) in line.iter_mut().zip(row.as_chunks::<4>().0) {
                let a = if opaque { 1.0 } else { p[3] as f32 / 255.0 };
                *l = [
                    to_lin[p[0] as usize] * a,
                    to_lin[p[1] as usize] * a,
                    to_lin[p[2] as usize] * a,
                    a,
                ];
            }
            let dst = &mut band[r * dw..(r + 1) * dw];
            for (d, t) in dst.iter_mut().zip(&hx) {
                let mut acc = [0f32; 4];
                for (s, &w) in line[t.first..].iter().zip(&t.weights) {
                    for c in 0..4 {
                        acc[c] += s[c] * w;
                    }
                }
                *d = acc;
            }
        }

        // Vertical pass, back to sRGB.
        let mut acc = vec![[0f32; 4]; dw];
        for (oy, t) in rows.iter().enumerate() {
            acc.iter_mut().for_each(|a| *a = [0.0; 4]);
            for (k, &w) in t.weights.iter().enumerate() {
                let r = t.first + k - first;
                for (a, s) in acc.iter_mut().zip(&band[r * dw..(r + 1) * dw]) {
                    for c in 0..4 {
                        a[c] += s[c] * w;
                    }
                }
            }
            let dst = &mut out_rows[oy * dw * 4..(oy + 1) * dw * 4];
            for (d, a) in dst.as_chunks_mut::<4>().0.iter_mut().zip(&acc) {
                let alpha = a[3].clamp(0.0, 1.0);
                if alpha <= 0.0 {
                    *d = [0; 4];
                    continue;
                }
                let inv = 1.0 / alpha;
                for c in 0..3 {
                    let l = (a[c] * inv).clamp(0.0, 1.0);
                    d[c] = to_srgb[(l * LIN_STEPS as f32 + 0.5) as usize];
                }
                d[3] = (alpha * 255.0 + 0.5) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, p: [u8; 4]) -> Vec<u8> {
        p.repeat((w * h) as usize)
    }

    #[test]
    fn flat_colours_stay_flat() {
        for (sw, sh, dw, dh) in [(100, 80, 33, 27), (7, 5, 20, 13), (64, 64, 64, 1)] {
            let out = resize(&solid(sw, sh, [10, 128, 250, 255]), sw, sh, dw, dh);
            assert_eq!(out.len(), (dw * dh * 4) as usize);
            for p in out.as_chunks::<4>().0 {
                assert_eq!(*p, [10, 128, 250, 255], "{sw}x{sh} -> {dw}x{dh}");
            }
        }
    }

    #[test]
    fn checkerboard_averages_in_linear_light() {
        // Black/white pixels average to linear 0.5 = sRGB 188, not 128.
        let (w, h) = (64, 64);
        let mut src = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                src.extend([v, v, v, 255]);
            }
        }
        let out = resize(&src, w, h, 8, 8);
        for p in out.as_chunks::<4>().0 {
            assert!(p[0].abs_diff(188) <= 1, "{p:?}");
        }
    }

    #[test]
    fn transparent_pixels_do_not_bleed() {
        // Left half opaque red, right half fully transparent green: the
        // visible colour near the edge stays red.
        let (w, h) = (40, 4);
        let mut src = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                src.extend(if x < w / 2 {
                    [0, 0, 255, 255]
                } else {
                    [0, 255, 0, 0]
                });
            }
        }
        let out = resize(&src, w, h, 10, 1);
        for p in out.as_chunks::<4>().0 {
            if p[3] > 0 {
                assert!(p[1] <= 2 && p[2] >= 250, "{p:?}");
            }
        }
        assert_eq!(out[3], 255);
        assert_eq!(out[9 * 4 + 3], 0);
    }

    #[test]
    fn matches_a_double_precision_reference() {
        // A smooth gradient, downscaled by a non-integer factor.
        let (sw, sh, dw, dh) = (97u32, 61u32, 40u32, 23u32);
        let mut src = Vec::new();
        for y in 0..sh {
            for x in 0..sw {
                src.extend([(x * 2) as u8, (y * 4) as u8, ((x + y) % 256) as u8, 255]);
            }
        }
        let out = resize(&src, sw, sh, dw, dh);
        let lin = |v: u8| {
            let c = v as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let enc = |l: f64| {
            let l = l.clamp(0.0, 1.0);
            let c = if l <= 0.003_130_8 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            (c * 255.0).round()
        };
        let weights = |src: u32, dst: u32, i: u32| {
            let s = src as f64 / dst as f64;
            let c = (i as f64 + 0.5) * s;
            let w: Vec<(usize, f64)> = (0..src as usize)
                .map(|j| (j, lanczos((j as f64 + 0.5 - c) / s.max(1.0))))
                .filter(|(_, w)| *w != 0.0)
                .collect();
            let sum: f64 = w.iter().map(|(_, w)| w).sum();
            w.into_iter().map(|(j, v)| (j, v / sum)).collect::<Vec<_>>()
        };
        for oy in 0..dh {
            let wy = weights(sh, dh, oy);
            for ox in 0..dw {
                let wx = weights(sw, dw, ox);
                for c in 0..3 {
                    let mut v = 0.0;
                    for &(y, a) in &wy {
                        for &(x, b) in &wx {
                            v += a * b * lin(src[(y * sw as usize + x) * 4 + c]);
                        }
                    }
                    let got = out[((oy * dw + ox) * 4) as usize + c] as f64;
                    assert!((got - enc(v)).abs() <= 1.0, "({ox},{oy}) c{c}");
                }
            }
        }
    }
}
