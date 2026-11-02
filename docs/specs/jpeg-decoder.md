---
status: todo
created: 2026-11-02
size: M
platforms: all
design: [blinzel.md §3, blinzel.md §4, blinzel.md §12 (M3)]
---

# Own parallel JPEG decoder

## Context

Opening a large JPEG is the most common thing the viewer does, and Huffman decoding dominates its cost (`blinzel.md` §3, "Insight from the measurements"). libjpeg-turbo decodes on one core. `blinzel-jpeg` is our own decoder: it decodes one image on several cores and writes YCbCr planes straight into buffers the caller provides. Its output is byte-identical with libjpeg-turbo, so libjpeg-turbo stays the yardstick and the pixel reference (`blinzel.md` §3, benchmark gate; AGENTS.md §4, §5.2).

`blinzel-core` uses it only where the benchmark gate says it wins: for the image the user waits for, decoded on several threads. Single-threaded decodes (prefetch, thumbnails) and region decodes stay with libjpeg-turbo, which also takes every file `blinzel-jpeg` declines.

## Scope

### In scope

- The marker parser (`blinzel_jpeg::parse`): header fields the viewer needs before decoding.
- Decoding 8-bit Huffman JPEGs (baseline, extended sequential, progressive) into YCbCr or gray planes at 1/8, 1/4, 1/2 and full size, on one or several threads.
- How `blinzel_core::decode::decode_with` chooses between `blinzel-jpeg` and libjpeg-turbo.

### Out of scope

- libjpeg-turbo's own behavior (scaling, regions, CMYK, truncated files) — because it is part of `image-formats.md`.
- The benchmark tooling and its CI gate (`blinzel-bench run --check`, `jpeg-ab`) — because `benchmark-gate.md` covers it; here it is only evidence.
- Colour conversion and chroma upsampling — because the GPU and the CPU reference renderer do them from the planes.
- EXIF, MPF and ICC interpretation — because `image-formats.md` and `color-management.md` cover what the probe does with the segment ranges the parser finds.
- Arithmetic coding, 12-bit, lossless and CMYK JPEGs — because they are rare; the decoder declines them and libjpeg-turbo or the system decoder handles them.

## Acceptance criteria

- [ ] AC-01 (always): For every 8-bit baseline JPEG, gray or YCbCr with 4:4:4, 4:2:2, 4:2:0 or 4:4:0 subsampling, the decoder's planes at 1/8, 1/4, 1/2 and full size are byte-identical with libjpeg-turbo's YCbCr planes at the same scale, over the area both cover (plane sizes may differ by one sample).
- [ ] AC-02 (always): For every 8-bit progressive JPEG, gray or YCbCr with 4:4:4, 4:2:2 or 4:2:0 subsampling, the planes at every scale are byte-identical with libjpeg-turbo's.
- [ ] AC-03 (always): JPEGs with restart markers, sequential or progressive, decode byte-identically with libjpeg-turbo at every scale.
- [ ] AC-04 (event): When the decoder gets several threads and the scan has at least 64 KiB of entropy-coded data per thread, then it decodes the image on more than one thread — sequential files with or without restart markers, gray files and progressive files — and the planes stay byte-identical with libjpeg-turbo's at every scale.
- [ ] AC-05 (always): The parser reads, from everything before the first scan, the frame size, precision, coding process, components and their sampling factors, the subsampling class (gray, 4:4:4, 4:2:2, 4:2:0, 4:4:0, 4:1:1, other), the MCU size, the restart interval, the JFIF marker, the Adobe colour transform, the byte ranges of the EXIF and MPF payloads, and the ICC profile chunks ordered by their sequence number.
- [ ] AC-06 (state): While the given bytes end anywhere after the first scan's header, the parser returns the same header as for the whole file; bytes that end before it are an error ("truncated").
- [ ] AC-07 (always): For a requested minimum size the decoder picks the smallest DCT scale n/8 whose scaled image (rounded up like libjpeg) still covers it, and decodes at the smallest of 1/8, 1/4, 1/2 and 8/8 that is at least n/8.
- [ ] AC-08 (error): If the data has no SOI marker, a header ends before the first scan, the frame has zero width or height, a component's sampling factor is outside 1–4, or a Huffman table declares more codes than its code lengths allow, then parsing or decoding fails with an error.
- [ ] AC-09 (error): If a JPEG is arithmetic-coded, lossless or not 8-bit, or the requested scale is not 1, 2, 4 or 8 eighths, then the decoder refuses it as unsupported without writing any output.
- [ ] AC-10 (error): If the caller passes fewer output buffers than the frame has components, or a buffer smaller than that component's plane layout, then decoding fails as unsupported without writing any output.
- [ ] AC-11 (error): If the entropy-coded data is truncated, then decoding on several threads returns a result or an error without panicking.
- [ ] AC-12 (event): When the viewer decodes a JPEG with several threads at full size or for a minimum size, then the image has at least the size libjpeg-turbo's single-threaded decode yields for the same request, it is YCbCr planes for colour files, and where both have the same size their pixels are identical.
- [ ] AC-13 (state): While a JPEG is decoded on one thread, or a full-resolution region is requested, the viewer decodes it with libjpeg-turbo (DCT scales 1/8 to 8/8 in eighths; regions cropped by libjpeg-turbo).
- [ ] AC-14 (error): If the decoder declines a JPEG that the viewer decodes with several threads (four components, three components that are not YCbCr, 4:1:1 or other unusual subsampling, a size over the pixel limit, corrupt entropy data), then the viewer hands it to libjpeg-turbo, which decodes it or reports its own error.
- [ ] AC-15 (option): Where the build has no `turbojpeg` feature, the viewer decodes every supported JPEG with this decoder on any number of threads, decodes a region request at full size and crops it, and reports every JPEG the decoder declines as not decoded natively.
- [ ] AC-16 (event): When a thread-init function has been registered, then every helper thread the decoder starts (scan threads, synchronization, bands, IDCT) runs it before decoding; only the first registration counts.
- [ ] AC-17 (always): Decoding a 24 MP 4:2:0 baseline JPEG at full size on all cores of an 8-core machine, and at 1/8, is faster than libjpeg-turbo on one core by at least the factors of the M3 target (`blinzel.md` §12), measured alternately with `blinzel-bench jpeg-ab` on the corpus file `jpeg-24mp-420.jpg`.

## Technical notes

- **Public API** (`crates/blinzel-jpeg/src/lib.rs`, `decoder.rs`): `parse(&[u8]) -> Result<JpegInfo, Error>`, `is_supported(&JpegInfo) -> bool` (precision 8, Huffman, baseline/extended/progressive), `plane_layouts(&JpegInfo, scale) -> Vec<PlaneLayout>`, `decode_planes(data, scale, planes: &mut [&mut [u8]], threads) -> Result<usize, Error>` (returns the most threads a scan used), `SCALES = [1, 2, 4, 8]`, `supported_scale`, `scaled`, `JpegInfo::dct_scale_for`, `set_thread_init(fn())`. `#![forbid(unsafe_code)]`.
- **Plane layout:** one plane per frame component, not upsampled; `stride = mcus_x × h × scale` bytes, `rows = mcus_y × v × scale`, i.e. padded to whole MCUs so blocks are written without edge cases; `width`/`height` are the samples that belong to the image. `blinzel-core` allocates the buffers from the buffer pool (`PoolBuf`).
- **Entropy data** (`bits.rs`): one pre-pass per scan removes byte stuffing and fill bytes and records restart-marker positions; the bit reader then works on plain bit positions and never checks for markers.
- **Huffman** (`huffman.rs`): 10-bit lookup; AC tables have a second lookup that decodes run, value and extra bits at once.
- **Sequential parallel** (`parallel.rs`): threads = min(requested, entropy bytes / 64 KiB, MCU rows). With dense restart markers (interval × 8 ≤ MCUs per band) each band starts at the last marker before it. Otherwise pass A decodes each byte chunk from a guessed state (Weißenberger & Schmidt), retrying one bit later on an invalid code; neighbours synchronize where the decoder reaches a recorded MCU start of the successor; prefix sums give absolute MCU numbers and DC offsets; pass C decodes 4 bands per thread into disjoint rows (`split_at_mut`). Any inconsistency in pass A or the synchronization falls back to the sequential decoder.
- **Progressive** (`progressive.rs`, port of `jdphuff.c`): all scans are read first; with several threads and 2–32 scans each scan runs in its own thread, row by row behind the scans it depends on (same component, overlapping spectral band); coefficients live in per-block-row mutexes. If the pipeline fails, the scans are redone in order and decoding stops at the first broken scan, as libjpeg does. The IDCT runs last in parallel bands.
- **IDCT** (`idct.rs`): ports of `jpeg_idct_islow` and the reduced `jpeg_idct_4x4/2x2/1x1` from libjpeg-turbo, written on fixed-width lane arrays the compiler vectorizes; at 1/8 only DC coefficients are kept (AC codes are decoded but not stored).
- **Choice in `blinzel-core`** (`decode/mod.rs`, `decode/own_jpeg.rs`): `own = !cfg!(feature = "turbojpeg") || (threads > 1 && !matches!(req, Request::Region(_)))`; `own_jpeg::decode` returns `None` to fall back. The loader passes all decode threads (P-cores on Windows) only for the job with priority 0 — the image the user waits for — and 1 for prefetch and thumbnails.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Image too small for several threads (under 64 KiB of entropy data per extra thread) | Decodes sequentially; `decode_planes` returns 1 |
| More than 32 progressive scans | Scans decode one after another; the IDCT still runs in parallel |
| Restart marker missing where the interval says one should be | The decoder resynchronizes at the next byte boundary and resets the DC predictors |
| Extended sequential (SOF1) 8-bit Huffman file | Accepted like baseline (no test with such a file) |
| Adobe APP14 with transform "YCbCr" and no JFIF | Treated as YCbCr; transform 0 with three components means RGB and is declined |
| File with a header but no scan | `Error::NoFrame` |
| A progressive scan that is broken half-way | The image keeps the coefficients of the scans before it |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `matches_turbojpeg_for_all_subsamplings_and_scales` (`crates/blinzel-core/tests/own_jpeg.rs`), CI: Linux | built | |
| AC-02 | `matches_turbojpeg_for_progressive` (`crates/blinzel-core/tests/own_jpeg.rs`), CI: Linux | built | |
| AC-03 | `matches_turbojpeg_with_restart_markers`, `matches_turbojpeg_for_progressive` ("progressive restart") (`crates/blinzel-core/tests/own_jpeg.rs`), CI: Linux | built | |
| AC-04 | `parallel_decoding_matches_turbojpeg`, `matches_turbojpeg_for_progressive` (threads 4) (`crates/blinzel-core/tests/own_jpeg.rs`), CI: Linux; additionally the Windows CI step "Library vs. own code benchmarks" (`blinzel-bench run --check`: 12–45 MP corpus at 8/8 and 1/8 on all logical CPUs) | built | |
| AC-05 | `parses_baseline_420`, `classifies_subsampling`, `finds_app_segments` (`crates/blinzel-jpeg/src/lib.rs`), CI: Linux | built | |
| AC-06 | `header_prefix_is_enough` (`crates/blinzel-jpeg/src/lib.rs`), CI: Linux | built | |
| AC-07 | `dct_scale_choice` (`crates/blinzel-jpeg/src/lib.rs`), `scales_round_up` (`crates/blinzel-jpeg/src/decoder.rs`), CI: Linux | built | |
| AC-08 | `rejects_garbage` (`crates/blinzel-jpeg/src/lib.rs`), `rejects_garbage` (`crates/blinzel-jpeg/src/decoder.rs`), `rejects_overfull_tables` (`crates/blinzel-jpeg/src/huffman.rs`), CI: Linux | built | |
| AC-09 | `refuses_unsupported_files_and_scales_without_writing` (`crates/blinzel-core/tests/own_jpeg.rs`), CI: Linux | built | |
| AC-10 | `refuses_missing_or_short_buffers_without_writing` (`crates/blinzel-core/tests/own_jpeg.rs`), CI: Linux | built | |
| AC-11 | `truncated_data_decodes_partially_without_panicking` (`crates/blinzel-core/tests/own_jpeg.rs`), CI: Linux | built | |
| AC-12 | `decode_with_threads_matches_the_turbojpeg_path` (`crates/blinzel-core/tests/own_jpeg.rs`), CI: Linux | built | |
| AC-13 | — (no test tells which decoder ran) | open | |
| AC-14 | — | open | |
| AC-15 | — (no CI job builds without `turbojpeg`) | open | |
| AC-16 | — | open | |
| AC-17 | — (`blinzel-bench run` and `jpeg-ab` measure but assert no speed ratio) | open | |

## Decisions

- 2026-11-02: Extracted from the code (`/spec-extract`). The design spec's "pipeline of entropy decode and IDCT" exists only for progressive files (scans pipelined across threads); sequential files decode and transform each block in one pass.
- 2026-11-02: AC-09 and AC-10 already held in `decode_planes`; their tests were green at once. They went red with the `is_supported`/scale check and the buffer check disabled, so they guard the behavior. The unsupported files are a real baseline file with only the frame marker and precision byte patched, so without the check the decoder would write output.
