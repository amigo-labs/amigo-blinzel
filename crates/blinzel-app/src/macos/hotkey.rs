//! The global screenshot key ⌘⇧1 through Carbon's hot keys, which need no
//! accessibility permission (unlike an event tap). Carbon delivers the
//! press on the main thread's event loop.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::null_mut;

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
        let mut hot_key = null_mut();
        if RegisterEventHotKey(KEY_1, CMD_KEY | SHIFT_KEY, id, target, 0, &mut hot_key) != 0 {
            return false;
        }
        HOT_KEY.with(|h| h.set(hot_key));
    }
    true
}
