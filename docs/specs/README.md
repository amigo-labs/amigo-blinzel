# Feature specs

One feature spec per feature, with testable acceptance criteria (ACs) and the tests that prove them. Written with `/spec`, built with `/spec-build`, accepted with `/spec-check` (AGENTS.md, "Spec-driven workflow"). The goal, the decisions, the architecture and the measurements they build on are in the design spec [`blinzel.md`](blinzel.md).

All specs below were extracted from the code on 2026-11-02 (`/spec-extract`). They describe what the code does today. An AC is **proven** when an existing test checks exactly that behavior. Tests marked `CI: windows` / `CI: macos` run only in that CI job, and `manual` ACs need a person on that platform. Specs that `/spec-check` has accepted move to [Done](#done); every spec in the order below still has ACs without a test. `/spec-build <name>` writes the missing tests (and fixes what they find), and `/spec-check <name>` then accepts the spec.

## Order

The specs that carry decisions of 2026-11-02 that change behavior come first. The rest are in the order of the product: viewing first, distribution last.

| # | Spec | ACs proven | CI-only | Manual | Next |
| - | ---- | ---------- | ------- | ------ | ---- |
| 1 | [selection-and-export](selection-and-export.md) | 20 / 22 | 2 | 2 | the manual checks AC-04 (visual design), AC-15 (save dialog); then `/spec-check` |
| 2 | [thumbnails](thumbnails.md) | 20 / 23 | 1 | 3 | the manual checks AC-20 (tiles at 100 % and 200 %), AC-21, AC-22 (measurements); then `/spec-check` |
| 3 | [startup](startup.md) | 10 / 14 | 6 | 3 | the Windows CI run of AC-11's `the_basic_render_driver_counts_as_warp` (`warp` was false on the Basic Render Driver); the manual checks AC-06 (visual), AC-08 (module-load trace; its Windows test is green in CI run 41532658575), AC-12 (measurement); then `/spec-check` |
| 4 | [browsing](browsing.md) | 20 / 21 | 4 | 1 | the manual check AC-21 (measurement); then `/spec-check` (the Windows and macOS tests of AC-13, AC-15, AC-16, AC-18 are green in CI run 41532658575) |
| 5 | [viewing](viewing.md) | 20 / 22 | 0 | 2 | the manual checks AC-17 (visual design), AC-22 (the "Open with" chooser); then `/spec-check` |
| 7 | [jpeg-decoder](jpeg-decoder.md) | 16 / 17 | 0 | 1 | the manual check AC-17 (speed factor on an 8-core machine, `blinzel-bench jpeg-ab`); then `/spec-check` |
| 8 | [animations](animations.md) | 13 / 14 | 0 | 1 | the manual check AC-14 (playback smoothness); then `/spec-check` |
| 9 | [color-management](color-management.md) | 17 / 19 | 3 | 2 | the manual checks AC-12 (Windows monitors), AC-15 (Mac display); then `/spec-check` |
| 10 | [batch-resize](batch-resize.md) | 26 / 28 | 1 | 2 | the manual checks AC-21 (measurement), AC-28 (visual design); then `/spec-check` |
| 11 | [platform-shells](platform-shells.md) | 19 / 24 | 11 | 5 | the manual checks AC-05, AC-06, AC-13, AC-20, AC-22; then `/spec-check` |
| 12 | [resident-mode](resident-mode.md) | 21 / 23 | 15 | 1 | the Windows CI run of AC-13's tests of the default item "Show" and the re-add after `TaskbarCreated`; the manual check AC-23 (measurement); then `/spec-check` |
| 13 | [screenshots](screenshots.md) | 21 / 23 | 8 | 1 | the Windows CI run of AC-03's notice test (`route_hint`); the manual check AC-21; then `/spec-check` |
| 14 | [installation](installation.md) | 18 / 21 | 12 | 3 | the manual checks AC-10 (setup window), AC-11 (no Direct3D), AC-21 (SmartScreen, Gatekeeper); then `/spec-check` |
| 15 | [updates](updates.md) | 19 / 33 | 4 | 1 | revised 2026-11-11 for the update choice: `/spec-build updates` (AC-12, AC-13, AC-15, AC-24 … AC-33, with `settings` AC-02, AC-10, AC-13, AC-17); the manual check AC-23; then `/spec-check` |
| 15a | [settings](settings.md) | 13 / 17 | 1 | 0 | revised 2026-11-11 with `updates` (the `updates` key, the menu without "Install updates automatically"); built together with `updates`, then `/spec-check` |
| 16 | [release](release.md) | 15 / 18 | 9 | 3 | the manual checks AC-01, AC-02 (release-please on GitHub), AC-14 (`Authority=amigo-labs`, shown in the v1.4.0 release run 41591512319); then `/spec-check` |
| 17 | [competitive-benchmark](competitive-benchmark.md) | 15 / 20 | 3 | 5 | the manual checks AC-07, AC-08, AC-09, AC-12, AC-17 (a real desktop with the viewers installed); then `/spec-check` |
| | **Total** | **303 / 359** | | | |

Why so many ACs are open: `blinzel-ui`'s `App`, which holds the keys, commands, menus and the loader's decisions, has no headless tests yet. A small test harness for `App` (a fake `Backend`) would prove a large share of the open ACs in viewing, browsing, thumbnails, animations, localization and settings at once.

## Done

| Spec | Completed |
| ---- | --------- |
| [localization](localization.md) (17 / 17 ACs proven, AC-10 and AC-16 by the Windows CI job) | 2026-11-02 |
| [benchmark-gate](benchmark-gate.md) (19 / 19 ACs proven, AC-04, AC-05, AC-16 … AC-19 by the Windows CI job) | 2026-11-02 |
| [image-formats](image-formats.md) (24 / 24 ACs proven, AC-16 and AC-23 by the macOS CI job, AC-21 and AC-22 by the Windows CI job) | 2026-11-03 |

## Defects found during the extraction

Behavior the code has today that contradicts the design or is plainly wrong. Each is recorded as an edge case in its spec; `/spec-build` turns it into a red test first.

None open.

## Backlog

Gaps not yet written as specs: planned in the design spec, not in the code.

| Gap | Design spec |
| --- | ----------- |
| Linux screenshots (X11 GetImage, Wayland portal), HDR screenshots, measuring Print key → overlay | §12 M12 |
| Thumbnail metric in `blinzel-race`; comparison of batch processing with vipsthumbnail, XnConvert and ImageMagick in the tooling | §11, §7 |
| A speed budget in CI: today no M1/M2/M4 target is asserted anywhere, only measured by hand | §12 |
| Yardsticks for the system decoders (WIC) and the Windows thumbnail cache in `blinzel-bench`; the gate of HEIC and AVIF against WIC | §3 |
| Startup and memory columns in the feature comparison (`docs/site/project.yml`) | §11 |
| winget: a PR after every release (manual today, `packaging/winget/README.md`) | §10 |
| Remembering the window position on Linux and macOS (Windows only today) | §6 |

## Open items outside the code

Things only a person can do.

| Item | How |
| ---- | --- |
| Comparison measurement on a Mac (Apple silicon) | `blinzel-race corpus testbilder`, then `blinzel-race run testbilder --cold --json docs/race/macos.json` (asks once for the `sudo` password), then `blinzel-race site docs/race/macos.json docs/site/benchmarks/macos.yml` and name the file under `benchmarks` in `docs/site/project.yml`. Install the viewers first and give the terminal "Screen Recording" and "Accessibility". |
| Gate of HEIC and AVIF against WIC on Windows | `set BLINZEL_DECODER=native` or `=wic`, then `blinzel.exe --bench <folder>` with 12/24 MP files, alternately. |
| First manual test on Windows 11, a Mac and Linux | The `manual` ACs of the specs above; the check lists per platform are in [blinzel.md](blinzel.md) §13. |
