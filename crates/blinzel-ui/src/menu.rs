//! The menu model: commands, the state menus reflect, and the menu tree
//! each platform shows natively (a popup on Windows, the menu bar on
//! macOS) or draws itself (Linux).

use blinzel_core::layout::ViewMode;
use blinzel_i18n::{Lang, Msg, tr};

/// Menu and keyboard commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Open,
    View(ViewMode),
    ToggleNoUpscale,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    RotateRight,
    RotateLeft,
    Fullscreen,
    Copy,
    Cut,
    SelectAll,
    ShowOriginal,
    SaveCutout,
    Next,
    Prev,
    First,
    Last,
    Recycle,
    Language(Option<Lang>),
    CheckUpdates,
    About,
    Notices,
    ToggleResident,
    ToggleAutoUpdate,
    ToggleSingleInstance,
    ToggleColorManagement,
    ToggleFilmstrip,
    ToggleGrid,
    /// Tray: show the (hidden) window.
    ShowWindow,
    /// Tray: exit for good, also in resident mode.
    Quit,
    Exit,
}

const LANG_BASE: u32 = 1000;

impl Cmd {
    /// A stable number for native menus (Windows command ids, macOS tags).
    pub fn id(self) -> u32 {
        match self {
            Cmd::Open => 1,
            Cmd::View(ViewMode::FitWindowToImage) => 2,
            Cmd::View(ViewMode::FitImageToWindow) => 3,
            Cmd::View(ViewMode::ActualSize) => 4,
            Cmd::ToggleNoUpscale => 5,
            Cmd::ZoomIn => 6,
            Cmd::ZoomOut => 7,
            Cmd::ZoomReset => 8,
            Cmd::RotateRight => 9,
            Cmd::RotateLeft => 10,
            Cmd::Fullscreen => 11,
            Cmd::Copy => 12,
            Cmd::Cut => 13,
            Cmd::SelectAll => 14,
            Cmd::ShowOriginal => 15,
            Cmd::Next => 16,
            Cmd::Prev => 17,
            Cmd::First => 18,
            Cmd::Last => 19,
            Cmd::Recycle => 20,
            Cmd::Exit => 21,
            Cmd::SaveCutout => 22,
            Cmd::CheckUpdates => 23,
            Cmd::About => 24,
            Cmd::ToggleResident => 25,
            Cmd::ToggleAutoUpdate => 26,
            Cmd::ToggleSingleInstance => 27,
            Cmd::ShowWindow => 28,
            Cmd::Quit => 29,
            Cmd::ToggleColorManagement => 30,
            Cmd::ToggleFilmstrip => 31,
            Cmd::ToggleGrid => 32,
            Cmd::Notices => 33,
            Cmd::Language(None) => LANG_BASE,
            Cmd::Language(Some(l)) => LANG_BASE + 1 + l as u32,
        }
    }

    pub fn from_id(id: u32) -> Option<Cmd> {
        if id >= LANG_BASE {
            let i = id - LANG_BASE;
            return if i == 0 {
                Some(Cmd::Language(None))
            } else {
                Lang::ALL
                    .get(i as usize - 1)
                    .map(|l| Cmd::Language(Some(*l)))
            };
        }
        ALL.iter().copied().find(|c| c.id() == id)
    }
}

pub const ALL: [Cmd; 33] = [
    Cmd::Open,
    Cmd::View(ViewMode::FitWindowToImage),
    Cmd::View(ViewMode::FitImageToWindow),
    Cmd::View(ViewMode::ActualSize),
    Cmd::ToggleNoUpscale,
    Cmd::ZoomIn,
    Cmd::ZoomOut,
    Cmd::ZoomReset,
    Cmd::RotateRight,
    Cmd::RotateLeft,
    Cmd::Fullscreen,
    Cmd::Copy,
    Cmd::Cut,
    Cmd::SelectAll,
    Cmd::ShowOriginal,
    Cmd::Next,
    Cmd::Prev,
    Cmd::First,
    Cmd::Last,
    Cmd::Recycle,
    Cmd::Exit,
    Cmd::SaveCutout,
    Cmd::CheckUpdates,
    Cmd::About,
    Cmd::ToggleResident,
    Cmd::ToggleAutoUpdate,
    Cmd::ToggleSingleInstance,
    Cmd::ShowWindow,
    Cmd::Quit,
    Cmd::ToggleColorManagement,
    Cmd::ToggleFilmstrip,
    Cmd::ToggleGrid,
    Cmd::Notices,
];

/// State the menu reflects.
#[derive(Clone, Debug)]
pub struct MenuState {
    pub mode: ViewMode,
    pub no_upscale: bool,
    pub fullscreen: bool,
    pub has_image: bool,
    pub has_selection: bool,
    pub is_cropped: bool,
    pub has_neighbours: bool,
    pub language: Option<Lang>,
    pub resident: bool,
    pub auto_update: bool,
    pub single_instance: bool,
    pub color_management: bool,
    pub filmstrip: bool,
    pub grid: bool,
}

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Item {
        cmd: Cmd,
        label: String,
        /// Shown on the right, e.g. "Ctrl+O".
        shortcut: Option<String>,
        checked: bool,
        enabled: bool,
    },
    Separator,
    Sub(String, Vec<MenuItem>),
}

impl MenuItem {
    fn item(
        cmd: Cmd,
        label: &str,
        shortcut: Option<String>,
        checked: bool,
        enabled: bool,
    ) -> MenuItem {
        MenuItem::Item {
            cmd,
            label: label.to_string(),
            shortcut,
            checked,
            enabled,
        }
    }
}

fn accel(keys: &[&str]) -> String {
    keys.join("+")
}

/// The hamburger menu (the menu bar on macOS) for `s`.
pub fn items(s: &MenuState) -> Vec<MenuItem> {
    let ctrl = tr(Msg::KeyCtrl);
    let mut m = Vec::new();
    m.push(MenuItem::item(
        Cmd::Open,
        tr(Msg::MenuOpen),
        Some(accel(&[ctrl, "O"])),
        false,
        true,
    ));
    m.push(MenuItem::Separator);

    let view = vec![
        MenuItem::item(
            Cmd::View(ViewMode::FitWindowToImage),
            tr(Msg::ViewFitWindowToImage),
            Some("1".into()),
            s.mode == ViewMode::FitWindowToImage,
            true,
        ),
        MenuItem::item(
            Cmd::View(ViewMode::FitImageToWindow),
            tr(Msg::ViewFitImageToWindow),
            Some("2".into()),
            s.mode == ViewMode::FitImageToWindow,
            true,
        ),
        MenuItem::item(
            Cmd::View(ViewMode::ActualSize),
            tr(Msg::ViewActualSize),
            Some("3".into()),
            s.mode == ViewMode::ActualSize,
            true,
        ),
        MenuItem::Separator,
        MenuItem::item(
            Cmd::ToggleNoUpscale,
            tr(Msg::ViewNoUpscale),
            None,
            s.no_upscale,
            s.mode == ViewMode::FitImageToWindow,
        ),
        MenuItem::Separator,
        MenuItem::item(
            Cmd::ToggleFilmstrip,
            tr(Msg::ViewFilmstrip),
            Some("T".into()),
            s.filmstrip,
            true,
        ),
        MenuItem::item(
            Cmd::ToggleGrid,
            tr(Msg::ViewGrid),
            Some("G".into()),
            s.grid,
            true,
        ),
    ];
    m.push(MenuItem::Sub(tr(Msg::MenuView).into(), view));

    let zoom = vec![
        MenuItem::item(
            Cmd::ZoomIn,
            tr(Msg::ZoomIn),
            Some("+".into()),
            false,
            s.has_image,
        ),
        MenuItem::item(
            Cmd::ZoomOut,
            tr(Msg::ZoomOut),
            Some("-".into()),
            false,
            s.has_image,
        ),
        MenuItem::item(
            Cmd::ZoomReset,
            tr(Msg::ZoomReset),
            Some("0".into()),
            false,
            s.has_image,
        ),
    ];
    m.push(MenuItem::Sub(tr(Msg::MenuZoom).into(), zoom));

    let rotate = vec![
        MenuItem::item(
            Cmd::RotateRight,
            tr(Msg::RotateRight),
            Some("R".into()),
            false,
            s.has_image,
        ),
        MenuItem::item(
            Cmd::RotateLeft,
            tr(Msg::RotateLeft),
            Some("L".into()),
            false,
            s.has_image,
        ),
    ];
    m.push(MenuItem::Sub(tr(Msg::MenuRotate).into(), rotate));
    m.push(MenuItem::item(
        Cmd::Fullscreen,
        tr(Msg::Fullscreen),
        Some("F11".into()),
        s.fullscreen,
        true,
    ));
    m.push(MenuItem::Separator);

    m.push(MenuItem::item(
        Cmd::Copy,
        tr(Msg::Copy),
        Some(accel(&[ctrl, "C"])),
        false,
        s.has_image,
    ));
    m.push(MenuItem::item(
        Cmd::Cut,
        tr(Msg::Cut),
        Some(accel(&[ctrl, "X"])),
        false,
        s.has_selection,
    ));
    m.push(MenuItem::item(
        Cmd::SelectAll,
        tr(Msg::SelectAll),
        Some(accel(&[ctrl, "A"])),
        false,
        s.has_image,
    ));
    m.push(MenuItem::item(
        Cmd::ShowOriginal,
        tr(Msg::ShowOriginal),
        Some(tr(Msg::KeyEsc).into()),
        false,
        s.is_cropped,
    ));
    m.push(MenuItem::item(
        Cmd::SaveCutout,
        tr(Msg::SaveCutout),
        Some(accel(&[ctrl, "S"])),
        false,
        s.has_selection || s.is_cropped,
    ));
    m.push(MenuItem::Separator);

    m.push(MenuItem::item(
        Cmd::Next,
        tr(Msg::NavNext),
        Some(tr(Msg::KeyRight).into()),
        false,
        s.has_neighbours,
    ));
    m.push(MenuItem::item(
        Cmd::Prev,
        tr(Msg::NavPrev),
        Some(tr(Msg::KeyLeft).into()),
        false,
        s.has_neighbours,
    ));
    m.push(MenuItem::item(
        Cmd::First,
        tr(Msg::NavFirst),
        Some(tr(Msg::KeyHome).into()),
        false,
        s.has_neighbours,
    ));
    m.push(MenuItem::item(
        Cmd::Last,
        tr(Msg::NavLast),
        Some(tr(Msg::KeyEnd).into()),
        false,
        s.has_neighbours,
    ));
    m.push(MenuItem::item(
        Cmd::Recycle,
        tr(Msg::Recycle),
        Some(tr(Msg::KeyDel).into()),
        false,
        s.has_image,
    ));
    m.push(MenuItem::Separator);

    let mut lang = Vec::new();
    lang.push(MenuItem::item(
        Cmd::Language(None),
        tr(Msg::LanguageAuto),
        None,
        s.language.is_none(),
        true,
    ));
    for l in Lang::ALL {
        lang.push(MenuItem::item(
            Cmd::Language(Some(l)),
            l.native_name(),
            None,
            s.language == Some(l),
            true,
        ));
    }
    m.push(MenuItem::Sub(tr(Msg::MenuLanguage).into(), lang));
    let settings = vec![
        MenuItem::item(
            Cmd::ToggleResident,
            tr(Msg::SettingResident),
            None,
            s.resident,
            true,
        ),
        MenuItem::item(
            Cmd::ToggleSingleInstance,
            tr(Msg::SettingSingleInstance),
            None,
            s.single_instance,
            true,
        ),
        MenuItem::item(
            Cmd::ToggleColorManagement,
            tr(Msg::SettingColorManagement),
            None,
            s.color_management,
            true,
        ),
        MenuItem::item(
            Cmd::ToggleAutoUpdate,
            tr(Msg::SettingAutoUpdate),
            None,
            s.auto_update,
            true,
        ),
    ];
    m.push(MenuItem::Sub(tr(Msg::MenuSettings).into(), settings));
    m.push(MenuItem::Separator);
    m.push(MenuItem::item(
        Cmd::CheckUpdates,
        tr(Msg::MenuCheckUpdates),
        None,
        false,
        true,
    ));
    m.push(MenuItem::item(
        Cmd::About,
        tr(Msg::MenuAbout),
        None,
        false,
        true,
    ));
    m.push(MenuItem::item(
        Cmd::Notices,
        tr(Msg::MenuNotices),
        None,
        false,
        true,
    ));
    m.push(MenuItem::Separator);
    m.push(MenuItem::item(
        Cmd::Exit,
        tr(Msg::Exit),
        Some("Alt+F4".into()),
        false,
        true,
    ));
    m
}

/// The tray icon's menu (resident mode).
pub fn tray_items() -> Vec<MenuItem> {
    vec![
        MenuItem::item(Cmd::ShowWindow, tr(Msg::TrayShow), None, false, true),
        MenuItem::item(Cmd::Open, tr(Msg::MenuOpen), None, false, true),
        MenuItem::Separator,
        MenuItem::item(Cmd::Quit, tr(Msg::Exit), None, false, true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_roundtrip() {
        for c in ALL {
            assert_eq!(Cmd::from_id(c.id()), Some(c));
        }
        for l in Lang::ALL {
            assert_eq!(
                Cmd::from_id(Cmd::Language(Some(l)).id()),
                Some(Cmd::Language(Some(l)))
            );
        }
        assert_eq!(
            Cmd::from_id(Cmd::Language(None).id()),
            Some(Cmd::Language(None))
        );
        assert_eq!(Cmd::from_id(0), None);
    }
}
