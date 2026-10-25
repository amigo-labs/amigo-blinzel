//! Saving a cutout ("Ausschnitt speichern"). Source files are never
//! touched; the result is a new file.
//!
//! JPEG → JPEG is lossless where the format allows it (crop starting on an
//! iMCU boundary, no view rotation): the DCT coefficients are copied, not
//! re-encoded, which is both instant and loss-free. Everything else decodes
//! the region at full resolution, applies the view orientation and encodes
//! JPEG (quality 95, source subsampling and ICC profile) or PNG.

use crate::decode::{DecodeError, Request};
use crate::format::{self, Format};
use crate::geom::{Orientation, PixelRect};
use crate::pixel::Image;
use crate::selection::orient_bgra;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFormat {
    Jpeg,
    Png,
}

impl SaveFormat {
    /// From a file name extension (case-insensitive); JPEG otherwise.
    pub fn from_path(path: &std::path::Path) -> SaveFormat {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => SaveFormat::Png,
            _ => SaveFormat::Jpeg,
        }
    }
}

/// What to save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cutout {
    /// Rectangle in stored (file) orientation, full resolution.
    pub stored_rect: PixelRect,
    /// Orientation stored in the file (EXIF).
    pub file_orientation: Orientation,
    /// Orientation as shown (EXIF followed by the user's view rotation).
    pub view_orientation: Orientation,
}

#[derive(Debug)]
pub struct Saved {
    pub bytes: Vec<u8>,
    /// True when no pixel was re-encoded.
    pub lossless: bool,
}

pub const JPEG_QUALITY: u8 = 95;

/// Decodes a region of a source file (native decoders plus the platform
/// fallback).
pub type RegionDecoder<'a> = dyn Fn(&[u8], Request) -> Result<Image, DecodeError> + 'a;

/// Produces the bytes of the saved cutout. `decode` decodes a region of
/// the source (native decoders plus the platform fallback).
pub fn export(
    data: &[u8],
    cut: &Cutout,
    format: SaveFormat,
    decode: &RegionDecoder<'_>,
) -> Result<Saved, String> {
    let source = format::detect(data);
    let info = (source == Format::Jpeg)
        .then(|| blinzel_jpeg::parse(data).ok())
        .flatten();

    #[cfg(feature = "turbojpeg")]
    if format == SaveFormat::Jpeg
        && cut.view_orientation == cut.file_orientation
        && let Some(info) = &info
        && info.precision == 8
    {
        let (mw, mh) = info.mcu_size();
        if cut.stored_rect.x.is_multiple_of(mw) && cut.stored_rect.y.is_multiple_of(mh) {
            let mut bytes = crate::decode::jpeg::lossless_crop(data, cut.stored_rect)
                .map_err(|e| e.to_string())?;
            clean_markers(&mut bytes);
            return Ok(Saved {
                bytes,
                lossless: true,
            });
        }
    }

    let img = decode(data, Request::Region(cut.stored_rect)).map_err(|e| e.to_string())?;
    let bgra = img.to_bgra();
    let (pixels, w, h) = orient_bgra(&bgra, img.width, img.height, cut.view_orientation);
    let bytes = match format {
        SaveFormat::Png => encode_png(&pixels, w, h)?,
        SaveFormat::Jpeg => encode_jpeg(&pixels, w, h, data, info.as_ref())?,
    };
    Ok(Saved {
        bytes,
        lossless: false,
    })
}

#[cfg(feature = "turbojpeg")]
fn encode_jpeg(
    bgra: &[u8],
    w: u32,
    h: u32,
    source: &[u8],
    info: Option<&blinzel_jpeg::JpegInfo>,
) -> Result<Vec<u8>, String> {
    use blinzel_jpeg::Subsampling;
    // Keep the source's chroma resolution; 4:2:0 otherwise.
    let subsampling = match info.map(|i| i.subsampling()) {
        Some(Subsampling::S444 | Subsampling::Gray) => 444,
        Some(Subsampling::S422) => 422,
        Some(Subsampling::S440) => 440,
        _ => 420,
    };
    let icc = info.and_then(|i| blinzel_jpeg::icc_profile(source, i));
    crate::decode::jpeg::encode_bgra(bgra, w, h, JPEG_QUALITY, subsampling, icc.as_deref())
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "turbojpeg"))]
fn encode_jpeg(
    _: &[u8],
    _: u32,
    _: u32,
    _: &[u8],
    _: Option<&blinzel_jpeg::JpegInfo>,
) -> Result<Vec<u8>, String> {
    Err("JPEG encoding needs the turbojpeg feature".into())
}

fn encode_png(bgra: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let opaque = bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
    let (color, data): (png::ColorType, Vec<u8>) = if opaque {
        (
            png::ColorType::Rgb,
            bgra.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0]])
                .collect(),
        )
    } else {
        (
            png::ColorType::Rgba,
            bgra.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect(),
        )
    };
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Balanced);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// After a lossless crop the copied markers describe the old image:
/// the MPF index points at images that are no longer appended (removed),
/// and the EXIF thumbnail shows the uncropped picture (unlinked by
/// clearing IFD0's next-IFD offset, so Explorer generates a fresh one).
pub fn clean_markers(jpeg: &mut Vec<u8>) {
    let segments: Vec<blinzel_jpeg::Segment> = blinzel_jpeg::Segments::new(jpeg).collect();
    let mut remove = Vec::new();
    for seg in &segments {
        let p = seg.payload.clone();
        match seg.marker {
            0xE2 if jpeg[p.clone()].starts_with(b"MPF\0") => remove.push(seg.offset..p.end),
            0xE1 if jpeg[p.clone()].starts_with(b"Exif\0\0") => {
                unlink_ifd1(&mut jpeg[p.start + 6..p.end])
            }
            _ => {}
        }
    }
    for r in remove.into_iter().rev() {
        jpeg.drain(r);
    }
}

/// Sets IFD0's "next IFD" offset to 0 in a TIFF block.
fn unlink_ifd1(tiff: &mut [u8]) {
    let le = match tiff.get(..4) {
        Some(b"II*\0") => true,
        Some(b"MM\0*") => false,
        _ => return,
    };
    let rd32 = |b: &[u8]| {
        let a: [u8; 4] = b.try_into().unwrap();
        if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    };
    let Some(ifd0) = tiff.get(4..8).map(rd32) else {
        return;
    };
    let ifd0 = ifd0 as usize;
    let Some(n) = tiff.get(ifd0..ifd0 + 2).map(|b| {
        if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        }
    }) else {
        return;
    };
    let next = ifd0 + 2 + 12 * n as usize;
    if let Some(slot) = tiff.get_mut(next..next + 4) {
        slot.copy_from_slice(&[0; 4]);
    }
}

#[cfg(all(test, feature = "turbojpeg"))]
mod tests {
    use super::*;
    use crate::cpuref::psnr;
    use crate::decode::jpeg::{EncodeOptions, encode_rgb};
    use crate::testimg;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        encode_rgb(&testimg::rgb(w, h), w, h, EncodeOptions::default()).unwrap()
    }

    fn region(data: &[u8], r: PixelRect) -> Vec<u8> {
        crate::decode::decode(data, Request::Region(r))
            .unwrap()
            .to_bgra()
    }

    fn cut(r: PixelRect, file: u16, view: u16) -> Cutout {
        Cutout {
            stored_rect: r,
            file_orientation: Orientation::from_exif(file),
            view_orientation: Orientation::from_exif(view),
        }
    }

    /// Inserts APP segments right after SOI.
    fn with_segments(mut jpeg: Vec<u8>, segs: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut head = vec![0xFF, 0xD8];
        for (m, p) in segs {
            head.extend_from_slice(&[0xFF, *m]);
            head.extend_from_slice(&((p.len() + 2) as u16).to_be_bytes());
            head.extend_from_slice(p);
        }
        head.extend(jpeg.drain(2..));
        head
    }

    #[test]
    fn aligned_jpeg_crop_is_lossless() {
        let src = jpeg(640, 480);
        let r = PixelRect::new(64, 32, 200, 150);
        let saved = export(
            &src,
            &cut(r, 1, 1),
            SaveFormat::Jpeg,
            &crate::decode::decode,
        )
        .unwrap();
        assert!(saved.lossless);
        let info = blinzel_jpeg::parse(&saved.bytes).unwrap();
        assert_eq!((info.width, info.height), (200, 150));
        let got = crate::decode::decode(
            &saved.bytes,
            Request::Region(PixelRect::new(0, 0, 200, 150)),
        )
        .unwrap()
        .to_bgra();
        // Same coefficients: only chroma upsampling at the new edges differs.
        assert!(psnr(&got, &region(&src, r)) > 40.0);
    }

    #[test]
    fn unaligned_or_rotated_crops_are_reencoded() {
        let src = jpeg(320, 240);
        let r = PixelRect::new(5, 7, 100, 50);
        let s = export(
            &src,
            &cut(r, 1, 1),
            SaveFormat::Jpeg,
            &crate::decode::decode,
        )
        .unwrap();
        assert!(!s.lossless);
        assert!(
            psnr(
                &crate::decode::decode(&s.bytes, Request::Full)
                    .unwrap()
                    .to_bgra(),
                &region(&src, r)
            ) > 30.0
        );
        // View rotated 90°: pixels are rotated, size swapped.
        let r = PixelRect::new(16, 16, 100, 50);
        let s = export(
            &src,
            &cut(r, 1, 6),
            SaveFormat::Jpeg,
            &crate::decode::decode,
        )
        .unwrap();
        assert!(!s.lossless);
        let i = blinzel_jpeg::parse(&s.bytes).unwrap();
        assert_eq!((i.width, i.height), (50, 100));
    }

    #[test]
    fn png_output_and_format_from_path() {
        let src = jpeg(64, 64);
        let s = export(
            &src,
            &cut(PixelRect::new(0, 0, 10, 20), 1, 1),
            SaveFormat::Png,
            &crate::decode::decode,
        )
        .unwrap();
        assert_eq!(crate::decode::png::header_size(&s.bytes).unwrap(), (10, 20));
        assert_eq!(
            SaveFormat::from_path(std::path::Path::new("a.PNG")),
            SaveFormat::Png
        );
        assert_eq!(
            SaveFormat::from_path(std::path::Path::new("a.jpeg")),
            SaveFormat::Jpeg
        );
    }

    #[test]
    fn lossless_crop_keeps_exif_but_drops_mpf_and_thumbnail_link() {
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend(crate::exif::tests::tiff_le(6, Some((60, 4))));
        exif.resize(6 + 64, 0);
        let mpf = b"MPF\0II*\0\x08\0\0\0\0\0".to_vec();
        let src = with_segments(jpeg(320, 240), &[(0xE1, exif), (0xE2, mpf)]);
        let s = export(
            &src,
            &cut(PixelRect::new(0, 0, 160, 160), 6, 6),
            SaveFormat::Jpeg,
            &crate::decode::decode,
        )
        .unwrap();
        assert!(s.lossless);
        let info = blinzel_jpeg::parse(&s.bytes).unwrap();
        assert!(info.mpf.is_none(), "MPF segment must be removed");
        let tiff = &s.bytes[info.exif.clone().expect("EXIF kept")];
        assert_eq!(
            crate::exif::orientation(tiff),
            Some(Orientation::from_exif(6))
        );
        assert_eq!(crate::exif::exif_thumbnail(tiff), None);
        assert!(crate::decode::decode(&s.bytes, Request::Full).is_ok());
    }
}
