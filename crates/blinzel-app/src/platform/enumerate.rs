//! Folder listing and change notification.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use blinzel_core::filelist::Entry;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FIND_FIRST_EX_LARGE_FETCH, FindClose,
    FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, OPEN_EXISTING,
    ReadDirectoryChangesW, WIN32_FIND_DATAW,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::PCWSTR;

use super::{from_wide, wide};

/// Lists a folder (files and directories) without opening any file.
/// `FIND_FIRST_EX_LARGE_FETCH` makes the kernel return big batches;
/// `FindExInfoBasic` skips the 8.3 short names.
pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let pattern = wide(dir.join("*"));
    let mut data = WIN32_FIND_DATAW::default();
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            std::ptr::from_mut(&mut data).cast(),
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    }
    .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
    let mut out = Vec::with_capacity(256);
    loop {
        let name = from_wide(&data.cFileName);
        if name != "." && name != ".." {
            out.push(Entry {
                name,
                size: ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64,
                mtime: ((data.ftLastWriteTime.dwHighDateTime as u64) << 32)
                    | data.ftLastWriteTime.dwLowDateTime as u64,
                attributes: data.dwFileAttributes,
            });
        }
        if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
            break;
        }
    }
    unsafe {
        let _ = FindClose(handle);
    }
    Ok(out)
}

/// Watches a folder and posts `message` to `hwnd` (debounced) when files
/// appear, disappear or change.
pub struct DirWatcher {
    stop: HANDLE,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub dir: PathBuf,
}

unsafe impl Send for DirWatcher {}

#[derive(Clone, Copy)]
struct SendHwnd(HWND);
unsafe impl Send for SendHwnd {}
#[derive(Clone, Copy)]
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

impl DirWatcher {
    pub fn start(dir: &Path, hwnd: HWND, message: u32) -> io::Result<DirWatcher> {
        let path = wide(dir);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(|e| io::Error::other(e.to_string()))?;
        let stop = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|e| io::Error::other(e.to_string()))?;
        let stopped = Arc::new(AtomicBool::new(false));
        let (h, s, w, flag) = (
            SendHandle(handle),
            SendHandle(stop),
            SendHwnd(hwnd),
            stopped.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("blinzel-watch".into())
            .spawn(move || {
                let (h, s, w) = (h, s, w);
                watch(h.0, s.0, w.0, message, &flag);
                unsafe {
                    let _ = CloseHandle(h.0);
                }
            })?;
        Ok(DirWatcher {
            stop,
            stopped,
            thread: Some(thread),
            dir: dir.to_path_buf(),
        })
    }
}

fn watch(dir: HANDLE, stop: HANDLE, hwnd: HWND, message: u32, stopped: &AtomicBool) {
    let mut buf = vec![0u32; 16 * 1024];
    let Ok(event) = (unsafe { CreateEventW(None, true, false, None) }) else {
        return;
    };
    let filter =
        FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;
    while !stopped.load(Ordering::Relaxed) {
        let mut ov = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let issued = unsafe {
            ReadDirectoryChangesW(
                dir,
                buf.as_mut_ptr().cast(),
                (buf.len() * 4) as u32,
                false,
                filter,
                None,
                Some(&mut ov),
                None,
            )
        };
        if issued.is_err() {
            break;
        }
        let r = unsafe { WaitForMultipleObjects(&[event, stop], false, u32::MAX) };
        if r != WAIT_OBJECT_0 {
            unsafe {
                let _ = CancelIoEx(dir, Some(&ov));
                let mut n = 0;
                let _ = GetOverlappedResult(dir, &ov, &mut n, true);
            }
            break;
        }
        let mut n = 0;
        let _ = unsafe { GetOverlappedResult(dir, &ov, &mut n, false) };
        // Debounce bursts (copying 100 files fires hundreds of events).
        std::thread::sleep(Duration::from_millis(150));
        let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
        unsafe {
            let _ = windows::Win32::System::Threading::ResetEvent(event);
        }
    }
    unsafe {
        let _ = CloseHandle(event);
    }
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        unsafe {
            let _ = SetEvent(self.stop);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _ = CloseHandle(self.stop);
        }
    }
}
