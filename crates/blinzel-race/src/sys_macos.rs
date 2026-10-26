//! macOS plumbing for the race: clock, processes, windows, input, cache.
//! A viewer is identified by its app bundle (`name` is the bundle path);
//! its processes are those whose executable lies inside the bundle.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType, CGPoint};
use objc2_core_graphics::{
    CGDisplayCopyDisplayMode, CGDisplayMode, CGEvent, CGEventTapLocation, CGMainDisplayID,
    CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess, CGWarpMouseCursorPosition,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowLayer,
    kCGWindowOwnerPID,
};
use objc2_foundation::NSProcessInfo;

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

/// Right arrow (`kVK_RightArrow`).
const KEY_RIGHT: u16 = 124;
const PATH_MAX: usize = 4096;
const SIGKILL: c_int = 9;

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> c_int;
    fn proc_listallpids(buffer: *mut c_void, size: c_int) -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
    fn geteuid() -> u32;
    fn sysctlbyname(
        name: *const c_char,
        old: *mut c_void,
        old_len: *mut usize,
        new: *const c_void,
        new_len: usize,
    ) -> c_int;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// `mach_absolute_time`, the clock ScreenCaptureKit's display times use.
pub fn now() -> i64 {
    // SAFETY: no arguments.
    unsafe { mach_absolute_time() as i64 }
}

pub fn ticks_to_ms(ticks: i64) -> f64 {
    let mut tb = Timebase { numer: 1, denom: 1 };
    // SAFETY: writes the struct.
    unsafe { mach_timebase_info(&mut tb) };
    ticks as f64 * tb.numer as f64 / tb.denom as f64 / 1e6
}

/// Both permissions the measurement needs, checked before anything runs;
/// for `cold`, root for `purge` (asked once, so the run is not interrupted).
pub fn prepare(cold: bool) -> Result<(), String> {
    if !CGPreflightScreenCaptureAccess() {
        CGRequestScreenCaptureAccess();
        return Err(
            "allow Screen Recording for this terminal in System Settings, \
                    Privacy & Security, then start it again"
                .into(),
        );
    }
    // SAFETY: no arguments.
    if unsafe { AXIsProcessTrusted() } == 0 {
        return Err("allow Accessibility for this terminal in System Settings, \
                    Privacy & Security (for the key presses), then start it again"
            .into());
    }
    // SAFETY: no arguments.
    if unsafe { geteuid() } == 0 {
        return Err(
            "do not run this as root: the viewers would start as root too; \
                    --cold asks for the password itself"
                .into(),
        );
    }
    if cold
        && !Command::new("/usr/bin/sudo")
            .arg("-v")
            .status()
            .is_ok_and(|s| s.success())
    {
        return Err("--cold needs sudo for purge".into());
    }
    Ok(())
}

/// Process ids of everything running from inside the bundle `name`.
pub fn pids_named(name: &str) -> Vec<i32> {
    let bundle = std::fs::canonicalize(name).unwrap_or_else(|_| name.into());
    let inside = bundle.join("Contents");
    let mut pids = vec![0 as c_int; 4096];
    // SAFETY: the buffer holds `pids.len()` ints.
    let n = unsafe {
        proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * size_of::<c_int>()) as c_int,
        )
    };
    pids.truncate(n.max(0) as usize);
    pids.into_iter()
        .filter(|&pid| pid > 0)
        .filter(|&pid| {
            let mut buf = [0u8; PATH_MAX];
            // SAFETY: the buffer holds PATH_MAX bytes.
            let len = unsafe { proc_pidpath(pid, buf.as_mut_ptr().cast(), PATH_MAX as u32) };
            len > 0
                && std::str::from_utf8(&buf[..len as usize])
                    .is_ok_and(|p| Path::new(p).starts_with(&inside))
        })
        .collect()
}

/// Quits every instance of the bundle `name`: politely first (the app
/// saves nothing it was not asked to), then for good.
pub fn close_all(name: &str) {
    let pids = pids_named(name);
    if pids.is_empty() {
        return;
    }
    for &pid in &pids {
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            app.terminate();
        }
    }
    for _ in 0..30 {
        if pids_named(name).is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for pid in pids_named(name) {
        // SAFETY: plain signal.
        unsafe { kill(pid, SIGKILL) };
    }
}

/// The process owning the frontmost normal window: the window list is
/// ordered front to back; layer 0 holds application windows.
fn frontmost_pid() -> Option<i32> {
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    // SAFETY: an array of dictionaries with string keys.
    let list = unsafe { list.cast_unchecked::<CFDictionary>() };
    list.iter().find_map(|w| {
        // SAFETY: as above.
        let w = unsafe { w.cast_unchecked::<CFString, CFType>() };
        // SAFETY: Core Graphics' constant keys.
        let get = |k: &CFString| w.get(k)?.downcast::<CFNumber>().ok()?.as_i64();
        let (layer, pid) = unsafe { (get(kCGWindowLayer), get(kCGWindowOwnerPID)) };
        (layer == Some(0)).then_some(pid? as i32)
    })
}

/// Brings the viewer to the front, as Launch Services does when its app
/// is opened again.
pub fn focus(name: &str) -> bool {
    if !owns_foreground(name) {
        let _ = Command::new("/usr/bin/open").arg("-a").arg(name).status();
        std::thread::sleep(Duration::from_millis(300));
    }
    owns_foreground(name)
}

/// Whether the frontmost window belongs to the viewer. Keys are only sent
/// then, never into whatever else has the focus.
pub fn owns_foreground(name: &str) -> bool {
    frontmost_pid().is_some_and(|pid| pids_named(name).contains(&pid))
}

fn key(down: bool) {
    if let Some(e) = CGEvent::new_keyboard_event(None, KEY_RIGHT, down) {
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&e));
    }
}

/// Presses and releases the Right arrow key.
pub fn tap_right() {
    key(true);
    key(false);
}

/// Parks the mouse in the middle of a display (viewers that open on the
/// display under the cursor then open where the capture looks).
pub fn park_cursor(rect: (i32, i32, i32, i32)) {
    let _ = CGWarpMouseCursorPosition(CGPoint::new(
        (rect.0 + rect.2) as f64 / 2.0,
        (rect.1 + rect.3) as f64 / 2.0,
    ));
}

/// Empties the file cache so the next start reads everything from disk.
/// `purge` needs root; `sudo` remembers the password from `prepare` (each
/// use extends that).
pub fn purge_cache() -> Result<(), String> {
    match Command::new("/usr/bin/sudo")
        .args(["-n", "/usr/sbin/purge"])
        .status()
    {
        Ok(s) if s.success() => Ok(()),
        _ => Err("purging the file cache failed (sudo purge)".into()),
    }
}

fn sysctl_string(name: &CStr) -> Option<String> {
    let mut buf = [0u8; 256];
    let mut len = buf.len();
    // SAFETY: the buffer holds `len` bytes.
    let r = unsafe {
        sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null(),
            0,
        )
    };
    (r == 0).then(|| {
        CStr::from_bytes_until_nul(&buf[..len])
            .map_or_else(|_| String::new(), |s| s.to_string_lossy().into_owned())
    })
}

fn sysctl_u64(name: &CStr) -> Option<u64> {
    let mut v = 0u64;
    let mut len = size_of::<u64>();
    // SAFETY: writes one u64.
    let r = unsafe {
        sysctlbyname(
            name.as_ptr(),
            (&raw mut v).cast(),
            &mut len,
            std::ptr::null(),
            0,
        )
    };
    (r == 0).then_some(v)
}

/// The GPU as System Information names it ("Apple M2", "AMD Radeon ...").
fn gpu() -> String {
    Command::new("/usr/sbin/system_profiler")
        .arg("SPDisplaysDataType")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|l| l.trim().strip_prefix("Chipset Model:").map(str::trim))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "?".into())
}

/// The machine, for the report.
pub fn machine(cap: &crate::capture::Capture) -> Vec<(&'static str, String)> {
    let main = CGMainDisplayID();
    let mode = CGDisplayCopyDisplayMode(main);
    let (w, h) = (
        CGDisplayMode::pixel_width(mode.as_deref()),
        CGDisplayMode::pixel_height(mode.as_deref()),
    );
    vec![
        (
            "cpu",
            sysctl_string(c"machdep.cpu.brand_string").unwrap_or_else(|| "?".into()),
        ),
        (
            "logical_cpus",
            std::thread::available_parallelism()
                .map_or(0, |n| n.get())
                .to_string(),
        ),
        (
            "ram_gb",
            format!(
                "{:.0}",
                sysctl_u64(c"hw.memsize").unwrap_or(0) as f64 / (1u64 << 30) as f64
            ),
        ),
        ("gpu", gpu()),
        ("monitor", format!("{w}x{h}")),
        ("monitors", cap.monitors.to_string()),
        (
            "os",
            format!(
                "macOS {}",
                NSProcessInfo::processInfo().operatingSystemVersionString()
            ),
        ),
    ]
}
