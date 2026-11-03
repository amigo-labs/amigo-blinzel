---
status: todo
created: 2026-11-02
size: L
platforms: all
design: [blinzel.md §2, blinzel.md §9, blinzel.md §10, blinzel.md §12 (M7, M10, M11), blinzel.md §13]
---

# Self-updater

## Context

Installed copies update themselves silently in the background (`blinzel.md` §2, "Updates"): at most once a day they ask GitHub for the latest release, download this platform's build, verify it and put it in place, active from the next start. Releases carry no code signing certificate from a CA (`blinzel.md` §2, "Signature"), so the project's own Ed25519 signature of `SHA256SUMS` is the only proof of origin. The decisions (versions, release JSON, checksums, signature, schedule, start counter) are platform-independent in `blinzel-core::update`; each shell downloads and swaps files its own way. This spec records what the code does today (reverse engineered on 2026-11-02).

## Scope

### In scope

- The automatic check, its schedule and the `auto_update` setting
- "Check for updates" in the menu
- Choosing the release and the asset for this platform and architecture
- Verifying the signature and the checksum
- Putting the update in place on Windows, Linux and macOS, and the rollback

### Out of scope

- How a resident instance hands over to an installed update (`restart_pending`, `start_installed_resident`) — because resident mode has its own spec
- Producing and signing the release files — because that is release automation (`docs/specs/release.md`)
- Pre-releases and release channels — because `Version::parse` rejects any tag other than `x.y.z` and drafts and pre-releases are skipped
- Delta updates — because the download is always the whole executable or bundle

## Acceptance criteria

### Choosing a release

- [ ] AC-01 (always): A release is offered only when it is neither a draft nor a pre-release, its tag is `x.y.z` or `vx.y.z` with three numeric parts, and it has this platform's asset plus `SHA256SUMS` and `SHA256SUMS.sig`; otherwise no update is offered.
- [ ] AC-02 (always): Versions compare part by part as numbers (`0.10.0` is newer than `0.9.9`, `1.0.0` newer than `0.99.99`); `1.2`, `1.2.3.4` and `1.2.3-rc.1` are no versions.
- [ ] AC-03 (event): When the release lists setup executables for several architectures, then the Windows updater takes `amigo-blinzel-setup-<arch>.exe` of its own architecture (`x64` or `arm64`), and a release without it offers no update.
- [ ] AC-04 (always): The Linux updater takes the bare executable `amigo-blinzel-linux-<x64|arm64>` and the macOS updater the zipped universal bundle `amigo-blinzel-macos.zip`.
- [ ] AC-05 (event): When the latest release's version is not higher than the running version, then nothing is downloaded and "Check for updates" reports `amigo-blinzel <running version> is up to date.`

### Verification

- [ ] AC-06 (always): `SHA256SUMS` is accepted only with a valid Ed25519 signature of the release key built into the app, the signature given as 64 raw bytes or as 128 hex digits; changed content, garbage or another key's signature is rejected.
- [ ] AC-07 (always): The expected hash is the line `<hex>  <name>` or `<hex> *<name>` for the asset's name in `SHA256SUMS`, and the SHA-256 computed over the download matches the FIPS 180-4 test vectors.
- [ ] AC-08 (error): If the signature does not verify, `SHA256SUMS` has no line for the asset, or the download's SHA-256 differs from it, then nothing is put in place and the check fails with a reason (`checksums are not signed with the release key`, `no checksum for us`, `download does not match its checksum`).
- [ ] AC-09 (always, CI: windows): On Windows the SHA-256 of the download is computed by Windows' CNG (`BCryptHash`) and matches the FIPS 180-2 test vector for `abc`.

### Schedule

- [ ] AC-10 (state): While the last automatic check is less than 24 hours old, no automatic check runs; a last check in the future (clock changed) counts as due.
- [ ] AC-11 (event): When an installed copy starts, then the automatic check runs on a background thread no earlier than 10 s after start, and the time of the check is saved before it contacts GitHub, so a failed check is not repeated within 24 hours.
- [ ] AC-12 (state): While a Windows instance keeps running (resident mode), it re-reads the setting and tests every hour whether a check is due; on Linux and macOS a process tests once, 10 s after start.
- [ ] AC-13 (option): Where `auto_update` is off (menu Settings → "Install updates automatically"), no automatic check runs; the setting is on by default and the menu entry toggles and saves it.
- [ ] AC-14 (state): While the running process is not the installed copy (Windows: not `%LOCALAPPDATA%\Programs\amigo-blinzel\blinzel.exe`; Linux: not the executable `--install` placed; macOS: not inside an `.app` bundle), it never checks automatically, and "Check for updates" reports `Update failed:` with the reason that it is not installed.
- [ ] AC-15 (event): When "Check for updates" is chosen in the menu (macOS also in the application menu), then a check runs at once in the background regardless of the schedule and `auto_update`, and its result appears as a notice: up to date, `Version <v> is downloaded and active from the next start.`, or `Update failed: <reason>`.

### Putting it in place

- [ ] AC-16 (event, CI: windows): When a Windows update is put in place, then the running `blinzel.exe` is renamed to `blinzel.exe.old` and the new executable takes its name; if moving the new file fails, the previous executable is back under its name.
- [ ] AC-17 (event): When a Linux update is put in place, then the previous executable is kept as `blinzel.old` next to the installed one and the new executable takes its name.
- [ ] AC-18 (event): When a macOS update is put in place, then the downloaded zip is unpacked next to the bundle, the running bundle is renamed to `.amigo-blinzel-previous.app` in the same folder and the unpacked `.app` takes its name; a zip without an `.app` fails with `the download holds no app bundle` and leaves the bundle untouched.
- [ ] AC-19 (event): When an update has been put in place, then the running process keeps the previous version, the next start runs the new version, and on Windows the "Installed apps" entry shows the new version once the new version has shown its first frame.

### Rollback

- [ ] AC-20 (state): While a freshly installed version has not shown its first frame, each of its starts is counted; after three such starts the fourth start restores the previous version (keeping the failed one as `.bad`), starts it with the same arguments and exits; a start of any other version, or a first frame, ends the trial.
- [ ] AC-21 (event): When a start finds no update on trial, then the previous version (`blinzel.exe.old`, `blinzel.old` or `.amigo-blinzel-previous.app`) and any `.bad` copy are deleted.
- [ ] AC-22 (always): The updater state (`last_check`, `pending`, `failed_starts`) survives a write and read unchanged, and an unreadable state file counts as empty.
- [ ] AC-23 (event, manual: needs a published release and a clean VM): When an installed v0.x.y is left running on a clean machine with a newer signed release published, then it updates itself without user action and the next start runs the new version.

## Technical notes

- Core: `crates/blinzel-core/src/update.rs` — `RELEASE_KEY` (public key `276b0fbd…a9b7`), `REPO = "amigo-labs/amigo-blinzel"`, `CHECK_INTERVAL_S = 86400`, `MAX_FAILED_STARTS = 3`, `asset_name`, `linux_asset_name`, `macos_asset_name`, `parse_release` (own JSON reader `crate::json`), `sum_for`, `sha256`, `verify_signature` (`ed25519-compact`), `check_due`, `State` (`key = value` text), `on_start` → `StartAction::{Continue, Rollback}`, `on_first_frame`.
- Windows: `crates/blinzel-app/src/update.rs`. HTTPS through WinHTTP loaded with `LoadLibraryW` (`WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, so the system proxy applies), SHA-256 through `bcrypt.dll` `BCryptHash`, both only when a check runs. The download is written to `%LOCALAPPDATA%\amigo-blinzel\updates\<version>\blinzel.exe`, renamed into place, and the folder removed; `SHA256SUMS` and its signature stay in memory. State: `%LOCALAPPDATA%\amigo-blinzel\update.state`.
- Linux and macOS: `crates/blinzel-app/src/selfupdate.rs` (the shared flow and the `Installation` trait), `linux/update.rs`, `macos/update.rs`; downloads through the system `curl` (`crates/blinzel-app/src/curl.rs`: `--fail --location --proto =https --proto-redir =https --max-time 300 --max-filesize`). macOS unpacks with `/usr/bin/ditto -x -k`. State: `<cache>/amigo-blinzel/update.state` (`$XDG_CACHE_HOME` or `~/.cache` on Linux, `~/Library/Caches` on macOS).
- Size limits: release JSON 1 MiB, `SHA256SUMS` 64 KiB, signature 4 KiB, download 64 MiB (Windows) or 128 MiB (Linux, macOS).
- The Linux swap writes the download as `blinzel.new` (mode 0755, synced), keeps the previous executable with a hard link `blinzel.old`, then renames the new file over the installed one.
- Where `RELEASE_KEY` is `None`, no automatic check starts and a manual check fails with `no release signing key in this build`.
- Errors of the automatic check are only logged: `BLINZEL_DEBUG` on Linux and macOS (stderr), `debug_log` on Windows.
- User agent `amigo-blinzel/<version>`; WinHTTP sends `Accept: application/vnd.github+json, application/octet-stream`.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| No network or GitHub unreachable | The check fails quietly; the next automatic try is 24 h later |
| GitHub answers with a status other than 200 (Windows) or curl fails | `HTTP <status> from <host>` / `curl: <message>`; nothing changes |
| A download is larger than its limit | `download too large`; nothing is put in place |
| The new version crashes before its first frame | Three counted starts, then the fourth restores the previous version |
| No previous version to restore (deleted by hand) | The trial ends and the current version keeps running |
| The user replaces the executable by hand while an update is on trial | The differing running version clears the trial |
| A developer or portable build (not installed) | Never checks; "Check for updates" says it is not installed |
| The latest release is a draft or pre-release | No update is offered |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `parse_release_offers_only_complete_published_releases`, `picks_the_assets_for_the_architecture` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-02 | `versions_parse_and_order` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-03 | `picks_the_assets_for_the_architecture` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-04 | `picks_the_linux_and_macos_assets` (`crates/blinzel-core/src/update.rs`), `fetches_the_bare_executable` (`crates/blinzel-app/src/linux/update.rs`), Linux CI; `fetches_the_zipped_bundle` (`crates/blinzel-app/src/macos/update.rs`), macOS CI | built |        |
| AC-05 | `check_downloads_nothing_when_not_newer` (`crates/blinzel-core/src/update.rs`), `a_manual_check_reports_its_result` (`crates/blinzel-app/src/selfupdate.rs`), Linux CI | built |        |
| AC-06 | `verifies_ed25519_signatures` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-07 | `reads_checksum_files`, `sha256_matches_known_digests` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-08 | `check_hands_over_only_a_verified_download` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-09 | `hashes_with_cng` (`crates/blinzel-app/src/update.rs`), Windows CI | built |        |
| AC-10 | `checks_at_most_daily` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-11 | `the_automatic_check_waits_and_saves_its_time_first`, `a_running_instance_looks_every_hour_and_checks_daily` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-12 | `a_running_instance_looks_every_hour_and_checks_daily` (`crates/blinzel-core/src/update.rs`), `a_process_looks_once_ten_seconds_after_start` (`crates/blinzel-app/src/selfupdate.rs`), Linux CI; `a_running_instance_looks_every_hour` (`crates/blinzel-app/src/update.rs`), Windows CI | built |        |
| AC-13 | `auto_update_off_checks_nothing` (`crates/blinzel-core/src/update.rs`), `a_missing_file_means_the_documented_defaults` (`crates/blinzel-core/src/settings.rs`), `install_updates_automatically_is_on_by_default_and_saved_when_toggled` (`crates/blinzel-ui/src/app_settings_tests.rs`), Linux CI | built |        |
| AC-14 | `a_copy_that_is_not_installed_never_checks` (`crates/blinzel-app/src/selfupdate.rs`), `a_copy_outside_the_installation_does_not_update` (`crates/blinzel-app/src/linux/update.rs`), Linux CI; `a_copy_outside_a_bundle_does_not_update` (`crates/blinzel-app/src/macos/update.rs`), macOS CI; `a_copy_outside_the_installation_does_not_update` (`crates/blinzel-app/src/update.rs`), Windows CI | built |        |
| AC-15 | `check_for_updates_asks_the_shell_to_check` (`crates/blinzel-ui/src/app_settings_tests.rs`), `a_manual_check_ignores_the_schedule`, `a_manual_check_reports_its_result` (`crates/blinzel-app/src/selfupdate.rs`), Linux CI; `check_for_updates_is_in_the_application_menu` (`crates/blinzel-app/src/macos/shell.rs`), macOS CI | built |        |
| AC-16 | `swaps_and_restores` (`crates/blinzel-app/src/update.rs`), Windows CI | built |        |
| AC-17 | `swap_keeps_the_previous_executable` (`crates/blinzel-app/src/linux/update.rs`), Linux CI | built |        |
| AC-18 | `the_unpacked_bundle_takes_the_running_bundles_name`, `a_downloaded_zip_is_unpacked_next_to_the_bundle` (`crates/blinzel-app/src/macos/update.rs`), macOS CI | built |        |
| AC-19 | `the_next_start_runs_the_installed_update` (`crates/blinzel-app/src/linux/update.rs`), Linux CI; `a_downloaded_zip_is_unpacked_next_to_the_bundle` (`crates/blinzel-app/src/macos/update.rs`), macOS CI; `installed_apps_shows_the_new_version_after_its_first_frame`, `swaps_and_restores` (`crates/blinzel-app/src/update.rs`), Windows CI | built |        |
| AC-20 | `rolls_back_after_three_failed_starts` (`crates/blinzel-core/src/update.rs`), `a_failing_update_is_rolled_back_on_the_fourth_start`, `without_a_previous_executable_the_update_keeps_running` (`crates/blinzel-app/src/linux/update.rs`), Linux CI; `a_rollback_restores_the_previous_bundle` (`crates/blinzel-app/src/macos/update.rs`), macOS CI; `a_failing_update_is_rolled_back_on_the_fourth_start` (`crates/blinzel-app/src/update.rs`), Windows CI | built |        |
| AC-21 | `a_start_without_a_trial_deletes_the_previous_copies` (`crates/blinzel-app/src/linux/update.rs`), Linux CI; `forgetting_deletes_the_previous_and_bad_bundles` (`crates/blinzel-app/src/macos/update.rs`), macOS CI; `a_start_without_a_trial_deletes_the_previous_copies` (`crates/blinzel-app/src/update.rs`), Windows CI | built |        |
| AC-22 | `state_round_trips` (`crates/blinzel-core/src/update.rs`), Linux CI | built |        |
| AC-23 |      | open   |        |

## Decisions

- **The check flow moved into `blinzel-core::update::check`** (new public API: `check`, `Found`, `latest_release_url`). Windows and the Linux/macOS flow had the same sequence twice (release JSON, version, signature, checksum line, hash), untestable behind their downloaders; `check` takes the shell's HTTPS GET and SHA-256 as closures, so AC-05 and AC-08 are tested once on Linux and Windows keeps hashing with CNG (AC-09). `blinzel.md` §10 already puts these decisions in `blinzel-core::update`.
- **Block 1 tests were green at once** (AC-01, AC-04, AC-05, AC-08): the spec was reverse engineered from working code. Each test was checked by breaking the code on purpose (`<=` to `<`, the draft check, the signature check, the hash comparison, the `.sig` lookup, the macOS asset name, the notice text, the Linux asset) and went red each time.
- **AC-04 on macOS:** `fetches_the_zipped_bundle` runs only in the macOS CI job (`cargo test -p blinzel-app`); the name itself is tested on Linux in `picks_the_linux_and_macos_assets`.
- **The schedule moved into `blinzel-core::update::auto_checks`** (new public API: `auto_checks`, the trait `AutoCheck`, `FIRST_CHECK_DELAY_S`, `RECHECK_S`). The shells had the same thread body twice (wait, read the setting, test `check_due`, save the time, check); the shell now only supplies sleep, clock, setting, state file and the check, and says whether it repeats (Windows hourly, Linux and macOS once). `AutoCheck::sleep` returns whether to go on, so a test can end the hourly loop; the shells always go on.
- **`Installation::fetch`** (default: `curl::get`) lets a test serve the downloads of a manual check (AC-15) without the network; `selfupdate::schedule` returns whether it started the check, so AC-14 tests that a copy that is not installed starts none.
- **AC-11's background thread is read from the code, not tested:** starting the real `schedule` in a test would read the real settings file after 10 s and contact GitHub. The test proves the order (10 s wait, setting, saved time, check); `schedule` runs it on the thread `blinzel-update`.
- **Block 2 tests were green at once** (AC-11..AC-15), like block 1; each went red with the code broken on purpose (save after check, no first wait, repeating on Linux, setting not read, `check_due` skipped, schedule started without an installation, a manual check gated by the schedule, the toggle not saved, `CheckUpdates` without its action, the Linux installation check). The Windows tests (`a_running_instance_looks_every_hour`, `a_copy_outside_the_installation_does_not_update`) and the macOS tests (`a_copy_outside_a_bundle_does_not_update`, `check_for_updates_is_in_the_application_menu`) were type-checked and clippy-checked locally for their targets; their first CI run is the proof.
- **The start-up half of the rollback became testable** (AC-19..AC-21): `selfupdate::start` (Linux and macOS) and `update::start` (Windows) take the running version, the arguments and a `spawn` for the restart, so a test runs the counted starts, the rollback and the clean-up on a real installation in a temporary folder and reads the restart command (program and arguments) without starting it; `on_start` passes `std::env::args_os()` and spawns. Windows' `on_first_frame` hands its work to `first_frame(&Layout)`, tested against a private HKCU branch like `tests/setup.rs`. macOS's swap is `put_in_place` (the unpacking with `ditto` stays in `install`).
- **macOS tests call `rollback` and `forget_previous` directly**, not `selfupdate::start`: its `state_file` is the real `~/Library/Caches` folder, and the shared `start` is already tested on Linux with the same code.
- **Block 3 tests were green at once** (AC-18..AC-21), like blocks 1 and 2; the Linux tests each went red with the code broken on purpose (the arguments not passed on, no restart, no exit after the rollback, no clean-up, clean-up during the trial, the counted start not saved, the rollback's state not saved, no `.bad` copy, a rollback without a previous executable, `.bad` not deleted, the swap copying over the installed file). The Windows tests (`update.rs`) and the macOS tests (`macos/update.rs`, including a zip packed by the real `/usr/bin/ditto`) were type-checked and clippy-checked locally for their targets; their first CI run is the proof.
