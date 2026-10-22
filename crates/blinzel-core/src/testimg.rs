//! Deterministic synthetic test images with photo-like statistics (smooth
//! gradients, texture noise, hard edges), so encoders produce realistic
//! file sizes and decoders realistic work. Used by tests and the benchmark
//! corpus.

/// xorshift64* — tiny, fast, deterministic.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Tightly packed RGB, `w × h × 3` bytes.
pub fn rgb(w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 3];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let fw = w.max(1) as f32;
    let fh = h.max(1) as f32;
    for y in 0..h {
        let fy = y as f32 / fh;
        let row = &mut out[y * w * 3..(y + 1) * w * 3];
        for x in 0..w {
            let fx = x as f32 / fw;
            let n = (rng.next() >> 58) as f32 - 32.0; // ±32 grain
            // Soft "landscape": sky gradient above a horizon, warm ground
            // below, a few hard-edged blocks for high frequencies.
            let horizon = 0.55 + 0.08 * (fx * 9.0).sin();
            let (mut r, mut g, mut b) = if fy < horizon {
                (90.0 + 80.0 * fy, 140.0 + 60.0 * fy, 230.0 - 40.0 * fy)
            } else {
                (
                    140.0 + 60.0 * fx,
                    110.0 + 40.0 * (fx * 20.0).sin(),
                    60.0 + 30.0 * fy,
                )
            };
            if ((x / 97) + (y / 61)) % 7 == 0 {
                r = 240.0 - r * 0.5;
                g = 30.0 + g * 0.3;
                b = 20.0;
            }
            let c = |v: f32| (v + n * 0.35).clamp(0.0, 255.0) as u8;
            r = r.max(0.0);
            g = g.max(0.0);
            b = b.max(0.0);
            row[x * 3] = c(r);
            row[x * 3 + 1] = c(g);
            row[x * 3 + 2] = c(b);
        }
    }
    out
}

/// One byte per pixel.
pub fn gray(w: u32, h: u32) -> Vec<u8> {
    rgb(w, h)
        .chunks_exact(3)
        .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
        .collect()
}

/// Tightly packed RGBA with a soft alpha ramp.
pub fn rgba(w: u32, h: u32) -> Vec<u8> {
    let rgb = rgb(w, h);
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for (i, p) in rgb.chunks_exact(3).enumerate() {
        let x = i % w.max(1) as usize;
        let a = (x * 255 / w.max(1) as usize) as u8;
        out.extend_from_slice(&[p[0], p[1], p[2], a]);
    }
    out
}
