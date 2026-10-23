//! Small geometry helpers: rectangles, 2D affine transforms and the eight
//! EXIF orientations (the dihedral group D4).

/// Axis-aligned rectangle in floating point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    /// Rectangle spanned by two corners in any order.
    pub fn from_points(a: (f64, f64), b: (f64, f64)) -> Self {
        let (x0, x1) = if a.0 <= b.0 { (a.0, b.0) } else { (b.0, a.0) };
        let (y0, y1) = if a.1 <= b.1 { (a.1, b.1) } else { (b.1, a.1) };
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x && p.0 < self.right() && p.1 >= self.y && p.1 < self.bottom()
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Snaps outwards/inwards to whole pixels and clamps to `0..w × 0..h`.
    pub fn to_pixels(&self, w: u32, h: u32) -> Option<PixelRect> {
        let x0 = self.x.round().clamp(0.0, w as f64) as u32;
        let y0 = self.y.round().clamp(0.0, h as f64) as u32;
        let x1 = self.right().round().clamp(0.0, w as f64) as u32;
        let y1 = self.bottom().round().clamp(0.0, h as f64) as u32;
        (x1 > x0 && y1 > y0).then_some(PixelRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// Integer rectangle in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PixelRect {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        PixelRect { x, y, w, h }
    }

    pub fn to_rect(self) -> Rect {
        Rect::new(self.x as f64, self.y as f64, self.w as f64, self.h as f64)
    }

    pub fn right(&self) -> u32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> u32 {
        self.y + self.h
    }
}

/// `x' = a·x + b·y + c`, `y' = d·x + e·y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 0.0,
        e: 1.0,
        f: 0.0,
    };

    pub fn scale(sx: f64, sy: f64) -> Self {
        Affine {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: sy,
            f: 0.0,
        }
    }

    pub fn translate(tx: f64, ty: f64) -> Self {
        Affine {
            a: 1.0,
            b: 0.0,
            c: tx,
            d: 0.0,
            e: 1.0,
            f: ty,
        }
    }

    pub fn apply(&self, p: (f64, f64)) -> (f64, f64) {
        (
            self.a * p.0 + self.b * p.1 + self.c,
            self.d * p.0 + self.e * p.1 + self.f,
        )
    }

    /// `self` first, then `next`.
    pub fn then(&self, next: &Affine) -> Affine {
        Affine {
            a: next.a * self.a + next.b * self.d,
            b: next.a * self.b + next.b * self.e,
            c: next.a * self.c + next.b * self.f + next.c,
            d: next.d * self.a + next.e * self.d,
            e: next.d * self.b + next.e * self.e,
            f: next.d * self.c + next.e * self.f + next.f,
        }
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.e - self.b * self.d
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine {
            a: self.e * inv,
            b: -self.b * inv,
            c: (self.b * self.f - self.e * self.c) * inv,
            d: -self.d * inv,
            e: self.a * inv,
            f: (self.d * self.c - self.a * self.f) * inv,
        })
    }

    /// Bounding box of a transformed rectangle.
    pub fn map_rect(&self, r: &Rect) -> Rect {
        let pts = [
            self.apply((r.x, r.y)),
            self.apply((r.right(), r.y)),
            self.apply((r.x, r.bottom())),
            self.apply((r.right(), r.bottom())),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// How stored pixels must be transformed for display: first an optional
/// horizontal mirror, then `quarter_turns` × 90° clockwise. Covers the eight
/// EXIF orientations and composes with user rotation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Orientation {
    pub quarter_turns: u8,
    pub mirror: bool,
}

impl Orientation {
    pub const NORMAL: Orientation = Orientation {
        quarter_turns: 0,
        mirror: false,
    };

    /// From the EXIF `Orientation` tag (1..=8); anything else is normal.
    pub fn from_exif(v: u16) -> Self {
        let (quarter_turns, mirror) = match v {
            2 => (0, true),
            3 => (2, false),
            4 => (2, true),
            5 => (3, true),
            6 => (1, false),
            7 => (1, true),
            8 => (3, false),
            _ => (0, false),
        };
        Orientation {
            quarter_turns,
            mirror,
        }
    }

    pub fn to_exif(self) -> u16 {
        match (self.quarter_turns % 4, self.mirror) {
            (0, false) => 1,
            (0, true) => 2,
            (2, false) => 3,
            (2, true) => 4,
            (3, true) => 5,
            (1, false) => 6,
            (1, true) => 7,
            _ => 8,
        }
    }

    /// `self` first, then `next`.
    pub fn then(self, next: Orientation) -> Orientation {
        // Mirror ∘ Rot(r) = Rot(−r) ∘ Mirror.
        let turns = if next.mirror {
            (4 - self.quarter_turns % 4) % 4
        } else {
            self.quarter_turns % 4
        };
        Orientation {
            quarter_turns: (turns + next.quarter_turns) % 4,
            mirror: self.mirror ^ next.mirror,
        }
    }

    pub fn rotated_cw(self) -> Orientation {
        self.then(Orientation {
            quarter_turns: 1,
            mirror: false,
        })
    }

    pub fn rotated_ccw(self) -> Orientation {
        self.then(Orientation {
            quarter_turns: 3,
            mirror: false,
        })
    }

    pub fn swaps_axes(self) -> bool {
        self.quarter_turns % 2 == 1
    }

    /// Display size of a `w × h` stored image.
    pub fn oriented_size<T>(self, w: T, h: T) -> (T, T) {
        if self.swaps_axes() { (h, w) } else { (w, h) }
    }

    /// Maps continuous stored coordinates (`0..w × 0..h`) to display ones.
    pub fn transform(self, w: f64, h: f64) -> Affine {
        let mut t = if self.mirror {
            Affine {
                a: -1.0,
                b: 0.0,
                c: w,
                d: 0.0,
                e: 1.0,
                f: 0.0,
            }
        } else {
            Affine::IDENTITY
        };
        let (mut cw, mut ch) = (w, h);
        for _ in 0..self.quarter_turns % 4 {
            // 90° clockwise of a cw × ch image: (x, y) → (ch − y, x).
            let rot = Affine {
                a: 0.0,
                b: -1.0,
                c: ch,
                d: 1.0,
                e: 0.0,
                f: 0.0,
            };
            t = t.then(&rot);
            std::mem::swap(&mut cw, &mut ch);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn exif_roundtrip_and_known_maps() {
        for v in 1..=8 {
            assert_eq!(Orientation::from_exif(v).to_exif(), v);
        }
        let (w, h) = (4.0, 2.0);
        // Point near the top-left corner of the stored image.
        let p = (0.5, 0.5);
        let expect = [
            (1, (0.5, 0.5)),
            (2, (3.5, 0.5)),
            (3, (3.5, 1.5)),
            (4, (0.5, 1.5)),
            (5, (0.5, 0.5)), // transpose
            (6, (1.5, 0.5)), // rotate 90° cw: top-left goes top-right
            (7, (1.5, 3.5)), // transverse
            (8, (0.5, 3.5)), // rotate 270° cw: top-left goes bottom-left
        ];
        for (v, q) in expect {
            let t = Orientation::from_exif(v).transform(w, h);
            assert!(close(t.apply(p), q), "exif {v}: {:?} != {q:?}", t.apply(p));
        }
    }

    #[test]
    fn composition_matches_transforms() {
        let (w, h) = (5.0, 3.0);
        let p = (1.25, 0.75);
        for a in 1..=8 {
            for b in 1..=8 {
                let oa = Orientation::from_exif(a);
                let ob = Orientation::from_exif(b);
                let (aw, ah) = oa.oriented_size(w, h);
                let seq = oa.transform(w, h).then(&ob.transform(aw, ah));
                let combined = oa.then(ob).transform(w, h);
                assert!(close(seq.apply(p), combined.apply(p)), "{a} then {b}");
            }
        }
        let o = Orientation::from_exif(6);
        assert_eq!(o.rotated_ccw(), Orientation::NORMAL);
        assert_eq!(
            Orientation::NORMAL.rotated_cw().rotated_cw(),
            Orientation::from_exif(3)
        );
    }

    #[test]
    fn affine_inverse_and_rects() {
        let t = Affine::scale(2.0, 3.0).then(&Affine::translate(5.0, -1.0));
        let inv = t.inverse().unwrap();
        assert!(close(inv.apply(t.apply((7.0, 9.0))), (7.0, 9.0)));
        let r = Orientation::from_exif(6)
            .transform(4.0, 2.0)
            .map_rect(&Rect::new(0.0, 0.0, 4.0, 2.0));
        assert_eq!(r, Rect::new(0.0, 0.0, 2.0, 4.0));
        assert_eq!(
            Rect::new(0.4, 0.6, 2.2, 1.0).to_pixels(10, 10),
            Some(PixelRect::new(0, 1, 3, 1))
        );
        assert_eq!(Rect::new(-5.0, -5.0, 2.0, 2.0).to_pixels(10, 10), None);
        assert_eq!(
            Rect::new(0.0, 0.0, 4.0, 4.0).intersect(&Rect::new(2.0, 2.0, 4.0, 4.0)),
            Some(Rect::new(2.0, 2.0, 2.0, 2.0))
        );
    }
}
