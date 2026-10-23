//! The first image through GDI. Creating the Direct3D device takes longer
//! (115–150 ms on a real GPU) than reading and decoding a camera JPEG, so
//! the loader hands the first decoded image to the window as soon as it
//! exists; it is drawn on the CPU (`blinzel_core::preview`) with the same
//! geometry as the GPU frame and shown right away. The Direct3D frame
//! replaces it as soon as the device is ready.

use std::cell::RefCell;
use std::sync::Arc;

use blinzel_core::cpuref::Frame;
use blinzel_core::geom::Orientation;
use blinzel_core::layout::{
    ViewMode, ViewOptions, ViewState, compute, content_in_stage, stage_to_client,
};
use blinzel_core::pixel::Image;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, DIB_RGB_COLORS, EndPaint, InvalidateRect,
    PAINTSTRUCT, SetDIBitsToDevice, UpdateWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, SW_SHOW, ShowWindow, WM_APP, WM_PAINT,
};

/// LPARAM: `Box<FirstFrame>`, owned by the receiver.
pub const WM_FIRST_FRAME: u32 = WM_APP + 5;

/// A decoded image the device was not ready for yet.
pub struct FirstFrame {
    pub image: Arc<Image>,
    pub orientation: Orientation,
}

#[derive(Default)]
struct State {
    view: ViewOptions,
    background: [u8; 3],
    /// Top-down BGRA at the client size.
    bitmap: Option<(Vec<u8>, u32, u32)>,
    shown: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The view settings the first frame is laid out with (UI thread).
pub fn configure(view: ViewOptions, background: [u8; 3]) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.view = view;
        s.background = background;
    });
}

/// Whether the GDI frame made the window visible.
pub fn shown() -> bool {
    STATE.with(|s| s.borrow().shown)
}

/// Frees the bitmap once Direct3D owns the window.
pub fn release() {
    STATE.with(|s| s.borrow_mut().bitmap = None);
}

/// Handles the first-frame messages while no app exists yet; `None` for
/// everything else.
pub fn on_message(hwnd: HWND, msg: u32, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_FIRST_FRAME => {
            // SAFETY: the loader posts a leaked Box<FirstFrame>.
            let ff = unsafe { Box::from_raw(lp.0 as *mut FirstFrame) };
            prepare(hwnd, &ff);
            Some(LRESULT(0))
        }
        WM_PAINT if STATE.with(|s| s.borrow().bitmap.is_some()) => {
            paint(hwnd);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn prepare(hwnd: HWND, ff: &FirstFrame) {
    let (view, background) = STATE.with(|s| {
        let s = s.borrow();
        (s.view, s.background)
    });
    if STATE.with(|s| s.borrow().shown) {
        return;
    }
    let img = &ff.image;
    let full = (img.full_width, img.full_height);
    let oriented = ff.orientation.oriented_size(full.0, full.1);
    if view.mode == ViewMode::FitWindowToImage {
        crate::window::fit_window(hwnd, oriented);
    }
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    let client = (
        (r.right - r.left).max(1) as u32,
        (r.bottom - r.top).max(1) as u32,
    );
    let l = compute(
        oriented,
        (client.0 as f64, client.1 as f64),
        &view,
        &ViewState::default(),
    );
    let stage = (img.width, img.height);
    let frame = Frame {
        client,
        stage_to_client: stage_to_client(&l, ff.orientation, full, stage),
        content: content_in_stage(&l, ff.orientation, full, stage),
        background,
    };
    blinzel_core::timing::mark("first_frame_received");
    let bgra = blinzel_core::preview::render(img, &frame);
    blinzel_core::timing::mark("first_frame_rendered");
    STATE.with(|s| s.borrow_mut().bitmap = Some((bgra, client.0, client.1)));
    unsafe {
        // Visible to Windows (still cloaked) so it gets painted; paint now.
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = InvalidateRect(Some(hwnd), None, false);
        let _ = UpdateWindow(hwnd);
    }
}

fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let first = STATE.with(|s| {
        let s = s.borrow();
        if let Some((bits, w, h)) = &s.bitmap {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: *w as i32,
                    // Negative: top-down rows.
                    biHeight: -(*h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            unsafe {
                SetDIBitsToDevice(
                    hdc,
                    0,
                    0,
                    *w,
                    *h,
                    0,
                    0,
                    0,
                    *h,
                    bits.as_ptr().cast(),
                    &info,
                    DIB_RGB_COLORS,
                );
            }
        }
        !s.shown
    });
    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
    if first {
        STATE.with(|s| s.borrow_mut().shown = true);
        blinzel_core::timing::mark("first_gdi_frame");
        crate::window::uncloak(hwnd);
    }
}
