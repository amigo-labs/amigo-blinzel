//! Fallback decoder through the Windows Imaging Component: TIFF, and HEIC,
//! AVIF, WebP, JPEG XR, RAW wherever the codecs are installed (Store
//! extensions, camera codec packs). WIC codecs that support scaled decoding
//! (`IWICBitmapSourceTransform`, e.g. many RAW and JPEG codecs) decode
//! directly at the requested size.

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

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

/// Benchmark gate for TIFF: native decoding wins only by decoding strips
/// in parallel; with one thread or one strip WIC is as fast or faster.
fn tiff_via_wic(data: &[u8], threads: usize) -> bool {
    use blinzel_core::format::{Format, detect};
    detect(data) == Format::Tiff && (threads < 2 || blinzel_core::decode::tiff::strips(data) < 2)
}

/// HEIC and AVIF: WIC goes first where Windows has the codec (the "HEVC
/// Video Extensions" and "AV1 Video Extension"; hardware decoding on most
/// GPUs). Without it WIC cannot open the file, and native decoding takes
/// over for the rest of the process (for HEIC only in builds with the
/// `heic` feature, which is off because HEVC is covered by patents). Probe
/// and decode take the same path, since the two may treat the HEIF
/// transformations differently.
static HEIF_WIC_MISSING: AtomicBool = AtomicBool::new(false);
static AVIF_WIC_MISSING: AtomicBool = AtomicBool::new(false);

/// The "codec missing" flag if `data` is HEIC or AVIF and WIC goes first
/// (unless `choice` puts native first).
fn heif_via_wic(data: &[u8], choice: Option<Decoder>) -> Option<&'static AtomicBool> {
    use blinzel_core::format::{Format, detect};
    let missing = match detect(data) {
        Format::Heif => &HEIF_WIC_MISSING,
        Format::Avif => &AVIF_WIC_MISSING,
        _ => return None,
    };
    (choice != Some(Decoder::Native) && !missing.load(Ordering::Relaxed)).then_some(missing)
}

/// The colour profile from the HEIF container (WIC does not hand it out).
fn heif_profile(data: &[u8]) -> Option<blinzel_core::icc::Profile> {
    blinzel_core::decode::avif::container_profile(data)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decoder {
    Native,
    Wic,
}

/// `BLINZEL_DECODER=native` or `=wic`: for benchmark gates, the given
/// decoder goes first wherever both can decode a file.
fn override_() -> Option<Decoder> {
    static D: OnceLock<Option<Decoder>> = OnceLock::new();
    *D.get_or_init(|| decoder_named(std::env::var("BLINZEL_DECODER").ok().as_deref()))
}

/// The decoder a `BLINZEL_DECODER` value names.
fn decoder_named(v: Option<&str>) -> Option<Decoder> {
    match v {
        Some("native") => Some(Decoder::Native),
        Some("wic") => Some(Decoder::Wic),
        _ => None,
    }
}

/// Which decoder goes first for a file.
#[derive(Debug)]
enum Route {
    /// HEIC or AVIF: WIC, and native once WIC has no codec (the flag).
    Heif(&'static AtomicBool),
    /// WIC, native as the fallback; WIC's error if both fail.
    WicFirst,
    /// Native, WIC for what native does not decode.
    NativeFirst,
}

/// The route for `data` decoded with `threads`; `choice` is
/// `BLINZEL_DECODER`.
fn route(data: &[u8], threads: usize, choice: Option<Decoder>) -> Route {
    if let Some(missing) = heif_via_wic(data, choice) {
        return Route::Heif(missing);
    }
    let wic_first = match choice {
        Some(d) => d == Decoder::Wic,
        None => webp_via_wic(data) || tiff_via_wic(data, threads),
    };
    if wic_first {
        Route::WicFirst
    } else {
        Route::NativeFirst
    }
}

/// Decodes along `route` with the two decoders.
fn decode_by(
    route: Route,
    wic: impl FnOnce() -> Result<Image>,
    native: impl FnOnce() -> Result<Image>,
) -> Result<Image> {
    match route {
        Route::Heif(missing) => match wic() {
            Err(DecodeError::Unsupported(_)) => {
                missing.store(true, Ordering::Relaxed);
                native()
            }
            r => r,
        },
        Route::WicFirst => match wic() {
            Ok(img) => Ok(img),
            Err(e) => native().map_err(|_| e),
        },
        Route::NativeFirst => match native() {
            Err(DecodeError::Unsupported(_)) => wic(),
            r => r,
        },
    }
}

/// Native decoding first, WIC for everything else (and for JPEGs the
/// native path rejects, e.g. 12-bit). Where WIC won the benchmark gate it
/// goes first: lossy WebP stills once its codec is loaded (`WEBP_CODEC`),
/// TIFFs that cannot decode in parallel. HEIC and AVIF go to WIC wherever
/// it has a codec (`HEIF_WIC_MISSING`, `AVIF_WIC_MISSING`).
pub fn decode_any(data: &[u8], req: Request) -> Result<Image> {
    decode_any_with(data, req, 1)
}

/// [`decode_any`] with up to `threads` threads for the native decoders that
/// parallelize (JPEG); for the image the user waits for.
pub fn decode_any_with(data: &[u8], req: Request, threads: usize) -> Result<Image> {
    decode_by(
        route(data, threads, override_()),
        || blinzel_core::decode::guard(|| decode(data, req)),
        || blinzel_core::decode::decode_with(data, req, threads),
    )
}

pub fn probe_any(data: &[u8]) -> Result<Probe> {
    probe_by(
        data,
        heif_via_wic(data, override_()),
        || blinzel_core::decode::guard(|| probe(data)),
        || blinzel_core::decode::probe(data),
    )
}

/// Probes `data`: through WIC first for HEIC and AVIF (`heif`, the flag
/// that records a missing codec), else natively and through WIC for what
/// native does not handle.
fn probe_by(
    data: &[u8],
    heif: Option<&'static AtomicBool>,
    wic: impl Fn() -> Result<Probe>,
    native: impl FnOnce() -> Result<Probe>,
) -> Result<Probe> {
    if let Some(missing) = heif {
        match wic() {
            Ok(mut p) => {
                p.icc = heif_profile(data);
                return Ok(p);
            }
            Err(DecodeError::Unsupported(_)) => missing.store(true, Ordering::Relaxed),
            Err(e) => return Err(e),
        }
    }
    match native() {
        Err(DecodeError::Unsupported(_)) => wic(),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_core::format::Format;

    /// An icon with one opaque red 16×16 entry (the smallest usual icon
    /// size), stored as a 32-bit bitmap: the header, the bitmap info with
    /// the height doubled for the mask, the BGRA rows bottom-up, then the
    /// empty AND mask (rows padded to 32 bits).
    fn small_icon() -> Vec<u8> {
        let n = 16u32;
        let mut dib = Vec::new();
        for v in [40, n, 2 * n] {
            dib.extend(v.to_le_bytes());
        }
        dib.extend(1u16.to_le_bytes());
        dib.extend(32u16.to_le_bytes());
        dib.extend([0; 24]);
        dib.extend([0u8, 0, 255, 255].repeat((n * n) as usize));
        dib.extend(vec![0; (n.div_ceil(32) * 4 * n) as usize]);
        let mut ico = vec![0, 0, 1, 0, 1, 0, n as u8, n as u8, 0, 0, 1, 0, 32, 0];
        ico.extend((dib.len() as u32).to_le_bytes());
        ico.extend(22u32.to_le_bytes());
        ico.extend(dib);
        ico
    }

    /// An uncompressed 8-bit RGB TIFF of 4 × 4 pixels in `strips` strips
    /// (1, 2 or 4).
    fn tiff(strips: u32) -> Vec<u8> {
        let (w, h) = (4u32, 4u32);
        let rows = h / strips;
        let entries = 9usize;
        let bits_at = 8 + 2 + 12 * entries + 4;
        let offsets_at = bits_at + 6;
        let counts_at = offsets_at + 4 * strips as usize;
        let pixels_at = counts_at + 4 * strips as usize;
        let strip_len = w * rows * 3;
        let mut d = b"II*\0".to_vec();
        d.extend(8u32.to_le_bytes());
        d.extend((entries as u16).to_le_bytes());
        // Tag, type (3 SHORT, 4 LONG), count, value or offset.
        let many = |at: usize, inline: u32| if strips == 1 { inline } else { at as u32 };
        for (tag, typ, count, value) in [
            (256u16, 3u16, 1u32, w),
            (257, 3, 1, h),
            (258, 3, 3, bits_at as u32),
            (259, 3, 1, 1),
            (262, 3, 1, 2),
            (273, 4, strips, many(offsets_at, pixels_at as u32)),
            (277, 3, 1, 3),
            (278, 3, 1, rows),
            (279, 4, strips, many(counts_at, strip_len)),
        ] {
            d.extend(tag.to_le_bytes());
            d.extend(typ.to_le_bytes());
            d.extend(count.to_le_bytes());
            d.extend(value.to_le_bytes());
        }
        d.extend(0u32.to_le_bytes());
        for _ in 0..3 {
            d.extend(8u16.to_le_bytes());
        }
        for s in 0..strips {
            d.extend((pixels_at as u32 + s * strip_len).to_le_bytes());
        }
        for _ in 0..strips {
            d.extend(strip_len.to_le_bytes());
        }
        d.extend((0..w * h * 3).map(|i| (i * 7) as u8));
        d
    }

    /// A HEIF file's `ftyp` box: enough for the routing.
    const HEIC: &[u8] = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic";
    const AVIF: &[u8] = include_bytes!("../../blinzel-core/testdata/p3-nclx.avif");

    /// A 1 × 1 image whose width tells the decoders apart.
    fn marked(width: u32) -> Result<Image> {
        Ok(Image {
            pixels: Pixels::Packed {
                plane: Plane::new(width, 1, 4),
                format: PackedFormat::Bgra8,
                alpha: Alpha::Opaque,
            },
            width,
            height: 1,
            full_width: width,
            full_height: 1,
            region: None,
        })
    }

    fn packed(img: &Image) -> PackedFormat {
        match &img.pixels {
            Pixels::Packed { format, .. } => *format,
            Pixels::Ycc { .. } => panic!("planes"),
        }
    }

    /// `docs/specs/image-formats.md` AC-21: native first; WIC first for
    /// TIFFs that cannot decode in parallel; `BLINZEL_DECODER` overrides.
    #[test]
    fn native_goes_first_except_where_wic_won_the_gate() {
        let png = blinzel_core::export::encode_png_bgra(&[0, 0, 255, 255], 1, 1).unwrap();
        assert!(matches!(route(&png, 4, None), Route::NativeFirst));
        assert!(matches!(route(&tiff(2), 2, None), Route::NativeFirst));
        assert!(
            matches!(route(&tiff(4), 1, None), Route::WicFirst),
            "one thread"
        );
        assert!(
            matches!(route(&tiff(1), 4, None), Route::WicFirst),
            "one strip"
        );
        // BLINZEL_DECODER puts its decoder first.
        assert_eq!(decoder_named(Some("native")), Some(Decoder::Native));
        assert_eq!(decoder_named(Some("wic")), Some(Decoder::Wic));
        assert_eq!(decoder_named(Some("WIC")), None);
        assert_eq!(decoder_named(None), None);
        let wic = Some(Decoder::Wic);
        assert!(matches!(route(&png, 4, wic), Route::WicFirst));
        let native = Some(Decoder::Native);
        assert!(matches!(route(&tiff(1), 1, native), Route::NativeFirst));
        assert!(matches!(route(HEIC, 1, native), Route::NativeFirst));
        assert!(matches!(route(AVIF, 1, native), Route::NativeFirst));

        // Through the real decoders: native TIFFs come out as RGBA, WIC's
        // as BGRA.
        let img = decode_any_with(&tiff(2), Request::Full, 2).unwrap();
        assert_eq!(packed(&img), PackedFormat::Rgba8);
        for (strips, threads) in [(2, 1), (1, 2)] {
            let img = decode_any_with(&tiff(strips), Request::Full, threads).unwrap();
            assert_eq!(
                packed(&img),
                PackedFormat::Bgra8,
                "{strips} strips, {threads} threads"
            );
            assert_eq!((img.width, img.height), (4, 4));
        }
    }

    /// AC-21: what native does not decode goes to WIC; when WIC goes first
    /// and fails, native is tried, and WIC's error is reported if both fail.
    #[test]
    fn each_decoder_falls_back_to_the_other() {
        let unsupported = || Err(DecodeError::Unsupported(Format::Ico));
        let corrupt = |s: &str| Err(DecodeError::Corrupt(s.into()));
        let width = |r: Result<Image>| r.map(|i| i.width);
        let unused = || -> Result<Image> { panic!("not asked") };
        // Native first.
        assert_eq!(
            width(decode_by(Route::NativeFirst, unused, || marked(2))),
            Ok(2)
        );
        assert_eq!(
            width(decode_by(Route::NativeFirst, || marked(1), unsupported)),
            Ok(1)
        );
        assert_eq!(
            width(decode_by(Route::NativeFirst, unused, || corrupt("native"))),
            Err(DecodeError::Corrupt("native".into())),
            "only \"not decoded natively\" goes to WIC"
        );
        // WIC first.
        assert_eq!(
            width(decode_by(Route::WicFirst, || marked(1), unused)),
            Ok(1)
        );
        assert_eq!(
            width(decode_by(Route::WicFirst, || corrupt("wic"), || marked(2))),
            Ok(2)
        );
        assert_eq!(
            width(decode_by(
                Route::WicFirst,
                || corrupt("wic"),
                || corrupt("native")
            )),
            Err(DecodeError::Corrupt("wic".into()))
        );

        // A real file only WIC reads: an icon with one 16×16 32-bit bitmap.
        let ico = small_icon();
        assert!(matches!(
            blinzel_core::decode::decode(&ico, Request::Full),
            Err(DecodeError::Unsupported(Format::Ico))
        ));
        let img = decode_any_with(&ico, Request::Full, 1).unwrap();
        assert_eq!(
            (img.width, img.height, packed(&img)),
            (16, 16, PackedFormat::Bgra8)
        );
        assert_eq!(&img.to_bgra()[..4], [0, 0, 255, 255]);
        let p = probe_any(&ico).unwrap();
        assert_eq!((p.format, p.width, p.height), (Format::Ico, 16, 16));
    }

    /// AC-21: the first lossy WebP still decodes natively and loads WIC's
    /// codec in the background; once it has loaded, WIC goes first.
    #[test]
    fn the_first_lossy_webp_decodes_natively_and_loads_the_codec() {
        let tiny = &blinzel_core::decode::webp::TINY_LOSSY;
        assert_eq!(WEBP_CODEC.load(Ordering::Acquire), COLD);
        assert!(matches!(route(tiny, 1, None), Route::NativeFirst));
        let t = std::time::Instant::now();
        while WEBP_CODEC.load(Ordering::Acquire) == LOADING {
            assert!(t.elapsed().as_secs() < 30, "the codec never loaded");
            std::thread::yield_now();
        }
        // Without the codec (Windows N, some server images) native stays.
        let loaded = WEBP_CODEC.load(Ordering::Acquire) == READY;
        assert_eq!(matches!(route(tiny, 1, None), Route::WicFirst), loaded);
        assert!(decode_any_with(tiny, Request::Full, 1).is_ok());
    }

    /// AC-22: HEIC and AVIF go to WIC first, the profile comes from the
    /// container, and once WIC cannot open one the format goes native.
    #[test]
    fn heic_and_avif_go_to_wic_until_it_has_no_codec() {
        static MISSING: AtomicBool = AtomicBool::new(false);
        let probed = || {
            Ok(Probe {
                format: Format::Avif,
                width: 64,
                height: 48,
                orientation: Orientation::NORMAL,
                previews: Vec::new(),
                icc: None,
            })
        };
        let no_codec = || Err(DecodeError::Unsupported(Format::Avif));
        let unused = || -> Result<Probe> { panic!("not asked") };
        // WIC's probe, with the container's Display P3 profile.
        let p = probe_by(AVIF, Some(&MISSING), probed, unused).unwrap();
        assert!(p.icc.is_some() && p.icc == heif_profile(AVIF));
        // WIC cannot open it: native, and the flag remembers.
        let p = probe_by(AVIF, Some(&MISSING), no_codec, probed).unwrap();
        assert_eq!(p.width, 64);
        assert!(MISSING.swap(false, Ordering::Relaxed));
        // Another WIC error is the answer.
        let broken = || Err(DecodeError::Corrupt("WIC".into()));
        assert!(probe_by(AVIF, Some(&MISSING), broken, unused).is_err());
        assert!(!MISSING.load(Ordering::Relaxed));
        let width = |r: Result<Image>| r.map(|i| i.width);
        let unused = || -> Result<Image> { panic!("not asked") };
        let no_codec = || Err(DecodeError::Unsupported(Format::Avif));
        assert_eq!(
            width(decode_by(Route::Heif(&MISSING), || marked(1), unused)),
            Ok(1)
        );
        assert!(!MISSING.load(Ordering::Relaxed));
        assert_eq!(
            width(decode_by(Route::Heif(&MISSING), no_codec, || marked(2))),
            Ok(2)
        );
        assert!(MISSING.load(Ordering::Relaxed));

        // The routes, with the process's flags: WIC first while it may
        // have the codec, native for the rest of the process once not.
        assert!(
            matches!(route(HEIC, 4, None), Route::Heif(f) if std::ptr::eq(f, &HEIF_WIC_MISSING))
        );
        assert!(
            matches!(route(AVIF, 4, None), Route::Heif(f) if std::ptr::eq(f, &AVIF_WIC_MISSING))
        );
        AVIF_WIC_MISSING.store(true, Ordering::Relaxed);
        assert!(matches!(route(AVIF, 4, None), Route::NativeFirst));
        assert!(heif_via_wic(AVIF, None).is_none());
        assert!(matches!(route(HEIC, 4, None), Route::Heif(_)), "per format");
        AVIF_WIC_MISSING.store(false, Ordering::Relaxed);
    }
}
