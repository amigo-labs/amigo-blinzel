//! The silent self-updater (M7). At most daily, ten seconds after start and
//! on a background thread, it asks GitHub for the latest release, downloads
//! the executable for this architecture, and verifies it two ways before
//! swapping it in (active from the next start):
//!
//! 1. Ed25519 signature of `SHA256SUMS` with our own release key,
//! 2. SHA-256 of the executable against `SHA256SUMS`.
//!
//! Releases carry no Authenticode signature (docs/PLAN.md, section 10), so
//! the release key is the only proof of origin.
//!
//! A new version that fails to reach its first frame three times is
//! replaced by the previous one again. WinHTTP and bcrypt are
//! loaded only when needed, never on the startup path. Only an installed
//! copy updates itself; a portable or development build never does.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blinzel_core::update::{self as core, Release, StartAction, State, Version};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{BOOL, PCSTR, PCWSTR, w};

use crate::platform::{debug_log, wide};
use crate::setup::{self, Layout, registry};

/// Public key of the release signing key (docs/PLAN.md, section 10). Until
/// it is set, the updater checks nothing and installs nothing.
pub const RELEASE_KEY: Option<[u8; 32]> = None;

/// Largest download accepted (the executable is about 3 MB).
const MAX_DOWNLOAD: usize = 64 << 20;

fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

fn running_version() -> Version {
    Version::parse(setup::version()).expect("crate version is x.y.z")
}

/// The layout when this process is the installed executable.
fn installed_layout() -> Option<Layout> {
    let layout = Layout::user()?;
    let exe = std::env::current_exe().ok()?;
    layout.is_installed_exe(&exe).then_some(layout)
}

fn state_path(layout: &Layout) -> PathBuf {
    layout.cache.join("update.state")
}

fn load_state(layout: &Layout) -> State {
    std::fs::read_to_string(state_path(layout))
        .map(|t| State::parse(&t))
        .unwrap_or_default()
}

fn save_state(layout: &Layout, state: &State) {
    let _ = std::fs::create_dir_all(&layout.cache);
    let _ = std::fs::write(state_path(layout), state.to_text());
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn old_exe(layout: &Layout) -> PathBuf {
    layout.exe().with_extension("exe.old")
}

/// Called early at every start. Returns `true` when this process should
/// exit because it rolled back and started the previous version instead.
pub fn on_start() -> bool {
    let Some(layout) = installed_layout() else {
        return false;
    };
    let mut state = load_state(&layout);
    let before = state.clone();
    let action = core::on_start(&mut state, running_version());
    if action == StartAction::Rollback {
        let (exe, old) = (layout.exe(), old_exe(&layout));
        let bad = exe.with_extension("exe.bad");
        let _ = std::fs::remove_file(&bad);
        if std::fs::rename(&exe, &bad).is_ok() && std::fs::rename(&old, &exe).is_ok() {
            state.pending = None;
            state.failed_starts = 0;
            save_state(&layout, &state);
            let _ = std::process::Command::new(&exe)
                .args(std::env::args_os().skip(1))
                .spawn();
            return true;
        }
        // Nothing to roll back to: keep going with this version.
        state.pending = None;
        state.failed_starts = 0;
    }
    if state.pending.is_none() {
        // The previous executable is no longer needed.
        let _ = std::fs::remove_file(old_exe(&layout));
        let _ = std::fs::remove_file(layout.exe().with_extension("exe.bad"));
    }
    if state != before {
        save_state(&layout, &state);
    }
    false
}

/// Called once after the first frame: the running version works.
pub fn on_first_frame() {
    let _ = std::thread::Builder::new()
        .name("blinzel-update-ok".into())
        .spawn(|| {
            let Some(layout) = installed_layout() else {
                return;
            };
            let mut state = load_state(&layout);
            if state.pending.is_some() {
                core::on_first_frame(&mut state);
                save_state(&layout, &state);
                if let Ok(k) = registry::Key::create(&layout.uninstall) {
                    let _ = k.set_str("DisplayVersion", setup::version());
                }
            }
        });
}

/// Starts the daily background check. A resident instance runs for days,
/// so it looks again every hour and reads the setting each time.
pub fn schedule() {
    if RELEASE_KEY.is_none() || installed_layout().is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("blinzel-update".into())
        .spawn(|| {
            std::thread::sleep(Duration::from_secs(10));
            loop {
                if crate::settings_store::load().auto_update {
                    check_if_due();
                }
                std::thread::sleep(Duration::from_secs(3600));
            }
        });
}

fn check_if_due() {
    let Some(layout) = installed_layout() else {
        return;
    };
    let mut state = load_state(&layout);
    if !core::check_due(state.last_check, now()) {
        return;
    }
    state.last_check = Some(now());
    save_state(&layout, &state);
    if let Err(e) = check_and_install(&layout) {
        debug_log(&format!("update: {e}"));
    }
}

/// An update was installed while this process kept running the previous
/// version (resident mode): it should hand over to the new executable.
pub fn restart_pending() -> bool {
    installed_layout().is_some_and(|l| {
        load_state(&l)
            .pending
            .is_some_and(|p| p != running_version())
    })
}

/// Starts the installed (updated) executable as the new resident instance.
pub fn start_installed_resident() {
    if let Some(layout) = Layout::user() {
        let _ = std::process::Command::new(layout.exe())
            .arg("--resident")
            .spawn();
    }
}

/// What a check found.
pub enum Outcome {
    Current(Version),
    Installed(Version),
}

/// "Check for updates" from the menu: runs in the background and posts the
/// result to `hwnd` as a notice.
pub fn check_now(hwnd: HWND, notice_msg: u32) {
    let raw = hwnd.0 as isize;
    let _ = std::thread::Builder::new()
        .name("blinzel-update-now".into())
        .spawn(move || {
            use blinzel_i18n::{Msg, trf};
            let text = match installed_layout()
                .ok_or_else(|| "not installed".to_string())
                .and_then(|l| check_and_install(&l))
            {
                Ok(Outcome::Current(v)) => trf(Msg::UpdateCurrent, &[("version", &v)]),
                Ok(Outcome::Installed(v)) => trf(Msg::UpdateReady, &[("version", &v)]),
                Err(e) => trf(Msg::UpdateFailed, &[("reason", &e)]),
            };
            let ptr = Box::into_raw(Box::new(text)) as isize;
            unsafe {
                if PostMessageW(
                    Some(HWND(raw as *mut _)),
                    notice_msg,
                    WPARAM(0),
                    LPARAM(ptr),
                )
                .is_err()
                {
                    drop(Box::from_raw(ptr as *mut String));
                }
            }
        });
}

fn check_and_install(layout: &Layout) -> Result<Outcome, String> {
    let key = RELEASE_KEY.ok_or("no release signing key in this build")?;
    let api = format!(
        "https://api.github.com/repos/{}/releases/latest",
        core::REPO
    );
    let json = http_get(&api, 1 << 20)?;
    let json = String::from_utf8(json).map_err(|_| "release info is not UTF-8")?;
    let Release {
        version,
        exe_url,
        sums_url,
        sig_url,
    } = core::parse_release(&json, arch()).ok_or("no usable release")?;
    let running = running_version();
    if version <= running {
        return Ok(Outcome::Current(running));
    }

    let sums = http_get(&sums_url, 64 << 10)?;
    let sig = http_get(&sig_url, 4 << 10)?;
    if !core::verify_signature(&sums, &sig, &key) {
        return Err("checksums are not signed with the release key".into());
    }
    let sums = String::from_utf8(sums).map_err(|_| "checksums are not UTF-8")?;
    let want = core::sum_for(&sums, &core::asset_name(arch())).ok_or("no checksum for us")?;
    let exe = http_get(&exe_url, MAX_DOWNLOAD)?;
    if sha256(&exe)? != want {
        return Err("download does not match its checksum".into());
    }

    let dir = layout.cache.join("updates").join(version.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let new = dir.join(setup::install::EXE_NAME);
    std::fs::write(&new, &exe).map_err(|e| e.to_string())?;

    swap(&layout.exe(), &new, &old_exe(layout))?;
    let mut state = load_state(layout);
    state.pending = Some(version);
    state.failed_starts = 0;
    save_state(layout, &state);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(Outcome::Installed(version))
}

/// Puts `new` in place of the running `installed` executable (renaming a
/// running executable is allowed; overwriting it is not).
fn swap(installed: &Path, new: &Path, old: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(old);
    std::fs::rename(installed, old).map_err(|e| format!("rename running exe: {e}"))?;
    if let Err(e) = std::fs::rename(new, installed) {
        let _ = std::fs::rename(old, installed);
        return Err(format!("move new exe: {e}"));
    }
    Ok(())
}

fn proc(dll: PCWSTR, name: PCSTR) -> Result<RawFn, String> {
    unsafe {
        let m: HMODULE = LoadLibraryW(dll).map_err(|e| e.message())?;
        GetProcAddress(m, name).ok_or_else(|| format!("{:?} missing", name.to_string()))
    }
}

/// SHA-256 through Windows' CNG (no crypto code of our own).
fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    type BCryptHash =
        unsafe extern "system" fn(isize, *const u8, u32, *const u8, u32, *mut u8, u32) -> i32;
    // BCRYPT_SHA256_ALG_HANDLE, a pseudo-handle (Windows 10 and newer).
    const SHA256_ALG: isize = 0x41;
    let f = proc(w!("bcrypt.dll"), windows::core::s!("BCryptHash"))?;
    let f: BCryptHash = unsafe { std::mem::transmute(f) };
    let len = u32::try_from(data.len()).map_err(|_| "too large")?;
    let mut out = [0u8; 32];
    let status = unsafe {
        f(
            SHA256_ALG,
            std::ptr::null(),
            0,
            data.as_ptr(),
            len,
            out.as_mut_ptr(),
            32,
        )
    };
    if status != 0 {
        return Err(format!("BCryptHash: 0x{status:08X}"));
    }
    Ok(out)
}

/// HTTPS GET through WinHTTP (system proxy settings), following redirects;
/// at most `limit` bytes.
fn http_get(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("only HTTPS")?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let path = if path.is_empty() { "/" } else { path };
    let api = WinHttp::load()?;
    let agent = wide(format!("amigo-blinzel/{}", setup::version()));
    let (host_w, path_w) = (wide(host), wide(path));
    let headers = wide("Accept: application/vnd.github+json, application/octet-stream\r\n");
    unsafe {
        // WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
        let session = Handle(
            &api,
            (api.open)(PCWSTR(agent.as_ptr()), 4, PCWSTR::null(), PCWSTR::null(), 0),
        );
        if session.1.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        let connect = Handle(
            &api,
            (api.connect)(session.1, PCWSTR(host_w.as_ptr()), 443, 0),
        );
        if connect.1.is_null() {
            return Err(format!("cannot connect to {host}"));
        }
        // WINHTTP_FLAG_SECURE
        let request = Handle(
            &api,
            (api.open_request)(
                connect.1,
                w!("GET"),
                PCWSTR(path_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                0x0080_0000,
            ),
        );
        if request.1.is_null() {
            return Err("WinHttpOpenRequest failed".into());
        }
        if !(api.send)(
            request.1,
            PCWSTR(headers.as_ptr()),
            u32::MAX,
            std::ptr::null(),
            0,
            0,
            0,
        )
        .as_bool()
            || !(api.receive)(request.1, std::ptr::null_mut()).as_bool()
        {
            return Err(format!("request to {host} failed"));
        }
        let mut status = 0u32;
        let mut size = 4u32;
        // WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER
        let _ = (api.query_headers)(
            request.1,
            19 | 0x2000_0000,
            PCWSTR::null(),
            std::ptr::from_mut(&mut status).cast(),
            &mut size,
            std::ptr::null_mut(),
        );
        if status != 200 {
            return Err(format!("HTTP {status} from {host}"));
        }
        let mut body = Vec::new();
        let mut buf = vec![0u8; 64 << 10];
        loop {
            let mut read = 0u32;
            if !(api.read)(
                request.1,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
            )
            .as_bool()
            {
                return Err("download interrupted".into());
            }
            if read == 0 {
                return Ok(body);
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > limit {
                return Err("download too large".into());
            }
        }
    }
}

type Hinternet = *mut c_void;

type RawFn = unsafe extern "system" fn() -> isize;
type OpenFn = unsafe extern "system" fn(PCWSTR, u32, PCWSTR, PCWSTR, u32) -> Hinternet;
type ConnectFn = unsafe extern "system" fn(Hinternet, PCWSTR, u16, u32) -> Hinternet;
type OpenRequestFn = unsafe extern "system" fn(
    Hinternet,
    PCWSTR,
    PCWSTR,
    PCWSTR,
    PCWSTR,
    *const PCWSTR,
    u32,
) -> Hinternet;
type SendFn =
    unsafe extern "system" fn(Hinternet, PCWSTR, u32, *const c_void, u32, u32, usize) -> BOOL;
type ReceiveFn = unsafe extern "system" fn(Hinternet, *mut c_void) -> BOOL;
type QueryHeadersFn =
    unsafe extern "system" fn(Hinternet, u32, PCWSTR, *mut c_void, *mut u32, *mut u32) -> BOOL;
type ReadFn = unsafe extern "system" fn(Hinternet, *mut c_void, u32, *mut u32) -> BOOL;
type CloseFn = unsafe extern "system" fn(Hinternet) -> BOOL;

/// The WinHTTP functions in use, loaded on demand.
struct WinHttp {
    open: OpenFn,
    connect: ConnectFn,
    open_request: OpenRequestFn,
    send: SendFn,
    receive: ReceiveFn,
    query_headers: QueryHeadersFn,
    read: ReadFn,
    close: CloseFn,
}

impl WinHttp {
    fn load() -> Result<WinHttp, String> {
        use std::mem::transmute;
        use windows::core::s;
        let p = |name: PCSTR| proc(w!("winhttp.dll"), name);
        // SAFETY: the signatures match winhttp.h.
        unsafe {
            Ok(WinHttp {
                open: transmute::<RawFn, OpenFn>(p(s!("WinHttpOpen"))?),
                connect: transmute::<RawFn, ConnectFn>(p(s!("WinHttpConnect"))?),
                open_request: transmute::<RawFn, OpenRequestFn>(p(s!("WinHttpOpenRequest"))?),
                send: transmute::<RawFn, SendFn>(p(s!("WinHttpSendRequest"))?),
                receive: transmute::<RawFn, ReceiveFn>(p(s!("WinHttpReceiveResponse"))?),
                query_headers: transmute::<RawFn, QueryHeadersFn>(p(s!("WinHttpQueryHeaders"))?),
                read: transmute::<RawFn, ReadFn>(p(s!("WinHttpReadData"))?),
                close: transmute::<RawFn, CloseFn>(p(s!("WinHttpCloseHandle"))?),
            })
        }
    }
}

/// A WinHTTP handle, closed on drop.
struct Handle<'a>(&'a WinHttp, Hinternet);

impl Drop for Handle<'_> {
    fn drop(&mut self) {
        if !self.1.is_null() {
            unsafe {
                let _ = (self.0.close)(self.1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_with_cng() {
        // SHA-256("abc"), FIPS 180-2.
        let want = core::parse_hex::<32>(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
        assert_eq!(sha256(b"abc").unwrap(), want);
    }

    /// Talks to GitHub; run explicitly with `--ignored`.
    #[test]
    #[ignore]
    fn fetches_over_https() {
        let ok = http_get(
            "https://api.github.com/repos/amigo-labs/amigo-blinzel",
            1 << 20,
        )
        .unwrap();
        assert!(String::from_utf8(ok).unwrap().contains("amigo-blinzel"));
        let missing = http_get(
            "https://api.github.com/repos/amigo-labs/does-not-exist-404",
            1 << 20,
        );
        assert!(missing.unwrap_err().contains("HTTP 404"));
    }

    #[test]
    fn swaps_and_restores() {
        let dir = std::env::temp_dir().join(format!("blinzel-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (installed, new, old) = (
            dir.join("a.exe"),
            dir.join("new.exe"),
            dir.join("a.exe.old"),
        );
        std::fs::write(&installed, b"v1").unwrap();
        std::fs::write(&new, b"v2").unwrap();
        swap(&installed, &new, &old).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), b"v2");
        assert_eq!(std::fs::read(&old).unwrap(), b"v1");
        // A missing new file leaves the installed one in place.
        assert!(swap(&installed, &dir.join("missing.exe"), &old).is_err());
        assert_eq!(std::fs::read(&installed).unwrap(), b"v2");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
