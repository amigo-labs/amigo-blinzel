//! Window placement: restored on a monitor that still exists, ignored
//! where none is, and remembered for the settings file. Uses a test window
//! (hidden, except while it is maximized).

#![cfg(windows)]

use blinzel_app::window::{remember_placement, restore_placement};
use blinzel_core::settings::Settings;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowPlacement, RegisterClassW,
    SW_MAXIMIZE, ShowWindow, WINDOW_EX_STYLE, WINDOWPLACEMENT, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::w;

extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, w, l) }
}

fn hidden_window() -> HWND {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap().into();
        let class = w!("BlinzelPlacementTest");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("test"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            300,
            200,
            None,
            None,
            Some(instance),
            None,
        )
        .unwrap()
    }
}

fn normal_rect(hwnd: HWND) -> [i32; 4] {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { GetWindowPlacement(hwnd, &mut wp) }.unwrap();
    let r = wp.rcNormalPosition;
    [r.left, r.top, r.right, r.bottom]
}

/// A 600 × 400 rect inside the primary monitor's work area.
fn inside_primary() -> [i32; 4] {
    let work = unsafe {
        let m = MonitorFromPoint(Default::default(), MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(m, &mut mi).unwrap();
        mi.rcWork
    };
    [
        work.left + 50,
        work.top + 40,
        work.left + 650,
        work.top + 440,
    ]
}

#[test]
fn restores_on_an_existing_monitor_only() {
    let hwnd = hidden_window();
    let inside = inside_primary();
    assert!(!restore_placement(hwnd, Some(inside), false));
    assert_eq!(normal_rect(hwnd), inside);
    // Maximized is reported back for the caller to apply.
    assert!(restore_placement(hwnd, Some(inside), true));
    // Far off every monitor: left where it is.
    let nowhere = [-100_000, -100_000, -99_000, -99_500];
    assert!(!restore_placement(hwnd, Some(nowhere), true));
    assert_eq!(normal_rect(hwnd), inside);
    assert!(!restore_placement(hwnd, None, true));
    unsafe { DestroyWindow(hwnd).unwrap() };
}

#[test]
fn remembers_the_normal_rect_and_whether_it_is_maximized() {
    let hwnd = hidden_window();
    let inside = inside_primary();
    assert!(!restore_placement(hwnd, Some(inside), false));
    let mut s = Settings::default();
    remember_placement(hwnd, &mut s);
    assert_eq!((s.window, s.maximized), (Some(inside), false));
    // Maximized: the rect to restore stays the normal one.
    unsafe {
        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
    }
    let mut s = Settings::default();
    remember_placement(hwnd, &mut s);
    assert_eq!((s.window, s.maximized), (Some(inside), true));
    // Both keys are written and read back.
    let text = s.to_text();
    let [l, t, r, b] = inside;
    let window = format!("window = {l},{t},{r},{b}");
    assert!(text.lines().any(|x| x == window), "{text}");
    assert!(text.lines().any(|x| x == "maximized = true"), "{text}");
    assert_eq!(Settings::parse(&text), s);
    unsafe { DestroyWindow(hwnd).unwrap() };
}
