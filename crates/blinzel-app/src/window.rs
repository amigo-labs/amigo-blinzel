//! Window class, window procedure and the frameless frame.
//!
//! The caption is removed in `WM_NCCALCSIZE` (left, right and bottom keep
//! their invisible resize borders, so DWM still draws the shadow and Aero
//! snap works). The top resize edge, the drag strip and the caption buttons
//! come from `WM_NCHITTEST`; reporting `HTMAXBUTTON` gives Windows 11 snap
//! layouts on hover.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{ScreenToClient, ValidateRect};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
    TrackMouseEvent, VK_CONTROL, VK_F10, VK_SHIFT,
};
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use blinzel_core::settings::Settings;
use blinzel_ui::app::Action;
use blinzel_ui::backend::{CursorKind, Key};
use blinzel_ui::chrome::{Button, Chrome};
use blinzel_ui::settings_store;

use crate::backend::{
    App, TIMER_ANIM, WM_BATCH, WM_COLOR_TARGET, WM_DIR_CHANGED, WM_LIST, WM_LOADED, WM_THUMB,
    monitor_rect,
};
use crate::instance::Request;
use crate::resident::{self, WM_TRAY};
use crate::{clipboard, instance, menu, platform, screenshot};

thread_local! {
    pub static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

const TIMER_UNCLOAK: usize = 1;
const TIMER_NOTICE: usize = 2;
const TIMER_HOTKEY: usize = 3;
/// Attempts left to get the Print key after it was taken at start.
static HOTKEY_TRIES: AtomicU32 = AtomicU32::new(0);
/// A message for the notice toast; LPARAM is a `Box<String>`.
pub const WM_NOTICE: u32 = WM_APP + 4;

/// Runs `f` on the app state unless it is already borrowed (re-entrant
/// message); `None` then.
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

fn lo_word(v: isize) -> i32 {
    (v & 0xFFFF) as i16 as i32
}

fn hi_word(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as i16 as i32
}

fn point_from_lparam(lp: LPARAM) -> (f64, f64) {
    (lo_word(lp.0) as f64, hi_word(lp.0) as f64)
}

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

/// Invisible resize border thickness (x, y) at `dpi`.
fn frame_borders(dpi: u32) -> (i32, i32) {
    unsafe {
        let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        (
            GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad,
            GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad,
        )
    }
}

fn has_thick_frame(hwnd: HWND) -> bool {
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    style & WS_THICKFRAME.0 != 0
}

fn is_maximized(hwnd: HWND) -> bool {
    unsafe { IsZoomed(hwnd).as_bool() }
}

/// Removes the caption, keeps the side and bottom resize borders.
fn nc_calc_size(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if wp.0 == 0 || !has_thick_frame(hwnd) {
        return LRESULT(0);
    }
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
    let params = unsafe { &mut *(lp.0 as *mut NCCALCSIZE_PARAMS) };
    params.rgrc[0] = client_area(params.rgrc[0], frame_borders(dpi), is_maximized(hwnd));
    LRESULT(0)
}

/// The client area of the frameless `window`: no caption, the invisible
/// side and bottom resize borders `(bx, by)` stay frame. A maximized
/// window hangs over the monitor edge by the border on all four sides, so
/// it loses the top one too.
fn client_area(window: RECT, (bx, by): (i32, i32), maximized: bool) -> RECT {
    RECT {
        left: window.left + bx,
        top: if maximized {
            window.top + by
        } else {
            window.top
        },
        right: window.right - bx,
        bottom: window.bottom - by,
    }
}

fn nc_hit_test(hwnd: HWND, app: &App, lp: LPARAM) -> u32 {
    let def = unsafe { DefWindowProcW(hwnd, WM_NCHITTEST, WPARAM(0), lp) }.0 as u32;
    let mut pt = POINT {
        x: lo_word(lp.0),
        y: hi_word(lp.0),
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut pt);
    }
    let frame = Frame {
        borders: frame_borders(app.dpi),
        maximized: is_maximized(hwnd),
        fullscreen: app.fullscreen,
        dragging: app.is_dragging(),
    };
    hit_zone(def, pt, &frame, &app.chrome())
}

/// The window state a hit test depends on.
struct Frame {
    /// Invisible resize border thickness (x, y).
    borders: (i32, i32),
    maximized: bool,
    fullscreen: bool,
    /// A selection drag runs.
    dragging: bool,
}

/// What the point `pt` (client coordinates) of the frameless window is:
/// `def` is `DefWindowProcW`'s answer (the side and bottom borders), the
/// top border and the caption strip with its buttons come from here.
fn hit_zone(def: u32, pt: POINT, frame: &Frame, chrome: &Chrome) -> u32 {
    if def != HTCLIENT || frame.fullscreen {
        return def;
    }
    if !frame.maximized {
        let (bx, by) = frame.borders;
        if pt.y < by {
            return if pt.x < bx * 2 {
                HTTOPLEFT
            } else if pt.x >= chrome.client.0 as i32 - bx * 2 {
                HTTOPRIGHT
            } else {
                HTTOP
            };
        }
    }
    let p = (pt.x as f64, pt.y as f64);
    if p.1 < chrome.caption_height() {
        return match chrome.hit(p) {
            Some(Button::Close) => HTCLOSE,
            Some(Button::Maximize) => HTMAXBUTTON,
            Some(Button::Minimize) => HTMINBUTTON,
            Some(Button::Menu) => HTCLIENT,
            // Free caption strip: drag the window, but not while a
            // selection drag runs.
            None if !frame.dragging => HTCAPTION,
            None => HTCLIENT,
        };
    }
    HTCLIENT
}

fn nc_button(hit: u32) -> Option<Button> {
    match hit {
        HTCLOSE => Some(Button::Close),
        HTMAXBUTTON => Some(Button::Maximize),
        HTMINBUTTON => Some(Button::Minimize),
        _ => None,
    }
}

enum Outcome {
    Result(LRESULT),
    Default,
}

pub extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Stateless handling that must work before the app exists.
    match msg {
        WM_NCCALCSIZE => return nc_calc_size(hwnd, wp, lp),
        WM_ERASEBKGND => return LRESULT(1),
        _ => {}
    }
    // Before Direct3D is up, the first image comes through GDI.
    let no_app = APP.with(|c| c.try_borrow().is_ok_and(|a| a.is_none()));
    if no_app && let Some(r) = crate::first_frame::on_message(hwnd, msg, lp) {
        return r;
    }
    if msg == crate::first_frame::WM_FIRST_FRAME {
        // Too late: Direct3D shows the image. Free the message's payload.
        drop(unsafe { Box::from_raw(lp.0 as *mut crate::first_frame::FirstFrame) });
        return LRESULT(0);
    }
    let handled = with_app(|app| {
        let mut actions = Vec::new();
        let outcome = handle(app, hwnd, msg, wp, lp, &mut actions);
        (outcome, actions)
    });
    let result = match handled {
        Some((Outcome::Result(r), actions)) => {
            run_actions(hwnd, actions);
            return r;
        }
        Some((Outcome::Default, actions)) => {
            run_actions(hwnd, actions);
            None
        }
        None => None,
    };
    result.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

fn handle(
    app: &mut App,
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    actions: &mut Vec<Action>,
) -> Outcome {
    let done = Outcome::Result(LRESULT(0));
    match msg {
        WM_PAINT => {
            actions.extend(app.render());
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            done
        }
        WM_SIZE => {
            if wp.0 as u32 != SIZE_MINIMIZED {
                app.maximized = wp.0 as u32 == SIZE_MAXIMIZED;
                app.resized(((lp.0 & 0xFFFF) as u32, ((lp.0 >> 16) & 0xFFFF) as u32));
                // Draw right away: during a live resize WM_PAINT comes late.
                actions.extend(app.render());
            }
            done
        }
        WM_EXITSIZEMOVE | WM_DISPLAYCHANGE => {
            app.update_screen_box();
            app.check_monitor(msg == WM_DISPLAYCHANGE);
            Outcome::Default
        }
        WM_DPICHANGED => {
            app.dpi = (wp.0 & 0xFFFF) as u32;
            let r = unsafe { &*(lp.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            // The WM_SIZE sent by SetWindowPos arrived re-entrantly and was
            // not seen by us: pick the new size up here.
            let mut cr = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut cr);
            }
            app.resized(((cr.right - cr.left) as u32, (cr.bottom - cr.top) as u32));
            app.update_screen_box();
            app.invalidate();
            done
        }
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
            let s = app.scale();
            mmi.ptMinTrackSize = POINT {
                x: (320.0 * s) as i32,
                y: (220.0 * s) as i32,
            };
            done
        }
        WM_NCHITTEST => Outcome::Result(LRESULT(nc_hit_test(hwnd, app, lp) as isize)),
        WM_NCMOUSEMOVE => {
            let b = nc_button(wp.0 as u32);
            if b != app.hover {
                app.hover = b;
                app.invalidate();
            }
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE | TME_NONCLIENT,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            // DefWindowProc still sees it: Windows 11 snap layouts hang off
            // the HTMAXBUTTON hover.
            Outcome::Default
        }
        WM_NCMOUSELEAVE | WM_MOUSELEAVE => {
            if app.hover.is_some() || app.pressed.is_some() {
                app.hover = None;
                app.pressed = None;
                app.invalidate();
            }
            Outcome::Default
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => match nc_button(wp.0 as u32) {
            Some(b) => {
                app.pressed = Some(b);
                app.invalidate();
                done
            }
            None => Outcome::Default,
        },
        WM_NCLBUTTONUP => match nc_button(wp.0 as u32) {
            Some(b) => {
                if app.pressed == Some(b) {
                    actions.push(match b {
                        Button::Close => Action::Close,
                        Button::Maximize => Action::ToggleMaximize,
                        Button::Minimize => Action::Minimize,
                        Button::Menu => unreachable!(),
                    });
                }
                app.pressed = None;
                app.hover = None;
                app.invalidate();
                done
            }
            None => Outcome::Default,
        },
        WM_LBUTTONDOWN | WM_MBUTTONDOWN => {
            let shift = (wp.0 & 0x0004) != 0;
            let ctrl = key_down(VK_CONTROL);
            actions.extend(app.mouse_down(
                point_from_lparam(lp),
                shift,
                ctrl,
                msg == WM_MBUTTONDOWN,
            ));
            if app.is_dragging() {
                unsafe { SetCapture(hwnd) };
            }
            done
        }
        WM_MOUSEMOVE => {
            app.mouse_move(point_from_lparam(lp));
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            done
        }
        WM_LBUTTONUP | WM_MBUTTONUP => {
            app.mouse_up();
            unsafe {
                let _ = ReleaseCapture();
            }
            done
        }
        WM_LBUTTONDBLCLK => {
            let p = point_from_lparam(lp);
            // The free caption strip is `HTCAPTION`: Windows maximizes.
            if let Some(a) = app.double_click(p) {
                actions.extend(a);
            }
            done
        }
        WM_RBUTTONUP => {
            actions.extend(app.right_click(point_from_lparam(lp)));
            done
        }
        WM_CONTEXTMENU => {
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            actions.push(Action::ShowMenu { x: pt.x, y: pt.y });
            done
        }
        WM_XBUTTONUP => {
            let which = hi_word(wp.0 as isize);
            actions.extend(app.command(if which == 1 {
                blinzel_ui::menu::Cmd::Prev
            } else {
                blinzel_ui::menu::Cmd::Next
            }));
            Outcome::Result(LRESULT(1))
        }
        WM_MOUSEWHEEL => {
            let mut pt = POINT {
                x: lo_word(lp.0),
                y: hi_word(lp.0),
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            actions.extend(app.wheel((pt.x as f64, pt.y as f64), hi_word(wp.0 as isize) as i16));
            done
        }
        WM_SETCURSOR => {
            if (lp.0 & 0xFFFF) as u32 != HTCLIENT {
                return Outcome::Default;
            }
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let kind = app.cursor_at((pt.x as f64, pt.y as f64), key_down(VK_SHIFT));
            set_cursor(kind);
            Outcome::Result(LRESULT(1))
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u16;
            if msg == WM_SYSKEYDOWN && vk != VK_F10.0 {
                return Outcome::Default; // Alt+F4 and friends
            }
            let handled =
                key_of(vk).and_then(|k| app.key(k, key_down(VK_CONTROL), key_down(VK_SHIFT)));
            match handled {
                Some(a) => {
                    actions.extend(a);
                    done
                }
                None => Outcome::Default,
            }
        }
        WM_TIMER if wp.0 == TIMER_UNCLOAK => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_UNCLOAK);
            }
            if app.cloaked && !app.hidden {
                actions.push(Action::Uncloak);
            }
            done
        }
        WM_TIMER if wp.0 == TIMER_ANIM => {
            app.on_anim_timer();
            done
        }
        WM_TIMER if wp.0 == TIMER_HOTKEY => {
            let wanted = app.settings.resident && app.settings.screenshot_hotkey;
            let left = HOTKEY_TRIES
                .fetch_sub(1, Ordering::Relaxed)
                .saturating_sub(1);
            if !wanted || screenshot::set_hotkey(hwnd, true) || left == 0 {
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_HOTKEY);
                }
                if wanted && left == 0 {
                    match app.tell(screenshot::hotkey_taken()) {
                        // Now in the tray, and again in the window: a
                        // notification may be switched off.
                        Some(text) => {
                            resident::balloon(hwnd, &text);
                            screenshot::hint_pending(true);
                        }
                        None => unsafe {
                            SetTimer(Some(hwnd), TIMER_NOTICE, 9000, None);
                        },
                    }
                }
            }
            done
        }
        WM_TIMER if wp.0 == TIMER_NOTICE => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_NOTICE);
            }
            app.notice = None;
            app.invalidate();
            done
        }
        WM_NOTICE => {
            let text = unsafe { Box::from_raw(lp.0 as *mut String) };
            app.show_notice(*text);
            unsafe {
                SetTimer(Some(hwnd), TIMER_NOTICE, 3500, None);
            }
            done
        }
        WM_LOADED => {
            actions.extend(app.on_loaded());
            done
        }
        WM_LIST => {
            actions.extend(app.on_list());
            done
        }
        WM_DIR_CHANGED => {
            app.on_dir_changed();
            done
        }
        WM_THUMB => {
            app.on_thumbs();
            done
        }
        WM_BATCH => {
            app.on_batch();
            done
        }
        // Characters for the batch dialog's text fields (keys go through
        // WM_KEYDOWN).
        WM_CHAR if app.dialog.is_some() => {
            if let Some(c) = char::from_u32(wp.0 as u32) {
                app.text_input(c.encode_utf8(&mut [0; 4]));
            }
            done
        }
        WM_COLOR_TARGET => {
            let b =
                unsafe { Box::from_raw(lp.0 as *mut (isize, Option<blinzel_core::icc::Profile>)) };
            let (monitor, target) = *b;
            app.on_color_target(monitor, target);
            done
        }
        WM_COPYDATA => {
            let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
            match instance::received(cds) {
                Some(Request::Open {
                    path,
                    sender_created,
                }) => {
                    // Marks of this hand-off count from the second launch's
                    // process creation, like a cold start's.
                    let age = sender_created.map_or(0, |c| (platform::now_filetime() - c) / 10);
                    blinzel_core::timing::init(age);
                    blinzel_core::timing::mark("handoff_received");
                    actions.extend(app.open(path));
                    actions.push(Action::Wake { opened: true });
                    Outcome::Result(LRESULT(1))
                }
                Some(Request::Show) => {
                    actions.push(Action::Wake { opened: false });
                    Outcome::Result(LRESULT(1))
                }
                Some(Request::Quit) => {
                    actions.push(Action::Exit);
                    Outcome::Result(LRESULT(1))
                }
                None => done,
            }
        }
        WM_TRAY => {
            actions.extend(resident::on_tray(wp, lp));
            done
        }
        WM_HOTKEY if screenshot::is_hotkey(wp) => {
            actions.push(Action::Capture {
                whole_screen: false,
            });
            done
        }
        screenshot::WM_SCREENSHOT => {
            let result = unsafe { Box::from_raw(lp.0 as *mut Result<screenshot::Taken, String>) };
            let text = match *result {
                Ok(t) => {
                    // Emptying the clipboard drops an earlier copy's
                    // delayed formats; WM_DESTROYCLIPBOARD cannot reach the
                    // borrowed app here.
                    app.clip = None;
                    let copied = clipboard::put_shot(hwnd, &t.bgra, t.width, t.height, &t.png);
                    match t.file {
                        Ok(file) => {
                            actions.extend(app.on_screenshot(file));
                            match (copied, app.settings.screenshot_open) {
                                (false, _) => Some(screenshot::failure("clipboard")),
                                (true, false) => Some(screenshot::copied()),
                                (true, true) => None,
                            }
                        }
                        Err(e) => Some(screenshot::failure(&e)),
                    }
                }
                Err(e) => Some(screenshot::failure(&e)),
            };
            if let Some(text) = text {
                match app.tell(text) {
                    Some(text) => resident::balloon(hwnd, &text),
                    None => unsafe {
                        SetTimer(Some(hwnd), TIMER_NOTICE, 3500, None);
                    },
                }
            }
            done
        }
        WM_DROPFILES => {
            let drop = HDROP(wp.0 as *mut _);
            let mut buf = [0u16; 32768];
            let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
            let paths: Vec<PathBuf> = (0..count)
                .filter_map(|i| {
                    let n = unsafe { DragQueryFileW(drop, i, Some(&mut buf)) } as usize;
                    (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
                })
                .collect();
            unsafe { DragFinish(drop) };
            actions.extend(app.files_dropped(paths));
            done
        }
        WM_RENDERFORMAT | WM_RENDERALLFORMATS | WM_DESTROYCLIPBOARD => {
            clipboard::on_message(hwnd, msg, wp, &mut app.clip);
            done
        }
        WM_CLOSE => {
            actions.push(Action::Close);
            done
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            done
        }
        WM_WINDOWPOSCHANGED => {
            // Another monitor may want other colours.
            app.check_monitor(false);
            Outcome::Default
        }
        _ if msg == resident::taskbar_created() => {
            resident::on_taskbar_created(hwnd);
            done
        }
        _ => Outcome::Default,
    }
}

/// The UI's key for a virtual-key code.
pub(crate) fn key_of(vk: u16) -> Option<Key> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    Some(match VIRTUAL_KEY(vk) {
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_SPACE => Key::Space,
        VK_BACK => Key::Backspace,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_TAB => Key::Tab,
        VK_DELETE => Key::Delete,
        VK_F10 => Key::F10,
        VK_F11 => Key::F11,
        VK_APPS => Key::Menu,
        VK_ADD | VK_OEM_PLUS => Key::Plus,
        VK_SUBTRACT | VK_OEM_MINUS => Key::Minus,
        _ if (0x30..=0x39).contains(&vk) => Key::Digit((vk - 0x30) as u8),
        _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) => Key::Digit((vk - VK_NUMPAD0.0) as u8),
        _ if (0x41..=0x5A).contains(&vk) => Key::Letter(vk as u8 as char),
        _ => return None,
    })
}

fn set_cursor(kind: CursorKind) {
    let id = match kind {
        CursorKind::Arrow => IDC_ARROW,
        CursorKind::Cross => IDC_CROSS,
        CursorKind::Hand => IDC_HAND,
        CursorKind::Move => IDC_SIZEALL,
        CursorKind::SizeNS => IDC_SIZENS,
        CursorKind::SizeWE => IDC_SIZEWE,
        CursorKind::SizeNWSE => IDC_SIZENWSE,
        CursorKind::SizeNESW => IDC_SIZENESW,
    };
    unsafe {
        if let Ok(c) = LoadCursorW(None, id) {
            SetCursor(Some(c));
        }
    }
}

/// Executes deferred actions outside the state borrow; follow-up actions
/// from the app are processed in the same loop.
pub fn run_actions(hwnd: HWND, mut actions: Vec<Action>) {
    actions.reverse();
    while let Some(a) = actions.pop() {
        let more = run_action(hwnd, a);
        for m in more.into_iter().rev() {
            actions.push(m);
        }
    }
}

fn run_action(hwnd: HWND, action: Action) -> Vec<Action> {
    match action {
        Action::ShowMenu { x, y } => {
            let Some(state) = with_app(|a| a.menu_state()) else {
                return vec![];
            };
            match menu::show(hwnd, x, y, &state) {
                Some(cmd) => with_app(|a| a.command(cmd)).unwrap_or_default(),
                None => vec![],
            }
        }
        Action::FitWindow { client, grow_only } => {
            fit_window(hwnd, client, grow_only);
            vec![]
        }
        Action::ToggleFullscreen => {
            toggle_fullscreen(hwnd);
            vec![]
        }
        Action::ToggleMaximize => {
            unsafe {
                let _ = ShowWindow(
                    hwnd,
                    if is_maximized(hwnd) {
                        SW_RESTORE
                    } else {
                        SW_MAXIMIZE
                    },
                );
            }
            vec![]
        }
        Action::Minimize => {
            unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            }
            vec![]
        }
        Action::OpenDialog => match crate::dialogs::open_file(hwnd) {
            // From the tray the window may be hidden.
            Some(p) => with_app(|a| a.open_chosen(p)).unwrap_or_default(),
            None => vec![],
        },
        Action::Recycle(path) => {
            if crate::dialogs::recycle(hwnd, &path) {
                with_app(|a| a.removed(&path)).unwrap_or_default()
            } else {
                vec![]
            }
        }
        Action::SaveCutout(job) => {
            let folder = job.source.parent().map(PathBuf::from).unwrap_or_default();
            if let Some(target) =
                crate::dialogs::save_file(hwnd, &folder, &job.default_name, job.prefer_png)
            {
                let raw = hwnd.0 as isize;
                if let Some(services) = with_app(|a| a.services.clone()) {
                    let _ =
                        blinzel_ui::app::save_in_background(services, job, target, move |text| {
                            let ptr = Box::into_raw(Box::new(text)) as isize;
                            unsafe {
                                if PostMessageW(
                                    Some(HWND(raw as *mut _)),
                                    WM_NOTICE,
                                    WPARAM(0),
                                    LPARAM(ptr),
                                )
                                .is_err()
                                {
                                    drop(Box::from_raw(ptr as *mut String));
                                }
                            }
                        });
                }
            }
            vec![]
        }
        Action::CheckUpdates => {
            crate::update::check_now(hwnd, WM_NOTICE);
            vec![]
        }
        Action::About => {
            use blinzel_i18n::{Msg, tr, trf};
            let text = crate::platform::wide(trf(
                Msg::AboutBody,
                &[
                    ("version", &crate::setup::version()),
                    ("url", &crate::setup::install::URL),
                ],
            ));
            let caption = crate::platform::wide(tr(Msg::MenuAbout));
            unsafe {
                MessageBoxW(
                    Some(hwnd),
                    windows::core::PCWSTR(text.as_ptr()),
                    windows::core::PCWSTR(caption.as_ptr()),
                    MB_ICONINFORMATION | MB_OK | platform::message_box_direction(),
                );
            }
            vec![]
        }
        Action::Notices => {
            if let Err(e) = crate::notices::show(hwnd) {
                platform::debug_log(&format!("notices: {e}"));
            }
            vec![]
        }
        Action::PickFolder => {
            if let Some(dir) = crate::dialogs::pick_folder(hwnd) {
                with_app(|a| a.batch_folder_picked(dir));
            }
            vec![]
        }
        Action::OpenFolder(dir) => {
            crate::dialogs::open_folder(hwnd, &dir);
            vec![]
        }
        Action::SetResident(on) => {
            resident::set_resident(hwnd, on, resident::set_autostart);
            vec![]
        }
        Action::TrayMenu { x, y } => match menu::show_tray(hwnd, x, y) {
            Some(cmd) => {
                use blinzel_ui::menu::Cmd;
                if matches!(cmd, Cmd::Capture | Cmd::CaptureScreen) {
                    screenshot::let_menu_fade();
                }
                with_app(|a| a.command(cmd)).unwrap_or_default()
            }
            None => vec![],
        },
        Action::Capture { whole_screen } => {
            screenshot::start(hwnd, whole_screen);
            vec![]
        }
        Action::SetScreenshotHotkey(on) => {
            // Replaces the attempts from the start; a failure shows now.
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_HOTKEY);
            }
            screenshot::hint_pending(false);
            if !screenshot::set_hotkey(hwnd, on) {
                with_app(|a| a.show_notice(screenshot::hotkey_taken()));
                unsafe {
                    SetTimer(Some(hwnd), TIMER_NOTICE, 9000, None);
                }
            }
            vec![]
        }
        Action::PickScreenshotFolder => {
            if let Some(dir) = crate::dialogs::pick_folder(hwnd) {
                with_app(|a| a.screenshot_folder_picked(dir));
            }
            vec![]
        }
        Action::OpenWith(file) => {
            screenshot::open_with(hwnd, &file);
            vec![]
        }
        Action::EditWith { editor, file } => {
            screenshot::edit_with(hwnd, &editor, &file);
            vec![]
        }
        Action::ChooseEditor { then } => match crate::dialogs::open_program(hwnd) {
            Some(editor) => with_app(|a| a.editor_chosen(editor, then)).unwrap_or_default(),
            None => vec![],
        },
        Action::Wake { opened } => wake(hwnd, opened),
        Action::Close => {
            let state = with_app(|a| (a.settings.resident, a.hidden));
            let hide = state.is_some_and(|(resident, hidden)| {
                close_hides(resident, hidden, || instance::hidden(Some(hwnd)).is_some())
            });
            if hide {
                hide_window(hwnd);
            } else {
                exit(hwnd);
            }
            vec![]
        }
        Action::Exit => {
            exit(hwnd);
            vec![]
        }
        Action::Activate => {
            unsafe {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(hwnd);
            }
            vec![]
        }
        Action::Uncloak => {
            uncloak(hwnd);
            let woke = with_app(|a| {
                a.cloaked = false;
                std::mem::take(&mut a.activate_on_uncloak).then(|| a.wake_report.clone())
            })
            .flatten();
            if let Some(report) = woke {
                unsafe {
                    let _ = SetForegroundWindow(hwnd);
                }
                if let Some(path) = report {
                    let json = format!("{{\"marks\": {}}}", blinzel_core::timing::marks_json());
                    let _ = std::fs::write(path, json);
                }
            }
            vec![]
        }
        Action::Quit => {
            unsafe { PostQuitMessage(0) };
            vec![]
        }
    }
}

/// The window's normal rect (workspace coordinates; before full screen
/// if it is there) and whether it is maximized.
fn placement(hwnd: HWND) -> Option<([i32; 4], bool)> {
    let wp = SAVED_PLACEMENT.with(|p| *p.borrow()).or_else(|| {
        let mut wp = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(hwnd, &mut wp) }
            .ok()
            .map(|_| wp)
    })?;
    let r = wp.rcNormalPosition;
    let maximized = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32 || is_maximized(hwnd);
    Some(([r.left, r.top, r.right, r.bottom], maximized))
}

/// Puts the window where it was last time (if a monitor is still there);
/// true if it was maximized. Call before it is first shown.
pub fn restore_placement(hwnd: HWND, saved: Option<[i32; 4]>, maximized: bool) -> bool {
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect};
    let Some([left, top, right, bottom]) = saved else {
        return false;
    };
    let rect = RECT {
        left,
        top,
        right,
        bottom,
    };
    if unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) }.is_invalid() {
        return false;
    }
    let wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        showCmd: SW_HIDE.0 as u32,
        rcNormalPosition: rect,
        ..Default::default()
    };
    unsafe { SetWindowPlacement(hwnd, &wp) }.is_ok() && maximized
}

/// Remembers where the window is, for the next start: its normal (not
/// maximized) rect and whether it is maximized.
pub fn remember_placement(hwnd: HWND, settings: &mut Settings) {
    if let Some((rect, maximized)) = placement(hwnd) {
        settings.window = Some(rect);
        settings.maximized = maximized;
    }
}

/// Saves the settings, keeps a copied image on the clipboard, removes the
/// tray icon and closes the window (which ends the message loop).
fn exit(hwnd: HWND) {
    let clip = with_app(|a| {
        // A hidden window's placement was remembered when it hid.
        if !a.hidden {
            remember_placement(hwnd, &mut a.settings);
        }
        a.save_settings_on_exit();
        a.clip.take()
    })
    .flatten();
    if let Some(src) = clip {
        clipboard::render_all(hwnd, &src);
    }
    resident::remove_tray(hwnd);
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// Whether closing the window hides it instead of ending the process
/// (`docs/specs/resident-mode.md` AC-03, AC-04, AC-11): in resident mode,
/// unless it is hidden already (someone else closed it, e.g. taskkill) or
/// `other_hidden` finds another hidden instance (several windows with
/// "Open images in the same window" off): one is enough.
fn close_hides(resident: bool, hidden: bool, other_hidden: impl FnOnce() -> bool) -> bool {
    resident && !hidden && !other_hidden()
}

/// Resident mode: the window goes away, the process stays warm. An
/// installed update takes over now instead.
fn hide_window(hwnd: HWND) {
    if with_app(|a| a.fullscreen) == Some(true) {
        toggle_fullscreen(hwnd);
    }
    with_app(|a| {
        remember_placement(hwnd, &mut a.settings);
        settings_store::save(&a.settings);
    });
    set_cloak(hwnd, true);
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_UNCLOAK);
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    with_app(|a| a.hide());
    if crate::update::restart_pending() {
        RESTART.with(|r| r.set(true));
        exit(hwnd);
    }
}

thread_local! {
    /// Start the updated executable once the message loop has ended.
    static RESTART: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the hidden instance exited for an installed update.
pub fn restart_requested() -> bool {
    RESTART.with(|r| r.get())
}

/// Brings a hidden resident window back: shown cloaked, uncloaked with the
/// first frame of the file just opened (or right away with the hint), then
/// activated. A visible window is only activated.
fn wake(hwnd: HWND, opened: bool) -> Vec<Action> {
    let was_hidden = with_app(|a| a.wake(opened)).unwrap_or(false);
    if screenshot::take_hint() {
        with_app(|a| a.show_notice(screenshot::hotkey_taken()));
        unsafe {
            SetTimer(Some(hwnd), TIMER_NOTICE, 9000, None);
        }
    }
    if !was_hidden {
        return vec![Action::Activate];
    }
    unsafe {
        // Painted while cloaked; not activated yet (costs ~20 ms).
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
    if opened {
        start_uncloak_timer(hwnd, UNCLOAK_FALLBACK_MS);
        vec![]
    } else {
        let mut more = with_app(|a| a.render()).unwrap_or_default();
        more.push(Action::Uncloak);
        more
    }
}

/// Windows (the Snipping Tool, or another app while signing in) holds the
/// Print key: tries again for a minute, then leaves a notice for the next
/// wake. Ctrl+Shift+Print works meanwhile.
pub fn retry_hotkey(hwnd: HWND) {
    HOTKEY_TRIES.store(12, Ordering::Relaxed);
    unsafe {
        SetTimer(Some(hwnd), TIMER_HOTKEY, 5000, None);
    }
}

pub fn set_cloak(hwnd: HWND, cloak: bool) {
    let v: i32 = cloak as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, std::ptr::from_ref(&v).cast(), 4);
    }
}

pub(crate) fn uncloak(hwnd: HWND) {
    set_cloak(hwnd, false);
    platform::debug_log("window visible");
    blinzel_core::timing::mark("uncloak");
}

/// How long a cloaked window with a file to show waits for its image
/// before it becomes visible anyway (`docs/specs/startup.md` AC-07).
pub const UNCLOAK_FALLBACK_MS: u32 = 400;

pub fn start_uncloak_timer(hwnd: HWND, ms: u32) {
    unsafe {
        SetTimer(Some(hwnd), TIMER_UNCLOAK, ms, None);
    }
}

/// Window rect for a client rect, given our frame (borders left, right,
/// bottom only).
fn window_rect_for_client(client: RECT, dpi: u32) -> RECT {
    let (bx, by) = frame_borders(dpi);
    RECT {
        left: client.left - bx,
        top: client.top,
        right: client.right + bx,
        bottom: client.bottom + by,
    }
}

/// Sizes the client area for `image` (see [`Action::FitWindow`]).
pub(crate) fn fit_window(hwnd: HWND, image: (u32, u32), grow_only: bool) {
    if is_maximized(hwnd) {
        return;
    }
    let Some(work) = monitor_rect(hwnd, true) else {
        return;
    };
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
    let s = dpi as f64 / 96.0;
    let max = (
        (work.right - work.left) as u32,
        (work.bottom - work.top) as u32,
    );
    let min = ((320.0 * s) as u32, (220.0 * s) as u32);
    let (cw, ch) = if grow_only {
        let mut cr = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut cr);
        }
        let current = ((cr.right - cr.left) as u32, (cr.bottom - cr.top) as u32);
        match blinzel_core::layout::grown_client_size(image, current, max, min) {
            Some(size) => size,
            None => return,
        }
    } else {
        blinzel_core::layout::window_client_size(image, max, min)
    };
    // Keep the current centre, stay inside the work area.
    let mut wr = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut wr);
    }
    let (cx, cy) = ((wr.left + wr.right) / 2, (wr.top + wr.bottom) / 2);
    let left = (cx - cw as i32 / 2).clamp(work.left, (work.right - cw as i32).max(work.left));
    let top = (cy - ch as i32 / 2).clamp(work.top, (work.bottom - ch as i32).max(work.top));
    let r = window_rect_for_client(
        RECT {
            left,
            top,
            right: left + cw as i32,
            bottom: top + ch as i32,
        },
        dpi,
    );
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

thread_local! {
    static SAVED_PLACEMENT: RefCell<Option<WINDOWPLACEMENT>> = const { RefCell::new(None) };
}

fn toggle_fullscreen(hwnd: HWND) {
    let entering = SAVED_PLACEMENT.with(|p| p.borrow().is_none());
    unsafe {
        if entering {
            let mut wp = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            let _ = GetWindowPlacement(hwnd, &mut wp);
            SAVED_PLACEMENT.with(|p| *p.borrow_mut() = Some(wp));
            let Some(m) = monitor_rect(hwnd, false) else {
                return;
            };
            with_app(|a| a.fullscreen = true);
            SetWindowLongPtrW(hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                m.left,
                m.top,
                m.right - m.left,
                m.bottom - m.top,
                SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
            );
        } else {
            let wp = SAVED_PLACEMENT.with(|p| p.borrow_mut().take()).unwrap();
            with_app(|a| a.fullscreen = false);
            SetWindowLongPtrW(
                hwnd,
                GWL_STYLE,
                (WS_OVERLAPPEDWINDOW | WS_VISIBLE).0 as isize,
            );
            let _ = SetWindowPlacement(hwnd, &wp);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER,
            );
        }
    }
    with_app(|a| a.invalidate());
}

/// Registers the window class (once).
pub fn register_class(
    instance: windows::Win32::Foundation::HINSTANCE,
) -> windows::core::Result<()> {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        hIcon: resident::app_icon(false),
        hIconSm: resident::app_icon(true),
        // No background brush: nothing is painted before our first frame.
        lpszClassName: instance::WINDOW_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::core::w;

    use crate::backend::{D3dCanvas, D3dDevice, Loader, WinHost, WinServices};
    use crate::gfx::{Gfx, Renderer, SwapChain};

    /// An 800 × 600 window at 100 % (with 8 px resize borders below); the
    /// caption buttons are 46 × 32: close from x 754, maximize from 708,
    /// minimize from 662, ☰ up to 46.
    fn chrome() -> Chrome {
        Chrome {
            scale: 1.0,
            client: (800.0, 600.0),
            rtl: false,
            system_buttons: false,
        }
    }

    fn frame(maximized: bool, fullscreen: bool, dragging: bool) -> Frame {
        Frame {
            borders: (8, 8),
            maximized,
            fullscreen,
            dragging,
        }
    }

    fn hit(f: &Frame, def: u32, x: i32, y: i32) -> u32 {
        hit_zone(def, POINT { x, y }, f, &chrome())
    }

    /// AC-03 (`docs/specs/platform-shells.md`): the plain window has no
    /// caption, keeps the system's side and bottom borders, resizes at the
    /// top border (diagonally in its outer corners) and moves on the free
    /// caption strip, except while a selection drag runs.
    #[test]
    fn the_plain_window_resizes_at_the_top_and_moves_on_the_free_strip() {
        let window = RECT {
            left: 100,
            top: 50,
            right: 900,
            bottom: 650,
        };
        let client = RECT {
            left: 108,
            top: 50,
            right: 892,
            bottom: 642,
        };
        assert_eq!(client_area(window, (8, 8), false), client);

        let f = frame(false, false, false);
        for def in [HTLEFT, HTRIGHT, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT] {
            assert_eq!(hit(&f, def, 400, 300), def, "the system's border");
        }
        assert_eq!(hit(&f, HTCLIENT, 400, 0), HTTOP);
        assert_eq!(hit(&f, HTCLIENT, 400, 7), HTTOP);
        assert_eq!(hit(&f, HTCLIENT, 0, 3), HTTOPLEFT);
        assert_eq!(hit(&f, HTCLIENT, 15, 3), HTTOPLEFT);
        assert_eq!(hit(&f, HTCLIENT, 16, 3), HTTOP);
        assert_eq!(hit(&f, HTCLIENT, 784, 3), HTTOPRIGHT);
        assert_eq!(hit(&f, HTCLIENT, 799, 3), HTTOPRIGHT);
        assert_eq!(hit(&f, HTCLIENT, 400, 8), HTCAPTION);
        assert_eq!(hit(&f, HTCLIENT, 400, 31), HTCAPTION);
        assert_eq!(hit(&f, HTCLIENT, 400, 32), HTCLIENT, "below the strip");
        assert_eq!(hit(&f, HTCLIENT, 20, 16), HTCLIENT, "☰");
        assert_eq!(hit(&f, HTCLIENT, 680, 16), HTMINBUTTON);
        assert_eq!(hit(&f, HTCLIENT, 730, 16), HTMAXBUTTON);
        assert_eq!(hit(&f, HTCLIENT, 780, 16), HTCLOSE);

        let dragging = frame(false, false, true);
        assert_eq!(hit(&dragging, HTCLIENT, 400, 16), HTCLIENT);
        assert_eq!(hit(&dragging, HTCLIENT, 780, 16), HTCLOSE);
    }

    /// AC-04: maximized, the content stays on the monitor and the top edge
    /// does not resize; in full screen nothing is a caption.
    #[test]
    fn maximized_stays_on_the_monitor_and_full_screen_has_no_caption() {
        // A maximized window on a 1920 × 1080 monitor hangs 8 px over
        // each edge.
        let window = RECT {
            left: -8,
            top: -8,
            right: 1928,
            bottom: 1088,
        };
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(client_area(window, (8, 8), true), monitor);

        let max = frame(true, false, false);
        assert_eq!(hit(&max, HTCLIENT, 400, 0), HTCAPTION);
        assert_eq!(hit(&max, HTCLIENT, 100, 3), HTCAPTION);
        assert_eq!(hit(&max, HTCLIENT, 790, 0), HTCLOSE, "the corner closes");
        assert_eq!(hit(&max, HTCLIENT, 20, 0), HTCLIENT, "☰");

        let full = frame(false, true, false);
        for (x, y) in [(400, 0), (400, 16), (5, 3), (680, 16), (730, 16), (780, 16)] {
            assert_eq!(hit(&full, HTCLIENT, x, y), HTCLIENT, "({x}, {y})");
        }
    }

    extern "system" fn plain_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    /// AC-08: in full screen the window covers the monitor, yet what is
    /// remembered for the next start is the rect from before full screen
    /// and whether the window was maximized then.
    #[test]
    fn full_screen_remembers_the_rect_from_before() {
        let instance = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let class = w!("BlinzelFullscreenTest");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(plain_proc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        let hwnd = unsafe {
            RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("test"),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                480,
                360,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .unwrap();
        let remembered = || {
            let mut s = Settings::default();
            remember_placement(hwnd, &mut s);
            (s.window.unwrap(), s.maximized)
        };
        let window_rect = || {
            let mut r = RECT::default();
            unsafe { GetWindowRect(hwnd, &mut r) }.unwrap();
            r
        };
        let (normal, maximized) = remembered();
        assert!(!maximized);

        toggle_fullscreen(hwnd);
        assert_eq!(
            Some(window_rect()),
            monitor_rect(hwnd, false),
            "full screen"
        );
        assert_eq!(remembered(), (normal, false));
        toggle_fullscreen(hwnd);
        assert_eq!(remembered(), (normal, false));

        unsafe {
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        }
        toggle_fullscreen(hwnd);
        assert_eq!(
            Some(window_rect()),
            monitor_rect(hwnd, false),
            "full screen"
        );
        assert_eq!(remembered(), (normal, true));
        toggle_fullscreen(hwnd);
        assert!(is_maximized(hwnd), "maximized again");
        assert_eq!(remembered(), (normal, true));
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    fn cloaked(hwnd: HWND) -> bool {
        let mut c = 0u32;
        // SAFETY: `c` is a live u32, the size passed.
        unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut c).cast(), 4) }.unwrap();
        c != 0
    }

    /// `docs/specs/startup.md` AC-07: with a file to show and the device
    /// and swap chain ready, a window whose image does not come stays
    /// cloaked, and the timer makes it visible after 400 ms. The loader has
    /// no device here, so the decoded image never reaches the screen (a
    /// slow file).
    #[test]
    fn a_window_whose_image_does_not_come_becomes_visible_after_400_ms() {
        let hwnd = cloaked_window(w!("BlinzelUncloakTest"));
        let mut app = warp_app(hwnd);
        app.cloaked = true;
        let actions = app.open(test_image());
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
        run_actions(hwnd, actions);

        start_uncloak_timer(hwnd, UNCLOAK_FALLBACK_MS);
        let after = pump_while_cloaked(hwnd);
        assert!(!cloaked(hwnd), "still invisible after {after:?}");
        assert!(
            after >= Duration::from_millis(350),
            "visible after {after:?} already"
        );
        assert!(unsafe { IsWindowVisible(hwnd) }.as_bool());
        assert_eq!(with_app(|a| a.cloaked), Some(false));
        drop(APP.with(|cell| cell.borrow_mut().take()));
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    /// `docs/specs/resident-mode.md` AC-09 (the shell half): a hidden
    /// window woken with a file is shown without activation and stays
    /// cloaked until its image comes, 400 ms at the latest; woken without
    /// one it is visible at once with the hint. A visible window is only
    /// activated.
    #[test]
    fn a_woken_window_is_shown_cloaked_until_its_image_or_400_ms() {
        let hwnd = cloaked_window(w!("BlinzelWakeTest"));
        let mut app = warp_app(hwnd);
        app.hide();
        let mut actions = app.open(test_image());
        actions.push(Action::Wake { opened: true });
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        run_actions(hwnd, actions);
        assert!(unsafe { IsWindowVisible(hwnd) }.as_bool(), "shown");
        assert!(cloaked(hwnd), "invisible until the first frame");
        assert_eq!(with_app(|a| a.activate_on_uncloak), Some(true));
        let after = pump_while_cloaked(hwnd);
        assert!(!cloaked(hwnd), "still invisible after {after:?}");
        assert!(
            after >= Duration::from_millis(350),
            "visible after {after:?}"
        );
        assert_eq!(
            with_app(|a| a.activate_on_uncloak),
            Some(false),
            "activated"
        );

        // Hidden again, woken without a file: visible with the hint at once.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        set_cloak(hwnd, true);
        with_app(|a| a.hide());
        run_actions(hwnd, vec![Action::Wake { opened: false }]);
        assert!(unsafe { IsWindowVisible(hwnd) }.as_bool());
        assert!(!cloaked(hwnd), "visible at once");
        // Awake already: only activated.
        assert_eq!(wake(hwnd, false), [Action::Activate]);
        drop(APP.with(|cell| cell.borrow_mut().take()));
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    /// `docs/specs/resident-mode.md` AC-03, AC-04, AC-11: closing hides the
    /// window only in resident mode, only while it is visible and only
    /// while no other instance is hidden; else the process ends.
    #[test]
    fn closing_hides_a_resident_window_unless_another_one_is_hidden() {
        assert!(close_hides(true, false, || false));
        assert!(!close_hides(true, false, || true), "another one is hidden");
        assert!(
            !close_hides(false, false, || panic!("no lookup")),
            "not resident"
        );
        assert!(
            !close_hides(true, true, || panic!("no lookup")),
            "closed while hidden"
        );
    }

    /// The test image the loader decodes but cannot show (no device).
    fn test_image() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../blinzel-core/testdata/rgb420-64x48.avif")
    }

    /// Pumps messages while `hwnd` is cloaked, for 2 s at most; returns
    /// how long.
    fn pump_while_cloaked(hwnd: HWND) -> Duration {
        let start = Instant::now();
        let mut msg = MSG::default();
        while cloaked(hwnd) && start.elapsed() < Duration::from_secs(2) {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        start.elapsed()
    }

    /// A cloaked, never shown 480 × 360 window of a new class with our
    /// window procedure.
    fn cloaked_window(class: windows::core::PCWSTR) -> HWND {
        let instance = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        let hwnd = unsafe {
            RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("test"),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                480,
                360,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .unwrap();
        set_cloak(hwnd, true);
        hwnd
    }

    /// An App on a WARP device for `hwnd`, as `run.rs` builds it once
    /// Direct3D is up.
    fn warp_app(hwnd: HWND) -> App {
        let mut cr = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut cr);
        }
        let client = (
            (cr.right - cr.left).max(1) as u32,
            (cr.bottom - cr.top).max(1) as u32,
        );
        let gfx = Gfx::create(true).expect("WARP device");
        let device = Arc::new(D3dDevice::new(&gfx));
        let renderer = Renderer::new(&gfx.device).unwrap();
        let swap = SwapChain::new(&gfx, hwnd, client).unwrap();
        let services = Arc::new(WinServices::default());
        let loader = Loader::new(services.clone(), 1, 2, 2);
        let color = blinzel_ui::color::ColorManager::new(false, crate::color::build_lut);
        App::new(
            WinHost { hwnd },
            device,
            D3dCanvas::new(gfx, renderer, swap),
            services,
            Settings::default(),
            loader,
            color,
            96,
            client,
        )
    }
}
