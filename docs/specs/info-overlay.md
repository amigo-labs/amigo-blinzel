---
status: todo
created: 2026-11-21
size: M
platforms: all
design: [blinzel.md §2, blinzel.md §5, blinzel.md §6, blinzel.md §8]
---

# Info overlay

## Context

The window has no frame and no toolbars (`blinzel.md` §2, "UI"). The name of the image is in the window title only, which the frameless window shows nowhere but the taskbar and Alt+Tab, and macOS hides it. When a person sorts through a shoot they want to see the file name (to tell the photographer which pictures they mean) and the basic facts of the picture: size, when it was taken, with which camera and settings.

The overlay is a box at the bottom left of the window that the user turns on and off with one key. It shows facts the viewer already knows (name, position in the folder, pixel size, format, file size) and facts from the image's EXIF block (date taken, camera, lens, exposure). The EXIF block is read today only for the orientation, the embedded previews (`exif.rs`) and, in the batch engine, for the date taken (`batch/meta.rs`).

Picking images and handing them over is a separate spec, [`picks.md`](picks.md); where both are built, the overlay also shows whether the image is marked (`picks.md` AC-07), and while "Only marked" is on its position counts only the marked images (`picks.md` AC-22).

## Scope

### In scope

- The key I, a menu entry and a setting that turn the overlay on and off and keep the choice
- The fields: name, position "n / total", pixel size, format, file size, date taken, camera, lens, f-number, exposure time, ISO, focal length
- Reading these EXIF fields for every format that carries an EXIF block, including TIFF-based RAW files
- The overlay in the grid, for the selected tile
- Date, number and size formats per UI language

### Out of scope

- File names under the tiles of the grid or the filmstrip — because they change the tile layout and its 30 ms budget (`thumbnails.md`), and the overlay for the selected tile covers the need
- GPS position, a map, a histogram, the full EXIF tag list — because the goal is telling images apart and judging them quickly, not a metadata browser
- Editing metadata — because the viewer never writes to an image (AGENTS.md §9)
- Choosing which fields appear, or where the box sits — because one fixed layout is enough until someone asks; a missing field simply drops out
- XMP and IPTC (title, rating, keywords) — because cameras write exposure data into EXIF; marks are `picks.md`'s

## Acceptance criteria

- [ ] AC-01 (event): When the user presses I (without Ctrl, with or without Shift) in the single view or the grid, or chooses "Show info" in the menu (next to "Filmstrip" and "Grid"), then the overlay turns on if it was off and off if it was on; the menu entry is checked while it is on. While a dialog or a popup is open, I goes to it as before.
- [ ] AC-02 (state): While the overlay is on and an image is shown, the viewer draws a box at the bottom left of the window with, one fact per line and in this order: the file name; the position "{n} / {total}" in the folder; "{width} × {height} · {format}"; the file size; the date taken; the camera; the lens; the exposure "f/{number} · {time} s · ISO {iso} · {focal} mm". Width and height are the image's full pixel size after the EXIF orientation (not the size of a smaller decode for the screen or of an embedded preview); the format is the container's common name: "JPEG", "PNG", "GIF", "BMP", "TIFF", "WebP", "HEIC", "AVIF", "JPEG XL", "ICO", "DDS", "PSD", "QOI", "JPEG XR", and for camera raw the file extension in capitals ("NEF", "CR2", "DNG", "CR3", "RAF", …), also where the viewer opens the file as TIFF.
- [ ] AC-03 (always): A fact the viewer does not know (no EXIF block, a tag missing, unreadable or out of range per AC-13) is left out together with its line or its part of the exposure line; no placeholder, no empty line. An image without any EXIF shows name, position, size and format, and file size.
- [ ] AC-04 (always): The camera line is make and model; the make is left out when the model, ignoring case, starts with the make's first word followed by a space or the end of the model (make "Canon", model "Canon EOS R6" → "Canon EOS R6"; make "NIKON CORPORATION", model "NIKON Z 6" → "NIKON Z 6"). Only one of the two present → that one. Surrounding spaces and trailing NULs are removed; bytes that are not valid UTF-8 are read as Latin-1.
- [ ] AC-05 (always): The exposure time reads as "1/{n}" below 0.5 s with n the reciprocal rounded to a whole number (10/2500 → "1/250"), and as a decimal with at most one decimal place, trailing zero dropped, from 0.5 s on (0.8 → "0.8", 2 → "2", 1.25 → "1.3"); the f-number is rounded to one decimal place and shown without it when that rounds to a whole number (f/2.8, f/8, 2.96 → f/3); the focal length is the lens's real focal length (not the 35 mm equivalent) rounded to whole millimetres. Decimal separators in these numbers follow the UI language (AC-07).
- [ ] AC-06 (always): The EXIF facts are read for JPEG, PNG, WebP, AVIF, HEIC, JPEG XL and TIFF, which includes the TIFF-based RAW files the viewer opens as TIFF (DNG, CR2, NEF, ARW, PEF); for every other format (also camera raw that is not plain TIFF, such as CR3, RAF, ORF, RW2) only the facts without EXIF appear. Where a file has several EXIF blocks, the first one counts.
- [ ] AC-07 (always): The date and the file size follow the UI language: the date as that language's pattern for day, month, year, hours and minutes, without seconds and without time zone (German "20.11.2026 14:32", English "2026-11-20 14:32"); the size in decimal units with that language's decimal separator — below 1 000 bytes "{n} B", then KB, MB, GB with one decimal place (German "8,4 MB", English "8.4 MB"). Every label and pattern is in every `i18n/*.toml`. When DateTimeOriginal is missing or unreadable, DateTime is used.
- [ ] AC-08 (event): When the shown image or the selected tile changes while the overlay is on, then the overlay shows only facts of the new one, never facts of the previous one: until its EXIF facts and pixel size are known, it shows the facts it already has (name, position, file size).
- [ ] AC-09 (state): While the grid is open and the overlay is on, the box shows the facts of the selected tile (with AC-02's lines), also for a tile whose image was never opened, and follows the selection. For such a tile the viewer reads what it needs from the file without decoding the pixels; where it cannot learn the full pixel size that way (e.g. a format only the system decoder reads), the size and format line shows the format alone. If the file cannot be read (locked, gone, a network drive that is away), the box shows name, position and, if known, file size.
- [ ] AC-10 (always): The box never covers the ☰ button, the window buttons or the filmstrip: it sits above the filmstrip when that is shown and stays inside the window below the row of the ☰ button and the window buttons. A notice is drawn over the box where both overlap. A line wider than 40 % of the window (at least 240 px at 100 % scale) is shortened: the name in the middle with "…" so the extension stays visible, every other line at the end. Lines never wrap.
- [ ] AC-11 (event): When the user turns the overlay on or off, then the choice is written to the settings file as `info = true` / `false` and holds after a restart; the default is off, and a value that is not `true` or `false` counts as off. If the file cannot be written, the overlay still turns on or off (`settings.md` AC-12).
- [ ] AC-12 (always): Reading the EXIF facts never delays a frame: the first frame of an image opened at startup and the next image while browsing are drawn without waiting for them, and a file whose facts are still being read never blocks keys or drawing. Proven by tests that hold the facts back and see the frame and the key handling go on.
- [ ] AC-13 (error): If the EXIF block is broken (offsets outside the block, loops, wrong types) or a value is out of range (a zero denominator, ISO 0, f-number 0 or above 1 000, focal length 0 or above 10 000 mm, exposure time 0 or above 3 600 s, a date before 1900 or after 2200), then the overlay leaves out exactly the affected facts and the viewer keeps running and shows the image.
- [ ] AC-14 (error): If the shown file is deleted, renamed or replaced, or the folder gains or loses files while the overlay is on, then the overlay follows what the viewer's folder list says (name and "{n} / {total}" update with it, `browsing.md`); facts of a replaced file are read again when the viewer reloads it.
- [ ] AC-15 (always, manual: measurement): With the overlay on, the next prefetched image still appears within 8 ms at p99 on Windows with a real GPU — the M2 budget of `blinzel.md` §12 — measured with `blinzel.exe --bench <dir> --json bench.json` (`navigation.latency.p99_ms`), run alternately with `info = true` and `info = false`.
- [ ] AC-16 (state, manual: visual design): While the overlay is on, the box looks like the viewer's other drawn surfaces (the shared amigo theme, `amigo-theme.md`) and its text stays readable over a white, a black and a busy photo.

## Technical notes

Hints, not requirements; the ACs decide.

- `batch::meta::read_exif` finds the TIFF block for JPEG, PNG, WebP, AVIF/HEIF and JPEG XL; a TIFF file (and so DNG, CR2, NEF, ARW, PEF) is itself the TIFF block. Both it and `date_taken` sit behind the `batch` feature today. Tags: Make 0x010F, Model 0x0110, DateTime 0x0132, DateTimeOriginal 0x9003, ExposureTime 0x829A, FNumber 0x829D, ISO 0x8827, FocalLength 0x920A, LensModel 0xA434.
- In the single view the loader already has the file's bytes; in the grid a tile comes from the thumbnail cache, so the facts of the selected tile need their own read of the file on a worker. EXIF is not always near the start (PNG `eXIf`, WebP `EXIF` and JPEG XL `Exif` may follow the image data, TIFF IFDs may sit at the end), so the read follows the container's structure (chunk and box headers, IFD offsets) instead of a fixed prefix; the probe that gives the pixel size already reads the header.
- The platform fallback (WIC, ImageIO) decodes from the file too; EXIF can still be read from the bytes by our parser, since it does not decode pixels.
- A new setting key `info` (`settings.md`): its AC-02 default list and AC-10 menu list grow by it when this spec is built.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| PNG screenshot without EXIF | Name, position, size and format, file size only |
| Image with orientation 6 (portrait shot), stored 6000 × 4000 | "4000 × 6000" (as displayed) |
| DateTimeOriginal missing, DateTime present | DateTime is shown |
| Date "0000:00:00 00:00:00" or not parseable | The date line is left out |
| ExposureTime 10/2500 | "1/250 s" |
| ExposureTime 0/0 | The time is left out of the exposure line, the rest stays |
| Make "NIKON CORPORATION", Model "NIKON Z 6" | "NIKON Z 6" (the model starts with the make's first word) |
| Name of 200 characters | Shortened in the middle, extension stays visible |
| Folder with one image | "1 / 1" |
| Image opened alone from a folder that is still being listed | The position line appears once the listing is done |
| Empty grid, or an image the folder list does not contain | No box in the empty grid; without a position the position line is left out |
| Grid selection moves quickly over many tiles | Only the last selected tile's facts appear (AC-08); reads for tiles passed over may be dropped |
| 2 GB TIFF | Only the file's first 64 MB are read for the facts, never all of it; a fact that lies beyond (IFDs written after the pixel data) is left out |
| Animated GIF | The facts of the file; the frame number is not shown, because it changes many times a second and says nothing about the file |
| Error image ("Could not open …") | Name, position and file size; size, format and EXIF are left out because nothing was decoded |
| Window too small for the box | Lines are shortened (AC-10); the box is cut at the window edge and never drawn over ☰, the window buttons or the filmstrip |

## Assumptions

| Assumption | Reason | Risk |
| ---------- | ------ | ---- |
| I is free and fits | No key uses I today; "I" for "info" is common in image viewers | A later feature wants I |
| 40 % of the window, at least 240 px, for a line | Leaves most of the image free while a typical camera name fits in a 1280 px window | Long lens names are shortened in small windows |
| The camera line drops the make when the model starts with the make's first word | Most cameras repeat the make in the model ("Canon EOS R6", "NIKON Z 6") | A make whose first word is part of an unrelated model name |
| The file size is shown in decimal units (1 MB = 1 000 000 bytes) | Matches Finder on macOS and most photo tools; Windows Explorer uses binary units | A Windows user compares with Explorer and sees a small difference |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `i_toggles_the_overlay_the_menu_follows_and_the_choice_is_kept`, `show_info_is_in_the_view_menu_next_to_filmstrip_and_grid` (`crates/blinzel-ui/src/app_info_tests.rs`) | built |  |
| AC-02 | `the_overlay_shows_every_fact_in_order`, `a_portrait_shot_shows_its_size_as_displayed` (`crates/blinzel-ui/src/app_info_tests.rs`); `raw_files_show_their_extension_and_others_their_format` (`crates/blinzel-ui/src/info.rs`) | built |  |
| AC-03 | `an_image_without_exif_shows_name_position_size_and_file_size` (`crates/blinzel-ui/src/app_info_tests.rs`); `out_of_range_values_are_left_out_and_the_rest_stays` (`crates/blinzel-core/src/exif_info.rs`) | built |  |
| AC-04 | `camera_drops_a_make_the_model_repeats`, `strings_lose_spaces_and_nuls_and_non_utf8_reads_as_latin1` (`crates/blinzel-core/src/exif_info.rs`) | built |  |
| AC-05 | `exposure_times_read_as_fractions_below_half_a_second`, `f_numbers_drop_a_zero_decimal_after_rounding` (`crates/blinzel-ui/src/info.rs`); `the_overlay_shows_every_fact_in_order` (`crates/blinzel-ui/src/app_info_tests.rs`) | built |  |
| AC-06 | `exif_comes_from_jpeg_png_webp_jxl_and_tiff`, `exif_comes_from_avif_and_heic`, `the_first_of_two_exif_segments_counts` (`crates/blinzel-core/src/exif_info.rs`) | built |  |
| AC-07 | `dates_and_numbers_follow_the_ui_language` (`crates/blinzel-ui/src/app_info_tests.rs`); `file_sizes_use_decimal_units` (`crates/blinzel-ui/src/info.rs`); `date_time_stands_in_for_a_missing_or_broken_date_taken` (`crates/blinzel-core/src/exif_info.rs`) | built |  |
| AC-08 | `going_on_never_shows_the_previous_images_facts_and_never_waits_for_them` (`crates/blinzel-ui/src/app_info_tests.rs`) | built |  |
| AC-09 | `in_the_grid_the_overlay_follows_the_selected_tile`, `in_the_grid_an_unreadable_tile_shows_what_is_known` (`crates/blinzel-ui/src/app_info_tests.rs`) | built |  |
| AC-10 | `the_box_sits_bottom_left_above_the_filmstrip_and_shortens_long_names`, `a_notice_is_drawn_after_the_box` (`crates/blinzel-ui/src/app_info_tests.rs`); `shorten_keeps_the_extension_in_the_middle_mode` (`crates/blinzel-ui/src/info.rs`) | built |  |
| AC-11 | `i_toggles_the_overlay_the_menu_follows_and_the_choice_is_kept` (`crates/blinzel-ui/src/app_info_tests.rs`); `info_is_off_unless_the_file_says_true` (`crates/blinzel-core/src/settings.rs`) | built |  |
| AC-12 | `the_first_frame_does_not_wait_for_the_facts`, `going_on_never_shows_the_previous_images_facts_and_never_waits_for_them` (`crates/blinzel-ui/src/app_info_tests.rs`) | built |  |
| AC-13 | `a_broken_exif_block_leaves_out_its_facts_and_the_image_shows` (`crates/blinzel-ui/src/app_info_tests.rs`); `a_broken_block_gives_no_facts_and_never_panics`, `out_of_range_values_are_left_out_and_the_rest_stays` (`crates/blinzel-core/src/exif_info.rs`) | built |  |
| AC-14 | `the_position_follows_the_folder_list`, `a_replaced_file_is_read_again_and_a_gone_one_loses_its_position`, `a_broken_image_shows_name_position_and_file_size_only` (`crates/blinzel-ui/src/app_info_tests.rs`) | built |  |
| AC-15 | manual: measurement | open |  |
| AC-16 | manual: visual design | open |  |

## Decisions

- 2026-11-21: Bottom left, the grid shows the selected tile, EXIF for every container plus TIFF-based RAW, formats per UI language — the user's answers in the spec dialogue. Clarification pass the same day: grid tiles read their facts from the file (they are not opened), ORF and RW2 are not TIFF for the viewer and get no EXIF facts, value ranges and number formats fixed.
- 2026-11-21 (spec-build): the facts come from one background reader in `blinzel-ui` (`info.rs`) that reads the file of the shown image or the selected tile, latest wins, rather than from the loader. Because the grid needs it for tiles that were never opened, and one path for both keeps AC-08 simple; the second read of the shown file comes from the OS cache and only happens while the overlay is on.
- 2026-11-21 (spec-build): the reader wakes the UI thread with the existing `Wake::Thumbs`, whose handler also polls it. Because a new `Wake` variant would change all three shells for one more poll.
- 2026-11-21 (spec-build): new public API in `blinzel-core`: `exif_info::{ExifInfo, Taken, exif_block}` and the test builders `testimg::{ExifValue, exif_block, jpeg_with_exif}`; `batch::meta::read_exif` now uses `exif_block` (a TIFF source still carries no block over). New `Cmd::ToggleInfo` (menu id 45), `MenuState::info`, `Settings::info`.
- 2026-11-21 (spec-build): an image that failed to open shows name, position and file size only, even when the reader got a format from its first bytes (edge case "Error image").
- 2026-11-21 (spec-build): the App-level tests were written before `Cmd::ToggleInfo`, the setting and the reader existed (red: the crate did not compile); the core tests ran red against stubs that returned no facts.
- 2026-11-21 (spec-build, round 2 after spec-check): a file bigger than 64 MB is read only up to 64 MB for its facts (`a_big_file_is_read_only_at_its_start`). Because the reader has only whole-file reads and contiguous byte offsets, and every format whose EXIF block sits after the pixel data is rare at that size (camera raw keeps its IFDs at the start); the edge case "2 GB TIFF" says so now instead of promising facts from the end of the file.
- 2026-11-21 (spec-build, round 2): a size that rounds to 1000.0 in a unit moves to the next unit ("1.0 MB", not "1000.0 KB").
