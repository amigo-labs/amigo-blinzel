//! `blinzel-race`: measures image viewers from the outside (M8), the same
//! way for every candidate, by watching what reaches the screen.
//!
//! ```text
//! blinzel-race corpus <dir> [--count N]      signature test images (24 MP)
//! blinzel-race list                          detected viewers and versions
//! blinzel-race run <dir> [--reps N] [--cold] [--only a,b] [--viewer name=exe]... [--json out.json]
//! ```
//!
//! Run on an otherwise idle machine with one monitor, the console window
//! out of the way; `--cold` needs an elevated console (macOS: it asks for
//! the `sudo` password once, for `purge`).
//! On macOS `--viewer` takes an app bundle (`name=/Applications/X.app`),
//! and the terminal needs Screen Recording and Accessibility permission.

mod corpus;
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod frames;
mod signature;

// The measurement needs a screen capture with present times, synthetic
// key presses and process control: Windows (Desktop Duplication) and macOS
// (ScreenCaptureKit).
#[cfg(any(windows, target_os = "macos"))]
mod race;

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod sys;
#[cfg(windows)]
mod viewers;

#[cfg(target_os = "macos")]
#[path = "capture_macos.rs"]
mod capture;
#[cfg(target_os = "macos")]
#[path = "sys_macos.rs"]
mod sys;
#[cfg(target_os = "macos")]
#[path = "viewers_macos.rs"]
mod viewers;

use std::path::PathBuf;

fn usage() -> String {
    "usage:\n  blinzel-race corpus <dir> [--count N]\n  blinzel-race list\n  \
     blinzel-race run <dir> [--reps N] [--cold] [--burst N] [--only name,...] \
     [--viewer name=exe]... [--json out.json]"
        .into()
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn number(args: &[String], name: &str, default: usize) -> Result<usize, String> {
    option(args, name).map_or(Ok(default), |v| {
        v.parse().map_err(|e| format!("{name}: {e}"))
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("corpus") => corpus_cmd(&args[1..]),
        #[cfg(any(windows, target_os = "macos"))]
        Some("list") => measure::list(),
        #[cfg(any(windows, target_os = "macos"))]
        Some("run") => measure::run(&args[1..]),
        _ => Err(usage()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(2);
    }
}

fn corpus_cmd(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or_else(usage)?);
    let spec = corpus::Spec {
        count: number(args, "--count", corpus::Spec::default().count)?,
        ..Default::default()
    };
    corpus::write(&dir, &spec)
}

#[cfg(any(windows, target_os = "macos"))]
mod measure {
    use std::path::PathBuf;

    use super::{number, option, usage};
    use crate::{capture, corpus, race, sys, viewers};

    pub fn list() -> Result<(), String> {
        for v in viewers::detect() {
            println!("{:<24} {:<16} {:?}", v.name, v.version(), v.launch);
        }
        Ok(())
    }

    pub fn run(args: &[String]) -> Result<(), String> {
        let dir = PathBuf::from(args.first().ok_or_else(usage)?);
        let count = corpus::Spec::default().count;
        // Absolute paths with backslashes: some viewers (Photos) reject
        // mixed separators.
        let dir = std::path::absolute(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let files = corpus::files(&dir, count);
        if let Some(missing) = files.iter().find(|f| !f.is_file()) {
            return Err(format!(
                "{} missing: run `blinzel-race corpus {}` first",
                missing.display(),
                dir.display()
            ));
        }
        let opts = race::Options {
            files,
            reps: number(args, "--reps", 10)?,
            cold: args.iter().any(|a| a == "--cold"),
            burst: number(args, "--burst", 20)?.min(count - 2),
        };
        let mut candidates = viewers::detect();
        for (i, a) in args.iter().enumerate() {
            if a == "--viewer"
                && let Some(v) = args.get(i + 1).and_then(|s| viewers::parse_override(s))
            {
                candidates.retain(|c| c.name != v.name);
                candidates.push(v);
            }
        }
        if let Some(only) = option(args, "--only") {
            let names: Vec<&str> = only.split(',').map(str::trim).collect();
            candidates.retain(|c| names.iter().any(|n| c.name.eq_ignore_ascii_case(n)));
        }
        sys::prepare(opts.cold)?;
        let cap = capture::Capture::start()?;
        let machine = sys::machine(&cap);
        for (k, v) in &machine {
            println!("{k}: {v}");
        }
        let mut results = Vec::new();
        for v in &candidates {
            println!("{} ({})", v.name, v.version());
            let r = race::measure(&cap, v, &opts);
            print_result(&r);
            results.push(r);
        }
        if let Some(out) = option(args, "--json") {
            std::fs::write(out, race::to_json(&machine, &results)).map_err(|e| e.to_string())?;
            println!("wrote {out}");
        }
        Ok(())
    }

    fn print_result(r: &race::Result_) {
        let f = |v: &[f64]| match race::median_p95(v) {
            Some((m, p)) => format!("{m:7.1} ms (p95 {p:7.1})"),
            None => "      -".into(),
        };
        let shown: Vec<f64> = r.bursts.iter().map(|b| b.shown as f64).collect();
        println!("  open warm {}", f(&r.open_warm));
        println!("  open cold {}", f(&r.open_cold));
        println!("  next      {}", f(&r.next));
        println!(
            "  burst     {} of {} images shown (median)",
            race::median_p95(&shown).map_or(0.0, |x| x.0),
            r.bursts.first().map_or(0, |b| b.presses)
        );
        for e in &r.errors {
            println!("  ! {e}");
        }
    }
}
