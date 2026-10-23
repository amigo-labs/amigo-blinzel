//! Cross-check: blinzel-jpeg must produce exactly libjpeg-turbo's YCbCr
//! planes at every supported DCT scale.

#![cfg(feature = "turbojpeg")]

use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
use blinzel_core::decode::{self, Request};
use blinzel_core::pixel::{Pixels, Plane};
use blinzel_core::testimg;

type OwnPlanes = Vec<(Vec<u8>, blinzel_jpeg::PlaneLayout)>;

/// Our planes for `data` at `scale` as (buffer, layout) pairs, and the
/// number of threads the decoder used.
fn own(data: &[u8], scale: u32, threads: usize) -> (OwnPlanes, usize) {
    let info = blinzel_jpeg::parse(data).unwrap();
    let layouts = blinzel_jpeg::plane_layouts(&info, scale);
    let mut bufs: Vec<Vec<u8>> = layouts.iter().map(|l| vec![0; l.len()]).collect();
    let mut planes: Vec<&mut [u8]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
    let used = blinzel_jpeg::decode_planes(data, scale, &mut planes, threads).unwrap();
    (bufs.into_iter().zip(layouts).collect(), used)
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
    check_threads(name, data, 1);
}

/// Compares with turbo at every scale; with `threads > 1` the decoder must
/// actually have run in parallel.
fn check_threads(name: &str, data: &[u8], threads: usize) {
    for scale in blinzel_jpeg::SCALES {
        let (ours, used) = own(data, scale, threads);
        assert!(
            threads == 1 || used > 1,
            "{name} scale {scale}/8: fell back to sequential decoding"
        );
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

/// Large enough for several threads (≥ 64 KiB of entropy data each).
#[test]
fn parallel_decoding_matches_turbojpeg() {
    for (sub, restart_rows) in [(420, 0), (444, 0), (420, 1), (422, 7)] {
        let opts = EncodeOptions {
            quality: 92,
            subsampling: sub,
            restart_rows,
            ..Default::default()
        };
        let data = encode(2000, 1500, opts);
        // 2 threads take the dense-restart shortcut, 8 threads pass A.
        for threads in [2, 3, 8] {
            check_threads(
                &format!("{sub} restart {restart_rows} threads {threads}"),
                &data,
                threads,
            );
        }
    }
    let gray = EncodeOptions {
        gray: true,
        quality: 95,
        ..Default::default()
    };
    check_threads("gray threads 4", &encode(2400, 1800, gray), 4);
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
    // Large enough that the parallel decoder is tried and must fall back.
    let data = encode(2000, 1500, EncodeOptions::default());
    for cut in [data.len() / 2, data.len() - 3, 700] {
        let _ = own_lossy(&data[..cut]);
    }
}

fn own_lossy(data: &[u8]) -> Option<usize> {
    let info = blinzel_jpeg::parse(data).ok()?;
    let layouts = blinzel_jpeg::plane_layouts(&info, 8);
    let mut bufs: Vec<Vec<u8>> = layouts.iter().map(|l| vec![0; l.len()]).collect();
    let mut planes: Vec<&mut [u8]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
    blinzel_jpeg::decode_planes(data, 8, &mut planes, 4).ok()
}

/// The viewer's entry point picks blinzel-jpeg with several threads; the
/// image must be the same as libjpeg-turbo's.
#[test]
fn decode_with_threads_matches_the_turbojpeg_path() {
    let data = encode(2000, 1500, EncodeOptions::default());
    for req in [
        Request::Full,
        Request::Fit { w: 500, h: 375 },
        Request::Fit { w: 700, h: 500 },
    ] {
        let a = decode::decode(&data, req).unwrap();
        let b = decode::decode_with(&data, req, 4).unwrap();
        assert!(b.width >= a.width && b.height >= a.height, "{req:?}");
        if (a.width, a.height) == (b.width, b.height) {
            assert!(a.to_bgra() == b.to_bgra(), "{req:?}");
        }
        assert!(matches!(b.pixels, Pixels::Ycc { .. }));
    }
}
