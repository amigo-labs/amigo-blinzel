//! HEIC (HEIF with HEVC) through `zune-heic` (pure Rust, its own HEVC
//! decoder): 8-bit 4:2:0 and monochrome stills, grids decoded on several
//! threads. `clap`, `irot` and `imir` are applied by the decoder, so the
//! pixels and the probe's size are already oriented. Everything the crate
//! rejects (10-bit, 4:4:4, AVIF, sequences, broken files) reports
//! `Unsupported`, which leaves it to the platform decoder.

use zune_heic::HeifDecoder;
use zune_heic::zune_core::bytestream::ZCursor;
use zune_heic::zune_core::colorspace::ColorSpace;
use zune_heic::zune_core::options::DecoderOptions;

use super::{DecodeError, Result, check_size};
use crate::format::Format;
use crate::geom::Orientation;
use crate::icc::Profile;
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

/// The image data (`mdat`) is read into memory; larger files are refused.
const MAX_MDAT: usize = 512 << 20;

fn unsupported() -> DecodeError {
    DecodeError::Unsupported(Format::Heif)
}

fn decoder(data: &[u8], threads: usize) -> HeifDecoder<ZCursor<&[u8]>> {
    let options = DecoderOptions::default()
        .set_max_width(1 << 20)
        .set_max_height(1 << 20)
        .set_hevc_max_mdat_size(MAX_MDAT)
        .set_num_threads(threads.clamp(1, 255) as u8);
    HeifDecoder::new_with_options(ZCursor::new(data), options)
}

/// Headers of the primary image: oriented size and colour space.
fn headers(dec: &mut HeifDecoder<ZCursor<&[u8]>>) -> Result<(u32, u32, ColorSpace)> {
    dec.decode_headers().map_err(|_| unsupported())?;
    let (w, h) = (dec.width(), dec.height());
    let (Some(w), Some(h), Some(cs)) = (w, h, dec.colorspace()) else {
        return Err(unsupported());
    };
    check_size(w as u64, h as u64)?;
    Ok((w as u32, h as u32, cs))
}

/// Size (already oriented), orientation and colour profile, without
/// decoding pixels. A file without an ICC profile is treated as sRGB.
pub fn info(data: &[u8]) -> Result<((u32, u32), Orientation, Option<Profile>)> {
    let mut dec = decoder(data, 1);
    let (w, h, _) = headers(&mut dec)?;
    let icc = dec
        .icc_data()
        .filter(|p| !crate::icc::is_srgb(p))
        .map(|p| Profile::new(p.clone()));
    Ok(((w, h), Orientation::NORMAL, icc))
}

/// Decodes the primary image; grid tiles on up to `threads` threads.
pub fn decode(data: &[u8], threads: usize) -> Result<Image> {
    let mut dec = decoder(data, threads);
    let (w, h, cs) = headers(&mut dec)?;
    let n = w as usize * h as usize;
    let (src_bpp, format) = match cs {
        ColorSpace::RGB => (3, PackedFormat::Rgba8),
        ColorSpace::RGBA => (4, PackedFormat::Rgba8),
        ColorSpace::LumaA => (2, PackedFormat::Rgba8),
        ColorSpace::Luma => (1, PackedFormat::Gray8),
        _ => return Err(unsupported()),
    };
    let mut plane = Plane::new(w, h, format.bytes_per_pixel());
    // Decode into the front of the plane, then widen in place from the back.
    dec.decode_into(&mut plane.data[..n * src_bpp])
        .map_err(|_| unsupported())?;
    let px = &mut plane.data[..];
    match src_bpp {
        3 => {
            for i in (0..n).rev() {
                let [r, g, b] = [px[3 * i], px[3 * i + 1], px[3 * i + 2]];
                px[4 * i..4 * i + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
        2 => {
            for i in (0..n).rev() {
                let [l, a] = [px[2 * i], px[2 * i + 1]];
                px[4 * i..4 * i + 4].copy_from_slice(&[l, l, l, a]);
            }
        }
        _ => {}
    }
    let mut alpha = Alpha::Opaque;
    if matches!(cs, ColorSpace::RGBA | ColorSpace::LumaA) {
        let px = plane.data.as_chunks_mut::<4>().0;
        if dec.is_alpha_premultiplied() {
            for p in px.iter_mut().filter(|p| p[3] != 0 && p[3] != 255) {
                let a = p[3] as u32;
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        if px.iter().any(|p| p[3] != 255) {
            alpha = Alpha::Straight;
        }
    }
    Ok(Image {
        pixels: Pixels::Packed {
            plane,
            format,
            alpha,
        },
        width: w,
        height: h,
        full_width: w,
        full_height: h,
        region: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Encoded with libheif's heif-enc (x265, quality 90) from a gradient:
    // R = 4x, G = 5y, B = 200 left of x = 32 and 40 right of it; the alpha
    // file is transparent below y = 24.
    const RGB: &[u8] = include_bytes!("../../testdata/rgb-64x48.heic");
    const RGBA: &[u8] = include_bytes!("../../testdata/rgba-64x48.heic");

    fn pixel(img: &Image, x: u32, y: u32) -> [u8; 4] {
        let Pixels::Packed { plane, .. } = &img.pixels else {
            panic!("packed expected");
        };
        let i = x as usize * 4;
        plane.row(y)[i..i + 4].try_into().unwrap()
    }

    fn close(a: [u8; 4], b: [u8; 4]) -> bool {
        a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 12)
    }

    #[test]
    fn probe_and_decode_rgb() {
        let probe = crate::decode::probe(RGB).unwrap();
        assert_eq!(probe.format, Format::Heif);
        assert_eq!((probe.width, probe.height), (64, 48));
        assert_eq!(probe.orientation, Orientation::NORMAL);
        for threads in [1, 4] {
            let img = decode(RGB, threads).unwrap();
            assert_eq!((img.width, img.height), (64, 48));
            assert!(!img.has_alpha());
            assert!(close(pixel(&img, 10, 10), [40, 50, 200, 255]));
            assert!(close(pixel(&img, 50, 40), [200, 200, 40, 255]));
        }
    }

    #[test]
    fn alpha_is_straight() {
        let img = decode(RGBA, 1).unwrap();
        assert!(img.has_alpha());
        assert_eq!(pixel(&img, 10, 5)[3], 255);
        assert_eq!(pixel(&img, 10, 40)[3], 0);
        assert!(close(pixel(&img, 10, 5), [40, 25, 200, 255]));
    }

    #[test]
    fn broken_files_go_to_the_platform() {
        assert!(matches!(
            decode(&RGB[..RGB.len() / 2], 1),
            Err(DecodeError::Unsupported(Format::Heif))
        ));
        let mut bad = RGB.to_vec();
        for b in bad.iter_mut().skip(200) {
            *b ^= 0x5a;
        }
        assert!(crate::decode::decode(&bad, crate::decode::Request::Full).is_err());
    }
}
