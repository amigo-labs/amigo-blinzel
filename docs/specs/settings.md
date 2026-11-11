---
status: todo
created: 2026-11-02
size: S
platforms: all
design: [blinzel.md §4, blinzel.md §5, blinzel.md §6, blinzel.md §12]
---

# Settings file

## Context

The viewer remembers the user's choices (view mode, language, colour management, resident mode, updates, screenshots, editor, window placement) in one small text file per user. The design spec lists the settings file in `blinzel-core` and `blinzel-ui` (`blinzel.md` §4) and in milestone M6 (`blinzel.md` §12); resident mode reloads it when a hidden instance wakes (`blinzel.md` §5) and Windows keeps the window placement in it (`blinzel.md` §6). Reading it must never stop the viewer from starting, and must not cost startup time.

This spec records what the code does today.

## Scope

### In scope

- Where the file lives on each platform
- Its format, every key and its default
- Robustness against unknown, invalid and old keys
- When it is written, and which settings the menu changes

### Out of scope

- What each setting does — because the feature specs that own them describe it (localization, color management, resident mode, updates, screenshots, platform shells)
- A settings dialog — because all settings are menu entries or file-only
- Migrating or versioning the file — because unknown keys are ignored and missing keys get defaults; the one exception is `auto_update` → `updates` (AC-17), because the update choice must tell a made choice from none
- Settings shared between users or synced between machines — because the file is per user

## Acceptance criteria

- [x] AC-01 (always): The settings file is `settings.toml` in `%APPDATA%\amigo-blinzel` on Windows, `~/Library/Application Support/amigo-blinzel` on macOS, and `$XDG_CONFIG_HOME/amigo-blinzel` on Linux (`~/.config/amigo-blinzel` if `XDG_CONFIG_HOME` is unset or empty).
- [ ] AC-02 (error): If the settings file is missing or unreadable, then the viewer starts with the defaults: `view_mode = "fit-image-to-window"`, `no_upscale = true`, `language = "auto"`, `single_instance = true`, `prefetch_ahead = 4`, `prefetch_behind = 2`, `background = "#1E1E1E"`, no `updates` (the update choice: not yet made, which means "install automatically", `updates.md` AC-13), `resident = false`, `color_management = true`, `filmstrip = false`, `maximized = false`, `screenshot_hotkey = true`, `screenshot_save = true`, `screenshot_open = true`, `screenshot_edit = false`, and no `window`, `screenshot_dir` or `editor`.
- [x] AC-03 (always): Every setting the viewer writes reads back unchanged, including negative window coordinates and paths that contain `#` or spaces.
- [x] AC-04 (error): If the file contains unknown keys (also keys of older versions such as `screenshot_clipboard` or `ram_budget_mb`), section headers, lines without `=`, trailing `# comments` or values of the wrong type, then those lines are ignored, every invalid value keeps its default, and all valid lines still apply.
- [x] AC-05 (error): If `prefetch_ahead` or `prefetch_behind` is larger than 32, then 32 is used.
- [x] AC-06 (error): If `window` is not four comma-separated integers with a positive width and height, then no window rect is used.
- [x] AC-07 (always): `language = "auto"` and `language = ""` both mean the system language.
- [x] AC-08 (error): If `language` names a language that is not shipped, then the viewer uses the system language and keeps the value in the file until the user picks a language in the menu.
- [x] AC-09 (error): If `background` is not `#RRGGBB` in hex, or `screenshot_dir` / `editor` is empty, unquoted or contains a quote, then the default is used.
- [ ] AC-10 (event): When the user changes a setting from the menu (view mode, "Don't enlarge small images", filmstrip, language, every entry under Menu → Settings), confirms or changes the update choice (first-start question, About dialog) or picks an editor or a screenshot folder, then the file is written at once with all current settings.
- [x] AC-11 (error): If the picked editor or screenshot folder path contains a quote, CR or LF, then it is not stored.
- [x] AC-12 (error): If the file cannot be written, then the viewer keeps running with the changed setting in memory and shows no message.
- [ ] AC-13 (always): Menu → Settings offers "Keep ready in the background" (Windows and macOS), "Open images in the same window", "Use colour profiles (ICC)", "Choose editor…" and, on Windows and macOS, the screenshot key ("Print key takes screenshots", on macOS "⌘⇧1 takes screenshots"), "Save screenshots in the folder", "Open screenshots in the viewer", "Open screenshots in the editor" and "Screenshot folder…"; each check item is checked when its setting is on (the screenshot key only while resident mode is on too); `prefetch_ahead`, `prefetch_behind` and `background` can only be changed in the file.
- [x] AC-14 (event, CI: windows): When the window closes or hides on Windows, then its normal (not maximized) rect and whether it is maximized are saved as `window` and `maximized`; Linux and macOS never write these keys.
- [x] AC-15 (event): When the viewer exits normally, then it writes the settings file once more, unless it is a hidden resident instance on Windows.
- [x] AC-16 (always): The settings file has no key `ram_budget_mb`: the viewer never writes it, and a file from an older version that contains it loads like any other unknown key (AC-04).
- [ ] AC-17 (always): The update choice is stored as `updates = "auto"`, `"notify"` or `"off"`, and the file holds `updates` only after the user chose, a silent setup stored it (`updates.md` AC-32), or it replaced an old `auto_update`; an unknown value counts as not chosen. A file from an older version with `auto_update = true` or `false` and no `updates` reads as `"auto"` or `"off"`, and the next write stores `updates` in its place and no `auto_update`.

## Technical notes

- Model: `blinzel_core::settings::Settings` (`parse`, `to_text`); file access: `blinzel_ui::settings_store` (`path`, `load`, `save`); folders: `blinzel_ui::dirs::config` (and `dirs::cache`, which holds caches such as thumbnails and, on Windows, the remembered display profile).
- Format: a TOML subset, one `key = value` per line; strings in double quotes, booleans `true`/`false`, integers; `window = left,top,right,bottom`. The first pass cuts every line at the first `#`; `background`, `screenshot_dir` and `editor` are read in a second pass from the raw line because they may contain `#`. `to_text` writes every key in a fixed order under a `# amigo-blinzel settings` header, `window` and `maximized` only when a rect is set, `screenshot_dir` and `editor` only when set.
- Writes are whole-file `std::fs::write` after `create_dir_all`; errors are ignored.
- Readers: the shells read the file once at startup (language before anything else); `App::reload_settings` re-reads it when a hidden resident instance wakes; the updaters re-read the update choice (`updates`) before checking.
- `ram_budget_mb` was parsed and written but nothing read it; it is gone from `Settings` (AC-16).

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| `prefetch_ahead = 1000` | 32 |
| `prefetch_ahead = -1` | Default 4 |
| `no_upscale = maybe` | Default `true` |
| `view_mode = "nope"` | Default view mode |
| `window = 10,10,5,20` (negative width) | No rect |
| `screenshot_dir = "D:\Bilder #2\Screens"` | Read with the `#` |
| `language = "xx"` | System language; value kept until the user picks one |
| File from an older version with `screenshot_clipboard = true` | Key ignored, other keys apply |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `the_settings_folder_follows_each_platforms_convention`, `this_platforms_settings_folder_is_the_one_tested` (`crates/blinzel-ui/src/dirs.rs`) | verified | 2c26c2f |
| AC-02 |      | open   |        |
| AC-03 | `roundtrip` (`crates/blinzel-core/src/settings.rs`) | verified | — |
| AC-04 | `unknown_keys_and_invalid_lines_are_ignored_and_valid_lines_apply`, `tolerates_garbage`, `old_screenshot_clipboard_key_is_ignored` (`crates/blinzel-core/src/settings.rs`) | verified | f66b7c5 |
| AC-05 | `tolerates_garbage` (`crates/blinzel-core/src/settings.rs`) | verified | cd47be9 |
| AC-06 | `roundtrip` (`crates/blinzel-core/src/settings.rs`) | verified | cd47be9 |
| AC-07 | `auto_and_empty_language_both_mean_the_system_language` (`crates/blinzel-core/src/settings.rs`) | verified | f66b7c5 |
| AC-08 | `an_unknown_language_shows_the_system_language_and_stays_in_the_file` (`crates/blinzel-ui/src/app_settings_tests.rs`) | verified | 5e84e1e |
| AC-09 | `invalid_background_and_paths_keep_their_defaults` (`crates/blinzel-core/src/settings.rs`) | verified | f66b7c5 |
| AC-10 |      | open   |        |
| AC-11 | `a_picked_path_with_a_quote_or_line_break_is_not_stored` (`crates/blinzel-ui/src/app_settings_tests.rs`) | verified | 5e84e1e |
| AC-12 | `an_unwritable_file_keeps_the_change_in_memory_without_a_message` (`crates/blinzel-ui/src/app_settings_tests.rs`) | verified | 5e84e1e |
| AC-13 |      | open   |        |
| AC-14 | `remembers_the_normal_rect_and_whether_it_is_maximized` (`crates/blinzel-app/tests/placement.rs`), CI: windows, green in CI run 186 on 0b5dc42; `the_window_placement_is_written_only_once_a_rect_is_known` (`crates/blinzel-core/src/settings.rs`), `the_app_never_writes_the_window_placement_itself` (`crates/blinzel-ui/src/app_settings_tests.rs`) | verified | c1d52c9 |
| AC-15 | `exiting_writes_the_settings_once_more`, `only_a_hidden_instance_on_windows_skips_the_write_on_exit` (`crates/blinzel-ui/src/app_settings_tests.rs`) | verified | 5e84e1e |
| AC-16 | `ram_budget_mb_is_neither_read_nor_written`, `unknown_keys_and_invalid_lines_are_ignored_and_valid_lines_apply` (`crates/blinzel-core/src/settings.rs`) | verified | f66b7c5 |
| AC-17 |      | open   |        |

## Summary

> Completed: 2026-11-02

- `settings.toml` lives in each platform's settings folder (`blinzel_ui::dirs::config`); a missing, unreadable or partly invalid file never stops the start: invalid lines and values fall back to the documented defaults, valid lines still apply.
- `blinzel_core::settings::Settings` is stricter: `ram_budget_mb` is gone, `background` must be six hex digits, and quoted values may carry a trailing `# comment`; `maximized` is written only together with `window`.
- Every menu change, a picked editor or screenshot folder, and a normal exit (`App::save_settings_on_exit`, skipped only by a hidden resident instance on Windows) write the whole file at once; a failed write keeps the change in memory without a message.
- Only the Windows shell writes `window` and `maximized` (`remember_placement` on close and hide); Linux and macOS never set either key.

## Decisions

- `ram_budget_mb` (AC-16, decided 2026-11-02): the field is removed from `blinzel_core::settings::Settings`, so `parse` treats the key like any unknown key and `to_text` no longer writes it; nothing read it.
- A quoted value read from the raw line (`background`, `screenshot_dir`, `editor`) may be followed by a `# comment` (AC-04): the value is the text between the first two quotes. Before, `background = "#102030" # dark` fell back to the default.
- `background` must be six hex digits after `#` (AC-09): `u32::from_str_radix` alone also took a sign, so `"#+12345"` was read as a colour.
- AC-01 green at once: the paths were right; `dirs::config` and `dirs::cache` now take the platform and the environment as parameters internally (`config_in`, `cache_in`), so the Windows and macOS conventions are tested on Linux too.
- AC-02 and AC-07 green at once: the defaults and the meaning of `"auto"` / `""` already held; the tests pin the documented values.
- AC-13: `menu::items` is now `menu::items_for(state, THIS_PLATFORM)` with a crate-private `menu::Platform` (resident mode, screenshots, macOS key name), so the Settings menus of Windows, macOS and Linux are all tested on Linux; each build still shows only its own.
- AC-15: the exit write moved into `blinzel-ui` as `App::save_settings_on_exit` (new public API), which the Windows, Linux and macOS shells call where they used to call `settings_store::save` on exit. Whether a hidden instance skips it is the constant `HIDDEN_EXIT_KEEPS_FILE = cfg!(windows)`; the rule is tested for both values on Linux. The shells' call sites themselves run in no test (they need a display, Windows or macOS).
- AC-08: tested through `App::reload_settings` (a hidden instance waking), which resolves the language with the same `lang_from_code(…).unwrap_or(system language)` as the three shells' starts.
- AC-08, AC-10 to AC-13 green at once: the behavior existed. Counter-checks went red: dropping the save after "Install updates automatically" (AC-10), the quote filter of `screenshot_folder_picked` (AC-11), keeping an unknown language in memory (AC-08) and the "only while resident" rule of the screenshot key (AC-13).
- AC-14 "Linux and macOS never write these keys": `to_text` wrote `maximized = false` into every file. It now writes `maximized` only together with `window`, which only the Windows shell sets (`remember_placement` on close and hide); a `maximized` without a rect had no effect anyway (`restore_placement` ignores it).
- AC-14, Windows part: `blinzel_app::window::remember_placement` now takes `&mut Settings` instead of the App and is public, so `tests/placement.rs` checks it on a real (test) window, maximized with `ShowWindow(SW_MAXIMIZE)`. The test runs only in the Windows CI job (`cargo test --workspace --release`); it could not even be compiled here (no MSVC tools for the C dependencies' build scripts), so its first CI run is the proof.
- AC-15 has no red run of the behavior: its tests call `App::save_settings_on_exit`, which did not exist before (the red run was the compile error); what it does is what the shells did before.
- 2026-11-11: revised for the update choice (`updates.md`, revision 2026-11-11; `blinzel.md` §2 "Updates"). `auto_update` gives way to `updates` with three values (AC-17, new), written only once the user has chosen, so a missing key tells a first start from a made choice; the Settings menu loses "Install updates automatically", whose place is the About dialog. AC-02, AC-10 and AC-13 reworded and back to `open`; the spec goes back to `status: todo`.
