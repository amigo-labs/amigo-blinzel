//! The thumbnail cache: one append-only pack file of encoded thumbnails
//! (`thumbs.pack`) and a log of fixed 64-byte index records
//! (`thumbs.idx`), loaded into a hash table on open.
//!
//! Appends run under an exclusive lock on the index file, so several
//! instances can share the cache; what another instance appended is seen
//! after the next open (a duplicate does no harm). When the pack would
//! grow past its limit, a new generation starts: both files are emptied.
//! Every blob starts with a header naming its key, so a stale index entry
//! (another process started a new generation) reads as a miss.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::decode::{self, Request};
use crate::filelist::Fnv;
use crate::geom::Orientation;
use crate::thumb::Thumb;

/// What a cached thumbnail belongs to: the file's path and the size and
/// modification time it had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub path_hash: u64,
    pub size: u64,
    pub mtime: u64,
}

impl Key {
    pub fn new(path: &Path, size: u64, mtime: u64) -> Key {
        let mut h = Fnv::new();
        // Windows paths are case-insensitive.
        h.write(path.to_string_lossy().to_lowercase().as_bytes());
        Key {
            path_hash: h.finish(),
            size,
            mtime,
        }
    }
}

const RECORD: usize = 64;
const BLOB_MAGIC: u32 = u32::from_le_bytes(*b"BLZT");
const BLOB_HEADER: usize = 16;
#[cfg_attr(not(feature = "turbojpeg"), allow(dead_code))]
const FORMAT_JPEG: u8 = 0;
const FORMAT_PNG: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Record {
    key: Key,
    offset: u64,
    len: u32,
    width: u16,
    height: u16,
    full: (u32, u32),
    orientation: u8,
    format: u8,
}

impl Record {
    fn to_bytes(self) -> [u8; RECORD] {
        let mut b = [0u8; RECORD];
        b[0..8].copy_from_slice(&self.key.path_hash.to_le_bytes());
        b[8..16].copy_from_slice(&self.key.size.to_le_bytes());
        b[16..24].copy_from_slice(&self.key.mtime.to_le_bytes());
        b[24..32].copy_from_slice(&self.offset.to_le_bytes());
        b[32..36].copy_from_slice(&self.len.to_le_bytes());
        b[36..38].copy_from_slice(&self.width.to_le_bytes());
        b[38..40].copy_from_slice(&self.height.to_le_bytes());
        b[40..44].copy_from_slice(&self.full.0.to_le_bytes());
        b[44..48].copy_from_slice(&self.full.1.to_le_bytes());
        b[48] = self.orientation;
        b[49] = self.format;
        let sum = checksum(&b[..56]);
        b[56..64].copy_from_slice(&sum.to_le_bytes());
        b
    }

    /// `None` for a torn or corrupt record.
    fn from_bytes(b: &[u8]) -> Option<Record> {
        let u64_at = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let u16_at = |i: usize| u16::from_le_bytes(b[i..i + 2].try_into().unwrap());
        if b.len() != RECORD || u64_at(56) != checksum(&b[..56]) {
            return None;
        }
        Some(Record {
            key: Key {
                path_hash: u64_at(0),
                size: u64_at(8),
                mtime: u64_at(16),
            },
            offset: u64_at(24),
            len: u32_at(32),
            width: u16_at(36),
            height: u16_at(38),
            full: (u32_at(40), u32_at(44)),
            orientation: b[48],
            format: b[49],
        })
    }
}

fn checksum(b: &[u8]) -> u64 {
    let mut h = Fnv::new();
    h.write(b);
    h.finish()
}

pub struct ThumbCache {
    dir: PathBuf,
    /// By path hash: the latest record.
    index: Mutex<HashMap<u64, Record>>,
    pack: File,
    /// Pack size that starts a new generation.
    limit: u64,
    appending: Mutex<()>,
}

impl ThumbCache {
    /// Opens (or creates) the cache in `dir`.
    pub fn open(dir: &Path, limit: u64) -> io::Result<ThumbCache> {
        std::fs::create_dir_all(dir)?;
        let pack = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("thumbs.pack"))?;
        let mut index = HashMap::new();
        if let Ok(mut f) = File::open(dir.join("thumbs.idx")) {
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes)?;
            for r in bytes
                .as_chunks::<RECORD>()
                .0
                .iter()
                .filter_map(|b| Record::from_bytes(b))
            {
                index.insert(r.key.path_hash, r);
            }
        }
        Ok(ThumbCache {
            dir: dir.to_path_buf(),
            index: Mutex::new(index),
            pack,
            limit,
            appending: Mutex::new(()),
        })
    }

    pub fn len(&self) -> usize {
        self.index.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The cached thumbnail for `key`, if it is there and current.
    pub fn get(&self, key: &Key) -> Option<Thumb> {
        let r = *self.index.lock().unwrap().get(&key.path_hash)?;
        if r.key != *key {
            return None; // the file changed since
        }
        let mut blob = vec![0u8; BLOB_HEADER + r.len as usize];
        read_at(&self.pack, &mut blob, r.offset).ok()?;
        let magic = u32::from_le_bytes(blob[0..4].try_into().unwrap());
        let len = u32::from_le_bytes(blob[4..8].try_into().unwrap());
        let hash = u64::from_le_bytes(blob[8..16].try_into().unwrap());
        if magic != BLOB_MAGIC || len != r.len || hash != key.path_hash {
            return None;
        }
        let img = decode::decode(&blob[BLOB_HEADER..], Request::Full).ok()?;
        if (img.width, img.height) != (r.width as u32, r.height as u32) {
            return None;
        }
        Some(Thumb {
            width: img.width,
            height: img.height,
            bgra: crate::thumb::to_bgra(&img, (img.width, img.height)),
            orientation: Orientation::from_exif(r.orientation as u16),
            full: r.full,
        })
    }

    /// Stores `thumb` for `key` (appends; a newer entry replaces older
    /// ones on load).
    pub fn put(&self, key: &Key, thumb: &Thumb) -> io::Result<()> {
        let (format, data) = encode(thumb)?;
        let _one_at_a_time = self.appending.lock().unwrap();
        // Not append mode: locking and truncating need read/write access.
        let mut idx = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join("thumbs.idx"))?;
        idx.lock()?;
        let result = (|| {
            let mut pack = OpenOptions::new()
                .write(true)
                .open(self.dir.join("thumbs.pack"))?;
            let mut offset = pack.metadata()?.len();
            if offset + (BLOB_HEADER + data.len()) as u64 > self.limit {
                // New generation.
                pack.set_len(0)?;
                idx.set_len(0)?;
                offset = 0;
                self.index.lock().unwrap().clear();
            }
            let mut blob = Vec::with_capacity(BLOB_HEADER + data.len());
            blob.extend(BLOB_MAGIC.to_le_bytes());
            blob.extend((data.len() as u32).to_le_bytes());
            blob.extend(key.path_hash.to_le_bytes());
            blob.extend(&data);
            pack.seek(SeekFrom::Start(offset))?;
            pack.write_all(&blob)?;
            let r = Record {
                key: *key,
                offset,
                len: data.len() as u32,
                width: thumb.width as u16,
                height: thumb.height as u16,
                full: thumb.full,
                orientation: thumb.orientation.to_exif() as u8,
                format,
            };
            idx.seek(SeekFrom::End(0))?;
            idx.write_all(&r.to_bytes())?;
            self.index.lock().unwrap().insert(key.path_hash, r);
            Ok(())
        })();
        let _ = idx.unlock();
        result
    }
}

/// JPEG (quality 85) for opaque thumbnails, PNG when they have alpha or
/// no JPEG encoder is built in.
fn encode(t: &Thumb) -> io::Result<(u8, Vec<u8>)> {
    #[cfg(feature = "turbojpeg")]
    if !t.has_alpha() {
        use crate::decode::jpeg::{EncodeOptions, encode_rgb};
        let rgb: Vec<u8> = t
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        let opts = EncodeOptions {
            quality: 85,
            ..Default::default()
        };
        return encode_rgb(&rgb, t.width, t.height, opts)
            .map(|d| (FORMAT_JPEG, d))
            .map_err(|e| io::Error::other(e.to_string()));
    }
    let rgba: Vec<u8> = t
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, t.width, t.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_compression(png::Compression::Fast);
    let mut w = enc.write_header().map_err(io::Error::other)?;
    w.write_image_data(&rgba).map_err(io::Error::other)?;
    w.finish().map_err(io::Error::other)?;
    Ok((FORMAT_PNG, out))
}

#[cfg(windows)]
fn read_at(f: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match f.seek_read(buf, offset)? {
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(buf, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb(w: u32, h: u32, alpha: bool) -> Thumb {
        let mut bgra = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let a = if alpha && x < w / 2 { 0 } else { 255 };
                bgra.extend([(x * 255 / w) as u8, (y * 255 / h) as u8, 128, a]);
            }
        }
        Thumb {
            width: w,
            height: h,
            bgra,
            orientation: Orientation::from_exif(6),
            full: (w * 10, h * 10),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("blinzel-thumbcache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn stores_and_reloads() {
        let dir = temp_dir("reload");
        let key = Key::new(Path::new(r"C:\Bilder\A.jpg"), 1234, 99);
        let t = thumb(256, 171, false);
        {
            let c = ThumbCache::open(&dir, 1 << 30).unwrap();
            assert!(c.get(&key).is_none());
            c.put(&key, &t).unwrap();
            let got = c.get(&key).unwrap();
            assert_eq!((got.width, got.height, got.full), (256, 171, t.full));
            assert_eq!(got.orientation, t.orientation);
            // Lossy (JPEG) where available, but close.
            assert!(crate::cpuref::psnr(&got.bgra, &t.bgra) > 30.0);
        }
        // Reopened: from the index file; the key is case-insensitive.
        let c = ThumbCache::open(&dir, 1 << 30).unwrap();
        let same = Key::new(Path::new(r"c:\bilder\a.JPG"), 1234, 99);
        assert!(c.get(&same).is_some());
        // A changed file misses.
        assert!(c.get(&Key { mtime: 100, ..key }).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn alpha_survives_and_newer_entries_win() {
        let dir = temp_dir("alpha");
        let c = ThumbCache::open(&dir, 1 << 30).unwrap();
        let key = Key::new(Path::new("x.png"), 1, 1);
        c.put(&key, &thumb(32, 16, false)).unwrap();
        let t = thumb(16, 8, true);
        let key2 = Key { size: 2, ..key };
        c.put(&key2, &t).unwrap();
        assert!(c.get(&key).is_none());
        let got = c.get(&key2).unwrap();
        assert_eq!(got.bgra, t.bgra); // PNG: lossless
        drop(c);
        assert!(
            ThumbCache::open(&dir, 1 << 30)
                .unwrap()
                .get(&key2)
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn starts_a_new_generation_at_the_limit() {
        let dir = temp_dir("limit");
        let c = ThumbCache::open(&dir, 40_000).unwrap();
        let keys: Vec<Key> = (0..20)
            .map(|i| Key::new(Path::new(&format!("{i}.png")), 1, 1))
            .collect();
        // Noise does not compress: about 16 KB per thumbnail.
        let mut noisy = thumb(64, 64, true);
        let mut x = 0x9E37_79B9u32;
        for v in noisy.bgra.iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *v = x as u8;
        }
        for k in &keys {
            c.put(k, &noisy).unwrap();
        }
        assert!(std::fs::metadata(dir.join("thumbs.pack")).unwrap().len() <= 40_000);
        // The latest entries are there, the earliest are gone.
        assert!(c.get(keys.last().unwrap()).is_some());
        assert!(c.get(&keys[0]).is_none());
        assert!(c.len() < keys.len());
        let reopened = ThumbCache::open(&dir, 40_000).unwrap();
        assert_eq!(reopened.len(), c.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignores_torn_records() {
        let r = Record {
            key: Key::new(Path::new("a"), 1, 2),
            offset: 3,
            len: 4,
            width: 5,
            height: 6,
            full: (7, 8),
            orientation: 1,
            format: FORMAT_PNG,
        };
        let mut b = r.to_bytes();
        assert_eq!(Record::from_bytes(&b), Some(r));
        b[30] ^= 1;
        assert_eq!(Record::from_bytes(&b), None);
        assert_eq!(Record::from_bytes(&b[..10]), None);
    }
}
