//! `blinzel-race`: measures image viewers from the outside (M8), the same
//! way for every candidate, by watching what reaches the screen.
//!
//! ```text
//! blinzel-race corpus <dir> [--count N]      signature test images (24 MP)
//! blinzel-race list                          detected viewers and versions
//! blinzel-race run <dir> [--reps N] [--cold] [--only a,b] [--viewer name=exe]... [--json out.json]
//! blinzel-race site <report.json> <out.yml>  the benchmark file of amigo-labs.dev
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
mod site;

// The measurement needs a screen capture with present times, synthetic
// key presses and process control: Windows (Desktop Duplication) and macOS
// (ScreenCaptureKit).
#[cfg(any(windows, target_os = "macos", test))]
mod plan;
#[cfg(any(windows, target_os = "macos"))]
mod race;
#[cfg(any(windows, target_os = "macos", test))]
mod report;

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
     [--viewer name=exe]... [--json out.json]\n  blinzel-race site <report.json> <out.yml>"
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
        Some("site") => site_cmd(&args[1..]),
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

/// Writes the benchmark file amigo-labs.dev shows from a `run --json` report.
fn site_cmd(args: &[String]) -> Result<(), String> {
    let [report, out] = args else {
        return Err(usage());
    };
    let text = std::fs::read_to_string(report).map_err(|e| format!("{report}: {e}"))?;
    let file = site::benchmark_file(&text, &report.replace('\\', "/"))
        .map_err(|e| format!("{report}: {e}"))?;
    std::fs::write(out, file).map_err(|e| format!("{out}: {e}"))?;
    println!("wrote {out}");
    Ok(())
}

#[cfg(any(windows, target_os = "macos"))]
mod measure {
    use crate::{capture, corpus, plan, race, report, sys, viewers};

    pub fn list() -> Result<(), String> {
        for v in viewers::detect() {
            println!("{:<24} {:<16} {:?}", v.name, v.version(), v.launch);
        }
        Ok(())
    }

    pub fn run(args: &[String]) -> Result<(), String> {
        let count = corpus::Spec::default().count;
        let a = plan::RunArgs::parse(args, count)?;
        // Absolute paths with backslashes: some viewers (Photos) reject
        // mixed separators.
        let dir = std::path::absolute(&a.dir).map_err(|e| format!("{}: {e}", a.dir.display()))?;
        let opts = race::Options {
            files: corpus::check(&dir, count)?,
            reps: a.reps,
            cold: a.cold,
            burst: a.burst,
        };
        let overrides = a
            .viewers
            .into_iter()
            .map(|(name, path)| viewers::Viewer::custom(name, path));
        let candidates = plan::select(
            viewers::detect(),
            overrides.collect(),
            a.only.as_deref(),
            |v| &v.name,
        );
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
        if let Some(out) = a.json {
            std::fs::write(&out, report::to_json(&machine, &results))
                .map_err(|e| format!("{}: {e}", out.display()))?;
            println!("wrote {}", out.display());
        }
        Ok(())
    }

    fn print_result(r: &report::Result_) {
        let f = |v: &[f64]| match report::median_p95(v) {
            Some((m, p)) => format!("{m:7.1} ms (p95 {p:7.1})"),
            None => "      -".into(),
        };
        let shown: Vec<f64> = r.bursts.iter().map(|b| b.shown as f64).collect();
        println!("  open warm {}", f(&r.open_warm));
        println!("  open cold {}", f(&r.open_cold));
        println!("  next      {}", f(&r.next));
        println!(
            "  burst     {} of {} images shown (median)",
            report::median_p95(&shown).map_or(0.0, |x| x.0),
            r.bursts.first().map_or(0, |b| b.presses)
        );
        for e in &r.errors {
            println!("  ! {e}");
        }
    }
}
