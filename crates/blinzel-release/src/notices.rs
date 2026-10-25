//! The third-party notices: the licence texts of everything linked into
//! the viewer (both architectures, without build-only and proc-macro
//! crates), plus the bundled C libraries and the Rust standard library.
//! One file per platform ([`PLATFORMS`]), each embedded in that platform's
//! executable and shipped with its release.
//!
//! The crate list comes from `cargo tree`, the texts from the crates'
//! sources in Cargo's registry. Of an `A OR B` choice the notice uses the
//! most preferred option ([`PREFERENCE`]) that the crate ships texts for;
//! `A AND B` reproduces both. A crate without any licence file gets the
//! standard text with its authors. An unknown licence (or a `WITH`
//! exception) stops the generator, so a new dependency is looked at by a
//! person.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A platform's notices file (in the workspace root), what the executable
/// is called there, and its targets.
pub struct Platform {
    pub file: &'static str,
    executable: &'static str,
    targets: [&'static str; 2],
}

pub const PLATFORMS: [Platform; 3] = [
    Platform {
        file: "THIRD-PARTY-NOTICES.txt",
        executable: "blinzel.exe",
        targets: ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"],
    },
    Platform {
        file: "THIRD-PARTY-NOTICES-linux.txt",
        executable: "The Linux build",
        targets: ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"],
    },
    Platform {
        file: "THIRD-PARTY-NOTICES-macos.txt",
        executable: "The macOS app",
        targets: ["x86_64-apple-darwin", "aarch64-apple-darwin"],
    },
];

/// Most preferred first: the shortest text that still names the authors.
const PREFERENCE: [&str; 10] = [
    "MIT",
    "ISC",
    "Zlib",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Apache-2.0",
    "Unlicense",
    "0BSD",
    // Unicode data tables (unicode-ident).
    "Unicode-3.0",
    // Public domain: no file is used, only a one-line note.
    "CC0-1.0",
];

const RULE: &str =
    "================================================================================";
const THIN_RULE: &str =
    "--------------------------------------------------------------------------------";

/// A package as `cargo tree` prints it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Package {
    name: String,
    version: String,
}

/// One notice: who it covers and the text.
struct Notice {
    covers: Vec<String>,
    licence: String,
    text: String,
}

/// Writes every platform's notices, or with `check` fails if one differs.
pub fn run_all(root: &Path, check: bool) -> Result<(), String> {
    for p in &PLATFORMS {
        run(root, p, check)?;
    }
    Ok(())
}

/// Writes `platform`'s notices, or with `check` fails if its file differs.
fn run(root: &Path, platform: &Platform, check: bool) -> Result<(), String> {
    let out = &root.join(platform.file);
    let text = generate(root, platform)?;
    if check {
        let current = fs::read_to_string(out).unwrap_or_default();
        if current.replace("\r\n", "\n") != text {
            return Err(format!(
                "{} is out of date: run `cargo run -p blinzel-release -- notices`",
                out.display()
            ));
        }
        println!("{} is up to date", out.display());
    } else {
        fs::write(out, &text).map_err(|e| format!("{}: {e}", out.display()))?;
        println!("wrote {} ({} bytes)", out.display(), text.len());
    }
    Ok(())
}

fn generate(root: &Path, platform: &Platform) -> Result<String, String> {
    let registry = registry_src()?;
    let mut notices = Vec::new();
    let mut crates: BTreeMap<String, Notice> = BTreeMap::new();
    for p in linked_packages(root, &platform.targets)? {
        let dir = find_source(&registry, &p)?;
        if p.name == "turbojpeg-sys" {
            notices.push(libjpeg_turbo(&dir)?);
        }
        if p.name == "libwebp-sys" {
            notices.push(libwebp(&dir)?);
        }
        if p.name == "lcms2-sys" {
            notices.push(little_cms(&dir)?);
        }
        let mut n = crate_notice(&p, &dir)?;
        // A patent grant shipped next to the licence (rav1e: the AV1
        // patent licence) goes with it.
        if let Some(patents) = read_text(&dir.join("PATENTS")) {
            n.text = format!("{}\n\n\n{}", n.text, normalize(&patents));
        }
        // Crates of one project usually share the text word for word.
        match crates.get_mut(&n.text) {
            Some(same) => same.covers.extend(n.covers),
            None => {
                crates.insert(n.text.clone(), n);
            }
        }
    }
    notices.push(Notice {
        covers: vec!["Rust standard library (std, core, alloc)".into()],
        licence: "MIT".into(),
        text: standard_text("MIT", "The Rust Project Contributors")?,
    });
    let mut crates: Vec<Notice> = crates.into_values().collect();
    crates.sort_by(|a, b| a.covers[0].cmp(&b.covers[0]));
    notices.extend(crates);

    let mut s = String::new();
    s.push_str(&format!(
        "amigo-blinzel: third-party notices\n\
         \n\
         amigo-blinzel is released under the MIT licence (see LICENSE). {}\n\
         also contains the software listed below, under the licences reproduced\n\
         here.\n\
         \n\
         This software is based in part on the work of the Independent JPEG Group.\n\
         \n\
         Generated by `cargo run -p blinzel-release -- notices`; do not edit.\n",
        platform.executable
    ));
    for n in &notices {
        s.push('\n');
        s.push_str(RULE);
        s.push('\n');
        for c in &n.covers {
            s.push_str(c);
            s.push('\n');
        }
        s.push_str(&format!("Licence: {}\n", n.licence));
        s.push_str(THIN_RULE);
        s.push('\n');
        s.push_str(&n.text);
        s.push('\n');
    }
    Ok(s)
}

/// Every package linked into `blinzel-app` on `targets`, except the
/// workspace's own crates.
fn linked_packages(root: &Path, targets: &[&str]) -> Result<Vec<Package>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root).args([
        "tree",
        "--locked",
        "-p",
        "blinzel-app",
        "-e",
        "normal,no-proc-macro",
        "--prefix",
        "none",
        "--format",
        "{p}",
    ]);
    for t in targets {
        cmd.args(["--target", t]);
    }
    let out = cmd.output().map_err(|e| format!("cargo tree: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo tree failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut packages: Vec<Package> = String::from_utf8_lossy(&out.stdout)
        .lines()
        // Path dependencies are ours ("name v1.0.0 (/path)").
        .filter(|l| !l.is_empty() && !l.contains(" (/") && !l.contains(":\\"))
        .map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next().unwrap_or_default().to_string();
            let version = it.next().unwrap_or_default();
            Package {
                name,
                version: version.trim_start_matches('v').to_string(),
            }
        })
        .collect();
    packages.sort();
    packages.dedup();
    if packages.is_empty() {
        return Err("cargo tree listed no packages".into());
    }
    Ok(packages)
}

/// `$CARGO_HOME/registry/src`, where Cargo unpacks downloaded crates.
fn registry_src() -> Result<PathBuf, String> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join(".cargo"))
        })
        .ok_or("cannot find Cargo's home directory")?;
    Ok(home.join("registry").join("src"))
}

fn find_source(registry: &Path, p: &Package) -> Result<PathBuf, String> {
    let dir_name = format!("{}-{}", p.name, p.version);
    let indexes = fs::read_dir(registry).map_err(|e| format!("{}: {e}", registry.display()))?;
    indexes
        .filter_map(Result::ok)
        .map(|e| e.path().join(&dir_name))
        .find(|d| d.join("Cargo.toml").is_file())
        .ok_or_else(|| {
            format!(
                "source of {dir_name} not found below {}",
                registry.display()
            )
        })
}

fn crate_notice(p: &Package, dir: &Path) -> Result<Notice, String> {
    let manifest = fs::read_to_string(dir.join("Cargo.toml"))
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let manifest: toml::Table = manifest
        .parse()
        .map_err(|e| format!("{}/Cargo.toml: {e}", dir.display()))?;
    let package = manifest.get("package").and_then(|v| v.as_table());
    let field = |k: &str| package.and_then(|t| t.get(k));
    let expr = field("license")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{}: no license field", p.name))?;
    let choices = alternatives(expr).ok_or_else(|| {
        format!(
            "{} {}: licence `{expr}` needs a look by hand",
            p.name, p.version
        )
    })?;
    let mut heading = format!("{} {}", p.name, p.version);
    if let Some(repo) = field("repository").and_then(|v| v.as_str()) {
        heading.push_str(&format!(" ({repo})"));
    }

    let authors: Vec<String> = field("authors")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.split('<').next().unwrap_or(s).trim().to_string())
                .collect()
        })
        .unwrap_or_default();
    let holder = if authors.is_empty() {
        format!("the {} authors", p.name)
    } else {
        authors.join(", ")
    };

    let files = licence_files(dir)?;
    // The first option with a file for each of its licences, else the
    // first one with standard texts.
    let option = choices
        .iter()
        .find(|option| option.iter().all(|id| files.contains_key(id)))
        .unwrap_or(&choices[0]);
    let mut texts = Vec::new();
    for id in option {
        texts.push(match files.get(id) {
            // Many MIT files are the bare template: name the authors.
            Some(text) if names_holder(text) => text.clone(),
            Some(text) => format!("Copyright (c) {holder}\n\n{text}"),
            None => standard_text(id, &holder)?,
        });
    }
    Ok(Notice {
        covers: vec![heading],
        licence: option.join(" AND "),
        text: texts.join("\n\n\n"),
    })
}

/// The options of an SPDX expression (`A OR B`, `A AND B`, parentheses,
/// old style `A/B`), each the licences that apply together, most
/// preferred first. Options with an unknown licence or a `WITH` exception
/// are left out; `None` if none is left.
fn alternatives(expr: &str) -> Option<Vec<Vec<&'static str>>> {
    let spaced = expr
        .replace('(', " ( ")
        .replace(')', " ) ")
        .replace('/', " OR ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let (mut options, rest) = spdx_or(&tokens)?;
    if !rest.is_empty() || options.is_empty() {
        return None;
    }
    for o in &mut options {
        o.sort_by_key(|id| rank(id));
        o.dedup();
    }
    // By the least preferred licence each option needs, then the next.
    options.sort_by_key(|o| {
        let mut r: Vec<usize> = o.iter().map(|id| rank(id)).collect();
        r.sort_unstable_by(|a, b| b.cmp(a));
        r
    });
    options.dedup();
    Some(options)
}

fn rank(id: &str) -> usize {
    PREFERENCE
        .iter()
        .position(|p| *p == id)
        .unwrap_or(usize::MAX)
}

type Dnf = Vec<Vec<&'static str>>;

/// `term (OR term)*`
fn spdx_or<'a>(t: &'a [&'a str]) -> Option<(Dnf, &'a [&'a str])> {
    let (mut out, mut rest) = spdx_and(t)?;
    while rest.first() == Some(&"OR") {
        let (more, r) = spdx_and(&rest[1..])?;
        out.extend(more);
        rest = r;
    }
    Some((out, rest))
}

/// `factor (AND factor)*`: every combination of the factors' options.
fn spdx_and<'a>(t: &'a [&'a str]) -> Option<(Dnf, &'a [&'a str])> {
    let (mut out, mut rest) = spdx_factor(t)?;
    while rest.first() == Some(&"AND") {
        let (more, r) = spdx_factor(&rest[1..])?;
        out = out
            .iter()
            .flat_map(|a| {
                more.iter().map(move |b| {
                    let mut c = a.clone();
                    c.extend(b);
                    c
                })
            })
            .collect();
        rest = r;
    }
    Some((out, rest))
}

/// A licence id (no options if unknown or with an exception) or a
/// parenthesised expression.
fn spdx_factor<'a>(t: &'a [&'a str]) -> Option<(Dnf, &'a [&'a str])> {
    match *t.first()? {
        "(" => {
            let (inner, rest) = spdx_or(&t[1..])?;
            (rest.first() == Some(&")")).then(|| (inner, &rest[1..]))
        }
        "OR" | "AND" | ")" | "WITH" => None,
        _ if t.get(1) == Some(&"WITH") => Some((Vec::new(), t.get(3..)?)),
        id => {
            let known = PREFERENCE.iter().find(|p| **p == id);
            Some((known.map(|k| vec![*k]).into_iter().collect(), &t[1..]))
        }
    }
}

/// The licence texts in a crate's root, by the licence they contain.
fn licence_files(dir: &Path) -> Result<BTreeMap<&'static str, String>, String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            p.is_file()
                && ["licen", "copying", "unlicense"]
                    .iter()
                    .any(|s| n.starts_with(s))
        })
        .collect();
    paths.sort();
    let mut found = BTreeMap::new();
    for p in paths {
        let Ok(text) = fs::read_to_string(&p) else {
            continue;
        };
        let text = normalize(&text);
        if let Some(id) = classify(&text) {
            found.entry(id).or_insert(text);
        }
    }
    Ok(found)
}

fn classify(text: &str) -> Option<&'static str> {
    let has = |s: &str| text.contains(s);
    // Before MIT: the Unicode licence uses the same grant.
    if has("UNICODE LICENSE V3") {
        Some("Unicode-3.0")
    } else if has("Permission is hereby granted, free of charge") {
        Some("MIT")
    } else if has("Permission to use, copy, modify, and/or distribute this software")
        && has("provided that the above copyright notice")
    {
        Some("ISC")
    } else if has("Altered source versions must be plainly marked") {
        Some("Zlib")
    } else if has("Redistribution and use in source and binary forms") {
        Some(if has("endorse or promote") {
            "BSD-3-Clause"
        } else {
            "BSD-2-Clause"
        })
    } else if has("Apache License") && has("Version 2.0") {
        Some("Apache-2.0")
    } else if has("free and unencumbered software released into the public domain") {
        Some("Unlicense")
    } else if has("Permission to use, copy, modify, and/or distribute this software") {
        Some("0BSD")
    } else {
        None
    }
}

fn names_holder(text: &str) -> bool {
    text.lines()
        .any(|l| l.trim_start().to_lowercase().starts_with("copyright"))
}

/// Unix line ends, no trailing blanks, no leading/trailing empty lines.
fn normalize(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    lines.join("\n").trim_matches('\n').to_string()
}

fn standard_text(id: &str, holder: &str) -> Result<String, String> {
    match id {
        "MIT" => Ok(format!("Copyright (c) {holder}\n\n{MIT_BODY}")),
        "BSD-3-Clause" => Ok(format!("Copyright (c) {holder}\n\n{BSD3_BODY}")),
        "Apache-2.0" => Ok(format!(
            "Copyright (c) {holder}\n\n{}",
            normalize(include_str!("../licenses/Apache-2.0.txt"))
        )),
        "CC0-1.0" => Ok(format!(
            "By {holder}, dedicated to the public domain under CC0 1.0 Universal\n\
             (https://creativecommons.org/publicdomain/zero/1.0/)."
        )),
        _ => Err(format!(
            "no licence file for {holder} and no standard {id} text"
        )),
    }
}

/// libjpeg-turbo, built from the sources bundled in turbojpeg-sys: the IJG
/// licence (README.ijg, "LEGAL ISSUES") and the TurboJPEG API's BSD licence
/// (LICENSE.md). Both demand them in the documentation of a binary.
fn libjpeg_turbo(turbojpeg_sys: &Path) -> Result<Notice, String> {
    let dir = turbojpeg_sys.join("libjpeg-turbo");
    let read = |f: &str| {
        fs::read_to_string(dir.join(f))
            .map(|t| normalize(&t))
            .map_err(|e| format!("{}/{f}: {e}", dir.display()))
    };
    let section = |text: &str, start: &str, end: &str| -> Result<String, String> {
        let a = text
            .find(start)
            .ok_or_else(|| format!("libjpeg-turbo: `{start}` not found"))?;
        let b = text[a..]
            .find(end)
            .map(|b| a + b)
            .ok_or_else(|| format!("libjpeg-turbo: `{end}` not found"))?;
        Ok(text[a..b].trim_end().to_string())
    };
    let ijg = section(&read("README.ijg")?, "LEGAL ISSUES\n", "\nREFERENCES\n")?;
    let bsd = section(
        &read("LICENSE.md")?,
        "The Modified (3-clause) BSD License\n",
        "\nWhy Two Licenses?\n",
    )?
    .replace("<br>", "");
    let version = read("CMakeLists.txt")?
        .lines()
        .find_map(|l| l.strip_prefix("set(VERSION ")?.strip_suffix(')'))
        .map(str::to_string)
        .ok_or("libjpeg-turbo: version not found in CMakeLists.txt")?;
    Ok(Notice {
        covers: vec![format!(
            "libjpeg-turbo {version} (https://libjpeg-turbo.org), \
             the Independent JPEG Group's software"
        )],
        licence: "IJG AND BSD-3-Clause".into(),
        text: format!("{ijg}\n\n\n{bsd}"),
    })
}

/// A text file in UTF-8 or, failing that, Windows-1252 (rav1e's PATENTS).
fn read_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(String::from_utf8(bytes).unwrap_or_else(|e| {
        e.into_bytes()
            .into_iter()
            .map(|b| match b {
                0x91 => '‘',
                0x92 => '’',
                0x93 => '“',
                0x94 => '”',
                0x96 => '–',
                0x97 => '—',
                b => char::from(b),
            })
            .collect()
    }))
}

/// libwebp, built from the sources bundled in libwebp-sys: Google's BSD
/// licence and patent grant (COPYING, PATENTS).
fn libwebp(libwebp_sys: &Path) -> Result<Notice, String> {
    let dir = libwebp_sys.join("vendor");
    let read = |f: &str| {
        fs::read_to_string(dir.join(f))
            .map(|t| normalize(&t))
            .map_err(|e| format!("{}/{f}: {e}", dir.display()))
    };
    Ok(Notice {
        covers: vec!["libwebp (https://chromium.googlesource.com/webm/libwebp)".into()],
        licence: "BSD-3-Clause".into(),
        text: format!("{}\n\n\n{}", read("COPYING")?, read("PATENTS")?),
    })
}

/// Little CMS (Linux, macOS), built from the sources bundled in lcms2-sys.
fn little_cms(lcms2_sys: &Path) -> Result<Notice, String> {
    let dir = lcms2_sys.join("vendor");
    let read = |f: &str| {
        fs::read_to_string(dir.join(f)).map_err(|e| format!("{}/{f}: {e}", dir.display()))
    };
    let header = read("include/lcms2.h")?;
    let version = header
        .lines()
        .find_map(|l| {
            l.strip_prefix("#define LCMS_VERSION")?
                .trim()
                .parse::<u32>()
                .ok()
        })
        .ok_or("Little CMS: version not found in lcms2.h")?;
    Ok(Notice {
        covers: vec![format!(
            "Little CMS {}.{} (https://www.littlecms.com)",
            version / 1000,
            version % 1000 / 10
        )],
        licence: "MIT".into(),
        text: normalize(&read("LICENSE")?),
    })
}

const MIT_BODY: &str = "\
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.";

const BSD3_BODY: &str = "\
Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software
   without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS \"AS IS\"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternatives_prefer_mit_and_reject_unknown() {
        assert_eq!(
            alternatives("Apache-2.0 OR MIT"),
            Some(vec![vec!["MIT"], vec!["Apache-2.0"]])
        );
        assert_eq!(
            alternatives("BSD-3-Clause/MIT"),
            Some(vec![vec!["MIT"], vec!["BSD-3-Clause"]])
        );
        assert_eq!(
            alternatives("Unlicense OR MIT"),
            Some(vec![vec!["MIT"], vec!["Unlicense"]])
        );
        assert_eq!(
            alternatives("Apache-2.0 AND MIT"),
            Some(vec![vec!["MIT", "Apache-2.0"]])
        );
        assert_eq!(
            alternatives("(MIT OR Apache-2.0) AND Zlib"),
            Some(vec![vec!["MIT", "Zlib"], vec!["Zlib", "Apache-2.0"]])
        );
        assert_eq!(
            alternatives("MIT AND Unicode-3.0"),
            Some(vec![vec!["MIT", "Unicode-3.0"]])
        );
        assert_eq!(alternatives("MIT AND GPL-3.0-only"), None);
        assert_eq!(alternatives("GPL-3.0-only"), None);
        assert_eq!(alternatives("Apache-2.0 WITH LLVM-exception"), None);
        assert_eq!(
            alternatives("Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT"),
            Some(vec![vec!["MIT"], vec!["Apache-2.0"]])
        );
        assert_eq!(alternatives("GPL-2.0 OR MIT"), Some(vec![vec!["MIT"]]));
        assert_eq!(alternatives("(MIT"), None);
    }

    #[test]
    fn classify_recognizes_the_usual_texts() {
        assert_eq!(classify(MIT_BODY), Some("MIT"));
        assert_eq!(classify(BSD3_BODY), Some("BSD-3-Clause"));
        assert_eq!(
            classify("Redistribution and use in source and binary forms, ..."),
            Some("BSD-2-Clause")
        );
        assert_eq!(
            classify("Apache License\nVersion 2.0, January 2004"),
            Some("Apache-2.0")
        );
        assert_eq!(classify("Licensed under either of ..."), None);
    }

    #[test]
    fn names_holder_needs_a_copyright_line() {
        assert!(!names_holder(MIT_BODY));
        assert!(names_holder("MIT License\n\n  Copyright (c) 2020 A\n"));
    }

    #[test]
    fn normalize_strips_trailing_blanks_and_crlf() {
        assert_eq!(normalize("\r\nA  \r\n\r\nB\r\n\r\n"), "A\n\nB");
    }
}
