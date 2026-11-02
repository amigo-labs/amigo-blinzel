---
status: todo
created: 2026-11-02
size: M
platforms: all
design: [blinzel.md §5, blinzel.md §12, blinzel.md §13, blinzel.md §15]
---

# Startup: parallel cold start and its measurement

## Context

amigo-blinzel is meant to be measurably faster than any other viewer at opening an image (`blinzel.md` §1). The cold start therefore runs everything that can run in parallel: the single-instance handoff first, then the decode of the file, then the window and the GPU device, and on Windows the first image is drawn on the CPU and shown before the Direct3D device exists (`blinzel.md` §5 "Parallel cold start", "First image via GDI"). M1 sets the budget "t0 → present ≤ 50 ms warm, ≤ 120 ms cold; no white frame" (`blinzel.md` §12). This spec records what the code does today (reverse engineered) and how the startup is measured.

## Scope

### In scope

- The order of the cold start on Windows, Linux and macOS: handoff, first decode job, window, GPU device
- Windows: the cloaked window, the first image through the CPU preview renderer and GDI, the 400 ms fallback, showing without activation, lazy loading of `d3d11.dll`, COM/WIC, Direct2D and DirectWrite
- The measurement mode `--exit-after-first-frame [--json <out>]` and its report
- The app benchmark `blinzel.exe --bench <dir>` (Windows)
- The M1 startup budgets and how they are measured

### Out of scope

- Which instance a launch hands over to and what the running window does with the file — because that is browsing (`browsing.md`, "Open images in the same window") and resident mode (`resident-mode.md`)
- The resident `--resident` start, its warm-up and the wake from the tray — because they are resident mode (`resident-mode.md`)
- The colour target remembered from the last run and its check after the first frame — because that is colour management
- Window placement, the frameless chrome and the shells' event loops — because they are the platform shells
- Prefetching the neighbours and the folder listing's content — because they are browsing (`browsing.md`)
- Decoders and the embedded camera preview — because they are the decoders
- The installer, the updater's start-time rollback (`update::on_start`) — because they are installation and updates (`installation.md`, `updates.md`)

## Acceptance criteria

### Order of the cold start

- [ ] AC-01 (event): When a launch with a file finds a running viewer it may hand over to ("Open images in the same window", `browsing.md`), then it hands the file over and exits with code 0 before it reads the image, creates a window or starts the GPU device (Windows and Linux; macOS delivers files to the running app itself).
- [ ] AC-02 (always): The decode of the file given on the command line is queued before the window and the GPU device are created: in the startup report the mark `first_job_queued` comes before `window_created` and before `device_ready`.
- [ ] AC-03 (event): When the viewer is started without a file, then its window becomes visible at once, showing the hint "Drop an image here or press Ctrl+O".

### Windows: first image before the device

- [ ] AC-04 (event, CI: windows): When the first image is decoded before the Direct3D device is ready, then the window shows it drawn on the CPU (mark `first_gdi_frame`), becomes visible and takes the foreground with that image; once the device is ready, the Direct3D frame replaces it.
- [ ] AC-05 (always): The CPU-drawn first frame has the GPU frame's geometry: for all eight EXIF orientations its letterbox pixels equal the reference renderer's and the picture is within 26 dB PSNR of it; straight alpha over the background is within 2 levels per channel; a colour transform changes the image, not the letterbox.
- [ ] AC-06 (state, CI: windows, manual: visual): While no image has been drawn, the window is not visible: no empty or white frame and no open animation appears before the first image.
- [ ] AC-07 (error, CI: windows): If, with a file to show, no image is on screen 400 ms after the Direct3D device and swap chain are ready (a slow or undecodable file), then the window becomes visible anyway.
- [ ] AC-08 (always, CI: windows, manual: module-load trace): Until the first image is presented, the process has not loaded `d2d1.dll` or `dwrite.dll`; `d3d11.dll` is loaded at run time on the device thread, not by the loader before `main`; COM is initialized only on the thread of the first WIC decode, open dialog or shortcut.

### Measurement

- [ ] AC-09 (event): When the viewer is started with an image and `--exit-after-first-frame --json <out>`, then it shows the first frame, writes the startup report to `<out>` and exits with code 0.
- [ ] AC-10 (option): Where `--exit-after-first-frame` is given, the viewer starts its own instance even while another one runs (no handoff, and on Linux no single-instance socket); without `--json` the report goes to `startup.json` in the working directory.
- [ ] AC-11 (always): The startup report is one JSON object with `adapter` (the GPU's name), `warp` (Windows: true when the device runs on WARP; Linux: true for a CPU adapter such as lavapipe; macOS: always false) and `marks` (mark name → microseconds), and the marks include `first_present`; on Windows the marks count from the creation of the process, on Linux and macOS from the start of `main`.
- [ ] AC-12 (always, manual: measurement): On Windows 11 with a real GPU, the time from process creation to the first image on screen is at most 120 ms cold and at most 50 ms warm (M1), measured with `blinzel.exe <24 MP JPEG> --exit-after-first-frame --json start.json` (marks `first_gdi_frame` / `uncloak` for the first image, `first_present` for the Direct3D frame; median of alternating runs, `blinzel.md` §13). No CI step asserts a time.
- [ ] AC-13 (event, CI: windows): When run as `blinzel.exe --bench <dir> [--warp] [--json <out>]` on a folder with images, then it times, without a window, opening every image (read, probe, decode to a 2560 × 1440 screen, upload, render), browsing at 30 key presses per second with prefetching, listing a folder of 10 000 files and drawing 200 thumbnails from the cache, prints the JSON report (and writes it to `<out>`), and exits with code 0.
- [ ] AC-14 (error, CI: windows): If the `--bench` folder contains no images, then it prints `bench failed: no images in <dir>` and exits with code 1.

## Technical notes

- Windows (`crates/blinzel-app/src/run.rs`): `timing::init(process_age_us())`, settings, language, setup modes, OS check and `update::on_start` run before `instance::target` / `instance::hand_off` (`WM_COPYDATA` + `AllowSetForegroundWindow`, `SendMessageTimeoutW` 2 s). Then the loader (`blinzel_ui::loader::Loader`) is created and the first `Job` (stage `Screen`, `Full` in "100 %" mode, `with_preview: true`, priority 0) is queued (`first_job_queued`). The device thread `blinzel-gfx-init` runs `Gfx::create` (`gfx/device.rs`, `LoadLibraryW("d3d11.dll")`) and posts `WM_NULL` to wake the UI thread. The window is created with `WINDOW_EX_STYLE(0)` (a redirection surface is kept for the GDI frame), `DWMWA_TRANSITIONS_FORCEDISABLED` and `DWMWA_CLOAK`, shown with `SW_SHOWNA` while cloaked; the UI thread pumps messages until the device arrives.
- First image (`first_frame.rs`, `backend.rs` `WinServices::early_image`): the loader's CPU worker hands the first decoded priority-0 image over while no device is set; unless the mode is "fit window to image" it is rendered there with `blinzel_core::preview::render` for the known client size and posted as `WM_FIRST_FRAME`; the UI thread resizes the cloaked window to the image (`window::fit_window`, not in "100 %"), paints with `SetDIBitsToDevice`, uncloaks and calls `SetForegroundWindow`. Marks: `first_frame_posted`, `first_frame_rendered`, `first_gdi_frame`, `uncloak`. `App::render` marks `first_present` and, in measurement mode, writes the report and returns `Action::Quit`.
- 400 ms fallback: `window::start_uncloak_timer(hwnd, 400)` after `ShowWindow(SW_SHOW)`, handled as `TIMER_UNCLOAK` in `window.rs`.
- Lazy loading: shaders are compiled at build time with `fxc` (`crates/blinzel-app/build.rs`, `vs.cso`, `ps_image.cso`, `ps_overlay.cso`, embedded with `include_bytes!`; a Windows build without `fxc` fails). `d2d1.dll`/`dwrite.dll` via `LoadLibraryW` (`gfx/text.rs`), warmed by `text::prewarm` from `Host::first_frame_shown`. COM per thread on first WIC use (`wic.rs` `ensure_com`), in `dialogs.rs` and `setup/shortcut.rs`.
- Linux (`linux/shell.rs` `run`, `Shell::create`) and macOS (`macos/shell.rs` `run`, `create_shell`): `timing::init(0)`; the first job is queued before the event loop / `NSApplication` starts; the window is created invisible (Linux `with_visible(false)` when a file is given, macOS `setAlphaValue(0.0)`), the device (`Gpu::new`) is created synchronously on the UI thread after the window, and the window becomes visible with the first presented image (`Action::Uncloak`). Linux and macOS compile their shaders at run time (`create_shader_module` from WGSL, `newLibraryWithSource`). There is no early CPU image (`Services::wants_early_image` is `false`).
- The folder listing starts in `App::open`, i.e. once the `App` exists after the device (all platforms), on the thread `blinzel-list`.
- Startup marks (Windows): `main`, `instance_checked`, `first_job_queued`, `device_ready`, `window_created`, `first_frame_posted`, `first_frame_rendered`, `first_gdi_frame`, `uncloak`, `swapchain_ready`, `list_dir_us`, `first_read_us`, `first_decode_us`, `first_upload_us`, `first_<stage>_ready`, `first_present`.
- `--bench` (`bench.rs`) keys: `warp`, `device_ms`, `screen`, `open[]` (`read_ms`, `probe_ms`, `decode_ms`, `upload_ms`, `render_ms`, `total_ms` or `error`), `navigation` (`presses`, `prefetch_hits` = presses under 1 ms, `latency` stats), `list_10k`, `thumb_grid_200`; stats are `min_ms`, `median_ms`, `p99_ms`, `mean_ms`, `n` (`blinzel_core::timing::Stats`).
- CI (`.github/workflows/ci.yml`): Linux "Linux viewer smoke test" (Xvfb, `rgb420-64x48.avif`, `timeout 120`, `cat startup.json`); macOS "App bundle and smoke test (window, first frame)" (same image, `alarm 120`); Windows "Startup smoke test (window, first frame)" (`jpeg-24mp-420.jpg`, `continue-on-error: true`) and "App benchmark (offscreen, WARP)" (`--bench testdata/generated --warp`, exit code checked). None of them checks a time.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Windows: the file given on the command line does not exist or cannot be read | No job is queued and the file is not opened: the window shows the "open" hint at once, without an error message |
| Linux: the file does not exist, or the first image fails to decode | The window becomes visible with the error message as soon as it is drawn: the App uncloaks the window once no image is coming (nothing open, or the current image failed), on every platform |
| macOS: the first image fails to decode at launch | The same: the window is shown with the error message (the 400 ms timer exists only for waking a hidden resident window) |
| Linux, macOS: the first image is slow to decode | The window stays invisible until its first image is presented; there is no timer (only a missing or failed image would never arrive) |
| Windows: Direct3D device creation fails | Message box "Direct3D 11: …", exit code 1 |
| Linux: no display | `amigo-blinzel: no display (…)` on stderr, exit code 1 |
| `--exit-after-first-frame` with an undecodable file | No image is ever presented, so the process never exits (the Linux and macOS CI steps kill it after 120 s) |
| Windows: the window was maximized last time | It is maximized while still cloaked, so the first frame is laid out for its real size |
| Windows, mode "fit window to image" | The loader does not pre-render the first frame (the window size depends on the image); the UI thread renders it after resizing the window |
| A second launch while the first has not created its window yet | No window of the class exists yet, so the second launch starts its own instance (Windows) |
| The first image arrives after the device | No GDI frame; the Direct3D frame uncloaks the window |
| Windows machine without a GPU (CI runner) | The hardware driver type succeeds on the "Microsoft Basic Render Driver", so the report says `"warp": false`; only `adapter` reveals software rendering |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | | open | |
| AC-02 | | open | |
| AC-03 | `without_a_file_the_window_shows_the_open_hint_at_once` (`crates/blinzel-ui/src/app_startup_tests.rs`; every shell uncovers the window on `Action::Uncloak`) | built | |
| AC-04 | | open | |
| AC-05 | `tracks_the_reference_renderer`, `straight_alpha_blends_over_the_background`, `applies_the_colour_transform_to_the_image_only`, `fast_path_matches_the_general_path` (`crates/blinzel-core/src/preview.rs`; Linux, macOS, Windows) | built | |
| AC-06 | manual: visual check on Windows 11 pending (`blinzel.md` §13, the GDI → Direct3D transition) | open | |
| AC-07 | | open | |
| AC-08 | | open | |
| AC-09 | CI step "Linux viewer smoke test" (`.github/workflows/ci.yml`, Linux job: exit code within 120 s, `cat startup.json`); also "App bundle and smoke test (window, first frame)" (macOS job); the Windows "Startup smoke test" is `continue-on-error` | built | |
| AC-10 | | open | |
| AC-11 | `marks_and_json` (`crates/blinzel-core/src/timing.rs`) covers only the marks' JSON formatting; the report's keys and origins are untested | open | |
| AC-12 | manual: measurement | open | |
| AC-13 | CI step "App benchmark (offscreen, WARP)" (`.github/workflows/ci.yml`, Windows job: exit code 0; the report's content is not checked) | built | |
| AC-14 | | open | |

## Decisions

- **No image coming uncloaks the window in the App, not by a timer in each shell.** The defect "Linux and macOS never show the window for a missing or broken first file" is fixed in `App::render`: when the window is cloaked and not hidden (resident) and no image will come (nothing open, or the current image failed), the frame with the hint or the error asks for `Action::Uncloak`, so all three shells get it at once instead of after 400 ms. Tests: `a_missing_first_file_shows_the_window_with_its_error`, `a_broken_first_file_shows_the_window_with_its_error` (red before the fix), `the_window_stays_cloaked_while_the_first_image_decodes`, `a_hidden_resident_window_is_not_shown_by_a_frame` (`crates/blinzel-ui/src/app_startup_tests.rs`). The Windows 400 ms timer stays for slow files.
