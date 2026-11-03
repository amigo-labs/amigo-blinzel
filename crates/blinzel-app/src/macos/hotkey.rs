//! The global screenshot key ⌘⇧1 through Carbon's hot keys, which need no
//! accessibility permission (unlike an event tap). Carbon delivers the
//! press on the main thread's event loop.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::null_mut;

use blinzel_core::settings::Settings;
use blinzel_i18n::{Msg, tr};
use blinzel_ui::app::Action;

use super::shell::{UserEvent, post};

type OsStatus = i32;
type EventHandlerProc = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> OsStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetEventDispatcherTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandlerProc,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> OsStatus;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> OsStatus;
}

/// 'keyb'
const K_EVENT_CLASS_KEYBOARD: u32 = 0x6B65_7962;
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
/// kVK_ANSI_1
const KEY_1: u32 = 0x12;
const CMD_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
/// 'blzl'
const SIGNATURE: u32 = 0x626C_7A6C;

thread_local! {
    static HANDLER: Cell<bool> = const { Cell::new(false) };
    static HOT_KEY: Cell<*mut c_void> = const { Cell::new(null_mut()) };
}

/// The key and its modifiers: kVK_ANSI_1 with cmdKey and shiftKey.
fn combo() -> (u32, u32) {
    (KEY_1, CMD_KEY | SHIFT_KEY)
}

/// Whether ⌘⇧1 belongs registered: resident mode and the key setting both
/// on.
pub fn wanted(settings: &Settings) -> bool {
    settings.resident && settings.screenshot_hotkey
}

/// What a press of ⌘⇧1 does, with the window shown or hidden.
pub const ON_PRESS: Action = Action::Capture {
    whole_screen: false,
};

/// The notice after [`set`]: none when it worked, else that another app
/// holds ⌘⇧1 and the menu bar icon still takes screenshots.
pub fn notice(registered: bool) -> Option<&'static str> {
    (!registered).then(|| tr(Msg::HotkeyTakenMac))
}

extern "C" fn pressed(_call: *mut c_void, _event: *mut c_void, _data: *mut c_void) -> OsStatus {
    post(UserEvent::Hotkey);
    0
}

/// Registers (or drops) ⌘⇧1; `false` if another app holds it. Main thread
/// only.
pub fn set(on: bool) -> bool {
    let current = HOT_KEY.with(Cell::get);
    if !on {
        if !current.is_null() {
            // SAFETY: a hot key this module registered.
            unsafe { UnregisterEventHotKey(current) };
            HOT_KEY.with(|h| h.set(null_mut()));
        }
        return true;
    }
    if !current.is_null() {
        return true;
    }
    // SAFETY: Carbon's documented calls with valid pointers; the handler
    // stays installed for the process.
    unsafe {
        let target = GetEventDispatcherTarget();
        if !HANDLER.with(Cell::get) {
            let spec = EventTypeSpec {
                event_class: K_EVENT_CLASS_KEYBOARD,
                event_kind: K_EVENT_HOT_KEY_PRESSED,
            };
            let mut handler = null_mut();
            if InstallEventHandler(target, pressed, 1, &spec, null_mut(), &mut handler) != 0 {
                return false;
            }
            HANDLER.with(|h| h.set(true));
        }
        let id = EventHotKeyId {
            signature: SIGNATURE,
            id: 1,
        };
        let (key, modifiers) = combo();
        let mut hot_key = null_mut();
        if RegisterEventHotKey(key, modifiers, id, target, 0, &mut hot_key) != 0 {
            return false;
        }
        HOT_KEY.with(|h| h.set(hot_key));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_i18n::{Lang, tr_in};

    /// `docs/specs/screenshots.md` AC-04: ⌘⇧1 opens the overlay, shown or
    /// hidden, where resident mode and the key are on; taken by another
    /// app, a notice points to the menu bar icon.
    #[test]
    fn cmd_shift_1_opens_the_overlay_or_says_the_menu_bar_icon_still_works() {
        // kVK_ANSI_1, cmdKey | shiftKey (Carbon's Events.h).
        assert_eq!(combo(), (0x12, 0x100 | 0x200));
        assert_eq!(
            ON_PRESS,
            Action::Capture {
                whole_screen: false
            }
        );
        for (resident, key) in [(true, true), (true, false), (false, true), (false, false)] {
            let s = Settings {
                resident,
                screenshot_hotkey: key,
                ..Settings::default()
            };
            assert_eq!(wanted(&s), resident && key);
        }
        assert_eq!(notice(true), None);
        assert_eq!(notice(false), Some(tr(Msg::HotkeyTakenMac)));
        let en = tr_in(Lang::En, Msg::HotkeyTakenMac);
        assert!(en.contains("⌘⇧1") && en.contains("menu bar icon"), "{en}");
    }
}
