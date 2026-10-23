//! One scan's MCU decoding, starting from any entropy-decoder state: the
//! building block of the sequential and the parallel decoder.

use crate::bits::{Bits, Stream, extend};
use crate::huffman::Huffman;
use crate::{Error, MAX_COMPONENTS, idct};

/// Zigzag index → natural (row-major) index.
pub(crate) const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// One component of a scan, resolved against the frame and tables.
pub(crate) struct ScanComp<'t> {
    /// Output plane (frame component index).
    pub plane: usize,
    pub dc: &'t Huffman,
    pub ac: &'t Huffman,
    /// Quantization table in zigzag order.
    pub quant: &'t [i32; 64],
    /// Blocks per MCU (1 × 1 in a non-interleaved scan).
    pub h: usize,
    pub v: usize,
    /// Bytes per output row.
    pub stride: usize,
}

/// Entropy-decoder state at an MCU boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct State {
    /// Bit position in the unstuffed stream.
    pub pos: usize,
    /// DC predictors, indexed like the scan's components.
    pub preds: [i32; MAX_COMPONENTS],
}

pub(crate) struct Scan<'t> {
    pub comps: Vec<ScanComp<'t>>,
    /// MCU grid.
    pub cols: u32,
    pub rows: u32,
    /// Output block size: the DCT scale numerator.
    pub bs: usize,
    pub restart_interval: u32,
    pub stream: Stream,
}

impl Scan<'_> {
    pub fn mcus(&self) -> u32 {
        self.cols * self.rows
    }

    /// Output rows of component `i` per MCU row.
    pub fn mcu_height(&self, i: usize) -> usize {
        self.comps[i].v * self.bs
    }

    /// Whether a restart interval ends before MCU `m`.
    #[inline(always)]
    fn is_restart(&self, m: u32) -> bool {
        let ri = self.restart_interval;
        ri != 0 && m != 0 && m.is_multiple_of(ri)
    }

    /// Before MCU `m`: jumps to the restart marker and resets the DC
    /// predictors when an interval ends there.
    #[inline(always)]
    fn restart(&self, m: u32, st: &mut State) {
        if self.is_restart(m) {
            // Without the marker (corrupt data), resynchronize at a byte.
            st.pos = match self
                .stream
                .restarts
                .get((m / self.restart_interval) as usize - 1)
            {
                Some(&b) => b * 8,
                None => st.pos.div_ceil(8) * 8,
            };
            st.preds = [0; MAX_COMPONENTS];
        }
    }

    /// Decodes MCU `m` without producing output (DC and positions only).
    #[inline(always)]
    pub fn skip_mcu(&self, m: u32, st: &mut State) -> Result<(), Error> {
        self.restart(m, st);
        let mut bits = Bits::new(&self.stream.data, st.pos);
        let mut block = [0i32; 64];
        for (i, sc) in self.comps.iter().enumerate() {
            for _ in 0..sc.h * sc.v {
                decode_block::<false>(sc, &mut bits, &mut st.preds[i], &mut block)?;
            }
        }
        st.pos = bits.pos();
        Ok(())
    }

    /// Decodes MCUs `m0..m1` into `planes` (indexed like the scan's
    /// components), whose first rows belong to MCU row `row0`.
    pub fn decode(
        &self,
        m0: u32,
        m1: u32,
        st: &mut State,
        planes: &mut [&mut [u8]],
        row0: u32,
    ) -> Result<(), Error> {
        let mut block = [0i32; 64];
        let bs = self.bs;
        let mut bits = Bits::new(&self.stream.data, st.pos);
        for m in m0..m1 {
            if m == m0 || self.is_restart(m) {
                st.pos = bits.pos();
                self.restart(m, st);
                bits = Bits::new(&self.stream.data, st.pos);
            }
            let mx = (m % self.cols) as usize;
            let my = (m / self.cols - row0) as usize;
            for (i, sc) in self.comps.iter().enumerate() {
                let out = &mut *planes[i];
                for by in 0..sc.v {
                    for bx in 0..sc.h {
                        let (x, y) = ((mx * sc.h + bx) * bs, (my * sc.v + by) * bs);
                        let dst = &mut out[y * sc.stride + x..];
                        block_out(sc, &mut bits, &mut st.preds[i], &mut block, bs, dst)?;
                    }
                }
            }
        }
        st.pos = bits.pos();
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
) -> Result<(), Error> {
    if bs == 1 {
        // Only DC matters at 1/8; AC codes are skipped, not stored.
        decode_block::<false>(sc, bits, pred, block)?;
        out[0] = idct::dc_value(block[0]);
        return Ok(());
    }
    let any_ac = decode_block::<true>(sc, bits, pred, block)?;
    write_block(block, any_ac, bs, out, sc.stride);
    Ok(())
}

/// IDCT of a dequantized block (natural order) into `bs × bs` output;
/// leaves `block` all zero.
#[inline(always)]
pub(crate) fn write_block(
    block: &mut [i32; 64],
    any_ac: bool,
    bs: usize,
    out: &mut [u8],
    stride: usize,
) {
    if !any_ac {
        idct::fill(out, stride, bs, idct::dc_value(block[0]));
        block[0] = 0;
        return;
    }
    match bs {
        8 => idct::islow(block, out, stride),
        4 => idct::idct4x4(block, out, stride),
        2 => idct::idct2x2(block, out, stride),
        _ => out[0] = idct::dc_value(block[0]),
    }
    *block = [0; 64];
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
}
