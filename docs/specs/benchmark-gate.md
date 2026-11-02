---
status: todo
created: 2026-11-02
size: L
platforms: all
design: [blinzel.md §3, blinzel.md §12 (M0), blinzel.md §13, blinzel.md §15]
---

# Benchmark gate

## Context

No library is adopted unchecked (`blinzel.md` §1, §3; AGENTS.md §4): for each hot path the library (the yardstick) and the in-house candidate are measured on the same deterministic input, the faster one ships, and the library stays the pixel reference. `crates/blinzel-bench` is the tool for this: it writes the corpus, times the candidates, checks the in-house JPEG decoder byte for byte against libjpeg-turbo, and has focused sub-benchmarks for the alternating JPEG comparison, thumbnails, batch resizing and the resampler. The Windows app measures its own end-to-end pipeline with `blinzel.exe --bench`. The Windows CI job runs both and fails when the gate is broken.

## Scope

### In scope

- `blinzel-bench corpus`, `run` (with `--quick`, `--check`, `--json`), `compare`, `jpeg-ab`, `thumbs`, `resize`, `batch`, and the hidden `simd-child`.
- The gate check of `run --check`, including its noise handling.
- `blinzel.exe --bench <dir> [--warp] [--json out.json]` (Windows only).
- How the Windows CI job uses them.

### Out of scope

- The decoders, thumbnails and startup measurements themselves — because they have their own specs; this spec covers the tooling that measures them.
- Gates against WIC for WebP, JPEG XL, TIFF, RAW, HEIC and AVIF — because `blinzel-bench` has no WIC candidate; they are measured by hand with `BLINZEL_DECODER=native|wic` and `blinzel.exe --bench` on Windows.
- Gates against the Windows thumbnail provider and thumbnail cache, the FindFirstFileExW vs. `GetFileInformationByHandleEx` listing comparison, and ImageMagick for batch resizing — because the tooling does not contain these yardsticks; the design spec numbers come from one-off measurements.
- Failing CI on a speed budget (e.g. `thumb_grid_200` ≤ 30 ms) — because no step compares numbers against a budget; only the JPEG gate fails the build.
- Linux and macOS equivalents of `blinzel --bench` — because it exists only in the Windows shell.

## Acceptance criteria

- [ ] AC-01 (event): When `corpus <dir>` runs, then it writes `jpeg-12mp-420.jpg`, `jpeg-24mp-420.jpg`, `jpeg-24mp-422.jpg`, `jpeg-24mp-444.jpg`, `jpeg-24mp-420-progressive.jpg`, `jpeg-24mp-420-restart.jpg`, `jpeg-24mp-gray.jpg`, `jpeg-45mp-420.jpg` (quality 90), `png-12mp-rgb.png`, `png-12mp-rgba.png` and `png-8mp-screenshot.png`, with identical bytes on every run, and keeps files that already exist.
- [ ] AC-02 (option): Where `--large` is set, `corpus` also writes `jpeg-100mp-420.jpg`.
- [ ] AC-03 (event): When `run <dir>` runs, then every candidate is timed after one untimed warm-up over 9 runs (3 with `--quick`), and one line per result shows group, candidate, file, median, minimum and megapixels per second; the groups are `jpeg-header`, `jpeg-full` (libjpeg-turbo packed and YCbCr planes, blinzel-jpeg on 1 and all logical CPUs, zune-jpeg), `jpeg-scaled` (1/2, 1/4, 1/8), `jpeg-screen`, `jpeg-region`, `png` (blinzel-core, zune-png, png crate with and without its RGBA expansion), `avif` and `heic` (1 and all threads, for such files placed in the folder), `simd`, `natsort` and `alloc-96mb` (fresh buffer vs. buffer pool).
- [ ] AC-04 (option, CI: windows): Where `--check` is set, `run` fails if blinzel-jpeg, decoding on all logical CPUs, differs from libjpeg-turbo in any row of any plane at full size or 1/8 for any corpus JPEG blinzel-jpeg supports.
- [ ] AC-05 (option, CI: windows): Where `--check` is set, `run` fails if zune-jpeg beats the shipped libjpeg-turbo YCbCr path on a JPEG in both the median and the minimum, and the message names the file, both medians and minima and the likely cause (an unoptimized C build).
- [ ] AC-06 (state): While zune-jpeg has the lower median on a file but libjpeg-turbo still has the lower or equal minimum (a disturbed runner), the gate does not fail.
- [ ] AC-07 (error): If the corpus folder cannot be read, then `run` fails with a hint to run `blinzel-bench corpus` first and exits with status 2.
- [ ] AC-08 (option): Where `--json out.json` is set, `run` writes `os`, `arch`, `logical_cpus`, `cpu`, `cpu_features`, `build_features` and one object per result on its own line with `group`, `candidate`, `file`, `megapixels`, `mp_per_s` and `stats` (`min_ms`, `median_ms`, `p99_ms`, `mean_ms`, `n`).
- [ ] AC-09 (event): When `compare a.json b.json` runs, then it prints a Markdown table with both runs' CPU and build features and, for every result present in both (same group, candidate and file), both medians and the factor a/b ("-" when a median is 0), and appends the table to `$GITHUB_STEP_SUMMARY` when that is set.
- [ ] AC-10 (event): When `run` finds `jpeg-24mp-420.jpg`, then the `simd` group holds libjpeg-turbo's YCbCr full, BGRA full and YCbCr 1/8 decode at each SIMD level the CPU supports (none, SSE2, AVX2 on x86-64; none, NEON on ARM64), each level in its own child process, plus the own pixel kernels at every available level on a 12 MP image, each with the factor against the lowest level.
- [ ] AC-11 (event): When `jpeg-ab <file> [scale] [threads] [rounds]` runs, then libjpeg-turbo and blinzel-jpeg decode alternately in one process for `rounds` (default 15) rounds after one discarded round, at `scale`/8 (default 8) on `threads` (default all logical CPUs), and it prints both medians and minima and their ratios.
- [ ] AC-12 (event): When `thumbs <dir> [--count N] [--threads T]` runs, then it writes N (default 200) distinct noisy 12 MP JPEGs to `<dir>/thumbs-12mp/` once and prints cold thumbnails per second (read, decode, reduce on T threads), the time to store them in a fresh pack cache, and the time to reopen the cache and read all back; the cache is deleted afterwards unless `--keep-cache` is set.
- [ ] AC-13 (event): When `resize [--runs N] [--quick]` runs, then for 24 MP → 1600 px, 24 MP → 400 px, 12 MP with alpha → 2048 px and 2 MP → 4 MP it prints the median of N (default 5) runs of the own resampler and of `fast_image_resize` with the same math, and the largest channel difference (alpha only where the image has alpha); `--quick` uses a quarter of every dimension.
- [ ] AC-14 (event): When `batch <dir> [--count N] [--threads T] [--format jpeg|png|webp|avif] [--edge E] [--from <dir>] [--keep <dir>]` runs, then the batch engine resizes N (default 48) 12 MP JPEGs of the `thumbs` corpus, or the files of `--from`, to long edge E (default 1920) in the format (default JPEG), and it prints files per second and KiB per file; outputs are deleted unless `--keep` names a folder.
- [ ] AC-15 (error): If the subcommand is unknown or `batch --format` names another format, then the tool prints the usage or "unknown format …" and exits with status 2.
- [ ] AC-16 (always, CI: windows): Every Windows CI run generates the corpus, runs `run --check --json bench-libs-windows.json`, runs `run --quick --json bench-libs-windows-v3.json` from a build for `x86-64-v3`, puts both side by side with `compare` in the step summary, and uploads all `bench-*.json` as the artifact `windows-bench-and-exe`, also when a step failed.
- [ ] AC-17 (event, CI: windows): When `blinzel.exe --bench <dir> --warp --json out.json` runs on the corpus, then it exits with status 0 and writes its report to `out.json`.
- [ ] AC-18 (event, CI: windows): When `blinzel.exe --bench <dir>` runs, then its report holds, on a 2560 × 1440 offscreen target, per file `read_ms`, `probe_ms`, `decode_ms`, `upload_ms`, `render_ms`, `total_ms` with full and stage size; `navigation` with presses at 30 Hz (twice the file count, at least 8), prefetch hits (under 1 ms) and latency stats; `list_10k` (listing a 10,000-file folder, 5 runs); and `thumb_grid_200` (200 thumbnails from a fresh cache into the atlas and on screen, 5 runs after a warm-up).
- [ ] AC-19 (error, CI: windows): If the folder has no images, or a step of the pipeline fails, then `blinzel.exe --bench` prints "bench failed: …" and exits with status 1; a file that cannot be probed is listed with its error and the run goes on.

## Technical notes

- Files: `crates/blinzel-bench/src/main.rs` (`corpus`, `run`, `verify_own_jpeg`, `zune_wins`, `check`, `jpeg_ab`, `write_json`), `compare.rs`, `simd.rs` (`simd-child <file> <iters>`, `JSIMD_FORCE*`), `thumbs.rs`, `resize.rs`, `batch.rs`; `crates/blinzel-app/src/bench.rs` (Windows, `#[cfg(windows)]` in `lib.rs`), dispatched from `--bench` in `crates/blinzel-app/src/run.rs`.
- Statistics: `blinzel_core::timing::Stats` (`min`, `median`, `p99`, `mean`, `n`); JSON numbers with three decimals.
- Corpus images come from `blinzel_core::testimg` (fixed-seed xorshift), encoded with `blinzel_core::decode::jpeg::encode_rgb` and the `png` crate (`Compression::Balanced`).
- `blinzel-bench` enables the `heic` feature of `blinzel-core` (not distributed) and, on x86-64, `av1-asm`.
- `blinzel.exe --bench` decodes the visible image on all P-cores and shows a covering RAW preview where one exists, like the viewer; `BLINZEL_DECODER=native|wic` (`crates/blinzel-app/src/wic.rs`) puts one decoder first for gates against WIC.
- CI (`.github/workflows/ci.yml`, job `windows`, `windows-2025`): `Generate corpus`, `Library vs. own code benchmarks`, `Benchmarks built for x86-64-v3` (`RUSTFLAGS=-C target-feature=+crt-static -C target-cpu=x86-64-v3`, own `CARGO_TARGET_DIR`), `Compare with x86-64-v3`, `App benchmark (offscreen, WARP)` (fails on a non-zero exit), artifact upload with `if: always()`.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| One disturbed CI runner raises the median of one file (CI run 41056426295: turbo median 165.8 ms, min 121 ms; zune 163.4 / 158.7 ms) | Not a win for zune-jpeg; the gate passes |
| libjpeg-turbo built without optimization (MSVC `/Od`) | zune-jpeg wins median and minimum; `run --check` fails with the hint to fix the C build |
| A JPEG blinzel-jpeg does not support (arithmetic, 12-bit, CMYK, …) placed in the folder | Timed for libjpeg-turbo and zune-jpeg only; not byte-checked |
| An AVIF or HEIC file in the folder that does not probe | Printed as "skipped"; the run goes on |
| `compare` with runs that have different result sets | Only results present in both appear |
| A timing below 0.0005 ms in the JSON | Reads as 0; `compare` prints "-" as factor |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `corpus_lists_every_file_at_its_size`, `corpus_is_identical_on_every_run_and_keeps_existing_files` (`crates/blinzel-bench/src/main.rs`) | built | |
| AC-02 | `corpus_lists_every_file_at_its_size`, `corpus_is_identical_on_every_run_and_keeps_existing_files` (`crates/blinzel-bench/src/main.rs`) | built | |
| AC-03 | — (CI step `Library vs. own code benchmarks` runs it; groups not asserted) | open | |
| AC-04 | CI step `Library vs. own code benchmarks` (`run … --check`, Windows job) | built | |
| AC-05 | CI step `Library vs. own code benchmarks` (Windows job); `a_noisy_median_alone_is_no_win` (`crates/blinzel-bench/src/main.rs`, Linux CI) for the decision | built | |
| AC-06 | `a_noisy_median_alone_is_no_win` (`crates/blinzel-bench/src/main.rs`, Linux CI) | built | |
| AC-07 | `run_without_a_corpus_says_to_write_it_and_exits_2` (`crates/blinzel-bench/tests/cli.rs`) | built | |
| AC-08 | — (`write_json` untested; `stats` in `crates/blinzel-core/src/timing.rs` covers only the `stats` object) | open | |
| AC-09 | — (`reads_fields` in `crates/blinzel-bench/src/compare.rs` covers only reading the fields; CI step `Compare with x86-64-v3` runs it) | open | |
| AC-10 | — | open | |
| AC-11 | — | open | |
| AC-12 | — | open | |
| AC-13 | — | open | |
| AC-14 | — | open | |
| AC-15 | `an_unknown_subcommand_prints_the_usage_and_exits_2`, `batch_with_an_unknown_format_exits_2` (`crates/blinzel-bench/tests/cli.rs`) | built | |
| AC-16 | `.github/workflows/ci.yml`, job `windows`, steps `Generate corpus` … `upload-artifact` | built | |
| AC-17 | CI step `App benchmark (offscreen, WARP)` (Windows job, fails on a non-zero exit) | built | |
| AC-18 | — (the CI step prints the report but asserts no fields) | open | |
| AC-19 | — | open | |

## Decisions

- 2026-11-02: Extracted from the code. The only automated gate is the JPEG one (byte identity of blinzel-jpeg and libjpeg-turbo ahead of zune-jpeg); all other comparisons print numbers for a person to judge.
- 2026-11-02: The corpus is a table (`corpus_files`) written by `write_corpus(dir, large, shrink)`; the tests write it with every side divided by 40, because the full corpus takes minutes in a test build. The behavior existed, so the tests were green at once; each was checked by breaking the code on purpose (no `exists` check, quality 85, a time-dependent pixel) and goes red.
- 2026-11-02: The CLI tests in `tests/cli.rs` run the real binary (`CARGO_BIN_EXE_blinzel-bench`), because the exit status is set in `main`. The usage now lists `jpeg-ab`, `--keep-cache`, `--from` and `--keep` too; the usage test was red before that.
