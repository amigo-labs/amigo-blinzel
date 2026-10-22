//! CPU topology (hybrid P/E cores) and per-thread scheduling hints.

use windows::Win32::System::SystemInformation::{
    GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, SetThreadPriority,
    THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE,
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
    let best = classes.iter().copied().max().unwrap_or(0);
    let performance = classes.iter().filter(|&&c| c == best).count().max(1);
    CpuInfo {
        logical,
        performance,
    }
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
    unsafe {
        let thread = GetCurrentThread();
        let (priority, throttle) = match kind {
            Kind::Latency => (THREAD_PRIORITY_ABOVE_NORMAL, 0),
            Kind::Background => (
                THREAD_PRIORITY_BELOW_NORMAL,
                THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            ),
        };
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
