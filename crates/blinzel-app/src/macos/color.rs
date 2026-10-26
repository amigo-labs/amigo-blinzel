//! The display's colour profile on macOS; the transforms themselves come
//! from Little CMS ([`crate::lcms`]).
//!
//! The Metal layer has no colour space of its own, so macOS shows its
//! pixels unconverted: the viewer targets the display's profile itself,
//! like on Windows.

use blinzel_core::icc::Profile;
use objc2_core_graphics::{CGColorSpace, CGDisplayCopyColorSpace};

pub use crate::lcms::build_lut;

/// The profile of display `monitor` (a `CGDirectDisplayID`); `None` for
/// sRGB. Core Graphics answers from any thread.
pub fn display_target(monitor: isize) -> Option<Profile> {
    let space = CGDisplayCopyColorSpace(monitor as u32);
    let icc = CGColorSpace::icc_data(Some(&space))?.to_vec();
    let p = Profile::new(icc);
    (!p.srgb).then_some(p)
}
