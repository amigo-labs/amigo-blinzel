//! ICC profiles: reading the ones embedded in images, telling sRGB apart
//! from everything else, and writing small matrix profiles for tests and
//! generated test images.
//!
//! The colour transform itself is built by the platform (Windows Color
//! System); this module only decides whether one is needed.

use std::sync::Arc;

/// An embedded or display profile, a hash of its bytes (cache key) and
/// whether it is sRGB (see [`is_srgb`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub bytes: Arc<[u8]>,
    pub hash: u64,
    pub srgb: bool,
}

impl Profile {
    pub fn new(bytes: Vec<u8>) -> Profile {
        Profile {
            hash: fnv1a(&bytes),
            srgb: is_srgb(&bytes),
            bytes: bytes.into(),
        }
    }
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h = crate::filelist::Fnv::new();
    h.write(data);
    h.finish()
}

/// The profile embedded in a JPEG (APP2 `ICC_PROFILE` chunks).
pub fn from_jpeg(data: &[u8], info: &blinzel_jpeg::JpegInfo) -> Option<Profile> {
    let bytes = blinzel_jpeg::icc_profile(data, info)?;
    header(&bytes)?;
    Some(Profile::new(bytes))
}

/// The profile embedded in a PNG (`iCCP`, read without decoding pixels).
pub fn from_png(data: &[u8]) -> Option<Profile> {
    // Real profiles are far smaller; a crafted chunk cannot inflate to
    // more than this.
    let limits = png::Limits { bytes: 8 << 20 };
    let reader = png::Decoder::new_with_limits(std::io::Cursor::new(data), limits)
        .read_info()
        .ok()?;
    let bytes = reader.info().icc_profile.as_ref()?.to_vec();
    header(&bytes)?;
    Some(Profile::new(bytes))
}

/// Header fields that matter here.
struct Header {
    colour_space: [u8; 4],
    pcs: [u8; 4],
}

fn header(p: &[u8]) -> Option<Header> {
    if p.len() < 132 || &p[36..40] != b"acsp" {
        return None;
    }
    let size = be32(p, 0)? as usize;
    if size > p.len() || size < 132 {
        return None;
    }
    Some(Header {
        colour_space: p[16..20].try_into().ok()?,
        pcs: p[20..24].try_into().ok()?,
    })
}

fn be32(p: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(p.get(at..at + 4)?.try_into().ok()?))
}

fn be16(p: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(p.get(at..at + 2)?.try_into().ok()?))
}

fn s15f16(p: &[u8], at: usize) -> Option<f64> {
    Some(be32(p, at)? as i32 as f64 / 65536.0)
}

/// The data of tag `sig`.
fn tag<'a>(p: &'a [u8], sig: &[u8; 4]) -> Option<&'a [u8]> {
    let count = be32(p, 128)? as usize;
    (0..count.min(256)).find_map(|i| {
        let e = 132 + i * 12;
        if p.get(e..e + 4)? != sig {
            return None;
        }
        let (off, len) = (be32(p, e + 4)? as usize, be32(p, e + 8)? as usize);
        p.get(off..off.checked_add(len)?)
    })
}

fn xyz(p: &[u8], sig: &[u8; 4]) -> Option<[f64; 3]> {
    let t = tag(p, sig)?;
    if t.get(..4)? != b"XYZ " {
        return None;
    }
    Some([s15f16(t, 8)?, s15f16(t, 12)?, s15f16(t, 16)?])
}

/// A tone curve (`curv` or `para`), encoded value → linear.
enum Curve {
    Gamma(f64),
    Table(Vec<f64>),
    /// ICC parametric type 3 and its special cases: g, a, b, c, d.
    Param([f64; 5]),
}

impl Curve {
    fn value(&self, x: f64) -> f64 {
        match self {
            Curve::Gamma(g) => x.powf(*g),
            Curve::Table(t) => {
                let pos = x.clamp(0.0, 1.0) * (t.len() - 1) as f64;
                let i = (pos.floor() as usize).min(t.len() - 2);
                let f = pos - i as f64;
                t[i] * (1.0 - f) + t[i + 1] * f
            }
            Curve::Param([g, a, b, c, d]) => {
                if x >= *d {
                    (a * x + b).max(0.0).powf(*g)
                } else {
                    c * x
                }
            }
        }
    }
}

fn curve(p: &[u8], sig: &[u8; 4]) -> Option<Curve> {
    let t = tag(p, sig)?;
    match t.get(..4)? {
        b"curv" => {
            let n = be32(t, 8)? as usize;
            match n {
                0 => Some(Curve::Gamma(1.0)),
                1 => Some(Curve::Gamma(be16(t, 12)? as f64 / 256.0)),
                _ => {
                    let table = (0..n)
                        .map(|i| be16(t, 12 + 2 * i).map(|v| v as f64 / 65535.0))
                        .collect::<Option<Vec<_>>>()?;
                    Some(Curve::Table(table))
                }
            }
        }
        b"para" => {
            let kind = be16(t, 8)?;
            let n = [1, 3, 4, 5, 7].get(kind as usize).copied()?;
            let v = (0..n)
                .map(|i| s15f16(t, 12 + 4 * i))
                .collect::<Option<Vec<_>>>()?;
            // Everything as type 3: Y = (aX + b)^g for X >= d, else cX.
            Some(Curve::Param(match kind {
                0 => [v[0], 1.0, 0.0, 0.0, 0.0],
                1 => [v[0], v[1], v[2], 0.0, -v[2] / v[1]],
                3 => [v[0], v[1], v[2], v[3], v[4]],
                // Types 2 and 4 add offsets that sRGB never has.
                _ => return None,
            }))
        }
        _ => None,
    }
}

/// sRGB's primaries adapted to the D50 profile connection space (as in
/// the IEC 61966-2.1 profile).
const SRGB_COLORANTS: [[f64; 3]; 3] = [
    [0.436_074_7, 0.222_504_5, 0.013_932_2],
    [0.385_064_9, 0.716_878_6, 0.097_104_5],
    [0.143_080_4, 0.060_616_9, 0.714_173_3],
];

/// sRGB's transfer function (encoded → linear).
pub fn srgb_to_linear(x: f64) -> f64 {
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// Whether `profile` is an RGB matrix profile with sRGB's primaries and
/// tone curve, so images in it need no conversion for an sRGB target.
/// Gamma 2.2 is not sRGB (it differs visibly in the shadows).
pub fn is_srgb(profile: &[u8]) -> bool {
    let Some(h) = header(profile) else {
        return false;
    };
    if &h.colour_space != b"RGB " || &h.pcs != b"XYZ " {
        return false;
    }
    let colorants = [b"rXYZ", b"gXYZ", b"bXYZ"].map(|s| xyz(profile, s));
    let primaries_match = colorants
        .iter()
        .zip(&SRGB_COLORANTS)
        .all(|(c, want)| c.is_some_and(|c| c.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.003)));
    if !primaries_match {
        return false;
    }
    [b"rTRC", b"gTRC", b"bTRC"].iter().all(|s| {
        curve(profile, s).is_some_and(|c| {
            (1..=50).all(|i| {
                let x = i as f64 / 50.0;
                (c.value(x) - srgb_to_linear(x)).abs() < 0.0015
            })
        })
    })
}

/// Tone curve of a profile written by [`matrix_profile`].
#[derive(Clone, Copy, Debug)]
pub enum Trc {
    Gamma(f64),
    /// sRGB's piecewise curve as a 1024-entry table.
    Srgb,
}

/// D50 colorants (rXYZ, gXYZ, bXYZ) of common RGB spaces, for
/// [`matrix_profile`].
pub mod colorants {
    pub const SRGB: [[f64; 3]; 3] = super::SRGB_COLORANTS;
    pub const ADOBE_RGB: [[f64; 3]; 3] = [
        [0.609_741_2, 0.311_111_5, 0.019_470_8],
        [0.205_276_5, 0.625_671_4, 0.060_867_2],
        [0.149_182_3, 0.063_217_1, 0.744_561_9],
    ];
    pub const DISPLAY_P3: [[f64; 3]; 3] = [
        [0.515_102_2, 0.241_182_2, -0.001_048_8],
        [0.291_965_1, 0.692_236_7, 0.041_882_6],
        [0.157_105_4, 0.066_581_1, 0.784_076_2],
    ];
}

/// A minimal ICC v2 display profile: matrix (D50 colorants) and one tone
/// curve for all channels. For tests and generated test images.
pub fn matrix_profile(colorants: [[f64; 3]; 3], trc: Trc, description: &str) -> Vec<u8> {
    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }
    fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        for c in v {
            t.extend(s15(c));
        }
        t
    }
    let mut desc = b"desc\0\0\0\0".to_vec();
    desc.extend((description.len() as u32 + 1).to_be_bytes());
    desc.extend(description.as_bytes());
    desc.push(0);
    desc.extend([0u8; 4 + 4 + 2 + 1 + 67]); // Unicode and ScriptCode: empty
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend(b"No copyright, use freely\0");
    let mut curv = b"curv\0\0\0\0".to_vec();
    match trc {
        Trc::Gamma(g) => {
            curv.extend(1u32.to_be_bytes());
            curv.extend(((g * 256.0).round() as u16).to_be_bytes());
        }
        Trc::Srgb => {
            curv.extend(1024u32.to_be_bytes());
            for i in 0..1024 {
                let v = srgb_to_linear(i as f64 / 1023.0);
                curv.extend(((v * 65535.0).round() as u16).to_be_bytes());
            }
        }
    }
    let d50 = [0.964_2, 1.0, 0.824_9];
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc),
        (b"cprt", cprt),
        (b"wtpt", xyz_tag(d50)),
        (b"rXYZ", xyz_tag(colorants[0])),
        (b"gXYZ", xyz_tag(colorants[1])),
        (b"bXYZ", xyz_tag(colorants[2])),
        (b"rTRC", curv.clone()),
        (b"gTRC", curv.clone()),
        (b"bTRC", curv),
    ];
    let mut data = Vec::new();
    let mut table = Vec::new();
    let mut offset = 128 + 4 + 12 * tags.len();
    for (sig, body) in &tags {
        table.push((*sig, offset, body.len()));
        let padded = body.len().div_ceil(4) * 4;
        data.extend(body);
        data.resize(data.len() + padded - body.len(), 0);
        offset += padded;
    }
    let mut p = vec![0u8; 128];
    p[8..12].copy_from_slice(&[2, 0x10, 0, 0]); // version 2.1
    p[12..16].copy_from_slice(b"mntr");
    p[16..20].copy_from_slice(b"RGB ");
    p[20..24].copy_from_slice(b"XYZ ");
    p[36..40].copy_from_slice(b"acsp");
    for (k, v) in d50.iter().enumerate() {
        p[68 + 4 * k..72 + 4 * k].copy_from_slice(&s15(*v));
    }
    p.extend((tags.len() as u32).to_be_bytes());
    for (sig, off, len) in table {
        p.extend(sig);
        p.extend((off as u32).to_be_bytes());
        p.extend((len as u32).to_be_bytes());
    }
    p.extend(data);
    let size = p.len() as u32;
    p[0..4].copy_from_slice(&size.to_be_bytes());
    p
}

/// Inserts `profile` into a JPEG as APP2 `ICC_PROFILE` chunks right after
/// SOI (at most 65519 bytes each). For tests and generated test images.
pub fn embed_in_jpeg(jpeg: &[u8], profile: &[u8]) -> Vec<u8> {
    const MAX: usize = 65_519;
    let chunks: Vec<&[u8]> = profile.chunks(MAX).collect();
    let mut out = jpeg[..2].to_vec();
    for (i, c) in chunks.iter().enumerate() {
        out.extend([0xFF, 0xE2]);
        out.extend(((2 + 14 + c.len()) as u16).to_be_bytes());
        out.extend(b"ICC_PROFILE\0");
        out.extend([(i + 1) as u8, chunks.len() as u8]);
        out.extend(*c);
    }
    out.extend(&jpeg[2..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_srgb_by_primaries_and_curve() {
        let srgb = matrix_profile(colorants::SRGB, Trc::Srgb, "sRGB test");
        assert!(is_srgb(&srgb));
        // Same primaries, gamma 2.2: not sRGB.
        let g22 = matrix_profile(colorants::SRGB, Trc::Gamma(2.2), "sRGB 2.2");
        assert!(!is_srgb(&g22));
        let adobe = matrix_profile(colorants::ADOBE_RGB, Trc::Gamma(563.0 / 256.0), "Adobe");
        assert!(!is_srgb(&adobe));
        let p3 = matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "Display P3");
        assert!(!is_srgb(&p3));
        assert!(!is_srgb(b"not a profile"));
        assert!(!is_srgb(&srgb[..100]));
    }

    #[test]
    fn parametric_srgb_curve_counts() {
        // Replace the table curve by `para` type 3 with sRGB's parameters.
        let mut p = matrix_profile(colorants::SRGB, Trc::Gamma(1.0), "para");
        let mut para = b"para\0\0\0\0".to_vec();
        para.extend(3u16.to_be_bytes());
        para.extend([0, 0]);
        for v in [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.040_45] {
            para.extend(((v * 65536.0f64).round() as i32).to_be_bytes());
        }
        let start = p.len();
        p.extend(&para);
        for sig in [b"rTRC", b"gTRC", b"bTRC"] {
            let count = be32(&p, 128).unwrap() as usize;
            let e = (0..count)
                .map(|i| 132 + i * 12)
                .find(|&e| &p[e..e + 4] == sig)
                .unwrap();
            p[e + 4..e + 8].copy_from_slice(&(start as u32).to_be_bytes());
            p[e + 8..e + 12].copy_from_slice(&(para.len() as u32).to_be_bytes());
        }
        let size = p.len() as u32;
        p[0..4].copy_from_slice(&size.to_be_bytes());
        assert!(is_srgb(&p));
    }

    #[cfg(feature = "turbojpeg")]
    #[test]
    fn reads_profiles_from_jpeg() {
        use crate::decode::jpeg::{EncodeOptions, encode_rgb};
        let (w, h) = (16, 8);
        let jpeg = encode_rgb(&crate::testimg::rgb(w, h), w, h, EncodeOptions::default()).unwrap();
        assert!(from_jpeg(&jpeg, &blinzel_jpeg::parse(&jpeg).unwrap()).is_none());
        // Larger than one APP2 segment: three chunks.
        let mut big = matrix_profile(colorants::ADOBE_RGB, Trc::Gamma(2.2), "big");
        big.resize(150_000, 0x55);
        let size = big.len() as u32;
        big[0..4].copy_from_slice(&size.to_be_bytes());
        let tagged = embed_in_jpeg(&jpeg, &big);
        let info = blinzel_jpeg::parse(&tagged).unwrap();
        assert_eq!(info.icc_chunks.len(), 3);
        let p = from_jpeg(&tagged, &info).unwrap();
        assert_eq!(&p.bytes[..], &big[..]);
        assert!(!p.srgb);
    }

    #[test]
    fn reads_profiles_from_png() {
        let (w, h) = (16, 8);
        let profile = matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3");
        let mut png_bytes = Vec::new();
        {
            let mut info = png::Info::with_size(w, h);
            info.color_type = png::ColorType::Rgb;
            info.bit_depth = png::BitDepth::Eight;
            info.icc_profile = Some(profile.clone().into());
            let enc = png::Encoder::with_info(&mut png_bytes, info).unwrap();
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&crate::testimg::rgb(w, h)).unwrap();
        }
        let p = from_png(&png_bytes).unwrap();
        assert_eq!(&p.bytes[..], &profile[..]);
        assert_eq!(p.hash, Profile::new(profile).hash);
    }
}
