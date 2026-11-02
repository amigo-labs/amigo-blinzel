---
status: todo
created: 2026-11-02
size: M
platforms: Windows, macOS
design: [blinzel.md §2, blinzel.md §5, blinzel.md §9, blinzel.md §12]
---

# Resident mode

## Context

Opening an image costs a process start, a GPU device and the first use of shaders and decoders. Resident mode (decision in `blinzel.md` §2: "optional (tray, kept warm)") keeps one process alive after the window is closed, so a later launch only hands its file over and the warm instance shows it. This spec records what the Windows and macOS shells do today; it was extracted from the code (reverse engineering), not designed anew.

## Scope

### In scope

- The setting and its menu entry, closing that hides instead of exiting, what a hidden instance releases and keeps
- Handing a launch over to a hidden instance, and how "Open images in the same window" changes the target
- Waking: cloaked show, settings reload, foreground
- The tray icon (Windows) and the menu bar icon (macOS) with their menu
- Start at sign-in (HKCU `Run` value on Windows, LaunchAgent on macOS) and the `--resident` start with its warm-up
- An installed update taking over when the window hides
- The M6 target "resident handoff → visible ≤ 25 ms"

### Out of scope

- Linux — because the Linux shell has no tray and no sign-in item yet (`blinzel.md` §9 "Not on Linux"); the setting is not offered there
- Screenshots taken by the resident instance — because they are their own feature (`screenshots.md`)
- The cold-start handoff of a non-resident single instance and its timing — because it belongs to the startup path (`blinzel.md` §5 "Parallel cold start")
- How updates are downloaded and installed — because that is the updater (`blinzel.md` §10)

## Acceptance criteria

- [ ] AC-01 (always): Resident mode is off unless the user turns it on; the Settings menu offers it on Windows and macOS, and on Linux the entry is missing and closing the window always ends the process.
- [ ] AC-02 (event): When resident mode is toggled in the Settings menu, the setting is saved, the tray / menu bar icon and the sign-in entry appear (on) or go away (off) at once, and the screenshot key follows (registered only while both resident mode and the screenshot key setting are on).
- [ ] AC-03 (state, CI: windows): While resident mode is on and no other hidden instance exists, closing the window (close button, Alt+F4, menu "Exit") leaves full screen if needed, saves the window placement and the settings, and hides the window instead of ending the process.
- [ ] AC-04 (state, CI: windows): While another hidden resident instance already exists (several windows because "Open images in the same window" is off), closing a further window ends that process instead of hiding it.
- [ ] AC-05 (event): When the window is hidden, the viewer drops the shown image, pending decode jobs, the folder listing and its watch, the textures, thumbnails, animations, the notice and the view state, and gives idle GPU and buffer-pool memory back; the graphics device, shaders and decoder threads stay alive.
- [ ] AC-06 (always, CI: windows): A request from a second launch is understood as "open" (absolute path plus, optionally, the sender's process creation time), "show" or "quit"; an open request with an empty path and requests with an unknown tag are ignored.
- [ ] AC-07 (option, CI: windows): Where "Open images in the same window" is off, a launch with a file hands over only to a hidden resident instance and otherwise opens its own window; where it is on, the launch hands over to any running instance, visible or hidden. A launch that handed over exits with code 0.
- [ ] AC-08 (event, CI: windows): When the viewer is launched without a file while resident mode is on and a target instance exists, that instance shows its window and the launch exits.
- [ ] AC-09 (event): When a hidden instance is woken, it is shown without activation and invisible (cloaked / alpha 0), becomes visible with the first frame of the handed-over file (after 400 ms at the latest) or at once with the empty-start hint, and then takes the foreground; a visible instance is only activated.
- [ ] AC-10 (event): When a hidden instance is woken, by a file or without one, it reloads the settings from disk first, including language and colour management.
- [ ] AC-11 (state, CI: windows): While an instance is hidden, ending it (tray "Exit", the uninstaller's quit request, or someone else closing the window) does not write the settings file.
- [ ] AC-12 (always): The tray menu holds, in this order: "Show amigo-blinzel", "Open…", a separator, "Take screenshot…", "Screenshot of the whole screen", a separator and "Exit"; "Exit" ends the process for good, also in resident mode, and "Open…" wakes the window with the chosen file.
- [ ] AC-13 (state, CI: windows): While resident mode is on, the notification area shows the app icon with the app name as tooltip; a left click, keyboard selection or a click on its balloon shows the window, a right click opens the tray menu (with "Show" as the default item), and after Explorer restarts the icon is added again.
- [ ] AC-14 (state, CI: macos): While resident mode is on, macOS shows a menu bar item (SF Symbol "eye" as a template image, the app name as tooltip) whose menu is the tray menu, filled each time it opens; while the window is hidden the app is not in the Dock or the app switcher, and the Dock icon, opening a file from the Finder or "Show" wake it.
- [ ] AC-15 (option, CI: windows): Where resident mode is on, the value `amigo-blinzel` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` holds `"<exe>" --resident`; turning resident mode off removes the value, and uninstalling removes it too.
- [ ] AC-16 (event, CI: windows): When an instance starts with resident mode on and the sign-in value points to another executable (moved or reinstalled), the value is rewritten to the running executable; before uninstalling, every running instance is asked to quit and awaited (up to 30 s each).
- [ ] AC-17 (always, CI: macos): On macOS the sign-in item is the LaunchAgent `~/Library/LaunchAgents/io.github.amigo-labs.blinzel.plist` with the label `io.github.amigo-labs.blinzel`, the program path (XML-escaped) and `--resident` as arguments and `RunAtLoad` true.
- [ ] AC-18 (option, CI: macos): Where resident mode is turned on in a copy that does not run from an app bundle, no LaunchAgent is written (an existing one is removed); turning resident mode off removes it.
- [ ] AC-19 (error): If the viewer is started with `--resident` while resident mode is off, it exits with code 0 without a window and removes the sign-in entry (the `Run` value on Windows, the LaunchAgent on macOS); on Windows a `--resident` start also exits at once if an instance is already running.
- [ ] AC-20 (event, CI: windows): When started with `--resident`, the viewer shows no window and no taskbar button, and before idling decodes, uploads and presents a generated 3072 × 2048 JPEG once and, with colour management on, builds one colour transform (Display P3 → sRGB), so the platform's colour system is loaded.
- [ ] AC-21 (event): When a window hides while an update has been installed whose version differs from the running one, the instance starts the installed version with `--resident` (macOS: `open -n -g <bundle> --args --resident`) and ends.
- [ ] AC-22 (state): While the window is hidden, messages for the user (screenshot copied or failed, screenshot key taken) appear as a tray balloon (Windows) or for 4 s as a title next to the menu bar icon (macOS) instead of an in-window notice.
- [ ] AC-23 (event): When a second launch hands a 24 MP JPEG to a warm hidden instance on an idle Windows 11 machine, the window is visible ≤ 25 ms after the instance received the handoff (median over repeated handoffs of `handoff_received` → `uncloak` in the marks file of an instance started with `blinzel.exe --resident --json <file>`).

## Technical notes

- Windows: `crates/blinzel-app/src/resident.rs` (tray via `Shell_NotifyIconW`, `NOTIFYICON_VERSION_4`, `TaskbarCreated`, balloons, `set_autostart` / `refresh_autostart`), `instance.rs` (`WM_COPYDATA` tags `COPYDATA_OPEN` 0xB11E, `COPYDATA_SHOW` 0xB11F, `COPYDATA_QUIT` 0xB120; `target(single_instance, resident)`, `hidden(except)`, `quit_all`), `window.rs` (`Action::Close` → `hide_window` or `exit`, `wake`, `restart_requested`), `run.rs` (`--resident` start, warm-up as a cloaked `WS_EX_TOOLWINDOW`), `setup/install.rs` (`autostart_command`, `set_autostart`, `uninstall`).
- macOS: `crates/blinzel-app/src/macos/resident.rs` (`NSStatusItem`, activation policy `Accessory` / `Regular`, `launch_agent`, `set_autostart` only via `update::bundle_program()`), `macos/shell.rs` (`windowShouldClose:`, `applicationShouldHandleReopen:hasVisibleWindows:`, `hide`, `wake`).
- Shared: `blinzel_ui::app::App::{hide, reload_settings, warm_up, open}`, `Action::{SetResident, TrayMenu, Wake { opened }, Close, Exit}`, `blinzel_ui::menu::{RESIDENT, tray_items}`, `Cmd::{ToggleResident, ShowWindow, Quit}`; setting `resident` (default `false`) in `blinzel_core::settings::Settings`.
- `App::hide` sets `bufpool::global().set_max_retained(0)`; `wake` restores `DEFAULT_MAX_RETAINED`.
- The warm-up runs only with the `turbojpeg` feature (default) and only on Windows; macOS starts hidden without a warm-up.
- Timing marks of a handoff count from the sender's process creation (`timing::init(age)` in `WM_COPYDATA`); the `wake_report` file gets `{"marks": …}` when the window is uncloaked.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Explorer crashes and restarts | The tray icon is added again (`TaskbarCreated`) |
| The hidden window is closed by another program (e.g. `taskkill` sends `WM_CLOSE`) | The process ends, without saving settings |
| A launch hands over, but the hidden instance hangs | `SendMessageTimeoutW` gives up after 2 s with `SMTO_ABORTIFHUNG`; the launch then starts its own window |
| A launch without a file while resident mode is off but "same window" is on | No handoff; a second window opens |
| macOS: the hidden app is told to quit (log-out, Dock) | The settings are saved (`applicationShouldTerminate:` saves unconditionally), unlike Windows |
| macOS: a bare executable (no app bundle) turns resident mode on | The menu bar icon appears, but no LaunchAgent is written |

## Open questions

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `a_missing_file_means_the_documented_defaults` (`crates/blinzel-core/src/settings.rs`), `menu_settings_offers_each_platforms_entries` (`crates/blinzel-ui/src/app_settings_tests.rs`), `closing_the_window_always_ends_the_process` (`crates/blinzel-app/src/linux/shell.rs`) | built |        |
| AC-02 | `toggling_resident_mode_saves_it_and_switches_the_icon_and_the_key` (`crates/blinzel-ui/src/app_resident_tests.rs`) proves the App half; the shells' `SetResident` handlers (tray icon, sign-in entry) have no test yet | open   |        |
| AC-03 | `closing_hides_a_resident_window_unless_another_one_is_hidden` (`crates/blinzel-app/src/window.rs`), CI: Windows — written, not run yet | open   |        |
| AC-04 | `closing_hides_a_resident_window_unless_another_one_is_hidden` (`crates/blinzel-app/src/window.rs`), CI: Windows — written, not run yet | open   |        |
| AC-05 | `hiding_drops_the_session_and_keeps_device_and_loader` (`crates/blinzel-ui/src/app_resident_tests.rs`) | built |        |
| AC-06 | `decodes_requests` (`crates/blinzel-app/src/instance.rs`), Windows CI job | built |        |
| AC-07 | `a_launch_goes_to_a_running_window_only_in_the_same_window_mode` (`crates/blinzel-app/src/instance.rs`), `a_launch_hands_its_file_or_a_show_request_over_and_exits_with_0` (`crates/blinzel-app/src/run.rs`), CI: Windows — written, not run yet | open   |        |
| AC-08 | `a_launch_hands_its_file_or_a_show_request_over_and_exits_with_0` (`crates/blinzel-app/src/run.rs`), CI: Windows — written, not run yet | open   |        |
| AC-09 | `a_woken_instance_stays_cloaked_until_its_first_frame_then_takes_the_foreground` (`crates/blinzel-ui/src/app_resident_tests.rs`) proves the App half; `a_woken_window_is_shown_cloaked_until_its_image_or_400_ms` (`crates/blinzel-app/src/window.rs`) the Windows shell's, CI: Windows — written, not run yet; the macOS shell's (alpha 0, `orderFront`, the 400 ms dispatch) has no test yet | open   |        |
| AC-10 | `a_woken_instance_reloads_the_settings_from_disk` (`crates/blinzel-ui/src/app_resident_tests.rs`) | built |        |
| AC-11 | `exiting_writes_the_settings_once_more` (`crates/blinzel-ui/src/app_settings_tests.rs`, its Windows branch), `closing_hides_a_resident_window_unless_another_one_is_hidden` (`crates/blinzel-app/src/window.rs`), CI: Windows — written, not run yet | open   |        |
| AC-12 | `the_tray_menu_shows_opens_takes_screenshots_and_exits` (`crates/blinzel-ui/src/app_resident_tests.rs`) | built |        |
| AC-13 |      | open   |        |
| AC-14 |      | open   |        |
| AC-15 | `installs_updates_and_uninstalls_in_a_sandbox` (`crates/blinzel-app/tests/setup.rs`), Windows CI job | built |        |
| AC-16 |      | open   |        |
| AC-17 | `launch_agent_starts_the_program_hidden` (`crates/blinzel-app/src/macos/resident.rs`), macOS CI job | built |        |
| AC-18 |      | open   |        |
| AC-19 | `a_resident_start_with_the_setting_off_removes_the_sign_in_value_and_exits` (`crates/blinzel-app/src/run.rs`), Windows CI job; `a_resident_start_with_the_setting_off_removes_the_launch_agent_and_exits` (`crates/blinzel-app/src/macos/shell.rs`), macOS CI job | built |        |
| AC-20 |      | open   |        |
| AC-21 |      | open   |        |
| AC-22 |      | open   |        |
| AC-23 |      | open   |        |

## Decisions

- AC-19: the defect from the extraction (a macOS `--resident` start with the setting off left the LaunchAgent in place) is fixed, and the AC now says both shells remove their sign-in entry. The decision is a private pure function in each shell (`resident_start` in `run.rs` and `macos/shell.rs`); the macOS test was written against the old behavior first (it type-checks, `remove_sign_in: false` fails its assertion), but neither test can run on Linux: their first Windows and macOS CI runs are the proof. The Windows half already behaved, its test is green at once.
- AC-05, AC-09, AC-10, AC-12: the wake state (reload the settings unless a file was just opened, cloak, foreground on uncloak, the buffer pool back) and "a file chosen in the open dialog wakes the window" were the same lines in the Windows and macOS shells; they moved into `App::wake` and `App::open_chosen` (new public API of `blinzel-ui`) so one Linux test covers both shells, and `menu::tray_items_with` (crate-private) gives the tray menu of a platform with screenshots on Linux. The tests describe existing behavior and were green at once; each was checked by breaking the code it covers (no save on toggle, the watch, textures or pool kept on hide, no reload, no colour or language switch, no cloak or foreground on wake, no wake after the dialog, "Exit" closing instead of exiting), and each break turned its test red.
- AC-02 and AC-09 stay open: their App halves are tested, the shell halves (tray icon and sign-in entry on `SetResident`; showing without activation, alpha 0 and the 400 ms fallback) are not.
- AC-01: the Linux shell ended the process in two places (the `Close`, `Exit` and `Quit` actions, and the window's close request); the close request now goes through `Action::Close` too, and one function `ends_process` (`linux/shell.rs`) decides for all of them before any other action is handled, so one Linux test covers every way of closing. The behavior existed and the test was green at once; removing `Close` from `ends_process` turns it red. Default off and the menu entry per platform were already tested (settings and Settings-menu tests).
- AC-03, AC-04, AC-11: the Windows close decision is a pure function `close_hides(resident, hidden, other_hidden)` in `window.rs` (the instance lookup only runs when it matters); its test covers hide or exit for every case. What `hide_window` does then (leave full screen, remember the placement, save the settings, hide) is not tested: a test of it would write the CI machine's real settings file (`settings_store` has its test folder only inside `blinzel-ui`). AC-11's "does not write" is the App's `save_settings_on_exit`, whose Windows branch the existing settings test checks; every way of ending a hidden instance (tray "Exit", the quit request, a close while hidden) goes through `exit`, which calls it. These tests run only in the Windows CI job; their first run there is the proof.
- AC-07, AC-08: the launch's hand-off is a pure function `hand_off(file, single_instance, resident, target, send)` in `run.rs` over `instance::target` (whose rules `pick` tests) and the two senders; it returns the exit code 0 when the instance took over. It now skips the instance lookup for a launch without a file while resident mode is off (it handed nothing over before either). The test ran green in a standalone copy on Linux, and red with the exit code changed; in the crate it runs only in the Windows CI job.
- AC-09: the WARP window test of `startup.md` AC-07 was split into helpers (`cloaked_window`, `warp_app`, `pump_while_cloaked`), and a second test wakes a hidden App through `run_actions`: shown, cloaked until the 400 ms timer (the loader has no device, so the image never comes), then activated; without a file visible at once; awake, only activated. It runs only in the Windows CI job.
