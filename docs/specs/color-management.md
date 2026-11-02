---
status: todo
created: 2026-11-02
size: L
platforms: all
design: [blinzel.md §5, blinzel.md §9]
---

# Color management

## Context

Photos from phones and cameras often carry a wide-gamut profile (Display P3, Adobe RGB), and some monitors have a profile that is not sRGB. Without conversion such images look dull or oversaturated. The design spec makes ICC color management a setting that is on by default (`blinzel.md` §5, "ICC color management (M6…)") and names the display target per platform (`blinzel.md` §9: Little CMS and `_ICC_PROFILE` on Linux, `CGDisplayCopyColorSpace` on macOS). The guiding constraint is startup speed (`blinzel.md` §5): the usual case, an sRGB image on an sRGB display, must cost nothing.

This spec records what the code does today.

## Scope

### In scope

- Reading embedded profiles when an image is probed, and treating images without one as sRGB
- Telling sRGB profiles apart from all others
- Building, caching and applying the transform from the image profile to the display target (GPU, CPU reference, CPU first frame)
- The display target per platform, when it is determined and when it is determined again
- The setting "Use colour profiles (ICC)"

### Out of scope

- HDR output and tone mapping (PQ, HLG) — because the viewer renders SDR only; such AVIFs are refused by the decoder (decoders spec)
- Perceptual or saturation rendering intents — because the viewer always converts relative colorimetric
- Converting grayscale or CMYK profiles — because the decoders already deliver RGB pixels and the RGB transform cannot be built for them
- A display target on Wayland — because Wayland has no protocol for it yet (`blinzel.md` §9)
- Color management of thumbnails (filmstrip, grid) and of batch output — because only the stage image is converted; batch keeps or drops the profile (batch spec)
- Decoding itself, and which decoder handles which format — because the decoders spec owns it

## Acceptance criteria

- [ ] AC-01 (always): Probing a JPEG yields its embedded profile (APP2 `ICC_PROFILE`, also when split over several segments), probing a PNG yields its `iCCP` profile, without decoding pixels; a file without a profile has none and is treated as sRGB.
- [ ] AC-02 (always): Probing an AVIF whose `colr` box is nclx with Display P3 primaries yields a generated Display P3 profile; an AVIF with BT.709/sRGB colour or without a colour box yields none.
- [ ] AC-03 (always): Probing a WebP (ICC chunk), a TIFF (ICC profile tag), a HEIC (`colr` profile, read from the container also when the system decoder decodes it) or a JPEG XL (the profile of the colour space it is rendered into, none for sRGB) yields that profile; on macOS, any other format ImageIO probes yields the profile ImageIO reports for an RGB image that is not sRGB.
- [ ] AC-04 (always): A profile counts as sRGB only if it is an RGB profile with an XYZ connection space whose red, green and blue colorants are each within 0.003 of sRGB's D50 colorants and whose three tone curves (gamma, table or parametric) are within 0.0015 of the sRGB curve at 50 sample points; sRGB primaries with gamma 2.2, Adobe RGB, Display P3, truncated profiles and non-ICC data are not sRGB.
- [ ] AC-05 (state): While both the image (with an sRGB profile or none) and the display target are sRGB, no transform is built, the platform colour engine is not touched and the image is drawn unconverted.
- [ ] AC-06 (always): The transform for one image profile and one display target is built once and reused for every image with that profile until the target changes; an image whose profile equals the target profile is drawn unconverted.
- [ ] AC-07 (always): On Linux and macOS, a transform converts relative colorimetric: sRGB → sRGB is the identity within 1/255 per channel, and Display P3 → sRGB keeps neutral greys within 1/255 and clips P3's full red to sRGB red (red ≥ 250/255, green and blue ≤ 5/255).
- [ ] AC-08 (always, CI: windows): On Windows, a transform converts relative colorimetric: sRGB → sRGB is the identity within 1/255 per channel, Display P3 → sRGB keeps neutral greys within 1/255, clips P3's full red to sRGB red (red ≥ 250/255, green and blue ≤ 5/255) and makes a muted P3 red (0.6, 0.3, 0.3) more saturated.
- [ ] AC-09 (always): The transform changes the image only, never the letterbox background, and is applied after scaling and before the background shows through transparent pixels; the GPU renderer on each platform matches the CPU reference renderer for this, and so does the CPU first frame.
- [ ] AC-10 (always): The 8-bit lookup used for the CPU first frame differs from the exact lookup by at most 1/255 per channel, and the identity transform returns every 8-bit colour unchanged.
- [ ] AC-11 (event, CI: windows): When the viewer starts on Windows with colour management on, then it uses the target remembered from the last run for the monitor under the cursor (sRGB if nothing is remembered or the remembered profile file is unreadable) without asking Windows, asks Windows for the monitor's target only after the first frame, redraws if the answer differs, and remembers the answer for the next start.
- [ ] AC-12 (state, manual: needs a monitor with an assigned non-sRGB profile and one with HDR or Auto Color Management): While Windows manages colour on the window's monitor (HDR or Auto Color Management) or the monitor's profile is sRGB or missing, the target is sRGB; otherwise the target is the monitor's profile.
- [ ] AC-13 (state): While running under X11, the target is the profile published on the root window for the window's monitor (`_ICC_PROFILE` for the first RandR monitor, `_ICC_PROFILE_<n>` for the n-th); if that property is missing, shorter than 128 bytes or an sRGB profile, or the monitor is unknown, the target is sRGB.
- [ ] AC-14 (state): While `WAYLAND_DISPLAY` is set and not empty, the target on Linux is sRGB.
- [ ] AC-15 (state, manual: needs a Mac display with a known non-sRGB profile): While running on macOS, the target is the profile of the display's colour space, or sRGB if that is sRGB or has no ICC data.
- [ ] AC-16 (event): When the window ends up on another monitor, or on Windows the display configuration changes, then the target is determined for that monitor, and if it changed, the image is drawn again with the new transform.
- [ ] AC-17 (option): Where "Use colour profiles (ICC)" in Menu → Settings is off, no image is converted; the setting is on by default, and toggling it takes effect on the next frame without a restart.
- [ ] AC-18 (error): If the colour engine cannot build a transform (a profile it cannot parse, a grayscale or CMYK profile), then the image is drawn unconverted and the build is not retried for that profile until the target changes.
- [ ] AC-19 (option, CI: windows): Where colour management is on, a resident start (`--resident`) loads the platform's colour engine once, with a Display P3 profile, before any image is opened.

## Technical notes

- Types: `blinzel_core::icc::Profile { bytes: Arc<[u8]>, hash: u64 (FNV-1a of the bytes), srgb: bool }`; `Probe::icc: Option<Profile>` (none = sRGB). `blinzel_core::lut::Lut3d`: 33³ nodes (`lut::SIZE`), `[u16; 3]` each, red fastest; `apply` (float, trilinear) and `apply_u8` (8.8 fixed point, CPU first frame).
- Engines: Windows Color System (`crates/blinzel-app/src/color.rs`: `OpenColorProfileW`, `CreateMultiProfileTransform` with `INTENT_RELATIVE_COLORIMETRIC`, `TranslateBitmapBits` with `BM_16b_RGB` over the grid); Little CMS on Linux and macOS (`crates/blinzel-app/src/lcms.rs`, `RGB_16`, `Intent::RelativeColorimetric`). The target `None` means the engine's sRGB profile.
- `blinzel_ui::color::ColorManager` (shared by loader and UI thread): `set_target` bumps a generation and drops finished LUTs; `prepare` builds on a `blinzel-color` thread as soon as the probe is known, in parallel with the decode; `lut` waits up to 200 ms for an unknown target, caches by (target generation, source hash; 0 for sRGB), and records `color_lut_us`. `App::gpu_lut` uploads each LUT once per device.
- Targets: Windows `display_target` (`GetMonitorInfoW` device name → `QueryDisplayConfig`/`DisplayConfigGetDeviceInfo` advanced-colour bit → `GetICMProfileW`), remembered in `%LOCALAPPDATA%\amigo-blinzel\display-color.txt` as `device<TAB>profile path` per line (empty path = sRGB). Linux `linux/color.rs` (x11rb, RandR monitors, monitor id = FNV-1a of the output name). macOS `macos/color.rs` (`CGDisplayCopyColorSpace`, `CGColorSpace::icc_data`); the Metal layer has no colour space of its own.
- When: Windows sets the remembered target at start and verifies after the first frame (`App::verify_color_target`); Linux and macOS start with sRGB, ask synchronously when the window exists (`App::check_monitor(true)`) and again after the first frame. Re-check on `WM_EXITSIZEMOVE`, `WM_WINDOWPOSCHANGED`, `WM_DISPLAYCHANGE` (forced) on Windows, `WindowEvent::Moved` on Linux, `windowDidChangeScreen:` on macOS.
- Measurements (Windows, `blinzel.md` §5): the ICM subsystem loads in about 7 ms on the first `GetICMProfileW`; a LUT builds in 4–7 ms cold.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| JPEG profile split over three APP2 segments (150 kB) | Read completely |
| Image profile identical to the display profile | Drawn unconverted |
| Display target not yet known when the first image needs a LUT | Waits up to 200 ms, then treats the target as sRGB |
| Remembered profile file on Windows was deleted | Start assumes sRGB; the check after the first frame corrects it |
| Wide-gamut image on an sRGB display | Converted (P3 red clipped to sRGB red) |
| sRGB image on a wide-gamut display | Converted too |
| X11 profile property shorter than 128 bytes | Target sRGB |
| Colour management turned off while a converted image is shown | Next frame unconverted; GPU LUTs dropped |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `reads_profiles_from_jpeg`, `reads_profiles_from_png` (`crates/blinzel-core/src/icc.rs`), CI: Linux | built | |
| AC-02 | `display_p3_gets_a_profile`, `decodes_8_and_10_bit` (`crates/blinzel-core/src/decode/avif.rs`), CI: Linux | built | |
| AC-03 | `probe_reads_the_icc_chunk` (`crates/blinzel-core/src/decode/webp.rs`), `probe_reads_the_icc_profile_tag` (`decode/tiff.rs`), `probe_reads_the_colour_profile` (`decode/heif.rs`), `heic_profile_comes_from_the_container` (`decode/avif.rs`), `probe_yields_the_rendered_colour_space` (`decode/jxl.rs`), CI: Linux; the macOS ImageIO clause: `probe_yields_the_profile_imageio_reports` (`crates/blinzel-app/src/macos/imageio.rs`), CI: macOS, macOS job ok (CI run 41504896644, `0acbc36`) | built | |
| AC-04 | `recognises_srgb_by_primaries_and_curve` (colorants 0.002 off are sRGB, 0.004 off are not: red with a 0.03 tolerance), `parametric_srgb_curve_counts` (`crates/blinzel-core/src/icc.rs`), CI: Linux | built | |
| AC-05 | `caches_per_profile_and_skips_srgb` (`crates/blinzel-ui/src/color.rs`), CI: Linux | built | |
| AC-06 | `caches_per_profile_and_skips_srgb` (`crates/blinzel-ui/src/color.rs`), CI: Linux | built | |
| AC-07 | `srgb_to_srgb_is_identity_and_p3_red_clips` (`crates/blinzel-app/src/lcms.rs`), CI: Linux, macOS | built | |
| AC-08 | `srgb_to_srgb_is_identity`, `wide_gamut_to_srgb` (`crates/blinzel-app/src/color.rs`), CI: Windows | built | |
| AC-09 | `colour_transform_lut` (`crates/blinzel-app/tests/wgpu_golden.rs` CI: Linux; `warp_golden.rs` CI: Windows; `metal_golden.rs` CI: macOS), `applies_the_colour_transform_to_the_image_only` (`crates/blinzel-core/src/preview.rs`), CI: Linux | built | |
| AC-10 | `integer_lookup_follows_the_float_one`, `identity_maps_to_itself` (`crates/blinzel-core/src/lut.rs`), CI: Linux | built | |
| AC-11 | `the_start_draws_with_the_remembered_target_and_asks_after_the_first_frame` (`crates/blinzel-ui/src/app_color_tests.rs`), CI: Linux; `remembers_the_target_per_monitor_for_the_next_start` (`crates/blinzel-app/src/color.rs`), CI: Windows, Windows job ok (CI run 41504896644, `0acbc36`) | built | |
| AC-12 | — | open | |
| AC-13 | `reads_the_x11_display_profile` (`crates/blinzel-app/src/linux/color.rs`), CI: Linux, step with `BLINZEL_X11_TEST` (returns early without it) | built | |
| AC-14 | `reads_the_x11_display_profile` (`crates/blinzel-app/src/linux/color.rs`), CI: Linux, step with `BLINZEL_X11_TEST` (returns early without it) | built | |
| AC-15 | — | open | |
| AC-16 | `a_new_monitor_retargets_and_redraws` (`crates/blinzel-ui/src/app_color_tests.rs`), CI: Linux | built | |
| AC-17 | `the_setting_switches_conversion_on_the_next_frame` (`crates/blinzel-ui/src/app_color_tests.rs`), `menu_settings_offers_each_platforms_entries`, `each_check_item_is_checked_when_its_setting_is_on` (`crates/blinzel-ui/src/app_settings_tests.rs`), CI: Linux | built | |
| AC-18 | `a_failed_build_is_not_retried_until_the_target_changes` (`crates/blinzel-ui/src/color.rs`), `no_transform_for_grey_cmyk_or_broken_profiles` (`crates/blinzel-app/src/lcms.rs`), CI: Linux, macOS | built | |
| AC-19 | `the_warm_up_builds_one_display_p3_transform` (`crates/blinzel-ui/src/app_color_tests.rs`), CI: Linux, built; `a_resident_start_warms_up_once_before_the_hint_frame` (`crates/blinzel-app/src/run.rs`, `warm_resident`: the `--resident` branch's warm-up runs once, before the hint frame), CI: Windows, Windows job ok (CI run 41532658575, `51202b8`) | built | |

## Decisions

- AC-14, AC-16, AC-17, AC-18 describe behavior the code already had (the spec was extracted), so their tests were green at once; each was proven by breaking the code once (Wayland check, the forced re-check, `set_enabled` in the toggle, caching a failed build, Little CMS falling back to sRGB) and seeing the test go red.
- The harness's colour engine builds an identity LUT instead of none, and the fake canvas records whether a frame drew an image through a LUT: App tests can see conversion without a real colour engine; the engine's own output is proven by AC-07, AC-08.
- AC-14 is checked inside the X11 test: both change the process environment, and the X11 one needs a published profile for the Wayland check to mean anything (without an X server the target is sRGB anyway).
- AC-18 proves the "cannot build" cases against Little CMS; the Windows Color System's refusal of such profiles has no test of its own (AC-08's test file covers only RGB profiles).
- AC-03 found a defect: the container reader treated HEIC's `hvcC` as an unknown essential property and refused the primary item, so the profile WIC and ImageIO probes take from the container was always lost for HEIC. `hvcC` is now ignored like `av1C`; `heic_profile_comes_from_the_container` was red before the fix.
- AC-03's fixtures: `testdata/p3-64x48.heic` is `rgb-64x48.heic` with a Display P3 `colr prof` box added by the batch engine's `avif_with_icc`; `testdata/p3-8x6.jxl` is `cjxl -d 0` of an 8 × 6 PNG tagged with `matrix_profile(DISPLAY_P3, Trc::Srgb, "Display P3 test")`.
- AC-03's macOS clause is tested with PNGs handed to ImageIO's `probe` directly (the native decoders would take them first in `probe_any`): a Display P3 profile comes back, an sRGB profile, no profile and a grey image yield none; the HEIC fixture `p3-64x48.heic` proves the container path. Only the macOS CI job runs it; its first run is the proof.
- AC-11 is split where the code is: the remembered-target file (`remembered`, `remember_in` in Windows `color.rs`, now taking the file so a test can use a temp folder) and the App's check after the first frame. The App test is green at once (the behavior existed); breaking it once each way turned it red: without `color_verified` it asked four times, without the redraw in `on_color_target` and without `verify_color_target` it failed. The Windows test was run once as a Linux copy of the two functions, green, and red when an answer no longer replaced the monitor's line and when an empty path (sRGB) was read as a file; its first Windows CI run is the proof.
- The harness counts colour-target questions (`FakeServices::target_queries`), can hold the answers back (`target_gate`), counts delivered answers (`Harness::color_answers`) and records each transform's source profile (`LUT_SOURCES`): the AC-11 test needs the first frames drawn before the answer arrives, the AC-19 test the warm-up's profile.
- AC-19 is proven at the App level: `App::warm_up` builds exactly one transform from a profile with Display P3's colorants while nothing is open, and none with colour management off (red with the warm-up call removed, with an Adobe RGB profile, and with the setting ignored). The `--resident` branch of Windows `run.rs` calls it before the message loop through `warm_resident(warm_up, render)`, whose Windows test checks that the warm-up runs once and before the hint frame (a copy of the function and test, compiled on Linux, goes red with the `warm_up()` call removed); no test starts `--resident`, because that registers the viewer to start at sign-in on the machine running the test.
