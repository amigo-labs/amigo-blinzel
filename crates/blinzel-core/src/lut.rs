//! 3D colour lookup tables: a colour transform (e.g. image profile →
//! display profile) sampled on a regular RGB grid and interpolated
//! trilinearly, exactly like the GPU's 3D texture filter.

/// Grid points per axis the viewer builds (33³ ≈ 36k entries).
pub const SIZE: usize = 33;

/// `size`³ RGB nodes (0..=65535) at inputs `i / (size - 1)`, red varying
/// fastest, then green, then blue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lut3d {
    size: usize,
    nodes: Vec<[u16; 3]>,
}

impl Lut3d {
    /// Samples `f` (RGB 0..1 → RGB 0..1) on the grid.
    pub fn from_fn(size: usize, mut f: impl FnMut([f64; 3]) -> [f64; 3]) -> Lut3d {
        assert!(size >= 2);
        let step = 1.0 / (size - 1) as f64;
        let mut nodes = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let out = f([r as f64 * step, g as f64 * step, b as f64 * step]);
                    nodes.push(out.map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16));
                }
            }
        }
        Lut3d { size, nodes }
    }

    /// Nodes produced elsewhere (e.g. by the platform's colour engine).
    pub fn from_nodes(size: usize, nodes: Vec<[u16; 3]>) -> Option<Lut3d> {
        (size >= 2 && nodes.len() == size * size * size).then_some(Lut3d { size, nodes })
    }

    pub fn identity(size: usize) -> Lut3d {
        Lut3d::from_fn(size, |c| c)
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn nodes(&self) -> &[[u16; 3]] {
        &self.nodes
    }

    fn node(&self, r: usize, g: usize, b: usize) -> [u16; 3] {
        self.nodes[(b * self.size + g) * self.size + r]
    }

    /// Trilinear lookup of `c` (0..1).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let last = (self.size - 1) as f32;
        let mut i = [0usize; 3];
        let mut f = [0f32; 3];
        for k in 0..3 {
            let x = c[k].clamp(0.0, 1.0) * last;
            let x0 = x.floor().min(last - 1.0);
            i[k] = x0 as usize;
            f[k] = x - x0;
        }
        let mut out = [0f32; 3];
        for (corner, w) in corners(f) {
            let n = self.node(i[0] + corner[0], i[1] + corner[1], i[2] + corner[2]);
            for k in 0..3 {
                out[k] += w * n[k] as f32;
            }
        }
        out.map(|v| v / 65535.0)
    }

    /// Integer trilinear lookup for 8-bit colours (the CPU first frame).
    #[inline]
    pub fn apply_u8(&self, c: [u8; 3]) -> [u8; 3] {
        // Fixed point 8.8 position on the grid.
        let scale = ((self.size - 1) << 8) as u32;
        let mut i = [0usize; 3];
        let mut f = [0u32; 3];
        for k in 0..3 {
            let pos = (c[k] as u32 * scale + 127) / 255;
            let (x0, frac) = ((pos >> 8) as usize, pos & 255);
            (i[k], f[k]) = if x0 >= self.size - 1 {
                (self.size - 2, 256)
            } else {
                (x0, frac)
            };
        }
        let at = |dr: usize, dg: usize, db: usize| self.node(i[0] + dr, i[1] + dg, i[2] + db);
        let lerp = |a: [u16; 3], b: [u16; 3], t: u32| -> [u32; 3] {
            [0, 1, 2].map(|k| (a[k] as u32 * (256 - t) + b[k] as u32 * t) >> 8)
        };
        let lerp32 = |a: [u32; 3], b: [u32; 3], t: u32| -> [u32; 3] {
            [0, 1, 2].map(|k| (a[k] * (256 - t) + b[k] * t) >> 8)
        };
        let x00 = lerp(at(0, 0, 0), at(1, 0, 0), f[0]);
        let x10 = lerp(at(0, 1, 0), at(1, 1, 0), f[0]);
        let x01 = lerp(at(0, 0, 1), at(1, 0, 1), f[0]);
        let x11 = lerp(at(0, 1, 1), at(1, 1, 1), f[0]);
        let y0 = lerp32(x00, x10, f[1]);
        let y1 = lerp32(x01, x11, f[1]);
        let v = lerp32(y0, y1, f[2]);
        v.map(|x| ((x * 255 + 32767) / 65535) as u8)
    }
}

/// The eight cell corners and their trilinear weights.
fn corners(f: [f32; 3]) -> impl Iterator<Item = ([usize; 3], f32)> {
    (0..8).map(move |m| {
        let d = [m & 1, (m >> 1) & 1, (m >> 2) & 1];
        let w = (0..3)
            .map(|k| if d[k] == 1 { f[k] } else { 1.0 - f[k] })
            .product();
        (d, w)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_maps_to_itself() {
        let lut = Lut3d::identity(SIZE);
        for v in [0.0f32, 0.1, 0.5, 0.73, 1.0] {
            let out = lut.apply([v, 1.0 - v, v * 0.5]);
            assert!((out[0] - v).abs() < 1e-4);
            assert!((out[1] - (1.0 - v)).abs() < 1e-4);
        }
        for v in 0..=255u8 {
            assert_eq!(lut.apply_u8([v, 255 - v, v / 2]), [v, 255 - v, v / 2]);
        }
    }

    #[test]
    fn integer_lookup_follows_the_float_one() {
        // Channel mixing plus a curve, like a gamut conversion.
        let f = |c: [f64; 3]| {
            [
                (0.8 * c[0] + 0.2 * c[1]).powf(1.1),
                (0.1 * c[0] + 0.9 * c[1]).sqrt(),
                c[2] * c[2],
            ]
        };
        let lut = Lut3d::from_fn(SIZE, f);
        let mut worst = 0i32;
        for r in (0..=255).step_by(5) {
            for g in (0..=255).step_by(7) {
                for b in (0..=255).step_by(11) {
                    let c = [r as u8, g as u8, b as u8];
                    let fl = lut.apply(c.map(|v| v as f32 / 255.0));
                    let int = lut.apply_u8(c);
                    for k in 0..3 {
                        let d = (int[k] as i32 - (fl[k] * 255.0).round() as i32).abs();
                        worst = worst.max(d);
                    }
                }
            }
        }
        assert!(worst <= 1, "max difference {worst}");
        // Nodes are hit exactly.
        assert_eq!(
            lut.apply([1.0, 1.0, 1.0]).map(|v| (v * 255.0).round()),
            [255.0; 3]
        );
        assert!(Lut3d::from_nodes(3, vec![[0; 3]; 26]).is_none());
    }
}
