//! Unsharp mask after downscaling: `out = in + amount · (in − blur(in))`,
//! with a Gaussian blur of the given radius (sigma, in output pixels).
//! Works on the colour channels in sRGB, like the usual web export
//! sharpening; alpha is left alone.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sharpen {
    /// Strength, 0 (off) to about 2.
    pub amount: f32,
    /// Gaussian sigma in pixels, 0.3 to 3.
    pub radius: f32,
}

impl Default for Sharpen {
    fn default() -> Self {
        Sharpen {
            amount: 0.5,
            radius: 0.8,
        }
    }
}

fn kernel(sigma: f32) -> Vec<f32> {
    let sigma = sigma.clamp(0.3, 3.0);
    let r = (sigma * 3.0).ceil() as i32;
    let mut k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// Sharpens `px` (`w`×`h`, BGRA8) in place.
pub fn apply(px: &mut [u8], w: u32, h: u32, s: Sharpen) {
    if s.amount <= 0.0 || w == 0 || h == 0 {
        return;
    }
    let (w, h) = (w as usize, h as usize);
    let k = kernel(s.radius);
    let r = k.len() / 2;
    // Horizontal blur of the three colour channels.
    let mut tmp = vec![[0f32; 3]; w * h];
    for y in 0..h {
        let row = &px[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            let mut acc = [0f32; 3];
            for (i, &kv) in k.iter().enumerate() {
                let sx = (x + i).saturating_sub(r).min(w - 1);
                for c in 0..3 {
                    acc[c] += row[sx * 4 + c] as f32 * kv;
                }
            }
            tmp[y * w + x] = acc;
        }
    }
    // Vertical blur, then the mask.
    let mut col = vec![[0f32; 3]; h];
    for x in 0..w {
        for (y, c) in col.iter_mut().enumerate() {
            let mut acc = [0f32; 3];
            for (i, &kv) in k.iter().enumerate() {
                let sy = (y + i).saturating_sub(r).min(h - 1);
                let t = tmp[sy * w + x];
                for ch in 0..3 {
                    acc[ch] += t[ch] * kv;
                }
            }
            *c = acc;
        }
        for (y, blur) in col.iter().enumerate() {
            let p = &mut px[(y * w + x) * 4..(y * w + x) * 4 + 3];
            for (v, b) in p.iter_mut().zip(blur) {
                let o = *v as f32;
                *v = (o + s.amount * (o - b)).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_areas_are_unchanged_and_edges_gain_contrast() {
        let (w, h) = (20u32, 4u32);
        let mut px = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                let v = if x < 10 { 60 } else { 200 };
                px.extend([v, v, v, 128]);
            }
        }
        let before = px.clone();
        apply(&mut px, w, h, Sharpen::default());
        let at = |p: &[u8], x: usize| p[(w as usize + x) * 4];
        assert_eq!(at(&px, 0), 60);
        assert_eq!(at(&px, 19), 200);
        assert!(at(&px, 9) < 60, "dark side darker: {}", at(&px, 9));
        assert!(at(&px, 10) > 200, "light side lighter: {}", at(&px, 10));
        // Alpha untouched.
        assert!(
            px.iter()
                .zip(&before)
                .skip(3)
                .step_by(4)
                .all(|(a, b)| a == b)
        );
    }

    #[test]
    fn zero_amount_is_a_no_op() {
        let mut px = vec![1, 2, 3, 4, 200, 100, 50, 255];
        let before = px.clone();
        apply(
            &mut px,
            2,
            1,
            Sharpen {
                amount: 0.0,
                radius: 1.0,
            },
        );
        assert_eq!(px, before);
    }
}
