//! Window class, window procedure and the frameless frame.
//!
//! The caption is removed in `WM_NCCALCSIZE` (left, right and bottom keep
//! their invisible resize borders, so DWM still draws the shadow and Aero
//! snap works). The top resize edge, the drag strip and the caption buttons
//! come from `WM_NCHITTEST`; reporting `HTMAXBUTTON` gives Windows 11 snap
//! layouts on hover.

use std::cell::RefCell;
use std::path::PathBuf;

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

use blinzel_ui::app::Action;
use blinzel_ui::backend::{CursorKind, Key};
use blinzel_ui::chrome::Button;
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
    let (bx, by) = frame_borders(dpi);
    let params = unsafe { &mut *(lp.0 as *mut NCCALCSIZE_PARAMS) };
    let r = &mut params.rgrc[0];
    r.left += bx;
    r.right -= bx;
    r.bottom -= by;
    if is_maximized(hwnd) {
        // Maximized windows hang over the monitor edge by the border.
        r.top += by;
    }
    LRESULT(0)
}

fn nc_hit_test(hwnd: HWND, app: &App, lp: LPARAM) -> u32 {
    let def = unsafe { DefWindowProcW(hwnd, WM_NCHITTEST, WPARAM(0), lp) }.0 as u32;
    if def != HTCLIENT || app.fullscreen {
        return def;
    }
    let mut pt = POINT {
        x: lo_word(lp.0),
        y: hi_word(lp.0),
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut pt);
    }
    let p = (pt.x as f64, pt.y as f64);
    let maximized = is_maximized(hwnd);
    if !maximized {
        let (bx, by) = frame_borders(app.dpi);
        if pt.y < by {
            return if pt.x < bx * 2 {
                HTTOPLEFT
            } else if pt.x >= app.client.0 as i32 - bx * 2 {
                HTTOPRIGHT
            } else {
                HTTOP
            };
        }
    }
    let chrome = app.chrome();
    if p.1 < chrome.caption_height() {
        return match chrome.hit(p) {
            Some(Button::Close) => HTCLOSE,
            Some(Button::Maximize) => HTMAXBUTTON,
            Some(Button::Minimize) => HTMINBUTTON,
            Some(Button::Menu) => HTCLIENT,
            // Free caption strip: drag the window, but not while a
            // selection drag runs.
            None if !app.is_dragging() => HTCAPTION,
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
            if let Some(a) = app.double_click(p) {
                actions.extend(a);
            } else if app.fullscreen || app.chrome().hit(p).is_none() {
                actions.push(Action::ToggleFullscreen);
            }
            done
        }
        WM_RBUTTONUP | WM_CONTEXTMENU => {
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
            let result = unsafe { Box::from_raw(lp.0 as *mut Result<PathBuf, String>) };
            match *result {
                Ok(path) => {
                    actions.extend(app.open(path));
                    if app.settings.screenshot_clipboard {
                        actions.extend(app.command(blinzel_ui::menu::Cmd::Copy));
                    }
                    actions.push(Action::Wake { opened: true });
                }
                Err(e) => {
                    app.show_notice(screenshot::failure(&e));
                    unsafe {
                        SetTimer(Some(hwnd), TIMER_NOTICE, 3500, None);
                    }
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
        WM_RENDERFORMAT => {
            if let Some(src) = &app.clip {
                clipboard::render(src, wp.0 as u32);
            }
            done
        }
        WM_RENDERALLFORMATS => {
            if let Some(src) = app.clip.clone() {
                clipboard::render_all(hwnd, &src);
            }
            done
        }
        WM_DESTROYCLIPBOARD => {
            app.clip = None;
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
        Action::FitWindow { client } => {
            fit_window(hwnd, client);
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
            Some(p) => {
                // From the tray the window may be hidden.
                let mut more = with_app(|a| a.open(p)).unwrap_or_default();
                more.push(Action::Wake { opened: true });
                more
            }
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
                let services = with_app(|a| a.services.clone());
                let _ = std::thread::Builder::new()
                    .name("blinzel-save".into())
                    .spawn(move || {
                        let Some(services) = services else { return };
                        let text = blinzel_ui::app::save_cutout(&*services, &job, &target);
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
            resident::set_autostart(on);
            if on {
                resident::add_tray(hwnd);
            } else {
                resident::remove_tray(hwnd);
            }
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
            // One hidden instance is enough (several windows with "open in
            // the same window" off). A hidden window closed by someone
            // else (e.g. taskkill) exits.
            let hide = with_app(|a| a.settings.resident && !a.hidden).unwrap_or(false)
                && instance::hidden(Some(hwnd)).is_none();
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

/// Remembers where the window is, for the next start.
fn remember_placement(hwnd: HWND, a: &mut App) {
    if let Some((rect, maximized)) = placement(hwnd) {
        a.settings.window = Some(rect);
        a.settings.maximized = maximized;
    }
}

/// Saves the settings, keeps a copied image on the clipboard, removes the
/// tray icon and closes the window (which ends the message loop).
fn exit(hwnd: HWND) {
    let clip = with_app(|a| {
        // A hidden instance's copy may be older than what another window
        // saved.
        if !a.hidden {
            remember_placement(hwnd, a);
            settings_store::save(&a.settings);
        }
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

/// Resident mode: the window goes away, the process stays warm. An
/// installed update takes over now instead.
fn hide_window(hwnd: HWND) {
    if with_app(|a| a.fullscreen) == Some(true) {
        toggle_fullscreen(hwnd);
    }
    with_app(|a| {
        remember_placement(hwnd, a);
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
    let was_hidden = with_app(|a| {
        let hidden = a.hidden;
        if hidden {
            if !opened {
                a.reload_settings();
            }
            a.hidden = false;
            a.cloaked = true;
            a.activate_on_uncloak = true;
        }
        hidden
    })
    .unwrap_or(false);
    if !was_hidden {
        return vec![Action::Activate];
    }
    blinzel_core::timing::mark("wake");
    blinzel_core::bufpool::global().set_max_retained(blinzel_core::bufpool::DEFAULT_MAX_RETAINED);
    unsafe {
        // Painted while cloaked; not activated yet (costs ~20 ms).
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
    if opened {
        start_uncloak_timer(hwnd, 400);
        vec![]
    } else {
        let mut more = with_app(|a| a.render()).unwrap_or_default();
        more.push(Action::Uncloak);
        more
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

pub(crate) fn fit_window(hwnd: HWND, image: (u32, u32)) {
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
    let (cw, ch) = blinzel_core::layout::window_client_size(image, max, min);
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
