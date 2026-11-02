//! Folder watching for the Linux and macOS shells: `notify` (inotify on
//! Linux, FSEvents on macOS). `docs/specs/platform-shells.md` AC-15.

use std::path::Path;

use notify::Watcher;

/// Watches `dir` (not its subfolders) and calls `changed` for every change
/// in it other than a mere access. The watch lasts as long as the returned
/// watcher; none if the folder cannot be watched.
pub(crate) fn folder(
    dir: &Path,
    changed: impl Fn() + Send + 'static,
) -> Option<notify::RecommendedWatcher> {
    let mut w = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
        if r.is_ok_and(|e| !e.kind.is_access()) {
            changed();
        }
    })
    .ok()?;
    w.watch(dir, notify::RecursiveMode::NonRecursive).ok()?;
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    /// Waits for a reported change, then lets the rest of its burst arrive
    /// and drops it.
    fn changed(rx: &Receiver<()>, what: &str) {
        rx.recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("{what}: no change reported"));
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
    }

    /// `docs/specs/platform-shells.md` AC-15: a file added, renamed or
    /// removed in the watched folder is reported; reading one is not, and
    /// nothing is reported once the watch is dropped.
    #[test]
    fn adding_renaming_and_removing_a_file_are_reported() {
        let dir = std::env::temp_dir().join(format!("blinzel-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let (tx, rx) = channel();
        let w = folder(&dir, move || {
            let _ = tx.send(());
        })
        .expect("the folder is watched");
        let (a, b) = (dir.join("a.png"), dir.join("b.png"));
        std::fs::write(&a, b"png").unwrap();
        changed(&rx, "added");
        std::fs::rename(&a, &b).unwrap();
        changed(&rx, "renamed");
        let _ = std::fs::read(&b).unwrap();
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "a read is no change"
        );
        std::fs::remove_file(&b).unwrap();
        changed(&rx, "removed");
        drop(w);
        std::fs::write(&a, b"png").unwrap();
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "the watch ended"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
