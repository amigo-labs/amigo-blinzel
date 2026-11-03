//! Format dispatch: probe headers, decode at the resolution that is needed,
//! never let a broken file take the process down.

pub mod avif;
pub mod bmp;
pub mod gif;
#[cfg(feature = "heic")]
pub mod heif;
#[cfg(feature = "turbojpeg")]
pub mod jpeg;
pub mod jxl;
pub mod own_jpeg;
pub mod png;
pub mod tiff;
pub mod webp;

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::exif::{self, EmbeddedPreview};
use crate::format::{self, Format};
use crate::geom::{Orientation, PixelRect};
use crate::kernels::{self, Level};
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

/// Refuse images larger than this many pixels (decompression bombs).
pub const MAX_PIXELS: u64 = 1 << 30;

/// What resolution to decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Full resolution.
    Full,
    /// At least `w × h` (stored orientation) where the format can decode
    /// smaller cheaply (JPEG DCT scaling); other formats decode fully.
    Fit { w: u32, h: u32 },
    /// A full-resolution rectangle (stored orientation), always as BGRA.
    Region(PixelRect),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not handled by the native decoders; try the platform fallback.
    Unsupported(Format),
    Corrupt(String),
    TooLarge {
        width: u64,
        height: u64,
    },
    /// A decoder panicked; the panic was contained.
    Panicked(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Unsupported(fmt) => write!(f, "{fmt:?} is not decoded natively"),
            DecodeError::Corrupt(s) => write!(f, "corrupt image: {s}"),
            DecodeError::TooLarge { width, height } => {
                write!(f, "image too large ({width}×{height})")
            }
            DecodeError::Panicked(s) => write!(f, "decoder crashed: {s}"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub type Result<T> = std::result::Result<T, DecodeError>;

/// One frame of an animation: the whole canvas after it, and how long it
/// stays.
pub struct AnimFrame {
    pub image: Image,
    pub delay_ms: u32,
    /// How often the whole animation plays, as far as the file has said
    /// up to this frame (`None`: for ever), so a decode error after it
    /// keeps the loop count.
    pub plays: Option<u32>,
}

pub struct Animation {
    pub frames: Vec<AnimFrame>,
    /// How often the whole animation plays; `None` for ever.
    pub plays: Option<u32>,
}

/// What a streamed animation turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimInfo {
    /// Frames handed to the sink (fewer than 2: a still image).
    pub frames: usize,
    /// How often the whole animation plays; `None` for ever.
    pub plays: Option<u32>,
}

/// Receives an animation's frames as they are decoded; returns `false` to
/// stop decoding (the frames so far are kept).
pub type FrameSink<'a> = dyn FnMut(AnimFrame) -> bool + 'a;

/// Collects a streamed animation; `None` for fewer than two frames.
pub(crate) fn collect_animation(
    stream: impl FnOnce(&mut FrameSink) -> Result<AnimInfo>,
) -> Result<Option<Animation>> {
    let mut frames = Vec::new();
    let info = stream(&mut |f| {
        frames.push(f);
        true
    })?;
    Ok((info.frames >= 2).then_some(Animation {
        frames,
        plays: info.plays,
    }))
}

/// Frame durations below 20 ms play at 100 ms, like in browsers (many
/// files say 0).
pub fn min_delay_ms(ms: u32) -> u32 {
    if ms < 20 { 100 } else { ms }
}

/// Header information, available without decoding pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub format: Format,
    /// Stored (not oriented) full-resolution size.
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,
    /// Embedded JPEG previews, largest first.
    pub previews: Vec<EmbeddedPreview>,
    /// Embedded colour profile (none: sRGB).
    pub icc: Option<crate::icc::Profile>,
}

impl Probe {
    pub fn oriented_size(&self) -> (u32, u32) {
        self.orientation.oriented_size(self.width, self.height)
    }

    /// A camera raw with embedded previews: its pixels need the platform
    /// decoder, which develops it in seconds.
    pub fn is_raw(&self) -> bool {
        matches!(self.format, Format::Tiff | Format::Raw) && !self.previews.is_empty()
    }
}

/// Validates dimensions reported by an external decoder.
pub fn probe_size_ok(w: u32, h: u32) -> Result<()> {
    check_size(w as u64, h as u64)
}

pub(crate) fn check_size(w: u64, h: u64) -> Result<()> {
    if w == 0 || h == 0 {
        return Err(DecodeError::Corrupt("zero size".into()));
    }
    if w * h > MAX_PIXELS || w > u32::MAX as u64 || h > u32::MAX as u64 {
        return Err(DecodeError::TooLarge {
            width: w,
            height: h,
        });
    }
    Ok(())
}

/// Reads size, orientation and previews from the first bytes of a file
/// (64 KiB are plenty for JPEG headers of camera files; previews outside
/// the given bytes are not listed).
pub fn probe(data: &[u8]) -> Result<Probe> {
    guard(|| probe_inner(data))
}

fn probe_inner(data: &[u8]) -> Result<Probe> {
    #[cfg(test)]
    tests::panic_if_injected(data);
    let format = format::detect(data);
    let (width, height, orientation, previews, icc) = match format {
        Format::Jpeg => {
            let info =
                blinzel_jpeg::parse(data).map_err(|e| DecodeError::Corrupt(e.to_string()))?;
            let meta = exif::jpeg_meta(data, &info);
            let icc = crate::icc::from_jpeg(data, &info);
            (
                info.width,
                info.height,
                meta.orientation,
                meta.previews,
                icc,
            )
        }
        Format::Png => {
            let (w, h) = png::header_size(data)?;
            let icc = crate::icc::from_png(data);
            (w, h, Orientation::NORMAL, Vec::new(), icc)
        }
        Format::Gif => {
            let (w, h) = gif::header_size(data)?;
            (w, h, Orientation::NORMAL, Vec::new(), None)
        }
        Format::Bmp => {
            let h = bmp::Header::parse(data)?;
            (h.width, h.height, Orientation::NORMAL, Vec::new(), None)
        }
        Format::WebP => {
            let ((w, h), orientation, icc) = webp::info(data)?;
            (w, h, orientation, Vec::new(), icc)
        }
        Format::Jxl => {
            let ((w, h), orientation, icc) = jxl::info(data)?;
            (w, h, orientation, Vec::new(), icc)
        }
        #[cfg(feature = "heic")]
        Format::Heif => {
            let ((w, h), orientation, icc) = heif::info(data)?;
            (w, h, orientation, Vec::new(), icc)
        }
        Format::Avif => {
            let ((w, h), orientation, icc) = avif::info(data)?;
            (w, h, orientation, Vec::new(), icc)
        }
        Format::Tiff => match tiff::info(data) {
            Ok(((w, h), orientation, icc)) => (w, h, orientation, Vec::new(), icc),
            Err(DecodeError::Unsupported(_)) => raw_probe(data, format)?,
            Err(e) => return Err(e),
        },
        Format::Raw => raw_probe(data, format)?,
        other => return Err(DecodeError::Unsupported(other)),
    };
    check_size(width as u64, height as u64)?;
    Ok(Probe {
        format,
        width,
        height,
        orientation,
        previews,
        icc,
    })
}

/// Width, height, orientation, previews and profile, as probes read them.
type Header = (
    u32,
    u32,
    Orientation,
    Vec<EmbeddedPreview>,
    Option<crate::icc::Profile>,
);

/// Camera raws: size, orientation and embedded previews read natively; the
/// sensor data stays with the platform decoder. Without previews the
/// platform probes too.
fn raw_probe(data: &[u8], format: Format) -> Result<Header> {
    let info = crate::raw::probe(data).ok_or(DecodeError::Unsupported(format))?;
    let (w, h) = info.image_size().ok_or(DecodeError::Unsupported(format))?;
    Ok((w, h, info.orientation, info.previews, None))
}

/// Decodes `data` (a whole file) as requested on the calling thread.
/// Panics inside decoders are caught and reported as
/// [`DecodeError::Panicked`].
pub fn decode(data: &[u8], req: Request) -> Result<Image> {
    decode_with(data, req, 1)
}

/// Like [`decode`], with up to `threads` threads for formats that decode in
/// parallel (JPEG). Use it for the image the user waits for.
pub fn decode_with(data: &[u8], req: Request, threads: usize) -> Result<Image> {
    guard(|| decode_inner(data, req, threads))
}

fn decode_inner(data: &[u8], req: Request, threads: usize) -> Result<Image> {
    #[cfg(test)]
    tests::panic_if_injected(data);
    match format::detect(data) {
        Format::Jpeg => decode_jpeg(data, req, threads, cfg!(feature = "turbojpeg")),
        Format::Png => finish(png::decode(data)?, req),
        Format::Gif => finish(gif::decode(data)?, req),
        Format::Bmp => finish(bmp::decode(data)?, req),
        Format::WebP => finish(webp::decode(data)?, req),
        Format::Jxl => finish(jxl::decode(data, threads)?, req),
        #[cfg(feature = "heic")]
        Format::Heif => finish(heif::decode(data, threads)?, req),
        Format::Avif => finish(avif::decode(data, threads)?, req),
        Format::Tiff => finish(tiff::decode(data, threads)?, req),
        other => Err(DecodeError::Unsupported(other)),
    }
}

/// The JPEG decoder choice (docs/specs/jpeg-decoder.md); `turbo` says
/// whether the build has libjpeg-turbo.
fn decode_jpeg(data: &[u8], req: Request, threads: usize, turbo: bool) -> Result<Image> {
    // Benchmark gate: blinzel-jpeg wins with several threads, libjpeg-turbo
    // on one thread and for regions (it skips the rest of the image).
    let own = !turbo || (threads > 1 && !matches!(req, Request::Region(_)));
    if own && let Some(img) = own_jpeg::decode(data, req, threads) {
        return finish(img, req);
    }
    if turbo {
        decode_jpeg_fallback(data, req)
    } else {
        Err(DecodeError::Unsupported(Format::Jpeg))
    }
}

#[cfg(feature = "turbojpeg")]
fn decode_jpeg_fallback(data: &[u8], req: Request) -> Result<Image> {
    jpeg::decode(data, req)
}

#[cfg(not(feature = "turbojpeg"))]
fn decode_jpeg_fallback(_: &[u8], _: Request) -> Result<Image> {
    Err(DecodeError::Unsupported(Format::Jpeg))
}

/// Decodes an embedded preview and labels it as a reduced stage of the main
/// image (`full` is the main image's stored size).
pub fn decode_preview(
    data: &[u8],
    preview: &EmbeddedPreview,
    full: (u32, u32),
    req: Request,
) -> Result<Image> {
    let bytes = data
        .get(preview.range.clone())
        .ok_or_else(|| DecodeError::Corrupt("preview outside file".into()))?;
    let req = match req {
        Request::Region(_) => Request::Full,
        r => r,
    };
    let mut img = decode(bytes, req)?;
    img.full_width = full.0;
    img.full_height = full.1;
    Ok(img)
}

/// Applies a region request to a fully decoded image.
fn finish(img: Image, req: Request) -> Result<Image> {
    match req {
        Request::Region(r) => crop_to_bgra(&img, r),
        _ => Ok(img),
    }
}

/// Copies `r` (full-resolution coordinates of a full-resolution image) into
/// a new BGRA image.
pub fn crop_to_bgra(img: &Image, r: PixelRect) -> Result<Image> {
    if r.right() > img.width || r.bottom() > img.height || r.w == 0 || r.h == 0 {
        return Err(DecodeError::Corrupt(format!(
            "region {r:?} outside {}×{}",
            img.width, img.height
        )));
    }
    let mut out = Plane::new(r.w, r.h, 4);
    let alpha = match &img.pixels {
        Pixels::Packed {
            plane,
            format,
            alpha,
        } => {
            let bpp = format.bytes_per_pixel();
            for y in 0..r.h {
                let src = &plane.row(r.y + y)[r.x as usize * bpp..r.right() as usize * bpp];
                let dst = out.row_mut(y);
                match format {
                    PackedFormat::Bgra8 => dst.copy_from_slice(src),
                    PackedFormat::Rgba8 => kernels::rgba_to_bgra(Level::Baseline, src, dst),
                    PackedFormat::Gray8 => {
                        for (d, &s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                            d.copy_from_slice(&[s, s, s, 255]);
                        }
                    }
                }
                if *alpha == Alpha::Opaque {
                    for px in dst.as_chunks_mut::<4>().0.iter_mut() {
                        px[3] = 255;
                    }
                }
            }
            *alpha
        }
        Pixels::Ycc { .. } => {
            let all = img.to_bgra();
            let w = img.width as usize;
            for y in 0..r.h as usize {
                let s = ((r.y as usize + y) * w + r.x as usize) * 4;
                out.row_mut(y as u32)
                    .copy_from_slice(&all[s..s + r.w as usize * 4]);
            }
            Alpha::Opaque
        }
    };
    Ok(Image {
        pixels: Pixels::Packed {
            plane: out,
            format: PackedFormat::Bgra8,
            alpha,
        },
        width: r.w,
        height: r.h,
        full_width: img.full_width,
        full_height: img.full_height,
        region: Some(r),
    })
}

/// Runs `f`, turning a panic into an error.
pub fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(DecodeError::Panicked(msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fault injection: probing or decoding these bytes panics where the
    /// decoders run, inside the guard (docs/specs/image-formats.md, Decisions).
    const PANICS: &[u8] = b"blinzel test: the decoder panics";

    pub(super) fn panic_if_injected(data: &[u8]) {
        if data == PANICS {
            panic!("injected decoder panic");
        }
    }

    #[test]
    fn panics_are_contained() {
        let r: Result<()> = guard(|| panic!("boom"));
        assert_eq!(r, Err(DecodeError::Panicked("boom".into())));
    }

    #[test]
    fn a_panicking_probe_or_decode_reports_decoder_crashed() {
        let crashed = Err(DecodeError::Panicked("injected decoder panic".into()));
        assert_eq!(probe(PANICS).map(|_| ()), crashed);
        assert_eq!(decode(PANICS, Request::Full).map(|_| ()), crashed);
        assert_eq!(decode_with(PANICS, Request::Full, 4).map(|_| ()), crashed);
        assert_eq!(
            DecodeError::Panicked("injected decoder panic".into()).to_string(),
            "decoder crashed: injected decoder panic"
        );
    }

    #[test]
    fn unknown_formats_are_unsupported() {
        assert!(matches!(
            decode(b"II*\0\x08\0\0\0", Request::Full),
            Err(DecodeError::Unsupported(Format::Tiff))
        ));
        assert!(matches!(
            probe(b"nothing"),
            Err(DecodeError::Unsupported(Format::Unknown))
        ));
    }

    /// Without the `heic` feature HEIC is left to the platform decoder; the
    /// colour profile still comes from the container.
    #[cfg(not(feature = "heic"))]
    #[test]
    fn heic_needs_the_feature() {
        let data = include_bytes!("../../testdata/rgb-64x48.heic");
        assert!(matches!(
            decode(data, Request::Full),
            Err(DecodeError::Unsupported(Format::Heif))
        ));
        assert!(matches!(
            probe(data),
            Err(DecodeError::Unsupported(Format::Heif))
        ));
        assert!(avif::container_profile(data).is_none());
    }

    /// A JPEG header the parser accepts: SOF `marker` with `precision`
    /// bits, three components, and an empty scan.
    fn jpeg_header(w: u16, h: u16, precision: u8, marker: u8) -> Vec<u8> {
        let mut d = vec![0xFF, 0xD8, 0xFF, marker, 0, 17, precision];
        d.extend(h.to_be_bytes());
        d.extend(w.to_be_bytes());
        d.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        d.extend([0xFF, 0xDA, 0, 12, 3, 1, 0, 2, 0x11, 3, 0x11, 0, 63, 0]);
        d.extend([0xFF, 0xD9]);
        d
    }

    /// `marker` segment (APPn) with `payload`, inserted after the SOI.
    fn with_segment(jpeg: &[u8], marker: u8, payload: &[u8]) -> Vec<u8> {
        let len = (payload.len() + 2) as u16;
        [
            &jpeg[..2],
            &[0xFF, marker],
            &len.to_be_bytes(),
            payload,
            &jpeg[2..],
        ]
        .concat()
    }

    fn unsupported<T>(r: Result<T>) -> Option<Format> {
        match r {
            Err(DecodeError::Unsupported(f)) => Some(f),
            _ => None,
        }
    }

    /// Formats without a native decoder, 12-bit JPEGs, raw pixels and raws
    /// without previews all go to the system decoder (AC-03).
    #[test]
    fn other_formats_and_raw_pixels_go_to_the_platform() {
        let signatures: [(&[u8], Format); 7] = [
            (b"II\xbc\x01\0\0\0\0", Format::JpegXr),
            (b"II\xbc\x00\0\0\0\0", Format::JpegXr),
            (&[0, 0, 1, 0, 1, 0, 16, 16], Format::Ico),
            (&[0, 0, 2, 0, 1, 0, 16, 16], Format::Ico),
            (b"DDS |\0\0\0", Format::Dds),
            (b"8BPS\0\x01\0\0", Format::Psd),
            (b"qoif\0\0\0\x04", Format::Qoi),
        ];
        for (data, f) in signatures {
            assert_eq!(format::detect(data), f);
            assert_eq!(unsupported(probe(data)), Some(f));
            assert_eq!(unsupported(decode(data, Request::Full)), Some(f));
        }
        let twelve = jpeg_header(16, 8, 12, 0xC1);
        for threads in [1, 4] {
            let r = decode_with(&twelve, Request::Full, threads);
            assert_eq!(unsupported(r), Some(Format::Jpeg));
        }
        // TIFF-based raws without previews: sub-IFDs, DNG, Canon CR2.
        use ::tiff::encoder::{TiffEncoder, colortype};
        use ::tiff::tags::Tag;
        let tiff = |tag: Option<(u16, u32)>| {
            let mut out = std::io::Cursor::new(Vec::new());
            let mut enc = TiffEncoder::new(&mut out).unwrap();
            let mut img = enc.new_image::<colortype::RGB8>(4, 3).unwrap();
            if let Some((t, v)) = tag {
                img.encoder().write_tag(Tag::Unknown(t), v).unwrap();
            }
            img.write_data(&crate::testimg::rgb(4, 3)).unwrap();
            out.into_inner()
        };
        assert!(decode(&tiff(None), Request::Full).is_ok());
        let mut cr2 = tiff(None);
        cr2[8..10].copy_from_slice(b"CR");
        for data in [tiff(Some((330, 8))), tiff(Some((50706, 0x0104_0000))), cr2] {
            assert_eq!(unsupported(probe(&data)), Some(Format::Tiff));
            assert_eq!(
                unsupported(decode(&data, Request::Full)),
                Some(Format::Tiff)
            );
        }
        // Other raw containers without previews.
        for data in [
            &b"FUJIFILMCCD-RAW 0201"[..],
            b"IIRO\x08\0\0\0\0\0",
            b"\0\0\0\x18ftypcrx \0\0",
        ] {
            assert_eq!(unsupported(probe(data)), Some(Format::Raw));
            assert_eq!(unsupported(decode(data, Request::Full)), Some(Format::Raw));
        }
        // A RAF that knows its size (record 0x111) but has no preview: the
        // size alone is no image, the system decoder gets it.
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(100, 0);
        raf[92..96].copy_from_slice(&100u32.to_be_bytes());
        raf.extend(1u32.to_be_bytes());
        raf.extend([0x01, 0x11, 0, 4]);
        raf.extend(4160u16.to_be_bytes());
        raf.extend(6240u16.to_be_bytes());
        let info = crate::raw::probe(&raf).unwrap();
        assert_eq!((info.size, info.previews.len()), (Some((6240, 4160)), 0));
        assert_eq!(unsupported(probe(&raf)), Some(Format::Raw));
        assert_eq!(unsupported(decode(&raf, Request::Full)), Some(Format::Raw));
    }

    /// JPEG (EXIF), WebP (`webp.rs`) and JPEG XL (header) carry an
    /// orientation; PNG, GIF and BMP are upright even with EXIF (AC-17).
    #[test]
    fn probe_reports_the_orientation() {
        let tiff = crate::exif::tests::tiff_le(6, None);
        let exif = [b"Exif\0\0".as_slice(), &tiff].concat();
        let jpeg = with_segment(&jpeg_header(16, 8, 8, 0xC0), 0xE1, &exif);
        let p = probe(&jpeg).unwrap();
        assert_eq!((p.width, p.height), (16, 8));
        assert_eq!(p.orientation, Orientation::from_exif(6));
        assert_eq!(p.oriented_size(), (8, 16));

        // Every EXIF orientation, in either byte order; others are upright.
        for v in 1..=9u16 {
            let o = if v <= 8 {
                Orientation::from_exif(v)
            } else {
                Orientation::NORMAL
            };
            let le = crate::exif::tests::tiff_le(v, None);
            let mut be = b"MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".to_vec();
            be.extend([0, v as u8, 0, 0, 0, 0, 0, 0]);
            for tiff in [le, be] {
                let exif = [b"Exif\0\0".as_slice(), &tiff].concat();
                let jpeg = with_segment(&jpeg_header(16, 8, 8, 0xC0), 0xE1, &exif);
                assert_eq!(probe(&jpeg).unwrap().orientation, o, "{v}: {tiff:x?}");
            }
        }

        // With an EXIF thumbnail in IFD1, which may carry an orientation of
        // its own (here 3), the orientation is still IFD0's.
        let thumb = jpeg_header(16, 8, 8, 0xC0);
        for ifd1 in [Some(3), None] {
            let tiff = crate::exif::tests::tiff_le_with_ifd1(6, ifd1, &thumb);
            let exif = [b"Exif\0\0".as_slice(), &tiff].concat();
            let jpeg = with_segment(&jpeg_header(64, 32, 8, 0xC0), 0xE1, &exif);
            let p = probe(&jpeg).unwrap();
            assert_eq!(p.orientation, Orientation::from_exif(6), "IFD1 {ifd1:?}");
            let sizes: Vec<_> = p.previews.iter().map(|v| (v.width, v.height)).collect();
            assert_eq!(sizes, [(16, 8)], "the thumbnail in IFD1 is read");
        }

        let jxl = probe(include_bytes!("../../testdata/orient6-4x2.jxl")).unwrap();
        assert_eq!((jxl.format, jxl.width, jxl.height), (Format::Jxl, 4, 2));
        assert_eq!(jxl.orientation, Orientation::from_exif(6));

        let mut png = Vec::new();
        {
            let mut e = ::png::Encoder::new(&mut png, 2, 1);
            e.set_color(::png::ColorType::Rgb);
            let mut w = e.write_header().unwrap();
            w.write_chunk(::png::chunk::ChunkType(*b"eXIf"), &tiff)
                .unwrap();
            w.write_image_data(&[0; 6]).unwrap();
        }
        let mut gif = Vec::new();
        ::gif::Encoder::new(&mut gif, 2, 1, &[0; 6])
            .unwrap()
            .write_frame(&::gif::Frame::from_indexed_pixels(2, 1, vec![0, 1], None))
            .unwrap();
        let mut bmp = b"BM".to_vec();
        bmp.extend(
            [62u32, 0, 54, 40, 2, 1]
                .iter()
                .flat_map(|v| v.to_le_bytes()),
        );
        bmp.extend(1u16.to_le_bytes());
        bmp.extend(24u16.to_le_bytes());
        bmp.extend([0u8; 24 + 8]);
        for (data, f) in [(png, Format::Png), (gif, Format::Gif), (bmp, Format::Bmp)] {
            let p = probe(&data).unwrap();
            assert_eq!((p.format, p.width, p.height), (f, 2, 1));
            assert_eq!(p.orientation, Orientation::NORMAL);
        }
    }

    /// The probe lists a JPEG's EXIF thumbnail and MPF images inside the
    /// given bytes that are 8-bit lossy JPEGs, largest first, each once; a
    /// decoded preview is a reduced stage of the main image (AC-19).
    #[cfg(feature = "turbojpeg")]
    #[test]
    fn jpeg_previews_from_exif_and_mpf() {
        let enc = |w, h| jpeg::encode_rgb(&crate::testimg::rgb(w, h), w, h, Default::default());
        let (main, thumb, big) = (enc(320, 240).unwrap(), enc(32, 24).unwrap(), enc(160, 120));
        let big = big.unwrap();
        let (twelve, lossless) = (
            jpeg_header(200, 150, 12, 0xC1),
            jpeg_header(300, 200, 8, 0xC3),
        );
        // APP1: the thumbnail right after IFD1 (56 bytes of TIFF).
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(crate::exif::tests::tiff_le(
            1,
            Some((56, thumb.len() as u32)),
        ));
        app1.extend(&thumb);
        // APP2: MPF index with five images after the primary one.
        let n = 6;
        let mpf_len = 4 + 26 + 16 * n;
        let mpf_at = 2 + 4 + app1.len() + 4 + 4; // TIFF header inside APP2
        let tail_at = 2 + 4 + app1.len() + 4 + mpf_len + main.len() - 2;
        let at = |i: usize| (tail_at + [0, big.len(), big.len() + twelve.len()][i] - mpf_at) as u32;
        let total = (tail_at + big.len() + twelve.len() + lossless.len() - mpf_at) as u32;
        let entries = [
            (0x03_0000, main.len() as u32, 0),
            (0x01_0002, big.len() as u32, at(0)),
            (0x01_0002, big.len() as u32, at(0)),
            (0x01_0002, twelve.len() as u32, at(1)),
            (0x01_0002, lossless.len() as u32, at(2)),
            (0x01_0002, 100, total),
        ];
        // The index in either byte order.
        let index = |le: bool| {
            let u16b = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
            let u32b = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
            let mut mpf = if le { b"MPF\0II*\0" } else { b"MPF\0MM\0*" }.to_vec();
            mpf.extend(u32b(8));
            mpf.extend(u16b(1));
            mpf.extend(u16b(0xB002));
            mpf.extend(u16b(7));
            mpf.extend(u32b(16 * n as u32));
            mpf.extend(u32b(26));
            mpf.extend(u32b(0));
            for (attr, size, off) in entries {
                mpf.extend([attr, size, off, 0u32].iter().flat_map(|&v| u32b(v)));
            }
            assert_eq!(mpf.len(), mpf_len);
            let data = with_segment(&with_segment(&main, 0xE2, &mpf), 0xE1, &app1);
            [data, big.clone(), twelve.clone(), lossless.clone()].concat()
        };
        let data = index(true);
        assert_eq!(&data[tail_at..tail_at + big.len()], &big[..]);

        let p = probe(&data).unwrap();
        assert_eq!((p.width, p.height), (320, 240));
        let sizes: Vec<_> = p.previews.iter().map(|v| (v.width, v.height)).collect();
        assert_eq!(sizes, [(160, 120), (32, 24)]);
        assert_eq!(p.previews[0].range, tail_at..tail_at + big.len());
        // Previews beyond the given bytes are not listed.
        let head = probe(&data[..tail_at]).unwrap();
        let sizes: Vec<_> = head.previews.iter().map(|v| (v.width, v.height)).collect();
        assert_eq!(sizes, [(32, 24)]);

        for req in [Request::Full, Request::Region(PixelRect::new(0, 0, 8, 8))] {
            let img = decode_preview(&data, &p.previews[0], (320, 240), req).unwrap();
            assert_eq!((img.width, img.height), (160, 120));
            assert_eq!((img.full_width, img.full_height), (320, 240));
        }
        // A big-endian index lists the same.
        let be = probe(&index(false)).unwrap();
        assert_eq!(be.previews, p.previews);
    }

    #[test]
    fn size_limits() {
        assert!(check_size(100, 100).is_ok());
        assert!(matches!(check_size(0, 5), Err(DecodeError::Corrupt(_))));
        assert!(matches!(check_size(5, 0), Err(DecodeError::Corrupt(_))));
        assert!(matches!(
            check_size(1 << 20, 1 << 20),
            Err(DecodeError::TooLarge { .. })
        ));
        // Exactly at the limit: 2^30 pixels are fine, one more is too many.
        assert!(check_size(1 << 15, 1 << 15).is_ok());
        assert!(
            check_size((1 << 30) + 1, 1).is_err_and(|e| matches!(e, DecodeError::TooLarge { .. }))
        );
        // The probe checks every format's header size, here a PNG's, whose
        // own header reader does not.
        let png = |w: u32, h: u32| {
            let mut d = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            d.extend(w.to_be_bytes());
            d.extend(h.to_be_bytes());
            d.extend([8, 2, 0, 0, 0, 0, 0, 0, 0]);
            d
        };
        assert!(probe(&png(1 << 15, 1 << 15)).is_ok());
        assert!(matches!(probe(&png(0, 4)), Err(DecodeError::Corrupt(_))));
        assert!(matches!(probe(&png(4, 0)), Err(DecodeError::Corrupt(_))));
        assert!(matches!(
            probe(&png((1 << 15) + 1, 1 << 15)),
            Err(DecodeError::TooLarge { .. })
        ));
    }

    /// The four errors read as AC-02 names them.
    #[test]
    fn errors_say_what_happened() {
        let (width, height) = (7, 9);
        for (e, says) in [
            (
                DecodeError::Unsupported(Format::Psd),
                "Psd is not decoded natively",
            ),
            (
                DecodeError::Corrupt("bad IHDR".into()),
                "corrupt image: bad IHDR",
            ),
            (
                DecodeError::TooLarge { width, height },
                "image too large (7×9)",
            ),
            (
                DecodeError::Panicked("boom".into()),
                "decoder crashed: boom",
            ),
        ] {
            assert_eq!(e.to_string(), says);
        }
    }

    /// A 1600 × 1200 4:2:0 baseline JPEG of the test pattern.
    #[cfg(feature = "turbojpeg")]
    fn jpeg_1600() -> Vec<u8> {
        let rgb = crate::testimg::rgb(1600, 1200);
        jpeg::encode_rgb(&rgb, 1600, 1200, Default::default()).unwrap()
    }

    /// libjpeg-turbo scales in eighths, blinzel-jpeg only to 1/8, 1/4, 1/2
    /// and 8/8: at least 401 pixels of 1600 are 3/8 with one, 1/2 with the
    /// other, so the size tells which decoder ran.
    #[cfg(feature = "turbojpeg")]
    const FIT_3_8: Request = Request::Fit { w: 401, h: 10 };

    /// Both failed alike, or both are images of the same size, kind and
    /// pixels.
    #[cfg(feature = "turbojpeg")]
    fn same(a: &Result<Image>, b: &Result<Image>) -> bool {
        match (a, b) {
            (Ok(a), Ok(b)) => {
                (a.width, a.height, a.full_width, a.full_height, a.region)
                    == (b.width, b.height, b.full_width, b.full_height, b.region)
                    && std::mem::discriminant(&a.pixels) == std::mem::discriminant(&b.pixels)
                    && a.to_bgra() == b.to_bgra()
            }
            (Err(a), Err(b)) => a == b,
            _ => false,
        }
    }

    /// One thread decodes with libjpeg-turbo at its DCT scales in eighths,
    /// a region is cropped by libjpeg-turbo with any number of threads
    /// (AC-13).
    #[cfg(feature = "turbojpeg")]
    #[test]
    fn one_thread_and_regions_decode_with_libjpeg_turbo() {
        let data = jpeg_1600();
        for (req, size) in [
            (FIT_3_8, (600, 450)),
            (Request::Fit { w: 801, h: 10 }, (1000, 750)),
            (Request::Fit { w: 1201, h: 10 }, (1400, 1050)),
            (Request::Full, (1600, 1200)),
        ] {
            let img = decode_with(&data, req, 1).unwrap();
            assert_eq!((img.width, img.height), size, "{req:?}");
            assert!(same(&Ok(img), &jpeg::decode(&data, req)), "{req:?}");
        }
        // Several threads: blinzel-jpeg, at its next larger scale.
        let img = decode_with(&data, FIT_3_8, 4).unwrap();
        assert_eq!((img.width, img.height), (800, 600));

        let own_full = own_jpeg::decode(&data, Request::Full, 1).unwrap();
        for r in [
            PixelRect::new(37, 11, 100, 50),
            PixelRect::new(1500, 1100, 100, 100),
        ] {
            let turbo = jpeg::decode(&data, Request::Region(r));
            // The two crops differ, so the result tells which decoder ran.
            assert!(!same(&crop_to_bgra(&own_full, r), &turbo), "{r:?}");
            for threads in [1, 4] {
                let img = decode_with(&data, Request::Region(r), threads);
                assert!(same(&img, &turbo), "{r:?} with {threads} threads");
            }
        }
    }

    /// `data` with a run of `FF 00` (stuffed one bits, an invalid Huffman
    /// code) written over the middle of its entropy-coded data.
    #[cfg(feature = "turbojpeg")]
    fn with_corrupt_entropy(data: &[u8]) -> Vec<u8> {
        let sos = data.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        let mut d = data.to_vec();
        let mut at = sos + (data.len() - sos) / 2;
        while d[at - 1] == 0xFF || d[at] == 0x00 {
            at += 1;
        }
        for pair in d[at..at + 64].chunks_mut(2) {
            pair.copy_from_slice(&[0xFF, 0x00]);
        }
        d
    }

    /// What blinzel-jpeg declines with several threads goes to
    /// libjpeg-turbo, which decodes it or reports its own error (AC-14).
    #[cfg(feature = "turbojpeg")]
    #[test]
    fn files_blinzel_jpeg_declines_go_to_libjpeg_turbo() {
        use jpeg::{Unusual, encode_unusual};
        let base = jpeg::encode_rgb(&crate::testimg::rgb(96, 64), 96, 64, Default::default());
        let cases = [
            ("CMYK", encode_unusual(Unusual::Cmyk, 96, 64), true),
            ("RGB", encode_unusual(Unusual::Rgb, 96, 64), true),
            ("4:1:1", encode_unusual(Unusual::S411, 96, 64), true),
            ("too large", jpeg_header(32768, 32769, 8, 0xC0), false),
            ("corrupt", with_corrupt_entropy(&base.unwrap()), true),
        ];
        for (name, data, decodes) in cases {
            for req in [Request::Full, Request::Fit { w: 20, h: 10 }] {
                assert!(own_jpeg::decode(&data, req, 4).is_none(), "{name}");
                let turbo = jpeg::decode(&data, req);
                assert_eq!(turbo.is_ok(), decodes, "{name} {req:?}: {:?}", turbo.err());
                let img = decode_with(&data, req, 4);
                assert!(same(&img, &turbo), "{name} {req:?}: {:?}", img.err());
            }
        }
    }

    /// Without libjpeg-turbo blinzel-jpeg decodes every JPEG it supports
    /// on any number of threads, a region at full size and cropped, and
    /// declines the rest as not decoded natively (AC-15).
    #[cfg(feature = "turbojpeg")]
    #[test]
    fn without_libjpeg_turbo_blinzel_jpeg_decodes_alone() {
        use jpeg::{Unusual, encode_unusual};
        let data = jpeg_1600();
        let own_full = own_jpeg::decode(&data, Request::Full, 1).unwrap();
        let r = PixelRect::new(37, 11, 100, 50);
        let declined =
            [Unusual::Cmyk, Unusual::Rgb, Unusual::S411].map(|k| encode_unusual(k, 96, 64));
        for threads in [1, 4] {
            let img = decode_jpeg(&data, FIT_3_8, threads, false).unwrap();
            assert_eq!((img.width, img.height), (800, 600), "{threads} threads");
            let img = decode_jpeg(&data, Request::Full, threads, false);
            assert!(same(
                &img,
                &Ok(own_jpeg::decode(&data, Request::Full, 1).unwrap())
            ));
            let img = decode_jpeg(&data, Request::Region(r), threads, false);
            assert!(same(&img, &crop_to_bgra(&own_full, r)), "{threads} threads");
            for d in &declined {
                let img = decode_jpeg(d, Request::Full, threads, false);
                assert_eq!(unsupported(img), Some(Format::Jpeg));
            }
        }
    }
}
