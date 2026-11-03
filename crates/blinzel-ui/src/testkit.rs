//! Headless harness for [`App`]: fake backend implementations that record
//! what the UI asks of the platform, a temp folder with real image files,
//! and a driver that runs the loader until it is idle, so App-level
//! behaviour (keys, commands, menu state, mouse, loader results, grid,
//! filmstrip, batch dialog) can be tested on Linux.
//!
//! ```text
//! let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
//! h.press(Key::Right, false, false);
//! assert_eq!(h.current_name().as_deref(), Some("b.png"));
//! ```
//!
//! Each [`Harness`] holds a process-wide lock while it lives: the UI
//! language is a global, so harness tests run one after another.

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::decode::{self, DecodeError, Probe, Request};
use blinzel_core::exif::EmbeddedPreview;
use blinzel_core::filelist::Entry;
use blinzel_core::geom::{Affine, Orientation, Rect};
use blinzel_core::icc::Profile;
use blinzel_core::layout::{Layout, ViewMode, ViewState};
use blinzel_core::lut::Lut3d;
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};
use blinzel_core::settings::Settings;
use blinzel_i18n::Lang;

use crate::app::{Action, App};
use crate::backend::{Backend, Canvas, Device, Host, Key, Services, Timer, Wake};
use crate::browse::grid_layout;
use crate::color::ColorManager;
use crate::gfx::{AtlasPages, ImageInfo, Placed, Rgba, Shape};
use crate::loader::Loader;
use crate::menu::{Cmd, MenuItem, MenuState};

// ------------------------------------------------------------------ device

/// An image stage "on the GPU": only its sizes.
#[derive(Debug)]
pub(crate) struct FakeImage {
    pub size: (u32, u32),
    pub full_size: (u32, u32),
    pub orientation: Orientation,
}

impl ImageInfo for FakeImage {
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn full_size(&self) -> (u32, u32) {
        self.full_size
    }
    fn orientation(&self) -> Orientation {
        self.orientation
    }
    fn bytes(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize * 4
    }
}

/// A device that keeps nothing but a log of the uploads.
#[derive(Default)]
pub(crate) struct FakeDevice {
    /// Stage size of every image uploaded, in order.
    pub uploads: Mutex<Vec<(u32, u32)>>,
    /// The texture budget it reports; `None`: 1 GiB, like Linux.
    pub budget: Option<usize>,
    /// When set, that many more uploads succeed, then one fails (out of
    /// GPU memory) and the later ones succeed again.
    pub uploads_left: Mutex<Option<usize>>,
}

impl Device for FakeDevice {
    type Image = FakeImage;
    type Lut = ();

    fn upload(&self, img: &Image, orientation: Orientation) -> Result<FakeImage, String> {
        let size = (img.width, img.height);
        let full_size = (img.full_width, img.full_height);
        let mut left = self.uploads_left.lock().unwrap();
        match left.as_mut() {
            Some(0) => {
                *left = None;
                return Err("out of memory".into());
            }
            Some(n) => *n -= 1,
            None => {}
        }
        drop(left);
        self.uploads.lock().unwrap().push(size);
        Ok(FakeImage {
            size,
            full_size,
            orientation,
        })
    }
    fn upload_lut(&self, _: &Lut3d) -> Result<(), String> {
        Ok(())
    }
    fn texture_budget(&self) -> usize {
        self.budget.unwrap_or(1 << 30)
    }
    fn adapter_name(&self) -> String {
        "test".into()
    }
    fn is_software(&self) -> bool {
        true
    }
}

// ------------------------------------------------------------------ canvas

/// An image drawn in a frame.
#[derive(Clone, Debug)]
pub(crate) struct DrawnImage {
    /// The stage's size (texels).
    pub size: (u32, u32),
    pub full_size: (u32, u32),
    /// Drawn through a colour transform.
    pub converted: bool,
}

/// What one [`App::render`] drew.
#[derive(Clone, Debug, Default)]
pub(crate) struct Frame {
    pub images: Vec<DrawnImage>,
    pub shapes: Vec<Shape>,
    /// Thumbnails, by destination rectangle.
    pub thumbs: Vec<Rect>,
    pub texts: Vec<String>,
    /// The colour of each of [`Frame::texts`].
    pub text_colors: Vec<Rgba>,
    /// The top-left corner of each of [`Frame::texts`].
    pub text_positions: Vec<(f32, f32)>,
}

/// A canvas that measures text (7 px per character, 16 px high) and
/// records each frame.
#[derive(Default)]
pub(crate) struct FakeCanvas {
    /// The frame being drawn, then the last one presented.
    pub frame: Frame,
    /// Frames presented so far.
    pub presented: usize,
    /// Every thumbnail uploaded to the atlas, in order.
    pub thumb_planes: Vec<blinzel_core::thumb::Planes>,
}

impl AtlasPages for FakeCanvas {
    fn add_page(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn upload(
        &mut self,
        _: usize,
        _: (u32, u32),
        _: (u32, u32),
        _: (u32, u32),
        planes: &blinzel_core::thumb::Planes,
    ) {
        self.thumb_planes.push(planes.clone());
    }
}

impl Canvas for FakeCanvas {
    type Device = FakeDevice;

    fn begin(&mut self, _: Rgba) -> bool {
        self.frame = Frame::default();
        true
    }
    fn draw_image(&mut self, img: &FakeImage, _: &Affine, _: &Rect, _: Rgba, lut: Option<&()>) {
        self.frame.images.push(DrawnImage {
            size: img.size,
            full_size: img.full_size,
            converted: lut.is_some(),
        });
    }
    fn draw_shapes(&mut self, shapes: &[Shape]) {
        self.frame.shapes.extend_from_slice(shapes);
    }
    fn draw_thumb(&mut self, _: &Placed, dest: &Rect, _: f32) {
        self.frame.thumbs.push(*dest);
    }
    fn text_size(&mut self, text: &str, _: f32, _: f32) -> Option<(u32, u32)> {
        Some((7 * text.chars().count() as u32, 16))
    }
    fn draw_text(&mut self, text: &str, _: f32, _: f32, at: (f32, f32), color: Rgba) {
        self.frame.texts.push(text.to_string());
        self.frame.text_colors.push(color);
        self.frame.text_positions.push(at);
    }
    fn present(&mut self) {
        self.presented += 1;
    }
    fn resize(&mut self, _: (u32, u32)) {}
}

// -------------------------------------------------------------------- host

/// Everything the UI asked of its window.
#[derive(Debug, Default)]
pub(crate) struct HostLog {
    pub invalidations: usize,
    pub title: String,
    /// Timers set and not killed, with their interval.
    pub timers: HashMap<Timer, u32>,
    pub watched: Vec<PathBuf>,
    pub clipboard_offers: usize,
    pub first_frame_shown: bool,
}

/// A window on a 1920 × 1080 monitor (id 1 until a test moves it);
/// client points are screen points.
pub(crate) struct FakeHost {
    pub log: RefCell<HostLog>,
    /// The id of the monitor the window is on.
    pub monitor: Cell<isize>,
}

impl Default for FakeHost {
    fn default() -> FakeHost {
        FakeHost {
            log: RefCell::default(),
            monitor: Cell::new(1),
        }
    }
}

impl Host for FakeHost {
    fn invalidate(&self) {
        self.log.borrow_mut().invalidations += 1;
    }
    fn set_title(&self, title: &str) {
        self.log.borrow_mut().title = title.to_string();
    }
    fn set_timer(&self, timer: Timer, ms: u32) {
        self.log.borrow_mut().timers.insert(timer, ms);
    }
    fn kill_timer(&self, timer: Timer) {
        self.log.borrow_mut().timers.remove(&timer);
    }
    fn to_screen(&self, p: (f64, f64)) -> (i32, i32) {
        (p.0 as i32, p.1 as i32)
    }
    fn monitor(&self) -> Option<(isize, (u32, u32))> {
        Some((self.monitor.get(), (1920, 1080)))
    }
    fn offer_clipboard(&self) -> bool {
        self.log.borrow_mut().clipboard_offers += 1;
        true
    }
    fn watch_dir(&self, dir: &Path) -> Option<Box<dyn std::any::Any>> {
        self.log.borrow_mut().watched.push(dir.to_path_buf());
        Some(Box::new(()))
    }
    fn first_frame_shown(&self) {
        self.log.borrow_mut().first_frame_shown = true;
    }
}

// ---------------------------------------------------------------- services

/// The real file system and the crate's own decoders; wakes go into a
/// channel the [`Harness`] drains. English unless a test sets a system
/// language ([`Harness::set_system_language`]), UTC unless a test sets an
/// offset ([`Harness::set_utc_offset`]), sRGB monitors unless a test
/// gives one a profile ([`FakeServices::targets`]); log lines go to the
/// test's output.
pub(crate) struct FakeServices {
    wakes: Mutex<Sender<Wake>>,
    cache: PathBuf,
    /// Every file read, with the reading thread's name.
    pub reads: Mutex<Vec<(String, PathBuf)>>,
    /// File attributes the listing reports, by name (cloud placeholders).
    pub attributes: Mutex<HashMap<String, u32>>,
    /// Seconds the local clock is ahead of UTC, at every moment.
    pub utc_offset: AtomicI32,
    /// When set, every decode fails with this error (e.g. one from WIC).
    pub decode_error: Mutex<Option<DecodeError>>,
    /// Every decode: the probed size of the image and the threads it got.
    pub decodes: Mutex<Vec<((u32, u32), usize)>>,
    /// Embedded previews the probe reports, by file length.
    pub previews: Mutex<HashMap<usize, EmbeddedPreview>>,
    /// A stand-in system decoder: what the native decoders report as not
    /// decoded natively (a raw's sensor data) develops to this size.
    pub develops: Mutex<Option<(u32, u32)>>,
    /// The colour target of each monitor that is not sRGB.
    pub targets: Mutex<HashMap<isize, Profile>>,
    /// How often the App asked for a monitor's colour target.
    pub target_queries: AtomicUsize,
    /// Held by a test, it keeps every colour-target answer back.
    pub target_gate: Mutex<()>,
    /// The language the system prefers ([`Harness::set_system_language`]).
    pub system_language: Mutex<Lang>,
}

impl FakeServices {
    fn with_attributes(&self, mut e: Entry) -> Entry {
        if let Some(a) = self.attributes.lock().unwrap().get(&e.name) {
            e.attributes |= a;
        }
        e
    }
}

impl Services for FakeServices {
    fn wake(&self, w: Wake) {
        let _ = self.wakes.lock().unwrap().send(w);
    }
    fn read_file(&self, path: &Path) -> std::io::Result<PoolBuf> {
        let thread = std::thread::current().name().unwrap_or("").to_string();
        self.reads
            .lock()
            .unwrap()
            .push((thread, path.to_path_buf()));
        crate::files::read_file(path)
    }
    fn list_dir(&self, dir: &Path) -> std::io::Result<Vec<Entry>> {
        let list = crate::files::list_dir(dir)?;
        Ok(list.into_iter().map(|e| self.with_attributes(e)).collect())
    }
    fn entry(&self, path: &Path) -> std::io::Result<Entry> {
        crate::files::entry(path).map(|e| self.with_attributes(e))
    }
    fn probe(&self, data: &[u8]) -> Result<Probe, DecodeError> {
        let mut p = decode::probe(data)?;
        if let Some(preview) = self.previews.lock().unwrap().get(&data.len()) {
            p.previews.insert(0, preview.clone());
        }
        Ok(p)
    }
    fn decode(&self, data: &[u8], req: Request, threads: usize) -> Result<Image, DecodeError> {
        if let Ok(p) = decode::probe(data) {
            let mut d = self.decodes.lock().unwrap();
            d.push(((p.width, p.height), threads));
        }
        if let Some(e) = self.decode_error.lock().unwrap().clone() {
            return Err(e);
        }
        match decode::decode_with(data, req, threads) {
            Err(DecodeError::Unsupported(_)) if let Some(size) = *self.develops.lock().unwrap() => {
                Ok(developed(size, req))
            }
            r => r,
        }
    }
    fn log(&self, msg: &str) {
        eprintln!("blinzel: {msg}");
    }
    fn display_target(&self, monitor: isize) -> Option<Profile> {
        self.target_queries.fetch_add(1, Ordering::Relaxed);
        let _answer = self.target_gate.lock().unwrap();
        self.targets.lock().unwrap().get(&monitor).cloned()
    }
    fn thumb_threads(&self) -> usize {
        1
    }
    fn cache_dir(&self) -> Option<PathBuf> {
        Some(self.cache.clone())
    }
    fn system_language(&self) -> Lang {
        *self.system_language.lock().unwrap()
    }
    fn utc_offset(&self, _t: std::time::SystemTime) -> i32 {
        self.utc_offset.load(Ordering::Relaxed)
    }
}

/// What the stand-in system decoder makes of a file it develops to
/// `size`: a gray image of that size, or fitted to a `Fit` request.
fn developed(size: (u32, u32), req: Request) -> Image {
    let (w, h) = match req {
        Request::Fit { w, h } => (w.min(size.0), h.min(size.1)),
        _ => size,
    };
    Image {
        pixels: Pixels::Packed {
            plane: Plane::new(w, h, 1),
            format: PackedFormat::Gray8,
            alpha: Alpha::Opaque,
        },
        width: w,
        height: h,
        full_width: size.0,
        full_height: size.1,
        region: None,
    }
}

/// The fake platform.
pub(crate) struct TestBackend;

impl Backend for TestBackend {
    type Device = FakeDevice;
    type Canvas = FakeCanvas;
    type Host = FakeHost;
    type Services = FakeServices;
}

// ------------------------------------------------------------- temp folder

/// A folder under the system's temp folder, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> TempDir {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("blinzel-ui-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An opaque PNG of `w` × `h` with the test pattern.
pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
    let bgra: Vec<u8> = blinzel_core::testimg::rgb(w, h)
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], 255])
        .collect();
    blinzel_core::export::encode_png_bgra(&bgra, w, h).unwrap()
}

/// A JPEG of `w` × `h` with the test pattern.
pub(crate) fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let opts = blinzel_core::decode::jpeg::EncodeOptions {
        quality: 90,
        subsampling: 420,
        ..Default::default()
    };
    let rgb = blinzel_core::testimg::rgb(w, h);
    blinzel_core::decode::jpeg::encode_rgb(&rgb, w, h, opts).unwrap()
}

/// A Fujifilm RAF whose header points at `preview` (a JPEG) and whose
/// cropped-size record says `size`: a camera raw only the system decoder
/// develops.
pub(crate) fn raf(preview: &[u8], size: (u16, u16)) -> Vec<u8> {
    let mut d = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
    d.resize(200, 0);
    let meta = 200 + preview.len() as u32;
    d[84..88].copy_from_slice(&200u32.to_be_bytes());
    d[88..92].copy_from_slice(&(preview.len() as u32).to_be_bytes());
    d[92..96].copy_from_slice(&meta.to_be_bytes());
    d.extend_from_slice(preview);
    // One record: 0x111, the cropped size (height, width).
    d.extend(1u32.to_be_bytes());
    d.extend(0x111u16.to_be_bytes());
    d.extend(4u16.to_be_bytes());
    d.extend(size.1.to_be_bytes());
    d.extend(size.0.to_be_bytes());
    d
}

/// A TIFF-based camera raw (laid out like a NEF) whose EXIF says `size`:
/// IFD0 is a reduced-resolution image whose JPEG is `thumb`, its one
/// sub-IFD a JPEG-compressed strip holding `preview`. Only the system
/// decoder develops it; the native probe lists both JPEGs.
pub(crate) fn tiff_raw(thumb: &[u8], preview: &[u8], size: (u32, u32)) -> Vec<u8> {
    // IFD0 at 8 (5 entries), the sub-IFD at 74 (3), the EXIF IFD at 116
    // (2), then the JPEGs.
    let (sub_at, exif_at, thumb_at) = (74u32, 116u32, 146u32);
    let preview_at = thumb_at + thumb.len() as u32;
    let ifd = |d: &mut Vec<u8>, entries: &[(u16, u16, u32)]| {
        d.extend((entries.len() as u16).to_le_bytes());
        for &(tag, typ, v) in entries {
            d.extend(tag.to_le_bytes());
            d.extend(typ.to_le_bytes());
            d.extend(1u32.to_le_bytes());
            d.extend(v.to_le_bytes());
        }
        d.extend(0u32.to_le_bytes());
    };
    let mut d = b"II*\0".to_vec();
    d.extend(8u32.to_le_bytes());
    ifd(
        &mut d,
        &[
            (0x00FE, 4, 1),
            (0x014A, 4, sub_at),
            (0x0201, 4, thumb_at),
            (0x0202, 4, thumb.len() as u32),
            (0x8769, 4, exif_at),
        ],
    );
    assert_eq!(d.len(), sub_at as usize);
    ifd(
        &mut d,
        &[
            (0x0103, 3, 6),
            (0x0111, 4, preview_at),
            (0x0117, 4, preview.len() as u32),
        ],
    );
    assert_eq!(d.len(), exif_at as usize);
    ifd(&mut d, &[(0xA002, 4, size.0), (0xA003, 4, size.1)]);
    assert_eq!(d.len(), thumb_at as usize);
    d.extend_from_slice(thumb);
    d.extend_from_slice(preview);
    d
}

/// An animated GIF of `w` × `h`: one frame per entry of `delays`
/// (centiseconds), frame `i` all in colour `i % 4` of a four-colour
/// palette, with a NETSCAPE loop count `repeat` (`None`: no extension, it
/// plays once).
pub(crate) fn gif(w: u16, h: u16, delays: &[u16], repeat: Option<u16>) -> Vec<u8> {
    gif_on_screen((w, h), (w, h), delays, repeat)
}

/// Like [`gif`], but every frame covers only the top left `w` × `h` of a
/// logical screen of `screen`: a large canvas in a small file.
pub(crate) fn gif_on_screen(
    screen: (u16, u16),
    (w, h): (u16, u16),
    delays: &[u16],
    repeat: Option<u16>,
) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend(screen.0.to_le_bytes());
    out.extend(screen.1.to_le_bytes());
    // A global table of four colours.
    out.extend([0xF1, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    if let Some(n) = repeat {
        out.extend(b"\x21\xFF\x0BNETSCAPE2.0\x03\x01");
        out.extend(n.to_le_bytes());
        out.push(0);
    }
    for (i, delay) in delays.iter().enumerate() {
        // Graphic control (disposal "keep", no transparency), then the
        // frame at the top left without a local table.
        out.extend([0x21, 0xF9, 4, 0x04]);
        out.extend(delay.to_le_bytes());
        out.extend([0, 0, 0x2C, 0, 0, 0, 0]);
        out.extend(w.to_le_bytes());
        out.extend(h.to_le_bytes());
        out.extend([0, 2]);
        // LZW with a clear code (4) before every two pixels keeps every
        // code 3 bits wide; 5 ends the data.
        let mut codes = Vec::new();
        for p in 0..w as usize * h as usize {
            if p % 2 == 0 {
                codes.push(4);
            }
            codes.push(i as u32 % 4);
        }
        codes.push(5);
        let (mut data, mut acc, mut bits) = (Vec::new(), 0u32, 0);
        for c in codes {
            acc |= c << bits;
            bits += 3;
            while bits >= 8 {
                data.push(acc as u8);
                acc >>= 8;
                bits -= 8;
            }
        }
        if bits > 0 {
            data.push(acc as u8);
        }
        for block in data.chunks(255) {
            out.push(block.len() as u8);
            out.extend(block);
        }
        out.push(0);
    }
    out.push(0x3B);
    out
}

/// An animated lossless WebP of `w` × `h`: one full-canvas frame per entry
/// of `delays` (ms), frame `i` all in colour `i % 4` like [`gif`],
/// playing `loops` times (0: for ever), with an EXIF chunk giving
/// `orientation` (1–8).
pub(crate) fn webp(w: u32, h: u32, delays: &[u32], loops: u16, orientation: u16) -> Vec<u8> {
    use blinzel_core::batch::encode::{Codec, Quality, encode};
    fn chunk(code: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = code.to_vec();
        c.extend((data.len() as u32).to_le_bytes());
        c.extend(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    }
    let u24 = |v: u32| v.to_le_bytes()[..3].to_vec();
    // VP8X: animation and EXIF; canvas size minus one.
    let vp8x = [vec![0x02 | 0x08, 0, 0, 0], u24(w - 1), u24(h - 1)].concat();
    let anim = [vec![0, 0, 0, 0], loops.to_le_bytes().to_vec()].concat();
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &vp8x));
    body.extend(chunk(b"ANIM", &anim));
    let q = Quality {
        webp_lossless: true,
        ..Quality::default()
    };
    let colours = [
        [0, 0, 255, 255],
        [0, 255, 0, 255],
        [255, 0, 0, 255],
        [255; 4],
    ];
    for (i, &delay) in delays.iter().enumerate() {
        let bgra = colours[i % 4].repeat(w as usize * h as usize);
        // A simple file: RIFF....WEBP, then the VP8L chunk.
        let still = encode(&bgra, w, h, Codec::Webp, &q, None, None).unwrap();
        assert_eq!(&still[12..16], b"VP8L");
        let mut anmf = [u24(0), u24(0), u24(w - 1), u24(h - 1), u24(delay)].concat();
        // No blending: each frame replaces the canvas.
        anmf.push(0x02);
        anmf.extend(&still[12..]);
        body.extend(chunk(b"ANMF", &anmf));
    }
    // Little-endian TIFF with one IFD entry: Orientation (SHORT).
    let mut tiff = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    tiff.extend(orientation.to_le_bytes());
    tiff.extend([0; 6]);
    body.extend(chunk(b"EXIF", &tiff));
    chunk(b"RIFF", &body)
}

// ------------------------------------------------------------------ driver

/// Size of the images [`Harness::with_images`] writes.
pub(crate) const IMAGE_SIZE: (u32, u32) = (64, 48);
/// Client area of the window.
pub(crate) const CLIENT: (u32, u32) = (800, 600);

/// How long [`Harness::pump`] waits for the loader before failing.
const PUMP_LIMIT: Duration = Duration::from_secs(30);

/// Colour transforms built so far ([`identity_lut`]).
pub(crate) static LUTS_BUILT: AtomicUsize = AtomicUsize::new(0);

/// The source profile of every transform built so far, in order.
pub(crate) static LUT_SOURCES: Mutex<Vec<Option<Profile>>> = Mutex::new(Vec::new());

/// The colour engine: every transform is the identity (the frame records
/// only whether one was used: [`DrawnImage::converted`]).
fn identity_lut(src: Option<&Profile>, _: Option<&Profile>) -> Option<Lut3d> {
    LUTS_BUILT.fetch_add(1, Ordering::Relaxed);
    LUT_SOURCES.lock().unwrap().push(src.cloned());
    let n = blinzel_core::lut::SIZE;
    let step = 65535.0 / (n - 1) as f64;
    let nodes = (0..n * n * n)
        .map(|i| [i % n, i / n % n, i / (n * n)].map(|v| (v as f64 * step).round() as u16))
        .collect();
    Lut3d::from_nodes(n, nodes)
}

/// Harness tests share globals (the UI language): one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

/// An [`App`] over the fake backend and a temp folder, at 96 dpi with an
/// 800 × 600 client area. Settings are saved to `<temp>/config`.
pub(crate) struct Harness {
    pub app: App<TestBackend>,
    /// The images' folder (`<temp>/images`).
    pub images: PathBuf,
    wakes: Receiver<Wake>,
    actions: Vec<Action>,
    rendered_at: usize,
    /// Colour-target answers delivered to the App ([`Wake::ColorTarget`]).
    pub color_answers: usize,
    /// The device's texture budget ([`FakeDevice::budget`]).
    budget: Option<usize>,
    // Dropped after the app (its threads may still read files).
    tmp: TempDir,
    _serial: MutexGuard<'static, ()>,
}

impl Harness {
    /// An empty folder, nothing open, default settings.
    pub fn new() -> Harness {
        Harness::with_settings(Settings::default())
    }

    /// An empty folder, nothing open.
    pub fn with_settings(settings: Settings) -> Harness {
        Harness::with_budget(settings, None)
    }

    /// An empty folder, nothing open, a device whose texture budget is
    /// `budget` bytes (`None`: 1 GiB).
    pub fn with_budget(settings: Settings, budget: Option<usize>) -> Harness {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        blinzel_i18n::set_lang(blinzel_i18n::FALLBACK);
        let tmp = TempDir::new();
        let images = tmp.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        let config = tmp.path().join("config");
        crate::settings_store::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = Some(config));
        let (app, wakes) = Harness::start(tmp.path(), settings, budget);
        Harness {
            app,
            images,
            wakes,
            actions: Vec::new(),
            rendered_at: 0,
            color_answers: 0,
            budget,
            tmp,
            _serial: serial,
        }
    }

    /// A new session over the same folders (images, settings file,
    /// thumbnail cache), with the settings as saved; nothing open.
    pub fn restart(&mut self) {
        let settings = self.saved_settings().unwrap_or_default();
        let (app, wakes) = Harness::start(self.tmp.path(), settings, self.budget);
        self.app = app;
        self.wakes = wakes;
        self.actions.clear();
        self.rendered_at = 0;
        self.color_answers = 0;
    }

    /// A new App over the folders under `tmp`.
    fn start(
        tmp: &Path,
        settings: Settings,
        budget: Option<usize>,
    ) -> (App<TestBackend>, Receiver<Wake>) {
        let (tx, wakes) = channel();
        let services = Arc::new(FakeServices {
            wakes: Mutex::new(tx),
            cache: tmp.join("cache"),
            reads: Mutex::new(Vec::new()),
            attributes: Mutex::new(HashMap::new()),
            utc_offset: AtomicI32::new(0),
            decode_error: Mutex::new(None),
            decodes: Mutex::new(Vec::new()),
            previews: Mutex::new(HashMap::new()),
            develops: Mutex::new(None),
            targets: Mutex::new(HashMap::new()),
            target_queries: AtomicUsize::new(0),
            target_gate: Mutex::new(()),
            system_language: Mutex::new(blinzel_i18n::FALLBACK),
        });
        let device = Arc::new(FakeDevice {
            budget,
            ..FakeDevice::default()
        });
        let loader = Loader::new(services.clone(), 1, 2, 2);
        loader.set_device(device.clone());
        let color = ColorManager::new(settings.color_management, identity_lut);
        color.set_target(None);
        loader.set_color(color.clone());
        let app = App::new(
            FakeHost::default(),
            device,
            FakeCanvas::default(),
            services,
            settings,
            loader,
            color,
            96,
            CLIENT,
        );
        app.update_screen_box();
        (app, wakes)
    }

    /// Writes [`IMAGE_SIZE`] PNGs named `names` and opens the first.
    pub fn with_images(names: &[&str]) -> Harness {
        let mut h = Harness::new();
        for n in names {
            h.write_png(n, IMAGE_SIZE.0, IMAGE_SIZE.1);
        }
        if let Some(first) = names.first() {
            h.open(first);
        }
        h
    }

    /// A PNG of `w` × `h` in the images' folder.
    pub fn write_png(&self, name: &str, w: u32, h: u32) -> PathBuf {
        self.write_file(name, &png(w, h))
    }

    /// A JPEG of `w` × `h` with the test pattern; above the 1920 × 1080
    /// screen box it decodes at a reduced size for the screen.
    pub fn write_jpeg(&self, name: &str, w: u32, h: u32) -> PathBuf {
        self.write_file(name, &jpeg(w, h))
    }

    /// Any file in the images' folder (e.g. a broken image).
    pub fn write_file(&self, name: &str, data: &[u8]) -> PathBuf {
        let p = self.images.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    /// Opens `name` from the images' folder and pumps until idle.
    pub fn open(&mut self, name: &str) -> Vec<Action> {
        let a = self.app.open(self.images.join(name));
        self.record(a)
    }

    // ------------------------------------------------------------- input

    /// [`App::key`] and pump; `None` when the App does not handle the key.
    pub fn press(&mut self, key: Key, ctrl: bool, shift: bool) -> Option<Vec<Action>> {
        let a = self.app.key(key, ctrl, shift)?;
        Some(self.record(a))
    }

    /// A key without modifiers that the App must handle.
    pub fn key(&mut self, key: Key) -> Vec<Action> {
        self.press(key, false, false)
            .unwrap_or_else(|| panic!("{key:?} not handled"))
    }

    /// A key with Ctrl that the App must handle.
    pub fn ctrl(&mut self, key: Key) -> Vec<Action> {
        self.press(key, true, false)
            .unwrap_or_else(|| panic!("Ctrl+{key:?} not handled"))
    }

    /// [`App::command`] (as from the menu) and pump.
    pub fn command(&mut self, cmd: Cmd) -> Vec<Action> {
        let a = self.app.command(cmd);
        self.record(a)
    }

    pub fn mouse_down(&mut self, p: (f64, f64)) -> Vec<Action> {
        let a = self.app.mouse_down(p, false, false, false);
        self.record(a)
    }

    /// Left button down with Shift held.
    pub fn shift_down(&mut self, p: (f64, f64)) -> Vec<Action> {
        let a = self.app.mouse_down(p, true, false, false);
        self.record(a)
    }

    /// Left click at `p` with Ctrl held.
    pub fn ctrl_click(&mut self, p: (f64, f64)) -> Vec<Action> {
        let a = self.app.mouse_down(p, false, true, false);
        let a = self.record(a);
        self.mouse_up();
        a
    }

    pub fn mouse_move(&mut self, p: (f64, f64)) {
        self.app.mouse_move(p);
        self.pump();
    }

    pub fn mouse_up(&mut self) {
        self.app.mouse_up();
        self.pump();
    }

    /// Left click (down and up) at `p`.
    pub fn click(&mut self, p: (f64, f64)) -> Vec<Action> {
        let a = self.mouse_down(p);
        self.mouse_up();
        a
    }

    /// Left drag from `from` to `to` in a few steps.
    pub fn drag(&mut self, from: (f64, f64), to: (f64, f64)) -> Vec<Action> {
        let a = self.mouse_down(from);
        for k in 1..=4 {
            let t = k as f64 / 4.0;
            self.mouse_move((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t));
        }
        self.mouse_up();
        a
    }

    /// Middle-button drag from `from` to `to` in a few steps.
    pub fn middle_drag(&mut self, from: (f64, f64), to: (f64, f64)) -> Vec<Action> {
        let a = self.app.mouse_down(from, false, false, true);
        let a = self.record(a);
        for k in 1..=4 {
            let t = k as f64 / 4.0;
            self.mouse_move((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t));
        }
        self.mouse_up();
        a
    }

    pub fn double_click(&mut self, p: (f64, f64)) -> Option<Vec<Action>> {
        let a = self.app.double_click(p)?;
        Some(self.record(a))
    }

    pub fn wheel(&mut self, p: (f64, f64), delta: i16) -> Vec<Action> {
        let a = self.app.wheel(p, delta);
        self.record(a)
    }

    /// The window's client area changed.
    pub fn resize(&mut self, client: (u32, u32)) {
        self.app.resized(client);
        self.pump();
    }

    // -------------------------------------------------------------- loop

    /// Delivers wakes and renders when invalidated, until the loader has
    /// nothing queued or running and the folder is listed. Returns the
    /// actions that came up meanwhile (they are also in
    /// [`Harness::actions`]). Thumbnails and the colour-target check may
    /// still arrive later.
    pub fn pump(&mut self) -> Vec<Action> {
        let start = self.actions.len();
        let t = Instant::now();
        loop {
            while let Ok(w) = self.wakes.try_recv() {
                self.deliver(w);
            }
            // Results whose wake was already taken.
            let a = self.app.on_loaded();
            self.actions.extend(a);
            if self.app.host.log.borrow().invalidations != self.rendered_at {
                self.render();
            }
            // A result is sent, then woken, then its job leaves the
            // scheduler; the listing is woken after it is sent. So with no
            // job left and no wake waiting, nothing more can arrive.
            if self.app.loader.pending() == 0 && !self.app.is_listing() {
                match self.wakes.try_recv() {
                    Ok(w) => {
                        self.deliver(w);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            assert!(t.elapsed() < PUMP_LIMIT, "the loader did not get idle");
            if let Ok(w) = self.wakes.recv_timeout(Duration::from_millis(5)) {
                self.deliver(w);
            }
        }
        self.actions[start..].iter().map(Action::clone).collect()
    }

    /// Lets every queued and running loader job run to its end before the
    /// App takes any of their results, then pumps. The App cancels a job
    /// once the stage it was for arrives, so after a plain [`Harness::pump`]
    /// a job that wrongly goes on (decoding again, asking the system
    /// decoder) has usually been cut short; a test that asserts what a job
    /// did not do settles instead.
    pub fn settle(&mut self) -> Vec<Action> {
        let t = Instant::now();
        while self.app.loader.pending() > 0 {
            assert!(t.elapsed() < PUMP_LIMIT, "the loader did not get idle");
            std::thread::sleep(Duration::from_millis(2));
        }
        self.pump()
    }

    /// Opens `name` from the images' folder and settles
    /// ([`Harness::settle`]).
    pub fn open_settled(&mut self, name: &str) -> Vec<Action> {
        let a = self.app.open(self.images.join(name));
        self.actions.extend(a.iter().map(Action::clone));
        let mut out = a;
        out.extend(self.settle());
        out
    }

    /// Pumps, then keeps delivering wakes (thumbnails, the colour-target
    /// check) and rendering until `done` holds; fails after a while.
    pub fn wait_for(&mut self, done: impl Fn(&Harness) -> bool) {
        let t = Instant::now();
        self.pump();
        while !done(self) {
            assert!(t.elapsed() < PUMP_LIMIT, "waited in vain");
            if let Ok(w) = self.wakes.recv_timeout(Duration::from_millis(5)) {
                self.deliver(w);
            }
            self.pump();
        }
    }

    /// Like [`Harness::pump`], but the folder listing's wake is held
    /// back: returns once the loader is idle and the listing has been sent
    /// to the App, which has not taken it yet ([`Harness::release_listing`]).
    pub fn pump_holding_listing(&mut self) {
        let t = Instant::now();
        let mut listed = false;
        loop {
            while let Ok(w) = self.wakes.try_recv() {
                match w {
                    Wake::List => listed = true,
                    w => self.deliver(w),
                }
            }
            let a = self.app.on_loaded();
            self.actions.extend(a);
            if self.app.host.log.borrow().invalidations != self.rendered_at {
                self.render();
            }
            if listed && self.app.loader.pending() == 0 {
                break;
            }
            assert!(t.elapsed() < PUMP_LIMIT, "the loader did not get idle");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Hands the listing held by [`Harness::pump_holding_listing`] to the
    /// App, without pumping.
    pub fn release_listing(&mut self) -> Vec<Action> {
        let a = self.app.on_list();
        self.actions.extend(a.iter().map(Action::clone));
        a
    }

    fn deliver(&mut self, w: Wake) {
        let a = match w {
            Wake::Loaded => self.app.on_loaded(),
            Wake::List => self.app.on_list(),
            Wake::DirChanged => {
                self.app.on_dir_changed();
                vec![]
            }
            Wake::Thumbs => {
                self.app.on_thumbs();
                vec![]
            }
            Wake::Batch => {
                self.app.on_batch();
                vec![]
            }
            Wake::ColorTarget { monitor, target } => {
                self.color_answers += 1;
                self.app.on_color_target(monitor, target);
                vec![]
            }
        };
        self.actions.extend(a);
    }

    /// Draws a frame ([`Harness::frame`]). Like every shell, an
    /// [`Action::Uncloak`] it returns uncovers the window at once.
    pub fn render(&mut self) -> Vec<Action> {
        self.rendered_at = self.app.host.log.borrow().invalidations;
        let a = self.app.render();
        if a.contains(&Action::Uncloak) {
            self.app.cloaked = false;
        }
        self.actions.extend(a.iter().map(Action::clone));
        a
    }

    /// Records `a` as returned by the App, pumps, and returns `a` plus
    /// whatever the pump brought.
    fn record(&mut self, a: Vec<Action>) -> Vec<Action> {
        let mut out: Vec<Action> = a.iter().map(Action::clone).collect();
        self.actions.extend(a);
        out.extend(self.pump());
        out
    }

    /// Does what the Linux shell does with the window-state actions in
    /// `actions` (full screen, maximise, uncloak, the menu drawn in the
    /// window, a resize to the asked size, recycling = deleting the temp
    /// file); returns the actions it did not handle.
    pub fn shell(&mut self, actions: Vec<Action>) -> Vec<Action> {
        let mut rest = Vec::new();
        for a in actions {
            let more = match a {
                Action::ToggleFullscreen => {
                    self.app.fullscreen = !self.app.fullscreen;
                    self.app.invalidate();
                    vec![]
                }
                Action::ToggleMaximize => {
                    self.app.maximized = !self.app.maximized;
                    self.app.invalidate();
                    vec![]
                }
                Action::Uncloak => {
                    self.app.cloaked = false;
                    vec![]
                }
                Action::ShowMenu { x, y } => {
                    self.app.open_menu((x as f64, y as f64));
                    vec![]
                }
                Action::FitWindow { client, .. } => {
                    self.app.resized(client);
                    self.app.invalidate();
                    vec![]
                }
                Action::Recycle(path) => {
                    std::fs::remove_file(&path).unwrap();
                    self.app.removed(&path)
                }
                a => {
                    rest.push(a);
                    vec![]
                }
            };
            self.actions.extend(more.iter().map(Action::clone));
            rest.extend(self.shell(more));
        }
        self.pump();
        rest
    }

    // ----------------------------------------------------------- observe

    /// Every action the App returned so far, in order.
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// Forgets the recorded actions.
    pub fn clear_actions(&mut self) {
        self.actions.clear();
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.app.current.as_ref().map(|c| c.path.clone())
    }

    pub fn current_name(&self) -> Option<String> {
        self.app.current.as_ref().map(|c| c.entry.name.clone())
    }

    /// The folder's file names in browsing order.
    pub fn names(&self) -> Vec<String> {
        self.app
            .list()
            .entries()
            .iter()
            .map(|e| e.name.clone())
            .collect()
    }

    /// Per-image view state: zoom, centre, crop.
    pub fn view(&self) -> ViewState {
        self.app.view
    }

    pub fn mode(&self) -> ViewMode {
        self.app.settings.view.mode
    }

    /// Layout of the image on screen (scale = client px per image px).
    pub fn layout(&self) -> Option<Layout> {
        self.app.current_layout()
    }

    pub fn menu_state(&self) -> MenuState {
        self.app.menu_state()
    }

    /// Whether the window menu offers `cmd` enabled (it must be in it).
    pub fn menu_enabled(&self, cmd: Cmd) -> bool {
        self.menu_entry(cmd).1
    }

    /// Whether the window menu shows `cmd` with a check mark.
    pub fn menu_checked(&self, cmd: Cmd) -> bool {
        self.menu_entry(cmd).0
    }

    /// `(checked, enabled)` of `cmd` in the window menu.
    fn menu_entry(&self, cmd: Cmd) -> (bool, bool) {
        fn find(items: &[MenuItem], cmd: Cmd) -> Option<(bool, bool)> {
            items.iter().find_map(|i| match i {
                MenuItem::Item {
                    cmd: c,
                    checked,
                    enabled,
                    ..
                } if *c == cmd => Some((*checked, *enabled)),
                MenuItem::Sub(_, sub) => find(sub, cmd),
                _ => None,
            })
        }
        find(&crate::menu::items(&self.menu_state()), cmd)
            .unwrap_or_else(|| panic!("{cmd:?} not in the menu"))
    }

    /// The client point where image point `p` (oriented full-resolution
    /// pixels) is shown.
    pub fn at(&self, p: (f64, f64)) -> (f64, f64) {
        self.layout().unwrap().image_to_client().apply(p)
    }

    /// The selected tile while the grid is open.
    pub fn grid_selected(&self) -> Option<usize> {
        self.app.browse.grid.as_ref().map(|g| g.selected)
    }

    /// Files the thumbnail pool read, in order.
    pub fn thumb_reads(&self) -> Vec<PathBuf> {
        let reads = self.app.services.reads.lock().unwrap();
        reads
            .iter()
            .filter(|(t, _)| t.starts_with("blinzel-thumb"))
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// Files the prefetch pipeline read, in order.
    pub fn view_reads(&self) -> Vec<String> {
        let reads = self.app.services.reads.lock().unwrap();
        reads
            .iter()
            .filter(|(t, _)| t.starts_with("blinzel-io"))
            .filter_map(|(_, p)| Some(p.file_name()?.to_string_lossy().into_owned()))
            .collect()
    }

    /// The listing reports `name` with `attributes` (e.g. a cloud
    /// placeholder) from now on.
    pub fn set_attributes(&self, name: &str, attributes: u32) {
        let mut a = self.app.services.attributes.lock().unwrap();
        a.insert(name.to_string(), attributes);
    }

    /// The probe reports an embedded preview of `size` at `range` in every
    /// file of `file_len` bytes.
    pub fn set_preview(&self, file_len: usize, range: std::ops::Range<usize>, size: (u32, u32)) {
        let p = EmbeddedPreview {
            range,
            width: size.0,
            height: size.1,
        };
        self.app
            .services
            .previews
            .lock()
            .unwrap()
            .insert(file_len, p);
    }

    /// A stand-in system decoder develops what the native decoders do not
    /// handle to `size` from now on (`None`: no system decoder, as on
    /// Linux).
    pub fn set_system_decoder(&self, size: Option<(u32, u32)>) {
        *self.app.services.develops.lock().unwrap() = size;
    }

    /// Decodes so far: the image's size and the threads it was given.
    pub fn decodes(&self) -> Vec<((u32, u32), usize)> {
        self.app.services.decodes.lock().unwrap().clone()
    }

    /// `n` more uploads to the device succeed, then one fails.
    pub fn fail_upload_after(&self, n: usize) {
        *self.app.device.uploads_left.lock().unwrap() = Some(n);
    }

    /// Every decode fails with `e` from now on (`None`: decode again).
    pub fn set_decode_error(&self, e: Option<DecodeError>) {
        *self.app.services.decode_error.lock().unwrap() = e;
    }

    /// The system prefers `lang` from now on (English until then).
    pub fn set_system_language(&self, lang: Lang) {
        *self.app.services.system_language.lock().unwrap() = lang;
    }

    /// The platform's clock is `secs` ahead of UTC from now on.
    pub fn set_utc_offset(&self, secs: i32) {
        self.app.services.utc_offset.store(secs, Ordering::Relaxed);
    }

    /// The marked tiles' names while the grid is open.
    pub fn grid_marked(&self) -> Vec<String> {
        let g = self.app.browse.grid.as_ref();
        g.map(|g| g.marked.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The files in the open batch dialog's list (`None`: it is closed).
    pub fn batch_files(&self) -> Option<Vec<PathBuf>> {
        self.app.dialog.as_ref().map(|d| d.file_paths())
    }

    /// Client rectangle of grid tile `i` while the grid is open.
    pub fn grid_cell(&self, i: usize) -> Option<Rect> {
        let g = self.app.browse.grid.as_ref()?;
        let (w, h) = self.app.client;
        let layout = grid_layout(
            self.app.list().len(),
            (w as f64, h as f64),
            self.app.scale(),
        );
        Some(layout.cell_rect(i, g.scroll))
    }

    /// What the UI asked of its window.
    pub fn host(&self) -> Ref<'_, HostLog> {
        self.app.host.log.borrow()
    }

    /// The last frame drawn.
    pub fn frame(&self) -> &Frame {
        &self.app.canvas.frame
    }

    /// Where the App saves its settings (`<temp>/config/settings.toml`).
    pub fn settings_file(&self) -> PathBuf {
        self.tmp.path().join("config").join("settings.toml")
    }

    /// The settings file's text as saved (`None`: never saved).
    pub fn saved_text(&self) -> Option<String> {
        std::fs::read_to_string(self.settings_file()).ok()
    }

    /// The settings file as saved (`None`: never saved).
    pub fn saved_settings(&self) -> Option<Settings> {
        self.saved_text().map(|t| Settings::parse(&t))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        crate::settings_store::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Key::*;

    fn centre(r: Rect) -> (f64, f64) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn the_fakes_record_what_the_app_asked_for() {
        let mut h = Harness::with_images(&["a.png", "b.png"]);
        assert_eq!(h.host().watched, [h.images.clone()]);
        assert!(h.host().first_frame_shown);
        assert!(h.host().timers.is_empty(), "a still image sets no timer");
        assert!(h.app.canvas.presented >= 1);
        assert!(h.app.device.uploads.lock().unwrap().contains(&IMAGE_SIZE));
        assert_eq!(h.frame().images[0].size, IMAGE_SIZE);
        h.ctrl(Letter('C'));
        assert_eq!(h.host().clipboard_offers, 1);
        assert_eq!(h.app.clip.as_ref().unwrap().path, h.images.join("a.png"));
        h.clear_actions();
        h.command(Cmd::Next);
        assert_eq!(h.current_name().as_deref(), Some("b.png"));
        assert!(!h.actions().contains(&Action::Uncloak), "cleared");
    }

    #[test]
    fn mouse_drag_selects_and_escape_clears_it() {
        let mut h = Harness::with_images(&["a.png"]);
        let shapes = h.frame().shapes.len();
        // The 64 × 48 image sits at 100 % in the middle of 800 × 600.
        h.drag((380.0, 285.0), (420.0, 315.0));
        let sel = h.app.selection().unwrap();
        assert!(sel.w > 30.0 && sel.h > 20.0, "{sel:?}");
        assert!(h.frame().shapes.len() > shapes, "selection drawn");
        assert_eq!(h.key(Escape), []);
        assert!(h.app.selection().is_none());
        // A plain click on the image keeps it clear.
        h.click((400.0, 300.0));
        assert!(h.app.selection().is_none());
    }

    #[test]
    fn wheel_keys_and_resize_change_the_layout() {
        let mut h = Harness::with_images(&["a.png"]);
        assert_eq!(h.layout().unwrap().scale, 1.0);
        h.wheel((400.0, 300.0), 120);
        assert!((h.layout().unwrap().scale - 1.2).abs() < 1e-9);
        assert!(h.view().zoom.is_some());
        h.key(Plus);
        assert_eq!(h.layout().unwrap().scale, 1.5);
        h.key(Digit(0));
        assert_eq!(h.view(), ViewState::default());
        h.resize((400, 300));
        assert_eq!(h.layout().unwrap().client, (400.0, 300.0));
    }

    #[test]
    fn shell_feedback_toggles_full_screen_and_rotation_resets_on_navigation() {
        let mut h = Harness::with_images(&["a.png", "b.png"]);
        let a = h.key(Enter);
        assert_eq!(a, [Action::ToggleFullscreen]);
        assert_eq!(h.shell(a), []);
        assert!(h.app.fullscreen && h.menu_state().fullscreen);
        h.key(Letter('R'));
        assert_eq!(h.app.user_rotation(), Orientation::NORMAL.rotated_cw());
        h.key(Right);
        assert_eq!(h.app.user_rotation(), Orientation::NORMAL);
        let a = h.key(Escape);
        assert_eq!(a, [Action::ToggleFullscreen]);
        h.shell(a);
        assert!(!h.app.fullscreen);
    }

    #[test]
    fn grid_tiles_are_clicked_and_opened() {
        let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
        h.key(Letter('G'));
        assert_eq!(h.grid_selected(), Some(0));
        assert!(h.menu_state().grid);
        h.wait_for(|h| h.frame().thumbs.len() == 3);
        let c = centre(h.grid_cell(2).unwrap());
        assert_eq!(h.click(c), []);
        assert_eq!(h.grid_selected(), Some(2));
        h.double_click(c).unwrap();
        assert_eq!(h.grid_selected(), None);
        assert_eq!(h.current_name().as_deref(), Some("c.png"));
    }

    #[test]
    fn a_broken_file_shows_its_error() {
        let mut h = Harness::new();
        h.write_file("bad.png", b"not an image");
        h.open("bad.png");
        let id = h.app.current.as_ref().unwrap().cache_id;
        assert!(h.app.error_of(id).is_some());
        assert!(h.frame().images.is_empty());
        assert!(
            h.frame().texts.iter().any(|t| t.contains("bad.png")),
            "{:?}",
            h.frame().texts
        );
    }
}
