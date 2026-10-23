//! Benchmark group `simd`: what each SIMD level is worth.
//!
//! - libjpeg-turbo per SIMD level. It reads its `JSIMD_FORCE*` variables
//!   once per thread, so every level runs in a child process
//!   (`blinzel-bench simd-child <file> <iters>`).
//! - Our own kernels (`blinzel_core::kernels`) per level on a 12 MP image.
//!
//! The upper bound "everything compiled for AVX2" is a second build with
//! `-C target-cpu=x86-64-v3`, put next to this one by `blinzel-bench
//! compare`.

use std::path::Path;
use std::process::Command;

use blinzel_core::decode::{self, Request};
use blinzel_core::geom::PixelRect;
use blinzel_core::kernels::{self, Level};
use blinzel_core::pixel::{ChromaSub, Plane};
use blinzel_core::testimg;
use blinzel_core::timing::Stats;

use super::{Result_, time};

/// The JPEG libjpeg-turbo is measured on at each level.
const JPEG_FILE: &str = "jpeg-24mp-420.jpg";

const JSIMD_VARS: [&str; 4] = [
    "JSIMD_FORCENONE",
    "JSIMD_FORCESSE2",
    "JSIMD_FORCEAVX2",
    "JSIMD_FORCENEON",
];

/// libjpeg-turbo levels this CPU can run and the variable forcing each.
/// libjpeg-turbo has no AVX level without AVX2.
fn turbo_levels() -> Vec<(&'static str, &'static str)> {
    #[allow(unused_mut)]
    let mut levels = vec![("none", "JSIMD_FORCENONE")];
    #[cfg(target_arch = "x86_64")]
    {
        levels.push(("sse2", "JSIMD_FORCESSE2"));
        if std::is_x86_feature_detected!("avx2") {
            levels.push(("avx2", "JSIMD_FORCEAVX2"));
        }
    }
    #[cfg(target_arch = "aarch64")]
    levels.push(("neon", "JSIMD_FORCENEON"));
    levels
}

fn turbo_cases(w: u32, h: u32) -> [(&'static str, Request); 3] {
    [
        ("turbojpeg YCbCr full", Request::Full),
        // Packed output adds libjpeg-turbo's SIMD colour conversion and
        // upsampling: the most SIMD can do for it.
        (
            "turbojpeg BGRA full",
            Request::Region(PixelRect::new(0, 0, w, h)),
        ),
        ("turbojpeg YCbCr 1/8", Request::Fit { w: w / 8, h: h / 8 }),
    ]
}

/// Hidden command run in a child process: times the libjpeg-turbo cases
/// and prints one `case\tmin\tmedian\tp99\tmean\tn` line each.
pub fn child(args: &[String]) -> Result<(), String> {
    let [file, iters] = args else {
        return Err("usage: blinzel-bench simd-child <file> <iters>".into());
    };
    let iters: usize = iters.parse().map_err(|e| format!("iters: {e}"))?;
    let data = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let info = blinzel_jpeg::parse(&data).map_err(|e| format!("{file}: {e}"))?;
    for (case, req) in turbo_cases(info.width, info.height) {
        let s = time(iters, || {
            std::hint::black_box(decode::decode(&data, req).unwrap());
        });
        println!(
            "{case}\t{}\t{}\t{}\t{}\t{}",
            s.min, s.median, s.p99, s.mean, s.n
        );
    }
    Ok(())
}

pub fn run(dir: &Path, iters: usize, results: &mut Vec<Result_>) -> Result<(), String> {
    let path = dir.join(JPEG_FILE);
    if path.is_file() {
        turbo(&path, iters, results)?;
    }
    own_kernels(iters, results);
    Ok(())
}

fn turbo(path: &Path, iters: usize, results: &mut Vec<Result_>) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let info = blinzel_jpeg::parse(&data).map_err(|e| format!("{JPEG_FILE}: {e}"))?;
    let mp = info.width as f64 * info.height as f64 / 1e6;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut by_case: Vec<(&str, Vec<(&str, Stats)>)> = turbo_cases(info.width, info.height)
        .iter()
        .map(|(case, _)| (*case, Vec::new()))
        .collect();
    for (level, var) in turbo_levels() {
        let mut cmd = Command::new(&exe);
        cmd.arg("simd-child").arg(path).arg(iters.to_string());
        for v in JSIMD_VARS {
            cmd.env_remove(v);
        }
        let out = cmd.env(var, "1").output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!(
                "simd-child ({var}) failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let (case, stats) = parse_child_line(line)?;
            let entry = by_case
                .iter_mut()
                .find(|(c, _)| *c == case)
                .ok_or_else(|| format!("simd-child: unknown case {case:?}"))?;
            entry.1.push((level, stats));
        }
    }
    for (case, levels) in by_case {
        report(results, case, JPEG_FILE, mp, levels);
    }
    Ok(())
}

fn parse_child_line(line: &str) -> Result<(&str, Stats), String> {
    let bad = || format!("simd-child: bad line {line:?}");
    let mut it = line.split('\t');
    let case = it.next().ok_or_else(bad)?;
    let mut num =
        || -> Result<f64, String> { it.next().and_then(|v| v.parse().ok()).ok_or_else(bad) };
    let stats = Stats {
        min: num()?,
        median: num()?,
        p99: num()?,
        mean: num()?,
        n: num()? as usize,
    };
    Ok((case, stats))
}

type Kernel = fn(Level, &[u8], &mut [u8]);

/// Each kernel per level, row by row as the decoders call them.
fn own_kernels(iters: usize, results: &mut Vec<Result_>) {
    const FILE: &str = "testimg 4000x3000";
    let (w, h) = (4000u32, 3000u32);
    let mp = w as f64 * h as f64 / 1e6;
    let rgb = testimg::rgb(w, h);
    let rgba = testimg::rgba(w, h);
    let gray = testimg::gray(w, h);
    let gray_alpha: Vec<u8> = gray
        .iter()
        .zip(rgba.iter().skip(3).step_by(4))
        .flat_map(|(&g, &a)| [g, a])
        .collect();
    let mut dst = vec![0u8; w as usize * h as usize * 4];
    let row_out = w as usize * 4;

    let packed: [(&str, &[u8], usize, Kernel); 3] = [
        ("kernel rgb->rgba", &rgb, 3, kernels::rgb_to_rgba),
        (
            "kernel gray+alpha->rgba",
            &gray_alpha,
            2,
            kernels::gray_alpha_to_rgba,
        ),
        ("kernel rgba->bgra", &rgba, 4, kernels::rgba_to_bgra),
    ];
    for (case, src, bpp, kernel) in packed {
        let row_in = w as usize * bpp;
        let levels = Level::available()
            .into_iter()
            .map(|level| {
                let stats = time(iters, || {
                    for (s, d) in src.chunks_exact(row_in).zip(dst.chunks_exact_mut(row_out)) {
                        kernel(level, s, d);
                    }
                    std::hint::black_box(&dst);
                });
                (level.name(), stats)
            })
            .collect();
        report(results, case, FILE, mp, levels);
    }

    let plane = |pw: u32, ph: u32, src: Vec<u8>| {
        let mut p = Plane::new(pw, ph, 1);
        p.data.copy_from_slice(&src);
        p
    };
    let (cw, ch) = (w / 2, h / 2);
    let chroma = testimg::rgb(cw, ch);
    let luma = plane(w, h, gray);
    let cb = plane(cw, ch, chroma.iter().step_by(3).copied().collect());
    let cr = plane(cw, ch, chroma.iter().skip(2).step_by(3).copied().collect());
    let sub = ChromaSub { h: 2, v: 2 };
    let levels = Level::available()
        .into_iter()
        .map(|level| {
            let stats = time(iters, || {
                for y in 0..h {
                    let row = &mut dst[y as usize * row_out..(y as usize + 1) * row_out];
                    kernels::ycc_to_bgra_row(level, [&luma, &cb, &cr], sub, h, y, row);
                }
                std::hint::black_box(&dst);
            });
            (level.name(), stats)
        })
        .collect();
    report(results, "kernel ycbcr420->bgra", FILE, mp, levels);
}

/// Prints one case at every level with the factor against the first
/// (lowest) level and stores the results.
fn report(results: &mut Vec<Result_>, case: &str, file: &str, mp: f64, levels: Vec<(&str, Stats)>) {
    let Some(base) = levels.first().map(|(_, s)| s.median) else {
        return;
    };
    for (level, stats) in levels {
        println!(
            "{:<14} {:<34} {:<10} median {:>8.2} ms  min {:>8.2} ms  {:>5.2}x",
            "simd",
            case,
            level,
            stats.median,
            stats.min,
            base / stats.median
        );
        results.push(Result_ {
            group: "simd".into(),
            candidate: format!("{case} [{level}]"),
            file: file.into(),
            megapixels: mp,
            stats,
        });
    }
}
