//! Benchmark gate "library vs. own code".
//!
//! `blinzel-bench corpus <dir> [--large]` writes a deterministic test corpus.
//! `blinzel-bench run <dir> [--json <file>] [--quick] [--check]` times every
//! candidate for each hot path and prints a table (and JSON for CI
//! artifacts). The numbers decide which implementation ships. `--check`
//! fails when a shipped implementation is beaten by its yardstick.
//! `blinzel-bench compare <a.json> <b.json>` puts two runs side by side.

mod batch;
mod compare;
mod resize;
mod simd;
mod thumbs;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use blinzel_core::bufpool::{self, PoolBuf};
use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
use blinzel_core::decode::{self, Request};
use blinzel_core::geom::PixelRect;
use blinzel_core::natsort::SortKey;
use blinzel_core::pixel::Pixels;
use blinzel_core::testimg;
use blinzel_core::timing::{Stats, json_num, json_str};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("corpus") => corpus(&args[1..]),
        Some("run") => run(&args[1..]),
        Some("compare") => compare::compare(&args[1..]),
        Some("simd-child") => simd::child(&args[1..]),
        Some("jpeg-ab") => jpeg_ab(&args[1..]),
        Some("thumbs") => thumbs::thumbs(&args[1..]),
        Some("resize") => resize::resize(&args[1..]),
        Some("batch") => batch::batch(&args[1..]),
        _ => Err(usage()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(2);
    }
}

fn usage() -> String {
    "usage:
  blinzel-bench corpus <dir> [--large]
  blinzel-bench run <dir> [--json <file>] [--quick] [--check]
  blinzel-bench compare <a.json> <b.json>
  blinzel-bench jpeg-ab <file> [scale] [threads] [rounds]
  blinzel-bench thumbs <dir> [--count N] [--threads T] [--keep-cache]
  blinzel-bench resize [--runs N] [--quick]
  blinzel-bench batch <dir> [--count N] [--threads T] [--format jpeg|png|webp|avif] [--edge E] [--from <dir>] [--keep <dir>]"
        .into()
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// What a corpus file holds.
#[derive(Clone, Copy)]
enum Content {
    Jpeg(EncodeOptions),
    Png(png::ColorType),
    /// Flat UI-like bands: compresses very differently from photos.
    Screenshot,
}

/// One file of the corpus.
struct CorpusFile {
    name: &'static str,
    w: u32,
    h: u32,
    content: Content,
    /// Only with `--large`.
    large: bool,
}

fn corpus_files() -> Vec<CorpusFile> {
    let base = EncodeOptions {
        quality: 90,
        ..Default::default()
    };
    let jpeg = |name, w, h, opts| CorpusFile {
        name,
        w,
        h,
        content: Content::Jpeg(opts),
        large: false,
    };
    let at = |subsampling| EncodeOptions {
        subsampling,
        ..base
    };
    vec![
        jpeg("jpeg-12mp-420.jpg", 4000, 3000, base),
        jpeg("jpeg-24mp-420.jpg", 6000, 4000, base),
        jpeg("jpeg-24mp-422.jpg", 6000, 4000, at(422)),
        jpeg("jpeg-24mp-444.jpg", 6000, 4000, at(444)),
        jpeg(
            "jpeg-24mp-420-progressive.jpg",
            6000,
            4000,
            EncodeOptions {
                progressive: true,
                ..base
            },
        ),
        jpeg(
            "jpeg-24mp-420-restart.jpg",
            6000,
            4000,
            EncodeOptions {
                restart_rows: 1,
                ..base
            },
        ),
        jpeg(
            "jpeg-24mp-gray.jpg",
            6000,
            4000,
            EncodeOptions { gray: true, ..base },
        ),
        jpeg("jpeg-45mp-420.jpg", 8256, 5504, base),
        CorpusFile {
            large: true,
            ..jpeg("jpeg-100mp-420.jpg", 12288, 8192, base)
        },
        CorpusFile {
            content: Content::Png(png::ColorType::Rgb),
            ..jpeg("png-12mp-rgb.png", 4000, 3000, base)
        },
        CorpusFile {
            content: Content::Png(png::ColorType::Rgba),
            ..jpeg("png-12mp-rgba.png", 4000, 3000, base)
        },
        CorpusFile {
            content: Content::Screenshot,
            ..jpeg("png-8mp-screenshot.png", 3840, 2160, base)
        },
    ]
}

fn corpus(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or_else(usage)?);
    write_corpus(&dir, flag(args, "--large"), 1)
}

/// Writes the corpus, every dimension divided by `shrink` (1 for the real
/// one). Existing files are kept (generation is slow).
fn write_corpus(dir: &Path, large: bool, shrink: u32) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for f in corpus_files() {
        let path = dir.join(f.name);
        if (f.large && !large) || path.exists() {
            continue;
        }
        let t = Instant::now();
        let data = render(f.content, f.w / shrink, f.h / shrink);
        fs::write(&path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        println!(
            "{}: {} KiB in {:.1} s",
            f.name,
            data.len() / 1024,
            t.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

/// The encoded bytes of one corpus file: the same on every run.
fn render(content: Content, w: u32, h: u32) -> Vec<u8> {
    match content {
        Content::Jpeg(opts) => {
            let px = if opts.gray {
                testimg::gray(w, h)
            } else {
                testimg::rgb(w, h)
            };
            encode_rgb(&px, w, h, opts).expect("encode")
        }
        Content::Png(color @ png::ColorType::Rgba) => png_encode(w, h, color, &testimg::rgba(w, h)),
        Content::Png(color) => png_encode(w, h, color, &testimg::rgb(w, h)),
        Content::Screenshot => {
            let mut px = vec![0u8; (w * h * 3) as usize];
            for (i, p) in px.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let (x, y) = (i as u32 % w, i as u32 / w);
                let band = (y / 40 + x / 300) % 5;
                p.copy_from_slice(
                    &[
                        [240, 240, 240],
                        [30, 30, 30],
                        [0, 120, 215],
                        [255, 255, 255],
                        [200, 60, 60],
                    ][band as usize],
                );
            }
            png_encode(w, h, png::ColorType::Rgb, &px)
        }
    }
}

fn png_encode(w: u32, h: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Balanced);
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(data).expect("png data");
    }
    out
}

struct Result_ {
    group: String,
    candidate: String,
    file: String,
    megapixels: f64,
    stats: Stats,
}

/// Megapixels per second at `median_ms`; 0 without pixels or time.
fn mp_per_s(megapixels: f64, median_ms: f64) -> f64 {
    if megapixels > 0.0 && median_ms > 0.0 {
        megapixels / (median_ms / 1e3)
    } else {
        0.0
    }
}

/// The printed line of one result.
fn line(r: &Result_) -> String {
    let v = mp_per_s(r.megapixels, r.stats.median);
    let rate = if v > 0.0 {
        format!("{v:.1} MP/s")
    } else {
        "-".to_string()
    };
    format!(
        "{:<14} {:<42} {:<32} median {:>8.2} ms  min {:>8.2} ms  {rate:>12}",
        r.group, r.candidate, r.file, r.stats.median, r.stats.min
    )
}

/// Prints one result's line (plus `extra`) and keeps the result.
fn record(results: &mut Vec<Result_>, r: Result_, extra: &str) {
    println!("{}{extra}", line(&r));
    results.push(r);
}

/// Timed runs per candidate.
fn iterations(quick: bool) -> usize {
    if quick { 3 } else { 9 }
}

/// Times `f` `iters` times after one warm-up run.
fn time(iters: usize, mut f: impl FnMut()) -> Stats {
    f();
    let samples: Vec<f64> = (0..iters)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    Stats::of(&samples).unwrap()
}

fn run(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or_else(usage)?);
    let quick = flag(args, "--quick");
    let iters = iterations(quick);
    let mut results: Vec<Result_> = Vec::new();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e} (run `blinzel-bench corpus` first)", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let verify = flag(args, "--check");
    let mut mismatches: Vec<String> = Vec::new();

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let data = fs::read(path).map_err(|e| e.to_string())?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mut push = |group: &str, candidate: &str, mp: f64, stats: Stats| {
            let r = Result_ {
                group: group.into(),
                candidate: candidate.into(),
                file: name.clone(),
                megapixels: mp,
                stats,
            };
            record(&mut results, r, "");
        };
        match ext {
            "jpg" => {
                let info = blinzel_jpeg::parse(&data).map_err(|e| format!("{name}: {e}"))?;
                if verify
                    && blinzel_jpeg::is_supported(&info)
                    && let Err(e) = verify_own_jpeg(&data, cpus)
                {
                    mismatches.push(format!("  {name}: {e}"));
                }
                let mp = info.width as f64 * info.height as f64 / 1e6;
                let full = PixelRect::new(0, 0, info.width, info.height);
                push(
                    "jpeg-header",
                    "blinzel-jpeg parse",
                    mp,
                    time(iters, || {
                        std::hint::black_box(blinzel_jpeg::parse(&data).unwrap());
                    }),
                );
                push(
                    "jpeg-full",
                    "turbojpeg packed BGRA",
                    mp,
                    time(iters, || {
                        std::hint::black_box(decode::decode(&data, Request::Region(full)).unwrap());
                    }),
                );
                push(
                    "jpeg-full",
                    "turbojpeg YCbCr planes",
                    mp,
                    time(iters, || {
                        std::hint::black_box(decode::decode(&data, Request::Full).unwrap());
                    }),
                );
                if blinzel_jpeg::is_supported(&info) {
                    push(
                        "jpeg-full",
                        "blinzel-jpeg YCbCr planes",
                        mp,
                        time(iters, || {
                            std::hint::black_box(own_jpeg(&data, 8, 1));
                        }),
                    );
                    push(
                        "jpeg-full",
                        &format!("blinzel-jpeg YCbCr planes {cpus} thr"),
                        mp,
                        time(iters, || {
                            std::hint::black_box(own_jpeg(&data, 8, cpus));
                        }),
                    );
                }
                push(
                    "jpeg-full",
                    "zune-jpeg RGBA",
                    mp,
                    time(iters, || {
                        std::hint::black_box(zune_decode(&data));
                    }),
                );
                for (label, div) in [("1/2", 2), ("1/4", 4), ("1/8", 8)] {
                    let req = Request::Fit {
                        w: info.width / div,
                        h: info.height / div,
                    };
                    push(
                        "jpeg-scaled",
                        &format!("turbojpeg YCbCr {label}"),
                        mp,
                        time(iters, || {
                            std::hint::black_box(decode::decode(&data, req).unwrap());
                        }),
                    );
                    if blinzel_jpeg::is_supported(&info) {
                        push(
                            "jpeg-scaled",
                            &format!("blinzel-jpeg YCbCr {label}"),
                            mp,
                            time(iters, || {
                                std::hint::black_box(own_jpeg(&data, 8 / div, 1));
                            }),
                        );
                        push(
                            "jpeg-scaled",
                            &format!("blinzel-jpeg YCbCr {label} {cpus} thr"),
                            mp,
                            time(iters, || {
                                std::hint::black_box(own_jpeg(&data, 8 / div, cpus));
                            }),
                        );
                    }
                }
                // Typical screen: fit a 2560×1440 monitor.
                let s = (2560.0 / info.width as f64)
                    .min(1440.0 / info.height as f64)
                    .min(1.0);
                let req = Request::Fit {
                    w: (info.width as f64 * s) as u32,
                    h: (info.height as f64 * s) as u32,
                };
                push(
                    "jpeg-screen",
                    "turbojpeg YCbCr fit 1440p",
                    mp,
                    time(iters, || {
                        std::hint::black_box(decode::decode(&data, req).unwrap());
                    }),
                );
                let region = PixelRect::new(info.width / 3, info.height / 3, 1920, 1080);
                push(
                    "jpeg-region",
                    "turbojpeg crop 1920x1080",
                    mp,
                    time(iters, || {
                        std::hint::black_box(
                            decode::decode(&data, Request::Region(region)).unwrap(),
                        );
                    }),
                );
            }
            "png" => {
                let (w, h) = decode::png::header_size(&data).map_err(|e| e.to_string())?;
                let mp = w as f64 * h as f64 / 1e6;
                push(
                    "png",
                    "blinzel-core (png + fused RGBA)",
                    mp,
                    time(iters, || {
                        std::hint::black_box(decode::decode(&data, Request::Full).unwrap());
                    }),
                );
                push(
                    "png",
                    "zune-png native layout",
                    mp,
                    time(iters, || {
                        std::hint::black_box(zune_png_native(&data));
                    }),
                );
                push(
                    "png",
                    "png crate native layout",
                    mp,
                    time(iters, || {
                        std::hint::black_box(png_crate_decode(&data, false));
                    }),
                );
                push(
                    "png",
                    "png crate + its RGBA expansion",
                    mp,
                    time(iters, || {
                        std::hint::black_box(png_crate_decode(&data, true));
                    }),
                );
            }
            // AVIF and HEIC are not generated by the corpus (no encoder in
            // Rust); measured when such files are put into the folder.
            "avif" => {
                let Ok(probe) = decode::probe(&data) else {
                    println!("avif           skipped                            {name}");
                    continue;
                };
                let mp = probe.width as f64 * probe.height as f64 / 1e6;
                for threads in [1, cpus] {
                    push(
                        "avif",
                        &format!("rav1d, {threads} thread(s)"),
                        mp,
                        time(iters, || {
                            std::hint::black_box(
                                decode::decode_with(&data, Request::Full, threads).unwrap(),
                            );
                        }),
                    );
                }
            }
            "heic" | "heif" | "hif" => {
                // HEIF also holds AVIF and others that go to the platform.
                let Ok(probe) = decode::probe(&data) else {
                    println!("heic           skipped                            {name}");
                    continue;
                };
                let mp = probe.width as f64 * probe.height as f64 / 1e6;
                for threads in [1, cpus] {
                    push(
                        "heic",
                        &format!("zune-heic, {threads} thread(s)"),
                        mp,
                        time(iters, || {
                            std::hint::black_box(
                                decode::decode_with(&data, Request::Full, threads).unwrap(),
                            );
                        }),
                    );
                }
            }
            _ => {}
        }
    }

    simd::run(&dir, iters, &mut results)?;

    // File-name sorting for a 10k-image folder.
    let names: Vec<String> = (0..10_000)
        .map(|i| format!("IMG_{:05} ({}).JPG", (i * 7919) % 10_000, i % 7))
        .collect();
    record(
        &mut results,
        Result_ {
            group: "natsort".into(),
            candidate: "SortKey + sort 10k".into(),
            file: "-".into(),
            megapixels: 0.0,
            stats: time(iters, || {
                let mut keyed: Vec<(SortKey, &String)> =
                    names.iter().map(|n| (SortKey::new(n), n)).collect();
                keyed.sort_unstable();
                std::hint::black_box(keyed);
            }),
        },
        "",
    );

    // Fresh 96 MB buffer (page faults) vs. a recycled one.
    let len = 6000 * 4000 * 4;
    let touch = |b: &mut [u8]| {
        for i in (0..b.len()).step_by(4096) {
            b[i] = 1;
        }
    };
    record(
        &mut results,
        Result_ {
            group: "alloc-96mb".into(),
            candidate: "fresh Vec + touch".into(),
            file: "-".into(),
            megapixels: 24.0,
            stats: time(iters, || {
                let mut v = vec![0u8; len];
                touch(&mut v);
                std::hint::black_box(v);
            }),
        },
        "",
    );
    bufpool::global().set_max_retained(1 << 30);
    record(
        &mut results,
        Result_ {
            group: "alloc-96mb".into(),
            candidate: "BufPool + touch".into(),
            file: "-".into(),
            megapixels: 24.0,
            stats: time(iters, || {
                // Dropping returns the buffer to the pool for the next round.
                let mut b = PoolBuf::new(len);
                touch(&mut b);
                std::hint::black_box(&b);
            }),
        },
        "",
    );

    if let Some(out) = option(args, "--json") {
        write_json(Path::new(out), &results)?;
        println!("wrote {out}");
    }
    if verify {
        check(&results, &mismatches)?;
    }
    Ok(())
}

/// blinzel-jpeg, decoding in parallel, must reproduce libjpeg-turbo's
/// planes exactly; the corpus has the file sizes where every parallel code
/// path (speculation, retries, restart markers) actually runs.
fn verify_own_jpeg(data: &[u8], threads: usize) -> Result<(), String> {
    let info = blinzel_jpeg::parse(data).map_err(|e| e.to_string())?;
    for scale in [8, 1] {
        let layouts = blinzel_jpeg::plane_layouts(&info, scale);
        let ours = own_jpeg(data, scale, threads);
        let req = Request::Fit {
            w: blinzel_jpeg::scaled(info.width, scale),
            h: blinzel_jpeg::scaled(info.height, scale),
        };
        let theirs = match decode::decode(data, req).map_err(|e| e.to_string())?.pixels {
            Pixels::Ycc { y, cb, cr, .. } => vec![y, cb, cr],
            Pixels::Packed { plane, .. } => vec![plane],
        };
        compare_planes(scale, &ours, &layouts, &theirs)?;
    }
    Ok(())
}

/// Compares blinzel-jpeg's planes (`ours`, laid out by `layouts`) with
/// libjpeg-turbo's row by row; the first differing row is the error.
fn compare_planes(
    scale: u32,
    ours: &[PoolBuf],
    layouts: &[blinzel_jpeg::PlaneLayout],
    theirs: &[blinzel_core::pixel::Plane],
) -> Result<(), String> {
    for (c, ((buf, l), t)) in ours.iter().zip(layouts).zip(theirs).enumerate() {
        let w = l.width.min(t.width) as usize;
        for y in 0..l.height.min(t.height) {
            if buf[y as usize * l.stride..][..w] != t.row(y)[..w] {
                return Err(format!(
                    "blinzel-jpeg differs from libjpeg-turbo at {scale}/8, component {c}, row {y}"
                ));
            }
        }
    }
    Ok(())
}

/// Whether zune-jpeg beats libjpeg-turbo: strictly faster in the median and
/// in the best run, so a tie is no win. An unoptimized build is slower in
/// every run; a noisy neighbour on the CI runner raises the median of one
/// file but not its minimum.
fn zune_wins(turbo: &Stats, zune: &Stats) -> bool {
    turbo.median > zune.median && turbo.min > zune.min
}

/// The shipped JPEG path (libjpeg-turbo to YCbCr planes) must beat
/// zune-jpeg on every file. It does by 1.4–2× when libjpeg-turbo is built
/// properly; an unoptimized C build (MSVC /Od) is 1.4× slower than zune.
/// blinzel-jpeg must match libjpeg-turbo byte for byte.
fn check(results: &[Result_], mismatches: &[String]) -> Result<(), String> {
    if !mismatches.is_empty() {
        return Err(format!("check failed:\n{}", mismatches.join("\n")));
    }
    let stats = |file: &str, candidate: &str| {
        results
            .iter()
            .find(|r| r.group == "jpeg-full" && r.file == file && r.candidate == candidate)
            .map(|r| r.stats)
    };
    let mut failures = Vec::new();
    for r in results
        .iter()
        .filter(|r| r.candidate == "turbojpeg YCbCr planes")
    {
        let Some(zune) = stats(&r.file, "zune-jpeg RGBA") else {
            continue;
        };
        if zune_wins(&r.stats, &zune) {
            failures.push(format!(
                "  {}: turbojpeg {:.1} ms (min {:.1}), zune-jpeg {:.1} ms (min {:.1})",
                r.file, r.stats.median, r.stats.min, zune.median, zune.min
            ));
        }
    }
    if failures.is_empty() {
        println!("check passed: turbojpeg beats zune-jpeg on every JPEG");
        Ok(())
    } else {
        Err(format!(
            "check failed: zune-jpeg beats the shipped libjpeg-turbo path\n{}\n\
             If libjpeg-turbo's C code is built without optimization (see \
             third_party/cmake-rs/README.md) fix the build; if zune-jpeg genuinely \
             got faster, the benchmark gate says to reconsider the decoder.",
            failures.join("\n")
        ))
    }
}

/// `blinzel-bench jpeg-ab <file> [scale] [threads] [rounds]`: libjpeg-turbo and
/// blinzel-jpeg alternately in one process, so machine noise hits both
/// alike. For development; not part of `run`.
fn jpeg_ab(args: &[String]) -> Result<(), String> {
    let (file, scale, threads, rounds) = ab_args(args)?;
    let data = fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let info = blinzel_jpeg::parse(&data).map_err(|e| e.to_string())?;
    let req = Request::Fit {
        w: blinzel_jpeg::scaled(info.width, scale),
        h: blinzel_jpeg::scaled(info.height, scale),
    };
    let (a, b) = alternate(
        rounds,
        || {
            std::hint::black_box(decode::decode(&data, req).unwrap());
        },
        || {
            std::hint::black_box(own_jpeg(&data, scale, threads));
        },
    )
    .ok_or("rounds must be at least 1")?;
    println!("{}", ab_line(file, scale, threads, &a, &b));
    Ok(())
}

/// `jpeg-ab`'s file, scale (default 8, full size), threads (default all
/// logical CPUs) and rounds (default 15).
fn ab_args(args: &[String]) -> Result<(&str, u32, usize, usize), String> {
    let file = args
        .first()
        .ok_or("usage: blinzel-bench jpeg-ab <file> [scale] [threads] [rounds]")?;
    let num = |i: usize, default: usize| -> Result<usize, String> {
        args.get(i)
            .map_or(Ok(default), |s| s.parse().map_err(|e| format!("{s}: {e}")))
    };
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    Ok((file, num(1, 8)? as u32, num(2, cpus)?, num(3, 15)?))
}

/// Times `a` and `b` alternately: one round that warms caches and the pool,
/// then `rounds` timed ones. `None` without timed rounds.
fn alternate(rounds: usize, mut a: impl FnMut(), mut b: impl FnMut()) -> Option<(Stats, Stats)> {
    let (mut ta, mut tb) = (Vec::new(), Vec::new());
    for i in 0..=rounds {
        let t = Instant::now();
        a();
        let da = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        b();
        let db = t.elapsed().as_secs_f64() * 1e3;
        if i > 0 {
            ta.push(da);
            tb.push(db);
        }
    }
    Some((Stats::of(&ta)?, Stats::of(&tb)?))
}

/// Both medians and minima and the ratios libjpeg-turbo / blinzel-jpeg.
fn ab_line(file: &str, scale: u32, threads: usize, a: &Stats, b: &Stats) -> String {
    format!(
        "{file} {scale}/8, {threads} threads: turbojpeg median {:.2} min {:.2} | blinzel-jpeg median {:.2} min {:.2} | ratio {:.2}x (min {:.2}x)",
        a.median,
        a.min,
        b.median,
        b.min,
        a.median / b.median,
        a.min / b.min
    )
}

/// blinzel-jpeg into pooled planes, like the viewer would.
fn own_jpeg(data: &[u8], scale: u32, threads: usize) -> Vec<PoolBuf> {
    let info = blinzel_jpeg::parse(data).expect("jpeg header");
    let mut bufs: Vec<PoolBuf> = blinzel_jpeg::plane_layouts(&info, scale)
        .iter()
        .map(|l| PoolBuf::new(l.len()))
        .collect();
    let mut planes: Vec<&mut [u8]> = bufs.iter_mut().map(|b| &mut b[..]).collect();
    blinzel_jpeg::decode_planes(data, scale, &mut planes, threads).expect("blinzel-jpeg");
    bufs
}

fn zune_decode(data: &[u8]) -> Vec<u8> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let opts = DecoderOptions::new_fast()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(1 << 16)
        .set_max_height(1 << 16);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), opts);
    d.decode().expect("zune-jpeg")
}

fn zune_png_native(data: &[u8]) -> Vec<u8> {
    use zune_core::bytestream::ZCursor;
    use zune_core::options::DecoderOptions;
    let opts = DecoderOptions::new_fast()
        .set_max_width(1 << 24)
        .set_max_height(1 << 24);
    zune_png::PngDecoder::new_with_options(ZCursor::new(data), opts)
        .decode_raw()
        .expect("zune-png")
}

fn png_crate_decode(data: &[u8], rgba: bool) -> Vec<u8> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    let mut t = png::Transformations::EXPAND | png::Transformations::STRIP_16;
    if rgba {
        t |= png::Transformations::ALPHA;
    }
    dec.set_transformations(t);
    let mut reader = dec.read_info().expect("png info");
    let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
    reader.next_frame(&mut buf).expect("png frame");
    buf
}

/// The machine a run measured.
struct Host {
    os: String,
    arch: String,
    logical_cpus: usize,
    cpu: String,
    cpu_features: Vec<&'static str>,
    build_features: Vec<&'static str>,
}

impl Host {
    fn this() -> Host {
        Host {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            logical_cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
            cpu: blinzel_core::kernels::cpu_name(),
            cpu_features: blinzel_core::kernels::cpu_features(),
            build_features: blinzel_core::kernels::build_features(),
        }
    }
}

fn write_json(path: &Path, results: &[Result_]) -> Result<(), String> {
    fs::write(path, json(&Host::this(), results)).map_err(|e| e.to_string())
}

/// The machine, then one result object per line (`compare` reads it line
/// by line).
fn json(host: &Host, results: &[Result_]) -> String {
    let items: Vec<String> = results
        .iter()
        .map(|r| {
            format!(
                "    {{\"group\": {}, \"candidate\": {}, \"file\": {}, \"megapixels\": {}, \"mp_per_s\": {}, \"stats\": {}}}",
                json_str(&r.group),
                json_str(&r.candidate),
                json_str(&r.file),
                json_num(r.megapixels),
                json_num(mp_per_s(r.megapixels, r.stats.median)),
                r.stats.json()
            )
        })
        .collect();
    let list = |v: &[&str]| {
        let v: Vec<String> = v.iter().map(|s| json_str(s)).collect();
        format!("[{}]", v.join(", "))
    };
    format!(
        "{{\n  \"os\": {}, \"arch\": {}, \"logical_cpus\": {},\n  \"cpu\": {}, \"cpu_features\": {}, \"build_features\": {},\n  \"results\": [\n{}\n  ]\n}}\n",
        json_str(&host.os),
        json_str(&host.arch),
        host.logical_cpus,
        json_str(&host.cpu),
        list(&host.cpu_features),
        list(&host.build_features),
        items.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(min: f64, median: f64) -> Stats {
        Stats {
            min,
            median,
            p99: median,
            mean: median,
            n: 10,
        }
    }

    /// A fresh folder under the system temp directory.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("blinzel-bench-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// AC-01, AC-02: the files, their sizes and the JPEG quality.
    #[test]
    fn corpus_lists_every_file_at_its_size() {
        let files: Vec<(&str, u32, u32, bool)> = corpus_files()
            .iter()
            .map(|f| (f.name, f.w, f.h, f.large))
            .collect();
        assert_eq!(
            files,
            [
                ("jpeg-12mp-420.jpg", 4000, 3000, false),
                ("jpeg-24mp-420.jpg", 6000, 4000, false),
                ("jpeg-24mp-422.jpg", 6000, 4000, false),
                ("jpeg-24mp-444.jpg", 6000, 4000, false),
                ("jpeg-24mp-420-progressive.jpg", 6000, 4000, false),
                ("jpeg-24mp-420-restart.jpg", 6000, 4000, false),
                ("jpeg-24mp-gray.jpg", 6000, 4000, false),
                ("jpeg-45mp-420.jpg", 8256, 5504, false),
                ("jpeg-100mp-420.jpg", 12288, 8192, true),
                ("png-12mp-rgb.png", 4000, 3000, false),
                ("png-12mp-rgba.png", 4000, 3000, false),
                ("png-8mp-screenshot.png", 3840, 2160, false),
            ]
        );
        for f in corpus_files() {
            if let Content::Jpeg(o) = f.content {
                assert_eq!(o.quality, 90, "{}", f.name);
                let sub = if f.name.contains("-422") {
                    422
                } else if f.name.contains("-444") {
                    444
                } else {
                    420
                };
                assert_eq!(o.subsampling, sub, "{}", f.name);
                assert_eq!(o.progressive, f.name.contains("progressive"), "{}", f.name);
                assert_eq!(o.restart_rows > 0, f.name.contains("restart"), "{}", f.name);
                assert_eq!(o.gray, f.name.contains("gray"), "{}", f.name);
            }
        }
    }

    /// AC-01, AC-02: identical bytes on every run, existing files kept, the
    /// 100 MP file only with `--large` (small images: every side / 40).
    #[test]
    fn corpus_is_identical_on_every_run_and_keeps_existing_files() {
        let (a, b, l) = (temp("corpus-a"), temp("corpus-b"), temp("corpus-l"));
        write_corpus(&a, false, 40).unwrap();
        write_corpus(&b, false, 40).unwrap();
        let small: Vec<&str> = corpus_files()
            .iter()
            .filter(|f| !f.large)
            .map(|f| f.name)
            .collect();
        let mut sorted = small.clone();
        sorted.sort();
        assert_eq!(names(&a), sorted);
        for name in &small {
            assert_eq!(
                fs::read(a.join(name)).unwrap(),
                fs::read(b.join(name)).unwrap(),
                "{name}"
            );
        }
        let jpeg = fs::read(a.join("jpeg-24mp-420.jpg")).unwrap();
        let info = blinzel_jpeg::parse(&jpeg).unwrap();
        assert_eq!((info.width, info.height), (150, 100));

        fs::write(a.join("jpeg-12mp-420.jpg"), b"kept").unwrap();
        write_corpus(&a, false, 40).unwrap();
        assert_eq!(fs::read(a.join("jpeg-12mp-420.jpg")).unwrap(), b"kept");

        write_corpus(&l, true, 40).unwrap();
        assert!(l.join("jpeg-100mp-420.jpg").is_file());
        assert_eq!(names(&l).len(), small.len() + 1);
        for d in [a, b, l] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// AC-03: one untimed warm-up, then 9 timed runs (3 with `--quick`).
    #[test]
    fn every_candidate_runs_once_untimed_then_9_or_3_times() {
        assert_eq!((iterations(false), iterations(true)), (9, 3));
        for iters in [3, 9] {
            let mut calls = 0;
            let s = time(iters, || calls += 1);
            assert_eq!((calls, s.n), (iters + 1, iters));
        }
    }

    fn result(group: &str, file: &str, mp: f64, s: Stats) -> Result_ {
        Result_ {
            group: group.into(),
            candidate: "cand \"x\"".into(),
            file: file.into(),
            megapixels: mp,
            stats: s,
        }
    }

    /// AC-03: group, candidate, file, median, minimum, MP/s.
    #[test]
    fn a_result_line_shows_group_candidate_file_median_min_and_rate() {
        let l = line(&result("jpeg-full", "a.jpg", 24.0, stats(100.0, 200.0)));
        let words: Vec<&str> = l.split_whitespace().collect();
        assert_eq!(
            words,
            [
                "jpeg-full",
                "cand",
                "\"x\"",
                "a.jpg",
                "median",
                "200.00",
                "ms",
                "min",
                "100.00",
                "ms",
                "120.0",
                "MP/s"
            ]
        );
        let l = line(&result("natsort", "-", 0.0, stats(1.0, 2.0)));
        assert!(l.ends_with("ms             -"), "{l}");
        assert!(!l.contains("MP/s"), "{l}");
    }

    /// AC-08: the machine, then one result per line with its stats.
    #[test]
    fn json_has_the_machine_and_one_result_per_line() {
        let host = Host {
            os: "linux".into(),
            arch: "x86_64".into(),
            logical_cpus: 4,
            cpu: "Test CPU".into(),
            cpu_features: vec!["sse4.2", "avx2"],
            build_features: vec![],
        };
        let mut a = stats(1.5, 2.0);
        a.p99 = 3.0;
        a.mean = 2.25;
        a.n = 9;
        let j = json(
            &host,
            &[
                result("png", "p.png", 12.0, a),
                result("natsort", "-", 0.0, a),
            ],
        );
        let lines: Vec<&str> = j.lines().collect();
        assert_eq!(lines[0], "{");
        assert_eq!(
            lines[1],
            r#"  "os": "linux", "arch": "x86_64", "logical_cpus": 4,"#
        );
        assert_eq!(
            lines[2],
            r#"  "cpu": "Test CPU", "cpu_features": ["sse4.2", "avx2"], "build_features": [],"#
        );
        assert_eq!(lines[3], r#"  "results": ["#);
        assert_eq!(
            lines[4],
            r#"    {"group": "png", "candidate": "cand \"x\"", "file": "p.png", "megapixels": 12.000, "mp_per_s": 6000.000, "stats": {"min_ms": 1.500, "median_ms": 2.000, "p99_ms": 3.000, "mean_ms": 2.250, "n": 9}},"#
        );
        assert!(
            lines[5].starts_with(r#"    {"group": "natsort", "#)
                && lines[5].contains(r#""megapixels": 0.000, "mp_per_s": 0.000,"#),
            "{}",
            lines[5]
        );
        assert_eq!(&lines[6..], ["  ]", "}"]);
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// AC-11: scale 8, all logical CPUs and 15 rounds by default.
    #[test]
    fn jpeg_ab_defaults_to_full_size_on_all_cpus_for_15_rounds() {
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        let args = strings(&["f.jpg"]);
        assert_eq!(ab_args(&args).unwrap(), ("f.jpg", 8, cpus, 15));
        let args = strings(&["f.jpg", "2", "3", "4"]);
        assert_eq!(ab_args(&args).unwrap(), ("f.jpg", 2, 3, 4));
        assert!(ab_args(&strings(&["f.jpg", "x"])).is_err());
        assert!(ab_args(&[]).is_err());
    }

    /// AC-11: the two decoders take turns, and the first round is not
    /// timed.
    #[test]
    fn jpeg_ab_alternates_and_discards_the_first_round() {
        let order = std::cell::RefCell::new(String::new());
        let (a, b) = alternate(
            4,
            || {
                let first = order.borrow().is_empty();
                order.borrow_mut().push('a');
                if first {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            },
            || order.borrow_mut().push('b'),
        )
        .unwrap();
        assert_eq!(order.into_inner(), "ababababab");
        assert_eq!((a.n, b.n), (4, 4));
        assert!(a.p99 < 50.0, "the warm-up round was timed: {a:?}");
        assert!(alternate(0, || (), || ()).is_none());
    }

    /// AC-11: both medians and minima and their ratios.
    #[test]
    fn jpeg_ab_prints_both_medians_minima_and_ratios() {
        let l = ab_line("f.jpg", 8, 4, &stats(10.0, 20.0), &stats(4.0, 8.0));
        assert_eq!(
            l,
            "f.jpg 8/8, 4 threads: turbojpeg median 20.00 min 10.00 | blinzel-jpeg median 8.00 min 4.00 | ratio 2.50x (min 2.50x)"
        );
    }

    #[test]
    fn a_noisy_median_alone_is_no_win() {
        // CI run 41056426295, jpeg-24mp-444.jpg: one disturbed file.
        assert!(!zune_wins(&stats(121.0, 165.8), &stats(158.7, 163.4)));
        // A properly built libjpeg-turbo.
        assert!(!zune_wins(&stats(118.5, 120.1), &stats(169.3, 170.7)));
        // Unoptimized C: slower in every run.
        assert!(zune_wins(&stats(230.0, 236.0), &stats(158.7, 163.4)));
    }

    /// AC-05, AC-06: a tie is no win; zune-jpeg has to be strictly faster
    /// in both the median and the minimum.
    #[test]
    fn a_tie_in_the_median_or_the_minimum_is_no_win() {
        // stats(min, median): a lower zune-jpeg median, an equal minimum.
        assert!(!zune_wins(&stats(150.0, 170.0), &stats(150.0, 160.0)));
        // Equal medians, a lower zune-jpeg minimum.
        assert!(!zune_wins(&stats(160.0, 170.0), &stats(150.0, 170.0)));
        assert!(!zune_wins(&stats(150.0, 170.0), &stats(150.0, 170.0)));
        let results = [
            jpeg_full("turbojpeg YCbCr planes", "a.jpg", stats(150.0, 170.0)),
            jpeg_full("zune-jpeg RGBA", "a.jpg", stats(150.0, 160.0)),
            jpeg_full("turbojpeg YCbCr planes", "b.jpg", stats(160.0, 170.0)),
            jpeg_full("zune-jpeg RGBA", "b.jpg", stats(150.0, 170.0)),
        ];
        assert!(check(&results, &[]).is_ok());
    }

    fn jpeg_full(candidate: &str, file: &str, stats: Stats) -> Result_ {
        Result_ {
            group: "jpeg-full".into(),
            candidate: candidate.into(),
            file: file.into(),
            megapixels: 24.0,
            stats,
        }
    }

    #[test]
    fn check_fails_on_a_blinzel_jpeg_mismatch() {
        let results = [
            jpeg_full("turbojpeg YCbCr planes", "a.jpg", stats(118.5, 120.1)),
            jpeg_full("zune-jpeg RGBA", "a.jpg", stats(169.3, 170.7)),
        ];
        assert!(check(&results, &[]).is_ok());
        let e = check(&results, &["a.jpg: blinzel-jpeg differs".into()]).unwrap_err();
        assert!(e.contains("a.jpg: blinzel-jpeg differs"), "{e}");
    }

    #[test]
    fn check_fails_on_a_zune_jpeg_win_and_names_file_medians_minima_and_cause() {
        let results = [
            jpeg_full(
                "turbojpeg YCbCr planes",
                "jpeg-24mp-420.jpg",
                stats(230.0, 236.0),
            ),
            jpeg_full("zune-jpeg RGBA", "jpeg-24mp-420.jpg", stats(158.7, 163.4)),
        ];
        let e = check(&results, &[]).unwrap_err();
        for part in ["jpeg-24mp-420.jpg", "236.0", "230.0", "163.4", "158.7"] {
            assert!(e.contains(part), "{part} missing from {e}");
        }
        assert!(e.contains("built without optimization"), "{e}");
    }

    #[test]
    fn a_differing_plane_row_fails_the_comparison() {
        use blinzel_core::pixel::Plane;
        let layout = blinzel_jpeg::PlaneLayout {
            width: 4,
            height: 3,
            stride: 8,
            rows: 3,
        };
        let mut ours = PoolBuf::new(layout.stride * layout.rows);
        ours.fill(7);
        let mut theirs = Plane::new(4, 3, 1);
        theirs.data.fill(7);
        assert!(compare_planes(1, &[ours], &[layout], &[theirs]).is_ok());

        let mut ours = PoolBuf::new(layout.stride * layout.rows);
        ours.fill(7);
        ours[2 * layout.stride + 3] = 8;
        let mut theirs = Plane::new(4, 3, 1);
        theirs.data.fill(7);
        let e = compare_planes(1, &[ours], &[layout], &[theirs]).unwrap_err();
        assert!(e.contains("1/8, component 0, row 2"), "{e}");
    }
}
