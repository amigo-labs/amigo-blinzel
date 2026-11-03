//! What a measurement found and how it is reported, independent of the
//! platform: images counted in a burst, medians and percentiles, and the
//! JSON report the website renders (docs/specs/competitive-benchmark.md).

use blinzel_core::timing::{json_num, json_str};

use crate::frames::Event;

/// One key-repeat burst.
#[derive(Default)]
pub struct Burst {
    pub presses: usize,
    /// Images that appeared during and after the burst.
    pub shown: usize,
    /// When the expected last image appeared, in ms after the last press
    /// (negative: before it); NaN when the burst ended on another image.
    pub settle_ms: f64,
}

/// Everything measured for one viewer.
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

/// The machine fields of a Windows report, in order.
#[cfg(any(windows, test))]
pub const MACHINE_WINDOWS: [&str; 7] = [
    "cpu",
    "logical_cpus",
    "ram_gb",
    "gpu",
    "monitor",
    "monitors",
    "os_build",
];

/// The machine fields of a macOS report, in order.
#[cfg(any(target_os = "macos", test))]
pub const MACHINE_MACOS: [&str; 7] = [
    "cpu",
    "logical_cpus",
    "ram_gb",
    "gpu",
    "monitor",
    "monitors",
    "os",
];

/// The machine description: `keys` (`MACHINE_WINDOWS` or `MACHINE_MACOS`)
/// with their values.
pub fn machine(keys: [&'static str; 7], values: [String; 7]) -> Vec<(&'static str, String)> {
    keys.into_iter().zip(values).collect()
}

/// When the image shown last first appeared, and its signature.
pub fn last_change(events: &[Event]) -> Option<(i64, usize)> {
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
pub fn transitions(events: &[Event]) -> usize {
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

/// What a burst of `presses` showed, from the frames since its first
/// press: the images that appeared, and when the expected last signature
/// `want` appeared relative to the last press at `last_press` (`ms`
/// converts clock ticks). A burst that ended on another image gets no
/// settle time, and the reason goes to `errors`.
pub fn burst(
    events: &[Event],
    presses: usize,
    last_press: i64,
    want: usize,
    ms: impl Fn(i64) -> f64,
    errors: &mut Vec<String>,
) -> Burst {
    let settle_ms = match last_change(events) {
        Some((at, s)) if s == want => ms(at - last_press),
        _ => {
            errors.push("burst ended on another image (presses dropped)".into());
            f64::NAN
        }
    };
    Burst {
        presses,
        shown: transitions(events),
        settle_ms,
    }
}

/// Median (mean of the middle two for an even count) and 95th percentile
/// (nearest rank), over the finite samples.
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

/// The JSON report: the machine, then one entry per viewer.
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
    use blinzel_core::json::Json;

    use super::*;

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

    #[test]
    fn burst_settles_on_the_expected_image_or_records_dropped_presses() {
        let at = |q: i64, s: Option<usize>| Event { at: q, sig: s };
        let ms = |t: i64| t as f64;
        let mut errors = Vec::new();
        // Four images, the last one (signature 3) 7 ticks after the last press.
        let seen = [
            at(0, Some(0)),
            at(10, None),
            at(20, Some(1)),
            at(30, Some(2)),
            at(47, Some(3)),
            at(60, Some(3)),
        ];
        let b = burst(&seen, 3, 40, 3, ms, &mut errors);
        assert_eq!((b.presses, b.shown, b.settle_ms), (3, 3, 7.0));
        assert!(errors.is_empty());
        // A fast viewer: the last image is there before the last press.
        let b = burst(&seen, 3, 50, 3, ms, &mut errors);
        assert_eq!(b.settle_ms, -3.0);
        // Dropped presses: the burst ends on another image.
        let b = burst(&seen[..4], 3, 40, 3, ms, &mut errors);
        assert_eq!(b.shown, 2);
        assert!(b.settle_ms.is_nan());
        assert_eq!(errors, ["burst ended on another image (presses dropped)"]);
    }

    fn keys(v: &Json) -> Vec<&str> {
        match v {
            Json::Obj(kv) => kv.iter().map(|(k, _)| k.as_str()).collect(),
            _ => panic!("not an object: {v:?}"),
        }
    }

    fn num(v: &Json) -> f64 {
        match v {
            Json::Num(n) => *n,
            _ => panic!("not a number: {v:?}"),
        }
    }

    const VIEWER: [&str; 7] = [
        "name",
        "version",
        "open_warm",
        "open_cold",
        "next",
        "burst",
        "errors",
    ];
    const STATS: [&str; 4] = ["median_ms", "p95_ms", "n", "samples_ms"];

    #[test]
    fn report_holds_the_machine_and_every_measurement() {
        assert_eq!(
            MACHINE_WINDOWS,
            [
                "cpu",
                "logical_cpus",
                "ram_gb",
                "gpu",
                "monitor",
                "monitors",
                "os_build"
            ]
        );
        assert_eq!(MACHINE_MACOS[..6], MACHINE_WINDOWS[..6]);
        assert_eq!(MACHINE_MACOS[6], "os");
        let m = machine(
            MACHINE_MACOS,
            ["a", "8", "16", "g", "1920x1080", "1", "macOS 15"].map(String::from),
        );
        assert_eq!(m[6], ("os", "macOS 15".to_string()));

        let measured = Result_ {
            name: "amigo-blinzel".into(),
            version: "1.3.0".into(),
            open_warm: vec![90.0, 80.0, 100.0],
            next: vec![17.0, f64::NAN],
            bursts: vec![
                Burst {
                    presses: 20,
                    shown: 19,
                    settle_ms: -12.5,
                },
                Burst {
                    presses: 20,
                    shown: 20,
                    settle_ms: f64::NAN,
                },
            ],
            errors: vec!["burst ended on another image (presses dropped)".into()],
            ..Default::default()
        };
        let failed = Result_ {
            name: "Pre\"view".into(),
            version: "?".into(),
            errors: vec!["warm-up: signature 0 not seen".into()],
            ..Default::default()
        };
        let json = Json::parse(&to_json(&m, &[measured, failed])).expect("valid JSON");
        let machine = json.get("machine").unwrap();
        assert_eq!(keys(machine), MACHINE_MACOS);
        assert_eq!(
            machine.get("monitor").and_then(Json::as_str),
            Some("1920x1080")
        );

        let viewers = json.get("viewers").and_then(Json::as_array).unwrap();
        assert_eq!(viewers.len(), 2);
        let v = &viewers[0];
        assert_eq!(keys(v), VIEWER);
        assert_eq!(v.get("name").and_then(Json::as_str), Some("amigo-blinzel"));
        assert_eq!(v.get("version").and_then(Json::as_str), Some("1.3.0"));
        let warm = v.get("open_warm").unwrap();
        assert_eq!(keys(warm), STATS);
        assert_eq!(num(warm.get("median_ms").unwrap()), 90.0);
        assert_eq!(num(warm.get("p95_ms").unwrap()), 100.0);
        assert_eq!(num(warm.get("n").unwrap()), 3.0);
        let samples: Vec<f64> = warm
            .get("samples_ms")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .map(num)
            .collect();
        assert_eq!(samples, [90.0, 80.0, 100.0]);
        assert_eq!(v.get("open_cold"), Some(&Json::Null));
        let next = v.get("next").unwrap();
        assert_eq!(num(next.get("median_ms").unwrap()), 17.0);
        assert_eq!(
            next.get("samples_ms").and_then(Json::as_array).unwrap()[1],
            Json::Null
        );
        let burst = v.get("burst").unwrap();
        assert_eq!(keys(burst), ["presses", "shown", "settle"]);
        assert_eq!(num(burst.get("presses").unwrap()), 20.0);
        assert_eq!(
            num(burst.get("shown").unwrap().get("median_ms").unwrap()),
            19.5
        );
        assert_eq!(
            num(burst.get("settle").unwrap().get("median_ms").unwrap()),
            -12.5
        );
        assert_eq!(v.get("errors").and_then(Json::as_array).unwrap().len(), 1);

        // Nothing measured: every statistic is null, the errors are kept.
        let v = &viewers[1];
        assert_eq!(keys(v), VIEWER);
        assert_eq!(v.get("name").and_then(Json::as_str), Some("Pre\"view"));
        for k in ["open_warm", "open_cold", "next"] {
            assert_eq!(v.get(k), Some(&Json::Null), "{k}");
        }
        let burst = v.get("burst").unwrap();
        assert_eq!(num(burst.get("presses").unwrap()), 0.0);
        assert_eq!(burst.get("shown"), Some(&Json::Null));
        assert_eq!(burst.get("settle"), Some(&Json::Null));
        let errors = v.get("errors").and_then(Json::as_array).unwrap();
        assert_eq!(errors[0].as_str(), Some("warm-up: signature 0 not seen"));
    }

    /// The published measurement the website renders has the same shape.
    #[test]
    fn published_report_has_the_reported_fields() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../website/data/race.json");
        let json = Json::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(keys(json.get("machine").unwrap()), MACHINE_WINDOWS);
        for v in json.get("viewers").and_then(Json::as_array).unwrap() {
            assert_eq!(keys(v), VIEWER);
            for k in ["open_warm", "open_cold", "next"] {
                let stats = v.get(k).unwrap();
                if *stats != Json::Null {
                    assert_eq!(keys(stats), STATS, "{k}");
                }
            }
            assert_eq!(
                keys(v.get("burst").unwrap()),
                ["presses", "shown", "settle"]
            );
        }
    }
}
