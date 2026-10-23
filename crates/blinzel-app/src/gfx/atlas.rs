//! The thumbnail atlas: pages of 64 slots for 256 × 256 thumbnails in
//! 4:2:0 YCbCr (a 2048² R8 texture for Y, a 1024² R8G8 one for Cb/Cr: 6 MB
//! a page, 1.5 bytes per pixel to upload). Slots are reused least recently
//! drawn first.

use std::collections::HashMap;

use blinzel_core::geom::Orientation;
use blinzel_core::thumb::{EDGE, Planes};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device,
    ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM, DXGI_SAMPLE_DESC,
};

pub const PAGE: u32 = 2048;
const PER_ROW: u32 = PAGE / EDGE;
const PER_PAGE: usize = (PER_ROW * PER_ROW) as usize;

/// Where a thumbnail sits in the atlas.
#[derive(Clone)]
pub struct Placed {
    pub y: ID3D11ShaderResourceView,
    pub cbcr: ID3D11ShaderResourceView,
    /// In the Y texture; half of it in the Cb/Cr one.
    pub origin: (u32, u32),
    pub size: (u32, u32),
    pub orientation: Orientation,
}

struct Page {
    y: (ID3D11Texture2D, ID3D11ShaderResourceView),
    cbcr: (ID3D11Texture2D, ID3D11ShaderResourceView),
}

#[derive(Clone, Copy, Default)]
struct Slot {
    id: Option<u64>,
    last_drawn: u64,
    size: (u32, u32),
    orientation: Orientation,
}

pub struct Atlas {
    pages: Vec<Page>,
    slots: Vec<Slot>,
    by_id: HashMap<u64, usize>,
    frame: u64,
    max_pages: usize,
}

impl Atlas {
    pub fn new(max_pages: usize) -> Atlas {
        Atlas {
            pages: Vec::new(),
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
        let page = &self.pages[i / PER_PAGE];
        Placed {
            y: page.y.1.clone(),
            cbcr: page.cbcr.1.clone(),
            origin: ((k % PER_ROW) * EDGE, (k / PER_ROW) * EDGE),
            size: s.size,
            orientation: s.orientation,
        }
    }

    /// Uploads `thumb` for `id`. Returns the id whose slot was reused (to
    /// be requested again when it is needed).
    pub fn insert(
        &mut self,
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        id: u64,
        thumb: &Planes,
    ) -> windows::core::Result<Option<u64>> {
        let (slot, evicted) = match self.by_id.get(&id) {
            Some(&i) => (i, None),
            None => self.free_slot(device)?,
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
        let page = &self.pages[slot / PER_PAGE];
        let (cw, ch) = thumb.chroma_size();
        let (ox, oy) = p.origin;
        unsafe {
            ctx.UpdateSubresource(
                &page.y.0,
                0,
                Some(&region(ox, oy, w, h)),
                thumb.y.as_ptr().cast(),
                thumb.width,
                0,
            );
            ctx.UpdateSubresource(
                &page.cbcr.0,
                0,
                Some(&region(ox / 2, oy / 2, cw.min(EDGE / 2), ch.min(EDGE / 2))),
                thumb.cbcr.as_ptr().cast(),
                cw * 2,
                0,
            );
        }
        Ok(evicted)
    }

    /// A free slot (adding a page if allowed), else the least recently
    /// drawn one and the id it held.
    fn free_slot(&mut self, device: &ID3D11Device) -> windows::core::Result<(usize, Option<u64>)> {
        if let Some(i) = self.slots.iter().position(|s| s.id.is_none()) {
            return Ok((i, None));
        }
        if self.pages.len() < self.max_pages {
            self.pages.push(new_page(device)?);
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

fn region(x: u32, y: u32, w: u32, h: u32) -> D3D11_BOX {
    D3D11_BOX {
        left: x,
        top: y,
        front: 0,
        right: x + w,
        bottom: y + h,
        back: 1,
    }
}

fn new_page(device: &ID3D11Device) -> windows::core::Result<Page> {
    Ok(Page {
        y: texture(device, PAGE, DXGI_FORMAT_R8_UNORM)?,
        cbcr: texture(device, PAGE / 2, DXGI_FORMAT_R8G8_UNORM)?,
    })
}

fn texture(
    device: &ID3D11Device,
    size: u32,
    format: DXGI_FORMAT,
) -> windows::core::Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size,
        Height: size,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    let mut srv: Option<ID3D11ShaderResourceView> = None;
    unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut tex))?;
        device.CreateShaderResourceView(tex.as_ref().unwrap(), None, Some(&mut srv))?;
    }
    Ok((tex.unwrap(), srv.unwrap()))
}
