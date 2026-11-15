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
- [ ] AC-05 (always): A control at rest is filled with `surface_raised`, under the pointer with `hover`, and outlined with `border`; a field at rest and an unchecked check box are outlined with `border_strong`; a focused field is outlined with `focus`, an invalid field with `danger`, and error text is `danger_ink`.
- [ ] AC-06 (always): A filled accent control (primary button, checked checkbox, the selected segment, the filled part of a slider and of a progress bar) is filled with `accent_solid`, and text or a glyph on it is `on_accent`.
- [ ] AC-07 (always): Accent lines and marks (the current thumbnail's frame in filmstrip and grid, the selection frame, the loupe's centre pixel) are `accent`; a marked thumbnail's tint is `accent` with the alpha it has today.
- [ ] AC-08 (always): No interface colour is the former blue (`Rgba(0.35, 0.62, 1.0, …)`, `Rgba(0.0, 0.47, 0.84, …)`) any more.
- [ ] AC-09 (always): Controls have the radius `RADIUS_SM` (6), dialogs, panels and the popup menu `RADIUS_LG` (12); a standard control is `CONTROL_MD` (32) high; dialog titles use the size of `TEXT_TITLE` (20), body text `TEXT_BODY_SM` (13), small text `TEXT_CAPTION` (12).
- [ ] AC-10 (always): With the dark palette, text on its ground has a contrast of at least 4.5 : 1 for `text`, `text_secondary`, `text_muted` and `danger_ink` on `surface` and on `surface_raised`, and for `on_accent` on `accent_solid`; `accent`, `focus` and `border_strong` have at least 3 : 1 on `surface`.
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
| AC-01 | `theme_module_is_generated_for_the_project_accent` (`crates/blinzel-ui/src/theme_tests.rs`); `cargo fmt --all --check`; `node scripts/theme.ts ../amigo-blinzel/docs/site/project.yml --check --rust …` in amigo-labs/website (branch `feat/theme-rustfmt`): "up to date" | built |  |
| AC-02 | `site_data_workflow_checks_the_theme_module` (`crates/blinzel-ui/src/theme_tests.rs`); CI: site-data | built |  |
| AC-03 | `no_colour_literal_outside_the_adapter`, `colour_literals_are_found_also_over_several_lines`, `rgb_converts_a_hex_colour`, `the_adapter_names_the_dark_palettes_roles` (`crates/blinzel-ui/src/theme_tests.rs`) | built |  |
| AC-04 | `the_adapter_names_the_dark_palettes_roles`, `a_dialog_is_a_surface_panel_with_a_border`, `the_popup_menu_is_a_raised_surface` (`crates/blinzel-ui/src/theme_tests.rs`) | built |  |
| AC-05 | `buttons_take_their_fills_from_the_theme`, `a_field_is_outlined_by_its_state`, `a_check_box_shows_its_state_in_theme_colours`, `the_chosen_segment_is_an_accent_fill` (`crates/blinzel-ui/src/theme_tests.rs`); error text in `danger_ink`: `the_footer_states_what_start_will_do` (`crates/blinzel-ui/src/batch_dialog.rs`), `unreadable_files_show_their_reason_in_red` (`crates/blinzel-ui/src/app_batch_tests.rs`) | built |  |
| AC-06 | `buttons_take_their_fills_from_the_theme`, `a_check_box_shows_its_state_in_theme_colours`, `the_chosen_segment_is_an_accent_fill`, `slider_and_progress_fill_with_the_accent` (`crates/blinzel-ui/src/theme_tests.rs`) | built |  |
| AC-07 | `the_grid_marks_with_the_accent`, `the_filmstrip_frames_the_current_image_with_the_accent`, `the_selection_frame_and_the_loupes_pixel_are_accent` (`crates/blinzel-ui/src/theme_tests.rs`) | built |  |
| AC-08 | `the_former_blue_is_gone`, `the_setup_window_takes_its_colours_from_the_palette` (`crates/blinzel-ui/src/theme_tests.rs`) | built |  |
| AC-09 | `sizes_come_from_the_theme`, `a_dialog_is_a_surface_panel_with_a_border`, `the_popup_menu_is_a_raised_surface`, `buttons_take_their_fills_from_the_theme` (`crates/blinzel-ui/src/theme_tests.rs`) | built |  |
| AC-10 | `the_dark_palette_passes_wcag_aa`, `contrast_follows_the_wcag_formula` (`crates/blinzel-ui/src/theme_tests.rs`) | built |  |
| AC-11 | `the_default_background_is_the_themes_bg` (`crates/blinzel-ui/src/theme_tests.rs`); `a_stored_background_wins_over_the_themes_default`, `a_missing_file_means_the_documented_defaults`, `invalid_background_and_paths_keep_their_defaults` (`crates/blinzel-core/src/settings.rs`) | built |  |
| AC-12 | `the_setup_window_takes_its_colours_from_the_palette` (`crates/blinzel-ui/src/theme_tests.rs`, reads the source); type-check and clippy of the Windows shell from Linux (`cargo clippy -p blinzel-app --no-default-features --tests --target x86_64-pc-windows-msvc -- -D warnings`, AGENTS.md Gotchas): clean; CI: windows builds and runs it | built |  |
| AC-13 | `cpuref::tests::*` (`crates/blinzel-core/src/cpuref.rs`), `tests/wgpu_golden.rs` (`crates/blinzel-app`; skips without a GPU adapter, as in the local run of 2026-11-15); no renderer source changed (`git diff 33330a7 -- crates/blinzel-core/src/cpuref.rs crates/blinzel-app/src/gfx crates/blinzel-app/src/linux/gpu.rs crates/blinzel-app/src/macos/gpu.rs` is empty); CI: linux (lavapipe), windows (WARP), macos (Metal) | built |  |
| AC-14 | manual: open — dialogs, popup menu, filmstrip, grid and selection frame in the running viewer | open |  |

## Decisions

- Dark palette first, light and the theme choice in a second spec: the dark palette is a change of constants; light needs a setting, a menu entry and each platform's appearance signal.
- The system's UI font stays (no bundled Inter): startup time is the product (`blinzel.md` §5).
- Elements drawn over the picture keep their black and white with alpha: they sit on arbitrary pixels, where a theme surface colour guarantees nothing.
- The `background` default follows the theme's `bg`, but a stored value wins: the file has always written every key, so existing users keep what they see.
- The adapter is `crates/blinzel-ui/src/palette.rs` (`rgb`, `rgba` and one constant per role); `widgets.rs` re-exports the names its call sites read. `popup.rs`, `browse.rs`, `chrome.rs`, `app.rs` and the setup window use the same constants.
- Red run (2026-11-15, the adapter still holding the old colours): `cargo test -p blinzel-ui --lib theme_tests` failed 13 of 21 tests, among them `the_adapter_names_the_dark_palettes_roles`, `no_colour_literal_outside_the_adapter`, `the_former_blue_is_gone`, `buttons_take_their_fills_from_the_theme`, `a_field_is_outlined_by_its_state`, `the_popup_menu_is_a_raised_surface` (radius 8 instead of 12) and `the_default_background_is_the_themes_bg`. Green at once, because they test the helpers or the generated values: `rgb_converts_a_hex_colour`, `colour_literals_are_found_also_over_several_lines`, `contrast_follows_the_wcag_formula`, `the_dark_palette_passes_wcag_aa`, `theme_module_is_generated_for_the_project_accent`.
- Panels and the popup menu are opaque: no test and nothing in the look needed the former 1.5 % transparency.
- A check box and a segment inside its track have `RADIUS_XS` (4), a list frame inside a panel and the screenshot hint `RADIUS_MD` (8): the theme's own components (`components.css`) round a 16 px box and nested parts that way; `RADIUS_SM` on them would read as a circle. Button, field and the segmented control's track have `RADIUS_SM`.
- A primary button under the pointer is `accent_solid` under a veil of white at 12 %: the palette has no lighter accent role, and white with alpha is the one colour allowed outside the adapter (AC-03). The Windows setup window does the same (AC-12 "lighter under the pointer").
- Every disabled button, primary or not, is `surface_raised` with a `border` outline and `text_muted` (edge case "a disabled primary button"); a disabled check box label is `text_muted` too (AC-04).
- A field at rest and an unchecked check box are outlined with `border_strong` (AC-05, revised 2026-11-15 after the build): the theme's own components do the same, and it keeps the 3 : 1 boundary the outline had before; `border` on `surface` has far less. Buttons, panels and menus keep `border`.
- The track of a slider and of a progress bar is `hover`, the slider's knob `text`.
- The link "Remove all" is `accent_ink`, the theme's role for accent text; AC-07 names lines and marks only. The contrast test covers `accent_ink` as well.
- A marked thumbnail's badge is an `accent` disc with an `on_accent` check (white on the red accent has 3.3 : 1).
- The hint and the error message in an empty window are `text_secondary` on the background.
- A thumbnail not loaded yet is a `surface` tile, a thumbnail under the pointer lies on `hover`. The band behind the filmstrip stays black at 30 %: it darkens whatever `background` the user chose.
- The close button's reds (window buttons are out of scope) moved into the adapter as `CLOSE_HOVER` / `CLOSE_PRESSED`, unchanged, so the source test of AC-03 holds without an exception list.
- The loupe's frame stays white at 10 % (the loupe is out of scope); the screenshot overlay's size tag and hint keep using the panel and text constants and so became `surface` with `text`.
- The dialog's header row is as high as the title's line (`TEXT_TITLE.line`, 26) instead of 24, so the 20 px title is not cut by its row.
- The setup window's buttons take `RADIUS_SM` too; its close button and glyph stay as they are.
- New public API of `blinzel-ui`: the modules `theme` (generated) and `palette`; `widgets::{RADIUS, RADIUS_INNER, RADIUS_FRAME, RADIUS_PANEL, ACCENT_INK, ACCENT_SOLID, ACCENT_HOVER, ON_ACCENT, FOCUS, DANGER_INK}`. `widgets::ACCENT_FILL` is gone (now `ACCENT_SOLID`), `widgets::DANGER` is the outline colour and `DANGER_INK` the text colour.
- AC-12 has no test that runs the setup window: it builds only on Windows. `the_setup_window_takes_its_colours_from_the_palette` reads its source on every platform; the Windows CI job compiles it.
- The hint under the file name pattern is `text_muted` (AC-04 "hint text"); the drop zone's line and other secondary lines stay `text_secondary`.
