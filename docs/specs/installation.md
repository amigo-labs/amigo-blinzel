---
status: todo
created: 2026-11-02
size: L
platforms: Windows, Linux, macOS
design: [blinzel.md §2, blinzel.md §9, blinzel.md §10, blinzel.md §12 (M7, M10, M11), blinzel.md §13]
---

# Installation

## Context

amigo-blinzel has no installer framework (`blinzel.md` §2, "Installer"). On Windows the viewer executable is its own installer and installs per user without admin rights; on Linux `blinzel --install` installs it per user; on macOS people drag an app bundle from a disk image into "Applications". None of the builds carries a code signing certificate from a CA (`blinzel.md` §2, "Signature"). The Mac app is signed with the project's own self-signed certificate, or ad hoc without it. This spec records what the code does today (reverse engineered on 2026-11-02).

## Scope

### In scope

- Windows: setup mode detection, per-user installation and registration below HKCU, Start menu shortcut, the setup window, uninstallation from "Installed apps", silent mode for winget, the Windows version check
- Linux: `blinzel --install` / `--uninstall`, desktop entry, icon, the `~/.local/bin` link
- macOS: the app bundle (`blinzel-release macos-bundle`) and the disk image (`blinzel-release macos-dmg`)

### Out of scope

- The self-updater, which also replaces installed files — because it is its own feature (`docs/specs/updates.md`)
- Building, signing and publishing the release files — because that is release automation (`docs/specs/release.md`)
- The sign-in autostart of resident mode (the HKCU `Run` value, the macOS LaunchAgent), except that uninstalling removes the `Run` value — because resident mode has its own spec
- Making amigo-blinzel the default image viewer automatically — because Windows does not let an app set this itself; on Linux the setup only prints the `xdg-mime` command
- A winget submission workflow — because `packaging/winget/` only holds templates that a person fills and submits (`packaging/winget/README.md`)

## Acceptance criteria

### Windows

- [ ] AC-01 (event, CI: windows): When the executable is started with `--install`, or from a file whose name contains `setup` in any case, then it runs as the installer; with `--uninstall` it runs the uninstaller; `--silent` next to either selects silent mode; any other command line starts the viewer.
- [ ] AC-02 (event, CI: windows): When it installs, then the executable is in the installation folder as `blinzel.exe`, the Start menu holds `amigo-blinzel.lnk`, the ProgID `amigo-blinzel.Image` opens `"<exe>" "%1"`, `.jpg` (like every supported extension) lists that ProgID under `OpenWithProgids`, `RegisteredApplications` names `amigo-blinzel` with its `Capabilities` key, and the "Installed apps" entry holds the version as `DisplayVersion` and `"<exe>" --uninstall` as `UninstallString`.
- [ ] AC-03 (event, CI: windows): When it installs over an existing installation, then the installation succeeds and `DisplayVersion` is the new version.
- [ ] AC-04 (always): The real installation goes to `%LOCALAPPDATA%\Programs\amigo-blinzel\`, the shortcut to the user's Start menu, and every registry entry below `HKEY_CURRENT_USER`; the setup never asks for administrator rights.
- [ ] AC-05 (event): When an installed older version is running while a new setup installs, then the old executable is renamed to `blinzel.exe.old` and the new one is copied in its place.
- [ ] AC-06 (event, CI: windows): When it uninstalls, then the installation folder, the Start menu shortcut, the "Installed apps" entry, the ProgID with its open command, the `OpenWithProgids` values and the resident sign-in `Run` value are gone.
- [ ] AC-07 (event): When the installed `blinzel.exe` is started with `--uninstall`, then it copies itself to `%TEMP%`, starts the copy and exits with 0; the copy waits up to 30 s for it to exit, asks a running resident instance to quit, removes the installation including `%LOCALAPPDATA%\amigo-blinzel\`, and deletes itself afterwards.
- [ ] AC-08 (option): Where `--silent` is set, installing and uninstalling show no window or message box, and installing exits with 0 when installed, 1 when it failed and 2 when Windows is too old.
- [ ] AC-09 (error): If the Windows build is below 26100, then the setup installs nothing and exits with 2 (after a localized message box unless silent), and the viewer started normally shows the same message instead of starting.
- [ ] AC-10 (event, manual: system UI and visual design): When the setup runs interactively, then a frameless dark window in the viewer's look shows the version and target folder with "Install" and "Cancel"; after success it offers "Open Default apps" (the Windows default-apps page for amigo-blinzel) and "Start"; after a failure it shows the error and "Close"; Enter presses the primary button, Esc closes; the exit code is 0 installed, 1 failed, 3 cancelled.
- [ ] AC-11 (error, manual: needs a machine without Direct3D): If the setup window cannot create its Direct3D device, then the same flow runs with message boxes (OK/Cancel to install, then a done or failed message) and the same exit codes.

### Linux

- [ ] AC-12 (event): When `blinzel --install` runs, then the executable is copied to `<data>/amigo-blinzel/blinzel` with mode 0755, `<home>/.local/bin/blinzel` is a symbolic link to it, `<data>/applications/amigo-blinzel.desktop` contains `Exec=<exe> %f` and a `MimeType=` list starting with `image/jpeg;`, and the icon `amigo-blinzel.svg` exists; `<data>` is `$XDG_DATA_HOME`, else `~/.local/share`. Running it again, or from the installed copy itself, succeeds.
- [ ] AC-13 (event): When `blinzel --uninstall` runs, then the installed executable, the link, the desktop entry, the icon and the viewer's cache folder are gone, the file it was installed from is untouched, and the settings folder stays (its path is printed).
- [ ] AC-14 (error): If `~/.local/bin/blinzel` already exists and is not a symbolic link, then installing and uninstalling leave that file unchanged.
- [ ] AC-15 (always): The desktop entry's `Exec` path is written as is when it consists only of ASCII letters, digits and `/._-+`, and otherwise in double quotes with `"`, `` ` ``, `$` and `\` escaped as the Desktop Entry Specification requires.
- [ ] AC-16 (always): The viewer window's Wayland app id and X11 `WM_CLASS` are `amigo-blinzel`, the desktop entry's id and `StartupWMClass`.

### macOS

- [ ] AC-17 (event): When `blinzel-release macos-bundle <out.app> <blinzel>` runs, then `<out.app>` contains `Contents/MacOS/blinzel`, an `Info.plist` with the bundle id `io.github.amigo-labs.blinzel`, the version, document types including `public.heic` and the screen-capture usage text, and `Contents/Resources/amigo-blinzel.icns` whose ten entries are PNGs.
- [ ] AC-18 (event, CI: release): When `macos-bundle` gets one executable per architecture, then the bundle's executable is one universal binary joined with `lipo`.
- [ ] AC-19 (option, CI: macos): Where `BLINZEL_MACOS_SIGN_IDENTITY` holds a non-blank identity, the bundle is signed with it (trimmed); otherwise it is signed ad hoc; either way `codesign --verify` accepts the bundle.
- [ ] AC-20 (event, CI: macos): When `blinzel-release macos-dmg <app> <out.dmg>` runs, then `<out.dmg>` passes `hdiutil verify` and, mounted, holds the app with a valid signature and a symbolic link `Applications`.
- [ ] AC-21 (event, manual: SmartScreen and Gatekeeper): When a downloaded release is started for the first time, then Windows SmartScreen and macOS Gatekeeper warn as the download page describes, and the app starts after it is allowed.

## Technical notes

- Windows code: `crates/blinzel-app/src/setup/` (`mod.rs` mode detection and run, `install.rs` the `Layout` with real or sandbox paths, `registry.rs` HKCU only, `shortcut.rs` `IShellLinkW` with AppUserModelID `amigo-labs.amigo-blinzel`, `ui.rs` the window). `run.rs` checks the setup mode before the Windows version check of the viewer.
- Registered extensions (`install::EXTENSIONS`): `.jpg .jpeg .jpe .jfif .png .gif .bmp .dib .tif .tiff .ico .webp .heic .heif .avif .jxl`. Also written: `Software\Classes\Applications\blinzel.exe` (open command, `SupportedTypes`), `App Paths\blinzel.exe`, `QuietUninstallString` (`--uninstall --silent`), `NoModify`, `NoRepair`, `EstimatedSize`, `URLInfoAbout`. After install and uninstall `SHChangeNotify(SHCNE_ASSOCCHANGED)` (off in the sandbox).
- The application manifest says `requestedExecutionLevel level="asInvoker"`, so Windows' installer detection does not ask for elevation for an exe named `*setup*`.
- The Windows version is read with `RtlGetVersion` (`platform/os.rs`, `MIN_BUILD = 26100`).
- The default-apps button opens `ms-settings:defaultapps?registeredAppUser=amigo-blinzel`.
- Linux code: `crates/blinzel-app/src/linux/install.rs`. MIME types: `image/jpeg image/png image/gif image/bmp image/webp image/avif image/jxl`. Files are written atomically (temp file + rename); the executable is copied to `blinzel.new` and renamed, so a running installed copy keeps its unlinked file. `update-desktop-database -q` refreshes the MIME cache where it exists. The cache folder is `$XDG_CACHE_HOME/amigo-blinzel` or `~/.cache/amigo-blinzel`.
- macOS code: `crates/blinzel-release/src/macos.rs`. `LSMinimumSystemVersion` 14.0, `LSHandlerRank` Alternate, role Viewer. `lipo` and `codesign` run only on macOS; elsewhere the bundle is written without them (that is what the Linux test sees). `macos-dmg` copies the app with `ditto` and writes an HFS+ UDZO image named `amigo-blinzel`; off macOS it fails with `macos-dmg needs hdiutil (macOS)`.
- The setup and Linux strings come from `i18n/*.toml` (`setup_*`, `uninstall_*`, `error_os`); `every_language_has_every_key_and_same_placeholders` (`crates/blinzel-i18n/src/lib.rs`) keeps every language complete.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| The installed `blinzel.exe` runs `--install` itself | Nothing is copied; shortcut and registration are rewritten |
| `--uninstall` run from a copy that is not the installed exe (e.g. the downloaded setup) | Uninstalls directly, without the `%TEMP%` copy |
| Silent uninstall of the installed exe | Exits with 0 as soon as the `%TEMP%` copy is started, before the removal has finished |
| A resident instance is running during uninstall | It is asked to quit first and waited for |
| One registry or file removal fails during uninstall | The others still run; the errors are joined into one message (shown unless silent), exit code 1 |
| `HOME` not set on Linux | `--install` / `--uninstall` print `amigo-blinzel: HOME is not set` and exit with 1 |
| `~/.local/bin` is not on the `PATH` | Installation still succeeds; the app menu and "Open with" work through the desktop entry |
| `macos-bundle` without an executable | Fails with `no executable` |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `detects_modes` (`crates/blinzel-app/src/setup/mod.rs`), Windows CI `cargo test --workspace --release` | built |        |
| AC-02 | `installs_updates_and_uninstalls_in_a_sandbox` (`crates/blinzel-app/tests/setup.rs`), Windows CI | built |        |
| AC-03 | `installs_updates_and_uninstalls_in_a_sandbox` (`crates/blinzel-app/tests/setup.rs`), Windows CI | built |        |
| AC-04 | `the_real_installation_is_per_user_below_hkcu` (`crates/blinzel-app/src/setup/install.rs`), `installs_updates_and_uninstalls_in_a_sandbox` (`crates/blinzel-app/tests/setup.rs`: `reg query` finds the entries below HKCU, none below HKLM), `the_executable_embeds_its_application_manifest` (`crates/blinzel-app/tests/manifest.rs`, `asInvoker`), CI: Windows — written, not run yet | open   |        |
| AC-05 | `a_running_old_version_is_renamed_to_old` (`crates/blinzel-app/src/setup/install.rs`), CI: Windows — written, not run yet | open   |        |
| AC-06 | `installs_updates_and_uninstalls_in_a_sandbox` (`crates/blinzel-app/tests/setup.rs`), Windows CI | built |        |
| AC-07 | `the_installed_exe_hands_the_uninstall_to_a_temp_copy_and_exits_0`, `the_temp_copy_waits_stops_removes_and_then_deletes_itself`, `the_temp_copy_is_deleted_after_it_exits`, `uninstalling_asks_every_instance_to_quit_and_waits_30_s_each` (`crates/blinzel-app/src/setup/mod.rs`), `installs_updates_and_uninstalls_in_a_sandbox` (`crates/blinzel-app/tests/setup.rs`, the cache folder), CI: Windows — written, not run yet | open   |        |
| AC-08 | `a_silent_install_shows_nothing_and_exits_0_or_1`, `an_unsupported_windows_refuses_the_install_with_2`, `the_temp_copy_waits_stops_removes_and_then_deletes_itself`, `a_direct_uninstall_shows_its_error_unless_silent` (`crates/blinzel-app/src/setup/mod.rs`), CI: Windows — written, not run yet | open   |        |
| AC-09 | `an_unsupported_windows_refuses_the_install_with_2`, `the_viewer_refuses_an_old_windows_with_the_setups_message` (`crates/blinzel-app/src/setup/mod.rs`), `windows_11_24h2_and_newer_are_supported` (`crates/blinzel-app/src/platform/os.rs`), CI: Windows — written, not run yet | open   |        |
| AC-10 |      | open   |        |
| AC-11 |      | open   |        |
| AC-12 | `installs_and_uninstalls` (`crates/blinzel-app/src/linux/install.rs`), Linux CI `cargo test --workspace` | built |        |
| AC-13 | `installs_and_uninstalls` (`crates/blinzel-app/src/linux/install.rs`), Linux CI; the settings message is not checked | built |        |
| AC-14 | `leaves_foreign_files_alone` (`crates/blinzel-app/src/linux/install.rs`), Linux CI | built |        |
| AC-15 | `quotes_exec_paths` (`crates/blinzel-app/src/linux/install.rs`), Linux CI | built |        |
| AC-16 |      | open   |        |
| AC-17 | `writes_a_bundle`, `icns_entries_are_pngs` (`crates/blinzel-release/src/macos.rs`), Linux and macOS CI | built |        |
| AC-18 | `release.yml` macOS job prints `lipo -info` but asserts no architecture | open   |        |
| AC-19 | `signs_ad_hoc_without_an_identity` (`crates/blinzel-release/src/macos.rs`), Linux and macOS CI; `ci.yml` macOS step "App bundle and smoke test" (`codesign --verify`, ad hoc) | built |        |
| AC-20 | `ci.yml` macOS step "Disk image" (`hdiutil verify`, `test -L "$mnt/Applications"`, `codesign --verify`) | built |        |
| AC-21 |      | open   |        |

## Decisions

- AC-04, AC-05, AC-07, AC-08, AC-09: the setup is Windows code (`setup/`), so its tests are `cfg(windows)` and run only in the Windows CI job, whose first run is their proof; they type-check for `x86_64-pc-windows-msvc` (clippy clean). Their pure parts ran in a standalone copy on Linux: green as written, and red with the refusal's message shown when silent or not at all, exit code 1 for an old Windows, a silent install exiting 0 after a failure, an interactive one installing without the window, no `--silent` for the `%TEMP%` copy, another copy name, a failed start exiting 0, no wait for the installed executable or 10 s instead of 30 s, the copy deleting itself before the removal, no request to running instances, a failure shown when silent, and `ping -n 1`.
- AC-04: the real paths are `Layout::in_profile(local, roaming)` (`Layout::user` passes `%LOCALAPPDATA%` and `%APPDATA%`), tested with fixed paths; every registry path is relative because `registry` opens only `HKEY_CURRENT_USER`, which the sandbox test checks from outside with `reg query HKCU\…` (and that `HKLM\…` has nothing). Not asking for administrator rights is the manifest's `asInvoker`, which `tests/manifest.rs` checks (`platform-shells.md` AC-02).
- AC-05: the file part of the install is `place_exe(source, exe)`. The test runs a copy of `PING.EXE` as the installed `blinzel.exe`, checks that it cannot be opened for writing, then places the test executable: the running one becomes `blinzel.exe.old`, the new one takes its name. On Linux nothing locks a running file, so only the rename was checked there (red without it).
- AC-07: the installed executable's `--uninstall` is `hand_to_temp(exe, temp_dir, pid, silent, spawn)` (copy to `%TEMP%\amigo-blinzel-uninstall-<pid>.exe`, start it, 0), and the copy's arguments parse back as `UninstallFromTemp` with that pid; the copy's run is `finish_from_temp(parent, silent, wait, stop, uninstall, tell, delete_self)`, whose order the test records: wait 30 s for the parent, ask instances to quit, remove, delete itself. The self-deletion's `cmd` line is `delete_command`. That the removal takes `%LOCALAPPDATA%\amigo-blinzel` is `Layout::in_profile`'s `cache` plus the sandbox test's cache folder, which `install::uninstall` removes.
- AC-08, edge cases: the defect: uninstalling from a copy that is not the installed executable (the downloaded setup) exited with 1 on a failure without showing it, unlike the `%TEMP%` copy. Both now go through `remove(silent, stop, uninstall, tell)`, which shows the joined errors unless silent; the test was red against the old branch (no message) in the standalone copy.
- AC-09: the viewer and the setup refuse an old build through `os::refusal(build)` (the localized `error_os` text, or `None`); the setup's `install_mode` shows it unless silent and returns 2 without calling the install. The viewer's `fatal` message box and its exit code 1 stay untested (`platform-shells.md` AC-01).
