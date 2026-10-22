//! Benchmark gate "library vs. own code".
//!
//! `blinzel-bench corpus <dir> [--large]` writes a deterministic test corpus.
//! `blinzel-bench run <dir> [--json <file>] [--quick]` times every candidate
//! for each hot path and prints a table (and JSON for CI artifacts). The
//! numbers decide which implementation ships.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use blinzel_core::bufpool::{self, PoolBuf};
use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
use blinzel_core::decode::{self, Request};
use blinzel_core::geom::PixelRect;
use blinzel_core::natsort::SortKey;
use blinzel_core::testimg;
use blinzel_core::timing::{Stats, json_num, json_str};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("corpus") => corpus(&args[1..]),
        Some("run") => run(&args[1..]),
        _ => Err(usage()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(2);
    }
}

fn usage() -> String {
    "usage:\n  blinzel-bench corpus <dir> [--large]\n  blinzel-bench run <dir> [--json <file>] [--quick]".into()
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
        for (i, p) in px.chunks_exact_mut(3).enumerate() {
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
            _ => {}
        }
    }

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
    Ok(())
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
    let json = format!(
        "{{\n  \"os\": {}, \"arch\": {}, \"logical_cpus\": {cpus},\n  \"results\": [\n{}\n  ]\n}}\n",
        json_str(std::env::consts::OS),
        json_str(std::env::consts::ARCH),
        items.join(",\n")
    );
    fs::write(path, json).map_err(|e| e.to_string())
}
