//! Hot pixel loops, compiled once per x86-64 SIMD level and picked at run
//! time.
//!
//! Each loop is written once as plain Rust; the compiler auto-vectorizes it
//! for the instruction set of every level. Windows 11 also runs on CPUs
//! without AVX (Pentium/Celeron), so nothing above the build baseline may be
//! assumed: callers pass a [`Level`], and a level the CPU lacks falls back
//! to the baseline code.

use crate::pixel::{ChromaSub, Plane, sample_plane, to_u8, ycc_to_rgb};

/// x86-64 SIMD levels a kernel is compiled for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    /// The build target's features (x86-64-v2 on Windows); the only level
    /// on other architectures.
    Baseline,
    Sse42,
    Avx,
    /// AVX2 + FMA.
    Avx2,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Baseline, Level::Sse42, Level::Avx, Level::Avx2];

    pub fn name(self) -> &'static str {
        match self {
            Level::Baseline => "baseline",
            Level::Sse42 => "sse4.2",
            Level::Avx => "avx",
            Level::Avx2 => "avx2+fma",
        }
    }

    /// Whether this CPU can run the level.
    pub fn is_available(self) -> bool {
        #[cfg(target_arch = "x86_64")]
        return match self {
            Level::Baseline => true,
            Level::Sse42 => std::is_x86_feature_detected!("sse4.2"),
            Level::Avx => std::is_x86_feature_detected!("avx"),
            Level::Avx2 => {
                std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
            }
        };
        #[cfg(not(target_arch = "x86_64"))]
        return self == Level::Baseline;
    }

    /// All levels this CPU can run, lowest first.
    pub fn available() -> Vec<Level> {
        Level::ALL
            .into_iter()
            .filter(|l| l.is_available())
            .collect()
    }
}

/// The CPU's brand string (cpuid leaves 0x8000_0002..4).
pub fn cpu_name() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        if __cpuid(0x8000_0000).eax >= 0x8000_0004 {
            let bytes: Vec<u8> = (0x8000_0002..=0x8000_0004u32)
                .flat_map(|leaf| {
                    let r = __cpuid(leaf);
                    [r.eax, r.ebx, r.ecx, r.edx]
                })
                .flat_map(u32::to_le_bytes)
                .collect();
            return String::from_utf8_lossy(&bytes)
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_string();
        }
    }
    "unknown".into()
}

/// SIMD features this CPU has.
pub fn cpu_features() -> Vec<&'static str> {
    #[cfg(target_arch = "x86_64")]
    return [
        ("sse4.2", std::is_x86_feature_detected!("sse4.2")),
        ("avx", std::is_x86_feature_detected!("avx")),
        ("avx2", std::is_x86_feature_detected!("avx2")),
        ("fma", std::is_x86_feature_detected!("fma")),
        ("avx512f", std::is_x86_feature_detected!("avx512f")),
    ]
    .into_iter()
    .filter_map(|(name, has)| has.then_some(name))
    .collect();
    #[cfg(not(target_arch = "x86_64"))]
    return Vec::new();
}

/// SIMD features this binary was compiled to assume.
pub fn build_features() -> Vec<&'static str> {
    [
        ("sse4.2", cfg!(target_feature = "sse4.2")),
        ("avx", cfg!(target_feature = "avx")),
        ("avx2", cfg!(target_feature = "avx2")),
        ("fma", cfg!(target_feature = "fma")),
        ("avx512f", cfg!(target_feature = "avx512f")),
    ]
    .into_iter()
    .filter_map(|(name, has)| has.then_some(name))
    .collect()
}

/// Defines `pub fn name(level: Level, args…)` for each body: the body is
/// inlined into one wrapper per level, each compiled with that level's
/// target features.
macro_rules! kernels {
    ($(
        $(#[$meta:meta])*
        pub fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $body:block
    )*) => {$(
        $(#[$meta])*
        pub fn $name(level: Level, $($arg: $ty),*) {
            #[inline(always)]
            fn imp($($arg: $ty),*) $body

            #[cfg(target_arch = "x86_64")]
            {
                #[target_feature(enable = "sse4.2")]
                fn sse42($($arg: $ty),*) { imp($($arg),*) }
                #[target_feature(enable = "avx")]
                fn avx($($arg: $ty),*) { imp($($arg),*) }
                #[target_feature(enable = "avx2,fma")]
                fn avx2($($arg: $ty),*) { imp($($arg),*) }

                // SAFETY: each wrapper runs only after `is_available` has
                // confirmed the CPU supports its target features.
                match level {
                    Level::Sse42 if level.is_available() => unsafe { sse42($($arg),*) },
                    Level::Avx if level.is_available() => unsafe { avx($($arg),*) },
                    Level::Avx2 if level.is_available() => unsafe { avx2($($arg),*) },
                    _ => imp($($arg),*),
                }
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = level;
                imp($($arg),*)
            }
        }
    )*};
}

kernels! {
    /// RGB → RGBA with opaque alpha, as many pixels as both slices hold.
    pub fn rgb_to_rgba(src: &[u8], dst: &mut [u8]) {
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<3>().0) {
            *d = [s[0], s[1], s[2], 255];
        }
    }

    /// Gray + alpha → RGBA, as many pixels as both slices hold.
    pub fn gray_alpha_to_rgba(src: &[u8], dst: &mut [u8]) {
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<2>().0) {
            *d = [s[0], s[0], s[0], s[1]];
        }
    }

    /// RGBA → BGRA, as many pixels as both slices hold.
    pub fn rgba_to_bgra(src: &[u8], dst: &mut [u8]) {
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
            *d = [s[2], s[1], s[0], s[3]];
        }
    }

    /// Row `y` of an image `dst.len() / 4` pixels wide and `height` rows
    /// high from Y, Cb, Cr planes to opaque BGRA, sampled and converted
    /// exactly like the shader.
    pub fn ycc_to_bgra_row(planes: [&Plane; 3], sub: ChromaSub, height: u32, y: u32, dst: &mut [u8]) {
        let [luma, cb, cr] = planes;
        let w = dst.len() / 4;
        let scale = sub.uv_scale((luma.width, luma.height), (cb.width, cb.height));
        let v = (y as f32 + 0.5) / height as f32;
        let row = luma.row(y);
        for (x, d) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let u = (x as f32 + 0.5) / w as f32;
            let b = sample_plane(cb, u * scale.0, v * scale.1);
            let r = sample_plane(cr, u * scale.0, v * scale.1);
            let rgb = ycc_to_rgb(row[x] as f32 / 255.0, b, r);
            *d = [to_u8(rgb[2]), to_u8(rgb[1]), to_u8(rgb[0]), 255];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg;

    /// Runs `f` at every available level and checks all outputs equal the
    /// baseline's.
    fn same_at_all_levels(f: impl Fn(Level) -> Vec<u8>) {
        let base = f(Level::Baseline);
        for level in Level::available() {
            assert!(f(level) == base, "{} differs from baseline", level.name());
        }
    }

    // Odd widths exercise the scalar tails after the vector loops.
    const W: u32 = 1001;
    const H: u32 = 7;

    #[test]
    fn baseline_is_always_available() {
        assert!(Level::Baseline.is_available());
        assert_eq!(Level::available()[0], Level::Baseline);
    }

    #[test]
    fn packed_kernels_match_across_levels() {
        let rgb = testimg::rgb(W, H);
        let rgba = testimg::rgba(W, H);
        let ga: Vec<u8> = testimg::gray(W, H)
            .iter()
            .zip(&rgba)
            .flat_map(|(&g, &a)| [g, a])
            .collect();
        let n = (W * H * 4) as usize;
        same_at_all_levels(|l| {
            let mut out = vec![0; n];
            rgb_to_rgba(l, &rgb, &mut out);
            out
        });
        same_at_all_levels(|l| {
            let mut out = vec![0; n];
            gray_alpha_to_rgba(l, &ga, &mut out);
            out
        });
        same_at_all_levels(|l| {
            let mut out = vec![0; n];
            rgba_to_bgra(l, &rgba, &mut out);
            out
        });

        let mut out = vec![0; n];
        rgb_to_rgba(Level::Baseline, &rgb, &mut out);
        assert_eq!(
            out[..8],
            [rgb[0], rgb[1], rgb[2], 255, rgb[3], rgb[4], rgb[5], 255]
        );
        rgba_to_bgra(Level::Baseline, &rgba, &mut out);
        assert_eq!(out[..4], [rgba[2], rgba[1], rgba[0], rgba[3]]);
        gray_alpha_to_rgba(Level::Baseline, &ga, &mut out);
        assert_eq!(out[..4], [ga[0], ga[0], ga[0], ga[1]]);
    }

    #[test]
    fn ycc_kernel_matches_across_levels() {
        let plane = |w: u32, h: u32, src: &[u8]| {
            let mut p = Plane::new(w, h, 1);
            p.data.copy_from_slice(src);
            p
        };
        let (cw, ch) = (W.div_ceil(2), H.div_ceil(2));
        let rgb = testimg::rgb(cw, ch);
        let luma = plane(W, H, &testimg::gray(W, H));
        let cb = plane(cw, ch, &rgb.iter().step_by(3).copied().collect::<Vec<_>>());
        let cr = plane(
            cw,
            ch,
            &rgb.iter().skip(2).step_by(3).copied().collect::<Vec<_>>(),
        );
        let sub = ChromaSub { h: 2, v: 2 };
        same_at_all_levels(|l| {
            let mut out = vec![0; (W * H * 4) as usize];
            let n = W as usize * 4;
            for y in 0..H {
                let row = &mut out[y as usize * n..(y as usize + 1) * n];
                ycc_to_bgra_row(l, [&luma, &cb, &cr], sub, H, y, row);
            }
            out
        });
    }
}
