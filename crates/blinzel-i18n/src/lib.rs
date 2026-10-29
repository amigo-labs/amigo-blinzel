//! Translations compiled into static tables.
//!
//! `tr(Msg::Copy)` is one atomic load plus one array index: nothing is parsed
//! or hashed at startup or at runtime. Sources live in `i18n/<lang>.toml`.

mod plural;

pub use plural::Category;

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The reference language everything falls back to.
pub const FALLBACK: Lang = Lang::ALL[0];

/// Sets the UI language for all following `tr*` calls.
pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// The current UI language.
pub fn lang() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize]
}

/// Whether the current language is written right to left: the window
/// mirrors its controls, menus and text direction.
pub fn is_rtl() -> bool {
    lang().is_rtl()
}

/// Translates `msg` into the current language.
#[inline]
pub fn tr(msg: Msg) -> &'static str {
    tr_in(lang(), msg)
}

/// Translates `msg` into `lang`.
#[inline]
pub fn tr_in(lang: Lang, msg: Msg) -> &'static str {
    TABLE[lang as usize][msg as usize]
}

/// Translates `msg` and fills `{name}` placeholders from `args`.
pub fn trf(msg: Msg, args: &[(&str, &dyn Display)]) -> String {
    fill(tr(msg), args)
}

/// Picks the plural form of `msg` for `n` in the current language and fills
/// `{n}` plus any other placeholders.
pub fn trp(msg: PluralMsg, n: u64, args: &[(&str, &dyn Display)]) -> String {
    trp_in(lang(), msg, n, args)
}

/// Like [`trp`] for an explicit language.
pub fn trp_in(lang: Lang, msg: PluralMsg, n: u64, args: &[(&str, &dyn Display)]) -> String {
    let category = lang.plural_rule().category(n);
    let text = PLURAL_TABLE[lang as usize][msg as usize][category as usize];
    let mut all: Vec<(&str, &dyn Display)> = Vec::with_capacity(args.len() + 1);
    all.push(("n", &n));
    all.extend_from_slice(args);
    fill(text, &all)
}

/// The plural category `n` falls into for `lang`.
pub fn plural_category(lang: Lang, n: u64) -> Category {
    lang.plural_rule().category(n)
}

/// Replaces `{name}` with the matching argument; unknown placeholders stay.
pub fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match args.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => {
                        let _ = write!(out, "{v}");
                    }
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Picks the best shipped language for a list of BCP 47 tags in order of
/// preference (e.g. from `GetUserPreferredUILanguages`): exact match, then
/// base language (`de-AT` → `de`), then a shipped regional variant of the
/// base language (`pt-PT` → `pt-br`), then the next tag, finally English.
/// Traditional Chinese never gets Simplified Chinese.
pub fn negotiate<'a>(preferred: impl IntoIterator<Item = &'a str>) -> Lang {
    for tag in preferred {
        let tag = tag.trim().to_ascii_lowercase().replace('_', "-");
        if tag.is_empty() {
            continue;
        }
        if let Some(l) = Lang::ALL.iter().find(|l| l.code() == tag) {
            return *l;
        }
        let base = tag.split('-').next().unwrap_or("");
        if let Some(l) = Lang::ALL.iter().find(|l| l.code() == base) {
            return *l;
        }
        if is_traditional_chinese(&tag) {
            continue;
        }
        if let Some(l) = Lang::ALL
            .iter()
            .find(|l| l.code().split('-').next() == Some(base))
        {
            return *l;
        }
    }
    FALLBACK
}

/// `zh-TW`, `zh-HK`, `zh-MO` and `zh-Hant-*` (lowercase tag).
fn is_traditional_chinese(tag: &str) -> bool {
    let mut parts = tag.split('-');
    parts.next() == Some("zh") && parts.any(|p| matches!(p, "hant" | "tw" | "hk" | "mo"))
}

/// Looks up a language by its code (`"de"`), case-insensitively.
pub fn lang_from_code(code: &str) -> Option<Lang> {
    Lang::ALL
        .iter()
        .copied()
        .find(|l| l.code().eq_ignore_ascii_case(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_languages_are_left_to_right() {
        // A right-to-left language sets `[meta] direction = "rtl"`.
        for l in Lang::ALL {
            assert!(!l.is_rtl(), "{}", l.code());
        }
    }

    fn placeholders(text: &str) -> Vec<&str> {
        let mut v = Vec::new();
        let mut rest = text;
        while let Some(s) = rest.find('{') {
            let after = &rest[s + 1..];
            let Some(e) = after.find('}') else { break };
            v.push(&after[..e]);
            rest = &after[e + 1..];
        }
        v.sort_unstable();
        v
    }

    #[test]
    fn english_is_reference_and_german_ships() {
        assert_eq!(FALLBACK.code(), "en");
        assert!(lang_from_code("de").is_some());
        assert_eq!(Lang::ALL.len(), LANG_COUNT);
    }

    #[test]
    fn every_language_has_every_key_and_same_placeholders() {
        if !MISSING.is_empty() {
            eprintln!("warning: untranslated keys fall back to English: {MISSING:?}");
        }
        for lang in Lang::ALL {
            for msg in Msg::ALL {
                let text = tr_in(lang, msg);
                assert!(!text.is_empty(), "{}: {} is empty", lang.code(), msg.key());
                assert_eq!(
                    placeholders(text),
                    placeholders(tr_in(FALLBACK, msg)),
                    "{}: {} placeholders differ",
                    lang.code(),
                    msg.key()
                );
            }
        }
    }

    #[test]
    fn german_is_complete() {
        assert!(
            !MISSING.iter().any(|(l, _)| *l == "de"),
            "German must be complete: {MISSING:?}"
        );
    }

    #[test]
    fn translate_and_fill() {
        let de = lang_from_code("de").unwrap();
        assert_eq!(tr_in(de, Msg::Copy), "Kopieren");
        assert_eq!(tr_in(Lang::ALL[0], Msg::Copy), "Copy");
        assert_eq!(
            fill("{index} / {total}", &[("index", &3), ("total", &10)]),
            "3 / 10"
        );
        assert_eq!(fill("{a} {b}", &[("a", &1)]), "1 {b}");
        assert_eq!(fill("no close {brace", &[]), "no close {brace");
    }

    #[test]
    fn plural_messages() {
        let de = lang_from_code("de").unwrap();
        assert_eq!(trp_in(de, PluralMsg::ImagesCount, 1, &[]), "1 Bild");
        assert_eq!(trp_in(de, PluralMsg::ImagesCount, 0, &[]), "0 Bilder");
        assert_eq!(trp_in(FALLBACK, PluralMsg::ImagesCount, 2, &[]), "2 images");
    }

    #[test]
    fn negotiation() {
        let de = lang_from_code("de").unwrap();
        assert_eq!(negotiate(["de-DE", "en-US"]), de);
        assert_eq!(negotiate(["de_AT"]), de);
        assert_eq!(negotiate(["xx-YY", "de-CH"]), de);
        assert_eq!(negotiate(["xx-YY"]), FALLBACK);
        assert_eq!(negotiate([]), FALLBACK);
    }

    #[test]
    fn negotiation_of_regional_variants() {
        let code = |tags: &[&str]| negotiate(tags.iter().copied()).code();
        assert_eq!(code(&["fr-FR"]), "fr");
        assert_eq!(code(&["fr-CA"]), "fr");
        assert_eq!(code(&["pt-BR"]), "pt-br");
        assert_eq!(code(&["pt-PT"]), "pt-br");
        assert_eq!(code(&["zh-CN"]), "zh-cn");
        assert_eq!(code(&["zh-Hans-CN"]), "zh-cn");
        assert_eq!(code(&["zh-SG"]), "zh-cn");
        // Traditional Chinese falls through to the next tag, then English.
        assert_eq!(code(&["zh-TW"]), "en");
        assert_eq!(code(&["zh-Hant-HK", "ja-JP"]), "ja");
        assert_eq!(code(&["zh-HK"]), "en");
    }

    #[test]
    fn plural_forms_of_shipped_languages() {
        let ru = lang_from_code("ru").unwrap();
        let forms: Vec<String> = [1, 2, 5, 21]
            .into_iter()
            .map(|n| trp_in(ru, PluralMsg::ImagesCount, n, &[]))
            .collect();
        assert_eq!(forms[0], tr_n(ru, Category::One, 1));
        assert_eq!(forms[1], tr_n(ru, Category::Few, 2));
        assert_eq!(forms[2], tr_n(ru, Category::Many, 5));
        assert_eq!(forms[3], tr_n(ru, Category::One, 21));
        assert_ne!(forms[1], forms[2]);
        // No plural forms: every count uses `other`.
        let ja = lang_from_code("ja").unwrap();
        assert_eq!(plural_category(ja, 1), Category::Other);
        let fr = lang_from_code("fr").unwrap();
        assert_eq!(plural_category(fr, 0), Category::One);
    }

    /// The `images_count` form `cat` of `lang`, filled with `n`.
    fn tr_n(lang: Lang, cat: Category, n: u64) -> String {
        let text = PLURAL_TABLE[lang as usize][PluralMsg::ImagesCount as usize][cat as usize];
        fill(text, &[("n", &n)])
    }

    #[test]
    fn current_language_switches() {
        let de = lang_from_code("de").unwrap();
        set_lang(de);
        assert_eq!(lang(), de);
        assert_eq!(tr(Msg::Exit), "Beenden");
        set_lang(FALLBACK);
        assert_eq!(tr(Msg::Exit), "Exit");
    }
}
