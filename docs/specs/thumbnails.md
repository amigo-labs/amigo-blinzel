---
status: todo
created: 2026-11-02
size: L
platforms: all
design: [blinzel.md §3, blinzel.md §6, blinzel.md §7, blinzel.md §12]
---

# Thumbnails: generation, cache, atlas, filmstrip and grid

## Context

amigo-blinzel is meant to be measurably faster than other viewers at thumbnails (`blinzel.md` §1). The decision is a filmstrip plus a grid browser, both off by default (`blinzel.md` §2). The benchmark gate (`blinzel.md` §3) chose in-house builds over the Windows thumbnail provider and the Windows thumbnail cache: our own reduction from the smallest sufficient decode, our own pack cache, and a GPU atlas in YCbCr 4:2:0. This spec records what the code does today (reverse engineered); the M4 targets come from `blinzel.md` §12.

## Scope

### In scope

- Making a thumbnail from an image file: size, source choice (embedded preview or the image), reduction, orientation
- The on-disk thumbnail cache shared across sessions and instances
- The GPU thumbnail atlas and drawing thumbnails into tiles
- The thumbnail pool: request order, failures, cloud placeholders
- The filmstrip (T, setting) and the grid (G): layout, keys, mouse
- Marking several images in the grid (Ctrl+click, Shift+click, Ctrl+A, Esc)
- The M4 speed targets and how they are measured

### Out of scope

- Browsing and prefetch of the full image (Next/Prev, budgets) — because they are specified with browsing/prefetch
- The batch dialog and its file list — because it is specified with batch processing; this spec only covers the marked set the grid keeps
- View modes, zoom and full screen of the single image — because they are specified with the view modes
- Window placement, window chrome and the platform shells' event loops — because they are specified with the platform shells
- File names under the tiles and a grid scenario in `blinzel-race` — because they are not built

## Acceptance criteria

### Generation

- [ ] AC-01 (always): A thumbnail is the image's full size shrunk to fit 256 × 256 px with the aspect ratio kept (rounded), never enlarged, and at least 1 px per side (6000 × 4000 → 256 × 171, 100 × 50 stays 100 × 50, 0 × 0 → 1 × 1).
- [ ] AC-02 (always): The thumbnail of a JPEG is within 30 dB PSNR of an exact area-average reduction of its full-resolution decode to the same size.
- [ ] AC-03 (always): Downscaling averages exactly over area: each output pixel is the area-weighted mean of the source pixels it covers (3 → 2 px: a source pixel shared half and half), a flat colour stays flat at any ratio, and fully transparent pixels contribute no colour to their neighbours.
- [ ] AC-04 (event): When an image file carries embedded previews (EXIF/MPF) and at least one covers the thumbnail size with an aspect ratio within 2 % of the image's, then the thumbnail is made from the smallest such preview; a preview that is smaller than the thumbnail or has another aspect ratio (bars) is not used.
- [ ] AC-05 (error): If the chosen embedded preview fails to decode, then the thumbnail is made from the image itself.

### Cache

- [ ] AC-06 (event): When a thumbnail has been stored for a file, then a lookup with the same path (case-insensitive), size and modification time returns it with its orientation and full image size, also after the cache is reopened (next session); an opaque thumbnail comes back within 30 dB PSNR, one with transparency pixel-identical; a lookup with another size or modification time misses; a newer entry for the same path replaces the older one, also after reopening.
- [ ] AC-07 (event): When a requested thumbnail is missing from the cache or stale, then the viewer makes it from the file, shows it and stores it, so that the next session takes it from the cache.
- [ ] AC-08 (always): The cache's pack file never grows beyond the cache limit (1 GiB in the viewer): an append that would pass it empties the cache first (new generation), so the latest entries are present, the earliest are gone, and a reopened cache holds the same number of entries.
- [ ] AC-09 (error): If an index record is torn or corrupt (wrong checksum, short), then it is ignored on load and the other records are used.
- [ ] AC-10 (state): While several viewer instances use the same cache, their appends do not interleave; an entry another instance appended is seen after the next open, and an index entry that points into a generation another instance has since started reads as a miss, never as a wrong thumbnail.

### Atlas and drawing

- [ ] AC-11 (always): Transparent areas of a thumbnail are shown in the viewer's background colour.
- [ ] AC-12 (state): While every atlas slot is taken and no further page may be added (64 thumbnails per page, at most 8 pages), a new thumbnail takes the slot of the thumbnail drawn least recently, and that one is requested again when it is needed.
- [ ] AC-13 (always, CI: windows): A thumbnail is drawn with its EXIF orientation applied (EXIF 6: stored top-left goes to the top right), scaled into its destination rectangle, without touching pixels outside it and without colours from the neighbouring atlas slot.

### Requests

- [ ] AC-14 (event): When the grid is drawn, then the thumbnails of the visible tiles are requested first in list order, then the next screenful, then the previous screenful nearest first; when the filmstrip is drawn, its visible tiles are requested nearest to the current image first, then further out alternating ahead and behind up to three times half the visible count. A request already handed out is not repeated until its atlas slot is reused.
- [ ] AC-15 (error): If a thumbnail cannot be made (unreadable or undecodable file), then its tile keeps the placeholder and the thumbnail is not requested again in this session; a cloud placeholder file (offline, recall on open or on data access) is never read for a thumbnail and keeps the placeholder.

### Filmstrip

- [ ] AC-16 (option): Where the filmstrip setting is on (toggled by T or "View → Filmstrip", saved in the settings file, off by default), a strip along the bottom shows the current image's neighbours with the current tile centred and outlined, the image is laid out in the area above it, and in "fit window to image" the window grows by the strip's height; a click on another tile opens that image, a click on the current tile and a double-click on the strip do nothing, the wheel over the strip goes to the previous image (wheel up) or the next (wheel down); while the grid is open, the strip is not shown.

### Grid

- [ ] AC-17 (always): The grid shows square tiles in as many columns as fit the window width (at least one), centred horizontally; a click on the gap between tiles, on the margin or past the last tile hits no tile, and at a scroll offset the same point hits the tile that many rows further down.
- [ ] AC-18 (state): While the grid is open (G or "View → All images in the folder (grid)"; it opens over the whole window with the current image selected and in view): arrows move the selection by one column or row, Page Up/Page Down by the number of full rows that fit, Home/End to the first/last image, always clamped to the images and scrolled just enough to show the selection fully; the wheel scrolls one row per notch; a click selects; a double-click, Enter or Space opens that image and closes the grid; G, or Esc with nothing marked, closes the grid.
- [ ] AC-19 (state): While the grid is open, Ctrl+click toggles the mark of a tile, Shift+click adds every image from the selected one to the clicked one to the marks, a plain click clears all marks, Ctrl+A marks every image of the folder, and Esc with marks clears them and keeps the grid open; marked tiles carry a check mark.
- [ ] AC-20 (state, manual: visual design): While the grid or the filmstrip is shown, tiles look right on Windows, macOS and Linux at 100 % and 200 % scaling: placeholder, hover highlight, accent outline on the selected/current tile, check mark badge on marked tiles, thumbnails sharp and correctly oriented.

### Speed (M4)

- [ ] AC-21 (always, manual: measurement): 200 thumbnails from the cache are on screen (open the cache, read and decode 200, load them into the atlas, draw the grid, GPU done) in at most 30 ms median, measured with `blinzel --bench <dir> --json out.json`, field `thumb_grid_200` (Windows only; the CI step "App benchmark (offscreen, WARP)" runs it on WARP and fails only on a cache miss, not on the time).
- [ ] AC-22 (always, manual: measurement): Over 200 different 12-MP JPEGs with sensor noise, cold thumbnails (read, probe, decode, reduce; no cache) reach at least 150 per second and warm reading of all 200 from a reopened cache takes at most 30 ms, measured with `blinzel-bench thumbs <dir> [--count N] [--threads T]` on the machine named with the result (not run in CI).
- [ ] AC-23 (event): When the user opens "Resize images…" (Ctrl+B or the menu) while tiles are marked in the grid, then the batch dialog starts with exactly the marked files, in the folder's order; marked files that no longer exist are left out. Without marks it starts empty.

## Technical notes

- **Generation** (`blinzel_core::thumb`): `EDGE = 256`; `fit(full, edge)`; `make(data, probe, edge, decode)` asks the decoder for `Request::Fit { w, h }` (JPEG uses DCT scaling down to 1/8). `preview_for` filters `probe.previews` by `width >= target.0 && height >= target.1` and `|(pw/ph)/aspect − 1| < 0.02`, picks the smallest area. `to_bgra` reduces YCbCr per plane (`reduce_plane`) and converts once at target size; packed images go through `reduce` (premultiplied, separable box filter with fractional weights from `weights`). Output `Thumb { width, height, bgra, orientation, full }`, BGRA in stored (not oriented) layout; orientation is only a tag. The app's decode function falls back to the system codecs (WIC, ImageIO); the pool decodes with one thread per image.
- **Cache** (`blinzel_core::thumbcache`): directory `<cache>/thumbs` with `<cache>` = `%LOCALAPPDATA%\amigo-blinzel`, `~/Library/Caches/amigo-blinzel` or `$XDG_CACHE_HOME/amigo-blinzel` (`blinzel_ui::dirs::cache`). `thumbs.pack`: blobs with a 16-byte header (`BLZT`, length, path hash) followed by JPEG quality 85 4:2:0 (opaque, with feature `turbojpeg`) or PNG (alpha, or without turbojpeg). `thumbs.idx`: 64-byte records (path hash, size, mtime, offset, length, width, height, full size, EXIF orientation, format, FNV checksum), loaded into a `HashMap` by path hash on `open`. `Key::new` hashes the lowercased path (FNV-1a). Appends take an in-process mutex plus an exclusive OS file lock on `thumbs.idx`. Limit `CACHE_LIMIT = 1 << 30` (`blinzel-ui/src/thumbs.rs`). Opened lazily by the first worker that needs it; an open error is logged and thumbnails are then made without a cache.
- **Pool** (`blinzel-ui/src/thumbs.rs`): `Thumbs::set_wanted` replaces the queue in priority order, skipping ids in `taken`; `forget(id)` after eviction, `reset()` when resident mode hides the window. Worker count from `Services::thumb_threads()` (Windows: number of performance cores, at least 2; Linux/macOS: half the logical CPUs, at least 2), normal priority. Results go back as `Wake::Thumbs` → `App::on_thumbs`.
- **Atlas** (`blinzel_ui::gfx::Atlas`, `AtlasPages`): `PAGE = 2048`, 8 × 8 slots of 256² per page; Y in a 2048² R8 texture, Cb/Cr in a 1024² R8G8 texture (6 MiB per page); `ATLAS_PAGES = 8`. LRU by `last_drawn` frame (`begin_frame`, `get`). Cache JPEGs (4:2:0) become `Planes` without an RGB round trip (`Planes::from_image`); everything else via `Planes::from_thumb` (alpha over the background). One draw per tile (`draws::thumb`). Shells: `blinzel-app/src/gfx/atlas.rs` (D3D11), `src/linux/gpu.rs` (wgpu), `src/macos/gpu.rs` (Metal).
- **Layout** (`blinzel_core::grid`, `blinzel-ui/src/browse.rs`): grid cell 176, gap 12; strip cell 72, gap 6, strip height `(72 + 2·6) × scale`; all logical px × DPI scale. `Grid::{visible, reveal, page_rows, step, hit}`, `strip(count, current, width, cell, gap)`, `fit_in` (centred, never enlarged).
- **Marks**: `GridState::marked: BTreeSet<String>` of file names; they live only while the grid is open (closing or reopening the grid starts with none).
- **Strings**: `view_filmstrip`, `view_grid` in every `i18n/*.toml`.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| EXIF thumbnail 160 × 120 with bars on a 3:2 photo | Not used (aspect ratio differs); the image is decoded with DCT scaling |
| Image smaller than 256 px | Thumbnail at its own size, not enlarged; drawn centred in the tile without enlarging |
| Extreme panorama 100 000 × 10 | Thumbnail 256 × 1 |
| File edited after its thumbnail was cached | Size or mtime differs → miss → new thumbnail made and appended; the newer entry wins |
| Linux/macOS: `A.jpg` and `a.jpg` in one folder with equal size and mtime | Same cache key (the path is lowercased on every platform): both show the same cached thumbnail |
| Two copies of a file in different folders with equal name, size and mtime | Same atlas id (name + size + mtime): the atlas thumbnail of one is reused for the other |
| Torn last record after a crash during an append | Ignored on load; the thumbnail is made again |
| Cache directory not writable | Error logged once; thumbnails are made without cache |
| Cloud placeholder (OneDrive "online only") | Not read (no download started); tile stays a placeholder |
| Broken file in the folder | Tile stays a placeholder; the viewer keeps running |
| Grid on an empty folder | No tiles; Enter or Space closes the grid without opening anything, a double-click does nothing |
| Delete, Ctrl+C, + or R while the grid is open | Delete (and "Open with", E) acts on the selected tile, not on the image that was open before the grid; Ctrl+C, Ctrl+X, Ctrl+S, zoom (+, −, 0) and rotation (R, L) do nothing while the grid covers the image |
| T while the grid is open | Toggles and saves the setting; the strip appears once the grid closes |
| Filmstrip with no current image (empty start) | Strip background only, no tiles |
| Resident mode hides the window | Queue, handed-out requests and atlas are dropped; the cache on disk stays |
| Batch dialog opened while tiles are marked | The dialog starts with the marked files (AC-23); files dropped onto it are added to them |

## Assumptions

| Assumption | Reason | Risk |
| ---------- | ------ | ---- |
| AC-13 holds for wgpu and Metal as well | Only the Direct3D 11 atlas has a render test (WARP); the shaders are ports of each other (`blinzel.md` §5 golden tests) | Orientation or slot bleeding wrong on Linux/macOS goes unnoticed |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `fits_the_tile` (`crates/blinzel-core/src/thumb.rs`; Linux, macOS, Windows) | built | |
| AC-02 | `thumbnails_a_jpeg_with_dct_scaling` (`crates/blinzel-core/src/thumb.rs`, feature `turbojpeg`, on by default; Linux, macOS, Windows) | built | |
| AC-03 | `area_average_is_exact`, `transparent_pixels_do_not_bleed` (`crates/blinzel-core/src/thumb.rs`; Linux, macOS, Windows) | built | |
| AC-04 | `takes_the_smallest_preview_that_covers_the_thumbnail`, `without_a_fitting_preview_the_image_itself_is_decoded` (`crates/blinzel-core/src/thumb.rs`; Linux, macOS, Windows) | built | |
| AC-05 | `a_broken_preview_falls_back_to_the_image` (`crates/blinzel-core/src/thumb.rs`; Linux, macOS, Windows) | built | |
| AC-06 | `stores_and_reloads`, `alpha_survives_and_newer_entries_win` (`crates/blinzel-core/src/thumbcache.rs`; Linux, macOS, Windows) | built | |
| AC-07 | `thumbnails_are_made_stored_and_taken_from_the_cache_next_session` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`; Linux, macOS, Windows) | built | |
| AC-08 | `starts_a_new_generation_at_the_limit` (`crates/blinzel-core/src/thumbcache.rs`, with a 40 000-byte limit; the 1 GiB constant is untested) | built | |
| AC-09 | `ignores_torn_records` (`crates/blinzel-core/src/thumbcache.rs`) | built | |
| AC-10 | `instances_share_the_cache_without_interleaving`, `an_entry_from_an_old_generation_reads_as_a_miss` (`crates/blinzel-core/src/thumbcache.rs`, two cache instances in one process, each with its own file handles; Linux, macOS, Windows) | built | |
| AC-11 | `planes_round_trip` (`crates/blinzel-core/src/thumb.rs`, last block: transparent over white → Y 255, Cb/Cr 128) | built | |
| AC-12 | `slots_fill_pages_then_reuse_the_least_recently_drawn` (`crates/blinzel-ui/src/gfx.rs`), `an_evicted_thumbnail_is_asked_for_again_when_it_is_needed` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`, 600 tiles); Linux, macOS, Windows | built | |
| AC-13 | `draws_oriented_and_scaled_without_bleeding` (`crates/blinzel-app/tests/atlas.rs`, `#![cfg(windows)]`, Windows CI `cargo test --workspace --release`) | built | |
| AC-14 | `the_grid_asks_for_the_visible_tiles_then_the_next_then_the_previous_screenful`, `the_filmstrip_asks_nearest_first_then_alternating_further_out` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`; Linux, macOS, Windows), one pool thread so requests are served in their order | built | |
| AC-15 | `a_broken_file_keeps_its_placeholder_and_is_not_asked_for_again`, `cloud_placeholders_are_never_read_for_a_thumbnail` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`; Linux, macOS, Windows) | built | |
| AC-16 | `filmstrip_toggles_saves_and_lays_out_the_image_above_it`, `filmstrip_centres_the_current_tile_and_opens_clicked_ones` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`; Linux, macOS, Windows) | built | |
| AC-17 | `columns_rows_and_cells`, `hit_testing` (`crates/blinzel-core/src/grid.rs`) | built | |
| AC-18 | `grid_keys_move_the_selection_clamped_and_scrolled_just_enough`, `grid_wheel_scrolls_a_row_per_notch_and_clicks_select`, `grid_opens_with_enter_space_or_a_double_click_and_closes_with_g_or_escape`, `an_empty_grid_opens_nothing` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`; Linux, macOS, Windows) | built | |
| AC-19 | `grid_marks_with_ctrl_shift_and_ctrl_a_and_escape_clears_them` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`; Linux, macOS, Windows) | built | |
| AC-20 | | open | |
| AC-21 | | open | |
| AC-22 | | open | |
| AC-23 | `resize_images_starts_with_the_marked_files_in_folder_order`, `resize_images_outside_the_grid_starts_empty` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`; Linux, macOS, Windows) | built | |

Supporting tests: the persistence of the filmstrip setting by `roundtrip` (`crates/blinzel-core/src/settings.rs`); the layout functions behind the grid keys by `keyboard_steps_clamp`, `visibility_and_scrolling`, the strip layout by `fitting_and_strip` (`crates/blinzel-core/src/grid.rs`).

## Decisions

- 2026-11-02: in the grid, file commands (Delete, "Open with", "Edit with") take the selected tile and commands on the shown image (copy, cut, save cutout, select all from the menu, zoom, rotation, show original) do nothing. Because the grid covers that image, and Delete recycling a file other than the one the user sees selected was the defect found in the extraction; a copy of a tile would need its probe on the UI thread.
- 2026-11-02: the tests of AC-04, AC-05 and AC-10 were green at once: preview choice, fallback and the shared cache already behaved as specified. AC-10's "instances" are two `ThumbCache`s in one test process: each opens its own handles, so the OS file lock excludes them as it does two processes.
- 2026-11-02: the App-level tests of AC-07, AC-12 (re-request after eviction), AC-14, AC-15, AC-16, AC-18 and AC-19 were green at once: thumbnail requests, failures, the cache, the filmstrip, the grid keys and mouse and the marks already behaved as specified; the tests guard that behaviour, none of it changed. Three of them were checked to go red with the behaviour broken on purpose (the previous screenful not reversed, placeholders read, an evicted id never forgotten).
- 2026-11-02: the grid's marks fill the batch dialog (AC-23, together with `batch-resize.md` AC-22). Because marking is how a person picks files in the viewer, and on macOS it is the only way to fill the batch list until drops reach the dialog there.
- 2026-11-02: AC-23 checks "no longer exists" with a file check per marked file when the dialog opens (on the UI thread). Because opening the dialog is not on the startup or browsing path, and the dialog's header inspection would otherwise list the missing file as unreadable instead of leaving it out.
