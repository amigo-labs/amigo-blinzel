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

use blinzel_core::timing::{json_num, json_str};

use crate::capture::Capture;
use crate::signature::COLORS;
use crate::sys;
use crate::viewers::{Viewer, launch};

pub struct Options {
    pub files: Vec<PathBuf>,
    pub reps: usize,
    pub cold: bool,
    pub burst: usize,
}

#[derive(Default)]
pub struct Burst {
    pub presses: usize,
    pub shown: usize,
    pub settle_ms: f64,
}

#[derive(Default)]
pub struct Result_ {
    pub name: String,
    pub version: String,
    pub open_warm: Vec<f64>,
    pub open_cold: Vec<f64>,
    pub next: Vec<f64>,
    pub bursts: Vec<Burst>,
    pub errors: Vec<String>,
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
    let shown = transitions(&events);
    let settle_ms = match last_change(&events) {
        Some((at, s)) if s == want => sys::ticks_to_ms(at - t3),
        _ => {
            r.errors
                .push("burst ended on another image (presses dropped)".into());
            f64::NAN
        }
    };
    r.bursts.push(Burst {
        presses: opts.burst,
        shown,
        settle_ms,
    });
    Ok(())
}

/// When the image shown last first appeared, and its signature.
fn last_change(events: &[crate::frames::Event]) -> Option<(i64, usize)> {
    let mut out: Option<(i64, usize)> = None;
    for e in events {
        if let Some(s) = e.sig
            && out.is_none_or(|(_, last)| last != s)
        {
            out = Some((e.at, s));
        }
    }
    out
}

/// Images that appeared: signature changes between presented frames
/// (consecutive images always differ).
fn transitions(events: &[crate::frames::Event]) -> usize {
    let mut last = None;
    let mut n = 0;
    for e in events {
        if let Some(s) = e.sig
            && last != Some(s)
        {
            if last.is_some() {
                n += 1;
            }
            last = Some(s);
        }
    }
    n
}

pub fn measure(cap: &Capture, v: &Viewer, opts: &Options) -> Result_ {
    let mut r = Result_ {
        name: v.name.clone(),
        version: v.version(),
        ..Default::default()
    };
    // Untimed first start: loads the executable and its libraries.
    if let Err(e) = open(cap, v, opts, false) {
        r.errors.push(format!("warm-up: {e}"));
        sys::close_all(&v.process_name());
        return r;
    }
    for rep in 0..opts.reps {
        match open(cap, v, opts, false) {
            Ok(ms) => {
                r.open_warm.push(ms);
                if let Err(e) = navigate(cap, v, opts, &mut r) {
                    r.errors.push(format!("rep {rep} navigate: {e}"));
                }
            }
            Err(e) => r.errors.push(format!("rep {rep} open: {e}")),
        }
        println!("  {} {}/{}", v.name, rep + 1, opts.reps);
    }
    if opts.cold {
        for rep in 0..opts.reps {
            match open(cap, v, opts, true) {
                Ok(ms) => r.open_cold.push(ms),
                Err(e) => {
                    r.errors.push(format!("cold {rep}: {e}"));
                    break;
                }
            }
        }
    }
    sys::close_all(&v.process_name());
    r
}

/// Median (mean of the middle two for an even count) and 95th percentile
/// (nearest rank).
pub fn median_p95(v: &[f64]) -> Option<(f64, f64)> {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return None;
    }
    s.sort_by(f64::total_cmp);
    let n = s.len();
    let median = if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    };
    let p95 = s[((0.95 * n as f64).ceil() as usize).clamp(1, n) - 1];
    Some((median, p95))
}

fn stats_json(v: &[f64]) -> String {
    match median_p95(v) {
        Some((m, p)) => format!(
            "{{\"median_ms\": {}, \"p95_ms\": {}, \"n\": {}, \"samples_ms\": [{}]}}",
            json_num(m),
            json_num(p),
            v.len(),
            v.iter()
                .map(|x| json_num(*x))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => "null".into(),
    }
}

pub fn to_json(machine: &[(&str, String)], results: &[Result_]) -> String {
    let m: Vec<String> = machine
        .iter()
        .map(|(k, v)| format!("{}: {}", json_str(k), json_str(v)))
        .collect();
    let viewers: Vec<String> = results
        .iter()
        .map(|r| {
            let shown: Vec<f64> = r.bursts.iter().map(|b| b.shown as f64).collect();
            let settle: Vec<f64> = r.bursts.iter().map(|b| b.settle_ms).collect();
            format!(
                "    {{\"name\": {}, \"version\": {}, \"open_warm\": {}, \"open_cold\": {}, \"next\": {}, \
                 \"burst\": {{\"presses\": {}, \"shown\": {}, \"settle\": {}}}, \"errors\": [{}]}}",
                json_str(&r.name),
                json_str(&r.version),
                stats_json(&r.open_warm),
                stats_json(&r.open_cold),
                stats_json(&r.next),
                r.bursts.first().map_or(0, |b| b.presses),
                stats_json(&shown),
                stats_json(&settle),
                r.errors.iter().map(|e| json_str(e)).collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    format!(
        "{{\n  \"machine\": {{{}}},\n  \"viewers\": [\n{}\n  ]\n}}\n",
        m.join(", "),
        viewers.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::Event;

    #[test]
    fn counts_shown_images() {
        let ev = |s: Option<usize>| Event { at: 0, sig: s };
        let events = [
            ev(Some(1)),
            ev(None),
            ev(Some(1)),
            ev(Some(2)),
            ev(Some(4)),
            ev(None),
            ev(Some(4)),
            ev(Some(5)),
        ];
        assert_eq!(transitions(&events), 3);
        assert_eq!(transitions(&[]), 0);
        let at = |q: i64, s: usize| Event {
            at: q,
            sig: Some(s),
        };
        assert_eq!(
            last_change(&[at(1, 2), at(2, 3), at(3, 3), at(4, 4), at(5, 4)]),
            Some((4, 4))
        );
    }

    #[test]
    fn median_and_p95() {
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(median_p95(&v), Some((10.5, 19.0)));
        assert_eq!(median_p95(&[3.0, 1.0, 2.0]), Some((2.0, 3.0)));
        assert_eq!(median_p95(&[f64::NAN]), None);
    }
}
