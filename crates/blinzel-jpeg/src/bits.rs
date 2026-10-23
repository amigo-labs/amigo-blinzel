//! Entropy-coded data: byte-stuffing removal and bit reading.
//!
//! The scan is unstuffed once up front, so every bit position is a plain
//! number (what the parallel decoder synchronizes on) and the bit reader
//! never looks for markers in its hot path.

/// Zero bytes after the data, so reads near the end need no bounds checks.
const PAD: usize = 16;

/// Entropy-coded data of one scan with byte stuffing and restart markers
/// removed.
pub(crate) struct Stream {
    /// Unstuffed bytes followed by `PAD` zero bytes.
    pub data: Vec<u8>,
    /// Unstuffed byte offsets where restart intervals 1, 2, … begin.
    pub restarts: Vec<usize>,
    /// File offset of the marker that ended the scan (or the file length).
    pub end: usize,
}

/// Unstuffs the entropy-coded data starting at `start` up to the next
/// marker that is neither a restart marker nor a fill byte.
pub(crate) fn unstuff(file: &[u8], start: usize) -> Stream {
    let start = start.min(file.len());
    let src = &file[start..];
    let mut data = Vec::with_capacity(src.len() + PAD);
    let mut restarts = Vec::new();
    let mut i = 0;
    let end = loop {
        let Some(n) = find_ff(&src[i..]) else {
            data.extend_from_slice(&src[i..]);
            break file.len();
        };
        data.extend_from_slice(&src[i..i + n]);
        i += n;
        match src.get(i + 1) {
            Some(0x00) => {
                data.push(0xFF);
                i += 2;
            }
            // Fill byte before a marker.
            Some(0xFF) => i += 1,
            Some(0xD0..=0xD7) => {
                restarts.push(data.len());
                i += 2;
            }
            Some(_) => break start + i,
            None => break file.len(),
        }
    };
    data.resize(data.len() + PAD, 0);
    Stream {
        data,
        restarts,
        end,
    }
}

/// Index of the first 0xFF byte, eight bytes at a time.
fn find_ff(s: &[u8]) -> Option<usize> {
    let (words, tail) = s.as_chunks::<8>();
    for (i, w) in words.iter().enumerate() {
        // A byte is 0xFF exactly when its complement is zero; the lowest
        // flagged byte of the classic zero-byte test is always exact.
        let x = !u64::from_le_bytes(*w);
        let z = x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080;
        if z != 0 {
            return Some(i * 8 + (z.trailing_zeros() / 8) as usize);
        }
    }
    let base = words.len() * 8;
    tail.iter().position(|&b| b == 0xFF).map(|p| base + p)
}

/// MSB-first bit reader over unstuffed data; reads past the end yield
/// zeros, as in libjpeg.
#[derive(Clone, Copy)]
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Position in bits.
    pub pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Bits { data, pos }
    }

    /// The next 57 or more bits, left-aligned.
    #[inline(always)]
    pub fn peek(&self) -> u64 {
        let i = self.pos >> 3;
        match self.data.get(i..i + 8) {
            Some(b) => u64::from_be_bytes(b.try_into().unwrap()) << (self.pos & 7),
            None => self.peek_tail(),
        }
    }

    #[cold]
    fn peek_tail(&self) -> u64 {
        let i = self.pos >> 3;
        let mut b = [0u8; 8];
        if let Some(rest) = self.data.get(i..) {
            b[..rest.len()].copy_from_slice(rest);
        }
        u64::from_be_bytes(b) << (self.pos & 7)
    }

    #[inline(always)]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    /// Reads `n` bits (0 ≤ n ≤ 16).
    #[inline(always)]
    pub fn get(&mut self, n: u32) -> u32 {
        // Two shifts so that n = 0 yields 0.
        let v = ((self.peek() >> 1) >> (63 - n)) as u32;
        self.pos += n as usize;
        v
    }
}

/// Sign-extends the `s` extra bits `v` of a coefficient (JPEG F.2.2.1).
#[inline(always)]
pub(crate) fn extend(v: u32, s: u32) -> i32 {
    let v = v as i32;
    // Top bit clear: v - (2^s - 1).
    v - (((v >> (s - 1)) - 1) & ((1 << s) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstuffs_and_finds_restarts() {
        let file = [
            0xAA, 0xFF, 0x00, 0x12, 0xFF, 0xD0, 0x34, 0xFF, 0xFF, 0xD1, 0x56, 0xFF, 0xD9, 0x99,
        ];
        let s = unstuff(&file, 0);
        assert_eq!(&s.data[..5], &[0xAA, 0xFF, 0x12, 0x34, 0x56]);
        assert!(s.data[5..].iter().all(|&b| b == 0));
        assert_eq!(s.restarts, [3, 4]);
        assert_eq!(s.end, 11);
        // No terminating marker: everything is data.
        let s = unstuff(&[1, 2, 3], 0);
        assert_eq!(&s.data[..3], &[1, 2, 3]);
        assert_eq!(s.end, 3);
    }

    #[test]
    fn finds_ff_in_every_lane() {
        for len in 1..40 {
            for at in 0..len {
                let mut v = vec![0x7Fu8; len];
                v[at] = 0xFF;
                if at + 1 < len {
                    v[at + 1] = 0xFF;
                }
                assert_eq!(find_ff(&v), Some(at), "len {len} at {at}");
            }
            assert_eq!(find_ff(&vec![0xFEu8; len]), None);
        }
    }

    #[test]
    fn reads_bits_msb_first() {
        let data = [0b1010_0000, 0xFF, 0x0F, 0, 0, 0, 0, 0, 0, 0];
        let mut b = Bits::new(&data, 0);
        assert_eq!(b.get(0), 0);
        assert_eq!(b.get(1), 1);
        assert_eq!(b.get(2), 0b01);
        assert_eq!(b.get(13), 0b0_0000_1111_1111);
        assert_eq!(b.pos, 16);
        assert_eq!(b.get(8), 0x0F);
        // Past the end: zeros.
        let mut b = Bits::new(&data[..3], 20);
        assert_eq!(b.get(4), 0xF);
        assert_eq!(b.get(16), 0);
    }

    #[test]
    fn extends_like_the_standard() {
        assert_eq!(extend(0, 1), -1);
        assert_eq!(extend(1, 1), 1);
        assert_eq!(extend(0b00, 2), -3);
        assert_eq!(extend(0b01, 2), -2);
        assert_eq!(extend(0b10, 2), 2);
        assert_eq!(extend(0, 11), -2047);
        assert_eq!(extend(2047, 11), 2047);
    }
}
