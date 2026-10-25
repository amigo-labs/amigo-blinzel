//! Encoders for resized copies: JPEG (libjpeg-turbo), PNG (`png`), WebP
//! (libwebp) and AVIF (rav1e through `ravif`). Each takes BGRA8 with
//! straight alpha plus the EXIF block and ICC profile to embed.

use ravif::{Img, RGBA8};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Jpeg,
    Png,
    Webp,
    Avif,
}

impl Codec {
    pub fn extension(self) -> &'static str {
        match self {
            Codec::Jpeg => "jpg",
            Codec::Png => "png",
            Codec::Webp => "webp",
            Codec::Avif => "avif",
        }
    }

    pub fn has_alpha(self) -> bool {
        self != Codec::Jpeg
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quality {
    /// 1..=100
    pub jpeg: u8,
    /// 0..=100; ignored when lossless
    pub webp: u8,
    pub webp_lossless: bool,
    /// 1..=100
    pub avif: u8,
    /// rav1e speed 1 (slow, small) ..= 10 (fast)
    pub avif_speed: u8,
}

impl Default for Quality {
    fn default() -> Self {
        Quality {
            jpeg: 85,
            webp: 80,
            webp_lossless: false,
            avif: 70,
            avif_speed: 6,
        }
    }
}

/// Encodes `bgra` (`w`×`h`). JPEG has no alpha: transparent images are
/// put on white first.
pub fn encode(
    bgra: &[u8],
    w: u32,
    h: u32,
    codec: Codec,
    q: &Quality,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    assert_eq!(bgra.len(), w as usize * h as usize * 4);
    match codec {
        Codec::Jpeg => jpeg(bgra, w, h, q.jpeg, exif, icc),
        Codec::Png => png(bgra, w, h, exif, icc),
        Codec::Webp => webp(bgra, w, h, q, exif, icc),
        Codec::Avif => avif(bgra, w, h, q, exif, icc),
    }
}

fn is_opaque(bgra: &[u8]) -> bool {
    bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
}

#[cfg(feature = "turbojpeg")]
fn jpeg(
    bgra: &[u8],
    w: u32,
    h: u32,
    quality: u8,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let flat;
    let px = if is_opaque(bgra) {
        bgra
    } else {
        flat = on_white(bgra);
        &flat
    };
    // Full chroma at high quality, where 4:2:0 would be the visible loss.
    let subsampling = if quality >= 90 { 444 } else { 420 };
    let mut out =
        crate::decode::jpeg::encode_bgra(px, w, h, quality.clamp(1, 100), subsampling, icc)
            .map_err(|e| e.to_string())?;
    if let Some(exif) = exif {
        insert_exif(&mut out, exif);
    }
    Ok(out)
}

#[cfg(not(feature = "turbojpeg"))]
fn jpeg(
    _: &[u8],
    _: u32,
    _: u32,
    _: u8,
    _: Option<&[u8]>,
    _: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    Err("JPEG encoding needs the turbojpeg feature".into())
}

#[cfg(feature = "turbojpeg")]
fn on_white(bgra: &[u8]) -> Vec<u8> {
    bgra.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as u32;
            let mix = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            [mix(p[0]), mix(p[1]), mix(p[2]), 255]
        })
        .collect()
}

#[cfg(feature = "turbojpeg")]
/// Puts an `Exif` APP1 segment right after SOI and the JFIF APP0, if any.
/// Blocks too large for one segment are left out.
fn insert_exif(jpeg: &mut Vec<u8>, exif: &[u8]) {
    let len = exif.len() + 8;
    if len > 0xFFFF || jpeg.len() < 4 {
        return;
    }
    let mut at = 2;
    if jpeg[2..4] == [0xFF, 0xE0]
        && let Some(l) = jpeg.get(4..6)
    {
        at = 4 + u16::from_be_bytes([l[0], l[1]]) as usize;
    }
    let mut seg = vec![0xFF, 0xE1];
    seg.extend((len as u16).to_be_bytes());
    seg.extend(b"Exif\0\0");
    seg.extend(exif);
    jpeg.splice(at..at, seg);
}

fn png(
    bgra: &[u8],
    w: u32,
    h: u32,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let opaque = is_opaque(bgra);
    let px = bgra.as_chunks::<4>().0;
    let (color, data): (png::ColorType, Vec<u8>) = if opaque {
        (
            png::ColorType::Rgb,
            px.iter().flat_map(|p| [p[2], p[1], p[0]]).collect(),
        )
    } else {
        (
            png::ColorType::Rgba,
            px.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect(),
        )
    };
    let mut info = png::Info::with_size(w, h);
    info.color_type = color;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = icc.map(Into::into);
    info.exif_metadata = exif.map(Into::into);
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::with_info(&mut out, info).map_err(|e| e.to_string())?;
        enc.set_compression(png::Compression::Balanced);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn webp(
    bgra: &[u8],
    w: u32,
    h: u32,
    q: &Quality,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    if w > 16383 || h > 16383 {
        return Err("WebP is limited to 16383×16383 pixels".into());
    }
    let mut ptr = std::ptr::null_mut();
    let (wi, hi, stride) = (w as i32, h as i32, w as i32 * 4);
    // SAFETY: `bgra` holds `w`·`h` BGRA pixels with that stride; libwebp
    // allocates the output, which is copied and freed right after.
    let len = unsafe {
        if q.webp_lossless {
            libwebp_sys::WebPEncodeLosslessBGRA(bgra.as_ptr(), wi, hi, stride, &mut ptr)
        } else {
            libwebp_sys::WebPEncodeBGRA(bgra.as_ptr(), wi, hi, stride, q.webp as f32, &mut ptr)
        }
    };
    if ptr.is_null() || len == 0 {
        return Err("WebP encoding failed".into());
    }
    let simple = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { libwebp_sys::WebPFree(ptr.cast()) };
    if exif.is_none() && icc.is_none() {
        return Ok(simple);
    }
    webp_extended(&simple, w, h, !is_opaque(bgra), exif, icc)
}

/// Rewrites a simple WebP (`VP8 `/`VP8L`, or `VP8X` with `ALPH`) as the
/// extended format with `ICCP` and `EXIF` chunks.
fn webp_extended(
    simple: &[u8],
    w: u32,
    h: u32,
    alpha: bool,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let mut image = Vec::new();
    let mut pos = 12;
    while let Some(head) = simple.get(pos..pos + 8) {
        let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
        let padded = len + (len & 1);
        let chunk = simple
            .get(pos..pos + 8 + padded)
            .ok_or("libwebp wrote a truncated file")?;
        if matches!(&head[..4], b"ALPH" | b"VP8 " | b"VP8L") {
            image.extend_from_slice(chunk);
        }
        pos += 8 + padded;
    }
    let chunk = |out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]| {
        out.extend(kind);
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
    };
    let mut flags = 0u8;
    if icc.is_some() {
        flags |= 0x20;
    }
    if alpha {
        flags |= 0x10;
    }
    if exif.is_some() {
        flags |= 0x08;
    }
    let mut vp8x = vec![flags, 0, 0, 0];
    vp8x.extend(&(w - 1).to_le_bytes()[..3]);
    vp8x.extend(&(h - 1).to_le_bytes()[..3]);
    let mut body = b"WEBP".to_vec();
    chunk(&mut body, b"VP8X", &vp8x);
    if let Some(icc) = icc {
        chunk(&mut body, b"ICCP", icc);
    }
    body.extend(image);
    if let Some(exif) = exif {
        chunk(&mut body, b"EXIF", exif);
    }
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    Ok(out)
}

fn avif(
    bgra: &[u8],
    w: u32,
    h: u32,
    q: &Quality,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let rgba: Vec<RGBA8> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| RGBA8::new(p[2], p[1], p[0], p[3]))
        .collect();
    let mut enc = ravif::Encoder::new()
        .with_quality(q.avif.clamp(1, 100) as f32)
        .with_alpha_quality(q.avif.clamp(1, 100) as f32)
        .with_speed(q.avif_speed.clamp(1, 10))
        .with_bit_depth(ravif::BitDepth::Eight)
        .with_num_threads(Some(1));
    if let Some(exif) = exif {
        enc = enc.with_exif(exif);
    }
    let out = enc
        .encode_rgba(Img::new(&rgba[..], w as usize, h as usize))
        .map_err(|e| e.to_string())?
        .avif_file;
    Ok(match icc {
        Some(icc) => avif_with_icc(&out, icc).unwrap_or(out),
        None => out,
    })
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// Children of a box body: (start of the child's header, total size, kind).
fn children(d: &[u8]) -> Option<Vec<(usize, usize, [u8; 4])>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < d.len() {
        let size = be32(d, pos)? as usize;
        if size < 8 || pos + size > d.len() {
            return None;
        }
        out.push((pos, size, d[pos + 4..pos + 8].try_into().ok()?));
        pos += size;
    }
    Some(out)
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend(kind);
    b.extend(body);
    b
}

/// Adds a `colr` box of type `prof` with `icc` to the primary item of an
/// AVIF written by `avif-serialize`: appended to `ipco`, associated in
/// `ipma`, and every `iloc` offset behind `meta` moved by the growth.
/// `None` when the layout is not the expected one (the file then keeps
/// its nclx description only).
fn avif_with_icc(file: &[u8], icc: &[u8]) -> Option<Vec<u8>> {
    let top = children(file)?;
    let &(meta_at, meta_size, _) = top.iter().find(|(_, _, k)| k == b"meta")?;
    let meta = &file[meta_at + 8..meta_at + meta_size];
    let fullbox = meta.get(..4)?;
    let kids = children(&meta[4..])?;
    let body = |at: usize, size: usize| &meta[4 + at + 8..4 + at + size];

    let &(pitm_at, pitm_size, _) = kids.iter().find(|(_, _, k)| k == b"pitm")?;
    let pitm = body(pitm_at, pitm_size);
    let primary = if pitm[0] == 0 {
        u16::from_be_bytes(pitm.get(4..6)?.try_into().ok()?) as u32
    } else {
        be32(pitm, 4)?
    };

    let &(iprp_at, iprp_size, _) = kids.iter().find(|(_, _, k)| k == b"iprp")?;
    let iprp = body(iprp_at, iprp_size);
    let iprp_kids = children(iprp)?;
    let &(ipco_at, ipco_size, _) = iprp_kids.iter().find(|(_, _, k)| k == b"ipco")?;
    let &(ipma_at, ipma_size, _) = iprp_kids.iter().find(|(_, _, k)| k == b"ipma")?;
    let ipco = &iprp[ipco_at + 8..ipco_at + ipco_size];
    let ipma = &iprp[ipma_at + 8..ipma_at + ipma_size];

    let mut colr = b"prof".to_vec();
    colr.extend(icc);
    let mut new_ipco = ipco.to_vec();
    new_ipco.extend(boxed(b"colr", &colr));
    let index = children(ipco)?.len() + 1;

    // ipma: version, flags, entries of (item, count, associations).
    let (v, wide) = (ipma[0], ipma[3] & 1 == 1);
    if (!wide && index > 127) || index > 0x7FFF {
        return None;
    }
    let mut new_ipma = ipma[..8].to_vec();
    let count = be32(ipma, 4)?;
    let mut pos = 8;
    let mut found = false;
    for _ in 0..count {
        let id_len = if v < 1 { 2 } else { 4 };
        let id = if v < 1 {
            u16::from_be_bytes(ipma.get(pos..pos + 2)?.try_into().ok()?) as u32
        } else {
            be32(ipma, pos)?
        };
        let n = *ipma.get(pos + id_len)? as usize;
        let assoc_len = n * if wide { 2 } else { 1 };
        let assocs = ipma.get(pos + id_len + 1..pos + id_len + 1 + assoc_len)?;
        new_ipma.extend(&ipma[pos..pos + id_len]);
        if id == primary {
            found = true;
            new_ipma.push(u8::try_from(n + 1).ok()?);
            new_ipma.extend(assocs);
            if wide {
                new_ipma.extend((index as u16).to_be_bytes());
            } else {
                new_ipma.push(index as u8);
            }
        } else {
            new_ipma.push(n as u8);
            new_ipma.extend(assocs);
        }
        pos += id_len + 1 + assoc_len;
    }
    if !found {
        return None;
    }

    let mut new_iprp = Vec::new();
    for &(at, size, kind) in &iprp_kids {
        match &kind {
            b"ipco" => new_iprp.extend(boxed(b"ipco", &new_ipco)),
            b"ipma" => new_iprp.extend(boxed(b"ipma", &new_ipma)),
            _ => new_iprp.extend(&iprp[at..at + size]),
        }
    }
    let delta = (new_iprp.len() + 8 - iprp_size) as u64;
    let meta_end = (meta_at + meta_size) as u64;

    let mut new_meta = fullbox.to_vec();
    for &(at, size, kind) in &kids {
        match &kind {
            b"iprp" => new_meta.extend(boxed(b"iprp", &new_iprp)),
            b"iloc" => {
                let mut iloc = meta[4 + at..4 + at + size].to_vec();
                shift_iloc(&mut iloc[8..], meta_end, delta)?;
                new_meta.extend(iloc);
            }
            _ => new_meta.extend(&meta[4 + at..4 + at + size]),
        }
    }

    let mut out = file[..meta_at].to_vec();
    out.extend(boxed(b"meta", &new_meta));
    out.extend(&file[meta_at + meta_size..]);
    Some(out)
}

/// Moves every file offset in an `iloc` body that points behind `from` by
/// `delta` (construction method 0 only).
fn shift_iloc(b: &mut [u8], from: u64, delta: u64) -> Option<()> {
    let v = *b.first()?;
    let sizes = *b.get(4)?;
    let (off_size, len_size) = ((sizes >> 4) as usize, (sizes & 15) as usize);
    let sizes = *b.get(5)?;
    let base_size = (sizes >> 4) as usize;
    let index_size = if v >= 1 { (sizes & 15) as usize } else { 0 };
    let mut pos = 6;
    let count = if v < 2 {
        let c = u16::from_be_bytes(b.get(pos..pos + 2)?.try_into().ok()?) as u32;
        pos += 2;
        c
    } else {
        let c = be32(b, pos)?;
        pos += 4;
        c
    };
    let read = |b: &[u8], at: usize, n: usize| -> Option<u64> {
        let s = b.get(at..at + n)?;
        Some(s.iter().fold(0u64, |a, &x| a << 8 | x as u64))
    };
    let write = |b: &mut [u8], at: usize, n: usize, v: u64| -> Option<()> {
        if n < 8 && v >> (8 * n) != 0 {
            return None;
        }
        for (i, x) in b.get_mut(at..at + n)?.iter_mut().enumerate() {
            *x = (v >> (8 * (n - 1 - i))) as u8;
        }
        Some(())
    };
    for _ in 0..count {
        pos += if v < 2 { 2 } else { 4 };
        let method = if v >= 1 {
            let m = read(b, pos, 2)? & 15;
            pos += 2;
            m
        } else {
            0
        };
        pos += 2; // data_reference_index
        let base_at = pos;
        let base = read(b, pos, base_size)?;
        pos += base_size;
        let extents = read(b, pos, 2)? as usize;
        pos += 2;
        let mut base_shifted = false;
        if method == 0 && base_size > 0 && base >= from {
            write(b, base_at, base_size, base + delta)?;
            base_shifted = true;
        }
        for _ in 0..extents {
            pos += index_size;
            let off = read(b, pos, off_size)?;
            if method == 0 && !base_shifted && off_size > 0 && base + off >= from {
                write(b, pos, off_size, off + delta)?;
            }
            pos += off_size + len_size;
        }
    }
    Some(())
}

// The tests write JPEGs, which needs libjpeg-turbo.
#[cfg(all(test, feature = "turbojpeg"))]
mod tests {
    use super::*;
    use crate::decode::{self, Request};
    use crate::format::{self, Format};

    fn gradient(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend([
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    128,
                    if alpha && y >= h / 2 { 0 } else { 255 },
                ]);
            }
        }
        px
    }

    fn icc() -> Vec<u8> {
        crate::icc::matrix_profile(
            crate::icc::colorants::DISPLAY_P3,
            crate::icc::Trc::Gamma(2.2),
            "Test",
        )
    }

    #[test]
    fn every_codec_round_trips_with_metadata() {
        let (w, h) = (48, 32);
        let exif = crate::batch::meta::rewrite(&crate::batch::meta::tests::sample(), (w, h), false)
            .unwrap();
        let icc = icc();
        for codec in [Codec::Jpeg, Codec::Png, Codec::Webp, Codec::Avif] {
            for alpha in [false, true] {
                let px = gradient(w, h, alpha);
                let file = encode(
                    &px,
                    w,
                    h,
                    codec,
                    &Quality::default(),
                    Some(&exif),
                    Some(&icc),
                )
                .unwrap_or_else(|e| panic!("{codec:?}: {e}"));
                let fmt = format::detect(&file);
                assert_eq!(
                    fmt,
                    match codec {
                        Codec::Jpeg => Format::Jpeg,
                        Codec::Png => Format::Png,
                        Codec::Webp => Format::WebP,
                        Codec::Avif => Format::Avif,
                    }
                );
                let probe = decode::probe(&file).unwrap_or_else(|e| panic!("{codec:?}: {e}"));
                assert_eq!((probe.width, probe.height), (w, h), "{codec:?}");
                let got_icc = probe.icc.map(|p| p.bytes.to_vec());
                assert_eq!(got_icc.as_deref(), Some(&icc[..]), "{codec:?} ICC");
                let got_exif = crate::batch::meta::read_exif(&file, fmt);
                assert_eq!(got_exif.as_deref(), Some(&exif[..]), "{codec:?} EXIF");

                let img = decode::decode(&file, Request::Full).unwrap();
                let back = img.to_bgra();
                let at = |x: u32, y: u32| &back[((y * w + x) * 4) as usize..][..4];
                let want = &px[((5 * w + 40) * 4) as usize..][..4];
                let tol = if codec == Codec::Png { 0 } else { 12 };
                for (got, want) in at(40, 5).iter().zip(want).take(3) {
                    assert!(got.abs_diff(*want) <= tol, "{codec:?} {:?}", at(40, 5));
                }
                if alpha && codec.has_alpha() {
                    assert!(at(10, 28)[3] <= 8, "{codec:?} alpha {:?}", at(10, 28));
                } else if alpha {
                    assert!(at(10, 28)[..3].iter().all(|&c| c >= 240), "on white");
                }
            }
        }
    }

    #[test]
    fn webp_lossless_is_exact() {
        let (w, h) = (17, 9);
        let px = gradient(w, h, true);
        let q = Quality {
            webp_lossless: true,
            ..Default::default()
        };
        let file = encode(&px, w, h, Codec::Webp, &q, None, None).unwrap();
        let img = decode::decode(&file, Request::Full).unwrap();
        let back = img.to_bgra();
        for (a, b) in back.as_chunks::<4>().0.iter().zip(px.as_chunks::<4>().0) {
            if b[3] != 0 {
                assert_eq!(a, b);
            }
        }
    }
}
