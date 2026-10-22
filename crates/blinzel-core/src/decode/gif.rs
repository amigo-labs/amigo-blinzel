//! GIF: the first frame composited onto the logical screen (animation
//! playback comes with milestone M5).

use super::{DecodeError, Result, check_size};
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

/// Logical screen size from the header.
pub fn header_size(d: &[u8]) -> Result<(u32, u32)> {
    if d.len() < 10 {
        return Err(DecodeError::Corrupt("GIF header truncated".into()));
    }
    let w = u16::from_le_bytes([d[6], d[7]]) as u32;
    let h = u16::from_le_bytes([d[8], d[9]]) as u32;
    Ok((w, h))
}

pub fn decode(data: &[u8]) -> Result<Image> {
    let err = |e: gif::DecodingError| DecodeError::Corrupt(e.to_string());
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts.read_info(data).map_err(err)?;
    let (sw, sh) = (dec.width() as u32, dec.height() as u32);
    let frame = dec
        .read_next_frame()
        .map_err(err)?
        .ok_or_else(|| DecodeError::Corrupt("no frames".into()))?;
    // Some files declare a 0×0 screen; fall back to the frame size.
    let (w, h) = if sw == 0 || sh == 0 {
        (
            frame.left as u32 + frame.width as u32,
            frame.top as u32 + frame.height as u32,
        )
    } else {
        (sw, sh)
    };
    check_size(w as u64, h as u64)?;
    let mut plane = Plane::new(w, h, 4);
    plane.data.fill(0);
    let (fx, fy, fw, fh) = (
        frame.left as u32,
        frame.top as u32,
        frame.width as u32,
        frame.height as u32,
    );
    let mut transparent = fx > 0 || fy > 0 || fx + fw < w || fy + fh < h;
    for row in 0..fh {
        let y = fy + row;
        if y >= h {
            break;
        }
        let src = &frame.buffer[(row * fw * 4) as usize..((row + 1) * fw * 4) as usize];
        let cols = fw.min(w.saturating_sub(fx));
        let dst = &mut plane.row_mut(y)[fx as usize * 4..(fx + cols) as usize * 4];
        dst.copy_from_slice(&src[..cols as usize * 4]);
        transparent |= src.chunks_exact(4).any(|p| p[3] != 255);
    }
    Ok(Image {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(w: u16, h: u16, frame: (u16, u16, u16, u16), transparent: Option<u8>) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
            let mut enc = gif::Encoder::new(&mut out, w, h, &palette).unwrap();
            let (left, top, fw, fh) = frame;
            let f = gif::Frame {
                left,
                top,
                width: fw,
                height: fh,
                transparent,
                buffer: std::borrow::Cow::Owned(
                    (0..fw as usize * fh as usize)
                        .map(|i| (i % 4) as u8)
                        .collect(),
                ),
                ..Default::default()
            };
            enc.write_frame(&f).unwrap();
        }
        out
    }

    #[test]
    fn decodes_first_frame() {
        let data = gif(4, 2, (0, 0, 4, 2), None);
        assert_eq!(header_size(&data).unwrap(), (4, 2));
        let img = decode(&data).unwrap();
        assert!(!img.has_alpha());
        let bgra = img.to_bgra();
        assert_eq!(&bgra[..8], &[0, 0, 255, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn offset_frames_and_transparency() {
        let img = decode(&gif(6, 4, (2, 1, 2, 2), None)).unwrap();
        assert!(img.has_alpha());
        assert_eq!(img.to_bgra()[3], 0);
        let img = decode(&gif(4, 2, (0, 0, 4, 2), Some(3))).unwrap();
        assert!(img.has_alpha());
        assert!(decode(b"GIF89a\x01\0\x01\0").is_err());
    }
}
