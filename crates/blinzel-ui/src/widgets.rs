//! Controls drawn by the UI itself (the batch dialog): one look on every
//! platform, in the colours of the shared amigo theme ([`crate::palette`]). Each draw call returns the rectangle it
//! used, so a dialog can lay itself out from top to bottom and remember
//! where to hit-test.

use blinzel_core::geom::Rect;

use crate::backend::Canvas;
use crate::gfx::{Rgba, Shape};

pub use crate::palette::{
    ACCENT, ACCENT_HOVER, ACCENT_INK, ACCENT_SOLID, BORDER, BORDER_STRONG, CONTROL, CONTROL_HOVER,
    DANGER, DANGER_INK, DIM, FAINT, FG, FOCUS, ON_ACCENT, PANEL, SCRIM,
};
use crate::theme;

/// Height of a segment row, a text field and a button (logical pixels).
pub const CONTROL_H: f64 = theme::CONTROL_MD as f64;
/// Height of a check box row and of a list row.
pub const CHECK_ROW: f64 = 22.0;
/// The type scale's dialog title.
pub const TEXT_TITLE: f64 = theme::TEXT_TITLE.size as f64;
/// The type scale's body text: labels, controls, rows, buttons, status.
pub const TEXT_BODY: f64 = theme::TEXT_BODY_SM.size as f64;
/// The type scale's hints.
pub const TEXT_SMALL: f64 = theme::TEXT_CAPTION.size as f64;
/// The corner of a control: button, field, segmented control.
pub const RADIUS: f64 = theme::RADIUS_SM as f64;
/// The corner of what sits inside a control (a segment in its track) and
/// of a check box.
pub const RADIUS_INNER: f64 = theme::RADIUS_XS as f64;
/// The corner of a frame inside a panel (a list).
pub const RADIUS_FRAME: f64 = theme::RADIUS_MD as f64;
/// The corner of dialogs, panels and the popup menu.
pub const RADIUS_PANEL: f64 = theme::RADIUS_LG as f64;
/// Room left and right of a segment's label.
const SEGMENT_PAD: f64 = 12.0;
/// A segment's inset in its track.
const SEGMENT_INSET: f64 = 2.0;

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

    /// An outline drawn inside `rect` (inset by half its stroke), so its
    /// ink ends on the same pixel as a filled control of that rectangle.
    pub fn edge_outline(&mut self, rect: Rect, thickness: f64, radius: f64, color: Rgba) {
        let half = thickness * self.scale / 2.0;
        let inner = Rect::new(
            rect.x + half,
            rect.y + half,
            rect.w - 2.0 * half,
            rect.h - 2.0 * half,
        );
        self.outline(inner, thickness, (radius - thickness / 2.0).max(0.0), color);
    }

    /// Where [`Painter::segmented`] puts each option in a control at
    /// `(x, y)` that is `w` wide: each segment as wide as its label needs,
    /// the spare width shared out equally; options that do not fit one row
    /// wrap into rows with the same number of options, on one column grid
    /// (each column as wide as its widest label), so the rows line up.
    pub fn segment_rects(&mut self, options: &[&str], x: f64, y: f64, w: f64) -> Vec<Rect> {
        let pad = self.px(SEGMENT_PAD);
        let natural: Vec<f64> = options
            .iter()
            .map(|o| self.text_size(o, TEXT_BODY).0 + 2.0 * pad)
            .collect();
        let n = natural.len().max(1);
        let columns = |per: usize| -> Vec<f64> {
            (0..per)
                .map(|c| {
                    natural
                        .iter()
                        .skip(c)
                        .step_by(per)
                        .fold(0.0, |a: f64, &b| a.max(b))
                })
                .collect()
        };
        let per = (1..=n)
            .map(|rows| n.div_ceil(rows))
            .find(|&per| columns(per).iter().sum::<f64>() <= w)
            .unwrap_or(1);
        let cols = columns(per);
        let spare = (w - cols.iter().sum::<f64>()).max(0.0) / per as f64;
        // The column edges every row shares; the last ends at the right.
        let mut edges = vec![x];
        for c in &cols[..per - 1] {
            let last = *edges.last().unwrap_or(&x);
            edges.push((last + c + spare).round());
        }
        edges.push(x + w);
        let h = self.px(CONTROL_H);
        (0..natural.len())
            .map(|i| {
                let (row, col) = (i / per, i % per);
                // A short last row stretches its last option to the edge.
                let right = if i + 1 == natural.len() {
                    x + w
                } else {
                    edges[col + 1]
                };
                let left = edges[col];
                Rect::new(left, y + h * row as f64, right - left, h)
            })
            .collect()
    }

    /// A choice of one option ([`Painter::segment_rects`]); returns each
    /// option's rectangle.
    pub fn segmented(
        &mut self,
        options: &[&str],
        chosen: usize,
        hover: Option<usize>,
        x: f64,
        y: f64,
        w: f64,
    ) -> Vec<Rect> {
        let rects = self.segment_rects(options, x, y, w);
        let bottom = rects.last().map_or(y, |r| r.bottom());
        self.fill(Rect::new(x, y, w, bottom - y), RADIUS, CONTROL);
        let i2 = self.px(SEGMENT_INSET);
        for (i, (o, r)) in options.iter().zip(&rects).enumerate() {
            let inner = Rect::new(r.x + i2, r.y + i2, r.w - 2.0 * i2, r.h - 2.0 * i2);
            let ink = if i == chosen {
                self.fill(inner, RADIUS_INNER, ACCENT_SOLID);
                ON_ACCENT
            } else {
                if hover == Some(i) {
                    self.fill(inner, RADIUS_INNER, CONTROL_HOVER);
                }
                FG
            };
            self.text_centred(o, TEXT_BODY, r, ink);
        }
        rects
    }

    /// A check box with its label; returns the clickable area. A checked
    /// box is an accent fill; a disabled one keeps its check, muted.
    pub fn checkbox(&mut self, label: &str, on: bool, enabled: bool, x: f64, y: f64) -> Rect {
        let b = self.px(16.0);
        let box_r = Rect::new(x, y + self.px(3.0), b, b);
        if on && enabled {
            self.fill(box_r, RADIUS_INNER, ACCENT_SOLID);
        } else {
            self.fill(box_r, RADIUS_INNER, CONTROL);
            self.edge_outline(box_r, 1.0, RADIUS_INNER, BORDER_STRONG);
        }
        if on {
            let s = self.scale;
            let (cx, cy) = (box_r.x + b / 2.0, box_r.y + b / 2.0);
            let color = if enabled { ON_ACCENT } else { FAINT };
            let line = |a: (f64, f64), z: (f64, f64)| Shape::Line {
                a: (a.0 as f32, a.1 as f32),
                b: (z.0 as f32, z.1 as f32),
                thickness: (1.8 * s) as f32,
                color,
            };
            let k = 3.6 * s;
            self.canvas.draw_shapes(&[
                line((cx - k, cy), (cx - k / 3.0, cy + k * 0.7)),
                line((cx - k / 3.0, cy + k * 0.7), (cx + k, cy - k * 0.8)),
            ]);
        }
        let (_, lh) = self.text_size(label, TEXT_BODY);
        let row = self.px(CHECK_ROW);
        let (tx, ty) = (box_r.right() + self.px(8.0), y + (row - lh) / 2.0);
        // A disabled label stays readable (FAINT passes AA on the panel).
        let (tw, th) = self.text(label, TEXT_BODY, tx, ty, if enabled { FG } else { FAINT });
        Rect::new(x, y, b + self.px(8.0) + tw, row.max(th))
    }

    /// A one-line text field.
    pub fn field(&mut self, text: &str, focused: bool, invalid: bool, r: Rect) {
        self.fill(r, RADIUS, CONTROL);
        let (border, thick) = if invalid {
            (DANGER, 1.5)
        } else if focused {
            (FOCUS, 1.5)
        } else {
            (BORDER_STRONG, 1.0)
        };
        self.edge_outline(r, thick, RADIUS, border);
        let pad = self.px(8.0);
        self.text_in(text, TEXT_BODY, &r, pad, FG);
        if focused {
            let (w, _) = self.text_size(text, TEXT_BODY);
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
        self.fill(filled, 2.0, ACCENT_SOLID);
        let k = self.px(14.0);
        let knob = Rect::new(
            track.x + track.w * value.clamp(0.0, 1.0) - k / 2.0,
            r.y + (r.h - k) / 2.0,
            k,
            k,
        );
        self.fill(knob, 7.0, FG);
    }

    /// A push button; `primary` is an accent fill with the ink that reads
    /// on it, lighter under the pointer. A disabled button is a quiet
    /// control with a muted label, primary or not.
    pub fn button(&mut self, label: &str, primary: bool, hover: bool, enabled: bool, r: Rect) {
        let ink = match (enabled, primary) {
            (false, _) => {
                self.fill(r, RADIUS, CONTROL);
                self.edge_outline(r, 1.0, RADIUS, BORDER);
                FAINT
            }
            (true, true) => {
                self.fill(r, RADIUS, ACCENT_SOLID);
                if hover {
                    self.fill(r, RADIUS, ACCENT_HOVER);
                }
                ON_ACCENT
            }
            (true, false) => {
                self.fill(r, RADIUS, if hover { CONTROL_HOVER } else { CONTROL });
                self.edge_outline(r, 1.0, RADIUS, BORDER);
                FG
            }
        };
        self.text_centred(label, TEXT_BODY, &r, ink);
    }

    /// The width a button needs for `label`.
    pub fn button_width(&mut self, label: &str) -> f64 {
        (self.text_size(label, TEXT_BODY).0 + self.px(32.0)).max(self.px(96.0))
    }

    /// `text` shortened in the middle to fit `max_w`; the end (numbers,
    /// extension) stays readable.
    pub fn elide_middle(&mut self, text: &str, size: f64, max_w: f64) -> String {
        if self.text_size(text, size).0 <= max_w {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        // The last ten characters, from the word they start in if it
        // begins close before ("…Teil 1.png", not "…eil 1.png").
        let mut cut = chars.len().saturating_sub(10);
        if let Some(k) = (cut.saturating_sub(6)..cut)
            .rev()
            .find(|&k| chars[k] == ' ')
        {
            cut = k + 1;
        }
        let tail: String = chars[cut..].iter().collect();
        let (mut lo, mut hi) = (0, cut);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let s: String = chars[..mid].iter().collect::<String>() + "…" + &tail;
            if self.text_size(&s, size).0 <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        chars[..lo].iter().collect::<String>() + "…" + &tail
    }

    /// `text` cut at the end to fit `max_w` (a message reads from its
    /// start).
    pub fn elide_end(&mut self, text: &str, size: f64, max_w: f64) -> String {
        if self.text_size(text, size).0 <= max_w {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let s: String = chars[..mid].iter().collect::<String>() + "…";
            if self.text_size(&s, size).0 <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        chars[..lo].iter().collect::<String>() + "…"
    }

    /// A progress bar for `value` in `0..=1`.
    pub fn progress(&mut self, value: f64, r: Rect) {
        self.fill(r, r.h / 2.0 / self.scale, CONTROL_HOVER);
        let w = (r.w * value.clamp(0.0, 1.0)).max(r.h);
        self.fill(
            Rect::new(r.x, r.y, w, r.h),
            r.h / 2.0 / self.scale,
            ACCENT_SOLID,
        );
    }
}
