//! Thumbnails for the grid and the filmstrip: a background pool that
//! takes them from the pack cache or makes them (and stores them), in the
//! order the UI asks for; results go back to the window as [`WM_THUMB`].
//!
//! Workers run at normal priority: the image the user waits for decodes
//! on above-normal threads and wins.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use blinzel_core::thumb::{self, EDGE, Planes};
use blinzel_core::thumbcache::{Key, ThumbCache};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::platform::{debug_log, io};
use crate::wic;

/// A thumbnail (or a failure) is ready in [`Thumbs::results`].
pub const WM_THUMB: u32 = WM_APP + 9;

/// Cache size that starts a new generation.
const CACHE_LIMIT: u64 = 1 << 30;

/// One thumbnail to get: the list entry's cache id and file identity.
#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: u64,
}

struct Shared {
    queue: Mutex<VecDeque<Request>>,
    wake: Condvar,
    /// Ids handed to a worker (done or in progress) since the last reset.
    taken: Mutex<HashSet<u64>>,
    cache: OnceLock<Option<ThumbCache>>,
    results: Mutex<Sender<(u64, Option<Planes>)>>,
    hwnd: AtomicIsize,
    /// What transparent thumbnails are composited over.
    background: [u8; 3],
}

pub struct Thumbs {
    shared: Arc<Shared>,
    pub results: Receiver<(u64, Option<Planes>)>,
}

impl Thumbs {
    pub fn new(hwnd: HWND, threads: usize, background: [u8; 3]) -> Thumbs {
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            taken: Mutex::new(HashSet::new()),
            cache: OnceLock::new(),
            results: Mutex::new(tx),
            hwnd: AtomicIsize::new(hwnd.0 as isize),
            background,
        });
        for i in 0..threads.max(1) {
            let s = shared.clone();
            let _ = std::thread::Builder::new()
                .name(format!("blinzel-thumb-{i}"))
                .spawn(move || worker(&s));
        }
        Thumbs {
            shared,
            results: rx,
        }
    }

    /// Replaces the queue: `wanted` in priority order. Ids already taken
    /// are skipped until [`Thumbs::forget`].
    pub fn set_wanted(&self, wanted: Vec<Request>) {
        let taken = self.shared.taken.lock().unwrap();
        let fresh: VecDeque<Request> = wanted
            .into_iter()
            .filter(|r| !taken.contains(&r.id))
            .collect();
        drop(taken);
        *self.shared.queue.lock().unwrap() = fresh;
        self.shared.wake.notify_all();
    }

    /// Forgets the queue and what was handed out (the atlas was emptied).
    pub fn reset(&self) {
        self.shared.queue.lock().unwrap().clear();
        self.shared.taken.lock().unwrap().clear();
    }

    /// Allows `id` to be requested again (its atlas slot was reused).
    pub fn forget(&self, id: u64) {
        self.shared.taken.lock().unwrap().remove(&id);
    }
}

fn cache_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join("amigo-blinzel").join("thumbs"))
}

fn worker(s: &Shared) {
    loop {
        let req = {
            let mut q = s.queue.lock().unwrap();
            loop {
                if let Some(r) = q.pop_front() {
                    if s.taken.lock().unwrap().insert(r.id) {
                        break r;
                    }
                    continue;
                }
                q = s.wake.wait(q).unwrap();
            }
        };
        let thumb = get_or_make(s, &req);
        let _ = s.results.lock().unwrap().send((req.id, thumb));
        let hwnd = s.hwnd.load(Ordering::Relaxed);
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_THUMB, WPARAM(0), LPARAM(0));
        }
    }
}

fn get_or_make(s: &Shared, r: &Request) -> Option<Planes> {
    let cache = s
        .cache
        .get_or_init(|| {
            let dir = cache_dir()?;
            ThumbCache::open(&dir, CACHE_LIMIT)
                .map_err(|e| debug_log(&format!("thumbnail cache: {e}")))
                .ok()
        })
        .as_ref();
    let key = Key::new(&r.path, r.size, r.mtime);
    if let Some(p) = cache.and_then(|c| c.get_planes(&key, s.background)) {
        return Some(p);
    }
    let data = io::read_file(&r.path).ok()?;
    let probe = wic::probe_any(&data).ok()?;
    let t = thumb::make(&data, &probe, EDGE, &wic::decode_any).ok()?;
    if let Some(c) = cache {
        let _ = c.put(&key, &t);
    }
    Some(Planes::from_thumb(&t, s.background))
}
