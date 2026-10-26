//! Colour management shared by the loader (building transforms ahead,
//! the CPU first frame) and the UI thread: the current display target and
//! the transforms (3D LUTs) built for it, by source profile.
//!
//! sRGB images on an sRGB target (the usual case) need nothing: the
//! platform's colour system is never touched and the shader skips the
//! lookup.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use blinzel_core::icc::Profile;
use blinzel_core::lut::Lut3d;

/// Builds the transform from a source profile to a target profile
/// (`None`: sRGB), relative colorimetric.
pub type Builder = fn(Option<&Profile>, Option<&Profile>) -> Option<Lut3d>;

enum Slot {
    Building,
    Ready(Option<Arc<Lut3d>>),
}

/// The current target: `None` until known, then `Some(target)`, with a
/// generation that changes with it.
#[derive(Default)]
struct TargetState {
    target: Option<Option<Profile>>,
    generation: u64,
}

/// Colour management shared by the loader (CPU first frame, building
/// ahead) and the UI thread: the current target and the LUTs built for it.
pub struct ColorManager {
    build: Builder,
    enabled: AtomicBool,
    target: Mutex<TargetState>,
    known: Condvar,
    /// By (target generation, source profile hash; 0 for sRGB).
    luts: Mutex<HashMap<(u64, u64), Slot>>,
    built: Condvar,
}

impl ColorManager {
    /// `build` makes the transform from a source profile to the target
    /// (`None`: sRGB) with the platform's colour system.
    pub fn new(enabled: bool, build: Builder) -> Arc<ColorManager> {
        Arc::new(ColorManager {
            build,
            enabled: AtomicBool::new(enabled),
            target: Mutex::new(TargetState::default()),
            known: Condvar::new(),
            luts: Mutex::new(HashMap::new()),
            built: Condvar::new(),
        })
    }

    /// Sets the target (on start, and when the window changes monitors).
    /// True if it changed.
    pub fn set_target(&self, target: Option<Profile>) -> bool {
        let mut t = self.target.lock().unwrap();
        let changed = t.target.as_ref() != Some(&target);
        if changed {
            t.target = Some(target);
            t.generation += 1;
            self.luts
                .lock()
                .unwrap()
                .retain(|_, s| matches!(s, Slot::Building));
        }
        self.known.notify_all();
        changed
    }

    /// Starts building the transform for `src` in the background (the
    /// image decodes meanwhile), if one is needed and the target is known.
    pub fn prepare(self: &Arc<Self>, src: Option<&Profile>) {
        let wide_target = matches!(&self.target.lock().unwrap().target, Some(Some(_)));
        if !self.enabled.load(Ordering::Relaxed) || (src.is_none_or(|p| p.srgb) && !wide_target) {
            return;
        }
        let (me, src) = (self.clone(), src.cloned());
        let _ = std::thread::Builder::new()
            .name("blinzel-color".into())
            .spawn(move || {
                me.lut(src.as_ref());
            });
    }

    /// Loads the platform's colour system once, ahead of the first image
    /// that needs it (resident start), with the profile most photos with
    /// one carry (phones: Display P3).
    pub fn warm_up(&self) {
        use blinzel_core::icc::{Trc, colorants, matrix_profile};
        let p3 = Profile::new(matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3"));
        let _ = (self.build)(Some(&p3), None);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// The transform for an image with profile `src` (none: sRGB), or
    /// `None` when it needs none. Waits briefly for the target on start.
    pub fn lut(&self, src: Option<&Profile>) -> Option<Arc<Lut3d>> {
        if !self.enabled.load(Ordering::Relaxed) {
            return None;
        }
        let (target, generation) = {
            let t = self.target.lock().unwrap();
            let (t, _) = self
                .known
                .wait_timeout_while(t, Duration::from_millis(200), |t| t.target.is_none())
                .unwrap();
            (t.target.clone().flatten(), t.generation)
        };
        let src_srgb = src.is_none_or(|p| p.srgb);
        if src_srgb && target.is_none() {
            return None;
        }
        if let (Some(s), Some(t)) = (src, &target)
            && s.hash == t.hash
        {
            return None;
        }
        let key = (
            generation,
            if src_srgb {
                0
            } else {
                src.map_or(0, |p| p.hash)
            },
        );
        {
            let mut luts = self.luts.lock().unwrap();
            loop {
                match luts.get(&key) {
                    Some(Slot::Ready(l)) => return l.clone(),
                    // Another thread builds it (e.g. ahead of the decode).
                    Some(Slot::Building) => luts = self.built.wait(luts).unwrap(),
                    None => break,
                }
            }
            luts.insert(key, Slot::Building);
        }
        let t0 = std::time::Instant::now();
        let built = (self.build)(src.filter(|_| !src_srgb), target.as_ref()).map(Arc::new);
        let us = t0.elapsed().as_secs_f64() * 1e6;
        blinzel_core::timing::record("color_lut_us", us);
        self.luts
            .lock()
            .unwrap()
            .insert(key, Slot::Ready(built.clone()));
        self.built.notify_all();
        built
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_core::icc::{Trc, colorants, matrix_profile};
    use std::sync::atomic::AtomicUsize;

    static BUILT: AtomicUsize = AtomicUsize::new(0);

    fn identity(_: Option<&Profile>, _: Option<&Profile>) -> Option<Lut3d> {
        BUILT.fetch_add(1, Ordering::Relaxed);
        let n = blinzel_core::lut::SIZE;
        let step = 65535.0 / (n - 1) as f64;
        let nodes = (0..n * n * n)
            .map(|i| [i % n, i / n % n, i / (n * n)].map(|v| (v as f64 * step).round() as u16))
            .collect();
        Lut3d::from_nodes(n, nodes)
    }

    #[test]
    fn caches_per_profile_and_skips_srgb() {
        let p3 = Profile::new(matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3 test"));
        let m = ColorManager::new(true, identity);
        m.set_target(None);
        assert!(m.lut(None).is_none());
        let a = m.lut(Some(&p3)).unwrap();
        let b = m.lut(Some(&p3)).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(BUILT.load(Ordering::Relaxed), 1);
        // A new target builds anew.
        assert!(m.set_target(Some(p3.clone())));
        assert!(m.lut(Some(&p3)).is_none(), "same profile as the target");
        m.set_enabled(false);
        assert!(m.lut(None).is_none());
    }
}
