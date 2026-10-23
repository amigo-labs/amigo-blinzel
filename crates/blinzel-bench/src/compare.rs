//! `blinzel-bench compare a.json b.json`: two runs side by side, e.g. the
//! shipped build and one compiled for x86-64-v3. Prints a Markdown table
//! and appends it to `$GITHUB_STEP_SUMMARY` when set.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;

struct Row {
    group: String,
    candidate: String,
    file: String,
    median: f64,
}

struct Run {
    label: String,
    cpu: String,
    build_features: String,
    rows: Vec<Row>,
}

pub fn compare(args: &[String]) -> Result<(), String> {
    let [a, b] = args else {
        return Err("usage: blinzel-bench compare <a.json> <b.json>".into());
    };
    let (a, b) = (load(Path::new(a))?, load(Path::new(b))?);
    let mut md = String::new();
    let _ = writeln!(md, "### Benchmark: {} vs. {}\n", a.label, b.label);
    let _ = writeln!(md, "| | {} | {} |\n|---|---|---|", a.label, b.label);
    let _ = writeln!(md, "| CPU | {} | {} |", a.cpu, b.cpu);
    let _ = writeln!(
        md,
        "| build features | {} | {} |\n",
        a.build_features, b.build_features
    );
    let _ = writeln!(
        md,
        "| group | candidate | file | {} ms | {} ms | factor |\n|---|---|---|--:|--:|--:|",
        a.label, b.label
    );
    for ra in &a.rows {
        let Some(rb) = b
            .rows
            .iter()
            .find(|r| r.group == ra.group && r.candidate == ra.candidate && r.file == ra.file)
        else {
            continue;
        };
        // JSON keeps 3 decimals: sub-microsecond timings read as 0.
        let factor = if ra.median > 0.0 && rb.median > 0.0 {
            format!("{:.2}x", ra.median / rb.median)
        } else {
            "-".into()
        };
        let _ = writeln!(
            md,
            "| {} | {} | {} | {:.3} | {:.3} | {factor} |",
            ra.group, ra.candidate, ra.file, ra.median, rb.median
        );
    }
    print!("{md}");
    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&summary)
            .and_then(|mut f| f.write_all(md.as_bytes()))
            .map_err(|e| format!("{summary}: {e}"))?;
    }
    Ok(())
}

/// Reads a file written by `write_json`, which puts every result object on
/// its own line.
fn load(path: &Path) -> Result<Run, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rows = text
        .lines()
        .filter_map(|l| {
            Some(Row {
                group: str_field(l, "group")?,
                candidate: str_field(l, "candidate")?,
                file: str_field(l, "file")?,
                median: num_field(l, "median_ms")?,
            })
        })
        .collect();
    Ok(Run {
        label: path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        cpu: str_field(&text, "cpu").unwrap_or_else(|| "?".into()),
        build_features: list_field(&text, "build_features").unwrap_or_else(|| "?".into()),
        rows,
    })
}

/// The string value of the first `"key": "…"` in `s`.
fn str_field(s: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\": \"");
    let rest = &s[s.find(&pat)? + pat.len()..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
    None
}

/// The number value of the first `"key": n` in `s`.
fn num_field(s: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\": ");
    let rest = &s[s.find(&pat)? + pat.len()..];
    rest.split([',', '}']).next()?.trim().parse().ok()
}

/// The elements of the first `"key": ["a", "b"]` in `s`, space-separated.
fn list_field(s: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\": [");
    let rest = &s[s.find(&pat)? + pat.len()..];
    let items: Vec<&str> = rest[..rest.find(']')?]
        .split(',')
        .map(|i| i.trim().trim_matches('"'))
        .filter(|i| !i.is_empty())
        .collect();
    Some(if items.is_empty() {
        "-".into()
    } else {
        items.join(" ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fields() {
        let line = r#"    {"group": "simd", "candidate": "a \"b\" [avx]", "file": "x.jpg", "stats": {"min_ms": 1.000, "median_ms": 2.500, "n": 3}}"#;
        assert_eq!(str_field(line, "group").as_deref(), Some("simd"));
        assert_eq!(
            str_field(line, "candidate").as_deref(),
            Some("a \"b\" [avx]")
        );
        assert_eq!(num_field(line, "median_ms"), Some(2.5));
        assert_eq!(num_field(line, "missing"), None);
        let head = r#"{"cpu": "AMD A", "build_features": ["sse4.2", "avx2"], "cpu_features": []}"#;
        assert_eq!(str_field(head, "cpu").as_deref(), Some("AMD A"));
        assert_eq!(
            list_field(head, "build_features").as_deref(),
            Some("sse4.2 avx2")
        );
        assert_eq!(list_field(head, "cpu_features").as_deref(), Some("-"));
    }
}
