//! Fallback decoder through the Windows Imaging Component: TIFF, and HEIC,
//! AVIF, WebP, JPEG XR, RAW wherever the codecs are installed (Store
//! extensions, camera codec packs). WIC codecs that support scaled decoding
//! (`IWICBitmapSourceTransform`, e.g. many RAW and JPEG codecs) decode
//! directly at the requested size.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};

use blinzel_core::decode::{DecodeError, Probe, Request, Result};
use blinzel_core::geom::Orientation;
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapDecoder,
    IWICBitmapFrameDecode, IWICBitmapSource, IWICBitmapSourceTransform, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICBitmapTransformRotate0,
    WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Variant::VT_UI2;
use windows::core::{GUID, Interface, w};

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

fn ensure_com() {
    COM_READY.with(|c| {
        if !c.get() {
            // Worker threads join the MTA; ignore "already initialized".
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            c.set(true);
        }
    });
}

fn err(e: windows::core::Error) -> DecodeError {
    DecodeError::Corrupt(format!("WIC: {}", e.message()))
}

fn factory() -> Result<IWICImagingFactory> {
    ensure_com();
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }.map_err(err)
}

fn open(f: &IWICImagingFactory, data: &[u8]) -> Result<(IWICBitmapDecoder, IWICBitmapFrameDecode)> {
    unsafe {
        let stream = f.CreateStream().map_err(err)?;
        // The stream reads `data` in place; it lives as long as this call.
        stream.InitializeFromMemory(data).map_err(err)?;
        let decoder = f
            .CreateDecoderFromStream(
                &stream,
                std::ptr::null::<GUID>(),
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(|_| DecodeError::Unsupported(blinzel_core::format::detect(data)))?;
        let frame = decoder.GetFrame(0).map_err(err)?;
        Ok((decoder, frame))
    }
}

fn orientation(frame: &IWICBitmapFrameDecode) -> Orientation {
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else {
            return Orientation::NORMAL;
        };
        let mut pv = PROPVARIANT::default();
        let mut o = Orientation::NORMAL;
        if reader
            .GetMetadataByName(w!("System.Photo.Orientation"), &mut pv)
            .is_ok()
        {
            let inner = &pv.Anonymous.Anonymous;
            if inner.vt == VT_UI2 {
                o = Orientation::from_exif(inner.Anonymous.uiVal);
            }
        }
        let _ = PropVariantClear(&mut pv);
        o
    }
}

/// Size and orientation via WIC (reads headers only).
pub fn probe(data: &[u8]) -> Result<Probe> {
    let f = factory()?;
    let (_d, frame) = open(&f, data)?;
    let (mut w, mut h) = (0u32, 0u32);
    unsafe { frame.GetSize(&mut w, &mut h) }.map_err(err)?;
    Ok(Probe {
        format: blinzel_core::format::detect(data),
        width: w,
        height: h,
        orientation: orientation(&frame),
        previews: Vec::new(),
        icc: None,
    })
}

/// Decodes through WIC into straight-alpha BGRA.
pub fn decode(data: &[u8], req: Request) -> Result<Image> {
    let f = factory()?;
    let (_d, frame) = open(&f, data)?;
    let (mut fw, mut fh) = (0u32, 0u32);
    unsafe { frame.GetSize(&mut fw, &mut fh) }.map_err(err)?;
    blinzel_core::decode::probe_size_ok(fw, fh)?;

    // Scaled decode where the codec supports it.
    let (mut w, mut h) = (fw, fh);
    let mut transform: Option<IWICBitmapSourceTransform> = None;
    if let Request::Fit { w: tw, h: th } = req
        && let Ok(t) = frame.cast::<IWICBitmapSourceTransform>()
    {
        let (mut cw, mut ch) = (tw.max(1), th.max(1));
        if unsafe { t.GetClosestSize(&mut cw, &mut ch) }.is_ok() && cw < fw && cw >= tw && ch >= th
        {
            (w, h) = (cw, ch);
            transform = Some(t);
        }
    }

    let mut plane = Plane::new(w, h, 4);
    let stride = plane.stride as u32;
    unsafe {
        match &transform {
            Some(t) => {
                let rc = WICRect {
                    X: 0,
                    Y: 0,
                    Width: fw as i32,
                    Height: fh as i32,
                };
                let ok = t.CopyPixels(
                    &rc,
                    w,
                    h,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapTransformRotate0,
                    stride,
                    &mut plane.data,
                );
                if ok.is_err() {
                    // Codec claims support but cannot deliver BGRA: convert.
                    return decode(data, Request::Full);
                }
            }
            None => {
                let conv = f.CreateFormatConverter().map_err(err)?;
                let src: IWICBitmapSource = frame.cast().map_err(err)?;
                conv.Initialize(
                    &src,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(err)?;
                conv.CopyPixels(std::ptr::null(), stride, &mut plane.data)
                    .map_err(err)?;
            }
        }
    }
    let alpha = if plane.data.as_chunks::<4>().0.iter().any(|p| p[3] != 255) {
        Alpha::Straight
    } else {
        Alpha::Opaque
    };
    let img = Image {
        pixels: Pixels::Packed {
            plane,
            format: PackedFormat::Bgra8,
            alpha,
        },
        width: w,
        height: h,
        full_width: fw,
        full_height: fh,
        region: None,
    };
    match req {
        Request::Region(r) => blinzel_core::decode::crop_to_bgra(&img, r),
        _ => Ok(img),
    }
}

/// WIC's WebP codec: decodes lossy stills about 20 % faster than
/// `image-webp`, but its first use in a process costs 200-400 ms, more than
/// it saves on one image. So the first lossy WebP decodes natively and
/// loads the codec in the background; later ones go to WIC.
static WEBP_CODEC: AtomicU8 = AtomicU8::new(COLD);
const COLD: u8 = 0;
const LOADING: u8 = 1;
const READY: u8 = 2;
/// No WebP codec in WIC (e.g. Windows N without the extension).
const MISSING: u8 = 3;

fn webp_via_wic(data: &[u8]) -> bool {
    use blinzel_core::format::{Format, detect};
    if detect(data) != Format::WebP || !blinzel_core::decode::webp::is_lossy_still(data) {
        return false;
    }
    match WEBP_CODEC.compare_exchange(COLD, LOADING, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            let _ = std::thread::Builder::new()
                .name("wic-webp".into())
                .spawn(|| {
                    let tiny = &blinzel_core::decode::webp::TINY_LOSSY;
                    let ok = blinzel_core::decode::guard(|| decode(tiny, Request::Full)).is_ok();
                    WEBP_CODEC.store(if ok { READY } else { MISSING }, Ordering::Release);
                });
            false
        }
        Err(state) => state == READY,
    }
}

/// Native decoding first, WIC for everything else (and for JPEGs the
/// native path rejects, e.g. 12-bit). Lossy WebP stills go to WIC once
/// its codec is loaded (benchmark gate, see `WEBP_CODEC`).
pub fn decode_any(data: &[u8], req: Request) -> Result<Image> {
    decode_any_with(data, req, 1)
}

/// [`decode_any`] with up to `threads` threads for the native decoders that
/// parallelize (JPEG); for the image the user waits for.
pub fn decode_any_with(data: &[u8], req: Request, threads: usize) -> Result<Image> {
    if webp_via_wic(data)
        && let Ok(img) = blinzel_core::decode::guard(|| decode(data, req))
    {
        return Ok(img);
    }
    match blinzel_core::decode::decode_with(data, req, threads) {
        Err(DecodeError::Unsupported(_)) => blinzel_core::decode::guard(|| decode(data, req)),
        r => r,
    }
}

pub fn probe_any(data: &[u8]) -> Result<Probe> {
    match blinzel_core::decode::probe(data) {
        Err(DecodeError::Unsupported(_)) => blinzel_core::decode::guard(|| probe(data)),
        r => r,
    }
}
