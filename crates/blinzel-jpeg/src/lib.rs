//! Own JPEG code for amigo-blinzel.
//!
//! The marker parser reads everything up to the first scan without touching
//! entropy-coded data, which is all the viewer needs to size the window,
//! pick a DCT scale and find embedded previews (EXIF, MPF) before a single
//! pixel is decoded. The decoder (milestone M3) turns sequential Huffman
//! JPEGs into YCbCr planes, byte-identical with libjpeg-turbo.

#![forbid(unsafe_code)]

mod bits;
mod decoder;
mod huffman;
mod idct;

use std::fmt;
use std::ops::Range;

pub use decoder::{
    PlaneLayout, SCALES, decode_planes, is_supported, plane_layouts, supported_scale,
};

/// Maximum number of components a frame may declare that we accept.
pub const MAX_COMPONENTS: usize = 4;

/// Coding process from the SOF marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Process {
    Baseline,
    ExtendedSequential,
    Progressive,
    Lossless,
}

/// Entropy coding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coding {
    Huffman,
    Arithmetic,
}

/// One frame component (SOF).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Component {
    pub id: u8,
    /// Horizontal sampling factor (1..=4).
    pub h: u8,
    /// Vertical sampling factor (1..=4).
    pub v: u8,
    /// Quantization table selector.
    pub tq: u8,
}

/// Chroma subsampling, classified from the sampling factors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subsampling {
    Gray,
    S444,
    S422,
    S420,
    S440,
    S411,
    /// Anything else (unusual factor combinations, 4 components, …).
    Other,
}

impl Subsampling {
    /// Chroma divisor (horizontal, vertical) relative to luma.
    pub fn chroma_divisor(self) -> Option<(u32, u32)> {
        Some(match self {
            Subsampling::S444 => (1, 1),
            Subsampling::S422 => (2, 1),
            Subsampling::S420 => (2, 2),
            Subsampling::S440 => (1, 2),
            Subsampling::S411 => (4, 1),
            Subsampling::Gray | Subsampling::Other => return None,
        })
    }
}

/// Colour transform signalled by an Adobe APP14 segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdobeTransform {
    /// RGB or CMYK stored as is.
    None,
    YCbCr,
    Ycck,
}

/// Everything in the JPEG header that matters before decoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JpegInfo {
    pub width: u32,
    pub height: u32,
    pub precision: u8,
    pub process: Process,
    pub coding: Coding,
    pub components: [Component; MAX_COMPONENTS],
    pub component_count: u8,
    /// Restart interval in MCUs from the last DRI before the first scan.
    pub restart_interval: u16,
    pub jfif: bool,
    pub adobe: Option<AdobeTransform>,
    /// TIFF payload of the APP1 `Exif\0\0` segment.
    pub exif: Option<Range<usize>>,
    /// TIFF payload of the APP2 `MPF\0` segment. MPF image offsets are
    /// relative to `mpf.start`.
    pub mpf: Option<Range<usize>>,
    /// ICC profile chunks (APP2 `ICC_PROFILE`), in sequence order.
    pub icc_chunks: Vec<Range<usize>>,
    /// Offset of the first SOS marker.
    pub first_scan: usize,
}

impl JpegInfo {
    pub fn components(&self) -> &[Component] {
        &self.components[..self.component_count as usize]
    }

    /// Largest horizontal/vertical sampling factors (MCU size = 8 × these).
    pub fn max_sampling(&self) -> (u8, u8) {
        self.components()
            .iter()
            .fold((1, 1), |(h, v), c| (h.max(c.h), v.max(c.v)))
    }

    /// MCU size in pixels.
    pub fn mcu_size(&self) -> (u32, u32) {
        let (h, v) = self.max_sampling();
        (8 * h as u32, 8 * v as u32)
    }

    pub fn subsampling(&self) -> Subsampling {
        let c = self.components();
        match c.len() {
            1 => Subsampling::Gray,
            3 => {
                let (y, cb, cr) = (c[0], c[1], c[2]);
                if cb.h != 1 || cb.v != 1 || cr.h != 1 || cr.v != 1 {
                    return Subsampling::Other;
                }
                match (y.h, y.v) {
                    (1, 1) => Subsampling::S444,
                    (2, 1) => Subsampling::S422,
                    (2, 2) => Subsampling::S420,
                    (1, 2) => Subsampling::S440,
                    (4, 1) => Subsampling::S411,
                    _ => Subsampling::Other,
                }
            }
            _ => Subsampling::Other,
        }
    }

    /// Whether three-component data is YCbCr (the overwhelmingly common
    /// case) rather than RGB, following libjpeg's heuristics.
    pub fn is_ycbcr(&self) -> bool {
        if self.component_count != 3 {
            return false;
        }
        if self.jfif {
            return true;
        }
        match self.adobe {
            Some(AdobeTransform::YCbCr) => true,
            Some(_) => false,
            None => {
                let c = self.components();
                [c[0].id, c[1].id, c[2].id] != *b"RGB"
            }
        }
    }

    /// Number of 8×8 DCT scale steps (1/8 … 8/8) for which the scaled image
    /// still covers `min_w × min_h`; returns the numerator over 8.
    pub fn dct_scale_for(&self, min_w: u32, min_h: u32) -> u32 {
        (1..=8)
            .find(|&n| scaled(self.width, n) >= min_w && scaled(self.height, n) >= min_h)
            .unwrap_or(8)
    }
}

/// Size after DCT scaling by `num/8`, rounded up like libjpeg does.
pub fn scaled(dim: u32, num: u32) -> u32 {
    (dim * num).div_ceil(8)
}

/// Parse errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    NotJpeg,
    Truncated,
    BadSegment(u8),
    UnsupportedFrame(u8),
    NoFrame,
    ZeroSize,
    /// Valid JPEG the decoder does not handle (another decoder may).
    Unsupported(&'static str),
    /// Broken entropy-coded data or tables.
    Corrupt(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotJpeg => f.write_str("not a JPEG (missing SOI)"),
            Error::Truncated => f.write_str("JPEG header is truncated"),
            Error::BadSegment(m) => write!(f, "malformed segment 0xFF{m:02X}"),
            Error::UnsupportedFrame(m) => write!(f, "unsupported frame type 0xFF{m:02X}"),
            Error::NoFrame => f.write_str("no frame header before the first scan"),
            Error::ZeroSize => f.write_str("image has zero width or height"),
            Error::Unsupported(what) => write!(f, "unsupported JPEG: {what}"),
            Error::Corrupt(what) => write!(f, "corrupt JPEG: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// Quick check for the SOI marker.
pub fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF
}

/// A marker segment: marker byte and the payload range (without length).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub marker: u8,
    /// Offset of the 0xFF of the marker.
    pub offset: usize,
    pub payload: Range<usize>,
}

/// Iterates marker segments from SOI up to and including the first SOS.
/// Stops (returning `None`) after SOS, EOI or on malformed data.
pub struct Segments<'a> {
    data: &'a [u8],
    pos: usize,
    done: bool,
    pub error: Option<Error>,
}

impl<'a> Segments<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let ok = is_jpeg(data);
        Segments {
            data,
            pos: 2,
            done: !ok,
            error: if ok { None } else { Some(Error::NotJpeg) },
        }
    }

    /// Continues at `pos`, e.g. after a scan's entropy-coded data.
    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Segments {
            data,
            pos,
            done: false,
            error: None,
        }
    }

    fn fail(&mut self, e: Error) -> Option<Segment> {
        self.error = Some(e);
        self.done = true;
        None
    }
}

impl Iterator for Segments<'_> {
    type Item = Segment;

    fn next(&mut self) -> Option<Segment> {
        if self.done {
            return None;
        }
        let d = self.data;
        // Skip garbage and fill bytes up to the next marker.
        loop {
            if self.pos + 1 >= d.len() {
                return self.fail(Error::Truncated);
            }
            if d[self.pos] == 0xFF && d[self.pos + 1] != 0xFF && d[self.pos + 1] != 0x00 {
                break;
            }
            self.pos += 1;
        }
        let offset = self.pos;
        let marker = d[self.pos + 1];
        self.pos += 2;
        // Standalone markers carry no length.
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            return Some(Segment {
                marker,
                offset,
                payload: self.pos..self.pos,
            });
        }
        if marker == 0xD9 {
            self.done = true;
            return Some(Segment {
                marker,
                offset,
                payload: self.pos..self.pos,
            });
        }
        if self.pos + 2 > d.len() {
            return self.fail(Error::Truncated);
        }
        let len = u16::from_be_bytes([d[self.pos], d[self.pos + 1]]) as usize;
        if len < 2 {
            return self.fail(Error::BadSegment(marker));
        }
        let start = self.pos + 2;
        let end = self.pos + len;
        if end > d.len() {
            return self.fail(Error::Truncated);
        }
        self.pos = end;
        if marker == 0xDA {
            self.done = true;
        }
        Some(Segment {
            marker,
            offset,
            payload: start..end,
        })
    }
}

/// Parses the header (everything before the first scan's entropy data).
///
/// Only the first ~64 KiB of a typical camera file are touched; pass the
/// whole file or just its beginning (truncation before SOS is an error).
pub fn parse(data: &[u8]) -> Result<JpegInfo, Error> {
    let mut segs = Segments::new(data);
    let mut frame: Option<(u8, &[u8])> = None;
    let mut restart_interval = 0u16;
    let mut jfif = false;
    let mut adobe = None;
    let mut exif = None;
    let mut mpf = None;
    let mut icc: Vec<(u8, Range<usize>)> = Vec::new();
    let mut first_scan = None;

    for seg in segs.by_ref() {
        let p = &data[seg.payload.clone()];
        match seg.marker {
            0xC0..=0xCF if !matches!(seg.marker, 0xC4 | 0xC8 | 0xCC) => {
                if frame.is_none() {
                    frame = Some((seg.marker, p));
                }
            }
            0xDD => {
                if p.len() < 2 {
                    return Err(Error::BadSegment(0xDD));
                }
                restart_interval = u16::from_be_bytes([p[0], p[1]]);
            }
            0xE0 if p.starts_with(b"JFIF\0") => jfif = true,
            0xE1 if p.starts_with(b"Exif\0\0") && exif.is_none() => {
                exif = Some(seg.payload.start + 6..seg.payload.end);
            }
            0xE2 if p.starts_with(b"MPF\0") && mpf.is_none() => {
                mpf = Some(seg.payload.start + 4..seg.payload.end);
            }
            0xE2 if p.starts_with(b"ICC_PROFILE\0") && p.len() >= 14 => {
                icc.push((p[12], seg.payload.start + 14..seg.payload.end));
            }
            0xEE if p.starts_with(b"Adobe") && p.len() >= 12 => {
                adobe = Some(match p[11] {
                    1 => AdobeTransform::YCbCr,
                    2 => AdobeTransform::Ycck,
                    _ => AdobeTransform::None,
                });
            }
            0xDA => first_scan = Some(seg.offset),
            _ => {}
        }
    }
    if let Some(e) = segs.error {
        // A header that reached SOS is fine even if the caller passed only
        // the first part of the file.
        if first_scan.is_none() {
            return Err(e);
        }
    }
    let first_scan = first_scan.ok_or(Error::NoFrame)?;
    let (sof, p) = frame.ok_or(Error::NoFrame)?;
    let (process, coding) = match sof {
        0xC0 => (Process::Baseline, Coding::Huffman),
        0xC1 => (Process::ExtendedSequential, Coding::Huffman),
        0xC2 => (Process::Progressive, Coding::Huffman),
        0xC3 => (Process::Lossless, Coding::Huffman),
        0xC9 => (Process::ExtendedSequential, Coding::Arithmetic),
        0xCA => (Process::Progressive, Coding::Arithmetic),
        0xCB => (Process::Lossless, Coding::Arithmetic),
        m => return Err(Error::UnsupportedFrame(m)),
    };
    if p.len() < 6 {
        return Err(Error::BadSegment(sof));
    }
    let precision = p[0];
    let height = u16::from_be_bytes([p[1], p[2]]) as u32;
    let width = u16::from_be_bytes([p[3], p[4]]) as u32;
    let count = p[5] as usize;
    if count == 0 || count > MAX_COMPONENTS || p.len() < 6 + 3 * count {
        return Err(Error::BadSegment(sof));
    }
    if width == 0 || height == 0 {
        // Height 0 (DNL) is legal but practically unused; treat as error.
        return Err(Error::ZeroSize);
    }
    let mut components = [Component::default(); MAX_COMPONENTS];
    for (i, c) in components.iter_mut().take(count).enumerate() {
        let b = &p[6 + 3 * i..9 + 3 * i];
        *c = Component {
            id: b[0],
            h: b[1] >> 4,
            v: b[1] & 0x0F,
            tq: b[2],
        };
        if !(1..=4).contains(&c.h) || !(1..=4).contains(&c.v) {
            return Err(Error::BadSegment(sof));
        }
    }
    icc.sort_by_key(|(seq, _)| *seq);
    Ok(JpegInfo {
        width,
        height,
        precision,
        process,
        coding,
        components,
        component_count: count as u8,
        restart_interval,
        jfif,
        adobe,
        exif,
        mpf,
        icc_chunks: icc.into_iter().map(|(_, r)| r).collect(),
        first_scan,
    })
}

/// Concatenates the ICC profile chunks.
pub fn icc_profile(data: &[u8], info: &JpegInfo) -> Option<Vec<u8>> {
    if info.icc_chunks.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(info.icc_chunks.iter().map(|r| r.len()).sum());
    for r in &info.icc_chunks {
        out.extend_from_slice(data.get(r.clone())?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal header: SOI, optional APPs, DQT, SOF, DRI, SOS.
    fn header(sof: u8, w: u16, h: u16, comps: &[(u8, u8, u8)], extra: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut d = vec![0xFF, 0xD8];
        let seg = |d: &mut Vec<u8>, m: u8, p: &[u8]| {
            d.extend_from_slice(&[0xFF, m]);
            d.extend_from_slice(&((p.len() + 2) as u16).to_be_bytes());
            d.extend_from_slice(p);
        };
        for (m, p) in extra {
            seg(&mut d, *m, p);
        }
        seg(&mut d, 0xDB, &[0; 65]);
        let mut f = vec![8];
        f.extend_from_slice(&h.to_be_bytes());
        f.extend_from_slice(&w.to_be_bytes());
        f.push(comps.len() as u8);
        for (id, hv, tq) in comps {
            f.extend_from_slice(&[*id, *hv, *tq]);
        }
        seg(&mut d, sof, &f);
        seg(&mut d, 0xDD, &[0, 4]);
        seg(&mut d, 0xDA, &[1, 1, 0, 0, 63, 0]);
        d.extend_from_slice(&[0x12, 0x34, 0xFF, 0x00, 0x56]);
        d
    }

    const YCC420: [(u8, u8, u8); 3] = [(1, 0x22, 0), (2, 0x11, 1), (3, 0x11, 1)];

    #[test]
    fn parses_baseline_420() {
        let d = header(
            0xC0,
            6000,
            4000,
            &YCC420,
            &[(0xE0, b"JFIF\0\x01\x01".to_vec())],
        );
        let i = parse(&d).unwrap();
        assert_eq!((i.width, i.height), (6000, 4000));
        assert_eq!(i.process, Process::Baseline);
        assert_eq!(i.subsampling(), Subsampling::S420);
        assert_eq!(i.mcu_size(), (16, 16));
        assert_eq!(i.restart_interval, 4);
        assert!(i.jfif && i.is_ycbcr());
        assert_eq!(&d[i.first_scan..i.first_scan + 2], &[0xFF, 0xDA]);
    }

    #[test]
    fn classifies_subsampling() {
        let s = |y: u8| {
            let c = [(1, y, 0), (2, 0x11, 1), (3, 0x11, 1)];
            parse(&header(0xC2, 10, 10, &c, &[])).unwrap().subsampling()
        };
        assert_eq!(s(0x11), Subsampling::S444);
        assert_eq!(s(0x21), Subsampling::S422);
        assert_eq!(s(0x12), Subsampling::S440);
        assert_eq!(s(0x41), Subsampling::S411);
        let g = parse(&header(0xC0, 10, 10, &[(1, 0x11, 0)], &[])).unwrap();
        assert_eq!(g.subsampling(), Subsampling::Gray);
    }

    #[test]
    fn finds_app_segments() {
        let exif = b"Exif\0\0MM\0\x2a\0\0\0\x08".to_vec();
        let mpf = b"MPF\0II\x2a\0".to_vec();
        let mut icc2 = b"ICC_PROFILE\0\x02\x02".to_vec();
        icc2.extend_from_slice(b"world");
        let mut icc1 = b"ICC_PROFILE\0\x01\x02".to_vec();
        icc1.extend_from_slice(b"hello ");
        let adobe = b"Adobe\0\x64\0\0\0\0\x02".to_vec();
        let d = header(
            0xC0,
            8,
            8,
            &[(1, 0x11, 0), (2, 0x11, 0), (3, 0x11, 0), (4, 0x11, 0)],
            &[
                (0xE1, exif),
                (0xE2, mpf),
                (0xE2, icc2),
                (0xE2, icc1),
                (0xEE, adobe),
            ],
        );
        let i = parse(&d).unwrap();
        assert_eq!(&d[i.exif.clone().unwrap()][..4], b"MM\0\x2a");
        assert_eq!(&d[i.mpf.clone().unwrap()][..4], b"II\x2a\0");
        assert_eq!(icc_profile(&d, &i).unwrap(), b"hello world");
        assert_eq!(i.adobe, Some(AdobeTransform::Ycck));
        assert!(!i.is_ycbcr());
    }

    #[test]
    fn header_prefix_is_enough() {
        let d = header(0xC0, 100, 50, &YCC420, &[]);
        let sos = parse(&d).unwrap().first_scan;
        // Cut right after the SOS segment: still parses.
        let i = parse(&d[..sos + 10]).unwrap();
        assert_eq!((i.width, i.height), (100, 50));
        // Cut before SOS: truncated.
        assert_eq!(parse(&d[..sos - 3]), Err(Error::Truncated));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse(b"\x89PNG\r\n"), Err(Error::NotJpeg));
        assert_eq!(parse(&[0xFF, 0xD8, 0xFF]), Err(Error::Truncated));
        let mut d = header(0xC0, 0, 10, &YCC420, &[]);
        assert_eq!(parse(&d), Err(Error::ZeroSize));
        d = header(0xC0, 10, 10, &[(1, 0x55, 0)], &[]);
        assert_eq!(parse(&d), Err(Error::BadSegment(0xC0)));
    }

    #[test]
    fn dct_scale_choice() {
        let i = parse(&header(0xC0, 6000, 4000, &YCC420, &[])).unwrap();
        assert_eq!(i.dct_scale_for(750, 500), 1);
        assert_eq!(i.dct_scale_for(751, 500), 2);
        assert_eq!(i.dct_scale_for(2560, 1440), 4);
        assert_eq!(i.dct_scale_for(7000, 1), 8);
        assert_eq!(scaled(6001, 1), 751);
    }
}
