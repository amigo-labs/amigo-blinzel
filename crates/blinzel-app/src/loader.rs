//! The decode pipeline: scheduler jobs that read a file once, then deliver
//! the embedded preview (if useful) and the wanted resolution as GPU
//! textures. Workers upload directly (the D3D11 device is free-threaded);
//! the UI thread only receives finished textures.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::decode::{Probe, Request};
use blinzel_core::layout::needed_decode_size;
use blinzel_core::sched::{CancelToken, Config, JobSpec, Key, Priority, Role, Scheduler};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::gfx::texture::{DEFAULT_TILE, GpuImage, upload};
use crate::platform::threads::{self, Kind};
use crate::platform::{debug_log, io};
use crate::wic;

/// Posted to the window when results are waiting in the channel.
pub const WM_LOADED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// Resolution levels of an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    /// JPEG preview embedded by the camera.
    Preview = 0,
    /// Fitted to the screen (DCT-scaled for JPEG).
    Screen = 1,
    Full = 2,
    /// Every frame of an animation (the current image only).
    Anim = 3,
}

/// Bytes of decoded frames an animation may use; longer ones play the
/// frames that fit.
const ANIM_BUDGET: usize = 256 << 20;

/// An animation's frames on the GPU and how long each stays.
pub struct GpuAnimation {
    pub frames: Vec<(Arc<GpuImage>, u32)>,
    /// How often the whole animation plays; `None` for ever.
    pub plays: Option<u32>,
}

/// Texture/job key: the file's cache id with the stage in the low bits.
pub fn key(cache_id: u64, stage: Stage) -> Key {
    (cache_id & !3) | stage as u64
}

pub fn stage_of(key: Key) -> Stage {
    match key & 3 {
        0 => Stage::Preview,
        1 => Stage::Screen,
        2 => Stage::Full,
        _ => Stage::Anim,
    }
}

/// What a job should produce.
#[derive(Clone, Debug)]
pub struct Job {
    pub path: PathBuf,
    pub cache_id: u64,
    pub stage: Stage,
    /// Also deliver the embedded camera preview first, if useful.
    pub with_preview: bool,
    pub priority: Priority,
}

/// A finished stage (or the reason it failed).
pub struct Loaded {
    pub cache_id: u64,
    pub stage: Stage,
    pub probe: Option<Probe>,
    pub image: Result<Arc<GpuImage>, String>,
    /// Stage `Anim`: the frames, or `None` for a still image.
    pub animation: Option<Arc<GpuAnimation>>,
    pub read_ms: f64,
    pub decode_ms: f64,
    pub upload_ms: f64,
}

struct IoOut {
    data: PoolBuf,
    read_ms: f64,
}

#[derive(Default)]
struct DeviceSlot {
    device: Mutex<Option<ID3D11Device>>,
    ready: Condvar,
}

struct Shared {
    jobs: Mutex<HashMap<Key, Job>>,
    /// Box (physical pixels) the Screen stage must cover.
    screen: Mutex<(u32, u32)>,
    device: DeviceSlot,
    tile: Mutex<u32>,
    results: Mutex<Sender<Loaded>>,
    hwnd: AtomicIsize,
    /// Threads for decoding the image the user waits for (priority 0).
    decode_threads: usize,
    /// The first decoded image went to the window for the GDI first frame.
    first_frame_sent: AtomicBool,
    first_frame_target: Mutex<Option<crate::first_frame::Target>>,
    color: Mutex<Option<Arc<crate::color::ColorManager>>>,
}

impl Shared {
    fn wait_device(&self, cancel: &CancelToken) -> Option<ID3D11Device> {
        let mut d = self.device.device.lock().unwrap();
        while d.is_none() {
            if cancel.is_cancelled() {
                return None;
            }
            d = self
                .device
                .ready
                .wait_timeout(d, std::time::Duration::from_millis(50))
                .unwrap()
                .0;
        }
        d.clone()
    }

    fn send(&self, l: Loaded) {
        let _ = self.results.lock().unwrap().send(l);
        let hwnd = self.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            let _ = unsafe {
                PostMessageW(Some(HWND(hwnd as *mut _)), WM_LOADED, WPARAM(0), LPARAM(0))
            };
        }
    }
}

pub struct Loader {
    sched: Scheduler<IoOut>,
    shared: Arc<Shared>,
    pub results: Receiver<Loaded>,
}

/// Job classes.
pub const CLASS_VIEW: u8 = 0;

impl Loader {
    /// `decode_threads`: how many threads may decode the visible image;
    /// prefetch decodes stay on their worker.
    pub fn new(io_threads: usize, cpu_threads: usize, decode_threads: usize) -> Loader {
        // blinzel-jpeg's helper threads run like the workers.
        blinzel_jpeg::set_thread_init(|| threads::tune_current_thread(Kind::Latency));
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            jobs: Mutex::new(HashMap::new()),
            screen: Mutex::new((1920, 1080)),
            device: DeviceSlot::default(),
            tile: Mutex::new(DEFAULT_TILE),
            results: Mutex::new(tx),
            hwnd: AtomicIsize::new(0),
            decode_threads: decode_threads.max(1),
            first_frame_sent: AtomicBool::new(false),
            first_frame_target: Mutex::new(None),
            color: Mutex::new(None),
        });
        let (s_io, s_cpu) = (shared.clone(), shared.clone());
        let sched = Scheduler::new(
            Config {
                io_threads,
                cpu_threads,
                // Both pools sit on the path to the image the user waits
                // for (thumbnails will get their own background pool).
                on_thread_start: Some(Box::new(|_: Role, _| {
                    threads::tune_current_thread(Kind::Latency)
                })),
            },
            move |key, cancel| io_stage(&s_io, key, cancel),
            move |key, input, cancel| cpu_stage(&s_cpu, key, input, cancel),
        );
        Loader {
            sched,
            shared,
            results: rx,
        }
    }

    /// Lets the loader render the GDI first frame itself (see first_frame).
    pub fn set_first_frame_target(&self, t: crate::first_frame::Target) {
        *self.shared.first_frame_target.lock().unwrap() = Some(t);
    }

    /// Colour management for the GDI first frame.
    pub fn set_color(&self, color: Arc<crate::color::ColorManager>) {
        *self.shared.color.lock().unwrap() = Some(color);
    }

    pub fn set_hwnd(&self, hwnd: HWND) {
        self.shared.hwnd.store(hwnd.0 as isize, Ordering::Relaxed);
    }

    pub fn set_device(&self, device: ID3D11Device, max_texture: u32) {
        *self.shared.tile.lock().unwrap() = DEFAULT_TILE.min(max_texture);
        *self.shared.device.device.lock().unwrap() = Some(device);
        self.shared.device.ready.notify_all();
    }

    pub fn set_screen_box(&self, size: (u32, u32)) {
        *self.shared.screen.lock().unwrap() = (size.0.max(64), size.1.max(64));
    }

    /// Replaces the wanted jobs (current image first, then neighbours).
    pub fn set_wanted(&self, jobs: Vec<Job>) {
        let specs: Vec<JobSpec> = jobs
            .iter()
            .map(|j| JobSpec {
                key: key(j.cache_id, j.stage),
                priority: j.priority,
                class: CLASS_VIEW,
            })
            .collect();
        {
            let mut map = self.shared.jobs.lock().unwrap();
            for j in jobs {
                map.insert(key(j.cache_id, j.stage), j);
            }
            // Keep descriptions for jobs that might still be running.
            if map.len() > 512 {
                let keep: std::collections::HashSet<Key> = specs.iter().map(|s| s.key).collect();
                map.retain(|k, _| keep.contains(k));
            }
        }
        self.sched.set_wanted(CLASS_VIEW, &specs);
    }

    /// Allows `keys` to be loaded again (their textures were evicted).
    pub fn forget(&self, keys: impl IntoIterator<Item = Key>) {
        self.sched.forget(keys);
    }

    pub fn is_done(&self, key: Key) -> bool {
        self.sched.is_completed(key)
    }

    pub fn pending(&self) -> usize {
        self.sched.pending()
    }
}

fn io_stage(shared: &Shared, key: Key, _cancel: &CancelToken) -> Option<IoOut> {
    let path = shared.jobs.lock().unwrap().get(&key)?.path.clone();
    let t = Instant::now();
    match io::read_file(&path) {
        Ok(data) => Some(IoOut {
            data,
            read_ms: t.elapsed().as_secs_f64() * 1e3,
        }),
        Err(e) => {
            let job = shared.jobs.lock().unwrap().get(&key).cloned()?;
            shared.send(Loaded {
                cache_id: job.cache_id,
                stage: job.stage,
                probe: None,
                image: Err(e.to_string()),
                animation: None,
                read_ms: 0.0,
                decode_ms: 0.0,
                upload_ms: 0.0,
            });
            // Reported; nothing for the CPU stage to do.
            None
        }
    }
}

fn cpu_stage(shared: &Shared, key: Key, input: IoOut, cancel: &CancelToken) {
    let Some(job) = shared.jobs.lock().unwrap().get(&key).cloned() else {
        return;
    };
    let data = &input.data[..];
    let fail = |msg: String, probe: Option<Probe>| {
        shared.send(Loaded {
            cache_id: job.cache_id,
            stage: job.stage,
            probe,
            image: Err(msg),
            animation: None,
            read_ms: input.read_ms,
            decode_ms: 0.0,
            upload_ms: 0.0,
        })
    };
    let probe = match wic::probe_any(data) {
        Ok(p) => p,
        Err(e) => return fail(e.to_string(), None),
    };
    let (sw, sh) = *shared.screen.lock().unwrap();
    // A colour transform, if the image needs one, builds while it decodes.
    if let Some(c) = &*shared.color.lock().unwrap() {
        c.prepare(probe.icc.as_ref());
    }
    if job.stage == Stage::Anim {
        return animation_stage(shared, &job, data, probe, input.read_ms, cancel);
    }
    let (ow, oh) = probe.oriented_size();
    let fit_scale = (sw as f64 / ow as f64).min(sh as f64 / oh as f64);
    // Stored-orientation size that covers the screen box.
    let fit = needed_decode_size((probe.width, probe.height), fit_scale);
    let tile = *shared.tile.lock().unwrap();

    let deliver = |stage: Stage, img: blinzel_core::pixel::Image, decode_ms: f64| -> bool {
        if cancel.is_cancelled() {
            return false;
        }
        let img = Arc::new(img);
        // The device is still starting: the window shows this image through
        // GDI meanwhile (see first_frame).
        let hwnd = shared.hwnd.load(Ordering::Relaxed);
        if job.priority == 0
            && hwnd != 0
            && shared.device.device.lock().unwrap().is_none()
            && !shared.first_frame_sent.swap(true, Ordering::Relaxed)
        {
            // Rendered here, in parallel with the UI thread showing the
            // window, when the layout does not depend on the image size.
            let target = *shared.first_frame_target.lock().unwrap();
            let color = shared.color.lock().unwrap().clone();
            let lut = color.and_then(|c| c.lut(probe.icc.as_ref()));
            let rendered = target
                .filter(|t| t.view.mode != blinzel_core::layout::ViewMode::FitWindowToImage)
                .map(|t| {
                    let bits = crate::first_frame::render(
                        &img,
                        probe.orientation,
                        t.client,
                        &t.view,
                        t.background,
                        lut.clone(),
                    );
                    (bits, t.client)
                });
            let ff = Box::new(crate::first_frame::FirstFrame {
                image: img.clone(),
                orientation: probe.orientation,
                rendered,
                lut,
            });
            let ptr = Box::into_raw(ff);
            blinzel_core::timing::mark("first_frame_posted");
            unsafe {
                if PostMessageW(
                    Some(HWND(hwnd as *mut _)),
                    crate::first_frame::WM_FIRST_FRAME,
                    WPARAM(0),
                    LPARAM(ptr as isize),
                )
                .is_err()
                {
                    drop(Box::from_raw(ptr));
                }
            }
        }
        let Some(device) = shared.wait_device(cancel) else {
            return false;
        };
        let t = Instant::now();
        let gpu = upload(&device, &img, probe.orientation, tile);
        let upload_ms = t.elapsed().as_secs_f64() * 1e3;
        drop(img); // back to the buffer pool before signalling
        shared.send(Loaded {
            cache_id: job.cache_id,
            stage,
            probe: Some(probe.clone()),
            image: gpu.map(Arc::new).map_err(|e| e.message()),
            animation: None,
            read_ms: input.read_ms,
            decode_ms,
            upload_ms,
        });
        true
    };

    // 1. Embedded preview: only worth it if it covers half the screen box
    //    and is much smaller than what the main stage decodes.
    if job.with_preview
        && let Some(p) = probe.previews.first()
        && (p.width >= fit.0 / 2 || p.height >= fit.1 / 2)
        && (p.width as u64 * p.height as u64) * 3 < probe.width as u64 * probe.height as u64
    {
        let t = Instant::now();
        let req = Request::Fit {
            w: fit.0.min(p.width),
            h: fit.1.min(p.height),
        };
        match blinzel_core::decode::decode_preview(data, p, (probe.width, probe.height), req) {
            Ok(img) => {
                if !deliver(Stage::Preview, img, t.elapsed().as_secs_f64() * 1e3) {
                    return;
                }
            }
            Err(e) => debug_log(&format!("preview: {e}")),
        }
    }
    if cancel.is_cancelled() {
        return;
    }

    // 2. The requested stage.
    let req = match job.stage {
        Stage::Full => Request::Full,
        _ if fit.0 >= probe.width => Request::Full,
        _ => Request::Fit { w: fit.0, h: fit.1 },
    };
    let t = Instant::now();
    let decode_threads = if job.priority == 0 {
        shared.decode_threads
    } else {
        1
    };
    match wic::decode_any_with(data, req, decode_threads) {
        Ok(img) => {
            deliver(job.stage, img, t.elapsed().as_secs_f64() * 1e3);
        }
        Err(e) => fail(e.to_string(), Some(probe.clone())),
    }
}

/// Decodes every frame of an animated image and uploads them.
fn animation_stage(
    shared: &Shared,
    job: &Job,
    data: &[u8],
    probe: Probe,
    read_ms: f64,
    cancel: &CancelToken,
) {
    let t = Instant::now();
    let decoded = match probe.format {
        blinzel_core::format::Format::Gif => {
            blinzel_core::decode::gif::decode_animation(data, ANIM_BUDGET)
                .ok()
                .flatten()
        }
        _ => None,
    };
    let decode_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let animation = decoded.and_then(|a| {
        let device = shared.wait_device(cancel)?;
        let tile = *shared.tile.lock().unwrap();
        let frames = a
            .frames
            .into_iter()
            .map(|f| {
                let gpu = upload(&device, &f.image, probe.orientation, tile).ok()?;
                Some((Arc::new(gpu), f.delay_ms))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Arc::new(GpuAnimation {
            frames,
            plays: a.plays,
        }))
    });
    if cancel.is_cancelled() {
        return;
    }
    shared.send(Loaded {
        cache_id: job.cache_id,
        stage: Stage::Anim,
        probe: Some(probe),
        image: Err("animation stage".into()),
        animation,
        read_ms,
        decode_ms,
        upload_ms: t.elapsed().as_secs_f64() * 1e3,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_carry_stage() {
        let id = 0xDEAD_BEEF_1234_5677;
        for s in [Stage::Preview, Stage::Screen, Stage::Full, Stage::Anim] {
            assert_eq!(stage_of(key(id, s)), s);
            assert_eq!(key(id, s) & !3, id & !3);
        }
    }
}
