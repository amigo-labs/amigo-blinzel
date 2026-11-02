//! The measurement plan, independent of the platform: the options of
//! `run`, which viewers it measures, and the order of the timed steps per
//! viewer with how a failure is recorded
//! (docs/specs/competitive-benchmark.md AC-07, AC-08, AC-10, AC-14).

use std::path::PathBuf;

use crate::report::Result_;
use crate::{number, option, usage};

/// The options of `run`.
#[derive(Debug, PartialEq)]
pub struct RunArgs {
    pub dir: PathBuf,
    pub reps: usize,
    pub cold: bool,
    /// Presses per burst, at most the corpus size − 2.
    pub burst: usize,
    /// `--only`: the names to measure, if given.
    pub only: Option<Vec<String>>,
    /// `--viewer name=path`, in order.
    pub viewers: Vec<(String, PathBuf)>,
    pub json: Option<PathBuf>,
}

impl RunArgs {
    /// `<dir> [--reps N] [--cold] [--burst N] [--only a,b] [--viewer name=path]...
    /// [--json out.json]` for a corpus of `count` images.
    pub fn parse(args: &[String], count: usize) -> Result<RunArgs, String> {
        let dir = PathBuf::from(args.first().ok_or_else(usage)?);
        let viewers = args
            .windows(2)
            .filter(|w| w[0] == "--viewer")
            .filter_map(|w| w[1].split_once('='))
            .map(|(name, path)| (name.trim().to_string(), PathBuf::from(path.trim())))
            .collect();
        Ok(RunArgs {
            dir,
            reps: number(args, "--reps", 10)?,
            cold: args.iter().any(|a| a == "--cold"),
            burst: number(args, "--burst", 20)?.min(count.saturating_sub(2)),
            only: option(args, "--only")
                .map(|o| o.split(',').map(|n| n.trim().to_string()).collect()),
            viewers,
            json: option(args, "--json").map(PathBuf::from),
        })
    }
}

/// The viewers to measure: the detected ones in order, each override
/// replacing the one of the same name or added at the end, then only those
/// named in `only` (case-insensitive).
pub fn select<V>(
    detected: Vec<V>,
    overrides: Vec<V>,
    only: Option<&[String]>,
    name: impl Fn(&V) -> &str,
) -> Vec<V> {
    let mut out = detected;
    for v in overrides {
        match out.iter().position(|c| name(c) == name(&v)) {
            Some(i) => out[i] = v,
            None => out.push(v),
        }
    }
    if let Some(only) = only {
        out.retain(|c| only.iter().any(|n| name(c).eq_ignore_ascii_case(n)));
    }
    out
}

/// The timed steps for one viewer, on the platform's screen.
pub trait Steps {
    /// Closes every instance, then starts the viewer with image 0 (`cold`:
    /// after emptying the file cache); ms until image 0 is on screen.
    fn open(&mut self, cold: bool) -> Result<f64, String>;
    /// After a warm start: one press and one burst, recorded in `r`.
    fn navigate(&mut self, r: &mut Result_) -> Result<(), String>;
    /// Closes every instance.
    fn close(&mut self);
}

/// Measures one viewer: an untimed first start (a failure skips the
/// viewer), `reps` warm starts each followed by navigation, then with
/// `cold` as many cold starts, ended by the first failure. Every failure
/// is recorded in the result; the next repetition goes on.
pub fn measure(
    s: &mut impl Steps,
    name: &str,
    version: String,
    reps: usize,
    cold: bool,
) -> Result_ {
    let mut r = Result_ {
        name: name.into(),
        version,
        ..Default::default()
    };
    // Untimed first start: loads the executable and its libraries.
    if let Err(e) = s.open(false) {
        r.errors.push(format!("warm-up: {e}"));
        s.close();
        return r;
    }
    for rep in 0..reps {
        match s.open(false) {
            Ok(ms) => {
                r.open_warm.push(ms);
                if let Err(e) = s.navigate(&mut r) {
                    r.errors.push(format!("rep {rep} navigate: {e}"));
                }
            }
            Err(e) => r.errors.push(format!("rep {rep} open: {e}")),
        }
        println!("  {name} {}/{reps}", rep + 1);
    }
    if cold {
        for rep in 0..reps {
            match s.open(true) {
                Ok(ms) => r.open_cold.push(ms),
                Err(e) => {
                    r.errors.push(format!("cold {rep}: {e}"));
                    break;
                }
            }
        }
    }
    s.close();
    r
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    #[test]
    fn run_options_have_defaults_and_the_burst_fits_the_corpus() {
        let a = RunArgs::parse(&args("corpus"), 24).unwrap();
        assert_eq!(
            a,
            RunArgs {
                dir: "corpus".into(),
                reps: 10,
                cold: false,
                burst: 20,
                only: None,
                viewers: Vec::new(),
                json: None,
            }
        );
        // A name with a space is one (quoted) argument; `--viewer x`
        // without a path is ignored.
        let mut v = args(
            "c --reps 3 --cold --burst 50 --viewer mine=/opt/v.app --viewer x --json out.json",
        );
        v.extend(["--only".into(), "IrfanView 64, mine".into()]);
        v.extend([
            "--viewer".into(),
            " amigo-blinzel = C:\\dev\\blinzel.exe".into(),
        ]);
        let a = RunArgs::parse(&v, 24).unwrap();
        assert_eq!((a.reps, a.cold, a.burst), (3, true, 22));
        assert_eq!(a.only, Some(vec!["IrfanView 64".into(), "mine".into()]));
        assert_eq!(
            a.viewers,
            [
                ("mine".to_string(), PathBuf::from("/opt/v.app")),
                (
                    "amigo-blinzel".to_string(),
                    PathBuf::from("C:\\dev\\blinzel.exe")
                ),
            ]
        );
        assert_eq!(a.json, Some("out.json".into()));
        assert_eq!(
            RunArgs::parse(&args("c --reps x"), 24).map(drop),
            Err("--reps: invalid digit found in string".into())
        );
        assert!(RunArgs::parse(&[], 24).unwrap_err().starts_with("usage:"));
    }

    #[test]
    fn overrides_replace_or_add_viewers_and_only_picks_by_name() {
        let detected = || {
            vec![
                ("amigo-blinzel", "a"),
                ("IrfanView 64", "i"),
                ("Windows Photos", "p"),
            ]
        };
        fn name<'a>(v: &'a (&str, &str)) -> &'a str {
            v.0
        }
        let overrides = vec![("IrfanView 64", "custom"), ("mine", "m")];
        assert_eq!(
            select(detected(), overrides.clone(), None, name),
            [
                ("amigo-blinzel", "a"),
                ("IrfanView 64", "custom"),
                ("Windows Photos", "p"),
                ("mine", "m"),
            ]
        );
        let only = ["irfanview 64".to_string(), "MINE".to_string()];
        assert_eq!(
            select(detected(), overrides, Some(&only), name),
            [("IrfanView 64", "custom"), ("mine", "m")]
        );
        assert_eq!(select(detected(), Vec::new(), Some(&[]), name), []);
    }

    /// Steps that answer from scripts and log what was asked.
    #[derive(Default)]
    struct Script {
        opens: VecDeque<Result<f64, String>>,
        navigations: VecDeque<Result<(), String>>,
        log: Vec<&'static str>,
    }

    impl Steps for Script {
        fn open(&mut self, cold: bool) -> Result<f64, String> {
            self.log.push(if cold { "cold" } else { "open" });
            self.opens.pop_front().unwrap_or(Ok(1.0))
        }
        fn navigate(&mut self, _: &mut Result_) -> Result<(), String> {
            self.log.push("navigate");
            self.navigations.pop_front().unwrap_or(Ok(()))
        }
        fn close(&mut self) {
            self.log.push("close");
        }
    }

    #[test]
    fn a_failed_repetition_is_recorded_and_the_next_one_goes_on() {
        let mut s = Script {
            opens: [
                Ok(5.0),
                Ok(10.0),
                Err("signature 0 not seen".into()),
                Ok(30.0),
            ]
            .into(),
            navigations: [Err("viewer window did not get the focus".into())].into(),
            ..Default::default()
        };
        let r = measure(&mut s, "qView", "7.0".into(), 3, false);
        assert_eq!((r.name.as_str(), r.version.as_str()), ("qView", "7.0"));
        assert_eq!(r.open_warm, [10.0, 30.0]);
        assert!(r.open_cold.is_empty());
        assert_eq!(
            r.errors,
            [
                "rep 0 navigate: viewer window did not get the focus",
                "rep 1 open: signature 0 not seen",
            ]
        );
        assert_eq!(
            s.log,
            [
                "open", "open", "navigate", "open", "open", "navigate", "close"
            ]
        );
    }

    #[test]
    fn a_failed_first_start_skips_the_viewer() {
        let mut s = Script {
            opens: [Err("signature 0 not seen".into())].into(),
            ..Default::default()
        };
        let r = measure(&mut s, "Preview", "11.0".into(), 10, true);
        assert_eq!(r.errors, ["warm-up: signature 0 not seen"]);
        assert!(r.open_warm.is_empty() && r.open_cold.is_empty() && r.bursts.is_empty());
        assert_eq!(s.log, ["open", "close"]);
    }

    #[test]
    fn cold_starts_follow_the_warm_ones_until_the_first_failure() {
        let mut s = Script {
            opens: [
                Ok(1.0),
                Ok(2.0),
                Ok(3.0),
                Ok(4.0),
                Ok(100.0),
                Err("run as administrator".into()),
                Ok(300.0),
            ]
            .into(),
            ..Default::default()
        };
        let r = measure(&mut s, "ImageGlass", "9".into(), 3, true);
        assert_eq!(r.open_warm, [2.0, 3.0, 4.0]);
        assert_eq!(r.open_cold, [100.0]);
        assert_eq!(r.errors, ["cold 1: run as administrator"]);
        let warm = ["open", "navigate"];
        let log = [
            &["open"][..],
            &warm,
            &warm,
            &warm,
            &["cold", "cold", "close"],
        ]
        .concat();
        assert_eq!(s.log, log);
        // Without --cold no cold start.
        let mut s = Script::default();
        assert!(
            measure(&mut s, "x", String::new(), 1, false)
                .open_cold
                .is_empty()
        );
        assert!(!s.log.contains(&"cold"));
    }
}
