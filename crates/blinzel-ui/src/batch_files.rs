//! The batch dialog's file list: files and folders dropped onto it, their
//! headers read in the background, and the name each copy will get
//! ([`plan::targets`], the same names the run writes). Drawn as a drop
//! zone while empty.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use blinzel_core::batch::Job;
use blinzel_core::batch::plan::{self, Item, SourceInfo};
use blinzel_core::format::has_image_extension;
use blinzel_core::geom::Rect;
use blinzel_core::natsort::SortKey;
use blinzel_i18n::{Msg, tr, trf};

use crate::backend::Canvas;
use crate::widgets::{self, Painter};

/// A file's header, read in the background.
pub type Inspected = (PathBuf, Result<SourceInfo, String>);

/// Height of one row (logical pixels).
pub const ROW: f64 = 22.0;

/// A dropped file and its header (None: still being read).
struct Row {
    path: PathBuf,
    info: Option<Result<SourceInfo, String>>,
}

/// Output paths for the rows as they are now.
struct Targets {
    job: Job,
    generation: u64,
    /// Per row; None while its header is read or when it is unreadable.
    paths: Vec<Option<Result<PathBuf, String>>>,
}

#[derive(Default)]
pub struct BatchFiles {
    rows: Vec<Row>,
    /// Bumped whenever the rows or their headers change.
    generation: u64,
    /// First row shown.
    scroll: usize,
    inspect: Vec<Receiver<Inspected>>,
    targets: Option<Targets>,
    /// The list's area and how many rows fit (last frame).
    area: Rect,
    visible: usize,
}

impl BatchFiles {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Dropped files and folders (a folder adds its images, not those of
    /// its subfolders). Returns the files that are new to the list; files
    /// already listed and non-images are left out.
    pub fn add(&mut self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut known: HashSet<PathBuf> = self.rows.iter().map(|r| r.path.clone()).collect();
        let mut new = Vec::new();
        for p in paths {
            let files = if p.is_dir() {
                images_in(&p)
            } else if p
                .file_name()
                .is_some_and(|n| has_image_extension(&n.to_string_lossy()))
            {
                vec![p]
            } else {
                vec![]
            };
            new.extend(files.into_iter().filter(|f| known.insert(f.clone())));
        }
        self.rows.extend(new.iter().map(|p| Row {
            path: p.clone(),
            info: None,
        }));
        if !new.is_empty() {
            self.generation += 1;
        }
        new
    }

    /// Header results for files [`BatchFiles::add`] returned.
    pub fn inspecting(&mut self, rx: Receiver<Inspected>) {
        self.inspect.push(rx);
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.rows.len() {
            self.rows.remove(i);
            self.generation += 1;
        }
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.inspect.clear();
        self.generation += 1;
        self.scroll = 0;
    }

    /// Takes the header results that arrived. True if any did.
    pub fn poll(&mut self) -> bool {
        let mut results = Vec::new();
        self.inspect.retain(|rx| {
            loop {
                match rx.try_recv() {
                    Ok(r) => results.push(r),
                    Err(TryRecvError::Empty) => return true,
                    Err(TryRecvError::Disconnected) => return false,
                }
            }
        });
        let mut any = false;
        for (path, info) in results {
            if let Some(row) = self
                .rows
                .iter_mut()
                .find(|r| r.info.is_none() && r.path == path)
            {
                row.info = Some(info);
                self.generation += 1;
                any = true;
            }
        }
        any
    }

    /// Every header read and at least one file to resize.
    pub fn ready(&self) -> bool {
        self.rows.iter().all(|r| r.info.is_some())
            && self.rows.iter().any(|r| matches!(r.info, Some(Ok(_))))
    }

    /// Where each row's copy goes with `job`. Numbers count the readable
    /// files in list order.
    fn targets(&mut self, job: Job) -> &[Option<Result<PathBuf, String>>] {
        let fresh = self
            .targets
            .as_ref()
            .is_some_and(|t| t.generation == self.generation && t.job == job);
        if !fresh {
            let ready: Vec<(&Path, &SourceInfo)> = self
                .rows
                .iter()
                .filter_map(|r| match &r.info {
                    Some(Ok(i)) => Some((r.path.as_path(), i)),
                    _ => None,
                })
                .collect();
            let mut planned = plan::targets(&job, &ready).into_iter();
            let paths = self
                .rows
                .iter()
                .map(|r| {
                    matches!(r.info, Some(Ok(_)))
                        .then(|| planned.next())
                        .flatten()
                })
                .collect();
            self.targets = Some(Targets {
                job,
                generation: self.generation,
                paths,
            });
        }
        self.targets.as_ref().map_or(&[], |t| &t.paths[..])
    }

    /// The readable files with their planned outputs, once every header
    /// is read; None while some are still read or a name cannot be made.
    pub fn items(&mut self, job: Job) -> Option<Vec<Item>> {
        if !self.ready() {
            return None;
        }
        let targets = self.targets(job).to_vec();
        let mut items = Vec::new();
        for (row, target) in self.rows.iter().zip(targets) {
            match target {
                Some(Ok(t)) => items.push(Item {
                    source: row.path.clone(),
                    target: Some(t),
                }),
                Some(Err(_)) => return None,
                None => {}
            }
        }
        Some(items)
    }

    /// The rows as (old name, new name or state, is an error); without a
    /// complete job there are no new names.
    pub fn texts(&mut self, job: Option<Job>) -> Vec<(String, String, bool)> {
        let targets = job.map(|j| self.targets(j).to_vec());
        self.rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let old = file_name(&r.path);
                let target = targets.as_ref().and_then(|t| t[i].as_ref());
                let (new, bad) = match (&r.info, target) {
                    (None, _) => (tr(Msg::BatchReading).to_string(), false),
                    (Some(Err(e)), _) => (trf(Msg::BatchUnreadable, &[("reason", e)]), true),
                    (Some(Ok(_)), Some(Ok(p))) => (file_name(p), false),
                    (Some(Ok(_)), Some(Err(e))) => (e.clone(), true),
                    (Some(Ok(_)), None) => ("–".into(), false),
                };
                (old, new, bad)
            })
            .collect()
    }

    /// The wheel over the list scrolls it (`delta` in 1/120 notches).
    /// True if it moved.
    pub fn wheel(&mut self, p: (f64, f64), delta: i16) -> bool {
        if !self.area.contains(p) {
            return false;
        }
        let max = self.rows.len().saturating_sub(self.visible);
        let step = (delta as f64 / 120.0 * 3.0).round() as isize;
        let scroll = (self.scroll as isize - step).clamp(0, max as isize) as usize;
        let moved = scroll != self.scroll;
        self.scroll = scroll;
        moved
    }

    /// Draws the list in `r`: a drop zone while empty, else one row per
    /// file with its new name. Returns the remove buttons by row.
    pub fn draw(
        &mut self,
        p: &mut Painter<impl Canvas>,
        r: Rect,
        job: Option<Job>,
        hover_remove: Option<usize>,
    ) -> Vec<(Rect, usize)> {
        self.area = r;
        p.fill(r, 8.0, widgets::CONTROL);
        if self.rows.is_empty() {
            self.visible = 0;
            p.outline(r, 1.0, 8.0, widgets::BORDER);
            p.text_centred(tr(Msg::BatchDrop), 14.0, &r, widgets::DIM);
            return vec![];
        }
        let rh = p.px(ROW);
        let pad = p.px(6.0);
        self.visible = (((r.h - 2.0 * pad) / rh).floor() as usize).max(1);
        self.scroll = self
            .scroll
            .min(self.rows.len().saturating_sub(self.visible));
        let texts = self.texts(job);
        let x_w = p.px(24.0);
        let col = (r.w - 2.0 * pad - x_w - p.px(24.0)) / 2.0;
        let mut removes = Vec::new();
        for (k, (i, (old, new, bad))) in texts
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.visible)
            .enumerate()
        {
            let y = r.y + pad + k as f64 * rh;
            let row = Rect::new(r.x + pad, y, r.w - 2.0 * pad, rh);
            let old = elide(p, old, 12.5, col);
            p.text_in(&old, 12.5, &row, p.px(4.0), widgets::DIM);
            let arrow = Rect::new(row.x + col + p.px(4.0), y, p.px(20.0), rh);
            p.text_centred("→", 12.5, &arrow, widgets::FAINT);
            let new_r = Rect::new(arrow.right(), y, col, rh);
            let new = elide(p, new, 12.5, col);
            let color = if *bad { widgets::DANGER } else { widgets::FG };
            p.text_in(&new, 12.5, &new_r, 0.0, color);
            let x = Rect::new(row.right() - x_w, y + p.px(1.0), x_w, rh - p.px(2.0));
            if hover_remove == Some(i) {
                p.fill(x, 4.0, widgets::CONTROL_HOVER);
            }
            p.text_centred("×", 14.0, &x, widgets::DIM);
            removes.push((x, i));
        }
        if self.rows.len() > self.visible {
            // Where the shown rows are in the whole list.
            let track = r.h - 2.0 * pad;
            let n = self.rows.len() as f64;
            let bar = Rect::new(
                r.right() - p.px(4.0),
                r.y + pad + track * self.scroll as f64 / n,
                p.px(2.0),
                (track * self.visible as f64 / n).max(p.px(12.0)),
            );
            p.fill(bar, 1.0, widgets::FAINT);
        }
        removes
    }

    #[cfg(test)]
    pub(crate) fn push_ready(&mut self, path: PathBuf, info: SourceInfo) {
        self.rows.push(Row {
            path,
            info: Some(Ok(info)),
        });
        self.generation += 1;
    }
}

/// The images directly in `dir` (not in its subfolders), in natural
/// order.
fn images_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<(SortKey, PathBuf)> = read
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| has_image_extension(&n.to_string_lossy()))
        })
        .map(|p| (SortKey::new(&file_name(&p)), p))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    files.into_iter().map(|(_, p)| p).collect()
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `text` shortened in the middle to fit `max_w`; the end (numbers,
/// extension) stays readable.
fn elide(p: &mut Painter<impl Canvas>, text: &str, size: f64, max_w: f64) -> String {
    if p.text_size(text, size).0 <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = chars.len().saturating_sub(10);
    let tail: String = chars[cut..].iter().collect();
    let (mut lo, mut hi) = (0, cut);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().collect::<String>() + "…" + &tail;
        if p.text_size(&s, size).0 <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…" + &tail
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use blinzel_core::batch::naming::Stamp;
    use blinzel_core::format::Format;
    use blinzel_core::geom::Orientation;

    pub(crate) fn info() -> SourceInfo {
        SourceInfo {
            format: Format::Heif,
            stored: (4032, 3024),
            orientation: Orientation::NORMAL,
            date: Some(Stamp {
                year: 2024,
                month: 5,
                day: 17,
                hour: 9,
                minute: 4,
                second: 30,
            }),
            alpha: false,
        }
    }

    #[test]
    fn dropped_files_and_folders_fill_the_list() {
        let dir = std::env::temp_dir().join(format!("blinzel-batch-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["img10.jpg", "img9.JPG", "notes.txt", "sub/deep.jpg"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let (a, b) = (dir.join("img9.JPG"), dir.join("img10.jpg"));

        let mut f = BatchFiles::default();
        let new = f.add(vec![dir.join("notes.txt"), dir.clone(), a.clone()]);
        assert_eq!(new, [a.clone(), b.clone()], "natural order, no subfolders");
        assert!(f.add(vec![b.clone()]).is_empty(), "listed already");
        let job = Job::default();
        assert_eq!(f.texts(Some(job.clone()))[0].1, tr(Msg::BatchReading));
        assert!(!f.ready() && f.items(job.clone()).is_none());

        let (tx, rx) = std::sync::mpsc::channel();
        f.inspecting(rx);
        tx.send((b.clone(), Err("broken".into()))).unwrap();
        tx.send((a.clone(), Ok(info()))).unwrap();
        drop(tx);
        assert!(f.poll());
        assert!(f.inspect.is_empty(), "finished reads are dropped");
        let texts = f.texts(Some(job.clone()));
        assert_eq!(texts[0].1, "img9.jpg");
        assert!(texts[1].2, "unreadable file shown as error");
        assert_eq!(f.texts(None)[0].1, "–", "no names without a job");
        assert_eq!(
            f.items(job).unwrap(),
            [Item {
                source: a,
                target: Some(dir.join("resized/img9.jpg")),
            }],
            "unreadable files are left out"
        );
        f.remove(1);
        assert_eq!(f.len(), 1);
        f.clear();
        assert!(f.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
