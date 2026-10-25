//! Per-draw uniforms of the wgpu (Linux) and Metal (macOS) renderers. Both
//! run the same shaders (`blinzel.wgsl`, `blinzel.metal`, ports of the
//! Direct3D `blinzel.hlsl`): one quad per draw, everything else in a
//! [`DrawCb`]. This module turns the [`crate::backend::Canvas`] calls into
//! those uniforms, so the two renderers only bind textures and submit.

use blinzel_core::cpuref::taps;
use blinzel_core::geom::{Affine, Rect};
use blinzel_core::layout::texels_per_pixel;

use crate::gfx::{PAGE, Placed, Rgba, Shape};

/// Mirror of the shaders' `Draw` uniforms.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DrawCb {
    pub m0: [f32; 4],
    pub m1: [f32; 4],
    pub rect: [f32; 4],
    pub tex0: [f32; 4],
    pub tex1: [f32; 4],
    pub view: [f32; 4],
    pub color: [f32; 4],
    pub params: [f32; 4],
    pub cms: [f32; 4],
}

impl DrawCb {
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: plain f32 arrays without padding.
        unsafe {
            std::slice::from_raw_parts(
                (self as *const DrawCb).cast::<u8>(),
                std::mem::size_of::<DrawCb>(),
            )
        }
    }
}

/// How an image stage's textures hold its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// One RGBA (or BGRA) texture.
    Rgba { opaque: bool },
    /// One single-channel texture.
    Gray,
    /// Y, Cb and Cr textures, chroma subsampled by `sub_h` × `sub_v`.
    Ycc { sub_h: u8, sub_v: u8 },
}

/// Where one tile of an image stage lies (see [`crate::gfx::plan_tiles`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileGeom {
    /// The image area the tile draws, `[x0, y0, x1, y1)`.
    pub rect: [u32; 4],
    /// Origin and size of the main texture in plane texels.
    pub main: ((u32, u32), (u32, u32)),
    /// Origin and size of the chroma textures (YCbCr).
    pub chroma: Option<((u32, u32), (u32, u32))>,
}

fn view_vec(client: (u32, u32), footprint: f32, taps: f32) -> [f32; 4] {
    [
        2.0 / client.0.max(1) as f32,
        2.0 / client.1.max(1) as f32,
        footprint,
        taps,
    ]
}

fn rect_vec(r: &Rect) -> [f32; 4] {
    [r.x as f32, r.y as f32, r.right() as f32, r.bottom() as f32]
}

/// The draws of an image stage: `(tile index, uniforms)` for each tile in
/// the visible part `content` (stage texels). `lut_size` is the edge of the
/// colour transform's 3D LUT, if one applies.
#[allow(clippy::too_many_arguments)]
pub fn image(
    kind: ImageKind,
    size: (u32, u32),
    tiles: &[TileGeom],
    xform: &Affine,
    content: &Rect,
    bg: Rgba,
    lut_size: Option<u32>,
    client: (u32, u32),
) -> Vec<(usize, DrawCb)> {
    let footprint = texels_per_pixel(xform);
    let cms = match lut_size {
        Some(n) => {
            let n = n as f32;
            [1.0, (n - 1.0) / n, 0.5 / n, 0.0]
        }
        None => [0.0; 4],
    };
    let n = taps(footprint);
    let (mode, sub, opaque) = match kind {
        ImageKind::Rgba { opaque } => (0.0, (1.0, 1.0), opaque),
        ImageKind::Gray => (1.0, (1.0, 1.0), true),
        ImageKind::Ycc { sub_h, sub_v } => (2.0, (sub_h as f32, sub_v as f32), true),
    };
    let bounds = Rect::new(0.0, 0.0, size.0 as f64, size.1 as f64);
    let Some(visible) = content.intersect(&bounds) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, tile) in tiles.iter().enumerate() {
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
        let (origin, tsize) = tile.main;
        let chroma = match tile.chroma {
            Some((o, s)) => [o.0 as f32, o.1 as f32, 1.0 / s.0 as f32, 1.0 / s.1 as f32],
            None => [0.0, 0.0, 1.0, 1.0],
        };
        out.push((
            i,
            DrawCb {
                m0: [xform.a as f32, xform.b as f32, xform.c as f32, 0.0],
                m1: [xform.d as f32, xform.e as f32, xform.f as f32, 0.0],
                rect: [q.x as f32, q.y as f32, q.right() as f32, q.bottom() as f32],
                tex0: [
                    origin.0 as f32,
                    origin.1 as f32,
                    1.0 / tsize.0 as f32,
                    1.0 / tsize.1 as f32,
                ],
                tex1: chroma,
                view: view_vec(client, footprint as f32, n as f32),
                color: [bg.0, bg.1, bg.2, 1.0],
                params: [sub.0, sub.1, mode, if opaque { 1.0 } else { 0.0 }],
                cms,
            },
        ));
    }
    out
}

/// A UI shape (overlay pipeline, no textures).
pub fn shape(s: &Shape, client: (u32, u32)) -> DrawCb {
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
    DrawCb {
        m0: [1.0, 0.0, 0.0, 0.0],
        m1: [0.0, 1.0, 0.0, 0.0],
        rect: rect_vec(&bounds),
        tex1: geom,
        view: view_vec(client, 1.0, 1.0),
        color: [color.0, color.1, color.2, color.3],
        params,
        ..Default::default()
    }
}

/// An atlas thumbnail into `dest` (overlay pipeline, the page's Y and
/// Cb/Cr textures).
pub fn thumb(t: &Placed, dest: &Rect, alpha: f32, client: (u32, u32)) -> DrawCb {
    let (w, h) = (t.size.0 as f64, t.size.1 as f64);
    let (ow, oh) = t.orientation.oriented_size(w, h);
    let xform = t
        .orientation
        .transform(w, h)
        .then(&Affine::scale(dest.w / ow, dest.h / oh))
        .then(&Affine::translate(dest.x, dest.y));
    let page = PAGE as f32;
    let (cw, ch) = (t.size.0.div_ceil(2) as f32, t.size.1.div_ceil(2) as f32);
    DrawCb {
        m0: [xform.a as f32, xform.b as f32, xform.c as f32, 0.0],
        m1: [xform.d as f32, xform.e as f32, xform.f as f32, 0.0],
        rect: [0.0, 0.0, w as f32, h as f32],
        tex0: [
            -(t.origin.0 as f32),
            -(t.origin.1 as f32),
            1.0 / page,
            1.0 / page,
        ],
        tex1: [w as f32, h as f32, cw, ch],
        view: view_vec(client, 1.0, 1.0),
        color: [1.0, 1.0, 1.0, alpha],
        params: [4.0, 0.0, 0.0, 0.0],
        ..Default::default()
    }
}

/// A text texture of `size` at `pos` (overlay pipeline, coverage in the
/// texture's alpha).
pub fn text(pos: (f32, f32), size: (u32, u32), color: Rgba, client: (u32, u32)) -> DrawCb {
    DrawCb {
        m0: [1.0, 0.0, 0.0, 0.0],
        m1: [0.0, 1.0, 0.0, 0.0],
        rect: [pos.0, pos.1, pos.0 + size.0 as f32, pos.1 + size.1 as f32],
        tex0: [
            pos.0,
            pos.1,
            1.0 / size.0.max(1) as f32,
            1.0 / size.1.max(1) as f32,
        ],
        view: view_vec(client, 1.0, 1.0),
        color: [color.0, color.1, color.2, color.3],
        params: [3.0, 0.0, 0.0, 0.0],
        ..Default::default()
    }
}

/// A value in `0..=1` as IEEE half float (3D LUT textures).
pub fn f16(v: f32) -> u16 {
    // Values in 0..1: no infinities or NaNs to care about.
    let bits = v.clamp(0.0, 1.0).to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    let mant = bits & 0x7F_FFFF;
    if exp <= 0 {
        // Subnormal (or zero).
        if exp < -10 {
            return 0;
        }
        let m = (mant | 0x80_0000) >> (1 - exp);
        return ((m + 0x1000) >> 13) as u16;
    }
    let h = ((exp as u32) << 10) | (mant >> 13);
    // Round to nearest.
    (h + ((mant >> 12) & 1)) as u16
}

/// A 3D LUT as RGBA16F texels (red fastest, then green, then blue).
pub fn lut_texels(lut: &blinzel_core::lut::Lut3d) -> Vec<u16> {
    lut.nodes()
        .iter()
        .flat_map(|c| {
            [
                f16(c[0] as f32 / 65535.0),
                f16(c[1] as f32 / 65535.0),
                f16(c[2] as f32 / 65535.0),
                f16(1.0),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(f16(0.0), 0);
        assert_eq!(f16(1.0), 0x3C00);
        assert_eq!(f16(0.5), 0x3800);
        assert_eq!(f16(2.0), 0x3C00);
        // Smallest normal half: 2^-14.
        assert_eq!(f16(6.103_515_6e-5), 0x0400);
    }

    #[test]
    fn image_draws_skip_invisible_tiles() {
        let tiles = [
            TileGeom {
                rect: [0, 0, 100, 50],
                main: ((0, 0), (104, 54)),
                chroma: None,
            },
            TileGeom {
                rect: [0, 50, 100, 100],
                main: ((0, 46), (104, 54)),
                chroma: None,
            },
        ];
        let d = image(
            ImageKind::Rgba { opaque: true },
            (100, 100),
            &tiles,
            &Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 100.0, 40.0),
            Rgba(0.0, 0.0, 0.0, 1.0),
            Some(33),
            (200, 100),
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].0, 0);
        assert_eq!(d[0].1.rect, [0.0, 0.0, 100.0, 40.0]);
        assert_eq!(d[0].1.view[..2], [0.01, 0.02]);
        assert_eq!(d[0].1.cms[0], 1.0);
    }
}
