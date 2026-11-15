---
status: todo
created: 2026-11-15
size: M
platforms: all
design: [blinzel.md §2, blinzel.md §4]
---

# Shared amigo theme (dark palette)

## Context

All amigo-labs apps share one theme: the same surfaces, text colours, status colours, radii and control sizes, and one accent per app, the brand colour from `docs/site/project.yml`. The theme is kept in amigo-labs/website (`theme/`) and generated into each app; the site-data workflow fails when an app's copy is stale. Pincel, Krake and Sofarr take the CSS output. Blinzel draws its own interface, so it takes the Rust output (`scripts/theme.ts --rust`): a module with the palettes `DARK` and `LIGHT`, text styles, spacing, control sizes and radii.

Today Blinzel's interface colours are constants spread over `blinzel-ui` (`widgets.rs`, `popup.rs`, `browse.rs`) and the Windows setup window (`blinzel-app/src/setup/ui.rs`): neutral greys built from white with alpha, and a blue accent that has nothing to do with the brand red `#ff4757`.

This spec moves those colours, radii and control sizes onto the shared theme's dark palette, the default of every amigo app. The light palette and a Dark / Light / System choice are a second spec (backlog): they need a setting, a menu entry and the system's appearance on three platforms.

## Scope

### In scope

- The generated theme module in `blinzel-ui`, checked by the site-data workflow
- Dialogs, panels, controls (button, checkbox, segmented control, field, slider, progress), the popup menu drawn by the interface, filmstrip and grid highlights, the selection frame: colours from the dark palette
- The accent: brand red in its theme roles instead of blue
- Radii and control height from the theme
- The Windows setup window: the same palette
- The default of the `background` setting (the area around the image)

### Out of scope

- The light palette, a theme setting and following the system — because that is its own feature (backlog "Light palette and theme choice")
- Fonts — because the interface draws with the system's UI font, and loading a bundled font costs startup time (`blinzel.md` §5); the theme's text sizes are used, its font family is not
- The native menus (`TrackPopupMenuEx` on Windows, `NSMenu` on macOS) — because the system draws them
- The window buttons (minimize, maximize, close) and the close button's red hover — because they follow each platform's convention
- Everything drawn over the picture itself (scrims, halos around glyphs, the white glyphs and messages on a dark pill, the screenshot overlay's dimming, the loupe) — because it must read on any picture, not on a theme surface
- The picture and thumbnails — because image pixels are never tinted
- An existing `settings.toml` — because the file stores `background`, so a user who has one keeps their colour

## Acceptance criteria

- [ ] AC-01 (always): `crates/blinzel-ui/src/theme.rs` is the Rust module amigo-labs/website generates for Blinzel's accent (`node scripts/theme.ts amigo-labs/amigo-blinzel --rust …`), unedited, and `cargo fmt --all --check` passes with it.
- [ ] AC-02 (always, CI: site-data): The site-data workflow passes `theme-rust: crates/blinzel-ui/src/theme.rs` to the website's `validate-project` workflow and runs on pull requests that touch that file, so a stale or edited copy fails the check.
- [ ] AC-03 (always): Every interface colour of `blinzel-ui` outside the cases under "Out of scope" is derived from `theme::DARK` through one conversion from `0xRRGGBB` to `Rgba`; a test over the sources of `blinzel-ui` finds no `Rgba(` literal with a colour other than black or white outside `gfx.rs`, the theme adapter and tests.
- [ ] AC-04 (always): Dialogs and panels are filled with `surface`, the interface's popup menu with `surface_raised`, both outlined with `border`; primary text is `text`, secondary text `text_secondary`, disabled and hint text `text_muted`.
- [ ] AC-05 (always): A control at rest is filled with `surface_raised`, under the pointer with `hover`, and outlined with `border`; a focused field is outlined with `focus`, an invalid field with `danger`, and error text is `danger_ink`.
- [ ] AC-06 (always): A filled accent control (primary button, checked checkbox, the selected segment, the filled part of a slider and of a progress bar) is filled with `accent_solid`, and text or a glyph on it is `on_accent`.
- [ ] AC-07 (always): Accent lines and marks (the current thumbnail's frame in filmstrip and grid, the selection frame, the loupe's centre pixel) are `accent`; a marked thumbnail's tint is `accent` with the alpha it has today.
- [ ] AC-08 (always): No interface colour is the former blue (`Rgba(0.35, 0.62, 1.0, …)`, `Rgba(0.0, 0.47, 0.84, …)`) any more.
- [ ] AC-09 (always): Controls have the radius `RADIUS_SM` (6), dialogs, panels and the popup menu `RADIUS_LG` (12); a standard control is `CONTROL_MD` (32) high; dialog titles use the size of `TEXT_TITLE` (20), body text `TEXT_BODY_SM` (13), small text `TEXT_CAPTION` (12).
- [ ] AC-10 (always): With the dark palette, text on its ground has a contrast of at least 4.5 : 1 for `text`, `text_secondary`, `text_muted` and `danger_ink` on `surface` and on `surface_raised`, and for `on_accent` on `accent_solid`; `accent` and `focus` have at least 3 : 1 on `surface`.
- [ ] AC-11 (always): If no settings file exists, the area around the image is the theme's `bg` (`background = "#15161A"`); a file that names a `background` keeps it.
- [ ] AC-12 (always, CI: windows): The Windows setup window uses the same palette: `surface` ground, `text` and `text_secondary`, a primary button in `accent_solid` with `on_accent` text (lighter under the pointer), secondary buttons in `surface_raised` / `hover`.
- [ ] AC-13 (always): The CPU reference renderer and the GPU golden tests still pass: the theme changes colours handed to the renderer, not the renderer.
- [ ] AC-14 (always, manual): Dialogs (batch resize, About, the update question), the popup menu, filmstrip, grid and the selection frame look right in the running viewer: readable, the accent red, no leftover blue or grey from before.

## Technical notes

- `theme.rs` is generated (`node scripts/theme.ts ../amigo-blinzel/docs/site/project.yml --rust ../amigo-blinzel/crates/blinzel-ui/src/theme.rs` in a checkout of amigo-labs/website; `--out` needs a folder, a scratch one will do) and never edited. Its `f32` sizes are converted where the interface works in `f64`.
- The adapter belongs next to the widgets (`widgets.rs`, or a small `palette.rs`): `const fn rgb(hex: u32) -> Rgba` and named constants for the roles the interface uses, so call sites keep reading `widgets::PANEL`, `FG`, `DIM`, `FAINT`, `CONTROL`, `CONTROL_HOVER`, `ACCENT`, `DANGER` where that is the smaller change. `popup.rs` and `browse.rs` use the same constants instead of their own copies.
- The former neutrals were white with alpha over whatever lay behind; the theme's are opaque. Panels keep the slight transparency they have today only if a test or the look needs it; otherwise they are opaque.
- `blinzel-app` reaches the theme through `blinzel-ui` (`blinzel_ui::theme`); `setup/ui.rs` builds only on Windows (`cargo xwin check`, or the cross-target type-check from the Gotchas).
- The `background` default is `blinzel_core::settings::Settings::default`; `blinzel-core` does not depend on `blinzel-ui`, so the value is written there as a literal and a test in `blinzel-ui` asserts that it equals `theme::DARK.bg`.
- Contrast (AC-10) is computed in a test with the WCAG formula from the palette's values.
- The site-data workflow's `paths` gain `crates/blinzel-ui/src/theme.rs`.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| `settings.toml` with `background = "#1E1E1E"` (written by an older version) | That colour stays |
| A disabled primary button | `surface_raised` with `text_muted`, not a faded accent |
| A checked but disabled checkbox | The check stays visible in `text_muted` on `surface_raised` |
| Theme regenerated with another accent in `project.yml` | Only `theme.rs` changes; no other source names a colour |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 |      | open   |        |
| AC-02 |      | open   |        |
| AC-03 |      | open   |        |
| AC-04 |      | open   |        |
| AC-05 |      | open   |        |
| AC-06 |      | open   |        |
| AC-07 |      | open   |        |
| AC-08 |      | open   |        |
| AC-09 |      | open   |        |
| AC-10 |      | open   |        |
| AC-11 |      | open   |        |
| AC-12 |      | open   |        |
| AC-13 |      | open   |        |
| AC-14 |      | open   |        |

## Decisions

- Dark palette first, light and the theme choice in a second spec: the dark palette is a change of constants; light needs a setting, a menu entry and each platform's appearance signal.
- The system's UI font stays (no bundled Inter): startup time is the product (`blinzel.md` §5).
- Elements drawn over the picture keep their black and white with alpha: they sit on arbitrary pixels, where a theme surface colour guarantees nothing.
- The `background` default follows the theme's `bg`, but a stored value wins: the file has always written every key, so existing users keep what they see.
