//! `blinzel-bench` as a program: exit codes, output and files
//! (`docs/specs/benchmark-gate.md`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bench(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blinzel-bench"))
        .args(args)
        .output()
        .expect("blinzel-bench")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A fresh folder for one test.
fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("blinzel-bench-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// AC-07: an unreadable corpus folder says to write the corpus first.
#[test]
fn run_without_a_corpus_says_to_write_it_and_exits_2() {
    let missing = temp("no-corpus").join("missing");
    let o = bench(&["run", missing.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("(run `blinzel-bench corpus` first)"),
        "{}",
        stderr(&o)
    );
}

/// AC-15: an unknown subcommand prints the usage.
#[test]
fn an_unknown_subcommand_prints_the_usage_and_exits_2() {
    for args in [&["frobnicate"][..], &[]] {
        let o = bench(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        let e = stderr(&o);
        assert!(e.starts_with("usage:"), "{e}");
        for cmd in [
            "corpus", "run", "compare", "jpeg-ab", "thumbs", "resize", "batch",
        ] {
            assert!(e.contains(&format!("blinzel-bench {cmd} ")), "{cmd}: {e}");
        }
    }
}

/// AC-15: `batch --format` with another format fails before any work.
#[test]
fn batch_with_an_unknown_format_exits_2() {
    let dir = temp("batch-format");
    let o = bench(&["batch", dir.to_str().unwrap(), "--format", "bmp"]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(stderr(&o).trim(), "unknown format bmp");
    assert!(!dir.join("thumbs-12mp").exists());
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// The string value of `"key": "…"` in a line `write_json` wrote.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    let pat = format!("\"{key}\": \"");
    let rest = &line[line.find(&pat).unwrap_or_else(|| panic!("{key}: {line}")) + pat.len()..];
    &rest[..rest.find('"').unwrap()]
}

/// The number value of `"key": n` in a line `write_json` wrote.
fn number(line: &str, key: &str) -> f64 {
    let pat = format!("\"{key}\": ");
    let rest = &line[line.find(&pat).unwrap_or_else(|| panic!("{key}: {line}")) + pat.len()..];
    rest.split([',', '}'])
        .next()
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// A small corpus: one JPEG under the name the `simd` group looks for
/// (big enough for the 1920 × 1080 crop), a PNG, an AVIF, a HEIC and an
/// AVIF that does not probe.
fn small_corpus(dir: &Path) {
    use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
    let (w, h) = (2880, 1620);
    let jpeg = encode_rgb(
        &blinzel_core::testimg::rgb(w, h),
        w,
        h,
        EncodeOptions {
            quality: 90,
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::write(dir.join("jpeg-24mp-420.jpg"), jpeg).unwrap();
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 64, 48);
        enc.set_color(png::ColorType::Rgb);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&blinzel_core::testimg::rgb(64, 48))
            .unwrap();
    }
    std::fs::write(dir.join("small.png"), png).unwrap();
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../blinzel-core/testdata");
    std::fs::copy(testdata.join("rgba-64x48.avif"), dir.join("a.avif")).unwrap();
    std::fs::copy(testdata.join("rgb-64x48.heic"), dir.join("h.heic")).unwrap();
    std::fs::write(dir.join("broken.avif"), b"not an avif").unwrap();
}

/// AC-03, AC-08, AC-10: `run --quick --json` times every group, prints one
/// line per result and writes the machine and every result.
#[test]
fn run_times_every_group_and_writes_every_result() {
    let dir = temp("run");
    small_corpus(&dir);
    let out = dir.join("out.json");
    let o = bench(&[
        "run",
        dir.to_str().unwrap(),
        "--quick",
        "--json",
        out.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let text = std::fs::read_to_string(&out).unwrap();

    // AC-08: the machine, then one object per result.
    for key in ["os", "arch", "cpu"] {
        assert!(!field(&text, key).is_empty(), "{key}");
    }
    assert!(number(&text, "logical_cpus") >= 1.0);
    for key in ["cpu_features", "build_features"] {
        assert!(text.contains(&format!("\"{key}\": [")), "{key}");
    }
    let rows: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("    {\"group\": "))
        .collect();
    let mut got: Vec<(String, String, String)> = Vec::new();
    for r in &rows {
        for key in [
            "megapixels",
            "mp_per_s",
            "min_ms",
            "median_ms",
            "p99_ms",
            "mean_ms",
        ] {
            number(r, key);
        }
        // AC-03: 3 timed runs with --quick, for every candidate.
        assert_eq!(number(r, "n"), 3.0, "{r}");
        got.push((
            field(r, "group").into(),
            field(r, "candidate").into(),
            field(r, "file").into(),
        ));
    }

    // AC-03: every group and candidate.
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let jpg = "jpeg-24mp-420.jpg";
    let mut want: Vec<(String, String, String)> = Vec::new();
    let mut add = |g: &str, c: &str, f: &str| want.push((g.into(), c.into(), f.into()));
    add("jpeg-header", "blinzel-jpeg parse", jpg);
    for c in [
        "turbojpeg packed BGRA".to_string(),
        "turbojpeg YCbCr planes".into(),
        "blinzel-jpeg YCbCr planes".into(),
        format!("blinzel-jpeg YCbCr planes {cpus} thr"),
        "zune-jpeg RGBA".into(),
    ] {
        add("jpeg-full", &c, jpg);
    }
    for s in ["1/2", "1/4", "1/8"] {
        add("jpeg-scaled", &format!("turbojpeg YCbCr {s}"), jpg);
        add("jpeg-scaled", &format!("blinzel-jpeg YCbCr {s}"), jpg);
        add(
            "jpeg-scaled",
            &format!("blinzel-jpeg YCbCr {s} {cpus} thr"),
            jpg,
        );
    }
    add("jpeg-screen", "turbojpeg YCbCr fit 1440p", jpg);
    add("jpeg-region", "turbojpeg crop 1920x1080", jpg);
    for c in [
        "blinzel-core (png + fused RGBA)",
        "zune-png native layout",
        "png crate native layout",
        "png crate + its RGBA expansion",
    ] {
        add("png", c, "small.png");
    }
    for t in [1, cpus] {
        add("avif", &format!("rav1d, {t} thread(s)"), "a.avif");
        add("heic", &format!("zune-heic, {t} thread(s)"), "h.heic");
    }
    // AC-10: libjpeg-turbo at every SIMD level, the own kernels at every
    // level on a 12 MP image.
    #[cfg(target_arch = "x86_64")]
    let turbo: Vec<&str> = if std::is_x86_feature_detected!("avx2") {
        vec!["none", "sse2", "avx2"]
    } else {
        vec!["none", "sse2"]
    };
    #[cfg(target_arch = "aarch64")]
    let turbo = vec!["none", "neon"];
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let turbo = vec!["none"];
    for case in [
        "turbojpeg YCbCr full",
        "turbojpeg BGRA full",
        "turbojpeg YCbCr 1/8",
    ] {
        for level in &turbo {
            add("simd", &format!("{case} [{level}]"), jpg);
        }
    }
    for case in [
        "kernel rgb->rgba",
        "kernel gray+alpha->rgba",
        "kernel rgba->bgra",
        "kernel ycbcr420->bgra",
    ] {
        for level in blinzel_core::kernels::Level::available() {
            add(
                "simd",
                &format!("{case} [{}]", level.name()),
                "testimg 4000x3000",
            );
        }
    }
    add("natsort", "SortKey + sort 10k", "-");
    add("alloc-96mb", "fresh Vec + touch", "-");
    add("alloc-96mb", "BufPool + touch", "-");
    got.sort();
    want.sort();
    assert_eq!(got, want);
    for r in rows
        .iter()
        .filter(|r| field(r, "file") == "testimg 4000x3000")
    {
        assert_eq!(number(r, "megapixels"), 12.0, "{r}");
    }

    // AC-03: one printed line per result; AC-10: the simd lines carry the
    // factor against the lowest level.
    let printed = stdout(&o);
    let lines: Vec<&str> = printed.lines().filter(|l| l.contains(" median ")).collect();
    assert_eq!(lines.len(), rows.len(), "{printed}");
    for (g, c, f) in &want {
        let l = lines
            .iter()
            .find(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                w[0] == g && l.contains(&format!(" {c} ")) && l.contains(&format!(" {f} "))
            })
            .unwrap_or_else(|| panic!("no line for {g} {c} {f}:\n{printed}"));
        assert!(l.contains(" min ") && l.contains(" ms "), "{l}");
        if g == "simd" {
            assert!(l.contains("x vs "), "{l}");
        }
    }
    assert!(
        printed
            .lines()
            .any(|l| l.starts_with("avif") && l.contains("skipped") && l.ends_with("broken.avif")),
        "{printed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
