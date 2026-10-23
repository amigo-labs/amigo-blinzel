//! Inverse DCTs, ported from libjpeg-turbo so that the output is
//! byte-identical: `jpeg_idct_islow` (jidctint.c) for full size and
//! `jpeg_idct_4x4/2x2/1x1` (jidctred.c) for DCT scaling.
//!
//! Inputs are dequantized coefficients in natural order. Arithmetic wraps
//! like the C code on corrupt input (the crate is built without overflow
//! checks); valid data never comes close.

const CONST_BITS: u32 = 13;
const PASS1_BITS: u32 = 2;

const FIX_0_211164243: i32 = 1730;
const FIX_0_298631336: i32 = 2446;
const FIX_0_390180644: i32 = 3196;
const FIX_0_509795579: i32 = 4176;
const FIX_0_541196100: i32 = 4433;
const FIX_0_601344887: i32 = 4926;
const FIX_0_720959822: i32 = 5906;
const FIX_0_765366865: i32 = 6270;
const FIX_0_850430095: i32 = 6967;
const FIX_0_899976223: i32 = 7373;
const FIX_1_061594337: i32 = 8697;
const FIX_1_175875602: i32 = 9633;
const FIX_1_272758580: i32 = 10426;
const FIX_1_451774981: i32 = 11893;
const FIX_1_501321110: i32 = 12299;
const FIX_1_847759065: i32 = 15137;
const FIX_1_961570560: i32 = 16069;
const FIX_2_053119869: i32 = 16819;
const FIX_2_172734803: i32 = 17799;
const FIX_2_562915447: i32 = 20995;
const FIX_3_072711026: i32 = 25172;
const FIX_3_624509785: i32 = 29692;

#[inline(always)]
fn descale(x: i32, n: u32) -> i32 {
    (x + (1 << (n - 1))) >> n
}

/// libjpeg's IDCT range limit: the value is taken modulo 1024 as a signed
/// 10-bit number, then shifted by 128 and clamped.
#[inline(always)]
fn limit(x: i32) -> u8 {
    (((x << 22) >> 22) + 128).clamp(0, 255) as u8
}

/// Output of a block whose AC coefficients are all zero, at any size.
#[inline(always)]
pub(crate) fn dc_value(dc: i32) -> u8 {
    limit(descale(dc, 3))
}

/// Fills an `n × n` block with one value.
#[inline(always)]
pub(crate) fn fill(out: &mut [u8], stride: usize, n: usize, v: u8) {
    for r in 0..n {
        out[r * stride..r * stride + n].fill(v);
    }
}

/// `N` lanes of `i32` the compiler keeps in SIMD registers: one IDCT pass
/// runs on all columns (or rows) of a block at once. libjpeg's shortcuts
/// for all-zero columns and rows give exactly the full computation's
/// result, so dropping them keeps the output identical.
#[derive(Clone, Copy)]
struct V<const N: usize>([i32; N]);

impl<const N: usize> V<N> {
    #[inline(always)]
    fn zip(self, o: V<N>, f: impl Fn(i32, i32) -> i32) -> V<N> {
        V(std::array::from_fn(|i| f(self.0[i], o.0[i])))
    }

    #[inline(always)]
    fn mul(self, k: i32) -> V<N> {
        V(self.0.map(|v| v * k))
    }

    #[inline(always)]
    fn shl(self, n: u32) -> V<N> {
        V(self.0.map(|v| v << n))
    }

    #[inline(always)]
    fn descale(self, n: u32) -> V<N> {
        V(self.0.map(|v| descale(v, n)))
    }
}

impl<const N: usize> std::ops::Add for V<N> {
    type Output = V<N>;
    #[inline(always)]
    fn add(self, o: V<N>) -> V<N> {
        self.zip(o, |a, b| a + b)
    }
}

impl<const N: usize> std::ops::Sub for V<N> {
    type Output = V<N>;
    #[inline(always)]
    fn sub(self, o: V<N>) -> V<N> {
        self.zip(o, |a, b| a - b)
    }
}

/// The block's coefficient rows as vectors over its 8 columns.
#[inline(always)]
fn rows(c: &[i32; 64]) -> [V<8>; 8] {
    let r = c.as_chunks::<8>().0;
    std::array::from_fn(|i| V(r[i]))
}

/// `m[r]` holds row `r` across `C` columns; the result holds column `i`
/// across the `R` rows.
#[inline(always)]
fn transpose<const R: usize, const C: usize>(m: [V<C>; R]) -> [V<R>; C] {
    std::array::from_fn(|i| V(std::array::from_fn(|r| m[r].0[i])))
}

/// Writes pass-2 results: `cols[x]` holds output column `x` across rows.
#[inline(always)]
fn store<const N: usize>(cols: [V<N>; N], shift: u32, out: &mut [u8], stride: usize) {
    for r in 0..N {
        let dst = &mut out[r * stride..r * stride + N];
        for (x, d) in dst.iter_mut().enumerate() {
            *d = limit(descale(cols[x].0[r], shift));
        }
    }
}

/// Full 8×8 IDCT (`jpeg_idct_islow`).
pub(crate) fn islow(c: &[i32; 64], out: &mut [u8], stride: usize) {
    let ws = idct8(rows(c)).map(|v| v.descale(CONST_BITS - PASS1_BITS));
    let cols = idct8(transpose(ws));
    store(cols, CONST_BITS + PASS1_BITS + 3, out, stride);
}

/// One 1-D pass of the islow IDCT, before descaling.
#[inline(always)]
fn idct8<const N: usize>(x: [V<N>; 8]) -> [V<N>; 8] {
    // Even part.
    let (z2, z3) = (x[2], x[6]);
    let z1 = (z2 + z3).mul(FIX_0_541196100);
    let tmp2 = z1 + z3.mul(-FIX_1_847759065);
    let tmp3 = z1 + z2.mul(FIX_0_765366865);
    let tmp0 = (x[0] + x[4]).shl(CONST_BITS);
    let tmp1 = (x[0] - x[4]).shl(CONST_BITS);
    let tmp10 = tmp0 + tmp3;
    let tmp13 = tmp0 - tmp3;
    let tmp11 = tmp1 + tmp2;
    let tmp12 = tmp1 - tmp2;

    // Odd part.
    let (t0, t1, t2, t3) = (x[7], x[5], x[3], x[1]);
    let z1 = t0 + t3;
    let z2 = t1 + t2;
    let z3 = t0 + t2;
    let z4 = t1 + t3;
    let z5 = (z3 + z4).mul(FIX_1_175875602);
    let t0 = t0.mul(FIX_0_298631336);
    let t1 = t1.mul(FIX_2_053119869);
    let t2 = t2.mul(FIX_3_072711026);
    let t3 = t3.mul(FIX_1_501321110);
    let z1 = z1.mul(-FIX_0_899976223);
    let z2 = z2.mul(-FIX_2_562915447);
    let z3 = z3.mul(-FIX_1_961570560) + z5;
    let z4 = z4.mul(-FIX_0_390180644) + z5;
    let t0 = t0 + z1 + z3;
    let t1 = t1 + z2 + z4;
    let t2 = t2 + z2 + z3;
    let t3 = t3 + z1 + z4;

    [
        tmp10 + t3,
        tmp11 + t2,
        tmp12 + t1,
        tmp13 + t0,
        tmp13 - t0,
        tmp12 - t1,
        tmp11 - t2,
        tmp10 - t3,
    ]
}

/// 4×4 output from an 8×8 block (`jpeg_idct_4x4`).
pub(crate) fn idct4x4(c: &[i32; 64], out: &mut [u8], stride: usize) {
    let ws = half4(rows(c)).map(|v| v.descale(CONST_BITS - PASS1_BITS + 1));
    let cols = half4(transpose(ws));
    store(cols, CONST_BITS + PASS1_BITS + 3 + 1, out, stride);
}

#[inline(always)]
fn half4<const N: usize>(x: [V<N>; 8]) -> [V<N>; 4] {
    let tmp0 = x[0].shl(CONST_BITS + 1);
    let tmp2 = x[2].mul(FIX_1_847759065) + x[6].mul(-FIX_0_765366865);
    let tmp10 = tmp0 + tmp2;
    let tmp12 = tmp0 - tmp2;
    let (z1, z2, z3, z4) = (x[7], x[5], x[3], x[1]);
    let tmp0 = z1.mul(-FIX_0_211164243)
        + z2.mul(FIX_1_451774981)
        + z3.mul(-FIX_2_172734803)
        + z4.mul(FIX_1_061594337);
    let tmp2 = z1.mul(-FIX_0_509795579)
        + z2.mul(-FIX_0_601344887)
        + z3.mul(FIX_0_899976223)
        + z4.mul(FIX_2_562915447);
    [tmp10 + tmp2, tmp12 + tmp0, tmp12 - tmp0, tmp10 - tmp2]
}

/// 2×2 output from an 8×8 block (`jpeg_idct_2x2`).
pub(crate) fn idct2x2(c: &[i32; 64], out: &mut [u8], stride: usize) {
    let ws = half2(rows(c)).map(|v| v.descale(CONST_BITS - PASS1_BITS + 2));
    let cols = half2(transpose(ws));
    store(cols, CONST_BITS + PASS1_BITS + 3 + 2, out, stride);
}

#[inline(always)]
fn half2<const N: usize>(x: [V<N>; 8]) -> [V<N>; 2] {
    let tmp10 = x[0].shl(CONST_BITS + 2);
    let tmp0 = x[7].mul(-FIX_0_720959822)
        + x[5].mul(FIX_0_850430095)
        + x[3].mul(-FIX_1_272758580)
        + x[1].mul(FIX_3_624509785);
    [tmp10 + tmp0, tmp10 - tmp0]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Floating-point reference 8×8 IDCT. The reduced-size IDCTs have no
    /// simple closed form; the cross-check against libjpeg-turbo in
    /// blinzel-core covers them exactly.
    fn reference(c: &[i32; 64]) -> Vec<f64> {
        let pi = std::f64::consts::PI;
        let cf = |u: usize| if u == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
        let mut out = vec![0.0; 64];
        for y in 0..8 {
            for x in 0..8 {
                let mut s = 0.0;
                for v in 0..8 {
                    for u in 0..8 {
                        s += cf(u)
                            * cf(v)
                            * c[v * 8 + u] as f64
                            * ((2 * x + 1) as f64 * u as f64 * pi / 16.0).cos()
                            * ((2 * y + 1) as f64 * v as f64 * pi / 16.0).cos();
                    }
                }
                out[y * 8 + x] = (s / 4.0 + 128.0).clamp(0.0, 255.0);
            }
        }
        out
    }

    fn blocks() -> Vec<[i32; 64]> {
        let mut rng = 0x1234_5678u32;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng
        };
        (0..200)
            .map(|i| {
                let mut c = [0i32; 64];
                c[0] = (next() % 2048) as i32 - 1024;
                // Sparse, decaying AC like real data; some blocks DC-only.
                if i % 5 != 0 {
                    for (k, v) in c.iter_mut().enumerate().skip(1) {
                        if next() % 3 == 0 {
                            let amp = 400 / (1 + k as i32 / 4);
                            *v = (next() % (2 * amp as u32 + 1)) as i32 - amp;
                        }
                    }
                }
                c
            })
            .collect()
    }

    #[test]
    fn islow_tracks_the_float_reference() {
        for c in blocks() {
            let mut out = [0u8; 64];
            islow(&c, &mut out, 8);
            for (got, want) in out.iter().zip(reference(&c)) {
                assert!((*got as f64 - want).abs() <= 1.0, "{got} vs {want:.2}");
            }
            assert!(
                (dc_value(c[0]) as f64 - (c[0] as f64 / 8.0 + 128.0).clamp(0.0, 255.0)).abs()
                    <= 0.5
            );
        }
    }

    #[test]
    fn dc_only_blocks_are_flat() {
        let mut c = [0i32; 64];
        for dc in [-1024, -9, 0, 7, 1023] {
            c[0] = dc;
            for (n, f) in [
                (8, islow as fn(&[i32; 64], &mut [u8], usize)),
                (4, idct4x4),
                (2, idct2x2),
            ] {
                let mut out = vec![0u8; n * n];
                f(&c, &mut out, n);
                assert!(out.iter().all(|&v| v == dc_value(dc)), "{n} {dc}");
            }
        }
    }

    #[test]
    fn range_limit_wraps_like_libjpeg() {
        assert_eq!(limit(0), 128);
        assert_eq!(limit(127), 255);
        assert_eq!(limit(500), 255);
        assert_eq!(limit(-128), 0);
        assert_eq!(limit(-500), 0);
        // 600 wraps to -424.
        assert_eq!(limit(600), 0);
    }
}
