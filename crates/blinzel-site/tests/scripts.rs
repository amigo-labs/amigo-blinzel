//! The website's two scripts (`website/src/site.js`, `website/src/lang.js`)
//! run in Node with a stand-in `navigator`, `document` and `location`
//! (`tests/browser.js`), against the download button of the generated start
//! pages (`docs/specs/website.md` AC-16, AC-17).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Builds a copy of the real website sources with the generator binary and
/// returns its `dist` folder.
fn built_site(tag: &str) -> (PathBuf, PathBuf) {
    let tmp = std::env::temp_dir().join(format!("blinzel-scripts-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    copy_dir(&root().join("website/src"), &tmp.join("src"));
    let out = Command::new(env!("CARGO_BIN_EXE_blinzel-site"))
        .arg(&tmp)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dist = tmp.join("dist");
    (tmp, dist)
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

/// The attributes of the start page's main download button
/// (`<a … data-download …>`), its text as `text`.
fn download_button(html: &str) -> Vec<(String, String)> {
    let start = html.find("data-download").expect("no download button");
    let start = html[..start].rfind("<a ").unwrap();
    let end = start + html[start..].find("</a>").unwrap();
    let tag = &html[start + 3..end];
    let (attrs, text) = tag.split_once('>').unwrap();
    let mut out = vec![("text".to_string(), text.to_string())];
    let mut rest = attrs.trim();
    while !rest.is_empty() {
        let name_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_string();
        rest = &rest[name_end..];
        let value = if let Some(r) = rest.strip_prefix("=\"") {
            let close = r.find('"').unwrap();
            rest = &r[close + 1..];
            r[..close].to_string()
        } else {
            String::new()
        };
        out.push((name, value));
        rest = rest.trim_start();
    }
    out
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One visitor's browser: user agent string, `navigator.userAgentData` as
/// (platform, mobile, architecture) where it has one, touch points.
struct Browser {
    name: &'static str,
    ua: &'static str,
    uad: Option<(&'static str, bool, &'static str)>,
    touch: u32,
}

/// Runs `script` once per case (each a JSON object) and returns per case
/// (link href, link text, address given to `location.replace`).
fn run(script: &Path, cases: &[String]) -> Vec<(String, String, String)> {
    let mut node = Command::new("node")
        .arg(root().join("crates/blinzel-site/tests/browser.js"))
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node not found: the website script tests need Node.js in the PATH");
    let input = format!("[{}]", cases.join(","));
    node.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = node.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<_> = stdout
        .lines()
        .map(|l| {
            let mut f = l.split('\t').map(str::to_string);
            (f.next().unwrap(), f.next().unwrap(), f.next().unwrap())
        })
        .collect();
    assert_eq!(lines.len(), cases.len(), "{stdout}");
    lines
}

const WIN_CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
const MAC_SAFARI: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const MAC_CHROME: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
const LINUX_CHROME: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// The browsers of AC-16 and the platform key whose file the button must
/// offer (`None`: it keeps linking the download section).
fn browsers() -> Vec<(Browser, Option<&'static str>)> {
    let b = |name, ua, uad, touch| Browser {
        name,
        ua,
        uad,
        touch,
    };
    vec![
        (
            b(
                "Windows Chrome x64",
                WIN_CHROME,
                Some(("Windows", false, "x86")),
                0,
            ),
            Some("x64"),
        ),
        (
            b(
                "Windows Chrome ARM64",
                WIN_CHROME,
                Some(("Windows", false, "arm")),
                0,
            ),
            Some("arm64"),
        ),
        (
            b(
                "Windows Firefox x64",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) Gecko/20100101 Firefox/132.0",
                None,
                0,
            ),
            Some("x64"),
        ),
        (b("macOS Safari", MAC_SAFARI, None, 0), Some("macos")),
        (
            b(
                "macOS Chrome on Apple silicon",
                MAC_CHROME,
                Some(("macOS", false, "arm")),
                0,
            ),
            Some("macos"),
        ),
        (
            b(
                "Linux Chrome x64",
                LINUX_CHROME,
                Some(("Linux", false, "x86")),
                0,
            ),
            Some("linux-x64"),
        ),
        (
            b(
                "Linux Chrome ARM64",
                LINUX_CHROME,
                Some(("Linux", false, "arm")),
                0,
            ),
            Some("linux-arm64"),
        ),
        (
            b(
                "Linux Firefox x64",
                "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0",
                None,
                0,
            ),
            Some("linux-x64"),
        ),
        (
            b(
                "Linux Firefox ARM64",
                "Mozilla/5.0 (X11; Linux aarch64; rv:132.0) Gecko/20100101 Firefox/132.0",
                None,
                0,
            ),
            Some("linux-arm64"),
        ),
        (
            b(
                "iPhone",
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
                None,
                5,
            ),
            None,
        ),
        // iPadOS presents itself as a Mac, with a touch screen.
        (b("iPad", MAC_SAFARI, None, 5), None),
        (
            b(
                "Android phone",
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36",
                Some(("Android", true, "")),
                5,
            ),
            None,
        ),
        (
            b(
                "Android tablet",
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
                Some(("Android", false, "arm")),
                5,
            ),
            None,
        ),
        // A phone that only its client hints give away.
        (
            b(
                "phone by client hints",
                LINUX_CHROME,
                Some(("Linux", true, "arm")),
                5,
            ),
            None,
        ),
        // Says "Linux" and has no client hints.
        (
            b(
                "Android Samsung Internet",
                "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/26.0 Chrome/122.0.0.0 Safari/537.36",
                None,
                5,
            ),
            None,
        ),
        (
            b(
                "Android Firefox",
                "Mozilla/5.0 (Android 14; Mobile; rv:132.0) Gecko/132.0 Firefox/132.0",
                None,
                5,
            ),
            None,
        ),
        (
            b(
                "ChromeOS",
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
                Some(("Chrome OS", false, "x86")),
                0,
            ),
            None,
        ),
        (
            b(
                "ChromeOS without client hints",
                "Mozilla/5.0 (X11; CrOS aarch64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
                None,
                0,
            ),
            None,
        ),
        (
            b(
                "FreeBSD Firefox",
                "Mozilla/5.0 (X11; FreeBSD amd64; rv:132.0) Gecko/20100101 Firefox/132.0",
                None,
                0,
            ),
            None,
        ),
        (
            b(
                "unknown",
                "Mozilla/5.0 (PlayStation 5 6.00) AppleWebKit/605.1.15 (KHTML, like Gecko)",
                None,
                0,
            ),
            None,
        ),
        (b("no user agent", "", None, 0), None),
    ]
}

/// The release file of a platform key.
fn file(key: &str) -> &'static str {
    match key {
        "x64" => "amigo-blinzel-setup-x64.exe",
        "arm64" => "amigo-blinzel-setup-arm64.exe",
        "macos" => "amigo-blinzel-macos.dmg",
        "linux-x64" => "amigo-blinzel-linux-x64.tar.gz",
        "linux-arm64" => "amigo-blinzel-linux-arm64.tar.gz",
        _ => unreachable!("{key}"),
    }
}

/// The label a platform's file must carry: its system, and "ARM64" exactly
/// for the ARM files.
fn label_matches(key: &str, label: &str) -> bool {
    let system = match key {
        "x64" | "arm64" => "Windows",
        "macos" => "macOS",
        _ => "Linux",
    };
    label.contains(system) && label.contains("ARM64") == key.ends_with("arm64")
}

#[test]
fn download_button_offers_the_visitors_platform() {
    let (tmp, dist) = built_site("download");
    let releases = "https://github.com/amigo-labs/amigo-blinzel/releases/latest/download/";
    for lang in ["de", "en"] {
        let html = fs::read_to_string(dist.join(lang).join("index.html")).unwrap();
        let attrs = download_button(&html);
        let attr = |name: &str| {
            attrs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(attr("href").as_deref(), Some("#download"), "{lang}");
        let text = attr("text").unwrap();
        let attrs_json: Vec<String> = attrs
            .iter()
            .map(|(n, v)| format!("{}:{}", json_str(n), json_str(v)))
            .collect();
        let browsers = browsers();
        let cases: Vec<String> = browsers
            .iter()
            .map(|(b, _)| {
                let uad = b.uad.map_or("null".into(), |(platform, mobile, arch)| {
                    format!(
                        "{{\"platform\":{},\"mobile\":{mobile},\"arch\":{}}}",
                        json_str(platform),
                        json_str(arch)
                    )
                });
                format!(
                    "{{\"ua\":{},\"touch\":{},\"uad\":{uad},\"attrs\":{{{}}}}}",
                    json_str(b.ua),
                    b.touch,
                    attrs_json.join(",")
                )
            })
            .collect();
        let results = run(&dist.join("site.js"), &cases);
        for ((b, want), (href, label, _)) in browsers.iter().zip(results) {
            match want {
                Some(key) => {
                    assert_eq!(
                        Some(&href),
                        attr(&format!("data-{key}")).as_ref(),
                        "{lang}: {}",
                        b.name
                    );
                    assert_eq!(
                        href,
                        format!("{releases}{}", file(key)),
                        "{lang}: {}",
                        b.name
                    );
                    assert_eq!(
                        Some(&label),
                        attr(&format!("data-label-{key}")).as_ref(),
                        "{lang}: {}",
                        b.name
                    );
                    assert!(label_matches(key, &label), "{lang}: {}: {label}", b.name);
                }
                None => {
                    assert_eq!(
                        (href.as_str(), label.as_str()),
                        ("#download", text.as_str()),
                        "{lang}: {}",
                        b.name
                    )
                }
            }
        }
    }
    // A browser that refuses the architecture hint still gets its system.
    let html = fs::read_to_string(dist.join("en/index.html")).unwrap();
    let attrs = download_button(&html);
    let attrs_json: Vec<String> = attrs
        .iter()
        .map(|(n, v)| format!("{}:{}", json_str(n), json_str(v)))
        .collect();
    let case = format!(
        "{{\"ua\":{},\"uad\":{{\"platform\":\"Windows\",\"mobile\":false,\"throws\":true}},\"attrs\":{{{}}}}}",
        json_str(WIN_CHROME),
        attrs_json.join(",")
    );
    let (href, _, _) = run(&dist.join("site.js"), &[case]).remove(0);
    assert!(href.ends_with("amigo-blinzel-setup-x64.exe"), "{href}");
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn site_root_sends_the_browser_to_its_language() {
    let (tmp, dist) = built_site("lang");
    let index = fs::read_to_string(dist.join("index.html")).unwrap();
    assert!(
        index.contains("<script src=\"/lang.js\"></script>"),
        "{index}"
    );
    let cases = [
        ("de-DE", "/de/"),
        ("de", "/de/"),
        ("DE-AT", "/de/"),
        ("de-CH", "/de/"),
        ("en-US", "/en/"),
        ("fr-FR", "/en/"),
        ("dsb", "/en/"),
        ("", "/en/"),
    ];
    let json: Vec<String> = cases
        .iter()
        .map(|(l, _)| format!("{{\"language\":{}}}", json_str(l)))
        .collect();
    let mut all = json.clone();
    // A browser without navigator.language.
    all.push("{}".into());
    let results = run(&dist.join("lang.js"), &all);
    for ((language, want), (_, _, to)) in cases.iter().chain([&("(none)", "/en/")]).zip(results) {
        assert_eq!(to, *want, "{language}");
    }
    let _ = fs::remove_dir_all(&tmp);
}
