//! A popup menu drawn by the UI itself, for platforms without native
//! popup menus in our window (Linux): the menu tree from [`crate::menu`]
//! with submenus, check marks, shortcuts, disabled items, mouse and
//! keyboard, mirrored for right-to-left languages.

use blinzel_core::geom::Rect;

use crate::backend::{Canvas, Key};
use crate::gfx::{Rgba, Shape};
use crate::menu::{Cmd, MenuItem};

/// Logical sizes (multiplied by the DPI scale).
const ROW: f64 = 28.0;
const SEPARATOR: f64 = 9.0;
const PAD: f64 = 6.0;
const CHECK: f64 = 26.0;
const GAP: f64 = 32.0;
const ARROW: f64 = 18.0;
const TEXT: f64 = 13.0;
const RADIUS: f64 = 8.0;

const BG: Rgba = Rgba(0.16, 0.16, 0.17, 0.98);
const BORDER: Rgba = Rgba(1.0, 1.0, 1.0, 0.10);
const HOVER: Rgba = Rgba(1.0, 1.0, 1.0, 0.09);
const FG: Rgba = Rgba(0.93, 0.93, 0.93, 1.0);
const DIM: Rgba = Rgba(0.93, 0.93, 0.93, 0.38);
const SHORTCUT: Rgba = Rgba(0.93, 0.93, 0.93, 0.55);

/// What an input did to the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing visible changed.
    Unchanged,
    Redraw,
    /// The menu closed without a choice.
    Closed,
    Chosen(Cmd),
}

struct Level {
    items: Vec<MenuItem>,
    rect: Rect,
    /// Row rectangles, one per item (separators included).
    rows: Vec<Rect>,
    hover: Option<usize>,
}

pub struct Popup {
    levels: Vec<Level>,
    scale: f64,
    client: (f64, f64),
    rtl: bool,
}

fn label(item: &MenuItem) -> Option<(&str, Option<&str>, bool, bool)> {
    match item {
        MenuItem::Item {
            label,
            shortcut,
            checked,
            enabled,
            ..
        } => Some((label, shortcut.as_deref(), *checked, *enabled)),
        MenuItem::Sub(label, _) => Some((label, None, false, true)),
        MenuItem::Separator => None,
    }
}

fn selectable(item: &MenuItem) -> bool {
    matches!(
        item,
        MenuItem::Item { enabled: true, .. } | MenuItem::Sub(..)
    )
}

impl Popup {
    /// Opens `items` at the client point `at` (below and right of it,
    /// left of it in right-to-left languages), kept inside the client
    /// area.
    pub fn open(
        items: Vec<MenuItem>,
        at: (f64, f64),
        client: (f64, f64),
        scale: f64,
        rtl: bool,
        canvas: &mut impl Canvas,
    ) -> Popup {
        let mut p = Popup {
            levels: Vec::new(),
            scale,
            client,
            rtl,
        };
        p.push(items, at, canvas);
        p
    }

    fn px(&self) -> f32 {
        (TEXT * self.scale) as f32
    }

    /// Lays out `items` with its corner at `at` and opens it as the top
    /// level.
    fn push(&mut self, items: Vec<MenuItem>, at: (f64, f64), canvas: &mut impl Canvas) {
        let s = self.scale;
        let px = self.px();
        let (mut text_w, mut short_w) = (0f64, 0f64);
        for it in &items {
            if let Some((l, sc, _, _)) = label(it) {
                if let Some(size) = canvas.text_size(l, px, 2000.0) {
                    text_w = text_w.max(size.0 as f64);
                }
                if let Some(size) = sc.and_then(|sc| canvas.text_size(sc, px, 2000.0)) {
                    short_w = short_w.max(size.0 as f64);
                }
            }
        }
        let w = (CHECK * s + text_w + GAP * s + short_w + ARROW * s).max(160.0 * s);
        let h: f64 = items
            .iter()
            .map(|i| if label(i).is_some() { ROW } else { SEPARATOR } * s)
            .sum::<f64>()
            + 2.0 * PAD * s;
        let x = if self.rtl { at.0 - w } else { at.0 };
        let x = x.clamp(0.0, (self.client.0 - w).max(0.0));
        let y = at.1.clamp(0.0, (self.client.1 - h).max(0.0));
        let rect = Rect::new(x.round(), y.round(), w.round(), h.round());
        let mut rows = Vec::with_capacity(items.len());
        let mut ry = rect.y + PAD * s;
        for it in &items {
            let rh = if label(it).is_some() { ROW } else { SEPARATOR } * s;
            rows.push(Rect::new(rect.x + PAD * s, ry, rect.w - 2.0 * PAD * s, rh));
            ry += rh;
        }
        self.levels.push(Level {
            items,
            rect,
            rows,
            hover: None,
        });
    }

    /// Opens the submenu of the hovered item of the top level, if it has
    /// one.
    fn open_sub(&mut self, canvas: &mut impl Canvas) -> bool {
        let Some(top) = self.levels.last() else {
            return false;
        };
        let Some(i) = top.hover else { return false };
        let MenuItem::Sub(_, sub) = &top.items[i] else {
            return false;
        };
        let (sub, row, rect) = (sub.clone(), top.rows[i], top.rect);
        let x = if self.rtl { rect.x } else { rect.right() };
        self.push(sub, (x, row.y - PAD * self.scale), canvas);
        // The first selectable item, as keyboard users expect.
        let top = self.levels.last_mut().unwrap();
        top.hover = top.items.iter().position(selectable);
        true
    }

    /// The level and row under `p`.
    fn hit(&self, p: (f64, f64)) -> Option<(usize, Option<usize>)> {
        self.levels.iter().enumerate().rev().find_map(|(l, level)| {
            level.rect.contains(p).then(|| {
                let row = level
                    .rows
                    .iter()
                    .position(|r| r.contains(p))
                    .filter(|&i| label(&level.items[i]).is_some());
                (l, row)
            })
        })
    }

    pub fn mouse_move(&mut self, p: (f64, f64), canvas: &mut impl Canvas) -> Outcome {
        let Some((l, row)) = self.hit(p) else {
            return Outcome::Unchanged;
        };
        let before = (self.levels.len(), self.levels[l].hover);
        self.levels.truncate(l + 1);
        self.levels[l].hover = row.filter(|&i| selectable(&self.levels[l].items[i]));
        if self.levels[l].hover.is_some() {
            self.open_sub(canvas);
        }
        if (self.levels.len(), self.levels[l].hover) == before {
            Outcome::Unchanged
        } else {
            Outcome::Redraw
        }
    }

    /// A press: chooses an item, or closes the menu outside it.
    pub fn mouse_down(&mut self, p: (f64, f64), canvas: &mut impl Canvas) -> Outcome {
        match self.hit(p) {
            None => Outcome::Closed,
            Some((l, Some(i))) => {
                self.levels.truncate(l + 1);
                self.levels[l].hover = Some(i);
                self.activate(canvas)
            }
            Some(_) => Outcome::Unchanged,
        }
    }

    fn activate(&mut self, canvas: &mut impl Canvas) -> Outcome {
        let Some(top) = self.levels.last() else {
            return Outcome::Closed;
        };
        let Some(i) = top.hover else {
            return Outcome::Unchanged;
        };
        match &top.items[i] {
            MenuItem::Item {
                cmd, enabled: true, ..
            } => Outcome::Chosen(*cmd),
            MenuItem::Sub(..) => {
                if self.open_sub(canvas) {
                    Outcome::Redraw
                } else {
                    Outcome::Unchanged
                }
            }
            _ => Outcome::Unchanged,
        }
    }

    pub fn key(&mut self, key: Key, canvas: &mut impl Canvas) -> Outcome {
        let (forward, back) = if self.rtl {
            (Key::Left, Key::Right)
        } else {
            (Key::Right, Key::Left)
        };
        match key {
            Key::Escape => {
                self.levels.pop();
                if self.levels.is_empty() {
                    Outcome::Closed
                } else {
                    Outcome::Redraw
                }
            }
            k if k == back && self.levels.len() > 1 => {
                self.levels.pop();
                Outcome::Redraw
            }
            k if k == forward => {
                if self.open_sub(canvas) {
                    Outcome::Redraw
                } else {
                    Outcome::Unchanged
                }
            }
            Key::Up | Key::Down => {
                let top = self.levels.last_mut().unwrap();
                let n = top.items.len();
                let step = if key == Key::Down { 1 } else { n - 1 };
                let start = top
                    .hover
                    .unwrap_or(if key == Key::Down { n - 1 } else { 0 });
                let mut i = start;
                for _ in 0..n {
                    i = (i + step) % n;
                    if selectable(&top.items[i]) {
                        top.hover = Some(i);
                        return Outcome::Redraw;
                    }
                }
                Outcome::Unchanged
            }
            Key::Enter | Key::Space => self.activate(canvas),
            _ => Outcome::Unchanged,
        }
    }

    pub fn draw(&self, canvas: &mut impl Canvas) {
        let s = self.scale;
        let px = self.px();
        for level in &self.levels {
            canvas.draw_shapes(&[
                Shape::Fill {
                    rect: level.rect,
                    radius: (RADIUS * s) as f32,
                    color: BG,
                },
                Shape::Outline {
                    rect: level.rect,
                    thickness: 1.0,
                    radius: (RADIUS * s) as f32,
                    color: BORDER,
                },
            ]);
            for (i, (item, row)) in level.items.iter().zip(&level.rows).enumerate() {
                let Some((text, shortcut, checked, enabled)) = label(item) else {
                    let y = (row.y + row.h / 2.0).round() as f32 + 0.5;
                    canvas.draw_shapes(&[Shape::Line {
                        a: (row.x as f32 + 4.0, y),
                        b: (row.right() as f32 - 4.0, y),
                        thickness: 1.0,
                        color: BORDER,
                    }]);
                    continue;
                };
                if level.hover == Some(i) {
                    canvas.draw_shapes(&[Shape::Fill {
                        rect: *row,
                        radius: (4.0 * s) as f32,
                        color: HOVER,
                    }]);
                }
                let color = if enabled { FG } else { DIM };
                // Mirrored x for right-to-left: measured from the far edge.
                let at = |dx: f64, w: f64| {
                    if self.rtl {
                        row.right() - dx - w
                    } else {
                        row.x + dx
                    }
                };
                let middle = row.y + row.h / 2.0;
                if checked {
                    let cx = at(CHECK * s / 2.0, 0.0);
                    let k = 4.0 * s;
                    let line = |a: (f64, f64), b: (f64, f64)| Shape::Line {
                        a: (a.0 as f32, a.1 as f32),
                        b: (b.0 as f32, b.1 as f32),
                        thickness: (1.6 * s) as f32,
                        color,
                    };
                    canvas.draw_shapes(&[
                        line((cx - k, middle), (cx - k / 3.0, middle + k * 0.7)),
                        line((cx - k / 3.0, middle + k * 0.7), (cx + k, middle - k * 0.8)),
                    ]);
                }
                if let Some(size) = canvas.text_size(text, px, 2000.0) {
                    let x = at(CHECK * s, size.0 as f64);
                    let y = (middle - size.1 as f64 / 2.0).round();
                    canvas.draw_text(text, px, 2000.0, (x.round() as f32, y as f32), color);
                }
                if let Some(sc) = shortcut
                    && let Some(size) = canvas.text_size(sc, px, 2000.0)
                {
                    let x = at(row.w - ARROW * s - size.0 as f64, size.0 as f64);
                    let y = (middle - size.1 as f64 / 2.0).round();
                    let c = if enabled { SHORTCUT } else { DIM };
                    canvas.draw_text(sc, px, 2000.0, (x.round() as f32, y as f32), c);
                }
                if matches!(item, MenuItem::Sub(..)) {
                    let k = 3.5 * s;
                    let tip = at(row.w - ARROW * s / 2.0, 0.0);
                    let dir = if self.rtl { -1.0 } else { 1.0 };
                    let line = |a: (f64, f64), b: (f64, f64)| Shape::Line {
                        a: (a.0 as f32, a.1 as f32),
                        b: (b.0 as f32, b.1 as f32),
                        thickness: (1.4 * s) as f32,
                        color,
                    };
                    canvas.draw_shapes(&[
                        line(
                            (tip - dir * k / 2.0, middle - k),
                            (tip + dir * k / 2.0, middle),
                        ),
                        line(
                            (tip + dir * k / 2.0, middle),
                            (tip - dir * k / 2.0, middle + k),
                        ),
                    ]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Device;
    use crate::gfx::{AtlasPages, ImageInfo, Placed};
    use blinzel_core::geom::{Affine, Orientation};

    /// A canvas that only measures: 7 px per character, 16 px high.
    struct Measure;
    struct NoDevice;
    struct NoImage;

    impl ImageInfo for NoImage {
        fn size(&self) -> (u32, u32) {
            (1, 1)
        }
        fn full_size(&self) -> (u32, u32) {
            (1, 1)
        }
        fn orientation(&self) -> Orientation {
            Orientation::NORMAL
        }
        fn bytes(&self) -> usize {
            0
        }
    }

    impl Device for NoDevice {
        type Image = NoImage;
        type Lut = ();
        fn upload(
            &self,
            _: &blinzel_core::pixel::Image,
            _: Orientation,
        ) -> Result<NoImage, String> {
            Ok(NoImage)
        }
        fn upload_lut(&self, _: &blinzel_core::lut::Lut3d) -> Result<(), String> {
            Ok(())
        }
        fn texture_budget(&self) -> usize {
            0
        }
        fn adapter_name(&self) -> String {
            String::new()
        }
        fn is_software(&self) -> bool {
            true
        }
    }

    impl AtlasPages for Measure {
        fn add_page(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn upload(
            &mut self,
            _: usize,
            _: (u32, u32),
            _: (u32, u32),
            _: (u32, u32),
            _: &blinzel_core::thumb::Planes,
        ) {
        }
    }

    impl Canvas for Measure {
        type Device = NoDevice;
        fn begin(&mut self, _: Rgba) -> bool {
            true
        }
        fn draw_image(&mut self, _: &NoImage, _: &Affine, _: &Rect, _: Rgba, _: Option<&()>) {}
        fn draw_shapes(&mut self, _: &[Shape]) {}
        fn draw_thumb(&mut self, _: &Placed, _: &Rect, _: f32) {}
        fn text_size(&mut self, text: &str, _: f32, _: f32) -> Option<(u32, u32)> {
            Some((7 * text.chars().count() as u32, 16))
        }
        fn draw_text(&mut self, _: &str, _: f32, _: f32, _: (f32, f32), _: Rgba) {}
        fn present(&mut self) {}
        fn resize(&mut self, _: (u32, u32)) {}
    }

    fn item(cmd: Cmd, label: &str, enabled: bool) -> MenuItem {
        MenuItem::Item {
            cmd,
            label: label.into(),
            shortcut: None,
            checked: false,
            enabled,
        }
    }

    fn menu() -> Vec<MenuItem> {
        vec![
            item(Cmd::Open, "Open", true),
            MenuItem::Separator,
            item(Cmd::Copy, "Copy", false),
            MenuItem::Sub(
                "Zoom".into(),
                vec![
                    item(Cmd::ZoomIn, "In", true),
                    item(Cmd::ZoomOut, "Out", true),
                ],
            ),
            item(Cmd::Exit, "Exit", true),
        ]
    }

    #[test]
    fn keyboard_skips_separators_and_disabled_items() {
        let c = &mut Measure;
        let mut p = Popup::open(menu(), (10.0, 10.0), (800.0, 600.0), 1.0, false, c);
        assert_eq!(p.key(Key::Down, c), Outcome::Redraw);
        assert_eq!(p.levels[0].hover, Some(0));
        p.key(Key::Down, c);
        assert_eq!(p.levels[0].hover, Some(3), "separator and disabled skipped");
        assert_eq!(p.key(Key::Right, c), Outcome::Redraw);
        assert_eq!(p.levels.len(), 2);
        p.key(Key::Down, c);
        assert_eq!(p.key(Key::Enter, c), Outcome::Chosen(Cmd::ZoomOut));
        assert_eq!(p.key(Key::Left, c), Outcome::Redraw);
        assert_eq!(p.key(Key::Escape, c), Outcome::Closed);
    }

    #[test]
    fn mouse_opens_submenus_and_chooses() {
        let c = &mut Measure;
        let mut p = Popup::open(menu(), (10.0, 10.0), (800.0, 600.0), 1.0, false, c);
        let zoom = p.levels[0].rows[3];
        let at = |r: Rect| (r.x + 5.0, r.y + r.h / 2.0);
        assert_eq!(p.mouse_move(at(zoom), c), Outcome::Redraw);
        assert_eq!(p.levels.len(), 2);
        let sub = p.levels[1].rect;
        assert!(
            sub.x >= p.levels[0].rect.right() - 1.0,
            "opens to the right"
        );
        let zin = p.levels[1].rows[0];
        assert_eq!(p.mouse_down(at(zin), c), Outcome::Chosen(Cmd::ZoomIn));
        let copy = p.levels[0].rows[2];
        assert_eq!(p.mouse_down(at(copy), c), Outcome::Unchanged, "disabled");
        assert_eq!(p.mouse_down((700.0, 500.0), c), Outcome::Closed);
    }

    #[test]
    fn stays_inside_and_mirrors_for_rtl() {
        let c = &mut Measure;
        let p = Popup::open(menu(), (790.0, 590.0), (800.0, 600.0), 1.0, false, c);
        let r = p.levels[0].rect;
        assert!(r.right() <= 800.0 && r.bottom() <= 600.0);
        let p = Popup::open(menu(), (400.0, 10.0), (800.0, 600.0), 1.0, true, c);
        assert!(
            (p.levels[0].rect.right() - 400.0).abs() < 1.0,
            "opens leftwards"
        );
    }
}
