//! `blinzel-bench batch <dir> [--count N] [--threads T] [--format F]
//! [--edge E] [--from <dir>] [--keep <dir>]`: batch resizing throughput
//! on the 12-MP JPEG corpus of `thumbs` (read, decode, resample, encode,
//! write), or on the files of `--from`; `--keep` keeps the outputs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use blinzel_core::batch::plan::Item;
use blinzel_core::batch::run::{self, Native, Options, Progress};
use blinzel_core::batch::{Codec, Job, Output, Size};

use crate::{option, usage};

pub fn batch(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or_else(usage)?);
    let parse = |name: &str, default: usize| -> Result<usize, String> {
        option(args, name).map_or(Ok(default), |v| {
            v.parse().map_err(|_| format!("bad {name}"))
        })
    };
    let count = parse("--count", 48)?;
    let threads = parse(
        "--threads",
        std::thread::available_parallelism().map_or(4, |n| n.get()),
    )?;
    let edge = parse("--edge", 1920)? as u32;
    let codec = match option(args, "--format").unwrap_or("jpeg") {
        "jpeg" => Codec::Jpeg,
        "png" => Codec::Png,
        "webp" => Codec::Webp,
        "avif" => Codec::Avif,
        f => return Err(format!("unknown format {f}")),
    };
    let files: Vec<PathBuf> = match option(args, "--from") {
        Some(from) => {
            let mut v: Vec<PathBuf> = std::fs::read_dir(from)
                .map_err(|e| format!("{from}: {e}"))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file())
                .collect();
            v.sort();
            v
        }
        None => crate::thumbs::corpus(&dir.join("thumbs-12mp"), count)?,
    };
    let keep = option(args, "--keep").map(PathBuf::from);
    let out = keep.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("blinzel-bench-batch-{}", std::process::id()))
    });
    let job = Job {
        size: Size::LongEdge(edge),
        output: Output::Codec(codec),
        out_dir: out.clone(),
        ..Job::default()
    };
    let items: Vec<Item> = files.iter().cloned().map(Item::from).collect();
    let bytes = AtomicU64::new(0);
    let t = Instant::now();
    let sum = run::run(
        &job,
        &items,
        &Native,
        &Options {
            threads,
            ..Options::default()
        },
        &|p| match p {
            Progress::Done { bytes: b, .. } => {
                bytes.fetch_add(b, Ordering::Relaxed);
            }
            Progress::Failed { source, error, .. } => eprintln!("{}: {error}", source.display()),
        },
        &AtomicBool::new(false),
    );
    let secs = t.elapsed().as_secs_f64();
    if keep.is_none() {
        let _ = std::fs::remove_dir_all(&out);
    }
    println!(
        "{} of {} files (12 MP JPEG → {codec:?}, long edge {edge}) in {:.2} s on {threads} threads = {:.1}/s, {:.0} KiB per file",
        sum.done,
        files.len(),
        secs,
        sum.done as f64 / secs,
        bytes.load(Ordering::Relaxed) as f64 / 1024.0 / sum.done.max(1) as f64,
    );
    Ok(())
}
