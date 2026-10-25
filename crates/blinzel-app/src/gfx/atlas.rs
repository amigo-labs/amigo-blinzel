//! The thumbnail atlas' textures: pages for 64 slots of 256 × 256
//! thumbnails in 4:2:0 YCbCr (a 2048² R8 texture for Y, a 1024² R8G8 one
//! for Cb/Cr: 6 MB a page, 1.5 bytes per pixel to upload). Which slot holds
//! what is `blinzel_ui::gfx::Atlas`'s business.

use blinzel_core::thumb::Planes;
use blinzel_ui::gfx::PAGE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device,
    ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM, DXGI_SAMPLE_DESC,
};

/// The atlas' pages, created and filled on the UI thread.
pub struct Pages {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pub pages: Vec<Page>,
}

impl Pages {
    pub fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Pages {
        Pages {
            device: device.clone(),
            context: context.clone(),
            pages: Vec::new(),
        }
    }
}

impl blinzel_ui::gfx::AtlasPages for Pages {
    fn add_page(&mut self) -> Result<(), String> {
        self.pages
            .push(Page::new(&self.device).map_err(|e| e.message())?);
        Ok(())
    }

    fn upload(
        &mut self,
        page: usize,
        origin: (u32, u32),
        size: (u32, u32),
        chroma: (u32, u32),
        thumb: &Planes,
    ) {
        if let Some(p) = self.pages.get(page) {
            p.upload(&self.context, origin, size, chroma, thumb);
        }
    }
}

/// One atlas page: Y and interleaved Cb/Cr.
pub struct Page {
    pub y: (ID3D11Texture2D, ID3D11ShaderResourceView),
    pub cbcr: (ID3D11Texture2D, ID3D11ShaderResourceView),
}

impl Page {
    pub fn new(device: &ID3D11Device) -> windows::core::Result<Page> {
        Ok(Page {
            y: texture(device, PAGE, DXGI_FORMAT_R8_UNORM)?,
            cbcr: texture(device, PAGE / 2, DXGI_FORMAT_R8G8_UNORM)?,
        })
    }

    /// Copies `thumb` (`size` luma texels, `chroma` Cb/Cr pairs) to
    /// `origin` (luma texels; half of it for Cb/Cr).
    pub fn upload(
        &self,
        ctx: &ID3D11DeviceContext,
        origin: (u32, u32),
        size: (u32, u32),
        chroma: (u32, u32),
        thumb: &Planes,
    ) {
        let (ox, oy) = origin;
        unsafe {
            ctx.UpdateSubresource(
                &self.y.0,
                0,
                Some(&region(ox, oy, size.0, size.1)),
                thumb.y.as_ptr().cast(),
                thumb.width,
                0,
            );
            ctx.UpdateSubresource(
                &self.cbcr.0,
                0,
                Some(&region(ox / 2, oy / 2, chroma.0, chroma.1)),
                thumb.cbcr.as_ptr().cast(),
                thumb.chroma_size().0 * 2,
                0,
            );
        }
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
