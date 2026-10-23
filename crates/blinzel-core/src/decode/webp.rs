//! WebP through `image-webp` (pure Rust): lossy and lossless stills, the
//! first frame of animations, and animations composited by the decoder.

use std::io::Cursor;

use image_webp::{DecodingError, LoopCount, WebPDecoder};

use super::{AnimFrame, Animation, Result, check_size};
use crate::geom::Orientation;
use crate::icc::Profile;
use crate::kernels::{self, Level};
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

fn err(e: DecodingError) -> super::DecodeError {
    super::DecodeError::Corrupt(e.to_string())
}

fn open(data: &[u8]) -> Result<WebPDecoder<Cursor<&[u8]>>> {
    let dec = WebPDecoder::new(Cursor::new(data)).map_err(err)?;
    let (w, h) = dec.dimensions();
    check_size(w as u64, h as u64)?;
    Ok(dec)
}

/// Size, EXIF orientation and colour profile, without decoding pixels.
pub fn info(data: &[u8]) -> Result<((u32, u32), Orientation, Option<Profile>)> {
    let mut dec = open(data)?;
    let orientation = dec
        .exif_metadata()
        .ok()
        .flatten()
        .and_then(|e| {
            // Some writers keep JPEG's "Exif\0\0" in front of the TIFF data.
            let tiff = e.strip_prefix(b"Exif\0\0").unwrap_or(&e);
            crate::exif::orientation(tiff)
        })
        .unwrap_or_default();
    let icc = dec
        .icc_profile()
        .ok()
        .flatten()
        .filter(|p| p.len() >= 132)
        .map(Profile::new);
    Ok((dec.dimensions(), orientation, icc))
}

/// `w × h` RGB or RGBA (as the decoder writes it) into an RGBA image.
fn image(buf: &[u8], (w, h): (u32, u32), alpha: bool) -> Image {
    let mut plane = Plane::new(w, h, 4);
    if alpha {
        plane.data.copy_from_slice(buf);
    } else {
        kernels::rgb_to_rgba(Level::Baseline, buf, &mut plane.data);
    }
    let transparent = alpha && plane.data.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
    Image {
        pixels: Pixels::Packed {
            plane,
            format: PackedFormat::Rgba8,
            alpha: if transparent {
                Alpha::Straight
            } else {
                Alpha::Opaque
            },
        },
        width: w,
        height: h,
        full_width: w,
        full_height: h,
        region: None,
    }
}

/// The still image (an animation's first frame).
pub fn decode(data: &[u8]) -> Result<Image> {
    let mut dec = open(data)?;
    let size = dec.dimensions();
    let mut buf = vec![0u8; dec.output_buffer_size().ok_or(super::DecodeError::TooLarge {
        width: size.0 as u64,
        height: size.1 as u64,
    })?];
    dec.read_image(&mut buf).map_err(err)?;
    Ok(image(&buf, size, dec.has_alpha()))
}

/// Every frame of an animated WebP as long as they fit into `budget`
/// bytes; `None` for a still image.
pub fn decode_animation(data: &[u8], budget: usize) -> Result<Option<Animation>> {
    let mut dec = open(data)?;
    if !dec.is_animated() || dec.num_frames() < 2 {
        return Ok(None);
    }
    let size = dec.dimensions();
    let alpha = dec.has_alpha();
    let mut buf = vec![0u8; dec.output_buffer_size().ok_or(super::DecodeError::TooLarge {
        width: size.0 as u64,
        height: size.1 as u64,
    })?];
    let frame_bytes = size.0 as usize * size.1 as usize * 4;
    let mut frames = Vec::new();
    while frames.len() < dec.num_frames() as usize && (frames.len() + 1) * frame_bytes <= budget {
        let ms = dec.read_frame(&mut buf).map_err(err)?;
        frames.push(AnimFrame {
            image: image(&buf, size, alpha),
            delay_ms: super::min_delay_ms(ms),
        });
    }
    if frames.len() < 2 {
        return Ok(None);
    }
    let plays = match dec.loop_count() {
        LoopCount::Forever => None,
        LoopCount::Times(n) => Some(n.get() as u32),
    };
    Ok(Some(Animation { frames, plays }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lossless(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        image_webp::WebPEncoder::new(&mut out)
            .encode(rgba, w, h, image_webp::ColorType::Rgba8)
            .unwrap();
        out
    }

    #[test]
    fn decodes_lossless_stills() {
        let (w, h) = (37, 23);
        let rgba = crate::testimg::rgba(w, h);
        let data = lossless(w, h, &rgba);
        let (size, o, icc) = info(&data).unwrap();
        assert_eq!((size, o, icc), ((w, h), Orientation::NORMAL, None));
        let img = decode(&data).unwrap();
        assert!(img.has_alpha());
        let back: Vec<u8> = img
            .to_bgra()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        assert_eq!(back, rgba);
        assert!(decode_animation(&data, usize::MAX).unwrap().is_none());
        assert!(decode(b"RIFF\0\0\0\0WEBPVP8 ").is_err());
    }
}
