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

/// A file in `write_json`'s layout with one result per entry.
fn result_json(cpu: &str, features: &str, rows: &[(&str, &str, f64)]) -> String {
    let rows: Vec<String> = rows
        .iter()
        .map(|(candidate, file, median)| {
            format!(
                "    {{\"group\": \"jpeg-full\", \"candidate\": \"{candidate}\", \"file\": \"{file}\", \"megapixels\": 24.000, \"mp_per_s\": 1.000, \"stats\": {{\"min_ms\": 1.000, \"median_ms\": {median:.3}, \"p99_ms\": 1.000, \"mean_ms\": 1.000, \"n\": 9}}}}"
            )
        })
        .collect();
    format!(
        "{{\n  \"os\": \"windows\", \"arch\": \"x86_64\", \"logical_cpus\": 4,\n  \"cpu\": \"{cpu}\", \"cpu_features\": [\"avx2\"], \"build_features\": [{features}],\n  \"results\": [\n{}\n  ]\n}}\n",
        rows.join(",\n")
    )
}

/// AC-09: `compare` reads two runs, prints the table of the shared results
/// and appends it to `$GITHUB_STEP_SUMMARY`.
#[test]
fn compare_prints_the_shared_results_and_appends_them_to_the_step_summary() {
    let dir = temp("compare");
    let (a, b) = (dir.join("base.json"), dir.join("v3.json"));
    std::fs::write(
        &a,
        result_json(
            "CPU A",
            "",
            &[("turbo", "x.jpg", 30.0), ("only-a", "x.jpg", 1.0)],
        ),
    )
    .unwrap();
    std::fs::write(
        &b,
        result_json(
            "CPU B",
            "\"avx2\", \"fma\"",
            &[("turbo", "x.jpg", 20.0), ("turbo", "y.jpg", 1.0)],
        ),
    )
    .unwrap();
    let summary = dir.join("summary.md");
    std::fs::write(&summary, "before\n").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_blinzel-bench"))
        .args(["compare", a.to_str().unwrap(), b.to_str().unwrap()])
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let md = stdout(&o);
    assert!(md.contains("| CPU | CPU A | CPU B |"), "{md}");
    assert!(md.contains("| build features | - | avx2 fma |"), "{md}");
    assert!(
        md.contains("| jpeg-full | turbo | x.jpg | 30.000 | 20.000 | 1.50x |"),
        "{md}"
    );
    assert!(!md.contains("only-a") && !md.contains("y.jpg"), "{md}");
    assert_eq!(
        std::fs::read_to_string(&summary).unwrap(),
        format!("before\n{md}")
    );

    // Without the variable nothing is written.
    std::fs::remove_file(&summary).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_blinzel-bench"))
        .args(["compare", a.to_str().unwrap(), b.to_str().unwrap()])
        .env_remove("GITHUB_STEP_SUMMARY")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(stdout(&o), md);
    assert!(!summary.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// AC-11: `jpeg-ab` decodes a file with both decoders and prints both.
#[test]
fn jpeg_ab_prints_both_decoders_and_their_ratio() {
    use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
    let dir = temp("jpeg-ab");
    let (w, h) = (256, 192);
    let jpeg = encode_rgb(
        &blinzel_core::testimg::rgb(w, h),
        w,
        h,
        EncodeOptions::default(),
    )
    .unwrap();
    let file = dir.join("a.jpg");
    std::fs::write(&file, jpeg).unwrap();
    let o = bench(&["jpeg-ab", file.to_str().unwrap(), "4", "2", "3"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains(" 4/8, 2 threads: turbojpeg median "), "{out}");
    assert!(
        out.contains(" | blinzel-jpeg median ") && out.contains(" | ratio "),
        "{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The entries of a folder whose names start with `prefix`.
fn entries(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .collect();
    v.sort();
    v
}

/// The size of a JPEG file.
fn jpeg_size(path: &Path) -> (u32, u32) {
    let info = blinzel_jpeg::parse(&std::fs::read(path).unwrap()).unwrap();
    (info.width, info.height)
}

/// AC-12, AC-14: `thumbs` writes its photos once and measures cold, store
/// and warm; `batch` resizes them (or `--from`'s files) and keeps the
/// outputs only with `--keep`.
#[test]
fn thumbs_and_batch_measure_the_photo_corpus() {
    let dir = temp("thumbs");
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let with_tmp = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_blinzel-bench"))
            .args(args)
            // `std::env::temp_dir` reads TMPDIR on Unix, TMP then TEMP on Windows.
            .env("TMPDIR", &tmp)
            .env("TMP", &tmp)
            .env("TEMP", &tmp)
            .output()
            .unwrap()
    };
    let d = dir.to_str().unwrap();

    // AC-12: two distinct 12 MP photos, written once.
    let o = with_tmp(&["thumbs", d, "--count", "2", "--threads", "2"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let photos = entries(&dir.join("thumbs-12mp"), "photo-");
    assert_eq!(photos.len(), 2);
    for p in &photos {
        assert_eq!(jpeg_size(p), (4000, 3000));
    }
    let bytes: Vec<Vec<u8>> = photos.iter().map(|p| std::fs::read(p).unwrap()).collect();
    assert_ne!(bytes[0], bytes[1]);
    let out = stdout(&o);
    assert!(
        out.contains("cold:  2/2 thumbnails in ") && out.contains(" on 2 threads = "),
        "{out}"
    );
    assert!(out.contains("store: 2 thumbnails in "), "{out}");
    assert!(
        out.contains("warm:  2/2 from the cache in ") && out.contains("(open "),
        "{out}"
    );
    assert!(entries(&tmp, "blinzel-bench-thumbs-").is_empty());

    let modified = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
    let before: Vec<_> = photos.iter().map(|p| modified(p)).collect();
    let o = with_tmp(&["thumbs", d, "--count", "2", "--keep-cache"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let after: Vec<_> = photos.iter().map(|p| modified(p)).collect();
    assert_eq!(before, after, "the photos were written again");
    assert_eq!(entries(&tmp, "blinzel-bench-thumbs-").len(), 1);

    // AC-14: the photos to a long edge of 64 as JPEG, kept in `--keep`.
    let keep = dir.join("keep");
    let o = with_tmp(&[
        "batch",
        d,
        "--count",
        "2",
        "--edge",
        "64",
        "--keep",
        keep.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(
        out.starts_with("2 of 2 files (12 MP JPEG → Jpeg, long edge 64) in "),
        "{out}"
    );
    assert!(
        out.contains("/s, ") && out.trim_end().ends_with(" KiB per file"),
        "{out}"
    );
    let made = entries(&keep, "");
    assert_eq!(made.len(), 2);
    for p in &made {
        assert_eq!(jpeg_size(p), (64, 48));
    }

    // Without `--keep` the outputs are deleted; `--from` takes its files.
    let from = dir.join("from");
    std::fs::create_dir_all(&from).unwrap();
    std::fs::copy(&photos[0], from.join("one.jpg")).unwrap();
    let o = with_tmp(&[
        "batch",
        d,
        "--from",
        from.to_str().unwrap(),
        "--edge",
        "32",
        "--format",
        "png",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(
        stdout(&o).starts_with("1 of 1 files (12 MP JPEG → Png, long edge 32)"),
        "{}",
        stdout(&o)
    );
    assert!(entries(&tmp, "blinzel-bench-batch-").is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// AC-13: `resize --quick` prints all four cases with both timings and the
/// largest difference.
#[test]
fn resize_prints_every_case() {
    let o = bench(&["resize", "--quick", "--runs", "1"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["case", "own", "ms", "fir", "ms", "max", "diff"]
    );
    let names = [
        "24 MP -> 1600",
        "24 MP -> 400",
        "12 MP -> 2048 alpha",
        "2 MP -> 4 MP up",
    ];
    assert_eq!(lines.len(), 1 + names.len(), "{out}");
    for (l, name) in lines[1..].iter().zip(names) {
        assert!(l.starts_with(name), "{l}");
        let nums: Vec<f64> = l[name.len()..]
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(nums.len(), 3, "{l}");
        assert!(nums[0] > 0.0 && nums[1] > 0.0 && nums[2] <= 255.0, "{l}");
    }
}
