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

/// The id the shell gives a monitor: a hash of its name (the RandR output
/// name on X11), stable across starts.
pub fn monitor_id(name: &str) -> isize {
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    }) as isize
}

/// The profile of `monitor` ([`monitor_id`]) that the desktop's colour
/// manager (colord, KDE) publishes on X11: `_ICC_PROFILE` on the root
/// window for the first monitor, `_ICC_PROFILE_<n>` for the n-th (ICC
/// Profiles in X Specification 0.4). Wayland has no such protocol yet, so
/// there images go to sRGB.
pub fn display_target(monitor: isize) -> Option<Profile> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty()) {
        return None;
    }
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let monitors = conn.randr_get_monitors(root, true).ok()?.reply().ok()?;
    let index = monitors.monitors.iter().position(|m| {
        conn.get_atom_name(m.name)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|n| monitor_id(&String::from_utf8_lossy(&n.name)) == monitor)
    })?;
    let atom_name = match index {
        0 => "_ICC_PROFILE".to_string(),
        n => format!("_ICC_PROFILE_{n}"),
    };
    let atom = conn
        .intern_atom(true, atom_name.as_bytes())
        .ok()?
        .reply()
        .ok()?
        .atom;
    if atom == 0 {
        return None;
    }
    let reply = conn
        .get_property(false, root, atom, AtomEnum::ANY, 0, 64 << 20 >> 2)
        .ok()?
        .reply()
        .ok()?;
    if reply.format != 8 || reply.value.len() < 128 {
        return None;
    }
    let profile = Profile::new(reply.value);
    (!profile.srgb).then_some(profile)
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

    /// Publishes a profile on the root window of the X server in
    /// `BLINZEL_X11_TEST` (an Xvfb display: the test changes it) and reads
    /// it back like the viewer does.
    #[test]
    fn reads_the_x11_display_profile() {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::randr::ConnectionExt as _;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
        use x11rb::wrapper::ConnectionExt as _;

        let Some(display) = std::env::var_os("BLINZEL_X11_TEST") else {
            return;
        };
        let display = display.to_string_lossy().into_owned();
        let (conn, screen) = x11rb::connect(Some(&display)).expect("X server");
        let root = conn.setup().roots[screen].root;
        let monitors = conn
            .randr_get_monitors(root, true)
            .unwrap()
            .reply()
            .unwrap();
        let name = conn
            .get_atom_name(monitors.monitors[0].name)
            .unwrap()
            .reply()
            .unwrap()
            .name;
        let id = monitor_id(&String::from_utf8_lossy(&name));
        let p3 = matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3 test");
        let atom = conn
            .intern_atom(false, b"_ICC_PROFILE")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        conn.change_property8(PropMode::REPLACE, root, atom, AtomEnum::CARDINAL, &p3)
            .unwrap();
        conn.sync().unwrap();
        // SAFETY: tests in this module do not read the environment
        // concurrently.
        unsafe {
            std::env::set_var("DISPLAY", &display);
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        let got = display_target(id).expect("profile");
        assert_eq!(&got.bytes[..], &p3[..]);
        assert!(display_target(id ^ 1).is_none());
        conn.delete_property(root, atom).unwrap();
        conn.sync().unwrap();
        assert!(display_target(id).is_none());
    }
}
