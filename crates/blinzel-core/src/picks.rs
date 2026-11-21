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
}
