//! The whole-repository analysis, off the thread that answers questions.
//!
//! `workspace/diagnostic` is one compilation and one lint pass per stale
//! target, and on a repository whose libraries fan out it is a fifth of a
//! second. On the request thread that is a fifth of a second in which nothing
//! else is read: a hover typed a millisecond later waits for a sweep it has
//! nothing to do with. So the sweep runs on a worker, and the requests a person
//! is waiting on are answered from what the server already knows.
//!
//! **What crosses the boundary is bytes, never a pointer.** Every cache in this
//! server is `Rc`-shared — the session, the source map, the parse cache, the
//! analyses — so none of it may be touched from two threads. The worker
//! therefore keeps a [`State`] of its own, warm in exactly the same way, and
//! what travels between them is a path, the editor's buffers, and the findings
//! as JSON. The cost is a second copy of the repository's text and parses; the
//! benefit is that neither side needs a lock and nothing here is unsafe.
//!
//! **Determinism is a mode, not an accident.** Every answer is a function of
//! the bytes it was computed from and stays so; what a worker makes
//! unpredictable is *when* a report lands, and the golden corpus records a byte
//! stream. `BURI_LSP_ANALYSIS` names which of the three schedules to use, and
//! the corpus runs [`Mode::Synchronous`], where the request that needs a sweep
//! waits for it and the stream is exactly what it was before this file existed.

use super::state::{State, Work};
use super::Published;
use crate::build::sources::Overlay;
use crate::build::workspace::TargetId;
use crate::json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// The environment variable that names the schedule.
pub const CHOSEN: &str = "BURI_LSP_ANALYSIS";

/// When a sweep runs, and when what it found reaches the client.
///
/// The answers do not differ between these — a sweep computes the same
/// findings from the same bytes whichever thread it is on. What differs is the
/// order the client reads them in, which is why a recorded session names one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// The worker sweeps while requests are answered, and a finished sweep
    /// publishes on its own. What an editor gets.
    Asynchronous,
    /// The worker sweeps, and a finished sweep waits for `buri/awaitAnalysis`
    /// before it says anything. What the asynchronous goldens record: the
    /// worker is real and the requests overtake it, but where its report lands
    /// in the stream is the session's decision rather than the scheduler's.
    Deferred,
    /// The request that needs a sweep waits for it, so one message is answered
    /// completely before the next is read. What the rest of the corpus records.
    Synchronous,
}

impl Mode {
    /// The schedule this word names, or `None` for a word that is not one of
    /// the three.
    pub fn named(word: &str) -> Option<Mode> {
        match word {
            "asynchronous" => Some(Mode::Asynchronous),
            "deferred" => Some(Mode::Deferred),
            "synchronous" => Some(Mode::Synchronous),
            _ => None,
        }
    }

    /// What `BURI_LSP_ANALYSIS` asked for, and asynchronous otherwise — a word
    /// nobody recognises is not a reason to answer differently.
    pub fn chosen() -> Mode {
        std::env::var(CHOSEN).ok().and_then(|word| Mode::named(&word)).unwrap_or(Mode::Asynchronous)
    }
}

/// One repository, to be swept for one state of its files.
struct Job {
    root: PathBuf,
    /// The editor's unsaved buffers as they stood when the job was sent. The
    /// worker reads the disk itself; this is the part of the truth only the
    /// request thread has.
    buffers: Overlay,
    generation: u64,
}

/// What one sweep came to.
pub struct Swept {
    pub root: PathBuf,
    /// Per target: the files it read, the state those files were in, and what
    /// the two passes said about them.
    pub targets: Vec<(TargetId, u64, Vec<PathBuf>, Published)>,
    /// What the sweep cost, to be added to the counters the trace reports.
    pub work: Work,
    /// What the worker decided to say out loud — a repository that will not
    /// open is found several calls below the sweep, and the reader has to be
    /// told.
    pub said: Vec<Value>,
    /// Whether a newer edit overtook it before it finished. An abandoned sweep
    /// keeps whatever targets it did get through, so the next one starts from
    /// there; what it does not do is claim to be a whole report.
    pub abandoned: bool,
}

/// Whether the sweep in flight is still the one the server wants.
///
/// A number rather than a flag: the worker compares the generation it was sent
/// with the newest one asked for, so a sweep cannot be stopped by an ask that
/// arrived before it started.
pub struct Wanted {
    latest: Arc<AtomicU64>,
    generation: u64,
}

impl Wanted {
    /// A sweep nothing can overtake — the synchronous schedule, where one
    /// message asks for a sweep and waits for it.
    pub fn steady() -> Wanted {
        Wanted { latest: Arc::new(AtomicU64::new(0)), generation: 0 }
    }

    pub fn superseded(&self) -> bool {
        self.latest.load(Ordering::Relaxed) != self.generation
    }
}

/// How many sweeps were run, skipped and given up on since the server started.
#[derive(Clone, Copy, Default)]
pub struct Counters {
    /// Sweeps that finished and reported.
    pub run: u64,
    /// Sweeps asked for that never ran, because a newer ask for the same
    /// repository replaced them before their turn came. This is the debounce,
    /// counted: a burst of keystrokes is one sweep in flight, one waiting, and
    /// the rest of them here.
    pub superseded: u64,
    /// Sweeps that started and were overtaken.
    pub abandoned: u64,
}

/// The worker, and what the request thread knows about what it is doing.
pub struct Sweeps {
    pub mode: Mode,
    /// Started at the first sweep: a session that never asks about a whole
    /// repository never starts a thread.
    worker: Option<Worker>,
    /// The newest generation asked for. The worker reads it to find out that
    /// what it is computing is already out of date.
    latest: Arc<AtomicU64>,
    generation: u64,
    /// The repository the worker was handed, and the generation it was handed
    /// for.
    running: Option<(PathBuf, u64)>,
    /// Repositories waiting their turn.
    ///
    /// One worker and a queue rather than a worker per repository: a person
    /// edits in one repository at a time, and two that went stale together are
    /// two compilations either way — a thread each would only decide which of
    /// them finishes first. A root already waiting is not queued twice, which
    /// is where a burst of keystrokes collapses into one sweep.
    queued: Vec<PathBuf>,
    /// Reports that have arrived and not been taken up yet.
    landed: Vec<Swept>,
    /// Where to knock when a sweep finishes, so that a server sitting idle at
    /// its input still publishes what the worker found.
    knocker: Option<Sender<super::Event>>,
    pub counters: Counters,
}

struct Worker {
    jobs: Sender<Job>,
    done: Receiver<Swept>,
}

impl Sweeps {
    pub fn new() -> Sweeps {
        Sweeps {
            mode: Mode::chosen(),
            worker: None,
            latest: Arc::new(AtomicU64::new(0)),
            generation: 0,
            running: None,
            queued: Vec::new(),
            landed: Vec::new(),
            knocker: None,
            counters: Counters::default(),
        }
    }

    /// Where to send a knock when a sweep finishes. Without one, a report waits
    /// for the client's next message.
    pub fn knock_on(&mut self, knocker: Sender<super::Event>) {
        self.knocker = Some(knocker);
    }

    /// Whether a sweep is running or waiting to.
    pub fn busy(&self) -> bool {
        self.running.is_some() || !self.queued.is_empty()
    }

    /// Asks for this repository to be swept for the state it is in now.
    ///
    /// Asking twice for a repository already waiting is not two sweeps: a job
    /// reads the files when it starts, so the earlier ask would compute the
    /// report the later one is going to. It is counted rather than dropped
    /// silently, because "a burst of keystrokes queued one sweep" is the claim
    /// the schedule rests on.
    pub fn schedule(&mut self, root: &Path, buffers: &Overlay) {
        if self.queued.iter().any(|waiting| waiting == root) {
            self.counters.superseded = self.counters.superseded.saturating_add(1);
            return;
        }
        if let Some((running, _)) = &self.running {
            let same = running == root;
            self.queued.push(root.to_path_buf());
            // The sweep in flight is reading bytes that have moved. Telling it
            // so is what keeps a hub keystroke from paying for the edit before
            // it as well as for its own. Not in the deferred schedule, where
            // where a report lands is the session's decision and a sweep that
            // gave up half way through would land somewhere else.
            if same && self.mode == Mode::Asynchronous {
                self.generation = self.generation.saturating_add(1);
                self.latest.store(self.generation, Ordering::Relaxed);
            }
            return;
        }
        self.queued.push(root.to_path_buf());
        self.hand_over(buffers);
    }

    /// Sends the next waiting repository to the worker, starting it if this is
    /// the first sweep of the session.
    fn hand_over(&mut self, buffers: &Overlay) {
        if self.running.is_some() || self.queued.is_empty() {
            return;
        }
        let root = self.queued.remove(0);
        self.generation = self.generation.saturating_add(1);
        self.latest.store(self.generation, Ordering::Relaxed);
        let latest = Arc::clone(&self.latest);
        let knocker = self.knocker.clone();
        let worker = self.worker.get_or_insert_with(|| start(latest, knocker));
        let job = Job { root: root.clone(), buffers: buffers.clone(), generation: self.generation };
        // A worker that has gone is one nothing here can bring back: the
        // reports stop and every answer falls back to the last complete one.
        if worker.jobs.send(job).is_ok() {
            self.running = Some((root, self.generation));
        }
    }

    /// Takes up whatever the worker has finished, without waiting for it.
    pub fn collect(&mut self, buffers: &Overlay) {
        while let Some(swept) = self.worker.as_ref().and_then(|worker| worker.done.try_recv().ok())
        {
            self.arrived(swept);
        }
        self.hand_over(buffers);
    }

    /// The same, waiting until nothing is running or queued.
    ///
    /// This is the whole of the synchronous schedule, and the whole of what
    /// `buri/awaitAnalysis` does.
    pub fn settle(&mut self, buffers: &Overlay) {
        while self.busy() {
            let Some(swept) = self.worker.as_ref().and_then(|worker| worker.done.recv().ok())
            else {
                // Nothing is coming. Forgetting what was asked for is what
                // keeps this from waiting for ever.
                self.running = None;
                self.queued.clear();
                return;
            };
            self.arrived(swept);
            self.hand_over(buffers);
        }
    }

    /// One report, back from the worker.
    fn arrived(&mut self, swept: Swept) {
        self.running = None;
        if swept.abandoned {
            self.counters.abandoned = self.counters.abandoned.saturating_add(1);
            // Still worth keeping: the targets it did get through are filed
            // under the closures they were read at, so the sweep that replaces
            // it starts from there.
            if !self.queued.iter().any(|waiting| *waiting == swept.root) {
                self.queued.push(swept.root.clone());
            }
        } else {
            self.counters.run = self.counters.run.saturating_add(1);
        }
        self.landed.push(swept);
    }

    /// The reports that have arrived, handed over to be believed.
    pub fn take_landed(&mut self) -> Vec<Swept> {
        std::mem::take(&mut self.landed)
    }
}

/// The worker thread: one repository at a time, for as long as the server runs.
fn start(latest: Arc<AtomicU64>, knocker: Option<Sender<super::Event>>) -> Worker {
    let (jobs, incoming) = channel::<Job>();
    let (outgoing, done) = channel::<Swept>();
    // Detached deliberately. The thread holds no lock and owns everything it
    // touches, so a server that is exiting has nothing to wait for it about.
    // A thread that will not start leaves `jobs` with no reader, which
    // `hand_over` already reads as a worker that is gone.
    let _started = std::thread::Builder::new()
        .name("buri-lsp-sweep".to_string())
        .spawn(move || serve(&incoming, &outgoing, &latest, knocker.as_ref()));
    Worker { jobs, done }
}

fn serve(
    incoming: &Receiver<Job>,
    outgoing: &Sender<Swept>,
    latest: &Arc<AtomicU64>,
    knocker: Option<&Sender<super::Event>>,
) {
    // The worker's own warm repository: its own graph, its own source map, its
    // own parses. Nothing in it is shared with the thread that answers
    // questions, which is what makes this a thread rather than a lock.
    let mut analyst = State::new();
    while let Ok(job) = incoming.recv() {
        let wanted = Wanted { latest: Arc::clone(latest), generation: job.generation };
        let swept = one(&mut analyst, job, &wanted);
        if outgoing.send(swept).is_err() {
            return;
        }
        // The report is on the channel; this is what makes the request thread
        // look at it rather than waiting for the client to say something.
        if let Some(knocker) = knocker {
            if knocker.send(super::Event::Swept).is_err() {
                return;
            }
        }
    }
}

/// One job, from the buffers it was sent with to the report it came to.
fn one(analyst: &mut State, job: Job, wanted: &Wanted) -> Swept {
    let before = analyst.work();
    analyst.stand_at(&job.root, job.buffers);
    analyst.sweep_now(&job.root, wanted);
    Swept {
        targets: analyst.swept_findings(&job.root),
        work: analyst.work().since(before),
        said: analyst.take_outgoing(),
        abandoned: wanted.superseded(),
        root: job.root,
    }
}
