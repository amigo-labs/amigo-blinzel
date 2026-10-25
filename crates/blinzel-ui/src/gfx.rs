//! What the UI draws, independent of the GPU API: colours, overlay shapes
//! and the thumbnail atlas' slot bookkeeping. Each platform renders these
//! with its own API (Direct3D 11, Metal, wgpu) and the same shader logic.

use std::collections::HashMap;

use blinzel_core::geom::{Orientation, Rect};
use blinzel_core::thumb::{EDGE, Planes};

/// Colour with straight alpha, 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

impl Rgba {
    pub fn from_u8(c: [u8; 3], a: f32) -> Rgba {
        Rgba(
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0,
            a,
        )
    }
}

/// UI primitives in client pixels.
#[derive(Clone, Debug)]
pub enum Shape {
    Fill {
        rect: Rect,
        radius: f32,
        color: Rgba,
    },
    Outline {
        rect: Rect,
        thickness: f32,
        radius: f32,
        color: Rgba,
    },
    Line {
        a: (f32, f32),
        b: (f32, f32),
        thickness: f32,
        color: Rgba,
    },
}

/// What the UI needs to know about an image stage on the GPU.
pub trait ImageInfo {
    /// Size of this stage in texels (stored orientation).
    fn size(&self) -> (u32, u32);
    /// Full-resolution size of the source (stored orientation).
    fn full_size(&self) -> (u32, u32);
    /// Orientation the file asks for.
    fn orientation(&self) -> Orientation;
    /// GPU memory it takes.
    fn bytes(&self) -> usize;

    fn is_full_resolution(&self) -> bool {
        self.size() == self.full_size()
    }
}

/// Atlas page edge in texels: 64 slots of [`EDGE`]² thumbnails, in 4:2:0
/// YCbCr (a page-sized one-channel texture for Y, a half-sized
/// two-channel one for Cb/Cr).
pub const PAGE: u32 = 2048;
const PER_ROW: u32 = PAGE / EDGE;
const PER_PAGE: usize = (PER_ROW * PER_ROW) as usize;

/// Where a thumbnail sits in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub page: usize,
    /// In the Y texture; half of it in the Cb/Cr one.
    pub origin: (u32, u32),
    pub size: (u32, u32),
    pub orientation: Orientation,
}

/// The GPU side of the atlas: pages and uploads into them.
pub trait AtlasPages {
    fn add_page(&mut self) -> Result<(), String>;
    /// Copies `thumb` (`size` luma texels, `chroma` Cb/Cr pairs) to
    /// `origin` of `page`.
    fn upload(
        &mut self,
        page: usize,
        origin: (u32, u32),
        size: (u32, u32),
        chroma: (u32, u32),
        thumb: &Planes,
    );
}

#[derive(Clone, Copy, Default)]
struct Slot {
    id: Option<u64>,
    last_drawn: u64,
    size: (u32, u32),
    orientation: Orientation,
}

/// The thumbnail atlas: slots are reused least recently drawn first.
pub struct Atlas {
    pages: usize,
    slots: Vec<Slot>,
    by_id: HashMap<u64, usize>,
    frame: u64,
    max_pages: usize,
}

impl Atlas {
    pub fn new(max_pages: usize) -> Atlas {
        Atlas {
            pages: 0,
            slots: Vec::new(),
            by_id: HashMap::new(),
            frame: 1,
            max_pages: max_pages.max(1),
        }
    }

    /// Starts a frame: what is drawn from now on is protected from reuse.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    pub fn contains(&self, id: u64) -> bool {
        self.by_id.contains_key(&id)
    }

    /// The slot of `id`, marked as drawn in this frame.
    pub fn get(&mut self, id: u64) -> Option<Placed> {
        let &i = self.by_id.get(&id)?;
        self.slots[i].last_drawn = self.frame;
        Some(self.placed(i))
    }

    fn placed(&self, i: usize) -> Placed {
        let s = &self.slots[i];
        let k = (i % PER_PAGE) as u32;
        Placed {
            page: i / PER_PAGE,
            origin: ((k % PER_ROW) * EDGE, (k / PER_ROW) * EDGE),
            size: s.size,
            orientation: s.orientation,
        }
    }

    /// Uploads `thumb` for `id`. Returns the id whose slot was reused (to
    /// be requested again when it is needed).
    pub fn insert(
        &mut self,
        gpu: &mut impl AtlasPages,
        id: u64,
        thumb: &Planes,
    ) -> Result<Option<u64>, String> {
        let (slot, evicted) = match self.by_id.get(&id) {
            Some(&i) => (i, None),
            None => self.free_slot(gpu)?,
        };
        if let Some(old) = evicted {
            self.by_id.remove(&old);
        }
        let (w, h) = (thumb.width.min(EDGE), thumb.height.min(EDGE));
        self.slots[slot] = Slot {
            id: Some(id),
            last_drawn: self.frame,
            size: (w, h),
            orientation: thumb.orientation,
        };
        self.by_id.insert(id, slot);
        let p = self.placed(slot);
        let (cw, ch) = thumb.chroma_size();
        gpu.upload(
            p.page,
            p.origin,
            (w, h),
            (cw.min(EDGE / 2), ch.min(EDGE / 2)),
            thumb,
        );
        Ok(evicted)
    }

    /// A free slot (adding a page if allowed), else the least recently
    /// drawn one and the id it held.
    fn free_slot(&mut self, gpu: &mut impl AtlasPages) -> Result<(usize, Option<u64>), String> {
        if let Some(i) = self.slots.iter().position(|s| s.id.is_none()) {
            return Ok((i, None));
        }
        if self.pages < self.max_pages {
            gpu.add_page()?;
            self.pages += 1;
            let first = self.slots.len();
            self.slots.resize(first + PER_PAGE, Slot::default());
            return Ok((first, None));
        }
        let (i, s) = self
            .slots
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.last_drawn)
            .expect("at least one page");
        Ok((i, s.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        pages: usize,
        uploads: Vec<(usize, (u32, u32))>,
    }

    impl AtlasPages for Fake {
        fn add_page(&mut self) -> Result<(), String> {
            self.pages += 1;
            Ok(())
        }
        fn upload(
            &mut self,
            page: usize,
            origin: (u32, u32),
            _: (u32, u32),
            _: (u32, u32),
            _: &Planes,
        ) {
            self.uploads.push((page, origin));
        }
    }

    fn planes() -> Planes {
        Planes {
            width: 200,
            height: 100,
            orientation: Orientation::NORMAL,
            y: vec![0; 200 * 100],
            cbcr: vec![0; 100 * 50 * 2],
            full: (200, 100),
        }
    }

    #[test]
    fn slots_fill_pages_then_reuse_the_least_recently_drawn() {
        let mut gpu = Fake::default();
        let mut atlas = Atlas::new(1);
        for id in 0..PER_PAGE as u64 {
            assert_eq!(atlas.insert(&mut gpu, id, &planes()), Ok(None));
        }
        assert_eq!(gpu.pages, 1);
        assert_eq!(gpu.uploads[1], (0, (EDGE, 0)));
        atlas.begin_frame();
        for id in 1..PER_PAGE as u64 {
            atlas.get(id);
        }
        // Id 0 was not drawn this frame: its slot goes.
        assert_eq!(atlas.insert(&mut gpu, 999, &planes()), Ok(Some(0)));
        assert!(!atlas.contains(0));
        assert_eq!(atlas.get(999).unwrap().size, (200, 100));
    }
}
