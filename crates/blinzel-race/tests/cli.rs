//! The command line of `blinzel-race`: what it refuses before measuring
//! (docs/specs/competitive-benchmark.md AC-11, AC-16).

use std::process::{Command, Output};

fn race(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blinzel-race"))
        .args(args)
        .output()
        .unwrap()
}

fn assert_usage(args: &[&str]) {
    let out = race(args);
    assert_eq!(out.status.code(), Some(2), "{args:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with("usage:\n  blinzel-race corpus <dir>"),
        "{args:?}: {err}"
    );
    assert!(err.contains("blinzel-race list") && err.contains("blinzel-race run <dir>"));
    assert!(out.stdout.is_empty(), "{args:?}");
}

#[test]
fn unknown_commands_print_the_usage_and_exit_2() {
    assert_usage(&[]);
    assert_usage(&["race"]);
    assert_usage(&["--help"]);
}

/// Linux has no capture with present times: only `corpus` works.
#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn list_and_run_print_the_usage_on_linux() {
    assert_usage(&["list"]);
    assert_usage(&["run", "."]);
}

/// A missing corpus stops `run` before anything is measured.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn run_without_a_corpus_says_how_to_write_one() {
    let dir = std::env::temp_dir().join(format!("blinzel-race-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = race(&["run", dir.to_str().unwrap()]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("race-000.jpg missing: run `blinzel-race corpus "),
        "{err}"
    );
    assert!(out.stdout.is_empty(), "measured something: {err}");
}
