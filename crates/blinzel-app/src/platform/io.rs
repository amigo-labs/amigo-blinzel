//! File reading for decode workers.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::filelist::Entry;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW,
    GetVolumePathNameW, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery,
    STORAGE_PROPERTY_QUERY, StorageDeviceSeekPenaltyProperty,
};
use windows::core::PCWSTR;

use super::wide;

const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
const DRIVE_REMOTE: u32 = 4;
/// Others may read, write and delete while we read (like Explorer).
const SHARE_ALL: u32 = 0x7;

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(SHARE_ALL)
        .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
        .open(path)
}

/// Reads a whole file into a pooled buffer with one sequential read.
pub fn read_file(path: &Path) -> io::Result<PoolBuf> {
    let mut f = open(path)?;
    let len = f.metadata()?.len() as usize;
    let mut buf = PoolBuf::new(len);
    let mut filled = 0;
    while filled < len {
        match f.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Reads at most `max` bytes from the start (header probing).
pub fn read_head(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    let mut f = open(path)?;
    let mut buf = vec![0u8; max];
    let mut filled = 0;
    while filled < max {
        match f.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Directory entry for a single file (name, size, mtime, attributes).
pub fn entry(path: &Path) -> io::Result<Entry> {
    let m = std::fs::metadata(path)?;
    Ok(Entry {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: m.file_size(),
        mtime: m.last_write_time(),
        attributes: m.file_attributes(),
    })
}

/// What kind of storage a path lives on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DriveInfo {
    /// Rotating disk: parallel reads only add seeks.
    pub seek_penalty: bool,
    /// Network share: high latency, keep prefetch small.
    pub remote: bool,
}

pub fn drive_info(path: &Path) -> DriveInfo {
    let mut root = [0u16; 261];
    let p = wide(path);
    if unsafe { GetVolumePathNameW(PCWSTR(p.as_ptr()), &mut root) }.is_err() {
        return DriveInfo::default();
    }
    let remote = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } == DRIVE_REMOTE;
    let root_str = super::from_wide(&root);
    let seek_penalty = !remote && seek_penalty(&root_str).unwrap_or(false);
    DriveInfo {
        seek_penalty,
        remote,
    }
}

/// Asks the storage stack whether the volume incurs a seek penalty (HDD).
fn seek_penalty(root: &str) -> Option<bool> {
    // "C:\" → "\\.\C:"
    let letter = root.trim_end_matches('\\');
    if letter.len() != 2 || !letter.ends_with(':') {
        return None;
    }
    let dev = wide(format!("\\\\.\\{letter}"));
    unsafe {
        let h: HANDLE = CreateFileW(
            PCWSTR(dev.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()?;
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceSeekPenaltyProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut out = DEVICE_SEEK_PENALTY_DESCRIPTOR::default();
        let mut returned = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(std::ptr::from_ref(&query).cast()),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(std::ptr::from_mut(&mut out).cast()),
            std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
            Some(&mut returned),
            None,
        );
        let _ = CloseHandle(h);
        ok.ok()?;
        Some(out.IncursSeekPenalty)
    }
}
