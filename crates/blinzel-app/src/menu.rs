//! The hamburger menu: a native popup menu (instant, keyboard accessible,
//! localized), dark themed through uxtheme's app mode.

use blinzel_core::layout::ViewMode;
use blinzel_i18n::{Lang, Msg, tr};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetSystemMetrics, HMENU, MENU_ITEM_FLAGS,
    MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SM_MENUDROPALIGNMENT,
    SetForegroundWindow, SetMenuDefaultItem, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD,
    TPM_RIGHTALIGN, TPM_RIGHTBUTTON, TPM_TOPALIGN, TrackPopupMenuEx, WM_NULL,
};
use windows::core::{PCSTR, PCWSTR, w};

use crate::platform::wide;

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
    ToggleResident,
    ToggleAutoUpdate,
    ToggleSingleInstance,
    ToggleColorManagement,
    /// Tray: show the (hidden) window.
    ShowWindow,
    /// Tray: exit for good, also in resident mode.
    Quit,
    Exit,
}

const LANG_BASE: u32 = 1000;

impl Cmd {
    fn id(self) -> u32 {
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
            Cmd::Language(None) => LANG_BASE,
            Cmd::Language(Some(l)) => LANG_BASE + 1 + l as u32,
        }
    }

    fn from_id(id: u32) -> Option<Cmd> {
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

const ALL: [Cmd; 30] = [
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
];

/// State the menu reflects.
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
}

fn accel(keys: &[&str]) -> String {
    keys.join("+")
}

struct Builder {
    menu: HMENU,
}

impl Builder {
    fn new() -> Option<Builder> {
        unsafe { CreatePopupMenu() }
            .ok()
            .map(|menu| Builder { menu })
    }

    fn item(&self, cmd: Cmd, label: &str, shortcut: Option<String>, checked: bool, enabled: bool) {
        let text = match shortcut {
            Some(s) => format!("{label}\t{s}"),
            None => label.to_string(),
        };
        let mut flags = MF_STRING;
        if checked {
            flags |= MF_CHECKED;
        }
        if !enabled {
            flags |= MF_GRAYED;
        }
        let w = wide(&text);
        unsafe {
            let _ = AppendMenuW(self.menu, flags, cmd.id() as usize, PCWSTR(w.as_ptr()));
        }
    }

    fn separator(&self) {
        unsafe {
            let _ = AppendMenuW(self.menu, MF_SEPARATOR, 0, PCWSTR::null());
        }
    }

    fn submenu(&self, label: &str, sub: Builder) {
        let w = wide(label);
        unsafe {
            let _ = AppendMenuW(
                self.menu,
                MF_POPUP | MENU_ITEM_FLAGS(0),
                sub.menu.0 as usize,
                PCWSTR(w.as_ptr()),
            );
        }
    }
}

fn build(s: &MenuState) -> Option<HMENU> {
    let ctrl = tr(Msg::KeyCtrl);
    let m = Builder::new()?;
    m.item(
        Cmd::Open,
        tr(Msg::MenuOpen),
        Some(accel(&[ctrl, "O"])),
        false,
        true,
    );
    m.separator();

    let view = Builder::new()?;
    view.item(
        Cmd::View(ViewMode::FitWindowToImage),
        tr(Msg::ViewFitWindowToImage),
        Some("1".into()),
        s.mode == ViewMode::FitWindowToImage,
        true,
    );
    view.item(
        Cmd::View(ViewMode::FitImageToWindow),
        tr(Msg::ViewFitImageToWindow),
        Some("2".into()),
        s.mode == ViewMode::FitImageToWindow,
        true,
    );
    view.item(
        Cmd::View(ViewMode::ActualSize),
        tr(Msg::ViewActualSize),
        Some("3".into()),
        s.mode == ViewMode::ActualSize,
        true,
    );
    view.separator();
    view.item(
        Cmd::ToggleNoUpscale,
        tr(Msg::ViewNoUpscale),
        None,
        s.no_upscale,
        s.mode == ViewMode::FitImageToWindow,
    );
    m.submenu(tr(Msg::MenuView), view);

    let zoom = Builder::new()?;
    zoom.item(
        Cmd::ZoomIn,
        tr(Msg::ZoomIn),
        Some("+".into()),
        false,
        s.has_image,
    );
    zoom.item(
        Cmd::ZoomOut,
        tr(Msg::ZoomOut),
        Some("-".into()),
        false,
        s.has_image,
    );
    zoom.item(
        Cmd::ZoomReset,
        tr(Msg::ZoomReset),
        Some("0".into()),
        false,
        s.has_image,
    );
    m.submenu(tr(Msg::MenuZoom), zoom);

    let rotate = Builder::new()?;
    rotate.item(
        Cmd::RotateRight,
        tr(Msg::RotateRight),
        Some("R".into()),
        false,
        s.has_image,
    );
    rotate.item(
        Cmd::RotateLeft,
        tr(Msg::RotateLeft),
        Some("L".into()),
        false,
        s.has_image,
    );
    m.submenu(tr(Msg::MenuRotate), rotate);
    m.item(
        Cmd::Fullscreen,
        tr(Msg::Fullscreen),
        Some("F11".into()),
        s.fullscreen,
        true,
    );
    m.separator();

    m.item(
        Cmd::Copy,
        tr(Msg::Copy),
        Some(accel(&[ctrl, "C"])),
        false,
        s.has_image,
    );
    m.item(
        Cmd::Cut,
        tr(Msg::Cut),
        Some(accel(&[ctrl, "X"])),
        false,
        s.has_selection,
    );
    m.item(
        Cmd::SelectAll,
        tr(Msg::SelectAll),
        Some(accel(&[ctrl, "A"])),
        false,
        s.has_image,
    );
    m.item(
        Cmd::ShowOriginal,
        tr(Msg::ShowOriginal),
        Some(tr(Msg::KeyEsc).into()),
        false,
        s.is_cropped,
    );
    m.item(
        Cmd::SaveCutout,
        tr(Msg::SaveCutout),
        Some(accel(&[ctrl, "S"])),
        false,
        s.has_selection || s.is_cropped,
    );
    m.separator();

    m.item(
        Cmd::Next,
        tr(Msg::NavNext),
        Some(tr(Msg::KeyRight).into()),
        false,
        s.has_neighbours,
    );
    m.item(
        Cmd::Prev,
        tr(Msg::NavPrev),
        Some(tr(Msg::KeyLeft).into()),
        false,
        s.has_neighbours,
    );
    m.item(
        Cmd::First,
        tr(Msg::NavFirst),
        Some(tr(Msg::KeyHome).into()),
        false,
        s.has_neighbours,
    );
    m.item(
        Cmd::Last,
        tr(Msg::NavLast),
        Some(tr(Msg::KeyEnd).into()),
        false,
        s.has_neighbours,
    );
    m.item(
        Cmd::Recycle,
        tr(Msg::Recycle),
        Some(tr(Msg::KeyDel).into()),
        false,
        s.has_image,
    );
    m.separator();

    let lang = Builder::new()?;
    lang.item(
        Cmd::Language(None),
        tr(Msg::LanguageAuto),
        None,
        s.language.is_none(),
        true,
    );
    for l in Lang::ALL {
        lang.item(
            Cmd::Language(Some(l)),
            l.native_name(),
            None,
            s.language == Some(l),
            true,
        );
    }
    m.submenu(tr(Msg::MenuLanguage), lang);
    let settings = Builder::new()?;
    settings.item(
        Cmd::ToggleResident,
        tr(Msg::SettingResident),
        None,
        s.resident,
        true,
    );
    settings.item(
        Cmd::ToggleSingleInstance,
        tr(Msg::SettingSingleInstance),
        None,
        s.single_instance,
        true,
    );
    settings.item(
        Cmd::ToggleColorManagement,
        tr(Msg::SettingColorManagement),
        None,
        s.color_management,
        true,
    );
    settings.item(
        Cmd::ToggleAutoUpdate,
        tr(Msg::SettingAutoUpdate),
        None,
        s.auto_update,
        true,
    );
    m.submenu(tr(Msg::MenuSettings), settings);
    m.separator();
    m.item(
        Cmd::CheckUpdates,
        tr(Msg::MenuCheckUpdates),
        None,
        false,
        true,
    );
    m.item(Cmd::About, tr(Msg::MenuAbout), None, false, true);
    m.separator();
    m.item(Cmd::Exit, tr(Msg::Exit), Some("Alt+F4".into()), false, true);
    Some(m.menu)
}

/// Shows the menu at screen position `(x, y)` and returns the choice.
/// Runs a modal loop: call without holding the app state borrowed.
pub fn show(hwnd: HWND, x: i32, y: i32, state: &MenuState) -> Option<Cmd> {
    enable_dark_menus(hwnd);
    let menu = build(state)?;
    let id = unsafe {
        TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0,
            x,
            y,
            hwnd,
            None,
        )
    };
    unsafe {
        let _ = DestroyMenu(menu);
    }
    Cmd::from_id(id.0 as u32)
}

/// The tray icon's menu at screen position `(x, y)`.
pub fn show_tray(hwnd: HWND, x: i32, y: i32) -> Option<Cmd> {
    enable_dark_menus(hwnd);
    let m = Builder::new()?;
    m.item(Cmd::ShowWindow, tr(Msg::TrayShow), None, false, true);
    m.item(Cmd::Open, tr(Msg::MenuOpen), None, false, true);
    m.separator();
    m.item(Cmd::Quit, tr(Msg::Exit), None, false, true);
    let horizontal = if unsafe { GetSystemMetrics(SM_MENUDROPALIGNMENT) } != 0 {
        TPM_RIGHTALIGN
    } else {
        TPM_LEFTALIGN
    };
    let id = unsafe {
        let _ = SetMenuDefaultItem(m.menu, Cmd::ShowWindow.id(), 0);
        // Otherwise the menu stays open when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenuEx(
            m.menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | horizontal).0,
            x,
            y,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m.menu);
        id
    };
    Cmd::from_id(id.0 as u32)
}

/// Undocumented but stable since Windows 10 1903 (used by Explorer,
/// Notepad++, …): uxtheme ordinal 135 `SetPreferredAppMode(ForceDark)`,
/// 133 `AllowDarkModeForWindow`, 136 `FlushMenuThemes`.
fn enable_dark_menus(hwnd: HWND) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else {
            return;
        };
        let ord = |n: usize| GetProcAddress(ux, PCSTR(n as *const u8));
        if let Some(f) = ord(135) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(2);
        }
        if let Some(f) = ord(133) {
            let allow: unsafe extern "system" fn(HWND, i32) -> i32 = std::mem::transmute(f);
            allow(hwnd, 1);
        }
        if let Some(f) = ord(136) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    });
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
