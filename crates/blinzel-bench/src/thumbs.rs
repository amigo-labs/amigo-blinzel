//! `blinzel-bench thumbs <dir> [--count N] [--threads T]`: thumbnail
//! throughput (M4 goals: cold ≥ 150/s, 200 from the cache ≤ 30 ms).
//!
//! Writes `<dir>/thumbs-12mp/` once: N different 12-MP JPEGs with sensor
//! noise (about 4 MB each, like phone photos). Then measures
//! - cold: read + probe + decode (DCT 1/8) + reduce, all files on T threads,
//! - store: encoding and appending them to a fresh pack cache,
//! - warm: reopening the cache and reading every thumbnail back.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
use blinzel_core::testimg;
use blinzel_core::thumb::{self, EDGE, Thumb};
use blinzel_core::thumbcache::{Key, ThumbCache};

use crate::{flag, option, usage};

const W: u32 = 4000;
const H: u32 = 3000;

pub fn thumbs(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or_else(usage)?);
    let count: usize = option(args, "--count")
        .map(|v| v.parse().map_err(|_| "bad --count"))
        .transpose()?
        .unwrap_or(200);
    let threads: usize = option(args, "--threads")
        .map(|v| v.parse().map_err(|_| "bad --threads"))
        .transpose()?
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let files = corpus(&dir.join("thumbs-12mp"), count)?;

    // Cold.
    let t = Instant::now();
    let read_us = AtomicUsize::new(0);
    let made: Vec<Option<Thumb>> = parallel(&files, threads, |path| {
        let r = Instant::now();
        let data = std::fs::read(path).ok()?;
        read_us.fetch_add(r.elapsed().as_micros() as usize, Ordering::Relaxed);
        thumb::make_native(&data, EDGE).ok()
    });
    let cold = t.elapsed().as_secs_f64();
    let ok = made.iter().filter(|m| m.is_some()).count();
    println!(
        "cold:  {ok}/{} thumbnails in {:.0} ms on {threads} threads = {:.0}/s (reading {:.1} ms per file)",
        files.len(),
        cold * 1e3,
        ok as f64 / cold,
        read_us.load(Ordering::Relaxed) as f64 / 1e3 / files.len() as f64,
    );

    // Store.
    let cache_dir =
        std::env::temp_dir().join(format!("blinzel-bench-thumbs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache_dir);
    let keys: Vec<Key> = files
        .iter()
        .map(|p| {
            let m = std::fs::metadata(p).map_err(|e| e.to_string())?;
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as u64);
            Ok(Key::new(p, m.len(), mtime))
        })
        .collect::<Result<_, String>>()?;
    let cache = ThumbCache::open(&cache_dir, 1 << 30).map_err(|e| e.to_string())?;
    let t = Instant::now();
    let pairs: Vec<(Key, Thumb)> = keys
        .iter()
        .zip(made)
        .filter_map(|(k, m)| Some((*k, m?)))
        .collect();
    parallel(&pairs, threads, |(k, th)| cache.put(k, th).ok());
    let store = t.elapsed().as_secs_f64();
    let pack = std::fs::metadata(cache_dir.join("thumbs.pack")).map_or(0, |m| m.len());
    println!(
        "store: {} thumbnails in {:.0} ms ({:.1} KB each)",
        pairs.len(),
        store * 1e3,
        pack as f64 / 1024.0 / pairs.len().max(1) as f64
    );
    drop(cache);

    // Warm.
    let t = Instant::now();
    let cache = ThumbCache::open(&cache_dir, 1 << 30).map_err(|e| e.to_string())?;
    let opened = t.elapsed().as_secs_f64();
    let got = parallel(&keys, threads, |k| cache.get(k));
    let warm = t.elapsed().as_secs_f64();
    let hits = got.iter().filter(|g| g.is_some()).count();
    println!(
        "warm:  {hits}/{} from the cache in {:.1} ms (open {:.2} ms) on {threads} threads",
        keys.len(),
        warm * 1e3,
        opened * 1e3
    );
    if !flag(args, "--keep-cache") {
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
    Ok(())
}

/// Runs `f` over `items` on `threads` threads, results in order.
fn parallel<T: Sync, R: Send>(
    items: &[T],
    threads: usize,
    f: impl Fn(&T) -> Option<R> + Sync,
) -> Vec<Option<R>> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    out.lock().unwrap()[i] = r;
                }
            });
        }
    });
    out.into_inner().unwrap()
}

/// `count` different noisy 12-MP JPEGs in `dir` (kept between runs).
fn corpus(dir: &Path, count: usize) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let base = testimg::rgb(W, H);
    let files: Vec<PathBuf> = (0..count)
        .map(|i| dir.join(format!("photo-{i:04}.jpg")))
        .collect();
    let missing: Vec<(usize, &PathBuf)> = files
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.exists())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "writing {} test photos to {} ...",
            missing.len(),
            dir.display()
        );
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let failed = parallel(&missing, threads, |(i, path)| {
        // Shifted colours per file, plus noise, so every file differs.
        let mut px = base.clone();
        let mut rng = 0x2545_F491_4F6C_DD1Du64 ^ ((*i as u64) << 20);
        for (k, v) in px.iter_mut().enumerate() {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let shift = ((*i * 37 + k % 3 * 50) % 64) as i32 - 32;
            *v = (*v as i32 + shift + (rng & 31) as i32 - 16).clamp(0, 255) as u8;
        }
        let data = encode_rgb(
            &px,
            W,
            H,
            EncodeOptions {
                quality: 90,
                ..Default::default()
            },
        )
        .ok()?;
        std::fs::write(path, data).err().map(|e| e.to_string())
    });
    if let Some(e) = failed.into_iter().flatten().next() {
        return Err(e);
    }
    Ok(files)
}
