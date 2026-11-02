---
status: todo
created: 2026-11-02
size: L
platforms: all
design: [blinzel.md §4, blinzel.md §5, blinzel.md §12, blinzel.md §15]
---

# Browsing: folder listing, navigation and prefetch

## Context

amigo-blinzel is meant to be measurably faster than any other viewer at flipping through images (`blinzel.md` §1). Opening a file lists its folder in natural order in the background; Next/Previous show the neighbours, which a two-stage prefetch scheduler has already read and decoded in browsing direction and uploaded to the GPU within a video-memory budget (`blinzel.md` §5 "Prefetch scheduler", "GPU", "I/O", "Hybrid CPUs", "Robust"). M2 sets the targets "next ≤ 8 ms p99 prefetched; 10k folder ≤ 25 ms" (`blinzel.md` §12). This spec records what the code does today (reverse engineered).

## Scope

### In scope

- The folder listing: which files belong to it, natural sort order, finding the opened file in it
- Navigation in the single-image view: Next, Previous, First, Last, wrapping around
- Prefetch: what is prepared in which order, the scheduler's priorities and cancellation, one read per job, full resolution when zoomed
- GPU tiles and the video-memory budget
- Folder watching
- "Open images in the same window" (single instance) for a visible window
- Cloud placeholders, rotating disks, decode threads on hybrid CPUs
- Robustness against broken files and panicking decoders
- The M2 browsing targets and how they are measured

### Out of scope

- The cold start and its order — because it is the startup path (`startup.md`)
- Handing over to a hidden resident instance — because it is resident mode (`resident-mode.md`)
- Thumbnails, the filmstrip (including its wheel navigation) and the grid with its own keys — because they are thumbnails (`thumbnails.md`)
- View modes, zoom and panning — because they are the view modes; this spec only covers when zooming asks for full resolution
- Decoders, embedded camera previews and raw previews — because they are the decoders
- Animation playback (stage `Anim`) — because it belongs to the animated formats
- Copy, cut, save (Ctrl+C < 1 ms in M2) and moving to the recycle bin — because they are selection and file operations (`blinzel.md` §7)
- Colour transforms built while images decode — because that is colour management

## Acceptance criteria

### Folder listing

- [ ] AC-01 (always): The folder's images are in natural order: digit runs compare as numbers (leading zeros ignored, any length: `img2` < `img10`, `IMG_0009` < `IMG_0010` < `IMG_0100`), letters case-insensitively with diacritics sorted with their base letter (`ä` with `a`, `ß` as `ss`), punctuation and spaces before digits before letters; names equal under these rules are ordered by their exact spelling.
- [ ] AC-02 (always): The list holds only files with an image extension (case-insensitive); directories and files that are both hidden and system are left out, files that are only hidden stay.
- [ ] AC-03 (event): When a file is opened, then it is shown without waiting for the folder listing, which runs in the background; once the listing is there, the file's position in it is found by exact name, else case-insensitively. While the listing is not there yet, Next, Previous, First and Last do nothing.

### Navigation

- [ ] AC-04 (event): When the user presses Right, Page Down, Space or Down, then the next image is shown; Left, Page Up, Backspace or Up show the previous one, Home the first and End the last; in a right-to-left language Right and Left are swapped. While the image is zoomed beyond the window, the arrow keys pan instead.
- [ ] AC-05 (always): Navigation wraps around: one step past the last image is the first, one step before the first is the last, and any number of steps wraps the same way.

### Prefetch

- [ ] AC-06 (event): When the current image changes, then the viewer prepares the current image first and then its neighbours in browsing direction, 4 ahead and 2 behind, in the order +1, then +2 and −1, then +3, −2, +4; the embedded preview is requested only for the current image and the two nearest neighbours. Images already prepared or that failed are skipped (cloud placeholders: AC-17); in a folder smaller than the window every image is prepared once. Next, Page Down and First set the direction forwards, Previous and Last backwards, opening a file forwards.
- [ ] AC-07 (option): Where `prefetch_ahead` or `prefetch_behind` is set in the settings file, that many images are prepared ahead or behind (0 turns it off); values above 32 count as 32, unreadable values keep the default of 4 ahead and 2 behind.
- [ ] AC-08 (always): Prepare jobs run most urgent first; replacing the wanted set drops queued jobs that are no longer wanted, re-prioritizes the rest and cancels running ones that are no longer wanted; a job that completed is not run again until its result has been dropped; job groups (classes) do not affect each other.
- [ ] AC-09 (always): Every prepare job reads its file once, and the embedded preview (if requested and useful) and the wanted resolution both come from that read; the image the user waits for is decoded with all decode threads, neighbours with one thread each.
- [ ] AC-10 (state): While the current image is zoomed so far that the screen-sized version no longer resolves it (more than 1.05 image texels per screen pixel), or the view mode is "100 %", the full-resolution image is prepared and replaces the screen-sized one; in "100 %" the neighbours are prepared at full resolution too.

### GPU

- [ ] AC-11 (always): An image wider or taller than 4096 texels is shown in full: it is uploaded as tiles of at most 4096 texels per edge (border included), each with a border of neighbouring texels, and the seams do not show (the rendering matches the CPU reference renderer).
- [ ] AC-12 (always): When the images on the GPU exceed the video-memory budget, the least recently loaded ones that are not protected are released until the budget holds; if every image is protected, the viewer stays over budget rather than releasing one.
- [ ] AC-13 (always): The current image and the images up to the larger of the ahead and behind distances on either side of it are protected from release; a released image is prepared again when it is needed. The budget is half of what Windows grants the process (DXGI), clamped to 256 MiB … 3 GiB, 1 GiB on Linux and a third of Metal's recommended working set on macOS.

### Folder watching

- [ ] AC-14 (event): When a file in the shown folder is added, removed or renamed, or its size or write time changes, then about 150 ms later the folder is listed again: added images become reachable with Next and Previous, removed ones are no longer shown, and the current image stays on screen.

### Same window

- [ ] AC-15 (option): Where "Open images in the same window" is on (the default; Settings menu, saved at once), opening an image while a viewer window is open shows it in that window, which comes to the foreground, and the new launch ends (Windows and Linux); where it is off, every launch opens its own window (Windows: unless a hidden resident instance exists, `resident-mode.md`).
- [ ] AC-16 (always, CI: windows): A handoff carries the file path and the start time of the sending process; the receiver accepts "open", "show" and "quit" requests, ignores an "open" request with an empty path and ignores unknown request types.

### I/O and threads

- [ ] AC-17 (state, CI: windows): While neighbours are cloud placeholders (offline, recall on open or recall on data access), they are not read ahead, so browsing never starts a download; an image the user opens or navigates to is still loaded.
- [ ] AC-18 (state, CI: windows): While the file opened at startup lies on a drive with a seek penalty (rotating disk), the viewer reads one file at a time, otherwise two in parallel; decode workers number the logical processors of the most performant efficiency class, at least 2 and at most 6, and run above normal priority without power throttling.

### Robustness

- [ ] AC-19 (error): If a decoder panics while probing or decoding a still image, then the panic becomes a decode error for that image and the viewer keeps running.
- [ ] AC-20 (error): If an image cannot be read or decoded, then the viewer shows "Could not open “{name}”: {reason}" for it instead of the previous image, does not try that file again while its size and write time stay the same, and Next and Previous keep working; a failed embedded preview does not count as a failed image.

### Targets

- [ ] AC-21 (always, manual: measurement): On Windows with a real GPU, the next image appears within 8 ms at p99 when it was prefetched, and listing a folder of 10 000 images takes at most 25 ms (M2), measured with `blinzel.exe --bench <dir> --json bench.json` (`navigation.latency.p99_ms` with `prefetch_hits`, `list_10k.median_ms`). No CI step asserts a time.

## Technical notes

- Listing: `blinzel_core::filelist::FileList` (`new` filters and sorts with `natsort::SortKey`, `index_of`, `offset` wraps with `rem_euclid`, `remove`), `Entry { name, size, mtime, attributes }`, `Entry::cache_id` (FNV-1a of lower-case name, size, mtime), `Entry::is_placeholder` (`attr::OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS`). Windows lists with `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)` (`platform/enumerate.rs`); Linux and macOS with `std::fs::read_dir` (`blinzel-ui/src/files.rs`, attributes 0, so no placeholders and no hidden/system flags there). The listing runs on the thread `blinzel-list` and reports through `Wake::List` → `App::on_list`.
- Navigation: `App::key`, `Cmd::Next/Prev/First/Last`, `App::step`, `App::go_to_index` (`blinzel-ui/src/app.rs`).
- Prefetch policy: `App::update_wanted`. Current image priority 0 (stage `Screen`, or `Full` per `current_stage_needed`), animation stage priority 1; ahead `k` priority `2k − 1`, behind `k` priority `3k`; ties are broken by the job key. `blinzel-bench`'s navigation loop (`bench.rs`) mirrors these priorities with fixed 4/2.
- Pipeline: `blinzel_ui::loader::Loader` over `blinzel_core::sched::Scheduler` (I/O stage `Services::read_file`, CPU stage probe → raw preview / embedded preview → `Services::decode` → `Device::upload` on the worker). Job key = cache id with the stage (`Preview`, `Screen`, `Full`, `Anim`) in the low two bits. The embedded preview is used if it covers half the screen box in one dimension and has under a third of the image's pixels (or the file is a raw).
- Budget: `App::on_loaded` → `ByteLru::evict_to(vram_budget, keep)` with `keep` = `App::wanted_ids`, evicted keys go to `Loader::forget`. `Device::texture_budget`: Windows `gfx/device.rs` (`QueryVideoMemoryInfo` local budget / 2, else max(dedicated, shared / 4) / 2), Linux `linux/gpu.rs` 1 GiB, macOS `macos/gpu.rs` `recommendedMaxWorkingSetSize / 3`. Tiles: `blinzel_ui::gfx::plan_tiles`, `BORDER = 4`, `DEFAULT_TILE = 4096`; Windows `gfx/texture.rs`.
- Watching: `Host::watch_dir` → `Wake::DirChanged` → `App::on_dir_changed` (relists; during a listing it asks for one more listing after it, `App::relist`). Windows `platform/enumerate.rs` `DirWatcher` (`ReadDirectoryChangesW`, file name, size and last write, non-recursive, 150 ms sleep before posting `WM_DIR_CHANGED`); Linux and macOS the `notify` crate (non-access events, `DIR_SETTLE` 150 ms in the shell).
- Same window: Windows `instance.rs` (`WINDOW_CLASS` "AmigoBlinzelViewer", `WM_COPYDATA` `COPYDATA_OPEN` with path and four UTF-16 units of the sender's `FILETIME`, `COPYDATA_SHOW`, `COPYDATA_QUIT`), received in `window.rs` → `App::open` + `Action::Wake` → `Action::Activate` for a visible window. Linux `linux/instance.rs` (Unix socket `$XDG_RUNTIME_DIR/amigo-blinzel.sock`, one line per launch, empty line = show). Setting `single_instance`, `Cmd::ToggleSingleInstance`, string `setting_single_instance`.
- Threads (Windows `run.rs`): `io_threads = 1` if `platform::io::drive_info(file).seek_penalty` (`IOCTL_STORAGE_QUERY_PROPERTY`, `StorageDeviceSeekPenaltyProperty`) else 2; `cpu_threads = cpu_info().performance.clamp(2, 6)` (`GetSystemCpuSetInformation`, highest `EfficiencyClass`); the visible image decodes with `performance` threads. `threads::tune_current_thread(Kind::Latency)`: `THREAD_PRIORITY_ABOVE_NORMAL`, power throttling off — no CPU-set affinity. Linux and macOS: 2 I/O threads, `available_parallelism().clamp(2, 6)` workers, all logical processors for the visible image, no thread tuning.
- Reading: Windows `platform/io.rs` `read_file` (`FILE_FLAG_SEQUENTIAL_SCAN`, share read/write/delete, one `PoolBuf` from `blinzel_core::bufpool`, no memory mapping).
- Robustness: `blinzel_core::decode::guard` (`catch_unwind`) wraps `probe`, `decode_with` (and so `decode_preview`), the WIC and ImageIO paths; `Cargo.toml` keeps `panic = "unwind"`. The animation streams (`gif`/`webp`/`jxl::stream_animation`, called from `loader::animation_stage`) run outside `guard`.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| A folder with one image | Next and Previous do nothing; no neighbour is prepared |
| A folder smaller than the prefetch window | Wrapping reaches each other image once; none is prepared twice |
| The opened file has an extension the list does not keep | It is shown, but has no position in the list: Next and Previous do nothing |
| The current file is deleted or renamed outside the viewer | After relisting it has no position in the list: it stays on screen, and Next and Previous go on from the place its name sorted at (Next shows the image after it, Previous the one before) |
| The current file is rewritten in place | After relisting its new size or write time gives it a new cache id: the new version loads, the old one stays on screen until it is there; neighbours that changed load anew |
| The folder changes while a listing is still running | Once that listing is in, the folder is listed once more |
| The user opens a cloud placeholder directly | It is read (Windows downloads it); only prefetching skips placeholders |
| A network drive | Detected (`DriveInfo::remote`) but treated like a local SSD: 2 I/O threads, normal prefetch |
| A file is opened later from another drive | The I/O thread count chosen for the startup file stays |
| Every texture is protected and over budget | Nothing is released; the viewer stays over budget |
| A decoder panics while decoding animation frames | Not contained by `guard`: the worker thread ends, that image's animation never arrives, the viewer keeps running with one worker less |
| Linux: a second launch without a file while a viewer runs | It asks the running window to show itself and exits (Windows opens a second window unless resident) |
| Linux: the previous instance crashed and left its socket | The next instance cannot connect, starts normally and replaces the socket |
| Linux: `XDG_RUNTIME_DIR` is unset | No single instance: every launch opens its own window |
| macOS: "Open images in the same window" is turned off | No effect: macOS delivers every opened file to the running app |
| A file cannot be read (permissions, gone) | Shown as "Could not open “{name}”: {reason}" like a decode error |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `numbers_compare_numerically`, `case_insensitive_with_stable_tiebreak`, `symbols_before_digits_before_letters` (`crates/blinzel-core/src/natsort.rs`; Linux, macOS, Windows) | built | |
| AC-02 | `filters_sorts_and_navigates` (`crates/blinzel-core/src/filelist.rs`); `lists_images_only_and_reads_them` (`crates/blinzel-ui/src/files.rs`, the Linux/macOS listing) | built | |
| AC-03 | `an_opened_file_shows_before_its_folder_is_listed_and_keys_wait_for_the_listing` (`crates/blinzel-ui/src/app_browsing_tests.rs`); `filters_sorts_and_navigates` (`crates/blinzel-core/src/filelist.rs`, the case-insensitive `index_of`) | built | |
| AC-04 | `navigation_keys_step_jump_wrap_and_pan_a_zoomed_image` (`crates/blinzel-ui/src/app_browsing_tests.rs`) | built | |
| AC-05 | `filters_sorts_and_navigates` (`crates/blinzel-core/src/filelist.rs`, `offset` wraps both ways and by 7 steps) | built | |
| AC-06 | `the_neighbours_are_wanted_in_browsing_direction_four_ahead_and_two_behind`, `neighbours_are_read_most_urgent_first_once_and_failed_ones_are_skipped` (`crates/blinzel-ui/src/app_browsing_tests.rs`) | built | |
| AC-07 | `the_settings_file_sets_how_many_are_prepared_ahead_and_behind` (`crates/blinzel-ui/src/app_browsing_tests.rs`); `tolerates_garbage`, `unknown_keys_and_invalid_lines_are_ignored_and_valid_lines_apply` (`crates/blinzel-core/src/settings.rs`, the cap at 32 and unreadable values) | built | |
| AC-08 | `runs_by_priority`, `replacing_wanted_drops_and_reprioritizes`, `running_jobs_get_cancelled`, `completed_jobs_are_not_repeated_until_forgotten`, `classes_are_independent` (`crates/blinzel-core/src/sched.rs`) | built | |
| AC-09 | `one_read_gives_the_preview_and_the_screen_stage_and_the_current_image_gets_every_thread` (`crates/blinzel-ui/src/app_browsing_tests.rs`) | built | |
| AC-10 | `zooming_past_the_screen_stage_prepares_the_full_resolution`, `actual_size_prepares_everything_at_full_resolution` (`crates/blinzel-ui/src/app_browsing_tests.rs`) | built | |
| AC-11 | `tiles_cover_the_image_with_borders` (`crates/blinzel-ui/src/gfx.rs`); `ycc_subsamplings_with_tile_seams` (`crates/blinzel-app/tests/wgpu_golden.rs`, lavapipe, Linux CI with `BLINZEL_REQUIRE_GPU=1`; also `warp_golden.rs` in Windows CI and `metal_golden.rs` in macOS CI) | built | |
| AC-12 | `evicts_least_recently_used_except_kept` (`crates/blinzel-core/src/cache.rs`) | built | |
| AC-13 | `the_wanted_window_stays_on_the_gpu_and_a_released_image_is_prepared_again` (`crates/blinzel-ui/src/app_browsing_tests.rs`); `texture_budget_is_half_the_os_budget_clamped`, `texture_budget_falls_back_to_the_adapter_memory` (`crates/blinzel-app/src/gfx/device.rs`, Windows CI `cargo test --workspace --release`); `the_texture_budget_is_one_gib` (`crates/blinzel-app/tests/wgpu_golden.rs`, lavapipe, Linux CI with `BLINZEL_REQUIRE_GPU=1`); `the_texture_budget_is_a_third_of_the_working_set` (`crates/blinzel-app/tests/metal_golden.rs`, macOS CI with `BLINZEL_REQUIRE_GPU=1`) | built | |
| AC-14 | `a_changed_folder_is_listed_again_and_the_current_image_stays`, `after_the_current_file_left_the_folder_next_and_previous_go_on_from_its_place`, `the_current_file_rewritten_in_place_is_loaded_anew`, `a_change_during_the_listing_is_listed_afterwards` (`crates/blinzel-ui/src/app_browsing_tests.rs`); `folder_changes_settle_for_150_ms_before_relisting` (`crates/blinzel-app/src/linux/shell.rs`) | built | |
| AC-15 | `a_setting_changed_in_the_menu_writes_the_file_at_once_with_all_settings`, `each_check_item_is_checked_when_its_setting_is_on`, `menu_settings_offers_each_platforms_entries` (`crates/blinzel-ui/src/app_settings_tests.rs`); `a_missing_file_means_the_documented_defaults` (`crates/blinzel-core/src/settings.rs`, on by default); `a_later_launch_hands_its_file_to_the_running_window`, `the_socket_of_a_crashed_instance_is_replaced`, `without_a_runtime_folder_there_is_no_socket` (`crates/blinzel-app/src/linux/instance.rs`); `same_window_follows_the_setting`, `a_handed_over_file_opens_and_the_window_comes_to_the_front` (`crates/blinzel-app/src/linux/shell.rs`); `a_launch_goes_to_a_running_window_only_in_the_same_window_mode` (`crates/blinzel-app/src/instance.rs`, `cfg(windows)`, Windows CI `cargo test --workspace --release`) | built | |
| AC-16 | `decodes_requests` (`crates/blinzel-app/src/instance.rs`, `cfg(windows)`, Windows CI `cargo test --workspace --release`) | built | |
| AC-17 | `cloud_placeholders_are_not_read_ahead_but_load_when_navigated_to` (`crates/blinzel-ui/src/app_browsing_tests.rs`); `remove_and_placeholders` (`crates/blinzel-core/src/filelist.rs`); both also run in the Windows CI job (`cargo test --workspace --release`) | built | |
| AC-18 | | open | |
| AC-19 | `panics_are_contained` (`crates/blinzel-core/src/decode/mod.rs`) | built | |
| AC-20 | `a_broken_file_is_not_tried_again_until_it_changes_and_browsing_goes_on` (`crates/blinzel-ui/src/app_browsing_tests.rs`); `a_broken_image_shows_why_instead_of_the_previous_one` (`crates/blinzel-ui/src/app_viewing_tests.rs`, the message) | built | |
| AC-21 | manual: measurement | open | |

## Decisions

- AC-03 and AC-04 held before their tests were written (the tests were green at once): the listing, the keys and wrapping worked; the tests pin them in `App`.
- No shipped language is right to left (`shipped_languages_are_left_to_right`), so AC-04's swap is tested on `nav_cmd`, the key-to-command mapping `App::key` calls with `blinzel_i18n::is_rtl()`.
- The three defects of the extraction are fixed with a red test each (AC-14): when the current file leaves the folder, Next and Previous go on from where its name sorts (`App::current_left`), only for a file that was in the list, so a file whose extension the list does not keep still has no neighbours; a current file whose cache id changed in the new listing is loaded anew; a change during a listing lists the folder once more after it.
- AC-14's 150 ms settle lives in the shells: the Linux test checks `DIR_SETTLE`; the macOS `DIR_SETTLE` and the Windows `DirWatcher` sleep are the same 150 ms without a test of their own.
- AC-06, AC-07, AC-10 and AC-17 held before their tests were written: the tests read what `App` wants prepared (`App::wanted_jobs`, split from `update_wanted` for that) and what the loader read, and each went red when the code under it was broken on purpose (priorities, previews, direction, the 1.05 threshold, the full-resolution neighbours, skipping placeholders and failed files).
- AC-17 is proven by the App test with the attributes the Windows listing reports; Linux and macOS never report placeholders. The test runs in the Linux and the Windows CI job.
- AC-09 and AC-20 held before their tests were written. The harness's probe reports an embedded preview where a test puts one (a JPEG appended to a larger JPEG, or garbage for a broken preview), and its decoder logs the threads each decode gets. That a failed file is not read again is held twice: `App::failed` and the scheduler, which does not repeat a completed job (AC-08); breaking only one of them keeps the test green.
- AC-13 held before its tests were written. The App test runs with a fake device whose budget fits one image (`Harness::with_budget`), so everything outside the protected window is released; it went red when the window was narrowed to the smaller of ahead and behind, when only the current image was kept, and when released keys were not handed to `Loader::forget` (then a released image is never read again). The Windows budget moved into a pure function (`gfx/device.rs` `texture_budget`) so its halving, fallback and clamp are tested without a GPU; the test only compiles on Windows, so its first Windows CI run is its proof (a standalone `rustc --test` copy was green here, and red for each of three mutations). The Linux budget test ran green on lavapipe here and red with a changed budget; the macOS test reads `recommendedMaxWorkingSetSize` from the real Metal device, so its first macOS CI run is its proof.
- AC-15 held before its tests were written. The setting, its menu entry and its immediate save already had App tests (`app_settings_tests.rs`); the new tests cover the shells. The Linux socket functions take the socket's path (`hand_off_to`, `listen_on`, `socket_in`) so the tests use their own folder instead of changing `XDG_RUNTIME_DIR` for the whole test process; the shell's decision (`same_window`) and what a hand-over does (`handed_over`: open, then `Action::Activate`) moved into functions so they are tested without a window. Each test went red when the code under it was broken on purpose (the stale socket not removed, the path not made absolute, an empty line not meaning "show", an empty runtime folder accepted, the setting ignored, no `Activate`). The Windows choice of the receiving instance moved into `pick` (`instance.rs`); its test only compiles on Windows, so its first Windows CI run is its proof (a standalone copy was green here and red when the resident branch was inverted). The Windows receiving side (`window.rs`: `App::open`, then `Action::Activate` for a visible window) has no test of its own.
