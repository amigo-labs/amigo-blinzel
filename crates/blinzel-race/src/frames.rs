//! What reached the screen: the frames a capture saw, with when they were
//! presented and which test image they showed. Shared by the platform
//! captures; times are ticks of the platform clock (`sys::now`).

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// One presented frame: when it reached the screen and what it showed.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub at: i64,
    pub sig: Option<usize>,
}

/// The frames seen so far; cheap to clone into capture threads.
#[derive(Clone, Default)]
pub struct Frames(Arc<(Mutex<Vec<Event>>, Condvar)>);

impl Frames {
    pub fn push(&self, ev: Event) {
        let (lock, cv) = &*self.0;
        lock.lock().unwrap().push(ev);
        cv.notify_all();
    }

    /// Waits until a frame presented at or after `since` shows signature
    /// `want`; returns its present time.
    pub fn wait_for(&self, since: i64, want: usize, timeout: Duration) -> Result<i64, String> {
        let deadline = Instant::now() + timeout;
        let (lock, cv) = &*self.0;
        let mut events = lock.lock().unwrap();
        loop {
            if let Some(ev) = events.iter().find(|e| e.at >= since && e.sig == Some(want)) {
                return Ok(ev.at);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("signature {want} not seen"));
            }
            events = cv.wait_timeout(events, left).unwrap().0;
        }
    }

    /// Frames presented at or after `since`.
    pub fn since(&self, since: i64) -> Vec<Event> {
        let events = self.0.0.lock().unwrap();
        events.iter().filter(|e| e.at >= since).copied().collect()
    }

    /// Forgets old frames (between measurements).
    pub fn clear(&self) {
        self.0.0.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_a_signature() {
        let f = Frames::default();
        f.push(Event {
            at: 5,
            sig: Some(1),
        });
        let g = f.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            g.push(Event {
                at: 9,
                sig: Some(2),
            });
        });
        assert_eq!(f.wait_for(0, 2, Duration::from_secs(5)), Ok(9));
        t.join().unwrap();
        assert_eq!(f.wait_for(6, 1, Duration::from_millis(10)).ok(), None);
        assert_eq!(f.since(6).len(), 1);
        f.clear();
        assert!(f.since(0).is_empty());
    }
}
