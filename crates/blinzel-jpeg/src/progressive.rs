//! Progressive Huffman JPEGs (JPEG Annex G), ported from libjpeg-turbo's
//! `jdphuff.c` so the coefficients, and with them the output, are
//! identical.
//!
//! All scans are read first. With several threads every scan then runs in
//! its own thread, row by row behind the scans it depends on (same
//! component, overlapping spectral band): the scans of a typical file
//! overlap almost completely. Coefficients live in block rows behind
//! mutexes, so the pipeline needs no unsafe code. The IDCT runs last, in
//! parallel bands of MCU rows.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::bits::{Bits, Stream, extend, unstuff};
use crate::decoder::{Frame, Tables};
use crate::huffman::Huffman;
use crate::idct;
use crate::scan::{ZIGZAG, write_block};
use crate::{Error, MAX_COMPONENTS, Segments};

/// More scans than this run one after another (each needs a thread).
const MAX_PIPELINED_SCANS: usize = 32;

/// One block row of a component.
struct Row {
    /// Coefficients in natural order, 64 per block.
    coefs: Vec<i16>,
    /// Per block: which AC coefficients (bit = zigzag index) are nonzero,
    /// so refinement scans jump straight to them.
    nonzero: Vec<u64>,
}

impl Row {
    /// Allocates the row on first use, so zeroing 50–100 MB of
    /// coefficients happens in the scan threads, not up front.
    fn ensure(&mut self, blocks_x: usize) {
        if self.coefs.is_empty() {
            self.coefs = vec![0; blocks_x * 64];
            self.nonzero = vec![0; blocks_x];
        }
    }
}

/// Coefficients of one component, one mutex per block row of the MCU grid.
struct Coefs {
    blocks_x: usize,
    rows: Vec<Mutex<Row>>,
}

impl Coefs {
    fn new(blocks_x: usize, blocks_y: usize) -> Coefs {
        Coefs {
            blocks_x,
            rows: (0..blocks_y)
                .map(|_| {
                    Mutex::new(Row {
                        coefs: Vec::new(),
                        nonzero: Vec::new(),
                    })
                })
                .collect(),
        }
    }

    /// Locks block row `y`, allocated.
    fn row(&self, y: usize) -> MutexGuard<'_, Row> {
        let mut r = self.rows[y].lock().unwrap();
        r.ensure(self.blocks_x);
        r
    }

    fn clear(&self) {
        for r in &self.rows {
            let mut r = r.lock().unwrap();
            r.coefs.fill(0);
            r.nonzero.fill(0);
        }
    }
}

/// One component of a progressive scan.
struct PComp {
    comp: usize,
    dc: Option<Arc<Huffman>>,
    ac: Option<Arc<Huffman>>,
}

#[derive(Clone, Copy)]
struct Band {
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
}

impl Band {
    fn overlaps(&self, o: &Band) -> bool {
        self.ss <= o.se && o.ss <= self.se
    }
}

struct PScan {
    comps: Vec<PComp>,
    band: Band,
    restart_interval: u32,
    stream: Stream,
}

/// Decodes a progressive JPEG into `planes`; returns the most threads used.
pub(crate) fn decode(
    data: &[u8],
    frame: &Frame,
    planes: &mut [&mut [u8]],
    threads: usize,
) -> Result<usize, Error> {
    let geo = frame.geo;
    let (scans, quants) = read_scans(data, frame)?;
    if scans.is_empty() {
        return Err(Error::NoFrame);
    }
    let coefs: Vec<Coefs> = geo
        .comps
        .iter()
        .map(|&(h, v, _, _)| Coefs::new((geo.mcus_x * h) as usize, (geo.mcus_y * v) as usize))
        .collect();

    let pipelined = threads > 1
        && scans.len() > 1
        && scans.len() <= MAX_PIPELINED_SCANS
        && pipeline(&scans, frame, &coefs).is_ok();
    if !pipelined {
        if threads > 1 {
            // A broken scan: redo in order and keep what decodes, as
            // libjpeg does.
            for c in &coefs {
                c.clear();
            }
        }
        for scan in &scans {
            if decode_scan(scan, frame, &coefs, None).is_err() {
                break;
            }
        }
    }
    let scan_threads = if pipelined { scans.len() } else { 1 };
    let idct_threads = output(frame, &coefs, &quants, planes, threads);
    Ok(scan_threads.max(idct_threads))
}

/// Parses every scan header and unstuffs its data; also latches each
/// component's quantization table at its first scan, as libjpeg does.
fn read_scans(data: &[u8], frame: &Frame) -> Result<(Vec<PScan>, Vec<[i32; 64]>), Error> {
    let n = frame.info.components().len();
    let mut quants: Vec<Option<[i32; 64]>> = vec![None; n];
    let mut tables = Tables::default();
    let mut scans = Vec::new();
    let mut pos = 0;
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
                        if quants[c.comp].is_none() {
                            quants[c.comp] = tables.quant[tq];
                        }
                    }
                    let stream = unstuff(data, seg.payload.end);
                    next = Some(stream.end);
                    scans.push(PScan {
                        comps,
                        band,
                        restart_interval: tables.restart_interval,
                        stream,
                    });
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
    let quants = quants.into_iter().map(|q| q.unwrap_or([1; 64])).collect();
    Ok((scans, quants))
}

fn scan_header(p: &[u8], frame: &Frame, tables: &Tables) -> Result<(Vec<PComp>, Band), Error> {
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
                        .clone()
                        .ok_or(missing.clone())?,
                )
            } else {
                None
            };
            let ac_table = if dc {
                None
            } else {
                Some(tables.ac[(sel & 15) as usize & 3].clone().ok_or(missing)?)
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

/// Row-level ordering between concurrently running scans.
struct Pipeline {
    /// Per scan and component: block rows finished.
    progress: Mutex<Vec<[usize; MAX_COMPONENTS]>>,
    changed: Condvar,
    /// Per scan: the earlier scans whose results it reads.
    deps: Vec<Vec<usize>>,
}

impl Pipeline {
    /// Waits until every dependency of scan `s` has finished the given
    /// number of block rows of each component.
    fn wait(&self, s: usize, needs: &[(usize, usize)]) {
        let mut p = self.progress.lock().unwrap();
        while !self.deps[s]
            .iter()
            .all(|&d| needs.iter().all(|&(c, rows)| p[d][c] >= rows))
        {
            p = self.changed.wait(p).unwrap();
        }
    }

    fn publish(&self, s: usize, done: &[(usize, usize)]) {
        let mut p = self.progress.lock().unwrap();
        for &(c, rows) in done {
            p[s][c] = rows;
        }
        drop(p);
        self.changed.notify_all();
    }
}

fn pipeline(scans: &[PScan], frame: &Frame, coefs: &[Coefs]) -> Result<(), Error> {
    let deps = scans
        .iter()
        .enumerate()
        .map(|(j, sj)| {
            (0..j)
                .filter(|&i| {
                    let si = &scans[i];
                    si.band.overlaps(&sj.band)
                        && si
                            .comps
                            .iter()
                            .any(|a| sj.comps.iter().any(|b| a.comp == b.comp))
                })
                .collect()
        })
        .collect();
    let pipe = Pipeline {
        progress: Mutex::new(vec![[0; MAX_COMPONENTS]; scans.len()]),
        changed: Condvar::new(),
        deps,
    };
    let results: Vec<Result<(), Error>> = std::thread::scope(|s| {
        let handles: Vec<_> = scans
            .iter()
            .enumerate()
            .map(|(i, scan)| {
                let pipe = &pipe;
                s.spawn(move || {
                    crate::parallel::thread_init();
                    let r = decode_scan(scan, frame, coefs, Some((pipe, i)));
                    // Finished or failed: never keep dependents waiting.
                    let all: Vec<(usize, usize)> =
                        (0..MAX_COMPONENTS).map(|c| (c, usize::MAX)).collect();
                    pipe.publish(i, &all);
                    r
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    results.into_iter().collect()
}

/// Entropy-decoder state that persists across blocks.
struct Ctx<'a> {
    bits: Bits<'a>,
    eobrun: u32,
    preds: [i32; MAX_COMPONENTS],
}

/// Decodes one scan, row by row; in a pipeline each row first waits for
/// the scans it depends on and is published when done.
fn decode_scan(
    scan: &PScan,
    frame: &Frame,
    coefs: &[Coefs],
    pipe: Option<(&Pipeline, usize)>,
) -> Result<(), Error> {
    let geo = frame.geo;
    let (comps, band) = (&scan.comps, scan.band);
    let mut cx = Ctx {
        bits: Bits::new(&scan.stream.data, 0),
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
    // Blocks per MCU of each scan component.
    let hv: Vec<(usize, usize)> = comps
        .iter()
        .map(|pc| {
            if interleaved {
                let (h, v, _, _) = geo.comps[pc.comp];
                (h as usize, v as usize)
            } else {
                (1, 1)
            }
        })
        .collect();
    let mut restarts = scan.stream.restarts.iter();
    let ri = scan.restart_interval as usize;
    let mut extent: Vec<(usize, usize)> = Vec::with_capacity(comps.len());
    for r in 0..rows {
        extent.clear();
        extent.extend(
            comps
                .iter()
                .zip(&hv)
                .map(|(pc, &(_, v))| (pc.comp, (r + 1) * v)),
        );
        if let Some((p, s)) = pipe {
            p.wait(s, &extent);
        }
        // Lock this MCU row's block rows, in (component, row) order.
        let mut guards: Vec<Vec<MutexGuard<Row>>> = comps
            .iter()
            .zip(&hv)
            .map(|(pc, &(_, v))| {
                (r * v..(r + 1) * v)
                    .map(|y| coefs[pc.comp].row(y))
                    .collect()
            })
            .collect();
        for mx in 0..cols {
            let m = r * cols + mx;
            if ri != 0 && m != 0 && m.is_multiple_of(ri) {
                let pos = match restarts.next() {
                    Some(&b) => b * 8,
                    None => cx.bits.pos().div_ceil(8) * 8,
                };
                cx.bits = Bits::new(&scan.stream.data, pos);
                cx.eobrun = 0;
                cx.preds = [0; MAX_COMPONENTS];
            }
            for (i, pc) in comps.iter().enumerate() {
                let (h, v) = hv[i];
                for row in guards[i][..v].iter_mut() {
                    let Row { coefs, nonzero } = &mut **row;
                    for bx in mx * h..(mx + 1) * h {
                        let blk: &mut [i16; 64] =
                            (&mut coefs[bx * 64..bx * 64 + 64]).try_into().unwrap();
                        let nz = &mut nonzero[bx];
                        match (band.ss == 0, band.ah == 0) {
                            (true, true) => dc_first(&mut cx, pc, i, band, blk)?,
                            (true, false) => dc_refine(&mut cx, band, blk),
                            (false, true) => ac_first(&mut cx, pc, band, blk, nz)?,
                            (false, false) => ac_refine(&mut cx, pc, band, blk, nz)?,
                        }
                    }
                }
            }
        }
        drop(guards);
        if let Some((p, s)) = pipe {
            p.publish(s, &extent);
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
    let s = pc.dc.as_deref().unwrap().decode(&mut cx.bits)?;
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
fn ac_first(
    cx: &mut Ctx,
    pc: &PComp,
    band: Band,
    blk: &mut [i16; 64],
    nz: &mut u64,
) -> Result<(), Error> {
    if cx.eobrun > 0 {
        cx.eobrun -= 1;
        return Ok(());
    }
    let ac = pc.ac.as_deref().unwrap();
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
            *nz |= 1 << k;
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

/// Bits `from..to` (zigzag indices, `to` ≤ 64).
#[inline(always)]
fn span(from: usize, to: usize) -> u64 {
    if from >= to {
        return 0;
    }
    let below_to = if to >= 64 { !0 } else { (1u64 << to) - 1 };
    below_to & (!0u64 << from)
}

/// Appends a correction bit to every nonzero coefficient in `from..to`, in
/// zigzag order.
#[inline(always)]
fn correct(bits: &mut Bits, blk: &mut [i16; 64], nz: u64, from: usize, to: usize, p1: i16) {
    let mut todo = nz & span(from, to);
    let mut left = todo.count_ones();
    while left > 0 {
        // Up to 16 correction bits per read, MSB first.
        let take = left.min(16);
        let mut word = bits.get(take) << (32 - take);
        for _ in 0..take {
            let c = &mut blk[ZIGZAG[todo.trailing_zeros() as usize]];
            // A set bit raises the magnitude unless that bit is already set.
            if (word as i32) < 0 && *c & p1 == 0 {
                *c = c.wrapping_add(if *c >= 0 { p1 } else { p1.wrapping_neg() });
            }
            word <<= 1;
            todo &= todo - 1;
        }
        left -= take;
    }
}

#[inline(always)]
fn ac_refine(
    cx: &mut Ctx,
    pc: &PComp,
    band: Band,
    blk: &mut [i16; 64],
    nz: &mut u64,
) -> Result<(), Error> {
    let p1 = (1i32 << band.al) as i16;
    let m1 = (-1i32 << band.al) as i16;
    let end = band.se + 1;
    let mut k = band.ss;
    if cx.eobrun == 0 {
        let ac = pc.ac.as_deref().unwrap();
        while k < end {
            let rs = ac.decode(&mut cx.bits)?;
            let (r, s) = (rs >> 4, rs & 15);
            let mut newval = 0i16;
            if s != 0 {
                // s must be 1 (libjpeg warns otherwise and goes on).
                newval = if cx.bits.get(1) != 0 { p1 } else { m1 };
            } else if r != 15 {
                cx.eobrun = 1 << r;
                if r != 0 {
                    cx.eobrun += cx.bits.get(r);
                }
                break;
            }
            // The target is the (r+1)-th still-zero coefficient from k on;
            // the nonzero ones before it get correction bits.
            let mut zeros = !*nz & span(k, end);
            for _ in 0..r {
                zeros &= zeros.wrapping_sub(1);
            }
            let target = if zeros == 0 {
                end
            } else {
                zeros.trailing_zeros() as usize
            };
            correct(&mut cx.bits, blk, *nz, k, target, p1);
            k = target;
            if newval != 0 && k <= 63 {
                blk[ZIGZAG[k]] = newval;
                *nz |= 1 << k;
            }
            k += 1;
        }
    }
    if cx.eobrun > 0 {
        // The rest of the band only gets correction bits.
        correct(&mut cx.bits, blk, *nz, k, end, p1);
        cx.eobrun -= 1;
    }
    Ok(())
}

/// Dequantizes and transforms all blocks into the planes, in parallel
/// bands of MCU rows; returns the threads used.
fn output(
    frame: &Frame,
    coefs: &[Coefs],
    quants: &[[i32; 64]],
    planes: &mut [&mut [u8]],
    threads: usize,
) -> usize {
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

    let work: Vec<Mutex<Option<Vec<&mut [u8]>>>> =
        per_band.into_iter().map(|p| Mutex::new(Some(p))).collect();
    let next = AtomicUsize::new(0);
    let run = || {
        crate::parallel::thread_init();
        let mut block = [0i32; 64];
        loop {
            let b = next.fetch_add(1, Ordering::Relaxed);
            let Some(slot) = work.get(b) else { break };
            let mut outs = slot.lock().unwrap().take().unwrap();
            let (r0, r1) = bands[b];
            for (c, out) in outs.iter_mut().enumerate() {
                let v = geo.comps[c].1 as usize;
                let stride = frame.layouts[c].stride;
                let (cf, q) = (&coefs[c], &quants[c]);
                for by in r0 * v..r1 * v {
                    // Take the row: it is freed here, in parallel, and a row
                    // no scan touched stays empty (all zero).
                    let src_row = std::mem::take(&mut cf.rows[by].lock().unwrap().coefs);
                    if src_row.is_empty() {
                        let fill = idct::dc_value(0);
                        for y in 0..bs {
                            let at = ((by - r0 * v) * bs + y) * stride;
                            out[at..at + cf.blocks_x * bs].fill(fill);
                        }
                        continue;
                    }
                    for (bx, src) in src_row.as_chunks::<64>().0.iter().enumerate() {
                        let mut any_ac = false;
                        for k in 0..64 {
                            let z = ZIGZAG[k];
                            if src[z] != 0 {
                                block[z] = (src[z] as i32).wrapping_mul(q[k]);
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
    threads
}
