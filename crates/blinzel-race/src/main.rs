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
//! out of the way; `--cold` needs an elevated console.

mod corpus;
mod signature;

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod race;
#[cfg(windows)]
mod sys;
#[cfg(windows)]
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
        #[cfg(windows)]
        Some("list") => win::list(),
        #[cfg(windows)]
        Some("run") => win::run(&args[1..]),
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

#[cfg(windows)]
mod win {
    use std::path::PathBuf;

    use super::{number, option, usage};
    use crate::{capture, corpus, race, viewers};

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
        // Physical pixels for monitor geometry and the mouse position.
        unsafe {
            use windows::Win32::UI::HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
            };
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let cap = capture::Capture::start()?;
        let machine = machine(&cap);
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

    fn machine(cap: &capture::Capture) -> Vec<(&'static str, String)> {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut mem = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = GlobalMemoryStatusEx(&mut mem);
        }
        let (l, t, r, b) = cap.monitor;
        vec![
            ("cpu", blinzel_core::kernels::cpu_name()),
            (
                "logical_cpus",
                std::thread::available_parallelism()
                    .map_or(0, |n| n.get())
                    .to_string(),
            ),
            (
                "ram_gb",
                format!("{:.0}", mem.ullTotalPhys as f64 / (1u64 << 30) as f64),
            ),
            ("gpu", cap.adapter.clone()),
            ("monitor", format!("{}x{}", r - l, b - t)),
            ("monitors", cap.monitors.to_string()),
            ("os_build", os_build().to_string()),
        ]
    }

    fn os_build() -> u32 {
        #[repr(C)]
        struct Info {
            size: u32,
            major: u32,
            minor: u32,
            build: u32,
            platform: u32,
            csd: [u16; 128],
        }
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        use windows::core::{s, w};
        unsafe {
            let Ok(ntdll) = GetModuleHandleW(w!("ntdll.dll")) else {
                return 0;
            };
            let Some(f) = GetProcAddress(ntdll, s!("RtlGetVersion")) else {
                return 0;
            };
            let f = std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(*mut Info) -> i32,
            >(f);
            let mut v = Info {
                size: std::mem::size_of::<Info>() as u32,
                major: 0,
                minor: 0,
                build: 0,
                platform: 0,
                csd: [0; 128],
            };
            if f(&mut v) == 0 { v.build } else { 0 }
        }
    }
}
