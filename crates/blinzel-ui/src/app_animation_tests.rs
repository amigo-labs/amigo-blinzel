//! App-level tests of animations: playback, timing, navigation and
//! errors (`docs/specs/animations.md`).

use super::*;
use crate::testkit::{Harness, gif, gif_on_screen, webp};

/// `n` GIF frames of 50 ms.
fn frames(n: usize) -> Vec<u16> {
    vec![5; n]
}

/// A harness with `data` as `name` in the images' folder, opened.
fn open_gif(data: &[u8], name: &str) -> Harness {
    let mut h = Harness::new();
    h.write_file(name, data);
    h.open(name);
    h
}

/// The current image's animation: frames received, loop count, complete.
fn anim(h: &Harness) -> Option<(usize, Option<u32>, bool)> {
    h.app
        .animation()
        .map(|a| (a.frames.len(), a.plays, a.complete))
}

/// The current image is known to be a still (its animation stage said so).
fn is_still(h: &Harness) -> bool {
    let id = h.app.current.as_ref().unwrap().cache_id;
    matches!(h.app.animations.get(&id), Some(None))
}

#[test]
fn a_decode_error_after_two_frames_plays_them_with_the_files_loop_count() {
    // Three frames, loop count 1 (two plays), cut inside the third.
    let data = gif(64, 48, &frames(3), Some(1));
    let h = open_gif(&data[..data.len() - 4], "cut.gif");
    assert_eq!(anim(&h), Some((2, Some(2), true)));
}

#[test]
fn a_decode_error_before_the_second_frame_leaves_a_still() {
    let data = gif(64, 48, &frames(2), Some(1));
    let h = open_gif(&data[..data.len() - 4], "cut.gif");
    assert!(is_still(&h));
    assert!(!h.host().timers.contains_key(&Timer::Anim));
    assert_eq!(h.frame().images.len(), 1);
}

#[test]
fn a_panicking_decoder_plays_the_frames_so_far_and_the_viewer_goes_on() {
    let mut h = Harness::new();
    let name = format!("{}.gif", loader::PANICS_ON_THIRD_FRAME);
    h.write_file(&name, &gif(64, 48, &frames(4), Some(2)));
    h.write_png("next.png", 64, 48);
    // The pump returns only once the loader is idle: the job finished.
    h.open(&name);
    assert_eq!(anim(&h), Some((2, Some(3), true)));
    h.key(Key::Right);
    assert_eq!(h.current_name().as_deref(), Some("next.png"));
    assert_eq!(h.frame().images.len(), 1);
}

#[test]
fn a_failed_upload_plays_the_frames_uploaded_so_far_with_the_files_loop_count() {
    let mut h = Harness::new();
    h.write_file("a.gif", &gif(64, 48, &frames(4), Some(2)));
    // The still and two frames fit on the GPU, the third does not; the
    // fourth would fit again, but decoding stopped.
    h.fail_upload_after(3);
    h.open("a.gif");
    assert_eq!(anim(&h), Some((2, Some(3), true)));
    assert_eq!(uploads(&h), 3);
}

/// Uploads to the device so far.
fn uploads(h: &Harness) -> usize {
    h.app.device.uploads.lock().unwrap().len()
}

/// The frame of the current animation on screen.
fn shown_frame(h: &Harness) -> Option<usize> {
    let (img, _, _) = h.app.displayed()?;
    let a = h.app.animation()?;
    a.frames.iter().position(|f| Arc::ptr_eq(&f.0, &img))
}

/// The animation timer fires, and the frame it brings is drawn.
fn tick(h: &mut Harness) {
    h.app.on_anim_timer();
    h.pump();
}

fn anim_timer(h: &Harness) -> Option<u32> {
    h.host().timers.get(&Timer::Anim).copied()
}

#[test]
fn only_the_current_gif_decodes_its_animation_behind_its_still() {
    let mut h = Harness::new();
    for n in ["a.gif", "b.gif", "c.gif"] {
        h.write_file(n, &gif(64, 48, &frames(3), Some(0)));
    }
    h.open("b.gif");
    // Three stills, and the three frames of b.gif only.
    assert_eq!(uploads(&h), 6);
    assert_eq!(anim(&h), Some((3, None, true)));
    // Without its animation, the still is shown and the animation wanted
    // after it in priority.
    let id = h.app.current.as_ref().unwrap().cache_id;
    let a = h.app.animations.remove(&id).unwrap();
    let still = h.app.best_texture(id).unwrap();
    assert!(Arc::ptr_eq(&h.app.displayed().unwrap().0, &still));
    let jobs: Vec<_> = h
        .app
        .wanted_jobs()
        .into_iter()
        .map(|j| (j.cache_id, j.stage, j.priority))
        .collect();
    assert_eq!(jobs, [(id, Stage::Anim, 1)]);
    h.app.animations.insert(id, a);
    assert_eq!(shown_frame(&h), Some(0));
}

#[test]
fn each_frame_stays_for_its_delay_and_the_last_play_holds_the_last_frame() {
    // No NETSCAPE extension: it plays once. 0 cs plays as 100 ms.
    let mut h = open_gif(&gif(64, 48, &[5, 7, 0], None), "once.gif");
    assert_eq!(anim(&h), Some((3, Some(1), true)));
    assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(0), Some(50)));
    tick(&mut h);
    assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(1), Some(70)));
    tick(&mut h);
    assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(2), Some(100)));
    tick(&mut h);
    assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(2), None));
}

#[test]
fn an_animation_without_a_loop_limit_repeats_until_the_image_changes() {
    let mut h = open_gif(&gif(64, 48, &frames(3), Some(0)), "loop.gif");
    for i in 1..=30 {
        tick(&mut h);
        assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(i % 3), Some(50)));
    }
}

#[test]
fn leaving_an_animation_stops_and_drops_it_and_coming_back_plays_it_anew() {
    let mut h = Harness::new();
    h.write_file("a.gif", &gif(64, 48, &frames(3), Some(0)));
    h.write_png("b.png", 64, 48);
    h.open("a.gif");
    let id = h.app.current.as_ref().unwrap().cache_id;
    tick(&mut h);
    assert_eq!(shown_frame(&h), Some(1));
    h.key(Key::Right);
    assert_eq!(anim_timer(&h), None);
    assert!(!h.app.animations.contains_key(&id));
    let before = uploads(&h);
    h.key(Key::Left);
    // Decoded again (its still is cached), from the first frame.
    assert_eq!(uploads(&h), before + 3);
    assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(0), Some(50)));
}

#[test]
fn an_animated_jpeg_xl_plays_its_keyframes_at_their_ticks_and_loop_count() {
    // Three keyframes of 15, 21 and 9 ticks at 500/3 ticks per second,
    // two plays (made as described in `blinzel-core`'s JPEG XL tests).
    let data = include_bytes!("../../blinzel-core/testdata/anim-8x6-loop2.jxl");
    let mut h = open_gif(data, "a.jxl");
    assert_eq!(anim(&h), Some((3, Some(2), true)));
    // 30 ms, 42 ms, and 18 ms played as 100 ms; twice, then it holds.
    let mut seen = vec![(shown_frame(&h), anim_timer(&h))];
    for _ in 0..6 {
        tick(&mut h);
        seen.push((shown_frame(&h), anim_timer(&h)));
    }
    let play = [
        (Some(0), Some(30)),
        (Some(1), Some(42)),
        (Some(2), Some(100)),
    ];
    assert_eq!(seen, [&play[..], &play[..], &[(Some(2), None)]].concat());
}

#[test]
fn the_viewer_decodes_only_the_frames_within_256_mb() {
    // 1024 × 1024 canvas: 4 MiB per decoded frame, so 64 of the 65 fit.
    let delays = vec![5; 65];
    let data = gif_on_screen((1024, 1024), (1, 1), &delays, Some(0));
    let h = open_gif(&data, "long.gif");
    assert_eq!(anim(&h), Some((64, None, true)));
    // The still and the 64 frames that fit; the 65th was not decoded.
    assert_eq!(uploads(&h), 1 + 64);
}

/// Calls [`App::on_loaded`] (without pumping, so no other result is taken)
/// until `done` holds.
fn take_results_until(h: &mut Harness, done: impl Fn(&Harness) -> bool) {
    let t = std::time::Instant::now();
    while !done(h) {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(30),
            "waited in vain"
        );
        h.app.on_loaded();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn playback_starts_while_decoding_and_holds_the_last_frame_received_until_more_arrive() {
    let mut h = Harness::new();
    let name = format!("{}.gif", loader::SLOW_FRAMES);
    let path = h.write_file(&name, &gif(64, 48, &frames(12), Some(0)));
    h.app.open(path);
    take_results_until(&mut h, |h| anim(h).is_some());
    // The first delivery: frames so far, more to come; it plays at once.
    let (n, plays, complete) = anim(&h).unwrap();
    assert!(
        (2..12).contains(&n) && !complete,
        "{n} frames, complete {complete}"
    );
    assert_eq!(plays, None);
    assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(0), Some(50)));
    for i in 1..n {
        h.app.on_anim_timer();
        assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(i), Some(50)));
    }
    // At the last frame received so far it holds, without a timer.
    h.app.on_anim_timer();
    assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(n - 1), None));
    // More frames: it goes on with the next one.
    take_results_until(&mut h, |h| anim(h).is_some_and(|a| a.0 > n));
    assert_eq!((shown_frame(&h), anim_timer(&h)), (Some(n), Some(50)));
    h.pump();
    assert_eq!(anim(&h), Some((12, None, true)));
}

#[test]
fn the_frames_so_far_go_to_the_window_at_most_every_200_ms() {
    let h = Harness::new();
    let name = format!("{}.gif", loader::SLOW_FRAMES);
    let path = h.write_file(&name, &gif(64, 48, &frames(12), Some(1)));
    // The loader alone, without the App taking its results.
    h.app.loader.set_wanted(vec![Job {
        path,
        cache_id: 0x1000,
        stage: Stage::Anim,
        with_preview: false,
        priority: 0,
    }]);
    let mut got: Vec<(usize, Option<u32>, bool, f64)> = Vec::new();
    while got.last().is_none_or(|g| !g.2) {
        let l = h
            .app
            .loader
            .results
            .recv_timeout(std::time::Duration::from_secs(30));
        let l = l.expect("the animation was delivered");
        let a = l.animation.unwrap();
        got.push((a.frames.len(), a.plays, a.complete, l.decode_ms));
    }
    // 12 frames of at least 50 ms: the first two, then more at least
    // 200 ms apart (decode time at sending), then all with the loop count.
    let (last, partial) = got.split_last().unwrap();
    assert_eq!((last.0, last.1, last.2), (12, Some(2), true), "{got:?}");
    assert!(partial.len() >= 2, "{got:?}");
    assert_eq!(partial[0].0, 2, "{got:?}");
    for w in partial.windows(2) {
        assert!(w[1].0 > w[0].0 && w[1].3 - w[0].3 >= 200.0, "{got:?}");
    }
    assert!(partial.iter().all(|p| p.1.is_none() && !p.2), "{got:?}");
}

/// How the current image would be shown as a still (its animation set
/// aside): orientation and layout.
fn still_view(h: &mut Harness) -> (Orientation, Layout) {
    let id = h.app.current.as_ref().unwrap().cache_id;
    let a = h.app.animations.remove(&id).unwrap();
    let (img, o, l) = h.app.displayed().unwrap();
    assert!(Arc::ptr_eq(&img, &h.app.best_texture(id).unwrap()));
    h.app.animations.insert(id, a);
    (o, l)
}

/// Every frame of one play is shown like the still.
fn every_frame_like_the_still(h: &mut Harness, want: Orientation) {
    let still = still_view(h);
    assert_eq!(still.0, want);
    for i in 0..3 {
        let (_, o, l) = h.app.displayed().unwrap();
        assert_eq!((shown_frame(h), (o, l)), (Some(i), still), "frame {i}");
        tick(h);
    }
}

#[test]
fn view_mode_zoom_pan_rotation_and_exif_orientation_apply_to_every_frame() {
    // Stored 640 × 480, EXIF orientation 6: shown 480 × 640 upright.
    let data = webp(640, 480, &[50; 3], 0, 6);
    let mut h = open_gif(&data, "a.webp");
    assert_eq!(anim(&h), Some((3, None, true)));
    let exif = Orientation::from_exif(6);
    every_frame_like_the_still(&mut h, exif);
    h.key(Key::Digit(3));
    assert_eq!(h.mode(), ViewMode::ActualSize);
    every_frame_like_the_still(&mut h, exif);
    h.key(Key::Plus);
    assert!(h.view().zoom.is_some());
    every_frame_like_the_still(&mut h, exif);
    h.middle_drag((400.0, 300.0), (360.0, 200.0));
    assert!(h.view().center.is_some(), "panned");
    every_frame_like_the_still(&mut h, exif);
    h.key(Key::Letter('R'));
    every_frame_like_the_still(&mut h, exif.then(Orientation::NORMAL.rotated_cw()));
}
