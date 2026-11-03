//! The measurements, the same for every viewer:
//!
//! 1. **Open:** process start → the first image on screen (warm; cold after
//!    emptying the file cache, which needs administrator rights).
//! 2. **Next:** after 1.5 s idle (prefetching viewers are ready by then),
//!    one Right-arrow press → the next image on screen.
//! 3. **Burst:** `burst` presses at 30 Hz: how many images were actually
//!    shown, and how long after the last press the last image appeared.

use std::path::PathBuf;
use std::time::Duration;

use crate::capture::Capture;
use crate::plan;
use crate::report::{self, Result_, last_change};
use crate::signature::COLORS;
use crate::sys;
use crate::viewers::{Viewer, launch};

pub struct Options {
    pub files: Vec<PathBuf>,
    pub reps: usize,
    pub cold: bool,
    pub burst: usize,
}

fn sig(i: usize) -> usize {
    i % COLORS.len()
}

/// One cold or warm start of image 0; returns ms until it is visible.
fn open(cap: &Capture, v: &Viewer, opts: &Options, cold: bool) -> Result<f64, String> {
    sys::close_all(&v.process_name());
    std::thread::sleep(Duration::from_millis(500));
    if cold {
        sys::purge_cache()?;
    }
    sys::park_cursor(cap.monitor);
    cap.frames.clear();
    let t0 = sys::now();
    launch(v, &opts.files[0])?;
    let t = cap.frames.wait_for(t0, sig(0), Duration::from_secs(20))?;
    Ok(sys::ticks_to_ms(t - t0))
}

/// After a warm open: one press, then a burst.
fn navigate(cap: &Capture, v: &Viewer, opts: &Options, r: &mut Result_) -> Result<(), String> {
    std::thread::sleep(Duration::from_millis(1500));
    if !sys::focus(&v.process_name()) {
        return Err("viewer window did not get the focus".into());
    }
    std::thread::sleep(Duration::from_millis(300));
    let t1 = sys::now();
    sys::tap_right();
    let t = cap.frames.wait_for(t1, sig(1), Duration::from_secs(10))?;
    r.next.push(sys::ticks_to_ms(t - t1));

    std::thread::sleep(Duration::from_millis(1500));
    let t2 = sys::now();
    for _ in 0..opts.burst {
        if !sys::owns_foreground(&v.process_name()) {
            return Err("viewer lost the focus during the burst".into());
        }
        sys::tap_right();
        std::thread::sleep(Duration::from_micros(1_000_000 / 30));
    }
    let t3 = sys::now();
    // Let the viewer catch up (slow ones work through queued key presses),
    // then read what reached the screen.
    let want = sig(1 + opts.burst);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline
        && last_change(&cap.frames.since(t3)).is_none_or(|(_, s)| s != want)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(500));
    let events = cap.frames.since(t2);
    let burst = report::burst(
        &events,
        opts.burst,
        t3,
        want,
        sys::ticks_to_ms,
        &mut r.errors,
    );
    r.bursts.push(burst);
    Ok(())
}

/// The steps on this platform's screen.
struct Live<'a> {
    cap: &'a Capture,
    v: &'a Viewer,
    opts: &'a Options,
}

impl plan::Steps for Live<'_> {
    fn open(&mut self, cold: bool) -> Result<f64, String> {
        open(self.cap, self.v, self.opts, cold)
    }

    fn navigate(&mut self, r: &mut Result_) -> Result<(), String> {
        navigate(self.cap, self.v, self.opts, r)
    }

    fn close(&mut self) {
        sys::close_all(&self.v.process_name());
    }
}

/// Measures one viewer (`plan::measure`).
pub fn measure(cap: &Capture, v: &Viewer, opts: &Options) -> Result_ {
    let mut live = Live { cap, v, opts };
    plan::measure(&mut live, &v.name, v.version(), opts.reps, opts.cold)
}
