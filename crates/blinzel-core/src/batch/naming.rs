//! Output file names from a pattern, and finding a free one.
//!
//! Placeholders: `{name}` (source name without extension), `{ext}` (source
//! extension), `{n}` or `{n:3}` (running number, optionally zero-padded),
//! `{w}` and `{h}` (output size), `{date}` (capture date from EXIF, else
//! the file's modification date, as `YYYY-MM-DD`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_PATTERN: &str = "{name}";

/// What a pattern can refer to.
pub struct Fields<'a> {
    pub name: &'a str,
    pub ext: &'a str,
    /// 1-based position in the batch.
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub date: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Text(&'a str),
    Name,
    Ext,
    Index(usize),
    Width,
    Height,
    Date,
}

fn parse(pattern: &str) -> Result<Vec<Part<'_>>, String> {
    let mut parts = Vec::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            parts.push(Part::Text(&rest[..open]));
        }
        let close = rest[open..]
            .find('}')
            .ok_or_else(|| format!("unclosed '{{' in “{pattern}”"))?;
        let token = &rest[open + 1..open + close];
        parts.push(match token {
            "name" => Part::Name,
            "ext" => Part::Ext,
            "n" => Part::Index(0),
            "w" => Part::Width,
            "h" => Part::Height,
            "date" => Part::Date,
            t => match t.strip_prefix("n:").map(str::parse::<usize>) {
                Some(Ok(d)) if d <= 9 => Part::Index(d),
                _ => return Err(format!("unknown placeholder {{{t}}}")),
            },
        });
        rest = &rest[open + close + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest));
    }
    Ok(parts)
}

/// Checks a pattern before a batch starts.
pub fn validate(pattern: &str) -> Result<(), String> {
    let parts = parse(pattern)?;
    if parts.is_empty() {
        return Err("empty file name pattern".into());
    }
    if parts
        .iter()
        .any(|p| matches!(p, Part::Text(t) if t.contains(['/', '\\'])))
    {
        return Err("the pattern makes a name, not a folder".into());
    }
    Ok(())
}

/// The file name (without extension) for `f`. Characters that are not
/// allowed in file names become `_`.
pub fn render(pattern: &str, f: &Fields) -> Result<String, String> {
    let mut out = String::new();
    for p in parse(pattern)? {
        match p {
            Part::Text(t) => out.push_str(t),
            Part::Name => out.push_str(f.name),
            Part::Ext => out.push_str(f.ext),
            Part::Index(d) => out.push_str(&format!("{:0d$}", f.index)),
            Part::Width => out.push_str(&f.width.to_string()),
            Part::Height => out.push_str(&f.height.to_string()),
            Part::Date => out.push_str(f.date),
        }
    }
    let clean: String = out
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let clean = clean.trim_end_matches(['.', ' ']).trim_start();
    Ok(if clean.is_empty() {
        "image".into()
    } else {
        clean.into()
    })
}

/// `YYYY-MM-DD` (UTC) of a point in time.
pub fn date_of(t: SystemTime) -> String {
    let days = t
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Names handed out in this batch but maybe not on disk yet.
#[derive(Default)]
pub struct Claims(Mutex<HashSet<PathBuf>>);

impl Claims {
    /// A path `dir/base.ext` that neither exists nor was handed out, with
    /// ` (2)`, ` (3)`, … appended as needed.
    pub fn claim(&self, dir: &Path, base: &str, ext: &str) -> PathBuf {
        let mut taken = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for i in 1u32.. {
            let name = if i == 1 {
                format!("{base}.{ext}")
            } else {
                format!("{base} ({i}).{ext}")
            };
            let p = dir.join(name);
            if !taken.contains(&p) && std::fs::symlink_metadata(&p).is_err() {
                taken.insert(p.clone());
                return p;
            }
        }
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Fields<'static> {
        Fields {
            name: "IMG_0042",
            ext: "HEIC",
            index: 7,
            width: 1600,
            height: 1200,
            date: "2024-05-17",
        }
    }

    #[test]
    fn placeholders() {
        let f = fields();
        assert_eq!(render("{name}", &f).unwrap(), "IMG_0042");
        assert_eq!(
            render("{date}_{n:3}_{w}x{h}-{ext}", &f).unwrap(),
            "2024-05-17_007_1600x1200-HEIC"
        );
        assert_eq!(render("web-{n}", &f).unwrap(), "web-7");
        assert_eq!(render("a:b?{name}. ", &f).unwrap(), "a_b_IMG_0042");
        assert_eq!(render("...", &f).unwrap(), "image");
    }

    #[test]
    fn bad_patterns_are_refused() {
        assert!(validate("{name").is_err());
        assert!(validate("{size}").is_err());
        assert!(validate("{n:x}").is_err());
        assert!(validate("sub/{name}").is_err());
        assert!(validate("").is_err());
        assert!(validate("{name}_{n:4}").is_ok());
    }

    #[test]
    fn dates() {
        assert_eq!(date_of(UNIX_EPOCH), "1970-01-01");
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800); // 2024-02-29
        assert_eq!(date_of(t), "2024-02-29");
    }

    #[test]
    fn claims_never_repeat_or_hit_existing_files() {
        let dir = std::env::temp_dir().join(format!("blinzel-claims-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        let c = Claims::default();
        assert_eq!(c.claim(&dir, "a", "jpg"), dir.join("a (2).jpg"));
        assert_eq!(c.claim(&dir, "a", "jpg"), dir.join("a (3).jpg"));
        assert_eq!(c.claim(&dir, "b", "jpg"), dir.join("b.jpg"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
