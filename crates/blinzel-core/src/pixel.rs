//! Decoded pixel data as it travels to the GPU.

use crate::bufpool::PoolBuf;
use crate::geom::PixelRect;
use crate::kernels::{self, Level};

/// Packed pixel layouts the renderer uploads without conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PackedFormat {
    /// B, G, R, A bytes (DXGI `B8G8R8A8`).
    Bgra8,
    /// R, G, B, A bytes (DXGI `R8G8B8A8`).
    Rgba8,
    /// One luminance byte.
    Gray8,
}

impl PackedFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PackedFormat::Bgra8 | PackedFormat::Rgba8 => 4,
            PackedFormat::Gray8 => 1,
        }
    }
}

/// What the alpha channel means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Alpha {
    /// Opaque or no alpha: the channel (if any) is ignored.
    Opaque,
    /// Straight (non-premultiplied) alpha.
    Straight,
}

/// One plane of 8-bit samples.
#[derive(Debug)]
pub struct Plane {
    pub data: PoolBuf,
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub stride: usize,
}

impl Plane {
    /// A plane of `width × height` with `bpp` bytes per pixel and tight rows.
    pub fn new(width: u32, height: u32, bpp: usize) -> Plane {
        let stride = width as usize * bpp;
        Plane {
            data: PoolBuf::new(stride * height as usize),
            width,
            height,
            stride,
        }
    }

    pub fn row(&self, y: u32) -> &[u8] {
        let s = y as usize * self.stride;
        &self.data[s..s + self.stride]
    }

    pub fn row_mut(&mut self, y: u32) -> &mut [u8] {
        let s = y as usize * self.stride;
        &mut self.data[s..s + self.stride]
    }

    pub fn bytes(&self) -> usize {
        self.data.len()
    }
}

/// Pixel payload.
#[derive(Debug)]
pub enum Pixels {
    Packed {
        plane: Plane,
        format: PackedFormat,
        alpha: Alpha,
    },
    /// Full-range BT.601 YCbCr (JFIF) planes; chroma planes may be
    /// subsampled. Upsampling and colour conversion happen in the shader.
    Ycc {
        y: Plane,
        cb: Plane,
        cr: Plane,
        sub: ChromaSub,
    },
}

/// Chroma subsampling factors: one chroma sample per `h × v` luma samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChromaSub {
    pub h: u8,
    pub v: u8,
}

impl ChromaSub {
    /// Factor that maps normalized luma coordinates onto the chroma plane,
    /// which may cover a padded luma area for odd sizes:
    /// `luma_size / (chroma_size · factor)`.
    pub fn uv_scale(self, luma: (u32, u32), chroma: (u32, u32)) -> (f32, f32) {
        (
            luma.0 as f32 / (chroma.0 as f32 * self.h as f32),
            luma.1 as f32 / (chroma.1 as f32 * self.v as f32),
        )
    }
}

/// A decoded image at some resolution ("stage") of a source file.
#[derive(Debug)]
pub struct Image {
    pub pixels: Pixels,
    /// Size of this stage in pixels (luma size for YCbCr).
    pub width: u32,
    pub height: u32,
    /// Full-resolution size of the source, stored orientation.
    pub full_width: u32,
    pub full_height: u32,
    /// For region decodes: the full-resolution rectangle this image covers.
    pub region: Option<PixelRect>,
}

impl Image {
    pub fn bytes(&self) -> usize {
        match &self.pixels {
            Pixels::Packed { plane, .. } => plane.bytes(),
            Pixels::Ycc { y, cb, cr, .. } => y.bytes() + cb.bytes() + cr.bytes(),
        }
    }

    pub fn is_full_resolution(&self) -> bool {
        self.width == self.full_width && self.height == self.full_height
    }

    pub fn has_alpha(&self) -> bool {
        matches!(
            self.pixels,
            Pixels::Packed {
                alpha: Alpha::Straight,
                ..
            }
        )
    }

    /// Converts to tightly packed BGRA (straight alpha). Used for the
    /// clipboard and tests; the display path never needs this.
    pub fn to_bgra(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; w * h * 4];
        match &self.pixels {
            Pixels::Packed {
                plane,
                format,
                alpha,
            } => {
                for y in 0..h {
                    let src = plane.row(y as u32);
                    let dst = &mut out[y * w * 4..(y + 1) * w * 4];
                    match format {
                        PackedFormat::Bgra8 => dst.copy_from_slice(&src[..w * 4]),
                        PackedFormat::Rgba8 => kernels::rgba_to_bgra(Level::Baseline, src, dst),
                        PackedFormat::Gray8 => {
                            for (d, &g) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                                *d = [g, g, g, 255];
                            }
                        }
                    }
                    if *alpha == Alpha::Opaque {
                        for d in dst.as_chunks_mut::<4>().0 {
                            d[3] = 255;
                        }
                    }
                }
            }
            Pixels::Ycc { y: yp, cb, cr, sub } => {
                for y in 0..h {
                    let dst = &mut out[y * w * 4..(y + 1) * w * 4];
                    kernels::ycc_to_bgra_row(
                        Level::Baseline,
                        [yp, cb, cr],
                        *sub,
                        h as u32,
                        y as u32,
                        dst,
                    );
                }
            }
        }
        out
    }
}

/// Full-range BT.601 (JFIF) to RGB; inputs and outputs in 0..1. The shader
/// uses the same constants.
#[inline]
pub fn ycc_to_rgb(y: f32, cb: f32, cr: f32) -> [f32; 3] {
    let (cb, cr) = (cb - 0.5, cr - 0.5);
    [
        y + 1.402 * cr,
        y - 0.344_136 * cb - 0.714_136 * cr,
        y + 1.772 * cb,
    ]
}

/// [`ycc_to_rgb`] in 16.16 fixed point on 8-bit values (fast paths: the
/// CPU first frame, thumbnails).
#[inline(always)]
pub fn ycc_to_rgb_u8(y: i32, cb: i32, cr: i32) -> [u8; 3] {
    let (cb, cr) = (cb - 128, cr - 128);
    let c = |v: i32| v.clamp(0, 255) as u8;
    [
        c(y + ((91_881 * cr + 32_768) >> 16)),
        c(y - ((22_554 * cb + 46_802 * cr - 32_768) >> 16)),
        c(y + ((116_130 * cb + 32_768) >> 16)),
    ]
}

#[inline]
pub fn to_u8(v: f32) -> u8 {
    (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Bilinear sample of a plane at normalized coordinates (texel centers at
/// `(i + 0.5) / size`, clamped at the edges) — what the GPU sampler does.
#[inline]
pub fn sample_plane(p: &Plane, u: f32, v: f32) -> f32 {
    let (pw, ph) = (p.width as f32, p.height as f32);
    let tx = u * pw - 0.5;
    let ty = v * ph - 0.5;
    let x0 = tx.floor();
    let y0 = ty.floor();
    let (fx, fy) = (tx - x0, ty - y0);
    let get = |x: f32, y: f32| {
        let xi = x.clamp(0.0, pw - 1.0) as u32;
        let yi = y.clamp(0.0, ph - 1.0) as u32;
        p.row(yi)[xi as usize] as f32 / 255.0
    };
    let top = get(x0, y0) * (1.0 - fx) + get(x0 + 1.0, y0) * fx;
    let bot = get(x0, y0 + 1.0) * (1.0 - fx) + get(x0 + 1.0, y0 + 1.0) * fx;
    top * (1.0 - fy) + bot * fy
}
