//! Encode job queue: a small pool of workers, per-job progress, cancel, optional verification
//! and a per-job log file written next to the output (`<name>_log.txt`).

pub mod measure;
pub mod preview;
pub mod run;
pub mod threads;

pub use run::EncodeSpec;

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pending,
    Running,
    Verifying,
    /// Finished and verified.
    Passed,
    /// Finished without verification.
    Completed,
    Failed,
    Canceled,
}

impl State {
    pub fn is_final(self) -> bool {
        matches!(self, State::Passed | State::Completed | State::Failed | State::Canceled)
    }
}

/// What the UI sees.
#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: u64,
    pub name: String,
    pub state: State,
    pub progress: f32,
    pub message: String,
    pub output: String,
    pub log_path: String,
    pub elapsed_secs: f64,
}

struct Job {
    id: u64,
    spec: EncodeSpec,
    verify: bool,
    state: State,
    progress: f32,
    message: String,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
    elapsed: f64,
    created_ms: u128,
}

impl Job {
    fn view(&self) -> JobView {
        JobView {
            id: self.id,
            name: self.spec.display_name(),
            state: self.state,
            progress: self.progress,
            message: self.message.clone(),
            output: self.spec.output.clone(),
            log_path: self.spec.log_path(),
            elapsed_secs: match (self.started, self.state.is_final()) {
                (Some(s), false) => s.elapsed().as_secs_f64(),
                _ => self.elapsed,
            },
        }
    }
}

struct Shared {
    jobs: Vec<Job>,
    next_id: u64,
    concurrency: usize,
    shutdown: bool,
}

type Notify = Arc<dyn Fn(Vec<JobView>) + Send + Sync>;

struct Inner {
    shared: Mutex<Shared>,
    wake: Condvar,
    notify: Notify,
}

#[derive(Clone)]
pub struct Queue {
    inner: Arc<Inner>,
}

/// Upper bound on worker threads; the live limit is `set_concurrency`.
const MAX_WORKERS: usize = 8;
/// The queue keeps this many jobs (finished jobs are dropped first when full).
const MAX_JOBS: usize = 99;

impl Queue {
    pub fn new(concurrency: usize, notify: Notify) -> Queue {
        let inner = Arc::new(Inner {
            shared: Mutex::new(Shared {
                jobs: Vec::new(),
                next_id: 1,
                concurrency: concurrency.clamp(1, MAX_WORKERS),
                shutdown: false,
            }),
            wake: Condvar::new(),
            notify,
        });
        for _ in 0..MAX_WORKERS {
            let inner = inner.clone();
            std::thread::spawn(move || worker(inner));
        }
        Queue { inner }
    }

    fn snapshot_locked(s: &Shared) -> Vec<JobView> {
        s.jobs.iter().map(Job::view).collect()
    }

    fn changed(&self, snap: Vec<JobView>) {
        (self.inner.notify)(snap);
    }

    pub fn snapshot(&self) -> Vec<JobView> {
        Self::snapshot_locked(&self.inner.shared.lock().unwrap())
    }

    pub fn enqueue(&self, spec: EncodeSpec, verify: bool) -> Result<u64, String> {
        let (id, snap) = {
            let mut s = self.inner.shared.lock().unwrap();
            if s.jobs.len() >= MAX_JOBS {
                // make room by dropping the oldest finished job
                match s.jobs.iter().position(|j| j.state.is_final()) {
                    Some(i) => {
                        s.jobs.remove(i);
                    }
                    None => return Err(format!("the queue is full ({MAX_JOBS} jobs)")),
                }
            }
            let id = s.next_id;
            s.next_id += 1;
            let created_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            s.jobs.push(Job {
                id,
                spec,
                verify,
                state: State::Pending,
                progress: 0.0,
                message: String::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                started: None,
                elapsed: 0.0,
                created_ms,
            });
            (id, Self::snapshot_locked(&s))
        };
        self.inner.wake.notify_all();
        self.changed(snap);
        Ok(id)
    }

    /// Cancel a pending or running job.
    pub fn cancel(&self, id: u64) {
        let snap = {
            let mut s = self.inner.shared.lock().unwrap();
            if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
                j.cancel.store(true, Ordering::Relaxed);
                if j.state == State::Pending {
                    j.state = State::Canceled;
                    j.message = "canceled before it started".into();
                }
            }
            Self::snapshot_locked(&s)
        };
        self.changed(snap);
    }

    /// Remove a finished or pending job from the list (files on disk are untouched).
    pub fn remove(&self, id: u64) {
        let snap = {
            let mut s = self.inner.shared.lock().unwrap();
            s.jobs
                .retain(|j| j.id != id || matches!(j.state, State::Running | State::Verifying));
            Self::snapshot_locked(&s)
        };
        self.changed(snap);
    }

    pub fn clear_finished(&self) {
        let snap = {
            let mut s = self.inner.shared.lock().unwrap();
            s.jobs.retain(|j| !j.state.is_final());
            Self::snapshot_locked(&s)
        };
        self.changed(snap);
    }

    pub fn set_concurrency(&self, n: usize) {
        self.inner.shared.lock().unwrap().concurrency = n.clamp(1, MAX_WORKERS);
        self.inner.wake.notify_all();
    }

    pub fn concurrency(&self) -> usize {
        self.inner.shared.lock().unwrap().concurrency
    }

    pub fn shutdown(&self) {
        let mut s = self.inner.shared.lock().unwrap();
        s.shutdown = true;
        for j in &s.jobs {
            j.cancel.store(true, Ordering::Relaxed);
        }
        self.inner.wake.notify_all();
    }
}

fn worker(inner: Arc<Inner>) {
    loop {
        // Wait for a pending job and a free slot.
        let (id, spec, verify, cancel, snap) = {
            let mut s = inner.shared.lock().unwrap();
            loop {
                if s.shutdown {
                    return;
                }
                let running = s
                    .jobs
                    .iter()
                    .filter(|j| matches!(j.state, State::Running | State::Verifying))
                    .count();
                if running < s.concurrency {
                    // FIFO: oldest pending job first
                    if let Some(j) = s
                        .jobs
                        .iter_mut()
                        .filter(|j| j.state == State::Pending)
                        .min_by_key(|j| j.created_ms)
                    {
                        j.state = State::Running;
                        j.started = Some(Instant::now());
                        let t = (j.id, j.spec.clone(), j.verify, j.cancel.clone());
                        let snap = Queue::snapshot_locked(&s);
                        break (t.0, t.1, t.2, t.3, snap);
                    }
                }
                s = inner.wake.wait(s).unwrap();
            }
        };
        (inner.notify)(snap);

        let progress_inner = inner.clone();
        let mut last = -1.0f32;
        let mut on_progress = |p: f32, verifying: bool| {
            if (p - last).abs() < 0.004 && p < 1.0 && !verifying {
                return;
            }
            last = p;
            let snap = {
                let mut s = progress_inner.shared.lock().unwrap();
                if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
                    j.progress = p;
                    if verifying {
                        j.state = State::Verifying;
                    }
                }
                Queue::snapshot_locked(&s)
            };
            (progress_inner.notify)(snap);
        };

        let result = run::execute(&spec, verify, &cancel, &mut on_progress);

        let snap = {
            let mut s = inner.shared.lock().unwrap();
            if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
                j.elapsed = j.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
                match &result {
                    Ok(o) => {
                        j.state = if o.verified { State::Passed } else { State::Completed };
                        j.progress = 1.0;
                        j.message = o.message.clone();
                    }
                    Err(run::Failure::Canceled) => {
                        j.state = State::Canceled;
                        j.message = "canceled".into();
                    }
                    Err(run::Failure::Error(m)) => {
                        j.state = State::Failed;
                        j.message = m.clone();
                    }
                }
            }
            Queue::snapshot_locked(&s)
        };
        (inner.notify)(snap);
        inner.wake.notify_all();
    }
}
