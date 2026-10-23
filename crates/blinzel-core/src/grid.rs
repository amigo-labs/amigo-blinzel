//! Thumbnail layouts: the grid browser (rows of square cells, scrolled
//! vertically) and the filmstrip (one row centred on the current image).
//! All values are client pixels.

use std::ops::Range;

use crate::geom::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub count: usize,
    pub cols: usize,
    /// Cell edge (square).
    pub cell: f64,
    pub gap: f64,
    /// Left edge of the first column (centres the columns).
    pub left: f64,
}

impl Grid {
    /// As many `cell`-sized columns as fit into `width`, centred.
    pub fn new(count: usize, width: f64, cell: f64, gap: f64) -> Grid {
        let cols = (((width - gap) / (cell + gap)).floor() as usize).max(1);
        let used = cols as f64 * (cell + gap) - gap;
        Grid {
            count,
            cols,
            cell,
            gap,
            left: ((width - used) / 2.0).max(0.0),
        }
    }

    pub fn rows(&self) -> usize {
        self.count.div_ceil(self.cols)
    }

    fn pitch(&self) -> f64 {
        self.cell + self.gap
    }

    /// Height of all rows, with a gap above and below.
    pub fn content_height(&self) -> f64 {
        self.rows() as f64 * self.pitch() + self.gap
    }

    pub fn max_scroll(&self, height: f64) -> f64 {
        (self.content_height() - height).max(0.0)
    }

    /// Cell `i` on screen at vertical scroll offset `scroll`.
    pub fn cell_rect(&self, i: usize, scroll: f64) -> Rect {
        let (row, col) = (i / self.cols, i % self.cols);
        Rect::new(
            self.left + col as f64 * self.pitch(),
            self.gap + row as f64 * self.pitch() - scroll,
            self.cell,
            self.cell,
        )
    }

    /// The cell under point `p`, if any.
    pub fn hit(&self, p: (f64, f64), scroll: f64) -> Option<usize> {
        let (x, y) = (p.0 - self.left, p.1 + scroll - self.gap);
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let (col, row) = ((x / self.pitch()) as usize, (y / self.pitch()) as usize);
        let inside =
            x - col as f64 * self.pitch() < self.cell && y - row as f64 * self.pitch() < self.cell;
        let i = row * self.cols + col;
        (inside && col < self.cols && i < self.count).then_some(i)
    }

    /// Items at least partly visible in a viewport of `height`.
    pub fn visible(&self, scroll: f64, height: f64) -> Range<usize> {
        // Row r spans gap + r·pitch .. (r + 1)·pitch.
        let first = (scroll / self.pitch()).floor().max(0.0) as usize;
        let last = ((scroll + height - self.gap) / self.pitch())
            .ceil()
            .max(0.0) as usize;
        (first * self.cols).min(self.count)..(last * self.cols).min(self.count)
    }

    /// The scroll offset closest to `scroll` that shows item `i` fully.
    pub fn reveal(&self, i: usize, scroll: f64, height: f64) -> f64 {
        let r = self.cell_rect(i, 0.0);
        let top = r.y - self.gap;
        let bottom = r.bottom() + self.gap - height;
        scroll
            .clamp(bottom.min(top), top)
            .clamp(0.0, self.max_scroll(height))
    }

    /// Full rows that fit into `height` (page up/down), at least one.
    pub fn page_rows(&self, height: f64) -> usize {
        ((height / self.pitch()).floor() as usize).max(1)
    }

    /// Item `i` moved by `dx` columns and `dy` rows, clamped to the items.
    pub fn step(&self, i: usize, dx: isize, dy: isize) -> usize {
        if self.count == 0 {
            return 0;
        }
        let j = i as isize + dx + dy * self.cols as isize;
        j.clamp(0, self.count as isize - 1) as usize
    }
}

/// The largest rect of `size`'s aspect ratio inside `cell`, centred; never
/// larger than `size` itself.
pub fn fit_in(cell: &Rect, size: (f64, f64)) -> Rect {
    let s = (cell.w / size.0).min(cell.h / size.1).min(1.0);
    let (w, h) = (size.0 * s, size.1 * s);
    Rect::new(
        (cell.x + (cell.w - w) / 2.0).round(),
        (cell.y + (cell.h - h) / 2.0).round(),
        w.round(),
        h.round(),
    )
}

/// The filmstrip: one row of `cell`-sized cells in a strip `width` wide,
/// the `current` item centred. Returns the visible items and their cells
/// (strip coordinates: y = 0 at the strip's top).
pub fn strip(count: usize, current: usize, width: f64, cell: f64, gap: f64) -> Vec<(usize, Rect)> {
    let pitch = cell + gap;
    let centre_x = width / 2.0 - cell / 2.0;
    let side = (width / 2.0 / pitch).ceil() as isize + 1;
    (-side..=side)
        .filter_map(|k| {
            let i = current as isize + k;
            (0..count as isize).contains(&i).then(|| {
                let x = centre_x + k as f64 * pitch;
                (i as usize, Rect::new(x, gap, cell, cell))
            })
        })
        .filter(|(_, r)| r.right() > 0.0 && r.x < width)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_rows_and_cells() {
        // (1000 - 10) / 170 = 5.8 → 5 columns of 160 + 10.
        let g = Grid::new(23, 1000.0, 160.0, 10.0);
        assert_eq!((g.cols, g.rows()), (5, 5));
        assert_eq!(g.left, (1000.0 - (5.0 * 170.0 - 10.0)) / 2.0);
        assert_eq!(g.cell_rect(0, 0.0), Rect::new(g.left, 10.0, 160.0, 160.0));
        assert_eq!(
            g.cell_rect(7, 50.0),
            Rect::new(g.left + 2.0 * 170.0, 10.0 + 170.0 - 50.0, 160.0, 160.0)
        );
        assert_eq!(g.content_height(), 5.0 * 170.0 + 10.0);
        // Narrow windows still get one column.
        assert_eq!(Grid::new(3, 50.0, 160.0, 10.0).cols, 1);
    }

    #[test]
    fn hit_testing() {
        let g = Grid::new(23, 1000.0, 160.0, 10.0);
        let c = g.cell_rect(7, 0.0);
        assert_eq!(g.hit((c.x + 1.0, c.y + 1.0), 0.0), Some(7));
        assert_eq!(g.hit((c.x + 159.0, c.bottom() - 1.0), 0.0), Some(7));
        // The gap between cells and the left margin hit nothing.
        assert_eq!(g.hit((c.right() + 5.0, c.y + 5.0), 0.0), None);
        assert_eq!(g.hit((g.left - 1.0, c.y), 0.0), None);
        // Scrolled: the same screen point is two rows further.
        assert_eq!(g.hit((c.x + 1.0, c.y + 1.0), 340.0), Some(17));
        // Past the last item.
        let last_row = g.cell_rect(22, 0.0);
        assert_eq!(
            g.hit((last_row.x + 170.0 + 1.0, last_row.y + 1.0), 0.0),
            None
        );
    }

    #[test]
    fn visibility_and_scrolling() {
        let g = Grid::new(100, 1000.0, 160.0, 10.0);
        assert_eq!(g.visible(0.0, 400.0), 0..15);
        assert_eq!(g.visible(170.0, 400.0), 5..20);
        assert_eq!(g.visible(10_000.0, 400.0), 100..100);
        let h = 400.0;
        // Already visible: unchanged.
        assert_eq!(g.reveal(5, 0.0, h), 0.0);
        // Below: scrolled just enough.
        let s = g.reveal(30, 0.0, h);
        let r = g.cell_rect(30, s);
        assert!(r.bottom() <= h && r.bottom() >= h - 20.0, "{s} {r:?}");
        // Above: top aligned.
        let s = g.reveal(0, 500.0, h);
        assert_eq!(s, 0.0);
        assert_eq!(g.page_rows(h), 2);
        assert_eq!(g.max_scroll(h), g.content_height() - h);
    }

    #[test]
    fn keyboard_steps_clamp() {
        let g = Grid::new(23, 1000.0, 160.0, 10.0);
        assert_eq!(g.step(7, 1, 0), 8);
        assert_eq!(g.step(7, 0, 1), 12);
        assert_eq!(g.step(7, 0, -2), 0);
        assert_eq!(g.step(20, 0, 1), 22);
        assert_eq!(Grid::new(0, 1000.0, 160.0, 10.0).step(0, 1, 0), 0);
    }

    #[test]
    fn fitting_and_strip() {
        let cell = Rect::new(10.0, 20.0, 160.0, 160.0);
        assert_eq!(
            fit_in(&cell, (256.0, 128.0)),
            Rect::new(10.0, 60.0, 160.0, 80.0)
        );
        // Small images are not enlarged.
        assert_eq!(
            fit_in(&cell, (40.0, 20.0)),
            Rect::new(70.0, 90.0, 40.0, 20.0)
        );
        let s = strip(100, 50, 1000.0, 80.0, 8.0);
        let centre = s.iter().find(|(i, _)| *i == 50).unwrap().1;
        assert_eq!(centre.x + 40.0, 500.0);
        assert!(s.iter().all(|(_, r)| r.right() > 0.0 && r.x < 1000.0));
        assert!(s.len() >= 11);
        // At the start nothing left of item 0.
        let s = strip(100, 0, 1000.0, 80.0, 8.0);
        assert_eq!(s[0].0, 0);
        assert!(strip(0, 0, 1000.0, 80.0, 8.0).is_empty());
    }
}
