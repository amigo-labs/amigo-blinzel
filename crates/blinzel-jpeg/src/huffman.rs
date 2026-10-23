//! Huffman tables (JPEG Annex C) with a 10-bit lookup and, for AC tables,
//! a combined lookup that also decodes the coefficient's extra bits.

use crate::Error;
use crate::bits::{Bits, extend};

pub(crate) const FAST_BITS: u32 = 10;
const FAST_SIZE: usize = 1 << FAST_BITS;

pub(crate) struct Huffman {
    /// Code prefix → `length << 8 | symbol`; 0 when the code is longer.
    fast: [u16; FAST_SIZE],
    /// Code prefix → `value << 16 | run << 8 | code and extra bits`; 0 when
    /// not covered (EOB, ZRL, longer codes).
    ac: [i32; FAST_SIZE],
    /// Exclusive upper bound of the codes of each length, left-aligned to 16
    /// bits; `maxcode[17]` is a sentinel.
    maxcode: [u32; 18],
    /// Symbol index minus code for the first code of each length.
    delta: [i32; 17],
    symbols: [u8; 256],
}

impl Huffman {
    /// Builds a table from the 16 code-length counts and the symbols of a
    /// DHT segment.
    pub fn new(counts: &[u8; 16], symbols: &[u8]) -> Result<Box<Huffman>, Error> {
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if total > 256 || symbols.len() < total {
            return Err(Error::Corrupt("Huffman table"));
        }
        let mut h = Box::new(Huffman {
            fast: [0; FAST_SIZE],
            ac: [0; FAST_SIZE],
            maxcode: [0; 18],
            delta: [0; 17],
            symbols: [0; 256],
        });
        h.symbols[..total].copy_from_slice(&symbols[..total]);
        let mut code = 0u32;
        let mut k = 0usize;
        for len in 1..=16u32 {
            h.delta[len as usize] = k as i32 - code as i32;
            for _ in 0..counts[len as usize - 1] {
                if len <= FAST_BITS {
                    let shift = FAST_BITS - len;
                    let base = (code << shift) as usize;
                    h.fast[base..base + (1 << shift)].fill((len << 8) as u16 | h.symbols[k] as u16);
                }
                code += 1;
                k += 1;
            }
            // No code may be all ones (libjpeg's check).
            if code >= 1 << len {
                return Err(Error::Corrupt("Huffman table"));
            }
            h.maxcode[len as usize] = code << (16 - len);
            code <<= 1;
        }
        h.maxcode[17] = u32::MAX;

        for (i, &e) in h.fast.iter().enumerate() {
            if e == 0 {
                continue;
            }
            let (len, rs) = (e as u32 >> 8, e as u32 & 0xFF);
            let (run, s) = (rs >> 4, rs & 15);
            if s != 0 && len + s <= FAST_BITS {
                let extra = (i as u32 >> (FAST_BITS - len - s)) & ((1 << s) - 1);
                h.ac[i] = (extend(extra, s) << 16) | (run << 8) as i32 | (len + s) as i32;
            }
        }
        Ok(h)
    }

    /// Decodes one symbol.
    #[inline(always)]
    pub fn decode(&self, bits: &mut Bits) -> Result<u32, Error> {
        let p = bits.peek();
        let e = self.fast[(p >> (64 - FAST_BITS)) as usize];
        if e != 0 {
            bits.skip(e as u32 >> 8);
            return Ok(e as u32 & 0xFF);
        }
        self.decode_slow(bits, p)
    }

    #[cold]
    fn decode_slow(&self, bits: &mut Bits, p: u64) -> Result<u32, Error> {
        let c16 = (p >> 48) as u32;
        let mut len = FAST_BITS as usize + 1;
        while c16 >= self.maxcode[len] {
            len += 1;
        }
        if len > 16 {
            return Err(Error::Corrupt("invalid Huffman code"));
        }
        let idx = (c16 >> (16 - len)) as i32 + self.delta[len];
        bits.skip(len as u32);
        Ok(self.symbols[(idx as usize) & 0xFF] as u32)
    }

    /// The combined AC lookup for the bits at the reader's position.
    #[inline(always)]
    pub fn ac_fast(&self, peeked: u64) -> i32 {
        self.ac[(peeked >> (64 - FAST_BITS)) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard luminance AC table (JPEG K.3.3.2) exercises short,
    /// long and 16-bit codes.
    fn std_ac_luma() -> Box<Huffman> {
        let counts = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
        let symbols: Vec<u8> = (0..=255u8)
            .filter(|s| s & 15 != 0 || *s == 0 || *s == 0xF0)
            .take(162)
            .collect();
        Huffman::new(&counts, &symbols).unwrap()
    }

    fn encode(codes: &[(u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut acc, mut n) = (0u64, 0u32);
        for &(code, len) in codes {
            acc = (acc << len) | code as u64;
            n += len;
            while n >= 8 {
                out.push((acc >> (n - 8)) as u8);
                n -= 8;
            }
        }
        if n > 0 {
            out.push((acc << (8 - n)) as u8);
        }
        out.resize(out.len() + 16, 0);
        out
    }

    #[test]
    fn decodes_short_and_long_codes() {
        // Lengths 1, 2, 3, 16: codes 0, 10, 110, 1110000000000000.
        let mut counts = [0u8; 16];
        counts[0] = 1;
        counts[1] = 1;
        counts[2] = 1;
        counts[15] = 1;
        let h = Huffman::new(&counts, &[7, 8, 9, 10]).unwrap();
        let data = encode(&[
            (0, 1),
            (0b10, 2),
            (0b110, 3),
            (0b1110_0000_0000_0000, 16),
            (0, 1),
        ]);
        let mut b = Bits::new(&data, 0);
        for want in [7, 8, 9, 10, 7] {
            assert_eq!(h.decode(&mut b).unwrap(), want);
        }
        assert_eq!(b.pos(), 23);
        // An unassigned 16-bit prefix is an error.
        let bad = encode(&[(0xFFFF, 16)]);
        assert!(h.decode(&mut Bits::new(&bad, 0)).is_err());
    }

    #[test]
    fn rejects_overfull_tables() {
        let mut counts = [0u8; 16];
        counts[0] = 2; // two 1-bit codes use "1", which is all ones
        assert!(Huffman::new(&counts, &[1, 2]).is_err());
        counts[0] = 0;
        counts[1] = 3;
        assert!(Huffman::new(&counts, &[1, 2, 3]).is_ok());
        assert!(Huffman::new(&counts, &[1, 2]).is_err());
    }

    #[test]
    fn combined_ac_lookup_matches_slow_path() {
        let h = std_ac_luma();
        // Every 10-bit prefix: if the combined entry exists, it must agree
        // with symbol decoding plus extra bits.
        for prefix in 0..FAST_SIZE as u32 {
            let data = encode(&[(prefix, 10)]);
            let mut b = Bits::new(&data, 0);
            let f = h.ac_fast(b.peek());
            if f == 0 {
                continue;
            }
            let mut b2 = b;
            let rs = h.decode(&mut b2).unwrap();
            let s = rs & 15;
            let v = extend(b2.get(s), s);
            assert_eq!(f >> 16, v);
            assert_eq!((f >> 8) & 0xFF, (rs >> 4) as i32);
            assert_eq!((f & 0xFF) as usize, b2.pos());
        }
    }
}
