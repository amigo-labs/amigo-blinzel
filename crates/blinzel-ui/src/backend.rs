//! What a platform shell provides. The UI ([`crate::app::App`], the
//! loader, the thumbnail pool) only talks to these traits; Windows,
//! macOS and Linux each implement them natively.
//!
//! - [`Device`]: GPU resources, shared with decode threads (uploads run
//!   where the image was decoded).
//! - [`Canvas`]: drawing a frame on the UI thread.
//! - [`Host`]: the window, on the UI thread.
//! - [`Services`]: everything background threads need from the platform
//!   (files, the platform's decoders, waking the UI thread).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::decode::{DecodeError, Probe, Request};
use blinzel_core::filelist::Entry;
use blinzel_core::geom::{Affine, Orientation, Rect};
use blinzel_core::icc::Profile;
use blinzel_core::lut::Lut3d;
use blinzel_core::pixel::Image;
use blinzel_i18n::Lang;

use crate::gfx::{AtlasPages, ImageInfo, Placed, Rgba, Shape};

/// The platform types one shell is built from.
pub trait Backend: 'static {
    type Device: Device;
    type Canvas: Canvas<Device = Self::Device>;
    type Host: Host;
    type Services: Services;
}

/// GPU resources. Thread-safe: decode workers upload what they decoded.
pub trait Device: Send + Sync + 'static {
    /// An image stage on the GPU.
    type Image: ImageInfo + Send + Sync + 'static;
    /// A colour transform (3D LUT) on the GPU.
    type Lut: Clone + 'static;

    fn upload(&self, img: &Image, orientation: Orientation) -> Result<Self::Image, String>;
    fn upload_lut(&self, lut: &Lut3d) -> Result<Self::Lut, String>;
    /// Bytes of video memory the viewer may keep in textures.
    fn texture_budget(&self) -> usize;
    /// The adapter's name (benchmarks tell real GPUs from software ones).
    fn adapter_name(&self) -> String;
    /// A software renderer (WARP, lavapipe): hundreds of ms per frame.
    fn is_software(&self) -> bool;
}

/// Drawing one frame into the window (UI thread only).
pub trait Canvas: AtlasPages {
    type Device: Device;

    /// Clears the target; false if there is nothing to draw into.
    fn begin(&mut self, clear: Rgba) -> bool;
    /// Draws the visible part (`content`, stage texel coordinates) of an
    /// image stage with `xform` (stage texels → client pixels).
    fn draw_image(
        &mut self,
        img: &<Self::Device as Device>::Image,
        xform: &Affine,
        content: &Rect,
        bg: Rgba,
        lut: Option<&<Self::Device as Device>::Lut>,
    );
    /// UI shapes in client pixels, premultiplied-alpha blended.
    fn draw_shapes(&mut self, shapes: &[Shape]);
    /// An atlas thumbnail into `dest`, oriented, with opacity `alpha`.
    fn draw_thumb(&mut self, t: &Placed, dest: &Rect, alpha: f32);
    /// Size of `text` laid out at `px` pixels, wrapped at `max_width`.
    fn text_size(&mut self, text: &str, px: f32, max_width: f32) -> Option<(u32, u32)>;
    /// Draws `text` (laid out as for [`Canvas::text_size`]) at `pos`.
    fn draw_text(&mut self, text: &str, px: f32, max_width: f32, pos: (f32, f32), color: Rgba);
    fn present(&mut self);
    /// The window's client area changed.
    fn resize(&mut self, size: (u32, u32));
    /// Gives idle GPU memory back (resident mode, hidden).
    fn trim(&mut self) {}
}

/// Timers the UI sets on its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Timer {
    /// Advances an animation.
    Anim,
}

/// The window (UI thread only).
pub trait Host {
    /// Asks for a repaint.
    fn invalidate(&self);
    fn set_title(&self, title: &str);
    fn set_timer(&self, timer: Timer, ms: u32);
    fn kill_timer(&self, timer: Timer);
    /// A client point in screen coordinates (where menus open).
    fn to_screen(&self, p: (f64, f64)) -> (i32, i32);
    /// The monitor showing the window: an id (colour target, monitor
    /// changes) and its size in physical pixels.
    fn monitor(&self) -> Option<(isize, (u32, u32))>;
    /// Takes the clipboard with pixels produced on demand; false if the
    /// clipboard could not be opened.
    fn offer_clipboard(&self) -> bool;
    /// Watches `dir` and sends [`Wake::DirChanged`] on changes until the
    /// returned guard is dropped.
    fn watch_dir(&self, dir: &Path) -> Option<Box<dyn std::any::Any>>;
    /// The first image reached the screen (updater bookkeeping, warming
    /// up what later frames need).
    fn first_frame_shown(&self) {}
}

/// Why the UI thread was woken from another thread.
#[derive(Debug)]
pub enum Wake {
    /// Decoded stages wait in the loader's channel.
    Loaded,
    /// The folder listing is ready.
    List,
    /// The folder changed on disk.
    DirChanged,
    /// Thumbnails wait in the thumbnail pool's channel.
    Thumbs,
    /// The colour target of a monitor, asked for in the background.
    ColorTarget {
        monitor: isize,
        target: Option<Profile>,
    },
}

/// What background threads need from the platform. Thread-safe.
pub trait Services: Send + Sync + 'static {
    /// Wakes the UI thread (posts to its message loop).
    fn wake(&self, w: Wake);
    fn read_file(&self, path: &Path) -> std::io::Result<PoolBuf>;
    fn list_dir(&self, dir: &Path) -> std::io::Result<Vec<Entry>>;
    fn entry(&self, path: &Path) -> std::io::Result<Entry>;
    /// Native decoders, then the platform's.
    fn probe(&self, data: &[u8]) -> Result<Probe, DecodeError>;
    fn decode(&self, data: &[u8], req: Request, threads: usize) -> Result<Image, DecodeError>;
    /// Thread priority: on the path to the image the user waits for, or
    /// background work.
    fn tune_thread(&self, _latency: bool) {}
    fn log(&self, msg: &str);
    /// The colour profile `monitor` wants (`None`: sRGB). May be slow.
    fn display_target(&self, monitor: isize) -> Option<Profile>;
    /// Threads for thumbnails.
    fn thumb_threads(&self) -> usize;
    /// Where the thumbnail cache lives.
    fn cache_dir(&self) -> Option<PathBuf>;
    fn system_language(&self) -> Lang;
    /// The first decoded image while the GPU is still starting (Windows
    /// shows it through GDI meanwhile).
    fn early_image(&self, _img: &Arc<Image>, _probe: &Probe) {}
    /// Whether [`Services::early_image`] still wants an image.
    fn wants_early_image(&self) -> bool {
        false
    }
}

/// Keys the UI reacts to, independent of the platform's key codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Space,
    Backspace,
    Enter,
    Escape,
    Delete,
    F10,
    F11,
    /// The context-menu key.
    Menu,
    Plus,
    Minus,
    /// 0-9, top row or numeric keypad.
    Digit(u8),
    /// A-Z (upper case).
    Letter(char),
}

/// Cursor shapes the shell maps to system cursors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Arrow,
    Cross,
    Hand,
    Move,
    SizeNS,
    SizeWE,
    SizeNWSE,
    SizeNESW,
}
