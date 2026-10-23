//! Progressive Huffman JPEGs (JPEG Annex G), ported from libjpeg-turbo's
//! `jdphuff.c` so the coefficients, and with them the output, are
//! identical.
//!
//! All scans are decoded into one coefficient buffer per component; the
//! IDCT then runs once, in parallel bands of MCU rows.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::bits::{Bits, Stream, extend, unstuff};
use crate::decoder::{Frame, Tables};
use crate::huffman::Huffman;
use crate::scan::{ZIGZAG, write_block};
use crate::{Error, MAX_COMPONENTS, Segments};

/// Coefficients of one component, natural order, 64 per block, over the
/// whole MCU grid.
struct Coefs {
    blocks_x: usize,
    data: Vec<i16>,
    /// Quantization table (zigzag order) from the component's first scan,
    /// as libjpeg latches it.
    quant: Option<[i32; 64]>,
}

impl Coefs {
    #[inline(always)]
    fn block(&mut self, bx: usize, by: usize) -> &mut [i16; 64] {
        let i = (by * self.blocks_x + bx) * 64;
        (&mut self.data[i..i + 64]).try_into().unwrap()
    }
}

/// One component of a progressive scan.
struct PComp<'t> {
    comp: usize,
    dc: Option<&'t Huffman>,
    ac: Option<&'t Huffman>,
}

#[derive(Clone, Copy)]
struct Band {
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
}

/// Decodes a progressive JPEG into `planes`; returns the threads used for
/// the IDCT.
pub(crate) fn decode(
    data: &[u8],
    frame: &Frame,
    planes: &mut [&mut [u8]],
    threads: usize,
) -> Result<usize, Error> {
    let geo = frame.geo;
    let mut coefs: Vec<Coefs> = geo
        .comps
        .iter()
        .map(|&(h, v, _, _)| {
            let (bx, by) = ((geo.mcus_x * h) as usize, (geo.mcus_y * v) as usize);
            Coefs {
                blocks_x: bx,
                data: vec![0; bx * by * 64],
                quant: None,
            }
        })
        .collect();

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
                    let (comps, band) = scan_header(p, frame, &tables)?;
                    for c in &comps {
                        let tq = frame.info.components()[c.comp].tq as usize & 3;
                        if coefs[c.comp].quant.is_none() {
                            coefs[c.comp].quant = tables.quant[tq];
                        }
                    }
                    let stream = unstuff(data, seg.payload.end);
                    let ri = tables.restart_interval;
                    // A broken scan keeps what earlier scans produced, as
                    // libjpeg does with a warning.
                    if decode_scan(&stream, frame, &comps, band, ri, &mut coefs).is_err() {
                        break 'file;
                    }
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
            None => break,
        }
    }
    if scans == 0 {
        return Err(Error::NoFrame);
    }
    output(frame, &coefs, planes, threads)
}

fn scan_header<'t>(
    p: &[u8],
    frame: &Frame,
    tables: &'t Tables,
) -> Result<(Vec<PComp<'t>>, Band), Error> {
    let bad = Error::BadSegment(0xDA);
    let n = *p.first().ok_or(bad.clone())? as usize;
    if n == 0 || n > MAX_COMPONENTS || p.len() < 1 + 2 * n + 3 {
        return Err(bad);
    }
    let band = Band {
        ss: p[1 + 2 * n] as usize,
        se: p[2 + 2 * n] as usize,
        ah: (p[3 + 2 * n] >> 4) as u32,
        al: (p[3 + 2 * n] & 15) as u32,
    };
    // JPEG G.1.1.1.1: DC scans may interleave, AC scans are single
    // component; bands stay within 0..=63.
    let dc = band.ss == 0;
    if (dc && band.se != 0)
        || (!dc && (band.se < band.ss || band.se > 63 || n != 1))
        || band.al > 13
        || band.ah > 13
    {
        return Err(Error::Corrupt("progressive scan parameters"));
    }
    let comps = (0..n)
        .map(|i| {
            let (id, sel) = (p[1 + 2 * i], p[2 + 2 * i]);
            let comp = frame
                .info
                .components()
                .iter()
                .position(|c| c.id == id)
                .ok_or(Error::Corrupt("scan names an unknown component"))?;
            let missing = Error::Corrupt("scan uses an undefined table");
            // Refining DC scans read raw bits and need no table.
            let dc_table = if dc && band.ah == 0 {
                Some(
                    tables.dc[(sel >> 4) as usize & 3]
                        .as_deref()
                        .ok_or(missing.clone())?,
                )
            } else {
                None
            };
            let ac_table = if dc {
                None
            } else {
                Some(
                    tables.ac[(sel & 15) as usize & 3]
                        .as_deref()
                        .ok_or(missing)?,
                )
            };
            Ok(PComp {
                comp,
                dc: dc_table,
                ac: ac_table,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok((comps, band))
}

/// Entropy-decoder state that persists across blocks.
struct Ctx<'a> {
    bits: Bits<'a>,
    eobrun: u32,
    preds: [i32; MAX_COMPONENTS],
}

fn decode_scan(
    stream: &Stream,
    frame: &Frame,
    comps: &[PComp],
    band: Band,
    restart_interval: u32,
    coefs: &mut [Coefs],
) -> Result<(), Error> {
    let geo = frame.geo;
    let mut cx = Ctx {
        bits: Bits::new(&stream.data, 0),
        eobrun: 0,
        preds: [0; MAX_COMPONENTS],
    };
    let interleaved = comps.len() > 1;
    let (cols, rows) = if interleaved {
        (geo.mcus_x as usize, geo.mcus_y as usize)
    } else {
        let (_, _, w, h) = geo.comps[comps[0].comp];
        (w.div_ceil(8) as usize, h.div_ceil(8) as usize)
    };
    let mut restarts = stream.restarts.iter();
    for m in 0..cols * rows {
        let ri = restart_interval as usize;
        if ri != 0 && m != 0 && m.is_multiple_of(ri) {
            let pos = match restarts.next() {
                Some(&b) => b * 8,
                None => cx.bits.pos().div_ceil(8) * 8,
            };
            cx.bits = Bits::new(&stream.data, pos);
            cx.eobrun = 0;
            cx.preds = [0; MAX_COMPONENTS];
        }
        let (mx, my) = (m % cols, m / cols);
        for (i, pc) in comps.iter().enumerate() {
            let (h, v) = if interleaved {
                let (h, v, _, _) = geo.comps[pc.comp];
                (h as usize, v as usize)
            } else {
                (1, 1)
            };
            for by in 0..v {
                for bx in 0..h {
                    let blk = coefs[pc.comp].block(mx * h + bx, my * v + by);
                    match (band.ss == 0, band.ah == 0) {
                        (true, true) => dc_first(&mut cx, pc, i, band, blk)?,
                        (true, false) => dc_refine(&mut cx, band, blk),
                        (false, true) => ac_first(&mut cx, pc, band, blk)?,
                        (false, false) => ac_refine(&mut cx, pc, band, blk)?,
                    }
                }
            }
        }
    }
    Ok(())
}

#[inline(always)]
fn dc_first(
    cx: &mut Ctx,
    pc: &PComp,
    i: usize,
    band: Band,
    blk: &mut [i16; 64],
) -> Result<(), Error> {
    let s = pc.dc.unwrap().decode(&mut cx.bits)?;
    if s > 16 {
        return Err(Error::Corrupt("DC magnitude"));
    }
    let diff = if s == 0 { 0 } else { extend(cx.bits.get(s), s) };
    cx.preds[i] = cx.preds[i].wrapping_add(diff);
    blk[0] = (cx.preds[i] << band.al) as i16;
    Ok(())
}

#[inline(always)]
fn dc_refine(cx: &mut Ctx, band: Band, blk: &mut [i16; 64]) {
    if cx.bits.get(1) != 0 {
        blk[0] |= (1 << band.al) as i16;
    }
}

#[inline(always)]
fn ac_first(cx: &mut Ctx, pc: &PComp, band: Band, blk: &mut [i16; 64]) -> Result<(), Error> {
    if cx.eobrun > 0 {
        cx.eobrun -= 1;
        return Ok(());
    }
    let ac = pc.ac.unwrap();
    let mut k = band.ss;
    while k <= band.se {
        let rs = ac.decode(&mut cx.bits)?;
        let (r, s) = ((rs >> 4) as usize, rs & 15);
        if s != 0 {
            k += r;
            let v = extend(cx.bits.get(s), s);
            if k > 63 {
                break;
            }
            blk[ZIGZAG[k]] = (v << band.al) as i16;
        } else if r == 15 {
            k += 15;
        } else {
            cx.eobrun = (1 << r) - 1;
            if r != 0 {
                cx.eobrun += cx.bits.get(r as u32);
            }
            break;
        }
        k += 1;
    }
    Ok(())
}

#[inline(always)]
fn ac_refine(cx: &mut Ctx, pc: &PComp, band: Band, blk: &mut [i16; 64]) -> Result<(), Error> {
    let p1 = (1i32 << band.al) as i16;
    let m1 = (-1i32 << band.al) as i16;
    let se = band.se;
    let mut k = band.ss;
    // Appends a correction bit to an already-nonzero coefficient.
    let correct = |bits: &mut Bits, c: &mut i16| {
        if bits.get(1) != 0 && *c & p1 == 0 {
            *c = c.wrapping_add(if *c >= 0 { p1 } else { m1 });
        }
    };
    if cx.eobrun == 0 {
        let ac = pc.ac.unwrap();
        while k <= se {
            let rs = ac.decode(&mut cx.bits)?;
            let (mut r, s) = ((rs >> 4) as i32, rs & 15);
            let mut newval = 0i16;
            if s != 0 {
                // s must be 1 (libjpeg warns otherwise and goes on).
                newval = if cx.bits.get(1) != 0 { p1 } else { m1 };
            } else if r != 15 {
                cx.eobrun = 1 << r;
                if r != 0 {
                    cx.eobrun += cx.bits.get(r as u32);
                }
                break;
            }
            // Skip already-nonzero coefficients (correcting them) and r
            // still-zero ones.
            while k <= se {
                let c = &mut blk[ZIGZAG[k]];
                if *c != 0 {
                    correct(&mut cx.bits, c);
                } else {
                    r -= 1;
                    if r < 0 {
                        break;
                    }
                }
                k += 1;
            }
            if newval != 0 && k <= 63 {
                blk[ZIGZAG[k]] = newval;
            }
            k += 1;
        }
    }
    if cx.eobrun > 0 {
        // The rest of the band only gets correction bits.
        while k <= se {
            let c = &mut blk[ZIGZAG[k]];
            if *c != 0 {
                correct(&mut cx.bits, c);
            }
            k += 1;
        }
        cx.eobrun -= 1;
    }
    Ok(())
}

/// Dequantizes and transforms all blocks into the planes, in parallel
/// bands of MCU rows.
fn output(
    frame: &Frame,
    coefs: &[Coefs],
    planes: &mut [&mut [u8]],
    threads: usize,
) -> Result<usize, Error> {
    let geo = frame.geo;
    let bs = frame.scale as usize;
    let rows = geo.mcus_y as usize;
    let threads = threads.min(rows).max(1);
    let nbands = (threads * 4).min(rows);
    let bands: Vec<(usize, usize)> = (0..nbands)
        .map(|i| (rows * i / nbands, rows * (i + 1) / nbands))
        .collect();

    // Each band's rows of every plane.
    let mut per_band: Vec<Vec<&mut [u8]>> = (0..nbands).map(|_| Vec::new()).collect();
    for (c, plane) in planes.iter_mut().enumerate() {
        let row_bytes = geo.comps[c].1 as usize * bs * frame.layouts[c].stride;
        let mut rest: &mut [u8] = plane;
        for (b, &(r0, r1)) in bands.iter().enumerate() {
            let (head, tail) = rest.split_at_mut(((r1 - r0) * row_bytes).min(rest.len()));
            per_band[b].push(head);
            rest = tail;
        }
    }
    let quants: Vec<[i32; 64]> = coefs.iter().map(|c| c.quant.unwrap_or([1; 64])).collect();

    let work: Vec<Mutex<Option<Vec<&mut [u8]>>>> =
        per_band.into_iter().map(|p| Mutex::new(Some(p))).collect();
    let next = AtomicUsize::new(0);
    let run = || {
        crate::parallel::thread_init();
        loop {
            let b = next.fetch_add(1, Ordering::Relaxed);
            let Some(slot) = work.get(b) else { break };
            let mut outs = slot.lock().unwrap().take().unwrap();
            let (r0, r1) = bands[b];
            for (c, out) in outs.iter_mut().enumerate() {
                let (h, v, _, _) = geo.comps[c];
                let (h, v) = (h as usize, v as usize);
                let stride = frame.layouts[c].stride;
                let cf = &coefs[c];
                let q = &quants[c];
                let mut block = [0i32; 64];
                for by in r0 * v..r1 * v {
                    for bx in 0..cf.blocks_x.min(geo.mcus_x as usize * h) {
                        let i = (by * cf.blocks_x + bx) * 64;
                        let src = &cf.data[i..i + 64];
                        let mut any_ac = false;
                        for k in 0..64 {
                            let z = ZIGZAG[k];
                            let val = src[z];
                            if val != 0 {
                                block[z] = (val as i32).wrapping_mul(q[k]);
                                any_ac |= k != 0;
                            }
                        }
                        let y = (by - r0 * v) * bs;
                        let dst = &mut out[y * stride + bx * bs..];
                        write_block(&mut block, any_ac, bs, dst, stride);
                    }
                }
            }
        }
    };
    if threads == 1 {
        run();
    } else {
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(run);
            }
        });
    }
    Ok(threads)
}
