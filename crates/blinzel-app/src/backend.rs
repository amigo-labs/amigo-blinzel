//! The Windows implementation of `blinzel_ui`'s backend traits: Direct3D 11
//! for the device and the canvas, the Win32 window as the host, and window
//! messages to wake the UI thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::decode::{DecodeError, Probe, Request};
use blinzel_core::filelist::Entry;
use blinzel_core::geom::{Affine, Orientation, Rect};
use blinzel_core::icc::Profile;
use blinzel_core::lut::Lut3d;
use blinzel_core::pixel::Image;
use blinzel_core::thumb::Planes;
use blinzel_i18n::Lang;
use blinzel_ui::backend::{Backend, Canvas, Device, Host, Services, Timer, Wake};
use blinzel_ui::color::ColorManager;
use blinzel_ui::gfx::{AtlasPages, Placed, Rgba, Shape};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, HMONITOR, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, PostMessageW, SetTimer, SetWindowTextW, WM_APP,
};
use windows::core::PCWSTR;

use crate::gfx::atlas::Pages;
use crate::gfx::text::Text;
use crate::gfx::texture::{DEFAULT_TILE, GpuImage, GpuLut, upload, upload_lut};
use crate::gfx::{Gfx, Renderer, SwapChain};
use crate::platform::threads::{self, Kind};
use crate::platform::{debug_log, enumerate, io, wide};
use crate::{clipboard, first_frame, wic};

/// Decoded stages wait in the loader's channel.
pub const WM_LOADED: u32 = WM_APP + 1;
/// The folder listing is ready.
pub const WM_LIST: u32 = WM_APP + 2;
/// The folder changed on disk.
pub const WM_DIR_CHANGED: u32 = WM_APP + 3;
/// LPARAM: `Box<(isize, Option<Profile>)>`, a monitor and its colour target.
pub const WM_COLOR_TARGET: u32 = WM_APP + 8;
/// Thumbnails wait in the thumbnail pool's channel.
pub const WM_THUMB: u32 = WM_APP + 9;
/// The animation timer (the window procedure dispatches it).
pub const TIMER_ANIM: usize = 3;

/// The Windows shell's backend types.
pub struct Win;

impl Backend for Win {
    type Device = D3dDevice;
    type Canvas = D3dCanvas;
    type Host = WinHost;
    type Services = WinServices;
}

pub type App = blinzel_ui::app::App<Win>;
pub type Loader = blinzel_ui::loader::Loader<D3dDevice, WinServices>;

// ------------------------------------------------------------------ device

/// The Direct3D 11 device, shared with the decode workers (free-threaded).
pub struct D3dDevice {
    pub device: ID3D11Device,
    tile: u32,
    budget: usize,
    adapter: String,
    warp: bool,
}

impl D3dDevice {
    pub fn new(gfx: &Gfx) -> D3dDevice {
        D3dDevice {
            device: gfx.device.clone(),
            tile: DEFAULT_TILE.min(gfx.max_texture_size()),
            budget: gfx.texture_budget(),
            adapter: gfx.adapter_name(),
            warp: gfx.warp,
        }
    }
}

impl Device for D3dDevice {
    type Image = GpuImage;
    type Lut = GpuLut;

    fn upload(&self, img: &Image, orientation: Orientation) -> Result<GpuImage, String> {
        upload(&self.device, img, orientation, self.tile).map_err(|e| e.message())
    }

    fn upload_lut(&self, lut: &Lut3d) -> Result<GpuLut, String> {
        upload_lut(&self.device, lut).map_err(|e| e.message())
    }

    fn texture_budget(&self) -> usize {
        self.budget
    }

    fn adapter_name(&self) -> String {
        self.adapter.clone()
    }

    fn is_software(&self) -> bool {
        self.warp
    }
}

// ------------------------------------------------------------------ canvas

/// Drawing into the window's swap chain on the UI thread.
pub struct D3dCanvas {
    pub gfx: Gfx,
    renderer: Renderer,
    pub swap: SwapChain,
    text: Option<Result<Text, ()>>,
    pages: Pages,
}

impl D3dCanvas {
    pub fn new(gfx: Gfx, renderer: Renderer, swap: SwapChain) -> D3dCanvas {
        D3dCanvas {
            pages: Pages::new(&gfx.device, &gfx.context),
            gfx,
            renderer,
            swap,
            text: None,
        }
    }

    fn text(&mut self) -> Option<&mut Text> {
        if self.text.is_none() {
            self.text = Some(Text::new(&self.gfx.device).map_err(|_| ()));
        }
        self.text.as_mut().and_then(|t| t.as_mut().ok())
    }
}

impl AtlasPages for D3dCanvas {
    fn add_page(&mut self) -> Result<(), String> {
        self.pages.add_page()
    }

    fn upload(
        &mut self,
        page: usize,
        origin: (u32, u32),
        size: (u32, u32),
        chroma: (u32, u32),
        thumb: &Planes,
    ) {
        self.pages.upload(page, origin, size, chroma, thumb);
    }
}

impl Canvas for D3dCanvas {
    type Device = D3dDevice;

    fn begin(&mut self, clear: Rgba) -> bool {
        let Some(rtv) = self.swap.rtv().cloned() else {
            return false;
        };
        self.renderer
            .begin(&self.gfx.context, &rtv, self.swap.size, clear);
        true
    }

    fn draw_image(
        &mut self,
        img: &GpuImage,
        xform: &Affine,
        content: &Rect,
        bg: Rgba,
        lut: Option<&GpuLut>,
    ) {
        self.renderer
            .draw_image(&self.gfx.context, img, xform, content, bg, lut);
    }

    fn draw_shapes(&mut self, shapes: &[Shape]) {
        self.renderer.draw_shapes(&self.gfx.context, shapes);
    }

    fn draw_thumb(&mut self, t: &Placed, dest: &Rect, alpha: f32) {
        if let Some(page) = self.pages.pages.get(t.page) {
            self.renderer
                .draw_thumb(&self.gfx.context, page, t, dest, alpha);
        }
    }

    fn text_size(&mut self, text: &str, px: f32, max_width: f32) -> Option<(u32, u32)> {
        self.text()?.get(text, px, max_width).ok().map(|t| t.size)
    }

    fn draw_text(&mut self, text: &str, px: f32, max_width: f32, pos: (f32, f32), color: Rgba) {
        let ctx = self.gfx.context.clone();
        let Some(t) = self.text() else { return };
        let Ok(t) = t.get(text, px, max_width) else {
            return;
        };
        let (srv, size) = (t.srv.clone(), t.size);
        self.renderer.draw_text(&ctx, &srv, pos, size, color);
    }

    fn present(&mut self) {
        let _ = self.swap.present();
    }

    fn resize(&mut self, size: (u32, u32)) {
        let _ = self.swap.resize(&self.gfx, size);
    }

    fn trim(&mut self) {
        self.gfx.trim();
    }
}

// -------------------------------------------------------------------- host

/// The viewer window.
pub struct WinHost {
    pub hwnd: HWND,
}

impl Host for WinHost {
    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn set_title(&self, title: &str) {
        let w = wide(title);
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(w.as_ptr()));
        }
    }

    fn set_timer(&self, timer: Timer, ms: u32) {
        let Timer::Anim = timer;
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_ANIM, ms, None);
        }
    }

    fn kill_timer(&self, timer: Timer) {
        let Timer::Anim = timer;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ANIM);
        }
    }

    fn to_screen(&self, p: (f64, f64)) -> (i32, i32) {
        let mut pt = POINT {
            x: p.0 as i32,
            y: p.1 as i32,
        };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        (pt.x, pt.y)
    }

    fn monitor(&self) -> Option<(isize, (u32, u32))> {
        let m = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) };
        let r = monitor_rect(self.hwnd, false)?;
        Some((
            m.0 as isize,
            ((r.right - r.left) as u32, (r.bottom - r.top) as u32),
        ))
    }

    fn offer_clipboard(&self) -> bool {
        clipboard::announce(self.hwnd)
    }

    fn watch_dir(&self, dir: &Path) -> Option<Box<dyn std::any::Any>> {
        enumerate::DirWatcher::start(dir, self.hwnd, WM_DIR_CHANGED)
            .ok()
            .map(|w| Box::new(w) as Box<dyn std::any::Any>)
    }

    fn first_frame_shown(&self) {
        crate::update::on_first_frame();
        crate::gfx::text::prewarm();
    }
}

/// Monitor rectangle (or work area) of the monitor showing `hwnd`.
pub fn monitor_rect(hwnd: HWND, work: bool) -> Option<windows::Win32::Foundation::RECT> {
    unsafe {
        let m = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(m, &mut mi)
            .as_bool()
            .then_some(if work { mi.rcWork } else { mi.rcMonitor })
    }
}

// ---------------------------------------------------------------- services

/// What background threads use: files, WIC, window messages.
#[derive(Default)]
pub struct WinServices {
    hwnd: AtomicIsize,
    /// Where the GDI first frame goes (see first_frame).
    first_frame_target: Mutex<Option<first_frame::Target>>,
    color: Mutex<Option<Arc<ColorManager>>>,
}

impl WinServices {
    /// From now on wake-ups (and the GDI first frame) go to `hwnd`.
    pub fn set_hwnd(&self, hwnd: HWND) {
        self.hwnd.store(hwnd.0 as isize, Ordering::Relaxed);
    }

    /// Lets the loader render the GDI first frame itself (see first_frame).
    pub fn set_first_frame_target(&self, t: first_frame::Target) {
        *self.first_frame_target.lock().unwrap() = Some(t);
    }

    /// Colour management for the GDI first frame.
    pub fn set_color(&self, color: Arc<ColorManager>) {
        *self.color.lock().unwrap() = Some(color);
    }

    fn post(&self, msg: u32, lp: isize) -> bool {
        let hwnd = self.hwnd.load(Ordering::Relaxed);
        hwnd != 0
            && unsafe {
                PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(lp)).is_ok()
            }
    }
}

impl Services for WinServices {
    fn wake(&self, w: Wake) {
        match w {
            Wake::Loaded => {
                self.post(WM_LOADED, 0);
            }
            Wake::List => {
                self.post(WM_LIST, 0);
            }
            Wake::DirChanged => {
                self.post(WM_DIR_CHANGED, 0);
            }
            Wake::Thumbs => {
                self.post(WM_THUMB, 0);
            }
            Wake::ColorTarget { monitor, target } => {
                let msg = Box::into_raw(Box::new((monitor, target)));
                if !self.post(WM_COLOR_TARGET, msg as isize) {
                    drop(unsafe { Box::from_raw(msg) });
                }
            }
        }
    }

    fn read_file(&self, path: &Path) -> std::io::Result<PoolBuf> {
        io::read_file(path)
    }

    fn list_dir(&self, dir: &Path) -> std::io::Result<Vec<Entry>> {
        enumerate::list_dir(dir)
    }

    fn entry(&self, path: &Path) -> std::io::Result<Entry> {
        io::entry(path)
    }

    fn probe(&self, data: &[u8]) -> Result<Probe, DecodeError> {
        wic::probe_any(data)
    }

    fn decode(&self, data: &[u8], req: Request, threads: usize) -> Result<Image, DecodeError> {
        wic::decode_any_with(data, req, threads)
    }

    fn tune_thread(&self, latency: bool) {
        threads::tune_current_thread(if latency {
            Kind::Latency
        } else {
            Kind::Background
        });
    }

    fn log(&self, msg: &str) {
        debug_log(msg);
    }

    fn display_target(&self, monitor: isize) -> Option<Profile> {
        crate::color::display_target(HMONITOR(monitor as *mut _))
    }

    fn thumb_threads(&self) -> usize {
        threads::cpu_info().performance.max(2)
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        blinzel_ui::dirs::cache()
    }

    fn system_language(&self) -> Lang {
        crate::run::system_language()
    }

    fn wants_early_image(&self) -> bool {
        self.hwnd.load(Ordering::Relaxed) != 0
    }

    /// Renders the GDI first frame here, in parallel with the UI thread
    /// showing the window, when the layout does not depend on the image
    /// size, and hands it to the window.
    fn early_image(&self, img: &Arc<Image>, probe: &Probe) {
        let target = *self.first_frame_target.lock().unwrap();
        let color = self.color.lock().unwrap().clone();
        let lut = color.and_then(|c| c.lut(probe.icc.as_ref()));
        let rendered = target
            .filter(|t| t.view.mode != blinzel_core::layout::ViewMode::FitWindowToImage)
            .map(|t| {
                let bits = first_frame::render(
                    img,
                    probe.orientation,
                    t.client,
                    &t.view,
                    t.background,
                    lut.clone(),
                );
                (bits, t.client)
            });
        let ff = Box::new(first_frame::FirstFrame {
            image: img.clone(),
            orientation: probe.orientation,
            rendered,
            lut,
        });
        let ptr = Box::into_raw(ff);
        blinzel_core::timing::mark("first_frame_posted");
        if !self.post(first_frame::WM_FIRST_FRAME, ptr as isize) {
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}
