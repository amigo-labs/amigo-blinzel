//! The screenshot overlay: a frozen picture of the desktop over which a
//! region, a window or a whole monitor is picked, as in Greenshot. The
//! shell captures the screen, shows the picture in a borderless topmost
//! window covering every monitor and feeds the input here. Coordinates
//! are physical pixels of that window, which are the picture's pixels.
//!
//! - Hovering highlights the window under the pointer; a click takes it.
//! - Dragging takes a region.
//! - Enter takes the monitor under the pointer, Escape cancels.
//! - The arrow keys move the pointer by one pixel (Shift: ten) for exact
//!   edges.

use blinzel_core::geom::{Affine, Rect};
use blinzel_i18n::{Msg, tr};

use crate::backend::{Canvas, Device, Key};
use crate::gfx::{Rgba, Shape};
use crate::widgets::{self, Painter};

/// Movement below which a press and release is a click, not a drag.
const DRAG_MIN: f64 = 4.0;
/// Magnifier: source pixels per side and zoom.
const LOUPE_SRC: f64 = 15.0;
const LOUPE_ZOOM: f64 = 8.0;

/// What an input asks the shell to do.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Unchanged,
    Redraw,
    Cancel,
    /// Take this part of the picture (whole pixels, inside the picture).
    Done(Rect),
}

pub struct Snip {
    /// The picture's size.
    size: (f64, f64),
    /// Monitors, in picture coordinates.
    monitors: Vec<Rect>,
    /// Visible windows front to back, in picture coordinates.
    windows: Vec<Rect>,
    cursor: (f64, f64),
    /// Where the button went down, while it is down.
    anchor: Option<(f64, f64)>,
}

impl Snip {
    pub fn new(
        size: (u32, u32),
        monitors: Vec<Rect>,
        windows: Vec<Rect>,
        cursor: (f64, f64),
    ) -> Snip {
        let size = (size.0 as f64, size.1 as f64);
        let bounds = Rect::new(0.0, 0.0, size.0, size.1);
        let clip = |r: Rect| r.intersect(&bounds).filter(|r| r.w >= 1.0 && r.h >= 1.0);
        Snip {
            size,
            monitors: monitors.into_iter().filter_map(clip).collect(),
            windows: windows.into_iter().filter_map(clip).collect(),
            cursor,
            anchor: None,
        }
    }

    fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.size.0, self.size.1)
    }

    fn clamp(&self, p: (f64, f64)) -> (f64, f64) {
        (p.0.clamp(0.0, self.size.0), p.1.clamp(0.0, self.size.1))
    }

    fn dragging(&self) -> bool {
        self.anchor.is_some_and(|a| {
            (a.0 - self.cursor.0).abs() >= DRAG_MIN || (a.1 - self.cursor.1).abs() >= DRAG_MIN
        })
    }

    /// The monitor under the pointer (the whole picture without one).
    fn monitor(&self) -> Rect {
        self.monitors
            .iter()
            .find(|m| m.contains(self.cursor))
            .copied()
            .unwrap_or_else(|| self.bounds())
    }

    /// The frontmost window under the pointer.
    fn window(&self) -> Option<Rect> {
        self.windows
            .iter()
            .find(|w| w.contains(self.cursor))
            .copied()
    }

    /// What a release or Enter would take now.
    pub fn selection(&self) -> Rect {
        match self.anchor {
            Some(a) if self.dragging() => {
                let r = Rect::from_points(a, self.cursor);
                let (x0, y0) = (r.x.round(), r.y.round());
                let (x1, y1) = (r.right().round(), r.bottom().round());
                Rect::new(x0, y0, x1 - x0, y1 - y0)
                    .intersect(&self.bounds())
                    .unwrap_or_else(|| self.monitor())
            }
            _ => self.window().unwrap_or_else(|| self.monitor()),
        }
    }

    fn whole(r: Rect) -> Rect {
        let (x0, y0) = (r.x.floor(), r.y.floor());
        Rect::new(
            x0,
            y0,
            (r.right().ceil() - x0).max(1.0),
            (r.bottom().ceil() - y0).max(1.0),
        )
    }

    pub fn mouse_move(&mut self, p: (f64, f64)) -> Outcome {
        let p = self.clamp(p);
        if p == self.cursor {
            return Outcome::Unchanged;
        }
        self.cursor = p;
        Outcome::Redraw
    }

    pub fn mouse_down(&mut self, p: (f64, f64)) -> Outcome {
        self.cursor = self.clamp(p);
        self.anchor = Some(self.cursor);
        Outcome::Redraw
    }

    pub fn mouse_up(&mut self, p: (f64, f64)) -> Outcome {
        if self.anchor.is_none() {
            return Outcome::Unchanged;
        }
        self.cursor = self.clamp(p);
        // A drag takes its region; a click the window (or monitor) under
        // the pointer.
        let r = Self::whole(self.selection());
        self.anchor = None;
        Outcome::Done(r)
    }

    pub fn key(&mut self, key: Key, shift: bool) -> Outcome {
        let step = if shift { 10.0 } else { 1.0 };
        let mut nudge = |dx: f64, dy: f64| {
            self.cursor = self.clamp((self.cursor.0 + dx, self.cursor.1 + dy));
            Outcome::Redraw
        };
        match key {
            Key::Left => nudge(-step, 0.0),
            Key::Right => nudge(step, 0.0),
            Key::Up => nudge(0.0, -step),
            Key::Down => nudge(0.0, step),
            Key::Escape if self.anchor.is_some() => {
                self.anchor = None;
                Outcome::Redraw
            }
            Key::Escape => Outcome::Cancel,
            Key::Enter => {
                let r = if self.dragging() {
                    self.selection()
                } else {
                    self.monitor()
                };
                self.anchor = None;
                Outcome::Done(Self::whole(r))
            }
            _ => Outcome::Unchanged,
        }
    }

    /// The pointer position the overlay draws at (the arrow keys move it
    /// without moving the system pointer).
    pub fn cursor(&self) -> (f64, f64) {
        self.cursor
    }

    /// Draws the frozen `picture` (shown 1:1), dimmed outside what would
    /// be taken, with its size, a magnifier and a hint.
    pub fn draw<C: Canvas>(
        &self,
        canvas: &mut C,
        picture: &<C::Device as Device>::Image,
        scale: f64,
    ) {
        let bounds = self.bounds();
        canvas.draw_image(
            picture,
            &Affine::IDENTITY,
            &bounds,
            Rgba(0.0, 0.0, 0.0, 1.0),
            None,
        );
        let sel = self.selection();
        let dim = Rgba(0.0, 0.0, 0.0, 0.42);
        let shapes: Vec<Shape> = [
            Rect::new(0.0, 0.0, self.size.0, sel.y),
            Rect::new(0.0, sel.bottom(), self.size.0, self.size.1 - sel.bottom()),
            Rect::new(0.0, sel.y, sel.x, sel.h),
            Rect::new(sel.right(), sel.y, self.size.0 - sel.right(), sel.h),
        ]
        .into_iter()
        .filter(|r| r.w > 0.0 && r.h > 0.0)
        .map(|rect| Shape::Fill {
            rect,
            radius: 0.0,
            color: dim,
        })
        .chain([Shape::Outline {
            rect: sel,
            thickness: (2.0 * scale).round() as f32,
            radius: 0.0,
            color: widgets::ACCENT,
        }])
        .collect();
        canvas.draw_shapes(&shapes);

        let mut p = Painter { canvas, scale };
        let monitor = self.monitor();
        // Size of what would be taken, under its top left corner.
        let label = format!("{} × {}", sel.w.round(), sel.h.round());
        let (lw, lh) = p.text_size(&label, 12.0);
        let pad = p.px(6.0);
        let mut lx = sel.x;
        let mut ly = sel.y - lh - 2.0 * pad - p.px(4.0);
        if ly < monitor.y {
            ly = sel.y + p.px(4.0);
            lx += p.px(4.0);
        }
        let tag = Rect::new(lx, ly, lw + 2.0 * pad, lh + 2.0 * pad);
        p.fill(tag, 6.0, widgets::PANEL);
        p.text(&label, 12.0, lx + pad, ly + pad, widgets::FG);

        // Hint, centred at the top of the monitor under the pointer.
        let hint = tr(Msg::SnipHint);
        let (hw, hh) = p.text_size(hint, 13.0);
        let hint_box = Rect::new(
            (monitor.x + (monitor.w - hw) / 2.0 - pad * 2.0).round(),
            monitor.y + p.px(16.0),
            hw + 4.0 * pad,
            hh + 2.0 * pad,
        );
        p.fill(hint_box, 8.0, widgets::PANEL);
        p.text(
            hint,
            13.0,
            hint_box.x + 2.0 * pad,
            hint_box.y + pad,
            widgets::FG,
        );

        // Magnifier beside the pointer, on the side with room.
        let (cx, cy) = (self.cursor.0.floor(), self.cursor.1.floor());
        let side = (LOUPE_SRC * LOUPE_ZOOM * scale.max(1.0)).round();
        let zoom = side / LOUPE_SRC;
        let gap = p.px(20.0);
        let mut x = cx + gap;
        let mut y = cy + gap;
        if x + side > monitor.right() {
            x = cx - gap - side;
        }
        if y + side > monitor.bottom() {
            y = cy - gap - side;
        }
        let half = (LOUPE_SRC / 2.0).floor();
        let src = Rect::new(cx - half, cy - half, LOUPE_SRC, LOUPE_SRC);
        let loupe = Rect::new(x, y, side, side);
        let Some(content) = src.intersect(&bounds) else {
            return;
        };
        let xform = Affine::translate(-src.x, -src.y)
            .then(&Affine::scale(zoom, zoom))
            .then(&Affine::translate(x, y));
        p.fill(loupe, 0.0, Rgba(0.0, 0.0, 0.0, 1.0));
        p.canvas
            .draw_image(picture, &xform, &content, Rgba(0.0, 0.0, 0.0, 1.0), None);
        // The pixel under the pointer, and the frame.
        let px = Rect::new(x + half * zoom, y + half * zoom, zoom, zoom);
        p.outline(px, 1.0, 0.0, widgets::ACCENT);
        p.outline(loupe, 2.0, 0.0, widgets::BORDER);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip() -> Snip {
        // Two monitors side by side, two overlapping windows (front first).
        Snip::new(
            (3840, 1080),
            vec![
                Rect::new(0.0, 0.0, 1920.0, 1080.0),
                Rect::new(1920.0, 0.0, 1920.0, 1080.0),
            ],
            vec![
                Rect::new(100.0, 100.0, 400.0, 300.0),
                Rect::new(50.0, 50.0, 1000.0, 800.0),
                // Partly off the picture: clipped.
                Rect::new(3700.0, 900.0, 400.0, 400.0),
            ],
            (10.0, 10.0),
        )
    }

    #[test]
    fn drags_a_region_in_any_direction() {
        let mut s = snip();
        s.mouse_down((300.0, 400.0));
        assert_eq!(s.mouse_move((120.5, 200.4)), Outcome::Redraw);
        assert_eq!(
            s.mouse_up((120.5, 200.4)),
            Outcome::Done(Rect::new(121.0, 200.0, 179.0, 200.0))
        );
        // Dragging past the edge stops at the picture.
        s.mouse_down((3800.0, 1000.0));
        assert_eq!(
            s.mouse_up((5000.0, 2000.0)),
            Outcome::Done(Rect::new(3800.0, 1000.0, 40.0, 80.0))
        );
    }

    #[test]
    fn a_click_takes_the_frontmost_window_or_the_monitor() {
        let mut s = snip();
        s.mouse_down((200.0, 200.0));
        assert_eq!(
            s.mouse_up((201.0, 201.0)),
            Outcome::Done(Rect::new(100.0, 100.0, 400.0, 300.0))
        );
        s.mouse_down((900.0, 700.0));
        assert_eq!(
            s.mouse_up((900.0, 700.0)),
            Outcome::Done(Rect::new(50.0, 50.0, 1000.0, 800.0))
        );
        s.mouse_down((3000.0, 100.0));
        assert_eq!(
            s.mouse_up((3000.0, 100.0)),
            Outcome::Done(Rect::new(1920.0, 0.0, 1920.0, 1080.0))
        );
        // The clipped window.
        s.mouse_down((3800.0, 1000.0));
        assert_eq!(
            s.mouse_up((3800.0, 1000.0)),
            Outcome::Done(Rect::new(3700.0, 900.0, 140.0, 180.0))
        );
    }

    #[test]
    fn enter_takes_the_monitor_escape_cancels() {
        let mut s = snip();
        s.mouse_move((2500.0, 500.0));
        assert_eq!(
            s.key(Key::Enter, false),
            Outcome::Done(Rect::new(1920.0, 0.0, 1920.0, 1080.0))
        );
        // Escape first drops a drag, then cancels.
        s.mouse_down((10.0, 10.0));
        s.mouse_move((50.0, 50.0));
        assert_eq!(s.key(Key::Escape, false), Outcome::Redraw);
        assert_eq!(s.key(Key::Escape, false), Outcome::Cancel);
    }

    #[test]
    fn arrows_move_the_pointer_and_enter_finishes_a_drag() {
        let mut s = snip();
        s.mouse_move((600.0, 600.0));
        s.mouse_down((600.0, 600.0));
        s.key(Key::Right, true);
        s.key(Key::Down, false);
        assert_eq!(s.cursor(), (610.0, 601.0));
        s.key(Key::Right, false);
        for _ in 0..5 {
            s.key(Key::Down, false);
        }
        assert_eq!(
            s.key(Key::Enter, false),
            Outcome::Done(Rect::new(600.0, 600.0, 11.0, 6.0))
        );
        // The pointer stays inside the picture.
        s.mouse_move((0.0, 0.0));
        s.key(Key::Left, true);
        assert_eq!(s.cursor(), (0.0, 0.0));
    }
}
