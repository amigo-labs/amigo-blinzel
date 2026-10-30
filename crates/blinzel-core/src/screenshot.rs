//! Where a screenshot goes besides the clipboard, which always gets it
//! (settings `screenshot_save`, `screenshot_open`, `screenshot_edit`),
//! independent of the platform. The shells capture, put the pixels on the
//! clipboard and write the file chosen here.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::settings::Settings;

/// Shots kept in the cache (opened, not saved) live this long.
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// The folder the file goes into: `folder` when the shot is saved,
/// `cache` when it is only opened (the viewer and the editor need a file),
/// none when it only goes to the clipboard.
pub fn destination(s: &Settings, folder: &Path, cache: &Path) -> Option<PathBuf> {
    if s.screenshot_save {
        Some(folder.to_path_buf())
    } else if s.screenshot_open || s.screenshot_edit {
        Some(cache.to_path_buf())
    } else {
        None
    }
}

/// The cache folder for shots that are opened but not saved.
pub fn cache_folder(cache: &Path) -> PathBuf {
    cache.join("screenshots")
}

/// `stem.png`, or `stem (2).png` … if that exists.
pub fn free_name(folder: &Path, stem: &str) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => folder.join(format!("{stem}.png")),
            n => folder.join(format!("{stem} ({n}).png")),
        })
        .find(|p| !p.exists())
        .expect("a free name")
}

/// Writes `png` as `stem.png` (or a free variant) into `folder`, through a
/// temporary file so a half-written shot never shows up.
pub fn write(folder: &Path, stem: &str, png: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let path = free_name(folder, stem);
    let tmp = path.with_extension("blinzel-tmp");
    std::fs::write(&tmp, png)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", path.display())
        })?;
    Ok(path)
}

/// Deletes the PNGs in `folder` older than `max_age` (the cached shots).
pub fn sweep(folder: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let path = e.path();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= max_age);
        if old && path.extension().is_some_and(|x| x == "png") {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(save: bool, open: bool, edit: bool) -> Settings {
        Settings {
            screenshot_save: save,
            screenshot_open: open,
            screenshot_edit: edit,
            ..Settings::default()
        }
    }

    #[test]
    fn destination_follows_the_settings() {
        let (folder, cache) = (Path::new("shots"), Path::new("cache"));
        let d = |save, open, edit| destination(&settings(save, open, edit), folder, cache);
        assert_eq!(d(true, false, false).as_deref(), Some(folder));
        assert_eq!(d(true, true, true).as_deref(), Some(folder));
        assert_eq!(d(false, true, false).as_deref(), Some(cache));
        assert_eq!(d(false, false, true).as_deref(), Some(cache));
        assert_eq!(d(false, false, false), None);
    }

    #[test]
    fn writes_free_names_and_sweeps_old_shots() {
        let dir = std::env::temp_dir().join(format!("blinzel-shots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = write(&dir, "Shot", b"1").unwrap();
        let b = write(&dir, "Shot", b"2").unwrap();
        assert_eq!(a.file_name().unwrap(), "Shot.png");
        assert_eq!(b.file_name().unwrap(), "Shot (2).png");
        assert_eq!(std::fs::read(&b).unwrap(), b"2");
        assert!(!dir.join("Shot.blinzel-tmp").exists());
        std::fs::write(dir.join("keep.txt"), b"").unwrap();
        sweep(&dir, Duration::from_secs(3600));
        assert!(a.exists() && b.exists());
        sweep(&dir, Duration::ZERO);
        assert!(!a.exists() && !b.exists());
        assert!(dir.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
