//! JPEG through libjpeg-turbo's TurboJPEG 3 API: DCT-scaled decoding
//! straight into YCbCr planes (the GPU does upsampling and colour
//! conversion), cropped decoding for regions.
//!
//! The FFI declarations are our own: the pregenerated bindings of
//! `turbojpeg-sys` declare `size_t` as `c_ulong`, which is 32 bits on Windows
//! and would corrupt memory where a `size_t*` is written. The crate is still
//! used to build and link the vendored library (with SIMD required).

use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_uchar, c_void};

use super::{DecodeError, Request, Result, check_size};
use crate::geom::PixelRect;
use crate::pixel::{Alpha, ChromaSub, Image, PackedFormat, Pixels, Plane};

// Pull in the static library.
use turbojpeg_sys as _;

#[allow(non_snake_case)]
mod ffi {
    use super::*;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct ScalingFactor {
        pub num: c_int,
        pub denom: c_int,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Region {
        pub x: c_int,
        pub y: c_int,
        pub w: c_int,
        pub h: c_int,
    }

    /// tjtransform; `custom_filter` is unused (NULL).
    #[repr(C)]
    pub struct Transform {
        pub r: Region,
        pub op: c_int,
        pub options: c_int,
        pub data: *mut c_void,
        pub custom_filter: *const c_void,
    }

    pub const INIT_COMPRESS: c_int = 0;
    pub const INIT_DECOMPRESS: c_int = 1;
    pub const INIT_TRANSFORM: c_int = 2;

    pub const XOPT_CROP: c_int = 1 << 2;

    pub const P_QUALITY: c_int = 3;
    pub const P_SUBSAMP: c_int = 4;
    pub const P_JPEGWIDTH: c_int = 5;
    pub const P_JPEGHEIGHT: c_int = 6;
    pub const P_PRECISION: c_int = 7;
    pub const P_COLORSPACE: c_int = 8;
    pub const P_PROGRESSIVE: c_int = 12;
    pub const P_LOSSLESS: c_int = 15;
    pub const P_RESTARTROWS: c_int = 19;
    pub const P_MAXPIXELS: c_int = 24;

    pub const PF_RGB: c_int = 0;
    pub const PF_GRAY: c_int = 6;
    pub const PF_BGRA: c_int = 8;
    pub const PF_CMYK: c_int = 11;

    pub const SAMP_444: c_int = 0;
    pub const SAMP_422: c_int = 1;
    pub const SAMP_420: c_int = 2;
    pub const SAMP_GRAY: c_int = 3;
    pub const SAMP_440: c_int = 4;

    pub const CS_YCBCR: c_int = 1;
    pub const CS_GRAY: c_int = 2;
    pub const CS_CMYK: c_int = 3;
    pub const CS_YCCK: c_int = 4;

    pub const ERR_WARNING: c_int = 0;

    unsafe extern "C" {
        pub fn tj3Init(init_type: c_int) -> *mut c_void;
        pub fn tj3Destroy(handle: *mut c_void);
        pub fn tj3Set(handle: *mut c_void, param: c_int, value: c_int) -> c_int;
        pub fn tj3Get(handle: *mut c_void, param: c_int) -> c_int;
        pub fn tj3GetErrorStr(handle: *mut c_void) -> *mut c_char;
        pub fn tj3GetErrorCode(handle: *mut c_void) -> c_int;
        pub fn tj3Free(buffer: *mut c_void);
        pub fn tj3DecompressHeader(handle: *mut c_void, jpeg: *const c_uchar, size: usize)
        -> c_int;
        pub fn tj3SetScalingFactor(handle: *mut c_void, factor: ScalingFactor) -> c_int;
        pub fn tj3SetCroppingRegion(handle: *mut c_void, region: Region) -> c_int;
        pub fn tj3Decompress8(
            handle: *mut c_void,
            jpeg: *const c_uchar,
            size: usize,
            dst: *mut c_uchar,
            pitch: c_int,
            pixel_format: c_int,
        ) -> c_int;
        pub fn tj3DecompressToYUVPlanes8(
            handle: *mut c_void,
            jpeg: *const c_uchar,
            size: usize,
            planes: *mut *mut c_uchar,
            strides: *mut c_int,
        ) -> c_int;
        pub fn tj3YUVPlaneWidth(component: c_int, width: c_int, subsamp: c_int) -> c_int;
        pub fn tj3YUVPlaneHeight(component: c_int, height: c_int, subsamp: c_int) -> c_int;
        pub fn tj3SetICCProfile(handle: *mut c_void, icc: *mut c_uchar, size: usize) -> c_int;
        pub fn tj3Transform(
            handle: *mut c_void,
            jpeg: *const c_uchar,
            size: usize,
            n: c_int,
            dst: *mut *mut c_uchar,
            dst_sizes: *mut usize,
            transforms: *const Transform,
        ) -> c_int;
        pub fn tj3Compress8(
            handle: *mut c_void,
            src: *const c_uchar,
            width: c_int,
            pitch: c_int,
            height: c_int,
            pixel_format: c_int,
            jpeg: *mut *mut c_uchar,
            size: *mut usize,
        ) -> c_int;
    }
}

/// An owned TurboJPEG instance.
struct Tj(*mut c_void);

impl Tj {
    fn new(kind: c_int) -> Result<Tj> {
        let h = unsafe { ffi::tj3Init(kind) };
        if h.is_null() {
            return Err(DecodeError::Corrupt("tj3Init failed".into()));
        }
        Ok(Tj(h))
    }

    fn set(&self, param: c_int, value: c_int) -> Result<()> {
        if unsafe { ffi::tj3Set(self.0, param, value) } != 0 {
            return Err(self.error());
        }
        Ok(())
    }

    fn get(&self, param: c_int) -> c_int {
        unsafe { ffi::tj3Get(self.0, param) }
    }

    fn error(&self) -> DecodeError {
        let s = unsafe { CStr::from_ptr(ffi::tj3GetErrorStr(self.0)) };
        DecodeError::Corrupt(s.to_string_lossy().into_owned())
    }

    /// Maps a decompression return code; warnings (truncated data and the
    /// like) still produce an image, which is better than none.
    fn check_decode(&self, rc: c_int) -> Result<()> {
        if rc == 0 || unsafe { ffi::tj3GetErrorCode(self.0) } == ffi::ERR_WARNING {
            Ok(())
        } else {
            Err(self.error())
        }
    }
}

impl Drop for Tj {
    fn drop(&mut self) {
        unsafe { ffi::tj3Destroy(self.0) }
    }
}

thread_local! {
    /// One decompressor per worker thread, created on first use.
    static DECOMPRESSOR: RefCell<Option<Tj>> = const { RefCell::new(None) };
}

fn with_decompressor<T>(f: impl FnOnce(&Tj) -> Result<T>) -> Result<T> {
    DECOMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let tj = Tj::new(ffi::INIT_DECOMPRESS)?;
            tj.set(
                ffi::P_MAXPIXELS,
                super::MAX_PIXELS.min(i32::MAX as u64) as c_int,
            )?;
            *slot = Some(tj);
        }
        f(slot.as_ref().unwrap())
    })
}

struct Header {
    width: u32,
    height: u32,
    subsamp: c_int,
    colorspace: c_int,
    lossless: bool,
    precision: c_int,
}

fn read_header(tj: &Tj, data: &[u8]) -> Result<(Header, blinzel_jpeg::JpegInfo)> {
    // Our own strict parse first: on a truncated header TurboJPEG only warns
    // and leaves the previous image's parameters on the reused handle.
    let info = blinzel_jpeg::parse(data).map_err(|e| DecodeError::Corrupt(e.to_string()))?;
    if unsafe { ffi::tj3DecompressHeader(tj.0, data.as_ptr(), data.len()) } != 0 {
        return Err(tj.error());
    }
    let h = Header {
        width: tj.get(ffi::P_JPEGWIDTH).max(0) as u32,
        height: tj.get(ffi::P_JPEGHEIGHT).max(0) as u32,
        subsamp: tj.get(ffi::P_SUBSAMP),
        colorspace: tj.get(ffi::P_COLORSPACE),
        lossless: tj.get(ffi::P_LOSSLESS) == 1,
        precision: tj.get(ffi::P_PRECISION),
    };
    check_size(h.width as u64, h.height as u64)?;
    if (h.width, h.height) != (info.width, info.height) {
        return Err(DecodeError::Corrupt("inconsistent JPEG header".into()));
    }
    Ok((h, info))
}

fn set_scale(tj: &Tj, num: u32) -> Result<()> {
    // TurboJPEG only accepts reduced fractions (1/2, not 4/8).
    let (mut a, mut b) = (num, 8u32);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let sf = ffi::ScalingFactor {
        num: (num / a) as c_int,
        denom: (8 / a) as c_int,
    };
    if unsafe { ffi::tj3SetScalingFactor(tj.0, sf) } != 0 {
        return Err(tj.error());
    }
    Ok(())
}

fn set_crop(tj: &Tj, r: ffi::Region) -> Result<()> {
    if unsafe { ffi::tj3SetCroppingRegion(tj.0, r) } != 0 {
        return Err(tj.error());
    }
    Ok(())
}

const UNCROPPED: ffi::Region = ffi::Region {
    x: 0,
    y: 0,
    w: 0,
    h: 0,
};

/// Decodes a JPEG file.
pub fn decode(data: &[u8], req: Request) -> Result<Image> {
    with_decompressor(|tj| {
        let (hdr, info) = read_header(tj, data)?;
        let inverted_cmyk = info.adobe.is_some();
        if hdr.precision != 8 {
            // 12/16-bit JPEGs are rare; the platform fallback handles them.
            return Err(DecodeError::Unsupported(crate::format::Format::Jpeg));
        }
        if let Request::Region(r) = req {
            return decode_region(tj, data, &hdr, r, inverted_cmyk);
        }
        let num = match req {
            Request::Fit { w, h } if !hdr.lossless => (1..=8u32)
                .find(|&n| scaled(hdr.width, n) >= w && scaled(hdr.height, n) >= h)
                .unwrap_or(8),
            _ => 8,
        };
        set_scale(tj, num)?;
        set_crop(tj, UNCROPPED)?;
        let (w, h) = (scaled(hdr.width, num), scaled(hdr.height, num));
        let planar = hdr.colorspace == ffi::CS_YCBCR
            && !hdr.lossless
            && matches!(
                hdr.subsamp,
                ffi::SAMP_444 | ffi::SAMP_422 | ffi::SAMP_420 | ffi::SAMP_440
            );
        let pixels = if planar {
            decode_planes(tj, data, w, h, hdr.subsamp)?
        } else if hdr.colorspace == ffi::CS_GRAY || hdr.subsamp == ffi::SAMP_GRAY {
            let mut plane = Plane::new(w, h, 1);
            decode_packed(tj, data, &mut plane, ffi::PF_GRAY)?;
            Pixels::Packed {
                plane,
                format: PackedFormat::Gray8,
                alpha: Alpha::Opaque,
            }
        } else if matches!(hdr.colorspace, ffi::CS_CMYK | ffi::CS_YCCK) {
            let mut cmyk = Plane::new(w, h, 4);
            decode_packed(tj, data, &mut cmyk, ffi::PF_CMYK)?;
            cmyk_to_bgra_in_place(&mut cmyk, inverted_cmyk);
            Pixels::Packed {
                plane: cmyk,
                format: PackedFormat::Bgra8,
                alpha: Alpha::Opaque,
            }
        } else {
            let mut plane = Plane::new(w, h, 4);
            decode_packed(tj, data, &mut plane, ffi::PF_BGRA)?;
            Pixels::Packed {
                plane,
                format: PackedFormat::Bgra8,
                alpha: Alpha::Opaque,
            }
        };
        Ok(Image {
            pixels,
            width: w,
            height: h,
            full_width: hdr.width,
            full_height: hdr.height,
            region: None,
        })
    })
}

fn scaled(dim: u32, num: u32) -> u32 {
    (dim * num).div_ceil(8)
}

fn decode_packed(tj: &Tj, data: &[u8], plane: &mut Plane, pf: c_int) -> Result<()> {
    let rc = unsafe {
        ffi::tj3Decompress8(
            tj.0,
            data.as_ptr(),
            data.len(),
            plane.data.as_mut_ptr(),
            plane.stride as c_int,
            pf,
        )
    };
    tj.check_decode(rc)
}

fn decode_planes(tj: &Tj, data: &[u8], w: u32, h: u32, subsamp: c_int) -> Result<Pixels> {
    let dims = |c: c_int| unsafe {
        (
            ffi::tj3YUVPlaneWidth(c, w as c_int, subsamp).max(1) as u32,
            ffi::tj3YUVPlaneHeight(c, h as c_int, subsamp).max(1) as u32,
        )
    };
    let (yw, yh) = dims(0);
    let (cw, ch) = dims(1);
    let mut y = Plane::new(yw, yh, 1);
    let mut cb = Plane::new(cw, ch, 1);
    let mut cr = Plane::new(cw, ch, 1);
    let mut ptrs = [
        y.data.as_mut_ptr(),
        cb.data.as_mut_ptr(),
        cr.data.as_mut_ptr(),
    ];
    let mut strides = [y.stride as c_int, cb.stride as c_int, cr.stride as c_int];
    let rc = unsafe {
        ffi::tj3DecompressToYUVPlanes8(
            tj.0,
            data.as_ptr(),
            data.len(),
            ptrs.as_mut_ptr(),
            strides.as_mut_ptr(),
        )
    };
    tj.check_decode(rc)?;
    let sub = match subsamp {
        ffi::SAMP_444 => ChromaSub { h: 1, v: 1 },
        ffi::SAMP_422 => ChromaSub { h: 2, v: 1 },
        ffi::SAMP_420 => ChromaSub { h: 2, v: 2 },
        _ => ChromaSub { h: 1, v: 2 },
    };
    // Luma planes are padded to whole MCUs only in the plane *buffers*; the
    // plane width reported by TurboJPEG equals the image width.
    y.width = w.min(yw);
    y.height = h.min(yh);
    Ok(Pixels::Ycc { y, cb, cr, sub })
}

/// Decodes a full-resolution rectangle as BGRA. TurboJPEG needs the left
/// edge on an iMCU boundary, so a slightly wider strip is decoded and the
/// exact rectangle copied out.
fn decode_region(
    tj: &Tj,
    data: &[u8],
    hdr: &Header,
    r: PixelRect,
    inverted_cmyk: bool,
) -> Result<Image> {
    if r.w == 0 || r.h == 0 || r.right() > hdr.width || r.bottom() > hdr.height {
        return Err(DecodeError::Corrupt(format!(
            "region {r:?} outside {}×{}",
            hdr.width, hdr.height
        )));
    }
    set_scale(tj, 8)?;
    let mcu_w = match hdr.subsamp {
        ffi::SAMP_422 | ffi::SAMP_420 => 16,
        5 /* 411 */ => 32,
        _ => 8,
    };
    let x0 = if hdr.lossless { 0 } else { r.x / mcu_w * mcu_w };
    let strip = ffi::Region {
        x: x0 as c_int,
        y: r.y as c_int,
        w: (r.right() - x0) as c_int,
        h: r.h as c_int,
    };
    let is_cmyk = matches!(hdr.colorspace, ffi::CS_CMYK | ffi::CS_YCCK);
    let pf = if is_cmyk { ffi::PF_CMYK } else { ffi::PF_BGRA };
    let result = if hdr.lossless {
        set_crop(tj, UNCROPPED).and_then(|_| {
            let mut full = Plane::new(hdr.width, hdr.height, 4);
            decode_packed(tj, data, &mut full, pf).map(|_| (full, r.x, r.y))
        })
    } else {
        set_crop(tj, strip).and_then(|_| {
            let mut s = Plane::new(strip.w as u32, r.h, 4);
            decode_packed(tj, data, &mut s, pf).map(|_| (s, r.x - x0, 0))
        })
    };
    // Never leave a cropping region behind on the shared handle.
    set_crop(tj, UNCROPPED)?;
    let (mut src, ox, oy) = result?;
    if is_cmyk {
        cmyk_to_bgra_in_place(&mut src, inverted_cmyk);
    }
    let mut out = Plane::new(r.w, r.h, 4);
    for row in 0..r.h {
        let s = &src.row(oy + row)[ox as usize * 4..(ox + r.w) as usize * 4];
        out.row_mut(row).copy_from_slice(s);
    }
    Ok(Image {
        pixels: Pixels::Packed {
            plane: out,
            format: PackedFormat::Bgra8,
            alpha: Alpha::Opaque,
        },
        width: r.w,
        height: r.h,
        full_width: hdr.width,
        full_height: hdr.height,
        region: Some(r),
    })
}

/// Adobe (Photoshop) CMYK JPEGs store inverted values.
fn cmyk_to_bgra_in_place(p: &mut Plane, inverted: bool) {
    for y in 0..p.height {
        let w = p.width as usize;
        for px in p.row_mut(y)[..w * 4].as_chunks_mut::<4>().0.iter_mut() {
            let (mut c, mut m, mut ye, mut k) =
                (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
            if !inverted {
                (c, m, ye, k) = (255 - c, 255 - m, 255 - ye, 255 - k);
            }
            // With inverted storage, R = C·K/255 etc.
            px[0] = (ye * k / 255) as u8;
            px[1] = (m * k / 255) as u8;
            px[2] = (c * k / 255) as u8;
            px[3] = 255;
        }
    }
}

/// Options for [`encode_rgb`] (test images and the benchmark corpus).
#[derive(Clone, Copy, Debug)]
pub struct EncodeOptions {
    pub quality: u8,
    /// 444, 422, 420 or 440; ignored for gray.
    pub subsampling: u16,
    pub progressive: bool,
    /// Restart marker every N MCU rows (0 = none).
    pub restart_rows: u16,
    pub gray: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            quality: 90,
            subsampling: 420,
            progressive: false,
            restart_rows: 0,
            gray: false,
        }
    }
}

/// Encodes tightly packed RGB (or gray if `opts.gray`) as JPEG.
pub fn encode_rgb(pixels: &[u8], width: u32, height: u32, opts: EncodeOptions) -> Result<Vec<u8>> {
    let bpp = if opts.gray { 1 } else { 3 };
    assert_eq!(pixels.len(), width as usize * height as usize * bpp);
    let tj = Tj::new(ffi::INIT_COMPRESS)?;
    tj.set(ffi::P_QUALITY, opts.quality as c_int)?;
    let samp = if opts.gray {
        ffi::SAMP_GRAY
    } else {
        match opts.subsampling {
            444 => ffi::SAMP_444,
            422 => ffi::SAMP_422,
            440 => ffi::SAMP_440,
            _ => ffi::SAMP_420,
        }
    };
    tj.set(ffi::P_SUBSAMP, samp)?;
    tj.set(ffi::P_PROGRESSIVE, opts.progressive as c_int)?;
    tj.set(ffi::P_RESTARTROWS, opts.restart_rows as c_int)?;
    let mut out: *mut c_uchar = std::ptr::null_mut();
    let mut size: usize = 0;
    let pf = if opts.gray { ffi::PF_GRAY } else { ffi::PF_RGB };
    let rc = unsafe {
        ffi::tj3Compress8(
            tj.0,
            pixels.as_ptr(),
            width as c_int,
            (width as usize * bpp) as c_int,
            height as c_int,
            pf,
            &mut out,
            &mut size,
        )
    };
    take_output(&tj, rc, out, size)
}

/// Compresses tightly packed BGRA (alpha ignored) at `quality` with the
/// given subsampling (444/422/420/440), embedding `icc` if given.
pub fn encode_bgra(
    pixels: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    subsampling: u16,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>> {
    assert_eq!(pixels.len(), width as usize * height as usize * 4);
    let tj = Tj::new(ffi::INIT_COMPRESS)?;
    tj.set(ffi::P_QUALITY, quality as c_int)?;
    let samp = match subsampling {
        444 => ffi::SAMP_444,
        422 => ffi::SAMP_422,
        440 => ffi::SAMP_440,
        _ => ffi::SAMP_420,
    };
    tj.set(ffi::P_SUBSAMP, samp)?;
    if let Some(icc) = icc {
        let mut copy = icc.to_vec();
        if unsafe { ffi::tj3SetICCProfile(tj.0, copy.as_mut_ptr(), copy.len()) } != 0 {
            return Err(tj.error());
        }
    }
    let mut out: *mut c_uchar = std::ptr::null_mut();
    let mut size: usize = 0;
    let rc = unsafe {
        ffi::tj3Compress8(
            tj.0,
            pixels.as_ptr(),
            width as c_int,
            (width * 4) as c_int,
            height as c_int,
            ffi::PF_BGRA,
            &mut out,
            &mut size,
        )
    };
    take_output(&tj, rc, out, size)
}

/// Copies a TurboJPEG-allocated output buffer into a Vec and frees it.
fn take_output(tj: &Tj, rc: c_int, out: *mut c_uchar, size: usize) -> Result<Vec<u8>> {
    if rc != 0 {
        if !out.is_null() {
            unsafe { ffi::tj3Free(out.cast()) };
        }
        return Err(tj.error());
    }
    let v = unsafe { std::slice::from_raw_parts(out, size) }.to_vec();
    unsafe { ffi::tj3Free(out.cast()) };
    Ok(v)
}

/// Lossless crop (no re-encoding): `r` must start on an iMCU boundary of
/// the stored image. All markers (EXIF, ICC, …) are copied unchanged.
pub fn lossless_crop(data: &[u8], r: PixelRect) -> Result<Vec<u8>> {
    let tj = Tj::new(ffi::INIT_TRANSFORM)?;
    let t = ffi::Transform {
        r: ffi::Region {
            x: r.x as c_int,
            y: r.y as c_int,
            w: r.w as c_int,
            h: r.h as c_int,
        },
        op: 0,
        options: ffi::XOPT_CROP,
        data: std::ptr::null_mut(),
        custom_filter: std::ptr::null(),
    };
    let mut out: *mut c_uchar = std::ptr::null_mut();
    let mut size: usize = 0;
    let rc =
        unsafe { ffi::tj3Transform(tj.0, data.as_ptr(), data.len(), 1, &mut out, &mut size, &t) };
    take_output(&tj, rc, out, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg;

    fn jpeg(w: u32, h: u32, opts: EncodeOptions) -> Vec<u8> {
        let rgb = if opts.gray {
            testimg::gray(w, h)
        } else {
            testimg::rgb(w, h)
        };
        encode_rgb(&rgb, w, h, opts).unwrap()
    }

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        let mse = a
            .iter()
            .zip(b)
            .map(|(x, y)| (*x as f64 - *y as f64).powi(2))
            .sum::<f64>()
            / a.len() as f64;
        if mse == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0f64 * 255.0 / mse).log10()
        }
    }

    #[test]
    fn planar_decode_matches_packed() {
        for sub in [444u16, 422, 420, 440] {
            let data = jpeg(
                203,
                117,
                EncodeOptions {
                    subsampling: sub,
                    ..Default::default()
                },
            );
            let img = decode(&data, Request::Full).unwrap();
            assert!(matches!(img.pixels, Pixels::Ycc { .. }), "{sub}");
            assert_eq!((img.width, img.height), (203, 117));
            assert!(img.is_full_resolution());
            // Reference: TurboJPEG's own colour conversion of the whole image.
            let r = decode(&data, Request::Region(PixelRect::new(0, 0, 203, 117))).unwrap();
            let p = psnr(&img.to_bgra(), &r.to_bgra());
            assert!(p > 35.0, "{sub}: psnr {p}");
        }
    }

    #[test]
    fn dct_scaling_picks_smallest_sufficient_size() {
        let data = jpeg(1600, 1200, EncodeOptions::default());
        let img = decode(&data, Request::Fit { w: 400, h: 300 }).unwrap();
        assert_eq!((img.width, img.height), (400, 300));
        assert_eq!((img.full_width, img.full_height), (1600, 1200));
        let img = decode(&data, Request::Fit { w: 401, h: 10 }).unwrap();
        assert_eq!((img.width, img.height), (600, 450));
        let img = decode(&data, Request::Fit { w: 1, h: 1 }).unwrap();
        assert_eq!((img.width, img.height), (200, 150));
        if let Pixels::Ycc { cb, sub, .. } = &img.pixels {
            assert_eq!((cb.width, cb.height), (100, 75));
            assert_eq!(*sub, ChromaSub { h: 2, v: 2 });
        } else {
            panic!("expected planes");
        }
    }

    #[test]
    fn region_equals_crop_of_full_decode() {
        let data = jpeg(
            321,
            200,
            EncodeOptions {
                subsampling: 420,
                ..Default::default()
            },
        );
        let full = decode(&data, Request::Region(PixelRect::new(0, 0, 321, 200)))
            .unwrap()
            .to_bgra();
        for r in [
            PixelRect::new(37, 11, 100, 50),
            PixelRect::new(0, 0, 5, 5),
            PixelRect::new(300, 190, 21, 10),
        ] {
            let img = decode(&data, Request::Region(r)).unwrap();
            assert_eq!(img.region, Some(r));
            let got = img.to_bgra();
            let mut want = Vec::new();
            for y in r.y..r.bottom() {
                let s = ((y * 321 + r.x) * 4) as usize;
                want.extend_from_slice(&full[s..s + r.w as usize * 4]);
            }
            // Upsampling near the strip edge may differ by a few levels.
            assert!(
                psnr(&got, &want) > 40.0,
                "{r:?}: psnr {}",
                psnr(&got, &want)
            );
        }
        assert!(decode(&data, Request::Region(PixelRect::new(300, 0, 30, 10))).is_err());
        // The handle is reusable after a region decode.
        assert_eq!(decode(&data, Request::Full).unwrap().width, 321);
    }

    #[test]
    fn gray_progressive_and_restart() {
        let g = decode(
            &jpeg(
                64,
                48,
                EncodeOptions {
                    gray: true,
                    ..Default::default()
                },
            ),
            Request::Full,
        )
        .unwrap();
        assert!(matches!(
            g.pixels,
            Pixels::Packed {
                format: PackedFormat::Gray8,
                ..
            }
        ));
        let p = jpeg(
            640,
            480,
            EncodeOptions {
                progressive: true,
                ..Default::default()
            },
        );
        assert_eq!(
            blinzel_jpeg::parse(&p).unwrap().process,
            blinzel_jpeg::Process::Progressive
        );
        assert_eq!(decode(&p, Request::Fit { w: 80, h: 60 }).unwrap().width, 80);
        let r = jpeg(
            640,
            480,
            EncodeOptions {
                restart_rows: 1,
                ..Default::default()
            },
        );
        assert!(blinzel_jpeg::parse(&r).unwrap().restart_interval > 0);
        assert!(decode(&r, Request::Full).is_ok());
    }

    #[test]
    fn truncated_files_still_decode_or_fail_cleanly() {
        let data = jpeg(300, 200, EncodeOptions::default());
        // Truncated entropy data: a warning, the visible part is decoded.
        assert!(decode(&data[..data.len() / 2], Request::Full).is_ok());
        // Truncated header: an error, no crash.
        assert!(decode(&data[..20], Request::Full).is_err());
    }

    #[test]
    fn cmyk_conversion() {
        let mut p = Plane::new(1, 1, 4);
        p.data.copy_from_slice(&[255, 0, 255, 255]); // inverted storage: only magenta ink
        cmyk_to_bgra_in_place(&mut p, true);
        assert_eq!(&p.data[..], &[255, 0, 255, 255]); // B=255, G=0, R=255: magenta
    }
}
