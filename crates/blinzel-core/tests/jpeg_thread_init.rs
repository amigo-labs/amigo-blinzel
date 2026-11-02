//! blinzel-jpeg's thread-init hook (AC-16). A test binary of its own: the
//! hook is process-wide and only the first registration counts.

#![cfg(feature = "turbojpeg")]

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, ThreadId};

use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
use blinzel_core::testimg;

/// The threads the registered init ran on, in call order.
static CALLS: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());
static SECOND: AtomicUsize = AtomicUsize::new(0);

fn first() {
    CALLS.lock().unwrap().push(thread::current().id());
}

fn second() {
    SECOND.fetch_add(1, Ordering::Relaxed);
}

fn encode(w: u32, h: u32, opts: EncodeOptions) -> Vec<u8> {
    let px = if opts.gray {
        testimg::gray(w, h)
    } else {
        testimg::rgb(w, h)
    };
    encode_rgb(&px, w, h, opts).unwrap()
}

/// Decodes at full size; returns the most threads a scan used.
fn decode(data: &[u8], threads: usize) -> usize {
    let info = blinzel_jpeg::parse(data).unwrap();
    let layouts = blinzel_jpeg::plane_layouts(&info, 8);
    let mut bufs: Vec<Vec<u8>> = layouts.iter().map(|l| vec![0; l.len()]).collect();
    let mut planes: Vec<&mut [u8]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
    blinzel_jpeg::decode_planes(data, 8, &mut planes, threads).unwrap()
}

/// Scans in the file (SOS markers; entropy data stuffs every 0xFF).
fn scans(data: &[u8]) -> usize {
    data.windows(2).filter(|w| *w == [0xFF, 0xDA]).count()
}

#[test]
fn every_helper_thread_runs_the_first_registered_init() {
    blinzel_jpeg::set_thread_init(first);
    blinzel_jpeg::set_thread_init(second);

    let q92 = |sub, restart_rows| EncodeOptions {
        quality: 92,
        subsampling: sub,
        restart_rows,
        ..Default::default()
    };
    let sequential = encode(2000, 1500, q92(420, 0));
    let restart = encode(2000, 1500, q92(420, 1));
    let gray = encode(
        2400,
        1800,
        EncodeOptions {
            gray: true,
            quality: 95,
            ..Default::default()
        },
    );
    let progressive = encode(
        1200,
        900,
        EncodeOptions {
            progressive: true,
            ..Default::default()
        },
    );
    let n = scans(&progressive);
    assert!(n > 2, "{n} progressive scans");
    // The helper threads each decode starts, by the number of threads its
    // scan used: pass A, synchronization of neighbours, bands; with
    // restart markers no synchronization, with dense ones no pass A; a
    // thread per progressive scan, then the IDCT bands.
    let cases: [(&str, &[u8], usize, fn(usize, usize) -> usize); 5] = [
        ("sequential", &sequential, 4, |t, _| 3 * t - 1),
        ("gray", &gray, 4, |t, _| 3 * t - 1),
        ("sparse restart", &restart, 4, |t, _| 2 * t),
        ("dense restart", &restart, 2, |t, _| t),
        ("progressive", &progressive, 4, |_, n| n + 4),
    ];
    for (name, data, threads, helpers) in cases {
        CALLS.lock().unwrap().clear();
        let used = decode(data, threads);
        assert!(used > 1, "{name}: decoded on one thread");
        let calls = std::mem::take(&mut *CALLS.lock().unwrap());
        let distinct: HashSet<_> = calls.iter().collect();
        assert_eq!(distinct.len(), calls.len(), "{name}: once per thread");
        assert!(
            !distinct.contains(&thread::current().id()),
            "{name}: ran on the caller"
        );
        let want = helpers(used, n);
        assert!(
            calls.len() >= want,
            "{name}: {} of {want} helper threads ran the init",
            calls.len()
        );
    }
    assert_eq!(SECOND.load(Ordering::Relaxed), 0, "a second registration ran");
}
