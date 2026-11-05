//! Whole-program work, spread over the cores the machine already has.
//!
//! It is at the top level for the reason the other top-level modules are: both
//! `build` and `compiler` need it and neither owns it. `build::actions` hashes
//! one codegen unit at a time and `compiler::middle::lower` lowers one function
//! at a time, and those are the two places where an incremental rebuild spends
//! its time on work that is `O(repoSize)` by construction — a content-addressed
//! `codegen` key cannot be computed without the program it names.
//!
//! # What this is allowed to change, and what it is not
//!
//! **Nothing about the answer.** [`map`] returns results in input order, and it
//! is only usable where the per-item function is a pure function of the item:
//! `Fn` rather than `FnMut`, `Sync` rather than `Send`, so a closure carrying
//! mutable state does not compile. Build output is compared byte for byte
//! (`two_checkouts_of_one_tree_build_identical_bytes`), and a pass whose output
//! depended on how the work was divided would be a pass that had to be measured
//! rather than trusted.
//!
//! **Not whether the work happens.** A worker that could not be started, or one
//! that did not finish, leaves its items to be computed here, in order, on the
//! calling thread. There is no path on which an item is skipped: a missing
//! result is recomputed rather than defaulted, so a panic-free fallback cannot
//! turn into a silently short answer — and a short answer is exactly the shape
//! that would corrupt a `link` key.
//!
//! # Why threads rather than a thread pool
//!
//! Two calls per build, of about a hundred milliseconds each. Starting ten
//! threads costs tens of microseconds; a pool would cost a dependency and a
//! piece of global state whose lifetime nothing here has an opinion about.
//! [`pool`] is the exception: `buri test` hands it jobs that are not a pure
//! function of an index, and its workers live only as long as one call.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The stack a worker gets.
///
/// The same reservation the toolchain's own main thread takes, and for the same
/// reason: every stage after parsing walks a tree by recursion, and a worker
/// running one of those stages needs the room the parser's depth bound was
/// chosen against. It is address space rather than memory — pages are committed
/// as they are touched — so a pass that recurses three deep pays for three.
pub const STACK: usize = 256 * 1024 * 1024;

/// How many workers to divide `len` items between.
///
/// Never more than there are items, so a program with four codegen units does
/// not start ten threads to leave six of them with nothing.
/// Under this many items the calling thread does the work itself: starting a
/// thread is tens of microseconds, and these functions are also called for a
/// package with three source files.
const WORTH_IT: usize = 4;

pub fn width(len: usize) -> usize {
    if len < WORTH_IT {
        return 1;
    }
    cores().clamp(1, len)
}

/// The cores whole-program work spreads over: all of them, or `BURI_THREADS`
/// where it's set. `BURI_THREADS=1` keeps that work on one thread, so a
/// profile's instruction counts hold still under load (design/PERFORMANCE.md §8).
pub fn cores() -> usize {
    static CORES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CORES.get_or_init(|| {
        let set = std::env::var("BURI_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0);
        set.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |c| c.get()))
    })
}

/// Frees `value` on a thread of its own.
///
/// The whole-program values a build produces are large — a monomorphized
/// program and a checked module set are tens of milliseconds of `free` at a
/// hundred thousand lines — and by the time they are dropped the answer the
/// command was asked for already exists. The work still happens; it stops
/// happening between the answer and the prompt.
///
/// One outstanding drop at a time. A `buri test --watch` loop hands one of
/// these over per pass, and the previous one is waited for first, so the
/// arrangement can never hold more than two programs at once — which is what
/// makes this a *deferred* free rather than a leak with a thread attached.
pub fn discard<T: Send + 'static>(value: T) {
    static PREVIOUS: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> =
        std::sync::Mutex::new(None);
    let mut slot = PREVIOUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(previous) = slot.take() {
        let _ = previous.join();
    }
    // With no thread to be had, the value was moved into the closure that could
    // not be started, so it has already been dropped where it stood.
    *slot = std::thread::Builder::new()
        .name("buri-discard".into())
        .spawn(move || drop(value))
        .ok();
}

/// `f` over `0..len`, in parallel, with the results in index order.
pub fn map<R, F>(len: usize, f: F) -> Vec<R>
where
    R: Send,
    F: Fn(usize) -> R + Sync,
{
    map_items_with(&vec![(); len], || (), |(), i, ()| f(i))
}

/// The same, where each worker needs a scratch table of its own.
pub fn map_with<S, R, I, F>(len: usize, init: I, f: F) -> Vec<R>
where
    R: Send,
    I: Fn() -> S + Sync,
    F: Fn(&mut S, usize) -> R + Sync,
{
    map_items_with(&vec![(); len], init, |state, i, ()| f(state, i))
}

/// `f` over each of `items` and its index, in parallel, with the results in
/// index order.
pub fn map_items<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(usize, &T) -> R + Sync,
{
    map_items_with(items, || (), |(), i, item| f(i, item))
}

/// The same, where each worker needs a scratch table of its own.
///
/// `init` runs once per worker rather than once per item, because the tables
/// this exists for — a `Layouts` memo, a type interner — are worth building for
/// a few hundred items and are not worth building for one. It is a *scratch*
/// table by contract: `f` may read and write it, and the result of `f` may not
/// depend on which items came before, because how the items are divided between
/// workers is not fixed. A memo table satisfies that; a running total does not.
///
/// Items are taken one at a time from a shared cursor rather than dealt out in
/// contiguous blocks, because the items are codegen units and functions and
/// those differ in size by two orders of magnitude — a block of the wrong ones
/// leaves one worker running long after the rest have finished.
pub fn map_items_with<T, S, R, I, F>(items: &[T], init: I, f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    I: Fn() -> S + Sync,
    F: Fn(&mut S, usize, &T) -> R + Sync,
{
    let len = items.len();
    let width = width(len);
    if width <= 1 {
        let mut state = init();
        return items
            .iter()
            .enumerate()
            .map(|(i, item)| f(&mut state, i, item))
            .collect();
    }
    let next = AtomicUsize::new(0);
    let phase = crate::profile::current();
    let tally = crate::profile::tally();
    let mut slots: Vec<Option<R>> = Vec::with_capacity(len);
    slots.resize_with(len, || None);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(width);
        for _ in 0..width {
            let cursor = &next;
            let each = &f;
            let start = &init;
            let tally = tally.clone();
            let worker = std::thread::Builder::new()
                .name("buri-worker".into())
                .stack_size(STACK)
                .spawn_scoped(scope, move || {
                    let _phase = crate::profile::adopt(phase);
                    crate::profile::tallied(tally, || {
                        let mut done: Vec<(usize, R)> = Vec::new();
                        let mut state = start();
                        loop {
                            let i = cursor.fetch_add(1, Ordering::Relaxed);
                            let Some(item) = items.get(i) else {
                                return done;
                            };
                            done.push((i, each(&mut state, i, item)));
                        }
                    })
                });
            match worker {
                Ok(worker) => workers.push(worker),
                // No thread to be had is a machine problem, and the items this
                // worker would have taken are still in the cursor: the workers
                // that did start take them, and whatever is left is computed
                // below.
                Err(_) => break,
            }
        }
        for worker in workers {
            // A worker that did not return left its indices to the loop below.
            for (i, value) in worker.join().unwrap_or_default() {
                #[expect(
                    clippy::indexing_slicing,
                    reason = "a worker takes only indices below `len`, and `slots` holds `len`"
                )]
                let slot = &mut slots[i];
                *slot = Some(value);
            }
        }
    });
    // Every index that has no result — because no worker started, or because
    // one did not return — is computed here. This is what makes the fallback a
    // slower path rather than a different answer.
    let mut out = Vec::with_capacity(len);
    let mut spare: Option<S> = None;
    for (i, (slot, item)) in slots.into_iter().zip(items).enumerate() {
        out.push(match slot {
            Some(value) => value,
            None => {
                let state = spare.get_or_insert_with(&init);
                f(state, i, item)
            }
        });
    }
    out
}

/// How much memory this machine has, in bytes, where it can say.
pub fn memory_bytes() -> Option<u64> {
    unsafe extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    #[cfg(target_os = "macos")]
    const PHYS_PAGES: i32 = 200;
    #[cfg(target_os = "macos")]
    const PAGE_SIZE: i32 = 29;
    #[cfg(not(target_os = "macos"))]
    const PHYS_PAGES: i32 = 85;
    #[cfg(not(target_os = "macos"))]
    const PAGE_SIZE: i32 = 30;
    // SAFETY: `sysconf` reads a system constant and touches no memory of ours.
    let (pages, size) = unsafe { (sysconf(PHYS_PAGES), sysconf(PAGE_SIZE)) };
    let pages = u64::try_from(pages).ok()?;
    let size = u64::try_from(size).ok()?;
    pages.checked_mul(size).filter(|&bytes| bytes > 0)
}

/// A queue of jobs and the workers that take them, for work that is not a pure
/// function of an index: `buri test` checks, builds and runs its suites on one.
///
/// A job may queue more jobs. A *heavy* job makes a whole program, and is
/// queued with the bytes it is expected to hold. A worker takes one only while
/// the heavy jobs already holding a program leave that much of the pool's
/// budget, and takes the next light job meanwhile. A heavy job larger than the
/// whole budget still runs, alone, rather than never. It holds its program from
/// the moment it is taken until it drops the [`Held`] its worker hands it,
/// which is usually long before it finishes: a suite emits or links, lets go of
/// its program, and only then runs.
///
/// The budget applies when a job is taken rather than when it is queued, so
/// queueing never waits. That is what lets a job queue heavy jobs while it
/// holds a program itself: a queue that waited could fill with jobs waiting
/// for room that only they would free.
pub struct Queue<J> {
    state: std::sync::Mutex<QueueState<J>>,
    ready: std::sync::Condvar,
    budget: u64,
    width: std::sync::Arc<Width>,
}

struct QueueState<J> {
    /// Each job with the bytes it holds, `0` for a light one.
    jobs: std::collections::VecDeque<(J, u64)>,
    /// Light jobs that run what is already built, taken only when no job in
    /// `jobs` can be ([`Queue::push_later`]): by rank, highest first, and in
    /// the order queued within a rank.
    later: std::collections::BTreeMap<(std::cmp::Reverse<u64>, u64), J>,
    /// How many jobs [`Queue::push_later`] has queued, which orders a rank.
    queued_later: u64,
    /// The bytes the heavy jobs holding a program are expected to hold.
    held: u64,
    /// How many heavy jobs hold a program.
    holding: usize,
    /// The most there have been at once, and the most bytes they were
    /// expected to hold ([`Queue::most_held`]).
    most: (usize, u64),
    closed: bool,
}

/// What a [`Held`] tells when it is dropped.
trait Release {
    fn release(&self, bytes: u64);
}

impl<J> Release for Queue<J> {
    fn release(&self, bytes: u64) {
        let mut state = self.lock();
        state.held = state.held.saturating_sub(bytes);
        state.holding = state.holding.saturating_sub(1);
        drop(state);
        self.ready.notify_all();
    }
}

/// A heavy job's claim on the pool's budget. Dropping it says the job no longer
/// holds its program, and gives its bytes back. A light job's claims nothing.
#[must_use = "dropping it at once lets another program in while this one is still held"]
pub struct Held<'a> {
    room: Option<(&'a dyn Release, u64)>,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        if let Some((room, bytes)) = self.room {
            room.release(bytes);
        }
    }
}

impl<J> Queue<J> {
    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState<J>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queues a job behind the others. `bytes` is what a heavy job is expected
    /// to hold, and `0` makes the job light.
    pub fn push(&self, job: J, bytes: u64) {
        self.lock().jobs.push_back((job, bytes));
        self.ready.notify_one();
    }

    /// Queues a light job that runs something already built, such as a test
    /// binary. It is taken only when no job [`Queue::push`] queued can be, so
    /// everything waiting to be built is built — and linked — first, and the
    /// runs fill the workers the builds leave idle. A higher `rank` goes
    /// ahead of the runs queued with a lower one, and behind those queued
    /// with its own.
    pub fn push_later(&self, job: J, rank: u64) {
        let mut state = self.lock();
        let order = state.queued_later;
        state.queued_later = order.saturating_add(1);
        state.later.insert((std::cmp::Reverse(rank), order), job);
        drop(state);
        self.ready.notify_one();
    }

    /// The first job a worker may start: the first light one, or the first
    /// heavy one the budget has room for, whichever is queued first; and only
    /// when there is neither, the highest-ranked run.
    fn take(&self) -> Option<(J, Held<'_>)> {
        let mut state = self.lock();
        loop {
            let held = state.held;
            let fits =
                |bytes: u64| bytes == 0 || held == 0 || held.saturating_add(bytes) <= self.budget;
            let at = state.jobs.iter().position(|(_, bytes)| fits(*bytes));
            if let Some((job, bytes)) = at.and_then(|at| state.jobs.remove(at)) {
                state.held = state.held.saturating_add(bytes);
                if bytes > 0 {
                    state.holding = state.holding.saturating_add(1);
                    let (programs, most) = state.most;
                    state.most = (programs.max(state.holding), most.max(state.held));
                }
                let room: &dyn Release = self;
                return Some((
                    job,
                    Held {
                        room: (bytes > 0).then_some((room, bytes)),
                    },
                ));
            }
            if let Some((_, job)) = state.later.pop_first() {
                return Some((job, Held { room: None }));
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// The most heavy jobs that have held a program at once, and the most
    /// bytes the ones holding one have been expected to hold between them.
    pub fn most_held(&self) -> (usize, u64) {
        self.lock().most
    }

    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
        self.width.close();
    }
}

/// How many of a [`pool`]'s jobs count against its width: each job a worker
/// has taken, or is waiting to take, less those [`aside_while_idle`] has found
/// waiting with nothing to run.
struct Width {
    limit: usize,
    state: std::sync::Mutex<WidthState>,
    freed: std::sync::Condvar,
}

struct WidthState {
    counted: usize,
    closed: bool,
}

impl Width {
    fn lock(&self) -> std::sync::MutexGuard<'_, WidthState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A place among the counted jobs, once there's room for one; `None`
    /// once the pool is closed.
    fn enter(self: &std::sync::Arc<Self>) -> Option<std::sync::Arc<Place>> {
        let mut state = self.lock();
        while !state.closed && state.counted >= self.limit {
            state = self
                .freed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if state.closed {
            return None;
        }
        state.counted = state.counted.saturating_add(1);
        Some(std::sync::Arc::new(Place {
            width: std::sync::Arc::clone(self),
            counted: std::sync::atomic::AtomicBool::new(true),
        }))
    }

    fn close(&self) {
        self.lock().closed = true;
        self.freed.notify_all();
    }
}

/// One worker's place in its pool's width, counted or set aside.
struct Place {
    width: std::sync::Arc<Width>,
    /// Changed only under `width`'s lock.
    counted: std::sync::atomic::AtomicBool,
}

impl Place {
    /// Counts this place against the width again, or stops counting it and
    /// lets a waiting worker in. A place counted again may leave more jobs
    /// running than the width for a while: nothing stops a process that has
    /// woken, so the next job waits until the count is back under it.
    fn count(&self, counted: bool) {
        let mut state = self.width.lock();
        if self.counted.swap(counted, Ordering::Relaxed) == counted {
            return;
        }
        if counted {
            state.counted = state.counted.saturating_add(1);
        } else {
            state.counted = state.counted.saturating_sub(1);
            self.width.freed.notify_one();
        }
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.count(false);
    }
}

thread_local! {
    /// The place of the [`pool`] job running on this thread, if one is.
    static PLACE: std::cell::RefCell<Option<std::sync::Arc<Place>>> =
        const { std::cell::RefCell::new(None) };
}

/// How often [`aside_while_idle`] looks at its process.
const LOOK: std::time::Duration = std::time::Duration::from_millis(100);

/// How many looks in a row must find no thread of the process runnable before
/// its job is set aside: a second.
const QUIET_LOOKS: u32 = 10;

/// While the answer lives, the [`pool`] job on this thread stops counting
/// against the pool's width whenever process `pid` has had no thread runnable
/// for a second, and counts again as soon as one is.
///
/// It's for a job that does nothing but wait on a process, such as a test
/// waiting out a thirty-second deadline. The place it held lets another job
/// start, so the machine isn't left a core short while it waits. Nothing
/// about the waiting process changes; it only stops holding the next job
/// back. It looks for a runnable thread rather than processor time, because
/// on macOS a busy thread that never yields can leave its time unread for
/// seconds, and it's runnable throughout.
///
/// Outside a pool job it does nothing.
#[must_use = "dropping it at once stops the watch"]
pub fn aside_while_idle(pid: u32) -> Aside {
    let place = PLACE.with(|p| p.borrow().clone());
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let watcher = place.clone().and_then(|place| {
        std::thread::Builder::new()
            .name("buri-idle".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let mut quiet = 0u32;
                while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    stopped.recv_timeout(LOOK)
                {
                    quiet = if os::runs(pid) {
                        0
                    } else {
                        quiet.saturating_add(1)
                    };
                    place.count(quiet < QUIET_LOOKS);
                }
            })
            .ok()
    });
    Aside {
        stop: Some(stop),
        watcher,
        place,
    }
}

/// [`aside_while_idle`]'s watch. Dropping it ends the watch and counts the
/// job's place again.
pub struct Aside {
    stop: Option<std::sync::mpsc::Sender<()>>,
    watcher: Option<std::thread::JoinHandle<()>>,
    place: Option<std::sync::Arc<Place>>,
}

impl Drop for Aside {
    fn drop(&mut self) {
        drop(self.stop.take());
        let _ = self.watcher.take().map(std::thread::JoinHandle::join);
        let _ = self.place.as_ref().map(|place| place.count(true));
    }
}

/// Whether a process has a thread on a core or waiting for one. A process
/// that is gone, or one the system won't say about, has none.
#[cfg(target_os = "macos")]
mod os {
    /// `PROC_PIDTASKINFO`'s `proc_taskinfo`, in `<sys/proc_info.h>`'s order.
    #[repr(C)]
    #[derive(Default)]
    struct TaskInfo {
        sizes_and_times: [u64; 6],
        counts: [i32; 10],
        numrunning: i32,
        priority: i32,
    }

    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            size: i32,
        ) -> i32;
    }

    pub fn runs(pid: u32) -> bool {
        const PROC_PIDTASKINFO: i32 = 4;
        const SIZE: i32 = std::mem::size_of::<TaskInfo>() as i32;
        let mut info = TaskInfo::default();
        let buffer: *mut TaskInfo = &mut info;
        // SAFETY: the kernel writes at most `SIZE` bytes, a `proc_taskinfo`,
        // into `info`. A call that fails writes nothing, and the zero left
        // there reads as no thread running.
        let _ = unsafe {
            proc_pidinfo(
                i32::try_from(pid).unwrap_or(-1),
                PROC_PIDTASKINFO,
                0,
                buffer.cast(),
                SIZE,
            )
        };
        info.numrunning > 0
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    /// Whether any of `/proc/<pid>/task/*/stat` is in state `R` or `D`. The
    /// state follows the name's closing bracket, and a name may hold one.
    pub fn runs(pid: u32) -> bool {
        std::fs::read_dir(format!("/proc/{pid}/task"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|thread| {
                std::fs::read_to_string(thread.path().join("stat"))
                    .ok()
                    .and_then(|stat| {
                        stat.rsplit_once(')')
                            .and_then(|(_, after)| after.split_whitespace().next())
                            .map(|state| matches!(state, "R" | "D"))
                    })
                    .unwrap_or(false)
            })
    }
}

/// Starts `width` workers running `work`, and hands `drive` the queue and the
/// results. Returns once `drive` does and the workers have stopped, or `None`
/// at once when the machine has no thread for any worker.
///
/// The heavy jobs holding a program at once are expected to hold at most
/// `budget` bytes between them. It limits nothing else: a job that has dropped
/// its [`Held`] leaves its worker free to run beside the others, so `width`
/// jobs can always be in flight. So can more, while some of them are set aside
/// waiting on an idle process ([`aside_while_idle`]).
///
/// `work` hands its result back by returning it, and may hand more back on the
/// way with the [`Tell`] it is given. A job that panics hands back its panic,
/// which [`Results::recv`] raises again on `drive`'s thread, so `drive` never
/// waits for a result that will not come.
pub fn pool<J, R, T>(
    width: usize,
    budget: u64,
    work: impl Fn(J, Held<'_>, &Queue<J>, &Tell<'_, R>) -> R + Sync,
    drive: impl FnOnce(&Queue<J>, &Results<R>) -> T,
) -> Option<T>
where
    J: Send,
    R: Send,
{
    let queue = Queue {
        state: std::sync::Mutex::new(QueueState {
            jobs: std::collections::VecDeque::new(),
            later: std::collections::BTreeMap::new(),
            queued_later: 0,
            held: 0,
            holding: 0,
            most: (0, 0),
            closed: false,
        }),
        ready: std::sync::Condvar::new(),
        budget,
        width: std::sync::Arc::new(Width {
            limit: width.max(1),
            state: std::sync::Mutex::new(WidthState {
                counted: 0,
                closed: false,
            }),
            freed: std::sync::Condvar::new(),
        }),
    };
    let results = Results {
        inbox: std::sync::Mutex::new(std::collections::VecDeque::new()),
        arrived: std::sync::Condvar::new(),
    };
    let phase = crate::profile::current();
    let tally = crate::profile::tally();
    std::thread::scope(|scope| {
        // A worker the machine has no thread for leaves its jobs to the ones
        // that started. Twice the width, so each job set aside while it waits
        // ([`aside_while_idle`]) has a worker to take the next job in its place.
        let started = (0..width.max(1).saturating_mul(2))
            .filter(|_| {
                let (queue, work, results) = (&queue, &work, &results);
                let tally = tally.clone();
                std::thread::Builder::new()
                    .name("buri-job".into())
                    .stack_size(STACK)
                    .spawn_scoped(scope, move || {
                        let _phase = crate::profile::adopt(phase);
                        crate::profile::tallied(tally, || {
                            let tell = Tell { results };
                            while let Some(place) = queue.width.enter() {
                                let Some((job, held)) = queue.take() else {
                                    break;
                                };
                                PLACE.with(|p| *p.borrow_mut() = Some(place));
                                // A job that panics drops its `Held` as it
                                // unwinds.
                                results.send(std::panic::catch_unwind(
                                    std::panic::AssertUnwindSafe(|| work(job, held, queue, &tell)),
                                ));
                                PLACE.with(|p| p.borrow_mut().take());
                            }
                        });
                    })
                    .is_ok()
            })
            .count();
        // Closed on the way out however `drive` leaves. If it panicked, the
        // workers would otherwise wait on the queue and the scope on them.
        let _closing = Closing(&queue);
        (started > 0).then(|| drive(&queue, &results))
    })
}

struct Closing<'q, J>(&'q Queue<J>);

impl<J> Drop for Closing<'_, J> {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// The results of a [`pool`]'s jobs, in the order they were handed back.
pub struct Results<R> {
    inbox: std::sync::Mutex<std::collections::VecDeque<std::thread::Result<R>>>,
    arrived: std::sync::Condvar,
}

impl<R> Results<R> {
    /// The next result, waiting for one. A job that panicked raises its panic
    /// here.
    pub fn recv(&self) -> R {
        let mut inbox = self
            .inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(next) = inbox.pop_front() {
                return next.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            }
            inbox = self
                .arrived
                .wait(inbox)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn send(&self, result: std::thread::Result<R>) {
        self.inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(result);
        self.arrived.notify_one();
    }
}

/// How a job hands back a result before its last one.
pub struct Tell<'r, R> {
    results: &'r Results<R>,
}

impl<R> Tell<'_, R> {
    /// Hands `result` to `drive` now, ahead of anything this job queues next.
    pub fn tell(&self, result: R) {
        self.results.send(Ok(result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order of the results is the order of the indices, whatever order the
    /// work happened in. Everything else here rests on that: a `codegen` key
    /// list that came back shuffled would name each unit's object with another
    /// unit's key.
    #[test]
    fn results_come_back_in_index_order() {
        let out = map(1000, |i| i * 2);
        assert_eq!(out.len(), 1000);
        for (i, value) in out.iter().enumerate() {
            assert_eq!(*value, i * 2);
        }
    }

    /// The degenerate widths, which are the ones a small repository takes.
    #[test]
    fn no_items_and_one_item_are_answers_rather_than_special_cases() {
        assert!(map(0, |i: usize| i).is_empty());
        assert_eq!(map(1, |i| i + 7), vec![7]);
        assert_eq!(width(0), 1);
        assert_eq!(width(1), 1);
        assert!(width(2) <= 2);
    }

    /// A counted region counts the workers it starts: the same work spread
    /// over them costs about what it costs on one thread. Where nothing
    /// counts, both read 0.
    #[test]
    fn a_counted_region_counts_its_workers() {
        let spin = |i: usize| (0..200_000u64).fold(i as u64, |a, k| std::hint::black_box(a ^ k));
        let (_, alone) = crate::profile::counted(|| (0..16).map(spin).collect::<Vec<_>>());
        let (_, spread) = crate::profile::counted(|| map(16, spin));
        if alone == 0 {
            return;
        }
        let ratio = spread as f64 / alone as f64;
        assert!(
            (0.8..1.5).contains(&ratio),
            "{spread} instructions spread against {alone} alone"
        );
    }

    /// A `drive` that panics still closes the queue, so the workers stop and
    /// the panic reaches the caller instead of the process waiting for ever.
    #[test]
    fn a_panicking_drive_stops_the_workers() {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(|| {
                pool(
                    4,
                    0,
                    |job: u8, _, _, _| job,
                    |_: &Queue<u8>, _| panic!("drive failed"),
                )
            });
            let _ = done.send(outcome.is_err());
        });
        // A safety net only: a pool that never closes never answers.
        let panicked = finished.recv_timeout(std::time::Duration::from_secs(60));
        assert_eq!(panicked, Ok(true));
    }

    /// A job that panics raises its panic in `drive`, which would otherwise
    /// wait for ever for the result it was owed.
    #[test]
    fn a_panicking_job_raises_its_panic_in_drive() {
        let outcome = std::panic::catch_unwind(|| {
            pool(
                2,
                0,
                |_: u8, _, _, _: &Tell<'_, u8>| -> u8 { std::panic::panic_any("job failed") },
                |queue: &Queue<u8>, results| {
                    queue.push(1, 0);
                    results.recv()
                },
            )
        });
        let payload = outcome.err().map(|p| p.downcast_ref::<&str>().copied());
        assert_eq!(payload, Some(Some("job failed")));
    }
}
