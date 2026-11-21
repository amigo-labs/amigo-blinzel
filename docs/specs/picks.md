---
status: todo
created: 2026-11-21
size: L
platforms: all
design: [blinzel.md §2, blinzel.md §5, blinzel.md §6, blinzel.md §7]
---

# Picking images and handing them over

## Context

After a shoot, the photographer sends a folder of pictures and the client chooses the ones they want. To do that in the viewer, a person has to be able to mark images while flipping through them, see what they marked, check the choice, and then hand it back: as a list of file names to paste into a mail, or as copies of the original files in a folder.

Marks exist today only inside the grid (`thumbnails.md` AC-19): Ctrl+click, Shift+click and Ctrl+A mark tiles, a plain click or Esc clears them, and they are gone when the grid closes. The only use is filling the batch dialog (`thumbnails.md` AC-23, `batch-resize.md` AC-22).

This spec makes the marks a lasting choice per folder (decision "Picks" in `blinzel.md` §2): marked with F in the single view or in the grid, shown in every view, kept across closing the grid, switching folders and restarting, and never written into the image folder (AGENTS.md §9). The info overlay (`info-overlay.md`) is a separate spec; where both are built, the overlay shows the mark too (AC-07).

Size L: `spec-build` builds it in AC blocks over two sessions (marking and keeping: AC-01 … AC-13; handing over and the filter: AC-14 … AC-26).

## Scope

### In scope

- Marking and unmarking the current image with F, in the single view and the grid; the grid's existing Ctrl+click, Shift+click and Ctrl+A
- The mark shown in the single view, the filmstrip and the grid, and a count
- Keeping the marks per folder outside the image folder
- "Remove all marks", with a confirmation
- "Copy marked file names" to the clipboard
- "Copy marked files to folder…": copies of the originals in a folder the user picks
- "Only marked": browsing, filmstrip and grid limited to the marked images
- Ctrl+B starting the batch dialog with the marks, also outside the grid

### Out of scope

- Including same-named partner files (IMG_1.jpg with IMG_1.CR2) — because the user chose "exactly the marked file" on 2026-11-21
- Star ratings, colour labels, several kinds of marks — because handing over a choice needs one yes/no mark; more kinds would need a reason first
- Writing marks into the files (XMP, EXIF) or sidecar files next to them — because the viewer never writes into the image folder (AGENTS.md §9) and a client's folder may be read-only
- Moving or deleting the marked files, zipping, mailing, uploading — because the two handovers (names, copies) cover the need and every other route is the system's job once the copies exist
- Marks across folders (one list for several folders) — because the photographer's delivery is one folder; each folder keeps its own marks
- Syncing marks between computers — because the photographer and the client work on different machines anyway and hand over names or copies; where the OS carries the config folder along (a roaming profile on Windows), marks come along unplanned

## Acceptance criteria

Messages below are shown as the viewer's notice: the box at the bottom centre that save results and errors use today. "The folder's order" is the order the folder list shows while browsing (`browsing.md`).

### Marking

- [ ] AC-01 (event): When the user presses F (without Ctrl, with or without Shift) in the single view, then the shown image is marked if it was not and unmarked if it was; in the grid, F does the same for the selected tile. While a dialog or popup is open, F goes to it as before. With no image shown or an empty grid, F does nothing; for an image that is not in a folder list (opened from the clipboard), F shows "Only images in a folder can be marked".
- [ ] AC-02 (event): When the user Ctrl+clicks a tile, Shift+clicks a tile or presses Ctrl+A in the grid, then Ctrl+click toggles that tile's mark, Shift+click adds every image from the selected tile to the clicked one, Ctrl+A marks every image of the folder; these are the same marks F sets.
- [ ] AC-03 (event): When the user clicks a tile without a modifier or presses Esc in the grid, then the marks stay as they are: the click selects the tile, Esc closes the grid (revising `thumbnails.md` AC-18 and AC-19).
- [ ] AC-04 (event): When the user chooses "Remove all marks" in the menu while images of the folder are marked, then the viewer asks "Remove the marks of {n} images?" with "Remove" and "Cancel"; only "Remove" clears every mark of this folder, also the names of files that are missing now (AC-11). Without marks the entry is disabled.
- [ ] AC-05 (state): While the shown image is marked, the single view shows the grid's check mark badge at its top right, below the window buttons (in full screen, where they are hidden, at the top right corner); its tile in the filmstrip and the grid carries the same badge.
- [ ] AC-06 (state): While images of the folder are marked, the grid shows "{n} marked" at its top, next to the ☰ button, and the menu entries of AC-04, AC-14 and AC-16 name the count ("Copy {n} file names").
- [ ] AC-07 (state): Where the info overlay (`info-overlay.md`) is built and on, it shows one more line "✓ Marked" below the position for a marked image, and "{m} marked in this folder" as its last line while m > 0; its other lines stay as `info-overlay.md` says.

### Keeping the marks

- [ ] AC-08 (event): When the user marks or unmarks an image, closes the grid, opens another folder, or quits and starts the viewer again, then the marks of each folder are the same as after the user's last change there — also when two viewer windows change marks of the same folder: each change applies only itself to the stored marks and keeps the other window's changes; "Remove all marks" clears every stored name of the folder, and Ctrl+A adds every image of the folder.
- [ ] AC-09 (always): The marks of a folder are stored outside that folder, in the viewer's per-user config folder (`settings.md` AC-01); the viewer never creates, changes or deletes a file in the image folder for them. A folder is identified by its absolute path as opened.
- [ ] AC-10 (always): Reading the marks never delays a frame or a key: an image opened at startup and the next image while browsing are drawn without waiting for the folder's marks, and badges and counts appear once they are read. Until then, "Remove all marks", "Copy marked file names", "Copy marked files to folder…" and "Only marked" are disabled, Ctrl+B and Ctrl+Shift+C wait for the read, and F presses made meanwhile are applied in order once it is done. Proven by tests that hold the read back.
- [ ] AC-11 (error): If a marked file is missing from the folder (deleted, renamed, moved, the card pulled out), then it is not shown as marked, not counted and not handed over, but its name stays stored, so a file that comes back under that name is marked again. Only unmarking, "Remove all marks" and moving the file to the recycle bin from the viewer remove a stored name (the last because the user deliberately threw the image away).
- [ ] AC-12 (error): If the marks file is missing, unreadable or broken (not UTF-8, names with a path separator, empty lines, NUL bytes), then the viewer uses the valid names in it (none if nothing is valid) and keeps running. If it cannot be written, marking still works for the rest of the session and the viewer shows "Marks cannot be saved: {reason}" once per session. A file name that is not valid UTF-8 (possible on Linux) can be marked, but is not kept after the viewer exits, because the marks file holds UTF-8 text and such names are too rare to justify a binary format.
- [ ] AC-13 (always): Storing the marks happens off the UI thread: a key press, Ctrl+A on a folder of 10 000 images or 500 F presses in a row never wait for a write, and each press or Ctrl+A is stored as one change (later writes may combine changes still pending). Proven by tests that hold the write back.

### Handing over

- [ ] AC-14 (event): When the user chooses "Copy marked file names" or presses Ctrl+Shift+C while images of the folder are marked, then the clipboard holds the names of the marked files that exist as plain text, with extension, without folder, in the folder's order, each followed by the platform's line end (CR LF on Windows, LF elsewhere), and the viewer shows "{n} file names copied". Ctrl+Shift+C no longer copies the image (Ctrl+C still does). If the clipboard cannot be written, the viewer shows "Could not copy to the clipboard: {reason}".
- [ ] AC-15 (state): While no existing image of the folder is marked, "Copy marked file names" and "Copy marked files to folder…" are disabled and Ctrl+Shift+C only shows "No images marked".
- [ ] AC-16 (event): When the user chooses "Copy marked files to folder…", then the system's folder dialog opens; after the user picks a folder, every marked file that exists is copied into it byte for byte under its own name, and the viewer shows "{n} files copied to “{folder name}”". Cancelling the dialog, or a dialog that cannot be shown, copies nothing and shows nothing (the system dialogs report both the same way).
- [ ] AC-17 (always): Copying never replaces or changes a file: where the target folder already holds a file of that name (as the target's file system compares names, also one created meanwhile by another program), the copy is named "{stem} (2).{ext}", "(3)" and so on, the first free one; the source files are never changed; a copy that fails or is cancelled never leaves a partial file under the file's own or a numbered name, and the viewer removes its temporary file where the target is still reachable.
- [ ] AC-18 (state): While files are being copied, the viewer stays usable (browsing, marking, other folders, menus) and shows "Copying {i} / {n} …"; "Copy marked files to folder…" is disabled until the copy is done; the copy works on the marks as they were when the folder was picked. When the viewer exits during a copy, the copy stops: finished files stay, the file in progress is removed (AC-17).
- [ ] AC-19 (error): If single files cannot be copied (source gone, target full or gone, no permission, name too long), then the others are still copied and the viewer shows "{k} of {n} files copied; {failed} failed: {first reason}" (also with k = 0, e.g. when every marked file vanished after the folder was picked). If the picked folder is the image folder (also when reached through another path, such as a symlink), the viewer copies nothing and shows "Choose a different folder than the images' own" — because numbered copies next to the originals would only clutter the photographer's folder.
- [ ] AC-20 (event): When the user presses Ctrl+B or chooses "Resize images…" while images of the folder are marked, then the batch dialog starts with exactly the marked files that exist, in the folder's order, also outside the grid (revising `thumbnails.md` AC-23 and `batch-resize.md` AC-22).

### Only marked

- [ ] AC-21 (event): When the user turns on "Only marked" in the menu while images of the folder are marked, then Next, Previous, First, Last, the filmstrip and the grid show only the marked images; the shown image stays if it is marked, otherwise the next marked one in the folder's order (or else the previous one) is shown.
- [ ] AC-22 (state): While "Only marked" is on, the position "{n} / {total}" (in the info overlay and wherever it is shown) counts only the marked images, the menu entry is checked, and the grid shows "Only marked" next to the count.
- [ ] AC-23 (event): When the user unmarks the shown image while "Only marked" is on, then it stays shown until the user moves on, while the filmstrip and the grid drop its tile at once and Next and Previous go on from where it was; when no marked image is left (the last unmarked, or the last marked file gone from the folder), "Only marked" turns off by itself and the whole folder is shown again from the shown image.
- [ ] AC-24 (event): When the user turns "Only marked" off or opens another folder, then the whole folder is shown again, from the image shown last; "Only marked" is not kept across folders or restarts — because it is a step to check a choice, and a folder that opens filtered would hide images without the user noticing.
- [ ] AC-25 (always): Every new label, message and confirmation is in every `i18n/*.toml`; "Remove all marks", "Copy marked file names", "Copy marked files to folder…" and "Only marked" are in the menu (on macOS also in the menu bar) in the group with "Filmstrip" and "Grid".
- [ ] AC-26 (state, manual: visual design): While images are marked, the badge in the single view, the filmstrip and the grid, the count and the confirmation look right on Windows, macOS and Linux at 100 % and 200 % scaling.

## Technical notes

Hints, not requirements; the ACs decide.

- Today's marks: `GridState::marked: BTreeSet<String>` (`browse.rs`), filled in `App::browse_click` and `App::grid_key`, read by `App::open_batch`, pruned by `forget_entry_in_grid`. They move from `GridState` to state that lives with the folder list.
- Storage (decision "Picks", `blinzel.md` §2): `<config>/picks/<hash>.txt`, where `<hash>` is a stable hash of the folder's absolute path; a header line holds the folder path (to tell hash collisions apart), each further line one file name in UTF-8. A change re-reads the file, applies only itself (add or remove names) and writes through a temporary file and a rename, so two windows do not lose each other's changes (AC-08). The config folder, not the cache, because the OS may clear caches and the marks are the user's work.
- The free-name rule exists for screenshots (`blinzel_core::screenshot::free_name`, PNG only); it generalises to any extension.
- The shells get two new actions: put text on the clipboard, and pick a folder and hand it back to the App (the pattern of `Action::PickScreenshotFolder`). The copy itself is platform-independent and runs on a worker.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| 500 images marked, Ctrl+Shift+C | 500 lines in the clipboard, in folder order |
| `IMG_1.jpg` and `IMG_1.CR2` in the folder, only the JPEG marked | Only `IMG_1.jpg` is listed and copied |
| `a.jpg` and `A.jpg` in one folder (Linux) | Two different marks; each name is kept exactly |
| Target folder already has `IMG_1.jpg` | The copy becomes `IMG_1 (2).jpg` |
| Target folder is the image folder | Nothing is copied (AC-19) |
| `a.jpg` and `A.jpg` marked, copied to a case-insensitive drive | The second becomes `A (2).jpg` (AC-17) |
| Target folder is on a USB stick that is pulled out mid-copy | The files done stay, the rest are reported as failed (AC-19) |
| A source file is being written by another program while it is copied | The copy holds what was read; the viewer does not check for that, because no copy tool locks a source and the photographer's files are finished |
| A marked file is renamed outside the viewer | The old name is not counted (AC-11); the new name is not marked; renaming it back marks it again |
| The folder is opened through another path (a symlink, a mapped drive) | Its marks are separate — the path is the key; documented as an assumption |
| Image opened from the clipboard or without a folder | F does nothing and shows "Only images in a folder can be marked" |
| Two viewer windows on the same folder | Both changes are kept (AC-08); a window shows the other's marks after it re-reads them (on its next change or when the folder is opened again) |
| "Only marked" on, the folder watch adds a new image | It is not shown until it is marked |
| Marks file of 100 000 names | Read off the UI thread; only names present in the folder count |
| Copying 10 GB of RAW files | Progress shows; the window stays usable (AC-18) |

## Assumptions

| Assumption | Reason | Risk |
| ---------- | ------ | ---- |
| The folder's absolute path identifies it | Simple and stable as long as the folder is not moved | Moving or renaming the folder, or opening it through a symlink, loses its marks |
| Ctrl+Shift+C fits | Ctrl+C copies the image; Shift makes the "copy the list" variant easy to remember; today Ctrl+Shift+C does the same as Ctrl+C, so nothing is lost | Someone used Ctrl+Shift+C for copying the image |
| The copy keeps the file's content but not necessarily its timestamps | A byte-for-byte copy is what the photographer needs; the OS copy routine keeps the modification time where it can | A photographer sorts by file date and finds the copy's date changed on some file system |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `f_marks_the_shown_image_and_a_badge_shows_it`, `f_in_the_grid_marks_the_selected_tile` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-02 | `grid_marks_with_ctrl_shift_and_ctrl_a_and_a_click_or_escape_keeps_them` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`) | built |  |
| AC-03 | `grid_marks_with_ctrl_shift_and_ctrl_a_and_a_click_or_escape_keeps_them` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`) | built |  |
| AC-04 | `remove_all_marks_asks_first_and_only_remove_clears` (`crates/blinzel-ui/src/app_picks_tests.rs`); `resize_images_starts_with_the_marked_files_in_folder_order` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`) | built |  |
| AC-05 | `f_marks_the_shown_image_and_a_badge_shows_it` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-06 | `the_grid_and_the_menu_name_the_count` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-07 | `the_info_overlay_shows_the_mark_and_the_count` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-08 | `marks_last_across_the_grid_folders_and_a_restart_and_never_touch_the_folder`, `a_change_keeps_what_another_window_marked` (`crates/blinzel-ui/src/app_picks_tests.rs`); `a_change_keeps_what_another_window_stored_meanwhile`, `each_folder_has_its_own_marks` (`crates/blinzel-core/src/picks.rs`) | built |  |
| AC-09 | `marks_last_across_the_grid_folders_and_a_restart_and_never_touch_the_folder` (`crates/blinzel-ui/src/app_picks_tests.rs`); `marks_survive_a_round_trip_through_the_file` (`crates/blinzel-core/src/picks.rs`) | built |  |
| AC-10 | `reading_and_storing_marks_never_hold_up_frames_or_keys` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-11 | `a_missing_file_keeps_its_mark_until_unmarked_and_the_recycle_bin_drops_it` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-12 | `a_broken_marks_file_counts_its_valid_names_and_a_failed_write_says_so_once` (`crates/blinzel-ui/src/app_picks_tests.rs`); `a_broken_file_keeps_its_valid_names`, `a_file_of_another_folder_or_without_header_counts_as_none`, `names_that_are_no_plain_file_names_are_not_stored` (`crates/blinzel-core/src/picks.rs`) | built |  |
| AC-13 | `reading_and_storing_marks_never_hold_up_frames_or_keys` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-14 | `ctrl_shift_c_puts_the_marked_names_on_the_clipboard_in_folder_order` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-15 | `without_marks_the_handover_is_disabled` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-16 | `copying_marked_files_makes_byte_identical_copies_and_never_replaces` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-17 | `copying_marked_files_makes_byte_identical_copies_and_never_replaces` (`crates/blinzel-ui/src/app_picks_tests.rs`); `copies_keep_the_bytes_take_a_free_name_and_never_replace`, `a_cancelled_copy_leaves_nothing_behind` (`crates/blinzel-core/src/picks.rs`) | built |  |
| AC-18 | `copying_marked_files_makes_byte_identical_copies_and_never_replaces` (`crates/blinzel-ui/src/app_picks_tests.rs`); `a_cancelled_copy_leaves_nothing_behind` (`crates/blinzel-core/src/picks.rs`) | built |  |
| AC-19 | `copying_reports_failures_and_refuses_the_images_own_folder` (`crates/blinzel-ui/src/app_picks_tests.rs`); `a_target_that_cannot_be_written_fails_each_file` (`crates/blinzel-core/src/picks.rs`) | built |  |
| AC-20 | `resize_images_outside_the_grid_takes_the_marks`, `resize_images_starts_with_the_marked_files_in_folder_order` (`crates/blinzel-ui/src/app_thumbnail_tests.rs`) | built |  |
| AC-21 | `only_marked_browses_the_marked_images_and_counts_them`, `only_marked_falls_back_to_the_previous_marked_image` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-22 | `only_marked_browses_the_marked_images_and_counts_them` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-23 | `unmarking_under_only_marked_keeps_the_image_until_moving_on` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-24 | `only_marked_ends_when_turned_off_or_in_another_folder` (`crates/blinzel-ui/src/app_picks_tests.rs`) | built |  |
| AC-25 | `the_marks_entries_sit_with_filmstrip_and_grid` (`crates/blinzel-ui/src/app_picks_tests.rs`); the strings in every `i18n/*.toml` (the i18n build checks that every key is in every language) | built |  |
| AC-26 | manual: visual design | open |  |

## Decisions

- 2026-11-21 (user, in the spec dialogue): F marks ("favourite"; Space already goes to the next image); exactly the marked file is handed over, no same-named partners; "Only marked" as a filter; a plain click and Esc in the grid no longer clear the marks, only "Remove all marks" with a confirmation does, because a lasting choice must not vanish by a stray click; Ctrl+B takes the marks also outside the grid; marks are kept per folder outside the image folder. Clarification pass the same day: two windows merge their changes, missing files keep their stored mark until unmarked, the folder copy refuses the image folder itself, Ctrl+Shift+C stops copying the image.
- 2026-11-21 (spec-build): the marked names live in `Browse::marks` (they are drawn there); `marks.rs` keeps which folder they belong to and reads and stores them on one worker thread in order, so a read finishes before the changes made meanwhile are stored (AC-10). Results wake the UI thread through `Wake::Thumbs`, like the info overlay's, so the shells stay unchanged.
- 2026-11-21 (spec-build): the marks are read once the folder is listed (`App::on_list`), not when an image opens: the first image never waits, and the listing is what tells which names exist.
- 2026-11-21 (spec-build): "Remove all marks" asks with a small drawn question (`confirm.rs`, Enter: Remove, Esc: Cancel), in the look of the batch dialog. Because no shell has a native yes/no question, and a drawn one behaves the same on every platform.
- 2026-11-21 (spec-build): menu labels and messages that name a count use the existing plural `images_count` ("12 images") in parentheses or after a colon, so no language needs new plural forms.
- 2026-11-21 (spec-build): new public API: `blinzel_core::picks::{Change, file_of, storable, parse, to_text, load, apply}`, `blinzel_ui::marks::Marks`, `blinzel_ui::confirm::{Confirm, Answer}`, `Browse::marks` (replacing `GridState::marked`), `browse::mark_badge`, `Cmd::ClearMarks` (id 46), `MenuState::marked`.
- 2026-11-21 (spec-build): the core tests of AC-08, AC-09, AC-12 ran red against stubs; the App-level tests of AC-01 … AC-13 were written together with the code, after the core, and were not run red first.
- 2026-11-21 (spec-build): the shell asks for the copy folder with the batch dialog's existing `Action::PickFolder`; `App::batch_folder_picked` sends it to the copy when no dialog is open. The names go to the clipboard with a new `Action::CopyText`, answered by `App::text_copied` (Windows `CF_UNICODETEXT`, macOS `NSPasteboard`, Linux arboard); the Windows and macOS parts are checked by their CI jobs.
- 2026-11-21 (spec-build): copies are created with "create new" and filled in place, the name taken by the creation itself, so a file made meanwhile by another program is never replaced (AC-17); a failure or a cancel removes the file again. The copy runs on its own thread and its progress wakes through `Wake::Thumbs` like the marks.
- 2026-11-21 (spec-build): "Only marked" swaps the folder list for the marked part of it and keeps the whole list aside, so browsing, the filmstrip, the grid and the position all follow without changes of their own; unmarking the shown image uses the existing "the shown file left the folder" path (AC-23).
- 2026-11-21 (spec-build): the App's own notices (copied, refused, no marks) go at the next key or click, because only the shells time a notice and the App's would otherwise stay.
- 2026-11-21 (spec-build): new public API: `blinzel_core::picks::{Copied, copy_files, create_free}`, `Action::CopyText`, `App::text_copied`, `Cmd::{CopyMarkedNames, CopyMarkedFiles, ToggleOnlyMarked}` (ids 47 … 49), `MenuState::{only_marked, copying}`.
- 2026-11-21 (spec-build): the tests of AC-14 … AC-25 were written after the code of this block; the core copy tests ran green at once against the first implementation.
- 2026-11-21 (user, revision): AC-16 drops "if the dialog cannot be shown, the viewer shows …": the folder dialogs of all three shells report a failure like Cancel, so a failure copies nothing and shows nothing, like Cancel.
