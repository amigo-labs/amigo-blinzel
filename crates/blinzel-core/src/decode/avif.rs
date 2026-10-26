//! AVIF: our own reader for the HEIF container (items, grids, alpha,
//! `clap`/`irot`/`imir`, `colr`) and `rav1d` (Rust port of dav1d, BSD-2)
//! for the AV1 pictures. The pixels stay in stored orientation, cropped by
//! `clap`; `irot` and `imir` become the probe's orientation, which the GPU
//! applies. 8 to 12 bits, 4:0:0 to 4:4:4, with any CICP matrix and range,
//! come out as RGBA8; HDR transfer functions (PQ, HLG), layered images and
//! anything else this reader does not know report `Unsupported`, which
//! leaves the file to the platform decoder.

use std::collections::HashMap;
use std::ptr::NonNull;

use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};

use super::{DecodeError, Result, check_size};
use crate::format::Format;
use crate::geom::Orientation;
use crate::icc::{self, Profile, Trc};
use crate::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};

fn unsupported() -> DecodeError {
    DecodeError::Unsupported(Format::Avif)
}

fn corrupt(what: &str) -> DecodeError {
    DecodeError::Corrupt(format!("AVIF: {what}"))
}

// ---------------------------------------------------------------------------
// Container
// ---------------------------------------------------------------------------

/// Big-endian reader over a box payload; every read fails cleanly at the
/// end.
struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn new(d: &'a [u8]) -> Self {
        Reader { d, p: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).filter(|&e| e <= self.d.len());
        let end = end.ok_or_else(|| corrupt("truncated box"))?;
        let s = &self.d[self.p..end];
        self.p = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// An unsigned number of `n` bytes (0, 4 or 8 in `iloc`, any up to 8).
    fn uint(&mut self, n: usize) -> Result<u64> {
        if n > 8 {
            return Err(corrupt("field too wide"));
        }
        Ok(self.take(n)?.iter().fold(0, |v, &b| v << 8 | b as u64))
    }

    /// FullBox header: version and flags.
    fn full(&mut self) -> Result<(u8, u32)> {
        let v = self.u32()?;
        Ok(((v >> 24) as u8, v & 0xff_ffff))
    }

    fn rest(&mut self) -> &'a [u8] {
        let s = &self.d[self.p..];
        self.p = self.d.len();
        s
    }
}

/// The boxes in `d`: type and payload.
fn boxes(d: &[u8]) -> impl Iterator<Item = Result<([u8; 4], &[u8])>> {
    let mut r = Reader::new(d);
    std::iter::from_fn(move || {
        if r.p >= r.d.len() {
            return None;
        }
        Some((|| {
            let size = r.u32()? as u64;
            let kind: [u8; 4] = r.take(4)?.try_into().unwrap();
            let header = if size == 1 { 16 } else { 8 };
            let size = match size {
                0 => (r.d.len() - r.p + 8) as u64,
                1 => r.uint(8)?,
                s => s,
            };
            let body = size
                .checked_sub(header)
                .ok_or_else(|| corrupt("box size"))?;
            Ok((
                kind,
                r.take(usize::try_from(body).map_err(|_| corrupt("box size"))?)?,
            ))
        })())
    })
}

#[derive(Clone, Debug, Default)]
struct Clap {
    width: (u32, u32),
    height: (u32, u32),
    horiz_off: (i32, u32),
    vert_off: (i32, u32),
}

#[derive(Clone, Debug)]
enum Colour {
    Nclx {
        primaries: u16,
        transfer: u16,
        matrix: u16,
        full_range: bool,
    },
    Icc(Vec<u8>),
}

#[derive(Clone, Debug)]
enum Prop {
    Ispe(u32, u32),
    Colr(Colour),
    /// Anticlockwise quarter turns.
    Irot(u8),
    /// 0: top-bottom, 1: left-right (as libavif, libheif and browsers
    /// read it).
    Imir(u8),
    Clap(Clap),
    AuxC(String),
    /// Known and not needed (`av1C`, `pixi`, ...).
    Ignored,
    /// Not known: if essential, the item cannot be shown correctly.
    Unknown,
}

#[derive(Clone, Debug, Default)]
struct Item {
    kind: [u8; 4],
    construction_method: u8,
    base_offset: u64,
    extents: Vec<(u64, u64)>,
    /// Property indexes (1-based into `ipco`) and whether each is essential.
    props: Vec<(u16, bool)>,
}

struct Container<'a> {
    file: &'a [u8],
    idat: &'a [u8],
    primary: u32,
    items: HashMap<u32, Item>,
    props: Vec<Prop>,
    /// (kind, from, to)
    refs: Vec<([u8; 4], u32, Vec<u32>)>,
}

impl<'a> Container<'a> {
    fn parse(file: &'a [u8]) -> Result<Container<'a>> {
        let mut meta = None;
        for b in boxes(file) {
            let (kind, body) = b?;
            if &kind == b"meta" {
                meta = Some(body);
                break;
            }
        }
        let mut r = Reader::new(meta.ok_or_else(|| corrupt("no meta box"))?);
        r.full()?;
        let mut c = Container {
            file,
            idat: &[],
            primary: 0,
            items: HashMap::new(),
            props: Vec::new(),
            refs: Vec::new(),
        };
        let mut primary = None;
        for b in boxes(r.rest()) {
            let (kind, body) = b?;
            let mut r = Reader::new(body);
            match &kind {
                b"pitm" => {
                    let (v, _) = r.full()?;
                    primary = Some(if v == 0 { r.u16()? as u32 } else { r.u32()? });
                }
                b"iloc" => c.iloc(&mut r)?,
                b"iinf" => c.iinf(&mut r)?,
                b"iref" => c.iref(&mut r)?,
                b"iprp" => c.iprp(body)?,
                b"idat" => c.idat = body,
                _ => {}
            }
        }
        c.primary = primary.ok_or_else(|| corrupt("no primary item"))?;
        if !c.items.contains_key(&c.primary) {
            return Err(corrupt("primary item missing"));
        }
        Ok(c)
    }

    fn item(&mut self, id: u32) -> &mut Item {
        self.items.entry(id).or_default()
    }

    fn iloc(&mut self, r: &mut Reader) -> Result<()> {
        let (v, _) = r.full()?;
        let sizes = r.u8()?;
        let (offset_size, length_size) = ((sizes >> 4) as usize, (sizes & 15) as usize);
        let sizes = r.u8()?;
        let base_offset_size = (sizes >> 4) as usize;
        let index_size = if v >= 1 { (sizes & 15) as usize } else { 0 };
        let count = if v < 2 { r.u16()? as u32 } else { r.u32()? };
        for _ in 0..count {
            let id = if v < 2 { r.u16()? as u32 } else { r.u32()? };
            let construction_method = if v >= 1 { (r.u16()? & 15) as u8 } else { 0 };
            r.u16()?; // data_reference_index
            let base_offset = r.uint(base_offset_size)?;
            let extent_count = r.u16()?;
            let mut extents = Vec::with_capacity(extent_count as usize);
            for _ in 0..extent_count {
                r.uint(index_size)?;
                extents.push((r.uint(offset_size)?, r.uint(length_size)?));
            }
            let item = self.item(id);
            item.construction_method = construction_method;
            item.base_offset = base_offset;
            item.extents = extents;
        }
        Ok(())
    }

    fn iinf(&mut self, r: &mut Reader) -> Result<()> {
        let (v, _) = r.full()?;
        if v == 0 {
            r.u16()? as u32
        } else {
            r.u32()?
        };
        for b in boxes(r.rest()) {
            let (kind, body) = b?;
            if &kind != b"infe" {
                continue;
            }
            let mut r = Reader::new(body);
            let (v, _) = r.full()?;
            if v < 2 {
                continue;
            }
            let id = if v == 2 { r.u16()? as u32 } else { r.u32()? };
            r.u16()?; // item_protection_index
            let kind: [u8; 4] = r.take(4)?.try_into().unwrap();
            self.item(id).kind = kind;
        }
        Ok(())
    }

    fn iref(&mut self, r: &mut Reader) -> Result<()> {
        let (v, _) = r.full()?;
        for b in boxes(r.rest()) {
            let (kind, body) = b?;
            let mut r = Reader::new(body);
            let id = |r: &mut Reader| -> Result<u32> {
                Ok(if v == 0 { r.u16()? as u32 } else { r.u32()? })
            };
            let from = id(&mut r)?;
            let n = r.u16()?;
            let to = (0..n).map(|_| id(&mut r)).collect::<Result<Vec<_>>>()?;
            self.refs.push((kind, from, to));
        }
        Ok(())
    }

    fn iprp(&mut self, body: &[u8]) -> Result<()> {
        for b in boxes(body) {
            let (kind, body) = b?;
            match &kind {
                b"ipco" => {
                    for p in boxes(body) {
                        let (kind, body) = p?;
                        self.props.push(property(kind, body)?);
                    }
                }
                b"ipma" => {
                    let mut r = Reader::new(body);
                    let (v, flags) = r.full()?;
                    let count = r.u32()?;
                    for _ in 0..count {
                        let id = if v < 1 { r.u16()? as u32 } else { r.u32()? };
                        let n = r.u8()?;
                        for _ in 0..n {
                            let (index, essential) = if flags & 1 != 0 {
                                let x = r.u16()?;
                                (x & 0x7fff, x & 0x8000 != 0)
                            } else {
                                let x = r.u8()?;
                                ((x & 0x7f) as u16, x & 0x80 != 0)
                            };
                            self.item(id).props.push((index, essential));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The properties of item `id`, in association order. Unknown
    /// essential ones make the item unsupported.
    fn props_of(&self, id: u32) -> Result<Vec<&Prop>> {
        let item = self.items.get(&id).ok_or_else(|| corrupt("missing item"))?;
        let mut out = Vec::new();
        for &(index, essential) in &item.props {
            let Some(p) = index
                .checked_sub(1)
                .and_then(|i| self.props.get(i as usize))
            else {
                continue;
            };
            if essential && matches!(p, Prop::Unknown) {
                return Err(unsupported());
            }
            out.push(p);
        }
        Ok(out)
    }

    /// The item's bytes (all extents in a row).
    fn data(&self, id: u32) -> Result<Vec<u8>> {
        let item = self.items.get(&id).ok_or_else(|| corrupt("missing item"))?;
        let src = match item.construction_method {
            0 => self.file,
            1 => self.idat,
            _ => return Err(unsupported()),
        };
        let mut out = Vec::new();
        for &(offset, length) in &item.extents {
            let start = item
                .base_offset
                .checked_add(offset)
                .and_then(|s| usize::try_from(s).ok());
            // Length 0: up to the end of the source.
            let end = match length {
                0 => Some(src.len()),
                l => start.and_then(|s| s.checked_add(usize::try_from(l).ok()?)),
            };
            let bytes = start
                .zip(end)
                .and_then(|(s, e)| src.get(s..e))
                .ok_or_else(|| corrupt("item data outside the file"))?;
            out.extend_from_slice(bytes);
        }
        Ok(out)
    }

    fn refs_from(&self, kind: &[u8; 4], from: u32) -> Option<&[u32]> {
        self.refs
            .iter()
            .find(|(k, f, _)| k == kind && *f == from)
            .map(|(_, _, to)| to.as_slice())
    }

    /// The alpha plane's item: an auxiliary image of the primary one.
    fn alpha(&self) -> Option<u32> {
        self.refs.iter().find_map(|(k, from, to)| {
            (k == b"auxl" && to.contains(&self.primary) && self.is_alpha(*from)).then_some(*from)
        })
    }

    fn is_alpha(&self, id: u32) -> bool {
        self.props_of(id).is_ok_and(|ps| {
            ps.iter().any(|p| {
                matches!(p, Prop::AuxC(urn) if urn == "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha"
                    || urn == "urn:mpeg:hevc:2015:auxid:1")
            })
        })
    }
}

fn property(kind: [u8; 4], body: &[u8]) -> Result<Prop> {
    let mut r = Reader::new(body);
    Ok(match &kind {
        b"ispe" => {
            r.full()?;
            Prop::Ispe(r.u32()?, r.u32()?)
        }
        b"colr" => match r.take(4)? {
            b"nclx" => Prop::Colr(Colour::Nclx {
                primaries: r.u16()?,
                transfer: r.u16()?,
                matrix: r.u16()?,
                full_range: r.u8()? & 0x80 != 0,
            }),
            b"prof" | b"rICC" => Prop::Colr(Colour::Icc(r.rest().to_vec())),
            _ => Prop::Ignored,
        },
        b"irot" => Prop::Irot(r.u8()? & 3),
        b"imir" => Prop::Imir(r.u8()? & 1),
        b"clap" => {
            let mut n = || r.u32();
            Prop::Clap(Clap {
                width: (n()?, n()?),
                height: (n()?, n()?),
                horiz_off: (n()? as i32, n()?),
                vert_off: (n()? as i32, n()?),
            })
        }
        b"auxC" => {
            r.full()?;
            let urn = r.rest();
            let end = urn.iter().position(|&b| b == 0).unwrap_or(urn.len());
            Prop::AuxC(String::from_utf8_lossy(&urn[..end]).into_owned())
        }
        b"av1C" | b"pixi" | b"pasp" | b"clli" | b"mdcv" | b"cclv" | b"amve" => Prop::Ignored,
        _ => Prop::Unknown,
    })
}

/// What the probe and the decoder need to know about the primary image.
struct Layout {
    /// Coded size (`ispe`, or the grid's output size).
    width: u32,
    height: u32,
    /// Crop of the coded image (`clap`), stored orientation.
    crop: (u32, u32, u32, u32),
    orientation: Orientation,
    colour: Option<Colour>,
}

fn layout(c: &Container) -> Result<Layout> {
    let item = &c.items[&c.primary];
    if &item.kind != b"av01" && &item.kind != b"grid" {
        return Err(unsupported());
    }
    let (mut width, mut height) = (0, 0);
    let mut clap = None;
    let mut orientation = Orientation::NORMAL;
    let mut colour = None;
    for p in c.props_of(c.primary)? {
        match p {
            Prop::Ispe(w, h) => (width, height) = (*w, *h),
            // Rotation and mirror apply after the crop, in this order.
            Prop::Irot(a) => {
                orientation = orientation.then(Orientation {
                    quarter_turns: (4 - a) % 4,
                    mirror: false,
                })
            }
            Prop::Imir(axis) => {
                orientation = orientation.then(Orientation {
                    quarter_turns: if *axis == 0 { 2 } else { 0 },
                    mirror: true,
                })
            }
            Prop::Clap(cl) => clap = Some(cl.clone()),
            // The first `colr` of each kind counts; an ICC profile wins.
            Prop::Colr(col) => match (&colour, col) {
                (None, _) | (Some(Colour::Nclx { .. }), Colour::Icc(_)) => {
                    colour = Some(col.clone())
                }
                _ => {}
            },
            _ => {}
        }
    }
    if &item.kind == b"grid" {
        let g = grid(c, c.primary)?;
        (width, height) = (g.width, g.height);
    }
    check_size(width as u64, height as u64)?;
    let crop = match clap {
        Some(cl) => clap_rect(&cl, width, height).ok_or_else(|| corrupt("bad clap"))?,
        None => (0, 0, width, height),
    };
    Ok(Layout {
        width,
        height,
        crop,
        orientation,
        colour,
    })
}

/// The `clap` rectangle in pixels: centred on the image centre plus the
/// offsets.
fn clap_rect(c: &Clap, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let f = |(n, d): (u32, u32)| (d != 0).then(|| n as f64 / d as f64);
    let fi = |(n, d): (i32, u32)| (d != 0).then(|| n as f64 / d as f64);
    let (cw, ch) = (f(c.width)?.round(), f(c.height)?.round());
    let x = (fi(c.horiz_off)? + (w as f64 - 1.0) / 2.0 - (cw - 1.0) / 2.0).round();
    let y = (fi(c.vert_off)? + (h as f64 - 1.0) / 2.0 - (ch - 1.0) / 2.0).round();
    let ok = cw >= 1.0 && ch >= 1.0 && x >= 0.0 && y >= 0.0;
    (ok && x + cw <= w as f64 && y + ch <= h as f64)
        .then_some((x as u32, y as u32, cw as u32, ch as u32))
}

struct Grid {
    cols: u32,
    width: u32,
    height: u32,
    tiles: Vec<u32>,
}

/// Grid item `id`: its output size and tiles in raster order.
fn grid(c: &Container, id: u32) -> Result<Grid> {
    let data = c.data(id)?;
    let mut r = Reader::new(&data);
    r.u8()?; // version
    let flags = r.u8()?;
    let rows = r.u8()? as u32 + 1;
    let cols = r.u8()? as u32 + 1;
    let (width, height) = if flags & 1 != 0 {
        (r.u32()?, r.u32()?)
    } else {
        (r.u16()? as u32, r.u16()? as u32)
    };
    let tiles = c.refs_from(b"dimg", id).unwrap_or_default().to_vec();
    if tiles.len() != (rows * cols) as usize {
        return Err(corrupt("grid tiles missing"));
    }
    Ok(Grid {
        cols,
        width,
        height,
        tiles,
    })
}

/// The colour profile the pixels are in: an embedded ICC profile, a
/// generated one for Display P3, none for sRGB/BT.709 (and for primaries we
/// do not convert, like WIC). `Err` for HDR transfer functions.
fn profile(colour: &Option<Colour>) -> Result<Option<Profile>> {
    match colour {
        Some(Colour::Icc(p)) if !icc::is_srgb(p) => Ok(Some(Profile::new(p.clone()))),
        Some(Colour::Nclx {
            primaries,
            transfer,
            ..
        }) => {
            // PQ and HLG need tone mapping.
            if matches!(transfer, 16 | 18) {
                return Err(unsupported());
            }
            Ok((*primaries == 12).then(|| {
                Profile::new(icc::matrix_profile(
                    icc::colorants::DISPLAY_P3,
                    Trc::Srgb,
                    "Display P3 (AVIF nclx)",
                ))
            }))
        }
        _ => Ok(None),
    }
}

/// The colour profile of the primary image of any HEIF file (HEIC too),
/// from the container alone; `None` for sRGB or when unknown.
pub fn container_profile(data: &[u8]) -> Option<Profile> {
    let c = Container::parse(data).ok()?;
    let mut colour = None;
    for p in c.props_of(c.primary).ok()? {
        if let Prop::Colr(col) = p
            && (colour.is_none() || matches!(col, Colour::Icc(_)))
        {
            colour = Some(col.clone());
        }
    }
    profile(&colour).ok().flatten()
}

/// The EXIF block (TIFF) of any HEIF file (HEIC too): the `Exif` item
/// describing the primary image, or else the first one. The item starts
/// with the offset of the TIFF header.
pub fn exif(data: &[u8]) -> Option<Vec<u8>> {
    let c = Container::parse(data).ok()?;
    let mut ids: Vec<u32> = c
        .items
        .iter()
        .filter(|(_, item)| &item.kind == b"Exif")
        .map(|(&id, _)| id)
        .collect();
    ids.sort_by_key(|&id| {
        let describes_primary = c
            .refs_from(b"cdsc", id)
            .is_some_and(|to| to.contains(&c.primary));
        (!describes_primary, id)
    });
    let d = c.data(*ids.first()?).ok()?;
    let skip = u32::from_be_bytes(d.get(..4)?.try_into().ok()?) as usize;
    d.get(skip.checked_add(4)?..).map(<[u8]>::to_vec)
}

/// Stored size (cropped), orientation and colour profile, from the
/// container alone.
pub fn info(data: &[u8]) -> Result<((u32, u32), Orientation, Option<Profile>)> {
    let c = Container::parse(data)?;
    let l = layout(&c)?;
    Ok(((l.crop.2, l.crop.3), l.orientation, profile(&l.colour)?))
}

// ---------------------------------------------------------------------------
// AV1
// ---------------------------------------------------------------------------

/// `-EAGAIN` from dav1d: feed more data or fetch the picture first.
const EAGAIN: i32 = -11;

/// A decoded picture, unreferenced on drop.
struct Picture(Dav1dPicture);

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: the picture came from `dav1d_get_picture`.
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut self.0))) };
    }
}

// SAFETY: the threads converting a picture only read its planes and
// headers, which dav1d does not change after handing the picture out.
unsafe impl Sync for Picture {}

impl Picture {
    fn size(&self) -> (u32, u32) {
        (self.0.p.w.max(0) as u32, self.0.p.h.max(0) as u32)
    }

    fn bpc(&self) -> u32 {
        self.0.p.bpc as u32
    }

    /// Chroma subsampling shifts (x, y); `None` for monochrome.
    fn subsampling(&self) -> Option<(u32, u32)> {
        match self.0.p.layout {
            DAV1D_PIXEL_LAYOUT_I400 => None,
            DAV1D_PIXEL_LAYOUT_I420 => Some((1, 1)),
            DAV1D_PIXEL_LAYOUT_I422 => Some((1, 0)),
            _ => Some((0, 0)),
        }
    }

    /// Row `y` of plane `i` (0: Y, 1: U, 2: V) as samples.
    fn row(&self, i: usize, y: u32) -> Row<'_> {
        let (w, h) = self.size();
        let (w, h) = match (i, self.subsampling()) {
            (0, _) | (_, None) => (w, h),
            (_, Some((sx, sy))) => ((w + sx) >> sx, (h + sy) >> sy),
        };
        let stride = self.0.stride[(i > 0) as usize];
        let base = self.0.data[i].expect("plane").as_ptr() as *const u8;
        let wide = self.bpc() > 8;
        // SAFETY: dav1d's planes hold `h` rows of `stride` bytes, each with
        // at least `w` samples (16-bit and aligned above 8 bits); the
        // picture outlives the row.
        unsafe {
            let p = base.offset(stride * y.min(h - 1) as isize);
            if wide {
                Row::U16(std::slice::from_raw_parts(p as *const u16, w as usize))
            } else {
                Row::U8(std::slice::from_raw_parts(p, w as usize))
            }
        }
    }

    /// Colour description from the sequence header.
    fn cicp(&self) -> (u16, bool) {
        // SAFETY: a picture from dav1d always has its sequence header.
        let seq = unsafe { self.0.seq_hdr.expect("sequence header").as_ref() };
        (seq.mtrx as u16, seq.color_range != 0)
    }
}

enum Row<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
}

impl Row<'_> {
    fn get(&self, x: usize) -> i32 {
        match self {
            Row::U8(r) => r[x] as i32,
            Row::U16(r) => r[x] as i32,
        }
    }
}

/// Decodes one AV1 image item (a key frame with its sequence header).
fn decode_av1(obus: &[u8], threads: usize) -> Result<Picture> {
    let fail = || corrupt("AV1 data");
    let mut settings = std::mem::MaybeUninit::<Dav1dSettings>::uninit();
    // SAFETY: dav1d_default_settings writes the whole struct.
    let mut settings = unsafe {
        dav1d_default_settings(NonNull::new(settings.as_mut_ptr()).unwrap());
        settings.assume_init()
    };
    settings.n_threads = threads.clamp(1, 256) as i32;
    settings.max_frame_delay = 1;
    settings.frame_size_limit = super::MAX_PIXELS as u32;
    let mut ctx: Option<Dav1dContext> = None;
    // SAFETY: both pointers are valid for the call.
    let r = unsafe {
        dav1d_open(
            Some(NonNull::from(&mut ctx)),
            Some(NonNull::from(&mut settings)),
        )
    };
    if r.0 != 0 || ctx.is_none() {
        return Err(fail());
    }
    /// Closes the decoder on every path.
    struct Ctx(Option<Dav1dContext>);
    impl Drop for Ctx {
        fn drop(&mut self) {
            // SAFETY: the context came from `dav1d_open`.
            unsafe { dav1d_close(Some(NonNull::from(&mut self.0))) };
        }
    }
    let ctx = Ctx(ctx);
    let c = ctx.0;

    // SAFETY: Dav1dData is plain data; dav1d_data_create fills it in and
    // returns its buffer of `len` bytes.
    let mut data: Dav1dData = unsafe { std::mem::zeroed() };
    let buf = unsafe { dav1d_data_create(Some(NonNull::from(&mut data)), obus.len()) };
    if buf.is_null() {
        return Err(fail());
    }
    // SAFETY: `buf` has room for `obus.len()` bytes.
    unsafe { std::ptr::copy_nonoverlapping(obus.as_ptr(), buf, obus.len()) };

    let mut pic: Dav1dPicture = unsafe { std::mem::zeroed() };
    let mut result = Err(fail());
    for _ in 0..64 {
        if data.sz > 0 {
            // SAFETY: context and data are valid.
            let r = unsafe { dav1d_send_data(c, Some(NonNull::from(&mut data))) };
            if r.0 != 0 && r.0 != EAGAIN {
                break;
            }
        }
        // SAFETY: context valid; `pic` is written on success.
        let r = unsafe { dav1d_get_picture(c, Some(NonNull::from(&mut pic))) };
        if r.0 == 0 {
            result = Ok(Picture(pic));
            break;
        }
        if r.0 != EAGAIN {
            break;
        }
    }
    if data.sz > 0 {
        // SAFETY: unconsumed data from dav1d_data_create.
        unsafe { dav1d_data_unref(Some(NonNull::from(&mut data))) };
    }
    drop(ctx);
    result
}

// ---------------------------------------------------------------------------
// YCbCr → RGBA
// ---------------------------------------------------------------------------

/// Fixed-point (14 bits) conversion for one matrix and range at one bit
/// depth: full-range 8-bit RGB from Y, Cb, Cr.
#[derive(Clone, Copy)]
struct Matrix {
    identity: bool,
    y_off: i64,
    y_mul: i64,
    c_mul: i64,
    half: i32,
    cr_r: i64,
    cb_g: i64,
    cr_g: i64,
    cb_b: i64,
}

const FIX: u32 = 14;

impl Matrix {
    fn new(mc: u16, full_range: bool, bpc: u32) -> Result<Matrix> {
        let (kr, kb) = match mc {
            0 => (0.0, 0.0),
            1 => (0.2126, 0.0722),
            // Unspecified: BT.601, as libavif and browsers assume.
            2 | 5 | 6 => (0.299, 0.114),
            4 => (0.30, 0.11),
            7 => (0.212, 0.087),
            9 => (0.2627, 0.0593),
            _ => return Err(unsupported()),
        };
        let max = ((1u32 << bpc) - 1) as f64;
        let scale = (1u32 << bpc) as f64 / 256.0;
        let (y_off, y_range, c_range) = if full_range {
            (0.0, max, max)
        } else {
            (16.0 * scale, 219.0 * scale, 224.0 * scale)
        };
        let one = (1u32 << FIX) as f64;
        let kg = 1.0 - kr - kb;
        let fx = |v: f64| (v * one).round() as i64;
        Ok(Matrix {
            identity: mc == 0,
            y_off: y_off as i64,
            // Scale to 8 bits in the same step.
            y_mul: fx(255.0 / y_range),
            c_mul: fx(255.0 / c_range),
            half: 1 << (bpc - 1),
            cr_r: fx(2.0 * (1.0 - kr)),
            cb_g: if kg > 0.0 {
                fx(2.0 * kb * (1.0 - kb) / kg)
            } else {
                0
            },
            cr_g: if kg > 0.0 {
                fx(2.0 * kr * (1.0 - kr) / kg)
            } else {
                0
            },
            cb_b: fx(2.0 * (1.0 - kb)),
        })
    }

    #[inline]
    fn to_u8(v: i64) -> u8 {
        ((v + (1 << (FIX - 1))) >> FIX).clamp(0, 255) as u8
    }

    /// Luma (or a GBR channel) as 8 bits, with FIX fraction bits.
    #[inline]
    fn luma(&self, v: i32) -> i64 {
        (v as i64 - self.y_off) * self.y_mul
    }

    #[inline]
    fn rgb(&self, y: i32, cb: i32, cr: i32) -> [u8; 3] {
        if self.identity {
            // GBR: Y is G, Cb is B, Cr is R.
            return [cr, y, cb].map(|v| Self::to_u8(self.luma(v)));
        }
        let y = self.luma(y);
        let cb = (cb - self.half) as i64 * self.c_mul;
        let cr = (cr - self.half) as i64 * self.c_mul;
        let r = y + ((cr * self.cr_r) >> FIX);
        let g = y - ((cb * self.cb_g + cr * self.cr_g) >> FIX);
        let b = y + ((cb * self.cb_b) >> FIX);
        [Self::to_u8(r), Self::to_u8(g), Self::to_u8(b)]
    }

    /// A luma sample of an alpha plane as 8 bits.
    #[inline]
    fn alpha(&self, v: i32) -> u8 {
        Self::to_u8(self.luma(v))
    }
}

/// Chroma row for output row `y`, linearly interpolated between the two
/// nearest chroma rows (centred siting), in 1/16 units.
fn chroma_rows(pic: &Picture, plane: usize, y: u32, sy: u32, out: &mut [i32]) {
    let (_, h) = pic.size();
    let ch = (h + sy) >> sy;
    if sy == 0 {
        let r = pic.row(plane, y);
        for (x, o) in out.iter_mut().enumerate() {
            *o = r.get(x) << 4;
        }
        return;
    }
    // Position of output row y in chroma rows, in 1/16 units.
    let pos = ((2 * y as i32 + 1) * 8 - 16).max(0) / 2;
    let (c0, f) = ((pos >> 4) as u32, pos & 15);
    let c1 = (c0 + 1).min(ch - 1);
    let (r0, r1) = (pic.row(plane, c0.min(ch - 1)), pic.row(plane, c1));
    for (x, o) in out.iter_mut().enumerate() {
        *o = r0.get(x) * (16 - f) + r1.get(x) * f;
    }
}

/// Chroma sample for output column `x` from an interpolated row (1/16
/// units), horizontally interpolated the same way; returns full samples.
#[inline]
fn chroma_at(row: &[i32], x: u32, sx: u32) -> i32 {
    if sx == 0 {
        return (row[x as usize] + 8) >> 4;
    }
    let pos = ((2 * x as i32 + 1) * 8 - 16).max(0) / 2;
    let (c0, f) = ((pos >> 4) as usize, pos & 15);
    let c1 = (c0 + 1).min(row.len() - 1);
    (row[c0.min(row.len() - 1)] * (16 - f) + row[c1] * f + 128) >> 8
}

/// Converts the picture's rows `ys` (cropped to `cx..cx+w` columns) into
/// RGBA rows of `out` (4 bytes per pixel, `out_stride` per row).
fn convert_rows(
    pic: &Picture,
    m: &Matrix,
    ys: std::ops::Range<u32>,
    (cx, w): (u32, u32),
    out: &mut [u8],
    out_stride: usize,
) {
    let (pw, _) = pic.size();
    let sub = pic.subsampling();
    let cw = sub.map_or(0, |(sx, _)| ((pw + sx) >> sx) as usize);
    let (mut cb, mut cr) = (vec![0i32; cw], vec![0i32; cw]);
    for (i, y) in ys.enumerate() {
        let dst = &mut out[i * out_stride..i * out_stride + w as usize * 4];
        let luma = pic.row(0, y);
        match sub {
            None => {
                for (x, d) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let v = luma.get(cx as usize + x);
                    let [g, _, _] = m.rgb(v, m.half, m.half);
                    *d = [g, g, g, 255];
                }
            }
            Some((sx, sy)) => {
                chroma_rows(pic, 1, y, sy, &mut cb);
                chroma_rows(pic, 2, y, sy, &mut cr);
                for (x, d) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let px = cx + x as u32;
                    let [r, g, b] = m.rgb(
                        luma.get(px as usize),
                        chroma_at(&cb, px, sx),
                        chroma_at(&cr, px, sx),
                    );
                    *d = [r, g, b, 255];
                }
            }
        }
    }
}

/// Writes `pic` (rows `src_y..`, columns `src_x..`, `w × h`) into the
/// canvas at (`dx`, `dy`), on up to `threads` threads.
#[allow(clippy::too_many_arguments)]
fn blit(
    pic: &Picture,
    m: &Matrix,
    (src_x, src_y): (u32, u32),
    (w, h): (u32, u32),
    canvas: &mut Plane,
    (dx, dy): (u32, u32),
    threads: usize,
    alpha_only: bool,
) {
    let stride = canvas.stride;
    let start = dy as usize * stride + dx as usize * 4;
    let rows = &mut canvas.data[start..];
    let band = (h as usize).div_ceil(threads.max(1)).max(16);
    std::thread::scope(|s| {
        let mut rest = rows;
        let mut y0 = 0u32;
        while y0 < h {
            let n = band.min((h - y0) as usize) as u32;
            let take = (n as usize * stride).min(rest.len());
            let (chunk, tail) = rest.split_at_mut(take);
            rest = tail;
            let ys = src_y + y0..src_y + y0 + n;
            let job = move || {
                if alpha_only {
                    for (i, y) in ys.enumerate() {
                        let r = pic.row(0, y);
                        let d = &mut chunk[i * stride..i * stride + w as usize * 4];
                        for (x, px) in d.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                            px[3] = m.alpha(r.get(src_x as usize + x));
                        }
                    }
                } else {
                    convert_rows(pic, m, ys, (src_x, w), chunk, stride);
                }
            };
            if y0 + n >= h {
                job();
            } else {
                s.spawn(job);
            }
            y0 += n;
        }
    });
}

/// Decodes item `id` (a picture or a grid of them) into the canvas, which
/// covers the crop (`crop` in coded coordinates). `alpha_only` writes the
/// luma into the alpha channel.
fn draw_item(
    c: &Container,
    id: u32,
    canvas: &mut Plane,
    crop: (u32, u32, u32, u32),
    colour: &Option<Colour>,
    threads: usize,
    alpha_only: bool,
) -> Result<()> {
    let item = c.items.get(&id).ok_or_else(|| corrupt("missing item"))?;
    let (tiles, cols) = match &item.kind {
        b"grid" => {
            let g = grid(c, id)?;
            (g.tiles, g.cols)
        }
        b"av01" => (vec![id], 1),
        _ => return Err(unsupported()),
    };
    let (crop_x, crop_y, crop_w, crop_h) = crop;
    let mut tile_w = 0;
    let mut tile_h = 0;
    for (i, &t) in tiles.iter().enumerate() {
        let pic = decode_av1(&c.data(t)?, threads)?;
        let (pw, ph) = pic.size();
        if i == 0 {
            (tile_w, tile_h) = (pw, ph);
        } else if (pw, ph) != (tile_w, tile_h) {
            return Err(corrupt("grid tiles differ in size"));
        }
        if pic.bpc() > 12 {
            return Err(unsupported());
        }
        // The container's `nclx` wins over the sequence header.
        let (mc, full) = match colour {
            Some(Colour::Nclx {
                matrix, full_range, ..
            }) if !alpha_only => (*matrix, *full_range),
            _ => pic.cicp(),
        };
        let m = Matrix::new(if alpha_only { 2 } else { mc }, full, pic.bpc())?;
        // Tile rectangle in coded coordinates, intersected with the crop.
        let (tx, ty) = ((i as u32 % cols) * tile_w, (i as u32 / cols) * tile_h);
        let x0 = tx.max(crop_x);
        let y0 = ty.max(crop_y);
        let x1 = (tx + pw).min(crop_x + crop_w);
        let y1 = (ty + ph).min(crop_y + crop_h);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        blit(
            &pic,
            &m,
            (x0 - tx, y0 - ty),
            (x1 - x0, y1 - y0),
            canvas,
            (x0 - crop_x, y0 - crop_y),
            threads,
            alpha_only,
        );
    }
    Ok(())
}

/// Decodes the primary image (and its alpha plane) on up to `threads`
/// threads, in stored orientation and cropped.
pub fn decode(data: &[u8], threads: usize) -> Result<Image> {
    let c = Container::parse(data)?;
    let l = layout(&c)?;
    profile(&l.colour)?;
    let (_, _, w, h) = l.crop;
    let mut canvas = Plane::new(w, h, 4);
    draw_item(
        &c,
        c.primary,
        &mut canvas,
        l.crop,
        &l.colour,
        threads,
        false,
    )?;
    let mut alpha = Alpha::Opaque;
    if let Some(a) = c.alpha() {
        // An alpha plane of another size cannot be placed; leave it opaque.
        let ok = c.props_of(a)?.iter().any(|p| match p {
            Prop::Ispe(aw, ah) => (*aw, *ah) == (l.width, l.height),
            _ => false,
        });
        if ok {
            draw_item(&c, a, &mut canvas, l.crop, &None, threads, true)?;
            let px = canvas.data.as_chunks_mut::<4>().0;
            // `prem`: the colour item's samples are multiplied by alpha.
            if c.refs_from(b"prem", c.primary)
                .is_some_and(|t| t.contains(&a))
            {
                for p in px.iter_mut().filter(|p| p[3] != 0 && p[3] != 255) {
                    let a = p[3] as u32;
                    for v in &mut p[..3] {
                        *v = ((*v as u32 * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
            if px.iter().any(|p| p[3] != 255) {
                alpha = Alpha::Straight;
            }
        }
    }
    Ok(Image {
        pixels: Pixels::Packed {
            plane: canvas,
            format: PackedFormat::Rgba8,
            alpha,
        },
        width: w,
        height: h,
        full_width: w,
        full_height: h,
        region: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Encoded with libavif's avifenc (aom, quality 90) from gradients:
    // 64 × 48 with R = 4x, G = 5y, B = 200 left of x = 32 and 40 right of
    // it; the grid file is 128 × 128 with R = 2x, G = 2y and the edge at
    // x = 64. The alpha file is transparent below y = 24.
    const RGB420: &[u8] = include_bytes!("../../testdata/rgb420-64x48.avif");
    const LIMITED_10BIT: &[u8] = include_bytes!("../../testdata/rgb444-10bit-limited-709.avif");
    const RGBA: &[u8] = include_bytes!("../../testdata/rgba-64x48.avif");
    const ROTATED: &[u8] = include_bytes!("../../testdata/irot1-imir1.avif");
    const CLAP: &[u8] = include_bytes!("../../testdata/clap-8-4-40x30.avif");
    const GRID: &[u8] = include_bytes!("../../testdata/grid-2x2-128.avif");
    const P3: &[u8] = include_bytes!("../../testdata/p3-nclx.avif");

    fn pixel(img: &Image, x: u32, y: u32) -> [u8; 4] {
        let Pixels::Packed { plane, .. } = &img.pixels else {
            panic!("packed expected");
        };
        let i = x as usize * 4;
        plane.row(y)[i..i + 4].try_into().unwrap()
    }

    fn assert_close(a: [u8; 4], b: [u8; 4]) {
        assert!(
            a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 6),
            "{a:?} vs {b:?}"
        );
    }

    fn source(x: u32, y: u32) -> [u8; 4] {
        [
            (x * 4) as u8,
            (y * 5) as u8,
            if x < 32 { 200 } else { 40 },
            255,
        ]
    }

    #[test]
    fn decodes_8_and_10_bit() {
        for data in [RGB420, LIMITED_10BIT] {
            let probe = crate::decode::probe(data).unwrap();
            assert_eq!(probe.format, Format::Avif);
            assert_eq!((probe.width, probe.height), (64, 48));
            assert_eq!(probe.orientation, Orientation::NORMAL);
            assert!(probe.icc.is_none());
            for threads in [1, 4] {
                let img = decode(data, threads).unwrap();
                assert_eq!((img.width, img.height), (64, 48));
                assert!(!img.has_alpha());
                for (x, y) in [(5, 5), (20, 40), (45, 10), (60, 44)] {
                    assert_close(pixel(&img, x, y), source(x, y));
                }
            }
        }
    }

    #[test]
    fn alpha_plane() {
        let img = decode(RGBA, 2).unwrap();
        assert!(img.has_alpha());
        assert_eq!(pixel(&img, 10, 5)[3], 255);
        assert_eq!(pixel(&img, 10, 40)[3], 0);
        assert_close(pixel(&img, 10, 5), source(10, 5));
    }

    #[test]
    fn irot_and_imir_become_the_orientation() {
        // Rotated 90° anticlockwise, then mirrored left-right: the same as
        // mirroring first and rotating clockwise (EXIF 7).
        let probe = crate::decode::probe(ROTATED).unwrap();
        assert_eq!((probe.width, probe.height), (64, 48));
        assert_eq!(probe.orientation, Orientation::from_exif(7));
        let img = decode(ROTATED, 1).unwrap();
        assert_close(pixel(&img, 5, 5), source(5, 5));
    }

    #[test]
    fn clap_crops() {
        let probe = crate::decode::probe(CLAP).unwrap();
        assert_eq!((probe.width, probe.height), (40, 30));
        let img = decode(CLAP, 1).unwrap();
        assert_eq!((img.width, img.height), (40, 30));
        assert_close(pixel(&img, 0, 0), source(8, 4));
        assert_close(pixel(&img, 39, 29), source(47, 33));
    }

    #[test]
    fn grid_tiles_are_stitched() {
        let img = decode(GRID, 4).unwrap();
        assert_eq!((img.width, img.height), (128, 128));
        for (x, y) in [(10, 10), (70, 10), (10, 100), (100, 100)] {
            let b = if x < 64 { 200 } else { 40 };
            assert_close(pixel(&img, x, y), [(x * 2) as u8, (y * 2) as u8, b, 255]);
        }
    }

    #[test]
    fn display_p3_gets_a_profile() {
        let probe = crate::decode::probe(P3).unwrap();
        assert!(probe.icc.is_some());
        assert!(container_profile(P3).is_some());
        assert!(container_profile(RGB420).is_none());
    }

    #[test]
    fn hdr_and_broken_files_are_refused_without_panicking() {
        let pq = Some(Colour::Nclx {
            primaries: 9,
            transfer: 16,
            matrix: 9,
            full_range: false,
        });
        assert!(matches!(
            profile(&pq),
            Err(DecodeError::Unsupported(Format::Avif))
        ));
        for data in [RGB420, RGBA, GRID, CLAP] {
            for len in (0..data.len()).step_by(37) {
                let _ = info(&data[..len]);
                let _ = decode(&data[..len], 1);
            }
            let mut bad = data.to_vec();
            for i in (0..bad.len()).step_by(13) {
                bad[i] ^= 0xa5;
            }
            let _ = info(&bad);
            let _ = decode(&bad, 1);
        }
    }
}
