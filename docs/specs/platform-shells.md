---
status: todo
created: 2026-11-02
size: L
platforms: Windows, macOS, Linux
design: [blinzel.md §2, blinzel.md §6, blinzel.md §9, blinzel.md §13]
---

# Platform shells

## Context

One interface (`blinzel-ui`) runs in three native shells (decision in `blinzel.md` §2): Win32 + Direct3D 11, AppKit + Metal, winit + wgpu. Each shell implements the traits in `blinzel-ui/src/backend.rs` and adds what only the platform can give: the window frame, menus, dialogs, trash, clipboard, folder watching, single instance and the GPU backend. The three GPU backends are line-by-line ports of each other and are checked against the CPU reference renderer. This spec records what each shell provides today beyond the shared UI; it was extracted from the code (reverse engineering).

## Scope

### In scope

- Windows: the version check, the application manifest, the frameless window with snap layouts, the native dark-mode menu, window placement
- Linux: the frameless winit window, the self-drawn menu, desktop file dialogs, trash, clipboard, folder watching, single instance via a Unix socket
- macOS: the window under a transparent title bar, menu bar and context menu, panels, Trash, clipboard, folder watching, Finder and drag & drop
- The golden render test per GPU backend and the startup smoke test per CI job

### Out of scope

- Resident mode, tray and menu bar icon — because they are their own feature (`resident-mode.md`)
- Screenshots — because they are their own feature (`screenshots.md`)
- Colour management, installation and updates, startup budgets, thumbnails, view modes and keys — because each has its own spec
- System decoders (WIC, ImageIO) — because they belong to the decoder gate (`blinzel.md` §3)
- Remembering the window position on Linux and macOS — because neither shell implements it; the window opens at 70 % of the screen, centred (macOS) or where the window manager puts it (Linux)

## Acceptance criteria

- [ ] AC-01 (error, CI: windows): If the real Windows build number (not the compatibility-shimmed one) is below 26100, the viewer shows the localized message "amigo-blinzel requires Windows 11 version 24H2 or newer." and exits with 1 before a window exists; the setup shows the same message (not with `--silent`) and exits with 2 without installing.
- [ ] AC-02 (always, CI: windows): The executable embeds an application manifest declaring the Windows 10/11 `supportedOS`, PerMonitorV2 DPI awareness, `longPathAware`, the UTF-8 active code page and `asInvoker`.
- [ ] AC-03 (state, CI: windows): While the window is neither maximized nor full screen, it has no caption but keeps the system's resize borders and shadow on the left, right and bottom; the top border resizes (the outer corners diagonally), and the free part of the caption strip moves the window, except while a selection drag runs.
- [ ] AC-04 (state, CI: windows): While the window is maximized, its content does not hang over the monitor edge and the top edge does not resize; while it is full screen, no part of it acts as a caption.
- [ ] AC-05 (event, manual: Windows 11 snap layouts flyout): When the pointer rests on the maximize button, Windows 11 shows its snap layouts.
- [ ] AC-06 (event, manual: native menu look on Windows 11): When ☰ is clicked, a native popup menu opens in dark mode with localized labels and shortcuts, check marks and disabled entries, mirrored for right-to-left languages.
- [ ] AC-07 (event, CI: windows): When the viewer starts with a saved window rectangle, the window is put there only if a monitor still contains it, and a saved maximized state is applied; without a rectangle, or with none on any monitor, the window stays at its default place.
- [ ] AC-08 (event, CI: windows): When a visible window closes or hides, its normal rectangle (the one before full screen) and its maximized state are saved for the next start.
- [ ] AC-09 (always, CI: windows): The Direct3D 11 shaders on WARP match the CPU reference renderer (PSNR ≥ 38 dB and mean difference ≤ 1 per channel) for every YCbCr subsampling across tile seams, all eight orientations minified, zoom, crop, gray and alpha images, heavy minification and the colour transform; overlay shapes and text produce the expected pixels.
- [ ] AC-10 (event, CI: windows): When `blinzel.exe <24 MP JPEG> --exit-after-first-frame --json <file>` runs on the Windows runner, it writes the startup JSON and exits with 0.
- [ ] AC-11 (state): While the Linux window is neither maximized nor full screen, within 6 logical px of an edge the pointer shows a resize cursor and a press resizes in that direction (diagonally in the corners); a press on the free caption strip moves the window, a double click there maximizes or restores it.
- [ ] AC-12 (always): On Linux the ☰ menu is drawn by the viewer: the keyboard skips separators and disabled entries, the mouse opens submenus and chooses entries, and the menu stays inside the window and is mirrored for right-to-left languages.
- [ ] AC-13 (event, manual: desktop file dialogs under GNOME and KDE): When the user opens a file, saves a cutout, picks a folder or chooses an editor on Linux, the desktop's file dialog (xdg portal) is shown; "Open" starts in the current image's folder and filters to image extensions.
- [ ] AC-14 (event): When the user deletes the current image on Linux, it is moved to the desktop's trash and leaves the file list; if the trash refuses, the file and the list stay unchanged. When an image or cutout is copied, it is put on the clipboard as an image (X11 or Wayland).
- [ ] AC-15 (event): When a file is added, removed or renamed in the shown folder on Linux or macOS, the folder listing is refreshed without a restart.
- [ ] AC-16 (option): Where "Open images in the same window" is on, a second Linux launch hands its absolute path (or, without a file, a show request) over the socket `$XDG_RUNTIME_DIR/amigo-blinzel.sock` and exits with 0; the running instance opens the file and activates its window. A socket left by a dead instance is replaced, and without `XDG_RUNTIME_DIR` every launch opens its own window.
- [ ] AC-17 (always): The wgpu shaders on lavapipe match the CPU reference renderer with the same cases and thresholds as AC-09.
- [ ] AC-18 (event): When the release build runs with a 64 × 48 AVIF and `--exit-after-first-frame --json startup.json` under Xvfb, it writes `startup.json` and exits with 0 within 120 s.
- [ ] AC-19 (always): Where the system draws the window buttons (macOS), the viewer's ☰ sits at the top right and the viewer neither draws nor hit-tests its own minimize, maximize and close buttons.
- [ ] AC-20 (state, manual: AppKit menus on a Mac): While the macOS app runs, the menu bar holds the application menu and "Image", both filled from the shared menu tree each time they open; a right click or Control-click on the image opens the same menu as a context menu.
- [ ] AC-21 (event, CI: macos): When the user deletes the current image on macOS, it goes to the Trash and leaves the file list; a copied image or cutout is put on the general pasteboard as PNG.
- [ ] AC-22 (event, manual: Finder and drag & drop on a Mac): When a file is opened from the Finder (also while the app runs) or dropped onto the window, the viewer opens it; of several Finder files, the first is opened.
- [ ] AC-23 (always, CI: macos): The Metal shaders match the CPU reference renderer with the same cases and thresholds as AC-09.
- [ ] AC-24 (event, CI: macos): When the app bundle is built and its signature verified, its program started with a 64 × 48 AVIF and `--exit-after-first-frame --json startup.json` writes `startup.json` and exits with 0 within 120 s.

## Technical notes

- Windows (`crates/blinzel-app/src/`): `platform/os.rs` (`MIN_BUILD = 26100`, `is_supported` via `RtlGetVersion`), checked in `run.rs` and `setup/mod.rs`; `res/blinzel.manifest`, embedded by `build.rs` only with MSVC's linker (`/MANIFEST:EMBED`); `window.rs` (`nc_calc_size`, `nc_hit_test`: `HTTOP*`, `HTCAPTION`, `HTMAXBUTTON`/`HTMINBUTTON`/`HTCLOSE` from `blinzel_ui::chrome::Chrome::hit`; `restore_placement`, `remember_placement`); `run.rs::prepare_window` (`DWMWA_TRANSITIONS_FORCEDISABLED`, `DWMWA_USE_IMMERSIVE_DARK_MODE`, 1 px `DwmExtendFrameIntoClientArea`, `DragAcceptFiles`); `menu.rs` (`TrackPopupMenuEx`, uxtheme ordinals 135 `SetPreferredAppMode`, 133 `AllowDarkModeForWindow`, 136 `FlushMenuThemes`); D3D11 feature level 11_1 or 11_0 (`gfx/device.rs`).
- Linux (`src/linux/`): `shell.rs` (winit `with_decorations(false)`, `drag_resize_window`, `drag_window`, `EDGE = 6.0`, `rfd::FileDialog`, `trash::delete`, `arboard` with `wayland-data-control`, `notify::recommended_watcher` = inotify, `WindowEvent::DroppedFile`), `instance.rs` (`hand_off`, `listen`), `gpu.rs` (wgpu with Vulkan and GL ES). The self-drawn menu is `blinzel_ui::popup`.
- macOS (`src/macos/shell.rs`): `NSWindow` with `FullSizeContentView`, `setTitlebarAppearsTransparent(true)`, hidden title, `CAMetalLayer` as the layer of a layer-hosting view; `build_menu_bar` + `menuNeedsUpdate:`; `NSOpenPanel` / `NSSavePanel`; `NSFileManager trashItemAtURL`; `NSPasteboardTypePNG`; `notify` (FSEvents); `application:openURLs:`; `registerForDraggedTypes` with file URLs, `performDragOperation:`. `Chrome { system_buttons: true }` moves ☰ to the right.
- Golden tests: `crates/blinzel-app/tests/warp_golden.rs` (`#![cfg(all(windows, feature = "turbojpeg"))]`), `wgpu_golden.rs` (`cfg(all(unix, not(target_os = "macos"), …))`), `metal_golden.rs` (`cfg(all(target_os = "macos", …))`); test fns `ycc_subsamplings_with_tile_seams`, `all_orientations_minified`, `zoom_crop_gray_and_alpha`, `heavy_minification_uses_box_filter`, `overlay_shapes_and_text_render`, `colour_transform_lut`. On mismatch both images go to `target/<backend>-golden/`. `BLINZEL_REQUIRE_GPU=1` (Linux and macOS jobs) turns a missing adapter into a failure; WARP always exists.
- Smoke tests in `.github/workflows/ci.yml`: Linux "Linux viewer smoke test" (Xvfb, `timeout 120`), macOS "App bundle and smoke test (window, first frame)" (`perl -e 'alarm 120'`, `codesign --verify`), Windows "Startup smoke test (window, first frame)".

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| The saved window rectangle lies on a monitor that was unplugged | The window opens at its default place (70 % of the work area, centred) |
| Windows: the setup exe is started silently on build 22631 (23H2) | No message, exit code 2, nothing installed |
| Linux: the window is maximized or full screen | No resize edges; the pointer near the edge acts on the image |
| Linux: no display (no X11 / Wayland) | `amigo-blinzel: no display (…)` on stderr, exit code 1 |
| Linux: "Open images in the same window" is off | No socket is created; every launch opens a window |
| No GPU adapter in a golden test without `BLINZEL_REQUIRE_GPU` | The test passes with a "skipped" note |

## Open questions

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `windows_11_24h2_and_newer_are_supported`, `the_build_number_is_the_real_one` (`crates/blinzel-app/src/platform/os.rs`), `an_unsupported_windows_refuses_the_install_with_2` (`crates/blinzel-app/src/setup/mod.rs`), CI: Windows — written, not run yet | open |        |
| AC-02 | `the_executable_embeds_its_application_manifest` (`crates/blinzel-app/tests/manifest.rs`), CI: Windows — written, not run yet | open |        |
| AC-03 |      | open   |        |
| AC-04 |      | open   |        |
| AC-05 |      | open   |        |
| AC-06 |      | open   |        |
| AC-07 | `restores_on_an_existing_monitor_only` (`crates/blinzel-app/tests/placement.rs`), Windows CI job | built |        |
| AC-08 |      | open   |        |
| AC-09 | `warp_golden.rs`: `ycc_subsamplings_with_tile_seams`, `all_orientations_minified`, `zoom_crop_gray_and_alpha`, `heavy_minification_uses_box_filter`, `overlay_shapes_and_text_render`, `colour_transform_lut`, Windows CI job | built |        |
| AC-10 | CI step "Startup smoke test (window, first frame)" of the Windows job, no longer `continue-on-error` — not run yet | open |        |
| AC-11 | `the_edges_resize_the_plain_window_only`, `a_press_resizes_moves_or_goes_to_the_app` (`crates/blinzel-app/src/linux/shell.rs`); `a_double_click_below_the_caption_strip_toggles_full_screen` (`crates/blinzel-ui/src/app_viewing_tests.rs`, the App answers none on the strip) | built |        |
| AC-12 | `keyboard_skips_separators_and_disabled_items`, `mouse_opens_submenus_and_chooses`, `stays_inside_and_mirrors_for_rtl` (`crates/blinzel-ui/src/popup.rs`), Linux CI job | built |        |
| AC-13 |      | open   |        |
| AC-14 | `a_deleted_image_goes_to_the_trash_and_a_refused_one_stays_listed` (`crates/blinzel-app/src/linux/shell.rs`); `delete_moves_the_file_away_and_shows_the_one_that_took_its_place` (`crates/blinzel-ui/src/app_viewing_tests.rs`); `a_copy_is_offered_to_other_programs_as_png` (`crates/blinzel-app/src/linux/clipboard.rs`, the Xvfb step of the Linux CI job) | built |        |
| AC-15 | `adding_renaming_and_removing_a_file_are_reported` (`crates/blinzel-app/src/watch.rs`, Linux and macOS jobs); `a_changed_folder_is_listed_again_and_the_current_image_stays` (`crates/blinzel-ui/src/app_browsing_tests.rs`) | built |        |
| AC-16 | `a_later_launch_hands_its_file_to_the_running_window`, `the_socket_of_a_crashed_instance_is_replaced`, `without_a_runtime_folder_there_is_no_socket` (`crates/blinzel-app/src/linux/instance.rs`); `same_window_follows_the_setting`, `a_handed_over_file_opens_and_the_window_comes_to_the_front` (`crates/blinzel-app/src/linux/shell.rs`); `a_launch_hands_its_file_over_and_exits_before_the_window` (`crates/blinzel-app/tests/startup.rs`, exit code 0) | built |        |
| AC-17 | `wgpu_golden.rs`: the same six tests on lavapipe, Linux CI job (`BLINZEL_REQUIRE_GPU=1`) | built |        |
| AC-18 | CI step "Linux viewer smoke test" (Linux job) | built |        |
| AC-19 | `system_buttons_leave_the_menu_alone` (`crates/blinzel-ui/src/chrome.rs`), Linux CI job and all others | built |        |
| AC-20 |      | open   |        |
| AC-21 |      | open   |        |
| AC-22 |      | open   |        |
| AC-23 | `metal_golden.rs`: the same six tests, macOS CI job (`BLINZEL_REQUIRE_GPU=1`) | built |        |
| AC-24 | CI step "App bundle and smoke test (window, first frame)" (macOS job) | built |        |

## Decisions

- AC-11's decisions moved out of the Linux shell's event handlers into functions without a window: `resize_edge` (the 6 logical px edges, none while maximized or full screen), `resize_cursor`, `press` (edge, caption button, double click, free caption strip, the App) and `double_clicked` (the App's answer, else maximize or restore). Each test went red when the code under it was broken on purpose (the maximized check dropped, `<=` at the edge, the double click checked after the strip, `ToggleFullscreen` for `ToggleMaximize`).
- AC-14's trash call moved into `recycle`; its test trashes a temporary file through the real freedesktop trash, finds it in `trash::os_limited::list` and purges it, so no test file stays in the developer's trash. A refusal is a file that does not exist. The test went red with `std::fs::remove_file` instead of the trash and with the list told on a refusal. Wayland's clipboard has no test (no compositor in CI); X11's is the Xvfb test.
- AC-15: both shells' notify watchers were the same code; they are now one function, `watch::folder` (`crates/blinzel-app/src/watch.rs`), so one test covers inotify (Linux) and FSEvents (macOS job). It went red with the callback dropped and with the access filter removed.
- AC-16 held before this spec was built: the tests of `docs/specs/browsing.md` AC-15 check exactly its clauses. The exit code 0 of the second launch went red in `tests/startup.rs` when `run` returned 3 instead.
- AC-01's threshold moved into `os::supported` and the setup's refusal into `setup::refuse_install` (message unless silent, exit code 2), so both are tested without an old Windows. `the_build_number_is_the_real_one` reads the runner's build through `RtlGetVersion`: the unit-test executable has no manifest, so a shimmed API would say 9200. The viewer's exit code 1 is `run::fatal`'s, which every fatal start error shares; it has no test of its own (a message box would wait for a click). These tests only compile on Windows; their first Windows CI run is their proof.
- AC-02 is checked on the built `blinzel.exe`: link.exe stores the manifest as an `RT_MANIFEST` resource in plain UTF-8, so the test looks for the five declarations in the executable's bytes (values, not formatting, since the linker may rewrite the XML). Its first Windows CI run is its proof.
- AC-10: the Windows startup smoke step was `continue-on-error`, so it asserted nothing; it now fails the job. Its last informative run (CI run 41491320340, 2026-11-02) wrote the startup JSON and exited with 0, so the step is expected to stay green. Its first CI run without `continue-on-error` is the proof.
