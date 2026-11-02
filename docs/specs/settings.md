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
- Migrating or versioning the file — because unknown keys are ignored and missing keys get defaults
- Settings shared between users or synced between machines — because the file is per user

## Acceptance criteria

- [ ] AC-01 (always): The settings file is `settings.toml` in `%APPDATA%\amigo-blinzel` on Windows, `~/Library/Application Support/amigo-blinzel` on macOS, and `$XDG_CONFIG_HOME/amigo-blinzel` on Linux (`~/.config/amigo-blinzel` if `XDG_CONFIG_HOME` is unset or empty).
- [ ] AC-02 (error): If the settings file is missing or unreadable, then the viewer starts with the defaults: `view_mode = "fit-image-to-window"`, `no_upscale = true`, `language = "auto"`, `single_instance = true`, `prefetch_ahead = 4`, `prefetch_behind = 2`, `background = "#1E1E1E"`, `auto_update = true`, `resident = false`, `color_management = true`, `filmstrip = false`, `maximized = false`, `screenshot_hotkey = true`, `screenshot_save = true`, `screenshot_open = true`, `screenshot_edit = false`, and no `window`, `screenshot_dir` or `editor`.
- [ ] AC-03 (always): Every setting the viewer writes reads back unchanged, including negative window coordinates and paths that contain `#` or spaces.
- [ ] AC-04 (error): If the file contains unknown keys (also keys of older versions such as `screenshot_clipboard` or `ram_budget_mb`), section headers, lines without `=`, trailing `# comments` or values of the wrong type, then those lines are ignored, every invalid value keeps its default, and all valid lines still apply.
- [ ] AC-05 (error): If `prefetch_ahead` or `prefetch_behind` is larger than 32, then 32 is used.
- [ ] AC-06 (error): If `window` is not four comma-separated integers with a positive width and height, then no window rect is used.
- [ ] AC-07 (always): `language = "auto"` and `language = ""` both mean the system language.
- [ ] AC-08 (error): If `language` names a language that is not shipped, then the viewer uses the system language and keeps the value in the file until the user picks a language in the menu.
- [ ] AC-09 (error): If `background` is not `#RRGGBB` in hex, or `screenshot_dir` / `editor` is empty, unquoted or contains a quote, then the default is used.
- [ ] AC-10 (event): When the user changes a setting from the menu (view mode, "Don't enlarge small images", filmstrip, language, every entry under Menu → Settings) or picks an editor or a screenshot folder, then the file is written at once with all current settings.
- [ ] AC-11 (error): If the picked editor or screenshot folder path contains a quote, CR or LF, then it is not stored.
- [ ] AC-12 (error): If the file cannot be written, then the viewer keeps running with the changed setting in memory and shows no message.
- [ ] AC-13 (always): Menu → Settings offers "Keep ready in the background" (Windows and macOS), "Open images in the same window", "Use colour profiles (ICC)", "Install updates automatically", "Choose editor…" and, on Windows and macOS, the screenshot key ("Print key takes screenshots", on macOS "⌘⇧1 takes screenshots"), "Save screenshots in the folder", "Open screenshots in the viewer", "Open screenshots in the editor" and "Screenshot folder…"; each check item is checked when its setting is on (the screenshot key only while resident mode is on too); `prefetch_ahead`, `prefetch_behind` and `background` can only be changed in the file.
- [ ] AC-14 (event, CI: windows): When the window closes or hides on Windows, then its normal (not maximized) rect and whether it is maximized are saved as `window` and `maximized`; Linux and macOS never write these keys.
- [ ] AC-15 (event): When the viewer exits normally, then it writes the settings file once more, unless it is a hidden resident instance on Windows.
- [ ] AC-16 (always): The settings file has no key `ram_budget_mb`: the viewer never writes it, and a file from an older version that contains it loads like any other unknown key (AC-04).

## Technical notes

- Model: `blinzel_core::settings::Settings` (`parse`, `to_text`); file access: `blinzel_ui::settings_store` (`path`, `load`, `save`); folders: `blinzel_ui::dirs::config` (and `dirs::cache`, which holds caches such as thumbnails and, on Windows, the remembered display profile).
- Format: a TOML subset, one `key = value` per line; strings in double quotes, booleans `true`/`false`, integers; `window = left,top,right,bottom`. The first pass cuts every line at the first `#`; `background`, `screenshot_dir` and `editor` are read in a second pass from the raw line because they may contain `#`. `to_text` writes every key in a fixed order under a `# amigo-blinzel settings` header, `window`, `screenshot_dir` and `editor` only when set.
- Writes are whole-file `std::fs::write` after `create_dir_all`; errors are ignored.
- Readers: the shells read the file once at startup (language before anything else); `App::reload_settings` re-reads it when a hidden resident instance wakes; the updaters re-read `auto_update` before checking.
- `ram_budget_mb` is parsed and written but nothing reads it; it is to be removed (AC-16, Decisions).

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
| AC-01 | — | open | |
| AC-02 | — (`roundtrip` checks that the defaults read back, not their values) | open | |
| AC-03 | `roundtrip` (`crates/blinzel-core/src/settings.rs`), CI: Linux | built | |
| AC-04 | `tolerates_garbage`, `old_screenshot_clipboard_key_is_ignored` (`crates/blinzel-core/src/settings.rs`), CI: Linux | open (reworded 2026-11-02) | |
| AC-05 | `tolerates_garbage` (`crates/blinzel-core/src/settings.rs`), CI: Linux | built | |
| AC-06 | `roundtrip` (`crates/blinzel-core/src/settings.rs`), CI: Linux | built | |
| AC-07 | — (`tolerates_garbage` covers `"auto"` only, not `""`) | open | |
| AC-08 | — | open | |
| AC-09 | — | open | |
| AC-10 | — | open | |
| AC-11 | — | open | |
| AC-12 | — | open | |
| AC-13 | — | open | |
| AC-14 | — | open | |
| AC-15 | — | open | |
| AC-16 | — | open | |

## Decisions
