//! Colour transforms through Little CMS (every ICC profile kind, like the
//! Windows Color System there): sampled into the UI's 3D LUT.

use blinzel_core::icc::Profile;
use blinzel_core::lut::{self, Lut3d};
use lcms2::{Intent, PixelFormat, Transform};

fn open(p: Option<&Profile>) -> Option<lcms2::Profile> {
    match p {
        Some(p) => lcms2::Profile::new_icc(&p.bytes).ok(),
        None => Some(lcms2::Profile::new_srgb()),
    }
}

/// The transform from `src` to `dst` (`None`: sRGB) sampled as a 3D LUT,
/// relative colorimetric.
pub fn build_lut(src: Option<&Profile>, dst: Option<&Profile>) -> Option<Lut3d> {
    let (s, d) = (open(src)?, open(dst)?);
    let t: Transform<[u16; 3], [u16; 3]> = Transform::new(
        &s,
        PixelFormat::RGB_16,
        &d,
        PixelFormat::RGB_16,
        Intent::RelativeColorimetric,
    )
    .ok()?;
    let n = lut::SIZE;
    let step = 65535.0 / (n - 1) as f64;
    let grid: Vec<[u16; 3]> = (0..n * n * n)
        .map(|i| [i % n, i / n % n, i / (n * n)].map(|v| (v as f64 * step).round() as u16))
        .collect();
    let mut out = vec![[0u16; 3]; grid.len()];
    t.transform_pixels(&grid, &mut out);
    Lut3d::from_nodes(n, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_core::icc::{Trc, colorants, matrix_profile};

    fn to8(v: f32) -> i32 {
        (v * 255.0).round() as i32
    }

    #[test]
    fn srgb_to_srgb_is_identity_and_p3_red_clips() {
        let srgb = Profile::new(matrix_profile(colorants::SRGB, Trc::Srgb, "sRGB test"));
        let lut = build_lut(Some(&srgb), None).expect("transform");
        for c in [[0.0f32, 0.0, 0.0], [1.0, 1.0, 1.0], [0.2, 0.5, 0.8]] {
            let out = lut.apply(c);
            for k in 0..3 {
                assert!((to8(out[k]) - to8(c[k])).abs() <= 1, "{c:?} -> {out:?}");
            }
        }
        let p3 = Profile::new(matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3 test"));
        let lut = build_lut(Some(&p3), None).expect("transform");
        let red = lut.apply([1.0, 0.0, 0.0]);
        assert!(
            to8(red[0]) >= 250 && to8(red[1]) <= 5 && to8(red[2]) <= 5,
            "{red:?}"
        );
        for v in [0.0f32, 0.25, 0.5, 1.0] {
            let out = lut.apply([v; 3]);
            for k in 0..3 {
                assert!((to8(out[k]) - to8(v)).abs() <= 1, "{v} -> {out:?}");
            }
        }
    }
}
