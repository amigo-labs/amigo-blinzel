//! The hamburger menu as a native popup menu (instant, keyboard
//! accessible, localized), dark themed through uxtheme's app mode. What it
//! holds comes from `blinzel_ui::menu`.

use blinzel_ui::menu::{Cmd, MenuItem, MenuState, items, tray_items};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetSystemMetrics, HMENU, MENU_ITEM_FLAGS,
    MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SM_MENUDROPALIGNMENT,
    SetForegroundWindow, SetMenuDefaultItem, TPM_BOTTOMALIGN, TPM_LAYOUTRTL, TPM_LEFTALIGN,
    TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON, TPM_TOPALIGN, TrackPopupMenuEx, WM_NULL,
};
use windows::core::{PCSTR, PCWSTR, w};

use crate::platform::wide;

/// A popup menu for `entries` (destroyed by the caller, with its submenus).
fn build(entries: &[MenuItem]) -> Option<HMENU> {
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    for e in entries {
        match e {
            MenuItem::Item {
                cmd,
                label,
                shortcut,
                checked,
                enabled,
            } => {
                let text = match shortcut {
                    Some(s) => format!("{label}\t{s}"),
                    None => label.clone(),
                };
                let mut flags = MF_STRING;
                if *checked {
                    flags |= MF_CHECKED;
                }
                if !enabled {
                    flags |= MF_GRAYED;
                }
                let w = wide(&text);
                unsafe {
                    let _ = AppendMenuW(menu, flags, cmd.id() as usize, PCWSTR(w.as_ptr()));
                }
            }
            MenuItem::Separator => unsafe {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            },
            MenuItem::Sub(label, sub) => {
                let sub = build(sub)?;
                let w = wide(label);
                unsafe {
                    let _ = AppendMenuW(
                        menu,
                        MF_POPUP | MENU_ITEM_FLAGS(0),
                        sub.0 as usize,
                        PCWSTR(w.as_ptr()),
                    );
                }
            }
        }
    }
    Some(menu)
}

/// Shows the menu at screen position `(x, y)` and returns the choice.
/// Runs a modal loop: call without holding the app state borrowed.
pub fn show(hwnd: HWND, x: i32, y: i32, state: &MenuState) -> Option<Cmd> {
    enable_dark_menus(hwnd);
    let menu = build(&items(state))?;
    // Right-to-left: mirrored items, opening leftwards from (x, y).
    let direction = if blinzel_i18n::is_rtl() {
        TPM_LAYOUTRTL | TPM_RIGHTALIGN
    } else {
        TPM_LEFTALIGN
    };
    let id = unsafe {
        TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | direction | TPM_TOPALIGN).0,
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
    let menu = build(&tray_items())?;
    let horizontal = if unsafe { GetSystemMetrics(SM_MENUDROPALIGNMENT) } != 0 {
        TPM_RIGHTALIGN
    } else {
        TPM_LEFTALIGN
    };
    let rtl = if blinzel_i18n::is_rtl() {
        TPM_LAYOUTRTL
    } else {
        Default::default()
    };
    let id = unsafe {
        let _ = SetMenuDefaultItem(menu, Cmd::ShowWindow.id(), 0);
        // Otherwise the menu stays open when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | horizontal | rtl).0,
            x,
            y,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
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
