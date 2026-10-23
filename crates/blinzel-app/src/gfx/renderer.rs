//! Draw calls: image tiles and UI overlays. One vertex shader, two pixel
//! shaders, one dynamic constant buffer; no vertex buffers.

use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE,
    D3D11_BLEND_OP_ADD, D3D11_BUFFER_DESC, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_NEVER,
    D3D11_CPU_ACCESS_WRITE, D3D11_CULL_NONE, D3D11_FILL_SOLID, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
    D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE, D3D11_RASTERIZER_DESC,
    D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC, D3D11_TEXTURE_ADDRESS_CLAMP,
    D3D11_USAGE_DYNAMIC, D3D11_VIEWPORT, ID3D11BlendState, ID3D11Buffer, ID3D11Device,
    ID3D11DeviceContext, ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11VertexShader,
};

use blinzel_core::cpuref::taps;
use blinzel_core::geom::{Affine, Rect};
use blinzel_core::layout::texels_per_pixel;

use super::texture::{GpuImage, GpuLut, Kind};

static VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vs.cso"));
static PS_IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ps_image.cso"));
static PS_OVERLAY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ps_overlay.cso"));

/// Mirror of the HLSL `Draw` constant buffer.
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct DrawCb {
    m0: [f32; 4],
    m1: [f32; 4],
    rect: [f32; 4],
    tex0: [f32; 4],
    tex1: [f32; 4],
    view: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    cms: [f32; 4],
}

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

pub struct Renderer {
    vs: ID3D11VertexShader,
    ps_image: ID3D11PixelShader,
    ps_overlay: ID3D11PixelShader,
    cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    raster: ID3D11RasterizerState,
    blend_premul: ID3D11BlendState,
    client: (u32, u32),
}

impl Renderer {
    pub fn new(device: &ID3D11Device) -> windows::core::Result<Renderer> {
        if cfg!(blinzel_no_shaders) || VS.is_empty() {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_FAIL,
                "built without compiled shaders (fxc unavailable)",
            ));
        }
        unsafe {
            let mut vs = None;
            device.CreateVertexShader(VS, None, Some(&mut vs))?;
            let mut ps_image = None;
            device.CreatePixelShader(PS_IMAGE, None, Some(&mut ps_image))?;
            let mut ps_overlay = None;
            device.CreatePixelShader(PS_OVERLAY, None, Some(&mut ps_overlay))?;
            let mut cb = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<DrawCb>() as u32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    MiscFlags: 0,
                    StructureByteStride: 0,
                },
                None,
                Some(&mut cb),
            )?;
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MipLODBias: 0.0,
                    MaxAnisotropy: 1,
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    BorderColor: [0.0; 4],
                    MinLOD: 0.0,
                    MaxLOD: f32::MAX,
                },
                Some(&mut sampler),
            )?;
            let mut raster = None;
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    // Mirrored orientations flip the winding.
                    CullMode: D3D11_CULL_NONE,
                    DepthClipEnable: true.into(),
                    ..Default::default()
                },
                Some(&mut raster),
            )?;
            let mut rt = [D3D11_RENDER_TARGET_BLEND_DESC::default(); 8];
            rt[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut blend_premul = None;
            device.CreateBlendState(
                &D3D11_BLEND_DESC {
                    AlphaToCoverageEnable: false.into(),
                    IndependentBlendEnable: false.into(),
                    RenderTarget: rt,
                },
                Some(&mut blend_premul),
            )?;
            Ok(Renderer {
                vs: vs.unwrap(),
                ps_image: ps_image.unwrap(),
                ps_overlay: ps_overlay.unwrap(),
                cb: cb.unwrap(),
                sampler: sampler.unwrap(),
                raster: raster.unwrap(),
                blend_premul: blend_premul.unwrap(),
                client: (1, 1),
            })
        }
    }

    /// Binds `rtv` (of `size` pixels), clears it and sets common state.
    pub fn begin(
        &mut self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        size: (u32, u32),
        clear: Rgba,
    ) {
        self.client = (size.0.max(1), size.1.max(1));
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.ClearRenderTargetView(rtv, &[clear.0, clear.1, clear.2, 1.0]);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.client.0 as f32,
                Height: self.client.1 as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.RSSetState(&self.raster);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        }
    }

    fn upload_cb(&self, ctx: &ID3D11DeviceContext, cb: &DrawCb) {
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if ctx
                .Map(&self.cb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
                .is_ok()
            {
                std::ptr::write(mapped.pData.cast::<DrawCb>(), *cb);
                ctx.Unmap(&self.cb, 0);
            }
        }
    }

    fn view_vec(&self, footprint: f32, taps: f32) -> [f32; 4] {
        [
            2.0 / self.client.0 as f32,
            2.0 / self.client.1 as f32,
            footprint,
            taps,
        ]
    }

    /// Draws the visible part (`content`, stage texel coordinates) of an
    /// image stage with `xform` (stage texels → client pixels).
    pub fn draw_image(
        &self,
        ctx: &ID3D11DeviceContext,
        img: &GpuImage,
        xform: &Affine,
        content: &Rect,
        bg: Rgba,
        lut: Option<&GpuLut>,
    ) {
        let footprint = texels_per_pixel(xform);
        let cms = match lut {
            Some(l) => {
                let n = l.size as f32;
                [1.0, (n - 1.0) / n, 0.5 / n, 0.0]
            }
            None => [0.0; 4],
        };
        let n = taps(footprint);
        let (mode, sub, opaque) = match img.kind {
            Kind::Rgba { opaque } => (0.0, (1.0, 1.0), opaque),
            Kind::Gray => (1.0, (1.0, 1.0), true),
            Kind::Ycc { sub_h, sub_v } => (2.0, (sub_h as f32, sub_v as f32), true),
        };
        let bounds = Rect::new(0.0, 0.0, img.width as f64, img.height as f64);
        let Some(visible) = content.intersect(&bounds) else {
            return;
        };
        unsafe {
            ctx.PSSetShader(&self.ps_image, None);
            ctx.OMSetBlendState(None, None, 0xFFFF_FFFF);
            ctx.PSSetShaderResources(3, Some(&[lut.map(|l| l.srv.clone())]));
        }
        for tile in &img.tiles {
            let r = tile.rect;
            let tr = Rect::new(
                r[0] as f64,
                r[1] as f64,
                (r[2] - r[0]) as f64,
                (r[3] - r[1]) as f64,
            );
            let Some(q) = tr.intersect(&visible) else {
                continue;
            };
            let main = &tile.main;
            let chroma: [f32; 4] = match &tile.chroma {
                Some((cb, _)) => [
                    cb.origin.0 as f32,
                    cb.origin.1 as f32,
                    1.0 / cb.size.0 as f32,
                    1.0 / cb.size.1 as f32,
                ],
                None => [0.0, 0.0, 1.0, 1.0],
            };
            let srvs: [Option<ID3D11ShaderResourceView>; 3] = match &tile.chroma {
                Some((cb, cr)) => [
                    Some(main.srv.clone()),
                    Some(cb.srv.clone()),
                    Some(cr.srv.clone()),
                ],
                None => [Some(main.srv.clone()), None, None],
            };
            let cb = DrawCb {
                m0: [xform.a as f32, xform.b as f32, xform.c as f32, 0.0],
                m1: [xform.d as f32, xform.e as f32, xform.f as f32, 0.0],
                rect: [q.x as f32, q.y as f32, q.right() as f32, q.bottom() as f32],
                tex0: [
                    main.origin.0 as f32,
                    main.origin.1 as f32,
                    1.0 / main.size.0 as f32,
                    1.0 / main.size.1 as f32,
                ],
                tex1: chroma,
                view: self.view_vec(footprint as f32, n as f32),
                color: [bg.0, bg.1, bg.2, 1.0],
                params: [sub.0, sub.1, mode, if opaque { 1.0 } else { 0.0 }],
                cms,
            };
            self.upload_cb(ctx, &cb);
            unsafe {
                ctx.PSSetShaderResources(0, Some(&srvs));
                ctx.Draw(4, 0);
            }
        }
    }

    /// Draws UI shapes (client pixels), premultiplied-alpha blended.
    pub fn draw_shapes(&self, ctx: &ID3D11DeviceContext, shapes: &[Shape]) {
        if shapes.is_empty() {
            return;
        }
        unsafe {
            ctx.PSSetShader(&self.ps_overlay, None);
            ctx.OMSetBlendState(&self.blend_premul, None, 0xFFFF_FFFF);
        }
        for s in shapes {
            let (bounds, geom, color, params) = match *s {
                Shape::Fill {
                    rect,
                    radius,
                    color,
                } => (rect, rect_vec(&rect), color, [0.0, 0.0, radius, 0.0]),
                Shape::Outline {
                    rect,
                    thickness,
                    radius,
                    color,
                } => {
                    let g = thickness as f64 / 2.0 + 1.0;
                    let b = Rect::new(rect.x - g, rect.y - g, rect.w + 2.0 * g, rect.h + 2.0 * g);
                    (b, rect_vec(&rect), color, [2.0, thickness, radius, 0.0])
                }
                Shape::Line {
                    a,
                    b,
                    thickness,
                    color,
                } => {
                    let g = thickness / 2.0 + 1.0;
                    let r = Rect::from_points(
                        ((a.0.min(b.0) - g) as f64, (a.1.min(b.1) - g) as f64),
                        ((a.0.max(b.0) + g) as f64, (a.1.max(b.1) + g) as f64),
                    );
                    (r, [a.0, a.1, b.0, b.1], color, [1.0, thickness, 0.0, 0.0])
                }
            };
            let cb = DrawCb {
                m0: [1.0, 0.0, 0.0, 0.0],
                m1: [0.0, 1.0, 0.0, 0.0],
                rect: rect_vec(&bounds),
                tex1: geom,
                view: self.view_vec(1.0, 1.0),
                color: [color.0, color.1, color.2, color.3],
                params,
                ..Default::default()
            };
            self.upload_cb(ctx, &cb);
            unsafe { ctx.Draw(4, 0) };
        }
    }

    /// Draws a text texture (coverage in alpha) at `pos`, tinted.
    pub fn draw_text(
        &self,
        ctx: &ID3D11DeviceContext,
        srv: &ID3D11ShaderResourceView,
        pos: (f32, f32),
        size: (u32, u32),
        color: Rgba,
    ) {
        unsafe {
            ctx.PSSetShader(&self.ps_overlay, None);
            ctx.OMSetBlendState(&self.blend_premul, None, 0xFFFF_FFFF);
            ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
        }
        let cb = DrawCb {
            m0: [1.0, 0.0, 0.0, 0.0],
            m1: [0.0, 1.0, 0.0, 0.0],
            rect: [pos.0, pos.1, pos.0 + size.0 as f32, pos.1 + size.1 as f32],
            tex0: [
                pos.0,
                pos.1,
                1.0 / size.0.max(1) as f32,
                1.0 / size.1.max(1) as f32,
            ],
            view: self.view_vec(1.0, 1.0),
            color: [color.0, color.1, color.2, color.3],
            params: [3.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        self.upload_cb(ctx, &cb);
        unsafe { ctx.Draw(4, 0) };
    }
}

fn rect_vec(r: &Rect) -> [f32; 4] {
    [r.x as f32, r.y as f32, r.right() as f32, r.bottom() as f32]
}
