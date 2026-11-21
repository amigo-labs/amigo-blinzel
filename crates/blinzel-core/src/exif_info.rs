//! The facts of a picture its EXIF block tells (docs/specs/info-overlay.md):
//! date taken, camera, lens and exposure, read from any container that
//! carries an EXIF block. Every read is bounds-checked; a broken block or
//! an out-of-range value leaves exactly that fact out.

use std::borrow::Cow;

use crate::exif::{Entry, Tiff};
use crate::format::Format;

const MAKE: u16 = 0x010F;
const MODEL: u16 = 0x0110;
const DATE_TIME: u16 = 0x0132;
const EXIF_IFD: u16 = 0x8769;
const EXPOSURE_TIME: u16 = 0x829A;
const F_NUMBER: u16 = 0x829D;
const ISO: u16 = 0x8827;
const DATE_TAKEN: u16 = 0x9003;
const FOCAL_LENGTH: u16 = 0x920A;
const LENS_MODEL: u16 = 0xA434;

/// A capture time to the minute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Taken {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

/// What a picture's EXIF block says about it; `None` where a fact is
/// missing, unreadable or out of range.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExifInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    /// DateTimeOriginal, or else DateTime.
    pub taken: Option<Taken>,
    /// Exposure time in seconds as numerator and denominator.
    pub exposure: Option<(u32, u32)>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    /// The real focal length in millimetres (not the 35 mm equivalent).
    pub focal_mm: Option<f64>,
}

impl ExifInfo {
    /// Reads the facts from an EXIF block (TIFF, `II*\0` / `MM\0*`).
    pub fn read(tiff: &[u8]) -> ExifInfo {
        let Some(t) = Tiff::new(tiff) else {
            return ExifInfo::default();
        };
        let Some((ifd0, _)) = t.first_ifd().and_then(|o| t.ifd(o)) else {
            return ExifInfo::default();
        };
        let exif = ifd0
            .iter()
            .find(|e| e.tag == EXIF_IFD)
            .and_then(|e| t.int(e))
            .and_then(|o| t.ifd(o as usize))
            .map(|(e, _)| e)
            .unwrap_or_default();
        fn find(entries: &[Entry], tag: u16) -> Option<&Entry> {
            entries.iter().find(|e| e.tag == tag)
        }
        let text = |entries: &[Entry], tag| text(&t, tiff, find(entries, tag)?);
        let ratio = |tag| rational(&t, tiff, find(&exif, tag)?);
        let date = |entries: &[Entry], tag| text(entries, tag).as_deref().and_then(parse_date);
        ExifInfo {
            make: text(&ifd0, MAKE),
            model: text(&ifd0, MODEL),
            lens: text(&exif, LENS_MODEL),
            taken: date(&exif, DATE_TAKEN).or_else(|| date(&ifd0, DATE_TIME)),
            exposure: ratio(EXPOSURE_TIME).filter(|&(n, d)| {
                let s = n as f64 / d as f64;
                s > 0.0 && s <= 3600.0
            }),
            f_number: ratio(F_NUMBER)
                .map(|(n, d)| n as f64 / d as f64)
                .filter(|&f| f > 0.0 && f <= 1000.0),
            iso: find(&exif, ISO)
                .filter(|e| e.typ == 3 || e.typ == 4)
                .and_then(|e| t.int(e))
                .filter(|&n| n > 0),
            focal_mm: ratio(FOCAL_LENGTH)
                .map(|(n, d)| n as f64 / d as f64)
                .filter(|&f| f > 0.0 && f <= 10_000.0),
        }
    }

    /// Reads the facts from a whole file of format `format`.
    pub fn from_file(data: &[u8], format: Format) -> ExifInfo {
        exif_block(data, format)
            .map(|b| ExifInfo::read(&b))
            .unwrap_or_default()
    }

    /// Make and model as one line; the make is left out when the model
    /// starts with its first word (ignoring case).
    pub fn camera(&self) -> Option<String> {
        match (&self.make, &self.model) {
            (Some(make), Some(model)) => {
                let first = make.split(' ').next().unwrap_or(make);
                let repeats = model
                    .get(..first.len())
                    .is_some_and(|p| p.eq_ignore_ascii_case(first))
                    && matches!(model.as_bytes().get(first.len()), None | Some(b' '));
                Some(if repeats {
                    model.clone()
                } else {
                    format!("{make} {model}")
                })
            }
            (a, b) => a.clone().or_else(|| b.clone()),
        }
    }
}

/// An ASCII entry without surrounding spaces and NULs; bytes that are not
/// UTF-8 read as Latin-1. Empty is `None`.
fn text(t: &Tiff, d: &[u8], e: &Entry) -> Option<String> {
    if e.typ != 2 {
        return None;
    }
    let b = d.get(t.payload(e)?)?;
    let b = b.split(|&c| c == 0).next().unwrap_or(b);
    let s = match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    };
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The first value of a RATIONAL entry; a zero denominator is `None`.
fn rational(t: &Tiff, d: &[u8], e: &Entry) -> Option<(u32, u32)> {
    if e.typ != 5 {
        return None;
    }
    let r = t.payload(e)?;
    d.get(r.start..r.start + 8)?;
    let (n, den) = (t.u32(r.start)?, t.u32(r.start + 4)?);
    (den != 0).then_some((n, den))
}

/// `YYYY:MM:DD hh:mm…` (also with `-` in the date) within 1900 … 2200.
fn parse_date(s: &str) -> Option<Taken> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<u16> {
        let d = b.get(r)?;
        d.iter()
            .all(u8::is_ascii_digit)
            .then(|| std::str::from_utf8(d).ok()?.parse().ok())?
    };
    let sep = |i: usize, ok: &[u8]| b.get(i).is_some_and(|c| ok.contains(c));
    if !(sep(4, b":-") && sep(7, b":-") && sep(10, b" T") && sep(13, b":")) {
        return None;
    }
    let t = Taken {
        year: num(0..4)?,
        month: num(5..7)? as u8,
        day: num(8..10)? as u8,
        hour: num(11..13)? as u8,
        minute: num(14..16)? as u8,
    };
    ((1900..=2200).contains(&t.year)
        && (1..=12).contains(&t.month)
        && (1..=31).contains(&t.day)
        && t.hour < 24
        && t.minute < 60)
        .then_some(t)
}

/// The EXIF block (TIFF) of a file, wherever its format keeps it: an APP1
/// segment (JPEG), the `eXIf` chunk (PNG), the `EXIF` chunk (WebP), the
/// `Exif` item (AVIF, HEIC) or box (JPEG XL); a TIFF file (also a
/// TIFF-based camera raw) is one itself. The first block counts.
pub fn exif_block(data: &[u8], format: Format) -> Option<Cow<'_, [u8]>> {
    let tiff: Cow<[u8]> = match format {
        Format::Jpeg => blinzel_jpeg::Segments::new(data)
            .filter(|s| s.marker == 0xE1)
            .find_map(|s| data[s.payload].strip_prefix(b"Exif\0\0"))
            .map(Cow::Borrowed)?,
        Format::Png => Cow::Borrowed(png_chunk(data, b"eXIf")?),
        Format::WebP => {
            let d = riff_chunk(data, b"EXIF")?;
            Cow::Borrowed(d.strip_prefix(b"Exif\0\0").unwrap_or(d))
        }
        Format::Avif | Format::Heif => Cow::Owned(crate::decode::avif::exif(data)?),
        Format::Jxl => {
            let d = isobmff_box(data, b"Exif")?;
            let skip = u32::from_be_bytes(d.get(..4)?.try_into().ok()?) as usize;
            Cow::Borrowed(d.get(skip.checked_add(4)?..)?)
        }
        Format::Tiff => Cow::Borrowed(data),
        _ => return None,
    };
    Tiff::new(&tiff)?;
    Some(tiff)
}

pub(crate) fn png_chunk<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
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

pub(crate) fn riff_chunk<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
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
pub(crate) fn isobmff_box<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::{ExifValue as V, exif_block as block, jpeg_with_exif};

    /// The smallest JPEG the segment walker accepts: SOI, EOI.
    const JPEG: [u8; 4] = [0xFF, 0xD8, 0xFF, 0xD9];

    fn ascii(s: &str) -> V {
        V::Ascii(s.as_bytes().to_vec())
    }

    fn full() -> Vec<u8> {
        block(
            &[
                (MAKE, ascii("Canon")),
                (MODEL, ascii("Canon EOS R6")),
                (DATE_TIME, ascii("2020:01:01 00:00:00")),
            ],
            &[
                (DATE_TAKEN, ascii("2026:11:20 14:32:09")),
                (EXPOSURE_TIME, V::Rational(10, 2500)),
                (F_NUMBER, V::Rational(28, 10)),
                (ISO, V::Short(400)),
                (FOCAL_LENGTH, V::Rational(850, 10)),
                (LENS_MODEL, ascii("RF24-105mm F4 L IS USM ")),
            ],
        )
    }

    #[test]
    fn read_gives_every_fact_of_a_full_block() {
        let i = ExifInfo::read(&full());
        assert_eq!(i.make.as_deref(), Some("Canon"));
        assert_eq!(i.model.as_deref(), Some("Canon EOS R6"));
        assert_eq!(i.lens.as_deref(), Some("RF24-105mm F4 L IS USM"));
        assert_eq!(
            i.taken,
            Some(Taken {
                year: 2026,
                month: 11,
                day: 20,
                hour: 14,
                minute: 32
            })
        );
        assert_eq!(i.exposure, Some((10, 2500)));
        assert_eq!(i.f_number, Some(2.8));
        assert_eq!(i.iso, Some(400));
        assert_eq!(i.focal_mm, Some(85.0));
    }

    #[test]
    fn date_time_stands_in_for_a_missing_or_broken_date_taken() {
        let b = block(&[(DATE_TIME, ascii("2019:02:03 04:05:06"))], &[]);
        assert_eq!(
            ExifInfo::read(&b).taken.map(|t| (t.year, t.minute)),
            Some((2019, 5))
        );
        let b = block(
            &[(DATE_TIME, ascii("2019:02:03 04:05:06"))],
            &[(DATE_TAKEN, ascii("0000:00:00 00:00:00"))],
        );
        assert_eq!(ExifInfo::read(&b).taken.map(|t| t.year), Some(2019));
    }

    #[test]
    fn out_of_range_values_are_left_out_and_the_rest_stays() {
        let b = block(
            &[(MODEL, ascii("X"))],
            &[
                (EXPOSURE_TIME, V::Rational(0, 0)),
                (F_NUMBER, V::Rational(0, 1)),
                (ISO, V::Short(0)),
                (FOCAL_LENGTH, V::Rational(10_001, 1)),
                (DATE_TAKEN, ascii("1899:12:31 23:59:00")),
            ],
        );
        let i = ExifInfo::read(&b);
        assert_eq!(i.model.as_deref(), Some("X"));
        assert_eq!(
            (i.exposure, i.f_number, i.iso, i.focal_mm, i.taken),
            (None, None, None, None, None)
        );
        let b = block(
            &[],
            &[
                (EXPOSURE_TIME, V::Rational(3601, 1)),
                (F_NUMBER, V::Rational(1001, 1)),
                (DATE_TAKEN, ascii("2201:01:01 00:00:00")),
            ],
        );
        let i = ExifInfo::read(&b);
        assert_eq!((i.exposure, i.f_number, i.taken), (None, None, None));
    }

    #[test]
    fn a_broken_block_gives_no_facts_and_never_panics() {
        let good = full();
        assert_eq!(ExifInfo::read(b"garbage"), ExifInfo::default());
        for cut in 0..good.len() {
            let _ = ExifInfo::read(&good[..cut]);
        }
        // The Exif IFD pointer aimed far outside the block.
        let mut bad = good.clone();
        let n = u16::from_le_bytes([bad[8], bad[9]]) as usize;
        for i in 0..n {
            let e = 10 + 12 * i;
            if u16::from_le_bytes([bad[e], bad[e + 1]]) == EXIF_IFD {
                bad[e + 8..e + 12].copy_from_slice(&u32::MAX.to_le_bytes());
            }
        }
        let i = ExifInfo::read(&bad);
        assert_eq!(i.model.as_deref(), Some("Canon EOS R6"));
        assert_eq!(i.exposure, None);
    }

    #[test]
    fn strings_lose_spaces_and_nuls_and_non_utf8_reads_as_latin1() {
        let b = block(
            &[
                (MODEL, V::Ascii(b"  Caf\xe9 X\0\0".to_vec())),
                (MAKE, ascii("   ")),
            ],
            &[],
        );
        let i = ExifInfo::read(&b);
        assert_eq!(i.model.as_deref(), Some("Café X"));
        assert_eq!(i.make, None);
    }

    #[test]
    fn camera_drops_a_make_the_model_repeats() {
        let cam = |make: Option<&str>, model: Option<&str>| {
            ExifInfo {
                make: make.map(str::to_string),
                model: model.map(str::to_string),
                ..ExifInfo::default()
            }
            .camera()
        };
        assert_eq!(
            cam(Some("Canon"), Some("Canon EOS R6")).as_deref(),
            Some("Canon EOS R6")
        );
        assert_eq!(
            cam(Some("NIKON CORPORATION"), Some("NIKON Z 6")).as_deref(),
            Some("NIKON Z 6")
        );
        assert_eq!(
            cam(Some("Sony"), Some("ILCE-7M4")).as_deref(),
            Some("Sony ILCE-7M4")
        );
        assert_eq!(
            cam(Some("SONY"), Some("SONYX 1")).as_deref(),
            Some("SONY SONYX 1")
        );
        assert_eq!(
            cam(Some("fujifilm"), Some("FUJIFILM")).as_deref(),
            Some("FUJIFILM")
        );
        assert_eq!(cam(Some("Leica"), None).as_deref(), Some("Leica"));
        assert_eq!(cam(None, Some("X100V")).as_deref(), Some("X100V"));
        assert_eq!(cam(None, None), None);
    }

    #[test]
    fn exif_comes_from_jpeg_png_webp_jxl_and_tiff() {
        let tiff = full();
        let jpeg = jpeg_with_exif(&JPEG, &tiff);
        assert_eq!(ExifInfo::from_file(&jpeg, Format::Jpeg).iso, Some(400));
        // PNG: signature, then the chunk.
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend((tiff.len() as u32).to_be_bytes());
        png.extend(b"eXIf");
        png.extend(&tiff);
        png.extend([0; 4]);
        assert_eq!(ExifInfo::from_file(&png, Format::Png).iso, Some(400));
        // WebP: RIFF header, then the chunk with the `Exif\0\0` prefix.
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        let body = [b"Exif\0\0".as_slice(), &tiff].concat();
        webp.extend(b"EXIF");
        webp.extend((body.len() as u32).to_le_bytes());
        webp.extend(&body);
        assert_eq!(ExifInfo::from_file(&webp, Format::WebP).iso, Some(400));
        // JPEG XL: an `Exif` box with a 4-byte offset of 0.
        let mut jxl = Vec::new();
        jxl.extend(((8 + 4 + tiff.len()) as u32).to_be_bytes());
        jxl.extend(b"Exif");
        jxl.extend([0; 4]);
        jxl.extend(&tiff);
        assert_eq!(ExifInfo::from_file(&jxl, Format::Jxl).iso, Some(400));
        // A TIFF file is the block itself.
        assert_eq!(ExifInfo::from_file(&tiff, Format::Tiff).iso, Some(400));
        // Formats without EXIF support give nothing.
        assert_eq!(ExifInfo::from_file(&tiff, Format::Raw), ExifInfo::default());
        assert_eq!(ExifInfo::from_file(&tiff, Format::Gif), ExifInfo::default());
    }

    #[test]
    fn the_first_of_two_exif_segments_counts() {
        let first = block(&[(MODEL, ascii("First"))], &[]);
        let second = block(&[(MODEL, ascii("Second"))], &[]);
        let jpeg = jpeg_with_exif(&jpeg_with_exif(&JPEG, &second), &first);
        assert_eq!(
            ExifInfo::from_file(&jpeg, Format::Jpeg).model.as_deref(),
            Some("First")
        );
    }
}
