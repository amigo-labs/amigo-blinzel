//! Pool of large, already-committed pixel buffers.
//!
//! A fresh 100 MP buffer costs ~100k page faults the first time it is
//! written (tens of milliseconds); the system allocator hands such blocks back
//! to the OS on free, so every new image would pay that again. Buffers here
//! are recycled by size class instead and stay faulted in.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Below this, the system allocator is fast enough and pooling only wastes
/// memory.
const MIN_POOLED: usize = 256 * 1024;
const PAGE: usize = 4096;

/// A buffer of exactly `len` bytes backed by a pooled allocation.
pub struct PoolBuf {
    vec: Vec<u8>,
    len: usize,
    pooled: bool,
}

impl PoolBuf {
    /// A buffer of `len` bytes from the global pool. Contents are
    /// initialized but unspecified (zero or stale pixels).
    pub fn new(len: usize) -> PoolBuf {
        global().take(len)
    }

    /// Wraps an existing vector (not returned to the pool).
    pub fn from_vec(vec: Vec<u8>) -> PoolBuf {
        let len = vec.len();
        PoolBuf {
            vec,
            len,
            pooled: false,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Shortens the buffer (e.g. after a short read).
    pub fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }

    /// Bytes actually held (size class capacity).
    pub fn capacity(&self) -> usize {
        self.vec.len()
    }
}

impl Deref for PoolBuf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.vec[..self.len]
    }
}

impl DerefMut for PoolBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.vec[..self.len]
    }
}

impl Drop for PoolBuf {
    fn drop(&mut self) {
        if self.pooled {
            global().give_back(std::mem::take(&mut self.vec));
        }
    }
}

impl std::fmt::Debug for PoolBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PoolBuf({} of {} bytes)", self.len, self.vec.len())
    }
}

/// Size-class pool. Classes are 2^k, 1.25·2^k, 1.5·2^k and 1.75·2^k, so at
/// most ~25 % is wasted per buffer.
pub struct BufPool {
    free: Mutex<HashMap<usize, Vec<Vec<u8>>>>,
    retained: AtomicUsize,
    max_retained: AtomicUsize,
    stats_hits: AtomicUsize,
    stats_misses: AtomicUsize,
}

/// The process-wide pool.
pub fn global() -> &'static BufPool {
    static POOL: OnceLock<BufPool> = OnceLock::new();
    POOL.get_or_init(|| BufPool::new(512 << 20))
}

/// Rounds `len` up to its size class.
pub fn size_class(len: usize) -> usize {
    if len <= MIN_POOLED {
        return len;
    }
    let k = usize::BITS - 1 - len.leading_zeros();
    let base = 1usize << k;
    let quarter = base >> 2;
    let steps = (len - base).div_ceil(quarter);
    base + steps * quarter
}

impl BufPool {
    pub fn new(max_retained: usize) -> Self {
        BufPool {
            free: Mutex::new(HashMap::new()),
            retained: AtomicUsize::new(0),
            max_retained: AtomicUsize::new(max_retained),
            stats_hits: AtomicUsize::new(0),
            stats_misses: AtomicUsize::new(0),
        }
    }

    /// Upper bound of idle bytes kept for reuse.
    pub fn set_max_retained(&self, bytes: usize) {
        self.max_retained.store(bytes, Ordering::Relaxed);
        self.trim(bytes);
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained.load(Ordering::Relaxed)
    }

    /// (reused, freshly allocated) counts.
    pub fn stats(&self) -> (usize, usize) {
        (
            self.stats_hits.load(Ordering::Relaxed),
            self.stats_misses.load(Ordering::Relaxed),
        )
    }

    pub fn take(&self, len: usize) -> PoolBuf {
        if len <= MIN_POOLED {
            return PoolBuf {
                vec: vec![0; len],
                len,
                pooled: false,
            };
        }
        let class = size_class(len);
        let reused = {
            let mut free = self.free.lock().unwrap();
            // Exact class first, then one class up to improve reuse.
            let next = size_class(class + 1);
            [class, next]
                .into_iter()
                .find_map(|c| free.get_mut(&c).and_then(|v| v.pop()))
        };
        match reused {
            Some(vec) => {
                self.retained.fetch_sub(vec.len(), Ordering::Relaxed);
                self.stats_hits.fetch_add(1, Ordering::Relaxed);
                PoolBuf {
                    vec,
                    len,
                    pooled: true,
                }
            }
            None => {
                self.stats_misses.fetch_add(1, Ordering::Relaxed);
                PoolBuf {
                    vec: vec![0; class],
                    len,
                    pooled: true,
                }
            }
        }
    }

    fn give_back(&self, vec: Vec<u8>) {
        let size = vec.len();
        if size == 0 {
            return;
        }
        let max = self.max_retained.load(Ordering::Relaxed);
        if self.retained.load(Ordering::Relaxed) + size > max {
            return; // dropped: memory goes back to the OS
        }
        self.retained.fetch_add(size, Ordering::Relaxed);
        self.free.lock().unwrap().entry(size).or_default().push(vec);
    }

    /// Drops idle buffers until at most `bytes` are retained.
    pub fn trim(&self, bytes: usize) {
        let mut free = self.free.lock().unwrap();
        let mut classes: Vec<usize> = free.keys().copied().collect();
        classes.sort_unstable_by(|a, b| b.cmp(a));
        for c in classes {
            while self.retained.load(Ordering::Relaxed) > bytes {
                match free.get_mut(&c).and_then(|v| v.pop()) {
                    Some(v) => {
                        self.retained.fetch_sub(v.len(), Ordering::Relaxed);
                    }
                    None => break,
                }
            }
        }
        free.retain(|_, v| !v.is_empty());
    }

    /// Allocates and touches a buffer of `len` bytes ahead of time so the
    /// next image of that size starts without page faults. Call off the
    /// critical path (e.g. after the first frame is on screen).
    pub fn warm(&self, len: usize) {
        let mut b = self.take(len);
        for i in (0..b.len()).step_by(PAGE) {
            b[i] = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_classes() {
        assert_eq!(size_class(100), 100);
        assert_eq!(size_class(1 << 20), 1 << 20);
        assert_eq!(size_class((1 << 20) + 1), (1 << 20) + (1 << 18));
        assert_eq!(size_class(3 << 19), 3 << 19); // 1.5 MiB
        assert_eq!(size_class((7 << 18) + 1), 2 << 20);
        for len in [300_000usize, 1_000_000, 24_000_000, 96_000_001] {
            let c = size_class(len);
            assert!(c >= len && (c as f64) < len as f64 * 1.26, "{len} -> {c}");
        }
    }

    #[test]
    fn buffers_are_reused() {
        let pool = BufPool::new(64 << 20);
        let a = pool.take(5_000_000);
        let cap = a.capacity();
        let ptr = a.as_ptr();
        assert_eq!(a.len(), 5_000_000);
        pool.give_back(a.vec.clone());
        std::mem::forget(a);
        assert_eq!(pool.retained_bytes(), cap);
        let b = pool.take(4_900_000);
        assert_eq!(b.capacity(), cap);
        assert_eq!(pool.retained_bytes(), 0);
        assert_ne!(b.as_ptr(), ptr); // the clone came back, not the forgotten one
        assert_eq!(pool.stats(), (1, 1));
        std::mem::forget(b);
    }

    #[test]
    fn retention_limit_and_trim() {
        let pool = BufPool::new(3 << 20);
        pool.give_back(vec![0; 2 << 20]);
        pool.give_back(vec![0; 2 << 20]); // over the limit: dropped
        assert_eq!(pool.retained_bytes(), 2 << 20);
        pool.trim(0);
        assert_eq!(pool.retained_bytes(), 0);
    }

    #[test]
    fn global_roundtrip() {
        let len = 3_000_000;
        let cap = {
            let mut b = PoolBuf::new(len);
            b[len - 1] = 7;
            b.capacity()
        };
        assert!(global().retained_bytes() >= cap);
        let small = PoolBuf::new(10);
        assert_eq!(small.len(), 10);
    }
}
