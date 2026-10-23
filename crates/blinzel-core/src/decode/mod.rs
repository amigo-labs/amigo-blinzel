//! Format dispatch: probe headers, decode at the resolution that is needed,
//! never let a broken file take the process down.

pub mod bmp;
pub mod gif;
#[cfg(feature = "turbojpeg")]
pub mod jpeg;
pub mod jxl;
pub mod own_jpeg;
pub mod png;
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
}

pub struct Animation {
    pub frames: Vec<AnimFrame>,
    /// How often the whole animation plays; `None` for ever.
    pub plays: Option<u32>,
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
    match format::detect(data) {
        Format::Jpeg => {
            // Benchmark gate: blinzel-jpeg wins with several threads,
            // libjpeg-turbo on one thread and for regions (it skips the
            // rest of the image).
            let own =
                !cfg!(feature = "turbojpeg") || (threads > 1 && !matches!(req, Request::Region(_)));
            if own && let Some(img) = own_jpeg::decode(data, req, threads) {
                return finish(img, req);
            }
            decode_jpeg_fallback(data, req)
        }
        Format::Png => finish(png::decode(data)?, req),
        Format::Gif => finish(gif::decode(data)?, req),
        Format::Bmp => finish(bmp::decode(data)?, req),
        Format::WebP => finish(webp::decode(data)?, req),
        Format::Jxl => finish(jxl::decode(data, threads)?, req),
        other => Err(DecodeError::Unsupported(other)),
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

    #[test]
    fn panics_are_contained() {
        let r: Result<()> = guard(|| panic!("boom"));
        assert_eq!(r, Err(DecodeError::Panicked("boom".into())));
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

    #[test]
    fn size_limits() {
        assert!(check_size(100, 100).is_ok());
        assert!(matches!(check_size(0, 5), Err(DecodeError::Corrupt(_))));
        assert!(matches!(
            check_size(1 << 20, 1 << 20),
            Err(DecodeError::TooLarge { .. })
        ));
    }
}
