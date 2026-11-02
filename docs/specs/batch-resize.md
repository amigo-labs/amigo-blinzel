---
status: todo
created: 2026-11-02
size: L
platforms: all (Windows, macOS, Linux)
design: [blinzel.md §2, blinzel.md §3, blinzel.md §7, blinzel.md §12]
---

# Batch resize

## Context

Users want to resize many images at once — for the web, for mail, for a fixed aspect ratio — without a second program. The design spec decides (`blinzel.md` §2): batch processing in the app only (no CLI); JPEG, PNG, WebP, AVIF or same as source; keep metadata, rename by pattern, crop to an aspect ratio, sharpen. Like every result, the copies are new files: source files are never changed and an existing name gets a number instead of being replaced (AGENTS.md §9). The resampler went through the benchmark gate against `fast_image_resize` (`blinzel.md` §3, row "Batch scaling (M9)").

This spec was reverse-engineered from the code on 2026-11-02: the engine `blinzel_core::batch` (feature `batch`), the dialog `blinzel-ui/src/batch_dialog.rs` with its file list `batch_files.rs`, the wiring in `App`, and the drop handlers and folder dialogs of the three shells. It records what the code does, not what the design spec says it should do.

Known gaps the code has today: file modification dates in names are local time only on Windows (UTC on Linux and macOS). (Until 2026-11-02 a drop onto the window on macOS always opened the first file, so the batch dialog's list could not be filled there; fixed, see Decisions.)

## Scope

### In scope

- The engine: size modes, "don't enlarge", fill-box crop, scaling, orientation, sharpening, output formats, metadata and colour profile, names from a pattern, target folder, planned names, writing without replacing, parallel run with memory budget, broken files, cancel
- The dialog: Ctrl+B and menu "Resize images…", its controls, the file list filled by drag and drop, progress, report and "Open folder"

### Out of scope

- A command line for batch work — because `blinzel.md` §2 keeps batch processing in the app (`blinzel-bench batch` is a developer benchmark, not a user feature)
- Marking thumbnails in the grid — because the grid and its marks belong to the thumbnails spec (`thumbnails.md` AC-19, AC-23); this spec only takes the marked files as the dialog's starting list
- Other batch operations (convert only, rotate, rename only) — because the design spec only plans resizing
- Comparison with `vipsthumbnail` and XnConvert in `blinzel-race` — because it is an open measurement item of `blinzel.md` §7, not app behavior

## Acceptance criteria

### Engine

- [ ] AC-01 (always): The target size refers to the image as shown (after its EXIF orientation): long edge, width, height, fit in box (aspect ratio kept) or percent; sizes round to whole pixels, at least 1. Every output is upright: its pixels are turned into display orientation and its EXIF orientation is 1.
- [ ] AC-02 (option): Where "don't enlarge" is set (the default), no output side exceeds the source's; without it, outputs scale up to the target.
- [ ] AC-03 (option): Where "fill box" is chosen, the output has the box's aspect ratio, cut evenly from the centre of the image as shown (a portrait-by-EXIF photo is cut along its displayed height); with "don't enlarge" and a box larger than the source, it is the largest centred region with that ratio at source resolution.
- [ ] AC-04 (always): Scaling is done in linear light with premultiplied alpha through a Lanczos3 filter: flat colours stay exactly flat, a fine black-and-white checkerboard averages to sRGB 188 (not 128), transparent pixels do not bleed colour into their neighbours, and every output value is within one level of a double-precision reference.
- [ ] AC-05 (event): When a source is a JPEG, it is decoded at the smallest DCT scale n/8 (n = 1…8) that still leaves at least twice the target size in each direction; other formats are decoded at full size.
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
- [ ] AC-23 (event): When images or folders are dropped onto the window (Windows, macOS, Linux) while the dialog is open, they are added to its list: a folder adds the images directly in it (not those of subfolders) in natural order; non-images and files already listed are left out; their headers are read in the background. Without an open dialog, a drop opens the first dropped file in the viewer.
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
- App wiring (`blinzel-ui/src/app.rs`): `Cmd::Batch` (Ctrl+B, also while the grid is shown), `files_dropped`, threads `blinzel-batch-inspect` and `blinzel-batch` (decoding through the platform's `Services`, one decode thread per file), `Wake::Batch` for progress, `last_dialog` keeps the settings until the app exits (not saved to disk). Drops: Windows `WM_DROPFILES` (`window.rs`), Linux `WindowEvent::DroppedFile` (`linux/shell.rs`); macOS `performDragOperation:` delivers every dropped path as `UserEvent::Dropped` (`macos/shell.rs`); all three hand them to `App::files_dropped`. Local time: `Services::utc_offset`, implemented only in `blinzel-app/src/backend.rs` (Windows).
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
| macOS: files dropped while the dialog is open | Added to the list, as on Windows and Linux (the first file opened in the viewer behind the dialog until 2026-11-02) |
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
| AC-05 | `jpeg_sources_decode_at_the_smallest_scale_that_keeps_twice_the_target` (`crates/blinzel-core/src/batch/run.rs`), Linux | built | |
| AC-06 | `flat_areas_are_unchanged_and_edges_gain_contrast`, `zero_amount_is_a_no_op` (`blinzel-core/src/batch/sharpen.rs`, Linux) | built | |
| AC-07 | `every_codec_round_trips_with_metadata`, `webp_lossless_is_exact` (`blinzel-core/src/batch/encode.rs`), `same_format_where_we_can_write_it` (`blinzel-core/src/batch/mod.rs`), Linux | built | |
| AC-08 | `jpeg_keeps_full_chroma_from_quality_90` (`blinzel-core/src/batch/encode.rs`, Linux) | built | |
| AC-09 | `rewrite_drops_what_must_go_and_keeps_the_rest`, `gps_stays_on_request`, `exif_is_found_in_every_container` (`blinzel-core/src/batch/meta.rs`), `every_codec_round_trips_with_metadata` (`encode.rs`), `jpeg_sources_are_decoded_at_a_reduced_scale_and_rotated` (`run.rs`, GPS removed), Linux | built | |
| AC-10 | `every_codec_round_trips_with_metadata` (`blinzel-core/src/batch/encode.rs`, Linux) | built | |
| AC-11 | `placeholders`, `date_formats` (`blinzel-core/src/batch/naming.rs`, Linux) | built | |
| AC-12 | `dates_are_the_capture_time_else_the_modification_time` (`blinzel-core/src/batch/plan.rs`), `planned_names_are_the_names_written` (`run.rs`, EXIF date), `date_formats` (`naming.rs`, empty date), Linux | built | |
| AC-13 | `file_dates_are_local_time_with_the_offset_of_their_moment` (`crates/blinzel-app/src/backend.rs`), CI: windows; `dates_are_the_capture_time_else_the_modification_time` (`blinzel-core/src/batch/plan.rs`, the default clock is UTC), `stamps` (`naming.rs`), Linux | open (CI: windows, first run pending) | |
| AC-14 | `names_in_order_with_numbers_for_collisions`, `relative_folders_follow_each_source_and_absolute_ones_are_kept` (`blinzel-core/src/batch/plan.rs`), `batch_writes_new_files_and_never_replaces` (`run.rs`, folder created), Linux | built | |
| AC-15 | `batch_writes_new_files_and_never_replaces` (`blinzel-core/src/batch/run.rs`), `claims_never_repeat_or_hit_existing_files` (`naming.rs`), Linux | built | |
| AC-16 | `planned_names_are_the_names_written` (`run.rs`), `names_in_order_with_numbers_for_collisions` (`plan.rs`), `keys_and_tab_order` (`blinzel-ui/src/batch_dialog.rs`), Linux | built | |
| AC-17 | `taken_or_mismatched_names_get_the_next_free_one` (`blinzel-core/src/batch/run.rs`, Linux) | built | |
| AC-18 | `unreadable_broken_and_panicking_files_fail_and_the_batch_goes_on`, `batch_writes_new_files_and_never_replaces` (`blinzel-core/src/batch/run.rs`, Linux) | built | |
| AC-19 | `files_run_in_parallel_within_the_memory_budget` (`blinzel-core/src/batch/run.rs`, Linux; the app runs with `Options::default()`) | built | |
| AC-20 | `cancelled_batches_skip_the_rest` (`blinzel-core/src/batch/run.rs`), `progress_and_summary` (`blinzel-ui/src/batch_dialog.rs`, Esc cancels), Linux | built | |
| AC-21 | manual: `blinzel-bench batch` (not run in CI) | open | |
| AC-22 | `resize_images_opens_a_modal_dialog_that_keeps_its_settings`, `resize_images_starts_with_the_marked_files_in_folder_order`, `resize_images_outside_the_grid_starts_empty` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`), `edits_make_the_job` (`blinzel-ui/src/batch_dialog.rs`), Linux | built | |
| AC-23 | `drops_fill_the_open_dialog_and_otherwise_open_the_first_file` (`crates/blinzel-ui/src/app_batch_tests.rs`), `dropped_files_and_folders_fill_the_list` (`blinzel-ui/src/batch_files.rs`), Linux; the macOS shell's forwarding builds only in the macOS CI job | built | |
| AC-24 | `dropped_files_and_folders_fill_the_list` (`blinzel-ui/src/batch_files.rs`), `new_names_follow_the_pattern` (`blinzel-ui/src/batch_dialog.rs`), Linux | built | |
| AC-25 | `start_needs_a_valid_size_pattern_folder_and_a_readable_file`, `dropped_files_wait_for_their_headers`, `edits_make_the_job`, `new_names_follow_the_pattern` (`blinzel-ui/src/batch_dialog.rs`), `bad_patterns_are_refused` (`blinzel-core/src/batch/naming.rs`), Linux | built | |
| AC-26 | `keys_and_tab_order`, `two_sizes_take_part_in_the_tab_order_and_enter_closes_a_finished_run`, `progress_and_summary` (`blinzel-ui/src/batch_dialog.rs`), `a_batch_reports_its_result_and_opens_the_output_folder` (`crates/blinzel-ui/src/app_batch_tests.rs`, Enter after a run), Linux | built | |
| AC-27 | `a_batch_reports_its_result_and_opens_the_output_folder`, `progress_counts_and_the_first_four_errors_are_shown` (`crates/blinzel-ui/src/app_batch_tests.rs`), `progress_and_summary` (`blinzel-ui/src/batch_dialog.rs`, drops ignored), Linux | built | |
| AC-28 | manual | open | |

## Decisions

- 2026-11-02: revised AC-05 to the smallest n/8 scale instead of only 1/2, 1/4 and 1/8. The extraction had over-specified it: `blinzel.md` §7 only asks for "at least twice the target size", and libjpeg-turbo's finer scales meet that with fewer pixels to resample. The red run against the old wording (3/8 decoded where the AC said 1/2) is what found it.
- 2026-11-02: revised AC-22: the dialog starts with the grid's marks when there are any (decision in `thumbnails.md`).
- 2026-11-02: `resize_images_opens_a_modal_dialog_that_keeps_its_settings` was green at once: Ctrl+B, the menu, the defaults, the modality and the kept settings already worked; only the start with the grid's marks was new (red first in `resize_images_starts_with_the_marked_files_in_folder_order`).
- 2026-11-02: the engine tests for AC-08, AC-12, AC-14, AC-17, AC-18 and AC-19 were green at once: the behavior existed and only the tests were missing. Each was run red against a mutation of the code instead (subsampling from quality 91, the modification time without the platform's offset, an absolute folder taken as relative, the planned name kept despite another extension, the decode without `catch_unwind`, the memory budget ignored), then the mutation was reverted.
- 2026-11-02: AC-13, Windows part: the body of `WinServices::utc_offset` moved into `utc_offset_in(tz, t)` in `blinzel-app/src/backend.rs`, so the test can hand it a fixed Central European zone (winter +1 h, summer +2 h, whatever zone the CI runner is in) and also compare the services with the system zone's current bias (`GetTimeZoneInformation`). The test runs only in the Windows CI job (`cargo test --workspace --release`) and could not be compiled here (no MSVC tools for the C dependencies' build scripts), so its first CI run is the proof. Linux and macOS: their `Services` keep the trait's default offset 0 (UTC), which `dates_are_the_capture_time_else_the_modification_time` checks through the batch `Source`'s default.
- 2026-11-02: fixed the defect that macOS drops opened the first file instead of filling the batch list (AC-23 revised from "on Windows or Linux" to all three platforms, at the user's request). `performDragOperation:` now hands every dropped path to the same `App::files_dropped` the Windows and Linux shells use (as `UserEvent::Dropped`) and, as before, brings the window to the front. The routing lives in `blinzel-ui`, where `drops_fill_the_open_dialog_and_otherwise_open_the_first_file` checks it; the macOS change itself is only built and linted by the macOS CI job.
- 2026-11-02: the App and dialog tests for AC-23, AC-25, AC-26 and AC-27 were green at once (the shared path already routed drops; the defect was the macOS shell's bypass). They were run red against mutations instead (drops never reaching the dialog, a size limit of 1000000, no check of the folder, Enter after a run starting nothing). The harness's window fits its small images, so the App tests widen it to 1280 × 900 before opening the dialog: in a narrow window the rows' names are elided.
- 2026-11-02: extracted from the code (reverse engineering); the design spec's ✅ markers for §7 "Batch processing" were not taken as evidence.
