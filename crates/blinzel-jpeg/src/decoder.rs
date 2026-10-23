//! Sequential Huffman JPEG decoding into YCbCr planes (milestone M3, stage
//! 1). The output is byte-identical with libjpeg-turbo's
//! `tj3DecompressToYUVPlanes8` at the same DCT scale.

use crate::bits::{Bits, Stream, extend, unstuff};
use crate::huffman::Huffman;
use crate::idct;
use crate::{Coding, Error, JpegInfo, MAX_COMPONENTS, Process, Segments, parse};

/// Zigzag index → natural (row-major) index.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

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
            Process::Baseline | Process::ExtendedSequential
        )
}

/// Frame geometry: MCU grid and per-component sizes.
struct Geometry {
    mcus_x: u32,
    mcus_y: u32,
    /// Per component: sampling factors and size in samples.
    comps: Vec<(u32, u32, u32, u32)>,
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
struct Tables {
    /// Quantization tables in zigzag order.
    quant: [Option<[i32; 64]>; 4],
    dc: [Option<Box<Huffman>>; 4],
    ac: [Option<Box<Huffman>>; 4],
    restart_interval: u32,
}

impl Tables {
    fn read_dqt(&mut self, mut p: &[u8]) -> Result<(), Error> {
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

    fn read_dht(&mut self, mut p: &[u8]) -> Result<(), Error> {
        while p.len() >= 17 {
            let (class, id) = (p[0] >> 4, (p[0] & 15) as usize);
            let counts: [u8; 16] = p[1..17].try_into().unwrap();
            let total: usize = counts.iter().map(|&c| c as usize).sum();
            if class > 1 || id > 3 || p.len() < 17 + total {
                return Err(Error::BadSegment(0xC4));
            }
            let table = Huffman::new(&counts, &p[17..17 + total])?;
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

/// One component of a scan, resolved against the frame and tables.
struct ScanComp<'t> {
    comp: usize,
    dc: &'t Huffman,
    ac: &'t Huffman,
    quant: &'t [i32; 64],
}

fn scan_components<'t>(
    p: &[u8],
    info: &JpegInfo,
    tables: &'t Tables,
) -> Result<Vec<ScanComp<'t>>, Error> {
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
    (0..n)
        .map(|i| {
            let (id, sel) = (p[1 + 2 * i], p[2 + 2 * i]);
            let comp = info
                .components()
                .iter()
                .position(|c| c.id == id)
                .ok_or(Error::Corrupt("scan names an unknown component"))?;
            let missing = Error::Corrupt("scan uses an undefined table");
            Ok(ScanComp {
                comp,
                dc: tables.dc[(sel >> 4) as usize & 3]
                    .as_deref()
                    .ok_or(missing.clone())?,
                ac: tables.ac[(sel & 15) as usize & 3]
                    .as_deref()
                    .ok_or(missing.clone())?,
                quant: tables.quant[info.components()[comp].tq as usize & 3]
                    .as_ref()
                    .ok_or(missing)?,
            })
        })
        .collect()
}

/// Decodes the whole image at `scale` (1, 2, 4 or 8 eighths) into one
/// buffer per component, each at least [`plane_layouts`]`[i].len()` bytes.
pub fn decode_planes(data: &[u8], scale: u32, planes: &mut [&mut [u8]]) -> Result<(), Error> {
    let info = parse(data)?;
    if !is_supported(&info) {
        return Err(Error::Unsupported("not 8-bit sequential Huffman"));
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

    let mut tables = Tables::default();
    let mut pos = 0;
    let mut scans = 0;
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
                    let comps = scan_components(p, &info, &tables)?;
                    let stream = unstuff(data, seg.payload.end);
                    let scan = Scan {
                        geo: &geo,
                        layouts: &layouts,
                        comps: &comps,
                        scale,
                        restart_interval: tables.restart_interval,
                    };
                    scan.decode(&stream, planes)?;
                    scans += 1;
                    next = Some(stream.end);
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
    Ok(())
}

struct Scan<'a> {
    geo: &'a Geometry,
    layouts: &'a [PlaneLayout],
    comps: &'a [ScanComp<'a>],
    scale: u32,
    restart_interval: u32,
}

impl Scan<'_> {
    fn decode(&self, stream: &Stream, planes: &mut [&mut [u8]]) -> Result<(), Error> {
        let mut bits = Bits::new(&stream.data, 0);
        let mut preds = [0i32; MAX_COMPONENTS];
        let mut block = [0i32; 64];
        let bs = self.scale as usize;
        let mut mcu = 0u32;
        let mut restart = 0usize;
        let mut next_mcu = |bits: &mut Bits, preds: &mut [i32; MAX_COMPONENTS]| {
            if self.restart_interval != 0 && mcu != 0 && mcu.is_multiple_of(self.restart_interval) {
                // Resynchronize at the marker; without one, at the next byte.
                bits.pos = match stream.restarts.get(restart) {
                    Some(&b) => b * 8,
                    None => bits.pos.div_ceil(8) * 8,
                };
                restart += 1;
                *preds = [0; MAX_COMPONENTS];
            }
            mcu += 1;
        };

        if let [sc] = self.comps {
            // Non-interleaved: one block per MCU, raster order over the
            // component's own blocks.
            let (_, _, w, h) = self.geo.comps[sc.comp];
            let stride = self.layouts[sc.comp].stride;
            for by in 0..h.div_ceil(8) as usize {
                for bx in 0..w.div_ceil(8) as usize {
                    next_mcu(&mut bits, &mut preds);
                    let out = &mut planes[sc.comp][by * bs * stride + bx * bs..];
                    block_out(
                        sc,
                        &mut bits,
                        &mut preds[sc.comp],
                        &mut block,
                        bs,
                        out,
                        stride,
                    )?;
                }
            }
            return Ok(());
        }

        for my in 0..self.geo.mcus_y as usize {
            for mx in 0..self.geo.mcus_x as usize {
                next_mcu(&mut bits, &mut preds);
                for sc in self.comps {
                    let (h, v, _, _) = self.geo.comps[sc.comp];
                    let (h, v) = (h as usize, v as usize);
                    let stride = self.layouts[sc.comp].stride;
                    for by in 0..v {
                        for bx in 0..h {
                            let (x, y) = ((mx * h + bx) * bs, (my * v + by) * bs);
                            let out = &mut planes[sc.comp][y * stride + x..];
                            block_out(
                                sc,
                                &mut bits,
                                &mut preds[sc.comp],
                                &mut block,
                                bs,
                                out,
                                stride,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Decodes one block and writes its `bs × bs` output.
#[inline(always)]
fn block_out(
    sc: &ScanComp,
    bits: &mut Bits,
    pred: &mut i32,
    block: &mut [i32; 64],
    bs: usize,
    out: &mut [u8],
    stride: usize,
) -> Result<(), Error> {
    if bs == 1 {
        // Only DC matters at 1/8; AC codes are skipped, not stored.
        decode_block::<false>(sc, bits, pred, block)?;
        out[0] = idct::dc_value(block[0]);
        return Ok(());
    }
    let any_ac = decode_block::<true>(sc, bits, pred, block)?;
    if !any_ac {
        idct::fill(out, stride, bs, idct::dc_value(block[0]));
    } else {
        match bs {
            8 => idct::islow(block, out, stride),
            4 => idct::idct4x4(block, out, stride),
            _ => idct::idct2x2(block, out, stride),
        }
        *block = [0; 64];
    }
    Ok(())
}

/// Decodes one block's coefficients, dequantized, into `block` (which must
/// be zero except for the DC slot). Returns whether any AC coefficient was
/// stored. With `STORE_AC = false` AC coefficients are only skipped.
#[inline(always)]
fn decode_block<const STORE_AC: bool>(
    sc: &ScanComp,
    bits: &mut Bits,
    pred: &mut i32,
    block: &mut [i32; 64],
) -> Result<bool, Error> {
    let s = sc.dc.decode(bits)?;
    if s > 16 {
        return Err(Error::Corrupt("DC magnitude"));
    }
    let diff = if s == 0 { 0 } else { extend(bits.get(s), s) };
    *pred = pred.wrapping_add(diff);
    block[0] = pred.wrapping_mul(sc.quant[0]);

    let mut any = false;
    let mut k = 1;
    while k < 64 {
        let p = bits.peek();
        let f = sc.ac.ac_fast(p);
        if f != 0 {
            k += ((f >> 8) & 0xFF) as usize;
            bits.skip((f & 0xFF) as u32);
            if k >= 64 {
                break;
            }
            if STORE_AC {
                block[ZIGZAG[k]] = (f >> 16) * sc.quant[k];
                any = true;
            }
            k += 1;
            continue;
        }
        let rs = sc.ac.decode(bits)?;
        let (r, s) = ((rs >> 4) as usize, rs & 15);
        if s == 0 {
            if r != 15 {
                break;
            }
            k += 16;
            continue;
        }
        k += r;
        let v = extend(bits.get(s), s);
        if k >= 64 {
            break;
        }
        if STORE_AC {
            block[ZIGZAG[k]] = v.wrapping_mul(sc.quant[k]);
            any = true;
        }
        k += 1;
    }
    Ok(any)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_is_a_permutation() {
        let mut seen = [false; 64];
        for &z in &ZIGZAG {
            assert!(!seen[z]);
            seen[z] = true;
        }
        assert_eq!(ZIGZAG[2], 8);
        assert_eq!(ZIGZAG[63], 63);
    }

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
        assert!(decode_planes(b"not a jpeg", 8, &mut planes).is_err());
    }
}
