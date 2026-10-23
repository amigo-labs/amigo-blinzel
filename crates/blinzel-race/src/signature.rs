//! Colour signatures: every test image is a photo-like texture around one
//! of a few strongly different base colours, so a coarse grid of screen
//! samples tells which image a viewer shows, wherever its window is and
//! however it scales.

/// Base colours: the six coloured corners of the RGB cube, pairwise 200
/// apart in at least one channel (black and white would be confused with
/// desktops and dialogs). Image `i` uses `COLORS[i % COLORS.len()]`, so
/// consecutive images always differ.
pub const COLORS: [[u8; 3]; 6] = [
    [230, 30, 30],
    [30, 230, 30],
    [30, 30, 230],
    [230, 230, 30],
    [230, 30, 230],
    [30, 230, 230],
];

/// Per-channel distance under which a sample counts as the base colour:
/// texture amplitude plus JPEG and scaling error, well below the gaps
/// between colours.
#[cfg_attr(not(windows), allow(dead_code))] // used by the Windows capture
pub const TOLERANCE: i32 = 56;

/// Grid samples that must match before a signature counts as visible.
#[cfg_attr(not(windows), allow(dead_code))]
pub const MIN_MATCHES: usize = 12;

/// Which signature, if any, a set of RGB samples shows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn classify(samples: &[[u8; 3]]) -> Option<usize> {
    let mut counts = [0usize; COLORS.len()];
    for s in samples {
        if let Some(i) = COLORS
            .iter()
            .position(|c| (0..3).all(|k| (s[k] as i32 - c[k] as i32).abs() <= TOLERANCE))
        {
            counts[i] += 1;
        }
    }
    let (best, &n) = counts.iter().enumerate().max_by_key(|&(_, n)| *n)?;
    (n >= MIN_MATCHES).then_some(best)
}

/// The image's pixels: base colour `i` modulated by a photo texture (grain,
/// gradients, hard edges), so decoders do realistic work.
pub fn image(i: usize, w: u32, h: u32) -> Vec<u8> {
    let base = COLORS[i % COLORS.len()];
    let mut px = blinzel_core::testimg::rgb(w, h);
    // Sensor-like noise per channel: real 24 MP photos are 8–12 MB as
    // JPEG, and the entropy decoder should work as hard as on those.
    let mut rng = 0x2545_F491_4F6C_DD1Du64 ^ (i as u64) << 32;
    for p in px.as_chunks_mut::<3>().0 {
        // Texture luminance around its mean, scaled into ±32.
        let lum = (p[0] as i32 * 77 + p[1] as i32 * 150 + p[2] as i32 * 29) >> 8;
        let d = (lum - 128) / 4;
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        for k in 0..3 {
            let noise = ((rng >> (k * 8)) & 31) as i32 - 16;
            p[k] = (base[k] as i32 + d + noise).clamp(0, 255) as u8;
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_unambiguous() {
        for (i, a) in COLORS.iter().enumerate() {
            for b in &COLORS[i + 1..] {
                let far = (0..3).any(|k| (a[k] as i32 - b[k] as i32).abs() > 2 * TOLERANCE);
                assert!(far, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn classifies_images_and_ignores_the_rest() {
        for i in 0..COLORS.len() {
            let px = image(i, 64, 48);
            let samples: Vec<[u8; 3]> = px.as_chunks::<3>().0.iter().step_by(97).copied().collect();
            assert_eq!(classify(&samples), Some(i));
        }
        // A dark desktop or another window shows nothing.
        assert_eq!(classify(&[[30, 30, 30]; 100]), None);
        // Too few matching samples (a thumbnail somewhere) do not count.
        let mut few = vec![[0u8, 0, 0]; 100];
        few[..MIN_MATCHES - 1].fill(COLORS[2]);
        assert_eq!(classify(&few), None);
    }
}
