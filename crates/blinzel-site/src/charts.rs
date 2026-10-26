//! Benchmark tables and inline SVG bar charts from a `blinzel-race`
//! report. Colours come from CSS classes, so charts follow the light and
//! dark theme.

use blinzel_core::json::Json;

/// HTML/XML text escaping.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn num(j: Option<&Json>) -> Option<f64> {
    match j? {
        Json::Num(n) => Some(*n),
        _ => None,
    }
}

fn median(j: Option<&Json>) -> Option<f64> {
    num(j?.get("median_ms"))
}

/// Milliseconds as text with the language's decimal separator.
fn ms(v: f64, sep: &str) -> String {
    if v >= 1000.0 {
        format!("{:.2} s", v / 1000.0).replace('.', sep)
    } else {
        format!("{v:.0} ms")
    }
}

/// A horizontal bar chart; `rows` are (label, value, value text). Each row
/// is a label and its value above a bar on a full-width track, so long
/// viewer names never collide with the bars on a phone. Every viewer
/// looks the same: the values make the comparison.
fn bars(title: &str, rows: &[(String, f64, String)]) -> String {
    // Narrow enough that the text stays legible when a phone scales it down.
    const W: f64 = 480.0;
    const ROW: f64 = 52.0;
    const BAR: f64 = 12.0;
    let max = rows.iter().map(|r| r.1).fold(0.0, f64::max).max(1e-9);
    let h = ROW * rows.len() as f64 - 10.0;
    let mut svg = format!(
        "<figure class=\"chart-card\"><svg class=\"chart\" viewBox=\"0 0 {W} {h}\" role=\"img\" aria-label=\"{}\">",
        esc(title)
    );
    for (i, (label, value, text)) in rows.iter().enumerate() {
        let y = ROW * i as f64;
        let w = (W * value / max).max(BAR);
        svg += &format!(
            "<g class=\"row\">\
             <text x=\"0\" y=\"{:.1}\" class=\"label\">{}</text>\
             <text x=\"{W}\" y=\"{:.1}\" text-anchor=\"end\" class=\"value\">{}</text>\
             <rect x=\"0\" y=\"{:.1}\" width=\"{W}\" height=\"{BAR}\" rx=\"6\" class=\"track\"/>\
             <rect x=\"0\" y=\"{:.1}\" width=\"{w:.1}\" height=\"{BAR}\" rx=\"6\" class=\"bar\"/>\
             </g>",
            y + 20.0,
            esc(label),
            y + 20.0,
            esc(text),
            y + 30.0,
            y + 30.0,
        );
    }
    svg + "</svg></figure>"
}

fn viewers(race: &Json) -> Result<&[Json], String> {
    race.get("viewers")
        .and_then(Json::as_array)
        .ok_or_else(|| "race.json: no viewers".to_string())
}

fn name(v: &Json) -> String {
    v.get("name")
        .and_then(Json::as_str)
        .unwrap_or("?")
        .to_string()
}

/// One chart or table: `open_warm`, `open_cold`, `next` (time, shorter
/// is better), `burst` (share of images shown), `table`, `machine`.
/// `raw` links the report itself; `t` translates the generator's keys,
/// `decimal_sep` and `machine_*` included.
pub fn race(
    kind: &str,
    race: &Json,
    raw: &str,
    t: impl Fn(&str) -> &'static str,
) -> Result<String, String> {
    let vs = viewers(race)?;
    let sep = t("decimal_sep");
    let none = || format!("<p class=\"pending\">{}</p>", t("none"));
    match kind {
        "open_warm" | "open_cold" | "next" => {
            let mut rows: Vec<(String, f64, String)> = vs
                .iter()
                .filter_map(|v| {
                    let m = median(v.get(kind))?;
                    Some((name(v), m, ms(m, sep)))
                })
                .collect();
            rows.sort_by(|a, b| a.1.total_cmp(&b.1));
            if rows.is_empty() {
                return Ok(none());
            }
            Ok(bars(t(kind), &rows))
        }
        "burst" => {
            let mut rows: Vec<(String, f64, String)> = vs
                .iter()
                .filter_map(|v| {
                    let b = v.get("burst")?;
                    let presses = num(b.get("presses"))?;
                    let shown = median(b.get("shown"))?;
                    (presses > 0.0).then(|| {
                        let share = shown / presses;
                        (name(v), share, format!("{shown:.0} / {presses:.0}"))
                    })
                })
                .collect();
            rows.sort_by(|a, b| b.1.total_cmp(&a.1));
            if rows.is_empty() {
                return Ok(none());
            }
            Ok(bars(t("burst"), &rows))
        }
        "table" => {
            let cell = |j: Option<&Json>| match (median(j), j.and_then(|j| num(j.get("p95_ms")))) {
                (Some(m), Some(p)) => format!("{} ({})", ms(m, sep), ms(p, sep)),
                _ => "–".into(),
            };
            let mut html = format!(
                "<div class=\"table-wrap\"><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
                t("viewer"),
                t("version"),
                t("open_warm"),
                t("open_cold"),
                t("next")
            );
            for v in vs {
                html += &format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    esc(&name(v)),
                    esc(v.get("version").and_then(Json::as_str).unwrap_or("?")),
                    cell(v.get("open_warm")),
                    cell(v.get("open_cold")),
                    cell(v.get("next"))
                );
            }
            html += &format!(
                "</tbody></table></div><p class=\"note\">{} · <a href=\"{}\">{}</a></p>",
                t("median"),
                esc(raw),
                t("raw")
            );
            Ok(html)
        }
        "machine" => {
            let Some(Json::Obj(m)) = race.get("machine") else {
                return Ok(String::new());
            };
            let items: String = m
                .iter()
                .map(|(k, v)| {
                    let v = match v {
                        Json::Str(s) => s.clone(),
                        Json::Num(n) => n.to_string(),
                        _ => String::new(),
                    };
                    let label = match t(&format!("machine_{k}")) {
                        "" => k.as_str(),
                        l => l,
                    };
                    format!("<div><dt>{}</dt><dd>{}</dd></div>", esc(label), esc(&v))
                })
                .collect();
            Ok(format!("<dl class=\"machine\">{items}</dl>"))
        }
        other => Err(format!("unknown race chart {other:?}")),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const SAMPLE: &str = r#"{
      "machine": {"cpu": "Test CPU", "gpu": "Test GPU"},
      "viewers": [
        {"name": "ImageGlass", "version": "8.8", "open_warm": {"median_ms": 1800.0, "p95_ms": 1900.0, "n": 3, "samples_ms": []},
         "open_cold": null, "next": {"median_ms": 800.0, "p95_ms": 850.0, "n": 3, "samples_ms": []},
         "burst": {"presses": 20, "shown": {"median_ms": 6.0, "p95_ms": 6.0, "n": 3, "samples_ms": []}, "settle": null}, "errors": []},
        {"name": "amigo-blinzel", "version": "0.1.0", "open_warm": {"median_ms": 270.0, "p95_ms": 330.0, "n": 3, "samples_ms": []},
         "open_cold": null, "next": {"median_ms": 9.0, "p95_ms": 15.0, "n": 3, "samples_ms": []},
         "burst": {"presses": 20, "shown": {"median_ms": 20.0, "p95_ms": 20.0, "n": 3, "samples_ms": []}, "settle": null}, "errors": []}
      ]}"#;

    #[test]
    fn charts_sort_without_highlight() {
        let j = Json::parse(SAMPLE).unwrap();
        let t = |k: &str| -> &'static str {
            match k {
                "open_warm" => "Open",
                "decimal_sep" => ",",
                "none" => "not measured",
                "machine_cpu" => "Processor",
                _ => "",
            }
        };
        let svg = race("open_warm", &j, "/race.json", t).unwrap();
        // Faster first; no viewer stands out.
        assert!(svg.find("amigo-blinzel").unwrap() < svg.find("ImageGlass").unwrap());
        assert!(!svg.contains("ours"));
        assert!(svg.contains("1,80 s") && svg.contains("270 ms"));
        assert!(
            race("open_cold", &j, "/race.json", t)
                .unwrap()
                .contains("not measured")
        );
        let burst = race("burst", &j, "/race.json", t).unwrap();
        assert!(burst.contains("20 / 20") && burst.contains("6 / 20"));
        let table = race("table", &j, "/race.json", t).unwrap();
        assert!(!table.contains("ours"));
        assert!(table.contains("href=\"/race.json\""));
        let machine = race("machine", &j, "/race.json", t).unwrap();
        assert!(machine.contains("<dt>Processor</dt>") && machine.contains("<dt>gpu</dt>"));
        assert!(race("bogus", &j, "/race.json", t).is_err());
        assert_eq!(esc("<a&\"b\">"), "&lt;a&amp;&quot;b&quot;&gt;");
    }
}
