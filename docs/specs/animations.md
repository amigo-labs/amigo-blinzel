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
- [ ] AC-11 (error): If decoding fails after at least two frames, the frames decoded so far play in an endless loop (the file's loop count is not used); if it fails before the second frame, the image stays a still.
- [ ] AC-12 (error): If uploading a frame to the GPU fails, decoding stops and the frames uploaded so far play with the file's loop count.
- [ ] AC-13 (state): While an animation plays, the view mode, zoom, pan, the user's rotation and the EXIF orientation apply to every frame as to the still image.
- [ ] AC-14 (state, manual: playback smoothness): While a large animated GIF (e.g. 1920 × 1080, 200 frames) plays, frames advance at their delays without visible stutter, also while it is still decoding.

## Technical notes

- **Decoders** (`blinzel-core::decode`): `gif::stream_animation`, `webp::stream_animation`, `jxl::stream_animation`, each `fn(&[u8], budget: usize, &mut FrameSink) -> Result<AnimInfo>`. `FrameSink = dyn FnMut(AnimFrame) -> bool`; returning `false` stops decoding and keeps the frames so far. `AnimFrame { image, delay_ms }` is the full canvas. `AnimInfo { frames, plays }`, `plays: None` = for ever. `decode_animation` collects the stream (`collect_animation`, `None` for fewer than two frames). `min_delay_ms` implements the 20 ms → 100 ms rule.
- **GIF**: composited in our code on an RGBA canvas of the logical screen; frames are clipped to the canvas; `gif::Repeat::Finite(n)` → `plays = n + 1` (no NETSCAPE extension reads as `Finite(0)`, i.e. once), `Infinite` → `None`. The budget counts `width × height × 4` per frame. **WebP**: `image-webp` composites; `LoopCount::Times(n)` → `n`. **JPEG XL**: `jxl-oxide` loads the file, then renders keyframe by keyframe; `num_loops == 0` → for ever.
- **Loader** (`blinzel-ui::loader`): `Stage::Anim` (key bits `3`), `ANIM_BUDGET = 256 << 20`, `ANIM_UPDATE = 200 ms`. `animation_stage` uploads each frame as it arrives, sends `Loaded { animation: Some(GpuAnimation { frames, plays, complete }) }` from the second frame on (intermediate deliveries `complete: false`, `plays: None`), and a final delivery with `complete: true`. A cancelled job sends nothing.
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
| A panic inside one of the animation decoders | Not turned into a decode error (the animation stage runs outside `decode::guard`): the CPU worker thread running it ends, the viewer keeps running and shows the still image |
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
| AC-05 | | open | |
| AC-06 | partial: `single_frames_budgets_and_plays` (`gif.rs`) and `plays_animations` (`webp.rs`) check the cut with a small budget; nothing checks the 256 MB the viewer passes, nor JPEG XL | open | |
| AC-07 | | open | |
| AC-08 | | open | |
| AC-09 | | open | |
| AC-10 | | open | |
| AC-11 | | open | |
| AC-12 | | open | |
| AC-13 | | open | |
| AC-14 | manual | open | |

## Decisions

- 2026-11-02: Extracted from the code. Recorded as current behavior: a decode error midway loses the file's loop count (the frames so far loop for ever), and the animation decoders run without `catch_unwind` (AGENTS.md §9 asks for it).
