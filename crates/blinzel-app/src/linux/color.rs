//! The display's colour profile on Linux; the transforms themselves come
//! from Little CMS ([`crate::lcms`]).

use blinzel_core::icc::Profile;

pub use crate::lcms::build_lut;

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

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_core::icc::{Trc, colorants, matrix_profile};

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
