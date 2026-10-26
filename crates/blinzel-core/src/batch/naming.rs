//! Output file names from a pattern, and finding a free one.
//!
//! Placeholders: `{name}` (source name without extension), `{ext}` (source
//! extension), `{n}` or `{n:3}` (running number, optionally zero-padded),
//! `{w}` and `{h}` (output size), `{date}` (capture date from EXIF, else
//! the file's modification time, as `YYYY-MM-DD`) or `{date:FORMAT}` with
//! `YYYY`, `YY`, `MM`, `DD`, `hh`, `mm` and `ss` in FORMAT.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_PATTERN: &str = "{name}";
const DEFAULT_DATE: &str = "YYYY-MM-DD";

/// A wall-clock date and time (local time, as the camera or the file
/// system's clock showed it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl Stamp {
    /// `fmt` with `YYYY`, `YY`, `MM`, `DD`, `hh`, `mm` and `ss` replaced;
    /// everything else is kept as it is.
    pub fn format(&self, fmt: &str) -> String {
        let mut out = String::new();
        let mut rest = fmt;
        'outer: while let Some(c) = rest.chars().next() {
            for (token, value) in [
                ("YYYY", format!("{:04}", self.year)),
                ("YY", format!("{:02}", self.year % 100)),
                ("MM", format!("{:02}", self.month)),
                ("DD", format!("{:02}", self.day)),
                ("hh", format!("{:02}", self.hour)),
                ("mm", format!("{:02}", self.minute)),
                ("ss", format!("{:02}", self.second)),
            ] {
                if let Some(r) = rest.strip_prefix(token) {
                    out.push_str(&value);
                    rest = r;
                    continue 'outer;
                }
            }
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
        out
    }
}

/// What a pattern can refer to.
pub struct Fields<'a> {
    pub name: &'a str,
    pub ext: &'a str,
    /// 1-based position in the batch.
    pub index: usize,
    pub width: u32,
    pub height: u32,
    /// None: `{date}` stays empty.
    pub date: Option<Stamp>,
}

#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Text(&'a str),
    Name,
    Ext,
    Index(usize),
    Width,
    Height,
    /// With its format.
    Date(&'a str),
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
            "date" => Part::Date(DEFAULT_DATE),
            t => match (t.strip_prefix("n:"), t.strip_prefix("date:")) {
                (Some(d), _) => match d.parse::<usize>() {
                    Ok(d) if d <= 9 => Part::Index(d),
                    _ => return Err(format!("unknown placeholder {{{t}}}")),
                },
                (_, Some(f)) if !f.is_empty() => Part::Date(f),
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
            Part::Date(fmt) => {
                if let Some(s) = f.date {
                    out.push_str(&s.format(fmt));
                }
            }
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

/// A point in time as a wall clock `utc_offset` seconds ahead of UTC
/// shows it.
pub fn stamp_of(t: SystemTime, utc_offset: i32) -> Stamp {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64 + utc_offset as i64;
    let days = secs.div_euclid(86_400);
    let time = secs.rem_euclid(86_400);
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
    Stamp {
        year: y.clamp(0, 9999) as u16,
        month: m as u8,
        day: d as u8,
        hour: (time / 3600) as u8,
        minute: (time / 60 % 60) as u8,
        second: (time % 60) as u8,
    }
}

/// Names handed out in this batch but maybe not on disk yet.
#[derive(Default)]
pub struct Claims(Mutex<HashSet<PathBuf>>);

impl Claims {
    /// Keeps `p` from being handed out by [`Claims::claim`].
    pub fn reserve(&self, p: PathBuf) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(p);
    }

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

    const TAKEN: Stamp = Stamp {
        year: 2024,
        month: 5,
        day: 17,
        hour: 9,
        minute: 4,
        second: 30,
    };

    fn fields() -> Fields<'static> {
        Fields {
            name: "IMG_0042",
            ext: "HEIC",
            index: 7,
            width: 1600,
            height: 1200,
            date: Some(TAKEN),
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
    fn date_formats() {
        let f = fields();
        assert_eq!(render("{date:DD.MM.YYYY}", &f).unwrap(), "17.05.2024");
        assert_eq!(
            render("{date:YYYYMMDD_hhmmss}", &f).unwrap(),
            "20240517_090430"
        );
        assert_eq!(render("{date:YY}-{name}", &f).unwrap(), "24-IMG_0042");
        assert_eq!(render("{date:hh:mm}", &f).unwrap(), "09_04");
        assert_eq!(
            render("{date:Tag DD}", &f).unwrap(),
            "Tag 17",
            "other letters stay"
        );
        let undated = Fields { date: None, ..f };
        assert_eq!(render("{date}_{name}", &undated).unwrap(), "_IMG_0042");
    }

    #[test]
    fn bad_patterns_are_refused() {
        assert!(validate("{name").is_err());
        assert!(validate("{size}").is_err());
        assert!(validate("{n:x}").is_err());
        assert!(validate("{date:}").is_err());
        assert!(validate("sub/{name}").is_err());
        assert!(validate("").is_err());
        assert!(validate("{name}_{n:4}").is_ok());
        assert!(validate("{date:DD.MM.YYYY hh-mm}").is_ok());
    }

    #[test]
    fn stamps() {
        assert_eq!(stamp_of(UNIX_EPOCH, 0).format(DEFAULT_DATE), "1970-01-01");
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800); // 2024-02-29
        assert_eq!(stamp_of(t, 0).format(DEFAULT_DATE), "2024-02-29");
        let late = t + std::time::Duration::from_secs(23 * 3600 + 30 * 60 + 5);
        assert_eq!(stamp_of(late, 0).format("hh:mm:ss"), "23:30:05");
        // Two hours ahead of UTC (CEST) is already the next day.
        assert_eq!(
            stamp_of(late, 2 * 3600).format("YYYY-MM-DD hh:mm"),
            "2024-03-01 01:30"
        );
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
