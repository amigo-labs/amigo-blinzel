---
status: todo
created: 2026-11-02
size: L
platforms: Windows, macOS
design: [blinzel.md §1, blinzel.md §11, blinzel.md §12 (M8)]
---

# Competitive benchmark

## Context

The product promise is to be measurably faster than other viewers at opening and flipping through images (`blinzel.md` §1), proven on the website with reproducible numbers. `crates/blinzel-race` measures every viewer the same way from the outside: it starts the viewer with a test image, watches what reaches the screen, sends arrow keys and times when the next image appears. Test images carry a colour signature so a coarse grid of screen samples tells which image is visible, wherever the window is and however it scales. The JSON report is what the website renders (`website.md`).

Windows uses the Desktop Duplication API (frame time `LastPresentTime`, QPC); macOS uses ScreenCaptureKit (display time, `mach_absolute_time`). The measurement flow is shared.

## Scope

### In scope

- `blinzel-race corpus <dir> [--count N]`: the signature test images.
- `blinzel-race list`: the detected viewers and their versions.
- `blinzel-race run <dir> [--reps N] [--cold] [--burst N] [--only a,b] [--viewer name=path]... [--json out.json]`: open warm, open cold, next image, key repeat; console output and JSON report.
- Viewer discovery, launch, focus, input and quit on Windows and macOS.

### Out of scope

- Linux — because there is no capture with present times implemented there; only `corpus` works.
- The thumbnail metric (time until a 1000-image grid is filled) — because it is not implemented yet (`blinzel.md` §11 lists it from M4).
- Batch tool comparisons (vipsthumbnail, XnConvert) — because they are not implemented (`blinzel.md` §7, M9).
- Resident-mode handoff — because the tool always starts a new process.
- Rendering the report — because the website spec covers it.

## Acceptance criteria

- [ ] AC-01 (event): When `corpus <dir>` runs, then it writes `count` (default 24) JPEGs `race-000.jpg`, `race-001.jpg`, … of 6000 × 4000 pixels at quality 92, image `i` around base colour `i mod 6` with photo-like texture and sensor noise, and keeps files that already exist.
- [ ] AC-02 (always): The six base colours (the coloured corners of the RGB cube) differ pairwise by more than twice the matching tolerance in at least one channel, and consecutive test images never share a base colour.
- [ ] AC-03 (event): When a set of screen samples is classified, then the result is the base colour matched by the most samples within the tolerance, if at least 12 samples match; a dark desktop or a small patch of a test image (a thumbnail) yields no signature.
- [ ] AC-04 (event): When the measurement waits for a signature, then it returns the presentation time of the first frame at or after the start time that shows it, or fails with "signature N not seen" after the timeout.
- [ ] AC-05 (always, CI: windows, macos): The images shown during key repeat are counted as changes of signature between consecutive frames that show one, ignoring frames without a signature; the time the last image appeared is that of the last change.
- [ ] AC-06 (always, CI: windows, macos): Reported medians are the middle value (the mean of the middle two for an even count) and p95 the nearest-rank 95th percentile, both over the finite samples only; without finite samples the statistic is `null`.
- [ ] AC-07 (event, manual: needs a real desktop with the viewers installed): When `run` measures a viewer, then it closes every instance, starts it once untimed with image 0, then for each of `--reps` (default 10) repetitions times a warm start until image 0 is on screen (timeout 20 s), waits 1.5 s, presses Right once and times until image 1 is on screen (timeout 10 s), waits 1.5 s and sends `--burst` (default 20, at most count − 2) Right presses at 30 Hz, then records how many images appeared and whether the last one is the expected one.
- [ ] AC-08 (option, manual: needs administrator rights / `sudo` on a real machine): Where `--cold` is set, after the warm repetitions the same number of starts are timed with the file cache emptied first (Windows: standby list, macOS: `sudo purge`); the first failing cold start ends the cold series with an error.
- [ ] AC-09 (state, manual: needs a real desktop): While the viewer's process does not own the foreground window, no key press is sent; the repetition records "viewer window did not get the focus" or "viewer lost the focus during the burst".
- [ ] AC-10 (error): If a repetition fails (viewer does not start, a signature is not seen in time, the burst ends on another image), then the error is recorded for that viewer and the run continues with the next repetition or viewer; a failed untimed first start skips that viewer.
- [ ] AC-11 (error): If a corpus file is missing, then `run` aborts before measuring with "… missing: run `blinzel-race corpus <dir>` first".
- [ ] AC-12 (error, manual: macOS system permissions): If the terminal lacks the Screen Recording or Accessibility permission, the tool runs as root, or `--cold` cannot get `sudo`, then `run` on macOS aborts before measuring with a hint where to allow it.
- [ ] AC-13 (event, CI: macos): When `list` runs, then it prints every detected viewer with its version: on Windows the known viewers at their default install locations (amigo-blinzel, IrfanView 64, ImageGlass, XnView MP, FastStone Image Viewer, JPEGView, qimgv) with the ProductVersion of their version resource, plus Windows Photos as "store app"; on macOS the app bundles of amigo-blinzel, Preview, XnView MP, qView, ApolloOne, Phoenix Slides and Pixea in `/Applications`, `~/Applications` or `/System/Applications` with their `CFBundleShortVersionString`; amigo-blinzel first.
- [ ] AC-14 (option): Where `--viewer name=path` is given (an executable on Windows, an app bundle on macOS), that viewer replaces a detected one of the same name or is added; where `--only a,b` is given, only viewers with these names (case-insensitive) are measured.
- [ ] AC-15 (option): Where `--json out.json` is given, the report holds `machine` (Windows: `cpu`, `logical_cpus`, `ram_gb`, `gpu`, `monitor`, `monitors`, `os_build`; macOS: `os` instead of `os_build`) and per viewer `name`, `version`, `open_warm`, `open_cold`, `next` (each `{median_ms, p95_ms, n, samples_ms}` or `null`), `burst` (`presses`, `shown`, `settle` with `settle` in ms after the last press) and `errors`.
- [ ] AC-16 (error): If `list` or `run` is called on Linux, or the command is unknown, then the tool prints the usage and exits with status 2.
- [ ] AC-17 (always, manual: a real measurement): Opening is timed from just before the viewer is launched until the presentation time of the first frame on any monitor that shows image 0's signature; next image from just before the key press until the first frame showing image 1.

## Technical notes

- Files: `crates/blinzel-race/src/main.rs` (commands), `corpus.rs`, `signature.rs` (`COLORS`, `TOLERANCE = 56`, `MIN_MATCHES = 12`, `classify`, `image`), `frames.rs` (`Frames::wait_for`, `since`, `clear`), `race.rs` (`measure`, `navigate`, `transitions`, `last_change`, `median_p95`, `to_json`), Windows `capture.rs` / `sys.rs` / `viewers.rs`, macOS `capture_macos.rs` / `sys_macos.rs` / `viewers_macos.rs`. `race.rs` and the platform modules compile only on Windows and macOS.
- Sample grid 40 × 24 over each monitor. Windows: one Desktop Duplication thread per output of every adapter; frames with `LastPresentTime == 0` (pointer only) are skipped; a monitor that fails (mode switch, secure desktop) stops reporting while the others go on. macOS: ScreenCaptureKit per display, scaled to 160 × 96 on the GPU.
- Launch: Windows starts the exe with the path as argument, Windows Photos via `ms-photos:viewer?fileName=…`; macOS `open -a <bundle> <file>`. Quit: `WM_CLOSE` / `NSRunningApplication.terminate`, after 3 s `TerminateProcess` / `SIGKILL`. Focus: Windows a synthetic Alt press plus `SetForegroundWindow`; macOS `open -a` again; foreground check via `GetForegroundWindow` / `CGWindowListCopyWindowInfo` (layer 0). Input: `SendInput` / `CGEvent` Right arrow. The mouse is parked in the middle of the primary monitor before each start.
- Windows cold start: `NtSetSystemInformation(SystemMemoryListInformation, MemoryPurgeStandbyList)` after enabling `SeProfileSingleProcessPrivilege`; fails with "run as administrator" without elevation.
- The Windows `blinzel.exe` carries a VERSIONINFO resource (`crates/blinzel-app/build.rs`) so its version is reported.
- Published measurement: `website/data/race.json` (2026-10-26, Ryzen 7 PRO 6850H laptop, amigo-blinzel 1.1.0 vs. Windows Photos, no `--cold`). `website/data/race-macos.json` does not exist yet.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Preview opened with a single file does not page through the folder | Next and key repeat fail with a recorded error; only the open time is reported |
| A slow viewer works through queued key presses after the burst | The tool waits up to 15 s for the expected last image, then 0.5 s more, before counting |
| A viewer drops presses during the burst | `settle` for that burst is `null` and "burst ended on another image (presses dropped)" is recorded |
| A fast viewer shows the last image before the burst's closing pause ends | `settle` is negative |
| `--burst 50` with the default 24 images | Capped at 22 presses |
| The viewer opens on another monitor | It is still seen: every monitor is captured |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `writes_signature_jpegs_and_keeps_existing_files` (`crates/blinzel-race/src/corpus.rs`, Linux CI) | built | |
| AC-02 | `colours_are_unambiguous` (`crates/blinzel-race/src/signature.rs`, Linux CI) | built | |
| AC-03 | `classifies_images_and_ignores_the_rest` (`crates/blinzel-race/src/signature.rs`, Linux CI) | built | |
| AC-04 | `waits_for_a_signature` (`crates/blinzel-race/src/frames.rs`, Linux CI) | built | |
| AC-05 | `counts_shown_images` (`crates/blinzel-race/src/race.rs`; Windows and macOS CI only) | built | |
| AC-06 | `median_and_p95` (`crates/blinzel-race/src/race.rs`; Windows and macOS CI only) | built | |
| AC-07 | manual | open | |
| AC-08 | manual | open | |
| AC-09 | manual | open | |
| AC-10 | — | open | |
| AC-11 | `check_names_the_first_missing_file` (`crates/blinzel-race/src/corpus.rs`, Linux CI); `run_without_a_corpus_says_how_to_write_one` (`crates/blinzel-race/tests/cli.rs`, Windows and macOS CI) | built | |
| AC-12 | manual | open | |
| AC-13 | macOS CI step `cargo run -q -p blinzel-race -- list` only checks the exit status, not the list | open | |
| AC-14 | — | open | |
| AC-15 | — (`to_json` untested; the website test uses a hand-written sample) | open | |
| AC-16 | `unknown_commands_print_the_usage_and_exit_2`, `list_and_run_print_the_usage_on_linux` (`crates/blinzel-race/tests/cli.rs`, Linux CI) | built | |
| AC-17 | manual | open | |

## Decisions

- 2026-11-02: Extracted from the code. `--burst N` exists (default 20) although the design spec does not list it; the first trial runs in the design spec used 6 presses.
- 2026-11-02: The code existed before its tests (the spec was extracted), so the new tests were green at once; each was proven by breaking the code on purpose (no skip of existing files, another quality, default count or file name, base colour, flat pixels, missing-file message, exit status, unknown command accepted) and seeing it go red.
- 2026-11-02: `run` checks the corpus with `corpus::check`, compiled on Windows, macOS and in tests only, so Linux tests it without the platform code; the end-to-end `run <empty dir>` test runs only where `run` exists (Windows and macOS CI), its first CI run is its proof.
