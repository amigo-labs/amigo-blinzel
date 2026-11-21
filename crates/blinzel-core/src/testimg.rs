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
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
        .collect()
}

/// Tightly packed RGBA with a soft alpha ramp.
pub fn rgba(w: u32, h: u32) -> Vec<u8> {
    let rgb = rgb(w, h);
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for (i, p) in rgb.as_chunks::<3>().0.iter().enumerate() {
        let x = i % w.max(1) as usize;
        let a = (x * 255 / w.max(1) as usize) as u8;
        out.extend_from_slice(&[p[0], p[1], p[2], a]);
    }
    out
}

/// A value of an EXIF entry for [`exif_block`].
#[derive(Clone, Debug)]
pub enum ExifValue {
    /// ASCII (type 2): the bytes as given, NUL-terminated by the builder.
    Ascii(Vec<u8>),
    /// SHORT (type 3).
    Short(u16),
    /// RATIONAL (type 5): numerator and denominator.
    Rational(u32, u32),
}

/// A little-endian EXIF block (TIFF) with `ifd0` entries and, if any,
/// `exif` entries in an Exif IFD, for tests of metadata readers.
pub fn exif_block(ifd0: &[(u16, ExifValue)], exif: &[(u16, ExifValue)]) -> Vec<u8> {
    let mut ifd0: Vec<(u16, ExifValue)> = ifd0.to_vec();
    // The Exif IFD pointer is patched in once its offset is known.
    let with_exif = !exif.is_empty();
    if with_exif {
        ifd0.push((0x8769, ExifValue::Short(0)));
    }
    ifd0.sort_by_key(|e| e.0);
    let ifd_len = |n: usize| 2 + 12 * n + 4;
    let ifd0_at = 8usize;
    let exif_at = ifd0_at + ifd_len(ifd0.len());
    let data_at = exif_at + if with_exif { ifd_len(exif.len()) } else { 0 };
    let mut out = b"II*\0".to_vec();
    out.extend((ifd0_at as u32).to_le_bytes());
    let mut data: Vec<u8> = Vec::new();
    let mut write_ifd = |out: &mut Vec<u8>, entries: &[(u16, ExifValue)]| {
        out.extend((entries.len() as u16).to_le_bytes());
        for (tag, v) in entries {
            out.extend(tag.to_le_bytes());
            let (typ, count, bytes): (u16, usize, Vec<u8>) = match v {
                ExifValue::Ascii(s) => {
                    let mut b = s.clone();
                    b.push(0);
                    (2, b.len(), b)
                }
                ExifValue::Short(_) if *tag == 0x8769 => {
                    (4, 1, (exif_at as u32).to_le_bytes().to_vec())
                }
                ExifValue::Short(n) => (3, 1, n.to_le_bytes().to_vec()),
                ExifValue::Rational(n, d) => {
                    let mut b = n.to_le_bytes().to_vec();
                    b.extend(d.to_le_bytes());
                    (5, 1, b)
                }
            };
            out.extend(typ.to_le_bytes());
            out.extend((count as u32).to_le_bytes());
            if bytes.len() <= 4 {
                let mut v = bytes.clone();
                v.resize(4, 0);
                out.extend(v);
            } else {
                out.extend(((data_at + data.len()) as u32).to_le_bytes());
                data.extend(&bytes);
            }
        }
        out.extend(0u32.to_le_bytes());
    };
    write_ifd(&mut out, &ifd0);
    if with_exif {
        let mut exif = exif.to_vec();
        exif.sort_by_key(|e| e.0);
        write_ifd(&mut out, &exif);
    }
    out.extend(data);
    out
}

/// `jpeg` with `tiff` as its EXIF segment (APP1) right after SOI.
pub fn jpeg_with_exif(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
    let len = (2 + 6 + tiff.len()) as u16;
    let mut out = jpeg[..2].to_vec();
    out.extend([0xFF, 0xE1]);
    out.extend(len.to_be_bytes());
    out.extend(b"Exif\0\0");
    out.extend(tiff);
    out.extend(&jpeg[2..]);
    out
}
