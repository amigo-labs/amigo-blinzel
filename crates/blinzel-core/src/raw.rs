//! Camera raw files: orientation and the embedded JPEG previews, read
//! without touching sensor data. The sensor data goes to the platform
//! decoder (WIC needs 1.5-5.5 s for a 20-80 MP raw); the previews, usually
//! full-size JPEGs, show within tens of milliseconds.
//!
//! Containers: TIFF-based raws (NEF, CR2, ARW, DNG, PEF, ORF, RW2, ...),
//! Fujifilm RAF and Canon CR3 (ISO base media). Every candidate must parse
//! as an 8-bit lossy JPEG; malformed data yields no preview, never a panic.

use std::ops::Range;

use crate::exif::{EmbeddedPreview, Tiff, push_preview};
use crate::geom::Orientation;

/// What a raw file reveals before decoding.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawInfo {
    pub orientation: Orientation,
    /// The developed image's size from EXIF (`PixelXDimension` and
    /// `PixelYDimension`), if the camera wrote it.
    pub size: Option<(u32, u32)>,
    /// Embedded JPEG previews, largest first.
    pub previews: Vec<EmbeddedPreview>,
}

impl RawInfo {
    /// The developed image's size: EXIF's when it matches the previews'
    /// aspect ratio, else the largest preview's. `None` without previews.
    pub fn image_size(&self) -> Option<(u32, u32)> {
        let p = self.previews.first()?;
        let aspect = |w: u32, h: u32| w as f64 / h.max(1) as f64;
        match self.size {
            Some((w, h))
                if w >= p.width
                    && (aspect(w, h) / aspect(p.width, p.height) - 1.0).abs() < 0.03 =>
            {
                Some((w, h))
            }
            _ => Some((p.width, p.height)),
        }
    }
}

/// Orientation and previews of a raw file; `None` if it is none of the
/// known containers.
pub fn probe(data: &[u8]) -> Option<RawInfo> {
    let mut info = if data.starts_with(b"FUJIFILMCCD-RAW") {
        raf(data)?
    } else if data.get(4..12) == Some(b"ftypcrx ") {
        cr3(data)?
    } else {
        tiff(data, 0..data.len())?
    };
    info.previews
        .sort_by_key(|p| std::cmp::Reverse(p.width as u64 * p.height as u64));
    Some(info)
}

fn orientation_of(v: u32) -> Orientation {
    match v {
        1..=8 => Orientation::from_exif(v as u16),
        _ => Orientation::NORMAL,
    }
}

/// Most IFDs a raw may chain or nest before we stop looking.
const MAX_IFDS: usize = 32;

/// TIFF structure inside `within` (the whole file, or a box of it):
/// IFD0's orientation and every JPEG an IFD points to, in the IFD chain
/// and in sub-IFDs. Offsets count from the start of `within`.
fn tiff(data: &[u8], within: Range<usize>) -> Option<RawInfo> {
    let base = within.start;
    let d = data.get(within)?;
    let t = Tiff::new_any_magic(d)?;
    let mut info = RawInfo::default();
    let mut queue = vec![t.first_ifd()?];
    let mut seen = Vec::new();
    while let Some(off) = queue.pop() {
        if off == 0 || seen.contains(&off) || seen.len() >= MAX_IFDS {
            continue;
        }
        seen.push(off);
        let Some((entries, next)) = t.ifd(off) else {
            continue;
        };
        queue.push(next);
        let find = |tag: u16| entries.iter().find(|e| e.tag == tag);
        let int = |tag: u16| find(tag).and_then(|e| t.int(e));
        if seen.len() == 1
            && let Some(o) = int(0x0112)
        {
            info.orientation = orientation_of(o);
        }
        if let (Some(w), Some(h)) = (int(0xA002), int(0xA003))
            && w > 0
            && h > 0
        {
            info.size.get_or_insert((w, h));
        }
        // Panasonic RW2: the sensor borders of the developed image.
        if seen.len() == 1
            && d.get(2..4) == Some(b"U\0")
            && let (Some(top), Some(left), Some(bottom), Some(right)) =
                (int(0x0004), int(0x0005), int(0x0006), int(0x0007))
            && right > left
            && bottom > top
        {
            info.size = Some((right - left, bottom - top));
        }
        // The EXIF IFD.
        if let Some(o) = int(0x8769) {
            queue.push(o as usize);
        }
        // JPEGInterchangeFormat / Length.
        if let (Some(at), Some(len)) = (int(0x0201), int(0x0202)) {
            push_range(&mut info.previews, data, base, at, len);
        }
        // A single strip of (old- or new-style) JPEG compressed data.
        if matches!(int(0x0103), Some(6 | 7))
            && let (Some(s), Some(c)) = (find(0x0111), find(0x0117))
            && s.count == 1
            && let (Some(at), Some(len)) = (t.int(s), t.int(c))
        {
            push_range(&mut info.previews, data, base, at, len);
        }
        // Panasonic RW2: the JPEG inline in tag 0x2E.
        if let Some(r) = find(0x002E).and_then(|e| t.payload(e)) {
            push_preview(&mut info.previews, data, base + r.start..base + r.end);
        }
        // Sub-IFDs (NEF and DNG keep previews and sensor data there).
        if let Some(r) = find(0x014A).and_then(|e| t.payload(e)) {
            for at in r.step_by(4) {
                if let Some(o) = t.u32(at) {
                    queue.push(o as usize);
                }
            }
        }
    }
    Some(info)
}

fn push_range(out: &mut Vec<EmbeddedPreview>, data: &[u8], base: usize, at: u32, len: u32) {
    let start = base + at as usize;
    if len > 0
        && let Some(end) = start.checked_add(len as usize)
    {
        push_preview(out, data, start..end);
    }
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn be16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

/// Fujifilm RAF: a big-endian header points at a JPEG with EXIF (the
/// orientation) and at a list of records, one of them the cropped size.
fn raf(data: &[u8]) -> Option<RawInfo> {
    let mut info = RawInfo::default();
    push_range(
        &mut info.previews,
        data,
        0,
        be32(data, 84)?,
        be32(data, 88)?,
    );
    if let Some(p) = info.previews.first() {
        let jpeg = &data[p.range.clone()];
        if let Some(exif) = blinzel_jpeg::parse(jpeg).ok().and_then(|j| j.exif)
            && let Some(t) = tiff(jpeg, exif)
        {
            info.orientation = t.orientation;
        }
    }
    // Records: count, then tag, size and data each. 0x111 is the cropped
    // size (height, width), 0x100 the full sensor.
    let meta = be32(data, 92)? as usize;
    let mut at = meta + 4;
    for _ in 0..be32(data, meta)?.min(256) {
        let (tag, size) = (be16(data, at)?, be16(data, at + 2)? as usize);
        if matches!(tag, 0x100 | 0x111) && size == 4 {
            let (h, w) = (be16(data, at + 4)? as u32, be16(data, at + 6)? as u32);
            if w > 0 && h > 0 && (tag == 0x111 || info.size.is_none()) {
                info.size = Some((w, h));
            }
        }
        at += 4 + size;
    }
    Some(info)
}

/// One ISO base media box: type, payload range, and the user type of
/// `uuid` boxes.
struct BmffBox {
    kind: [u8; 4],
    uuid: Option<[u8; 16]>,
    payload: Range<usize>,
}

/// The boxes directly inside `range`.
fn boxes(d: &[u8], range: Range<usize>) -> Vec<BmffBox> {
    let mut out = Vec::new();
    let mut at = range.start;
    while at + 8 <= range.end && out.len() < 1024 {
        let Some(size32) = be32(d, at) else { break };
        let kind: [u8; 4] = d[at + 4..at + 8].try_into().unwrap();
        let (size, mut header) = match size32 {
            0 => (range.end - at, 8),
            1 => match d.get(at + 8..at + 16) {
                Some(b) => (u64::from_be_bytes(b.try_into().unwrap()) as usize, 16),
                None => break,
            },
            n => (n as usize, 8),
        };
        let Some(end) = at.checked_add(size).filter(|&e| e <= range.end) else {
            break;
        };
        let uuid = if &kind == b"uuid" {
            header += 16;
            d.get(at + 8..at + 24).map(|u| u.try_into().unwrap())
        } else {
            None
        };
        if size < header {
            break;
        }
        out.push(BmffBox {
            kind,
            uuid,
            payload: at + header..end,
        });
        at = end;
    }
    out
}

/// Canon's metadata box inside `moov`.
const CR3_META: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];
/// The top-level box holding the `PRVW` preview.
const CR3_PREVIEW: [u8; 16] = [
    0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16,
];

fn child<'a>(list: &'a [BmffBox], kind: &[u8; 4]) -> Option<&'a BmffBox> {
    list.iter().find(|b| &b.kind == kind)
}

/// Canon CR3: orientation from the TIFF in `CMT1`; previews from `THMB`,
/// `PRVW` and the full-size JPEG that the first track holds.
fn cr3(data: &[u8]) -> Option<RawInfo> {
    let mut info = RawInfo::default();
    let top = boxes(data, 0..data.len());
    let moov = child(&top, b"moov")?;
    let moov_children = boxes(data, moov.payload.clone());
    if let Some(meta) = moov_children.iter().find(|b| b.uuid == Some(CR3_META)) {
        let inner = boxes(data, meta.payload.clone());
        if let Some(cmt1) = child(&inner, b"CMT1")
            && let Some(t) = tiff(data, cmt1.payload.clone())
        {
            info.orientation = t.orientation;
        }
        // The EXIF IFD as a TIFF of its own.
        if let Some(cmt2) = child(&inner, b"CMT2")
            && let Some(t) = tiff(data, cmt2.payload.clone())
        {
            info.size = t.size;
        }
        if let Some(thmb) = child(&inner, b"THMB") {
            push_jpeg_within(&mut info.previews, data, thmb.payload.clone());
        }
    }
    if let Some(p) = top.iter().find(|b| b.uuid == Some(CR3_PREVIEW)) {
        // 8 bytes of Canon data, then the `PRVW` box.
        let inner = boxes(data, p.payload.start + 8..p.payload.end);
        if let Some(prvw) = child(&inner, b"PRVW") {
            push_jpeg_within(&mut info.previews, data, prvw.payload.clone());
        }
    }
    // Track 1: one sample, the full-size JPEG.
    if let Some(trak) = child(&moov_children, b"trak") {
        let stbl =
            [&b"mdia"[..], b"minf", b"stbl"]
                .iter()
                .try_fold(trak.payload.clone(), |r, k| {
                    let kind: &[u8; 4] = (*k).try_into().ok()?;
                    Some(child(&boxes(data, r), kind)?.payload.clone())
                });
        if let Some(stbl) = stbl {
            let list = boxes(data, stbl);
            let size = child(&list, b"stsz").and_then(|b| {
                // Version and flags, a common size (or 0), the count, sizes.
                let common = be32(data, b.payload.start + 4)?;
                if common != 0 {
                    Some(common)
                } else {
                    be32(data, b.payload.start + 12)
                }
            });
            let offset = child(&list, b"co64")
                .and_then(|b| {
                    let o = data.get(b.payload.start + 8..b.payload.start + 16)?;
                    Some(u64::from_be_bytes(o.try_into().ok()?) as usize)
                })
                .or_else(|| {
                    child(&list, b"stco")
                        .and_then(|b| be32(data, b.payload.start + 8).map(|o| o as usize))
                });
            if let (Some(at), Some(len)) = (offset, size)
                && let Some(end) = at.checked_add(len as usize)
            {
                push_preview(&mut info.previews, data, at..end);
            }
        }
    }
    Some(info)
}

/// A JPEG that starts somewhere in the first bytes of `r` (Canon puts a
/// small header in front) and runs to its end.
fn push_jpeg_within(out: &mut Vec<EmbeddedPreview>, data: &[u8], r: Range<usize>) {
    let head = r.start..(r.start + 64).min(r.end);
    if let Some(i) = data
        .get(head)
        .and_then(|h| h.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]))
    {
        push_preview(out, data, r.start + i..r.end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG header the parser accepts: SOF (8-bit, three components,
    /// `marker` 0xC0 baseline or 0xC3 lossless) and an empty scan.
    fn jpeg(w: u16, h: u16, marker: u8) -> Vec<u8> {
        let mut d = vec![0xFF, 0xD8, 0xFF, marker, 0, 17, 8];
        d.extend(h.to_be_bytes());
        d.extend(w.to_be_bytes());
        d.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        d.extend([0xFF, 0xDA, 0, 12, 3, 1, 0, 2, 0x11, 3, 0x11, 0, 63, 0]);
        d.extend([0xFF, 0xD9]);
        d
    }

    /// Little-endian TIFF writer: IFDs at given offsets, each a list of
    /// (tag, type, values); values that do not fit inline go 256 bytes
    /// after the IFD.
    struct TiffLe {
        d: Vec<u8>,
    }

    impl TiffLe {
        fn new(len: usize) -> Self {
            let mut d = vec![0u8; len];
            d[..4].copy_from_slice(b"II*\0");
            TiffLe { d }
        }

        fn put(&mut self, at: usize, bytes: &[u8]) {
            self.d[at..at + bytes.len()].copy_from_slice(bytes);
        }

        /// Writes an IFD at `at` and links it from `link` (4 for IFD0).
        fn ifd(&mut self, at: usize, link: Option<usize>, entries: &[(u16, u16, Vec<u32>)]) {
            if let Some(l) = link {
                self.put(l, &(at as u32).to_le_bytes());
            }
            self.put(at, &(entries.len() as u16).to_le_bytes());
            let mut extra = at + 256;
            for (i, (tag, typ, values)) in entries.iter().enumerate() {
                let e = at + 2 + 12 * i;
                self.put(e, &tag.to_le_bytes());
                self.put(e + 2, &typ.to_le_bytes());
                self.put(e + 4, &(values.len() as u32).to_le_bytes());
                let bytes: Vec<u8> = values
                    .iter()
                    .flat_map(|&v| match typ {
                        3 => (v as u16).to_le_bytes().to_vec(),
                        _ => v.to_le_bytes().to_vec(),
                    })
                    .collect();
                if bytes.len() <= 4 {
                    self.put(e + 8, &bytes);
                } else {
                    self.put(e + 8, &(extra as u32).to_le_bytes());
                    self.put(extra, &bytes);
                    extra += bytes.len();
                }
            }
        }
    }

    #[test]
    fn tiff_raws_list_their_jpegs() {
        let thumb = jpeg(160, 120, 0xC0);
        let full = jpeg(600, 400, 0xC0);
        let sensor = jpeg(610, 410, 0xC3);
        let mid = jpeg(320, 213, 0xC0);
        let mut t = TiffLe::new(4096);
        let (at_thumb, at_full, at_sensor, at_mid) = (3000, 3300, 3600, 3900);
        t.put(at_thumb, &thumb);
        t.put(at_full, &full);
        t.put(at_sensor, &sensor);
        t.put(at_mid, &mid);
        // IFD0: a reduced-resolution image with the thumbnail, two
        // sub-IFDs (the sensor data first) and the EXIF IFD.
        t.ifd(
            8,
            Some(4),
            &[
                (0x00FE, 4, vec![1]),
                (0x0112, 3, vec![8]),
                (0x014A, 4, vec![1400, 1000]),
                (0x0201, 4, vec![at_thumb as u32]),
                (0x0202, 4, vec![thumb.len() as u32]),
                (0x8769, 4, vec![1800]),
            ],
        );
        // IFD1, chained after IFD0's six entries: another preview, and an
        // orientation that is not the image's.
        t.ifd(
            2200,
            Some(8 + 2 + 12 * 6),
            &[
                (0x0112, 3, vec![1]),
                (0x0201, 4, vec![at_mid as u32]),
                (0x0202, 4, vec![mid.len() as u32]),
            ],
        );
        // The preview as a JPEG-compressed strip, the sensor data as
        // lossless JPEG (no preview), the developed size (twice the
        // preview's) in EXIF.
        let strip = |at: usize, len: usize| {
            vec![
                (0x0103, 3, vec![6]),
                (0x0111, 4, vec![at as u32]),
                (0x0117, 4, vec![len as u32]),
            ]
        };
        t.ifd(1000, None, &strip(at_full, full.len()));
        t.ifd(1400, None, &strip(at_sensor, sensor.len()));
        t.ifd(
            1800,
            None,
            &[(0xA002, 4, vec![1200]), (0xA003, 4, vec![800])],
        );

        let info = probe(&t.d).unwrap();
        assert_eq!(info.orientation, Orientation::from_exif(8));
        let sizes: Vec<_> = info.previews.iter().map(|p| (p.width, p.height)).collect();
        assert_eq!(sizes, [(600, 400), (320, 213), (160, 120)]);
        assert_eq!(info.previews[0].range, at_full..at_full + full.len());
        assert_eq!(info.image_size(), Some((1200, 800)));

        // Through the decoder's probe: a raw, its size and orientation.
        let p = crate::decode::probe(&t.d).unwrap();
        assert!(p.is_raw());
        // Its sensor data goes to the system decoder (AC-03).
        assert!(matches!(
            crate::decode::decode(&t.d, crate::decode::Request::Full),
            Err(crate::decode::DecodeError::Unsupported(
                crate::format::Format::Tiff
            ))
        ));
        assert_eq!(
            (p.width, p.height, p.orientation),
            (1200, 800, Orientation::from_exif(8))
        );
    }

    #[test]
    fn image_size_trusts_exif_only_with_the_previews_aspect() {
        let info = |size| RawInfo {
            orientation: Orientation::NORMAL,
            size,
            previews: vec![EmbeddedPreview {
                range: 0..1,
                width: 1616,
                height: 1080,
            }],
        };
        assert_eq!(info(Some((6000, 4000))).image_size(), Some((6000, 4000)));
        assert_eq!(info(Some((4000, 6000))).image_size(), Some((1616, 1080)));
        assert_eq!(info(Some((800, 533))).image_size(), Some((1616, 1080)));
        // The aspect ratio may differ by less than 3 % (AC-18), either way.
        assert_eq!(info(Some((6000, 3897))).image_size(), Some((6000, 3897))); // 2.9 % wider
        assert_eq!(info(Some((6000, 3889))).image_size(), Some((1616, 1080))); // 3.1 % wider
        assert_eq!(info(Some((6000, 4130))).image_size(), Some((6000, 4130))); // 2.9 % narrower
        assert_eq!(info(Some((6000, 4136))).image_size(), Some((1616, 1080))); // 3.05 % narrower
        // The EXIF width may equal the preview's (AC-18: at least its width).
        assert_eq!(info(Some((1616, 1090))).image_size(), Some((1616, 1090))); // 0.9 % narrower
        assert_eq!(info(None).image_size(), Some((1616, 1080)));
        assert_eq!(RawInfo::default().image_size(), None);
    }

    #[test]
    fn a_lone_sub_ifd_is_read_inline_and_ifd_typed_pointers_count() {
        let full = jpeg(600, 400, 0xC0);
        let at_full = 3000;
        // The pointers as LONG (4) or IFD (13), the strip as old- or
        // new-style JPEG (6, 7).
        for (typ, compression) in [(4, 6), (4, 7), (13, 6), (13, 7)] {
            let mut t = TiffLe::new(4096);
            t.put(at_full, &full);
            // IFD0: one sub-IFD, its offset inline (count 1), and the EXIF
            // IFD; a size with a zero width does not count.
            t.ifd(
                8,
                Some(4),
                &[
                    (0x014A, typ, vec![1000]),
                    (0x8769, typ, vec![1800]),
                    (0xA002, 4, vec![0]),
                    (0xA003, 4, vec![800]),
                ],
            );
            // The sub-IFD (read before the EXIF IFD): the preview, and a size
            // with a zero height.
            t.ifd(
                1000,
                None,
                &[
                    (0x0103, 3, vec![compression]),
                    (0x0111, 4, vec![at_full as u32]),
                    (0x0117, 4, vec![full.len() as u32]),
                    (0xA002, 4, vec![1200]),
                    (0xA003, 4, vec![0]),
                ],
            );
            t.ifd(
                1800,
                None,
                &[(0xA002, 4, vec![1200]), (0xA003, 4, vec![800])],
            );
            let info = probe(&t.d).unwrap();
            let case = (typ, compression);
            let previews: Vec<_> = info
                .previews
                .iter()
                .map(|p| (p.range.start, p.range.end))
                .collect();
            assert_eq!(previews, [(at_full, at_full + full.len())], "{case:?}");
            assert_eq!(info.size, Some((1200, 800)), "{case:?}");
        }
    }

    #[test]
    fn rw2_sensor_borders_and_inline_jpeg() {
        let full = jpeg(600, 400, 0xC0);
        let at_full = 2000;
        for (magic, size) in [(b"IIU\0", Some((6000, 4000))), (b"II*\0", None)] {
            let mut t = TiffLe::new(4096);
            t.put(0, magic);
            t.put(at_full, &full);
            // Borders top, left, bottom, right; the JPEG in tag 0x2E
            // (UNDEFINED, its length as the count).
            t.ifd(
                8,
                Some(4),
                &[
                    (0x0004, 3, vec![8]),
                    (0x0005, 3, vec![16]),
                    (0x0006, 3, vec![4008]),
                    (0x0007, 3, vec![6016]),
                    (0x002E, 7, vec![at_full as u32]),
                ],
            );
            t.put(8 + 2 + 12 * 4 + 4, &(full.len() as u32).to_le_bytes());
            let info = probe(&t.d).unwrap();
            assert_eq!(info.size, size, "{magic:?}");
            let previews: Vec<_> = info
                .previews
                .iter()
                .map(|p| (p.range.start, p.range.end))
                .collect();
            assert_eq!(previews, [(at_full, at_full + full.len())], "{magic:?}");
        }
    }

    #[test]
    fn raf_header_and_records() {
        // The JPEG carries the orientation in its EXIF.
        let tiff = crate::exif::tests::tiff_le(6, None);
        let app1 = [b"Exif\0\0".as_slice(), &tiff].concat();
        let plain = jpeg(600, 400, 0xC0);
        let len = (app1.len() as u16 + 2).to_be_bytes();
        let full = [
            &plain[..2],
            &[0xFF, 0xE1, len[0], len[1]],
            &app1,
            &plain[2..],
        ]
        .concat();
        // The cropped size (0x111) wins over the sensor's (0x100) in
        // either order.
        let records = [(0x100u16, 4182u16, 6384u16), (0x111, 4160, 6240)];
        for order in [records, [records[1], records[0]]] {
            let mut d = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
            d.resize(200, 0);
            let at_meta = 200 + full.len() as u32;
            d[84..88].copy_from_slice(&200u32.to_be_bytes());
            d[88..92].copy_from_slice(&(full.len() as u32).to_be_bytes());
            d[92..96].copy_from_slice(&at_meta.to_be_bytes());
            d.extend(&full);
            d.extend(3u32.to_be_bytes());
            // A record of another size first: the next starts after it.
            d.extend([0x01, 0x30, 0, 6, 0x01, 0x11, 0, 4, 0, 1]);
            for (tag, h, w) in order {
                d.extend(tag.to_be_bytes());
                d.extend(4u16.to_be_bytes());
                d.extend(h.to_be_bytes());
                d.extend(w.to_be_bytes());
            }
            let info = probe(&d).unwrap();
            assert_eq!(info.size, Some((6240, 4160)), "{order:x?}");
            assert_eq!(info.orientation, Orientation::from_exif(6));
            assert_eq!(info.previews.len(), 1);
            assert_eq!(info.previews[0].range.start, 200);
            assert_eq!(info.image_size(), Some((6240, 4160)));
        }
    }

    fn bmff(kind: &[u8; 4], uuid: Option<[u8; 16]>, payload: &[u8]) -> Vec<u8> {
        let header = if uuid.is_some() { 24 } else { 8 };
        let mut b = ((header + payload.len()) as u32).to_be_bytes().to_vec();
        b.extend(kind);
        if let Some(u) = uuid {
            b.extend(u);
        }
        b.extend(payload);
        b
    }

    #[test]
    fn cr3_boxes() {
        let thumb = jpeg(160, 120, 0xC0);
        let prvw = jpeg(320, 213, 0xC0);
        let full = jpeg(600, 400, 0xC0);
        // CMT1: IFD0 with the orientation; CMT2: the EXIF IFD.
        let mut cmt1 = TiffLe::new(64);
        cmt1.ifd(8, Some(4), &[(0x0112, 3, vec![6])]);
        let mut cmt2 = TiffLe::new(300);
        cmt2.ifd(
            8,
            Some(4),
            &[(0xA002, 4, vec![600]), (0xA003, 4, vec![400])],
        );
        let thmb = [vec![0u8; 16], thumb].concat();
        let meta = [
            bmff(b"CMT1", None, &cmt1.d),
            bmff(b"CMT2", None, &cmt2.d),
            bmff(b"THMB", None, &thmb),
        ]
        .concat();
        let prvw_box = [vec![0u8; 16], prvw].concat();
        let preview_uuid = [vec![0u8; 8], bmff(b"PRVW", None, &prvw_box)].concat();

        // Track 1's one sample lies in mdat, whose position depends on the
        // boxes in front: build twice. Its size is the common one or listed
        // per sample (common size 0), its offset 64-bit (`co64`) or 32-bit
        // (`stco`).
        let build = |offset: u64, common: bool, wide: bool| {
            let len = (full.len() as u32).to_be_bytes().to_vec();
            let stsz = if common {
                [vec![0u8; 4], len, vec![0, 0, 0, 1]].concat()
            } else {
                [vec![0u8; 8], vec![0, 0, 0, 1], len].concat()
            };
            let (kind, at) = if wide {
                (b"co64", offset.to_be_bytes().to_vec())
            } else {
                (b"stco", (offset as u32).to_be_bytes().to_vec())
            };
            let chunks = [vec![0u8; 4], vec![0, 0, 0, 1], at].concat();
            let stbl = [bmff(b"stsz", None, &stsz), bmff(kind, None, &chunks)].concat();
            let minf = bmff(b"minf", None, &bmff(b"stbl", None, &stbl));
            let trak = bmff(b"trak", None, &bmff(b"mdia", None, &minf));
            let moov = [bmff(b"uuid", Some(CR3_META), &meta), trak].concat();
            let mut f = bmff(b"ftyp", None, b"crx \0\0\0\x01crx isom");
            f.extend(bmff(b"moov", None, &moov));
            f.extend(bmff(b"uuid", Some(CR3_PREVIEW), &preview_uuid));
            let mdat_at = f.len() + 8;
            f.extend(bmff(b"mdat", None, &full));
            (f, mdat_at)
        };
        for (common, wide) in [(true, true), (false, true), (true, false), (false, false)] {
            let (_, mdat_at) = build(0, common, wide);
            let (d, _) = build(mdat_at as u64, common, wide);

            let info = probe(&d).unwrap();
            let case = (common, wide);
            assert_eq!(info.orientation, Orientation::from_exif(6), "{case:?}");
            assert_eq!(info.size, Some((600, 400)));
            let sizes: Vec<_> = info.previews.iter().map(|p| (p.width, p.height)).collect();
            assert_eq!(sizes, [(600, 400), (320, 213), (160, 120)], "{case:?}");
            assert_eq!(info.previews[0].range, mdat_at..mdat_at + full.len());
        }
    }

    #[test]
    fn garbage_yields_nothing() {
        assert_eq!(probe(b"nothing"), None);
        for d in [
            &b"II*\0\xff\xff\xff\xff"[..],
            b"FUJIFILMCCD-RAW ",
            b"\0\0\0\x18ftypcrx \0\0\0\x01",
        ] {
            assert!(probe(d).is_none_or(|i| i.previews.is_empty()));
        }
        // A sub-IFD pointing at its own IFD and a JPEG past the end.
        let mut t = TiffLe::new(512);
        t.ifd(
            8,
            Some(4),
            &[
                (0x014A, 4, vec![8]),
                (0x0201, 4, vec![400]),
                (0x0202, 4, vec![9999]),
            ],
        );
        assert!(probe(&t.d).unwrap().previews.is_empty());
    }

    /// AC-18: a preview the end of the file cuts in the middle of its scan
    /// data is not listed, although the bytes the file has parse as a JPEG;
    /// the same preview whole is.
    #[test]
    fn a_preview_cut_by_the_end_of_the_file_is_not_listed() {
        let mut full = jpeg(600, 400, 0xC0);
        full.truncate(full.len() - 2);
        full.extend([0x55; 64]);
        full.extend([0xFF, 0xD9]);
        let at = 512;
        let tiff = |len: usize| {
            let mut t = TiffLe::new(at + len);
            t.put(at, &full[..len]);
            t.ifd(
                8,
                Some(4),
                &[
                    (0x0201, 4, vec![at as u32]),
                    (0x0202, 4, vec![full.len() as u32]),
                ],
            );
            t.d
        };
        let whole = probe(&tiff(full.len())).unwrap();
        assert_eq!(whole.previews.len(), 1, "the whole preview is listed");
        // Half of the scan data and the EOI are missing.
        let cut = full.len() - 2 - 32;
        assert!(
            blinzel_jpeg::parse(&full[..cut]).is_ok(),
            "the cut bytes parse"
        );
        assert!(probe(&tiff(cut)).unwrap().previews.is_empty());
    }
}
