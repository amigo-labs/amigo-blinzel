//! JPEG through blinzel-jpeg: decoded in parallel straight into YCbCr
//! planes, byte-identical with the libjpeg-turbo path. The benchmark gate
//! decides when it is used: with several threads it is 3–4× faster than
//! libjpeg-turbo, on one thread slightly slower.

use super::Request;
use crate::bufpool::PoolBuf;
use crate::pixel::{Alpha, ChromaSub, Image, PackedFormat, Pixels, Plane};
use blinzel_jpeg::Subsampling;

/// Decodes `data` with up to `threads` threads, or returns `None` when
/// blinzel-jpeg does not handle the file or request (the caller then uses
/// another decoder). Region requests decode the whole image.
pub fn decode(data: &[u8], req: Request, threads: usize) -> Option<Image> {
    let info = blinzel_jpeg::parse(data).ok()?;
    if !blinzel_jpeg::is_supported(&info) {
        return None;
    }
    let sub = match info.subsampling() {
        Subsampling::Gray => None,
        _ if !info.is_ycbcr() => return None,
        Subsampling::S444 => Some(ChromaSub { h: 1, v: 1 }),
        Subsampling::S422 => Some(ChromaSub { h: 2, v: 1 }),
        Subsampling::S420 => Some(ChromaSub { h: 2, v: 2 }),
        Subsampling::S440 => Some(ChromaSub { h: 1, v: 2 }),
        _ => return None,
    };
    let num = match req {
        Request::Fit { w, h } => blinzel_jpeg::supported_scale(info.dct_scale_for(w, h)),
        Request::Full | Request::Region(_) => 8,
    };
    super::check_size(info.width as u64, info.height as u64).ok()?;

    let layouts = blinzel_jpeg::plane_layouts(&info, num);
    let mut bufs: Vec<PoolBuf> = layouts.iter().map(|l| PoolBuf::new(l.len())).collect();
    let mut outs: Vec<&mut [u8]> = bufs.iter_mut().map(|b| &mut b[..]).collect();
    // Errors (corrupt data) go to the other decoder, which may still
    // recover part of the image.
    blinzel_jpeg::decode_planes(data, num, &mut outs, threads).ok()?;

    let mut planes = bufs.into_iter().zip(&layouts).map(|(data, l)| Plane {
        data,
        width: l.width,
        height: l.height,
        stride: l.stride,
    });
    let y = planes.next()?;
    let (width, height) = (y.width, y.height);
    let pixels = match sub {
        None => Pixels::Packed {
            plane: y,
            format: PackedFormat::Gray8,
            alpha: Alpha::Opaque,
        },
        Some(sub) => Pixels::Ycc {
            y,
            cb: planes.next()?,
            cr: planes.next()?,
            sub,
        },
    };
    Some(Image {
        pixels,
        width,
        height,
        full_width: info.width,
        full_height: info.height,
        region: None,
    })
}
