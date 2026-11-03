//! CPU topology (hybrid P/E cores) and per-thread scheduling hints.

use windows::Win32::System::SystemInformation::{
    GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, SetThreadPriority,
    THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY,
    THREAD_PRIORITY_ABOVE_NORMAL, THREAD_PRIORITY_BELOW_NORMAL, ThreadPowerThrottling,
};

#[derive(Clone, Copy, Debug)]
pub struct CpuInfo {
    pub logical: usize,
    /// Logical processors of the most performant efficiency class (all of
    /// them on non-hybrid CPUs).
    pub performance: usize,
}

pub fn cpu_info() -> CpuInfo {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let mut len = 0u32;
    unsafe {
        let _ = GetSystemCpuSetInformation(None, 0, &mut len, None, None);
    }
    if len == 0 {
        return CpuInfo {
            logical,
            performance: logical,
        };
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let ok = unsafe {
        GetSystemCpuSetInformation(Some(buf.as_mut_ptr().cast()), len, &mut len, None, None)
    };
    if !ok.as_bool() {
        return CpuInfo {
            logical,
            performance: logical,
        };
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), len as usize) };
    let mut classes = Vec::new();
    let mut off = 0usize;
    while off + std::mem::size_of::<SYSTEM_CPU_SET_INFORMATION>() <= bytes.len() {
        let info = unsafe { &*(bytes.as_ptr().add(off) as *const SYSTEM_CPU_SET_INFORMATION) };
        if info.Size == 0 {
            break;
        }
        classes.push(unsafe { info.Anonymous.CpuSet.EfficiencyClass });
        off += info.Size as usize;
    }
    CpuInfo {
        logical,
        performance: performance(&classes),
    }
}

/// Logical processors of the highest efficiency class among `classes` (one
/// entry per logical processor; a higher class is more performant), at
/// least 1.
fn performance(classes: &[u8]) -> usize {
    let best = classes.iter().copied().max().unwrap_or(0);
    classes.iter().filter(|&&c| c == best).count().max(1)
}

/// How a thread should be scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The image the user waits for: above normal, never power-throttled
    /// (stays on P-cores).
    Latency,
    /// Background work (later: thumbnails): EcoQoS, E-cores welcome.
    Background,
}

pub fn tune_current_thread(kind: Kind) {
    let (priority, throttle) = scheduling(kind);
    unsafe {
        let thread = GetCurrentThread();
        let _ = SetThreadPriority(thread, priority);
        let state = THREAD_POWER_THROTTLING_STATE {
            Version: 1,
            ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: throttle,
        };
        let _ = SetThreadInformation(
            thread,
            ThreadPowerThrottling,
            std::ptr::from_ref(&state).cast(),
            std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

/// The priority and the power-throttling state (`StateMask` for
/// `THREAD_POWER_THROTTLING_EXECUTION_SPEED`) a thread of `kind` gets.
fn scheduling(kind: Kind) -> (THREAD_PRIORITY, u32) {
    match kind {
        Kind::Latency => (THREAD_PRIORITY_ABOVE_NORMAL, 0),
        Kind::Background => (
            THREAD_PRIORITY_BELOW_NORMAL,
            THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, performance, scheduling, tune_current_thread};
    use windows::Win32::System::Threading::{
        GetCurrentThread, GetThreadPriority, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        THREAD_PRIORITY_ABOVE_NORMAL, THREAD_PRIORITY_BELOW_NORMAL,
    };

    #[test]
    fn performance_counts_the_most_performant_efficiency_class() {
        // Hybrid: 8 E-cores (class 0), 4 P-cores with SMT (class 1).
        let hybrid: Vec<u8> = [0; 8].into_iter().chain([1; 8]).collect();
        assert_eq!(performance(&hybrid), 8);
        assert_eq!(performance(&[1, 1, 0, 0, 0, 0]), 2);
        // Not hybrid: every processor is in the same class.
        assert_eq!(performance(&[0; 12]), 12);
        assert_eq!(performance(&[]), 1);
    }

    #[test]
    fn scheduling_keeps_latency_threads_above_normal_and_unthrottled() {
        assert_eq!(scheduling(Kind::Latency), (THREAD_PRIORITY_ABOVE_NORMAL, 0));
        assert_eq!(
            scheduling(Kind::Background),
            (
                THREAD_PRIORITY_BELOW_NORMAL,
                THREAD_POWER_THROTTLING_EXECUTION_SPEED
            )
        );
    }

    #[test]
    fn tune_current_thread_sets_the_priority() {
        let priority = |kind: Kind| {
            std::thread::spawn(move || {
                tune_current_thread(kind);
                // SAFETY: the pseudo handle of the calling thread needs no
                // closing and is valid while it runs.
                unsafe { GetThreadPriority(GetCurrentThread()) }
            })
            .join()
            .unwrap()
        };
        assert_eq!(priority(Kind::Latency), THREAD_PRIORITY_ABOVE_NORMAL.0);
        assert_eq!(priority(Kind::Background), THREAD_PRIORITY_BELOW_NORMAL.0);
    }
}
