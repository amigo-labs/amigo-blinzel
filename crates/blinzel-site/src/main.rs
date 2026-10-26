//! Builds the website (M8): `website/src` (layout, stylesheet, one HTML
//! fragment per page and language) plus `website/data/race.json` and
//! `race-macos.json` (from `blinzel-race` on Windows and macOS) and
//! `CHANGELOG.md` → `website/dist`, ready for Cloudflare
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

/// Where the site is served: canonical links, Open Graph and the sitemap
/// need absolute URLs.
const SITE: &str = "https://blinzel.amigo-labs.dev";

/// The `blinzel-race` reports: placeholder prefix (`{{race:open_warm}}`)
/// and file in `website/data/`, published next to the pages.
const RACES: [(&str, &str); 2] = [("race", "race.json"), ("race-macos", "race-macos.json")];

/// One report, `None` until its reference measurement exists.
struct Race {
    prefix: &'static str,
    file: &'static str,
    data: Option<Json>,
}

/// Pages in navigation order; `index` is the start page.
const PAGES: [&str; 9] = [
    "index",
    "features",
    "batch",
    "compare",
    "benchmarks",
    "method",
    "download",
    "changelog",
    "privacy",
];

/// Shown in the navigation bar; the rest is linked from the footer.
const NAV: [&str; 7] = [
    "index",
    "features",
    "batch",
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
            (Lang::De, "og_locale") => "de_DE",
            (Lang::En, "og_locale") => "en_US",
            (Lang::De, "skip") => "Zum Inhalt springen",
            (Lang::En, "skip") => "Skip to content",
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
            (Lang::De, "machine_os") => "Betriebssystem",
            (Lang::En, "machine_os") => "Operating system",
            _ => "",
        }
    }
}

struct Page {
    slug: &'static str,
    title: String,
    description: String,
    body: String,
}

/// `<!-- key: value -->` on one line.
fn header<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.trim()
        .strip_prefix("<!--")?
        .trim_start()
        .strip_prefix(key)?
        .strip_prefix(':')?
        .strip_suffix("-->")
        .map(str::trim)
}

/// Reads a fragment: the first lines are `<!-- title: … -->` and
/// `<!-- description: … -->` (for search engines and link previews).
fn read_page(src: &Path, lang: Lang, slug: &'static str) -> Result<Page, String> {
    let path = src.join(lang.code()).join(format!("{slug}.html"));
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut lines = text.splitn(3, '\n');
    let mut next = |key: &str| {
        lines
            .next()
            .and_then(|l| header(l, key))
            .map(str::to_string)
            .ok_or_else(|| format!("{}: missing <!-- {key}: … -->", path.display()))
    };
    let title = next("title")?;
    let description = next("description")?;
    Ok(Page {
        slug,
        title,
        description,
        body: lines.next().unwrap_or("").to_string(),
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
fn expand(body: &str, lang: Lang, races: &[Race], changelog: &str) -> Result<String, String> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let end = rest[i..].find("}}").ok_or("unclosed {{")? + i;
        let key = rest[i + 2..end].trim();
        out.push_str(&placeholder(key, lang, races, changelog)?);
        rest = &rest[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn placeholder(key: &str, lang: Lang, races: &[Race], changelog: &str) -> Result<String, String> {
    const RELEASES: &str = "https://github.com/amigo-labs/amigo-blinzel/releases/latest/download";
    if let Some((prefix, kind)) = key.split_once(':')
        && let Some(race) = races.iter().find(|r| r.prefix == prefix)
    {
        return match &race.data {
            None => Ok(format!("<p class=\"pending\">{}</p>", lang.t("pending"))),
            Some(r) => charts::race(kind, r, &format!("/{}", race.file), |k| lang.t(k)),
        };
    }
    Ok(match key {
        "version" => env!("CARGO_PKG_VERSION").to_string(),
        "download:x64" => format!("{RELEASES}/amigo-blinzel-setup-x64.exe"),
        "download:arm64" => format!("{RELEASES}/amigo-blinzel-setup-arm64.exe"),
        "download:macos" => format!("{RELEASES}/amigo-blinzel-macos.zip"),
        "download:linux-x64" => format!("{RELEASES}/amigo-blinzel-linux-x64.tar.gz"),
        "download:linux-arm64" => format!("{RELEASES}/amigo-blinzel-linux-arm64.tar.gz"),
        "download:sums" => format!("{RELEASES}/SHA256SUMS"),
        "changelog" => markdown::to_html(changelog),
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
        .collect();
    // A page that brings its own <h1> (the start page's hero) gets no
    // generic page heading.
    let heading = if page.body.contains("<h1") {
        String::new()
    } else {
        format!("<div class=\"page-head\"><h1>{}</h1></div>", page.title)
    };
    let other = lang.other();
    layout
        .replace("{{lang}}", lang.code())
        .replace("{{slug}}", page.slug)
        .replace("{{skip}}", lang.t("skip"))
        .replace("{{version}}", env!("CARGO_PKG_VERSION"))
        .replace("{{heading}}", &heading)
        .replace("{{title}}", &page.title)
        .replace("{{description}}", &charts::esc(&page.description))
        .replace("{{site}}", SITE)
        .replace("{{canonical}}", &format!("{SITE}{}", url(lang, page.slug)))
        .replace("{{url_de}}", &format!("{SITE}{}", url(Lang::De, page.slug)))
        .replace("{{url_en}}", &format!("{SITE}{}", url(Lang::En, page.slug)))
        .replace("{{og_locale}}", lang.t("og_locale"))
        .replace("{{nav}}", &nav)
        .replace("{{footer}}", &footer)
        .replace("{{other_lang_url}}", &url(other, page.slug))
        .replace("{{other_lang_name}}", other.t("lang_name"))
        .replace("{{other_lang}}", other.code())
        .replace("{{content}}", &page.body)
}

pub fn build(site: &Path, changelog: &Path) -> Result<PathBuf, String> {
    let src = site.join("src");
    let dist = site.join("dist");
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let layout = read(&src.join("layout.html"))?;
    let changelog = read(changelog).unwrap_or_default();
    let mut races = Vec::new();
    for (prefix, file) in RACES {
        let path = site.join("data").join(file);
        let data = match fs::read_to_string(&path) {
            Ok(t) => {
                Some(Json::parse(&t).ok_or_else(|| format!("{}: invalid JSON", path.display()))?)
            }
            Err(_) => None,
        };
        races.push(Race { prefix, file, data });
    }

    if dist.exists() {
        fs::remove_dir_all(&dist).map_err(|e| e.to_string())?;
    }
    for lang in Lang::ALL {
        let dir = dist.join(lang.code());
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut pages = Vec::new();
        for slug in PAGES {
            let mut p = read_page(&src, lang, slug)?;
            p.body = expand(&p.body, lang, &races, &changelog)
                .map_err(|e| format!("{}/{slug}: {e}", lang.code()))?;
            pages.push(p);
        }
        for p in &pages {
            let html = render(&layout, lang, &pages, p);
            fs::write(dir.join(format!("{}.html", p.slug)), html).map_err(|e| e.to_string())?;
        }
    }
    fs::write(dist.join("sitemap.xml"), sitemap()).map_err(|e| e.to_string())?;
    fs::write(
        dist.join("robots.txt"),
        format!("User-agent: *\nAllow: /\n\nSitemap: {SITE}/sitemap.xml\n"),
    )
    .map_err(|e| e.to_string())?;
    for file in [
        "style.css",
        "site.js",
        "lang.js",
        "favicon.svg",
        "og.png",
        "index.html",
        "404.html",
        "_headers",
    ] {
        let from = src.join(file);
        if from.exists() {
            fs::copy(&from, dist.join(file)).map_err(|e| format!("{file}: {e}"))?;
        }
    }
    for race in races.iter().filter(|r| r.data.is_some()) {
        fs::copy(site.join("data").join(race.file), dist.join(race.file))
            .map_err(|e| format!("{}: {e}", race.file))?;
    }
    Ok(dist)
}

/// Every page in both languages, each with its translations.
fn sitemap() -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">\n",
    );
    for lang in Lang::ALL {
        for slug in PAGES {
            xml += &format!("<url><loc>{SITE}{}</loc>", url(lang, slug));
            for alt in Lang::ALL {
                xml += &format!(
                    "<xhtml:link rel=\"alternate\" hreflang=\"{}\" href=\"{SITE}{}\"/>",
                    alt.code(),
                    url(alt, slug)
                );
            }
            xml += "</url>\n";
        }
    }
    xml + "</urlset>\n"
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
        let races = [Race {
            prefix: "race",
            file: "race.json",
            data: None,
        }];
        let s = expand("a {{version}} b {{download:x64}}", Lang::De, &races, "").unwrap();
        assert!(s.starts_with(&format!("a {} b https://", env!("CARGO_PKG_VERSION"))));
        assert!(s.ends_with("amigo-blinzel-setup-x64.exe"));
        let pending = expand("{{race:open_warm}}", Lang::En, &races, "").unwrap();
        assert!(pending.contains("reference measurement"));
        assert!(expand("{{nope}}", Lang::De, &races, "").is_err());
        assert!(expand("{{race-other:next}}", Lang::De, &races, "").is_err());
        assert!(expand("{{version", Lang::De, &races, "").is_err());
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
        // macOS still pending; then both reports, each linking its own data.
        assert!(html.contains("reference measurement"));
        assert!(!dist.join("race-macos.json").exists());
        fs::write(tmp.join("data/race-macos.json"), charts::tests::SAMPLE).unwrap();
        let dist = build(&tmp, &root.join("CHANGELOG.md")).unwrap();
        let html = fs::read_to_string(dist.join("en/benchmarks.html")).unwrap();
        assert!(!html.contains("reference measurement"));
        assert!(html.contains("href=\"/race.json\"") && html.contains("href=\"/race-macos.json\""));
        assert!(dist.join("race-macos.json").is_file());
        for file in [
            "sitemap.xml",
            "robots.txt",
            "site.js",
            "og.png",
            "style.css",
        ] {
            assert!(dist.join(file).is_file(), "{file}");
        }
        let sitemap = fs::read_to_string(dist.join("sitemap.xml")).unwrap();
        assert_eq!(sitemap.matches("<url>").count(), 2 * PAGES.len());
        for lang in Lang::ALL {
            let code = lang.code();
            for p in PAGES {
                let html = fs::read_to_string(dist.join(code).join(format!("{p}.html")))
                    .unwrap_or_else(|e| panic!("{code}/{p}: {e}"));
                // One heading per page (the start page brings its own), and
                // the privacy page linked from every page.
                assert_eq!(html.matches("<h1").count(), 1, "{code}/{p}");
                assert!(
                    html.contains(&format!("href=\"/{code}/privacy\"")),
                    "{code}/{p}"
                );
                assert!(!html.contains("{{"), "{code}/{p}");
                // Description, canonical link and both translations of
                // this very page.
                assert_eq!(html.matches("<meta name=\"description\"").count(), 1);
                let canonical = format!("<link rel=\"canonical\" href=\"{SITE}{}\">", url(lang, p));
                assert!(html.contains(&canonical), "{code}/{p}");
                for alt in Lang::ALL {
                    let link =
                        format!("hreflang=\"{}\" href=\"{SITE}{}\"", alt.code(), url(alt, p));
                    assert!(html.contains(&link), "{code}/{p}: {link}");
                }
                // The CSP allows no inline scripts.
                assert_eq!(
                    html.matches("<script").count(),
                    html.matches("<script src=").count(),
                    "{code}/{p}"
                );
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn pages_need_title_and_description() {
        let tmp = std::env::temp_dir().join(format!("blinzel-page-{}", std::process::id()));
        fs::create_dir_all(tmp.join("en")).unwrap();
        fs::write(
            tmp.join("en/index.html"),
            "<!-- title: Home -->\n<!-- description: A \"fast\" viewer -->\n<p>x</p>\n",
        )
        .unwrap();
        let p = read_page(&tmp, Lang::En, "index").unwrap();
        assert_eq!(
            (p.title.as_str(), p.description.as_str()),
            ("Home", "A \"fast\" viewer")
        );
        assert_eq!(p.body, "<p>x</p>\n");
        fs::write(
            tmp.join("en/index.html"),
            "<!-- title: Home -->\n<p>x</p>\n",
        )
        .unwrap();
        assert!(read_page(&tmp, Lang::En, "index").is_err());
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
