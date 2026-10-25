//! The grid browser (all images of the folder) and the filmstrip (the
//! current image's neighbours): layout, drawing from the thumbnail atlas,
//! hit testing, and asking the thumbnail pool for what is visible first.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use blinzel_core::filelist::FileList;
use blinzel_core::geom::Rect;
use blinzel_core::grid::{Grid, fit_in, strip};

use crate::backend::{Canvas, Services};
use crate::gfx::{Atlas, Rgba, Shape};
use crate::thumbs::{Request, Thumbs};

/// Logical sizes (multiplied by the DPI scale).
const GRID_CELL: f64 = 176.0;
const GRID_GAP: f64 = 12.0;
const STRIP_CELL: f64 = 72.0;
const STRIP_GAP: f64 = 6.0;
/// Atlas pages (16 MB each, 64 thumbnails).
const ATLAS_PAGES: usize = 8;

const PLACEHOLDER: Rgba = Rgba(1.0, 1.0, 1.0, 0.05);
const HOVER: Rgba = Rgba(1.0, 1.0, 1.0, 0.10);
const ACCENT: Rgba = Rgba(0.35, 0.62, 1.0, 1.0);
const STRIP_BG: Rgba = Rgba(0.0, 0.0, 0.0, 0.30);
const MARKED: Rgba = Rgba(0.35, 0.62, 1.0, 0.22);

#[derive(Clone, Debug)]
pub struct GridState {
    pub scroll: f64,
    pub selected: usize,
    /// Names of the marked images (Ctrl/Shift+click, Ctrl+A), e.g. for the
    /// batch dialog.
    pub marked: BTreeSet<String>,
}

pub struct Browse<S> {
    thumbs: Option<Thumbs<S>>,
    atlas: Atlas,
    failed: HashSet<u64>,
    pub grid: Option<GridState>,
    pub hover: Option<usize>,
}

impl<S> Default for Browse<S> {
    fn default() -> Self {
        Browse {
            thumbs: None,
            atlas: Atlas::new(ATLAS_PAGES),
            failed: HashSet::new(),
            grid: None,
            hover: None,
        }
    }
}

/// The grid of `count` items in a client area `client` wide.
pub fn grid_layout(count: usize, client: (f64, f64), scale: f64) -> Grid {
    Grid::new(
        count,
        client.0,
        (GRID_CELL * scale).round(),
        (GRID_GAP * scale).round(),
    )
}

/// Height of the filmstrip at DPI `scale`.
pub fn strip_height(scale: f64) -> f64 {
    ((STRIP_CELL + 2.0 * STRIP_GAP) * scale).round()
}

impl<S: Services> Browse<S> {
    /// Forgets everything shown (resident mode hides; GPU memory goes).
    pub fn reset(&mut self) {
        let thumbs = self.thumbs.take();
        if let Some(t) = &thumbs {
            t.reset();
        }
        *self = Browse {
            thumbs,
            ..Browse::default()
        };
    }

    /// Opens the grid with `current` selected and in view.
    pub fn open_grid(&mut self, count: usize, current: usize, client: (f64, f64), scale: f64) {
        let g = grid_layout(count, client, scale);
        let selected = current.min(count.saturating_sub(1));
        self.grid = Some(GridState {
            scroll: g.reveal(selected, 0.0, client.1),
            selected,
            marked: BTreeSet::new(),
        });
        self.hover = None;
    }

    /// Thumbnails that arrived: into the atlas. True if any did.
    pub fn on_thumbs(&mut self, canvas: &mut impl Canvas) -> bool {
        let Some(t) = &self.thumbs else {
            return false;
        };
        let mut any = false;
        while let Ok((id, thumb)) = t.results.try_recv() {
            any = true;
            match thumb {
                Some(th) => {
                    if let Ok(Some(evicted)) = self.atlas.insert(canvas, id, &th) {
                        t.forget(evicted);
                    }
                }
                None => {
                    self.failed.insert(id);
                }
            }
        }
        any
    }

    /// Asks for `items` (list indices, most wanted first) that are not in
    /// the atlas yet.
    fn request(
        &mut self,
        services: &Arc<S>,
        list: &FileList,
        background: [u8; 3],
        items: impl Iterator<Item = usize>,
    ) {
        let wanted: Vec<Request> = items
            .filter_map(|i| {
                let e = list.get(i)?;
                let id = e.cache_id();
                if e.is_placeholder() {
                    // Reading it would start a download.
                    self.failed.insert(id);
                    return None;
                }
                (!self.atlas.contains(id) && !self.failed.contains(&id)).then(|| Request {
                    id,
                    path: list.dir().join(&e.name),
                    size: e.size,
                    mtime: e.mtime,
                })
            })
            .collect();
        if wanted.is_empty() && self.thumbs.is_none() {
            return;
        }
        self.thumbs
            .get_or_insert_with(|| {
                Thumbs::new(services.clone(), services.thumb_threads(), background)
            })
            .set_wanted(wanted);
    }

    /// Draws the thumbnail of item `i` into `cell` (or a placeholder).
    fn draw_item(&mut self, r: &mut impl Canvas, list: &FileList, i: usize, cell: &Rect) {
        let placed = list.get(i).and_then(|e| self.atlas.get(e.cache_id()));
        match placed {
            Some(p) => {
                let size = p
                    .orientation
                    .oriented_size(p.size.0 as f64, p.size.1 as f64);
                r.draw_thumb(&p, &fit_in(cell, size), 1.0);
            }
            None => r.draw_shapes(&[Shape::Fill {
                rect: *cell,
                radius: 4.0,
                color: PLACEHOLDER,
            }]),
        }
    }

    /// Draws the grid over the whole client area and asks for what it
    /// shows (then the next and previous screenful).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_grid(
        &mut self,
        r: &mut impl Canvas,
        services: &Arc<S>,
        list: &FileList,
        client: (f64, f64),
        scale: f64,
        background: [u8; 3],
    ) {
        let Some(mut state) = self.grid.take() else {
            return;
        };
        self.atlas.begin_frame();
        let g = grid_layout(list.len(), client, scale);
        state.scroll = state.scroll.clamp(0.0, g.max_scroll(client.1));
        let visible = g.visible(state.scroll, client.1);
        for i in visible.clone() {
            let cell = g.cell_rect(i, state.scroll);
            if self.hover == Some(i) {
                r.draw_shapes(&[Shape::Fill {
                    rect: grow(&cell, 4.0 * scale),
                    radius: (6.0 * scale) as f32,
                    color: HOVER,
                }]);
            }
            let marked = list.get(i).is_some_and(|e| state.marked.contains(&e.name));
            if marked {
                r.draw_shapes(&[Shape::Fill {
                    rect: grow(&cell, 4.0 * scale),
                    radius: (6.0 * scale) as f32,
                    color: MARKED,
                }]);
            }
            self.draw_item(r, list, i, &cell);
            if marked {
                mark_badge(r, &cell, scale);
            }
        }
        if visible.contains(&state.selected) {
            let cell = g.cell_rect(state.selected, state.scroll);
            r.draw_shapes(&[Shape::Outline {
                rect: grow(&cell, 4.0 * scale),
                thickness: (2.0 * scale) as f32,
                radius: (6.0 * scale) as f32,
                color: ACCENT,
            }]);
        }
        self.grid = Some(state);
        let page = visible.len();
        let next = visible.end..(visible.end + page).min(list.len());
        let prev = visible.start.saturating_sub(page)..visible.start;
        self.request(
            services,
            list,
            background,
            visible.chain(next).chain(prev.rev()),
        );
    }

    /// Draws the filmstrip along the bottom and asks for its thumbnails.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_strip(
        &mut self,
        r: &mut impl Canvas,
        services: &Arc<S>,
        list: &FileList,
        current: Option<usize>,
        client: (f64, f64),
        scale: f64,
        background: [u8; 3],
    ) {
        self.atlas.begin_frame();
        let h = strip_height(scale);
        let top = client.1 - h;
        r.draw_shapes(&[Shape::Fill {
            rect: Rect::new(0.0, top, client.0, h),
            radius: 0.0,
            color: STRIP_BG,
        }]);
        let Some(cur) = current else { return };
        let cells = strip(
            list.len(),
            cur,
            client.0,
            (STRIP_CELL * scale).round(),
            (STRIP_GAP * scale).round(),
        );
        for (i, c) in &cells {
            let cell = Rect::new(c.x, top + c.y, c.w, c.h);
            if self.hover == Some(*i) {
                r.draw_shapes(&[Shape::Fill {
                    rect: grow(&cell, 3.0 * scale),
                    radius: (4.0 * scale) as f32,
                    color: HOVER,
                }]);
            }
            self.draw_item(r, list, *i, &cell);
            if *i == cur {
                r.draw_shapes(&[Shape::Outline {
                    rect: grow(&cell, 3.0 * scale),
                    thickness: (2.0 * scale) as f32,
                    radius: (4.0 * scale) as f32,
                    color: ACCENT,
                }]);
            }
        }
        // Nearest first, then a little further either way.
        let mut order: Vec<usize> = cells.iter().map(|(i, _)| *i).collect();
        order.sort_by_key(|i| i.abs_diff(cur));
        let half = cells.len() / 2;
        let extra = (half + 1..=3 * half)
            .flat_map(|k| [cur.checked_add(k), cur.checked_sub(k)])
            .flatten()
            .filter(|&i| i < list.len());
        self.request(services, list, background, order.into_iter().chain(extra));
    }

    /// The grid cell under `p`.
    pub fn hit_grid(
        &self,
        p: (f64, f64),
        count: usize,
        client: (f64, f64),
        scale: f64,
    ) -> Option<usize> {
        let state = self.grid.as_ref()?;
        grid_layout(count, client, scale).hit(p, state.scroll)
    }

    /// The filmstrip cell under `p`.
    pub fn hit_strip(
        &self,
        p: (f64, f64),
        count: usize,
        current: usize,
        client: (f64, f64),
        scale: f64,
    ) -> Option<usize> {
        let top = client.1 - strip_height(scale);
        if p.1 < top {
            return None;
        }
        strip(
            count,
            current,
            client.0,
            (STRIP_CELL * scale).round(),
            (STRIP_GAP * scale).round(),
        )
        .into_iter()
        .find(|(_, r)| r.contains((p.0, p.1 - top)))
        .map(|(i, _)| i)
    }
}

/// A check mark in a circle at the top right of a marked cell.
fn mark_badge(r: &mut impl Canvas, cell: &Rect, scale: f64) {
    let d = 22.0 * scale;
    let c = Rect::new(cell.right() - d - 4.0 * scale, cell.y + 4.0 * scale, d, d);
    let (x, y, k) = (c.x + d / 2.0, c.y + d / 2.0, 4.5 * scale);
    let line = |a: (f64, f64), b: (f64, f64)| Shape::Line {
        a: (a.0 as f32, a.1 as f32),
        b: (b.0 as f32, b.1 as f32),
        thickness: (2.0 * scale) as f32,
        color: Rgba(1.0, 1.0, 1.0, 1.0),
    };
    r.draw_shapes(&[
        Shape::Fill {
            rect: c,
            radius: (d / 2.0) as f32,
            color: ACCENT,
        },
        line((x - k, y), (x - k / 3.0, y + k * 0.7)),
        line((x - k / 3.0, y + k * 0.7), (x + k, y - k * 0.8)),
    ]);
}

fn grow(r: &Rect, d: f64) -> Rect {
    Rect::new(r.x - d, r.y - d, r.w + 2.0 * d, r.h + 2.0 * d)
}
