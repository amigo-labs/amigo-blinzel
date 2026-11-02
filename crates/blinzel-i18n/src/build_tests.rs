//! The build script's checks, fed hand-built languages (docs/specs/localization.md
//! AC-05, AC-06).

#[path = "../build/tables.rs"]
mod tables;

use std::collections::BTreeMap;

use tables::{Language, check};

type Plural<'a> = (&'a str, &'a [(&'a str, &'a str)]);

fn lang(code: &str, simple: &[(&str, &str)], plural: &[Plural]) -> Language {
    let owned = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    Language {
        file: format!("i18n/{code}.toml"),
        code: code.to_owned(),
        name: code.to_owned(),
        plural_rule: "auto".to_owned(),
        direction: None,
        simple: owned(simple),
        plural: plural
            .iter()
            .map(|(k, forms)| ((*k).to_owned(), owned(forms)))
            .collect(),
    }
}

fn english() -> Language {
    lang(
        "en",
        &[("bye", "Bye"), ("hello", "Hello {name}")],
        &[("files", &[("one", "one file"), ("other", "{n} files")])],
    )
}

/// The build's error lines for English plus `de`, or a panic if it would succeed.
fn errors(de: Language) -> Vec<String> {
    match check(vec![english(), de]) {
        Ok(_) => panic!("the build succeeds"),
        Err(errors) => errors,
    }
}

/// Some error line contains every one of `parts`.
fn names(errors: &[String], parts: &[&str]) -> bool {
    errors.iter().any(|e| parts.iter().all(|p| e.contains(p)))
}

#[test]
fn a_missing_text_warns_and_falls_back_to_english() {
    let de = lang("de", &[("bye", "Tschüss")], &[]);
    let Ok(tables) = check(vec![de, english()]) else {
        panic!("a missing text must not fail the build");
    };
    assert_eq!(
        tables.warnings(),
        [
            "cargo:warning=i18n: `hello` is not translated to `de`, using English",
            "cargo:warning=i18n: `files` is not translated to `de`, using English",
        ]
    );
    let code = tables.code();
    // German's rows hold its own text where it has one and the English text elsewhere.
    assert!(
        code.contains("    // de\n    [\n        \"Tschüss\",\n        \"Hello {name}\",\n    ],"),
        "{code}"
    );
    assert!(
        code.contains(
            "    // de\n    [\n        [\"{n} files\", \"one file\", \"{n} files\", \
             \"{n} files\", \"{n} files\", \"{n} files\"],\n    ],"
        ),
        "{code}"
    );
    assert!(
        code.contains("    (\"de\", \"hello\"),\n    (\"de\", \"files\"),\n];"),
        "{code}"
    );
}

#[test]
fn a_complete_translation_builds_without_warnings() {
    let de = lang(
        "de",
        &[("bye", "Tschüss"), ("hello", "Hallo {name}")],
        &[("files", &[("one", "{n} Datei"), ("other", "{n} Dateien")])],
    );
    let Ok(tables) = check(vec![english(), de]) else {
        panic!("a complete translation must build");
    };
    assert!(tables.warnings().is_empty());
}

#[test]
fn placeholders_that_differ_from_english_fail_the_build() {
    let de = lang("de", &[("bye", "Tschüss"), ("hello", "Hallo {nme}")], &[]);
    assert!(names(&errors(de), &["de", "`hello`", "placeholders"]));
}

#[test]
fn plural_placeholders_that_differ_from_the_english_other_form_fail_the_build() {
    // English's `one` form has no `{n}`; the reference is its `other` form.
    let de = lang(
        "de",
        &[],
        &[("files", &[("one", "eine Datei"), ("other", "{n} Dateien")])],
    );
    assert!(names(&errors(de), &["de", "`files`", "placeholders"]));
    let de = lang("de", &[], &[("files", &[("other", "{count} Dateien")])]);
    assert!(names(&errors(de), &["de", "`files`", "placeholders"]));
}

#[test]
fn a_key_english_does_not_have_fails_the_build() {
    let de = lang("de", &[("bye", "Tschüss"), ("ciao", "Ciao")], &[]);
    assert!(names(&errors(de), &["de", "`ciao`", "unknown key"]));
}

#[test]
fn an_unknown_plural_category_fails_the_build() {
    let de = lang(
        "de",
        &[],
        &[("files", &[("lots", "{n}"), ("other", "{n} Dateien")])],
    );
    assert!(names(
        &errors(de),
        &["de", "`files`", "`lots`", "unknown plural category"]
    ));
}

#[test]
fn a_plural_message_without_an_other_form_fails_the_build() {
    let de = lang("de", &[], &[("files", &[("one", "{n} Datei")])]);
    assert!(names(
        &errors(de),
        &["i18n/de.toml", "`files`", "`other` form"]
    ));
}

#[test]
fn a_key_that_is_not_snake_case_fails_the_build() {
    for key in ["Bye", "good-bye", "good bye"] {
        let de = lang("de", &[(key, "Tschüss")], &[]);
        let errors = errors(de);
        assert!(
            names(
                &errors,
                &["i18n/de.toml", &format!("`{key}`"), "snake_case"]
            ),
            "{errors:?}"
        );
    }
}

#[test]
fn a_direction_other_than_ltr_or_rtl_fails_the_build() {
    for direction in ["ltr", "rtl"] {
        let mut de = lang("de", &[("bye", "Tschüss")], &[]);
        de.direction = Some(direction.to_owned());
        assert!(check(vec![english(), de]).is_ok(), "{direction}");
    }
    let mut de = lang("de", &[("bye", "Tschüss")], &[]);
    de.direction = Some("up".to_owned());
    assert!(names(&errors(de), &["i18n/de.toml", "direction", "\"up\""]));
}
