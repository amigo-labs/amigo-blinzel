//! `blinzel-bench` as a program: exit codes, output and files
//! (`docs/specs/benchmark-gate.md`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bench(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blinzel-bench"))
        .args(args)
        .output()
        .expect("blinzel-bench")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A fresh folder for one test.
fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("blinzel-bench-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// AC-07: an unreadable corpus folder says to write the corpus first.
#[test]
fn run_without_a_corpus_says_to_write_it_and_exits_2() {
    let missing = temp("no-corpus").join("missing");
    let o = bench(&["run", missing.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("(run `blinzel-bench corpus` first)"),
        "{}",
        stderr(&o)
    );
}

/// AC-15: an unknown subcommand prints the usage.
#[test]
fn an_unknown_subcommand_prints_the_usage_and_exits_2() {
    for args in [&["frobnicate"][..], &[]] {
        let o = bench(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        let e = stderr(&o);
        assert!(e.starts_with("usage:"), "{e}");
        for cmd in [
            "corpus", "run", "compare", "jpeg-ab", "thumbs", "resize", "batch",
        ] {
            assert!(e.contains(&format!("blinzel-bench {cmd} ")), "{cmd}: {e}");
        }
    }
}

/// AC-15: `batch --format` with another format fails before any work.
#[test]
fn batch_with_an_unknown_format_exits_2() {
    let dir = temp("batch-format");
    let o = bench(&["batch", dir.to_str().unwrap(), "--format", "bmp"]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(stderr(&o).trim(), "unknown format bmp");
    assert!(!dir.join("thumbs-12mp").exists());
}
