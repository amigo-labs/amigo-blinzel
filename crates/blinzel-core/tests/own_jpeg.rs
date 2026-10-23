//! Cross-check: blinzel-jpeg must produce exactly libjpeg-turbo's YCbCr
//! planes at every supported DCT scale.

#![cfg(feature = "turbojpeg")]

use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
use blinzel_core::decode::{self, Request};
use blinzel_core::pixel::{Pixels, Plane};
use blinzel_core::testimg;

/// Our planes for `data` at `scale`, as (buffer, layout) pairs.
fn own(data: &[u8], scale: u32) -> Vec<(Vec<u8>, blinzel_jpeg::PlaneLayout)> {
    let info = blinzel_jpeg::parse(data).unwrap();
    let layouts = blinzel_jpeg::plane_layouts(&info, scale);
    let mut bufs: Vec<Vec<u8>> = layouts.iter().map(|l| vec![0; l.len()]).collect();
    let mut planes: Vec<&mut [u8]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
    blinzel_jpeg::decode_planes(data, scale, &mut planes).unwrap();
    bufs.into_iter().zip(layouts).collect()
}

/// libjpeg-turbo's planes at `scale`.
fn turbo(data: &[u8], scale: u32) -> Vec<Plane> {
    let info = blinzel_jpeg::parse(data).unwrap();
    let req = Request::Fit {
        w: blinzel_jpeg::scaled(info.width, scale),
        h: blinzel_jpeg::scaled(info.height, scale),
    };
    match decode::decode(data, req).unwrap().pixels {
        Pixels::Ycc { y, cb, cr, .. } => vec![y, cb, cr],
        Pixels::Packed { plane, .. } => vec![plane],
    }
}

fn check(name: &str, data: &[u8]) {
    for scale in blinzel_jpeg::SCALES {
        let ours = own(data, scale);
        let theirs = turbo(data, scale);
        assert_eq!(ours.len(), theirs.len(), "{name}");
        for (c, ((buf, l), t)) in ours.iter().zip(&theirs).enumerate() {
            // Plane sizes may round differently by one sample; compare the
            // common area.
            assert!(l.width.abs_diff(t.width) <= 1 && l.height.abs_diff(t.height) <= 1);
            let (w, h) = (l.width.min(t.width) as usize, l.height.min(t.height));
            for y in 0..h {
                let a = &buf[y as usize * l.stride..][..w];
                let b = &t.row(y)[..w];
                if a != b {
                    let x = a.iter().zip(b).position(|(p, q)| p != q).unwrap();
                    panic!(
                        "{name} scale {scale}/8 component {c}: first difference at ({x}, {y}): \
                         ours {} turbo {}",
                        a[x], b[x]
                    );
                }
            }
        }
    }
}

fn encode(w: u32, h: u32, opts: EncodeOptions) -> Vec<u8> {
    let px = if opts.gray {
        testimg::gray(w, h)
    } else {
        testimg::rgb(w, h)
    };
    encode_rgb(&px, w, h, opts).unwrap()
}

#[test]
fn matches_turbojpeg_for_all_subsamplings_and_scales() {
    for (w, h) in [(333, 217), (64, 64), (17, 9)] {
        for sub in [444, 422, 420, 440] {
            for quality in [50, 92] {
                let opts = EncodeOptions {
                    quality,
                    subsampling: sub,
                    ..Default::default()
                };
                check(&format!("{w}x{h} {sub} q{quality}"), &encode(w, h, opts));
            }
        }
        let gray = EncodeOptions {
            gray: true,
            ..Default::default()
        };
        check(&format!("{w}x{h} gray"), &encode(w, h, gray));
    }
}

#[test]
fn matches_turbojpeg_with_restart_markers() {
    for rows in [1, 3] {
        let opts = EncodeOptions {
            restart_rows: rows,
            ..Default::default()
        };
        check(&format!("restart {rows}"), &encode(401, 203, opts));
    }
}

#[test]
fn rejects_progressive_as_unsupported() {
    let data = encode(
        64,
        64,
        EncodeOptions {
            progressive: true,
            ..Default::default()
        },
    );
    let info = blinzel_jpeg::parse(&data).unwrap();
    assert!(!blinzel_jpeg::is_supported(&info));
}

#[test]
fn truncated_data_decodes_partially_without_panicking() {
    let data = encode(200, 120, EncodeOptions::default());
    for cut in [data.len() / 2, data.len() - 3, 700] {
        let _ = own_lossy(&data[..cut]);
    }
}

fn own_lossy(data: &[u8]) -> Option<()> {
    let info = blinzel_jpeg::parse(data).ok()?;
    let layouts = blinzel_jpeg::plane_layouts(&info, 8);
    let mut bufs: Vec<Vec<u8>> = layouts.iter().map(|l| vec![0; l.len()]).collect();
    let mut planes: Vec<&mut [u8]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
    blinzel_jpeg::decode_planes(data, 8, &mut planes).ok()
}
