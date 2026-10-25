//! The setup window: frameless and dark like the viewer, drawn with its
//! renderer and DirectWrite text. It asks, installs, then offers the
//! default-apps settings (Windows does not let apps make themselves the
//! default) or starting the viewer.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use blinzel_core::geom::Rect;
use blinzel_i18n::{Msg, tr, trf};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    ScreenToClient, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::{Layout, install_now};
use crate::gfx::text::Text;
use crate::gfx::{Gfx, Renderer, Rgba, Shape, SwapChain};
use crate::platform::wide;

/// Exit codes of the interactive setup.
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_CANCELLED: i32 = 3;

const CLASS: PCWSTR = w!("AmigoBlinzelSetup");
/// Window and layout sizes in DIPs.
const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 290.0;
const PAD: f64 = 28.0;
const BUTTON_W: f64 = 176.0;
const BUTTON_H: f64 = 34.0;
const GAP: f64 = 12.0;
const CLOSE_W: f64 = 46.0;
const CLOSE_H: f64 = 32.0;

const BACKGROUND: [u8; 3] = [0x1E, 0x1E, 0x1E];
const ACCENT: Rgba = Rgba(0.0, 0.47, 0.84, 1.0);
const ACCENT_HOVER: Rgba = Rgba(0.10, 0.55, 0.92, 1.0);

/// Shows a message box (also where Direct3D is unavailable).
pub fn message(text: &str, error: bool) {
    let (t, c) = (wide(text), wide(tr(Msg::AppName)));
    let icon = if error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(t.as_ptr()),
            PCWSTR(c.as_ptr()),
            icon | MB_OK | crate::platform::message_box_direction(),
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Btn {
    Primary,
    Secondary,
    Close,
}

enum State {
    Ask,
    Done,
    Failed(String),
}

struct Ui {
    gfx: Gfx,
    renderer: Renderer,
    swap: SwapChain,
    text: Text,
    /// Physical pixels per DIP.
    scale: f64,
    client: (u32, u32),
    state: State,
    hover: Option<Btn>,
    pressed: Option<Btn>,
    layout: Layout,
    exe: PathBuf,
    exit: i32,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

/// Runs the interactive installation; returns the exit code.
pub fn run_install(layout: &Layout, exe: &Path) -> i32 {
    match open_window(layout, exe) {
        Ok(()) => message_loop(),
        // No usable Direct3D: the same flow with message boxes.
        Err(_) => fallback(layout, exe),
    }
}

fn fallback(layout: &Layout, exe: &Path) -> i32 {
    let body = body_ask(layout);
    let (t, c) = (
        wide(format!("{}\n\n{body}", tr(Msg::SetupTitle))),
        wide(tr(Msg::AppName)),
    );
    let answer = unsafe {
        MessageBoxW(
            None,
            PCWSTR(t.as_ptr()),
            PCWSTR(c.as_ptr()),
            MB_OKCANCEL | MB_ICONQUESTION | crate::platform::message_box_direction(),
        )
    };
    if answer != IDOK {
        return EXIT_CANCELLED;
    }
    match install_now(layout, exe) {
        Ok(()) => {
            message(
                &format!("{}\n\n{}", tr(Msg::SetupDoneTitle), tr(Msg::SetupDoneBody)),
                false,
            );
            EXIT_OK
        }
        Err(e) => {
            message(&format!("{}\n\n{e}", tr(Msg::SetupFailedTitle)), true);
            EXIT_FAILED
        }
    }
}

fn body_ask(layout: &Layout) -> String {
    trf(
        Msg::SetupBody,
        &[
            ("version", &super::version()),
            ("dir", &layout.dir.display()),
        ],
    )
}

fn open_window(layout: &Layout, exe: &Path) -> Result<(), String> {
    let gfx = Gfx::create(false).map_err(|e| e.message())?;
    let renderer = Renderer::new(&gfx.device).map_err(|e| e.message())?;
    let text = Text::new(&gfx.device).map_err(|e| e.message())?;
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.message())?.into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.message())?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // Centred on the monitor under the cursor, sized for its DPI.
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let (mut dpi, mut dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
        let scale = dpi.max(96) as f64 / 96.0;
        let (w, h) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut mi);
        let work = mi.rcWork;
        let (x, y) = (
            work.left + (work.right - work.left - w) / 2,
            work.top + (work.bottom - work.top - h) / 2,
        );
        let title = wide(tr(Msg::SetupTitle));
        // Fixed size: a caption-style window without resize or maximize,
        // whose caption WM_NCCALCSIZE removes.
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| e.message())?;
        let on: i32 = 1;
        for attr in [
            DWMWA_TRANSITIONS_FORCEDISABLED,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
        ] {
            let _ = DwmSetWindowAttribute(hwnd, attr, std::ptr::from_ref(&on).cast(), 4);
        }
        let _ = DwmExtendFrameIntoClientArea(
            hwnd,
            &MARGINS {
                cyTopHeight: 1,
                ..Default::default()
            },
        );
        // Apply WM_NCCALCSIZE (caption removed).
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_FRAMECHANGED);
        let client = client_size(hwnd);
        let swap = SwapChain::new(&gfx, hwnd, client).map_err(|e| e.message())?;
        UI.with(|cell| {
            *cell.borrow_mut() = Some(Ui {
                gfx,
                renderer,
                swap,
                text,
                scale,
                client,
                state: State::Ask,
                hover: None,
                pressed: None,
                layout: layout.clone(),
                exe: exe.to_path_buf(),
                exit: EXIT_CANCELLED,
            })
        });
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

fn message_loop() -> i32 {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    UI.with(|cell| {
        cell.borrow_mut()
            .take()
            .map_or(EXIT_CANCELLED, |ui| ui.exit)
    })
}

fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    (
        (r.right - r.left).max(1) as u32,
        (r.bottom - r.top).max(1) as u32,
    )
}

impl Ui {
    fn buttons(&self) -> Vec<(Btn, Rect)> {
        let s = self.scale;
        let (cw, ch) = (self.client.0 as f64, self.client.1 as f64);
        let (bw, bh) = (BUTTON_W * s, BUTTON_H * s);
        let y = ch - PAD * s - bh;
        let primary = Rect::new(cw - PAD * s - bw, y, bw, bh);
        let mut out = vec![
            (
                Btn::Close,
                Rect::new(cw - CLOSE_W * s, 0.0, CLOSE_W * s, CLOSE_H * s),
            ),
            (Btn::Primary, primary),
        ];
        if !matches!(self.state, State::Failed(_)) {
            out.push((
                Btn::Secondary,
                Rect::new(primary.x - GAP * s - bw, y, bw, bh),
            ));
        }
        out
    }

    fn hit(&self, p: (f64, f64)) -> Option<Btn> {
        self.buttons()
            .into_iter()
            .find(|(_, r)| r.contains(p))
            .map(|(b, _)| b)
    }

    /// Title, body, primary and secondary labels for the current state.
    fn texts(&self) -> (String, String, &'static str, &'static str) {
        match &self.state {
            State::Ask => (
                tr(Msg::SetupTitle).into(),
                body_ask(&self.layout),
                tr(Msg::SetupInstall),
                tr(Msg::SetupCancel),
            ),
            State::Done => (
                tr(Msg::SetupDoneTitle).into(),
                tr(Msg::SetupDoneBody).into(),
                tr(Msg::SetupDefaultApps),
                tr(Msg::SetupStart),
            ),
            State::Failed(e) => (
                tr(Msg::SetupFailedTitle).into(),
                e.clone(),
                tr(Msg::SetupClose),
                "",
            ),
        }
    }

    fn paint(&mut self) {
        let Some(rtv) = self.swap.rtv().cloned() else {
            return;
        };
        let s = self.scale;
        let ctx = self.gfx.context.clone();
        self.renderer
            .begin(&ctx, &rtv, self.client, Rgba::from_u8(BACKGROUND, 1.0));

        let mut shapes = Vec::new();
        for (b, r) in self.buttons() {
            let hot = self.hover == Some(b);
            let color = match b {
                Btn::Primary if hot => ACCENT_HOVER,
                Btn::Primary => ACCENT,
                Btn::Secondary => Rgba(1.0, 1.0, 1.0, if hot { 0.16 } else { 0.09 }),
                Btn::Close if hot => Rgba(0.769, 0.169, 0.110, 1.0),
                Btn::Close => continue,
            };
            let radius = if b == Btn::Close { 0.0 } else { 4.0 * s as f32 };
            shapes.push(Shape::Fill {
                rect: r,
                radius,
                color,
            });
        }
        // The close glyph, as in the viewer's caption.
        let (_, cr) = self.buttons()[0];
        let (cx, cy, d) = (
            (cr.x + cr.w / 2.0) as f32,
            (cr.y + cr.h / 2.0) as f32,
            5.0 * s as f32,
        );
        for (a, b) in [
            ((cx - d, cy - d), (cx + d, cy + d)),
            ((cx - d, cy + d), (cx + d, cy - d)),
        ] {
            shapes.push(Shape::Line {
                a,
                b,
                thickness: 1.1 * s as f32,
                color: Rgba(1.0, 1.0, 1.0, 0.95),
            });
        }
        self.renderer.draw_shapes(&ctx, &shapes);

        let (title, body, primary, secondary) = self.texts();
        let max_w = (self.client.0 as f64 - 2.0 * PAD * s) as f32;
        let white = Rgba(1.0, 1.0, 1.0, 0.96);
        let mut y = (PAD + 6.0) * s;
        if let Ok(t) = self
            .text
            .get_weighted(&title, (22.0 * s) as f32, max_w, true)
        {
            self.renderer
                .draw_text(&ctx, &t.srv, ((PAD * s) as f32, y as f32), t.size, white);
            y += t.size.1 as f64 + 10.0 * s;
        }
        if let Ok(t) = self
            .text
            .get_weighted(&body, (14.0 * s) as f32, max_w, false)
        {
            let dim = Rgba(1.0, 1.0, 1.0, 0.80);
            self.renderer
                .draw_text(&ctx, &t.srv, ((PAD * s) as f32, y as f32), t.size, dim);
        }
        for (b, r) in self.buttons() {
            let label = match b {
                Btn::Primary => primary,
                Btn::Secondary => secondary,
                Btn::Close => continue,
            };
            if let Ok(t) = self
                .text
                .get_weighted(label, (14.0 * s) as f32, r.w as f32, true)
            {
                let pos = (
                    (r.x + (r.w - t.size.0 as f64) / 2.0) as f32,
                    (r.y + (r.h - t.size.1 as f64) / 2.0) as f32,
                );
                self.renderer.draw_text(&ctx, &t.srv, pos, t.size, white);
            }
        }
        let _ = self.swap.present();
    }

    /// Acts on a button; returns whether the window should close.
    fn activate(&mut self, b: Btn) -> bool {
        match (&self.state, b) {
            (State::Ask, Btn::Primary) => {
                self.state = match install_now(&self.layout, &self.exe) {
                    Ok(()) => {
                        self.exit = EXIT_OK;
                        State::Done
                    }
                    Err(e) => {
                        self.exit = EXIT_FAILED;
                        State::Failed(e)
                    }
                };
                false
            }
            (State::Done, Btn::Primary) => {
                open_default_apps();
                true
            }
            (State::Done, Btn::Secondary) => {
                let _ = std::process::Command::new(self.layout.exe()).spawn();
                true
            }
            // Cancel, close, or close after a failure.
            _ => true,
        }
    }
}

/// The default-apps page with amigo-blinzel preselected.
fn open_default_apps() {
    let uri = wide(format!(
        "ms-settings:defaultapps?registeredAppUser={}",
        super::install::PRODUCT
    ));
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(uri.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|cell| cell.borrow_mut().as_mut().map(f))
}

fn client_point(hwnd: HWND, lp: LPARAM, screen: bool) -> (f64, f64) {
    let mut p = POINT {
        x: (lp.0 & 0xFFFF) as i16 as i32,
        y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    if screen {
        unsafe {
            let _ = ScreenToClient(hwnd, &mut p);
        }
    }
    (p.x as f64, p.y as f64)
}

fn press(hwnd: HWND, b: Btn) {
    let close = with_ui(|ui| {
        let close = ui.activate(b);
        ui.hover = None;
        ui.pressed = None;
        close
    })
    .unwrap_or(true);
    unsafe {
        if close {
            let _ = DestroyWindow(hwnd);
        } else {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // Client area = whole window (the caption style keeps the DWM
            // shadow and rounded corners).
            WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
            WM_NCHITTEST => {
                let p = client_point(hwnd, lp, true);
                let on_button = with_ui(|ui| ui.hit(p).is_some()).unwrap_or(false);
                // Drag the window by any empty spot.
                return LRESULT(if on_button {
                    HTCLIENT as isize
                } else {
                    HTCAPTION as isize
                });
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_DPICHANGED => {
                // Dragged to a monitor with another scale.
                let dpi = (wp.0 >> 16) & 0xFFFF;
                with_ui(|ui| ui.scale = dpi.max(96) as f64 / 96.0);
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return LRESULT(0);
            }
            WM_PAINT => {
                with_ui(|ui| ui.paint());
                let _ = ValidateRect(Some(hwnd), None);
                return LRESULT(0);
            }
            WM_SIZE => {
                let size = (
                    (lp.0 & 0xFFFF).max(1) as u32,
                    ((lp.0 >> 16) & 0xFFFF).max(1) as u32,
                );
                with_ui(|ui| {
                    ui.client = size;
                    let _ = ui.swap.resize(&ui.gfx, size);
                });
                return LRESULT(0);
            }
            WM_MOUSEMOVE => {
                let p = client_point(hwnd, lp, false);
                let changed = with_ui(|ui| {
                    let h = ui.hit(p);
                    std::mem::replace(&mut ui.hover, h) != h
                })
                .unwrap_or(false);
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                if changed {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
            WM_MOUSELEAVE => {
                with_ui(|ui| ui.hover = None);
                let _ = InvalidateRect(Some(hwnd), None, false);
                return LRESULT(0);
            }
            WM_LBUTTONDOWN => {
                let p = client_point(hwnd, lp, false);
                with_ui(|ui| ui.pressed = ui.hit(p));
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let p = client_point(hwnd, lp, false);
                let clicked = with_ui(|ui| {
                    let h = ui.hit(p);
                    (ui.pressed.take() == h).then_some(h).flatten()
                })
                .flatten();
                if let Some(b) = clicked {
                    press(hwnd, b);
                }
                return LRESULT(0);
            }
            WM_KEYDOWN => {
                let key = wp.0 as u16;
                if key == VK_RETURN.0 {
                    press(hwnd, Btn::Primary);
                } else if key == VK_ESCAPE.0 {
                    press(hwnd, Btn::Close);
                }
                return LRESULT(0);
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
