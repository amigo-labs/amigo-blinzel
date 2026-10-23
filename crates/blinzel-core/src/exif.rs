//! Minimal TIFF/EXIF/MPF reader: only what the viewer needs before decoding
//! (orientation and the location of embedded previews). Every read is
//! bounds-checked; malformed data yields `None`, never a panic.

use std::ops::Range;

use crate::geom::Orientation;

#[derive(Clone, Copy)]
struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

impl<'a> Tiff<'a> {
    fn new(d: &'a [u8]) -> Option<Self> {
        let le = match d.get(..4)? {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return None,
        };
        Some(Tiff { d, le })
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.d.get(at..at + 2)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.d.get(at..at + 4)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn first_ifd(&self) -> Option<usize> {
        self.u32(4).map(|o| o as usize)
    }

    /// Entries of the IFD at `off` and the offset of the next IFD.
    fn ifd(&self, off: usize) -> Option<(Vec<Entry>, usize)> {
        let n = self.u16(off)? as usize;
        if n > 1000 {
            return None;
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let e = off + 2 + 12 * i;
            entries.push(Entry {
                tag: self.u16(e)?,
                typ: self.u16(e + 2)?,
                count: self.u32(e + 4)?,
                value_at: e + 8,
            });
        }
        let next = self.u32(off + 2 + 12 * n).unwrap_or(0) as usize;
        Some((entries, next))
    }

    /// First value of an entry as an integer (SHORT or LONG).
    fn int(&self, e: &Entry) -> Option<u32> {
        match e.typ {
            3 => self.u16(e.value_at).map(u32::from),
            4 | 9 => self.u32(e.value_at),
            1 | 7 => self.d.get(e.value_at).map(|&b| b as u32),
            _ => None,
        }
    }

    /// Byte range of an entry's payload (inline or at its offset).
    fn payload(&self, e: &Entry) -> Option<Range<usize>> {
        let size = match e.typ {
            1 | 2 | 6 | 7 => 1usize,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        }
        .checked_mul(e.count as usize)?;
        let start = if size <= 4 {
            e.value_at
        } else {
            self.u32(e.value_at)? as usize
        };
        let end = start.checked_add(size)?;
        (end <= self.d.len()).then_some(start..end)
    }
}

struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    value_at: usize,
}

/// EXIF orientation from a TIFF block (the APP1 payload after `Exif\0\0`).
pub fn orientation(tiff: &[u8]) -> Option<Orientation> {
    let t = Tiff::new(tiff)?;
    let (entries, _) = t.ifd(t.first_ifd()?)?;
    let e = entries.iter().find(|e| e.tag == 0x0112)?;
    let v = t.int(e)?;
    (1..=8)
        .contains(&v)
        .then(|| Orientation::from_exif(v as u16))
}

/// The JPEG thumbnail in IFD1, as a range inside `tiff`.
pub fn exif_thumbnail(tiff: &[u8]) -> Option<Range<usize>> {
    let t = Tiff::new(tiff)?;
    let (_, ifd1) = t.ifd(t.first_ifd()?)?;
    if ifd1 == 0 {
        return None;
    }
    let (entries, _) = t.ifd(ifd1)?;
    let off = t.int(entries.iter().find(|e| e.tag == 0x0201)?)? as usize;
    let len = t.int(entries.iter().find(|e| e.tag == 0x0202)?)? as usize;
    let end = off.checked_add(len)?;
    (len > 0 && end <= tiff.len()).then_some(off..end)
}

/// An image listed in the MPF index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MpImage {
    /// Type code (low 24 bits of the attribute), e.g. `0x010001` large
    /// thumbnail (VGA), `0x010002` large thumbnail (full HD), `0x030000`
    /// baseline primary image.
    pub kind: u32,
    /// Range relative to the start of the MPF TIFF header.
    pub range: Range<usize>,
}

/// Images listed in an MPF index IFD (the APP2 payload after `MPF\0`).
/// The first entry (offset 0) is the primary image itself and is skipped.
pub fn mpf_images(mpf: &[u8], file_len_after_mpf: usize) -> Vec<MpImage> {
    let mut out = Vec::new();
    let Some(t) = Tiff::new(mpf) else { return out };
    let Some((entries, _)) = t.first_ifd().and_then(|o| t.ifd(o)) else {
        return out;
    };
    let Some(e) = entries.iter().find(|e| e.tag == 0xB002 && e.typ == 7) else {
        return out;
    };
    let Some(r) = t.payload(e) else { return out };
    for chunk in mpf[r].as_chunks::<16>().0.iter() {
        let rd = |o: usize| {
            let b: [u8; 4] = chunk[o..o + 4].try_into().unwrap();
            if t.le {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            }
        };
        let (attr, size, offset) = (rd(0), rd(4) as usize, rd(8) as usize);
        if offset == 0 || size == 0 {
            continue;
        }
        let Some(end) = offset.checked_add(size) else {
            continue;
        };
        if end <= file_len_after_mpf {
            out.push(MpImage {
                kind: attr & 0x00FF_FFFF,
                range: offset..end,
            });
        }
    }
    out
}

/// A JPEG preview embedded in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedPreview {
    /// Byte range in the file.
    pub range: Range<usize>,
    pub width: u32,
    pub height: u32,
}

/// Metadata the viewer needs from a JPEG before decoding.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JpegMeta {
    pub orientation: Orientation,
    /// Embedded JPEG previews, largest first.
    pub previews: Vec<EmbeddedPreview>,
}

/// Reads orientation and embedded previews (EXIF IFD1 thumbnail, MPF
/// images) from a parsed JPEG. `data` must contain the bytes the ranges
/// point at for previews to be found; a 64 KiB prefix yields orientation and
/// whichever previews lie inside it.
pub fn jpeg_meta(data: &[u8], info: &blinzel_jpeg::JpegInfo) -> JpegMeta {
    let mut meta = JpegMeta::default();
    if let Some(r) = info.exif.clone() {
        let tiff = &data[r.clone()];
        meta.orientation = orientation(tiff).unwrap_or_default();
        if let Some(t) = exif_thumbnail(tiff) {
            push_preview(&mut meta.previews, data, r.start + t.start..r.start + t.end);
        }
    }
    if let Some(r) = info.mpf.clone() {
        for img in mpf_images(&data[r.clone()], data.len() - r.start) {
            push_preview(
                &mut meta.previews,
                data,
                r.start + img.range.start..r.start + img.range.end,
            );
        }
    }
    meta.previews
        .sort_by_key(|p| std::cmp::Reverse(p.width as u64 * p.height as u64));
    meta
}

fn push_preview(out: &mut Vec<EmbeddedPreview>, data: &[u8], range: Range<usize>) {
    let Some(bytes) = data.get(range.clone()) else {
        return;
    };
    if let Ok(i) = blinzel_jpeg::parse(bytes) {
        out.push(EmbeddedPreview {
            range,
            width: i.width,
            height: i.height,
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes a little-endian TIFF with IFD0 (orientation) and optional
    /// IFD1 (thumbnail offset/length).
    pub(crate) fn tiff_le(orientation: u16, thumb: Option<(u32, u32)>) -> Vec<u8> {
        let mut d = b"II*\0".to_vec();
        d.extend_from_slice(&8u32.to_le_bytes());
        // IFD0 at 8: one entry.
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&0x0112u16.to_le_bytes());
        d.extend_from_slice(&3u16.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&(orientation as u32).to_le_bytes());
        let next = if thumb.is_some() {
            8 + 2 + 12 + 4
        } else {
            0u32
        };
        d.extend_from_slice(&next.to_le_bytes());
        if let Some((off, len)) = thumb {
            d.extend_from_slice(&2u16.to_le_bytes());
            for (tag, v) in [(0x0201u16, off), (0x0202, len)] {
                d.extend_from_slice(&tag.to_le_bytes());
                d.extend_from_slice(&4u16.to_le_bytes());
                d.extend_from_slice(&1u32.to_le_bytes());
                d.extend_from_slice(&v.to_le_bytes());
            }
            d.extend_from_slice(&0u32.to_le_bytes());
        }
        d
    }

    #[test]
    fn reads_orientation_le_and_be() {
        assert_eq!(
            orientation(&tiff_le(6, None)),
            Some(Orientation::from_exif(6))
        );
        // Big endian, SHORT stored in the first two value bytes.
        let mut be = b"MM\0*".to_vec();
        be.extend_from_slice(&8u32.to_be_bytes());
        be.extend_from_slice(&1u16.to_be_bytes());
        be.extend_from_slice(&0x0112u16.to_be_bytes());
        be.extend_from_slice(&3u16.to_be_bytes());
        be.extend_from_slice(&1u32.to_be_bytes());
        be.extend_from_slice(&[0, 8, 0, 0]);
        be.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(orientation(&be), Some(Orientation::from_exif(8)));
        assert_eq!(orientation(&tiff_le(9, None)), None);
    }

    #[test]
    fn reads_thumbnail_range() {
        let mut t = tiff_le(1, Some((60, 10)));
        t.resize(70, 0xAB);
        assert_eq!(exif_thumbnail(&t), Some(60..70));
        let short = tiff_le(1, Some((60, 100)));
        assert_eq!(exif_thumbnail(&short), None);
    }

    #[test]
    fn malformed_data_never_panics() {
        let base = tiff_le(6, Some((40, 8)));
        for cut in 0..base.len() {
            let _ = orientation(&base[..cut]);
            let _ = exif_thumbnail(&base[..cut]);
            let _ = mpf_images(&base[..cut], 1000);
        }
        let mut evil = base.clone();
        evil[8] = 0xFF;
        evil[9] = 0xFF; // absurd entry count
        assert_eq!(orientation(&evil), None);
    }

    #[test]
    fn reads_mpf_index() {
        // MP index IFD with tag B002 (UNDEFINED, 32 bytes) at offset 50.
        let mut d = b"II*\0".to_vec();
        d.extend_from_slice(&8u32.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&0xB002u16.to_le_bytes());
        d.extend_from_slice(&7u16.to_le_bytes());
        d.extend_from_slice(&32u32.to_le_bytes());
        d.extend_from_slice(&50u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.resize(50, 0);
        // Primary image (offset 0), then a full-HD preview at 1000.
        for (attr, size, off) in [(0x2003_0000u32, 5000u32, 0u32), (0x0001_0002, 400, 1000)] {
            d.extend_from_slice(&attr.to_le_bytes());
            d.extend_from_slice(&size.to_le_bytes());
            d.extend_from_slice(&off.to_le_bytes());
            d.extend_from_slice(&[0; 4]);
        }
        let imgs = mpf_images(&d, 2000);
        assert_eq!(
            imgs,
            vec![MpImage {
                kind: 0x010002,
                range: 1000..1400
            }]
        );
        assert!(mpf_images(&d, 1200).is_empty());
    }
}
