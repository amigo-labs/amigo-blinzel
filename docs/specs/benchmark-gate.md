---
status: implemented
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

- [x] AC-01 (event): When `corpus <dir>` runs, then it writes `jpeg-12mp-420.jpg`, `jpeg-24mp-420.jpg`, `jpeg-24mp-422.jpg`, `jpeg-24mp-444.jpg`, `jpeg-24mp-420-progressive.jpg`, `jpeg-24mp-420-restart.jpg`, `jpeg-24mp-gray.jpg`, `jpeg-45mp-420.jpg` (quality 90), `png-12mp-rgb.png`, `png-12mp-rgba.png` and `png-8mp-screenshot.png`, with identical bytes on every run, and keeps files that already exist.
- [x] AC-02 (option): Where `--large` is set, `corpus` also writes `jpeg-100mp-420.jpg`.
- [x] AC-03 (event): When `run <dir>` runs, then every candidate is timed after one untimed warm-up over 9 runs (3 with `--quick`), and one line per result shows group, candidate, file, median, minimum and megapixels per second; the groups are `jpeg-header`, `jpeg-full` (libjpeg-turbo packed and YCbCr planes, blinzel-jpeg on 1 and all logical CPUs, zune-jpeg), `jpeg-scaled` (1/2, 1/4, 1/8), `jpeg-screen`, `jpeg-region`, `png` (blinzel-core, zune-png, png crate with and without its RGBA expansion), `avif` and `heic` (1 and all threads, for such files placed in the folder), `simd`, `natsort` and `alloc-96mb` (fresh buffer vs. buffer pool).
- [x] AC-04 (option, CI: windows): Where `--check` is set, `run` fails if blinzel-jpeg, decoding on all logical CPUs, differs from libjpeg-turbo in any row of any plane at full size or 1/8 for any corpus JPEG blinzel-jpeg supports.
- [x] AC-05 (option, CI: windows): Where `--check` is set, `run` fails if zune-jpeg beats the shipped libjpeg-turbo YCbCr path on a JPEG in both the median and the minimum, and the message names the file, both medians and minima and the likely cause (an unoptimized C build).
- [x] AC-06 (state): While zune-jpeg has the lower median on a file but libjpeg-turbo still has the lower or equal minimum (a disturbed runner), the gate does not fail.
- [x] AC-07 (error): If the corpus folder cannot be read, then `run` fails with a hint to run `blinzel-bench corpus` first and exits with status 2.
- [x] AC-08 (option): Where `--json out.json` is set, `run` writes `os`, `arch`, `logical_cpus`, `cpu`, `cpu_features`, `build_features` and one object per result on its own line with `group`, `candidate`, `file`, `megapixels`, `mp_per_s` and `stats` (`min_ms`, `median_ms`, `p99_ms`, `mean_ms`, `n`).
- [x] AC-09 (event): When `compare a.json b.json` runs, then it prints a Markdown table with both runs' CPU and build features and, for every result present in both (same group, candidate and file), both medians and the factor a/b ("-" when a median is 0), and appends the table to `$GITHUB_STEP_SUMMARY` when that is set.
- [x] AC-10 (event): When `run` finds `jpeg-24mp-420.jpg`, then the `simd` group holds libjpeg-turbo's YCbCr full, BGRA full and YCbCr 1/8 decode at each SIMD level the CPU supports (none, SSE2, AVX2 on x86-64; none, NEON on ARM64), each level in its own child process, plus the own pixel kernels at every available level on a 12 MP image, each with the factor against the lowest level.
- [x] AC-11 (event): When `jpeg-ab <file> [scale] [threads] [rounds]` runs, then libjpeg-turbo and blinzel-jpeg decode alternately in one process for `rounds` (default 15) rounds after one discarded round, at `scale`/8 (default 8) on `threads` (default all logical CPUs), and it prints both medians and minima and their ratios.
- [x] AC-12 (event): When `thumbs <dir> [--count N] [--threads T]` runs, then it writes N (default 200) distinct noisy 12 MP JPEGs to `<dir>/thumbs-12mp/` once and prints cold thumbnails per second (read, decode, reduce on T threads), the time to store them in a fresh pack cache, and the time to reopen the cache and read all back; the cache is deleted afterwards unless `--keep-cache` is set.
- [x] AC-13 (event): When `resize [--runs N] [--quick]` runs, then for 24 MP → 1600 px, 24 MP → 400 px, 12 MP with alpha → 2048 px and 2 MP → 4 MP it prints the median of N (default 5) runs of the own resampler and of `fast_image_resize` with the same math, and the largest channel difference (alpha only where the image has alpha); `--quick` uses a quarter of every dimension.
- [x] AC-14 (event): When `batch <dir> [--count N] [--threads T] [--format jpeg|png|webp|avif] [--edge E] [--from <dir>] [--keep <dir>]` runs, then the batch engine resizes N (default 48) 12 MP JPEGs of the `thumbs` corpus, or the files of `--from`, to long edge E (default 1920) in the format (default JPEG), and it prints files per second and KiB per file; outputs are deleted unless `--keep` names a folder.
- [x] AC-15 (error): If the subcommand is unknown or `batch --format` names another format, then the tool prints the usage or "unknown format …" and exits with status 2.
- [x] AC-16 (always, CI: windows): Every Windows CI run generates the corpus, runs `run --check --json bench-libs-windows.json`, runs `run --quick --json bench-libs-windows-v3.json` from a build for `x86-64-v3`, puts both side by side with `compare` in the step summary, and uploads all `bench-*.json` as the artifact `windows-bench-and-exe`, also when a step failed.
- [x] AC-17 (event, CI: windows): When `blinzel.exe --bench <dir> --warp --json out.json` runs on the corpus, then it exits with status 0 and writes its report to `out.json`.
- [x] AC-18 (event, CI: windows): When `blinzel.exe --bench <dir>` runs, then its report holds, on a 2560 × 1440 offscreen target, per file `read_ms`, `probe_ms`, `decode_ms`, `upload_ms`, `render_ms`, `total_ms` with full and stage size; `navigation` with presses at 30 Hz (twice the file count, at least 8), prefetch hits (under 1 ms) and latency stats; `list_10k` (listing a 10,000-file folder, 5 runs); and `thumb_grid_200` (200 thumbnails from a fresh cache into the atlas and on screen, 5 runs after a warm-up).
- [x] AC-19 (error, CI: windows): If the folder has no images, or a step of the pipeline fails, then `blinzel.exe --bench` prints "bench failed: …" and exits with status 1; a file that cannot be probed is listed with its error and the run goes on.

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
| AC-01 | `corpus_lists_every_file_at_its_size`, `corpus_is_identical_on_every_run_and_keeps_existing_files` (`crates/blinzel-bench/src/main.rs`) | verified | 7e2844f |
| AC-02 | `corpus_lists_every_file_at_its_size`, `corpus_is_identical_on_every_run_and_keeps_existing_files` (`crates/blinzel-bench/src/main.rs`) | verified | 7e2844f |
| AC-03 | `every_candidate_runs_once_untimed_then_9_or_3_times`, `a_result_line_shows_group_candidate_file_median_min_and_rate` (`crates/blinzel-bench/src/main.rs`); `run_times_every_group_and_writes_every_result` (`crates/blinzel-bench/tests/cli.rs`); red with 8 runs | verified | 05172de |
| AC-04 | `check_fails_on_a_blinzel_jpeg_mismatch`, `a_differing_plane_row_fails_the_comparison`, `check_verifies_full_size_and_one_eighth_on_all_logical_cpus` (`crates/blinzel-bench/src/main.rs`; red with the scales `[8]` and with one thread); CI step `Library vs. own code benchmarks` (`run … --check`, Windows job, CI run 41547804490, `95b1fe8`: "check passed") | verified | 9025482 |
| AC-05 | `check_fails_on_a_zune_jpeg_win_and_names_file_medians_minima_and_cause`, `a_noisy_median_alone_is_no_win`, `a_tie_in_the_median_or_the_minimum_is_no_win` (`crates/blinzel-bench/src/main.rs`; the tie test red with `>=` in the median or the minimum of `zune_wins`); CI step `Library vs. own code benchmarks` (Windows job, CI run 41547804490, `95b1fe8`) | verified | 9025482 |
| AC-06 | `a_noisy_median_alone_is_no_win`, `a_tie_in_the_median_or_the_minimum_is_no_win` (`crates/blinzel-bench/src/main.rs`; both red with `zune_wins` reading the median only) | verified | 80d77b1 |
| AC-07 | `run_without_a_corpus_says_to_write_it_and_exits_2` (`crates/blinzel-bench/tests/cli.rs`) | verified | 7e2844f |
| AC-08 | `json_has_the_machine_and_one_result_per_line` (`crates/blinzel-bench/src/main.rs`); `run_times_every_group_and_writes_every_result` (`crates/blinzel-bench/tests/cli.rs`) | verified | 05172de |
| AC-09 | `the_table_has_both_machines_and_the_shared_results` (`crates/blinzel-bench/src/compare.rs`); `compare_prints_the_shared_results_and_appends_them_to_the_step_summary` (`crates/blinzel-bench/tests/cli.rs`) | verified | 18145ec |
| AC-10 | `each_level_runs_in_a_child_with_only_its_variable`, `the_factor_is_against_the_lowest_level` (`crates/blinzel-bench/src/simd.rs`); `run_times_every_group_and_writes_every_result` (`crates/blinzel-bench/tests/cli.rs`) | verified | 05172de |
| AC-11 | `jpeg_ab_defaults_to_full_size_on_all_cpus_for_15_rounds`, `jpeg_ab_alternates_and_discards_the_first_round`, `jpeg_ab_prints_both_medians_minima_and_ratios` (`crates/blinzel-bench/src/main.rs`; red with 14 default rounds); `jpeg_ab_prints_both_decoders_and_their_ratio` (`crates/blinzel-bench/tests/cli.rs`) | verified | 18145ec |
| AC-12 | `thumbs_defaults_to_200_photos_on_all_cpus` (`crates/blinzel-bench/src/thumbs.rs`); `thumbs_and_batch_measure_the_photo_corpus` (`crates/blinzel-bench/tests/cli.rs`) | verified | 18145ec |
| AC-13 | `resize_has_four_cases_quartered_when_quick`, `the_difference_counts_alpha_only_where_the_image_has_it` (`crates/blinzel-bench/src/resize.rs`); `resize_prints_every_case` (`crates/blinzel-bench/tests/cli.rs`) | verified | 18145ec |
| AC-14 | `batch_defaults_to_48_jpegs_at_1920` (`crates/blinzel-bench/src/batch.rs`); `thumbs_and_batch_measure_the_photo_corpus` (`crates/blinzel-bench/tests/cli.rs`) | verified | 18145ec |
| AC-15 | `an_unknown_subcommand_prints_the_usage_and_exits_2`, `batch_with_an_unknown_format_exits_2` (`crates/blinzel-bench/tests/cli.rs`) | verified | 7e2844f |
| AC-16 | `.github/workflows/ci.yml`, job `windows`, steps `Generate corpus` … `upload-artifact` (`if: always()`); all green in CI run 41547804490 (`95b1fe8`), the step summary holds the `compare` table | verified | — (on `main`) |
| AC-17 | CI step `App benchmark (offscreen, WARP)` (Windows job, exits with the app's status), green in CI run 41547804490 (`95b1fe8`) | verified | — (on `main`) |
| AC-18 | `the_report_holds_every_measurement` (`crates/blinzel-app/src/bench.rs`), Windows job, CI run 41547804490 (`95b1fe8`) | verified | 8959678 |
| AC-19 | `the_report_holds_every_measurement` (a file that does not probe), `a_failing_step_ends_the_run_with_exit_code_1`, `a_folder_without_images_fails_with_exit_code_1` (`crates/blinzel-app/src/bench.rs`), Windows job, CI run 41547804490 (`95b1fe8`) | verified | 8959678 |

## Summary

> Completed: 2026-11-02

- `blinzel-bench corpus` writes the deterministic JPEG/PNG corpus (`--large` adds 100 MP) and keeps existing files; `run` times every candidate group after a warm-up and writes the machine and every result to JSON.
- `run --check` is the JPEG gate: blinzel-jpeg must match libjpeg-turbo byte for byte at full size and 1/8 on all logical CPUs, and zune-jpeg fails it only when strictly faster in both the median and the minimum.
- `compare`, `jpeg-ab`, `thumbs`, `resize` and `batch` print what they measure; bad input exits with status 2.
- `blinzel.exe --bench` (Windows) reports the per-file pipeline, navigation, `list_10k` and `thumb_grid_200`, and exits 1 on a failed step; the Windows CI job runs the gate, the x86-64-v3 comparison and the app benchmark on every push.

## Decisions

- 2026-11-02: Extracted from the code. The only automated gate is the JPEG one (byte identity of blinzel-jpeg and libjpeg-turbo ahead of zune-jpeg); all other comparisons print numbers for a person to judge.
- 2026-11-02: The corpus is a table (`corpus_files`) written by `write_corpus(dir, large, shrink)`; the tests write it with every side divided by 40, because the full corpus takes minutes in a test build. The behavior existed, so the tests were green at once; each was checked by breaking the code on purpose (no `exists` check, quality 85, a time-dependent pixel) and goes red.
- 2026-11-02: The CLI tests in `tests/cli.rs` run the real binary (`CARGO_BIN_EXE_blinzel-bench`), because the exit status is set in `main`. The usage now lists `jpeg-ab`, `--keep-cache`, `--from` and `--keep` too; the usage test was red before that.
- 2026-11-02: `jpeg-header` was timed over 10× the runs of every other candidate (90, 30 with `--quick`); it now gets 9 (3) like the rest, as AC-03 says. A header parse takes microseconds, so its median is noisier, and no gate reads it.
- 2026-11-02: Every result prints the same line (`line`: group, candidate, file, median, minimum, MP/s or "-" without pixels), as AC-03 says; `natsort`, `alloc-96mb` and `simd` used to print shorter lines without file, minimum or rate. The `simd` lines append the factor against the lowest level (`  2.00x vs none`).
- 2026-11-02: The `run` test runs the real binary on a small folder (a 2880 × 1620 JPEG named `jpeg-24mp-420.jpg` so the `simd` group runs, a PNG, the AVIF and HEIC fixtures of `blinzel-core`, a broken AVIF). It takes about 75 s in a test build, mostly the own kernels on the 12 MP image AC-10 asks for, because workspace crates are not optimized in the test profile. The JSON machine fields and the result layout are checked exactly by a unit test on `json(host, results)`.
- 2026-11-02: `compare`, `jpeg-ab`, `thumbs`, `resize` and `batch` keep their measuring code; what decides the output moved into small functions with unit tests (`table`, `ab_args`, `alternate`, `ab_line`, `options` in `thumbs.rs` and `batch.rs`, `cases`, `max_diff`, `runs`), and `tests/cli.rs` runs each command once on small input (`thumbs --count 2`, `batch --edge 64`, `resize --quick --runs 1`). `TMPDIR` points the cache and the batch outputs at the test's folder, so the tests see them deleted. `jpeg-ab` with 0 rounds now says "rounds must be at least 1" instead of panicking.
- 2026-11-02: AC-18, AC-19: `blinzel.exe --bench` is Windows only, so its tests in `crates/blinzel-app/src/bench.rs` run only in the Windows CI job (`cargo test --workspace --release`, WARP). They were type-checked and clippy-checked here for `x86_64-pc-windows-msvc`, not run; their first Windows CI run is the proof. The folder holds two PNGs (written with the `png` crate, so the test does not need the `turbojpeg` feature) and a broken file named to sort last: the thumbnail grid uses the folder's first image, so a first file that does not probe would end the run. The "bench failed: …" text is `failed(e)`, because a unit test cannot read the process's stderr; a step that fails is the listing of a missing folder.
- 2026-11-02: `zune_wins` counted equal values as a zune-jpeg win (`>=`), against AC-05 ("beats") and AC-06 ("lower or equal minimum"); it now needs zune-jpeg strictly faster in both the median and the minimum. `a_tie_in_the_median_or_the_minimum_is_no_win` was red before the fix.
- 2026-11-02: AC-04's "at full size or 1/8" and "on all logical CPUs" are pinned by a test: `verify_own_jpeg` calls `verify_own_jpeg_with(data, own_jpeg)`, which takes blinzel-jpeg's decode as a parameter and reads the thread count itself (`available_parallelism`) instead of taking it from `run`. The test passes a recording decode on a 64 × 48 corpus JPEG and expects scale 8 and 1, each with the logical CPU count; on a one-CPU machine the thread half cannot tell one thread from all.
