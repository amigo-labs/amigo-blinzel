//! Own BMP decoder: all common variants (1/2/4/8-bit palette, RLE4/RLE8,
//! 16/24/32-bit with and without bit fields, OS/2 core headers, top-down and
//! bottom-up) straight into BGRA.

use super::{DecodeError, Result, check_size};
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

const BI_RGB: u32 = 0;
const BI_RLE8: u32 = 1;
const BI_RLE4: u32 = 2;
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;

fn corrupt(msg: &str) -> DecodeError {
    DecodeError::Corrupt(format!("BMP: {msg}"))
}

fn u16_at(d: &[u8], o: usize) -> Result<u16> {
    d.get(o..o + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| corrupt("truncated"))
}

fn u32_at(d: &[u8], o: usize) -> Result<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| corrupt("truncated"))
}

/// Parsed file and info headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub top_down: bool,
    pub bpp: u16,
    pub compression: u32,
    /// R, G, B, A masks for 16/32-bit images.
    pub masks: [u32; 4],
    pub palette_offset: usize,
    pub palette_entry_size: usize,
    pub palette_len: usize,
    pub data_offset: usize,
}

impl Header {
    pub fn parse(d: &[u8]) -> Result<Header> {
        if d.get(..2) != Some(b"BM") {
            return Err(corrupt("missing BM signature"));
        }
        let data_offset = u32_at(d, 10)? as usize;
        let hsize = u32_at(d, 14)? as usize;
        let (width, raw_height, bpp, compression) = if hsize == 12 {
            (
                u16_at(d, 18)? as i32,
                u16_at(d, 20)? as i32,
                u16_at(d, 24)?,
                BI_RGB,
            )
        } else if hsize >= 40 {
            (
                u32_at(d, 18)? as i32,
                u32_at(d, 22)? as i32,
                u16_at(d, 28)?,
                u32_at(d, 30)?,
            )
        } else {
            return Err(corrupt("unknown header size"));
        };
        if width <= 0 || raw_height == 0 || raw_height == i32::MIN {
            return Err(corrupt("bad dimensions"));
        }
        let top_down = raw_height < 0;
        let height = raw_height.unsigned_abs();
        let width = width as u32;
        check_size(width as u64, height as u64)?;

        let mut masks = match bpp {
            16 => [0x7C00, 0x03E0, 0x001F, 0],
            32 => [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0],
            _ => [0; 4],
        };
        let mut after_header = 14 + hsize;
        if compression == BI_BITFIELDS || compression == BI_ALPHABITFIELDS {
            if hsize >= 52 {
                masks[0] = u32_at(d, 54)?;
                masks[1] = u32_at(d, 58)?;
                masks[2] = u32_at(d, 62)?;
                if hsize >= 56 {
                    masks[3] = u32_at(d, 66)?;
                }
            } else {
                // Masks follow a 40-byte header.
                let n = if compression == BI_ALPHABITFIELDS {
                    4
                } else {
                    3
                };
                for (i, m) in masks.iter_mut().take(n).enumerate() {
                    *m = u32_at(d, after_header + 4 * i)?;
                }
                after_header += 4 * n;
            }
        } else if hsize >= 56 && bpp == 32 {
            // V3+ headers may carry an alpha mask even for BI_RGB.
            masks[3] = u32_at(d, 66).unwrap_or(0);
        }
        let palette_entry_size = if hsize == 12 { 3 } else { 4 };
        let palette_len = if bpp <= 8 {
            let used = if hsize >= 40 {
                u32_at(d, 46)? as usize
            } else {
                0
            };
            let max = 1usize << bpp;
            if used == 0 || used > max { max } else { used }
        } else {
            0
        };
        Ok(Header {
            width,
            height,
            top_down,
            bpp,
            compression,
            masks,
            palette_offset: after_header,
            palette_entry_size,
            palette_len,
            data_offset,
        })
    }
}

pub fn decode(d: &[u8]) -> Result<Image> {
    let h = Header::parse(d)?;
    let (w, ht) = (h.width as usize, h.height as usize);
    let mut plane = Plane::new(h.width, h.height, 4);
    let palette = read_palette(d, &h)?;
    let data = d
        .get(h.data_offset..)
        .ok_or_else(|| corrupt("pixel data offset beyond file"))?;
    // Destination row for source row `r` (BMPs are bottom-up by default).
    let dst_row = |r: usize| if h.top_down { r } else { ht - 1 - r };
    let mut alpha = Alpha::Opaque;

    match (h.compression, h.bpp) {
        (BI_RLE8, 8) | (BI_RLE4, 4) => {
            // Unwritten pixels stay transparent black, like Windows does.
            plane.data.fill(0);
            decode_rle(data, &h, &palette, &mut plane)?;
            alpha = Alpha::Straight;
        }
        (BI_RGB, 1 | 2 | 4 | 8) => {
            let stride = (h.bpp as usize * w).div_ceil(32) * 4;
            let bits = h.bpp as usize;
            let per_byte = 8 / bits;
            let mask = ((1u16 << bits) - 1) as u8;
            for r in 0..ht {
                let src = data
                    .get(r * stride..r * stride + stride)
                    .ok_or_else(|| corrupt("truncated pixels"))?;
                let dst = plane.row_mut(dst_row(r) as u32);
                for x in 0..w {
                    let byte = src[x / per_byte];
                    let shift = 8 - bits * (x % per_byte + 1);
                    let idx = ((byte >> shift) & mask) as usize;
                    dst[x * 4..x * 4 + 4]
                        .copy_from_slice(&palette.get(idx).copied().unwrap_or([0, 0, 0, 255]));
                }
            }
        }
        (BI_RGB, 24) => {
            let stride = (3 * w).div_ceil(4) * 4;
            for r in 0..ht {
                let src = data
                    .get(r * stride..r * stride + w * 3)
                    .ok_or_else(|| corrupt("truncated pixels"))?;
                let dst = plane.row_mut(dst_row(r) as u32);
                for (o, i) in dst.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
                    o.copy_from_slice(&[i[0], i[1], i[2], 255]);
                }
            }
        }
        (BI_RGB | BI_BITFIELDS | BI_ALPHABITFIELDS, 16 | 32) => {
            let bytes = h.bpp as usize / 8;
            let stride = (bytes * w).div_ceil(4) * 4;
            let fields = h.masks.map(Field::new);
            let standard_bgrx =
                bytes == 4 && h.masks[..3] == [0x00FF_0000, 0x0000_FF00, 0x0000_00FF];
            let has_alpha_mask = h.masks[3] != 0;
            for r in 0..ht {
                let src = data
                    .get(r * stride..r * stride + w * bytes)
                    .ok_or_else(|| corrupt("truncated pixels"))?;
                let dst = plane.row_mut(dst_row(r) as u32);
                if standard_bgrx {
                    // Memory layout already is B, G, R, X/A.
                    dst.copy_from_slice(src);
                    if !has_alpha_mask {
                        for px in dst.chunks_exact_mut(4) {
                            px[3] = 255;
                        }
                    }
                    continue;
                }
                for (o, i) in dst.chunks_exact_mut(4).zip(src.chunks_exact(bytes)) {
                    let v = if bytes == 2 {
                        u16::from_le_bytes([i[0], i[1]]) as u32
                    } else {
                        u32::from_le_bytes([i[0], i[1], i[2], i[3]])
                    };
                    let a = if has_alpha_mask {
                        fields[3].get(v)
                    } else {
                        255
                    };
                    o.copy_from_slice(&[fields[2].get(v), fields[1].get(v), fields[0].get(v), a]);
                }
            }
            if has_alpha_mask {
                alpha = Alpha::Straight;
            }
        }
        (c, b) => {
            return Err(corrupt(&format!(
                "unsupported compression {c} with {b} bpp"
            )));
        }
    }
    Ok(Image {
        pixels: Pixels::Packed {
            plane,
            format: PackedFormat::Bgra8,
            alpha,
        },
        width: h.width,
        height: h.height,
        full_width: h.width,
        full_height: h.height,
        region: None,
    })
}

fn read_palette(d: &[u8], h: &Header) -> Result<Vec<[u8; 4]>> {
    let mut pal = Vec::with_capacity(h.palette_len);
    for i in 0..h.palette_len {
        let o = h.palette_offset + i * h.palette_entry_size;
        match d.get(o..o + 3) {
            Some(c) => pal.push([c[0], c[1], c[2], 255]),
            None => break,
        }
    }
    if h.palette_len > 0 && pal.is_empty() {
        return Err(corrupt("missing palette"));
    }
    Ok(pal)
}

/// Extracts one channel from a bit-field pixel, scaled to 8 bits.
#[derive(Clone, Copy)]
struct Field {
    shift: u32,
    max: u32,
}

impl Field {
    fn new(mask: u32) -> Field {
        if mask == 0 {
            return Field { shift: 0, max: 0 };
        }
        let shift = mask.trailing_zeros();
        Field {
            shift,
            max: mask >> shift,
        }
    }

    #[inline]
    fn get(self, v: u32) -> u8 {
        if self.max == 0 {
            return 0;
        }
        let c = (v >> self.shift) & self.max;
        ((c * 255 + self.max / 2) / self.max) as u8
    }
}

fn decode_rle(data: &[u8], h: &Header, palette: &[[u8; 4]], plane: &mut Plane) -> Result<()> {
    let (w, ht) = (h.width as usize, h.height as usize);
    let four = h.bpp == 4;
    let (mut x, mut y, mut i) = (0usize, 0usize, 0usize);
    let put = |plane: &mut Plane, x: usize, y: usize, idx: u8| {
        if x < w && y < ht {
            let row = if h.top_down { y } else { ht - 1 - y };
            let c = palette.get(idx as usize).copied().unwrap_or([0, 0, 0, 255]);
            plane.row_mut(row as u32)[x * 4..x * 4 + 4].copy_from_slice(&c);
        }
    };
    while i + 1 < data.len() && y < ht {
        let (n, v) = (data[i] as usize, data[i + 1]);
        i += 2;
        if n > 0 {
            for k in 0..n {
                let idx = if four {
                    if k % 2 == 0 { v >> 4 } else { v & 0x0F }
                } else {
                    v
                };
                put(plane, x, y, idx);
                x += 1;
            }
            continue;
        }
        match v {
            0 => {
                x = 0;
                y += 1;
            }
            1 => return Ok(()),
            2 => {
                let (dx, dy) = (
                    *data.get(i).unwrap_or(&0) as usize,
                    *data.get(i + 1).unwrap_or(&0) as usize,
                );
                i += 2;
                x += dx;
                y += dy;
            }
            count => {
                let count = count as usize;
                let bytes = if four { count.div_ceil(2) } else { count };
                let run = data
                    .get(i..i + bytes)
                    .ok_or_else(|| corrupt("truncated RLE run"))?;
                for k in 0..count {
                    let idx = if four {
                        let b = run[k / 2];
                        if k % 2 == 0 { b >> 4 } else { b & 0x0F }
                    } else {
                        run[k]
                    };
                    put(plane, x, y, idx);
                    x += 1;
                }
                // Absolute runs are padded to 16 bits.
                i += bytes.div_ceil(2) * 2;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a BMP with a 40-byte header (or `hsize`), optional palette and
    /// masks, and raw pixel rows (already padded).
    fn bmp(w: i32, h: i32, bpp: u16, comp: u32, extra: &[u8], pixels: &[u8]) -> Vec<u8> {
        let off = 14 + 40 + extra.len();
        let mut d = b"BM".to_vec();
        d.extend_from_slice(&((off + pixels.len()) as u32).to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&(off as u32).to_le_bytes());
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&w.to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&bpp.to_le_bytes());
        d.extend_from_slice(&comp.to_le_bytes());
        d.extend_from_slice(&[0; 20]);
        d.extend_from_slice(extra);
        d.extend_from_slice(pixels);
        d
    }

    fn px(img: &Image, x: usize, y: usize) -> [u8; 4] {
        let b = img.to_bgra();
        let i = (y * img.width as usize + x) * 4;
        [b[i], b[i + 1], b[i + 2], b[i + 3]]
    }

    #[test]
    fn bottom_up_24_bit() {
        // 2×2, rows padded to 8 bytes; bottom row first.
        let rows = [1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0];
        let img = decode(&bmp(2, 2, 24, BI_RGB, &[], &rows)).unwrap();
        assert_eq!(px(&img, 0, 0), [7, 8, 9, 255]);
        assert_eq!(px(&img, 1, 1), [4, 5, 6, 255]);
        assert!(!img.has_alpha());
    }

    #[test]
    fn top_down_32_bit_and_bitfields() {
        let rows = [1, 2, 3, 99, 4, 5, 6, 99];
        let img = decode(&bmp(2, -1, 32, BI_RGB, &[], &rows)).unwrap();
        assert_eq!(px(&img, 0, 0), [1, 2, 3, 255]);
        // RGB565 via bit fields: pure red, pure green.
        let masks: Vec<u8> = [0xF800u32, 0x07E0, 0x001F]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        let rows = [0x00, 0xF8, 0xE0, 0x07];
        let img = decode(&bmp(2, 1, 16, BI_BITFIELDS, &masks, &rows)).unwrap();
        assert_eq!(px(&img, 0, 0), [0, 0, 255, 255]);
        assert_eq!(px(&img, 1, 0), [0, 255, 0, 255]);
        // ARGB with alpha mask.
        let masks: Vec<u8> = [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        let img = decode(&bmp(1, 1, 32, BI_ALPHABITFIELDS, &masks, &[10, 20, 30, 40])).unwrap();
        assert_eq!(px(&img, 0, 0), [10, 20, 30, 40]);
        assert!(img.has_alpha());
    }

    #[test]
    fn palettes_1_4_8_bit() {
        let pal = [0, 0, 0, 0, 255, 255, 255, 0];
        let img = decode(&bmp(3, 1, 1, BI_RGB, &pal, &[0b1010_0000, 0, 0, 0])).unwrap();
        assert_eq!(px(&img, 0, 0), [255, 255, 255, 255]);
        assert_eq!(px(&img, 1, 0), [0, 0, 0, 255]);
        let mut pal16 = vec![0u8; 64];
        pal16[4 * 5..4 * 5 + 3].copy_from_slice(&[1, 2, 3]);
        let img = decode(&bmp(2, 1, 4, BI_RGB, &pal16, &[0x50, 0, 0, 0])).unwrap();
        assert_eq!(px(&img, 0, 0), [1, 2, 3, 255]);
        let mut pal256 = vec![0u8; 1024];
        pal256[4 * 200..4 * 200 + 3].copy_from_slice(&[9, 8, 7]);
        let img = decode(&bmp(1, 1, 8, BI_RGB, &pal256, &[200, 0, 0, 0])).unwrap();
        assert_eq!(px(&img, 0, 0), [9, 8, 7, 255]);
    }

    #[test]
    fn rle8() {
        let mut pal = vec![0u8; 1024];
        pal[4..7].copy_from_slice(&[10, 10, 10]);
        pal[8..11].copy_from_slice(&[20, 20, 20]);
        // Row 0 (bottom): run of 3×index1, EOL; row 1: absolute run [2,1,2], EOF.
        let data = [3, 1, 0, 0, 0, 3, 2, 1, 2, 0, 0, 1];
        let img = decode(&bmp(3, 2, 8, BI_RLE8, &pal, &data)).unwrap();
        assert_eq!(px(&img, 0, 1), [10, 10, 10, 255]);
        assert_eq!(px(&img, 0, 0), [20, 20, 20, 255]);
        assert_eq!(px(&img, 1, 0), [10, 10, 10, 255]);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(decode(b"BM").is_err());
        assert!(decode(&bmp(0, 1, 24, BI_RGB, &[], &[])).is_err());
        assert!(decode(&bmp(4, 4, 24, BI_RGB, &[], &[0; 10])).is_err());
        assert!(decode(&bmp(1, 1, 24, 4 /* JPEG */, &[], &[0; 4])).is_err());
        let h = Header::parse(&bmp(5, -3, 24, BI_RGB, &[], &[])).unwrap();
        assert_eq!((h.width, h.height, h.top_down), (5, 3, true));
    }
}
