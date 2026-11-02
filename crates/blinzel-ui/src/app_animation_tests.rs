//! App-level tests of animations: playback, timing, navigation and
//! errors (`docs/specs/animations.md`).

use super::*;
use crate::testkit::{Harness, gif};

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
