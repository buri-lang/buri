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
    let cores = std::thread::available_parallelism().map(|c| c.get()).unwrap_or(1);
    cores.clamp(1, len)
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
    let Ok(mut slot) = PREVIOUS.lock() else {
        drop(value);
        return;
    };
    if let Some(previous) = slot.take() {
        let _ = previous.join();
    }
    match std::thread::Builder::new().name("buri-discard".into()).spawn(move || drop(value)) {
        Ok(handle) => *slot = Some(handle),
        // No thread to be had: the value was moved into the closure that could
        // not be started, so it has already been dropped where it stood.
        Err(_) => {}
    }
}

/// `f` over `0..len`, in parallel, with the results in index order.
pub fn map<R, F>(len: usize, f: F) -> Vec<R>
where
    R: Send,
    F: Fn(usize) -> R + Sync,
{
    map_with(len, || (), |(), i| f(i))
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
pub fn map_with<S, R, I, F>(len: usize, init: I, f: F) -> Vec<R>
where
    R: Send,
    I: Fn() -> S + Sync,
    F: Fn(&mut S, usize) -> R + Sync,
{
    let width = width(len);
    if width <= 1 {
        let mut state = init();
        return (0..len).map(|i| f(&mut state, i)).collect();
    }
    let next = AtomicUsize::new(0);
    let mut slots: Vec<Option<R>> = Vec::with_capacity(len);
    slots.resize_with(len, || None);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(width);
        for _ in 0..width {
            let cursor = &next;
            let each = &f;
            let start = &init;
            let worker = std::thread::Builder::new()
                .name("buri-worker".into())
                .stack_size(STACK)
                .spawn_scoped(scope, move || {
                    let mut done: Vec<(usize, R)> = Vec::new();
                    let mut state = start();
                    loop {
                        let i = cursor.fetch_add(1, Ordering::Relaxed);
                        if i >= len {
                            return done;
                        }
                        done.push((i, each(&mut state, i)));
                    }
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
            let Ok(done) = worker.join() else { continue };
            for (i, value) in done {
                if let Some(slot) = slots.get_mut(i) {
                    *slot = Some(value);
                }
            }
        }
    });
    // Every index that has no result — because no worker started, or because
    // one did not return — is computed here. This is what makes the fallback a
    // slower path rather than a different answer.
    let mut out = Vec::with_capacity(len);
    let mut spare: Option<S> = None;
    for (i, slot) in slots.into_iter().enumerate() {
        out.push(match slot {
            Some(value) => value,
            None => {
                let state = spare.get_or_insert_with(&init);
                f(state, i)
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
/// function of an index: `buri test` builds and runs its suites on one.
///
/// A job may queue more jobs. A *heavy* job holds a whole program in memory, so
/// [`Queue::push`] waits while the pool's limit of heavy jobs are already
/// queued or running.
pub struct Queue<J> {
    state: std::sync::Mutex<QueueState<J>>,
    ready: std::sync::Condvar,
    room: std::sync::Condvar,
    heavy_limit: usize,
}

struct QueueState<J> {
    jobs: std::collections::VecDeque<(J, bool)>,
    heavy: usize,
    closed: bool,
}

impl<J> Queue<J> {
    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState<J>> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queues a job behind the others, waiting first for room if it is heavy.
    pub fn push(&self, job: J, heavy: bool) {
        let mut state = self.lock();
        while heavy && state.heavy >= self.heavy_limit {
            state = self.room.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.heavy = state.heavy.saturating_add(usize::from(heavy));
        state.jobs.push_back((job, heavy));
        self.ready.notify_one();
    }

    /// Queues a light job ahead of the others: it finishes work already started.
    pub fn push_first(&self, job: J) {
        self.lock().jobs.push_front((job, false));
        self.ready.notify_one();
    }

    fn take(&self) -> Option<(J, bool)> {
        let mut state = self.lock();
        loop {
            if let Some(job) = state.jobs.pop_front() {
                return Some(job);
            }
            if state.closed {
                return None;
            }
            state = self.ready.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn finished(&self, heavy: bool) {
        if heavy {
            let mut state = self.lock();
            state.heavy = state.heavy.saturating_sub(1);
            self.room.notify_all();
        }
    }

    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }
}

/// Starts `width` workers running `work`, at most `heavy_limit` of them on heavy
/// jobs, and hands `drive` the queue and the results. Returns once `drive` does
/// and the workers have stopped.
///
/// A result is `None` when its job panicked, so `drive` never waits for a result
/// that will not come.
pub fn pool<J, R, T>(
    width: usize,
    heavy_limit: usize,
    work: impl Fn(J, &Queue<J>) -> R + Sync,
    drive: impl FnOnce(&Queue<J>, &std::sync::mpsc::Receiver<Option<R>>) -> T,
) -> T
where
    J: Send,
    R: Send,
{
    let queue = Queue {
        state: std::sync::Mutex::new(QueueState {
            jobs: std::collections::VecDeque::new(),
            heavy: 0,
            closed: false,
        }),
        ready: std::sync::Condvar::new(),
        room: std::sync::Condvar::new(),
        heavy_limit: heavy_limit.max(1),
    };
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..width.max(1) {
            let (queue, work, send) = (&queue, &work, send.clone());
            let started = std::thread::Builder::new()
                .name("buri-job".into())
                .stack_size(STACK)
                .spawn_scoped(scope, move || {
                    while let Some((job, heavy)) = queue.take() {
                        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            work(job, queue)
                        }));
                        queue.finished(heavy);
                        let _ = send.send(done.ok());
                    }
                });
            if started.is_err() {
                break;
            }
        }
        drop(send);
        let out = drive(&queue, &receive);
        queue.close();
        out
    })
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
}
