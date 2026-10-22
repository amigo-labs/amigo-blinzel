//! PNG via the `png` crate (fdeflate). The benchmark gate measured it
//! 1.7× faster than `zune-png` on RGB photos and 3–4× on screenshots; its
//! built-in RGB→RGBA expansion however costs 4× the decode time on flat
//! images, so we expand ourselves, row by row while the row is in cache.
//! Output stays in a layout the GPU takes directly (RGBA8 or Gray8).

use std::io::Cursor;

use super::{DecodeError, Result, check_size};
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

/// Width and height from the IHDR chunk.
pub fn header_size(d: &[u8]) -> Result<(u32, u32)> {
    if d.len() < 24 || &d[12..16] != b"IHDR" {
        return Err(DecodeError::Corrupt("missing IHDR".into()));
    }
    let w = u32::from_be_bytes(d[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(d[20..24].try_into().unwrap());
    Ok((w, h))
}

fn err(e: png::DecodingError) -> DecodeError {
    DecodeError::Corrupt(e.to_string())
}

pub fn decode(data: &[u8]) -> Result<Image> {
    let (w, h) = header_size(data)?;
    check_size(w as u64, h as u64)?;
    // Size is checked above; the crate's default 64 MiB limit would reject
    // legitimate large images.
    let mut dec =
        png::Decoder::new_with_limits(Cursor::new(data), png::Limits { bytes: usize::MAX });
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(err)?;
    let interlaced = reader.info().interlaced;
    let (color, _) = reader.output_color_type();

    let (plane, format, alpha) = match color {
        png::ColorType::Grayscale => {
            let mut p = Plane::new(w, h, 1);
            reader.next_frame(&mut p.data).map_err(err)?;
            (p, PackedFormat::Gray8, Alpha::Opaque)
        }
        png::ColorType::Rgba => {
            let mut p = Plane::new(w, h, 4);
            reader.next_frame(&mut p.data).map_err(err)?;
            (p, PackedFormat::Rgba8, Alpha::Straight)
        }
        png::ColorType::Rgb | png::ColorType::GrayscaleAlpha => {
            let src_bpp = if color == png::ColorType::Rgb { 3 } else { 2 };
            let mut p = Plane::new(w, h, 4);
            if interlaced {
                let mut tmp = vec![0u8; w as usize * h as usize * src_bpp];
                reader.next_frame(&mut tmp).map_err(err)?;
                for (y, row) in tmp.chunks_exact(w as usize * src_bpp).enumerate() {
                    expand(row, p.row_mut(y as u32), src_bpp);
                }
            } else {
                let mut row = vec![0u8; w as usize * src_bpp];
                for y in 0..h {
                    match reader.read_row(&mut row).map_err(err)? {
                        Some(_) => expand(&row, p.row_mut(y), src_bpp),
                        None => break,
                    }
                }
            }
            let alpha = if src_bpp == 2 {
                Alpha::Straight
            } else {
                Alpha::Opaque
            };
            (p, PackedFormat::Rgba8, alpha)
        }
        other => {
            return Err(DecodeError::Corrupt(format!(
                "unexpected PNG output {other:?}"
            )));
        }
    };
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

/// RGB or gray+alpha row → RGBA row.
#[inline]
fn expand(src: &[u8], dst: &mut [u8], src_bpp: usize) {
    if src_bpp == 3 {
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
            d.copy_from_slice(&[s[0], s[1], s[2], 255]);
        }
    } else {
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(2)) {
            d.copy_from_slice(&[s[0], s[0], s[0], s[1]]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg;

    fn encode(w: u32, h: u32, color: png::ColorType, depth: png::BitDepth, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(color);
            enc.set_depth(depth);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(data).unwrap();
        }
        out
    }

    #[test]
    fn decodes_color_types() {
        let (w, h) = (37u32, 21u32);
        let rgb = testimg::rgb(w, h);
        let img = decode(&encode(
            w,
            h,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &rgb,
        ))
        .unwrap();
        assert_eq!(
            header_size(&encode(
                w,
                h,
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                &rgb
            ))
            .unwrap(),
            (w, h)
        );
        let bgra = img.to_bgra();
        assert_eq!(&bgra[..4], &[rgb[2], rgb[1], rgb[0], 255]);
        assert!(!img.has_alpha());

        let rgba: Vec<u8> = rgb
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 128])
            .collect();
        let img = decode(&encode(
            w,
            h,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &rgba,
        ))
        .unwrap();
        assert!(img.has_alpha());
        assert_eq!(&img.to_bgra()[..4], &[rgb[2], rgb[1], rgb[0], 128]);

        let gray = testimg::gray(w, h);
        let img = decode(&encode(
            w,
            h,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &gray,
        ))
        .unwrap();
        assert!(matches!(
            img.pixels,
            Pixels::Packed {
                format: PackedFormat::Gray8,
                ..
            }
        ));

        let ga: Vec<u8> = gray.iter().flat_map(|&g| [g, 200]).collect();
        let img = decode(&encode(
            w,
            h,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            &ga,
        ))
        .unwrap();
        assert_eq!(&img.to_bgra()[..4], &[gray[0], gray[0], gray[0], 200]);

        let rgb16: Vec<u8> = rgb.iter().flat_map(|&v| [v, 0x77]).collect();
        let img = decode(&encode(
            w,
            h,
            png::ColorType::Rgb,
            png::BitDepth::Sixteen,
            &rgb16,
        ))
        .unwrap();
        assert_eq!(&img.to_bgra()[..3], &[rgb[2], rgb[1], rgb[0]]);
    }

    #[test]
    fn interlaced_and_palette() {
        let (w, h) = (19u32, 13u32);
        let rgb = testimg::rgb(w, h);
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&rgb).unwrap();
        }
        let plain = decode(&out).unwrap().to_bgra();
        // Palette image: 2 colours, one transparent.
        let mut pal_png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut pal_png, 2, 1);
            enc.set_color(png::ColorType::Indexed);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_palette(vec![255, 0, 0, 0, 0, 255]);
            enc.set_trns(vec![255, 0]);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&[0, 1]).unwrap();
        }
        let img = decode(&pal_png).unwrap();
        assert!(img.has_alpha());
        assert_eq!(img.to_bgra(), [0, 0, 255, 255, 255, 0, 0, 0]);
        assert_eq!(plain.len(), (w * h * 4) as usize);
    }

    #[test]
    fn corrupt_input_is_an_error() {
        let rgb = testimg::rgb(10, 10);
        let data = encode(10, 10, png::ColorType::Rgb, png::BitDepth::Eight, &rgb);
        assert!(decode(&data[..30]).is_err());
        assert!(header_size(b"\x89PNG").is_err());
    }
}
