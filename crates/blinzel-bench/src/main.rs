//! Benchmark gate "library vs. own code".
//!
//! `blinzel-bench corpus <dir> [--large]` writes a deterministic test corpus.
//! `blinzel-bench run <dir> [--json <file>] [--quick] [--check]` times every
//! candidate for each hot path and prints a table (and JSON for CI
//! artifacts). The numbers decide which implementation ships. `--check`
//! fails when a shipped implementation is beaten by its yardstick.
//! `blinzel-bench compare <a.json> <b.json>` puts two runs side by side.

mod compare;
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
        _ => Err(usage()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(2);
    }
}

fn usage() -> String {
    "usage:\n  blinzel-bench corpus <dir> [--large]\n  blinzel-bench run <dir> [--json <file>] [--quick] [--check]\n  blinzel-bench compare <a.json> <b.json>
  blinzel-bench thumbs <dir> [--count N] [--threads T]".into()
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

struct JpegCase {
    name: &'static str,
    w: u32,
    h: u32,
    opts: EncodeOptions,
    large: bool,
}

fn jpeg_cases() -> Vec<JpegCase> {
    let base = EncodeOptions {
        quality: 90,
        ..Default::default()
    };
    let c = |name, w, h, opts, large| JpegCase {
        name,
        w,
        h,
        opts,
        large,
    };
    vec![
        c("jpeg-12mp-420", 4000, 3000, base, false),
        c("jpeg-24mp-420", 6000, 4000, base, false),
        c(
            "jpeg-24mp-422",
            6000,
            4000,
            EncodeOptions {
                subsampling: 422,
                ..base
            },
            false,
        ),
        c(
            "jpeg-24mp-444",
            6000,
            4000,
            EncodeOptions {
                subsampling: 444,
                ..base
            },
            false,
        ),
        c(
            "jpeg-24mp-420-progressive",
            6000,
            4000,
            EncodeOptions {
                progressive: true,
                ..base
            },
            false,
        ),
        c(
            "jpeg-24mp-420-restart",
            6000,
            4000,
            EncodeOptions {
                restart_rows: 1,
                ..base
            },
            false,
        ),
        c(
            "jpeg-24mp-gray",
            6000,
            4000,
            EncodeOptions { gray: true, ..base },
            false,
        ),
        c("jpeg-45mp-420", 8256, 5504, base, false),
        c("jpeg-100mp-420", 12288, 8192, base, true),
    ]
}

/// Writes the corpus. Existing files are kept (generation is slow).
fn corpus(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or_else(usage)?);
    let large = flag(args, "--large");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let write = |name: &str, make: &dyn Fn() -> Vec<u8>| -> Result<(), String> {
        let path = dir.join(name);
        if path.exists() {
            return Ok(());
        }
        let t = Instant::now();
        let data = make();
        fs::write(&path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        println!(
            "{name}: {} KiB in {:.1} s",
            data.len() / 1024,
            t.elapsed().as_secs_f64()
        );
        Ok(())
    };
    for c in jpeg_cases() {
        if c.large && !large {
            continue;
        }
        write(&format!("{}.jpg", c.name), &|| {
            let px = if c.opts.gray {
                testimg::gray(c.w, c.h)
            } else {
                testimg::rgb(c.w, c.h)
            };
            encode_rgb(&px, c.w, c.h, c.opts).expect("encode")
        })?;
    }
    write("png-12mp-rgb.png", &|| {
        png_encode(4000, 3000, png::ColorType::Rgb, &testimg::rgb(4000, 3000))
    })?;
    write("png-12mp-rgba.png", &|| {
        png_encode(4000, 3000, png::ColorType::Rgba, &testimg::rgba(4000, 3000))
    })?;
    write("png-8mp-screenshot.png", &|| {
        // Flat UI-like content compresses very differently from photos.
        let (w, h) = (3840u32, 2160u32);
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
    })?;
    Ok(())
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
    let iters = if quick { 3 } else { 9 };
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
            println!(
                "{group:<14} {candidate:<34} {name:<32} median {:>8.2} ms  min {:>8.2} ms  {:>7.1} MP/s",
                stats.median,
                stats.min,
                mp / (stats.median / 1e3)
            );
            results.push(Result_ {
                group: group.into(),
                candidate: candidate.into(),
                file: name.clone(),
                megapixels: mp,
                stats,
            });
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
                    time(iters * 10, || {
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
            // Not generated by the corpus (no encoder in Rust); measured
            // when such files are put into the folder.
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
    results.push(Result_ {
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
    });

    // Fresh 96 MB buffer (page faults) vs. a recycled one.
    let len = 6000 * 4000 * 4;
    let touch = |b: &mut [u8]| {
        for i in (0..b.len()).step_by(4096) {
            b[i] = 1;
        }
    };
    results.push(Result_ {
        group: "alloc-96mb".into(),
        candidate: "fresh Vec + touch".into(),
        file: "-".into(),
        megapixels: 24.0,
        stats: time(iters, || {
            let mut v = vec![0u8; len];
            touch(&mut v);
            std::hint::black_box(v);
        }),
    });
    bufpool::global().set_max_retained(1 << 30);
    results.push(Result_ {
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
    });
    for r in results.iter().filter(|r| r.file == "-") {
        println!(
            "{:<14} {:<34} median {:>8.3} ms",
            r.group, r.candidate, r.stats.median
        );
    }

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
        for (c, ((buf, l), t)) in ours.iter().zip(&layouts).zip(&theirs).enumerate() {
            let w = l.width.min(t.width) as usize;
            for y in 0..l.height.min(t.height) {
                if buf[y as usize * l.stride..][..w] != t.row(y)[..w] {
                    return Err(format!(
                        "blinzel-jpeg differs from libjpeg-turbo at {scale}/8, component {c}, row {y}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The shipped JPEG path (libjpeg-turbo to YCbCr planes) must beat
/// zune-jpeg on every file. It does by 1.4–2× when libjpeg-turbo is built
/// properly; an unoptimized C build (MSVC /Od) is 1.4× slower than zune.
/// blinzel-jpeg must match libjpeg-turbo byte for byte.
fn check(results: &[Result_], mismatches: &[String]) -> Result<(), String> {
    if !mismatches.is_empty() {
        return Err(format!("check failed:\n{}", mismatches.join("\n")));
    }
    let median = |file: &str, candidate: &str| {
        results
            .iter()
            .find(|r| r.group == "jpeg-full" && r.file == file && r.candidate == candidate)
            .map(|r| r.stats.median)
    };
    let mut failures = Vec::new();
    for r in results
        .iter()
        .filter(|r| r.candidate == "turbojpeg YCbCr planes")
    {
        let Some(zune) = median(&r.file, "zune-jpeg RGBA") else {
            continue;
        };
        if r.stats.median >= zune {
            failures.push(format!(
                "  {}: turbojpeg {:.1} ms, zune-jpeg {:.1} ms",
                r.file, r.stats.median, zune
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
    let file = args
        .first()
        .ok_or("usage: blinzel-bench jpeg-ab <file> [scale] [threads] [rounds]")?;
    let num = |i: usize, default: usize| -> Result<usize, String> {
        args.get(i)
            .map_or(Ok(default), |s| s.parse().map_err(|e| format!("{s}: {e}")))
    };
    let scale = num(1, 8)? as u32;
    let threads = num(
        2,
        std::thread::available_parallelism().map_or(1, |n| n.get()),
    )?;
    let rounds = num(3, 15)?;
    let data = fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let info = blinzel_jpeg::parse(&data).map_err(|e| e.to_string())?;
    let req = Request::Fit {
        w: blinzel_jpeg::scaled(info.width, scale),
        h: blinzel_jpeg::scaled(info.height, scale),
    };
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for i in 0..=rounds {
        let t = Instant::now();
        std::hint::black_box(decode::decode(&data, req).unwrap());
        let ta = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        std::hint::black_box(own_jpeg(&data, scale, threads));
        let tb = t.elapsed().as_secs_f64() * 1e3;
        // The first round warms caches and the pool.
        if i > 0 {
            a.push(ta);
            b.push(tb);
        }
    }
    let (a, b) = (Stats::of(&a).unwrap(), Stats::of(&b).unwrap());
    println!(
        "{file} {scale}/8, {threads} threads: turbojpeg median {:.2} min {:.2} | blinzel-jpeg median {:.2} min {:.2} | ratio {:.2}x (min {:.2}x)",
        a.median,
        a.min,
        b.median,
        b.min,
        a.median / b.median,
        a.min / b.min
    );
    Ok(())
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

fn write_json(path: &Path, results: &[Result_]) -> Result<(), String> {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let items: Vec<String> = results
        .iter()
        .map(|r| {
            let mps = if r.megapixels > 0.0 { r.megapixels / (r.stats.median / 1e3) } else { 0.0 };
            format!(
                "    {{\"group\": {}, \"candidate\": {}, \"file\": {}, \"megapixels\": {}, \"mp_per_s\": {}, \"stats\": {}}}",
                json_str(&r.group),
                json_str(&r.candidate),
                json_str(&r.file),
                json_num(r.megapixels),
                json_num(mps),
                r.stats.json()
            )
        })
        .collect();
    let list = |v: Vec<&str>| {
        let v: Vec<String> = v.into_iter().map(json_str).collect();
        format!("[{}]", v.join(", "))
    };
    let json = format!(
        "{{\n  \"os\": {}, \"arch\": {}, \"logical_cpus\": {cpus},\n  \"cpu\": {}, \"cpu_features\": {}, \"build_features\": {},\n  \"results\": [\n{}\n  ]\n}}\n",
        json_str(std::env::consts::OS),
        json_str(std::env::consts::ARCH),
        json_str(&blinzel_core::kernels::cpu_name()),
        list(blinzel_core::kernels::cpu_features()),
        list(blinzel_core::kernels::build_features()),
        items.join(",\n")
    );
    fs::write(path, json).map_err(|e| e.to_string())
}
