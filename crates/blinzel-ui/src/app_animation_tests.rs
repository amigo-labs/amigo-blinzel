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
    // The still and two frames fit on the GPU.
    h.fail_uploads_after(3);
    h.open("a.gif");
    assert_eq!(anim(&h), Some((2, Some(3), true)));
}
