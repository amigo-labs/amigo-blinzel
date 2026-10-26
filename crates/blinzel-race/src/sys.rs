//! Windows plumbing for the race: clock, processes, windows, input, cache.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_TERMINATE, TerminateProcess,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_MENU, VK_RIGHT,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
    SW_SHOWNORMAL, SetCursorPos, SetForegroundWindow, WM_CLOSE,
};
use windows::core::{BOOL, PCWSTR, w};

pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

pub fn from_wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// QueryPerformanceCounter, the clock DXGI present times use.
pub fn now() -> i64 {
    let mut t = 0;
    unsafe {
        let _ = QueryPerformanceCounter(&mut t);
    }
    t
}

pub fn ticks_to_ms(ticks: i64) -> f64 {
    let mut f = 1;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
    }
    ticks as f64 * 1e3 / f as f64
}

/// Activates a URI (`ms-photos:` etc.).
pub fn open_uri(uri: &str) -> Result<(), String> {
    let u = wide(uri);
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(u.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success with a value above 32.
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("cannot open {uri}"))
    }
}

/// Process ids of every running process with this image name.
pub fn pids_named(name: &str) -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            if from_wide(&e.szExeFile).eq_ignore_ascii_case(name) {
                out.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Visible top-level windows of `pid`.
pub fn windows_of(pid: u32) -> Vec<HWND> {
    struct Find {
        pid: u32,
        found: Vec<HWND>,
    }
    extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let f = unsafe { &mut *(lp.0 as *mut Find) };
        let mut p = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut p)) };
        if p == f.pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            f.found.push(hwnd);
        }
        BOOL(1)
    }
    let mut f = Find {
        pid,
        found: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(std::ptr::from_mut(&mut f) as isize));
    }
    f.found
}

/// Closes every instance of `name`: politely first, then for good.
pub fn close_all(name: &str) {
    let pids = pids_named(name);
    if pids.is_empty() {
        return;
    }
    for &pid in &pids {
        for h in windows_of(pid) {
            unsafe {
                let _ = PostMessageW(Some(h), WM_CLOSE, Default::default(), Default::default());
            }
        }
    }
    for _ in 0..30 {
        if pids_named(name).is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for pid in pids_named(name) {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(h, 1);
                let _ = CloseHandle(h);
            }
        }
    }
}

/// Brings a window of process `name` to the foreground (a synthetic Alt
/// press lifts the foreground lock).
pub fn focus(name: &str) -> bool {
    let Some(h) = pids_named(name).into_iter().flat_map(windows_of).next() else {
        return false;
    };
    if !owns_foreground(name) {
        key(VK_MENU, false);
        key(VK_MENU, true);
        unsafe {
            let _ = SetForegroundWindow(h);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    owns_foreground(name)
}

/// Whether the foreground window belongs to a process named `name`. Keys
/// are only sent then, never into whatever else has the focus.
pub fn owns_foreground(name: &str) -> bool {
    let h = unsafe { GetForegroundWindow() };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid != 0 && pids_named(name).contains(&pid)
}

fn key(vk: VIRTUAL_KEY, up: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

/// Presses and releases the Right arrow key.
pub fn tap_right() {
    key(VK_RIGHT, false);
    key(VK_RIGHT, true);
}

/// Parks the mouse in the middle of a monitor (viewers that open on the
/// monitor under the cursor then open where the capture looks).
pub fn park_cursor(rect: (i32, i32, i32, i32)) {
    unsafe {
        let _ = SetCursorPos((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2);
    }
}

/// Empties the standby list so the next start reads everything from disk
/// (needs administrator rights).
pub fn purge_cache() -> Result<(), String> {
    unsafe {
        // SeProfileSingleProcessPrivilege allows purging the standby list.
        let mut token = Default::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .map_err(|e| e.message())?;
        let mut luid = LUID::default();
        LookupPrivilegeValueW(
            PCWSTR::null(),
            w!("SeProfileSingleProcessPrivilege"),
            &mut luid,
        )
        .map_err(|e| e.message())?;
        let mut tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            ..Default::default()
        };
        tp.Privileges[0].Luid = luid;
        tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
        let adjusted = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
        let _ = CloseHandle(token);
        adjusted.map_err(|e| e.message())?;

        type NtSetSystemInformation = unsafe extern "system" fn(u32, *const u32, u32) -> i32;
        let ntdll = GetModuleHandleW(w!("ntdll.dll")).map_err(|e| e.message())?;
        let f = GetProcAddress(ntdll, windows::core::s!("NtSetSystemInformation"))
            .ok_or("NtSetSystemInformation missing")?;
        let f: NtSetSystemInformation =
            std::mem::transmute::<unsafe extern "system" fn() -> isize, NtSetSystemInformation>(f);
        // SystemMemoryListInformation, MemoryPurgeStandbyList
        let command = 4u32;
        let status = f(80, &command, 4);
        if status == 0 {
            Ok(())
        } else {
            Err(format!(
                "purging the standby list failed (0x{status:08X}); run as administrator"
            ))
        }
    }
}

/// Physical pixels for monitor geometry and the mouse position. (`--cold`
/// needs an elevated console, checked when purging.)
pub fn prepare(_cold: bool) -> Result<(), String> {
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    Ok(())
}

/// The machine, for the report.
pub fn machine(cap: &crate::capture::Capture) -> Vec<(&'static str, String)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut mem = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GlobalMemoryStatusEx(&mut mem);
    }
    let (l, t, r, b) = cap.monitor;
    vec![
        ("cpu", blinzel_core::kernels::cpu_name()),
        (
            "logical_cpus",
            std::thread::available_parallelism()
                .map_or(0, |n| n.get())
                .to_string(),
        ),
        (
            "ram_gb",
            format!("{:.0}", mem.ullTotalPhys as f64 / (1u64 << 30) as f64),
        ),
        ("gpu", cap.adapter.clone()),
        ("monitor", format!("{}x{}", r - l, b - t)),
        ("monitors", cap.monitors.to_string()),
        ("os_build", os_build().to_string()),
    ]
}

fn os_build() -> u32 {
    #[repr(C)]
    struct Info {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    unsafe {
        let Ok(ntdll) = GetModuleHandleW(w!("ntdll.dll")) else {
            return 0;
        };
        let Some(f) = GetProcAddress(ntdll, windows::core::s!("RtlGetVersion")) else {
            return 0;
        };
        let f = std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            unsafe extern "system" fn(*mut Info) -> i32,
        >(f);
        let mut v = Info {
            size: std::mem::size_of::<Info>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            csd: [0; 128],
        };
        if f(&mut v) == 0 { v.build } else { 0 }
    }
}
