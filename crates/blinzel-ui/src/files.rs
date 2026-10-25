//! Files with the standard library only, for platforms without faster
//! APIs of their own (macOS, Linux): reading into pooled buffers and
//! listing a folder's images.

use std::io::{self, Read};
use std::path::Path;
use std::time::UNIX_EPOCH;

use blinzel_core::bufpool::PoolBuf;
use blinzel_core::filelist::Entry;
use blinzel_core::format::has_image_extension;

/// Reads a whole file into a pooled buffer.
pub fn read_file(path: &Path) -> io::Result<PoolBuf> {
    let mut f = std::fs::File::open(path)?;
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

fn to_entry(name: String, m: &std::fs::Metadata) -> Entry {
    // Modification time in 100 ns steps, like Windows' FILETIME.
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| (d.as_nanos() / 100) as u64);
    Entry {
        name,
        size: m.len(),
        mtime,
        attributes: 0,
    }
}

/// Name, size and modification time of one file.
pub fn entry(path: &Path) -> io::Result<Entry> {
    let m = std::fs::metadata(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(to_entry(name, &m))
}

/// The images in `dir` (by extension), unsorted.
pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !has_image_extension(&name) {
            continue;
        }
        // Follows symbolic links, like opening the file would.
        if let Ok(m) = std::fs::metadata(e.path())
            && m.is_file()
        {
            out.push(to_entry(name, &m));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_images_only_and_reads_them() {
        let dir = std::env::temp_dir().join(format!("blinzel-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub.jpg")).unwrap();
        std::fs::write(dir.join("a.JPG"), b"12345").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let list = list_dir(&dir).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].size), ("a.JPG", 5));
        assert_eq!(entry(&dir.join("a.JPG")).unwrap(), list[0]);
        assert_eq!(&read_file(&dir.join("a.JPG")).unwrap()[..], b"12345");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
