---
status: todo
created: 2026-11-02
size: M
platforms: all
design: [blinzel.md §3, blinzel.md §11]
---

# Animations

## Context

Animated GIF, WebP and JPEG XL files play in the viewer (milestone M5, `blinzel.md` §11 "Animations (M5)"). All three are decoded natively on every platform (`gif`, `image-webp`, `jxl-oxide`; §3 gate table: "Animations native"). The first frame appears as fast as any still image; the animation is decoded in the background for the current image only and starts while the rest is still decoding. Frames are composited the way browsers do.

This spec records what the code does today (reverse engineered on 2026-11-02).

## Scope

### In scope

- Compositing animated GIF frames onto the canvas (transparency, disposal)
- Frame delays (with the browsers' minimum), loop counts, the memory budget
- JPEG XL keyframe animations
- The loader stage for animations and playback while decoding
- Playback timing, end of playback, behavior on navigation and on errors

### Out of scope

- APNG, animated AVIF/HEIC sequences, multi-page TIFF — because no animated decoder exists for them; they show their first image as a still
- Playback controls (pause, frame stepping, speed) — because there are none
- Thumbnails of animations — because the thumbnail spec covers them (they show a still)
- The still first image (decode, prefetch, first frame) — because the browsing and startup specs cover it

## Acceptance criteria

- [ ] AC-01 (always): Every frame of an animated GIF shows the whole canvas after that frame: its transparent pixels leave the canvas as it was, disposal "background" clears the frame's rectangle to transparent before the next frame, and disposal "previous" restores the canvas from before the frame.
- [ ] AC-02 (always): A GIF or WebP frame stays on screen for its stated delay, except that a delay under 20 ms plays as 100 ms.
- [ ] AC-03 (always): A GIF with a NETSCAPE loop count n plays n + 1 times and with loop count 0 for ever; a WebP with loop count n plays n times and with 0 for ever.
- [ ] AC-04 (always): A GIF, WebP or JPEG XL file with fewer than two frames (or a still file) yields no animation and is shown as a still image.
- [ ] AC-05 (event): When an animated JPEG XL (animation header, at least two keyframes, ticks per second not zero) is the current image, every keyframe plays for `duration × 1000 × tps_denominator / tps_numerator` ms with the 20 ms / 100 ms rule of AC-02, and the animation plays `num_loops` times (0: for ever).
- [ ] AC-06 (state): While the decoded frames of an animation would exceed 256 MB (4 bytes per canvas pixel and frame), the frames beyond that are not decoded and the frames that fit play; if fewer than two fit, the image is shown as a still.
- [ ] AC-07 (event): When the probe of the current image (it arrives with its first decoded still stage) says GIF, WebP or JPEG XL, the viewer decodes its animation in the background, behind the still image in priority; neighbours in the prefetch window never decode animations, and the still image is shown as it would be without animation until playback starts.
- [ ] AC-08 (event): When two frames of the current image's animation are on the GPU, playback starts with the first frame while decoding continues; the viewer receives the frames decoded so far at most every 200 ms; when playback reaches the last frame received so far, it holds that frame and continues with the next frame once more arrive.
- [ ] AC-09 (always): Each frame stays for its delay (at least 10 ms); after the last frame of the last play, the last frame stays on screen; an animation without a loop limit repeats until the image changes.
- [ ] AC-10 (event): When the user moves to another image, playback stops and the previous image's frames are released; coming back decodes the animation again and plays it from the first frame.
- [ ] AC-11 (error): If decoding fails (a decode error or a panic in the decoder) after at least two frames, the frames decoded so far play with the loop count the file gave up to there; if it fails before the second frame, the image stays a still.
- [ ] AC-12 (error): If uploading a frame to the GPU fails, decoding stops and the frames uploaded so far play with the file's loop count.
- [ ] AC-13 (state): While an animation plays, the view mode, zoom, pan, the user's rotation and the EXIF orientation apply to every frame as to the still image.
- [ ] AC-14 (state, manual: playback smoothness): While a large animated GIF (e.g. 1920 × 1080, 200 frames) plays, frames advance at their delays without visible stutter, also while it is still decoding.

## Technical notes

- **Decoders** (`blinzel-core::decode`): `gif::stream_animation`, `webp::stream_animation`, `jxl::stream_animation`, each `fn(&[u8], budget: usize, &mut FrameSink) -> Result<AnimInfo>`. `FrameSink = dyn FnMut(AnimFrame) -> bool`; returning `false` stops decoding and keeps the frames so far. `AnimFrame { image, delay_ms, plays }` is the full canvas; `plays` is the loop count as far as the file has said by that frame, so a decode error after it keeps it. `AnimInfo { frames, plays }`, `plays: None` = for ever. `decode_animation` collects the stream (`collect_animation`, `None` for fewer than two frames). `min_delay_ms` implements the 20 ms → 100 ms rule.
- **GIF**: composited in our code on an RGBA canvas of the logical screen; frames are clipped to the canvas; `gif::Repeat::Finite(n)` → `plays = n + 1` (no NETSCAPE extension reads as `Finite(0)`, i.e. once), `Infinite` → `None`. The budget counts `width × height × 4` per frame. **WebP**: `image-webp` composites; `LoopCount::Times(n)` → `n`. **JPEG XL**: `jxl-oxide` loads the file, then renders keyframe by keyframe; `num_loops == 0` → for ever.
- **Loader** (`blinzel-ui::loader`): `Stage::Anim` (key bits `3`), `ANIM_BUDGET = 256 << 20`, `ANIM_UPDATE = 200 ms`. `animation_stage` uploads each frame as it arrives, sends `Loaded { animation: Some(GpuAnimation { frames, plays, complete }) }` from the second frame on (intermediate deliveries `complete: false`, `plays: None`), and a final delivery with `complete: true`. The decoder runs inside `decode::guard`: a decode error or a panic ends the animation with the frames so far, at the last frame's `plays`. A cancelled job sends nothing.
- **App** (`blinzel-ui::app`): `update_wanted` adds the `Anim` job (priority 1, after the current image's still at priority 0) while the format may animate and the animation is not complete. `on_loaded` stores the animation per cache id, starts the `Timer::Anim` on the first delivery and resumes a waiting playback on later ones. `on_anim_timer` advances frames, counts plays, holds at the end. `after_navigation` stops the timer and drops every other image's animation (`loader.forget` of its `Anim` key). Results arriving while the window is hidden (resident mode) are dropped.
- **Measurement** (design spec): 480 × 270 GIF, 1.2 ms decode per frame.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| GIF without a NETSCAPE extension | Plays once, then holds the last frame |
| GIF frame extending beyond the logical screen | Clipped to the canvas |
| GIF with a 0 × 0 logical screen | The still shows (its size comes from the first frame); the animation stage fails before any frame, so it stays a still |
| JPEG XL with an animation header but ticks per second 0, or a single keyframe | Shown as a still |
| User leaves the image while its animation decodes | The job is cancelled; nothing is delivered; the next image is unaffected |
| A delivery that says "still" after frames were already shown | The frames shown so far keep playing |
| A panic inside one of the animation decoders | Turned into a decode error (`decode::guard`): the worker thread goes on, the frames so far play as in AC-11 |
| Very long animation | Only the frames within 256 MB play, with the file's loop count |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `composites_animation_frames` (`crates/blinzel-core/src/decode/gif.rs`) | built | |
| AC-02 | `single_frames_budgets_and_plays` (`crates/blinzel-core/src/decode/gif.rs`), `plays_animations` (`crates/blinzel-core/src/decode/webp.rs`) | built | |
| AC-03 | `composites_animation_frames`, `single_frames_budgets_and_plays` (`crates/blinzel-core/src/decode/gif.rs`), `plays_animations` (`crates/blinzel-core/src/decode/webp.rs`) | built | |
| AC-04 | `single_frames_budgets_and_plays` (`gif.rs`), `plays_animations`, `decodes_lossless_stills` (`webp.rs`), `lossless_round_trips` (`jxl.rs`), all in `crates/blinzel-core/src/decode/` | built | |
| AC-05 | `plays_keyf| AC-06 | `single_frames_budgets_and_plays` (`gif.rs`), `plays_animations` (`webp.rs`), `keyframes_beyond_the_budget_are_not_decoded` (`jxl.rs`), all in `crates/blinzel-core/src/decode/`; `the_viewer_decodes_only_the_frames_within_256_mb` (`crates/blinzel-ui/src/app_animation_tests.rs`) | built | |rs`) | built | |
| AC-06 | partial: `single_frames_budgets_and_plays` (`gif.rs`) and `plays_animations` (`webp.rs`) check the cut with a small budget; nothing checks the 256 MB the viewer passes, nor JPEG XL | open | |
| AC-07 | `only_the_current_gif_decodes_its_animation_behind_its_still` (`crates/blinzel-ui/src/app_animation_tests.rs`) | built | |
| AC-08 | | open | |
| AC-09 | `each_frame_stays_for_its_delay_and_the_last_play_holds_the_last_frame`, `an_animation_without_a_loop_limit_repeats_until_the_image_changes` (`crates/blinzel-ui/src/app_animation_tests.rs`) | built | |
| AC-10 | `leaving_an_animation_stops_and_drops_it_and_coming_back_plays_it_anew` (`crates/blinzel-ui/src/app_animation_tests.rs`) | built | |
| AC-11 | `a_decode_error_after_two_frames_plays_them_with_the_files_loop_count`, `a_decode_error_before_the_second_frame_leaves_a_still`, `a_panicking_decoder_plays_the_frames_so_far_and_the_viewer_goes_on` (`crates/blinzel-ui/src/app_animation_tests.rs`), `frames_carry_the_loop_count_up_to_a_decode_error` (`crates/blinzel-core/src/decode/gif.rs`) | built | |
| AC-12 | `a_failed_upload_plays_the_frames_uploaded_so_far_with_the_files_loop_count` (`crates/blinzel-ui/src/app_animation_tests.rs`) | built | |
| AC-13 | | open | |
| AC-14 | manual | open | |

## Decisions

- 2026-11-02: Extracted from the code. Recorded as current behavior: a decode error midway loses the file's loop count (the frames so far loop for ever), and the animation decoders run without `catch_unwind` (AGENTS.md §9 asks for it).
- 2026-11-02: Both defects fixed, AC-11 and the panic edge case revised to match: the loop count travels with every frame (`AnimFrame::plays`, a new public field of `blinzel-core`), because a decode error or a panic leaves no `AnimInfo` to read it from; the loader runs the stream inside `decode::guard`. Red runs: `a_decode_error_after_two_frames_…` played `None` (for ever) instead of `Some(2)`; `a_panicking_decoder_…` failed with "the loader did not get idle" (the worker thread was gone).
- 2026-11-02: The panic is injected by a test-only seam in the loader (`loader::PANICS_ON_THIRD_FRAME`, `cfg(test)`): the sink panics on a file of that name, which unwinds through the decoder like a decoder bug; `blinzel-core`'s own `cfg(test)` is not active in `blinzel-ui`'s tests.
- 2026-11-02: The App tests write their GIFs with a small hand-made encoder in the testkit (`testkit::gif`) instead of taking `gif` as a dev-dependency of `blinzel-ui`.
- 2026-11-02: Green at once, the behavior already existed: AC-11's "fails before the second frame" half and AC-12 (an upload failure stops the sink, the decoder returns its `AnimInfo` with the loop count). Each test was checked by breaking the code once (the two-frame minimum, `return true` after a failed upload): it went red.
- 2026-11-02: AC-07, AC-09 and AC-10 were green at once (extracted behavior). Each test went red with the code broken on purpose: the Anim job at priority 0, neighbours given an Anim job, the timer ignoring the delay, no hold after the last play, an endless loop played once, no `drop_animations` or no `kill_timer` on navigation. The tests drive `Timer::Anim` by calling `App::on_anim_timer`, as a shell does when the timer fires.
- 2026-11-02: AC-09's 10 ms floor (`schedule_frame`) cannot be reached through a file: all three decoders give at least 20 ms (`min_delay_ms`), so the tests prove the delays files give.
- 2026-11-02: The JPEG XL fixtures (`crates/blinzel-core/testdata/anim-8x6-*.jxl`, 95 and 93 bytes) were made once with `cjxl` 0.7 (Ubuntu's `libjxl-tools`) from a hand-written APNG; libjxl 0.7 writes neither a loop count nor ticks per second other than 1000/1 for it, so the loop-2 file's animation header was rewritten bit by bit to 500/3 ticks per second and two loops (checked with `djxl`). The odd rate makes the formula's numerator and denominator both visible in the delays. No JPEG XL file with ticks per second 0 or a single keyframe exists among them (edge case only, AC-04 covers stills).
- 2026-11-02: AC-05 and AC-06 were green at once (extracted behavior). Each test went red with the code broken on purpose: the delay without `tps_denominator`, without `min_delay_ms`, the loop count dropped, the budget check `<` instead of `<=`, `ANIM_BUDGET` at 255 or 260 MB, JPEG XL missing from the formats that may animate. The viewer's 256 MB is proven with a 1024 × 1024 GIF of 65 one-pixel frames (4 MiB per decoded frame, 64 fit), written by `testkit::gif_on_screen`.
