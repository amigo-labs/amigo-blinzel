//! Startup and interaction timing marks, reported as JSON.
//!
//! Marks are microseconds since an origin. The Windows app sets the origin
//! to the *process creation time* so the numbers include everything Windows
//! does before `main` (loader, Defender).

use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;

static MARKS: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());
static START: Mutex<Option<Instant>> = Mutex::new(None);
/// Microseconds between the origin (e.g. process creation) and `START`.
static ORIGIN_OFFSET_US: AtomicI64 = AtomicI64::new(0);

fn start() -> Instant {
    *START.lock().unwrap().get_or_insert_with(Instant::now)
}

/// Starts the clock now and declares that the true origin lies
/// `already_elapsed_us` earlier (time spent before `main`).
pub fn init(already_elapsed_us: i64) {
    *START.lock().unwrap() = Some(Instant::now());
    ORIGIN_OFFSET_US.store(already_elapsed_us, Ordering::Relaxed);
    MARKS.lock().unwrap().clear();
}

/// Microseconds since the origin.
pub fn now_us() -> f64 {
    start().elapsed().as_secs_f64() * 1e6 + ORIGIN_OFFSET_US.load(Ordering::Relaxed) as f64
}

/// Records a named mark at the current time.
pub fn mark(name: impl Into<String>) -> f64 {
    let t = now_us();
    MARKS.lock().unwrap().push((name.into(), t));
    t
}

/// Records a mark with an explicit value (e.g. a measured duration).
pub fn record(name: impl Into<String>, value_us: f64) {
    MARKS.lock().unwrap().push((name.into(), value_us));
}

pub fn marks() -> Vec<(String, f64)> {
    MARKS.lock().unwrap().clone()
}

/// Escapes a string for JSON.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Formats a number for JSON (finite, 3 decimals).
pub fn json_num(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.3}")
    } else {
        "null".into()
    }
}

/// The marks as a JSON object `{"name": microseconds, ...}`.
pub fn marks_json() -> String {
    let marks = marks();
    let body: Vec<String> = marks
        .iter()
        .map(|(k, v)| format!("{}: {}", json_str(k), json_num(*v)))
        .collect();
    format!("{{{}}}", body.join(", "))
}

/// Statistics over repeated measurements (milliseconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub min: f64,
    pub median: f64,
    pub p99: f64,
    pub mean: f64,
    pub n: usize,
}

impl Stats {
    pub fn of(samples: &[f64]) -> Option<Stats> {
        if samples.is_empty() {
            return None;
        }
        let mut s = samples.to_vec();
        s.sort_by(|a, b| a.total_cmp(b));
        let pick = |q: f64| s[((s.len() - 1) as f64 * q).round() as usize];
        Some(Stats {
            min: s[0],
            median: pick(0.5),
            p99: pick(0.99),
            mean: s.iter().sum::<f64>() / s.len() as f64,
            n: s.len(),
        })
    }

    pub fn json(&self) -> String {
        format!(
            "{{\"min_ms\": {}, \"median_ms\": {}, \"p99_ms\": {}, \"mean_ms\": {}, \"n\": {}}}",
            json_num(self.min),
            json_num(self.median),
            json_num(self.p99),
            json_num(self.mean),
            self.n
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_json() {
        init(1500);
        let t = mark("window");
        assert!(t >= 1500.0);
        record("decode", 12.5);
        let j = marks_json();
        assert!(j.starts_with("{\"window\": "), "{j}");
        assert!(j.ends_with("\"decode\": 12.500}"), "{j}");
        assert_eq!(json_str("a\"b\\c\n\u{1}"), "\"a\\\"b\\\\c\\n\\u0001\"");
        assert_eq!(json_num(f64::NAN), "null");
    }

    #[test]
    fn stats() {
        let s = Stats::of(&[5.0, 1.0, 3.0, 2.0, 4.0]).unwrap();
        assert_eq!(
            (s.min, s.median, s.p99, s.mean, s.n),
            (1.0, 3.0, 5.0, 3.0, 5)
        );
        assert!(Stats::of(&[]).is_none());
        assert!(s.json().contains("\"median_ms\": 3.000"));
    }
}
