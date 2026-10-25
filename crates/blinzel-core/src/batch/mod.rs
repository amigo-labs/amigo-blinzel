//! Batch resizing: many files in, resized copies out. Source files are
//! never changed (docs/PLAN.md, section 7): every copy is a new file, and
//! a name that already exists gets a number instead of being replaced.
//!
//! Per file: read, probe, decode (JPEG straight at a reduced DCT scale
//! where that keeps at least twice the target size), crop to the target's
//! aspect ratio if asked, resample in linear light ([`resample`]), turn
//! into display orientation, sharpen, encode with the source's EXIF
//! ([`meta`]) and ICC profile, and write atomically under a name from a
//! pattern ([`naming`]).

pub mod encode;
pub mod meta;
pub mod naming;
pub mod resample;
mod rows;
pub mod run;
pub mod sharpen;

use std::path::PathBuf;

use crate::format::Format;
use crate::geom::{Orientation, PixelRect};

pub use encode::{Codec, Quality};
pub use sharpen::Sharpen;

/// Target size, in display orientation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    /// The longer side becomes this many pixels.
    LongEdge(u32),
    Width(u32),
    Height(u32),
    /// Fits inside the box, keeping the aspect ratio.
    Fit {
        w: u32,
        h: u32,
    },
    /// Fills the box exactly, cropping the overhang evenly on both sides.
    Fill {
        w: u32,
        h: u32,
    },
    /// Percent of the source size.
    Percent(f32),
}

/// Output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// The source's format where we can write it (JPEG, PNG, WebP, AVIF);
    /// PNG for other sources with transparency, JPEG for the rest.
    Same,
    Codec(Codec),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub size: Size,
    /// Allow outputs larger than the source.
    pub enlarge: bool,
    pub sharpen: Option<Sharpen>,
    pub output: Output,
    pub quality: Quality,
    pub keep_exif: bool,
    /// Only with `keep_exif`.
    pub keep_gps: bool,
    pub keep_icc: bool,
    /// See [`naming`].
    pub pattern: String,
    /// Where copies go. A relative path is taken relative to each source's
    /// folder.
    pub out_dir: PathBuf,
}

impl Default for Job {
    fn default() -> Self {
        Job {
            size: Size::LongEdge(1920),
            enlarge: false,
            sharpen: None,
            output: Output::Same,
            quality: Quality::default(),
            keep_exif: true,
            keep_gps: true,
            keep_icc: true,
            pattern: naming::DEFAULT_PATTERN.into(),
            out_dir: PathBuf::from("resized"),
        }
    }
}

impl Job {
    /// Checks everything that does not depend on a file.
    pub fn validate(&self) -> Result<(), String> {
        naming::validate(&self.pattern)?;
        let zero = match self.size {
            Size::LongEdge(n) | Size::Width(n) | Size::Height(n) => n == 0,
            Size::Fit { w, h } | Size::Fill { w, h } => w == 0 || h == 0,
            Size::Percent(p) => p.is_nan() || p <= 0.0,
        };
        if zero {
            return Err("the target size is zero".into());
        }
        Ok(())
    }
}

/// The codec for a source of `format`, with or without transparency.
pub fn codec_for(output: Output, format: Format, alpha: bool) -> Codec {
    match (output, format) {
        (Output::Codec(c), _) => c,
        (Output::Same, Format::Jpeg) => Codec::Jpeg,
        (Output::Same, Format::Png) => Codec::Png,
        (Output::Same, Format::WebP) => Codec::Webp,
        (Output::Same, Format::Avif) => Codec::Avif,
        (Output::Same, _) if alpha => Codec::Png,
        (Output::Same, _) => Codec::Jpeg,
    }
}

/// What to take from the source and how large to make it, both in stored
/// orientation (the file's own, before EXIF rotation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// Full-resolution source rectangle (the whole image unless filling).
    pub crop: PixelRect,
    /// Output size before turning into display orientation.
    pub out: (u32, u32),
}

/// Plans one file: `stored` is the full size in stored orientation and
/// `o` the orientation to display it in.
pub fn geometry(stored: (u32, u32), o: Orientation, size: Size, enlarge: bool) -> Geometry {
    let (sw, sh) = (stored.0 as f64, stored.1 as f64);
    // Display-orientation source size and target box.
    let (dw, dh) = o.oriented_size(sw, sh);
    let cap = |s: f64| if enlarge { s } else { s.min(1.0) };
    let round = |v: f64| (v.round() as u32).max(1);
    let (crop_d, out_d) = match size {
        Size::Fill { w, h } => {
            let (w, h) = (w as f64, h as f64);
            // The largest centred source region with the box's aspect
            // ratio; without enlarging it may stay smaller than the box.
            let fill = (w / dw).max(h / dh);
            let (cw, ch) = ((w / fill).min(dw), (h / fill).min(dh));
            let s = cap(fill);
            ((cw, ch), (round(cw * s), round(ch * s)))
        }
        _ => {
            let s = cap(match size {
                Size::LongEdge(n) => n as f64 / dw.max(dh),
                Size::Width(n) => n as f64 / dw,
                Size::Height(n) => n as f64 / dh,
                Size::Fit { w, h } => (w as f64 / dw).min(h as f64 / dh),
                Size::Percent(p) => p as f64 / 100.0,
                Size::Fill { .. } => unreachable!(),
            });
            ((dw, dh), (round(dw * s), round(dh * s)))
        }
    };
    // A centred rectangle stays centred under rotation and mirroring, so
    // the stored crop only swaps its sides with the axes.
    let (cw, ch) = o.oriented_size(crop_d.0, crop_d.1);
    let (cw, ch) = (
        (cw.round() as u32).clamp(1, stored.0),
        (ch.round() as u32).clamp(1, stored.1),
    );
    Geometry {
        crop: PixelRect {
            x: (stored.0 - cw) / 2,
            y: (stored.1 - ch) / 2,
            w: cw,
            h: ch,
        },
        out: o.oriented_size(out_d.0, out_d.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(w: u32, h: u32) -> PixelRect {
        PixelRect { x: 0, y: 0, w, h }
    }

    #[test]
    fn sizes_in_display_orientation() {
        let n = Orientation::NORMAL;
        let g = |s, e| geometry((6000, 4000), n, s, e);
        assert_eq!(g(Size::LongEdge(1500), false).out, (1500, 1000));
        assert_eq!(g(Size::Width(600), false).out, (600, 400));
        assert_eq!(g(Size::Height(600), false).out, (900, 600));
        assert_eq!(g(Size::Fit { w: 1000, h: 1000 }, false).out, (1000, 667));
        assert_eq!(g(Size::Percent(10.0), false).out, (600, 400));
        assert_eq!(g(Size::LongEdge(9000), false).out, (6000, 4000));
        assert_eq!(g(Size::LongEdge(9000), true).out, (9000, 6000));
        assert_eq!(g(Size::LongEdge(1500), false).crop, full(6000, 4000));

        // Portrait by EXIF: stored 6000×4000, shown 4000×6000.
        let r = Orientation::from_exif(6);
        let g = geometry((6000, 4000), r, Size::Width(400), false);
        assert_eq!(g.out, (600, 400), "stored orientation, shown 400×600");
    }

    #[test]
    fn fill_crops_evenly() {
        let g = geometry(
            (6000, 4000),
            Orientation::NORMAL,
            Size::Fill { w: 1080, h: 1080 },
            false,
        );
        assert_eq!(g.out, (1080, 1080));
        assert_eq!(
            g.crop,
            PixelRect {
                x: 1000,
                y: 0,
                w: 4000,
                h: 4000
            }
        );

        // Shown as portrait 4000×6000; a 16:9 landscape box crops the
        // display height, which is the stored width.
        let r = Orientation::from_exif(6);
        let g = geometry((6000, 4000), r, Size::Fill { w: 1600, h: 900 }, false);
        assert_eq!(g.out, (900, 1600));
        assert_eq!(
            g.crop,
            PixelRect {
                x: 1875,
                y: 0,
                w: 2250,
                h: 4000
            }
        );

        // Box larger than the source without enlarging: only the aspect
        // ratio is applied.
        let g = geometry(
            (800, 600),
            Orientation::NORMAL,
            Size::Fill { w: 1000, h: 1000 },
            false,
        );
        assert_eq!(g.out, (600, 600));
        assert_eq!(
            g.crop,
            PixelRect {
                x: 100,
                y: 0,
                w: 600,
                h: 600
            }
        );
    }

    #[test]
    fn same_format_where_we_can_write_it() {
        assert_eq!(codec_for(Output::Same, Format::WebP, true), Codec::Webp);
        assert_eq!(codec_for(Output::Same, Format::Heif, false), Codec::Jpeg);
        assert_eq!(codec_for(Output::Same, Format::Gif, true), Codec::Png);
        assert_eq!(
            codec_for(Output::Codec(Codec::Avif), Format::Png, true),
            Codec::Avif
        );
    }

    #[test]
    fn job_validation() {
        assert!(Job::default().validate().is_ok());
        let bad = Job {
            size: Size::Percent(0.0),
            ..Job::default()
        };
        assert!(bad.validate().is_err());
    }
}
