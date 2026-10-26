//! Metadata for resized copies: the EXIF block of the source, rewritten.
//!
//! The source's TIFF structure is not patched but written anew (little
//! endian), which keeps the result small and lets entries go cleanly:
//!
//! - Orientation becomes 1 (the pixels are already turned).
//! - `PixelXDimension`/`PixelYDimension` get the new size.
//! - The IFD1 thumbnail and the maker notes are dropped. The thumbnail
//!   shows the uncropped original, and maker notes hold offsets that break
//!   when moved (and serial numbers).
//! - GPS goes on request.
//!
//! Every read is bounds-checked; a malformed block yields `None`.

use super::naming::Stamp;
use crate::exif::{Entry, Tiff};
use crate::format::Format;

const ORIENTATION: u16 = 0x0112;
const EXIF_IFD: u16 = 0x8769;
const GPS_IFD: u16 = 0x8825;
const INTEROP_IFD: u16 = 0xA005;
const PIXEL_X: u16 = 0xA002;
const PIXEL_Y: u16 = 0xA003;
const DATE_TAKEN: u16 = 0x9003;
/// Dropped wherever they appear: they describe the source's own image
/// data (TIFF strips, sizes, thumbnails) or cannot be moved (maker notes).
const DROPPED: [u16; 10] = [
    0x00FE, 0x0100, 0x0101, 0x0103, 0x0111, 0x0115, 0x0116, 0x0117, 0x0201, 0x927C,
];

/// The raw EXIF block (TIFF, starting with `II*\0`/`MM\0*`) of a source
/// file, wherever its format keeps it.
pub fn read_exif(data: &[u8], format: Format) -> Option<Vec<u8>> {
    let tiff = match format {
        Format::Jpeg => blinzel_jpeg::Segments::new(data)
            .filter(|s| s.marker == 0xE1)
            .find_map(|s| {
                data[s.payload]
                    .strip_prefix(b"Exif\0\0")
                    .map(<[u8]>::to_vec)
            }),
        Format::Png => png_chunk(data, b"eXIf").map(<[u8]>::to_vec),
        Format::WebP => {
            riff_chunk(data, b"EXIF").map(|d| d.strip_prefix(b"Exif\0\0").unwrap_or(d).to_vec())
        }
        Format::Avif | Format::Heif => crate::decode::avif::exif(data),
        Format::Jxl => isobmff_box(data, b"Exif").and_then(|d| {
            let skip = u32::from_be_bytes(d.get(..4)?.try_into().ok()?) as usize;
            d.get(skip.checked_add(4)?..).map(<[u8]>::to_vec)
        }),
        _ => None,
    }?;
    Tiff::new(&tiff)?;
    Some(tiff)
}

fn png_chunk<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut pos = 8;
    while let Some(head) = data.get(pos..pos + 8) {
        let len = u32::from_be_bytes(head[..4].try_into().ok()?) as usize;
        let body = data.get(pos + 8..(pos + 8).checked_add(len)?)?;
        if &head[4..] == kind {
            return Some(body);
        }
        if &head[4..] == b"IEND" {
            return None;
        }
        pos += 12 + len;
    }
    None
}

fn riff_chunk<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut pos = 12;
    while let Some(head) = data.get(pos..pos + 8) {
        let len = u32::from_le_bytes(head[4..].try_into().ok()?) as usize;
        let body = data.get(pos + 8..(pos + 8).checked_add(len)?)?;
        if &head[..4] == kind {
            return Some(body);
        }
        pos += 8 + len + (len & 1);
    }
    None
}

/// A top-level box of an ISOBMFF-style file (the JPEG XL container).
fn isobmff_box<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut pos = 0usize;
    while let Some(head) = data.get(pos..pos + 8) {
        let size = u32::from_be_bytes(head[..4].try_into().ok()?) as usize;
        let (hdr, size) = match size {
            0 => (8, data.len() - pos),
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(
                    data.get(pos + 8..pos + 16)?.try_into().ok()?,
                ))
                .ok()?,
            ),
            s => (8, s),
        };
        if size < hdr {
            return None;
        }
        let body = data.get(pos + hdr..pos.checked_add(size)?)?;
        if &head[4..] == kind {
            return Some(body);
        }
        pos += size;
    }
    None
}

/// The capture time (DateTimeOriginal, `YYYY:MM:DD hh:mm:ss`); the time
/// is midnight where only the date is readable.
pub fn date_taken(tiff: &[u8]) -> Option<Stamp> {
    let t = Tiff::new(tiff)?;
    let (ifd0, _) = t.ifd(t.first_ifd()?)?;
    let exif = t.int(ifd0.iter().find(|e| e.tag == EXIF_IFD)?)? as usize;
    let (entries, _) = t.ifd(exif)?;
    let e = entries.iter().find(|e| e.tag == DATE_TAKEN && e.typ == 2)?;
    let b = tiff.get(t.payload(e)?)?;
    let num = |r: std::ops::Range<usize>| -> Option<u16> {
        let d = b.get(r)?;
        if !d.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(d).ok()?.parse().ok()
    };
    let sep = |i: usize, ok: &[u8]| b.get(i).is_some_and(|c| ok.contains(c));
    if !(sep(4, b":-") && sep(7, b":-")) {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if year == 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let time = (sep(10, b" T") && sep(13, b":") && sep(16, b":"))
        .then(|| Some((num(11..13)?, num(14..16)?, num(17..19)?)))
        .flatten()
        .filter(|&(h, m, s)| h < 24 && m < 60 && s < 61)
        .unwrap_or_default();
    Some(Stamp {
        year,
        month: month as u8,
        day: day as u8,
        hour: time.0 as u8,
        minute: time.1 as u8,
        second: time.2 as u8,
    })
}

/// One entry to write: tag, type, count and the value bytes in little
/// endian.
struct Out {
    tag: u16,
    typ: u16,
    count: u32,
    value: Vec<u8>,
    /// Points at a sub-IFD written later.
    sub: Option<Vec<Out>>,
}

fn element_size(typ: u16) -> Option<usize> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

/// The entry's value converted to little endian.
fn value_le(t: &Tiff, d: &[u8], e: &Entry) -> Option<Vec<u8>> {
    let r = t.payload(e)?;
    let size = element_size(e.typ)?;
    let raw = d.get(r)?;
    let mut out = Vec::with_capacity(raw.len());
    match (e.typ, size) {
        (_, 1) => out.extend_from_slice(raw),
        (_, 2) => (0..raw.len() / 2).for_each(|i| out.extend(t.u16_at(raw, i * 2).to_le_bytes())),
        (5 | 10, _) | (_, 4) => {
            (0..raw.len() / 4).for_each(|i| out.extend(t.u32_at(raw, i * 4).to_le_bytes()))
        }
        // DOUBLE: swap all eight bytes when big endian.
        _ => raw.as_chunks::<8>().0.iter().for_each(|&c| {
            let mut c = c;
            if !t.little_endian() {
                c.reverse();
            }
            out.extend(c);
        }),
    }
    Some(out)
}

fn read_ifd(
    t: &Tiff,
    d: &[u8],
    off: usize,
    keep_gps: bool,
    size: (u32, u32),
    depth: u8,
) -> Option<Vec<Out>> {
    if depth > 2 {
        return None;
    }
    let (entries, _) = t.ifd(off)?;
    let mut out = Vec::new();
    for e in &entries {
        if DROPPED.contains(&e.tag) || (e.tag == GPS_IFD && !keep_gps) {
            continue;
        }
        if matches!(e.tag, EXIF_IFD | GPS_IFD | INTEROP_IFD) {
            let Some(sub) = t
                .int(e)
                .and_then(|o| read_ifd(t, d, o as usize, keep_gps, size, depth + 1))
            else {
                continue;
            };
            out.push(Out {
                tag: e.tag,
                typ: 4,
                count: 1,
                value: vec![0; 4],
                sub: Some(sub),
            });
            continue;
        }
        let (typ, count, value) = match e.tag {
            ORIENTATION => (3, 1, 1u16.to_le_bytes().to_vec()),
            PIXEL_X => (4, 1, size.0.to_le_bytes().to_vec()),
            PIXEL_Y => (4, 1, size.1.to_le_bytes().to_vec()),
            _ => match value_le(t, d, e) {
                Some(v) => (e.typ, e.count, v),
                None => continue,
            },
        };
        out.push(Out {
            tag: e.tag,
            typ,
            count,
            value,
            sub: None,
        });
    }
    out.sort_by_key(|o| o.tag);
    out.dedup_by_key(|o| o.tag);
    Some(out)
}

/// Writes `ifd` at the end of `buf` (word aligned) and returns its offset.
fn write_ifd(buf: &mut Vec<u8>, ifd: &[Out]) -> u32 {
    if buf.len() % 2 == 1 {
        buf.push(0);
    }
    let at = buf.len();
    buf.extend((ifd.len() as u16).to_le_bytes());
    buf.resize(at + 2 + 12 * ifd.len() + 4, 0);
    for (i, o) in ifd.iter().enumerate() {
        let e = at + 2 + 12 * i;
        let value = match &o.sub {
            Some(sub) => write_ifd(buf, sub).to_le_bytes().to_vec(),
            None if o.value.len() > 4 => {
                if buf.len() % 2 == 1 {
                    buf.push(0);
                }
                let off = buf.len() as u32;
                buf.extend(&o.value);
                off.to_le_bytes().to_vec()
            }
            None => o.value.clone(),
        };
        buf[e..e + 2].copy_from_slice(&o.tag.to_le_bytes());
        buf[e + 2..e + 4].copy_from_slice(&o.typ.to_le_bytes());
        buf[e + 4..e + 8].copy_from_slice(&o.count.to_le_bytes());
        buf[e + 8..e + 8 + value.len()].copy_from_slice(&value);
    }
    at as u32
}

/// The source's EXIF block rewritten for a resized copy of `size`.
/// `None` when the block is malformed or ends up empty.
pub fn rewrite(tiff: &[u8], size: (u32, u32), keep_gps: bool) -> Option<Vec<u8>> {
    let t = Tiff::new(tiff)?;
    let ifd0 = read_ifd(&t, tiff, t.first_ifd()?, keep_gps, size, 0)?;
    if ifd0.is_empty() {
        return None;
    }
    let mut buf = b"II*\0\0\0\0\0".to_vec();
    let at = write_ifd(&mut buf, &ifd0);
    buf[4..8].copy_from_slice(&at.to_le_bytes());
    Some(buf)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A big-endian EXIF block with orientation 6, a camera model, an
    /// Exif IFD (date, pixel size, maker note), GPS and an IFD1 thumbnail.
    pub(crate) fn sample() -> Vec<u8> {
        let mut d = b"MM\0*\0\0\0\x08".to_vec();
        let be16 = |v: u16| v.to_be_bytes();
        let be32 = |v: u32| v.to_be_bytes();
        // IFD0 at 8: 4 entries.
        let model = b"Blinzel Cam\0"; // 12 bytes, out of line
        let ifd0 = 8;
        let ifd0_len = 2 + 4 * 12 + 4;
        let model_at = ifd0 + ifd0_len;
        let exif_at = model_at + model.len();
        let exif_len = 2 + 4 * 12 + 4;
        let date_at = exif_at + exif_len;
        let date = b"2024:05:17 10:11:12\0";
        let gps_at = date_at + date.len();
        let gps_len = 2 + 12 + 4;
        let ifd1_at = gps_at + gps_len;
        let ifd1_len = 2 + 2 * 12 + 4;
        let thumb_at = ifd1_at + ifd1_len;
        let entry = |d: &mut Vec<u8>, tag: u16, typ: u16, count: u32, v: [u8; 4]| {
            d.extend(be16(tag));
            d.extend(be16(typ));
            d.extend(be32(count));
            d.extend(v);
        };
        d.extend(be16(4));
        entry(&mut d, 0x0110, 2, model.len() as u32, be32(model_at as u32));
        entry(&mut d, ORIENTATION, 3, 1, [0, 6, 0, 0]);
        entry(&mut d, EXIF_IFD, 4, 1, be32(exif_at as u32));
        entry(&mut d, GPS_IFD, 4, 1, be32(gps_at as u32));
        d.extend(be32(ifd1_at as u32));
        d.extend(model);
        d.extend(be16(4));
        entry(
            &mut d,
            DATE_TAKEN,
            2,
            date.len() as u32,
            be32(date_at as u32),
        );
        entry(&mut d, 0x927C, 7, 4, *b"junk");
        entry(&mut d, PIXEL_X, 3, 1, [0x17, 0x70, 0, 0]);
        entry(&mut d, PIXEL_Y, 3, 1, [0x0F, 0xA0, 0, 0]);
        d.extend(be32(0));
        d.extend(date);
        d.extend(be16(1));
        entry(&mut d, 0x0001, 2, 2, *b"N\0\0\0");
        d.extend(be32(0));
        d.extend(be16(2));
        entry(&mut d, 0x0201, 4, 1, be32(thumb_at as u32));
        entry(&mut d, 0x0202, 4, 1, be32(4));
        d.extend(be32(0));
        d.extend([0xFF, 0xD8, 0xFF, 0xD9]);
        d
    }

    fn tags(tiff: &[u8], off: usize) -> Vec<u16> {
        let t = Tiff::new(tiff).unwrap();
        t.ifd(off).unwrap().0.iter().map(|e| e.tag).collect()
    }

    #[test]
    fn rewrite_drops_what_must_go_and_keeps_the_rest() {
        let src = sample();
        assert_eq!(
            date_taken(&src)
                .map(|s| s.format("YYYY-MM-DD hh:mm:ss"))
                .as_deref(),
            Some("2024-05-17 10:11:12")
        );
        let out = rewrite(&src, (1600, 1200), false).unwrap();
        let t = Tiff::new(&out).unwrap();
        assert!(t.little_endian());
        let (ifd0, next) = t.ifd(t.first_ifd().unwrap()).unwrap();
        assert_eq!(next, 0, "no IFD1");
        assert_eq!(
            ifd0.iter().map(|e| e.tag).collect::<Vec<_>>(),
            [0x0110, ORIENTATION, EXIF_IFD]
        );
        assert_eq!(
            crate::exif::orientation(&out),
            Some(crate::geom::Orientation::NORMAL)
        );
        let model = &ifd0[0];
        assert_eq!(&out[t.payload(model).unwrap()], b"Blinzel Cam\0");
        let exif = t.int(&ifd0[2]).unwrap() as usize;
        assert_eq!(tags(&out, exif), [DATE_TAKEN, PIXEL_X, PIXEL_Y]);
        let (e, _) = t.ifd(exif).unwrap();
        assert_eq!(t.int(&e[1]), Some(1600));
        assert_eq!(t.int(&e[2]), Some(1200));
        assert_eq!(
            date_taken(&out)
                .map(|s| s.format("YYYY-MM-DD hh:mm:ss"))
                .as_deref(),
            Some("2024-05-17 10:11:12")
        );
        assert!(!out.windows(4).any(|w| w == b"junk"));
        assert!(!out.windows(4).any(|w| w == [0xFF, 0xD8, 0xFF, 0xD9]));
        assert!(out.len() < src.len());
    }

    #[test]
    fn gps_stays_on_request() {
        let out = rewrite(&sample(), (10, 10), true).unwrap();
        let t = Tiff::new(&out).unwrap();
        let (ifd0, _) = t.ifd(t.first_ifd().unwrap()).unwrap();
        let gps = ifd0.iter().find(|e| e.tag == GPS_IFD).unwrap();
        assert_eq!(tags(&out, t.int(gps).unwrap() as usize), [0x0001]);
    }

    #[test]
    fn malformed_blocks_are_refused() {
        let src = sample();
        for n in [0, 4, 9, 20, 60] {
            let _ = rewrite(&src[..n], (1, 1), false);
        }
        assert!(rewrite(b"II*\0\xff\xff\xff\xff", (1, 1), false).is_none());
        let mut bad = src.clone();
        bad[9] = 0xFF; // absurd entry count
        assert!(rewrite(&bad, (1, 1), false).is_none());
    }

    #[test]
    fn exif_is_found_in_every_container() {
        let tiff = sample();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, body) in [
            (b"IHDR", &[0u8; 13][..]),
            (b"eXIf", &tiff[..]),
            (b"IEND", &[]),
        ] {
            png.extend((body.len() as u32).to_be_bytes());
            png.extend(kind);
            png.extend(body);
            png.extend([0; 4]);
        }
        assert_eq!(read_exif(&png, Format::Png).as_deref(), Some(&tiff[..]));

        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        for (kind, body) in [(b"VP8L", &[1u8, 2, 3][..]), (b"EXIF", &tiff[..])] {
            webp.extend(kind);
            webp.extend((body.len() as u32).to_le_bytes());
            webp.extend(body);
            if body.len() % 2 == 1 {
                webp.push(0);
            }
        }
        assert_eq!(read_exif(&webp, Format::WebP).as_deref(), Some(&tiff[..]));

        let mut jxl = Vec::new();
        for (kind, body) in [
            (b"JXL ", vec![0x0D, 0x0A, 0x87, 0x0A]),
            (b"Exif", {
                let mut b = vec![0, 0, 0, 0];
                b.extend(&tiff);
                b
            }),
        ] {
            jxl.extend((body.len() as u32 + 8).to_be_bytes());
            jxl.extend(kind);
            jxl.extend(body);
        }
        assert_eq!(read_exif(&jxl, Format::Jxl).as_deref(), Some(&tiff[..]));

        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend(((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend(b"Exif\0\0");
        jpeg.extend(&tiff);
        jpeg.extend([0xFF, 0xD9]);
        assert_eq!(read_exif(&jpeg, Format::Jpeg).as_deref(), Some(&tiff[..]));
    }
}
