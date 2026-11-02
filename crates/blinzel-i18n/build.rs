//! Turns `i18n/<lang>.toml` into static tables at compile time.
//!
//! The generated code contains one `Msg` enum (keys of the English reference
//! file) and one `[&str; N]` table per language, so a translation at runtime is
//! a single array index. Missing translations fall back to English at build
//! time and are listed in `MISSING`; a placeholder mismatch is a build error.
//! This file only reads the TOML; the checks and the code generation live in
//! `build/tables.rs`, which the lib tests include too.

#[path = "build/tables.rs"]
mod tables;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{env, fs};

use tables::Language;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let dir = manifest.join("../../i18n");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut langs = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    entries.sort();
    for path in entries {
        println!("cargo:rerun-if-changed={}", path.display());
        langs.push(load(&path));
    }

    let tables =
        tables::check(langs).unwrap_or_else(|errors| panic!("i18n errors:\n{}", errors.join("\n")));
    for warning in tables.warnings() {
        println!("{warning}");
    }
    let out_path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("generated.rs");
    fs::write(out_path, tables.code()).unwrap();
}

fn load(path: &Path) -> Language {
    let code = path.file_stem().unwrap().to_string_lossy().to_lowercase();
    let text = fs::read_to_string(path).unwrap();
    let doc: toml::Table = text
        .parse()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let meta = doc.get("meta").and_then(|m| m.as_table());
    let meta_str = |field: &str| meta.and_then(|m| m.get(field)).and_then(|v| v.as_str());
    let name = meta_str("name")
        .unwrap_or_else(|| panic!("{}: missing [meta] name", path.display()))
        .to_owned();

    let mut simple = BTreeMap::new();
    let mut plural = BTreeMap::new();
    let messages = doc
        .get("messages")
        .and_then(|m| m.as_table())
        .unwrap_or_else(|| panic!("{}: missing [messages]", path.display()));
    for (key, value) in messages {
        match value {
            toml::Value::String(s) => {
                simple.insert(key.clone(), s.clone());
            }
            toml::Value::Table(t) => {
                let forms = t
                    .iter()
                    .map(|(cat, v)| {
                        let s = v.as_str().unwrap_or_else(|| {
                            panic!("{}: `{key}.{cat}` must be a string", path.display())
                        });
                        (cat.clone(), s.to_owned())
                    })
                    .collect::<BTreeMap<_, _>>();
                plural.insert(key.clone(), forms);
            }
            _ => panic!(
                "{}: `{key}` must be a string or a plural table",
                path.display()
            ),
        }
    }
    Language {
        file: path.display().to_string(),
        code,
        name,
        plural_rule: meta_str("plural").unwrap_or("auto").to_owned(),
        direction: meta_str("direction").map(str::to_owned),
        simple,
        plural,
    }
}
