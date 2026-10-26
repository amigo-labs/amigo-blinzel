//! Fallback decoder through ImageIO: TIFF, camera RAW, JPEG XR, PSD and
//! everything else macOS reads. HEIC goes here first: Apple decodes HEVC in
//! hardware and holds the patent licence. Scaled requests use ImageIO's
//! thumbnail path, which decodes RAW and HEIC at reduced size.

use std::ffi::c_void;

use blinzel_core::decode::{DecodeError, Probe, Request, Result};
use blinzel_core::format::{Format, detect};
use blinzel_core::geom::Orientation;
use blinzel_core::icc::Profile;
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};
use objc2_core_foundation::{
    CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGColorSpaceModel, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_image_io::{
    CGImageSource, kCGImagePropertyOrientation, kCGImagePropertyPixelHeight,
    kCGImagePropertyPixelWidth, kCGImageSourceCreateThumbnailFromImageAlways,
    kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceShouldCacheImmediately,
    kCGImageSourceThumbnailMaxPixelSize,
};

fn source(data: &[u8]) -> Result<CFRetained<CGImageSource>> {
    let cf = CFData::from_bytes(data);
    // SAFETY: valid data, no options.
    unsafe { CGImageSource::with_data(&cf, None) }
        .filter(|s| unsafe { s.count() } > 0)
        .ok_or(DecodeError::Unsupported(detect(data)))
}

fn number(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    dict.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}

/// Size and orientation via ImageIO (reads headers only).
pub fn probe(data: &[u8]) -> Result<Probe> {
    let src = source(data)?;
    // SAFETY: index 0 exists (checked in `source`); the properties are a
    // dictionary with string keys.
    let props = unsafe { src.properties_at_index(0, None) }
        .ok_or_else(|| DecodeError::Corrupt("ImageIO: no properties".into()))?;
    let props = unsafe { props.cast_unchecked::<CFString, CFType>() };
    // SAFETY: ImageIO's constant keys.
    let (w, h, o) = unsafe {
        (
            number(props, kCGImagePropertyPixelWidth),
            number(props, kCGImagePropertyPixelHeight),
            number(props, kCGImagePropertyOrientation),
        )
    };
    let (Some(w), Some(h)) = (w, h) else {
        return Err(DecodeError::Corrupt("ImageIO: no size".into()));
    };
    Ok(Probe {
        format: detect(data),
        width: w as u32,
        height: h as u32,
        orientation: o.map_or(Orientation::NORMAL, |o| Orientation::from_exif(o as u16)),
        previews: Vec::new(),
        icc: container_profile(data),
    })
}

/// The colour profile: from the container where blinzel-core reads it
/// (HEIF, AVIF), else what ImageIO reports for the image.
fn container_profile(data: &[u8]) -> Option<Profile> {
    if matches!(detect(data), Format::Heif | Format::Avif) {
        return blinzel_core::decode::avif::container_profile(data);
    }
    let src = source(data).ok()?;
    // SAFETY: index 0 exists.
    let img = unsafe { src.image_at_index(0, None) }?;
    rgb_profile(&img).map(|(_, p)| p)
}

/// The image's colour space and its profile, if it is RGB with one.
fn rgb_profile(img: &CGImage) -> Option<(CFRetained<CGColorSpace>, Profile)> {
    let space = CGImage::color_space(Some(img))?;
    if CGColorSpace::model(Some(&space)) != CGColorSpaceModel::RGB {
        return None;
    }
    let icc = CGColorSpace::icc_data(Some(&space))?.to_vec();
    let p = Profile::new(icc);
    (!p.srgb).then_some((space, p))
}

/// Decodes through ImageIO into straight-alpha BGRA, in the image's own
/// RGB colour space (the viewer colour-manages from its profile).
pub fn decode(data: &[u8], req: Request) -> Result<Image> {
    let src = source(data)?;
    let probe = probe(data)?;
    let (fw, fh) = (probe.width, probe.height);
    blinzel_core::decode::probe_size_ok(fw, fh)?;
    // SAFETY: ImageIO's constant keys with the value types it documents.
    let img = unsafe {
        let yes = CFBoolean::new(true);
        let no = CFBoolean::new(false);
        match req {
            Request::Fit { w, h } if w < fw && h < fh => {
                let edge = CFNumber::new_i64(w.max(h) as i64);
                let opts = CFDictionary::<CFString, CFType>::from_slices(
                    &[
                        kCGImageSourceCreateThumbnailFromImageAlways,
                        kCGImageSourceCreateThumbnailWithTransform,
                        kCGImageSourceShouldCacheImmediately,
                        kCGImageSourceThumbnailMaxPixelSize,
                    ],
                    &[yes.as_ref(), no.as_ref(), yes.as_ref(), edge.as_ref()],
                );
                src.thumbnail_at_index(0, Some(opts.as_ref()))
            }
            _ => {
                let opts = CFDictionary::<CFString, CFType>::from_slices(
                    &[kCGImageSourceShouldCacheImmediately],
                    &[yes.as_ref()],
                );
                src.image_at_index(0, Some(opts.as_ref()))
            }
        }
    }
    .ok_or_else(|| DecodeError::Corrupt("ImageIO: cannot decode".into()))?;
    let (w, h) = (
        CGImage::width(Some(&img)) as u32,
        CGImage::height(Some(&img)) as u32,
    );
    if w == 0 || h == 0 {
        return Err(DecodeError::Corrupt("ImageIO: empty image".into()));
    }
    let space = match rgb_profile(&img) {
        Some((s, _)) => s,
        None => {
            // SAFETY: Core Graphics' constant name.
            CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
                .ok_or_else(|| DecodeError::Corrupt("no sRGB colour space".into()))?
        }
    };
    let mut plane = Plane::new(w, h, 4);
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    {
        // SAFETY: `plane` holds `h` rows of `stride` bytes and outlives the
        // context.
        let ctx: CFRetained<CGContext> = unsafe {
            CGBitmapContextCreate(
                plane.data.as_mut_ptr().cast::<c_void>(),
                w as usize,
                h as usize,
                8,
                plane.stride,
                Some(&space),
                info,
            )
        }
        .ok_or_else(|| DecodeError::Corrupt("no bitmap context".into()))?;
        let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
        CGContext::draw_image(Some(&ctx), rect, Some(&img));
    }
    // Core Graphics premultiplies; the viewer takes straight alpha.
    let mut opaque = true;
    for p in plane.data.as_chunks_mut::<4>().0 {
        let a = p[3];
        if a != 255 {
            opaque = false;
            if a != 0 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                }
            }
        }
    }
    let image = Image {
        pixels: Pixels::Packed {
            plane,
            format: PackedFormat::Bgra8,
            alpha: if opaque {
                Alpha::Opaque
            } else {
                Alpha::Straight
            },
        },
        width: w,
        height: h,
        full_width: fw,
        full_height: fh,
        region: None,
    };
    match req {
        Request::Region(r) => blinzel_core::decode::crop_to_bgra(&image, r),
        _ => Ok(image),
    }
}

/// HEIF to ImageIO first (hardware HEVC), the native decoders first for
/// everything else and ImageIO for what they do not handle.
pub fn decode_any_with(data: &[u8], req: Request, threads: usize) -> Result<Image> {
    let imageio = || blinzel_core::decode::guard(|| decode(data, req));
    if detect(data) == Format::Heif {
        return match imageio() {
            Ok(img) => Ok(img),
            Err(e) => blinzel_core::decode::decode_with(data, req, threads).map_err(|_| e),
        };
    }
    match blinzel_core::decode::decode_with(data, req, threads) {
        Err(DecodeError::Unsupported(_)) => imageio(),
        r => r,
    }
}

pub fn probe_any(data: &[u8]) -> Result<Probe> {
    if detect(data) == Format::Heif
        && let Ok(p) = blinzel_core::decode::guard(|| probe(data))
    {
        return Ok(p);
    }
    match blinzel_core::decode::probe(data) {
        Err(DecodeError::Unsupported(_)) => blinzel_core::decode::guard(|| probe(data)),
        r => r,
    }
}
