//! Parallel decoding of one scan (milestone M3, stages 2 and 3).
//!
//! 1. **Pass A** finds the decoder state at every MCU start. With restart
//!    markers each thread starts exactly at a marker. Without them each
//!    thread starts at a byte offset with guessed state (MCU start, DC
//!    predictors 0) and relies on the self-synchronization of Huffman
//!    codes (Weißenberger & Schmidt).
//! 2. **Synchronization** (no markers): each thread decodes past its end
//!    until it reaches an MCU start its successor recorded. From there both
//!    decode identically, so the successor's records are valid from that
//!    point; prefix sums give absolute MCU numbers and DC offsets.
//! 3. **Pass C**: threads decode bands of MCU rows from the known start
//!    states, writing straight into their own rows of the planes.
//!
//! With dense restart markers pass A is skipped: a band starts at the last
//! marker before it. Anything inconsistent (truncated or corrupt data,
//! no synchronization) falls back to the sequential decoder.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::scan::{Scan, State};
use crate::{Error, MAX_COMPONENTS};

/// Minimum entropy-coded bytes per thread; below that threads cost more
/// than they save.
const MIN_CHUNK_BYTES: usize = 64 * 1024;

/// Bands per thread in pass C, for load balancing.
const BANDS_PER_THREAD: usize = 4;

/// Decodes `scan` into `planes` (indexed like the scan's components) with
/// up to `threads` threads; returns the number of threads used.
pub(crate) fn decode_scan(
    scan: &Scan,
    planes: &mut [&mut [u8]],
    threads: usize,
) -> Result<usize, Error> {
    let data_len = scan.stream.data.len();
    let threads = threads
        .min(data_len / MIN_CHUNK_BYTES)
        .min(scan.rows as usize)
        .max(1);
    if threads > 1 {
        let bands = bands(scan.rows, threads * BANDS_PER_THREAD);
        if let Some(starts) = band_starts(scan, &bands, threads) {
            decode_bands(scan, planes, &bands, &starts, threads)?;
            return Ok(threads);
        }
    }
    let mut st = State::default();
    scan.decode(0, scan.mcus(), &mut st, planes, 0)?;
    Ok(1)
}

/// Splits `rows` MCU rows into at most `n` bands of nearly equal height.
fn bands(rows: u32, n: usize) -> Vec<(u32, u32)> {
    let n = (n as u32).min(rows).max(1);
    (0..n).map(|i| (rows * i / n, rows * (i + 1) / n)).collect()
}

/// Where to start decoding for a band: decode (without output) from MCU
/// `from` in state `st`, then produce output from the band's first MCU.
#[derive(Clone, Copy, Debug)]
struct Start {
    from: u32,
    st: State,
}

fn band_starts(scan: &Scan, bands: &[(u32, u32)], threads: usize) -> Option<Vec<Start>> {
    let ri = scan.restart_interval;
    if ri != 0 {
        let segments = scan.mcus().div_ceil(ri) as usize;
        if scan.stream.restarts.len() + 1 < segments {
            return None;
        }
        let band_mcus = scan.mcus() as usize / bands.len();
        if (ri as usize) * 8 <= band_mcus {
            // Dense markers: start each band at the last marker before it.
            return Some(
                bands
                    .iter()
                    .map(|&(r0, _)| {
                        let k = r0 * scan.cols / ri;
                        let pos = if k == 0 {
                            0
                        } else {
                            scan.stream.restarts[k as usize - 1] * 8
                        };
                        Start {
                            from: k * ri,
                            st: State {
                                pos,
                                preds: [0; MAX_COMPONENTS],
                            },
                        }
                    })
                    .collect(),
            );
        }
    }
    let states = mcu_states(scan, threads)?;
    bands
        .iter()
        .map(|&(r0, _)| {
            let m = r0 * scan.cols;
            states.at(m).map(|st| Start { from: m, st })
        })
        .collect()
}

/// Pass A results for one chunk of the stream.
struct Chunk {
    /// Absolute index of the first record when known (restart markers).
    first: Option<u32>,
    /// Decoder state at each MCU start, in the chunk's own DC frame.
    recs: Vec<State>,
    /// State after the last recorded MCU; `None` if decoding failed.
    end: Option<State>,
}

/// Pass A for chunk `i` of `n`.
fn pass_a(scan: &Scan, i: usize, n: usize) -> Chunk {
    let ri = scan.restart_interval;
    let total = scan.mcus();
    let mut recs = Vec::new();
    if ri != 0 {
        // Exact: whole restart intervals.
        let segments = total.div_ceil(ri) as usize;
        let (k0, k1) = (segments * i / n, segments * (i + 1) / n);
        let (m0, m1) = (k0 as u32 * ri, (k1 as u32 * ri).min(total));
        let mut st = State {
            pos: if k0 == 0 {
                0
            } else {
                scan.stream.restarts[k0 - 1] * 8
            },
            preds: [0; MAX_COMPONENTS],
        };
        recs.reserve((m1 - m0) as usize);
        for m in m0..m1 {
            recs.push(st);
            if scan.skip_mcu(m, &mut st).is_err() {
                return Chunk {
                    first: Some(m0),
                    recs,
                    end: None,
                };
            }
        }
        return Chunk {
            first: Some(m0),
            recs,
            end: Some(st),
        };
    }
    // Speculative: start at a byte offset, stop at the next chunk's.
    let bytes = scan.stream.data.len();
    let (b0, b1) = (bytes * i / n, bytes * (i + 1) / n);
    let mut st = State {
        pos: b0 * 8,
        preds: [0; MAX_COMPONENTS],
    };
    recs.reserve(total as usize / n + 16);
    while st.pos < b1 * 8 {
        recs.push(st);
        let start = st.pos;
        if scan.skip_mcu(0, &mut st).is_err() {
            if i == 0 {
                // Chunk 0 starts in the true state: the data is corrupt.
                return Chunk {
                    first: Some(0),
                    recs,
                    end: None,
                };
            }
            // A guessed start that runs into an invalid code was not yet
            // synchronized: drop its records and guess again one bit on.
            recs.clear();
            st = State {
                pos: start + 1,
                preds: [0; MAX_COMPONENTS],
            };
        }
    }
    Chunk {
        first: (i == 0).then_some(0),
        recs,
        end: Some(st),
    }
}

/// Where decoding from the end of chunk `i` meets chunk `i + 1`.
struct Sync {
    /// Index into chunk `i + 1`'s records.
    j: usize,
    /// States of the MCUs decoded past chunk `i`'s end, before the meeting
    /// point, in chunk `i`'s DC frame.
    overflow: Vec<State>,
    /// DC predictors at the meeting point, in chunk `i`'s DC frame.
    preds: [i32; MAX_COMPONENTS],
}

fn sync(scan: &Scan, end: State, next: &[State]) -> Option<Sync> {
    let mut st = end;
    let mut overflow = Vec::new();
    let mut j = 0;
    loop {
        while j < next.len() && next[j].pos < st.pos {
            j += 1;
        }
        if j == next.len() {
            return None;
        }
        if next[j].pos == st.pos {
            return Some(Sync {
                j,
                overflow,
                preds: st.preds,
            });
        }
        overflow.push(st);
        scan.skip_mcu(0, &mut st).ok()?;
    }
}

/// Decoder state at every MCU start, assembled from the chunks.
struct McuStates {
    /// Per chunk: absolute index of its first valid record, index of that
    /// record, DC offset to the true predictors, and the states.
    parts: Vec<(u32, usize, [i32; MAX_COMPONENTS], Vec<State>)>,
}

impl McuStates {
    fn at(&self, m: u32) -> Option<State> {
        let p = self
            .parts
            .partition_point(|(a, ..)| *a <= m)
            .checked_sub(1)?;
        let (a, j, off, recs) = &self.parts[p];
        let st = recs.get(j + (m - a) as usize)?;
        let mut preds = st.preds;
        for (p, o) in preds.iter_mut().zip(off) {
            *p = p.wrapping_add(*o);
        }
        Some(State { pos: st.pos, preds })
    }
}

fn mcu_states(scan: &Scan, threads: usize) -> Option<McuStates> {
    let chunks: Vec<Chunk> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|i| s.spawn(move || pass_a(scan, i, threads)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    if scan.restart_interval != 0 {
        // Exact: every record is valid as is.
        let parts = chunks
            .into_iter()
            .map(|c| Some((c.first?, 0, [0; MAX_COMPONENTS], c.recs)))
            .collect::<Option<_>>()?;
        return Some(McuStates { parts });
    }

    // Synchronize neighbours in parallel.
    let syncs: Vec<Sync> = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .windows(2)
            .map(|w| {
                let (end, next) = (w[0].end, &w[1].recs);
                s.spawn(move || sync(scan, end?, next))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Option<_>>()
    })?;

    // Prefix sums over the chunks: absolute index of the first valid record
    // (a), its index (j) and the DC offset to the true predictors (off).
    let mut meta = Vec::with_capacity(chunks.len());
    let (mut a, mut j, mut off) = (0u32, 0usize, [0i32; MAX_COMPONENTS]);
    for (i, s) in syncs.iter().enumerate() {
        meta.push((a, j, off));
        let valid = chunks[i].recs.len().checked_sub(j)?;
        a = a.checked_add((valid + s.overflow.len()) as u32)?;
        let theirs = chunks[i + 1].recs.get(s.j)?.preds;
        off = std::array::from_fn(|c| s.preds[c].wrapping_add(off[c]).wrapping_sub(theirs[c]));
        j = s.j;
    }
    meta.push((a, j, off));

    let mut syncs = syncs.into_iter();
    let parts = chunks
        .into_iter()
        .zip(meta)
        .map(|(c, (a, j, off))| {
            let mut recs = c.recs;
            // Chunk i's states continue with what it decoded past its end.
            if let Some(s) = syncs.next() {
                recs.extend(s.overflow);
            }
            (a, j, off, recs)
        })
        .collect();
    Some(McuStates { parts })
}

fn decode_bands(
    scan: &Scan,
    planes: &mut [&mut [u8]],
    bands: &[(u32, u32)],
    starts: &[Start],
    threads: usize,
) -> Result<(), Error> {
    // Split every plane into the bands' rows.
    let mut per_band: Vec<Vec<&mut [u8]>> = (0..bands.len()).map(|_| Vec::new()).collect();
    for (i, plane) in planes.iter_mut().enumerate() {
        let row_bytes = scan.mcu_height(i) * scan.comps[i].stride;
        let mut rest: &mut [u8] = plane;
        for (b, &(r0, r1)) in bands.iter().enumerate() {
            let len = ((r1 - r0) as usize * row_bytes).min(rest.len());
            let (head, tail) = rest.split_at_mut(len);
            per_band[b].push(head);
            rest = tail;
        }
    }
    let work: Vec<Mutex<Option<Vec<&mut [u8]>>>> =
        per_band.into_iter().map(|p| Mutex::new(Some(p))).collect();
    let next = AtomicUsize::new(0);
    let result: Mutex<Result<(), Error>> = Mutex::new(Ok(()));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let b = next.fetch_add(1, Ordering::Relaxed);
                    let Some(slot) = work.get(b) else { break };
                    let mut planes = slot.lock().unwrap().take().unwrap();
                    let (r0, r1) = bands[b];
                    let Start { from, mut st } = starts[b];
                    let m0 = r0 * scan.cols;
                    let r = (from..m0)
                        .try_for_each(|m| scan.skip_mcu(m, &mut st))
                        .and_then(|()| scan.decode(m0, r1 * scan.cols, &mut st, &mut planes, r0));
                    if let Err(e) = r {
                        *result.lock().unwrap() = Err(e);
                        break;
                    }
                }
            });
        }
    });
    result.into_inner().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_cover_all_rows() {
        for rows in [1, 2, 7, 125] {
            for n in [1, 3, 32] {
                let b = bands(rows, n);
                assert_eq!(b[0].0, 0);
                assert_eq!(b.last().unwrap().1, rows);
                assert!(b.windows(2).all(|w| w[0].1 == w[1].0 && w[0].0 < w[0].1));
            }
        }
    }
}
