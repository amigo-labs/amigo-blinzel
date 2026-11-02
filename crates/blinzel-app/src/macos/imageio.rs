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
    in_order(
        detect(data) == Format::Heif,
        || blinzel_core::decode::guard(|| decode(data, req)),
        || blinzel_core::decode::decode_with(data, req, threads),
    )
}

pub fn probe_any(data: &[u8]) -> Result<Probe> {
    in_order(
        detect(data) == Format::Heif,
        || blinzel_core::decode::guard(|| probe(data)),
        || blinzel_core::decode::probe(data),
    )
}

/// The order of the two decoders: for HEIF (`heif`) `imageio` first and
/// `native` only if it fails, ImageIO's error if both do; else `native`
/// first and `imageio` when it reports "not decoded natively".
fn in_order<T>(
    heif: bool,
    imageio: impl FnOnce() -> Result<T>,
    native: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if heif {
        return match imageio() {
            Ok(v) => Ok(v),
            Err(e) => native().map_err(|_| e),
        };
    }
    match native() {
        Err(DecodeError::Unsupported(_)) => imageio(),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An icon with one opaque red 2×2 entry, stored as a 32-bit bitmap:
    /// the header, the bitmap info with the height doubled for the mask,
    /// the BGRA rows bottom-up, then the empty AND mask.
    fn tiny_icon() -> Vec<u8> {
        let mut dib = Vec::new();
        for v in [40u32, 2, 4] {
            dib.extend(v.to_le_bytes());
        }
        dib.extend(1u16.to_le_bytes());
        dib.extend(32u16.to_le_bytes());
        dib.extend([0; 24]);
        dib.extend([0u8, 0, 255, 255].repeat(4));
        dib.extend([0; 8]);
        let mut ico = vec![0, 0, 1, 0, 1, 0, 2, 2, 0, 0, 1, 0, 32, 0];
        ico.extend((dib.len() as u32).to_le_bytes());
        ico.extend(22u32.to_le_bytes());
        ico.extend(dib);
        ico
    }

    const HEIC: &[u8] = include_bytes!("../../../blinzel-core/testdata/rgb-64x48.heic");

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

    fn width(r: Result<Image>) -> Result<u32> {
        r.map(|i| i.width)
    }

    fn corrupt(s: &str) -> Result<Image> {
        Err(DecodeError::Corrupt(s.into()))
    }

    fn unused() -> Result<Image> {
        panic!("not asked")
    }

    /// `docs/specs/image-formats.md` AC-23: HEIC goes to ImageIO first and
    /// to the native decoder only if ImageIO fails.
    #[test]
    fn heic_goes_to_imageio_first_and_native_only_if_it_fails() {
        assert_eq!(width(in_order(true, || marked(1), unused)), Ok(1));
        assert_eq!(
            width(in_order(true, || corrupt("imageio"), || marked(2))),
            Ok(2)
        );
        assert_eq!(
            width(in_order(true, || corrupt("imageio"), || corrupt("native"))),
            Err(DecodeError::Corrupt("imageio".into())),
            "ImageIO's error if both fail"
        );

        // The real file: this job builds without `heic`, so only ImageIO
        // reads it, into BGRA.
        let img = decode_any_with(HEIC, Request::Full, 1).unwrap();
        assert_eq!(
            (img.width, img.height, packed(&img)),
            (64, 48, PackedFormat::Bgra8)
        );
        let p = probe_any(HEIC).unwrap();
        assert_eq!((p.format, p.width, p.height), (Format::Heif, 64, 48));
    }

    /// AC-23: every other file goes to the native decoders first and to
    /// ImageIO when they report "not decoded natively".
    #[test]
    fn other_files_go_native_first_and_to_imageio_when_not_native() {
        let unsupported = || Err(DecodeError::Unsupported(Format::Ico));
        assert_eq!(width(in_order(false, unused, || marked(2))), Ok(2));
        assert_eq!(width(in_order(false, || marked(1), unsupported)), Ok(1));
        assert_eq!(
            width(in_order(false, unused, || corrupt("native"))),
            Err(DecodeError::Corrupt("native".into())),
            "only \"not decoded natively\" goes to ImageIO"
        );

        // A PNG decodes natively (RGBA), not through ImageIO (BGRA).
        let red = [[0u8, 0, 255, 255]; 4].concat();
        let png = blinzel_core::export::encode_png_bgra(&red, 2, 2).unwrap();
        let img = decode_any_with(&png, Request::Full, 1).unwrap();
        assert_eq!(
            (img.width, img.height, packed(&img)),
            (2, 2, PackedFormat::Rgba8)
        );

        // An icon with one 2×2 32-bit bitmap: native refuses it, ImageIO
        // reads it (a PNG entry this small is refused by ImageIO too).
        let ico = tiny_icon();
        assert!(matches!(
            blinzel_core::decode::decode(&ico, Request::Full),
            Err(DecodeError::Unsupported(Format::Ico))
        ));
        let img = decode_any_with(&ico, Request::Full, 1).unwrap();
        assert_eq!(
            (img.width, img.height, packed(&img)),
            (2, 2, PackedFormat::Bgra8)
        );
        let p = probe_any(&ico).unwrap();
        assert_eq!((p.format, p.width, p.height), (Format::Ico, 2, 2));
    }

    /// AC-23: a reduced-size request to ImageIO decodes through its
    /// thumbnail path at that size.
    #[test]
    fn a_reduced_request_decodes_through_the_thumbnail_path() {
        let img = decode_any_with(HEIC, Request::Fit { w: 32, h: 24 }, 1).unwrap();
        assert_eq!(
            (img.width, img.height, packed(&img)),
            (32, 24, PackedFormat::Bgra8)
        );
        assert_eq!((img.full_width, img.full_height), (64, 48));
        // A box the image fits into decodes it whole.
        let img = decode(HEIC, Request::Fit { w: 128, h: 96 }).unwrap();
        assert_eq!((img.width, img.height), (64, 48));
    }
}
