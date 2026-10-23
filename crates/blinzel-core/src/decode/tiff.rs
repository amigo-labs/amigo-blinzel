//! TIFF through the `tiff` crate (pure Rust): the first image, 8- and
//! 16-bit gray, gray + alpha, RGB and RGBA, interleaved, uncompressed,
//! LZW, Deflate or PackBits. Everything else (JPEG compression, CMYK,
//! YCbCr, palettes, planar layout, float samples) reports `Unsupported` and
//! goes to the platform decoder, as do TIFF-based camera raws (NEF, DNG,
//! ARW, CR2, ...): their first image is a preview or sensor data.
//!
//! Benchmark gate: strips decode in parallel, each thread with its own
//! decoder over the same bytes, 3-4x faster than WIC on 12/24 MP LZW and
//! Deflate files. A single strip decodes no faster than WIC ([`strips`]).

use std::io::Cursor;
use std::sync::Mutex;

use tiff::decoder::{ChunkType, Decoder, DecodingResult, Limits};
use tiff::tags::Tag;
use tiff::{ColorType, TiffError};

use super::{DecodeError, Result, check_size};
use crate::format::Format;
use crate::geom::Orientation;
use crate::icc::Profile;
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

const UNSUPPORTED: DecodeError = DecodeError::Unsupported(Format::Tiff);

/// Whatever the crate cannot read may still be a file the platform decoder
/// handles (raws, odd variants), so every error hands over.
fn err(_: TiffError) -> DecodeError {
    UNSUPPORTED
}

/// DNG marks itself with this tag.
const DNG_VERSION: Tag = Tag::Unknown(50706);

type TiffDecoder<'a> = Decoder<Cursor<&'a [u8]>>;

/// The first image's decoder and size, if it is a plain full-resolution
/// TIFF image: no reduced-resolution subfile (raw previews), no sub-IFDs
/// (raw data), no DNG, no Canon CR2.
fn open(data: &[u8]) -> Result<(TiffDecoder<'_>, (u32, u32))> {
    let mut dec = Decoder::new(Cursor::new(data)).map_err(err)?;
    let reduced = dec
        .find_tag_unsigned::<u32>(Tag::NewSubfileType)
        .map_err(err)?
        .is_some_and(|t| t & 1 != 0);
    let raw = reduced
        || data.get(8..10) == Some(b"CR")
        || dec.find_tag(Tag::SubIfd).map_err(err)?.is_some()
        || dec.find_tag(DNG_VERSION).map_err(err)?.is_some();
    if raw {
        return Err(UNSUPPORTED);
    }
    let (w, h) = dec.dimensions().map_err(err)?;
    check_size(w as u64, h as u64)?;
    // Our own size limit applies; the crate's default buffer limit would
    // reject large 16-bit scans.
    Ok((dec.with_limits(Limits::unlimited()), (w, h)))
}

/// Size, orientation and colour profile, without decoding pixels.
pub fn info(data: &[u8]) -> Result<((u32, u32), Orientation, Option<Profile>)> {
    let (mut dec, size) = open(data)?;
    let orientation = match dec.find_tag_unsigned::<u16>(Tag::Orientation) {
        Ok(Some(o @ 1..=8)) => Orientation::from_exif(o),
        _ => Orientation::NORMAL,
    };
    let icc = dec
        .find_tag(Tag::IccProfile)
        .ok()
        .flatten()
        .and_then(|v| v.into_u8_vec().ok())
        .filter(|p| p.len() >= 132)
        .map(Profile::new);
    Ok((size, orientation, icc))
}

fn strip_count(dec: &mut TiffDecoder) -> usize {
    match dec.get_chunk_type() {
        ChunkType::Strip => dec.strip_count().unwrap_or(0) as usize,
        ChunkType::Tile => 0,
    }
}

/// Strips of a TIFF this module decodes, which can decode in parallel; 0
/// for tiled images and files left to the platform decoder.
pub fn strips(data: &[u8]) -> usize {
    open(data).map_or(0, |(mut dec, _)| strip_count(&mut dec))
}

/// 16-bit samples to 8 bits, rounded.
fn to_u8(v: u16) -> u8 {
    ((v as u32 * 255 + 32767) / 65535) as u8
}

/// All samples of the image as 8 bits. Strips spread over `threads`
/// threads, each with its own decoder; tiles and single strips read in
/// order.
fn read_samples(
    data: &[u8],
    dec: &mut TiffDecoder,
    (w, h): (u32, u32),
    channels: usize,
    bytes: usize,
    threads: usize,
) -> Result<Vec<u8>> {
    let strips = strip_count(dec);
    if threads < 2 || strips < 2 {
        let mut buf = DecodingResult::U8(Vec::new());
        dec.read_image_to_buffer(&mut buf).map_err(err)?;
        return match buf {
            DecodingResult::U8(v) => Ok(v),
            DecodingResult::U16(v) => Ok(v.into_iter().map(to_u8).collect()),
            _ => Err(UNSUPPORTED),
        };
    }

    let row_bytes = w as usize * channels * bytes;
    let rows = dec.chunk_dimensions().1 as usize;
    let mut raw = vec![0u8; row_bytes * h as usize];
    let parts: Vec<&mut [u8]> = raw.chunks_mut(rows * row_bytes).collect();
    if parts.len() != strips {
        return Err(UNSUPPORTED);
    }
    let parts = Mutex::new(parts.into_iter().enumerate());
    std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads.min(strips))
            .map(|_| {
                s.spawn(|| -> Result<()> {
                    let (mut dec, _) = open(data)?;
                    loop {
                        let Some((i, part)) = parts.lock().unwrap().next() else {
                            return Ok(());
                        };
                        dec.read_chunk_bytes(i as u32, part).map_err(err)?;
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .try_for_each(|w| w.join().unwrap_or(Err(UNSUPPORTED)))
    })?;
    Ok(if bytes == 2 {
        raw.as_chunks::<2>()
            .0
            .iter()
            .map(|&b| to_u8(u16::from_ne_bytes(b)))
            .collect()
    } else {
        raw
    })
}

/// The first image on up to `threads` threads.
pub fn decode(data: &[u8], threads: usize) -> Result<Image> {
    let (mut dec, (w, h)) = open(data)?;
    let (channels, bits) = match dec.colortype().map_err(err)? {
        ColorType::Gray(b) => (1, b),
        ColorType::GrayA(b) => (2, b),
        ColorType::RGB(b) => (3, b),
        ColorType::RGBA(b) => (4, b),
        _ => return Err(UNSUPPORTED),
    };
    if !matches!(bits, 8 | 16)
        || dec
            .find_tag_unsigned::<u16>(Tag::PlanarConfiguration)
            .map_err(err)?
            == Some(2)
    {
        return Err(UNSUPPORTED);
    }
    // ExtraSamples 1: associated (premultiplied) alpha.
    let premultiplied = channels % 2 == 0
        && dec
            .find_tag_unsigned_vec::<u16>(Tag::ExtraSamples)
            .ok()
            .flatten()
            .is_some_and(|v| v.first() == Some(&1));

    let samples = read_samples(data, &mut dec, (w, h), channels, bits as usize / 8, threads)?;
    let n = w as usize * h as usize * channels;
    if samples.len() < n {
        return Err(UNSUPPORTED);
    }
    let (bpp, format) = if channels == 1 {
        (1, PackedFormat::Gray8)
    } else {
        (4, PackedFormat::Rgba8)
    };
    let mut plane = Plane::new(w, h, bpp);
    let px = plane.data.as_chunks_mut::<4>().0;
    match channels {
        2 => px
            .iter_mut()
            .zip(samples.as_chunks::<2>().0)
            .for_each(|(p, &[g, a])| *p = [g, g, g, a]),
        3 => px
            .iter_mut()
            .zip(samples.as_chunks::<3>().0)
            .for_each(|(p, &[r, g, b])| *p = [r, g, b, 255]),
        _ => plane.data.copy_from_slice(&samples[..n]),
    }
    let px = plane.data.as_chunks_mut::<4>().0;
    if premultiplied {
        for p in px.iter_mut().filter(|p| p[3] != 0 && p[3] != 255) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let transparent = channels % 2 == 0 && px.iter().any(|p| p[3] != 255);
    Ok(Image {
        pixels: Pixels::Packed {
            plane,
            format,
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype};
    use tiff::tags::ExtraSamples;

    fn rgba_of(img: &Image) -> Vec<u8> {
        img.to_bgra()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect()
    }

    /// RGB8, LZW, `rows` rows per strip, optional orientation and profile.
    fn rgb_lzw(w: u32, h: u32, rgb: &[u8], rows: u32, orientation: Option<u16>) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut enc = TiffEncoder::new(&mut out)
            .unwrap()
            .with_compression(Compression::Lzw);
        let mut img = enc.new_image::<colortype::RGB8>(w, h).unwrap();
        img.rows_per_strip(rows).unwrap();
        if let Some(o) = orientation {
            img.encoder().write_tag(Tag::Orientation, o).unwrap();
        }
        img.write_data(rgb).unwrap();
        out.into_inner()
    }

    #[test]
    fn strips_decode_in_parallel() {
        let (w, h) = (37, 23);
        let rgb = crate::testimg::rgb(w, h);
        let want: Vec<u8> = rgb
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        for rows in [1, 4, h] {
            let data = rgb_lzw(w, h, &rgb, rows, Some(6));
            assert_eq!(strips(&data), h.div_ceil(rows) as usize);
            let (size, o, icc) = info(&data).unwrap();
            assert_eq!((size, o, icc), ((w, h), Orientation::from_exif(6), None));
            for threads in [1, 3, 16] {
                let img = decode(&data, threads).unwrap();
                assert!(!img.has_alpha());
                assert_eq!(rgba_of(&img), want, "rows {rows}, threads {threads}");
            }
        }
    }

    #[test]
    fn gray16_and_premultiplied_alpha() {
        let (w, h) = (9, 5);
        let gray: Vec<u16> = (0..w * h).map(|i| (i * 1500) as u16).collect();
        let mut out = Cursor::new(Vec::new());
        TiffEncoder::new(&mut out)
            .unwrap()
            .with_compression(Compression::Deflate(DeflateLevel::Fast))
            .write_image::<colortype::Gray16>(w, h, &gray)
            .unwrap();
        let img = decode(out.get_ref(), 4).unwrap();
        let px = rgba_of(&img);
        for (i, &g) in gray.iter().enumerate() {
            assert_eq!(
                px[i * 4..i * 4 + 4],
                [to_u8(g); 3].into_iter().chain([255]).collect::<Vec<_>>()[..]
            );
        }

        // Premultiplied 100,50,25 at alpha 128 is straight 199,100,50.
        let mut out = Cursor::new(Vec::new());
        let mut enc = TiffEncoder::new(&mut out).unwrap();
        let mut img = enc.new_image::<colortype::RGB8>(2, 1).unwrap();
        img.extra_samples(&[ExtraSamples::AssociatedAlpha]).unwrap();
        img.write_data(&[100, 50, 25, 128, 7, 8, 9, 255]).unwrap();
        let img = decode(out.get_ref(), 1).unwrap();
        assert!(img.has_alpha());
        assert_eq!(rgba_of(&img), [199, 100, 50, 128, 7, 8, 9, 255]);
    }

    #[test]
    fn raws_go_to_the_platform() {
        let (w, h) = (4, 3);
        let rgb = crate::testimg::rgb(w, h);
        // A reduced-resolution first image, as raw previews are.
        let mut out = Cursor::new(Vec::new());
        let mut enc = TiffEncoder::new(&mut out).unwrap();
        let mut img = enc.new_image::<colortype::RGB8>(w, h).unwrap();
        img.encoder().write_tag(Tag::NewSubfileType, 1u32).unwrap();
        img.write_data(&rgb).unwrap();
        assert!(matches!(info(out.get_ref()), Err(UNSUPPORTED)));
        assert!(matches!(decode(out.get_ref(), 4), Err(UNSUPPORTED)));
        assert_eq!(strips(out.get_ref()), 0);
        assert!(matches!(decode(b"II*\0\x08\0\0\0", 1), Err(UNSUPPORTED)));
    }
}
