//! Controls drawn by the UI itself (the batch dialog): one look on every
//! platform, dark like the menus. Each draw call returns the rectangle it
//! used, so a dialog can lay itself out from top to bottom and remember
//! where to hit-test.

use blinzel_core::geom::Rect;

use crate::backend::Canvas;
use crate::gfx::{Rgba, Shape};

/// Dims the viewer behind a modal panel.
pub const SCRIM: Rgba = Rgba(0.0, 0.0, 0.0, 0.45);
pub const PANEL: Rgba = Rgba(0.15, 0.15, 0.16, 0.985);
pub const BORDER: Rgba = Rgba(1.0, 1.0, 1.0, 0.10);
pub const FG: Rgba = Rgba(0.93, 0.93, 0.93, 1.0);
pub const DIM: Rgba = Rgba(0.93, 0.93, 0.93, 0.55);
pub const FAINT: Rgba = Rgba(0.93, 0.93, 0.93, 0.32);
pub const CONTROL: Rgba = Rgba(1.0, 1.0, 1.0, 0.07);
pub const CONTROL_HOVER: Rgba = Rgba(1.0, 1.0, 1.0, 0.12);
pub const ACCENT: Rgba = Rgba(0.35, 0.62, 1.0, 1.0);
pub const ACCENT_FILL: Rgba = Rgba(0.35, 0.62, 1.0, 0.85);
pub const DANGER: Rgba = Rgba(1.0, 0.45, 0.42, 1.0);

/// Drawing with a DPI scale and a font size.
pub struct Painter<'a, C: Canvas> {
    pub canvas: &'a mut C,
    pub scale: f64,
}

impl<C: Canvas> Painter<'_, C> {
    pub fn px(&self, logical: f64) -> f64 {
        (logical * self.scale).round()
    }

    fn font(&self, logical: f64) -> f32 {
        (logical * self.scale) as f32
    }

    pub fn text_size(&mut self, text: &str, size: f64) -> (f64, f64) {
        let f = self.font(size);
        self.canvas
            .text_size(text, f, 4000.0)
            .map_or((0.0, 0.0), |(w, h)| (w as f64, h as f64))
    }

    /// Text with its top-left at `(x, y)`; returns its size.
    pub fn text(&mut self, text: &str, size: f64, x: f64, y: f64, color: Rgba) -> (f64, f64) {
        let f = self.font(size);
        let s = self.text_size(text, size);
        self.canvas
            .draw_text(text, f, 4000.0, (x.round() as f32, y.round() as f32), color);
        s
    }

    /// Text centred vertically in `r`, from its left edge (plus `pad`).
    pub fn text_in(&mut self, text: &str, size: f64, r: &Rect, pad: f64, color: Rgba) {
        let (_, h) = self.text_size(text, size);
        self.text(text, size, r.x + pad, r.y + (r.h - h) / 2.0, color);
    }

    /// Text centred in `r`.
    pub fn text_centred(&mut self, text: &str, size: f64, r: &Rect, color: Rgba) {
        let (w, h) = self.text_size(text, size);
        self.text(
            text,
            size,
            r.x + (r.w - w) / 2.0,
            r.y + (r.h - h) / 2.0,
            color,
        );
    }

    pub fn fill(&mut self, rect: Rect, radius: f64, color: Rgba) {
        self.canvas.draw_shapes(&[Shape::Fill {
            rect,
            radius: (radius * self.scale) as f32,
            color,
        }]);
    }

    pub fn outline(&mut self, rect: Rect, thickness: f64, radius: f64, color: Rgba) {
        self.canvas.draw_shapes(&[Shape::Outline {
            rect,
            thickness: (thickness * self.scale) as f32,
            radius: (radius * self.scale) as f32,
            color,
        }]);
    }

    /// A row of options of which one is chosen; returns each option's
    /// rectangle.
    pub fn segmented(
        &mut self,
        options: &[&str],
        chosen: usize,
        hover: Option<usize>,
        x: f64,
        y: f64,
        w: f64,
    ) -> Vec<Rect> {
        let h = self.px(30.0);
        let outer = Rect::new(x, y, w, h);
        self.fill(outer, 6.0, CONTROL);
        let n = options.len().max(1) as f64;
        let each = w / n;
        let mut out = Vec::with_capacity(options.len());
        for (i, o) in options.iter().enumerate() {
            let r = Rect::new((x + each * i as f64).round(), y, each.round(), h);
            let inner = Rect::new(r.x + 2.0, r.y + 2.0, r.w - 4.0, r.h - 4.0);
            if i == chosen {
                self.fill(inner, 5.0, ACCENT_FILL);
            } else if hover == Some(i) {
                self.fill(inner, 5.0, CONTROL_HOVER);
            }
            self.text_centred(o, 12.5, &r, if i == chosen { FG } else { DIM });
            out.push(r);
        }
        out
    }

    /// A check box with its label; returns the clickable area.
    pub fn checkbox(&mut self, label: &str, on: bool, enabled: bool, x: f64, y: f64) -> Rect {
        let b = self.px(16.0);
        let box_r = Rect::new(x, y + self.px(3.0), b, b);
        if on && enabled {
            self.fill(box_r, 4.0, ACCENT_FILL);
            let s = self.scale;
            let (cx, cy) = (box_r.x + b / 2.0, box_r.y + b / 2.0);
            let line = |a: (f64, f64), z: (f64, f64)| Shape::Line {
                a: (a.0 as f32, a.1 as f32),
                b: (z.0 as f32, z.1 as f32),
                thickness: (1.8 * s) as f32,
                color: FG,
            };
            let k = 3.6 * s;
            self.canvas.draw_shapes(&[
                line((cx - k, cy), (cx - k / 3.0, cy + k * 0.7)),
                line((cx - k / 3.0, cy + k * 0.7), (cx + k, cy - k * 0.8)),
            ]);
        } else {
            self.outline(box_r, 1.2, 4.0, if enabled { DIM } else { FAINT });
        }
        let (_, lh) = self.text_size(label, 13.0);
        let (tx, ty) = (box_r.right() + self.px(8.0), y + (self.px(22.0) - lh) / 2.0);
        let (tw, th) = self.text(label, 13.0, tx, ty, if enabled { FG } else { FAINT });
        Rect::new(x, y, b + self.px(8.0) + tw, self.px(22.0).max(th))
    }

    /// A one-line text field.
    pub fn field(&mut self, text: &str, focused: bool, invalid: bool, r: Rect) {
        self.fill(r, 5.0, CONTROL);
        let border = if invalid {
            DANGER
        } else if focused {
            ACCENT
        } else {
            BORDER
        };
        self.outline(r, if focused { 1.5 } else { 1.0 }, 5.0, border);
        let pad = self.px(8.0);
        self.text_in(text, 13.0, &r, pad, FG);
        if focused {
            let (w, _) = self.text_size(text, 13.0);
            let x = (r.x + pad + w + 1.0).min(r.right() - pad);
            let y0 = r.y + self.px(7.0);
            self.canvas.draw_shapes(&[Shape::Line {
                a: (x as f32, y0 as f32),
                b: (x as f32, (r.bottom() - self.px(7.0)) as f32),
                thickness: (1.2 * self.scale) as f32,
                color: FG,
            }]);
        }
    }

    /// A slider for `value` in `0..=1` with a track in `r`.
    pub fn slider(&mut self, value: f64, r: Rect) {
        let t = self.px(4.0);
        let track = Rect::new(r.x, r.y + (r.h - t) / 2.0, r.w, t);
        self.fill(track, 2.0, CONTROL_HOVER);
        let filled = Rect::new(track.x, track.y, track.w * value.clamp(0.0, 1.0), t);
        self.fill(filled, 2.0, ACCENT_FILL);
        let k = self.px(14.0);
        let knob = Rect::new(
            track.x + track.w * value.clamp(0.0, 1.0) - k / 2.0,
            r.y + (r.h - k) / 2.0,
            k,
            k,
        );
        self.fill(knob, 7.0, FG);
    }

    /// A push button; `primary` is filled with the accent colour.
    pub fn button(&mut self, label: &str, primary: bool, hover: bool, enabled: bool, r: Rect) {
        let fill = match (primary, hover, enabled) {
            (_, _, false) => CONTROL,
            (true, false, true) => ACCENT_FILL,
            (true, true, true) => ACCENT,
            (false, true, true) => CONTROL_HOVER,
            (false, false, true) => CONTROL,
        };
        self.fill(r, 6.0, fill);
        self.text_centred(label, 13.0, &r, if enabled { FG } else { FAINT });
    }

    /// The width a button needs for `label`.
    pub fn button_width(&mut self, label: &str) -> f64 {
        (self.text_size(label, 13.0).0 + self.px(32.0)).max(self.px(96.0))
    }

    /// A progress bar for `value` in `0..=1`.
    pub fn progress(&mut self, value: f64, r: Rect) {
        self.fill(r, r.h / 2.0 / self.scale, CONTROL_HOVER);
        let w = (r.w * value.clamp(0.0, 1.0)).max(r.h);
        self.fill(
            Rect::new(r.x, r.y, w, r.h),
            r.h / 2.0 / self.scale,
            ACCENT_FILL,
        );
    }
}
