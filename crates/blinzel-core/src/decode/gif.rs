//! GIF: the first frame composited onto the logical screen, and for
//! animations every frame composited the way browsers do (transparency,
//! disposal, minimum delay).

use super::{AnimFrame, Animation, DecodeError, Result, check_size, min_delay_ms};
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

fn delay_ms(centiseconds: u16) -> u32 {
    min_delay_ms(centiseconds as u32 * 10)
}

/// Every frame of an animated GIF on its canvas, as long as the frames
/// fit into `budget` bytes (the rest is dropped); `None` for a single
/// frame.
pub fn decode_animation(data: &[u8], budget: usize) -> Result<Option<Animation>> {
    let err = |e: gif::DecodingError| DecodeError::Corrupt(e.to_string());
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts.read_info(data).map_err(err)?;
    let (w, h) = (dec.width() as usize, dec.height() as usize);
    check_size(w as u64, h as u64)?;
    let mut canvas = vec![0u8; w * h * 4];
    let mut frames = Vec::new();
    let mut used = 0usize;
    while let Some(f) = dec.read_next_frame().map_err(err)? {
        if used + canvas.len() > budget {
            break;
        }
        let before = (f.dispose == gif::DisposalMethod::Previous).then(|| canvas.clone());
        let (fx, fy, fw, fh) = (
            f.left as usize,
            f.top as usize,
            f.width as usize,
            f.height as usize,
        );
        let cols = fw.min(w.saturating_sub(fx));
        for row in 0..fh.min(h.saturating_sub(fy)) {
            let src = &f.buffer[row * fw * 4..][..cols * 4];
            let dst = &mut canvas[((fy + row) * w + fx) * 4..][..cols * 4];
            // Transparent pixels leave the canvas as it is.
            for (d, s) in dst
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<4>().0)
            {
                if s[3] != 0 {
                    *d = *s;
                }
            }
        }
        let mut plane = Plane::new(w as u32, h as u32, 4);
        plane.data.copy_from_slice(&canvas);
        let opaque = canvas.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
        frames.push(AnimFrame {
            image: Image {
                pixels: Pixels::Packed {
                    plane,
                    format: PackedFormat::Rgba8,
                    alpha: if opaque {
                        Alpha::Opaque
                    } else {
                        Alpha::Straight
                    },
                },
                width: w as u32,
                height: h as u32,
                full_width: w as u32,
                full_height: h as u32,
                region: None,
            },
            delay_ms: delay_ms(f.delay),
        });
        used += canvas.len();
        match (f.dispose, before) {
            (gif::DisposalMethod::Background, _) => {
                for row in 0..fh.min(h.saturating_sub(fy)) {
                    canvas[((fy + row) * w + fx) * 4..][..cols * 4].fill(0);
                }
            }
            (gif::DisposalMethod::Previous, Some(b)) => canvas = b,
            _ => {}
        }
    }
    if frames.len() < 2 {
        return Ok(None);
    }
    let plays = match dec.repeat() {
        gif::Repeat::Infinite => None,
        gif::Repeat::Finite(n) => Some(n as u32 + 1),
    };
    Ok(Some(Animation { frames, plays }))
}

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
        transparent |= src.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
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

    /// Palette colour, disposal, and left, top, width, height.
    type TestFrame = (u8, gif::DisposalMethod, (u16, u16, u16, u16));

    /// 4 × 2 frames of one palette colour each: red, green, blue, white;
    /// colour 3 can be the transparent one.
    fn animated(frames: &[TestFrame], transparent: Option<u8>, repeat: gif::Repeat) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
            let mut enc = gif::Encoder::new(&mut out, 4, 2, &palette).unwrap();
            enc.set_repeat(repeat).unwrap();
            for &(colour, dispose, (left, top, fw, fh)) in frames {
                let f = gif::Frame {
                    left,
                    top,
                    width: fw,
                    height: fh,
                    delay: 5,
                    dispose,
                    transparent,
                    buffer: std::borrow::Cow::Owned(vec![colour; fw as usize * fh as usize]),
                    ..Default::default()
                };
                enc.write_frame(&f).unwrap();
            }
        }
        out
    }

    fn rgba_at(f: &AnimFrame, x: u32, y: u32) -> [u8; 4] {
        let b = f.image.to_bgra();
        let i = ((y * 4 + x) * 4) as usize;
        [b[i + 2], b[i + 1], b[i], b[i + 3]]
    }

    #[test]
    fn composites_animation_frames() {
        use gif::DisposalMethod::{Background, Keep, Previous};
        const RED: [u8; 4] = [255, 0, 0, 255];
        const GREEN: [u8; 4] = [0, 255, 0, 255];
        const BLUE: [u8; 4] = [0, 0, 255, 255];
        let data = animated(
            &[
                (0, Keep, (0, 0, 4, 2)),       // all red
                (1, Previous, (0, 0, 2, 2)),   // green left, then undone
                (2, Background, (2, 0, 2, 2)), // blue right, then cleared
                (3, Keep, (0, 0, 4, 2)),       // transparent: no change
            ],
            Some(3),
            gif::Repeat::Infinite,
        );
        let a = decode_animation(&data, usize::MAX).unwrap().unwrap();
        assert_eq!(a.frames.len(), 4);
        assert_eq!(a.plays, None);
        assert_eq!(a.frames[0].delay_ms, 50);
        assert_eq!(rgba_at(&a.frames[0], 3, 1), RED);
        assert_eq!(rgba_at(&a.frames[1], 0, 0), GREEN);
        assert_eq!(rgba_at(&a.frames[1], 3, 0), RED);
        // "Previous" undid the green; blue on the right.
        assert_eq!(rgba_at(&a.frames[2], 0, 0), RED);
        assert_eq!(rgba_at(&a.frames[2], 3, 0), BLUE);
        // "Background" cleared the right half to transparent.
        assert_eq!(rgba_at(&a.frames[3], 0, 0), RED);
        assert_eq!(rgba_at(&a.frames[3], 3, 0)[3], 0);
        assert!(a.frames[3].image.has_alpha());
        assert!(!a.frames[0].image.has_alpha());
    }

    #[test]
    fn single_frames_budgets_and_plays() {
        use gif::DisposalMethod::Keep;
        let one = animated(&[(0, Keep, (0, 0, 4, 2))], None, gif::Repeat::Infinite);
        assert!(decode_animation(&one, usize::MAX).unwrap().is_none());
        let three = animated(&[(0, Keep, (0, 0, 4, 2)); 3], None, gif::Repeat::Finite(2));
        assert_eq!(
            decode_animation(&three, usize::MAX).unwrap().unwrap().plays,
            Some(3)
        );
        // Room for two frames of 4 × 2 × 4 bytes.
        let cut = decode_animation(&three, 2 * 32).unwrap().unwrap();
        assert_eq!(cut.frames.len(), 2);
        assert_eq!(delay_ms(0), 100);
        assert_eq!(delay_ms(1), 100);
        assert_eq!(delay_ms(7), 70);
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
