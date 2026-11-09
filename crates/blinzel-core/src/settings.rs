//! User settings in a tiny TOML subset (`key = value` lines with strings,
//! integers and booleans). Parsing takes microseconds, so it never shows up
//! in startup time; the file stays readable and editable by hand.

use crate::layout::{ViewMode, ViewOptions};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub view: ViewOptions,
    /// BCP 47 code, or `None` for the system language.
    pub language: Option<String>,
    /// Reuse a running window instead of starting a second one.
    pub single_instance: bool,
    /// Images to prefetch ahead / behind.
    pub prefetch_ahead: u32,
    pub prefetch_behind: u32,
    /// Background colour around the image, `#RRGGBB`.
    pub background: [u8; 3],
    /// Check for updates daily and install them silently.
    pub auto_update: bool,
    /// Stay running in the background (tray) after closing, and start at
    /// sign-in, so opening an image needs no process or GPU start.
    pub resident: bool,
    /// Convert images with colour profiles (and all images on displays
    /// with a non-sRGB profile) to the display's colours.
    pub color_management: bool,
    /// Thumbnails of the neighbours along the bottom.
    pub filmstrip: bool,
    /// The window's last normal (not maximized) rect, left, top, right,
    /// bottom in workspace coordinates (`GetWindowPlacement`).
    pub window: Option<[i32; 4]>,
    /// Whether it was maximized; written only together with `window`.
    pub maximized: bool,
    /// The Print key takes screenshots (in resident mode).
    pub screenshot_hotkey: bool,
    /// Where screenshots go; `None`: the system's screenshot folder.
    pub screenshot_dir: Option<String>,
    /// What happens to a screenshot besides the clipboard, which always
    /// gets it: saved into `screenshot_dir`, opened in the viewer, opened
    /// in the editor.
    pub screenshot_save: bool,
    pub screenshot_open: bool,
    pub screenshot_edit: bool,
    /// The program "Edit with…" starts with the image.
    pub editor: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            view: ViewOptions::default(),
            language: None,
            single_instance: true,
            prefetch_ahead: 4,
            prefetch_behind: 2,
            background: [0x1E, 0x1E, 0x1E],
            auto_update: true,
            resident: false,
            color_management: true,
            filmstrip: false,
            window: None,
            maximized: false,
            screenshot_hotkey: true,
            screenshot_dir: None,
            screenshot_save: true,
            screenshot_open: true,
            screenshot_edit: false,
            editor: None,
        }
    }
}

/// `left,top,right,bottom` with a positive size.
fn parse_rect(v: &str) -> Option<[i32; 4]> {
    let n: Vec<i32> = v
        .split(',')
        .map(|x| x.trim().parse().ok())
        .collect::<Option<_>>()?;
    let r: [i32; 4] = n.try_into().ok()?;
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

fn unquote(v: &str) -> Option<&str> {
    v.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
}

/// A quoted value read from the raw line, which may contain `#`: the text
/// between the first two quotes, followed by nothing or a `# comment`.
fn unquote_raw(v: &str) -> Option<&str> {
    let (value, rest) = v.strip_prefix('"')?.split_once('"')?;
    let rest = rest.trim_start();
    (rest.is_empty() || rest.starts_with('#')).then_some(value)
}

/// A path setting: no line breaks or quotes (which a path cannot hold on
/// Windows and the file could not store), not empty.
fn path_value(v: Option<&str>) -> Option<String> {
    v.filter(|v| !v.is_empty() && !v.contains(['"', '\n', '\r']))
        .map(str::to_owned)
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

impl Settings {
    /// Parses settings; unknown keys and bad values are ignored so an old
    /// or hand-edited file never prevents startup.
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "view_mode" => {
                    if let Some(m) = unquote(value).and_then(ViewMode::from_key) {
                        s.view.mode = m;
                    }
                }
                "no_upscale" => s.view.no_upscale = value.parse().unwrap_or(s.view.no_upscale),
                "language" => {
                    s.language = unquote(value)
                        .filter(|v| !v.is_empty() && *v != "auto")
                        .map(str::to_owned)
                }
                "single_instance" => s.single_instance = value.parse().unwrap_or(s.single_instance),
                "prefetch_ahead" => {
                    s.prefetch_ahead = value.parse::<u32>().unwrap_or(s.prefetch_ahead).min(32)
                }
                "prefetch_behind" => {
                    s.prefetch_behind = value.parse::<u32>().unwrap_or(s.prefetch_behind).min(32)
                }
                "auto_update" => s.auto_update = value.parse().unwrap_or(s.auto_update),
                "resident" => s.resident = value.parse().unwrap_or(s.resident),
                "color_management" => {
                    s.color_management = value.parse().unwrap_or(s.color_management)
                }
                "filmstrip" => s.filmstrip = value.parse().unwrap_or(s.filmstrip),
                "window" => s.window = parse_rect(value).or(s.window),
                "maximized" => s.maximized = value.parse().unwrap_or(s.maximized),
                "screenshot_hotkey" => {
                    s.screenshot_hotkey = value.parse().unwrap_or(s.screenshot_hotkey)
                }
                "screenshot_save" => s.screenshot_save = value.parse().unwrap_or(s.screenshot_save),
                "screenshot_open" => s.screenshot_open = value.parse().unwrap_or(s.screenshot_open),
                "screenshot_edit" => s.screenshot_edit = value.parse().unwrap_or(s.screenshot_edit),
                _ => {}
            }
        }
        // Colours and paths may contain '#': read them from the raw line.
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unquote_raw(value.trim());
            match key.trim() {
                "background" => {
                    if let Some(c) = value.and_then(parse_color) {
                        s.background = c;
                    }
                }
                "screenshot_dir" => s.screenshot_dir = path_value(value),
                "editor" => s.editor = path_value(value),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let [r, g, b] = self.background;
        // Only the Windows shell knows the window's placement: no rect, no
        // placement keys.
        let mut optional = match self.window {
            Some([l, t, r, b]) => {
                format!("window = {l},{t},{r},{b}\nmaximized = {}\n", self.maximized)
            }
            None => String::new(),
        };
        if let Some(d) = &self.screenshot_dir {
            optional += &format!("screenshot_dir = \"{d}\"\n");
        }
        if let Some(e) = &self.editor {
            optional += &format!("editor = \"{e}\"\n");
        }
        format!(
            "# Blinzel settings\n\
             view_mode = \"{}\"\n\
             no_upscale = {}\n\
             language = \"{}\"\n\
             single_instance = {}\n\
             prefetch_ahead = {}\n\
             prefetch_behind = {}\n\
             background = \"#{r:02X}{g:02X}{b:02X}\"\n\
             auto_update = {}\n\
             resident = {}\n\
             color_management = {}\n\
             filmstrip = {}\n\
             screenshot_hotkey = {}\n\
             screenshot_save = {}\n\
             screenshot_open = {}\n\
             screenshot_edit = {}\n\
             {optional}",
            self.view.mode.key(),
            self.view.no_upscale,
            self.language.as_deref().unwrap_or("auto"),
            self.single_instance,
            self.prefetch_ahead,
            self.prefetch_behind,
            self.auto_update,
            self.resident,
            self.color_management,
            self.filmstrip,
            self.screenshot_hotkey,
            self.screenshot_save,
            self.screenshot_open,
            self.screenshot_edit,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Settings {
            view: ViewOptions {
                mode: ViewMode::ActualSize,
                no_upscale: false,
            },
            language: Some("de".into()),
            single_instance: false,
            prefetch_ahead: 6,
            prefetch_behind: 1,
            background: [0x12, 0xAB, 0xFF],
            auto_update: false,
            resident: true,
            color_management: false,
            filmstrip: true,
            window: Some([-1900, 30, 200, 1000]),
            maximized: true,
            screenshot_hotkey: false,
            screenshot_dir: Some(r"D:\Bilder #2\Screens".into()),
            screenshot_save: false,
            screenshot_open: false,
            screenshot_edit: true,
            editor: Some(r"C:\Program Files\paint.net\paintdotnet.exe".into()),
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        // Broken rects are ignored.
        for bad in [
            "1,2,3",
            "1,2,3,4,5",
            "a,b,c,d",
            "10,10,5,20",
            "10,10,10,20",
            "10,20,30,10",
            "10,10,20,10",
        ] {
            assert_eq!(Settings::parse(&format!("window = {bad}")).window, None);
        }
        assert_eq!(
            Settings::parse(&Settings::default().to_text()),
            Settings::default()
        );
    }

    #[test]
    fn tolerates_garbage() {
        let s = Settings::parse(
            "view_mode = \"nope\"\nno_upscale = maybe\n[section]\nprefetch_ahead = 1000\n= =\n",
        );
        assert_eq!(s.view, ViewOptions::default());
        assert_eq!(s.prefetch_ahead, 32);
        assert_eq!(
            Settings::parse("prefetch_behind = 1000").prefetch_behind,
            32
        );
        assert_eq!(Settings::parse("language = \"auto\"").language, None);
    }

    #[test]
    fn a_missing_file_means_the_documented_defaults() {
        let s = Settings::parse("");
        assert_eq!(s, Settings::default());
        assert_eq!(s.view.mode, ViewMode::FitImageToWindow);
        assert_eq!(s.view.mode.key(), "fit-image-to-window");
        assert!(s.view.no_upscale);
        assert_eq!(s.language, None, "auto");
        assert!(s.single_instance);
        assert_eq!((s.prefetch_ahead, s.prefetch_behind), (4, 2));
        assert_eq!(s.background, [0x1E, 0x1E, 0x1E]);
        assert!(s.auto_update && !s.resident && s.color_management);
        assert!(!s.filmstrip && !s.maximized);
        assert!(s.screenshot_hotkey && s.screenshot_save && s.screenshot_open);
        assert!(!s.screenshot_edit);
        assert_eq!(
            (s.window, &s.screenshot_dir, &s.editor),
            (None, &None, &None)
        );
        let text = s.to_text();
        for line in [
            "view_mode = \"fit-image-to-window\"",
            "no_upscale = true",
            "language = \"auto\"",
            "background = \"#1E1E1E\"",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} in\n{text}");
        }
        for key in ["window", "screenshot_dir", "editor"] {
            assert!(
                !text.lines().any(|l| l.starts_with(key)),
                "{key} in\n{text}"
            );
        }
    }

    #[test]
    fn unknown_keys_and_invalid_lines_are_ignored_and_valid_lines_apply() {
        let s = Settings::parse(
            "# Blinzel settings\n\
             [viewer]\n\
             screenshot_clipboard = true\n\
             ram_budget_mb = 2048\n\
             some_future_key = \"x\"\n\
             just some words\n\
             filmstrip = true # show the strip\n\
             no_upscale = 3\n\
             prefetch_ahead = \"six\"\n\
             resident = yes\n\
             language = de\n\
             prefetch_behind = 5\n\
             view_mode = \"actual-size\" # key 3\n\
             background = \"#102030\" # dark blue\n\
             auto_update = false\n",
        );
        let expected = Settings {
            view: ViewOptions {
                mode: ViewMode::ActualSize,
                no_upscale: true,
            },
            filmstrip: true,
            prefetch_behind: 5,
            background: [0x10, 0x20, 0x30],
            auto_update: false,
            ..Settings::default()
        };
        assert_eq!(s, expected);
    }

    #[test]
    fn auto_and_empty_language_both_mean_the_system_language() {
        assert_eq!(Settings::parse("language = \"auto\"").language, None);
        assert_eq!(Settings::parse("language = \"\"").language, None);
        let de = Settings::parse("language = \"de\"");
        assert_eq!(de.language.as_deref(), Some("de"));
        // The system language is written as "auto".
        let text = Settings::parse("language = \"\"").to_text();
        assert!(text.lines().any(|l| l == "language = \"auto\""), "{text}");
    }

    #[test]
    fn invalid_background_and_paths_keep_their_defaults() {
        for bad in [
            "\"#12345\"",
            "\"#1234567\"",
            "\"123456\"",
            "\"#GGGGGG\"",
            "\"#+12345\"",
            "#123456",
            "\"\"",
        ] {
            let s = Settings::parse(&format!("background = {bad}"));
            assert_eq!(s.background, [0x1E, 0x1E, 0x1E], "background = {bad}");
        }
        assert_eq!(
            Settings::parse("background = \"#1e2f3a\"").background,
            [0x1E, 0x2F, 0x3A]
        );
        for key in ["screenshot_dir", "editor"] {
            for bad in ["\"\"", r"C:\Bilder", r#""C:\a"b""#, r#""C:\a" "b""#] {
                let s = Settings::parse(&format!("{key} = {bad}"));
                assert_eq!(
                    (&s.screenshot_dir, &s.editor),
                    (&None, &None),
                    "{key} = {bad}"
                );
            }
            let s = Settings::parse(&format!(r#"{key} = "D:\Bilder #2\a b""#));
            let v = if key == "editor" {
                s.editor
            } else {
                s.screenshot_dir
            };
            assert_eq!(v.as_deref(), Some(r"D:\Bilder #2\a b"));
        }
    }

    #[test]
    fn the_window_placement_is_written_only_once_a_rect_is_known() {
        let text = Settings::default().to_text();
        for key in ["window", "maximized"] {
            assert!(
                !text.lines().any(|l| l.starts_with(key)),
                "{key} in\n{text}"
            );
        }
        let placed = Settings {
            window: Some([-1900, 30, 200, 1000]),
            maximized: true,
            ..Settings::default()
        };
        let text = placed.to_text();
        assert!(
            text.lines().any(|l| l == "window = -1900,30,200,1000"),
            "{text}"
        );
        assert!(text.lines().any(|l| l == "maximized = true"), "{text}");
        assert_eq!(Settings::parse(&text), placed);
    }

    #[test]
    fn ram_budget_mb_is_neither_read_nor_written() {
        let text = Settings::default().to_text();
        assert!(!text.contains("ram_budget_mb"), "{text}");
        // An old file with it loads like one without it.
        let old = Settings::parse("ram_budget_mb = 2048\nfilmstrip = true\n");
        assert_eq!(old, Settings::parse("filmstrip = true\n"));
        assert!(!old.to_text().contains("ram_budget_mb"));
    }

    #[test]
    fn old_screenshot_clipboard_key_is_ignored() {
        // The clipboard used to be optional; now it always gets the shot.
        let s = Settings::parse("screenshot_clipboard = true\nscreenshot_open = false\n");
        assert!(s.screenshot_save && !s.screenshot_open && !s.screenshot_edit);
    }
}
