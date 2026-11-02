//! The checks and the code generation of `build.rs`, apart from the TOML parsing.
//!
//! `build.rs` reads every `i18n/<code>.toml` into a [`Language`] and hands them to
//! [`check`]; the lib tests include this file too and feed it hand-built languages
//! (docs/specs/localization.md AC-05, AC-06).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// The reference language: every key comes from it, a missing text falls back to it.
pub const REFERENCE: &str = "en";
const PLURAL_CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

/// One translation file as written, before any check.
pub struct Language {
    /// The file, for error messages.
    pub file: String,
    /// BCP 47 code, the lowercase file stem.
    pub code: String,
    /// `[meta] name`.
    pub name: String,
    /// `[meta] plural`, `auto` when absent.
    pub plural_rule: String,
    /// `[meta] direction`, if given.
    pub direction: Option<String>,
    /// `key = "text"`.
    pub simple: BTreeMap<String, String>,
    /// `[messages.<key>]`: plural category → text.
    pub plural: BTreeMap<String, BTreeMap<String, String>>,
}

/// A checked language with every text resolved (its own or the English fallback).
struct Resolved {
    code: String,
    name: String,
    rtl: bool,
    rule: &'static str,
    /// One text per simple key, in key order.
    simple: Vec<String>,
    /// Six forms per plural key (in `PLURAL_CATEGORIES` order), in key order.
    plural: Vec<Vec<String>>,
}

/// The checked tables, ready to be written out.
pub struct Tables {
    simple_keys: Vec<String>,
    plural_keys: Vec<String>,
    langs: Vec<Resolved>,
    /// Texts that fell back to English: (language, key).
    missing: Vec<(String, String)>,
}

/// Checks the languages against each other and resolves every text; `Err` holds one
/// line per problem, each naming the language or file and the key.
pub fn check(mut langs: Vec<Language>) -> Result<Tables, Vec<String>> {
    // Reference language first, so its index is stable and it is the fallback.
    langs.sort_by_key(|l| (l.code != REFERENCE, l.code.clone()));
    if langs.first().is_none_or(|l| l.code != REFERENCE) {
        return Err(vec![format!("i18n/{REFERENCE}.toml is required")]);
    }

    let mut errors = Vec::new();
    let mut rtl = Vec::new();
    let mut rules = Vec::new();
    for lang in &langs {
        check_file(&mut errors, lang);
        rtl.push(match lang.direction.as_deref() {
            None | Some("ltr") => false,
            Some("rtl") => true,
            Some(d) => {
                errors.push(format!(
                    "{}: [meta] direction must be \"ltr\" or \"rtl\", not {d:?}",
                    lang.file
                ));
                false
            }
        });
        match plural_rule_ident(&lang.plural_rule, &lang.code) {
            Ok(rule) => rules.push(rule),
            Err(e) => errors.push(e),
        }
    }

    let reference = &langs[0];
    let simple_keys: Vec<String> = reference.simple.keys().cloned().collect();
    let plural_keys: Vec<String> = reference.plural.keys().cloned().collect();
    let mut missing = Vec::new();
    for lang in &langs[1..] {
        for key in lang.simple.keys().chain(lang.plural.keys()) {
            if !reference.simple.contains_key(key) && !reference.plural.contains_key(key) {
                errors.push(format!(
                    "{}: unknown key `{key}` (not in {REFERENCE}.toml)",
                    lang.code
                ));
            }
        }
        for key in &simple_keys {
            match lang.simple.get(key) {
                Some(text) => {
                    check_placeholders(&mut errors, lang, key, text, &reference.simple[key])
                }
                None => missing.push((lang.code.clone(), key.clone())),
            }
        }
        for key in &plural_keys {
            match lang.plural.get(key) {
                Some(forms) => {
                    if let Some(reference_other) = reference.plural[key].get("other") {
                        for text in forms.values() {
                            check_placeholders(&mut errors, lang, key, text, reference_other);
                        }
                    }
                }
                None => missing.push((lang.code.clone(), key.clone())),
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let resolved = langs
        .iter()
        .zip(rtl)
        .zip(rules)
        .map(|((lang, rtl), rule)| Resolved {
            code: lang.code.clone(),
            name: lang.name.clone(),
            rtl,
            rule,
            simple: simple_keys
                .iter()
                .map(|key| {
                    lang.simple
                        .get(key)
                        .unwrap_or(&reference.simple[key])
                        .clone()
                })
                .collect(),
            plural: plural_keys
                .iter()
                .map(|key| {
                    let forms = lang.plural.get(key).unwrap_or(&reference.plural[key]);
                    let other = &forms["other"];
                    PLURAL_CATEGORIES
                        .iter()
                        .map(|c| forms.get(*c).unwrap_or(other).clone())
                        .collect()
                })
                .collect(),
        })
        .collect();
    Ok(Tables {
        simple_keys,
        plural_keys,
        langs: resolved,
        missing,
    })
}

/// The checks that need only the file itself: snake_case keys, plural tables with an
/// `other` form and only known categories.
fn check_file(errors: &mut Vec<String>, lang: &Language) {
    for key in lang.simple.keys().chain(lang.plural.keys()) {
        if !key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            errors.push(format!("{}: key `{key}` must be snake_case", lang.file));
        }
    }
    for (key, forms) in &lang.plural {
        if !forms.contains_key("other") {
            errors.push(format!(
                "{}: plural message `{key}` needs an `other` form",
                lang.file
            ));
        }
        for cat in forms.keys() {
            if !PLURAL_CATEGORIES.contains(&cat.as_str()) {
                errors.push(format!(
                    "{}: `{key}` has unknown plural category `{cat}`",
                    lang.code
                ));
            }
        }
    }
}

impl Tables {
    /// One `cargo:warning` line per text that fell back to English.
    pub fn warnings(&self) -> Vec<String> {
        self.missing
            .iter()
            .map(|(lang, key)| {
                format!("cargo:warning=i18n: `{key}` is not translated to `{lang}`, using English")
            })
            .collect()
    }

    /// The generated Rust source (`OUT_DIR/generated.rs`).
    pub fn code(&self) -> String {
        let mut out = String::new();
        out.push_str("// @generated by build.rs from i18n/*.toml. Do not edit.\n\n");

        let _ = writeln!(
            out,
            "/// Number of simple messages.\npub const MSG_COUNT: usize = {};",
            self.simple_keys.len()
        );
        let _ = writeln!(
            out,
            "/// Number of plural messages.\npub const PLURAL_MSG_COUNT: usize = {};",
            self.plural_keys.len()
        );
        let _ = writeln!(
            out,
            "/// Number of languages.\npub const LANG_COUNT: usize = {};\n",
            self.langs.len()
        );

        emit_enum(
            &mut out,
            "Msg",
            "A translatable message.",
            &self.simple_keys,
        );
        emit_enum(
            &mut out,
            "PluralMsg",
            "A translatable message with plural forms.",
            &self.plural_keys,
        );

        let lang_idents: Vec<String> = self.langs.iter().map(|l| camel(&l.code)).collect();
        out.push_str("/// A language that ships with the application.\n");
        out.push_str(
            "#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n#[repr(u8)]\npub enum Lang {\n",
        );
        for (ident, lang) in lang_idents.iter().zip(&self.langs) {
            let _ = writeln!(out, "    /// {}", lang.name);
            let _ = writeln!(out, "    {ident},");
        }
        out.push_str("}\n\nimpl Lang {\n");
        let _ = writeln!(
            out,
            "    /// All languages; the reference language comes first.\n    pub const ALL: [Lang; LANG_COUNT] = [{}];",
            lang_idents
                .iter()
                .map(|i| format!("Lang::{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        out.push_str("    /// BCP 47 code, e.g. `de`.\n    pub const fn code(self) -> &'static str {\n        match self {\n");
        for (ident, lang) in lang_idents.iter().zip(&self.langs) {
            let _ = writeln!(out, "            Lang::{ident} => {:?},", lang.code);
        }
        out.push_str("        }\n    }\n    /// Name of the language in that language, e.g. `Deutsch`.\n    pub const fn native_name(self) -> &'static str {\n        match self {\n");
        for (ident, lang) in lang_idents.iter().zip(&self.langs) {
            let _ = writeln!(out, "            Lang::{ident} => {:?},", lang.name);
        }
        out.push_str("        }\n    }\n    /// Written right to left (mirrored UI).\n    pub const fn is_rtl(self) -> bool {\n        match self {\n");
        for (ident, lang) in lang_idents.iter().zip(&self.langs) {
            let _ = writeln!(out, "            Lang::{ident} => {},", lang.rtl);
        }
        out.push_str("        }\n    }\n    pub(crate) const fn plural_rule(self) -> crate::plural::Rule {\n        match self {\n");
        for (ident, lang) in lang_idents.iter().zip(&self.langs) {
            let _ = writeln!(
                out,
                "            Lang::{ident} => crate::plural::Rule::{},",
                lang.rule
            );
        }
        out.push_str("        }\n    }\n}\n\n");

        out.push_str("pub(crate) static TABLE: [[&str; MSG_COUNT]; LANG_COUNT] = [\n");
        for lang in &self.langs {
            let _ = writeln!(out, "    // {}", lang.code);
            out.push_str("    [\n");
            for text in &lang.simple {
                let _ = writeln!(out, "        {text:?},");
            }
            out.push_str("    ],\n");
        }
        out.push_str("];\n\n");

        out.push_str(
            "pub(crate) static PLURAL_TABLE: [[[&str; 6]; PLURAL_MSG_COUNT]; LANG_COUNT] = [\n",
        );
        for lang in &self.langs {
            let _ = writeln!(out, "    // {}", lang.code);
            out.push_str("    [\n");
            for forms in &lang.plural {
                let row: Vec<String> = forms.iter().map(|f| format!("{f:?}")).collect();
                let _ = writeln!(out, "        [{}],", row.join(", "));
            }
            out.push_str("    ],\n");
        }
        out.push_str("];\n\n");

        out.push_str(
            "/// Translations that fell back to English at build time: (language, key).\n",
        );
        out.push_str("pub static MISSING: &[(&str, &str)] = &[\n");
        for (lang, key) in &self.missing {
            let _ = writeln!(out, "    ({lang:?}, {key:?}),");
        }
        out.push_str("];\n");
        out
    }
}

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        set.insert(after[..end].to_owned());
        rest = &after[end + 1..];
    }
    set
}

fn check_placeholders(
    errors: &mut Vec<String>,
    lang: &Language,
    key: &str,
    text: &str,
    reference: &str,
) {
    let have = placeholders(text);
    let want = placeholders(reference);
    if have != want {
        errors.push(format!(
            "{}: `{key}` has placeholders {have:?}, {REFERENCE} has {want:?}",
            lang.code
        ));
    }
}

fn emit_enum(out: &mut String, name: &str, doc: &str, keys: &[String]) {
    let _ = writeln!(out, "/// {doc}");
    out.push_str("#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n#[repr(u16)]\n");
    let _ = writeln!(out, "pub enum {name} {{");
    for key in keys {
        let _ = writeln!(out, "    /// `{key}`");
        let _ = writeln!(out, "    {},", camel(key));
    }
    out.push_str("}\n\n");
    let _ = writeln!(out, "impl {name} {{");
    let _ = writeln!(
        out,
        "    /// Every message, in table order.\n    pub const ALL: [{name}; {}] = [{}];",
        keys.len(),
        keys.iter()
            .map(|k| format!("{name}::{}", camel(k)))
            .collect::<Vec<_>>()
            .join(", ")
    );
    out.push_str("    /// The key as written in the TOML files.\n    pub const fn key(self) -> &'static str {\n        match self {\n");
    for key in keys {
        let _ = writeln!(out, "            {name}::{} => {key:?},", camel(key));
    }
    out.push_str("        }\n    }\n}\n\n");
}

fn camel(key: &str) -> String {
    key.split(['_', '-'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            let first = c.next().unwrap().to_ascii_uppercase();
            std::iter::once(first).chain(c).collect::<String>()
        })
        .collect()
}

fn plural_rule_ident(rule: &str, code: &str) -> Result<&'static str, String> {
    let rule = if rule == "auto" {
        match code.split('-').next().unwrap() {
            "fr" | "pt" => "zero-one-other",
            "ru" | "uk" | "be" => "east-slavic",
            "pl" => "polish",
            "cs" | "sk" => "czech",
            "ja" | "zh" | "ko" | "vi" | "th" | "id" => "none",
            _ => "one-other",
        }
    } else {
        rule
    };
    Ok(match rule {
        "one-other" => "OneOther",
        "zero-one-other" => "ZeroOneOther",
        "east-slavic" => "EastSlavic",
        "polish" => "Polish",
        "czech" => "Czech",
        "none" => "None",
        other => return Err(format!("unknown plural rule `{other}` for `{code}`")),
    })
}
