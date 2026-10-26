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
    /// Memory budget for decoded images (MiB, 0 = automatic).
    pub ram_budget_mb: u32,
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
    pub maximized: bool,
    /// The Print key takes screenshots (in resident mode).
    pub screenshot_hotkey: bool,
    /// Where screenshots go; `None`: the system's screenshot folder.
    pub screenshot_dir: Option<String>,
    /// Screenshots also go to the clipboard.
    pub screenshot_clipboard: bool,
    /// The program "Edit with…" starts with the image.
    pub editor: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            view: ViewOptions::default(),
            language: None,
            single_instance: true,
            ram_budget_mb: 0,
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
            screenshot_clipboard: false,
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

/// A path setting: no line breaks or quotes (which a path cannot hold on
/// Windows and the file could not store), not empty.
fn path_value(v: Option<&str>) -> Option<String> {
    v.filter(|v| !v.is_empty() && !v.contains(['"', '\n', '\r']))
        .map(str::to_owned)
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
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
                "ram_budget_mb" => s.ram_budget_mb = value.parse().unwrap_or(s.ram_budget_mb),
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
                "screenshot_clipboard" => {
                    s.screenshot_clipboard = value.parse().unwrap_or(s.screenshot_clipboard)
                }
                _ => {}
            }
        }
        // Colours and paths may contain '#': read them from the raw line.
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unquote(value.trim());
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
        let mut optional = match self.window {
            Some([l, t, r, b]) => format!("window = {l},{t},{r},{b}\n"),
            None => String::new(),
        };
        if let Some(d) = &self.screenshot_dir {
            optional += &format!("screenshot_dir = \"{d}\"\n");
        }
        if let Some(e) = &self.editor {
            optional += &format!("editor = \"{e}\"\n");
        }
        format!(
            "# amigo-blinzel settings\n\
             view_mode = \"{}\"\n\
             no_upscale = {}\n\
             language = \"{}\"\n\
             single_instance = {}\n\
             ram_budget_mb = {}\n\
             prefetch_ahead = {}\n\
             prefetch_behind = {}\n\
             background = \"#{r:02X}{g:02X}{b:02X}\"\n\
             auto_update = {}\n\
             resident = {}\n\
             color_management = {}\n\
             filmstrip = {}\n\
             maximized = {}\n\
             screenshot_hotkey = {}\n\
             screenshot_clipboard = {}\n\
             {optional}",
            self.view.mode.key(),
            self.view.no_upscale,
            self.language.as_deref().unwrap_or("auto"),
            self.single_instance,
            self.ram_budget_mb,
            self.prefetch_ahead,
            self.prefetch_behind,
            self.auto_update,
            self.resident,
            self.color_management,
            self.filmstrip,
            self.maximized,
            self.screenshot_hotkey,
            self.screenshot_clipboard,
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
            ram_budget_mb: 2048,
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
            screenshot_clipboard: true,
            editor: Some(r"C:\Program Files\paint.net\paintdotnet.exe".into()),
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        // Broken rects are ignored.
        for bad in ["1,2,3", "10,10,5,20", "a,b,c,d"] {
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
        assert_eq!(Settings::parse("language = \"auto\"").language, None);
    }
}
