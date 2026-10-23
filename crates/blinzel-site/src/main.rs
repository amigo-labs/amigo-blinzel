//! Builds the website (M8): `website/src` (layout, stylesheet, one HTML
//! fragment per page and language) plus `website/data/race.json` (from
//! `blinzel-race`) and `CHANGELOG.md` → `website/dist`, ready for Cloudflare
//! Workers static assets. Charts are generated here as inline SVG; the
//! browser runs no chart library.
//!
//! ```text
//! cargo run -p blinzel-site [-- <website-dir> <changelog>]
//! ```

mod charts;
mod markdown;

use std::fs;
use std::path::{Path, PathBuf};

use blinzel_core::json::Json;

/// Pages in navigation order; `index` is the start page.
const PAGES: [&str; 9] = [
    "index",
    "features",
    "compare",
    "benchmarks",
    "method",
    "download",
    "changelog",
    "privacy",
    "signing",
];

/// Shown in the navigation bar; the rest is linked from the footer.
const NAV: [&str; 6] = [
    "index",
    "features",
    "compare",
    "benchmarks",
    "method",
    "download",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    De,
    En,
}

impl Lang {
    const ALL: [Lang; 2] = [Lang::De, Lang::En];

    fn code(self) -> &'static str {
        match self {
            Lang::De => "de",
            Lang::En => "en",
        }
    }

    fn other(self) -> Lang {
        match self {
            Lang::De => Lang::En,
            Lang::En => Lang::De,
        }
    }

    /// Short UI strings of the generator itself.
    fn t(self, key: &str) -> &'static str {
        match (self, key) {
            (Lang::De, "lang_name") => "Deutsch",
            (Lang::En, "lang_name") => "English",
            (Lang::De, "pending") => {
                "Die Referenzmessung steht noch aus. Sobald sie vorliegt, erscheinen hier die Zahlen mit Rohdaten."
            }
            (Lang::En, "pending") => {
                "The reference measurement is still to come. Once it exists, the numbers and raw data appear here."
            }
            (Lang::De, "viewer") => "Viewer",
            (Lang::En, "viewer") => "Viewer",
            (Lang::De, "version") => "Version",
            (Lang::En, "version") => "Version",
            (Lang::De, "open_warm") => "Öffnen (warm)",
            (Lang::En, "open_warm") => "Open (warm)",
            (Lang::De, "open_cold") => "Öffnen (kalt)",
            (Lang::En, "open_cold") => "Open (cold)",
            (Lang::De, "next") => "Nächstes Bild",
            (Lang::En, "next") => "Next image",
            (Lang::De, "burst") => "Dauerfeuer: gezeigte Bilder",
            (Lang::En, "burst") => "Key repeat: images shown",
            (Lang::De, "median") => "Median (p95)",
            (Lang::En, "median") => "median (p95)",
            (Lang::De, "raw") => "Rohdaten (JSON)",
            (Lang::En, "raw") => "Raw data (JSON)",
            (Lang::De, "none") => "Nicht gemessen.",
            (Lang::En, "none") => "Not measured.",
            (Lang::De, "decimal_sep") => ",",
            (Lang::En, "decimal_sep") => ".",
            (Lang::De, "machine_cpu") => "Prozessor",
            (Lang::En, "machine_cpu") => "Processor",
            (Lang::De, "machine_logical_cpus") => "Logische Kerne",
            (Lang::En, "machine_logical_cpus") => "Logical cores",
            (Lang::De, "machine_ram_gb") => "Arbeitsspeicher (GB)",
            (Lang::En, "machine_ram_gb") => "Memory (GB)",
            (Lang::De, "machine_gpu") => "Grafik",
            (Lang::En, "machine_gpu") => "Graphics",
            (Lang::De, "machine_monitor") => "Hauptmonitor",
            (Lang::En, "machine_monitor") => "Main monitor",
            (Lang::De, "machine_monitors") => "Monitore",
            (Lang::En, "machine_monitors") => "Monitors",
            (Lang::De, "machine_os_build") => "Windows-Build",
            (Lang::En, "machine_os_build") => "Windows build",
            _ => "",
        }
    }
}

struct Page {
    slug: &'static str,
    title: String,
    body: String,
}

/// Reads a fragment: the first line is `<!-- title: … -->`.
fn read_page(src: &Path, lang: Lang, slug: &'static str) -> Result<Page, String> {
    let path = src.join(lang.code()).join(format!("{slug}.html"));
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (first, body) = text.split_once('\n').unwrap_or((&text, ""));
    let title = first
        .trim()
        .strip_prefix("<!-- title:")
        .and_then(|t| t.strip_suffix("-->"))
        .map(|t| t.trim().to_string())
        .ok_or_else(|| format!("{}: first line must be <!-- title: … -->", path.display()))?;
    Ok(Page {
        slug,
        title,
        body: body.to_string(),
    })
}

fn url(lang: Lang, slug: &str) -> String {
    if slug == "index" {
        format!("/{}/", lang.code())
    } else {
        format!("/{}/{slug}", lang.code())
    }
}

/// Fills the `{{…}}` placeholders of a page body.
fn expand(body: &str, lang: Lang, race: Option<&Json>, changelog: &str) -> Result<String, String> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let end = rest[i..].find("}}").ok_or("unclosed {{")? + i;
        let key = rest[i + 2..end].trim();
        out.push_str(&placeholder(key, lang, race, changelog)?);
        rest = &rest[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn placeholder(
    key: &str,
    lang: Lang,
    race: Option<&Json>,
    changelog: &str,
) -> Result<String, String> {
    const RELEASES: &str = "https://github.com/amigo-labs/amigo-blinzel/releases/latest/download";
    Ok(match key {
        "version" => env!("CARGO_PKG_VERSION").to_string(),
        "download:x64" => format!("{RELEASES}/amigo-blinzel-setup-x64.exe"),
        "download:arm64" => format!("{RELEASES}/amigo-blinzel-setup-arm64.exe"),
        "download:sums" => format!("{RELEASES}/SHA256SUMS"),
        "changelog" => markdown::to_html(changelog),
        k if k.starts_with("race:") => match race {
            None => format!("<p class=\"pending\">{}</p>", lang.t("pending")),
            Some(r) => charts::race(&k[5..], r, |key| lang.t(key))?,
        },
        other => return Err(format!("unknown placeholder {{{{{other}}}}}")),
    })
}

fn render(layout: &str, lang: Lang, pages: &[Page], page: &Page) -> String {
    let nav: String = NAV
        .iter()
        .filter_map(|slug| pages.iter().find(|p| p.slug == *slug))
        .map(|p| {
            let current = if p.slug == page.slug {
                " aria-current=\"page\""
            } else {
                ""
            };
            format!("<a href=\"{}\"{current}>{}</a>", url(lang, p.slug), p.title)
        })
        .collect();
    let footer: String = pages
        .iter()
        .filter(|p| !NAV.contains(&p.slug))
        .map(|p| format!("<a href=\"{}\">{}</a>", url(lang, p.slug), p.title))
        .collect::<Vec<_>>()
        .join(" · ");
    let other = lang.other();
    layout
        .replace("{{lang}}", lang.code())
        .replace("{{title}}", &page.title)
        .replace("{{nav}}", &nav)
        .replace("{{footer}}", &footer)
        .replace("{{other_lang_url}}", &url(other, page.slug))
        .replace("{{other_lang_name}}", other.t("lang_name"))
        .replace("{{content}}", &page.body)
}

pub fn build(site: &Path, changelog: &Path) -> Result<PathBuf, String> {
    let src = site.join("src");
    let dist = site.join("dist");
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let layout = read(&src.join("layout.html"))?;
    let changelog = read(changelog).unwrap_or_default();
    let race_path = site.join("data").join("race.json");
    let race = match fs::read_to_string(&race_path) {
        Ok(t) => {
            Some(Json::parse(&t).ok_or_else(|| format!("{}: invalid JSON", race_path.display()))?)
        }
        Err(_) => None,
    };

    if dist.exists() {
        fs::remove_dir_all(&dist).map_err(|e| e.to_string())?;
    }
    for lang in Lang::ALL {
        let dir = dist.join(lang.code());
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut pages = Vec::new();
        for slug in PAGES {
            let mut p = read_page(&src, lang, slug)?;
            p.body = expand(&p.body, lang, race.as_ref(), &changelog)
                .map_err(|e| format!("{}/{slug}: {e}", lang.code()))?;
            pages.push(p);
        }
        for p in &pages {
            let html = render(&layout, lang, &pages, p);
            fs::write(dir.join(format!("{}.html", p.slug)), html).map_err(|e| e.to_string())?;
        }
    }
    for file in [
        "style.css",
        "lang.js",
        "favicon.svg",
        "index.html",
        "404.html",
        "_headers",
    ] {
        let from = src.join(file);
        if from.exists() {
            fs::copy(&from, dist.join(file)).map_err(|e| format!("{file}: {e}"))?;
        }
    }
    if race.is_some() {
        fs::copy(&race_path, dist.join("race.json")).map_err(|e| e.to_string())?;
    }
    Ok(dist)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let site = PathBuf::from(args.first().map_or("website", String::as_str));
    let changelog = PathBuf::from(args.get(1).map_or("CHANGELOG.md", String::as_str));
    match build(&site, &changelog) {
        Ok(dist) => println!("built {}", dist.display()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_placeholders() {
        let s = expand("a {{version}} b {{download:x64}}", Lang::De, None, "").unwrap();
        assert!(s.starts_with(&format!("a {} b https://", env!("CARGO_PKG_VERSION"))));
        assert!(s.ends_with("amigo-blinzel-setup-x64.exe"));
        let pending = expand("{{race:open_warm}}", Lang::En, None, "").unwrap();
        assert!(pending.contains("reference measurement"));
        assert!(expand("{{nope}}", Lang::De, None, "").is_err());
        assert!(expand("{{version", Lang::De, None, "").is_err());
    }

    /// The real website sources build, with and without measurements.
    #[test]
    fn builds_the_site() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let tmp = std::env::temp_dir().join(format!("blinzel-site-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        copy_dir(&root.join("website/src"), &tmp.join("src"));
        let dist = build(&tmp, &root.join("CHANGELOG.md")).unwrap();
        let html = fs::read_to_string(dist.join("de/benchmarks.html")).unwrap();
        assert!(html.contains("Referenzmessung"));
        assert!(!html.contains("{{"));

        fs::write(tmp.join("data/race.json"), charts::tests::SAMPLE).unwrap();
        let dist = build(&tmp, &root.join("CHANGELOG.md")).unwrap();
        let html = fs::read_to_string(dist.join("en/benchmarks.html")).unwrap();
        assert!(html.contains("<svg"));
        assert!(html.contains("ImageGlass"));
        for lang in ["de", "en"] {
            for p in PAGES {
                assert!(
                    dist.join(lang).join(format!("{p}.html")).is_file(),
                    "{lang}/{p}"
                );
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy_dir(&p, &to.join(e.file_name()));
            } else {
                fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
}
