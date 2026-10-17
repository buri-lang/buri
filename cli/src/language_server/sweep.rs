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
    /// The worker is handed a sweep when it is asked for and starts it when
    /// `buri/awaitAnalysis` arrives. What the asynchronous goldens record: the
    /// requests overtake the sweep exactly as they do in an editor, but how far
    /// the sweep got before they did is the session's decision rather than the
    /// scheduler's.
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
        std::env::var(CHOSEN)
            .ok()
            .and_then(|word| Mode::named(&word))
            .unwrap_or(Mode::Asynchronous)
    }
}

/// One repository, to be swept for one state of its files.
struct Job {
    root: PathBuf,
    /// The editor's unsaved buffers as they stood when the job was handed
    /// over. The worker reads the disk itself; this is the part of the truth
    /// only the request thread has.
    buffers: Overlay,
    generation: u64,
}

/// What one sweep came to.
pub struct Swept {
    pub root: PathBuf,
    /// Per target: the files it read, the state those files were in, and what
    /// the two passes said about them.
    pub targets: Vec<(TargetId, u64, Vec<PathBuf>, Published)>,
    /// The same, merged into one report. Merging is deduplication across
    /// targets that share a library, and the worker has already done it — the
    /// request thread would be doing it again to publish from.
    pub found: Published,
    /// What the sweep cost, to be added to the counters the trace reports.
    pub work: Work,
    /// What the worker decided to say out loud — a repository that will not
    /// open is found several calls below the sweep, and the reader has to be
    /// told. The words rather than the message: the request thread may have
    /// said them already, and it is the one that knows.
    pub said: Vec<String>,
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
    /// Where jobs go and reports come back. Until [`Sweeps::start`] nothing is
    /// at the other end of either.
    jobs: Sender<Job>,
    done: Receiver<Swept>,
    /// The newest generation asked for. The worker reads it to find out that
    /// what it is computing is already out of date.
    latest: Arc<AtomicU64>,
    generation: u64,
    /// The repository the worker was handed, and the generation it was handed
    /// for.
    running: Option<(PathBuf, u64)>,
    /// The running job while the worker has not been given it, which outside
    /// the asynchronous schedule is until something settles.
    held: Option<Job>,
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
    pub counters: Counters,
}

impl Sweeps {
    pub fn new() -> Sweeps {
        let (jobs, _) = channel();
        let (_, done) = channel();
        Sweeps {
            mode: Mode::chosen(),
            jobs,
            done,
            latest: Arc::new(AtomicU64::new(0)),
            generation: 0,
            running: None,
            held: None,
            queued: Vec::new(),
            landed: Vec::new(),
            counters: Counters::default(),
        }
    }

    /// Starts the worker. `knocker` hears each time a sweep finishes, so that
    /// a server sitting idle at its input still publishes what it found.
    ///
    /// Detached deliberately. The thread holds no lock and owns everything it
    /// touches, so a server that is exiting has nothing to wait for it about.
    /// Its stack is the toolchain's own: a sweep compiles every target, and
    /// every stage after parsing walks the tree by recursion.
    pub fn start(&mut self, knocker: Sender<super::Event>) -> std::io::Result<()> {
        let (jobs, incoming) = channel::<Job>();
        let (outgoing, done) = channel::<Swept>();
        let latest = Arc::clone(&self.latest);
        std::thread::Builder::new()
            .name("buri-lsp-sweep".to_string())
            .stack_size(crate::parallel::STACK)
            .spawn(move || serve(&incoming, &outgoing, &latest, &knocker))?;
        self.jobs = jobs;
        self.done = done;
        Ok(())
    }

    /// Whether a sweep is running. One waiting is never all there is: the
    /// queue is handed over whenever nothing runs.
    pub fn busy(&self) -> bool {
        self.running.is_some()
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
        self.queued.push(root.to_path_buf());
        match &self.running {
            // The sweep in flight is reading bytes that have moved. Telling it
            // so is what keeps a hub keystroke from paying for the edit before
            // it as well as for its own.
            Some((running, _)) => {
                if running == root {
                    self.generation = self.generation.saturating_add(1);
                    self.latest.store(self.generation, Ordering::Relaxed);
                }
            }
            None => self.hand_over(buffers),
        }
    }

    /// Hands the next waiting repository to the worker. In the asynchronous
    /// schedule the worker starts on it at once; in the other two it is held
    /// until something settles.
    fn hand_over(&mut self, buffers: &Overlay) {
        if self.queued.is_empty() {
            return;
        }
        let root = self.queued.remove(0);
        self.generation = self.generation.saturating_add(1);
        self.latest.store(self.generation, Ordering::Relaxed);
        self.running = Some((root.clone(), self.generation));
        self.held = Some(Job {
            root,
            buffers: buffers.clone(),
            generation: self.generation,
        });
        if self.mode == Mode::Asynchronous {
            self.release();
        }
    }

    /// Gives the worker the job it was handed, if it has not had it yet.
    fn release(&mut self) {
        if let Some(job) = self.held.take() {
            // A worker that has gone takes nothing, and `settle` says so.
            let _ = self.jobs.send(job);
        }
    }

    /// Takes up whatever the worker has finished, without waiting for it.
    pub fn collect(&mut self, buffers: &Overlay) {
        while let Ok(swept) = self.done.try_recv() {
            self.arrived(swept, buffers);
        }
    }

    /// The same, waiting until nothing is running or queued.
    ///
    /// This is the whole of the synchronous schedule, and the whole of what
    /// `buri/awaitAnalysis` does.
    pub fn settle(&mut self, buffers: &Overlay) {
        while self.busy() {
            self.release();
            match self.done.recv() {
                Ok(swept) => self.arrived(swept, buffers),
                // The worker panicked, which printed why and, in a release
                // build, ended the process. Nothing is running any more.
                Err(_) => self.running = None,
            }
        }
    }

    /// One report, back from the worker, and the next job handed over.
    ///
    /// An abandoned sweep was overtaken by an ask for the same repository, and
    /// that ask is in the queue: the sweep that replaces it is the next one.
    fn arrived(&mut self, swept: Swept, buffers: &Overlay) {
        self.running = None;
        if swept.abandoned {
            self.counters.abandoned = self.counters.abandoned.saturating_add(1);
        } else {
            self.counters.run = self.counters.run.saturating_add(1);
        }
        self.landed.push(swept);
        self.hand_over(buffers);
    }

    /// The reports that have arrived, handed over to be believed.
    pub fn take_landed(&mut self) -> Vec<Swept> {
        std::mem::take(&mut self.landed)
    }
}

/// The worker thread: one repository at a time, for as long as the server runs.
fn serve(
    incoming: &Receiver<Job>,
    outgoing: &Sender<Swept>,
    latest: &Arc<AtomicU64>,
    knocker: &Sender<super::Event>,
) {
    // The worker's own warm repository: its own graph, its own source map, its
    // own parses. Nothing in it is shared with the thread that answers
    // questions, which is what makes this a thread rather than a lock.
    let mut analyst = State::new();
    // A `for`, so the end isn't a branch: whether the server lets go of
    // `jobs` before it exits is a race.
    for job in incoming.iter() {
        let wanted = Wanted {
            latest: Arc::clone(latest),
            generation: job.generation,
        };
        let swept = one(&mut analyst, job, &wanted);
        // A send fails only once the server has gone, and then `incoming` ends
        // the loop too. Not a branch, since which comes first is a race.
        let _ = outgoing.send(swept);
        // The report is on the channel; this is what makes the request thread
        // look at it rather than waiting for the client to say something.
        let _ = knocker.send(super::Event::Swept);
    }
}

/// One job, from the buffers it was sent with to the report it came to.
fn one(analyst: &mut State, job: Job, wanted: &Wanted) -> Swept {
    let before = analyst.work();
    analyst.stand_at(&job.root, job.buffers);
    let found = analyst.sweep_now(&job.root, wanted).unwrap_or_default();
    Swept {
        found,
        targets: analyst.swept_findings(&job.root),
        work: analyst.work().since(before),
        said: analyst.take_said(),
        abandoned: wanted.superseded(),
        root: job.root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that is not a repository, which is the cheapest job the
    /// worker can be given: the graph will not open and the sweep is over.
    fn nowhere() -> PathBuf {
        std::env::temp_dir().join("buri-lsp-sweep-tests-nothing-here")
    }

    /// Sweeps in the deferred schedule, with a worker behind them. Nothing
    /// starts between the asks, which is what makes the counts a decision
    /// rather than a race.
    fn deferred() -> (Sweeps, Receiver<super::super::Event>) {
        let mut sweeps = Sweeps::new();
        sweeps.mode = Mode::Deferred;
        let (knocker, knocks) = channel();
        sweeps.start(knocker).unwrap();
        (sweeps, knocks)
    }

    #[test]
    fn a_burst_of_asks_is_one_sweep_given_up_one_run_and_the_rest_counted() {
        let (mut sweeps, _knocks) = deferred();
        let root = nowhere();
        let buffers = Overlay::new();
        for _ in 0..4 {
            sweeps.schedule(&root, &buffers);
        }
        assert_eq!(sweeps.counters.superseded, 2);
        sweeps.settle(&buffers);
        assert_eq!(sweeps.counters.run, 1);
        assert_eq!(sweeps.counters.abandoned, 1);
        assert_eq!(sweeps.take_landed().len(), 2);
        assert!(!sweeps.busy());
    }

    #[test]
    fn two_repositories_take_the_one_worker_in_turn() {
        let (mut sweeps, _knocks) = deferred();
        let buffers = Overlay::new();
        let first = nowhere();
        let second = nowhere().join("beside-it");
        sweeps.schedule(&first, &buffers);
        sweeps.schedule(&second, &buffers);
        assert_eq!(sweeps.counters.superseded, 0);
        sweeps.settle(&buffers);
        let landed = sweeps.take_landed();
        assert_eq!(landed.len(), 2);
        assert_eq!(landed[0].root, first);
        assert_eq!(landed[1].root, second);
        assert!(landed.iter().all(|swept| !swept.abandoned));
    }

    #[test]
    fn a_sweep_hears_that_a_newer_edit_has_overtaken_it() {
        let latest = Arc::new(AtomicU64::new(1));
        let wanted = Wanted {
            latest: Arc::clone(&latest),
            generation: 1,
        };
        assert!(!wanted.superseded());
        latest.store(2, Ordering::Relaxed);
        assert!(wanted.superseded());
    }

    #[test]
    fn the_schedule_is_named_or_it_is_the_asynchronous_one() {
        assert_eq!(Mode::named("synchronous"), Some(Mode::Synchronous));
        assert_eq!(Mode::named("deferred"), Some(Mode::Deferred));
        assert_eq!(Mode::named("asynchronous"), Some(Mode::Asynchronous));
        assert_eq!(Mode::named("sometimes"), None);
    }
}
