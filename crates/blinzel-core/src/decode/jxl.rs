//! JPEG XL through `jxl-oxide` (pure Rust): stills and animations, in the
//! colour space the decoder renders to (described by the returned profile).
//! Pixels stay in stored orientation; the probe reports the header's.

use jxl_oxide::image::BitDepth;
use jxl_oxide::{InitializeResult, JxlImage, JxlThreadPool, PixelFormat, Render};
use jxl_render::ImageBuffer;

use super::{AnimFrame, Animation, DecodeError, Result, check_size};
use crate::format::Format;
use crate::geom::Orientation;
use crate::icc::Profile;
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

fn err(e: Box<dyn std::error::Error + Send + Sync>) -> DecodeError {
    DecodeError::Corrupt(e.to_string())
}

/// Header and colour information only: feeds the file in steps until the
/// header parses, without loading frames.
fn header(data: &[u8]) -> Result<JxlImage> {
    let mut uninit = JxlImage::builder()
        .pool(JxlThreadPool::none())
        .build_uninit();
    let mut fed = 0;
    let mut step = 64 << 10;
    loop {
        let end = (fed + step).min(data.len());
        fed += uninit.feed_bytes(&data[fed..end]).map_err(err)?;
        match uninit.try_init().map_err(err)? {
            InitializeResult::Initialized(img) => return Ok(img),
            InitializeResult::NeedMoreData(u) if end < data.len() => uninit = u,
            InitializeResult::NeedMoreData(_) => {
                return Err(DecodeError::Corrupt("truncated header".into()));
            }
        }
        // Large ICC profiles or boxes before the codestream.
        step *= 4;
    }
}

/// The whole file, decoded on `threads` threads.
fn load(data: &[u8], threads: usize) -> Result<JxlImage> {
    let pool = if threads > 1 {
        JxlThreadPool::rayon(Some(threads))
    } else {
        JxlThreadPool::none()
    };
    let img = JxlImage::builder()
        .pool(pool)
        .read(std::io::Cursor::new(data))
        .map_err(err)?;
    let s = &img.image_header().size;
    check_size(s.width as u64, s.height as u64)?;
    Ok(img)
}

/// sRGB output needs no profile (CICP: BT.709 primaries, sRGB transfer).
fn profile(img: &JxlImage) -> Option<Profile> {
    if img.rendered_cicp().is_some_and(|c| c[..2] == [1, 13]) {
        return None;
    }
    Some(Profile::new(img.rendered_icc()))
}

fn orientation(img: &JxlImage) -> Orientation {
    match img.image_header().metadata.orientation {
        o @ 1..=8 => Orientation::from_exif(o as u16),
        _ => Orientation::NORMAL,
    }
}

/// Stored size, orientation and colour profile of the rendered pixels,
/// without decoding them.
pub fn info(data: &[u8]) -> Result<((u32, u32), Orientation, Option<Profile>)> {
    let img = header(data)?;
    let s = &img.image_header().size;
    check_size(s.width as u64, s.height as u64)?;
    Ok(((s.width, s.height), orientation(&img), profile(&img)))
}

/// Row `y` of a channel as 8-bit samples, converted like jxl-oxide's own
/// streams: floats are nominally 0..1, integers have the given bit depth.
/// `None` for float samples stored as integers.
fn row_u8(grid: &ImageBuffer, depth: BitDepth, y: usize, out: &mut [u8]) -> Option<()> {
    let bits = match depth {
        BitDepth::IntegerSample { bits_per_sample } => bits_per_sample,
        BitDepth::FloatSample { .. } => 0,
    };
    let int = |v: i32| -> u8 {
        if bits == 8 {
            v.clamp(0, 255) as u8
        } else {
            let f = v as f32 / ((1i64 << bits) - 1) as f32;
            (f * 255.0 + 0.5).clamp(0.0, 255.0) as u8
        }
    };
    match grid {
        ImageBuffer::F32(g) => {
            for (o, &v) in out.iter_mut().zip(g.try_get_row(y)?) {
                *o = (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
        ImageBuffer::I32(g) if bits > 0 => {
            for (o, &v) in out.iter_mut().zip(g.try_get_row(y)?) {
                *o = int(v);
            }
        }
        ImageBuffer::I16(g) if bits > 0 => {
            for (o, &v) in out.iter_mut().zip(g.try_get_row(y)?) {
                *o = int(v as i32);
            }
        }
        _ => return None,
    }
    Some(())
}

/// One rendered frame as RGBA8 in stored orientation. Channels start at
/// the image origin and may be padded to whole blocks; smaller ones
/// (offset regions) and CMYK report `Unsupported`, which leaves them to the
/// platform decoder.
fn image(img: &JxlImage, render: &Render) -> Result<Image> {
    let unsupported = || DecodeError::Unsupported(Format::Jxl);
    let header = img.image_header();
    let (w, h) = (header.size.width, header.size.height);
    let (gray, alpha) = match img.pixel_format() {
        PixelFormat::Gray => (true, false),
        PixelFormat::Graya => (true, true),
        PixelFormat::Rgb => (false, false),
        PixelFormat::Rgba => (false, true),
        PixelFormat::Cmyk | PixelFormat::Cmyka => return Err(unsupported()),
    };
    let (infos, extra) = render.extra_channels();
    let alpha_idx = infos.iter().position(|e| e.is_alpha()).filter(|_| alpha);
    let mut grids: Vec<(&ImageBuffer, BitDepth)> = render
        .color_channels()
        .iter()
        .map(|g| (g, header.metadata.bit_depth))
        .collect();
    if let Some(i) = alpha_idx {
        let depth = header
            .metadata
            .ec_info
            .get(i)
            .ok_or_else(unsupported)?
            .bit_depth;
        grids.push((extra.get(i).ok_or_else(unsupported)?, depth));
    }
    let expected = if gray { 1 } else { 3 } + usize::from(alpha_idx.is_some());
    if grids.len() != expected
        || grids
            .iter()
            .any(|(g, _)| g.width() < w as usize || g.height() < h as usize)
    {
        return Err(unsupported());
    }

    let mut plane = Plane::new(w, h, 4);
    let stride = plane.stride;
    let mut tmp = vec![0u8; w as usize];
    for y in 0..h as usize {
        let row = &mut plane.data[y * stride..y * stride + w as usize * 4];
        let px = row.as_chunks_mut::<4>().0;
        if alpha_idx.is_none() {
            px.iter_mut().for_each(|p| p[3] = 255);
        }
        for (c, (g, depth)) in grids.iter().enumerate() {
            row_u8(g, *depth, y, &mut tmp).ok_or_else(unsupported)?;
            match (gray, c) {
                (true, 0) => px.iter_mut().zip(&tmp).for_each(|(p, &v)| {
                    p[0] = v;
                    p[1] = v;
                    p[2] = v;
                }),
                (true, _) => px.iter_mut().zip(&tmp).for_each(|(p, &v)| p[3] = v),
                (false, c) => px.iter_mut().zip(&tmp).for_each(|(p, &v)| p[c] = v),
            }
        }
    }
    let transparent =
        alpha_idx.is_some() && plane.data.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
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

/// The still image (an animation's first frame) on up to `threads`
/// threads.
pub fn decode(data: &[u8], threads: usize) -> Result<Image> {
    let img = load(data, threads)?;
    let render = img.render_frame(0).map_err(err)?;
    image(&img, &render)
}

/// Every frame of an animated JPEG XL as long as they fit into `budget`
/// bytes; `None` for a still image.
pub fn decode_animation(data: &[u8], budget: usize) -> Result<Option<Animation>> {
    if header(data)?.image_header().metadata.animation.is_none() {
        return Ok(None);
    }
    let img = load(data, 1)?;
    let Some(anim) = img.image_header().metadata.animation.as_ref() else {
        return Ok(None);
    };
    let count = img.num_loaded_keyframes();
    let s = &img.image_header().size;
    let frame_bytes = s.width as usize * s.height as usize * 4;
    if count < 2 || anim.tps_numerator == 0 {
        return Ok(None);
    }
    let mut frames = Vec::new();
    while frames.len() < count && (frames.len() + 1) * frame_bytes <= budget {
        let render = img.render_frame(frames.len()).map_err(err)?;
        let ticks = render.duration() as u64;
        let ms = ticks * 1000 * anim.tps_denominator as u64 / anim.tps_numerator as u64;
        frames.push(AnimFrame {
            image: image(&img, &render)?,
            delay_ms: super::min_delay_ms(ms.min(u32::MAX as u64) as u32),
        });
    }
    if frames.len() < 2 {
        return Ok(None);
    }
    let plays = (anim.num_loops > 0).then_some(anim.num_loops);
    Ok(Some(Animation { frames, plays }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zune_core::bit_depth::BitDepth as ZBitDepth;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::EncoderOptions;
    use zune_jpegxl::JxlSimpleEncoder;

    /// 8×6 of 0x3080C0, lossy (ffmpeg/libjxl, distance 1).
    const LOSSY: [u8; 60] = [
        0xff, 0x0a, 0x28, 0xb0, 0x01, 0x00, 0x13, 0x88, 0x02, 0x00, 0xc0, 0x00, 0xb5, 0x9f, 0x20,
        0x00, 0x00, 0x15, 0x2a, 0xa3, 0x8c, 0x1b, 0xbc, 0x9c, 0xeb, 0xf9, 0xf2, 0x43, 0x87, 0xc5,
        0xb4, 0x8d, 0xeb, 0x0c, 0x6d, 0xb5, 0x6d, 0x61, 0x09, 0x63, 0xb3, 0xbd, 0x30, 0x48, 0x48,
        0x38, 0x83, 0x8d, 0x8b, 0x4c, 0xdc, 0x9c, 0x8e, 0x16, 0xa1, 0x06, 0x64, 0x44, 0x25, 0x09,
    ];

    fn lossless(w: u32, h: u32, cs: ColorSpace, samples: &[u8]) -> Vec<u8> {
        let opts = EncoderOptions::new(w as usize, h as usize, cs, ZBitDepth::Eight);
        let mut out = Vec::new();
        JxlSimpleEncoder::new(samples, opts)
            .encode(&mut out)
            .unwrap();
        out
    }

    fn rgba_of(img: &Image) -> Vec<u8> {
        img.to_bgra()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect()
    }

    /// jxl-oxide's generic stream for the same file, as RGBA.
    fn via_stream(data: &[u8]) -> Vec<u8> {
        let img = load(data, 1).unwrap();
        let render = img.render_frame(0).unwrap();
        let mut stream = render.stream();
        let c = stream.channels() as usize;
        let mut buf = vec![0u8; stream.width() as usize * stream.height() as usize * c];
        stream.write_to_buffer(&mut buf);
        buf.chunks(c)
            .flat_map(|p| match p {
                [g] => [*g, *g, *g, 255],
                [g, a] => [*g, *g, *g, *a],
                [r, g, b] => [*r, *g, *b, 255],
                [r, g, b, a] => [*r, *g, *b, *a],
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn lossless_round_trips() {
        let (w, h) = (37, 23);
        let rgba = crate::testimg::rgba(w, h);
        let rgb = crate::testimg::rgb(w, h);
        let gray = crate::testimg::gray(w, h);
        let gray_alpha: Vec<u8> = gray
            .iter()
            .zip(rgba.chunks(4))
            .flat_map(|(&g, p)| [g, p[3]])
            .collect();
        let expand = |px: &[u8], f: fn(&[u8]) -> [u8; 4], n: usize| -> Vec<u8> {
            px.chunks(n).flat_map(f).collect()
        };
        let cases = [
            (ColorSpace::RGBA, rgba.clone(), rgba.clone(), true),
            (
                ColorSpace::RGB,
                rgb.clone(),
                expand(&rgb, |p| [p[0], p[1], p[2], 255], 3),
                false,
            ),
            (
                ColorSpace::Luma,
                gray.clone(),
                expand(&gray, |p| [p[0], p[0], p[0], 255], 1),
                false,
            ),
            (
                ColorSpace::LumaA,
                gray_alpha.clone(),
                expand(&gray_alpha, |p| [p[0], p[0], p[0], p[1]], 2),
                true,
            ),
        ];
        for (cs, samples, want, alpha) in cases {
            let data = lossless(w, h, cs, &samples);
            let (size, o, icc) = info(&data).unwrap();
            assert_eq!((size, o), ((w, h), Orientation::NORMAL), "{cs:?}");
            assert!(icc.is_none() || cs.is_grayscale(), "{cs:?}");
            let img = decode(&data, 1).unwrap();
            assert_eq!(img.has_alpha(), alpha, "{cs:?}");
            assert_eq!(rgba_of(&img), want, "{cs:?}");
            assert_eq!(rgba_of(&decode(&data, 4).unwrap()), want, "{cs:?} threaded");
            assert_eq!(via_stream(&data), want, "{cs:?} stream");
            assert!(decode_animation(&data, usize::MAX).unwrap().is_none());
        }
    }

    #[test]
    fn lossy_matches_the_generic_stream() {
        let (size, o, icc) = info(&LOSSY).unwrap();
        assert_eq!((size, o, icc), ((8, 6), Orientation::NORMAL, None));
        let img = decode(&LOSSY, 1).unwrap();
        let px = rgba_of(&img);
        assert_eq!(px, via_stream(&LOSSY));
        let near = |a: u8, b: u8| a.abs_diff(b) <= 3;
        assert!(
            near(px[0], 0x30) && near(px[1], 0x80) && near(px[2], 0xC0),
            "{:?}",
            &px[..4]
        );
    }

    #[test]
    fn rejects_broken_files() {
        assert!(info(&LOSSY[..3]).is_err());
        assert!(decode(&LOSSY[..30], 1).is_err());
        assert!(info(b"\xff\x0a").is_err());
    }
}
