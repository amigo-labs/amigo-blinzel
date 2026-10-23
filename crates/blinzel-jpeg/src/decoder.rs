//! Huffman JPEG decoding into YCbCr planes (milestone M3): frame setup and
//! the sequential scan loop; progressive files go to `progressive`. The
//! output is byte-identical with libjpeg-turbo's
//! `tj3DecompressToYUVPlanes8` at the same DCT scale.

use std::sync::Arc;

use crate::bits::unstuff;
use crate::huffman::Huffman;
use crate::parallel;
use crate::progressive;
use crate::scan::{Scan, ScanComp};
use crate::{Coding, Error, JpegInfo, MAX_COMPONENTS, Process, Segments, parse};

/// Geometry of one output plane at a DCT scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneLayout {
    /// Samples that belong to the image.
    pub width: u32,
    pub height: u32,
    /// Bytes per buffer row; the buffer covers whole MCUs.
    pub stride: usize,
    /// Buffer rows.
    pub rows: usize,
}

impl PlaneLayout {
    /// Buffer size in bytes.
    pub fn len(&self) -> usize {
        self.stride * self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The DCT scales (numerator over 8) the decoder produces directly.
pub const SCALES: [u32; 4] = [1, 2, 4, 8];

/// The smallest supported scale that is at least `num / 8`.
pub fn supported_scale(num: u32) -> u32 {
    SCALES.into_iter().find(|&s| s >= num).unwrap_or(8)
}

/// Whether [`decode_planes`] handles this file; otherwise use another
/// decoder.
pub fn is_supported(info: &JpegInfo) -> bool {
    info.precision == 8
        && info.coding == Coding::Huffman
        && matches!(
            info.process,
            Process::Baseline | Process::ExtendedSequential | Process::Progressive
        )
}

/// Frame geometry: MCU grid and per-component sizes.
pub(crate) struct Geometry {
    pub mcus_x: u32,
    pub mcus_y: u32,
    /// Per component: sampling factors and size in samples.
    pub comps: Vec<(u32, u32, u32, u32)>,
}

impl Geometry {
    fn new(info: &JpegInfo) -> Geometry {
        let (hmax, vmax) = info.max_sampling();
        let (hmax, vmax) = (hmax as u32, vmax as u32);
        let comps = info
            .components()
            .iter()
            .map(|c| {
                let (h, v) = (c.h as u32, c.v as u32);
                (
                    h,
                    v,
                    (info.width * h).div_ceil(hmax),
                    (info.height * v).div_ceil(vmax),
                )
            })
            .collect();
        Geometry {
            mcus_x: info.width.div_ceil(8 * hmax),
            mcus_y: info.height.div_ceil(8 * vmax),
            comps,
        }
    }

    fn layout(&self, c: usize, scale: u32) -> PlaneLayout {
        let (h, v, w, ht) = self.comps[c];
        PlaneLayout {
            width: (w * scale).div_ceil(8),
            height: (ht * scale).div_ceil(8),
            stride: (self.mcus_x * h * scale) as usize,
            rows: (self.mcus_y * v * scale) as usize,
        }
    }
}

/// Output planes for every component at `scale` (1, 2, 4 or 8).
pub fn plane_layouts(info: &JpegInfo, scale: u32) -> Vec<PlaneLayout> {
    let g = Geometry::new(info);
    (0..g.comps.len()).map(|c| g.layout(c, scale)).collect()
}

#[derive(Default)]
pub(crate) struct Tables {
    /// Quantization tables in zigzag order.
    pub quant: [Option<[i32; 64]>; 4],
    /// Shared so progressive scans can keep tables that later DHT
    /// segments replace.
    pub dc: [Option<Arc<Huffman>>; 4],
    pub ac: [Option<Arc<Huffman>>; 4],
    pub restart_interval: u32,
}

impl Tables {
    pub fn read_dqt(&mut self, mut p: &[u8]) -> Result<(), Error> {
        while let Some((&pq_tq, rest)) = p.split_first() {
            let (wide, id) = (pq_tq >> 4 != 0, (pq_tq & 15) as usize);
            let n = if wide { 128 } else { 64 };
            if id > 3 || rest.len() < n {
                return Err(Error::BadSegment(0xDB));
            }
            let mut q = [0i32; 64];
            for (k, v) in q.iter_mut().enumerate() {
                *v = if wide {
                    u16::from_be_bytes([rest[2 * k], rest[2 * k + 1]]) as i32
                } else {
                    rest[k] as i32
                };
            }
            self.quant[id] = Some(q);
            p = &rest[n..];
        }
        Ok(())
    }

    pub fn read_dht(&mut self, mut p: &[u8]) -> Result<(), Error> {
        while p.len() >= 17 {
            let (class, id) = (p[0] >> 4, (p[0] & 15) as usize);
            let counts: [u8; 16] = p[1..17].try_into().unwrap();
            let total: usize = counts.iter().map(|&c| c as usize).sum();
            if class > 1 || id > 3 || p.len() < 17 + total {
                return Err(Error::BadSegment(0xC4));
            }
            let table = Arc::from(Huffman::new(&counts, &p[17..17 + total])?);
            if class == 0 {
                self.dc[id] = Some(table);
            } else {
                self.ac[id] = Some(table);
            }
            p = &p[17 + total..];
        }
        Ok(())
    }
}

/// What a scan is resolved against.
pub(crate) struct Frame<'a> {
    pub info: &'a JpegInfo,
    pub geo: &'a Geometry,
    pub layouts: &'a [PlaneLayout],
    pub scale: u32,
}

/// Resolves a sequential scan header (payload `p`, entropy-coded data from
/// `entropy_start`) against the frame and tables.
fn scan_header<'t>(
    p: &[u8],
    frame: &Frame,
    tables: &'t Tables,
    data: &[u8],
    entropy_start: usize,
) -> Result<Scan<'t>, Error> {
    let Frame {
        info,
        geo,
        layouts,
        scale,
    } = *frame;
    let bad = Error::BadSegment(0xDA);
    let n = *p.first().ok_or(bad.clone())? as usize;
    if n == 0 || n > MAX_COMPONENTS || p.len() < 1 + 2 * n + 3 {
        return Err(bad);
    }
    let (ss, se, a) = (p[1 + 2 * n], p[2 + 2 * n], p[3 + 2 * n]);
    if ss != 0 || se != 63 || a != 0 {
        return Err(Error::Unsupported(
            "spectral selection in a sequential scan",
        ));
    }
    let comps = (0..n)
        .map(|i| {
            let (id, sel) = (p[1 + 2 * i], p[2 + 2 * i]);
            let plane = info
                .components()
                .iter()
                .position(|c| c.id == id)
                .ok_or(Error::Corrupt("scan names an unknown component"))?;
            let missing = Error::Corrupt("scan uses an undefined table");
            let (h, v, _, _) = geo.comps[plane];
            // Non-interleaved scans have one block per MCU.
            let (h, v) = if n == 1 {
                (1, 1)
            } else {
                (h as usize, v as usize)
            };
            Ok(ScanComp {
                plane,
                dc: tables.dc[(sel >> 4) as usize & 3]
                    .as_deref()
                    .ok_or(missing.clone())?,
                ac: tables.ac[(sel & 15) as usize & 3]
                    .as_deref()
                    .ok_or(missing.clone())?,
                quant: tables.quant[info.components()[plane].tq as usize & 3]
                    .as_ref()
                    .ok_or(missing)?,
                h,
                v,
                stride: layouts[plane].stride,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    // A non-interleaved scan covers the component's own blocks.
    let (cols, rows) = if let [c] = comps.as_slice() {
        let (_, _, w, h) = geo.comps[c.plane];
        (w.div_ceil(8), h.div_ceil(8))
    } else {
        (geo.mcus_x, geo.mcus_y)
    };
    Ok(Scan {
        comps,
        cols,
        rows,
        bs: scale as usize,
        restart_interval: tables.restart_interval,
        stream: unstuff(data, entropy_start),
    })
}

/// Decodes the whole image at `scale` (1, 2, 4 or 8 eighths) into one
/// buffer per component, each at least [`plane_layouts`]`[i].len()` bytes,
/// using up to `threads` threads. Returns the most threads a scan used
/// (1: decoded sequentially, e.g. because the image is small).
pub fn decode_planes(
    data: &[u8],
    scale: u32,
    planes: &mut [&mut [u8]],
    threads: usize,
) -> Result<usize, Error> {
    let info = parse(data)?;
    if !is_supported(&info) {
        return Err(Error::Unsupported("not 8-bit Huffman"));
    }
    if !SCALES.contains(&scale) {
        return Err(Error::Unsupported("DCT scale"));
    }
    let geo = Geometry::new(&info);
    let layouts: Vec<PlaneLayout> = (0..geo.comps.len()).map(|c| geo.layout(c, scale)).collect();
    if planes.len() != layouts.len() || planes.iter().zip(&layouts).any(|(p, l)| p.len() < l.len())
    {
        return Err(Error::Unsupported("output buffers too small"));
    }

    let frame = Frame {
        info: &info,
        geo: &geo,
        layouts: &layouts,
        scale,
    };
    if info.process == Process::Progressive {
        return progressive::decode(data, &frame, planes, threads);
    }
    let mut tables = Tables::default();
    let mut pos = 0;
    let mut scans = 0;
    let mut used = 1;
    'file: loop {
        let mut next = None;
        let segs = if pos == 0 {
            Segments::new(data)
        } else {
            Segments::at(data, pos)
        };
        for seg in segs {
            let p = &data[seg.payload.clone()];
            match seg.marker {
                0xDB => tables.read_dqt(p)?,
                0xC4 => tables.read_dht(p)?,
                0xDD if p.len() >= 2 => {
                    tables.restart_interval = u16::from_be_bytes([p[0], p[1]]) as u32;
                }
                0xDA => {
                    let scan = scan_header(p, &frame, &tables, data, seg.payload.end)?;
                    // The scan's planes, in the scan's component order.
                    let mut slots: Vec<Option<&mut [u8]>> =
                        planes.iter_mut().map(|p| Some(&mut **p)).collect();
                    let mut scan_planes = scan
                        .comps
                        .iter()
                        .map(|c| slots[c.plane].take())
                        .collect::<Option<Vec<_>>>()
                        .ok_or(Error::Corrupt("component repeated in a scan"))?;
                    used = used.max(parallel::decode_scan(&scan, &mut scan_planes, threads)?);
                    scans += 1;
                    next = Some(scan.stream.end);
                    break;
                }
                0xD9 => break 'file,
                _ => {}
            }
        }
        match next {
            Some(n) => pos = n,
            // End of data: a truncated file keeps what was decoded.
            None => break,
        }
    }
    if scans == 0 {
        return Err(Error::NoFrame);
    }
    Ok(used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_round_up() {
        assert_eq!(supported_scale(1), 1);
        assert_eq!(supported_scale(3), 4);
        assert_eq!(supported_scale(5), 8);
        assert_eq!(supported_scale(8), 8);
    }

    #[test]
    fn rejects_garbage() {
        let mut planes: [&mut [u8]; 0] = [];
        assert!(decode_planes(b"not a jpeg", 8, &mut planes, 1).is_err());
    }
}
