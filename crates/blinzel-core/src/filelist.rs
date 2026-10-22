//! The images of one folder in natural order, with neighbour navigation.

use std::path::{Path, PathBuf};

use crate::format::has_image_extension;
use crate::natsort::SortKey;

/// File attributes we care about (subset of Windows' `FILE_ATTRIBUTE_*`).
pub mod attr {
    pub const HIDDEN: u32 = 0x2;
    pub const SYSTEM: u32 = 0x4;
    pub const DIRECTORY: u32 = 0x10;
    pub const OFFLINE: u32 = 0x1000;
    pub const RECALL_ON_OPEN: u32 = 0x40000;
    pub const RECALL_ON_DATA_ACCESS: u32 = 0x400000;
}

/// One directory entry as delivered by the enumeration (no file opened).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    /// Last write time (FILETIME ticks on Windows).
    pub mtime: u64,
    pub attributes: u32,
}

impl Entry {
    /// Cloud placeholder whose content is not local: reading it would start
    /// a download, so prefetching and thumbnailing skip it.
    pub fn is_placeholder(&self) -> bool {
        self.attributes & (attr::OFFLINE | attr::RECALL_ON_OPEN | attr::RECALL_ON_DATA_ACCESS) != 0
    }

    /// Stable 64-bit identity for caches: name + size + mtime.
    pub fn cache_id(&self) -> u64 {
        let mut h = Fnv::new();
        h.write(self.name.to_lowercase().as_bytes());
        h.write(&self.size.to_le_bytes());
        h.write(&self.mtime.to_le_bytes());
        h.finish()
    }
}

/// FNV-1a, enough for cache keys.
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Fnv {
    fn default() -> Self {
        Fnv::new()
    }
}

/// Sorted image list of a folder.
#[derive(Clone, Debug, Default)]
pub struct FileList {
    dir: PathBuf,
    entries: Vec<Entry>,
}

impl FileList {
    /// Keeps image files (by extension, no directories/hidden system files)
    /// and sorts them naturally.
    pub fn new(dir: impl Into<PathBuf>, entries: Vec<Entry>) -> FileList {
        let mut keyed: Vec<(SortKey, Entry)> = entries
            .into_iter()
            .filter(|e| {
                e.attributes & attr::DIRECTORY == 0
                    && e.attributes & (attr::HIDDEN | attr::SYSTEM) != (attr::HIDDEN | attr::SYSTEM)
                    && has_image_extension(&e.name)
            })
            .map(|e| (SortKey::new(&e.name), e))
            .collect();
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
        FileList {
            dir: dir.into(),
            entries: keyed.into_iter().map(|(_, e)| e).collect(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<&Entry> {
        self.entries.get(i)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn path(&self, i: usize) -> Option<PathBuf> {
        self.entries.get(i).map(|e| self.dir.join(&e.name))
    }

    /// Index of a file name (case-insensitive, as on Windows).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.name == name)
            .or_else(|| {
                self.entries
                    .iter()
                    .position(|e| e.name.eq_ignore_ascii_case(name))
            })
            .or_else(|| {
                let lower = name.to_lowercase();
                self.entries
                    .iter()
                    .position(|e| e.name.to_lowercase() == lower)
            })
    }

    /// Index `steps` away from `i`, wrapping around at the ends.
    pub fn offset(&self, i: usize, steps: isize) -> Option<usize> {
        let n = self.entries.len() as isize;
        if n == 0 {
            return None;
        }
        Some((i as isize + steps).rem_euclid(n) as usize)
    }

    /// Removes an entry (after deleting the file); returns the index of the
    /// image to show next.
    pub fn remove(&mut self, i: usize) -> Option<usize> {
        if i >= self.entries.len() {
            return None;
        }
        self.entries.remove(i);
        if self.entries.is_empty() {
            None
        } else {
            Some(i.min(self.entries.len() - 1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, attributes: u32) -> Entry {
        Entry {
            name: name.into(),
            size: 10,
            mtime: 1,
            attributes,
        }
    }

    #[test]
    fn filters_sorts_and_navigates() {
        let list = FileList::new(
            "C:\\pics",
            vec![
                e("img10.jpg", 0),
                e("notes.txt", 0),
                e("img2.JPG", 0),
                e("sub.jpg", attr::DIRECTORY),
                e("thumbs.png", attr::HIDDEN | attr::SYSTEM),
                e("img1.png", attr::HIDDEN),
            ],
        );
        let names: Vec<&str> = list.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["img1.png", "img2.JPG", "img10.jpg"]);
        assert_eq!(list.index_of("IMG2.jpg"), Some(1));
        assert_eq!(list.offset(2, 1), Some(0));
        assert_eq!(list.offset(0, -1), Some(2));
        assert_eq!(list.offset(1, 7), Some(2));
        assert!(list.path(0).unwrap().ends_with("img1.png"));
    }

    #[test]
    fn remove_and_placeholders() {
        let mut list = FileList::new(
            "d",
            vec![e("a.jpg", 0), e("b.jpg", attr::RECALL_ON_DATA_ACCESS)],
        );
        assert!(list.get(1).unwrap().is_placeholder());
        assert!(!list.get(0).unwrap().is_placeholder());
        assert_eq!(list.remove(1), Some(0));
        assert_eq!(list.remove(0), None);
        assert!(FileList::default().offset(0, 1).is_none());
    }

    #[test]
    fn cache_ids_change_with_content() {
        let a = e("a.jpg", 0);
        let mut b = a.clone();
        assert_eq!(a.cache_id(), b.cache_id());
        b.mtime = 2;
        assert_ne!(a.cache_id(), b.cache_id());
        let mut c = a.clone();
        c.name = "A.JPG".into();
        assert_eq!(a.cache_id(), c.cache_id());
    }
}
