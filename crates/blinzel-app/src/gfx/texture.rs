//! Uploading decoded images as immutable textures, tiled.
//!
//! Every image is cut into tiles of at most `tile` texels per edge (4096 by
//! default): upload spikes stay bounded, eviction is finer-grained, and
//! nothing ever exceeds the 16384 limit. Each tile texture carries
//! a small border of neighbouring texels so bilinear and box filtering are
//! seamless across tiles. Sub-rectangles are uploaded straight from the
//! decoder's planes via the row pitch, without copying.
//!
//! The D3D11 device is free-threaded, so this runs on decode workers.

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_IMMUTABLE, ID3D11Device, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_SAMPLE_DESC,
};

use blinzel_core::geom::Orientation;
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

/// Texels of neighbouring data around each luma/RGBA tile (chroma: half).
pub const BORDER: u32 = 4;
pub const DEFAULT_TILE: u32 = 4096;

/// How the shader interprets a texture set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rgba { opaque: bool },
    Gray,
    Ycc { sub_h: u8, sub_v: u8 },
}

/// One texture plus where its texel (0, 0) lies in plane coordinates.
pub struct TexRef {
    pub srv: ID3D11ShaderResourceView,
    pub origin: (u32, u32),
    pub size: (u32, u32),
}

pub struct Tile {
    /// Area this tile is responsible for, in stage (luma) texels:
    /// x0, y0, x1, y1 (without border).
    pub rect: [u32; 4],
    pub main: TexRef,
    /// Cb and Cr for YCbCr images.
    pub chroma: Option<(TexRef, TexRef)>,
}

/// An image stage resident on the GPU.
pub struct GpuImage {
    pub kind: Kind,
    pub width: u32,
    pub height: u32,
    pub full_width: u32,
    pub full_height: u32,
    pub orientation: Orientation,
    pub tiles: Vec<Tile>,
    pub bytes: usize,
}

impl GpuImage {
    pub fn is_full_resolution(&self) -> bool {
        self.width == self.full_width && self.height == self.full_height
    }
}

/// Texture of the plane area `[x0, y0, x1, y1)`.
fn texture(
    device: &ID3D11Device,
    plane: &Plane,
    bpp: usize,
    format: DXGI_FORMAT,
    [x0, y0, x1, y1]: [u32; 4],
) -> windows::core::Result<TexRef> {
    let (w, h) = (x1 - x0, y1 - y0);
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let offset = y0 as usize * plane.stride + x0 as usize * bpp;
    let last = offset + (h as usize - 1) * plane.stride + w as usize * bpp;
    assert!(last <= plane.data.len(), "tile outside plane");
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: plane.data[offset..].as_ptr().cast(),
        SysMemPitch: plane.stride as u32,
        SysMemSlicePitch: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    let mut srv: Option<ID3D11ShaderResourceView> = None;
    unsafe {
        device.CreateTexture2D(&desc, Some(&init), Some(&mut tex))?;
        device.CreateShaderResourceView(tex.as_ref().unwrap(), None, Some(&mut srv))?;
    }
    Ok(TexRef {
        srv: srv.unwrap(),
        origin: (x0, y0),
        size: (w, h),
    })
}

/// Splits `0..len` into spans of at most `tile` that start at multiples of
/// `align` (so chroma tiles line up with luma tiles).
fn spans(len: u32, tile: u32, align: u32) -> Vec<(u32, u32)> {
    let step = (tile / align).max(1) * align;
    (0..len.div_ceil(step))
        .map(|i| (i * step, ((i + 1) * step).min(len)))
        .collect()
}

/// Uploads `img` with tiles of at most `tile` texels (border included).
pub fn upload(
    device: &ID3D11Device,
    img: &Image,
    orientation: Orientation,
    tile: u32,
) -> windows::core::Result<GpuImage> {
    let tile = tile.max(64);
    let inner = tile - 2 * BORDER;
    let mut tiles = Vec::new();
    let mut bytes = 0usize;
    let (w, h) = (img.width, img.height);
    let kind = match &img.pixels {
        Pixels::Packed {
            plane,
            format,
            alpha,
        } => {
            let (dxgi, bpp) = match format {
                PackedFormat::Bgra8 => (DXGI_FORMAT_B8G8R8A8_UNORM, 4),
                PackedFormat::Rgba8 => (DXGI_FORMAT_R8G8B8A8_UNORM, 4),
                PackedFormat::Gray8 => (DXGI_FORMAT_R8_UNORM, 1),
            };
            for &(y0, y1) in &spans(h, inner, 1) {
                for &(x0, x1) in &spans(w, inner, 1) {
                    let (bx0, by0) = (x0.saturating_sub(BORDER), y0.saturating_sub(BORDER));
                    let (bx1, by1) = ((x1 + BORDER).min(w), (y1 + BORDER).min(h));
                    let main = texture(device, plane, bpp, dxgi, [bx0, by0, bx1, by1])?;
                    bytes += ((bx1 - bx0) * (by1 - by0)) as usize * bpp;
                    tiles.push(Tile {
                        rect: [x0, y0, x1, y1],
                        main,
                        chroma: None,
                    });
                }
            }
            match format {
                PackedFormat::Gray8 => Kind::Gray,
                _ => Kind::Rgba {
                    opaque: *alpha == Alpha::Opaque,
                },
            }
        }
        Pixels::Ycc { y, cb, cr, sub } => {
            let (sh, sv) = (sub.h as u32, sub.v as u32);
            // Luma tile edges on multiples of the chroma factor.
            for &(y0, y1) in &spans(h, inner, sv.max(1) * 8) {
                for &(x0, x1) in &spans(w, inner, sh.max(1) * 8) {
                    let (bx0, by0) = (x0.saturating_sub(BORDER), y0.saturating_sub(BORDER));
                    let (bx1, by1) = ((x1 + BORDER).min(w), (y1 + BORDER).min(h));
                    let main = texture(device, y, 1, DXGI_FORMAT_R8_UNORM, [bx0, by0, bx1, by1])?;
                    // Chroma: the same area divided by the factors, with a
                    // border of BORDER / factor chroma texels.
                    let (cbx, cby) = (BORDER.div_ceil(sh), BORDER.div_ceil(sv));
                    let (cx0, cy0) = ((x0 / sh).saturating_sub(cbx), (y0 / sv).saturating_sub(cby));
                    let cx1 = (x1.div_ceil(sh) + cbx).min(cb.width);
                    let cy1 = (y1.div_ceil(sv) + cby).min(cb.height);
                    let tcb = texture(device, cb, 1, DXGI_FORMAT_R8_UNORM, [cx0, cy0, cx1, cy1])?;
                    let tcr = texture(device, cr, 1, DXGI_FORMAT_R8_UNORM, [cx0, cy0, cx1, cy1])?;
                    bytes += ((bx1 - bx0) * (by1 - by0) + 2 * (cx1 - cx0) * (cy1 - cy0)) as usize;
                    tiles.push(Tile {
                        rect: [x0, y0, x1, y1],
                        main,
                        chroma: Some((tcb, tcr)),
                    });
                }
            }
            Kind::Ycc {
                sub_h: sub.h,
                sub_v: sub.v,
            }
        }
    };
    Ok(GpuImage {
        kind,
        width: w,
        height: h,
        full_width: img.full_width,
        full_height: img.full_height,
        orientation,
        tiles,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::spans;

    #[test]
    fn spans_cover_and_align() {
        assert_eq!(spans(10, 4, 1), vec![(0, 4), (4, 8), (8, 10)]);
        assert_eq!(
            spans(100, 40, 16),
            vec![(0, 32), (32, 64), (64, 96), (96, 100)]
        );
        assert_eq!(spans(5, 4096, 16), vec![(0, 5)]);
    }
}
