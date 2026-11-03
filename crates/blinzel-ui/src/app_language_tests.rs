//! App-level tests of the UI language: Menu → Language, switching
//! without a restart and right-to-left paging
//! (`docs/specs/localization.md`).

use super::*;
use crate::menu::{self, MenuItem};
use crate::testkit::Harness;
use blinzel_i18n::{Lang, fill, lang_from_code, tr_in};

/// `(command, label, checked)` of every entry under Menu → Language.
fn language_menu(h: &Harness) -> Vec<(Cmd, String, bool)> {
    let items = menu::items(&h.menu_state());
    let sub = items
        .iter()
        .find_map(|i| match i {
            MenuItem::Sub(label, sub) if label == tr(Msg::MenuLanguage) => Some(sub),
            _ => None,
        })
        .expect("Menu → Language");
    sub.iter()
        .map(|i| match i {
            MenuItem::Item {
                cmd,
                label,
                checked,
                ..
            } => (*cmd, label.clone(), *checked),
            other => panic!("not a language: {other:?}"),
        })
        .collect()
}

fn lang(code: &str) -> Lang {
    lang_from_code(code).unwrap()
}

/// AC-12
#[test]
fn the_language_menu_lists_automatic_then_every_language_in_its_own_name() {
    let mut h = Harness::new();
    let names = [
        "English",
        "Deutsch",
        "Español",
        "Français",
        "Italiano",
        "日本語",
        "Português (Brasil)",
        "Русский",
        "简体中文",
    ];
    let mut expected = vec![(Cmd::Language(None), "Automatic (system)".to_string(), true)];
    for (l, name) in Lang::ALL.into_iter().zip(names) {
        expected.push((Cmd::Language(Some(l)), name.to_string(), false));
    }
    assert_eq!(language_menu(&h), expected);

    // The check mark follows the choice; the names stay native.
    let ja = lang("ja");
    h.command(Cmd::Language(Some(ja)));
    let entries = language_menu(&h);
    assert_eq!(entries[0].1, tr_in(ja, Msg::LanguageAuto));
    let checked: Vec<Cmd> = entries.iter().filter(|e| e.2).map(|e| e.0).collect();
    assert_eq!(checked, [Cmd::Language(Some(ja))]);
    let labels: Vec<&str> = entries[1..].iter().map(|e| e.1.as_str()).collect();
    assert_eq!(labels, names);
}

/// AC-13
#[test]
fn picking_a_language_switches_menu_title_and_drawn_text_at_once_and_saves_it() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.open("a.png");
    let title = |l: Lang| fill(tr_in(l, Msg::WindowTitle), &[("name", &"a.png")]);
    let drawn_menu = |h: &mut Harness| {
        h.app.open_menu((10.0, 10.0));
        h.render();
        h.app.popup = None;
        h.frame().texts.clone()
    };
    assert!(drawn_menu(&mut h).iter().any(|t| t == "Exit"));

    let de = lang("de");
    h.app.host.log.borrow_mut().title.clear();
    h.command(Cmd::Language(Some(de)));
    assert_eq!(blinzel_i18n::lang(), de);
    assert_eq!(h.host().title, title(de));
    assert_eq!(language_menu(&h)[0].1, "Automatisch (System)");
    let texts = drawn_menu(&mut h);
    assert!(texts.iter().any(|t| t == "Beenden"), "{texts:?}");
    assert!(!texts.iter().any(|t| t == "Exit"), "{texts:?}");
    assert_eq!(h.saved_settings().unwrap().language.as_deref(), Some("de"));

    // Automatic: no language in the file, the system's language at once.
    let ja = lang("ja");
    h.set_system_language(ja);
    h.app.host.log.borrow_mut().title.clear();
    h.command(Cmd::Language(None));
    assert_eq!(blinzel_i18n::lang(), ja);
    assert_eq!(h.host().title, title(ja));
    assert_eq!(h.saved_settings().unwrap().language, None);
    let texts = drawn_menu(&mut h);
    assert!(texts.iter().any(|t| t == tr_in(ja, Msg::Exit)), "{texts:?}");
    let checked: Vec<Cmd> = language_menu(&h)
        .iter()
        .filter(|e| e.2)
        .map(|e| e.0)
        .collect();
    assert_eq!(checked, [Cmd::Language(None)]);
}

/// AC-15: the arrow keys page the other way while the language is
/// right to left; the other paging keys keep their direction.
#[test]
fn right_to_left_swaps_what_the_arrow_keys_show() {
    use Key::*;
    let arrows = |rtl| [Right, Left].map(|k| nav_cmd(k, rtl));
    assert_eq!(arrows(false), [Some(Cmd::Next), Some(Cmd::Prev)]);
    assert_eq!(arrows(true), [Some(Cmd::Prev), Some(Cmd::Next)]);
    for k in [PageDown, Space, Down, PageUp, Backspace, Up, Home, End] {
        assert_eq!(nav_cmd(k, true), nav_cmd(k, false), "{k:?}");
    }
}
