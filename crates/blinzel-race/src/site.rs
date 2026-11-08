//! The benchmark file of amigo-labs.dev/blinzel from a `run --json` report
//! (docs/specs/competitive-benchmark.md AC-18 … AC-20). The format is
//! "Benchmark files" in amigo-labs/website `docs/project-data.md`: YAML with
//! texts in English and German, one group per metric, the median as value.

use std::fmt::Write as _;

use blinzel_core::json::Json;
use blinzel_core::timing::json_str;

use crate::corpus;

/// Where the site links "method".
const METHOD: &str =
    "https://github.com/amigo-labs/amigo-blinzel/blob/main/docs/specs/competitive-benchmark.md";

/// The viewer the site highlights as its own.
const OWN: &str = "amigo-blinzel";

/// German for the viewer names and versions the tool reports in English.
const GERMAN: [(&str, &str); 3] = [
    ("Windows Photos", "Windows Fotos"),
    ("Preview", "Vorschau"),
    ("store app", "Store-App"),
];

/// A text in English and German.
#[derive(Debug, Clone, PartialEq)]
struct Text {
    en: String,
    de: String,
}

impl Text {
    fn new(en: impl Into<String>, de: impl Into<String>) -> Text {
        Text {
            en: en.into(),
            de: de.into(),
        }
    }

    fn same(s: impl Into<String>) -> Text {
        let s = s.into();
        Text::new(s.clone(), s)
    }

    /// A name or version as reported, in German where `GERMAN` has it.
    fn reported(s: &str) -> Text {
        let de = GERMAN
            .iter()
            .find(|(en, _)| *en == s)
            .map_or(s, |(_, de)| de);
        Text::new(s, de)
    }

    /// Flow style: `"x"`, or `{ en: "x", de: "y" }` when the languages differ.
    fn inline(&self) -> String {
        if self.en == self.de {
            json_str(&self.en)
        } else {
            format!(
                "{{ en: {}, de: {} }}",
                json_str(&self.en),
                json_str(&self.de)
            )
        }
    }

    /// A top-level field, the languages on lines of their own.
    fn field(&self, key: &str, out: &mut String) {
        if self.en == self.de {
            let _ = writeln!(out, "{key}: {}", json_str(&self.en));
        } else {
            let _ = writeln!(
                out,
                "{key}:\n  en: {}\n  de: {}",
                json_str(&self.en),
                json_str(&self.de)
            );
        }
    }
}

/// One viewer's entry in a group.
struct Entry {
    name: Text,
    value: f64,
    detail: Option<Text>,
    own: bool,
}

struct Group {
    label: Text,
    /// `better` and `unit` where they differ from the file's.
    better_unit: Option<(&'static str, String)>,
    entries: Vec<Entry>,
}

fn num(v: &Json) -> Option<f64> {
    match v {
        Json::Num(n) => Some(*n),
        _ => None,
    }
}

/// Median and p95 of a statistic, `None` for `null`.
fn stats(v: Option<&Json>) -> Option<(f64, f64)> {
    let v = v?;
    Some((num(v.get("median_ms")?)?, num(v.get("p95_ms")?)?))
}

/// A number as the site shows it: at most one decimal.
fn number(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r:.1}")
    }
}

/// The p95 of a time: whole milliseconds, from one second on seconds with
/// two decimals (decimal comma in German).
fn p95(ms: f64) -> Text {
    if ms.round() < 1000.0 {
        Text::same(format!("p95 {} ms", number(ms.round())))
    } else {
        let s = format!("{:.2}", ms / 1000.0);
        Text::new(
            format!("p95 {s} s"),
            format!("p95 {} s", s.replace('.', ",")),
        )
    }
}

/// A CPU or GPU name without trademark signs and the integrated graphics
/// a CPU name mentions ("… with Radeon Graphics").
fn hardware(name: &str) -> String {
    let name = name.split(" with ").next().unwrap_or(name);
    let name = ["(TM)", "(R)", "™", "®"]
        .iter()
        .fold(name.to_string(), |s, mark| s.replace(mark, ""));
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Joins the clauses of the intro: "a", "a and b", "a, b, and c" (German
/// with a comma before "und", since every clause ends a subordinate one).
fn clauses(parts: &[(&str, &str)]) -> Text {
    let en: Vec<&str> = parts.iter().map(|p| p.0).collect();
    let de: Vec<&str> = parts.iter().map(|p| p.1).collect();
    let en = match en.as_slice() {
        [] => String::new(),
        [a] => a.to_string(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    };
    let de = match de.as_slice() {
        [] => String::new(),
        [a] => a.to_string(),
        [rest @ .., last] => format!("{}, und {last}", rest.join(", ")),
    };
    Text::new(en, de)
}

/// The benchmark file for the report `report`, read from `source` (named in
/// the header), or why there is none.
pub fn benchmark_file(report: &str, source: &str) -> Result<String, String> {
    let json = Json::parse(report).ok_or("not a blinzel-race report: invalid JSON")?;
    let machine = json
        .get("machine")
        .ok_or("not a blinzel-race report: no machine")?;
    let field = |k: &str| {
        machine
            .get(k)
            .and_then(Json::as_str)
            .ok_or(format!("not a blinzel-race report: machine has no {k}"))
    };
    let (platform, os) = match (machine.get("os_build"), machine.get("os")) {
        (Some(_), _) => {
            let b = field("os_build")?;
            (
                "Windows",
                Text::new(format!("Windows build {b}"), format!("Windows-Build {b}")),
            )
        }
        (None, Some(_)) => ("macOS", Text::same(field("os")?)),
        (None, None) => {
            return Err("not a blinzel-race report: machine has no os_build or os".into());
        }
    };
    let viewers = json
        .get("viewers")
        .and_then(Json::as_array)
        .ok_or("not a blinzel-race report: no viewers")?;

    let mut warm = Vec::new();
    let mut cold = Vec::new();
    let mut next = Vec::new();
    let mut burst = Vec::new();
    let mut presses = 0.0f64;
    let mut versions = Vec::new();
    for v in viewers {
        let name = v
            .get("name")
            .and_then(Json::as_str)
            .ok_or("not a blinzel-race report: a viewer has no name")?;
        let version = v.get("version").and_then(Json::as_str).unwrap_or("?");
        let own = name == OWN;
        let label = Text::reported(name);
        let time = |s: (f64, f64)| Entry {
            name: label.clone(),
            value: s.0.round(),
            detail: Some(p95(s.1)),
            own,
        };
        let mut measured = false;
        for (key, group) in [
            ("open_warm", &mut warm),
            ("open_cold", &mut cold),
            ("next", &mut next),
        ] {
            if let Some(s) = stats(v.get(key)) {
                group.push(time(s));
                measured = true;
            }
        }
        let b = v.get("burst");
        let shown = b
            .and_then(|b| b.get("shown"))
            .and_then(|s| num(s.get("median_ms")?));
        if let Some(shown) = shown {
            burst.push(Entry {
                name: label.clone(),
                value: shown,
                detail: None,
                own,
            });
            presses = presses.max(b.and_then(|b| num(b.get("presses")?)).unwrap_or(0.0));
            measured = true;
        }
        if measured {
            let v = if version.starts_with(|c: char| c.is_ascii_digit()) {
                Text::new(
                    format!("{name} {version}"),
                    format!("{} {version}", label.de),
                )
            } else {
                let ver = Text::reported(version);
                Text::new(
                    format!("{name} ({})", ver.en),
                    format!("{} ({})", label.de, ver.de),
                )
            };
            versions.push(v);
        }
    }
    if versions.is_empty() {
        return Err("nothing measured: every value in the report is null".into());
    }

    let presses = number(presses);
    let groups = [
        Group {
            label: Text::new("Open (warm)", "Öffnen (warm)"),
            better_unit: None,
            entries: warm,
        },
        Group {
            label: Text::new("Open (cold)", "Öffnen (kalt)"),
            better_unit: None,
            entries: cold,
        },
        Group {
            label: Text::new("Next image", "Nächstes Bild"),
            better_unit: None,
            entries: next,
        },
        Group {
            label: Text::new("Key repeat: images shown", "Dauerfeuer: gezeigte Bilder"),
            better_unit: Some(("higher", format!("/ {presses}"))),
            entries: burst,
        },
    ];
    let has = |i: usize| !groups[i].entries.is_empty();

    let mut measures = vec![(
        "from process start until the image is on screen",
        "vom Prozessstart, bis das Bild auf dem Bildschirm ist",
    )];
    if has(2) {
        measures.push((
            "from a key press until the next image appears",
            "vom Tastendruck, bis das nächste Bild erscheint",
        ));
    }
    let burst_en =
        format!("how many of {presses} presses at 30 Hz (a held arrow key) show an image");
    let burst_de = format!(
        "wie viele von {presses} Tastendrücken mit 30 Hz (gehaltene Pfeiltaste) ein Bild zeigen"
    );
    if has(3) {
        measures.push((&burst_en, &burst_de));
    }
    let measures = clauses(&measures);
    let (cold_en, cold_de) = if has(1) {
        (
            " Cold: with the file cache emptied before each start.",
            " Kalt: mit vor jedem Start geleertem Dateicache.",
        )
    } else {
        ("", "")
    };
    let spec = corpus::Spec::default();
    let count = spec.count;
    let mp = u64::from(spec.width) * u64::from(spec.height) / 1_000_000;
    let list = |f: fn(&Text) -> &str| versions.iter().map(f).collect::<Vec<_>>().join(", ");
    let intro = Text::new(
        format!(
            "Measured from the outside, the same way for every viewer, with the open tool \
             blinzel-race — {}.{cold_en} {count} JPEGs of {mp} megapixels; median over several \
             repetitions. Measured versions — {}.",
            measures.en,
            list(|t| &t.en)
        ),
        format!(
            "Gemessen von außen, für alle Viewer gleich, mit dem offenen Werkzeug blinzel-race \
             — {}.{cold_de} {count} JPEGs mit {mp} Megapixeln; Median über mehrere \
             Wiederholungen. Gemessene Versionen — {}.",
            measures.de,
            list(|t| &t.de)
        ),
    );

    let monitors: usize = field("monitors")?.parse().unwrap_or(1);
    let monitor = field("monitor")?.replace('x', "×");
    let (screens_en, screens_de) = if monitors == 1 {
        (
            format!("1 monitor ({monitor})"),
            format!("1 Monitor ({monitor})"),
        )
    } else {
        (
            format!("{monitors} monitors (main {monitor})"),
            format!("{monitors} Monitore (Hauptmonitor {monitor})"),
        )
    };
    let (cpu, cores, ram, gpu) = (
        hardware(field("cpu")?),
        field("logical_cpus")?,
        field("ram_gb")?,
        hardware(field("gpu")?),
    );
    let machine = Text::new(
        format!(
            "{cpu} ({cores} logical cores), {ram} GB memory, {gpu}, {screens_en}, {}",
            os.en
        ),
        format!(
            "{cpu} ({cores} logische Kerne), {ram} GB Arbeitsspeicher, {gpu}, {screens_de}, {}",
            os.de
        ),
    );

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Generated by `blinzel-race site` from {source}; do not edit by hand.\n\
         # Values are medians: milliseconds for opening and the next image (p95 in\n\
         # `detail`), images shown of {presses} presses for key repeat."
    );
    let title = match platform {
        "Windows" => Text::new(
            "Windows: opening, next image, key repeat",
            "Windows: Öffnen, nächstes Bild, Dauerfeuer",
        ),
        _ => Text::new(
            "macOS: opening, next image, key repeat",
            "macOS: Öffnen, nächstes Bild, Dauerfeuer",
        ),
    };
    title.field("title", &mut out);
    intro.field("intro", &mut out);
    out.push_str("better: lower\nunit: ms\n");
    machine.field("machine", &mut out);
    let _ = writeln!(out, "method: {METHOD}\ngroups:");
    for g in groups.iter().filter(|g| !g.entries.is_empty()) {
        let _ = writeln!(out, "  - label: {}", g.label.inline());
        if let Some((better, unit)) = &g.better_unit {
            let _ = writeln!(out, "    better: {better}\n    unit: {}", json_str(unit));
        }
        out.push_str("    results:\n");
        for e in &g.entries {
            let _ = write!(
                out,
                "      - {{ name: {}, value: {}",
                e.name.inline(),
                number(e.value)
            );
            if let Some(d) = &e.detail {
                let _ = write!(out, ", detail: {}", d.inline());
            }
            if e.own {
                out.push_str(", own: true");
            }
            out.push_str(" }\n");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    /// A macOS report: one monitor, a cold series, a viewer without next
    /// image and key repeat, and one that measured nothing.
    const MACOS: &str = r#"{
  "machine": {"cpu": "Apple M2", "logical_cpus": "8", "ram_gb": "16", "gpu": "Apple M2", "monitor": "2560x1600", "monitors": "1", "os": "macOS 15.1"},
  "viewers": [
    {"name": "amigo-blinzel", "version": "1.4.0", "open_warm": {"median_ms": 61.4, "p95_ms": 70.6, "n": 3, "samples_ms": [61.4, 50.0, 70.6]}, "open_cold": {"median_ms": 140.5, "p95_ms": 1004.9, "n": 2, "samples_ms": [140.5, 1004.9]}, "next": {"median_ms": 9.2, "p95_ms": 12.0, "n": 2, "samples_ms": [9.2, 12.0]}, "burst": {"presses": 20, "shown": {"median_ms": 19.5, "p95_ms": 20.0, "n": 2, "samples_ms": [19.0, 20.0]}, "settle": null}, "errors": []},
    {"name": "Preview", "version": "11.0", "open_warm": {"median_ms": 480.0, "p95_ms": 512.0, "n": 1, "samples_ms": [480.0]}, "open_cold": null, "next": null, "burst": {"presses": 20, "shown": null, "settle": null}, "errors": ["rep 0 navigate: signature 1 not seen"]},
    {"name": "Pixea", "version": "?", "open_warm": null, "open_cold": null, "next": null, "burst": {"presses": 0, "shown": null, "settle": null}, "errors": ["warm-up: signature 0 not seen"]}
  ]
}"#;

    #[test]
    fn writes_every_measured_group_with_median_and_p95() {
        let file = benchmark_file(MACOS, "docs/race/macos.json").unwrap();
        let want = r#"# Generated by `blinzel-race site` from docs/race/macos.json; do not edit by hand.
# Values are medians: milliseconds for opening and the next image (p95 in
# `detail`), images shown of 20 presses for key repeat.
title:
  en: "macOS: opening, next image, key repeat"
  de: "macOS: Öffnen, nächstes Bild, Dauerfeuer"
intro:
  en: "Measured from the outside, the same way for every viewer, with the open tool blinzel-race — from process start until the image is on screen, from a key press until the next image appears, and how many of 20 presses at 30 Hz (a held arrow key) show an image. Cold: with the file cache emptied before each start. 24 JPEGs of 24 megapixels; median over several repetitions. Measured versions — amigo-blinzel 1.4.0, Preview 11.0."
  de: "Gemessen von außen, für alle Viewer gleich, mit dem offenen Werkzeug blinzel-race — vom Prozessstart, bis das Bild auf dem Bildschirm ist, vom Tastendruck, bis das nächste Bild erscheint, und wie viele von 20 Tastendrücken mit 30 Hz (gehaltene Pfeiltaste) ein Bild zeigen. Kalt: mit vor jedem Start geleertem Dateicache. 24 JPEGs mit 24 Megapixeln; Median über mehrere Wiederholungen. Gemessene Versionen — amigo-blinzel 1.4.0, Vorschau 11.0."
better: lower
unit: ms
machine:
  en: "Apple M2 (8 logical cores), 16 GB memory, Apple M2, 1 monitor (2560×1600), macOS 15.1"
  de: "Apple M2 (8 logische Kerne), 16 GB Arbeitsspeicher, Apple M2, 1 Monitor (2560×1600), macOS 15.1"
method: https://github.com/amigo-labs/amigo-blinzel/blob/main/docs/specs/competitive-benchmark.md
groups:
  - label: { en: "Open (warm)", de: "Öffnen (warm)" }
    results:
      - { name: "amigo-blinzel", value: 61, detail: "p95 71 ms", own: true }
      - { name: { en: "Preview", de: "Vorschau" }, value: 480, detail: "p95 512 ms" }
  - label: { en: "Open (cold)", de: "Öffnen (kalt)" }
    results:
      - { name: "amigo-blinzel", value: 141, detail: { en: "p95 1.00 s", de: "p95 1,00 s" }, own: true }
  - label: { en: "Next image", de: "Nächstes Bild" }
    results:
      - { name: "amigo-blinzel", value: 9, detail: "p95 12 ms", own: true }
  - label: { en: "Key repeat: images shown", de: "Dauerfeuer: gezeigte Bilder" }
    better: higher
    unit: "/ 20"
    results:
      - { name: "amigo-blinzel", value: 19.5, own: true }
"#;
        assert_eq!(file, want);
    }

    #[test]
    fn windows_machine_names_lose_trademarks_and_integrated_graphics() {
        assert_eq!(
            hardware("AMD Ryzen 7 PRO 6850H with Radeon Graphics"),
            "AMD Ryzen 7 PRO 6850H"
        );
        assert_eq!(hardware("AMD Radeon(TM) Graphics"), "AMD Radeon Graphics");
        assert_eq!(
            hardware("Intel(R) Core(TM) i7-1185G7"),
            "Intel Core i7-1185G7"
        );
        assert_eq!(p95(999.4), Text::same("p95 999 ms"));
        assert_eq!(p95(999.6), Text::new("p95 1.00 s", "p95 1,00 s"));
        assert_eq!(
            Text::reported("store app"),
            Text::new("store app", "Store-App")
        );
    }

    #[test]
    fn reports_without_a_value_or_the_report_shape_are_refused() {
        let err = |s: &str| benchmark_file(s, "r.json").unwrap_err();
        assert_eq!(err("{"), "not a blinzel-race report: invalid JSON");
        assert_eq!(err("{}"), "not a blinzel-race report: no machine");
        assert_eq!(
            err(r#"{"machine": {"cpu": "x"}, "viewers": []}"#),
            "not a blinzel-race report: machine has no os_build or os"
        );
        let nothing = MACOS
            .lines()
            .filter(|l| !l.contains("amigo-blinzel") && !l.contains("Preview"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            err(&nothing),
            "nothing measured: every value in the report is null"
        );
    }

    /// The file amigo-labs.dev shows is the one the published report gives.
    #[test]
    fn published_benchmark_file_is_generated_from_the_report() {
        let report = std::fs::read_to_string(format!("{ROOT}/docs/race/windows.json")).unwrap();
        let published =
            std::fs::read_to_string(format!("{ROOT}/docs/site/benchmarks/windows.yml")).unwrap();
        assert_eq!(
            benchmark_file(&report, "docs/race/windows.json").unwrap(),
            published,
            "regenerate: cargo run -p blinzel-race -- site docs/race/windows.json \
             docs/site/benchmarks/windows.yml"
        );
    }
}
