//! The decode pipeline: scheduler jobs that read a file once, then deliver
//! the embedded preview (if useful) and the wanted resolution as GPU
//! textures. Workers upload directly (every backend's device is
//! thread-safe); the UI thread only receives finished textures.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::decode::{Probe, Request};
use blinzel_core::layout::needed_decode_size;
use blinzel_core::sched::{CancelToken, Config, JobSpec, Key, Priority, Role, Scheduler};

use crate::backend::{Device, Services, Wake};
use crate::color::ColorManager;

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
pub struct GpuAnimation<I> {
    pub frames: Vec<(Arc<I>, u32)>,
    /// How often the whole animation plays; `None` for ever.
    pub plays: Option<u32>,
    /// All frames are here; otherwise more follow in a later `Loaded`.
    pub complete: bool,
}

/// How often an animation still being decoded goes to the window with the
/// frames so far.
const ANIM_UPDATE: std::time::Duration = std::time::Duration::from_millis(200);

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
pub struct Loaded<I> {
    pub cache_id: u64,
    pub stage: Stage,
    pub probe: Option<Probe>,
    pub image: Result<Arc<I>, String>,
    /// Stage `Anim`: the frames, or `None` for a still image.
    pub animation: Option<Arc<GpuAnimation<I>>>,
    pub read_ms: f64,
    pub decode_ms: f64,
    pub upload_ms: f64,
}

struct IoOut {
    data: PoolBuf,
    read_ms: f64,
}

struct DeviceSlot<D> {
    device: Mutex<Option<Arc<D>>>,
    ready: Condvar,
}

struct Shared<D: Device, S> {
    services: Arc<S>,
    jobs: Mutex<HashMap<Key, Job>>,
    /// Box (physical pixels) the Screen stage must cover.
    screen: Mutex<(u32, u32)>,
    device: DeviceSlot<D>,
    results: Mutex<Sender<Loaded<D::Image>>>,
    /// Threads for decoding the image the user waits for (priority 0).
    decode_threads: usize,
    /// The first decoded image went to the platform's early display.
    first_frame_sent: AtomicBool,
    color: Mutex<Option<Arc<ColorManager>>>,
}

impl<D: Device, S: Services> Shared<D, S> {
    fn wait_device(&self, cancel: &CancelToken) -> Option<Arc<D>> {
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

    fn send(&self, l: Loaded<D::Image>) {
        let _ = self.results.lock().unwrap().send(l);
        self.services.wake(Wake::Loaded);
    }
}

pub struct Loader<D: Device, S> {
    sched: Scheduler<IoOut>,
    shared: Arc<Shared<D, S>>,
    pub results: Receiver<Loaded<D::Image>>,
}

/// Job classes.
pub const CLASS_VIEW: u8 = 0;

impl<D: Device, S: Services> Loader<D, S> {
    /// `decode_threads`: how many threads may decode the visible image;
    /// prefetch decodes stay on their worker.
    pub fn new(
        services: Arc<S>,
        io_threads: usize,
        cpu_threads: usize,
        decode_threads: usize,
    ) -> Loader<D, S> {
        // blinzel-jpeg's helper threads run like the workers. It takes a
        // plain function, so the platform's tuning goes through a global.
        let tune = services.clone();
        let _ = TUNE.set(Box::new(move || tune.tune_thread(true)));
        blinzel_jpeg::set_thread_init(|| {
            if let Some(t) = TUNE.get() {
                t()
            }
        });
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            services,
            jobs: Mutex::new(HashMap::new()),
            screen: Mutex::new((1920, 1080)),
            device: DeviceSlot {
                device: Mutex::new(None),
                ready: Condvar::new(),
            },
            results: Mutex::new(tx),
            decode_threads: decode_threads.max(1),
            first_frame_sent: AtomicBool::new(false),
            color: Mutex::new(None),
        });
        let (s_io, s_cpu, s_tune) = (shared.clone(), shared.clone(), shared.clone());
        let sched = Scheduler::new(
            Config {
                io_threads,
                cpu_threads,
                // Both pools sit on the path to the image the user waits
                // for (thumbnails have their own background pool).
                on_thread_start: Some(Box::new(move |_: Role, _| {
                    s_tune.services.tune_thread(true)
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

    /// Colour management: transforms build while images decode.
    pub fn set_color(&self, color: Arc<ColorManager>) {
        *self.shared.color.lock().unwrap() = Some(color);
    }

    pub fn set_device(&self, device: Arc<D>) {
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

/// How blinzel-jpeg's helper threads are tuned.
static TUNE: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

fn io_stage<D: Device, S: Services>(
    shared: &Shared<D, S>,
    key: Key,
    _cancel: &CancelToken,
) -> Option<IoOut> {
    let path = shared.jobs.lock().unwrap().get(&key)?.path.clone();
    let t = Instant::now();
    match shared.services.read_file(&path) {
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

fn cpu_stage<D: Device, S: Services>(
    shared: &Shared<D, S>,
    key: Key,
    input: IoOut,
    cancel: &CancelToken,
) {
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
    let probe = match shared.services.probe(data) {
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

    let deliver =
        |stage: Stage, img: blinzel_core::pixel::Image, decode_ms: f64, probe: &Probe| -> bool {
            if cancel.is_cancelled() {
                return false;
            }
            let img = Arc::new(img);
            // The device is still starting: the platform may show this
            // image another way meanwhile (Windows: through GDI).
            if job.priority == 0
                && shared.services.wants_early_image()
                && shared.device.device.lock().unwrap().is_none()
                && !shared.first_frame_sent.swap(true, Ordering::Relaxed)
            {
                shared.services.early_image(&img, probe);
            }
            let Some(device) = shared.wait_device(cancel) else {
                return false;
            };
            let t = Instant::now();
            let gpu = device.upload(&img, probe.orientation);
            let upload_ms = t.elapsed().as_secs_f64() * 1e3;
            drop(img); // back to the buffer pool before signalling
            shared.send(Loaded {
                cache_id: job.cache_id,
                stage,
                probe: Some(probe.clone()),
                image: gpu.map(Arc::new),
                animation: None,
                read_ms: input.read_ms,
                decode_ms,
                upload_ms,
            });
            true
        };

    // 0. Raws: WIC develops the sensor data in seconds, so an embedded
    //    preview that covers what the stage needs (the screen box, for Full
    //    the whole image, 1 % tolerance) is the stage.
    let need = match job.stage {
        Stage::Full => (probe.width, probe.height),
        _ => fit,
    };
    if let Some(p) = covering_raw_preview(&probe, need) {
        let t = Instant::now();
        let req = match job.stage {
            Stage::Full => Request::Full,
            _ => Request::Fit {
                w: fit.0.min(p.width),
                h: fit.1.min(p.height),
            },
        };
        match blinzel_core::decode::decode_preview(data, p, (probe.width, probe.height), req) {
            Ok(img) => {
                deliver(job.stage, img, t.elapsed().as_secs_f64() * 1e3, &probe);
                return;
            }
            Err(e) => shared.services.log(&format!("raw preview: {e}")),
        }
    }

    // 1. Embedded preview: only worth it if it covers half the screen box
    //    and is much smaller than what the main stage decodes (or that is
    //    a raw).
    if job.with_preview
        && let Some(p) = probe.previews.first()
        && (p.width >= fit.0 / 2 || p.height >= fit.1 / 2)
        && (probe.is_raw()
            || (p.width as u64 * p.height as u64) * 3 < probe.width as u64 * probe.height as u64)
    {
        let t = Instant::now();
        let req = Request::Fit {
            w: fit.0.min(p.width),
            h: fit.1.min(p.height),
        };
        match blinzel_core::decode::decode_preview(data, p, (probe.width, probe.height), req) {
            Ok(img) => {
                if !deliver(Stage::Preview, img, t.elapsed().as_secs_f64() * 1e3, &probe) {
                    return;
                }
            }
            Err(e) => shared.services.log(&format!("preview: {e}")),
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
    match shared.services.decode(data, req, decode_threads) {
        Ok(img) => {
            // A raw's probe estimates the developed size; WIC knows it.
            let probe = if probe.is_raw() {
                Probe {
                    width: img.full_width,
                    height: img.full_height,
                    ..probe.clone()
                }
            } else {
                probe.clone()
            };
            deliver(job.stage, img, t.elapsed().as_secs_f64() * 1e3, &probe);
        }
        Err(e) => fail(e.to_string(), Some(probe.clone())),
    }
}

/// A raw's largest embedded preview, if it covers `need` (stored
/// orientation, 1 % tolerance): WIC develops the sensor data in seconds,
/// so such a preview stands in for the image.
pub fn covering_raw_preview(
    probe: &Probe,
    need: (u32, u32),
) -> Option<&blinzel_core::exif::EmbeddedPreview> {
    let p = probe.previews.first().filter(|_| probe.is_raw())?;
    (p.width as u64 * 100 >= need.0 as u64 * 99 && p.height as u64 * 100 >= need.1 as u64 * 99)
        .then_some(p)
}

/// Decodes the frames of an animated image and uploads each as soon as it
/// is ready. From the second frame on the window gets the frames so far
/// (every [`ANIM_UPDATE`]), so playback starts while the rest decodes.
fn animation_stage<D: Device, S: Services>(
    shared: &Shared<D, S>,
    job: &Job,
    data: &[u8],
    probe: Probe,
    read_ms: f64,
    cancel: &CancelToken,
) {
    let t = Instant::now();
    use blinzel_core::decode::{AnimFrame, FrameSink, gif, jxl, webp};
    use blinzel_core::format::Format;
    let stream: fn(&[u8], usize, &mut FrameSink) -> blinzel_core::decode::Result<_> =
        match probe.format {
            Format::Gif => gif::stream_animation,
            Format::WebP => webp::stream_animation,
            Format::Jxl => jxl::stream_animation,
            _ => return,
        };
    let send = |frames: &[(Arc<D::Image>, u32)], plays, complete, upload_ms| {
        shared.send(Loaded {
            cache_id: job.cache_id,
            stage: Stage::Anim,
            probe: Some(probe.clone()),
            image: Err("animation stage".into()),
            animation: (frames.len() >= 2).then(|| {
                Arc::new(GpuAnimation {
                    frames: frames.to_vec(),
                    plays,
                    complete,
                })
            }),
            read_ms,
            decode_ms: t.elapsed().as_secs_f64() * 1e3,
            upload_ms,
        })
    };
    let mut frames: Vec<(Arc<D::Image>, u32)> = Vec::new();
    let mut device = None;
    let mut upload_ms = 0.0;
    let mut sent = None::<Instant>;
    let info = stream(data, ANIM_BUDGET, &mut |f: AnimFrame| {
        if cancel.is_cancelled() {
            return false;
        }
        if device.is_none() {
            device = shared.wait_device(cancel);
        }
        let Some(d) = &device else { return false };
        let tu = Instant::now();
        // Out of GPU memory: play the frames uploaded so far.
        let Ok(gpu) = d.upload(&f.image, probe.orientation) else {
            return false;
        };
        upload_ms += tu.elapsed().as_secs_f64() * 1e3;
        frames.push((Arc::new(gpu), f.delay_ms));
        if frames.len() >= 2 && sent.is_none_or(|s| s.elapsed() >= ANIM_UPDATE) {
            send(&frames, None, false, upload_ms);
            sent = Some(Instant::now());
        }
        true
    });
    if cancel.is_cancelled() {
        return;
    }
    // A decode error ends the animation with the frames decoded so far.
    let plays = info.ok().and_then(|i| i.plays);
    send(&frames, plays, true, upload_ms);
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
