---
status: todo
created: 2026-11-02
size: M
platforms: all (Windows, macOS, Linux; the clipboard differs per platform)
design: [blinzel.md §2, blinzel.md §7, blinzel.md §12]
---

# Selection, copy, cut and save cutout

## Context

The viewer lets the user mark a rectangle on the image, copy it, crop the view to it, and save it as a new file. The design spec decides (`blinzel.md` §2): rectangle selection; Ctrl+C copies; Ctrl+X = copy + crop the view; image files are never modified, a cutout is saved only through "Save as" (Ctrl+S). The mouse is context-dependent: if the image fits, a left drag selects; if it is larger, a left drag pans and Shift+drag selects.

This spec was reverse-engineered from the code on 2026-11-02 (selection geometry and cutout export in `blinzel-core`, the interaction in `blinzel-ui`'s `App`, the clipboard and save dialogs in the three shells of `blinzel-app`). It records what the code does, not what the design spec says it should do.

## Scope

### In scope

- Drawing, moving and resizing the selection rectangle; Ctrl+A; clearing it (click, Esc)
- Ctrl+C / menu "Copy" and the clipboard formats on Windows, Linux and macOS
- Ctrl+X / menu "Cut (crop view to selection)", Esc / "Show original"
- Ctrl+S / menu "Save cutout as…": lossless JPEG crop, re-encoding to JPEG, PNG, WebP or AVIF, the notice afterwards

### Out of scope

- Multi-selection of thumbnails in the grid (Ctrl+click, Shift+click, Ctrl+A in the grid) — because it belongs to the thumbnails spec; while the grid is shown, Ctrl+A and Esc go to the grid
- Zoom, pan, view modes, rotation — because they belong to the viewing spec; this spec only says how rotation interacts with the selection
- Screenshots on the clipboard — because they belong to the screenshot feature (they are put on the clipboard at once, not lazily)
- Batch resizing — because it has its own spec (`batch-resize.md`)

## Acceptance criteria

- [ ] AC-01 (state): While the image fits the window, a left-button drag on the image starts a new selection; while the image is larger than the window, a left drag pans the image and a Shift+left drag starts a new selection.
- [ ] AC-02 (always): The selection is a rectangle in full-resolution pixels of the image as shown (after the EXIF orientation and the user's rotation): its edges snap to whole pixels, it is clamped to the visible content (the whole image, or the cropped region), and a rectangle smaller than 1 × 1 pixel is no selection.
- [ ] AC-03 (event): When the user drags inside an existing selection, it moves without changing size and stops at the edge of the content; when the user drags one of its eight handles, the edges of that handle follow the pointer, and pulling a handle across the opposite edge flips the rectangle. A selection less than two handle sizes wide or high on screen shows no handles (a press on it moves it), and corners take precedence over edges.
- [ ] AC-04 (state, manual: visual design): While a selection exists, the image outside it is dimmed, the rectangle has a white outline on a dark border with eight white handles, and a label `W × H` (image pixels) sits above its top-left corner, or inside it when there is no room above.
- [ ] AC-05 (event): When the user presses the left button on the image outside the selection and releases it after moving less than 3 logical pixels, the selection is cleared and no new one is made.
- [ ] AC-06 (event): When the user presses Ctrl+A or chooses "Select all", the selection becomes the whole visible content: the full image, or the cropped region while the view is cropped.
- [ ] AC-07 (event): When the user presses Esc, the viewer does the first of these that applies: ends a drag in progress, clears the selection, shows the original after a cut, leaves full screen, closes the window.
- [ ] AC-08 (event): When the user presses Ctrl+C or chooses "Copy", the viewer puts on the clipboard the selection — without a selection the cropped region, without a crop the whole image — at full resolution and in the orientation shown, decoded from the file (not from the screen).
- [ ] AC-09 (event, CI: windows): When the user copies on Windows, the clipboard offers `CF_DIBV5` (32-bit with alpha, sRGB) and `PNG` without any pixels; the region is decoded only when an application pastes, and if the viewer still owns the clipboard when it exits, both formats are rendered first so a later paste still works.
- [ ] AC-10 (event): When the user copies on Linux, the region is decoded on a background thread right after the copy and set as an image on the system clipboard (offered as `image/png`).
- [ ] AC-11 (event, CI: macos): When the user copies on macOS, the region is decoded on a background thread right after the copy, encoded as PNG and placed on the general pasteboard as `public.png`.
- [ ] AC-12 (event): When the user presses Ctrl+X or chooses "Cut" with a selection, the viewer copies it (as AC-08), then shows only that region — zoom and pan reset, the selection cleared, the window fitted as for a new image; without a selection Ctrl+X does nothing and "Cut" is disabled in the menu.
- [ ] AC-13 (event): When the user presses Esc or chooses "Show original" while the view is cropped, the whole image is shown again; switching to another image or rotating the view also drops the crop and the selection. "Show original" is disabled while the view is not cropped.
- [ ] AC-14 (event): When the user presses Ctrl+S or chooses "Save cutout as…" with a selection or a cropped view, the viewer asks for a target file in the source's folder, proposing `{source name}_{suffix}.jpg` for a JPEG source and `{source name}_{suffix}.png` for any other (suffix translated, English `cutout`); the selection wins over the crop. Without a selection or crop nothing happens and the menu entry is disabled.
- [ ] AC-15 (event, manual: system dialog): When the save dialog opens on Windows or Linux, it offers the file types JPEG, PNG, WebP and AVIF (on Windows JPEG or PNG preselected as in AC-14); on macOS it offers no type choice (the typed extension decides). On every platform it asks before replacing an existing file.
- [ ] AC-16 (always): The saved format follows the extension of the chosen name, ignoring case: `.png` → PNG, `.webp` → WebP, `.avif` → AVIF, anything else → JPEG.
- [ ] AC-17 (event): When a JPEG source is saved as JPEG, the cutout's left and top edges lie on the source's MCU grid, the user has not rotated the view, and the source has 8-bit precision, then the cutout is written without re-encoding (the DCT coefficients are copied), keeps the source's EXIF block including its orientation, has no MPF segment and no link to the stale EXIF thumbnail.
- [ ] AC-18 (event): When a cutout cannot be saved losslessly (other source or target format, MCU-unaligned, view rotated), the region is decoded at full resolution, turned into the orientation shown (a 90° rotation swaps width and height in the file) and encoded.
- [ ] AC-19 (event): When a re-encoded cutout is written, JPEG uses quality 95, the chroma subsampling of a JPEG source (4:2:0 for other sources) and a JPEG source's ICC profile; PNG is 8-bit RGB when every pixel is opaque, else RGBA, without ICC profile; WebP (lossy, quality 90) and AVIF (quality 85) embed the source's ICC profile. No re-encoded cutout carries EXIF.
- [ ] AC-20 (event): When saving finishes, a notice shows `Saved “{name}” losslessly` (no re-encoding) or `Saved “{name}”`; when reading, decoding, encoding or writing fails, the notice names the file and the reason. The save runs off the UI thread.
- [ ] AC-21 (always): Selecting, copying and cutting never write a file; saving a cutout writes only the target chosen in the save dialog, first to a temporary file next to it (`{target stem}.blinzel-tmp`), then renamed onto the target.
- [ ] AC-22 (error): If the target chosen in the save dialog is the image the cutout comes from (the same file, also under another spelling of its path), then nothing is written and a notice says that the source cannot be replaced; any other existing target is replaced after the dialog's overwrite confirmation.

## Technical notes

- Selection geometry: `blinzel_core::selection` (`left_drag_action`, `Drag::begin`/`update`, `hit_test`, `handle_rects`, `to_stored`, `orient_bgra`). The app keeps `selection: Option<Rect>` in oriented full-resolution coordinates and the cut as `ViewState::crop: Option<PixelRect>` (pure view state, `blinzel_core::layout`). Handle size on screen: 7 logical px; a drag becomes a selection after more than 3 logical px.
- Copy: `App::copy` stores a `ClipSource { path, stored_rect, orientation }` (region in stored file orientation, orientation = EXIF then user rotation) when `Host::offer_clipboard()` returns true. Windows: `blinzel-app/src/clipboard.rs` (`announce` with NULL handles = delayed rendering, `render` on `WM_RENDERFORMAT`, `render_all` on `WM_RENDERALLFORMATS` and on close; decode via `wic::decode_any`, region request). Linux: `linux/shell.rs` `after_event` → thread `blinzel-copy` → `arboard::Clipboard::set_image`. macOS: `macos/shell.rs` → PNG via the batch PNG encoder → `NSPasteboard` `NSPasteboardTypePNG`.
- Save: `App::save_job` → `Action::SaveCutout(SaveJob)`; the shells show `IFileSaveDialog` (Windows, `dialogs.rs`), `rfd::FileDialog` (Linux), `NSSavePanel` (macOS, no type filter set), then call `blinzel_ui::app::save_cutout` on a thread `blinzel-save`, which calls `blinzel_core::export::export`.
- Lossless crop: `decode::jpeg::lossless_crop` (`tj3Transform` with `TJXOPT_CROP`), then `export::clean_markers` (drops `MPF\0` APP2, zeroes IFD0's next-IFD offset). Needs the `turbojpeg` feature; WebP/AVIF cutouts need the `batch` feature (the batch encoders).
- Menu entries (`blinzel-ui/src/menu.rs`): Copy and Select all enabled with an image; Cut with a selection; Show original while cropped; Save cutout with a selection or crop.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Image still loading (no header yet) when Ctrl+C or Ctrl+S is pressed | Nothing happens |
| Decoding the region for the clipboard fails | The clipboard gets no image; the error is only logged |
| Windows: another program holds the clipboard when Ctrl+C is pressed | `offer_clipboard` fails, the previous copy is kept as it was |
| Selection dragged out of the image | Clamped to the image edge; moving stops at the edge without shrinking |
| EXIF-rotated JPEG saved as JPEG without user rotation, aligned | Lossless; the file keeps its EXIF orientation, so it shows as the viewer showed it |
| User-rotated view saved as JPEG | Re-encoded upright (AC-18), never lossless |
| Target chosen is an existing file other than the source | The system dialog asks; on confirmation the file is replaced by rename |
| Target chosen is the source itself, under any spelling of its path | The system dialog asks; then nothing is written and a notice says the source is not replaced (AC-22) |
| A file named `{target stem}.blinzel-tmp` exists next to the target | It is overwritten by the temporary file and then renamed away |
| Ctrl+X on a crop | Crops again within the cropped region (the new crop replaces the old) |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `drag_modes_depend_on_overflow_and_shift` (`blinzel-core/src/selection.rs`, Linux) | built | |
| AC-02 | `new_drag_snaps_and_clamps` (`blinzel-core/src/selection.rs`, Linux) | built | |
| AC-03 | `move_and_resize`, `hit_testing` (`blinzel-core/src/selection.rs`, Linux) | built | |
| AC-04 | manual | open | |
| AC-05 | `a_short_click_outside_the_selection_clears_it`, `a_short_click_clears_the_selection_while_left_drag_pans` (`blinzel-ui/src/app_selection_tests.rs`, Linux) | built | |
| AC-06 | `select_all_takes_the_whole_image_or_the_cropped_region` (`blinzel-ui/src/app_selection_tests.rs`, Linux) | built | |
| AC-07 | `escape_ends_a_drag_then_clears_then_uncrops_then_leaves_full_screen_then_closes` (`blinzel-ui/src/app_selection_tests.rs`, Linux) | built | |
| AC-08 | `copy_announces_the_selection_else_the_crop_else_the_whole_image`, `copy_maps_a_rotated_view_back_to_the_file_at_full_resolution`, `clip_pixels_are_the_files_region_in_the_orientation_shown` (`blinzel-ui/src/app_selection_tests.rs`, Linux); `stored_mapping_roundtrips_all_orientations` (`blinzel-core/src/selection.rs`) | built | |
| AC-09 | | open | |
| AC-10 | `a_copy_is_decoded_right_away_on_its_own_thread` (`blinzel-ui/src/app_selection_tests.rs`, Linux); `a_copy_is_offered_to_other_programs_as_png` (`blinzel-app/src/linux/clipboard.rs`, Linux CI step on Xvfb with `BLINZEL_X11_TEST`) | built | |
| AC-11 | | open | |
| AC-12 | `cut_crops_the_view_to_the_selection_and_needs_one` (`blinzel-ui/src/app_selection_tests.rs`, Linux); `crop_restricts_content` (`blinzel-core/src/layout.rs`) | built | |
| AC-13 | `show_original_navigation_and_rotation_drop_the_crop` (`blinzel-ui/src/app_selection_tests.rs`, Linux) | built | |
| AC-14 | `save_asks_with_a_name_by_source_type_and_prefers_the_selection`, `nothing_happens_before_the_header_is_known` (`blinzel-ui/src/app_selection_tests.rs`, Linux) | built | |
| AC-15 | manual | open | |
| AC-16 | `png_output_and_format_from_path`, `webp_and_avif_cutouts` (`blinzel-core/src/export.rs`, Linux) | built | |
| AC-17 | `aligned_jpeg_crop_is_lossless`, `lossless_crop_keeps_exif_but_drops_mpf_and_thumbnail_link` (`blinzel-core/src/export.rs`, Linux) | built | |
| AC-18 | `unaligned_or_rotated_crops_are_reencoded`, `webp_and_avif_cutouts` (`blinzel-core/src/export.rs`, Linux) | built | |
| AC-19 | `reencoded_jpeg_uses_quality_95_the_source_subsampling_and_icc`, `png_cutouts_are_rgb_when_opaque_else_rgba_without_icc_or_exif`, `webp_and_avif_cutouts_embed_the_icc_profile_but_no_exif` (`blinzel-core/src/export.rs`, Linux) | built | |
| AC-20 | `saving_tells_how_it_went_and_names_the_file`, `saving_runs_on_its_own_thread_and_reports_there` (`blinzel-ui/src/app_selection_tests.rs`, Linux) | built | |
| AC-21 | `only_the_chosen_target_is_written_through_a_temporary_file` (`blinzel-ui/src/app_selection_tests.rs`, Linux) | built | |
| AC-22 | `the_source_is_never_replaced_other_files_are` (`blinzel-ui/src/app_selection_tests.rs`, Linux) | built | |

## Decisions

- 2026-11-02: "Save cutout as" never replaces its own source; other existing files may be replaced after the system dialog's confirmation (AC-22). Because the viewer never modifies the image it shows (`blinzel.md` §2, AGENTS.md §9), while numbering an existing name would break the save dialog people know.
- 2026-11-02 (spec-build): the AC-05 test went red for a larger image: a left click there started a pan, and releasing it kept the selection. A left-button pan now remembers its start until the pointer moves more than 3 logical px, and a release before that clears the selection like a click on a fitting image (a middle-button pan never does). Because the AC says "on the image", not "on an image that fits".
- 2026-11-02 (spec-build): the App-level tests of AC-06, AC-07, AC-08 (the clip's region), AC-12, AC-13 and AC-14 were green at once: the behavior was extracted from the code. The cut tests do not apply the `FitWindow` a cut returns, because the test harness has no minimum window size and the window would shrink under the menu button; they check that the action is returned.
- 2026-11-02 (spec-build): decoding a copied region moved into `blinzel-ui` as `ClipSource::pixels` (read the file, decode the region at full resolution, orient it), which the Linux and macOS shells call on their `blinzel-copy` thread. Windows keeps decoding in `clipboard.rs` through `wic::decode_any` (the native decoders plus WIC). Because the behavior belongs in the shared crate (AGENTS.md §5.4) and a test there proves AC-08 for both shells.
- 2026-11-02 (spec-build): the AC-19 tests were green at once (the behavior existed). JPEG quality 95 is proven by the quantisation tables, which equal those of a quality-95 encode; the WebP and AVIF settings are the new constant `blinzel_core::export::CUTOUT_QUALITY` (public, `batch` feature), checked by value next to a check that the WebP is lossy (`VP8 `, not `VP8L`). Because the encoded bitstreams do not carry their quality setting.
- 2026-11-02 (spec-build): saving runs through the new `blinzel_ui::app::save_in_background` (a `blinzel-save` thread that hands the notice text to a callback), which all three shells call instead of spawning the thread themselves. Because "off the UI thread" (AC-20) is then shared behavior a test can prove; the Windows and macOS shells only pass the callback that posts the notice.
- 2026-11-02 (spec-build): AC-22 is checked in `save_cutout` before anything is read or written: on Unix the source and the target are the same file when device and inode match (this covers `.`/`..`, symbolic links, hard links and letter case on a case-insensitive APFS volume); on Windows when `std::fs::canonicalize` resolves both to the same final path (it normalises case, short names and links). The notice is the new key `error_save_source` in every `i18n/*.toml`. The test proves `.`, `..` and a symbolic link on Linux; the letter-case spelling needs a case-insensitive volume (Windows, macOS) and is covered by the same check there. The edge-case row that still said the source is replaced after confirmation was split to match AC-22.
- 2026-11-02 (spec-build): the edge case "image still loading (no header yet)" is tested with a file whose header cannot be read: both leave the App without a probe for the current image, which is what Ctrl+C and Ctrl+S check.
- 2026-11-02 (spec-build): copying on Linux and macOS runs through the new `blinzel_ui::app::copy_in_background` (a `blinzel-copy` thread that decodes the clip at once and hands the pixels to a callback); the Linux shell's arboard call moved into `linux/clipboard.rs` (`put_image`, `rgba`). Because the threading is then shared behavior a headless test proves, like `save_in_background`. The clipboard itself needs a display: `a_copy_is_offered_to_other_programs_as_png` runs only with `BLINZEL_X11_TEST` (the Linux CI job's Xvfb step, now `--test-threads=1` since it sets `DISPLAY`), where a second X client asks for `image/png` and decodes the copied pixels; locally it is a no-op without that variable. Not covered by a test: the Wayland path (arboard's data-control protocol; CI has no compositor) and the shell's two lines that call `copy_in_background` after the event in which `offer_clipboard` was called (they need a winit event loop). Both tests were red first: the helper did not exist, and `put_image` stubbed to `Ok(())` left the selection without an owner.
- 2026-11-02: extracted from the code (reverse engineering); the design spec's 🟡 markers for §7 were not taken as evidence.
