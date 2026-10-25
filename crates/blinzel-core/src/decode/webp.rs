//! WebP through `image-webp` (pure Rust): lossy and lossless stills, the
//! first frame of animations, and animations composited by the decoder.
//!
//! Benchmark gate (12/24 MP photos, identical pixels): `image-webp` decodes
//! lossless 31 % faster than WIC, lossy about 20 % slower once WIC's codec
//! is loaded. Where a platform decoder exists, lossy stills go there
//! ([`is_lossy_still`]).

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

/// A tiny lossy still (8×6 of 0x3080C0, libwebp quality 50): loads a
/// platform codec before the first real image needs it.
pub const TINY_LOSSY: [u8; 60] = [
    0x52, 0x49, 0x46, 0x46, 0x34, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38, 0x20,
    0x28, 0x00, 0x00, 0x00, 0x90, 0x01, 0x00, 0x9d, 0x01, 0x2a, 0x08, 0x00, 0x06, 0x00, 0x02, 0xc0,
    0x4c, 0x25, 0xa0, 0x02, 0x74, 0xba, 0x00, 0x03, 0x98, 0x00, 0xfe, 0xde, 0x12, 0x7f, 0xaa, 0xe4,
    0xec, 0x4f, 0x8e, 0x1f, 0xf7, 0x1a, 0xdb, 0x9f, 0xac, 0x40, 0x00, 0x00,
];

/// Largest ICC profile or EXIF block read.
const METADATA_LIMIT: usize = 8 << 20;

fn open(data: &[u8]) -> Result<WebPDecoder<Cursor<&[u8]>>> {
    let mut dec = WebPDecoder::new(Cursor::new(data)).map_err(err)?;
    dec.set_memory_limit(METADATA_LIMIT);
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

/// A still image with lossy (VP8) pixels; `false` for lossless and
/// animated files and for anything that does not parse.
pub fn is_lossy_still(data: &[u8]) -> bool {
    open(data).is_ok_and(|mut d| !d.is_animated() && d.is_lossy())
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
    let mut buf = vec![
        0u8;
        dec.output_buffer_size()
            .ok_or(super::DecodeError::TooLarge {
                width: size.0 as u64,
                height: size.1 as u64,
            })?
    ];
    dec.read_image(&mut buf).map_err(err)?;
    Ok(image(&buf, size, dec.has_alpha()))
}

/// Every frame of an animated WebP as long as they fit into `budget`
/// bytes; `None` for a still image.
pub fn decode_animation(data: &[u8], budget: usize) -> Result<Option<Animation>> {
    super::collect_animation(|sink| stream_animation(data, budget, sink))
}

/// Like [`decode_animation`], handing each frame to `sink` as soon as it
/// is decoded.
pub fn stream_animation(
    data: &[u8],
    budget: usize,
    sink: &mut super::FrameSink,
) -> Result<super::AnimInfo> {
    let mut dec = open(data)?;
    let still = super::AnimInfo {
        frames: 0,
        plays: None,
    };
    if !dec.is_animated() || dec.num_frames() < 2 {
        return Ok(still);
    }
    let size = dec.dimensions();
    let alpha = dec.has_alpha();
    let mut buf = vec![
        0u8;
        dec.output_buffer_size()
            .ok_or(super::DecodeError::TooLarge {
                width: size.0 as u64,
                height: size.1 as u64,
            })?
    ];
    let frame_bytes = size.0 as usize * size.1 as usize * 4;
    let mut count = 0;
    while count < dec.num_frames() as usize && (count + 1) * frame_bytes <= budget {
        let ms = dec.read_frame(&mut buf).map_err(err)?;
        count += 1;
        let more = sink(AnimFrame {
            image: image(&buf, size, alpha),
            delay_ms: super::min_delay_ms(ms),
        });
        if !more {
            break;
        }
    }
    let plays = match dec.loop_count() {
        LoopCount::Forever => None,
        LoopCount::Times(n) => Some(n.get() as u32),
    };
    Ok(super::AnimInfo {
        frames: count,
        plays,
    })
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

    /// RIFF chunk: four-character code, little-endian size, data, padding.
    fn chunk(code: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = code.to_vec();
        c.extend((data.len() as u32).to_le_bytes());
        c.extend(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    fn u24(v: u32) -> [u8; 3] {
        let b = v.to_le_bytes();
        [b[0], b[1], b[2]]
    }

    /// An animation of lossless full-canvas frames `(rgba, delay ms)`
    /// playing `loops` times (0: for ever).
    fn animated(w: u32, h: u32, frames: &[(Vec<u8>, u32)], loops: u16) -> Vec<u8> {
        let mut vp8x = vec![0x02 | 0x10, 0, 0, 0];
        vp8x.extend(u24(w - 1));
        vp8x.extend(u24(h - 1));
        let mut anim = vec![0, 0, 0, 0];
        anim.extend(loops.to_le_bytes());
        let mut body = b"WEBP".to_vec();
        body.extend(chunk(b"VP8X", &vp8x));
        body.extend(chunk(b"ANIM", &anim));
        for (rgba, delay) in frames {
            // The encoder writes RIFF....WEBP and then the VP8L chunk.
            let file = lossless(w, h, rgba);
            let mut anmf = [u24(0), u24(0), u24(w - 1), u24(h - 1), u24(*delay)].concat();
            // No blending: each frame replaces the canvas.
            anmf.push(0x02);
            anmf.extend(&file[12..]);
            body.extend(chunk(b"ANMF", &anmf));
        }
        chunk(b"RIFF", &body)
    }

    #[test]
    fn plays_animations() {
        let (w, h) = (5, 4);
        let a = crate::testimg::rgba(w, h);
        let b: Vec<u8> = a.iter().map(|v| 255 - v).collect();
        let data = animated(w, h, &[(a.clone(), 70), (b.clone(), 5)], 3);
        assert_eq!(info(&data).unwrap().0, (w, h));
        assert!(!is_lossy_still(&data));
        let anim = decode_animation(&data, usize::MAX).unwrap().unwrap();
        assert_eq!(anim.plays, Some(3));
        let delays: Vec<u32> = anim.frames.iter().map(|f| f.delay_ms).collect();
        // Browsers stretch delays under 20 ms to 100 ms.
        assert_eq!(delays, [70, 100]);
        let rgba = |img: &Image| -> Vec<u8> {
            img.to_bgra()
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect()
        };
        assert_eq!(rgba(&anim.frames[0].image), a);
        assert_eq!(rgba(&anim.frames[1].image), b);
        // The still image is the first frame.
        assert_eq!(rgba(&decode(&data).unwrap()), a);
        // Only the frames that fit; a single one is no animation.
        let frame = (w * h * 4) as usize;
        assert!(decode_animation(&data, frame).unwrap().is_none());
        let forever = animated(w, h, &[(a.clone(), 50), (b, 50)], 0);
        assert_eq!(
            decode_animation(&forever, usize::MAX)
                .unwrap()
                .unwrap()
                .plays,
            None
        );
    }

    #[test]
    fn lossy_stills() {
        assert!(is_lossy_still(&TINY_LOSSY));
        let img = decode(&TINY_LOSSY).unwrap();
        assert!(!img.has_alpha());
        assert_eq!((img.width, img.height), (8, 6));
        let px = &img.to_bgra()[..4];
        let near = |a: u8, b: u8| a.abs_diff(b) <= 3;
        assert!(
            near(px[2], 0x30) && near(px[1], 0x80) && near(px[0], 0xC0),
            "{px:?}"
        );
        assert!(!is_lossy_still(b"RIFF    WEBPVP8 "));
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
        assert!(!is_lossy_still(&data));
        assert!(decode(b"RIFF\0\0\0\0WEBPVP8 ").is_err());
    }
}
