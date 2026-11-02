---
status: todo
created: 2026-11-02
size: M
platforms: all
design: [blinzel.md §2, blinzel.md §8]
---

# Localization

## Context

Every user-visible string goes through `blinzel-i18n` (AGENTS.md §5.5, §9). The design spec decides nine UI languages (`blinzel.md` §2, Languages row) and translation as an array index with no parsing at startup (`blinzel.md` §8). This spec records what the code does today: compile-time tables from `i18n/*.toml`, English as reference and fallback, CLDR plural rules for integers, the language taken from the system with an override in the menu, and right-to-left support that is prepared but not used by any shipped language.

## Scope

### In scope

- The translation sources `i18n/<code>.toml` and what the build checks in them
- Looking up, filling and pluralizing messages at runtime
- Choosing the UI language: from the system on each platform, overridden in Menu → Language, switched without restart
- Right-to-left layout as far as it is implemented

### Out of scope

- Shipping a right-to-left language — because it needs a translation by native speakers (`blinzel.md` §8)
- Right-to-left layout of the filmstrip, the grid, the batch dialog and the other UI-drawn dialogs, the Windows setup dialog, text shaping direction on Linux and macOS and the macOS menu bar — because nothing mirrors them yet and no shipped language needs it
- Plural forms for fractional numbers — because the UI only counts whole things
- Localized number and date formatting — because no string needs it
- The settings file itself — because `settings.md` owns it

## Acceptance criteria

- [ ] AC-01 (always): The viewer ships exactly nine UI languages: English (`en`), German (`de`), Spanish (`es`), French (`fr`), Italian (`it`), Japanese (`ja`), Portuguese (Brazil) (`pt-br`), Russian (`ru`) and Simplified Chinese (`zh-cn`); English is the reference and the fallback language.
- [ ] AC-02 (always): Every shipped language has a non-empty text for every message, and each text has the same set of `{placeholders}` as the English one.
- [ ] AC-03 (always): No German message falls back to English.
- [ ] AC-04 (always): No message of any other shipped language falls back to English.
- [ ] AC-05 (error): If a translation file lacks a message that English has, then the build succeeds, prints a build warning naming the message key and the language, and the viewer shows the English text for that message in that language.
- [ ] AC-06 (error): If a translation file has a text whose placeholders differ from English (for a plural message: from the English `other` form), a key English does not have, an unknown plural category, a plural message without an `other` form, a key that is not snake_case, or a `[meta] direction` other than `"ltr"` or `"rtl"`, then the build fails with a message naming the language or file and the key.
- [ ] AC-07 (always): Filling a message replaces every `{name}` with the matching argument and leaves a placeholder without an argument, and an unclosed `{`, as written.
- [ ] AC-08 (always): A counted message picks its form by the CLDR integer rule family: "one-other" (1 → one, else other), "zero-one-other" (0 and 1 → one, else other), "east-slavic" (n mod 10 = 1 except n mod 100 = 11 → one; n mod 10 in 2–4 except n mod 100 in 12–14 → few; else many), "polish", "czech", and "none" (always other); a form a language does not define falls back to its `other` form, and `{n}` is filled with the count.
- [ ] AC-09 (always): English, German, Spanish and Italian use "one-other", French and Portuguese (Brazil) "zero-one-other", Russian "east-slavic", Japanese and Simplified Chinese "none".
- [ ] AC-10 (event): When the viewer starts and the settings name no shipped language, then it takes the UI language from the system's ordered preference list: the user's preferred UI languages on Windows, the preferred languages on macOS, and on Linux the variables `LANGUAGE` (colon-separated), `LC_ALL`, `LC_MESSAGES` and `LANG` in that order, ignoring `C`, `POSIX` and any `.encoding` or `@modifier` suffix.
- [ ] AC-11 (always): A preference list maps to a shipped language by trying, per tag in order: the exact tag, then its base language (`de-AT` → `de`), then a shipped regional variant of the base (`pt-PT` → `pt-br`, `fr-CA` → `fr`, `zh-SG` and `zh-Hans-CN` → `zh-cn`); `_` counts as `-`, case is ignored, and Traditional Chinese (`zh-TW`, `zh-HK`, `zh-MO`, `zh-Hant-*`) never maps to Simplified Chinese; if no tag maps, English.
- [ ] AC-12 (always): Menu → Language lists "Automatic (system)" first, then every shipped language under its own name in that language (e.g. "Deutsch", "日本語"), with a check mark on the current choice.
- [ ] AC-13 (event): When the user picks a language in Menu → Language, then the menu, the window title and every text drawn afterwards are in that language without a restart, and the choice is saved; picking "Automatic (system)" saves no language and switches to the system language.
- [ ] AC-14 (state): While the UI language is right-to-left, the caption buttons are mirrored (menu button at the right edge, close, maximize and minimize from the left edge), and the menu drawn by the viewer opens leftwards from the point it is opened at.
- [ ] AC-15 (state): While the UI language is right-to-left, → shows the previous image and ← the next one.
- [ ] AC-16 (state, CI: windows): While the UI language is right-to-left on Windows, the native image menu and tray menu are mirrored and open leftwards, text is laid out right to left, and message boxes are right-aligned with right-to-left reading.
- [ ] AC-17 (always): No shipped language is right-to-left.

## Technical notes

- Sources: `i18n/<code>.toml`, file stem = BCP 47 code (lowercase). `[meta]` has `name` (native name, required), `plural` (rule family, default `auto`: derived from the code) and optional `direction` (`"ltr"` default, `"rtl"`). `[messages]` holds `key = "text"` or a plural table `[messages.<key>]` with any of `zero`, `one`, `two`, `few`, `many`, `other` (`other` required). Currently 140 messages, one of them plural (`images_count`).
- `crates/blinzel-i18n/build.rs` generates `enum Msg`, `enum PluralMsg`, `enum Lang` (English first, then by code), `TABLE: [[&str; MSG_COUNT]; LANG_COUNT]`, `PLURAL_TABLE: [[[&str; 6]; PLURAL_MSG_COUNT]; LANG_COUNT]` and `MISSING` (the fallbacks). Missing texts are copied from English at build time.
- Runtime (`crates/blinzel-i18n/src/lib.rs`): the current language is one `AtomicU8`; `tr(msg)` is a load plus an array index; `trf`/`fill` substitute placeholders; `trp`/`trp_in` pick the plural form and add `n`; `negotiate` maps preference lists; `lang_from_code` is case-insensitive.
- System language: `run.rs::system_language` (`GetUserPreferredUILanguages`), `macos/shell.rs::system_language` (`NSLocale.preferredLanguages`), `linux/shell.rs::system_language` (environment). The settings key `language` overrides it (see `settings.md`).
- Right-to-left: `Lang::is_rtl()` feeds `Chrome::rtl` and `Popup` (all platforms, UI-drawn), the arrow-key swap in `App::key`, and on Windows `TPM_LAYOUTRTL`, `DWRITE_READING_DIRECTION_RIGHT_TO_LEFT` and `MB_RIGHT | MB_RTLREADING`.
- CI: `blinzel-i18n` tests run in the Linux and Windows jobs (`cargo test --workspace`); the macOS job does not test this crate.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Settings name a language that is not shipped (`language = "xx"`) | The system language is used |
| Linux with `LANG=C.UTF-8` only | `C` is ignored; English |
| Preference list `zh-Hant-HK, ja-JP` | Japanese, not Simplified Chinese |
| Empty preference list, or the system call fails | English |
| A message text contains `{` without `}` | Shown as written |
| Russian count 21, 22, 25, 11 | one, few, many, many |
| French count 0 | one form ("0 image") |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `exactly_nine_languages_ship_with_english_first_and_as_fallback` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |
| AC-02 | `every_language_has_every_key_and_same_placeholders` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |
| AC-03 | `german_is_complete` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |
| AC-04 | `no_shipped_language_falls_back_to_english` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |
| AC-05 | — (build script, no test) | open | |
| AC-06 | — (build script, no test) | open | |
| AC-07 | `translate_and_fill` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |
| AC-08 | `rules` (`crates/blinzel-i18n/src/plural.rs`), `plural_messages`, `plural_forms_of_shipped_languages` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |
| AC-09 | `shipped_languages_use_their_plural_rule_family` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |
| AC-10 | — | open | |
| AC-11 | `negotiation`, `negotiation_of_regional_variants` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |
| AC-12 | — (`ids_roundtrip` in `crates/blinzel-ui/src/menu.rs` checks only command ids) | open | |
| AC-13 | — (`current_language_switches` in `crates/blinzel-i18n/src/lib.rs` covers the lookup switch only, not the menu command) | open | |
| AC-14 | `right_to_left_mirrors` (`crates/blinzel-ui/src/chrome.rs`), `stays_inside_and_mirrors_for_rtl` (`crates/blinzel-ui/src/popup.rs`), CI: Linux | built | |
| AC-15 | — | open | |
| AC-16 | — | open | |
| AC-17 | `shipped_languages_are_left_to_right` (`crates/blinzel-i18n/src/lib.rs`), CI: Linux, Windows | built | |

## Decisions

- AC-01, AC-04, AC-09: the behavior already existed (this spec was extracted from the code), so their tests were green at once; each went red when broken on purpose (an extra `i18n/xx.toml`, a key removed from `es.toml`, `it.toml` switched to `zero-one-other`).
