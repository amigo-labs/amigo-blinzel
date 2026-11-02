//! Copy with delayed rendering: Ctrl+C only announces the formats, which is
//! instant even for 100 MP. The pixels are decoded (only the selected
//! region, at full resolution) when an application actually pastes.
//! Screenshots go on at once ([`put_shot`]).

use blinzel_core::decode::Request;
use blinzel_core::selection::orient_bgra;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, WPARAM};
use windows::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIBV5;
use windows::Win32::UI::WindowsAndMessaging::{
    WM_DESTROYCLIPBOARD, WM_RENDERALLFORMATS, WM_RENDERFORMAT,
};
use windows::core::w;

use crate::platform::{debug_log, io};
use crate::wic;

/// What to put on the clipboard when asked.
pub use blinzel_ui::app::ClipSource as Source;

pub fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

/// Takes clipboard ownership and announces DIBV5 and PNG without data.
pub fn announce(hwnd: HWND) -> bool {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let ok = EmptyClipboard().is_ok();
        // NULL handle = delayed rendering (WM_RENDERFORMAT later).
        let _ = SetClipboardData(CF_DIBV5.0 as u32, None);
        let _ = SetClipboardData(png_format(), None);
        let _ = CloseClipboard();
        ok
    }
}

/// The clipboard's messages to the window that owns it, with the copy it
/// announced: `WM_RENDERFORMAT` (a program pastes: decode the region now),
/// `WM_RENDERALLFORMATS` (the window goes away: render both formats) and
/// `WM_DESTROYCLIPBOARD` (another copy replaced ours: forget it). `false`
/// for every other message.
pub fn on_message(hwnd: HWND, msg: u32, wp: WPARAM, clip: &mut Option<Source>) -> bool {
    match msg {
        WM_RENDERFORMAT => {
            if let Some(src) = clip {
                render(src, wp.0 as u32);
            }
        }
        WM_RENDERALLFORMATS => {
            if let Some(src) = clip {
                render_all(hwnd, src);
            }
        }
        WM_DESTROYCLIPBOARD => *clip = None,
        _ => return false,
    }
    true
}

/// Decodes the region and places `format` on the clipboard. Called from
/// WM_RENDERFORMAT (clipboard already open by the requester) or from
/// WM_RENDERALLFORMATS after opening it ourselves.
pub fn render(src: &Source, format: u32) {
    let t = std::time::Instant::now();
    let pixels = match decode(src) {
        Ok(p) => p,
        Err(e) => {
            debug_log(&format!("clipboard decode failed: {e}"));
            return;
        }
    };
    let (bgra, w, h) = pixels;
    let bytes = if format == CF_DIBV5.0 as u32 {
        dibv5(&bgra, w, h)
    } else if format == png_format() {
        match png(&bgra, w, h) {
            Some(b) => b,
            None => return,
        }
    } else {
        return;
    };
    if set(format, &bytes) {
        debug_log(&format!(
            "clipboard format {format} rendered in {:.1} ms",
            t.elapsed().as_secs_f64() * 1e3
        ));
    }
}

/// Places `bytes` as `format` on the opened clipboard.
fn set(format: u32, bytes: &[u8]) -> bool {
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
            return false;
        };
        let p = GlobalLock(mem);
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(mem);
        if SetClipboardData(format, Some(HANDLE(mem.0))).is_err() {
            let _ = GlobalFree(Some(HGLOBAL(mem.0)));
            return false;
        }
        true
    }
}

/// Puts a screenshot on the clipboard at once (DIBV5 and the `png`
/// already encoded): it is small, and nothing could render it later.
/// Another program may hold the clipboard for a moment, so this retries
/// briefly.
pub fn put_shot(hwnd: HWND, bgra: &[u8], w: u32, h: u32, png: &[u8]) -> bool {
    let opened = (0..10).any(|i| {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        unsafe { OpenClipboard(Some(hwnd)).is_ok() }
    });
    if !opened {
        return false;
    }
    unsafe {
        let ok = EmptyClipboard().is_ok()
            && set(CF_DIBV5.0 as u32, &dibv5(bgra, w, h))
            && set(png_format(), png);
        let _ = CloseClipboard();
        ok
    }
}

/// Renders all formats before we lose the data (window closing).
pub fn render_all(hwnd: HWND, src: &Source) {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        // Only if we still own it.
        if windows::Win32::System::DataExchange::GetClipboardOwner().ok() == Some(hwnd) {
            render(src, CF_DIBV5.0 as u32);
            render(src, png_format());
        }
        let _ = CloseClipboard();
    }
}

fn decode(src: &Source) -> Result<(Vec<u8>, u32, u32), String> {
    let data = io::read_file(&src.path).map_err(|e| e.to_string())?;
    let img =
        wic::decode_any(&data, Request::Region(src.stored_rect)).map_err(|e| e.to_string())?;
    let bgra = img.to_bgra();
    Ok(orient_bgra(&bgra, img.width, img.height, src.orientation))
}

/// BITMAPV5HEADER + bottom-up BGRA rows with an alpha mask.
fn dibv5(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    let header = BITMAPV5HEADER {
        bV5Size: std::mem::size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: w as i32,
        bV5Height: h as i32,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: w * h * 4,
        bV5RedMask: 0x00FF_0000,
        bV5GreenMask: 0x0000_FF00,
        bV5BlueMask: 0x0000_00FF,
        bV5AlphaMask: 0xFF00_0000,
        bV5CSType: 0x7352_4742, // 'sRGB'
        bV5Intent: 4,           // LCS_GM_IMAGES
        ..Default::default()
    };
    let hsize = std::mem::size_of::<BITMAPV5HEADER>();
    let mut out = vec![0u8; hsize + bgra.len()];
    unsafe {
        std::ptr::copy_nonoverlapping(
            std::ptr::from_ref(&header).cast::<u8>(),
            out.as_mut_ptr(),
            hsize,
        )
    };
    let row = w as usize * 4;
    for y in 0..h as usize {
        let src = &bgra[y * row..(y + 1) * row];
        let dst_y = h as usize - 1 - y;
        out[hsize + dst_y * row..hsize + (dst_y + 1) * row].copy_from_slice(src);
    }
    out
}

fn png(bgra: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let rgba: Vec<u8> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.write_header().ok()?.write_image_data(&rgba).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_core::geom::{Orientation, PixelRect};
    use std::cell::RefCell;
    use windows::Win32::Foundation::{LPARAM, LRESULT};
    use windows::Win32::System::DataExchange::{
        GetClipboardData, GetClipboardOwner, IsClipboardFormatAvailable,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::GlobalSize;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_MESSAGE, RegisterClassW,
        WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
    };

    thread_local! {
        /// The test window's copy, as `App::clip` is the viewer window's.
        static CLIP: RefCell<Option<Source>> = const { RefCell::new(None) };
    }

    /// Hands the clipboard's messages to [`on_message`], as the viewer's
    /// window procedure does.
    extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let handled = CLIP.with(|c| {
            c.try_borrow_mut()
                .is_ok_and(|mut c| on_message(hwnd, msg, wp, &mut c))
        });
        if handled {
            LRESULT(0)
        } else {
            // SAFETY: the arguments are the ones the system passed.
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }

    /// A message-only window that owns the clipboard.
    fn window() -> HWND {
        let class = w!("BlinzelClipboardTest");
        // SAFETY: the class lives in this module's image; the window is
        // destroyed by the test that made it.
        unsafe {
            let instance = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            )
            .unwrap()
        }
    }

    /// Opens the clipboard as a program that pastes; another program may
    /// hold it for a moment.
    fn open() {
        let opened = (0..50).any(|i| {
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // SAFETY: closed again by the caller.
            unsafe { OpenClipboard(None).is_ok() }
        });
        assert!(opened, "the clipboard opens");
    }

    /// Pastes `format` from the open clipboard (the owner renders it now
    /// if it has not yet). `GlobalSize` may round up: the bytes can end
    /// with padding.
    fn paste(format: u32) -> Vec<u8> {
        // SAFETY: the handle belongs to the open clipboard and stays valid
        // until it closes; it is locked while it is read.
        unsafe {
            let h = GetClipboardData(format).expect("the format is on the clipboard");
            let mem = HGLOBAL(h.0);
            let p = GlobalLock(mem);
            assert!(!p.is_null());
            let bytes = std::slice::from_raw_parts(p.cast::<u8>(), GlobalSize(mem)).to_vec();
            let _ = GlobalUnlock(mem);
            bytes
        }
    }

    /// Checks a pasted DIBV5 and PNG against the region's pixels.
    fn check(dib: &[u8], encoded: &[u8], bgra: &[u8], w: u32, h: u32) {
        let hsize = std::mem::size_of::<BITMAPV5HEADER>();
        assert!(dib.len() >= hsize + bgra.len());
        // SAFETY: at least `hsize` bytes (checked above); the header may be
        // unaligned in the vector.
        let header = unsafe { std::ptr::read_unaligned(dib.as_ptr().cast::<BITMAPV5HEADER>()) };
        assert_eq!((header.bV5Width, header.bV5Height), (w as i32, h as i32));
        assert_eq!(header.bV5BitCount, 32);
        assert_eq!(header.bV5AlphaMask, 0xFF00_0000, "with alpha");
        assert_eq!(header.bV5CSType, 0x7352_4742, "sRGB");
        let row = w as usize * 4;
        for y in 0..h as usize {
            let bottom_up = hsize + (h as usize - 1 - y) * row;
            assert_eq!(
                &dib[bottom_up..bottom_up + row],
                &bgra[y * row..(y + 1) * row]
            );
        }
        let mut reader = png::Decoder::new(std::io::Cursor::new(encoded))
            .read_info()
            .expect("a PNG");
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (w, h));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        let rgba: Vec<u8> = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        assert_eq!(&buf[..info.buffer_size()], &rgba[..]);
    }

    /// `docs/specs/selection-and-export.md` AC-09: a copy announces
    /// `CF_DIBV5` and `PNG` without pixels, decodes the region when a
    /// program pastes, and renders both before the viewer exits.
    #[test]
    fn a_copy_is_decoded_when_pasted_and_kept_after_exit() {
        let dir = std::env::temp_dir().join(format!("blinzel-clip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (16u32, 12u32);
        let file: Vec<u8> = blinzel_core::testimg::rgb(w, h)
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        let file = blinzel_core::export::encode_png_bgra(&file, w, h).unwrap();
        // A region, turned 90° clockwise by the user.
        let src = Source {
            path: dir.join("a.png"),
            stored_rect: PixelRect::new(2, 1, 9, 7),
            orientation: Orientation::from_exif(6),
        };
        let hwnd = window();

        // The copy: the file does not even exist yet, nothing is decoded.
        assert!(announce(hwnd), "the clipboard is ours");
        CLIP.with(|c| *c.borrow_mut() = Some(src.clone()));
        // SAFETY: plain queries.
        unsafe {
            assert_eq!(GetClipboardOwner().ok(), Some(hwnd));
            assert!(IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok());
            assert!(IsClipboardFormatAvailable(png_format()).is_ok());
        }
        // A program pastes: now the region is decoded from the file.
        std::fs::write(&src.path, file).unwrap();
        let (px, pw, ph) = src.pixels(&crate::backend::WinServices::default()).unwrap();
        assert_eq!((pw, ph), (7, 9), "turned upright");
        open();
        let (dib, encoded) = (paste(CF_DIBV5.0 as u32), paste(png_format()));
        // SAFETY: opened above.
        unsafe {
            let _ = CloseClipboard();
        }
        check(&dib, &encoded, &px, pw, ph);

        // Copied again, then the viewer exits while it owns the clipboard
        // (`window::exit`): both formats are rendered first, so a paste
        // after the exit needs neither the window nor the file.
        assert!(announce(hwnd));
        CLIP.with(|c| *c.borrow_mut() = Some(src.clone()));
        let clip = CLIP.with(|c| c.borrow_mut().take()).unwrap();
        render_all(hwnd, &clip);
        // SAFETY: the test's own window.
        unsafe { DestroyWindow(hwnd).unwrap() };
        std::fs::remove_dir_all(&dir).unwrap();
        open();
        let (dib, encoded) = (paste(CF_DIBV5.0 as u32), paste(png_format()));
        // SAFETY: opened above.
        unsafe {
            let _ = CloseClipboard();
        }
        check(&dib, &encoded, &px, pw, ph);
    }
}
