# Feature specs

One feature spec per feature, with testable acceptance criteria (ACs) and the tests that prove them. Written with `/spec`, built with `/spec-build`, accepted with `/spec-check` (`.claude/rules/specs.md`). The goal, the decisions, the architecture and the measurements they build on are in the design spec [`blinzel.md`](blinzel.md).

All specs below were extracted from the code on 2026-11-02 (`/spec-extract`). They describe what the code does today. An AC is **proven** when an existing test checks exactly that behavior. Tests marked `CI: windows` / `CI: macos` run only in that CI job, and `manual` ACs need a person on that platform. None of them is `implemented` yet: every spec still has ACs without a test. `/spec-build <name>` writes the missing tests (and fixes what they find), and `/spec-check <name>` then accepts the spec.

## Order

The first three carry decisions of 2026-11-02 that change behavior, so they come first. The rest are in the order of the product: viewing first, distribution last.

| # | Spec | ACs proven | CI-only | Manual | Next |
| - | ---- | ---------- | ------- | ------ | ---- |
| 1 | [selection-and-export](selection-and-export.md) | 17 / 22 | 2 | 2 | `/spec-build`: the clipboard block is left (AC-09 Windows, AC-10 Linux, AC-11 macOS); then the manual checks AC-04, AC-15 |
| 2 | [thumbnails](thumbnails.md) | 20 / 23 | 1 | 3 | the manual checks AC-20 (tiles at 100 % and 200 %), AC-21, AC-22 (measurements), then `/spec-check` |
| 3 | [settings](settings.md) | 15 / 16 | 1 | 0 | push for the Windows CI run of AC-14 (`tests/placement.rs`), then `/spec-check` |
| 4 | [startup](startup.md) | 3 / 14 | 6 | 3 | `/spec-build` |
| 5 | [browsing](browsing.md) | 8 / 21 | 3 | 1 | `/spec-build` |
| 6 | [viewing](viewing.md) | 7 / 22 | 0 | 2 | `/spec-build` |
| 7 | [image-formats](image-formats.md) | 16 / 24 | 4 | 0 | `/spec-build` |
| 8 | [jpeg-decoder](jpeg-decoder.md) | 10 / 17 | 0 | 0 | `/spec-build` |
| 9 | [animations](animations.md) | 4 / 14 | 0 | 1 | `/spec-build` |
| 10 | [color-management](color-management.md) | 10 / 19 | 3 | 2 | `/spec-build` |
| 11 | [batch-resize](batch-resize.md) | 18 / 28 | 1 | 2 | `/spec-build` |
| 12 | [localization](localization.md) | 7 / 17 | 1 | 0 | `/spec-build` |
| 13 | [platform-shells](platform-shells.md) | 8 / 24 | 11 | 5 | `/spec-build` |
| 14 | [resident-mode](resident-mode.md) | 3 / 23 | 13 | 0 | `/spec-build` |
| 15 | [screenshots](screenshots.md) | 8 / 23 | 8 | 1 | `/spec-build` |
| 16 | [installation](installation.md) | 11 / 21 | 7 | 3 | `/spec-build` |
| 17 | [updates](updates.md) | 9 / 23 | 2 | 1 | `/spec-build` |
| 18 | [release](release.md) | 5 / 18 | 9 | 3 | `/spec-build` |
| 19 | [benchmark-gate](benchmark-gate.md) | 5 / 19 | 6 | 0 | `/spec-build` |
| 20 | [competitive-benchmark](competitive-benchmark.md) | 5 / 17 | 3 | 5 | `/spec-build` |
| 21 | [website](website.md) | 12 / 21 | 0 | 3 | `/spec-build` |
| | **Total** | **201 / 426** | | | |

Why so many ACs are open: `blinzel-ui`'s `App`, which holds the keys, commands, menus and the loader's decisions, has no headless tests yet. A small test harness for `App` (a fake `Backend`) would prove a large share of the open ACs in viewing, browsing, thumbnails, animations, localization and settings at once.

## Done

| Spec | Completed |
| ---- | --------- |

## Defects found during the extraction

Behavior the code has today that contradicts the design or is plainly wrong. Each is recorded as an edge case in its spec; `/spec-build` turns it into a red test first.

| Defect | Spec |
| ------ | ---- |
| Animation decoding (GIF, WebP, JPEG XL) runs without `catch_unwind`: a panic ends that worker thread (AGENTS.md §9) | [animations](animations.md) |
| A decode error in the middle of an animation drops its loop count: the frames so far loop for ever | [animations](animations.md) |
| Linux (missing or broken first file) and macOS (broken first file) never show the window: there is no 400 ms fallback at a cold start | [startup](startup.md) |
| `--exit-after-first-frame` with an undecodable file never exits, and the Windows CI step has no timeout | [startup](startup.md) |
| When the current file is deleted outside the viewer, next/previous stop working; a file rewritten in place is not reloaded; a folder change during the listing is dropped | [browsing](browsing.md) |
| The thumbnail cache key lowercases the path on every platform: `A.jpg` and `a.jpg` share an entry on Linux and macOS | [thumbnails](thumbnails.md) |
| An AVIF whose `ftyp` major brand is `mif1` is taken for HEIF and never decoded by rav1d | [image-formats](image-formats.md) |
| A GIF with a 0×0 logical screen is rejected by the probe although the decoder could show it | [image-formats](image-formats.md) |
| On macOS, files dropped onto the batch dialog open the first image instead: the batch list cannot be filled there | [batch-resize](batch-resize.md) |
| A macOS `--resident` start with the setting off leaves the LaunchAgent in place (Windows removes its Run value) | [resident-mode](resident-mode.md) |

## Backlog

Gaps not yet written as specs: planned in the design spec, not in the code.

| Gap | Design spec |
| --- | ----------- |
| Linux screenshots (X11 GetImage, Wayland portal), HDR screenshots, measuring Print key → overlay | §12 M12 |
| Thumbnail metric in `blinzel-race`; comparison of batch processing with vipsthumbnail, XnConvert and ImageMagick in the tooling | §11, §7 |
| A speed budget in CI: today no M1/M2/M4 target is asserted anywhere, only measured by hand | §12 |
| Yardsticks for the system decoders (WIC) and the Windows thumbnail cache in `blinzel-bench`; the gate of HEIC and AVIF against WIC | §3 |
| Changelog page and legal notice (Impressum) on the website; startup and memory columns in the feature table | §11 |
| winget: a PR after every release (manual today, `packaging/winget/README.md`) | §10 |
| Remembering the window position on Linux and macOS (Windows only today) | §6 |

## Open items outside the code

Things only a person can do.

| Item | How |
| ---- | --- |
| Mac signing certificate | `bash packaging/macos/create-signing-cert.sh` once ([docs/SECRETS.md](../SECRETS.md)). Until then releases are signed ad hoc. After the next release the macOS job's "Signing identity" step must not say `MACOS_SIGNING_CERT is not set`. |
| Comparison measurement on a Mac (Apple silicon) | `blinzel-race corpus testbilder`, then `blinzel-race run testbilder --cold --json website/data/race-macos.json` (asks once for the `sudo` password). Install the viewers first and give the terminal "Screen Recording" and "Accessibility". |
| Gate of HEIC and AVIF against WIC on Windows | `set BLINZEL_DECODER=native` or `=wic`, then `blinzel.exe --bench <folder>` with 12/24 MP files, alternately. |
| Legal notice (Impressum) | Name, address and contact of the website operator, if the site is operated commercially. |
| First manual test on Windows 11, a Mac and Linux | The `manual` ACs of the specs above; the check lists per platform are in [blinzel.md](blinzel.md) §13. |
| Custom domain for the website (optional) | Cloudflare Workers Builds is connected; the website already uses `blinzel.amigo-labs.dev` in its canonical URLs. |
