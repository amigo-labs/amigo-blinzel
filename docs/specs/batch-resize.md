---
status: todo
created: 2026-11-02
size: L
platforms: all (Windows, macOS, Linux; dropping files onto the dialog works on Windows and Linux only)
design: [blinzel.md §2, blinzel.md §3, blinzel.md §7, blinzel.md §12]
---

# Batch resize

## Context

Users want to resize many images at once — for the web, for mail, for a fixed aspect ratio — without a second program. The design spec decides (`blinzel.md` §2): batch processing in the app only (no CLI); JPEG, PNG, WebP, AVIF or same as source; keep metadata, rename by pattern, crop to an aspect ratio, sharpen. Like every result, the copies are new files: source files are never changed and an existing name gets a number instead of being replaced (AGENTS.md §9). The resampler went through the benchmark gate against `fast_image_resize` (`blinzel.md` §3, row "Batch scaling (M9)").

This spec was reverse-engineered from the code on 2026-11-02: the engine `blinzel_core::batch` (feature `batch`), the dialog `blinzel-ui/src/batch_dialog.rs` with its file list `batch_files.rs`, the wiring in `App`, and the drop handlers and folder dialogs of the three shells. It records what the code does, not what the design spec says it should do.

Known gaps the code has today: on macOS a drop onto the window always opens the first file, so the batch dialog's list cannot be filled there; file modification dates in names are local time only on Windows (UTC on Linux and macOS).

## Scope

### In scope

- The engine: size modes, "don't enlarge", fill-box crop, scaling, orientation, sharpening, output formats, metadata and colour profile, names from a pattern, target folder, planned names, writing without replacing, parallel run with memory budget, broken files, cancel
- The dialog: Ctrl+B and menu "Resize images…", its controls, the file list filled by drag and drop, progress, report and "Open folder"

### Out of scope

- A command line for batch work — because `blinzel.md` §2 keeps batch processing in the app (`blinzel-bench batch` is a developer benchmark, not a user feature)
- Starting a batch from the grid's marked thumbnails — because the code does not connect the grid multi-selection to the dialog (the grid belongs to the thumbnails spec)
- Other batch operations (convert only, rotate, rename only) — because the design spec only plans resizing
- Comparison with `vipsthumbnail` and XnConvert in `blinzel-race` — because it is an open measurement item of `blinzel.md` §7, not app behavior

## Acceptance criteria

### Engine

- [ ] AC-01 (always): The target size refers to the image as shown (after its EXIF orientation): long edge, width, height, fit in box (aspect ratio kept) or percent; sizes round to whole pixels, at least 1. Every output is upright: its pixels are turned into display orientation and its EXIF orientation is 1.
- [ ] AC-02 (option): Where "don't enlarge" is set (the default), no output side exceeds the source's; without it, outputs scale up to the target.
- [ ] AC-03 (option): Where "fill box" is chosen, the output has the box's aspect ratio, cut evenly from the centre of the image as shown (a portrait-by-EXIF photo is cut along its displayed height); with "don't enlarge" and a box larger than the source, it is the largest centred region with that ratio at source resolution.
- [ ] AC-04 (always): Scaling is done in linear light with premultiplied alpha through a Lanczos3 filter: flat colours stay exactly flat, a fine black-and-white checkerboard averages to sRGB 188 (not 128), transparent pixels do not bleed colour into their neighbours, and every output value is within one level of a double-precision reference.
- [ ] AC-05 (event): When a source is a JPEG, it is decoded at the smallest DCT scale (1/2, 1/4 or 1/8) that still leaves at least twice the target size in each direction.
- [ ] AC-06 (option): Where "sharpen" is set, an unsharp mask (amount 0.5, Gaussian sigma 0.8 output pixels) is applied to the colour channels after scaling: flat areas stay unchanged, both sides of an edge gain contrast, alpha is untouched.
- [ ] AC-07 (option): Where the format is JPEG, PNG, WebP (lossy, or lossless) or AVIF, the output is a valid file of that format at the planned size; lossless WebP reproduces every visible pixel exactly, and JPEG puts transparent pixels onto white. Where the format is "as source" (the default), JPEG, PNG, WebP and AVIF sources keep their format, other sources become PNG if any pixel is transparent and JPEG otherwise.
- [ ] AC-08 (option): Where the format is JPEG and the quality is 90 or more, the output has no chroma subsampling (4:4:4); below 90 it is 4:2:0.
- [ ] AC-09 (option): Where "keep metadata" is set (the default), the source's EXIF from JPEG, PNG, WebP, AVIF/HEIF or JPEG XL is written anew into the output: orientation 1, pixel dimensions set to the output size, no IFD1 thumbnail, no maker note, no image-data tags of the source (strips, tiles, compression, sizes), every other entry of a standard TIFF type kept with its value. Where "remove location (GPS)" is also set, the GPS block is left out; otherwise it is kept.
- [ ] AC-10 (option): Where "keep colour profile" is set (the default), the source's ICC profile is embedded in every output format (AVIF: as a `prof` colour box next to the nclx description).
- [ ] AC-11 (always): The output name comes from the pattern: `{name}` and `{ext}` of the source, `{n}` or `{n:D}` (position in the list, 1-based, zero-padded to D ≤ 9 digits), `{w}` and `{h}` (output size as shown), `{date}` (`YYYY-MM-DD`) or `{date:FORMAT}` with `YYYY YY MM DD hh mm ss`; other text stays. Characters not allowed in file names become `_`, trailing dots and spaces are dropped, and an empty result becomes `image`.
- [ ] AC-12 (always): `{date}` is the EXIF capture time (DateTimeOriginal) when the source has one, else the file's modification time; a pattern with `{date}` on a file with neither leaves the placeholder empty.
- [ ] AC-13 (state, CI: windows): While running on Windows, a modification time in a name is shown in local time with the time zone's offset at that moment (summer or winter time); on Linux and macOS it is UTC.
- [ ] AC-14 (always): The outputs of a source go to the target folder taken relative to that source's folder (default `resized`); an absolute folder is used as is; the folder is created when missing.
- [ ] AC-15 (always): A batch never replaces or changes a file: a name that exists or that an earlier file of the batch got gets ` (2)`, ` (3)`, …; the sources are byte-identical afterwards, and no temporary file is left in the target folder.
- [ ] AC-16 (always): The names are planned before the run from each file's header and EXIF, in list order, with numbers for identical names; the run writes exactly the planned paths.
- [ ] AC-17 (error): If a planned name is taken by another file between planning and writing, or the output format turns out different from the planned extension, then the file gets the next free name instead of replacing anything.
- [ ] AC-18 (error): If a file cannot be read, probed or decoded (also when the decoder panics), then it is reported as failed with its reason and the batch goes on; the summary counts done, failed and skipped files.
- [ ] AC-19 (state): While a batch runs, as many files are processed at once as the machine has hardware threads, as long as their decoded pixels fit a memory budget of 2 GiB; a single image larger than the budget still runs, alone.
- [ ] AC-20 (event): When the batch is cancelled (Cancel button or Esc in the dialog), no further file starts, files in progress finish, and the files not started count as skipped.
- [ ] AC-21 (always, manual: measurement): Resizing 24 12-MP JPEGs to a long edge of 1920 px as JPEG is faster per second than ImageMagick (`convert -resize 1920x1920`, 4 processes) on the same machine (`blinzel-bench batch <dir> --format jpeg --edge 1920` against a timed ImageMagick run on the same files).

### Dialog

- [ ] AC-22 (event): When the user presses Ctrl+B or chooses "Resize images…", a modal panel opens over the viewer with the files marked in the grid (`thumbnails.md` AC-23), otherwise an empty file list, and the settings last used in this session (the first time: long edge 1920, "don't enlarge", format "as source", quality 85, keep metadata, location and colour profile, pattern `{name}`, folder `resized`); while it is open, every key and click goes to it.
- [ ] AC-23 (event): When images or folders are dropped onto the window on Windows or Linux while the dialog is open, they are added to its list: a folder adds the images directly in it (not those of subfolders) in natural order; non-images and files already listed are left out; their headers are read in the background. Without an open dialog, a drop opens the first dropped file in the viewer.
- [ ] AC-24 (state): While the list is shown, each row reads old name → new name, the new name being the planned path with the real output size and the file's real date; rows still being read show "reading…", unreadable files show "unreadable: {reason}" in red and are left out of the run; × removes a row and "Remove all" clears the list.
- [ ] AC-25 (error): If a header is still being read, no file is readable, the size is empty, zero or above 100000, the pattern is invalid (unclosed `{`, unknown placeholder, empty, or a `/` or `\`) or the folder is empty, then Start and Enter do nothing and the new names show `–`. Size fields accept only digits, at most six.
- [ ] AC-26 (event): When the user presses Enter the batch starts (or, after a run, the dialog closes); Esc closes the dialog, or cancels while a batch runs; Tab and Shift+Tab move between the text fields (the second size field only for "fit in box" and "fill box").
- [ ] AC-27 (state): While a batch runs, the dialog shows `{done} of {total}` and ignores dropped files; afterwards it shows `{done} saved, {failed} failed` or `Cancelled after {done} of {total}`, the first four errors as `{file}: {reason}`, and "Open folder" opens the folder of the first written copy in the system's file manager.
- [ ] AC-28 (state, manual: visual design): While the dialog is open, the viewer behind it is dimmed and the panel shows size mode, dimensions, "don't enlarge", format, quality slider, lossless (WebP), sharpen, metadata, location and colour profile switches, the name pattern with its placeholders, and the folder with a "…" button that opens the system's folder dialog.

## Technical notes

- Types (`blinzel_core::batch`): `Job { size: Size, enlarge, sharpen: Option<Sharpen>, output: Output, quality: Quality, keep_exif, keep_gps, keep_icc, pattern, out_dir }`; `Size::{LongEdge, Width, Height, Fit{w,h}, Fill{w,h}, Percent}`; `Output::{Same, Codec(Codec)}`; `Codec::{Jpeg, Png, Webp, Avif}`; `Quality { jpeg: 85, webp: 80, webp_lossless: false, avif: 70, avif_speed: 6 }` (the dialog's single slider sets jpeg, webp and avif alike). `plan::{inspect, targets, Item { source, target }}`, `run::{run, Source, Native, Options { threads, memory_budget }, Progress, Summary}`.
- Per file (`run::one`): read → probe → `geometry` (crop and size in stored orientation) → decode (JPEG `Request::Fit` at twice the target, else `Request::Full`) → resample rows read straight from the decoded planes (`rows.rs`, YCbCr in fixed point) in bands of 64 output rows → `orient_bgra` → sharpen → encode with rewritten EXIF and ICC → write.
- Encoders: JPEG through libjpeg-turbo (needs the `turbojpeg` feature), PNG through `png`, WebP through libwebp (`VP8X` with `ICCP`/`EXIF` chunks added by us), AVIF through `ravif`/rav1e (8-bit, one thread per file, `colr`/`prof` box spliced in, `iloc` offsets shifted).
- Writing (`run::write_new`): a temporary file `.{base}.{pid}-{n}.blinzel-tmp` in the target folder, then `hard_link` to the name (fails if the name exists → next number); file systems without hard links fall back to `rename` after checking the name is free.
- The memory budget counts `width × height × 4` bytes of the full stored size per file, also when the JPEG is decoded smaller.
- App wiring (`blinzel-ui/src/app.rs`): `Cmd::Batch` (Ctrl+B, also while the grid is shown), `files_dropped`, threads `blinzel-batch-inspect` and `blinzel-batch` (decoding through the platform's `Services`, one decode thread per file), `Wake::Batch` for progress, `last_dialog` keeps the settings until the app exits (not saved to disk). Drops: Windows `WM_DROPFILES` (`window.rs`), Linux `WindowEvent::DroppedFile` (`linux/shell.rs`); macOS `performDragOperation:` delivers `UserEvent::Open` (`macos/shell.rs`). Local time: `Services::utc_offset`, implemented only in `blinzel-app/src/backend.rs` (Windows).
- "Remove location (GPS)" can only be toggled while "keep metadata" is on.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Output wider or higher than 16383 px as WebP | That file fails with "WebP is limited to 16383×16383 pixels"; the rest goes on |
| EXIF block larger than one JPEG segment (64 KB) | The JPEG output is written without EXIF |
| Malformed EXIF block in the source | The output is written without EXIF |
| Animated GIF or WebP source | The first frame is resized |
| RAW, TIFF, HEIC or JPEG XL source with "as source" | JPEG (PNG only if pixels are transparent; RAW is never checked for transparency) |
| Target folder `.` and pattern `{name}`, same format as the source | The copy gets ` (2)`; the source stays as it is |
| The same file dropped twice, or a folder containing it | Listed once |
| Several files that make the same name | Numbered in list order, as shown in the list before the start |
| macOS: files dropped while the dialog is open | The first file opens in the viewer behind the dialog; the list stays empty (known gap; marking in the grid fills it instead) |
| File system without hard links (FAT, some shares) | Written by rename; a file appearing at that very moment could be replaced (race window) |
| Cancel while large files are in progress | Those files finish and are written; only files not started are skipped |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `sizes_in_display_orientation` (`blinzel-core/src/batch/mod.rs`), `jpeg_sources_are_decoded_at_a_reduced_scale_and_rotated` (`blinzel-core/src/batch/run.rs`), Linux | built | |
| AC-02 | `sizes_in_display_orientation` (`blinzel-core/src/batch/mod.rs`), `edits_make_the_job` (`blinzel-ui/src/batch_dialog.rs`, default), Linux | built | |
| AC-03 | `fill_crops_evenly` (`blinzel-core/src/batch/mod.rs`, Linux) | built | |
| AC-04 | `flat_colours_stay_flat`, `checkerboard_averages_in_linear_light`, `transparent_pixels_do_not_bleed`, `matches_a_double_precision_reference` (`blinzel-core/src/batch/resample.rs`, Linux) | built | |
| AC-05 | partial: `jpeg_sources_are_decoded_at_a_reduced_scale_and_rotated` checks the result, not the decode scale | open | |
| AC-06 | `flat_areas_are_unchanged_and_edges_gain_contrast`, `zero_amount_is_a_no_op` (`blinzel-core/src/batch/sharpen.rs`, Linux) | built | |
| AC-07 | `every_codec_round_trips_with_metadata`, `webp_lossless_is_exact` (`blinzel-core/src/batch/encode.rs`), `same_format_where_we_can_write_it` (`blinzel-core/src/batch/mod.rs`), Linux | built | |
| AC-08 | | open | |
| AC-09 | `rewrite_drops_what_must_go_and_keeps_the_rest`, `gps_stays_on_request`, `exif_is_found_in_every_container` (`blinzel-core/src/batch/meta.rs`), `every_codec_round_trips_with_metadata` (`encode.rs`), `jpeg_sources_are_decoded_at_a_reduced_scale_and_rotated` (`run.rs`, GPS removed), Linux | built | |
| AC-10 | `every_codec_round_trips_with_metadata` (`blinzel-core/src/batch/encode.rs`, Linux) | built | |
| AC-11 | `placeholders`, `date_formats` (`blinzel-core/src/batch/naming.rs`, Linux) | built | |
| AC-12 | partial: `planned_names_are_the_names_written` (`run.rs`, EXIF date), `date_formats` (empty date); the modification-time fallback is untested | open | |
| AC-13 | partial: `stamps` (`blinzel-core/src/batch/naming.rs`) covers the offset arithmetic, not the platforms | open | |
| AC-14 | `names_in_order_with_numbers_for_collisions` (`blinzel-core/src/batch/plan.rs`), `batch_writes_new_files_and_never_replaces` (`run.rs`, folder created), Linux; absolute folders untested | built | |
| AC-15 | `batch_writes_new_files_and_never_replaces` (`blinzel-core/src/batch/run.rs`), `claims_never_repeat_or_hit_existing_files` (`naming.rs`), Linux | built | |
| AC-16 | `planned_names_are_the_names_written` (`run.rs`), `names_in_order_with_numbers_for_collisions` (`plan.rs`), `keys_and_tab_order` (`blinzel-ui/src/batch_dialog.rs`), Linux | built | |
| AC-17 | | open | |
| AC-18 | `batch_writes_new_files_and_never_replaces` (`blinzel-core/src/batch/run.rs`, Linux); the panic path is untested | built | |
| AC-19 | partial: `batch_writes_new_files_and_never_replaces` runs with a 1-byte budget (images larger than the budget still finish); the concurrency limit is untested | open | |
| AC-20 | `cancelled_batches_skip_the_rest` (`blinzel-core/src/batch/run.rs`), `progress_and_summary` (`blinzel-ui/src/batch_dialog.rs`, Esc cancels), Linux | built | |
| AC-21 | manual: `blinzel-bench batch` (not run in CI) | open | |
| AC-22 | partial: `edits_make_the_job` (`blinzel-ui/src/batch_dialog.rs`) checks the defaults; Ctrl+B, the menu and the kept settings are untested | open | |
| AC-23 | partial: `dropped_files_and_folders_fill_the_list` (`blinzel-ui/src/batch_files.rs`) covers the list; the routing of drops in `App` is untested | open | |
| AC-24 | `dropped_files_and_folders_fill_the_list` (`blinzel-ui/src/batch_files.rs`), `new_names_follow_the_pattern` (`blinzel-ui/src/batch_dialog.rs`), Linux | built | |
| AC-25 | `dropped_files_wait_for_their_headers`, `edits_make_the_job`, `new_names_follow_the_pattern` (`blinzel-ui/src/batch_dialog.rs`), `bad_patterns_are_refused` (`blinzel-core/src/batch/naming.rs`), Linux; the empty folder and the 100000 limit are untested | built | |
| AC-26 | `keys_and_tab_order`, `progress_and_summary` (`blinzel-ui/src/batch_dialog.rs`, Linux); Enter after a run is untested | built | |
| AC-27 | partial: `progress_and_summary` (`blinzel-ui/src/batch_dialog.rs`) checks drops ignored and the output folder; texts and the error list are untested | open | |
| AC-28 | manual | open | |

## Decisions

- 2026-11-02: revised AC-22: the dialog starts with the grid's marks when there are any (decision in `thumbnails.md`).
- 2026-11-02: extracted from the code (reverse engineering); the design spec's ✅ markers for §7 "Batch processing" were not taken as evidence.
