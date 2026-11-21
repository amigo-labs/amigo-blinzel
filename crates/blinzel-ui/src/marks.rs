//! The lasting marks of the open folder (`docs/specs/picks.md`): read and
//! stored by a worker thread ([`blinzel_core::picks`]) so that neither a
//! frame nor a key waits for the disk. Results wake the UI thread through
//! [`Wake::Thumbs`], whose handler polls this too.
//!
//! The marked names themselves live in [`crate::browse::Browse::marks`],
//! which draws them; this holds which folder they belong to, whether they
//! are read yet, and the changes made before that.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use blinzel_core::picks::{self, Change};

use crate::backend::{Services, Wake};

/// The worker thread's name.
pub const THREAD: &str = "blinzel-marks";

/// Held by a test, it keeps every read and write of the marks back
/// (`docs/specs/picks.md` AC-10, AC-13). Harness tests run one at a time.
#[cfg(test)]
pub(crate) static HOLD: std::sync::Mutex<()> = std::sync::Mutex::new(());

enum Job {
    Load(PathBuf),
    Apply(PathBuf, Change),
}

enum Done {
    Loaded(PathBuf, std::io::Result<BTreeSet<String>>),
    Saved(PathBuf, std::io::Result<BTreeSet<String>>),
}

/// What [`Marks::poll`] brought.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Polled {
    /// The marked names changed.
    pub changed: bool,
    /// Storing failed for the first time this session: the reason.
    pub error: Option<String>,
}

/// A change waiting for the folder's marks to be read.
enum Pending {
    Change(Change),
    /// F: marks if unmarked, unmarks if marked, decided once they are read.
    Toggle(String),
}

#[derive(Default)]
pub struct Marks {
    /// The per-user config folder the files go to; `None`: kept in memory.
    store: Option<PathBuf>,
    folder: Option<PathBuf>,
    loaded: bool,
    /// Changes made before the folder's marks were read; applied after.
    queued: Vec<Pending>,
    /// Changes sent and not yet stored.
    in_flight: usize,
    worker: Option<(Sender<Job>, Receiver<Done>)>,
    error_shown: bool,
}

impl Marks {
    pub fn new(store: Option<PathBuf>) -> Marks {
        Marks {
            store,
            ..Marks::default()
        }
    }

    /// The folder whose marks these are.
    pub fn folder(&self) -> Option<&PathBuf> {
        self.folder.as_ref()
    }

    /// The folder's marks are read (or there is nothing to read).
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// Reading or storing is under way.
    pub fn busy(&self) -> bool {
        (self.folder.is_some() && !self.loaded) || self.in_flight > 0
    }

    /// Switches to `folder`'s marks: `names` empties until they are read.
    pub fn open<S: Services>(
        &mut self,
        folder: PathBuf,
        names: &mut BTreeSet<String>,
        services: &Arc<S>,
    ) {
        if self.folder.as_ref() == Some(&folder) {
            return;
        }
        names.clear();
        self.queued.clear();
        self.folder = Some(folder.clone());
        self.loaded = false;
        if self.store.is_none() {
            self.loaded = true;
            return;
        }
        self.send(services, Job::Load(folder));
    }

    /// Applies `change` to `names` and stores it; before the folder's
    /// marks are read it waits and applies then, in order.
    pub fn change<S: Services>(
        &mut self,
        change: Change,
        names: &mut BTreeSet<String>,
        services: &Arc<S>,
    ) {
        if self.folder.is_none() {
            return;
        }
        if !self.loaded {
            self.queued.push(Pending::Change(change));
            return;
        }
        apply(&change, names);
        self.store_change(change, services);
    }

    /// Marks `name` if it is not, unmarks it if it is (once read).
    pub fn toggle<S: Services>(
        &mut self,
        name: String,
        names: &mut BTreeSet<String>,
        services: &Arc<S>,
    ) {
        if self.folder.is_none() {
            return;
        }
        if !self.loaded {
            self.queued.push(Pending::Toggle(name));
            return;
        }
        let c = toggled(name, names);
        self.change(c, names, services);
    }

    fn store_change<S: Services>(&mut self, change: Change, services: &Arc<S>) {
        if let (Some(_), Some(folder)) = (&self.store, &self.folder) {
            self.in_flight += 1;
            let job = Job::Apply(folder.clone(), change);
            self.send(services, job);
        }
    }

    /// Takes what the worker finished.
    pub fn poll<S: Services>(&mut self, names: &mut BTreeSet<String>, services: &Arc<S>) -> Polled {
        let mut out = Polled::default();
        let done: Vec<Done> = match &self.worker {
            Some((_, rx)) => rx.try_iter().collect(),
            None => return out,
        };
        for d in done {
            match d {
                Done::Loaded(folder, r) if Some(&folder) == self.folder.as_ref() => {
                    *names = r.unwrap_or_default();
                    self.loaded = true;
                    for p in std::mem::take(&mut self.queued) {
                        let c = match p {
                            Pending::Change(c) => c,
                            Pending::Toggle(n) => toggled(n, names),
                        };
                        apply(&c, names);
                        self.store_change(c, services);
                    }
                    out.changed = true;
                }
                Done::Saved(folder, r) => {
                    let current = Some(&folder) == self.folder.as_ref();
                    if current {
                        self.in_flight = self.in_flight.saturating_sub(1);
                    }
                    match r {
                        // Another window's marks show once ours are stored.
                        Ok(stored) if current && self.in_flight == 0 && *names != stored => {
                            *names = stored;
                            out.changed = true;
                        }
                        Ok(_) => {}
                        Err(e) if !self.error_shown => {
                            self.error_shown = true;
                            out.error = Some(e.to_string());
                        }
                        Err(_) => {}
                    }
                }
                Done::Loaded(..) => {}
            }
        }
        out
    }

    fn send<S: Services>(&mut self, services: &Arc<S>, job: Job) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let (tx, _) = self.worker.get_or_insert_with(|| {
            let (jobs, jobs_rx) = channel::<Job>();
            let (done_tx, done) = channel();
            let services = services.clone();
            let _ = std::thread::Builder::new()
                .name(THREAD.into())
                .spawn(move || {
                    services.tune_thread(false);
                    for job in jobs_rx {
                        #[cfg(test)]
                        drop(HOLD.lock().unwrap_or_else(|e| e.into_inner()));
                        let d = match job {
                            Job::Load(f) => {
                                let r = picks::load(&store, &f);
                                Done::Loaded(f, r)
                            }
                            Job::Apply(f, c) => {
                                let r = picks::apply(&store, &f, &c);
                                Done::Saved(f, r)
                            }
                        };
                        if done_tx.send(d).is_err() {
                            return;
                        }
                        services.wake(Wake::Thumbs);
                    }
                });
            (jobs, done)
        });
        let _ = tx.send(job);
    }
}

fn toggled(name: String, names: &BTreeSet<String>) -> Change {
    if names.contains(&name) {
        Change::Unmark(vec![name])
    } else {
        Change::Mark(vec![name])
    }
}

fn apply(change: &Change, names: &mut BTreeSet<String>) {
    match change {
        Change::Mark(n) => names.extend(n.iter().cloned()),
        Change::Unmark(n) => names.retain(|x| !n.contains(x)),
        Change::Clear => names.clear(),
    }
}
