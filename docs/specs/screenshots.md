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
- CI: the Windows and macOS jobs' steps "Screenshot capture test" check the exit code, the PNG signature and ≥ 640 × 480 (the macOS runner grants Screen Recording).

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
| AC-02 | `print_and_ctrl_shift_print_open_the_overlay_in_resident_mode` (`crates/blinzel-app/src/screenshot.rs`), CI: Windows, Windows job ok (CI run 41532658575, `51202b8`) | built |        |
| AC-03 | `a_taken_print_key_is_retried_for_a_minute_then_reported` (`crates/blinzel-app/src/screenshot.rs`), CI: Windows, Windows job ok (CI run 41532658575, `51202b8`); the notice's way: `the_print_key_notice_is_a_balloon_while_hidden_and_9_s_in_the_window` (`screenshot::route_hint`, `crates/blinzel-app/src/screenshot.rs`), CI: Windows — written, not run yet (a standalone copy on Linux was red without the `hint_pending` mark and with 3.5 s); `a_message_while_hidden_goes_to_the_tray_instead_of_the_notice` (`crates/blinzel-ui/src/app_resident_tests.rs`) for `App::tell` | open   |        |
| AC-04 | `cmd_shift_1_opens_the_overlay_or_says_the_menu_bar_icon_still_works` (`crates/blinzel-app/src/macos/hotkey.rs`), CI: macOS, macOS job ok (CI run 41532658575, `51202b8`) | built |        |
| AC-05 | `turning_the_screenshot_key_on_turns_resident_mode_on_too` (`crates/blinzel-ui/src/app_screenshot_tests.rs`) | built |        |
| AC-06 | `the_tray_entries_take_the_screen_or_open_the_overlay` (`crates/blinzel-ui/src/app_screenshot_tests.rs`), `the_whole_screen_is_the_monitor_under_the_pointer` (`crates/blinzel-ui/src/snip.rs`) | built |        |
| AC-07 | `a_screenshot_freezes_the_virtual_desktop_under_a_cloaked_overlay` (`crates/blinzel-app/src/screenshot.rs`), CI: Windows, Windows job ok (CI run 41532658575, `51202b8`) | built |        |
| AC-08 | `each_display_gets_a_panel_above_the_menu_bar_and_keeps_its_selection`, `a_frozen_picture_is_converted_to_srgb`, `every_display_is_frozen` (`crates/blinzel-app/src/macos/screenshot.rs`), CI: macOS, macOS job ok (CI run 41532658575, `51202b8`) | built |        |
| AC-09 | `quartz_frames_count_down_from_the_main_display`, `windows_map_into_display_pixels`, `crops_rows` (`crates/blinzel-app/src/macos/screenshot.rs`), macOS CI job | built |        |
| AC-10 | `a_click_takes_the_frontmost_window_or_the_monitor`, `a_release_3_px_away_is_a_click_and_4_px_away_a_drag` (red with `DRAG_MIN` = 6) (`crates/blinzel-ui/src/snip.rs`), Linux CI job and all others | built |        |
| AC-11 | `drags_a_region_in_any_direction`, `a_release_3_px_away_is_a_click_and_4_px_away_a_drag` (`crates/blinzel-ui/src/snip.rs`) | built |        |
| AC-12 | `enter_takes_the_monitor_escape_cancels`, `arrows_move_the_pointer_and_enter_finishes_a_drag` (`crates/blinzel-ui/src/snip.rs`) | built |        |
| AC-13 | `arrows_move_the_pointer_and_enter_finishes_a_drag` (`crates/blinzel-ui/src/snip.rs`) | built |        |
| AC-14 | `a_right_click_or_losing_the_foreground_cancels_the_overlay` (`crates/blinzel-app/src/screenshot.rs`), CI: Windows, Windows job ok (CI run 41532658575, `51202b8`); `a_right_click_or_losing_the_foreground_cancels_the_overlay` (`crates/blinzel-app/src/macos/screenshot.rs`), CI: macOS, macOS job ok (CI run 41532658575, `51202b8`) | built |        |
| AC-15 | `a_shot_that_misses_the_clipboard_or_its_file_says_why` (`crates/blinzel-ui/src/app_screenshot_tests.rs`) covers the notice; `a_screenshot_goes_on_the_clipboard_as_dibv5_and_png` (`crates/blinzel-app/src/clipboard.rs`), CI: Windows, Windows job ok (CI run 41532658575, `51202b8`); `a_screenshot_goes_on_the_pasteboard_as_png` (`crates/blinzel-app/src/macos/shell.rs`), CI: macOS, macOS job ok (CI run 41532658575, `51202b8`) | built |        |
| AC-16 | `destination_follows_the_settings` (`crates/blinzel-core/src/screenshot.rs`) | built |        |
| AC-17 | `writes_free_names_and_sweeps_old_shots` (`crates/blinzel-core/src/screenshot.rs`) | built |        |
| AC-18 | `the_folder_is_the_chosen_one_else_the_systems`, `the_file_is_named_after_the_time_in_the_ui_language` (`crates/blinzel-app/src/screenshot.rs`), CI: Windows, Windows job ok (CI run 41532658575, `51202b8`); `the_folder_is_the_chosen_one_else_pictures_screenshots`, `the_file_is_named_after_the_time_in_the_ui_language` (`crates/blinzel-app/src/macos/screenshot.rs`), CI: macOS, macOS job ok (CI run 41532658575, `51202b8`) | built |        |
| AC-19 | `a_shot_into_the_cache_sweeps_its_old_pngs_first`, `writes_free_names_and_sweeps_old_shots` (`crates/blinzel-core/src/screenshot.rs`) | built |        |
| AC-20 | `a_shot_opens_in_the_viewer_or_says_it_was_copied_then_goes_to_the_editor` (`crates/blinzel-ui/src/app_screenshot_tests.rs`), `edit_asks_for_an_editor_first` (`crates/blinzel-ui/src/app.rs`) | built |        |
| AC-21 |      | open   |        |
| AC-22 | CI step "Screenshot capture test" (Windows job, `.github/workflows/ci.yml`) | built |        |
| AC-23 | `the_capture_test_exits_0_with_its_size_else_1` (`crates/blinzel-app/src/macos/screenshot.rs`) and the CI step "Screenshot capture test" (macOS job: exit code, PNG signature, ≥ 640 × 480), CI: macOS, macOS job ok (CI run 41532658575, `51202b8`) | built |        |

## Decisions

- AC-01: the menus' halves existed and were tested per platform (`menu::items_for`, `menu::tray_items_with`). That the Linux shell ignores the screenshot actions the App still gives (resident mode's toggle sends `SetScreenshotHotkey` on every platform) is the private `takes_screenshots`, checked before the shell's dispatch like `ends_process`; the App itself stays platform-independent and keeps answering the commands, since its tests run every platform's menu on Linux. The behavior existed; dropping an action from `takes_screenshots` turns the test red.
- AC-05, AC-06 (the App half): the behavior existed and the tests were green at once; skipping the `SetResident(true)` or swapping `whole_screen` turns them red. The monitor under the pointer was the same `find` in the overlay and both shells; it is now `snip::monitor_under` (new public API of `blinzel-ui`), tested on Linux. That the shells take it at once without the overlay is their early return in `start` (Windows) and `frozen` (macOS), without a test of its own.
- AC-15, AC-20: which notice a taken shot gets (clipboard failed, file failed, only copied, none while it opens in the viewer) was the same match in the Windows and macOS shells; `App::on_screenshot(copied, file)` now returns the actions and that text (changed public API of `blinzel-ui`), and each shell shows it through `App::tell`. The tests were written against the new signature first (they did not type-check). A hidden App reloads the settings when it opens a file, so the test answers the `Wake` with `App::wake` as the shell does before it changes more settings.
- AC-19: the shells swept the cache before writing in their own copy of the same lines; `screenshot::store` in `blinzel-core` (new public API) now chooses the destination, sweeps the cache only when the shot goes there and writes, and both shells call it. The test was written against the missing function first; sweeping always or never turns it red.
- AC-15: what goes on the clipboard is a pure function in each shell: `clipboard::shot_formats` (Windows: `CF_DIBV5` of the pixels and the registered `PNG` format with the shot's own PNG), and `png_item` (macOS: the PNG as `public.png`, shared by a copy and a shot). The tests check the payloads, not the system clipboard: a second test writing the real clipboard would race the copy tests of `selection-and-export.md` in the same test binary. That `put_shot` and `put_png` put exactly these payloads is their only line. The tests type-check for both targets (clippy clean); they run only in the Windows and macOS CI jobs, whose first runs are the proof.
- AC-18: the folder is `folder_in(settings, known)` (Windows; `known` stands for `SHGetKnownFolderPath`) and `folder_in(settings, home)` (macOS), the name `file_stem_at(lang, time)` in both; the shells pass the real folder lookup, `$HOME`, the UI language and the local time. The Windows tests also check this machine's real folder (absolute, ending in `Screenshots`). The Windows folder and name logic ran green in a standalone copy on Linux, and red with Screenshots and Pictures swapped, without `\Screenshots`, ignoring the chosen folder, with `HH:mm:ss` and in English only. The macOS name goes through `NSDateFormatter` and is checked by its shape (`yyyy-MM-dd HHmmss` in local time) only on macOS. In the crate the tests run only in the Windows and macOS CI jobs, whose first runs are the proof.
- AC-03: the defect: after its twelfth try the retry timer said "Windows uses the Print key" even when that try had registered the key (`wanted && left == 0` did not ask whether it was free). The decision is now `screenshot::retry(wanted, free, left)` (`Again`, `Stop`, `GiveUp`) with `RETRY_TRIES` × `RETRY_MS` = one minute. The test ran in a standalone copy on Linux: red against the old condition ("the last try got it"), with 11 tries and without the give-up, green with the fix. Where the notice goes after `GiveUp` is `screenshot::route_hint(App::tell(…))`: a balloon that marks the notice for the next wake while hidden, otherwise `HINT_MS` (9 s) in the window, the time the wake and the menu toggle use too; the toggle's "at once" stays shell code without a test.
- AC-02: the keys are `hotkeys()` (Ctrl+Shift+Print, then Print, both `MOD_NOREPEAT`, on `VK_SNAPSHOT`), which `set_hotkey` registers; when they belong registered is `hotkey_wanted` (resident mode and the key setting), used at start and by the retry timer; what `WM_HOTKEY` does is `ON_HOTKEY` (`Action::Capture { whole_screen: false }`), the same whether the window is shown or hidden. No test registers a real global key.
- AC-07: the overlay's window is `overlay_window(origin, size)`: the test grabs the CI desktop (`grab`, `SRCCOPY | CAPTUREBLT` through `capture_rop`), opens the overlay's window over it and reads its rect (the virtual screen, 1:1), its extended style (`WS_EX_TOPMOST`, `WS_EX_TOOLWINDOW`, `WS_EX_NOREDIRECTIONBITMAP`) and `DWMWA_CLOAKED`; it is never shown. Uncloaking after the first present is `open_overlay`'s, which needs the viewer's Direct3D device, and stays untested.
- AC-14 (Windows): what closes the overlay is `cancels(msg, wp)`: `WM_RBUTTONUP`, or `WM_ACTIVATE` with `WA_INACTIVE` in its low word.
- AC-02, AC-07, AC-14: the tests type-check for `x86_64-pc-windows-msvc` (clippy clean) and run only in the Windows CI job, whose first run is the proof.
- AC-04: the key is `combo()` (kVK_ANSI_1 with cmdKey | shiftKey), which `hotkey::set` registers; when it belongs registered is `hotkey::wanted` (resident mode and the key setting), used at launch; what a press does is `ON_PRESS` (`Action::Capture { whole_screen: false }`), shown or hidden; the notice after registering is `hotkey::notice(registered)`, used at launch and by `SetScreenshotHotkey`. No test registers a real Carbon hot key: Carbon's event target belongs to the main thread, and the tests run on others.
- AC-08: each panel's window is `panel_style()` (borderless, `NSScreenSaverWindowLevel`, on every Space and over full-screen apps, outside the window cycle), its overlay `snip_of(frozen)` with that display's picture as the only monitor, the pointer's place on it `cursor_on`. The sRGB conversion is `to_bgra`, tested with a Display P3 picture; `every_display_is_frozen` grabs every active display through ScreenCaptureKit off the main thread, as the app does. A panel itself needs the main thread (AppKit), which no test has, so the panels are never opened in a test. The drag and the pointer ran green in a standalone copy on Linux, and red with the overlay twice as wide and with the pointer ignoring the Retina scale.
- AC-14 (macOS): the panel's view answers `rightMouseDown:` itself (NSView would pass it on) with `close()`, and the app delegate's `applicationDidResignActive:` closes the overlay when another app comes to the front; the test checks both through the Objective-C runtime. A panel losing key status alone does not cancel: moving the pointer to another display makes that display's panel key.
- AC-23: what `--capture-test` prints and returns is `capture_test_exit` (0 and the size, or 1 and the reason); no permission is `may_record(false)`. The exit-code mapping ran green in a standalone copy on Linux, and red with an error exiting 0. The last three macOS CI runs wrote a 1920 × 1080 PNG, so the step's `continue-on-error` is gone and it checks the PNG like the Windows step.
- AC-04, AC-08, AC-14, AC-23 (macOS): the tests type-check for `aarch64-apple-darwin` (clippy clean) and run only in the macOS CI job, whose first run is the proof.
