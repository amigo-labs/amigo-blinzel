//! Two-stage priority scheduler for prefetching: I/O workers read files, CPU
//! workers decode them. The navigation replaces the *wanted* set on every
//! move; queued jobs that are no longer wanted vanish, running ones are
//! cancelled, priorities follow the browsing direction.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

pub type Key = u64;
/// Lower runs sooner.
pub type Priority = u32;
/// Independent job groups (e.g. prefetch vs. clipboard); `set_wanted` only
/// touches its own class.
pub type Class = u8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobSpec {
    pub key: Key,
    pub priority: Priority,
    pub class: Class,
}

/// Cooperative cancellation flag handed to every job.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Which pool a worker thread belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Io,
    Cpu,
}

type IoFn<T> = dyn Fn(Key, &CancelToken) -> Option<T> + Send + Sync;
type CpuFn<T> = dyn Fn(Key, T, &CancelToken) + Send + Sync;
type StartFn = dyn Fn(Role, usize) + Send + Sync;

enum Stage<T> {
    IoQueued,
    IoRunning,
    CpuQueued(T),
    CpuRunning,
}

struct Job<T> {
    spec: JobSpec,
    token: CancelToken,
    stage: Stage<T>,
}

struct State<T> {
    jobs: HashMap<Key, Job<T>>,
    wanted: HashMap<Class, HashMap<Key, Priority>>,
    completed: HashSet<Key>,
    shutdown: bool,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    io_cv: Condvar,
    cpu_cv: Condvar,
    io: Box<IoFn<T>>,
    cpu: Box<CpuFn<T>>,
}

/// Thread counts and a hook to tune worker threads (priority, QoS).
pub struct Config {
    pub io_threads: usize,
    pub cpu_threads: usize,
    pub on_thread_start: Option<Box<StartFn>>,
}

pub struct Scheduler<T: Send + 'static> {
    shared: Arc<Shared<T>>,
    threads: Vec<JoinHandle<()>>,
}

impl<T: Send + 'static> Scheduler<T> {
    pub fn new(
        config: Config,
        io: impl Fn(Key, &CancelToken) -> Option<T> + Send + Sync + 'static,
        cpu: impl Fn(Key, T, &CancelToken) + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                jobs: HashMap::new(),
                wanted: HashMap::new(),
                completed: HashSet::new(),
                shutdown: false,
            }),
            io_cv: Condvar::new(),
            cpu_cv: Condvar::new(),
            io: Box::new(io),
            cpu: Box::new(cpu),
        });
        let on_start: Option<Arc<StartFn>> = config.on_thread_start.map(Arc::from);
        let mut threads = Vec::new();
        for (role, n) in [
            (Role::Io, config.io_threads.max(1)),
            (Role::Cpu, config.cpu_threads.max(1)),
        ] {
            for i in 0..n {
                let shared = shared.clone();
                let on_start = on_start.clone();
                let name = format!(
                    "blinzel-{}-{i}",
                    if role == Role::Io { "io" } else { "cpu" }
                );
                threads.push(
                    std::thread::Builder::new()
                        .name(name)
                        .spawn(move || {
                            if let Some(f) = on_start {
                                f(role, i);
                            }
                            worker(&shared, role);
                        })
                        .expect("spawn worker"),
                );
            }
        }
        Scheduler { shared, threads }
    }

    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.shared.state.lock().unwrap()
    }

    /// Replaces the wanted jobs of `specs`' class. Jobs of that class not
    /// listed are dropped (queued) or cancelled (running); completed keys
    /// are skipped until [`forget`](Self::forget) is called for them.
    pub fn set_wanted(&self, class: Class, specs: &[JobSpec]) {
        let mut st = self.lock();
        let wanted: HashMap<Key, Priority> = specs.iter().map(|s| (s.key, s.priority)).collect();
        st.jobs.retain(|key, job| {
            if job.spec.class != class || wanted.contains_key(key) {
                return true;
            }
            match job.stage {
                Stage::IoQueued | Stage::CpuQueued(_) => false,
                Stage::IoRunning | Stage::CpuRunning => {
                    job.token.cancel();
                    true
                }
            }
        });
        for s in specs {
            let s = JobSpec { class, ..*s };
            if st.completed.contains(&s.key) {
                continue;
            }
            match st.jobs.get_mut(&s.key) {
                Some(job) => job.spec.priority = s.priority,
                None => {
                    st.jobs.insert(
                        s.key,
                        Job {
                            spec: s,
                            token: CancelToken::default(),
                            stage: Stage::IoQueued,
                        },
                    );
                }
            }
        }
        st.wanted.insert(class, wanted);
        drop(st);
        self.shared.io_cv.notify_all();
        self.shared.cpu_cv.notify_all();
    }

    /// Adds one job without touching others of its class.
    pub fn submit(&self, spec: JobSpec) {
        let mut st = self.lock();
        st.completed.remove(&spec.key);
        st.wanted
            .entry(spec.class)
            .or_default()
            .insert(spec.key, spec.priority);
        match st.jobs.get_mut(&spec.key) {
            Some(job) => job.spec.priority = job.spec.priority.min(spec.priority),
            None => {
                st.jobs.insert(
                    spec.key,
                    Job {
                        spec,
                        token: CancelToken::default(),
                        stage: Stage::IoQueued,
                    },
                );
            }
        }
        drop(st);
        self.shared.io_cv.notify_one();
    }

    /// Allows keys to be scheduled again (their results were dropped).
    pub fn forget(&self, keys: impl IntoIterator<Item = Key>) {
        let mut st = self.lock();
        for k in keys {
            st.completed.remove(&k);
        }
    }

    /// Number of queued or running jobs.
    pub fn pending(&self) -> usize {
        self.lock().jobs.len()
    }

    pub fn is_completed(&self, key: Key) -> bool {
        self.lock().completed.contains(&key)
    }
}

impl<T: Send + 'static> Drop for Scheduler<T> {
    fn drop(&mut self) {
        {
            let mut st = self.lock();
            st.shutdown = true;
            for job in st.jobs.values() {
                job.token.cancel();
            }
        }
        self.shared.io_cv.notify_all();
        self.shared.cpu_cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn worker<T: Send + 'static>(shared: &Shared<T>, role: Role) {
    let cv = if role == Role::Io {
        &shared.io_cv
    } else {
        &shared.cpu_cv
    };
    let mut st = shared.state.lock().unwrap();
    loop {
        if st.shutdown {
            return;
        }
        // Most urgent runnable job for this pool.
        let next = st
            .jobs
            .iter()
            .filter(|(_, j)| match role {
                Role::Io => matches!(j.stage, Stage::IoQueued),
                Role::Cpu => matches!(j.stage, Stage::CpuQueued(_)),
            })
            .min_by_key(|(k, j)| (j.spec.priority, **k))
            .map(|(k, _)| *k);
        let Some(key) = next else {
            st = cv.wait(st).unwrap();
            continue;
        };
        let job = st.jobs.get_mut(&key).unwrap();
        let token = job.token.clone();
        match role {
            Role::Io => {
                job.stage = Stage::IoRunning;
                drop(st);
                let out = (shared.io)(key, &token);
                st = shared.state.lock().unwrap();
                match out {
                    Some(v) if !token.is_cancelled() => {
                        if let Some(job) = st.jobs.get_mut(&key) {
                            job.stage = Stage::CpuQueued(v);
                        }
                        shared.cpu_cv.notify_one();
                    }
                    _ => finish(&mut st, key, false),
                }
            }
            Role::Cpu => {
                let Stage::CpuQueued(input) = std::mem::replace(&mut job.stage, Stage::CpuRunning)
                else {
                    unreachable!()
                };
                drop(st);
                (shared.cpu)(key, input, &token);
                st = shared.state.lock().unwrap();
                finish(&mut st, key, !token.is_cancelled());
                // A cancelled job may have been re-queued; wake the I/O side.
                shared.io_cv.notify_one();
            }
        }
    }
}

/// Removes a finished job; a cancelled one that is wanted again is queued
/// anew.
fn finish<T>(st: &mut State<T>, key: Key, success: bool) {
    let Some(job) = st.jobs.remove(&key) else {
        return;
    };
    if success {
        st.completed.insert(key);
        return;
    }
    if let Some(&priority) = st.wanted.get(&job.spec.class).and_then(|w| w.get(&key))
        && job.token.is_cancelled()
    {
        let spec = JobSpec {
            priority,
            ..job.spec
        };
        st.jobs.insert(
            key,
            Job {
                spec,
                token: CancelToken::default(),
                stage: Stage::IoQueued,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn spec(key: Key, priority: Priority) -> JobSpec {
        JobSpec {
            key,
            priority,
            class: 0,
        }
    }

    fn config() -> Config {
        Config {
            io_threads: 1,
            cpu_threads: 1,
            on_thread_start: None,
        }
    }

    /// A scheduler whose I/O blocks until the test opens the gate.
    type Gate = Arc<(Mutex<bool>, Condvar)>;

    fn gated() -> (Scheduler<Key>, Gate, mpsc::Receiver<Key>) {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let g = gate.clone();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let s = Scheduler::new(
            config(),
            move |k, _| {
                let (m, cv) = &*g;
                let mut open = m.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                Some(k)
            },
            move |k, _, _| tx.lock().unwrap().send(k).unwrap(),
        );
        (s, gate, rx)
    }

    fn open(gate: &Gate) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    fn recv_all(rx: &mpsc::Receiver<Key>, n: usize) -> Vec<Key> {
        (0..n)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect()
    }

    #[test]
    fn runs_by_priority() {
        let (s, gate, rx) = gated();
        // Key 100 grabs the only I/O thread and waits at the gate.
        s.submit(spec(100, 0));
        std::thread::sleep(Duration::from_millis(50));
        s.set_wanted(1, &[spec(3, 30), spec(1, 10), spec(2, 20)]);
        open(&gate);
        assert_eq!(recv_all(&rx, 4), [100, 1, 2, 3]);
        assert!(s.is_completed(2));
    }

    #[test]
    fn replacing_wanted_drops_and_reprioritizes() {
        let (s, gate, rx) = gated();
        s.submit(JobSpec {
            key: 100,
            priority: 0,
            class: 9,
        });
        std::thread::sleep(Duration::from_millis(50));
        s.set_wanted(0, &[spec(1, 1), spec(2, 2), spec(3, 3)]);
        // Direction change: 3 first now, 1 no longer wanted.
        s.set_wanted(0, &[spec(3, 1), spec(2, 2)]);
        open(&gate);
        assert_eq!(recv_all(&rx, 3), [100, 3, 2]);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn completed_jobs_are_not_repeated_until_forgotten() {
        let (s, gate, rx) = gated();
        open(&gate);
        s.set_wanted(0, &[spec(7, 1)]);
        assert_eq!(recv_all(&rx, 1), [7]);
        while s.pending() > 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        s.set_wanted(0, &[spec(7, 1)]);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        s.forget([7]);
        s.set_wanted(0, &[spec(7, 1)]);
        assert_eq!(recv_all(&rx, 1), [7]);
    }

    #[test]
    fn running_jobs_get_cancelled() {
        let (started_tx, started_rx) = mpsc::channel();
        let started_tx = Mutex::new(started_tx);
        let (done_tx, done_rx) = mpsc::channel();
        let done_tx = Mutex::new(done_tx);
        let s = Scheduler::new(
            config(),
            move |k, t: &CancelToken| {
                started_tx.lock().unwrap().send(k).unwrap();
                while !t.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                None::<Key>
            },
            move |_, _, _| {},
        );
        s.set_wanted(0, &[spec(1, 1)]);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        s.set_wanted(0, &[]);
        // The cancelled job finishes and leaves the queue.
        for _ in 0..500 {
            if s.pending() == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(s.pending(), 0);
        done_tx.lock().unwrap().send(()).unwrap();
        done_rx.recv().unwrap();
    }

    #[test]
    fn classes_are_independent() {
        let (s, gate, rx) = gated();
        s.set_wanted(1, &[spec(10, 5)]);
        s.set_wanted(2, &[spec(20, 1)]);
        s.set_wanted(1, &[]);
        open(&gate);
        let got = recv_all(&rx, 1);
        assert_eq!(got, [20]);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err() || s.pending() == 0);
    }

    #[test]
    fn drop_joins_workers() {
        let s: Scheduler<()> = Scheduler::new(
            Config {
                io_threads: 2,
                cpu_threads: 3,
                on_thread_start: Some(Box::new(|_, _| {})),
            },
            |_, _| Some(()),
            |_, _, _| {},
        );
        s.set_wanted(0, &[spec(1, 1)]);
        drop(s);
    }
}
