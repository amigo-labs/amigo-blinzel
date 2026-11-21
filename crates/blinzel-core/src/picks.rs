//! The lasting marks of a folder (docs/specs/picks.md, decision "Picks" in
//! docs/specs/blinzel.md §2): one text file per folder in the per-user
//! config folder, never next to the images.
//!
//! The file is `<store>/<hash of the folder path>.txt`: a header line
//! with the folder's path (two folders whose hashes collide are told
//! apart by it), then one file name per line, UTF-8. A change re-reads
//! the file, applies only itself and writes through a temporary file and
//! a rename, so two windows that mark in the same folder keep each
//! other's marks.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

const HEADER: &str = "# Blinzel marks: ";

/// A change to a folder's marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Mark(Vec<String>),
    Unmark(Vec<String>),
    /// Every stored name of the folder goes.
    Clear,
}

/// The marks file of `folder` in `store`.
pub fn file_of(store: &Path, folder: &Path) -> PathBuf {
    store.join(format!(
        "{:016x}.txt",
        fnv1a(folder.to_string_lossy().as_bytes())
    ))
}

fn fnv1a(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325, |h, &c| {
        (h ^ c as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Whether `name` can be stored: a plain file name.
pub fn storable(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', '\0', '\n', '\r', '\u{FFFD}'])
        && name != "."
        && name != ".."
}

/// The names in a marks file's text for `folder`: only valid names, none
/// if the header names another folder or is missing.
pub fn parse(text: &str, folder: &Path) -> BTreeSet<String> {
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let header = format!("{HEADER}{}", folder.to_string_lossy());
    if lines.next() != Some(header.as_str()) {
        return BTreeSet::new();
    }
    lines.filter(|l| storable(l)).map(str::to_string).collect()
}

/// The text of a marks file of `folder` holding `names`.
pub fn to_text(folder: &Path, names: &BTreeSet<String>) -> String {
    let mut s = format!("{HEADER}{}\n", folder.to_string_lossy());
    for n in names.iter().filter(|n| storable(n)) {
        s.push_str(n);
        s.push('\n');
    }
    s
}

/// The stored marks of `folder` (none if there is no file yet). A file
/// that is not UTF-8 keeps its valid lines.
pub fn load(store: &Path, folder: &Path) -> io::Result<BTreeSet<String>> {
    match std::fs::read(file_of(store, folder)) {
        Ok(b) => Ok(parse(&String::from_utf8_lossy(&b), folder)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e),
    }
}

/// Applies `change` to the stored marks of `folder` and returns them.
pub fn apply(store: &Path, folder: &Path, change: &Change) -> io::Result<BTreeSet<String>> {
    let mut names = load(store, folder)?;
    match change {
        Change::Mark(n) => names.extend(n.iter().filter(|n| storable(n)).cloned()),
        Change::Unmark(n) => names.retain(|x| !n.contains(x)),
        Change::Clear => names.clear(),
    }
    std::fs::create_dir_all(store)?;
    let file = file_of(store, folder);
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, to_text(folder, &names))?;
    std::fs::rename(&tmp, &file).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(names)
}

/// The result of [`copy_files`].
#[derive(Debug, Default)]
pub struct Copied {
    /// Files copied, by where they went.
    pub done: Vec<PathBuf>,
    /// Files that could not be copied, with the reason.
    pub failed: Vec<(PathBuf, String)>,
    /// Stopped by `cancel` before the end.
    pub cancelled: bool,
}

/// Copies `files` byte for byte into `target` under their own names, or
/// "stem (2).ext", "(3)" … where a name is taken, never replacing a file.
/// `progress` gets the number of files finished; `cancel` stops after the
/// file in progress is removed again.
pub fn copy_files(
    files: &[PathBuf],
    target: &Path,
    progress: impl Fn(usize),
    cancel: &std::sync::atomic::AtomicBool,
) -> Copied {
    use std::sync::atomic::Ordering;
    let mut out = Copied::default();
    for (i, src) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        match copy_one(src, target, cancel) {
            Ok(Some(to)) => out.done.push(to),
            Ok(None) => {
                out.cancelled = true;
                break;
            }
            Err(e) => out.failed.push((src.clone(), e.to_string())),
        }
        progress(i + 1);
    }
    out
}

/// One file; `None` if cancelled (nothing left behind).
fn copy_one(
    src: &Path,
    target: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> io::Result<Option<PathBuf>> {
    use std::io::{Read, Write};
    let mut from = std::fs::File::open(src)?;
    let name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let (to, mut file) = create_free(target, &name.to_string_lossy())?;
    let mut buf = vec![0u8; 1 << 20];
    let r: io::Result<bool> = (|| {
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(false);
            }
            let n = from.read(&mut buf)?;
            if n == 0 {
                file.flush()?;
                return Ok(true);
            }
            file.write_all(&buf[..n])?;
        }
    })();
    match r {
        Ok(true) => Ok(Some(to)),
        Ok(false) => {
            drop(file);
            let _ = std::fs::remove_file(&to);
            Ok(None)
        }
        Err(e) => {
            drop(file);
            let _ = std::fs::remove_file(&to);
            Err(e)
        }
    }
}

/// Creates `name` in `folder`, or the first free "stem (n).ext"; the
/// creation itself checks, so a file made meanwhile is never replaced.
pub fn create_free(folder: &Path, name: &str) -> io::Result<(PathBuf, std::fs::File)> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    for n in 1..10_000 {
        let candidate = match (n, ext) {
            (1, _) => name.to_string(),
            (n, Some(e)) => format!("{stem} ({n}).{e}"),
            (n, None) => format!("{stem} ({n})"),
        };
        let path = folder.join(&candidate);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blinzel-picks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn marks_survive_a_round_trip_through_the_file() {
        let store = tmp("roundtrip");
        let folder = Path::new("/photos/shoot 1");
        apply(
            &store,
            folder,
            &Change::Mark(vec!["a.jpg".into(), "b #2.jpg".into()]),
        )
        .unwrap();
        assert_eq!(load(&store, folder).unwrap(), set(&["a.jpg", "b #2.jpg"]));
        apply(&store, folder, &Change::Unmark(vec!["a.jpg".into()])).unwrap();
        assert_eq!(load(&store, folder).unwrap(), set(&["b #2.jpg"]));
        apply(&store, folder, &Change::Clear).unwrap();
        assert!(load(&store, folder).unwrap().is_empty());
        // The image folder is never touched.
        assert!(
            std::fs::read_dir(&store)
                .unwrap()
                .all(|e| e.unwrap().path().starts_with(&store))
        );
    }

    #[test]
    fn each_folder_has_its_own_marks() {
        let store = tmp("folders");
        let (a, b) = (Path::new("/p/a"), Path::new("/p/b"));
        apply(&store, a, &Change::Mark(vec!["x.jpg".into()])).unwrap();
        apply(&store, b, &Change::Mark(vec!["y.jpg".into()])).unwrap();
        assert_eq!(load(&store, a).unwrap(), set(&["x.jpg"]));
        assert_eq!(load(&store, b).unwrap(), set(&["y.jpg"]));
        assert_ne!(file_of(&store, a), file_of(&store, b));
    }

    #[test]
    fn a_change_keeps_what_another_window_stored_meanwhile() {
        let store = tmp("merge");
        let f = Path::new("/p/shoot");
        apply(&store, f, &Change::Mark(vec!["a.jpg".into()])).unwrap();
        // Another window adds b; this one, knowing only a, adds c.
        apply(&store, f, &Change::Mark(vec!["b.jpg".into()])).unwrap();
        let after = apply(&store, f, &Change::Mark(vec!["c.jpg".into()])).unwrap();
        assert_eq!(after, set(&["a.jpg", "b.jpg", "c.jpg"]));
        let after = apply(&store, f, &Change::Unmark(vec!["b.jpg".into()])).unwrap();
        assert_eq!(after, set(&["a.jpg", "c.jpg"]));
    }

    #[test]
    fn a_broken_file_keeps_its_valid_names() {
        let store = tmp("broken");
        let f = Path::new("/p/shoot");
        let text = format!("{HEADER}/p/shoot\na.jpg\n\nsub/b.jpg\nc\\d.jpg\ne\0.jpg\n..\nf.jpg\n");
        std::fs::write(file_of(&store, f), text).unwrap();
        assert_eq!(load(&store, f).unwrap(), set(&["a.jpg", "f.jpg"]));
        // Not UTF-8: the valid lines still count.
        let mut b = format!("{HEADER}/p/shoot\ng.jpg\n").into_bytes();
        b.extend(b"\xff\xfe\nh.jpg\n");
        std::fs::write(file_of(&store, f), b).unwrap();
        assert_eq!(load(&store, f).unwrap(), set(&["g.jpg", "h.jpg"]));
    }

    #[test]
    fn a_file_of_another_folder_or_without_header_counts_as_none() {
        let store = tmp("collide");
        let f = Path::new("/p/shoot");
        std::fs::write(file_of(&store, f), format!("{HEADER}/other\na.jpg\n")).unwrap();
        assert!(load(&store, f).unwrap().is_empty());
        std::fs::write(file_of(&store, f), "a.jpg\n").unwrap();
        assert!(load(&store, f).unwrap().is_empty());
        // A change then starts from nothing and takes the file over.
        let after = apply(&store, f, &Change::Mark(vec!["b.jpg".into()])).unwrap();
        assert_eq!(after, set(&["b.jpg"]));
        assert_eq!(load(&store, f).unwrap(), set(&["b.jpg"]));
    }

    #[test]
    fn a_missing_file_is_no_marks_and_an_unwritable_store_an_error() {
        let store = tmp("missing");
        assert!(load(&store, Path::new("/p/none")).unwrap().is_empty());
        let file = store.join("not-a-folder");
        std::fs::write(&file, "x").unwrap();
        assert!(apply(&file, Path::new("/p"), &Change::Mark(vec!["a.jpg".into()])).is_err());
    }

    #[test]
    fn names_that_are_no_plain_file_names_are_not_stored() {
        assert!(storable("a.jpg") && storable("Ünïcödé 1.jpg"));
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "a\0",
            "a\nb",
            "bad\u{FFFD}.jpg",
        ] {
            assert!(!storable(bad), "{bad:?}");
        }
    }

    #[test]
    fn copies_keep_the_bytes_take_a_free_name_and_never_replace() {
        let d = tmp("copy");
        let (src, dst) = (d.join("src"), d.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.jpg"), b"AAAA").unwrap();
        std::fs::write(src.join("b"), b"BB").unwrap();
        std::fs::write(dst.join("a.jpg"), b"old").unwrap();
        std::fs::write(dst.join("a (2).jpg"), b"old2").unwrap();
        let files = vec![src.join("a.jpg"), src.join("b"), src.join("gone.jpg")];
        let seen = std::sync::Mutex::new(Vec::new());
        let r = copy_files(
            &files,
            &dst,
            |i| seen.lock().unwrap().push(i),
            &Default::default(),
        );
        assert_eq!(r.done, [dst.join("a (3).jpg"), dst.join("b")]);
        assert_eq!(std::fs::read(dst.join("a (3).jpg")).unwrap(), b"AAAA");
        assert_eq!(
            std::fs::read(dst.join("a.jpg")).unwrap(),
            b"old",
            "never replaced"
        );
        assert_eq!(
            std::fs::read(src.join("a.jpg")).unwrap(),
            b"AAAA",
            "source unchanged"
        );
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].0, src.join("gone.jpg"));
        assert_eq!(*seen.lock().unwrap(), [1, 2, 3]);
        assert!(!r.cancelled);
    }

    #[test]
    fn a_cancelled_copy_leaves_nothing_behind() {
        let d = tmp("cancel");
        std::fs::write(d.join("a.jpg"), vec![7u8; 3 << 20]).unwrap();
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let r = copy_files(&[d.join("a.jpg")], &out, |_| {}, &cancel);
        assert!(r.cancelled && r.done.is_empty());
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
        // Cancelled while copying: the partial file goes.
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let c = &cancel;
        let r = std::thread::scope(|s| {
            s.spawn(|| c.store(true, std::sync::atomic::Ordering::Relaxed));
            copy_one(&d.join("a.jpg"), &out, c)
        })
        .unwrap();
        if r.is_none() {
            assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
        }
    }

    #[test]
    fn a_target_that_cannot_be_written_fails_each_file() {
        let d = tmp("nowrite");
        std::fs::write(d.join("a.jpg"), b"A").unwrap();
        let r = copy_files(
            &[d.join("a.jpg")],
            &d.join("missing"),
            |_| {},
            &Default::default(),
        );
        assert!(r.done.is_empty() && r.failed.len() == 1);
    }
}
