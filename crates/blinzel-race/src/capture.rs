//! Watches every monitor through the Desktop Duplication API (viewers open
//! wherever they last were). Every presented frame carries
//! `LastPresentTime` (QPC, comparable across monitors); a grid of samples
//! is copied out and classified by colour signature. Measuring from
//! outside, from what actually reached the screen, treats every viewer the
//! same.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIFactory1,
    IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use crate::signature;

/// Sample grid over the monitor.
const COLS: u32 = 40;
const ROWS: u32 = 24;

/// One presented frame: when it reached the screen and what it showed.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub qpc: i64,
    pub sig: Option<usize>,
}

#[derive(Default)]
struct Shared {
    events: Vec<Event>,
}

pub struct Capture {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// The primary monitor (desktop coordinates).
    pub monitor: (i32, i32, i32, i32),
    /// The GPU driving the primary monitor.
    pub adapter: String,
    pub monitors: usize,
}

impl Capture {
    /// One duplication thread per monitor of every adapter.
    pub fn start() -> Result<Capture, String> {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut primary = None;
        let mut errors = Vec::new();
        for (a, o) in outputs() {
            let (tx, rx) = std::sync::mpsc::channel();
            let (s2, stop2) = (shared.clone(), stop.clone());
            threads.push(std::thread::spawn(move || {
                let dup = match Dup::new(a, o) {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                let _ = tx.send(Ok((dup.rect, dup.adapter.clone())));
                dup.run(&s2, &stop2);
            }));
            match rx.recv().map_err(|e| e.to_string()) {
                Ok(Ok((rect, name))) => {
                    if rect.0 == 0 && rect.1 == 0 || primary.is_none() {
                        primary = Some((rect, name));
                    }
                }
                Ok(Err(e)) | Err(e) => errors.push(e),
            }
        }
        let monitors = threads.len() - errors.len();
        let (monitor, adapter) = primary.ok_or_else(|| errors.join("; "))?;
        Ok(Capture {
            shared,
            stop,
            threads,
            monitor,
            adapter,
            monitors,
        })
    }

    /// Waits until a frame presented at or after `since` shows signature
    /// `want`; returns its present time.
    pub fn wait_for(&self, since: i64, want: usize, timeout: Duration) -> Result<i64, String> {
        let deadline = Instant::now() + timeout;
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap();
        loop {
            if let Some(ev) = s
                .events
                .iter()
                .find(|e| e.qpc >= since && e.sig == Some(want))
            {
                return Ok(ev.qpc);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("signature {want} not seen"));
            }
            s = cv.wait_timeout(s, left).unwrap().0;
        }
    }

    /// Frames presented at or after `since`.
    pub fn events_since(&self, since: i64) -> Vec<Event> {
        let s = self.shared.0.lock().unwrap();
        s.events
            .iter()
            .filter(|e| e.qpc >= since)
            .copied()
            .collect()
    }

    /// Forgets old frames (between measurements).
    pub fn clear(&self) {
        self.shared.0.lock().unwrap().events.clear();
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// (adapter, output) index pairs of all attached monitors.
fn outputs() -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return out;
        };
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while adapter.EnumOutputs(o).is_ok() {
                out.push((a, o));
                o += 1;
            }
            a += 1;
        }
    }
    out
}

struct Dup {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    rect: (i32, i32, i32, i32),
    adapter: String,
}

impl Dup {
    fn new(adapter_index: u32, output_index: u32) -> Result<Dup, String> {
        let e =
            |what: &'static str| move |e: windows::core::Error| format!("{what}: {}", e.message());
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(e("CreateDXGIFactory1"))?;
            let adapter = factory
                .EnumAdapters1(adapter_index)
                .map_err(e("EnumAdapters1"))?;
            let name = adapter
                .GetDesc1()
                .map(|d| crate::sys::from_wide(&d.Description))
                .unwrap_or_default();
            let output = adapter
                .EnumOutputs(output_index)
                .map_err(e("EnumOutputs"))?;
            let desc = output.GetDesc().map_err(e("GetDesc"))?;
            let r = desc.DesktopCoordinates;
            let output1: IDXGIOutput1 = output.cast().map_err(e("IDXGIOutput1"))?;
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .map_err(e("D3D11CreateDevice"))?;
            let (device, ctx) = (device.unwrap(), ctx.unwrap());
            let dup = output1
                .DuplicateOutput(&device)
                .map_err(e("DuplicateOutput"))?;
            let td = D3D11_TEXTURE2D_DESC {
                Width: COLS,
                Height: ROWS,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            device
                .CreateTexture2D(&td, None, Some(&mut staging))
                .map_err(e("CreateTexture2D"))?;
            Ok(Dup {
                device,
                ctx,
                dup,
                staging: staging.unwrap(),
                rect: (r.left, r.top, r.right, r.bottom),
                adapter: name,
            })
        }
    }

    fn run(&self, shared: &(Mutex<Shared>, Condvar), stop: &AtomicBool) {
        let _ = &self.device;
        while !stop.load(Ordering::Relaxed) {
            match self.frame() {
                Ok(Some(ev)) => {
                    let (lock, cv) = shared;
                    lock.lock().unwrap().events.push(ev);
                    cv.notify_all();
                }
                Ok(None) => {}
                Err(e) => {
                    // Mode switch, secure desktop, ...: this monitor stops
                    // reporting; the others go on.
                    eprintln!("capture {}: {e}", self.adapter);
                    let _ = shared;
                    return;
                }
            }
        }
    }

    /// The next presented frame, classified; `None` on timeout or when only
    /// the mouse pointer moved.
    fn frame(&self) -> Result<Option<Event>, String> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        unsafe {
            match self.dup.AcquireNextFrame(50, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) => return Err(format!("AcquireNextFrame: {}", e.message())),
            }
            let out = if info.LastPresentTime != 0 {
                let tex: ID3D11Texture2D = res.as_ref().unwrap().cast().map_err(|e| e.message())?;
                Some(Event {
                    qpc: info.LastPresentTime,
                    sig: self.classify(&tex)?,
                })
            } else {
                None
            };
            let _ = self.dup.ReleaseFrame();
            Ok(out)
        }
    }

    fn classify(&self, tex: &ID3D11Texture2D) -> Result<Option<usize>, String> {
        // The frame's own size: physical pixels whatever the DPI setting.
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut d) };
        let (w, h) = (d.Width, d.Height);
        unsafe {
            for gy in 0..ROWS {
                for gx in 0..COLS {
                    let x = (gx * 2 + 1) * w / (COLS * 2);
                    let y = (gy * 2 + 1) * h / (ROWS * 2);
                    let src = D3D11_BOX {
                        left: x,
                        top: y,
                        front: 0,
                        right: x + 1,
                        bottom: y + 1,
                        back: 1,
                    };
                    self.ctx
                        .CopySubresourceRegion(&self.staging, 0, gx, gy, 0, tex, 0, Some(&src));
                }
            }
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .map_err(|e| e.message())?;
            let mut samples = Vec::with_capacity((COLS * ROWS) as usize);
            for gy in 0..ROWS as usize {
                let row = std::slice::from_raw_parts(
                    m.pData.cast::<u8>().add(gy * m.RowPitch as usize),
                    COLS as usize * 4,
                );
                for px in row.as_chunks::<4>().0 {
                    samples.push([px[2], px[1], px[0]]);
                }
            }
            self.ctx.Unmap(&self.staging, 0);
            Ok(signature::classify(&samples))
        }
    }
}
