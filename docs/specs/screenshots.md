---
status: todo
created: 2026-11-02
size: M
platforms: Windows, macOS
design: [blinzel.md §9, blinzel.md §12]
---

# Screenshots

## Context

M12 (`blinzel.md` §12): the resident instance takes screenshots in the style of Greenshot. A key or the tray / menu bar icon freezes the screen, an overlay picks a region, a window or a monitor, and the result always goes to the clipboard and then, as three switches say, into a file, the viewer and an external editor. Image files are never modified (AGENTS.md §9): every shot is a new file. This spec records what the code does today; it was extracted from the code (reverse engineering).

## Scope

### In scope

- Triggers: Print and Ctrl+Shift+Print (Windows), ⌘⇧1 (macOS), the tray / menu bar menu entries
- Capture: the GDI virtual desktop (Windows), ScreenCaptureKit per display (macOS)
- The overlay: window under the pointer, drag = region, click = window, Enter = monitor, Escape, arrow keys
- Clipboard, the three switches (save, open in the viewer, open in the editor), the screenshot folder, file names, the cache of unsaved shots
- The macOS Screen Recording permission flow
- `--capture-test` for CI

### Out of scope

- Linux — because no capture is implemented there yet (X11 `GetImage`, Wayland portal are open in `blinzel.md` §12); the menu does not offer screenshots
- HDR displays — because the capture is 8-bit sRGB only and HDR is listed as open in `blinzel.md` §12
- An editor of our own — because "Edit with…" hands the file to an external program by design
- Resident mode itself (tray, hiding, sign-in) — because it is its own feature (`resident-mode.md`)
- The time from the Print key to the overlay — because no measurement exists yet (open in `blinzel.md` §12)

## Acceptance criteria

- [ ] AC-01 (always): Screenshots are offered on Windows and macOS only: the screenshot settings appear in the Settings menu and the capture entries in the tray / menu bar menu there; on Linux neither appears and screenshot actions do nothing.
- [ ] AC-02 (option, CI: windows): Where resident mode and "Print key takes screenshots" are both on, Print and Ctrl+Shift+Print open the overlay, also while the window is hidden.
- [ ] AC-03 (error, CI: windows): If Windows keeps the Print key at start (Snipping Tool), Ctrl+Shift+Print still works, Print is retried every 5 s for one minute, and then the "Windows uses the Print key…" notice is shown once (as a balloon while hidden, in the window at the next wake); turned on from the menu while Print is taken, the notice shows at once for 9 s.
- [ ] AC-04 (option, CI: macos): Where resident mode and "⌘⇧1 takes screenshots" are both on, ⌘⇧1 opens the overlay; if another app holds ⌘⇧1, a notice says that screenshots still work from the menu bar icon.
- [ ] AC-05 (event): When the screenshot key setting is turned on while resident mode is off, resident mode is turned on too; the menu shows the key as checked only while both are on.
- [ ] AC-06 (event): When "Screenshot of the whole screen" is chosen, the monitor under the pointer is taken at once without an overlay; "Take screenshot…" opens the overlay.
- [ ] AC-07 (event, CI: windows): When a screenshot starts on Windows, the whole virtual desktop (all monitors, layered windows included) is frozen first, and the overlay shows that picture 1:1 over all monitors, topmost, without a taskbar button, invisible until its first frame is drawn.
- [ ] AC-08 (event, CI: macos): When a screenshot starts on macOS, every display is frozen and converted to sRGB, and each display gets its own borderless panel at screen-saver level (above the menu bar); a selection never spans two displays.
- [ ] AC-09 (always, CI: macos): Window frames and the pointer are mapped into each display's pixels, for Retina scale and for displays left of or above the main display; a cropped part keeps whole rows.
- [ ] AC-10 (event): When the user clicks in the overlay (press and release less than 4 px apart), the frontmost window under the pointer is taken, clipped to the picture; where there is no window, the monitor under the pointer.
- [ ] AC-11 (event): When the user drags at least 4 px, the rectangle between press and release is taken, in any direction, rounded to whole pixels and clipped to the picture.
- [ ] AC-12 (event): When Enter is pressed without a drag, the monitor under the pointer is taken; during a drag Enter finishes the drag. Escape during a drag drops the drag, otherwise it cancels the overlay.
- [ ] AC-13 (event): When an arrow key is pressed in the overlay, the pointer moves by 1 px (with Shift 10 px) without leaving the picture.
- [ ] AC-14 (event): When the overlay loses the foreground (Alt+Tab or another app) or gets a right click, it closes without taking a screenshot.
- [ ] AC-15 (always): Every taken screenshot is put on the clipboard as an image (Windows: DIBV5 and PNG, macOS: PNG); if that fails, the notice "Screenshot failed: clipboard" appears.
- [ ] AC-16 (option): Where "Save screenshots in the folder" is on, the PNG goes into the screenshot folder; where it is off but "open in the viewer" or "open in the editor" is on, it goes into the cache folder; where all three are off, no file is written.
- [ ] AC-17 (always): A screenshot file never replaces an existing one (`Shot.png`, then `Shot (2).png`, …) and never shows up half-written.
- [ ] AC-18 (always): The screenshot folder is the one chosen with "Screenshot folder…", else the system's (Windows: the Screenshots known folder, else Pictures\Screenshots; macOS: ~/Pictures/Screenshots); the file is named "Screenshot YYYY-MM-DD HHMMSS.png" in the UI language.
- [ ] AC-19 (event): When a shot is written to the cache folder, PNG files there at least 24 h old are deleted first; other files stay.
- [ ] AC-20 (option): Where "Open screenshots in the viewer" is on, the file opens in the viewer and the window comes up, also from hidden, and no "copied" notice appears; where it is off, the notice "Screenshot copied to the clipboard" appears. Where "Open screenshots in the editor" is on, the file then goes to the chosen editor, and without one the user is first asked to choose a program.
- [ ] AC-21 (error, manual: macOS system permission prompt): If the Screen Recording permission is missing, the first attempt in a process triggers the system's prompt and takes no shot; every later attempt shows an alert explaining where to allow it, whose "Open System Settings" opens Privacy & Security › Screen Recording.
- [ ] AC-22 (event, CI: windows): When `blinzel.exe --capture-test out.png` runs, it writes the whole virtual desktop as a PNG of at least 640 × 480, prints its size and exits with 0, without an overlay.
- [ ] AC-23 (event, CI: macos): When `blinzel --capture-test out.png` runs with the Screen Recording permission, it writes the main display as a PNG and exits with 0; without the permission it exits with 1.

## Technical notes

- Platform-independent: `crates/blinzel-ui/src/snip.rs` (`Snip`, `Outcome::{Unchanged, Redraw, Cancel, Done(Rect)}`, `DRAG_MIN = 4.0`, loupe 15 px × 8), `crates/blinzel-core/src/screenshot.rs` (`destination`, `cache_folder` = `<cache>/screenshots`, `free_name`, `write` through `*.blinzel-tmp`, `sweep`, `CACHE_MAX_AGE` = 24 h).
- Settings (`blinzel_core::settings::Settings`): `screenshot_hotkey` (default `true`), `screenshot_dir` (`None`), `screenshot_save` (`true`), `screenshot_open` (`true`), `screenshot_edit` (`false`), `editor` (`None`). The old key `screenshot_clipboard` is ignored.
- Commands: `Cmd::{Capture, CaptureScreen, ToggleScreenshotHotkey, ToggleScreenshotSave, ToggleScreenshotOpen, ToggleScreenshotEdit, ScreenshotFolder, OpenWith, EditWith, ChooseEditor}`; `blinzel_ui::menu::SCREENSHOTS`; `App::on_screenshot`, `edit_action`.
- Windows (`crates/blinzel-app/src/screenshot.rs`): `RegisterHotKey` with `VK_SNAPSHOT` (ids 1 and 2, `MOD_NOREPEAT`), GDI `BitBlt` with `SRCCOPY | CAPTUREBLT` of `SM_*VIRTUALSCREEN`, windows from `EnumWindows` (visible, not minimized, not cloaked, not `Progman`/`WorkerW`, ≥ 8 px, `DWMWA_EXTENDED_FRAME_BOUNDS`), overlay class `BlinzelSnip` (`WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP`, cloaked until the first present), result to the main window as `WM_SCREENSHOT`; clipboard `clipboard::put_shot` (10 tries, 20 ms apart).
- macOS (`macos/screenshot.rs`, `macos/hotkey.rs`): Carbon `RegisterEventHotKey` (kVK_ANSI_1, cmd + shift; no accessibility permission), `SCScreenshotManager` per display, `CGWindowListCopyWindowInfo` for windows, `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`, one `SnipWindow` panel per display at `NSScreenSaverWindowLevel`.
- CI: Windows job step "Screenshot capture test" checks the PNG signature and ≥ 640 × 480; the macOS job's step of the same name is `continue-on-error` (the runner may not grant Screen Recording).

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Dragging past the edge of the picture | The region stops at the picture's edge |
| A window partly outside the desktop | It is clipped to the picture when clicked |
| A second capture while the macOS overlay is open | Ignored |
| The tray menu is still fading out when a capture starts (Windows) | The capture waits for a DWM flush and 150 ms so the menu is not in the picture |
| The screenshot folder cannot be created or written | The shot stays on the clipboard; the notice "Screenshot failed: {reason}" appears |
| An editor that cannot be started | The notice "Cannot start the editor: {reason}" appears |
| `--capture-test` finds no monitor (Windows) | It prints `capture: no monitor` and exits with 1 |
| Screen Recording granted while the app runs (macOS) | macOS applies it only after the app restarts; the alert says so |

## Open questions

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `menu_settings_offers_each_platforms_entries` (`crates/blinzel-ui/src/app_settings_tests.rs`), `the_tray_menu_shows_opens_takes_screenshots_and_exits` (`crates/blinzel-ui/src/app_resident_tests.rs`), `screenshot_actions_do_nothing_on_linux` (`crates/blinzel-app/src/linux/shell.rs`) | built |        |
| AC-02 |      | open   |        |
| AC-03 |      | open   |        |
| AC-04 |      | open   |        |
| AC-05 | `turning_the_screenshot_key_on_turns_resident_mode_on_too` (`crates/blinzel-ui/src/app_screenshot_tests.rs`) | built |        |
| AC-06 | `the_tray_entries_take_the_screen_or_open_the_overlay` (`crates/blinzel-ui/src/app_screenshot_tests.rs`), `the_whole_screen_is_the_monitor_under_the_pointer` (`crates/blinzel-ui/src/snip.rs`) | built |        |
| AC-07 |      | open   |        |
| AC-08 |      | open   |        |
| AC-09 | `quartz_frames_count_down_from_the_main_display`, `windows_map_into_display_pixels`, `crops_rows` (`crates/blinzel-app/src/macos/screenshot.rs`), macOS CI job | built |        |
| AC-10 | `a_click_takes_the_frontmost_window_or_the_monitor` (`crates/blinzel-ui/src/snip.rs`), Linux CI job and all others | built |        |
| AC-11 | `drags_a_region_in_any_direction` (`crates/blinzel-ui/src/snip.rs`) | built |        |
| AC-12 | `enter_takes_the_monitor_escape_cancels`, `arrows_move_the_pointer_and_enter_finishes_a_drag` (`crates/blinzel-ui/src/snip.rs`) | built |        |
| AC-13 | `arrows_move_the_pointer_and_enter_finishes_a_drag` (`crates/blinzel-ui/src/snip.rs`) | built |        |
| AC-14 |      | open   |        |
| AC-15 | `a_shot_that_misses_the_clipboard_or_its_file_says_why` (`crates/blinzel-ui/src/app_screenshot_tests.rs`) covers the notice; the clipboard formats (DIBV5 and PNG, PNG) have no test yet | open |        |
| AC-16 | `destination_follows_the_settings` (`crates/blinzel-core/src/screenshot.rs`) | built |        |
| AC-17 | `writes_free_names_and_sweeps_old_shots` (`crates/blinzel-core/src/screenshot.rs`) | built |        |
| AC-18 |      | open   |        |
| AC-19 | `writes_free_names_and_sweeps_old_shots` covers the sweep by age and extension; that the shell sweeps before writing is untested | open |        |
| AC-20 | `a_shot_opens_in_the_viewer_or_says_it_was_copied_then_goes_to_the_editor` (`crates/blinzel-ui/src/app_screenshot_tests.rs`), `edit_asks_for_an_editor_first` (`crates/blinzel-ui/src/app.rs`) | built |        |
| AC-21 |      | open   |        |
| AC-22 | CI step "Screenshot capture test" (Windows job, `.github/workflows/ci.yml`) | built |        |
| AC-23 | CI step "Screenshot capture test" (macOS job) is `continue-on-error`, so it asserts nothing | open |        |

## Decisions

- AC-01: the menus' halves existed and were tested per platform (`menu::items_for`, `menu::tray_items_with`). That the Linux shell ignores the screenshot actions the App still gives (resident mode's toggle sends `SetScreenshotHotkey` on every platform) is the private `takes_screenshots`, checked before the shell's dispatch like `ends_process`; the App itself stays platform-independent and keeps answering the commands, since its tests run every platform's menu on Linux. The behavior existed; dropping an action from `takes_screenshots` turns the test red.
- AC-05, AC-06 (the App half): the behavior existed and the tests were green at once; skipping the `SetResident(true)` or swapping `whole_screen` turns them red. The monitor under the pointer was the same `find` in the overlay and both shells; it is now `snip::monitor_under` (new public API of `blinzel-ui`), tested on Linux. That the shells take it at once without the overlay is their early return in `start` (Windows) and `frozen` (macOS), without a test of its own.
- AC-15, AC-20: which notice a taken shot gets (clipboard failed, file failed, only copied, none while it opens in the viewer) was the same match in the Windows and macOS shells; `App::on_screenshot(copied, file)` now returns the actions and that text (changed public API of `blinzel-ui`), and each shell shows it through `App::tell`. The tests were written against the new signature first (they did not type-check). A hidden App reloads the settings when it opens a file, so the test answers the `Wake` with `App::wake` as the shell does before it changes more settings.
