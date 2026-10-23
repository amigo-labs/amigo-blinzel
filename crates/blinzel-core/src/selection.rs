//! Rectangle selection in oriented full-resolution image coordinates, with
//! handles, moving, and mapping back to stored pixels for decoding.

use crate::geom::{Orientation, PixelRect, Rect};
use crate::layout::Layout;

/// What a left-button drag does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftDrag {
    Select,
    Pan,
}

/// If the image fits, left-drag selects; if it is larger than the window,
/// left-drag pans and Shift+left-drag selects.
pub fn left_drag_action(layout: &Layout, shift: bool) -> LeftDrag {
    if layout.overflows && !shift {
        LeftDrag::Pan
    } else {
        LeftDrag::Select
    }
}

/// The eight resize handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Handle {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::N,
        Handle::NE,
        Handle::E,
        Handle::SE,
        Handle::S,
        Handle::SW,
        Handle::W,
        Handle::NW,
    ];

    /// Which edges the handle moves: (left, top, right, bottom).
    fn edges(self) -> (bool, bool, bool, bool) {
        match self {
            Handle::N => (false, true, false, false),
            Handle::NE => (false, true, true, false),
            Handle::E => (false, false, true, false),
            Handle::SE => (false, false, true, true),
            Handle::S => (false, false, false, true),
            Handle::SW => (true, false, false, true),
            Handle::W => (true, false, false, false),
            Handle::NW => (true, true, false, false),
        }
    }

    /// Position on a rectangle (centre of the handle).
    fn anchor(self, r: &Rect) -> (f64, f64) {
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        match self {
            Handle::N => (cx, r.y),
            Handle::NE => (r.right(), r.y),
            Handle::E => (r.right(), cy),
            Handle::SE => (r.right(), r.bottom()),
            Handle::S => (cx, r.bottom()),
            Handle::SW => (r.x, r.bottom()),
            Handle::W => (r.x, cy),
            Handle::NW => (r.x, r.y),
        }
    }
}

/// Hit-test result in client space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Inside,
    Handle(Handle),
}

/// Handle squares of a client-space selection rectangle.
pub fn handle_rects(sel_client: &Rect, size: f64) -> [(Handle, Rect); 8] {
    Handle::ALL.map(|h| {
        let (x, y) = h.anchor(sel_client);
        (h, Rect::new(x - size / 2.0, y - size / 2.0, size, size))
    })
}

/// Hit-tests a client point against a client-space selection rectangle.
/// Handles are only offered when the rectangle is large enough to grab.
pub fn hit_test(sel_client: &Rect, p: (f64, f64), handle_size: f64) -> Hit {
    let grab = handle_size * 1.5;
    if sel_client.w >= handle_size * 2.0 && sel_client.h >= handle_size * 2.0 {
        // Corners take precedence over edges.
        let order = [
            Handle::NW,
            Handle::NE,
            Handle::SE,
            Handle::SW,
            Handle::N,
            Handle::E,
            Handle::S,
            Handle::W,
        ];
        for h in order {
            let (x, y) = h.anchor(sel_client);
            if (p.0 - x).abs() <= grab / 2.0 && (p.1 - y).abs() <= grab / 2.0 {
                return Hit::Handle(h);
            }
        }
    }
    if sel_client.contains(p) {
        Hit::Inside
    } else {
        Hit::Outside
    }
}

/// An ongoing drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    New { anchor: (f64, f64) },
    Move { start: (f64, f64), rect: Rect },
    Resize { handle: Handle, rect: Rect },
}

impl Drag {
    /// Starts a drag at image point `p` given what was hit.
    pub fn begin(hit: Hit, p: (f64, f64), current: Option<Rect>) -> Drag {
        match (hit, current) {
            (Hit::Inside, Some(rect)) => Drag::Move { start: p, rect },
            (Hit::Handle(handle), Some(rect)) => Drag::Resize { handle, rect },
            _ => Drag::New { anchor: p },
        }
    }

    /// The selection for the pointer at image point `p`, snapped to whole
    /// pixels and clamped to `bounds`; `None` while it is empty.
    pub fn update(&self, p: (f64, f64), bounds: &Rect) -> Option<Rect> {
        let clamp = |q: (f64, f64)| {
            (
                q.0.clamp(bounds.x, bounds.right()),
                q.1.clamp(bounds.y, bounds.bottom()),
            )
        };
        let r = match *self {
            Drag::New { anchor } => Rect::from_points(clamp(anchor), clamp(p)),
            Drag::Move { start, rect } => {
                let dx = (p.0 - start.0).clamp(bounds.x - rect.x, bounds.right() - rect.right());
                let dy = (p.1 - start.1).clamp(bounds.y - rect.y, bounds.bottom() - rect.bottom());
                Rect::new(rect.x + dx, rect.y + dy, rect.w, rect.h)
            }
            Drag::Resize { handle, rect } => {
                let p = clamp(p);
                let (l, t, r, b) = handle.edges();
                let x0 = if l { p.0 } else { rect.x };
                let y0 = if t { p.1 } else { rect.y };
                let x1 = if r { p.0 } else { rect.right() };
                let y1 = if b { p.1 } else { rect.bottom() };
                Rect::from_points((x0, y0), (x1, y1))
            }
        };
        let snapped = Rect::from_points(
            (r.x.round(), r.y.round()),
            (r.right().round(), r.bottom().round()),
        );
        (snapped.w >= 1.0 && snapped.h >= 1.0).then_some(snapped)
    }
}

/// Maps an oriented full-resolution rectangle back to stored pixels (the
/// orientation of the file), e.g. to decode exactly that region.
pub fn to_stored(rect: &PixelRect, orientation: Orientation, stored: (u32, u32)) -> PixelRect {
    let fwd = orientation.transform(stored.0 as f64, stored.1 as f64);
    let inv = fwd
        .inverse()
        .expect("orientation transforms are invertible");
    let r = inv.map_rect(&rect.to_rect());
    r.to_pixels(stored.0, stored.1).unwrap_or_default()
}

/// Rotates/mirrors tightly packed BGRA pixels of a stored-orientation
/// region into display orientation (for the clipboard).
pub fn orient_bgra(src: &[u8], w: u32, h: u32, o: Orientation) -> (Vec<u8>, u32, u32) {
    if o == Orientation::NORMAL {
        return (src.to_vec(), w, h);
    }
    let (ow, oh) = o.oriented_size(w, h);
    let t = o.transform(w as f64, h as f64);
    let mut out = vec![0u8; src.len()];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = t.apply((x as f64 + 0.5, y as f64 + 0.5));
            let (dx, dy) = (dx.floor() as usize, dy.floor() as usize);
            let s = ((y * w + x) * 4) as usize;
            let d = (dy * ow as usize + dx) * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    (out, ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{ViewMode, ViewOptions, ViewState, compute};

    #[test]
    fn drag_modes_depend_on_overflow_and_shift() {
        let fit = ViewOptions {
            mode: ViewMode::FitImageToWindow,
            no_upscale: true,
        };
        let actual = ViewOptions {
            mode: ViewMode::ActualSize,
            no_upscale: true,
        };
        let l = compute((4000, 3000), (800.0, 600.0), &fit, &ViewState::default());
        assert_eq!(left_drag_action(&l, false), LeftDrag::Select);
        let l = compute((4000, 3000), (800.0, 600.0), &actual, &ViewState::default());
        assert_eq!(left_drag_action(&l, false), LeftDrag::Pan);
        assert_eq!(left_drag_action(&l, true), LeftDrag::Select);
    }

    #[test]
    fn new_drag_snaps_and_clamps() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 50.0);
        let d = Drag::begin(Hit::Outside, (10.4, 10.6), None);
        assert_eq!(
            d.update((30.6, 20.2), &bounds),
            Some(Rect::new(10.0, 11.0, 21.0, 9.0))
        );
        // Dragging up-left and out of the image.
        assert_eq!(
            d.update((-20.0, -5.0), &bounds),
            Some(Rect::new(0.0, 0.0, 10.0, 11.0))
        );
        assert_eq!(d.update((10.45, 10.55), &bounds), None);
    }

    #[test]
    fn move_and_resize() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        let sel = Rect::new(10.0, 10.0, 20.0, 20.0);
        let mv = Drag::begin(Hit::Inside, (15.0, 15.0), Some(sel));
        assert_eq!(
            mv.update((25.0, 20.0), &bounds),
            Some(Rect::new(20.0, 15.0, 20.0, 20.0))
        );
        // Moving stops at the image edge without shrinking.
        assert_eq!(
            mv.update((500.0, -500.0), &bounds),
            Some(Rect::new(80.0, 0.0, 20.0, 20.0))
        );
        let rs = Drag::begin(Hit::Handle(Handle::SE), (30.0, 30.0), Some(sel));
        assert_eq!(
            rs.update((50.0, 40.0), &bounds),
            Some(Rect::new(10.0, 10.0, 40.0, 30.0))
        );
        // Pulling a handle across the opposite edge flips the rectangle.
        let rs = Drag::begin(Hit::Handle(Handle::W), (10.0, 20.0), Some(sel));
        assert_eq!(
            rs.update((40.0, 0.0), &bounds),
            Some(Rect::new(30.0, 10.0, 10.0, 20.0))
        );
    }

    #[test]
    fn hit_testing() {
        let r = Rect::new(100.0, 100.0, 200.0, 100.0);
        assert_eq!(hit_test(&r, (100.0, 100.0), 8.0), Hit::Handle(Handle::NW));
        assert_eq!(hit_test(&r, (300.0, 150.0), 8.0), Hit::Handle(Handle::E));
        assert_eq!(hit_test(&r, (200.0, 150.0), 8.0), Hit::Inside);
        assert_eq!(hit_test(&r, (50.0, 50.0), 8.0), Hit::Outside);
        // Tiny selections have no handles.
        let tiny = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(hit_test(&tiny, (0.0, 0.0), 8.0), Hit::Inside);
        assert_eq!(
            handle_rects(&r, 8.0)[0],
            (Handle::N, Rect::new(196.0, 96.0, 8.0, 8.0))
        );
    }

    #[test]
    fn stored_mapping_roundtrips_all_orientations() {
        let stored = (40u32, 30u32);
        for v in 1..=8 {
            let o = Orientation::from_exif(v);
            let (ow, oh) = o.oriented_size(stored.0, stored.1);
            let sel = PixelRect::new(3, 5, ow.min(12), oh.min(7));
            let back = to_stored(&sel, o, stored);
            // Mapping forward again yields the selection.
            let fwd = o
                .transform(stored.0 as f64, stored.1 as f64)
                .map_rect(&back.to_rect());
            assert_eq!(fwd.to_pixels(ow, oh), Some(sel), "exif {v}");
        }
    }

    #[test]
    fn orient_pixels() {
        // 2×1 image: red, green (BGRA).
        let src = [0, 0, 255, 255, 0, 255, 0, 255];
        let (out, w, h) = orient_bgra(&src, 2, 1, Orientation::from_exif(6));
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, src); // 90° cw: left pixel on top
        let (out, _, _) = orient_bgra(&src, 2, 1, Orientation::from_exif(2));
        assert_eq!(&out[..4], &src[4..]);
    }
}
