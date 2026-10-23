//! Render targets: the window's flip-model swap chain, and offscreen
//! textures with CPU readback for tests and benchmarks.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_PRESENT, DXGI_SCALING_NONE, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGISwapChain1,
};

use super::device::Gfx;

pub struct SwapChain {
    chain: IDXGISwapChain1,
    rtv: Option<ID3D11RenderTargetView>,
    pub size: (u32, u32),
}

impl SwapChain {
    pub fn new(gfx: &Gfx, hwnd: HWND, size: (u32, u32)) -> windows::core::Result<SwapChain> {
        let mut desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.0.max(1),
            Height: size.1.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            // No stretching: during a live resize DWM shows the old frame
            // unscaled until the next present instead of a blurry one.
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let chain = unsafe {
            gfx.factory
                .CreateSwapChainForHwnd(&gfx.device, hwnd, &desc, None, None)
        }
        .or_else(|_| {
            // Very old drivers reject SCALING_NONE.
            desc.Scaling = DXGI_SCALING_STRETCH;
            unsafe {
                gfx.factory
                    .CreateSwapChainForHwnd(&gfx.device, hwnd, &desc, None, None)
            }
        })?;
        let mut sc = SwapChain {
            chain,
            rtv: None,
            size: (desc.Width, desc.Height),
        };
        sc.create_rtv(&gfx.device)?;
        Ok(sc)
    }

    fn create_rtv(&mut self, device: &ID3D11Device) -> windows::core::Result<()> {
        let back: ID3D11Texture2D = unsafe { self.chain.GetBuffer(0)? };
        let mut rtv = None;
        unsafe { device.CreateRenderTargetView(&back, None, Some(&mut rtv))? };
        self.rtv = rtv;
        Ok(())
    }

    pub fn resize(&mut self, gfx: &Gfx, size: (u32, u32)) -> windows::core::Result<()> {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size && self.rtv.is_some() {
            return Ok(());
        }
        self.rtv = None;
        unsafe {
            gfx.context.OMSetRenderTargets(None, None);
            gfx.context.Flush();
            self.chain.ResizeBuffers(
                0,
                size.0,
                size.1,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.size = size;
        self.create_rtv(&gfx.device)
    }

    pub fn rtv(&self) -> Option<&ID3D11RenderTargetView> {
        self.rtv.as_ref()
    }

    pub fn present(&self) -> windows::core::HRESULT {
        unsafe { self.chain.Present(1, DXGI_PRESENT(0)) }
    }
}

/// An offscreen BGRA target with readback.
pub struct Offscreen {
    tex: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    probe: ID3D11Texture2D,
    pub rtv: ID3D11RenderTargetView,
    pub size: (u32, u32),
}

impl Offscreen {
    pub fn new(device: &ID3D11Device, size: (u32, u32)) -> windows::core::Result<Offscreen> {
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: size.0,
            Height: size.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex = None;
        let mut rtv = None;
        let mut staging = None;
        let mut probe = None;
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut tex))?;
            device.CreateRenderTargetView(tex.as_ref().unwrap(), None, Some(&mut rtv))?;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            desc.Width = 1;
            desc.Height = 1;
            device.CreateTexture2D(&desc, None, Some(&mut probe))?;
        }
        Ok(Offscreen {
            tex: tex.unwrap(),
            staging: staging.unwrap(),
            probe: probe.unwrap(),
            rtv: rtv.unwrap(),
            size,
        })
    }

    /// Waits until the GPU has finished all work on the target (copies one
    /// pixel back) — for timing.
    pub fn sync(&self, ctx: &ID3D11DeviceContext) -> windows::core::Result<()> {
        unsafe {
            let b = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: 1,
                bottom: 1,
                back: 1,
            };
            ctx.CopySubresourceRegion(&self.probe, 0, 0, 0, 0, &self.tex, 0, Some(&b));
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&self.probe, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            ctx.Unmap(&self.probe, 0);
        }
        Ok(())
    }

    /// Copies the target to the CPU as tightly packed BGRA. Blocks until the
    /// GPU has finished, so it doubles as a GPU sync point for timing.
    pub fn read_bgra(&self, ctx: &ID3D11DeviceContext) -> windows::core::Result<Vec<u8>> {
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        let mut out = vec![0u8; w * h * 4];
        unsafe {
            ctx.CopyResource(&self.staging, &self.tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            for y in 0..h {
                let src = std::slice::from_raw_parts(
                    m.pData.cast::<u8>().add(y * m.RowPitch as usize),
                    w * 4,
                );
                out[y * w * 4..(y + 1) * w * 4].copy_from_slice(src);
            }
            ctx.Unmap(&self.staging, 0);
        }
        Ok(out)
    }
}
