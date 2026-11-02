//! Builds the website (M8): `website/src` (layout, stylesheet, one HTML
//! fragment per page and language) plus `website/data/race.json` and
//! `race-macos.json` (from `blinzel-race` on Windows and macOS) →
//! `website/dist`, ready for Cloudflare Workers static assets. Charts are
//! generated here as inline SVG; the browser runs no chart library.
//!
//! ```text
//! cargo run -p blinzel-site [-- <website-dir>]
//! ```

mod charts;

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

/// The one-page site plus the privacy page (linked from the footer).
const PAGES: [&str; 2] = ["index", "privacy"];

/// Sections of the start page in the navigation bar: anchor id, which is
/// also the key of its title in [`Lang::t`].
const SECTIONS: [&str; 3] = ["features", "measured", "download"];

const RELEASES_PAGE: &str = "https://github.com/amigo-labs/amigo-blinzel/releases";

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
            (Lang::De, "features") => "Funktionen",
            (Lang::En, "features") => "Features",
            (Lang::De, "measured") => "Vergleiche",
            (Lang::En, "measured") => "Comparisons",
            (Lang::De, "download") => "Download",
            (Lang::En, "download") => "Download",
            (Lang::De, "changes") => "Änderungen",
            (Lang::En, "changes") => "Changes",
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
fn expand(body: &str, lang: Lang, races: &[Race]) -> Result<String, String> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let end = rest[i..].find("}}").ok_or("unclosed {{")? + i;
        let key = rest[i + 2..end].trim();
        out.push_str(&placeholder(key, lang, races)?);
        rest = &rest[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn placeholder(key: &str, lang: Lang, races: &[Race]) -> Result<String, String> {
    const RELEASES: &str = "https://github.com/amigo-labs/amigo-blinzel/releases/latest/download";
    if let Some((prefix, kind)) = key.split_once(':')
        && let Some(race) = races.iter().find(|r| r.prefix == prefix)
    {
        if !charts::KINDS.contains(&kind) {
            return Err(format!("unknown race chart {kind:?}"));
        }
        return match &race.data {
            None => Ok(format!("<p class=\"pending\">{}</p>", lang.t("pending"))),
            Some(r) => charts::race(kind, r, &format!("/{}", race.file), |k| lang.t(k)),
        };
    }
    Ok(match key {
        "version" => env!("CARGO_PKG_VERSION").to_string(),
        "download:x64" => format!("{RELEASES}/amigo-blinzel-setup-x64.exe"),
        "download:arm64" => format!("{RELEASES}/amigo-blinzel-setup-arm64.exe"),
        "download:macos" => format!("{RELEASES}/amigo-blinzel-macos.dmg"),
        "download:linux-x64" => format!("{RELEASES}/amigo-blinzel-linux-x64.tar.gz"),
        "download:linux-arm64" => format!("{RELEASES}/amigo-blinzel-linux-arm64.tar.gz"),
        "download:sums" => format!("{RELEASES}/SHA256SUMS"),
        other => return Err(format!("unknown placeholder {{{{{other}}}}}")),
    })
}

fn render(layout: &str, lang: Lang, pages: &[Page], page: &Page) -> String {
    let nav: String = SECTIONS
        .iter()
        .map(|id| format!("<a href=\"{}#{id}\">{}</a>", url(lang, "index"), lang.t(id)))
        .collect();
    let footer: String = pages
        .iter()
        .filter(|p| p.slug != "index")
        .map(|p| format!("<a href=\"{}\">{}</a>", url(lang, p.slug), p.title))
        .chain([format!(
            "<a href=\"{RELEASES_PAGE}\">{}</a>",
            lang.t("changes")
        )])
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

pub fn build(site: &Path) -> Result<PathBuf, String> {
    let src = site.join("src");
    let dist = site.join("dist");
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let layout = read(&src.join("layout.html"))?;
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
            p.body = expand(&p.body, lang, &races)
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
        "_redirects",
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
    match build(&site) {
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
        let s = expand("a {{version}} b {{download:x64}}", Lang::De, &races).unwrap();
        assert!(s.starts_with(&format!("a {} b https://", env!("CARGO_PKG_VERSION"))));
        assert!(s.ends_with("amigo-blinzel-setup-x64.exe"));
        let pending = expand("{{race:open_warm}}", Lang::En, &races).unwrap();
        assert!(pending.contains("reference measurement"));
        assert!(expand("{{nope}}", Lang::De, &races).is_err());
        assert!(expand("{{race-other:next}}", Lang::De, &races).is_err());
        assert!(expand("{{version", Lang::De, &races).is_err());
    }

    /// A page with an unknown placeholder, an unknown race chart kind (with
    /// or without the report) or an unclosed `{{` stops the build.
    #[test]
    fn bad_placeholders_fail_the_build() {
        let tmp = fresh_site("placeholders");
        let page = tmp.join("src/en/privacy.html");
        let good = fs::read_to_string(&page).unwrap();
        for bad in [
            "{{nope}}",
            "{{race:bogus}}",
            "{{race-macos:tabel}}",
            "{{version",
        ] {
            fs::write(&page, format!("{good}{bad}\n")).unwrap();
            let e = build(&tmp).expect_err(bad);
            assert!(e.starts_with("en/privacy: "), "{bad}: {e}");
        }
        fs::write(tmp.join("data/race.json"), charts::tests::SAMPLE).unwrap();
        fs::write(&page, format!("{good}{{{{race:bogus}}}}\n")).unwrap();
        assert!(build(&tmp).is_err());
        fs::write(&page, good).unwrap();
        assert!(build(&tmp).is_ok());
        let _ = fs::remove_dir_all(&tmp);
    }

    /// The download placeholders resolve to the stable URLs of the latest
    /// release, and the start page of each language offers every file.
    #[test]
    fn download_placeholders_resolve_to_latest_release() {
        let base = "https://github.com/amigo-labs/amigo-blinzel/releases/latest/download/";
        let files = [
            ("x64", "amigo-blinzel-setup-x64.exe"),
            ("arm64", "amigo-blinzel-setup-arm64.exe"),
            ("macos", "amigo-blinzel-macos.dmg"),
            ("linux-x64", "amigo-blinzel-linux-x64.tar.gz"),
            ("linux-arm64", "amigo-blinzel-linux-arm64.tar.gz"),
            ("sums", "SHA256SUMS"),
        ];
        for (key, file) in files {
            let s = expand(&format!("{{{{download:{key}}}}}"), Lang::En, &[]).unwrap();
            assert_eq!(s, format!("{base}{file}"), "{key}");
        }
        let tmp = fresh_site("download");
        let dist = build(&tmp).unwrap();
        for lang in Lang::ALL {
            let html = fs::read_to_string(dist.join(lang.code()).join("index.html")).unwrap();
            for (_, file) in files {
                assert!(html.contains(&format!("href=\"{base}{file}\"")), "{file}");
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A report that exists but is broken stops the build, naming the file.
    #[test]
    fn invalid_race_report_fails_naming_the_file() {
        let tmp = fresh_site("invalid");
        fs::write(tmp.join("data/race.json"), "{\"machine\": ").unwrap();
        let e = build(&tmp).expect_err("broken race.json builds");
        assert!(e.contains("race.json") && e.contains("invalid JSON"), "{e}");
        fs::write(tmp.join("data/race.json"), charts::tests::SAMPLE).unwrap();
        fs::write(tmp.join("data/race-macos.json"), "not json").unwrap();
        let e = build(&tmp).expect_err("broken race-macos.json builds");
        assert!(e.contains("race-macos.json"), "{e}");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Every page links the releases page as its changelog and the same
    /// page in the other language.
    #[test]
    fn pages_link_the_changelog_and_the_other_language() {
        let tmp = fresh_site("links");
        let dist = build(&tmp).unwrap();
        for lang in Lang::ALL {
            let other = lang.other();
            for p in PAGES {
                let html =
                    fs::read_to_string(dist.join(lang.code()).join(format!("{p}.html"))).unwrap();
                let changes = format!("<a href=\"{RELEASES_PAGE}\">{}</a>", lang.t("changes"));
                assert!(html.contains(&changes), "{}/{p}: {changes}", lang.code());
                let switch = format!(
                    "<a class=\"lang\" href=\"{}\" hreflang=\"{}\" lang=\"{1}\">{}</a>",
                    url(other, p),
                    other.code(),
                    other.t("lang_name")
                );
                assert!(html.contains(&switch), "{}/{p}: {switch}", lang.code());
            }
        }
        assert_eq!(
            (Lang::De.t("changes"), Lang::En.t("changes")),
            ("Änderungen", "Changes")
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// One sitemap entry per page and language, each naming both language
    /// versions, and a robots.txt that points to the sitemap.
    #[test]
    fn sitemap_lists_every_page_and_robots_names_it() {
        let tmp = fresh_site("sitemap");
        let dist = build(&tmp).unwrap();
        let sitemap = fs::read_to_string(dist.join("sitemap.xml")).unwrap();
        let entries: Vec<&str> = sitemap.split("<url>").skip(1).collect();
        let mut locs: Vec<String> = Vec::new();
        for entry in &entries {
            let entry = entry.split("</url>").next().unwrap();
            let loc = entry
                .strip_prefix("<loc>")
                .and_then(|e| e.split("</loc>").next())
                .unwrap_or_else(|| panic!("no <loc>: {entry}"));
            let (lang, slug) = Lang::ALL
                .iter()
                .flat_map(|&l| PAGES.iter().map(move |&s| (l, s)))
                .find(|&(l, s)| loc == format!("{SITE}{}", url(l, s)))
                .unwrap_or_else(|| panic!("unknown page {loc}"));
            assert_eq!(entry.matches("<xhtml:link").count(), 2, "{loc}");
            for alt in Lang::ALL {
                let link = format!(
                    "<xhtml:link rel=\"alternate\" hreflang=\"{}\" href=\"{SITE}{}\"/>",
                    alt.code(),
                    url(alt, slug)
                );
                assert!(entry.contains(&link), "{loc}: {link}");
            }
            locs.push(url(lang, slug));
        }
        locs.sort();
        locs.dedup();
        assert_eq!(locs.len(), entries.len(), "a page is listed twice");
        assert_eq!(locs.len(), Lang::ALL.len() * PAGES.len());
        let robots = fs::read_to_string(dist.join("robots.txt")).unwrap();
        assert!(
            robots
                .lines()
                .any(|l| l == format!("Sitemap: {SITE}/sitemap.xml")),
            "{robots}"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A copy of the real website sources in its own temp folder, without
    /// measurements.
    fn fresh_site(tag: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let tmp = std::env::temp_dir().join(format!("blinzel-site-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        copy_dir(&root.join("website/src"), &tmp.join("src"));
        tmp
    }

    /// The real website sources build, with and without measurements.
    #[test]
    fn builds_the_site() {
        let tmp = fresh_site("build");
        let dist = build(&tmp).unwrap();
        let html = fs::read_to_string(dist.join("de/index.html")).unwrap();
        assert!(html.contains("Referenzmessung"));
        assert!(!html.contains("{{"));

        fs::write(tmp.join("data/race.json"), charts::tests::SAMPLE).unwrap();
        let dist = build(&tmp).unwrap();
        let html = fs::read_to_string(dist.join("en/index.html")).unwrap();
        assert!(html.contains("<svg"));
        assert!(html.contains("ImageGlass"));
        // macOS still pending; then both reports, each linking its own data.
        assert!(html.contains("reference measurement"));
        assert!(!dist.join("race-macos.json").exists());
        fs::write(tmp.join("data/race-macos.json"), charts::tests::SAMPLE).unwrap();
        let dist = build(&tmp).unwrap();
        let html = fs::read_to_string(dist.join("en/index.html")).unwrap();
        assert!(!html.contains("reference measurement"));
        assert!(html.contains("href=\"/race.json\"") && html.contains("href=\"/race-macos.json\""));
        assert!(dist.join("race-macos.json").is_file());
        for file in [
            "sitemap.xml",
            "robots.txt",
            "site.js",
            "og.png",
            "style.css",
            "_redirects",
        ] {
            assert!(dist.join(file).is_file(), "{file}");
        }
        // Every section in the navigation exists on the start page, and the
        // old page addresses lead there.
        let redirects = fs::read_to_string(dist.join("_redirects")).unwrap();
        for lang in Lang::ALL {
            let code = lang.code();
            let html = fs::read_to_string(dist.join(code).join("index.html")).unwrap();
            for id in SECTIONS {
                assert!(html.contains(&format!("id=\"{id}\"")), "{code}: #{id}");
            }
            for line in redirects
                .lines()
                .filter(|l| l.starts_with(&format!("/{code}/")))
            {
                let target = line.split_whitespace().nth(1).unwrap();
                if let Some((_, id)) = target.split_once('#') {
                    assert!(html.contains(&format!("id=\"{id}\"")), "{line}");
                }
            }
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

    /// Four viewers: one with a name to escape and an unmeasured cold
    /// start, one fastest everywhere, one with no key presses, one with no
    /// next image and no burst at all; plus a machine key the site does not
    /// know.
    const REPORT: &str = r#"{
      "machine": {"cpu": "Test CPU", "logical_cpus": "16", "ram_gb": "32", "gpu": "Test GPU",
        "monitor": "1920x1080", "monitors": "2", "os_build": "26100", "os": "Windows 11", "disk": "NVMe"},
      "viewers": [
        {"name": "A<&\"b>", "version": "1.0", "open_warm": {"median_ms": 1800.0, "p95_ms": 1900.0},
         "open_cold": null, "next": {"median_ms": 40.0, "p95_ms": 55.0},
         "burst": {"presses": 20, "shown": {"median_ms": 10.0, "p95_ms": 10.0}}},
        {"name": "Fast", "version": "2.0", "open_warm": {"median_ms": 80.0, "p95_ms": 95.0},
         "open_cold": {"median_ms": 300.0, "p95_ms": 320.0}, "next": {"median_ms": 9.4, "p95_ms": 12.0},
         "burst": {"presses": 20, "shown": {"median_ms": 20.0, "p95_ms": 20.0}}},
        {"name": "Mid", "version": "3.0", "open_warm": {"median_ms": 500.0, "p95_ms": 600.0},
         "open_cold": null, "next": {"median_ms": 20.0, "p95_ms": 30.0},
         "burst": {"presses": 0, "shown": {"median_ms": 0.0, "p95_ms": 0.0}}},
        {"name": "NoBurst", "version": "4.0", "open_warm": {"median_ms": 2500.0, "p95_ms": 2600.0},
         "open_cold": null, "next": null}
      ]}"#;

    fn report_chart(kind: &str, lang: Lang) -> String {
        let race = Json::parse(REPORT).unwrap();
        charts::race(kind, &race, "/race.json", |k| lang.t(k)).unwrap()
    }

    /// The viewer names in the order a chart lists them.
    fn order(svg: &str) -> Vec<&str> {
        svg.split("class=\"label\">")
            .skip(1)
            .map(|s| s.split('<').next().unwrap())
            .collect()
    }

    /// Time charts list the fastest first, without the viewers that have no
    /// value; 1000 ms and more read as seconds in the page's language.
    #[test]
    fn time_charts_list_the_fastest_first() {
        let warm = report_chart("open_warm", Lang::De);
        assert_eq!(
            order(&warm),
            ["Fast", "Mid", "A&lt;&amp;&quot;b&gt;", "NoBurst"]
        );
        assert!(!warm.contains("A<&"), "unescaped name");
        for text in ["80 ms", "500 ms", "1,80 s", "2,50 s"] {
            assert!(warm.contains(&format!(">{text}</text>")), "{text}: {warm}");
        }
        let warm = report_chart("open_warm", Lang::En);
        assert!(warm.contains(">1.80 s</text>") && warm.contains(">2.50 s</text>"));
        assert_eq!(order(&report_chart("open_cold", Lang::En)), ["Fast"]);
        let next = report_chart("next", Lang::En);
        assert_eq!(order(&next), ["Fast", "Mid", "A&lt;&amp;&quot;b&gt;"]);
        assert!(next.contains(">9 ms</text>"), "{next}");
    }

    /// The key-repeat chart lists the highest share of images shown first,
    /// without viewers that have no key presses.
    #[test]
    fn burst_chart_lists_the_highest_share_first() {
        let burst = report_chart("burst", Lang::En);
        assert_eq!(order(&burst), ["Fast", "A&lt;&amp;&quot;b&gt;"]);
        assert!(burst.contains(">20 / 20</text>") && burst.contains(">10 / 20</text>"));
    }

    /// Every bar looks the same: no viewer gets a class, colour or style of
    /// its own.
    #[test]
    fn charts_highlight_no_viewer() {
        for kind in ["open_warm", "open_cold", "next", "burst"] {
            let svg = report_chart(kind, Lang::En);
            let rows = svg.matches("<g class=\"row\">").count();
            assert_eq!(svg.matches("<g ").count(), rows, "{kind}");
            assert_eq!(svg.matches("class=\"bar\"").count(), rows, "{kind}");
            assert_eq!(svg.matches("class=\"track\"").count(), rows, "{kind}");
            assert!(!svg.contains("style=") && !svg.contains("fill="), "{kind}");
            for class in svg.split("class=\"").skip(1) {
                let class = class.split('"').next().unwrap();
                assert!(
                    [
                        "chart-card",
                        "chart",
                        "row",
                        "label",
                        "value",
                        "track",
                        "bar"
                    ]
                    .contains(&class),
                    "{kind}: class {class}"
                );
            }
        }
    }

    /// A metric no viewer has reads "Nicht gemessen." / "Not measured.".
    #[test]
    fn charts_without_values_read_not_measured() {
        let race = Json::parse(
            r#"{"machine": {}, "viewers": [{"name": "X", "version": "1", "open_warm": null,
                "open_cold": null, "next": null, "burst": {"presses": 0}}]}"#,
        )
        .unwrap();
        for (lang, text) in [(Lang::De, "Nicht gemessen."), (Lang::En, "Not measured.")] {
            for kind in ["open_warm", "open_cold", "next", "burst"] {
                let html = charts::race(kind, &race, "/race.json", |k| lang.t(k)).unwrap();
                assert_eq!(html, format!("<p class=\"pending\">{text}</p>"), "{kind}");
            }
        }
        assert!(report_chart("open_warm", Lang::En).contains("<svg"));
    }

    /// The table lists every viewer in the report with its version and
    /// median (p95) for open warm, open cold and next image, and links the
    /// raw report.
    #[test]
    fn table_lists_every_viewer_with_median_and_p95() {
        let table = report_chart("table", Lang::De);
        let head = "<tr><th>Viewer</th><th>Version</th><th>Öffnen (warm)</th><th>Öffnen (kalt)</th><th>Nächstes Bild</th></tr>";
        assert!(table.contains(head), "{table}");
        for row in [
            "<tr><td>A&lt;&amp;&quot;b&gt;</td><td>1.0</td><td>1,80 s (1,90 s)</td><td>–</td><td>40 ms (55 ms)</td></tr>",
            "<tr><td>Fast</td><td>2.0</td><td>80 ms (95 ms)</td><td>300 ms (320 ms)</td><td>9 ms (12 ms)</td></tr>",
            "<tr><td>Mid</td><td>3.0</td><td>500 ms (600 ms)</td><td>–</td><td>20 ms (30 ms)</td></tr>",
            "<tr><td>NoBurst</td><td>4.0</td><td>2,50 s (2,60 s)</td><td>–</td><td>–</td></tr>",
        ] {
            assert!(table.contains(row), "{row}");
        }
        assert_eq!(table.matches("<tr><td>").count(), 4);
        assert!(table.contains("Median (p95) · <a href=\"/race.json\">Rohdaten (JSON)</a>"));
        let en = report_chart("table", Lang::En);
        assert!(en.contains("<a href=\"/race.json\">Raw data (JSON)</a>"));
        assert!(en.contains("<td>1.80 s (1.90 s)</td>"));
    }

    /// Every machine entry, labelled in the page language where the key is
    /// known and with the raw key otherwise.
    #[test]
    fn machine_block_labels_every_entry() {
        let labels = [
            ("cpu", "Prozessor", "Processor"),
            ("logical_cpus", "Logische Kerne", "Logical cores"),
            ("ram_gb", "Arbeitsspeicher (GB)", "Memory (GB)"),
            ("gpu", "Grafik", "Graphics"),
            ("monitor", "Hauptmonitor", "Main monitor"),
            ("monitors", "Monitore", "Monitors"),
            ("os_build", "Windows-Build", "Windows build"),
            ("os", "Betriebssystem", "Operating system"),
            ("disk", "disk", "disk"),
        ];
        let race = Json::parse(REPORT).unwrap();
        let Some(Json::Obj(machine)) = race.get("machine") else {
            panic!("no machine");
        };
        for lang in Lang::ALL {
            let html = report_chart("machine", lang);
            assert_eq!(html.matches("<dt>").count(), machine.len());
            for (key, value) in machine {
                let value = value.as_str().unwrap();
                let (_, de, en) = labels.iter().find(|l| l.0 == key).unwrap();
                let label = if lang == Lang::De { de } else { en };
                let item = format!("<div><dt>{label}</dt><dd>{value}</dd></div>");
                assert!(html.contains(&item), "{}: {item}", lang.code());
            }
        }
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
