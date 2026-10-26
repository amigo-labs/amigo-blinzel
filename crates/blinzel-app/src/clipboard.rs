//! Copy with delayed rendering: Ctrl+C only announces the formats, which is
//! instant even for 100 MP. The pixels are decoded (only the selected
//! region, at full resolution) when an application actually pastes.

use blinzel_core::decode::Request;
use blinzel_core::selection::orient_bgra;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIBV5;
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
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
            return;
        };
        let p = GlobalLock(mem);
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(mem);
        if SetClipboardData(format, Some(HANDLE(mem.0))).is_err() {
            let _ = GlobalFree(Some(HGLOBAL(mem.0)));
        }
    }
    debug_log(&format!(
        "clipboard format {format} rendered in {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    ));
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
