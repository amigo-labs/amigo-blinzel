//! The only visible UI: a hamburger button top left and minimize /
//! maximize / close top right, drawn straight over the image (no bar).
//! Right-to-left languages mirror both sides. On macOS the system draws its
//! window buttons top left, so the menu button moves to the other side.
//! Pure geometry; icons are SDF shapes, so the first frame needs no font.

use blinzel_core::geom::Rect;

use crate::gfx::{Rgba, Shape};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Menu,
    Minimize,
    Maximize,
    Close,
}

/// Button size in DIPs (Windows 11 caption buttons are 46 × 32).
const BUTTON_W: f64 = 46.0;
const BUTTON_H: f64 = 32.0;

#[derive(Clone, Copy, Debug)]
pub struct Chrome {
    /// Physical pixels per DIP.
    pub scale: f64,
    pub client: (f64, f64),
    /// Mirrored for right-to-left languages.
    pub rtl: bool,
    /// The system draws minimize / maximize / close (macOS).
    pub system_buttons: bool,
}

impl Chrome {
    pub fn caption_height(&self) -> f64 {
        BUTTON_H * self.scale
    }

    pub fn rect(&self, b: Button) -> Rect {
        let (w, h) = (BUTTON_W * self.scale, BUTTON_H * self.scale);
        let right = |i: f64| Rect::new(self.client.0 - w * i, 0.0, w, h);
        let r = match b {
            Button::Menu if self.system_buttons => right(1.0),
            Button::Menu => Rect::new(0.0, 0.0, w, h),
            _ if self.system_buttons => Rect::new(0.0, 0.0, 0.0, 0.0),
            Button::Close => right(1.0),
            Button::Maximize => right(2.0),
            Button::Minimize => right(3.0),
        };
        if self.rtl {
            Rect::new(self.client.0 - r.right(), r.y, r.w, r.h)
        } else {
            r
        }
    }

    pub fn hit(&self, p: (f64, f64)) -> Option<Button> {
        [
            Button::Menu,
            Button::Close,
            Button::Maximize,
            Button::Minimize,
        ]
        .into_iter()
        .find(|&b| self.rect(b).contains(p))
    }

    /// Shapes for all buttons. Icons get a dark halo so they stay visible
    /// on bright images.
    pub fn shapes(
        &self,
        hover: Option<Button>,
        pressed: Option<Button>,
        maximized: bool,
    ) -> Vec<Shape> {
        let mut out = Vec::new();
        let s = self.scale as f32;
        for b in [
            Button::Menu,
            Button::Minimize,
            Button::Maximize,
            Button::Close,
        ] {
            if self.system_buttons && b != Button::Menu {
                continue;
            }
            let r = self.rect(b);
            if hover == Some(b) || pressed == Some(b) {
                let color = match (b, pressed == Some(b)) {
                    (Button::Close, false) => Rgba(0.769, 0.169, 0.110, 1.0),
                    (Button::Close, true) => Rgba(0.698, 0.157, 0.102, 1.0),
                    (_, false) => Rgba(1.0, 1.0, 1.0, 0.14),
                    (_, true) => Rgba(1.0, 1.0, 1.0, 0.08),
                };
                out.push(Shape::Fill {
                    rect: r,
                    radius: 0.0,
                    color,
                });
            }
            let (cx, cy) = ((r.x + r.w / 2.0) as f32, (r.y + r.h / 2.0) as f32);
            let icon = icon(b, cx, cy, s, maximized);
            let halo = Rgba(0.0, 0.0, 0.0, if hover == Some(b) { 0.0 } else { 0.55 });
            for g in &icon {
                out.push(with(g, 1.6 * s, halo));
            }
            for g in &icon {
                out.push(with(g, 0.0, Rgba(1.0, 1.0, 1.0, 0.95)));
            }
        }
        out
    }
}

/// Icon strokes before colouring: lines and squares.
enum Glyph {
    Line((f32, f32), (f32, f32), f32),
    Square(Rect, f32),
}

fn icon(b: Button, cx: f32, cy: f32, s: f32, maximized: bool) -> Vec<Glyph> {
    let line = |x0: f32, y0: f32, x1: f32, y1: f32, t: f32| {
        Glyph::Line(
            (cx + x0 * s, cy + y0 * s),
            (cx + x1 * s, cy + y1 * s),
            t * s,
        )
    };
    let square = |x: f32, y: f32, w: f32, t: f32| {
        Glyph::Square(
            Rect::new(
                (cx + x * s) as f64,
                (cy + y * s) as f64,
                (w * s) as f64,
                (w * s) as f64,
            ),
            t * s,
        )
    };
    match b {
        Button::Menu => vec![
            line(-8.0, -5.0, 8.0, -5.0, 1.5),
            line(-8.0, 0.0, 8.0, 0.0, 1.5),
            line(-8.0, 5.0, 8.0, 5.0, 1.5),
        ],
        Button::Minimize => vec![line(-5.0, 0.0, 5.0, 0.0, 1.0)],
        Button::Maximize if maximized => vec![
            square(-5.0, -3.0, 8.0, 1.0),
            line(-3.0, -5.0, 5.0, -5.0, 1.0),
            line(5.0, -5.0, 5.0, 3.0, 1.0),
        ],
        Button::Maximize => vec![square(-5.0, -5.0, 10.0, 1.0)],
        Button::Close => vec![
            line(-5.0, -5.0, 5.0, 5.0, 1.1),
            line(-5.0, 5.0, 5.0, -5.0, 1.1),
        ],
    }
}

fn with(g: &Glyph, extra: f32, color: Rgba) -> Shape {
    match *g {
        Glyph::Line(a, b, t) => Shape::Line {
            a,
            b,
            thickness: t + extra,
            color,
        },
        Glyph::Square(r, t) => Shape::Outline {
            rect: r,
            thickness: t + extra,
            radius: 0.0,
            color,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_hits() {
        let c = Chrome {
            scale: 1.5,
            client: (1000.0, 600.0),
            rtl: false,
            system_buttons: false,
        };
        assert_eq!(
            c.rect(Button::Close),
            Rect::new(1000.0 - 69.0, 0.0, 69.0, 48.0)
        );
        assert_eq!(c.hit((10.0, 10.0)), Some(Button::Menu));
        assert_eq!(c.hit((990.0, 10.0)), Some(Button::Close));
        assert_eq!(c.hit((1000.0 - 100.0, 10.0)), Some(Button::Maximize));
        assert_eq!(c.hit((500.0, 10.0)), None);
        assert_eq!(c.hit((990.0, 100.0)), None);
        assert!(!c.shapes(Some(Button::Close), None, false).is_empty());
    }

    #[test]
    fn right_to_left_mirrors() {
        let c = Chrome {
            scale: 1.0,
            client: (1000.0, 600.0),
            rtl: true,
            system_buttons: false,
        };
        assert_eq!(c.rect(Button::Close), Rect::new(0.0, 0.0, 46.0, 32.0));
        assert_eq!(c.rect(Button::Menu), Rect::new(954.0, 0.0, 46.0, 32.0));
        assert_eq!(c.hit((990.0, 10.0)), Some(Button::Menu));
        assert_eq!(c.hit((60.0, 10.0)), Some(Button::Maximize));
    }

    #[test]
    fn system_buttons_leave_the_menu_alone() {
        let c = Chrome {
            scale: 1.0,
            client: (1000.0, 600.0),
            rtl: false,
            system_buttons: true,
        };
        assert_eq!(c.rect(Button::Menu), Rect::new(954.0, 0.0, 46.0, 32.0));
        assert_eq!(c.hit((990.0, 10.0)), Some(Button::Menu));
        assert_eq!(c.hit((10.0, 10.0)), None);
        assert_eq!(c.hit((920.0, 10.0)), None);
        // Only the menu icon: three lines with halos.
        assert_eq!(c.shapes(None, None, false).len(), 6);
    }
}
