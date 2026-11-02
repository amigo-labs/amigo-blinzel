---
status: todo
created: 2026-11-02
size: M
platforms: all
design: [blinzel.md §2, blinzel.md §5, blinzel.md §6]
---

# Viewing an image

## Context

The viewer shows one image in a frameless window without toolbars: a ☰ button and the window buttons are drawn over the image, everything else lives in the menu (`blinzel.md` §2, "UI"). Three view modes decide how image and window relate (§2, "View modes"); zoom, pan and a view-only rotation change what is shown without touching the file (§2, "Saving"; AGENTS.md §9). The left mouse button depends on context: when the image fits, a drag selects; when it is larger than the window, a drag pans and Shift+drag selects (§2, "Mouse").

This spec records what the code does today (reverse engineered on 2026-11-02). The behavior lives in the platform-independent `App` state machine (`blinzel-ui`), the layout math (`blinzel-core::layout`), the menu model and the chrome geometry; the shells only execute the actions `App` returns.

## Scope

### In scope

- The view modes 1/2/3, the no-upscaling option and their persistence
- Zoom (wheel, +, −, 0), pan (left drag, middle drag, arrow keys)
- Rotation in the view (R, L, Shift+R)
- Full screen (F11, Enter, double-click) and the Esc chain
- The window following the image in "fit window to image" and growing in "fit image to window"
- The empty-start hint and the open-error message
- Opening via Ctrl+O and drag & drop; Delete to the Recycle Bin / Trash
- The chrome (☰ and window buttons), opening the menu, the menu model's enabled and checked states, the UI-drawn popup menu
- "Open with…" and "Edit with…" (E)

### Out of scope

- Thumbnails, grid and filmstrip — because they have their own spec (thumbnails agent); here they appear only where they take keys or space away from the view
- Selection, copy, cut, save cutout — because they have their own spec; here only the Esc chain and the menu's enabled states touch them
- Browsing (next/previous, Home/End, mouse back/forward), prefetch and startup — because they have their own spec
- Platform window behavior: `WM_NCCALCSIZE`, resize borders, drag strip, snap layouts, the native Windows popup menu in dark mode, the macOS menu bar, how each shell positions the resized window — because the platform-shells spec covers it
- Resident mode (what "close" does when resident), screenshots — because they have their own specs
- Animated images — because `animations.md` covers them

## Acceptance criteria

- [ ] AC-01 (always): The settings file stores the view mode and the no-upscaling option: written values read back unchanged, and an unknown or unparsable value falls back to the default of AC-02.
- [ ] AC-02 (event): When the user presses 1, 2 or 3 (without Ctrl) or picks a view mode in the menu, the viewer switches to "fit window to image", "fit image to window" or "100 %", saves the choice to the settings file at once, drops user zoom and pan (a view crop from Ctrl+X stays), and the menu shows that mode checked. Without a saved choice, the mode is "fit image to window" with "Don't enlarge small images" on.
- [ ] AC-03 (state): While the mode is "fit image to window", the whole image is fitted into the window and centred, and images smaller than the window stay at 100 % unless "Don't enlarge small images" is off (then they are enlarged to fit); while the mode is "100 %", one image pixel covers one physical screen pixel, centred, and an image larger than the window can be panned but never past its edges; in every mode the image's top-left corner lands on a whole client pixel and repeated sub-pixel pans add up instead of being lost.
- [ ] AC-04 (always): The client size for "fit window to image" is the image at 100 % shrunk with its aspect ratio to the monitor's work area and never below the minimum window size; the client size for an image larger than the window in "fit image to window" grows towards that size, never shrinks either axis and stays within the work area, and stays unchanged when the image fits or the window cannot grow.
- [ ] AC-05 (event): When the size of the current image becomes known, or the user rotates it, switches the view mode, toggles the filmstrip, cuts or returns to the original, then in "fit window to image" the viewer resizes the window to the size of AC-04 for the oriented image (or the view crop) plus the filmstrip height, and in "fit image to window" it lets the window grow as in AC-04 — at most once per image and rotation, never while the window is maximized or full screen, and never in "100 %".
- [ ] AC-06 (event): When the user turns the mouse wheel over the image (not over the filmstrip or grid), the scale changes by a factor of 1.2 per notch of 120 units, between 1 % and 6400 %, and the image point under the cursor stays under the cursor within half a client pixel; when the user presses + or −, the scale moves to the next higher or lower step of 5, 10, 12.5, 16.7, 25, 33.3, 50, 66.7, 75, 100, 150, 200, 300, 400, 600, 800, 1200, 1600, 2400, 3200, 6400 %, keeping the image point at the window centre in place; when the user presses 0, the mode's scale applies again and the image is centred.
- [ ] AC-07 (state): While the image overflows the window, a left drag on it pans and Shift+left drag selects; while it fits, a left drag selects.
- [ ] AC-08 (event): When the user drags with the middle button over the image, the image pans whatever its size; when the user presses an arrow key while the image overflows, it pans by 10 % of the window's shorter side (Left/Right with Ctrl browse instead); while it does not overflow, the arrow keys browse.
- [ ] AC-09 (event): When the user presses R (without Ctrl), the view turns 90° clockwise; L or Shift+R turn it 90° counter-clockwise; the selection, zoom, pan and any view crop are dropped, the image file is not written, and the next image opened starts unrotated.
- [ ] AC-10 (always): The image is shown in its EXIF orientation combined with the user's rotation, applied as a transform of the unrotated pixels: composing the two gives the same mapping as applying them one after the other for all 8 × 8 combinations, and all eight EXIF orientations render like the CPU reference renderer.
- [ ] AC-11 (event): When the user presses F11 or Enter (outside the grid, menu and dialogs), or double-clicks the image area below the caption strip (not on the filmstrip, the grid or a window button), the window enters or leaves full screen; while full screen, the ☰ and window buttons are neither drawn nor clickable and the window is not resized for new images.
- [ ] AC-12 (event): When the user presses Esc outside the grid, menu and dialogs, the viewer does the first that applies: ends a drag in progress; clears the selection; returns from the view crop to the whole image; leaves full screen; closes the window.
- [ ] AC-13 (state): While no image is open, the window shows the centred hint "Drop an image here or press Ctrl+O" (localized), or "Could not open “{name}”: {reason}" when the last open failed; while the current image failed to decode, the window shows "Could not open “{name}”: {reason}" instead of the previous image, with the reason "unsupported or damaged image format" when no decoder supports the format or the error came from WIC (otherwise the decoder's own message); while the current image is still loading, no message is shown.
- [ ] AC-14 (event): When the user presses Ctrl+O or picks "Open…", the viewer asks the platform for its open-file dialog; when files are dropped onto the window while the batch dialog is closed, the first dropped file opens and the others are ignored.
- [ ] AC-15 (event): When the user presses Del or picks "Move to Recycle Bin" with an image open, the file goes to the Recycle Bin (Windows) or the Trash (macOS, Linux); once it is gone, the viewer shows the file that took its place in the folder (the last one when the deleted file was last), or the empty hint when the folder is now empty; if moving fails, the image stays shown.
- [ ] AC-16 (always): The ☰ button sits at the top left and minimize, maximize and close at the top right, each 46 × 32 DIP scaled by the display scale, and only these rectangles are buttons; in right-to-left languages both sides are mirrored; where the system draws its own window buttons (macOS), only ☰ is drawn, at the top right.
- [ ] AC-17 (state, manual: visual design): While the window is not full screen, the button icons stay visible on bright and dark images (dark halo), the hovered button is highlighted and the hovered close button is red.
- [ ] AC-18 (event): When the user clicks ☰, presses F10 or the context-menu key, the menu opens below ☰, starting at the button's outer corner (the right one in right-to-left languages); when the user right-clicks, the menu opens at the cursor.
- [ ] AC-19 (state): While no image is open, zoom, rotate, copy, select all, "Open with…", "Edit with…" and "Move to Recycle Bin" are disabled in the menu; "Cut" is enabled only with a selection, "Show original" only while the view is cropped, "Save cutout" with a selection or a crop, next/previous/first/last only with more than one file in the folder, "Don't enlarge small images" only in "fit image to window"; the current view mode and full screen carry a check mark.
- [ ] AC-20 (event): When the UI-drawn menu (Linux) is open, Up/Down move over the enabled items skipping separators and disabled ones, Right or hovering opens a submenu, Left closes it (both swapped in right-to-left languages), Enter or a click chooses an enabled item, a click on a disabled item does nothing, Esc closes the innermost open level and a click outside closes the menu; the menu stays inside the window and opens leftwards in right-to-left languages.
- [ ] AC-21 (event): When the user presses E (without Ctrl) or picks "Edit with…", the current image opens in the configured editor; if no editor is configured, the viewer first asks for an editor program, remembers it and then opens the image with it.
- [ ] AC-22 (event, manual: system dialog): When the user picks "Open with…", the platform's application chooser opens for the current file.

## Technical notes

- **View state.** `ViewOptions { mode, no_upscale }` is persistent (`Settings::view`, keys `view_mode` = `fit-window-to-image` | `fit-image-to-window` | `actual-size`, `no_upscale`). `ViewState { zoom, center, crop }` is per image and reset by `App::reset_view` on every navigation. All layout coordinates are in the oriented full-resolution space; client coordinates are physical pixels.
- **Layout.** `layout::compute` picks the scale (`mode_scale`, or the user zoom clamped to `MIN_ZOOM` 0.01 … `MAX_ZOOM` 64), centres an axis that fits and clamps the centre of an axis that overflows; `overflows` is true when the scaled content exceeds the client area by more than half a pixel. "Fit window to image" uses `fit.min(1.0)`, because the window already has the image size and only images larger than the work area need fitting.
- **Zoom.** Wheel: `1.2^(delta / 120)` around the cursor (`zoom_at`). Keys: `zoom_step` over `ZOOM_STEPS`, around the client centre, not the cursor. `Digit(0)` also resets with Ctrl held. Zooming in beyond what the screen-sized stage resolves asks the loader for the full-resolution stage.
- **Pan.** `layout::pan` fixes the current scale as user zoom (`state.zoom.get_or_insert`), so after a pan the mode no longer refits the image when the window is resized.
- **Window sizing.** `App::fit_window_actions` returns `Action::FitWindow { client, grow_only }` (`grow_only` in "fit image to window"); it remembers `sized_for = cache_id ^ quarter_turns` to resize once per image and rotation. The shells compute the size with `layout::window_client_size` / `grown_client_size` against the work area and a minimum of 320 × 220 DIP (Windows `window::fit_window` keeps the window centre and clamps to the work area).
- **Rotation.** `App::user_rotation` (an `Orientation`) is combined with the EXIF orientation by `Orientation::then`; the GPU and the CPU reference apply it as an affine transform (`layout::stage_to_client`).
- **Esc chain.** `App::escape`. In the grid, `grid_key` handles Esc first (clears marks, then closes the grid); an open popup menu or batch dialog takes every key.
- **Delete.** `Cmd::Recycle` → `Action::Recycle(path)`; the shell moves the file (`SHFileOperationW` with `FOF_ALLOWUNDO` on Windows, the `trash` crate on Linux, `NSFileManager.trashItemAtURL` on macOS) and calls `App::removed`, which uses `FileList::remove` to pick the next index.
- **Menu.** `menu::items(&MenuState)` builds the tree every shell shows (Windows `TrackPopupMenuEx`, macOS menu bar, Linux `Popup` drawn by the UI). Commands carry stable ids (`Cmd::id` / `from_id`). Resident and screenshot entries are compiled in only on Windows and macOS (`RESIDENT`, `SCREENSHOTS`).
- **Editor.** `edit_action` returns `Action::EditWith` or `Action::ChooseEditor { then: Some(file) }`; `App::editor_chosen` saves the editor path unless it contains `"`, `\n` or `\r`.
- **Testing.** The App-level ACs are tested in `crates/blinzel-ui/src/app_viewing_tests.rs` with the headless harness `testkit::Harness`: keys, commands and mouse go in, the returned `Action`s, the view state, the menu model and the recorded frame (texts and their positions) come out. The window-state actions are applied the way the Linux shell does (`Harness::shell`).

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Rotating an image that was cut (Ctrl+X) | The view crop is dropped together with zoom and pan; the whole rotated image shows |
| Switching the view mode while cut | Zoom and pan are dropped, the view crop stays; the window is refit to the crop in "fit window to image" |
| Wheel at the zoom limits | The scale stays between 1 % and 6400 % |
| Tiny image in "fit window to image" | The window takes the minimum client size (320 × 220 DIP on Windows), the image stays at 100 %, centred |
| Image larger than the work area in "fit window to image" | The window fills the work area with the image's aspect ratio; the image is fitted |
| Tall image in a wide window, "fit image to window" | The window grows in height only; its width stays |
| Window maximized or full screen | No `FitWindow` request; the image is laid out in the current client area |
| Size not yet known (no probe) | No resize until the first decoded stage brings the size |
| Esc during a selection drag | The drag ends; the selection keeps the size it had |
| Deleting the last file of the folder | Title returns to the app name, the empty hint shows |
| Dropping several files | Only the first opens; with the batch dialog open, all go into its list instead |
| Editor path with a quote or line break | Not saved, nothing launched |
| Ctrl+R, Ctrl+1 … Ctrl+3, Ctrl+E | Not handled by the view (no rotation, no mode change, no editor) |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `roundtrip`, `tolerates_garbage` (`crates/blinzel-core/src/settings.rs`) | built | |
| AC-02 | `keys_and_the_menu_switch_the_mode_save_it_and_drop_zoom_and_pan` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-03 | `fit_image_to_window`, `actual_size_overflows_and_clamps_pan`, `origin_snaps_to_whole_pixels_without_drift` (`crates/blinzel-core/src/layout.rs`) | built | |
| AC-04 | `window_sizing_and_decode_size`, `window_grows_for_large_images_only` (`crates/blinzel-core/src/layout.rs`) | built | |
| AC-05 | `the_window_takes_each_images_size_once_per_image_and_rotation`, `in_fit_image_to_window_the_window_may_grow_for_each_image` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-06 | `the_wheel_zooms_by_1_2_per_notch_around_the_cursor_within_the_limits`, `plus_and_minus_step_through_the_levels_around_the_window_centre`, `zero_returns_to_the_modes_scale_centred` (`crates/blinzel-ui/src/app_viewing_tests.rs`); `zoom_keeps_point_under_cursor`, `zoom_steps_and_modes` (`crates/blinzel-core/src/layout.rs`) | built | |
| AC-07 | `drag_modes_depend_on_overflow_and_shift` (`crates/blinzel-core/src/selection.rs`) | built | |
| AC-08 | `a_middle_drag_pans_whatever_the_size_and_arrows_pan_only_an_overflowing_image` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-09 | `r_and_l_turn_the_view_and_drop_selection_zoom_pan_and_crop` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-10 | `composition_matches_transforms` (`crates/blinzel-core/src/geom.rs`), `all_orientations_minified` (`crates/blinzel-app/tests/wgpu_golden.rs`, Linux job with lavapipe; also `warp_golden.rs` in the Windows job and `metal_golden.rs` in the macOS job) | built | |
| AC-11 | `f11_and_enter_toggle_full_screen_which_hides_the_buttons`, `a_double_click_below_the_caption_strip_toggles_full_screen` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-12 | `escape_ends_a_drag_clears_the_selection_shows_the_original_leaves_full_screen_and_closes` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-13 | `without_an_image_the_window_shows_the_hint_or_why_the_open_failed`, `a_broken_image_shows_why_instead_of_the_previous_one` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-14 | `ctrl_o_and_open_ask_for_the_dialog_and_a_drop_opens_its_first_file` (`crates/blinzel-ui/src/app_viewing_tests.rs`); `drops_fill_the_open_dialog_and_otherwise_open_the_first_file` (`crates/blinzel-ui/src/app_batch_tests.rs`) | built | |
| AC-15 | `delete_moves_the_file_away_and_shows_the_one_that_took_its_place` (`crates/blinzel-ui/src/app_viewing_tests.rs`); `remove_and_placeholders` (`crates/blinzel-core/src/filelist.rs`) | built | |
| AC-16 | `layout_and_hits`, `right_to_left_mirrors`, `system_buttons_leave_the_menu_alone` (`crates/blinzel-ui/src/chrome.rs`) | built | |
| AC-17 | manual | open | |
| AC-18 | `the_menu_opens_below_its_button_and_at_a_right_click` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-19 | `the_menu_enables_and_checks_its_entries_by_state` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | built | |
| AC-20 | `keyboard_skips_separators_and_disabled_items`, `mouse_opens_submenus_and_chooses`, `stays_inside_and_mirrors_for_rtl` (`crates/blinzel-ui/src/popup.rs`) | built | |
| AC-21 | `e_opens_the_image_in_the_editor_asking_for_one_the_first_time` (`crates/blinzel-ui/src/app_viewing_tests.rs`); `edit_asks_for_an_editor_first` (`crates/blinzel-ui/src/app.rs`) | built | |
| AC-22 | manual; the request to the platform: `open_with_hands_the_current_file_to_the_platform` (`crates/blinzel-ui/src/app_viewing_tests.rs`) | open | |

## Decisions

- 2026-11-02: Extracted from the code. Deviations from the design spec recorded as current behavior: + and − zoom around the window centre, not the cursor; "fit image to window" also resizes the window (grows it for images larger than the window); rotation drops a view crop; the "Move to Recycle Bin" label is used on macOS and Linux too, where the file goes to the Trash.
- 2026-11-02 (spec-build): AC-02, AC-06, AC-08 and AC-09 held already; their App tests went green at once and were checked instead by breaking the code on purpose (saving, the dropped zoom, pan and crop, the menu's check mark, Ctrl with the digits, the wheel factor, its fixed point, the step table, the limits, the reset of the centre, the middle button, the pan step, Ctrl with the arrows, the rotation's direction and what it drops): every such change turned a test red.
- 2026-11-02 (spec-build): AC-05 is tested on the `FitWindow` request `App` hands the shell: its size is the oriented image (or the crop) plus the strip, and the shell turns it into the size of AC-04 (`layout::window_client_size` / `grown_client_size`, tested there). The behavior held already; breaking each trigger, the once-per-image tag, the crop, the strip, `grow_only` and the maximized, full screen and "100 %" exits turned the tests red.
- 2026-11-02 (spec-build): AC-11's double click was decided in each shell (Linux and macOS: the caption strip maximizes, everything else toggles full screen; Windows: everything but a button). `App::double_click` now returns the full screen toggle itself for the image area below the caption strip and for any point in full screen, and `None` only for the caption strip and its buttons, which the shells keep for maximizing (Linux, macOS) or leave to `HTCAPTION` (Windows). The behavior is the same on every platform, now tested once. The keys, the hidden buttons, AC-12 and AC-13 held already and were checked by breaking the code on purpose.
- 2026-11-02 (spec-build): AC-18's right click was turned into a menu request by each shell. New public API `App::right_click(p)` returns that request (`ShowMenu` at `Host::to_screen(p)`); the Linux and macOS shells and the Windows `WM_RBUTTONUP` call it (`WM_CONTEXTMENU` keeps the cursor position). The menu button's corner is the function `menu_corner(button, rtl)`, tested for both directions directly: no right-to-left language exists yet, so the App-level test can only run left to right.
- 2026-11-02 (spec-build): AC-15 is tested in `App`: Del and the menu ask the shell to move the current file (`Action::Recycle`), the image stays until the shell reports the move with `App::removed` (a failed move reports nothing), then the file that took its place shows, or the empty hint. The move itself is one platform call per shell (`SHFileOperationW` with `FOF_ALLOWUNDO`, `trash::delete`, `NSFileManager.trashItemAtURL`) and has no automated test. The menu label stays "Move to Recycle Bin" on macOS and Linux as recorded above: no AC or edge case asks for another label there.
- 2026-11-02 (spec-build): AC-14, AC-15, AC-19 and AC-21 held already; breaking Ctrl+O, the open command, the first dropped file, the dialog's drop, the Del key, a premature removal, the next index, the empty state and title, each enabled and checked state, the E key and remembering, saving and filtering the editor path turned their tests red. Dropping `shown` after the last file goes is not observable (nothing is drawn without a current image) and survives such a change.
- 2026-11-02 (spec-build): AC-06's fixed point under the cursor is tested where the zoomed image overflows the window. Where it still fits, AC-03's centring wins on that axis (`layout::compute` centres an axis that fits), so a small image zoomed by the wheel stays centred rather than sliding towards the cursor. For + and − the point at the window centre is checked within one client pixel: after a pan the exact origin is fractional, and both the point and its new place go through the whole-pixel snap.
