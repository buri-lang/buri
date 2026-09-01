//! The carrier runtime — the tokio handle, the scheduler and the task table,
//! behind feature `net`.
//!
//! Design: `design/native` track B, §0.1 ("carrier threads with a run baton,
//! then stack switching") and §4 ("Runtime choices, concretely").
//!
//! ## 0. What a green task is, and what it is not
//!
//! A Buri task is **a pair of stacks**, not a state machine. Buri machine code
//! stays exactly what it is — ordinary frame-threaded synchronous code, no
//! CPS, no coroutine, no `musttail` — and a suspending host call is one
//! [`park_on`] in an otherwise unremarkable `extern "C"` body. That is the
//! whole of the integration, and it is deliberate: a native CPS transform is
//! larger than both backends' relooper-shaped alternatives put together, and
//! `middle/mod.rs` and `design/native/CODEGEN-LLVM.md` have each already
//! rejected the shape once.
//!
//! **Phase 2 landed in B9 and this is it.** A task was an OS thread from a
//! pool until then, and the row said what would replace it: *"a hand-written
//! `swapcontext`-shaped switch per `(arch, os)`, behind the same
//! `buri_rt_task_*` ABI; the per-task data stack from B7 is reused; parked
//! tasks stop costing an OS thread."*
//!
//! ```text
//!   a task  = a machine stack   (mmap, 64 MiB + a 1 MiB PROT_NONE guard
//!             |                  below it: it grows down)
//!             + a Buri data stack list  (B7's, moved off the thread)
//!             + one saved stack pointer
//!
//!   a carrier = an OS thread running `carrier_loop`, and nothing else:
//!               take a task, switch to its stack, come back when it parks
//!               or finishes, take the next one.
//! ```
//!
//! Nothing above this file changed. `park_on` is still the name a suspension
//! goes through, `on_carrier` still answers a [`Handoff`], `Tasks.parallel` is
//! still one exported symbol with the same signature, and `host.rs` was not
//! edited. What changed is the cost of waiting: **ten thousand parked tasks,
//! which the thread pool could not create at all — `pthread_create` refuses at
//! 8 192 on this platform — run on a couple of dozen threads and 40 % less
//! resident memory.** `reports/wave8-b9.md` has the table.
//!
//! ## 1. The baton is gone, and what stands in its place
//!
//! Until G3 this file held a **run baton**: a token admitting exactly one
//! carrier to Buri code at a time. It was a staging device, and it said so —
//! non-atomic reference counts, the `rc == 1` in-place licence and a
//! single-threaded allocator were all correct only because the thing they are
//! not safe against did not happen. `design/native/MEMORY.md` prices atomic
//! refcounting at 2–3×, and the baton let that cost land as its own slice
//! rather than as a prerequisite for every other one.
//!
//! **That slice landed, and this is it.** What replaces the baton is not
//! another lock; it is that the *values* two carriers can both reach are
//! marked, and a marked block is counted atomically and is never eligible for
//! an in-place write:
//!
//! * `middle::rc::crosses_tasks` asks the whole program whether any of its
//!   values can come to be reachable from a second carrier. Both native
//!   backends turn a `true` into one call at startup,
//!   `memory::buri_rt_values_may_cross_tasks`.
//! * That call makes `memory::finish` stamp `CAP_SHARED_FLAG` into **every
//!   block the program allocates**, so G2's fork takes its atomic arm
//!   everywhere and `buri_rt_unique_cap` answers `None` everywhere.
//! * [`fan_out`] is **gated on the same latch**. Two carriers run Buri code
//!   beside each other only in a program whose blocks are all marked, and
//!   `memory::values_may_cross_tasks` is the run-time proof of that rather
//!   than an assumption about who called whom.
//!
//! So the exclusion the baton provided is still there; it moved from a lock
//! held across a whole call into the header word of the blocks the call
//! touches, which is the only place it can be while two steps genuinely
//! compute at once. Everything else in this runtime was already thread-safe
//! and was checked rather than assumed: `host.rs`'s four streams are
//! process-global `Mutex`es, `memory.rs`'s counters are atomics and its caches
//! are per-thread, `rng.rs` and the `Alloc` counters are `Mutex`es.
//!
//! ## 2. What creates a second carrier, and what still does not
//!
//! [`buri_rt_host_tasks_parallel`] does: it fans a `Tasks.parallel` call's
//! steps onto the pool, one carrier each, and waits for them. That is the only
//! thing in a Buri program that starts a carrier today — [`task_start`] and
//! the table below are the shape track F's `core/actor` needs and nothing
//! calls them yet.
//!
//! It does it only where **both** statements the artifact makes about itself
//! are true, and they are different facts:
//!
//! * `lib.rs`'s `buri_rt_frames_are_per_carrier` — a carrier entering Buri
//!   code gets frames of its own. A property of the *backend*: the LLVM one
//!   says it, the frame-threaded one does not until each carrier owns a Buri
//!   stack (track B, B7).
//! * `memory::buri_rt_values_may_cross_tasks` — a block this program allocates
//!   may be reached from two carriers, so it is marked. A property of the
//!   *program*, and both backends say it for the programs it is true of.
//!
//! Where either is missing the steps run one after another on the calling
//! carrier, in index order, answering the same `[B]`. The order promise is
//! what makes those two the same program; the timing is not part of it.
//!
//! [`Clock::sleepMillis`][slp] and [`Net::fetch`][fch] route through
//! [`park_on`], so two steps that wait overlap. **Two steps that compute now
//! overlap too**, which is the whole of what this slice changed at this level:
//! `the_steps_of_one_fan_out_compute_at_the_same_time` is the case that would
//! have been impossible to write while the baton existed, and
//! `a_shared_counter_survives_a_fan_out` is the one that says the counts under
//! it are exact.
//!
//! ### The `buri_rt_task_*` ABI, and the half of it that is still undefined
//!
//! This file used to say there was **no `buri_rt_task_*` C ABI here, on
//! purpose**, because an exported symbol is a contract somebody emits calls
//! into and writing one before there is a caller is a guess about a
//! signature. That reason has not changed, and B9 splits the family in two by
//! it:
//!
//! | symbol | caller | status |
//! |---|---|---|
//! | `buri_rt_task_switch` | `rt.rs`, through [`switch`] | **defined**, and its caller is in this repository |
//! | `buri_rt_task_launch` | the switch's own `ret` | **defined**, for the same reason |
//! | [`buri_rt_task_main`] | `buri_rt_task_launch`, in `switch_*.s` | **defined**: the Rust side of the pad |
//! | a Buri-facing start / join / is-live | nothing, still | **not defined** — track F |
//!
//! The three that exist are the ones this slice is *made of*: a hand-written
//! assembly block cannot call a Rust function without a C symbol between them,
//! so the caller is not a prediction but a file three directories away in the
//! same commit. [`task_start`], [`task_join`] and [`task_is_live`] stay
//! Rust-only, because `core/actor` still does not exist and their signatures
//! would still be guesses.
//!
//! [slp]: crate::buri_rt_host_clock_sleep_millis
//! [fch]: crate::buri_rt_host_net_fetch
//!
//! ## 3. Threads, locks and poisoning
//!
//! Locks recover from poisoning, for the reason `host.rs` and `testing.rs`
//! give at theirs: a poisoned lock means this runtime already panicked, and
//! failing a second time on top of the first helps nobody.
//!
//! Carrier stacks are [`CARRIER_STACK_BYTES`], and since B9 that number
//! bounds a scheduler loop rather than any Buri code: a *task* runs on a
//! mapping of its own, `memory::BURI_RT_STACK_BYTES` wide with a `PROT_NONE`
//! guard at the end it grows towards, and it acquires its Buri data stack from
//! `memory::buri_rt_stack_acquire` exactly as B7 wrote it. `main`'s static
//! block is left exactly as it is.
//!
//! ## 4. Thread-local storage, and the one hazard the switch introduces
//!
//! A task may be resumed on a carrier it did not start on. That makes
//! **thread-local storage the one thing in this file that can be silently
//! wrong**: the address of a thread-local is a value a compiler is entitled to
//! compute once and reuse, and a task that switched carriers between the
//! computing and the using would read or write another thread's slot.
//!
//! There are exactly two thread-locals here — [`CARRIER_SP`] and [`HERE`] —
//! and every access to either goes through an `#[inline(never)]` function that
//! takes the address, uses it, and does not let it out. The rule is stated at
//! [`running`] and it is why those four one-line functions exist rather than
//! `.with(…)` at the use.
//!
//! `memory.rs`'s own thread-locals are not affected and were checked rather
//! than assumed: the G2 block caches hold `malloc` blocks, which any thread
//! may free, so which carrier a cached block came from does not matter. The
//! one that *did* matter is B7's Buri-stack free list, and it moved onto the
//! task — `memory::stack_list` is the seam.

use std::cell::{Cell, UnsafeCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread;

use crate::list::{block, StepEntry};
use crate::memory::Blocks;
use crate::switch;
use crate::value::BuriList;

// ---------------------------------------------------------------------------
// The tokio handle
// ---------------------------------------------------------------------------

/// The process's reactor, timer wheel and blocking pool.
///
/// A `OnceLock` and never dropped, which is the point: dropping a
/// multi-threaded runtime joins its workers, and the last thing a program
/// should do on the way out is wait for an idle timer thread. The process
/// exits and the kernel reclaims them.
///
/// Built on first use rather than at startup, because a program that never
/// suspends should not pay for a reactor. `buri_rt_argv_init` would be the
/// obvious place for an eager one and it is deliberately not used: the
/// generated `main` calls it unconditionally, and most programs never reach
/// this file at all.
static REACTOR: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// The handle every [`park_on`] blocks on.
///
/// Multi-threaded, with tokio's own default worker count, which is the
/// available parallelism. Carriers are **not** worker threads — they are this
/// file's own OS threads — so `Handle::block_on` from one is legal, and that
/// legality is the whole of the integration. Nothing in `host.rs` is `async`.
///
/// # Panics
/// If the reactor cannot be built, which on a supported platform means the
/// process cannot create threads.
pub fn handle() -> &'static tokio::runtime::Handle {
    REACTOR
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("buri-reactor")
                .build()
                .expect("the buri runtime could not start a reactor")
        })
        .handle()
}

// ---------------------------------------------------------------------------
// Parking
// ---------------------------------------------------------------------------

/// Wait for `future`, **without holding a carrier while it waits**.
///
/// **Three lines until G3, one after it, and a scheduler after B9.** The two
/// G3 deleted were the run baton's. What replaced the one that was left is not
/// a bigger wait but a smaller one: a task that suspends now switches its
/// machine stack out from under the carrier and the carrier goes back for
/// other work, so a parked task costs a mapping and not a thread.
///
/// ```text
///   on a task            poll -> Pending -> save this stack -> the carrier
///                        ^                                       goes back
///                        |                                       for work
///                        +---- the waker puts the task on the run queue
///
///   not on a task        handle().block_on(future)     -- what it always was
/// ```
///
/// **Both arms are here on purpose.** `main`'s own thread is not a task and
/// must not become one — the process's Buri stack is the `__bss` block
/// `asm.rs` guards and its machine stack is the OS's — so a program that never
/// starts a second task reaches exactly the `block_on` it reached before, and
/// the switch is code it never runs. [`running`] is the one-word test that
/// chooses, and it is the same test `memory::stack_list` makes for the other
/// stack.
///
/// The future is polled on **this** stack, in both arms, so it need be neither
/// `Send` nor `'static`: it lives in this frame, and a task's frame goes where
/// the task goes. A body that is still synchronous (`http.rs`'s client) costs
/// no copy and no thread hop to route through here.
///
/// **The loop is not a spin.** `Poll::Pending` is answered by a switch, and
/// the only thing that goes round again is a task the waker reached *during*
/// the poll — the [`NOTIFIED`] arm — which is a re-poll the waker asked for
/// and not a wait.
///
/// # Panics
/// The non-task arm panics if called from a tokio worker thread, which
/// `Handle::block_on` refuses. No carrier is one, and nothing in `host.rs`
/// runs inside a tokio task.
pub fn park_on<T>(future: impl Future<Output = T>) -> T {
    let here = running();
    if here.is_null() {
        return handle().block_on(future);
    }
    // SAFETY: the carrier that resumed this task holds an `Arc` to it for as
    // long as the task is on its stack, which is the whole of this call.
    let task: &Task = unsafe { &*here };
    let waker = task.waker();
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        task.state.store(RUNNING, Ordering::Release);
        // The reactor's context is entered around the **poll and nothing
        // else**. `EnterGuard` restores a thread-local on drop, and a task
        // that switched carriers between the two would restore it on the
        // wrong thread; per-poll is a thread-local swap and per-park would be
        // a bug that only shows up under migration.
        let polled = {
            let _in_reactor = handle().enter();
            future.as_mut().poll(&mut cx)
        };
        if let Poll::Ready(answer) = polled {
            return answer;
        }
        if task
            .state
            .compare_exchange(RUNNING, PARKING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // The waker reached this task while it was being polled, so the
            // poll it asked for is the next turn of this loop rather than a
            // park and a wake.
            continue;
        }
        task.why.store(WHY_PARK, Ordering::Release);
        leave(task);
    }
}

// ---------------------------------------------------------------------------
// Tasks: a machine stack, a Buri data stack, and one saved word
// ---------------------------------------------------------------------------

/// A carrier's machine stack, in bytes.
///
/// 512 KiB, from `design/native` track B §4, and **what it bounds changed in
/// B9**. It used to be the stack a job's Buri code ran on, which made it the
/// LLVM backend's recursion limit and left the two backends with two different
/// depths (`reports/wave6-b7b8.md` §5.2). A carrier now runs
/// [`carrier_loop`] and nothing else: it takes a task off the queue, switches
/// to *the task's* stack, and is back here the moment the task parks. So this
/// number bounds a scheduler loop, and every Buri frame — on either backend —
/// is on the [`memory::BURI_RT_STACK_BYTES`] mapping the task owns.
pub const CARRIER_STACK_BYTES: usize = 512 * 1024;

/// How many carrier threads this process will start.
///
/// **A ceiling and not a target**: carriers are started one at a time, only
/// when a task is queued that no idle carrier will pick up, and a program
/// whose tasks all park keeps one or two however many tasks it has. The
/// measurement is in `reports/wave8-b9.md`: ten thousand parked tasks, which
/// the thread-per-task pool could not create at all, run on a couple of dozen.
///
/// It exists because **a task may block rather than park.** `park_on` is the
/// door a suspension goes through and a task that instead calls something
/// blocking — a `std` channel, a `Mutex`, a spin over another task's flag —
/// holds its carrier while it does. Two hundred and fifty-six is what a
/// program may have blocked at once and still make progress; past it the queue
/// waits. The old pool had no such ceiling and paid for it in the other
/// direction: it could not reach ten thousand threads because the kernel
/// refused at 8 192 (`os error 35`), which is a ceiling too, discovered at
/// run time and expressed as an abort.
const MAX_CARRIERS: usize = 256;

/// What a task is doing, as one atomic word.
///
/// Six states, and the two that look redundant are the handshake that makes
/// the switch safe: **a task must not be resumed by a second carrier before
/// the first has finished saving its context.** [`PARKING`] is the window
/// between "the task has decided to leave" and "the carrier has written its
/// stack pointer down", and a waker that arrives inside it leaves
/// [`NOTIFIED_PARKING`] for the carrier to act on rather than queueing the
/// task itself.
///
/// ```text
///   QUEUED --take--> RUNNING --poll Pending--> PARKING --switch--> PARKED
///                      |  ^                       |                  |
///                 wake |  | re-poll          wake |             wake |
///                      v  |                       v                  v
///                   NOTIFIED              NOTIFIED_PARKING  ------> QUEUED
///                                          (the carrier queues it)
/// ```
const RUNNING: u8 = 0;
/// Woken while running: poll again rather than park.
const NOTIFIED: u8 = 1;
/// Leaving; the context is not saved yet, so nobody else may resume it.
const PARKING: u8 = 2;
/// Woken while leaving; the carrier queues it once the context is down.
const NOTIFIED_PARKING: u8 = 3;
/// Context saved. A waker may queue it from here.
const PARKED: u8 = 4;
/// On the run queue, waiting for a carrier.
const QUEUED: u8 = 5;
/// The body returned. Terminal: a waker that arrives now does nothing.
const FINISHED: u8 = 6;

/// Why a task switched back to its carrier: it parked, or it finished.
const WHY_PARK: u8 = 0;
const WHY_DONE: u8 = 1;

/// One task: two stacks, one saved stack pointer, and a state word.
///
/// The design's `Task` (track B §4) named a `StackBlock` and a mailbox
/// `Sender`. The first is here twice over — `stack` is the machine one and
/// `blocks` is B7's Buri data list, moved off the thread because a parked task
/// outlives the carrier it started on — and the second is still track F's.
pub(crate) struct Task {
    /// The state machine above.
    state: AtomicU8,
    /// Which of the two switches back the carrier is looking at.
    why: AtomicU8,
    /// The task's machine stack pointer while it is **not** running.
    ///
    /// Written by [`switch::buri_rt_task_switch`] on the way out and read on
    /// the way in, so it is only ever touched by the one carrier the task is
    /// on — which is what makes an `UnsafeCell` right and a lock wrong: a lock
    /// would have to be released *after* the stack it protects had gone.
    sp: UnsafeCell<*mut u8>,
    /// The base of the mapping `stack` is the top of, for the carrier to give
    /// back when the task ends.
    stack: *mut u8,
    /// The task's Buri data stacks: B7's free list, keyed by task rather than
    /// by thread. `memory::stack_list` is what reaches it.
    blocks: UnsafeCell<Blocks>,
    /// Taken by [`buri_rt_task_main`] on the task's own stack, exactly once.
    body: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Set by the carrier after the task's stack has been given back, which is
    /// the moment a joiner may look at the answer.
    done: AtomicBool,
    /// Whether the body returned rather than unwinding out of it.
    ok: AtomicBool,
    /// Everybody waiting for `done`, woken once.
    waiters: Mutex<Vec<Waker>>,
    /// G5: the `core/alloc` scope this task is inside — the arena, and the bump
    /// window into it (`memory::ArenaSlot`).
    ///
    /// **On the task and not on the thread**, because since B9 two tasks share
    /// a carrier's thread and the arena a value is allocated out of belongs to
    /// the one whose stack is running. A task that parks inside a `scoped`
    /// leaves its scope here and finds it again on whichever carrier resumes
    /// it, and the carrier's own slot goes back to what it was.
    ///
    /// A `Mutex` rather than an atomic because it is three words now; it is
    /// taken twice per turn of a task, which is nowhere near anything hot.
    arena: Mutex<crate::memory::ArenaSlot>,
}

// SAFETY: every field is either atomic, behind a `Mutex`, or an `UnsafeCell`
// touched only by the single carrier the task is running on — and a task runs
// on one carrier at a time by construction, because it is on the run queue or
// on a carrier and never both (the state machine above is what enforces it).
// `stack` is a mapping nobody but the reaping carrier touches.
unsafe impl Send for Task {}
// SAFETY: as above; sharing a `&Task` is what a `Waker` does, and every field
// a waker reaches is atomic or locked.
unsafe impl Sync for Task {}

impl Task {
    /// A `Waker` that queues this task.
    ///
    /// Built from a borrowed pointer rather than from an owned `Arc` because
    /// the running task only has a `&Task` to hand: [`running`] answers an
    /// address, and the `Arc` it came from is the one the carrier is holding.
    fn waker(&self) -> Waker {
        let p: *const Task = self;
        // SAFETY: `p` came from an `Arc<Task>` the carrier holds for the
        // length of this task's turn, so incrementing is sound and the count
        // the vtable's `drop` decrements is the one incremented here.
        unsafe {
            Arc::increment_strong_count(p);
            Waker::from_raw(RawWaker::new(p.cast(), &WAKER))
        }
    }
}

static WAKER: RawWakerVTable = RawWakerVTable::new(waker_clone, waker_wake, waker_wake_ref, waker_drop);

/// # Safety
/// `p` came from `Arc<Task>::into_raw`, or from a live `Arc<Task>`.
unsafe fn waker_clone(p: *const ()) -> RawWaker {
    // SAFETY: the caller's promise.
    unsafe { Arc::increment_strong_count(p.cast::<Task>()) };
    RawWaker::new(p, &WAKER)
}

/// # Safety
/// As [`waker_clone`], and this consumes the count.
unsafe fn waker_wake(p: *const ()) {
    // SAFETY: the caller's promise; the `Arc` is dropped at the end of the
    // scope, after the notification has taken a count of its own if it needs
    // one.
    unsafe {
        let task = Arc::from_raw(p.cast::<Task>());
        notify(Arc::as_ptr(&task));
    }
}

/// # Safety
/// As [`waker_clone`]; the count is not consumed.
unsafe fn waker_wake_ref(p: *const ()) {
    // SAFETY: the caller's promise.
    unsafe { notify(p.cast::<Task>()) };
}

/// # Safety
/// As [`waker_clone`], and this consumes the count.
unsafe fn waker_drop(p: *const ()) {
    // SAFETY: the caller's promise.
    unsafe { Arc::decrement_strong_count(p.cast::<Task>()) };
}

/// Make a parked task runnable again, or record that it was woken too early
/// for that to mean anything yet.
///
/// **The one place the state machine's races are resolved**, and the whole of
/// its correctness is that it never queues a task whose context is not yet
/// saved: [`PARKING`] leaves [`NOTIFIED_PARKING`] behind, and the carrier that
/// is doing the saving is the one that then queues it. A wake that lands on a
/// [`FINISHED`] task does nothing, which is what makes a waker that outlives
/// its future harmless.
///
/// # Safety
/// `p` names a live `Task` — the caller holds a reference to it for the length
/// of this call.
unsafe fn notify(p: *const Task) {
    // SAFETY: the caller's promise.
    let task = unsafe { &*p };
    loop {
        let seen = task.state.load(Ordering::Acquire);
        let next = match seen {
            RUNNING => NOTIFIED,
            PARKING => NOTIFIED_PARKING,
            PARKED => QUEUED,
            // NOTIFIED, NOTIFIED_PARKING, QUEUED, FINISHED: somebody else has
            // already taken responsibility, or there is nothing left to do.
            _ => return,
        };
        if task
            .state
            .compare_exchange_weak(seen, next, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        if seen == PARKED {
            // SAFETY: `p` came from an `Arc` the caller holds, so a count for
            // the queue can be taken from it.
            let owned = unsafe {
                Arc::increment_strong_count(p);
                Arc::from_raw(p)
            };
            push(owned);
        }
        return;
    }
}

// ---------------------------------------------------------------------------
// The carrier pool
// ---------------------------------------------------------------------------

/// The run queue and the two counts that decide whether a carrier is started.
///
/// One lock over all three, because the decision is a **relation** between
/// them — "is there a queued task no idle carrier will take?" — and reading
/// two atomics would answer it about no instant in particular.
struct Sched {
    queue: VecDeque<Arc<Task>>,
    /// Carriers inside [`take`], whether or not they are blocked yet.
    idle: usize,
    /// Carrier threads started, ever. Never decremented: a carrier is not
    /// retired, for the reason the pool never retired one before — a pool that
    /// reaped idle threads would trade a thread for a thread creation on every
    /// burst.
    carriers: usize,
}

static SCHED: Mutex<Sched> =
    Mutex::new(Sched { queue: VecDeque::new(), idle: 0, carriers: 0 });
/// Woken by [`push`], waited on by [`take`].
static READY: Condvar = Condvar::new();

fn sched() -> MutexGuard<'static, Sched> {
    match SCHED.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// How many carrier threads exist.
#[must_use]
pub fn carriers() -> usize {
    sched().carriers
}

/// Put a runnable task on the queue, starting a carrier if nothing idle will
/// take it.
fn push(task: Arc<Task>) {
    let start = {
        let mut s = sched();
        s.queue.push_back(task);
        let short = s.queue.len() > s.idle && s.carriers < MAX_CARRIERS;
        if short {
            // Counted here, under the lock, rather than in the thread that is
            // about to be created: two pushes racing would otherwise each see
            // the same count and start a carrier apiece.
            s.carriers += 1;
        }
        short
    };
    READY.notify_one();
    if start {
        start_carrier();
    }
}

/// Take the next runnable task, waiting for one.
///
/// `armed` says the caller has already counted itself idle — which the
/// finishing arm of [`carrier_loop`] does **before** it tells a joiner the
/// answer is ready, so that a caller which dispatches again the instant it has
/// one finds an idle carrier rather than starting a second. That ordering is
/// the pool's oldest promise; what changed in B9 is that it is a counter
/// rather than a channel put back in a vector.
fn take(armed: bool) -> Arc<Task> {
    let mut s = sched();
    if !armed {
        s.idle += 1;
    }
    loop {
        if let Some(task) = s.queue.pop_front() {
            s.idle -= 1;
            return task;
        }
        s = match READY.wait(s) {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
    }
}

/// Count this carrier as available before it goes back for work.
fn arm() {
    sched().idle += 1;
}

/// Start one carrier thread. The count was taken by [`push`].
fn start_carrier() {
    let id = {
        let s = sched();
        s.carriers
    };
    let started = thread::Builder::new()
        .name(format!("buri-carrier-{id}"))
        .stack_size(CARRIER_STACK_BYTES)
        .spawn(carrier_loop);
    if let Err(e) = started {
        panic!("the buri runtime could not start a carrier: {e}");
    }
}

thread_local! {
    /// Where this carrier's own context is saved while a task is on its stack.
    ///
    /// Read back by [`leave`] on whichever thread the task is running on, which
    /// is what makes migration work: a task does not remember the carrier it
    /// started on, it asks the one it is on now.
    static CARRIER_SP: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    /// The task on this thread's stack, or null.
    static HERE: Cell<*const Task> = const { Cell::new(std::ptr::null()) };
}

/// **`#[inline(never)]` on all four of these, and it is load-bearing.**
///
/// The address of a thread-local is a value a compiler is entitled to compute
/// once and reuse, and a task that switched carriers between the computing and
/// the using would read or write the wrong thread's slot. An opaque call is
/// what stops it: the address is computed inside the callee, used inside the
/// callee, and never crosses the switch. It is the one thing in this file that
/// would still be correct-looking and wrong.
#[inline(never)]
fn running() -> *const Task {
    HERE.with(Cell::get)
}

#[inline(never)]
fn set_running(task: *const Task) {
    HERE.with(|slot| slot.set(task));
}

#[inline(never)]
fn carrier_slot() -> *mut *mut u8 {
    CARRIER_SP.with(Cell::as_ptr)
}

#[inline(never)]
fn carrier_context() -> *mut u8 {
    CARRIER_SP.with(Cell::get)
}

/// The list `memory::buri_rt_stack_acquire` draws a Buri data stack from, when
/// a task is running on this thread.
///
/// The seam between the two stacks, and it points the way it does — the
/// allocator asking the scheduler — because the *task* is what owns a data
/// stack now and only this file knows which task that is.
#[inline(never)]
pub(crate) fn running_task_blocks() -> Option<*mut Blocks> {
    let task = running();
    if task.is_null() {
        return None;
    }
    // SAFETY: a task on this thread's stack is one the carrier holds an `Arc`
    // to, and it is the only task this thread can be inside.
    Some(unsafe { (*task).blocks.get() })
}

/// Leave this task and return to the carrier that is running it.
///
/// Comes back when — and if — a carrier resumes the task, which need not be
/// the same one.
#[inline(never)]
fn leave(task: &Task) {
    let carrier = carrier_context();
    // SAFETY: `carrier` is the context this carrier saved when it switched
    // into this task, and `task.sp` is this task's own slot, which nothing
    // else touches while the task is running.
    unsafe { switch::buri_rt_task_switch(task.sp.get(), carrier) };
}

/// The scope a parked task left behind, and where it is put back.
fn task_arena(task: &Task) -> crate::memory::ArenaSlot {
    match task.arena.lock() {
        Ok(g) => *g,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn set_task_arena(task: &Task, slot: crate::memory::ArenaSlot) {
    match task.arena.lock() {
        Ok(mut g) => *g = slot,
        Err(poisoned) => *poisoned.into_inner() = slot,
    }
}

/// What every carrier thread does, for the life of the process.
///
/// **This loop is the slice.** Before B9 a carrier ran a job to completion and
/// a job that waited held the thread; now it runs a task until the task parks
/// or finishes, and either way it is back here with a thread to spend on
/// something else.
fn carrier_loop() {
    let mut armed = false;
    loop {
        let task = take(armed);
        armed = false;
        set_running(Arc::as_ptr(&task));
        task.state.store(RUNNING, Ordering::Release);
        // G5: the arena belongs to the task, not to the thread it is on this
        // turn. The carrier's own slot goes aside, the task's comes in, and the
        // two are exchanged again on the way back — so a task that parks inside
        // a `core/alloc::scoped` finds its arena on whichever carrier resumes
        // it, and a carrier between tasks is inside no scope at all.
        let carrier_arena = crate::memory::arena_slot_of_carrier();
        crate::memory::set_arena_slot_of_carrier(task_arena(&task));
        // SAFETY: the task came off the queue, so no other carrier is running
        // it, and its saved context is either the frame `spawn_task` prepared
        // or one this very call wrote on a previous turn. The `Arc` held here
        // keeps the task — and the stack under that context — alive for the
        // whole of it.
        unsafe { switch::buri_rt_task_switch(carrier_slot(), *task.sp.get()) };
        set_task_arena(&task, crate::memory::arena_slot_of_carrier());
        crate::memory::set_arena_slot_of_carrier(carrier_arena);
        set_running(std::ptr::null());

        if task.why.load(Ordering::Acquire) == WHY_DONE {
            // On the carrier's stack again, which is the only place the task's
            // own stack can be given back from.
            //
            // SAFETY: `task.stack` came from `buri_rt_task_stack_acquire` and
            // nothing is running on it — the task's last act was to switch off
            // it, and its state is `FINISHED`, so no waker will queue it.
            unsafe { crate::memory::buri_rt_task_stack_release(task.stack) };
            // Available *before* anybody is told the answer is ready, so a
            // caller that dispatches again immediately finds this carrier.
            arm();
            armed = true;
            task.done.store(true, Ordering::Release);
            wake_waiters(&task);
        } else if task
            .state
            .compare_exchange(PARKING, PARKED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // `NOTIFIED_PARKING`: a waker reached the task while its context
            // was still being saved and left the queueing here, where the
            // context is known to be down.
            task.state.store(QUEUED, Ordering::Release);
            push(task);
        }
    }
}

/// The entry every task is reached through, on its own stack.
///
/// **A `buri_rt_task_*` symbol, and the first one this runtime has.** `rt.rs`
/// §2 refused to export one before a backend had a call to emit, because a
/// signature with no caller is a guess. This one's caller is
/// `switch_*.s`'s `buri_rt_task_launch`, which is in this repository, in this
/// slice, and cannot disagree with it: the launch pad moves the first
/// callee-saved register into the first argument register and calls this, and
/// `switch::prepare` is what put the argument there. The Buri-facing task
/// vocabulary — start, join, is-live — is still Rust-only, still for the
/// reason §2 gives, and still track F's to give a caller.
///
/// It never returns: a task that has finished has nowhere to return *to*, its
/// caller being a frame this runtime wrote by hand. The last thing it does is
/// switch to the carrier, which reaps it.
///
/// **The panic guard is not decoration.** The runtime archive is built
/// `panic = "abort"`, so in a shipped program `catch_unwind` never catches
/// anything; under a test harness, which unwinds, a body that panics would
/// otherwise unwind into a frame with no landing pad above it and take the
/// process out. Catching it here keeps the old shape exactly: `Handoff::join`
/// answers `None`, and `fan_out`'s `finish` turns that into *"a buri task did
/// not finish"*.
///
/// # Safety
/// `arg` is the address of a live `Task` whose `Arc` the carrier holds, and
/// this is the first and only entry into that task.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_task_main(arg: *mut u8) -> ! {
    // SAFETY: the carrier planted the address of the task it is holding.
    let task: &Task = unsafe { &*arg.cast::<Task>() };
    let body = match task.body.lock() {
        Ok(mut slot) => slot.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    if let Some(body) = body {
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        task.ok.store(ran.is_ok(), Ordering::Release);
    }
    task.why.store(WHY_DONE, Ordering::Release);
    task.state.store(FINISHED, Ordering::Release);
    leave(task);
    // Nothing switches back into a finished task, and a runtime that found
    // itself here would be running on a stack it had already given back.
    std::process::abort()
}

/// Wake everybody waiting for a task, once.
fn wake_waiters(task: &Task) {
    let waiters = {
        let mut held = match task.waiters.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        std::mem::take(&mut *held)
    };
    for waker in waiters {
        waker.wake();
    }
}

/// Map a task's machine stack, build the frame it starts from, and queue it.
fn spawn_task(body: Box<dyn FnOnce() + Send>) -> Arc<Task> {
    let (base, top) = crate::memory::buri_rt_task_stack_acquire();
    let task = Arc::new(Task {
        state: AtomicU8::new(QUEUED),
        why: AtomicU8::new(WHY_PARK),
        sp: UnsafeCell::new(std::ptr::null_mut()),
        stack: base,
        blocks: UnsafeCell::new(Blocks::new()),
        body: Mutex::new(Some(body)),
        done: AtomicBool::new(false),
        ok: AtomicBool::new(false),
        waiters: Mutex::new(Vec::new()),
        arena: Mutex::new(crate::memory::ArenaSlot::NONE),
    });
    // The task's *own* address travels in the frame, and the `Arc` that keeps
    // it alive travels on the queue: the launch pad hands the address back and
    // `buri_rt_task_main` borrows it.
    let arg = Arc::as_ptr(&task).cast_mut().cast::<u8>();
    // SAFETY: `top` is the high end of a mapping just made, nothing is running
    // on it, and `task.sp` is written before the task is queued, so no carrier
    // can read it half-built.
    unsafe { *task.sp.get() = switch::prepare(top, arg) };
    push(Arc::clone(&task));
    task
}

/// What [`on_carrier`] answers: the value the task produced, once it has.
///
/// The answer travels in a slot rather than out of the switch because the
/// carrier loop is what reaps a task and it does not know `T`. `join` answers
/// `None` where the task did not finish — which, under `panic = "abort"`,
/// cannot happen in a released runtime, and can happen under a test harness
/// that unwinds.
#[must_use = "dropping the handoff drops the answer the task is computing"]
pub struct Handoff<T> {
    task: Arc<Task>,
    answer: Arc<Mutex<Option<T>>>,
}

impl<T> Handoff<T> {
    /// Wait for the task and answer what it produced.
    ///
    /// **Through [`park_on`]**, which is the difference B9 makes to joining: a
    /// task that joins another task parks rather than blocking, so a nested
    /// fan-out costs one carrier for the whole tree instead of one per level.
    /// A join from a thread that is not a task is the `block_on` it always
    /// was.
    pub fn join(self) -> Option<T> {
        park_on(Complete(&self.task));
        if !self.task.ok.load(Ordering::Acquire) {
            return None;
        }
        match self.answer.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }
}

/// Ready when a task has finished and its stack has been given back.
///
/// Borrows the task rather than owning it, because the only caller has one on
/// its own frame and a future that outlives the frame it was polled on is not
/// a shape this runtime has.
struct Complete<'a>(&'a Arc<Task>);

impl Future for Complete<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.done.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        let mut waiters = match self.0.waiters.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Under the lock, because `wake_waiters` takes the same one: a waiter
        // registered after the drain and before the flag would never be woken.
        if self.0.done.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        waiters.push(cx.waker().clone());
        Poll::Pending
    }
}

/// Run `f` as a task, and answer the handle that waits for it.
///
/// The name is what it always was and so is the contract; what is underneath
/// it is a stack switch rather than a thread. A carrier is started only if
/// none is idle (see [`push`]), so a program that fans out over work that
/// waits keeps the handful of threads it started with.
pub fn on_carrier<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Handoff<T> {
    let answer = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&answer);
    let task = spawn_task(Box::new(move || {
        let out = f();
        match slot.lock() {
            Ok(mut slot) => *slot = Some(out),
            Err(poisoned) => *poisoned.into_inner() = Some(out),
        }
    }));
    Handoff { task, answer }
}

// ---------------------------------------------------------------------------
// The task handle table
// ---------------------------------------------------------------------------

/// One task's state.
///
/// The shape `testing.rs`'s table already uses, for the same reason: a handle
/// is a *position* in one vector, so every task-shaped Buri value — `Address<M>`
/// and `Reply<R>` when track F writes them — is a `struct X(I64)` and the
/// runtime side is one table rather than one per vocabulary.
///
/// The design's `Task` carries two more fields that are two other slices': a
/// `StackBlock` from `buri_rt_stack_acquire` (B7) and a mailbox `Sender` (F6).
/// Both are additions to this enum, neither changes the handle.
enum Slot {
    /// Started, not yet joined. The receiver answers when the body returns.
    Running(Handoff<()>),
    /// Joined. **Tombstoned rather than reused within a run**, so that a handle
    /// held past its task's life names a dead task instead of somebody else's
    /// live one.
    Done,
}

static TASKS: Mutex<Vec<Slot>> = Mutex::new(Vec::new());

fn tasks() -> MutexGuard<'static, Vec<Slot>> {
    match TASKS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Record a slot and answer the handle that names it.
fn install(slot: Slot) -> i64 {
    let mut table = tasks();
    table.push(slot);
    (table.len() as i64) - 1
}

/// Start `f` on a carrier as a task, and answer its handle.
///
/// `f` runs **beside its starter**, which is what changed in G3: a task used
/// to take the run baton before its first instruction, so a task started from
/// Buri code waited for the starter to suspend. Now it runs, and the values it
/// and its starter share are the marked ones (§1).
///
/// Nothing in a Buri program reaches this yet — track F's `core/actor` is what
/// does — so what the change costs today is one line of this file and one
/// assertion in `a_task_runs_beside_the_thread_that_started_it`.
pub fn task_start(f: impl FnOnce() + Send + 'static) -> i64 {
    install(Slot::Running(on_carrier(f)))
}

/// Wait for a task and tombstone its slot. `false` where the handle names
/// nothing, or a task that was already joined.
///
/// A handle that names nothing answers `false` rather than aborting, for the
/// reason `testing.rs` gives: it cannot arise from a program, and a runtime
/// that aborted on it would report a toolchain bug as a program error.
pub fn task_join(handle: i64) -> bool {
    let taken = {
        let mut table = tasks();
        match usize::try_from(handle).ok().and_then(|i| table.get_mut(i)) {
            Some(slot @ Slot::Running(_)) => match std::mem::replace(slot, Slot::Done) {
                Slot::Running(handoff) => Some(handoff),
                Slot::Done => None,
            },
            _ => None,
        }
    };
    // Outside the lock: joining under it would stop every other task from
    // being started or joined for the length of this one.
    match taken {
        Some(handoff) => handoff.join().is_some(),
        None => false,
    }
}

/// Whether the handle names a task that has been started and not joined.
pub fn task_is_live(handle: i64) -> bool {
    let table = tasks();
    matches!(
        usize::try_from(handle).ok().and_then(|i| table.get(i)),
        Some(Slot::Running(_))
    )
}

// ---------------------------------------------------------------------------
// `Tasks.parallel`
// ---------------------------------------------------------------------------
//
// The first exported symbol this file has, and §2 said why there was none: an
// exported symbol is a contract both backends emit calls into, so it lands the
// day a backend has a call to emit. This is that day — `core/tasks` is written,
// `host.HostTasks.parallel` has a row in both runtime tables, and the C
// signature below is the one those two rows describe rather than a prediction
// of one.
//
// **Parallel, on the carrier pool, answering in index order.** Every step is
// dispatched to a carrier of its own and they run at the same time — two that
// wait overlap and, since G3, two that *compute* overlap as well. What used to
// stop the second was the run baton, and what stops it being a race now is
// that the blocks the steps share are marked and therefore counted atomically
// (§1). The allocator was already thread-safe; the counts and the in-place
// write licence are what the mark buys.
//
// The order promise is kept the way `core/tasks` says it is: the `[B]` block is
// allocated up front and each step writes its own element into it, so the answer
// is in the *items'* order whatever order the work finished in, and nothing is
// sorted afterwards.
//
// It lives in this file, behind `net`, rather than in `list.rs`, and that is
// the reason `backend/runtime_native.rs::net_intrinsic` already names
// `host.HostTasks.*`: a toolchain whose archive has no reactor refuses the key
// with a sentence before code generation.

/// How many steps of one `parallel` may be in flight at once.
///
/// **Sixty-four until B9, and the reason for the number changed rather than
/// going away.** It used to bound *threads*: a step was an OS thread, a list
/// of ten thousand items would have asked the kernel for ten thousand of them,
/// and the kernel refuses — measured on this platform, `pthread_create`
/// answers `EAGAIN` on the 8 192nd — which the old `spawn_carrier` turned into
/// an abort.
///
/// A step is now a pair of mappings and no thread, so what the window bounds
/// is **address space**: a task reserves `memory::BURI_RT_STACK_BYTES` for its
/// machine stack and, once it enters Buri code, the same again for its Buri
/// data stack, which is 130 MiB of *reservation* — not of resident memory —
/// each. A thousand and twenty-four of them is about 133 GiB against the 128
/// TiB a 47-bit user address space offers, and measured: ten thousand pairs
/// map without complaint and cost 321 MiB resident.
///
/// The design row says B9 *removes* the bound rather than raising it. It is
/// raised sixteen-fold instead, and the reason is the sentence above: the
/// thing being spent stopped being a thread and started being a reservation,
/// which is cheap but not free, and a `parallel` over a million items should
/// slide a window rather than ask for 130 TiB. The window's behaviour is
/// unchanged — the (n + 1)-th step starts when the first has finished, and the
/// answer and its order are the same either way.
const IN_FLIGHT: usize = 1024;

/// One `parallel` call's boundary, in a shape a carrier can be handed.
///
/// The four words after `len` are [`crate::StepEntry`]'s ABI, which
/// `buri_rt_list_map_ctx_step` established and which `cli/runtime/lib.rs` §2
/// rule 5 states: the generated entry thunk, the backend's opaque record, and
/// the two strides. Gathered into one `Copy` value because a fan-out hands the
/// same six things to every step and differs only in the index.
#[derive(Clone, Copy)]
struct Steps {
    entry: StepEntry,
    state: *mut u8,
    src: *const u8,
    dst: *mut u8,
    in_stride: usize,
    out_stride: usize,
}

// SAFETY: the three pointers are the call's own — a source the caller keeps
// alive for the whole of the call, the record the backend generated for it, and
// a destination block allocated here and not published until every step has
// written into it. Each step reads and writes at its own index, so no two
// carriers touch the same byte of the source or the answer. The blocks they
// *both* reach — the closure's record, the caller's context, an element that
// appears twice in the list — are reached through reference operations, and
// those are safe because `buri_rt_host_tasks_parallel` fans out only where
// every block in the program carries `CAP_SHARED_FLAG` (§1).
unsafe impl Send for Steps {}

impl Steps {
    /// The `i`-th step: its own element in, its own slot out.
    ///
    /// # Safety
    /// `i` is below the `len` the caller promised, and this is the only call
    /// for that `i`.
    unsafe fn run(self, i: usize) {
        // SAFETY: `i * in_stride` is inside the `n`-element source the caller
        // promised, and `i * out_stride` inside the block just allocated. The
        // thunk is the one the backend generated for these two element types.
        unsafe {
            (self.entry)(
                self.state,
                i as u64,
                self.src.add(i.saturating_mul(self.in_stride)),
                self.dst.add(i.saturating_mul(self.out_stride)),
            );
        }
    }
}

/// Every step on the calling carrier, one after another, in index order.
///
/// What this file did before there was a fan-out, and still the answer where
/// either of the artifact's two statements about itself is missing — a program
/// that shares one Buri stack (`lib.rs`'s `buri_rt_frames_are_per_carrier`) or
/// one whose blocks are not marked (`memory::values_may_cross_tasks`) — and
/// for the one- and no-item cases, where a carrier would be a thread started
/// to do what this thread is already doing.
///
/// **This arm is the safe answer, not merely the slow one.** It is what a
/// toolchain that forgot to emit either statement gets, which is why both
/// statements are gates here rather than assertions: a missing one is a
/// program that computes the same `[B]` a little slower, and never two
/// carriers counting an unmarked block.
///
/// # Safety
/// `steps` describes `n` live elements and `n` writable slots.
unsafe fn in_order(steps: Steps, n: usize) {
    for i in 0..n {
        // SAFETY: `i < n`, and each index is run exactly once.
        unsafe { steps.run(i) };
    }
}

/// Every step on a carrier of its own, at most [`IN_FLIGHT`] at a time —
/// **genuinely at once**, which is G3's half of this function.
///
/// Until G3 the calling carrier gave the run baton up here before the first
/// dispatch and took it back after the last join, and the steps then took it
/// one at a time: two that waited overlapped and two that computed did not.
/// The baton is gone, so both overlap, and what makes that safe is not
/// anything this function does but that every block the program allocated
/// carries `CAP_SHARED_FLAG` — see §1 and [`buri_rt_host_tasks_parallel`]'s
/// gate, which is the run-time proof rather than an assumption about callers.
///
/// The **window** survives the baton and is unrelated to it: a step that
/// finishes during the dispatch loop puts its carrier back in the pool in time
/// for the next index to reuse it, so a `parallel` over items that do not wait
/// costs a handful of threads rather than one per item.
///
/// This thread dispatches and waits. It runs no Buri code between the first
/// dispatch and the last join — not because it is excluded but because there
/// is nothing here for it to run — so a **nested** fan-out is a carrier
/// dispatching to other carriers, and needs nothing given up first.
/// `a_nested_fan_out_answers_the_same_numbers` is that case, and it is what
/// `a_nested_fan_out_gives_the_baton_up_first` became.
///
/// # Safety
/// As [`in_order`].
unsafe fn fan_out(steps: Steps, n: usize) {
    let mut window: VecDeque<Handoff<()>> = VecDeque::with_capacity(n.min(IN_FLIGHT));
    for i in 0..n {
        if window.len() == IN_FLIGHT {
            finish(window.pop_front());
        }
        // SAFETY: `i < n`, each index dispatched once, and `Steps` is `Send`
        // for the reason stated at its `unsafe impl`.
        window.push_back(on_carrier(move || unsafe { steps.run(i) }));
    }
    while let Some(handoff) = window.pop_front() {
        finish(Some(handoff));
    }
}

/// Wait for one dispatched step.
///
/// A carrier that did not finish leaves its slot of the answer unwritten, and
/// handing that back would be a `[B]` with a hole in it — so it is named here
/// instead. Under `panic = "abort"`, which is how the runtime archive is built,
/// it cannot happen at all; under a test harness that unwinds it can, and this
/// is the difference between a failed test and a garbage answer.
fn finish(handoff: Option<Handoff<()>>) {
    if let Some(handoff) = handoff {
        assert!(handoff.join().is_some(), "a buri task did not finish");
    }
}

/// `Tasks.parallel(self, ctx, items, f) -> [B]` — `f` at every item, answering
/// in index order.
///
/// The `[B]` block is allocated here and every element of it is written by a
/// step, so the result is fully initialised before it is handed back — there is
/// no arm below that skips one.
///
/// Whether the steps run on carriers of their own or one after another on this
/// one is the artifact's answer, not this call's, and it takes **two**
/// statements rather than one: [`crate::buri_rt_frames_are_per_carrier`] and
/// [`crate::memory::buri_rt_values_may_cross_tasks`] (§2). Either way the `[B]`
/// is the same `[B]`, which is what `core/tasks`'s order promise is worth.
///
/// `in_order` below is `buri_rt_list_map_ctx_step`'s walk, and the two stay
/// separate for the reason D3 gave when they were the same three lines: they are
/// the same *code* and different *contracts*. One is a pilot for the boundary that
/// may be deleted the day a real key uses it; this one is `Tasks.parallel`'s body,
/// and it is now the arm a scheduler falls back to rather than the whole of it.
///
/// # Safety
/// `ptr` covers `len * in_stride` bytes; `entry` is the thunk the backend
/// generated for this call and `state` the record it was generated against;
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_tasks_parallel(
    ptr: *const u8,
    len: u64,
    entry: StepEntry,
    state: *mut u8,
    in_stride: u64,
    out_stride: u64,
    out: *mut BuriList,
) {
    let (n, from, to) = (len as usize, in_stride as usize, out_stride as usize);
    let result = block(n, to);
    let steps = Steps {
        entry,
        state,
        src: ptr,
        dst: result.ptr,
        in_stride: from,
        out_stride: to,
    };
    // SAFETY: the caller's `n` elements, at the strides it named.
    //
    // **Two gates, two different facts** (§2). The frames one is the backend's
    // — a second carrier must have somewhere of its own to put a frame. The
    // marking one is the program's — the blocks two carriers would both count
    // must carry `CAP_SHARED_FLAG`, or the counts race. Neither implies the
    // other and neither is assumed: an artifact that made only one of the two
    // calls gets `in_order`, which answers the same `[B]`.
    unsafe {
        if n > 1 && crate::frames_are_per_carrier() && crate::memory::values_may_cross_tasks() {
            fan_out(steps, n);
        } else {
            in_order(steps, n);
        }
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(result) }
}

// ---------------------------------------------------------------------------
// `core/actor`
// ---------------------------------------------------------------------------
//
// Nine exported entries, and between them they are the only place in this
// runtime that **keeps a pointer it was passed**. `lib.rs` §3's third bullet is
// amended for them and for nothing else, because an actor is by definition a
// value that outlives the call which handed it over: a mailbox holds messages
// between the `send` that posted one and the step that reads it, and the state
// sits in the table between two steps.
//
// What makes that expressible against a runtime compiled once and against no
// Buri type is the shape `core/actor` crosses in: **a one-element `[T]`**. A
// `[T]` is `{ ptr, len }` whatever `T` is (VALUE-MODEL.md §4), so a message is
// two words here and nothing about its element ever crosses — no stride, no
// retain glue, no descriptor. The runtime takes a reference on the block
// ([`Held::keep`]) and gives that same reference back ([`Held::give`]); every
// block it takes is handed to somebody, so the count balances without this file
// ever knowing how to walk one. `core/actor`'s own `Carried<T>` wrapper is what
// keeps such a block from being *empty*, which is the one case a null `ptr`
// would make unreadable.
//
// The scheduling is `core/actor`'s, in Buri: this file holds the queue, the
// state, the reply slots and the two waits, and never calls a Buri closure. A
// step is entered by the task that drove it, from `core/actor::draining`, and
// the arm where an actor has a carrier of its own is the one that needs a step
// record outliving its call — §2's undefined half, still.

/// How many messages wait in a mailbox that asked for no number.
///
/// It is **also** written in `core/actor` as `MAILBOX`, and the two must agree:
/// the bound is enforced from both sides — this file refuses to take a message
/// past it, and `core/actor::send` runs the mailbox down when it reaches it —
/// so a bound only one side knew would be a bound the other could not respect.
/// `the_default_mailbox_is_the_one_core_actor_names` is that agreement as a
/// test.
pub const MAILBOX: i64 = 64;

/// One Buri block this runtime is holding on a program's behalf.
///
/// The two words of a `[T]`, and a reference on the block behind them. It is
/// deliberately not a `BuriList`: that type is the *ABI* shape and is copied
/// freely, and this one is an **owned** reference with a rule attached — it was
/// increfed when it was taken and the count is given away when it is handed
/// back, exactly once.
#[derive(Clone, Copy)]
struct Held {
    ptr: *mut u8,
    len: u64,
}

// SAFETY: the pointer names a Buri block, and a program that reaches this file
// at all is one `middle::rc::crosses_tasks` marked — `actor.` is on that list —
// so every block it allocated carries `CAP_SHARED_FLAG` and is counted
// atomically (§1). Moving one between carriers is therefore what the mark was
// bought for, and the queue below is exactly the hand-off it describes.
unsafe impl Send for Held {}

impl Held {
    /// Take a reference on the caller's block, so that the release the caller
    /// already emits for its own reference does not free it.
    ///
    /// A null `ptr` is an empty block and there is nothing to count;
    /// `core/actor` does not produce one — every carrier it builds has a
    /// non-zero stride, which is what `Carried<T>` is for — and this is total
    /// anyway, because a runtime that aborted on it would report a toolchain
    /// bug as a program error.
    fn keep(list: BuriList) -> Held {
        // SAFETY: `ptr` is null or a live payload pointer the caller owns for
        // the duration of the call (`lib.rs` §3's borrowed-parameter rule).
        unsafe { crate::memory::buri_rt_incref(list.ptr) };
        Held { ptr: list.ptr, len: list.len }
    }

    /// Give the reference away. The caller of the entry that answers this owns
    /// it now, and its own release is what eventually frees the block.
    fn give(self) -> BuriList {
        BuriList { ptr: self.ptr, len: self.len }
    }
}

/// One actor: a bounded queue, a state, and the two waits that order them.
struct Mailbox {
    /// Posted and not yet stepped, oldest first. One sender's messages are in
    /// the order it sent them because a `VecDeque` is, and because the permit
    /// below is handed out in the order it was asked for.
    queue: VecDeque<Held>,
    /// `Carried<S>`, or `None` while a driver has it — and after the close,
    /// which is what makes `mailboxClose` answer once.
    state: Option<Held>,
    /// Set by `mailboxClose`. A closed mailbox takes no more messages and
    /// hands out no more state; it still gives back what is already in it,
    /// which is what `core/actor::stop`'s discard loop reads.
    closed: bool,
    /// One permit per free slot. `mailboxPush` acquires one and `mailboxPop`
    /// gives it back, so a full mailbox is a `send` that waits rather than a
    /// queue that grows. Closing it is what turns a waiting `send` into
    /// `.Err(.Stopped)` instead of a wait nothing would end.
    room: Arc<tokio::sync::Semaphore>,
    /// Exactly one permit, and holding it *is* holding the state. It is what
    /// makes a step exclusive without a lock held across a call into Buri
    /// code, and what `mailboxClose` waits on so that a `stop` racing a step
    /// lets that step finish.
    baton: Arc<tokio::sync::Semaphore>,
}

/// Every actor this program has started. Tombstoned rather than reused, for
/// [`Slot`]'s reason: a handle held past its actor's life names a dead actor
/// and not somebody else's live one.
static ACTORS: Mutex<Vec<Mailbox>> = Mutex::new(Vec::new());

fn actors() -> MutexGuard<'static, Vec<Mailbox>> {
    match ACTORS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// One `ask`'s answer, on its way back.
enum Answer {
    /// Opened by `replyOpen` and not yet answered.
    Waiting,
    /// Answered and not yet read.
    Ready(Held),
    /// Read. A second `replyTake` answers `.None` from here, which is what
    /// makes an answer arrive once.
    Spent,
}

/// The reply slots, and the generation of each, beside the free list.
///
/// **Reused, unlike an actor's slot, and the generation is why that is safe.**
/// A server answering a million `ask`s opens a million reply slots, so a table
/// that only grew would be a leak proportional to the program's uptime. A
/// handle is `generation << 20 | index`, so a stale one — held past the answer
/// it named — finds a generation that has moved on and is `.None` rather than
/// somebody else's answer. Twenty bits of index is a million live slots at
/// once, which is a different and much smaller number than a million over a
/// lifetime.
struct Slots {
    /// One `(generation, answer)` per index, never shrinking.
    slots: Vec<(u64, Answer)>,
    /// The indices a `replyTake` gave back, ready for the next `replyOpen`.
    free: Vec<usize>,
}

static REPLIES: Mutex<Slots> = Mutex::new(Slots { slots: Vec::new(), free: Vec::new() });

/// The index half of a reply handle.
const REPLY_INDEX_BITS: u32 = 20;

/// The mask that reads it back.
const REPLY_INDEX_MASK: u64 = (1 << REPLY_INDEX_BITS) - 1;

fn replies() -> MutexGuard<'static, Slots> {
    match REPLIES.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// `actor.mailboxOpen(ctx, state, bound) -> Int`.
///
/// # Safety
/// `ptr` is null or a live `[Carried<S>]` block the caller owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_actor_mailbox_open(ptr: *mut u8, len: u64, bound: i64) -> i64 {
    // A bound that is not a positive number is [`MAILBOX`]. `core/actor` always
    // passes one — it has to, because it enforces the bound itself — so this is
    // the fallback for a caller that has not been written yet rather than the
    // path a program takes.
    let asked = if bound > 0 { bound } else { MAILBOX };
    let room = usize::try_from(asked).unwrap_or(MAILBOX as usize).max(1);
    let mut table = actors();
    table.push(Mailbox {
        queue: VecDeque::new(),
        state: Some(Held::keep(BuriList { ptr, len })),
        closed: false,
        room: Arc::new(tokio::sync::Semaphore::new(room)),
        baton: Arc::new(tokio::sync::Semaphore::new(1)),
    });
    (table.len() as i64) - 1
}

/// The mailbox a handle names, or nothing.
///
/// A handle that names none cannot arise from a program — `core/actor` mints
/// every one of them — so this answers `None` rather than aborting, for the
/// reason `task_join` gives at its own table.
fn at(table: &mut [Mailbox], handle: i64) -> Option<&mut Mailbox> {
    usize::try_from(handle).ok().and_then(move |i| table.get_mut(i))
}

/// `actor.mailboxPush(ctx, handle, message) -> Option<Int>` — the number
/// waiting, or `.None` for a closed mailbox.
///
/// **Waits while the mailbox is full**, on the permit `mailboxPop` gives back.
/// `core/actor::send` runs the mailbox down after every post that reaches the
/// bound, so a single-task program never reaches this wait; a second task
/// posting into an actor somebody else drives is what does.
///
/// # Safety
/// `ptr` is null or a live `[Carried<M>]` block the caller owns; `out` is
/// writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_actor_mailbox_push(
    handle: i64,
    ptr: *mut u8,
    len: u64,
    out: *mut i64,
) -> i32 {
    let room = {
        let mut table = actors();
        match at(&mut table, handle) {
            Some(mailbox) if !mailbox.closed => Arc::clone(&mailbox.room),
            _ => return 0,
        }
    };
    // Outside the table's lock: the permit that frees this one is given back by
    // `mailboxPop`, which takes the same lock.
    let Ok(permit) = park_on(room.acquire()) else {
        // The semaphore was closed while this call waited, which is `stop`.
        return 0;
    };
    permit.forget();
    let mut table = actors();
    let Some(mailbox) = at(&mut table, handle) else { return 0 };
    if mailbox.closed {
        // Closed between the permit and the lock. The permit is not given back
        // — a closed semaphore hands out no more anyway — and the block is not
        // taken, so the caller's own release frees it.
        return 0;
    }
    mailbox.queue.push_back(Held::keep(BuriList { ptr, len }));
    let waiting = mailbox.queue.len() as i64;
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(waiting) };
    crate::BURI_OK
}

/// `actor.mailboxPop(ctx, handle) -> Option<[Carried<M>]>` — the oldest
/// message, or `.None` for an empty mailbox. A closed mailbox still hands back
/// what is in it, which is what `core/actor::stop`'s discard loop reads.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_actor_mailbox_pop(handle: i64, out: *mut BuriList) -> i32 {
    let mut table = actors();
    let Some(mailbox) = at(&mut table, handle) else { return 0 };
    let Some(held) = mailbox.queue.pop_front() else { return 0 };
    mailbox.room.add_permits(1);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(held.give()) };
    crate::BURI_OK
}

/// `actor.mailboxClose(ctx, handle) -> Option<[Carried<S>]>` — closes, once,
/// and answers the final state.
///
/// **Waits for the baton**, so a `stop` that races a step lets that step
/// finish. A second close answers `.None` without waiting, which is what makes
/// `core/actor`'s `onStop` run exactly once.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_actor_mailbox_close(handle: i64, out: *mut BuriList) -> i32 {
    let baton = {
        let mut table = actors();
        match at(&mut table, handle) {
            Some(mailbox) if !mailbox.closed => {
                mailbox.closed = true;
                // Every `send` waiting for room is answered `.Err(.Stopped)`
                // rather than left waiting for a drain that will not come.
                mailbox.room.close();
                Arc::clone(&mailbox.baton)
            }
            _ => return 0,
        }
    };
    // Outside the lock: the step this waits for is Buri code, and it takes the
    // lock itself when it puts the state back.
    if let Ok(permit) = park_on(baton.acquire()) {
        // Kept, never given back: the baton is what a `stateTake` needs, so
        // holding it forever is what makes a stopped actor unsteppable.
        permit.forget();
    }
    let mut table = actors();
    let Some(mailbox) = at(&mut table, handle) else { return 0 };
    let Some(held) = mailbox.state.take() else { return 0 };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(held.give()) };
    crate::BURI_OK
}

/// `actor.stateTake(ctx, handle) -> Option<[Carried<S>]>` — the state, and with
/// it the right to step this actor.
///
/// Never waits. `.None` is "somebody else is stepping it, or it has stopped",
/// and `core/actor::drive` reads both as "not mine to run" — which is what
/// keeps two carriers from stepping one actor at once without either of them
/// blocking.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_actor_state_take(handle: i64, out: *mut BuriList) -> i32 {
    let mut table = actors();
    let Some(mailbox) = at(&mut table, handle) else { return 0 };
    if mailbox.closed {
        return 0;
    }
    let Ok(permit) = mailbox.baton.try_acquire() else { return 0 };
    permit.forget();
    let Some(held) = mailbox.state.take() else {
        mailbox.baton.add_permits(1);
        return 0;
    };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(held.give()) };
    crate::BURI_OK
}

/// `actor.statePut(ctx, handle, state) -> Option<Int>` — the state back, and
/// the number of messages waiting.
///
/// The block is taken on every path where the actor exists, **including a
/// closed one**: a close is waiting on the baton this call gives back, and the
/// state it is waiting for is this one.
///
/// # Safety
/// `ptr` is null or a live `[Carried<S>]` block the caller owns; `out` is
/// writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_actor_state_put(
    handle: i64,
    ptr: *mut u8,
    len: u64,
    out: *mut i64,
) -> i32 {
    let mut table = actors();
    let Some(mailbox) = at(&mut table, handle) else { return 0 };
    mailbox.state = Some(Held::keep(BuriList { ptr, len }));
    let waiting = mailbox.queue.len() as i64;
    mailbox.baton.add_permits(1);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(waiting) };
    crate::BURI_OK
}

/// `actor.replyOpen(ctx) -> Int` — a fresh slot, and the handle that names it.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_actor_reply_open() -> i64 {
    let mut table = replies();
    match table.free.pop() {
        Some(index) => {
            let generation = table.slots[index].0;
            table.slots[index].1 = Answer::Waiting;
            ((generation << REPLY_INDEX_BITS) | index as u64) as i64
        }
        None => {
            table.slots.push((0, Answer::Waiting));
            (table.slots.len() as i64) - 1
        }
    }
}

/// The slot a reply handle names, checked against its generation.
fn reply_at(slots: &mut [(u64, Answer)], handle: i64) -> Option<&mut Answer> {
    let handle = u64::try_from(handle).ok()?;
    let index = usize::try_from(handle & REPLY_INDEX_MASK).ok()?;
    let generation = handle >> REPLY_INDEX_BITS;
    let slot = slots.get_mut(index)?;
    (slot.0 == generation).then_some(&mut slot.1)
}

/// `actor.replyPut(ctx, handle, value) -> Option<Int>` — the answer, once.
///
/// # Safety
/// `ptr` is null or a live `[Carried<R>]` block the caller owns; `out` is
/// writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_actor_reply_put(
    handle: i64,
    ptr: *mut u8,
    len: u64,
    out: *mut i64,
) -> i32 {
    let mut table = replies();
    let Some(slot) = reply_at(&mut table.slots, handle) else { return 0 };
    if !matches!(slot, Answer::Waiting) {
        return 0;
    }
    *slot = Answer::Ready(Held::keep(BuriList { ptr, len }));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(0) };
    crate::BURI_OK
}

/// `actor.replyTake(ctx, handle) -> Option<[Carried<R>]>` — the answer, once,
/// and the slot back for the next `ask`.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_actor_reply_take(handle: i64, out: *mut BuriList) -> i32 {
    let mut table = replies();
    let Some(slot) = reply_at(&mut table.slots, handle) else { return 0 };
    let held = match std::mem::replace(slot, Answer::Spent) {
        Answer::Ready(held) => held,
        // Put back exactly what was there. An unanswered slot stays
        // unanswered — a `replyPut` still to come is the ordinary case, and
        // spending it here would lose the answer — and a spent one stays
        // spent.
        other => {
            *slot = other;
            return 0;
        }
    };
    // The slot is free for reuse, at the next generation, so the handle just
    // spent names nothing from here on.
    let index = (handle as u64 & REPLY_INDEX_MASK) as usize;
    table.slots[index].0 = table.slots[index].0.wrapping_add(1);
    table.free.push(index);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(held.give()) };
    crate::BURI_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::channel;
    use std::thread::ThreadId;
    use std::time::{Duration, Instant};

    /// The pool and the task table are one shared thing, and `cargo test` runs
    /// this module's cases on many threads at once. A case that asks "was the
    /// idle carrier reused" is asking about global state, so the cases that do
    /// take this first. Nothing outside this file touches either, so it is a
    /// lock over this module and not over the crate.
    ///
    /// The **marking latch** is the one piece of global state this module
    /// shares with others, and it has a lock of its own: `memory::latch`, taken
    /// after this one by the cases that need both.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn alone() -> MutexGuard<'static, ()> {
        match ONE_AT_A_TIME.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// One `parallel` call, with the arm chosen here rather than read out of
    /// the artifact's answer.
    ///
    /// The two arms are the whole of what the exported entry decides between,
    /// so a case that is about the *walk* asks for one directly and says
    /// nothing about a global every other case can see. The one case that is
    /// about the decision drives the entry itself.
    ///
    /// # Safety
    /// `src` covers `n * in_stride` bytes and `entry` is a thunk written for
    /// these two element types.
    unsafe fn steps_of(
        src: *const u8,
        n: usize,
        entry: StepEntry,
        state: *mut u8,
        in_stride: usize,
        out_stride: usize,
        concurrent: bool,
    ) -> BuriList {
        let result = block(n, out_stride);
        let steps = Steps { entry, state, src, dst: result.ptr, in_stride, out_stride };
        // SAFETY: the caller's promise, forwarded.
        unsafe {
            if concurrent {
                fan_out(steps, n);
            } else {
                in_order(steps, n);
            }
        }
        result
    }

    /// Read an `i64` answer back.
    ///
    /// # Safety
    /// `got` holds `n` `i64`s at a stride of eight.
    unsafe fn i64s(got: &BuriList, n: usize) -> Vec<i64> {
        // SAFETY: the caller's promise.
        (0..n).map(|i| unsafe { got.ptr.add(i * 8).cast::<i64>().read() }).collect()
    }

    /// **The count of a marked block is exact under every carrier at once.**
    ///
    /// The invariant the run baton used to provide, restated as the thing that
    /// replaced it. Eight carriers each `incref` a marked block a thousand
    /// times; the count is one plus eight thousand or an update was lost.
    ///
    /// `increment_slowly` is what makes the *other* half of this test — the
    /// one in `a_shared_counter_survives_a_fan_out` — a wrong number rather
    /// than undefined behaviour. This half needs no such stand-in, because
    /// `buri_rt_incref`'s marked arm is a real `fetch_add` and its unmarked
    /// arm is the load-and-store the mark exists to keep two carriers out of.
    /// **Running this without the mark is a data race and therefore not a
    /// test**; the red-proof is in `reports/wave8-g3.md`, taken against a tree
    /// with the marking latch forced off, where it loses updates every run.
    #[test]
    fn the_count_of_a_marked_block_is_exact_under_every_carrier() {
        let _alone = alone();
        // `alone()` first, then this; `memory::latch` states the order.
        let _latch = crate::memory::latch();
        const CARRIERS: usize = 8;
        const ROUNDS: usize = 1000;

        crate::memory::buri_rt_values_may_cross_tasks();
        let p = crate::memory::buri_rt_alloc(64);
        // SAFETY: `p` is live and was just allocated under the latch.
        let (rc, marked) = unsafe { crate::memory::count_and_mark(p) };
        assert_eq!((rc, marked), (1, true), "a block allocated under the latch was not marked");

        let shared = p as usize;
        thread::scope(|scope| {
            for _ in 0..CARRIERS {
                scope.spawn(move || {
                    for _ in 0..ROUNDS {
                        // SAFETY: the block outlives this scope and every
                        // increment here is matched below.
                        unsafe { crate::memory::buri_rt_incref(shared as *mut u8) };
                    }
                });
            }
        });
        // SAFETY: `p` is still live — nothing decremented it.
        let (rc, _) = unsafe { crate::memory::count_and_mark(p) };
        assert_eq!(
            rc,
            1 + (CARRIERS * ROUNDS) as u64,
            "an increment was lost: the marked arm is not atomic",
        );

        // And back down the same way, so the block is freed exactly once.
        thread::scope(|scope| {
            for _ in 0..CARRIERS {
                scope.spawn(move || {
                    for _ in 0..ROUNDS {
                        // SAFETY: one decrement per increment above, and the
                        // count is above one throughout.
                        unsafe { crate::memory::buri_rt_decref(shared as *mut u8, None) };
                    }
                });
            }
        });
        // SAFETY: `p` is still live at a count of one.
        let (rc, marked) = unsafe { crate::memory::count_and_mark(p) };
        assert_eq!((rc, marked), (1, true), "a decrement was lost");
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(p) };
        crate::memory::forget_values_may_cross_tasks();
    }

    /// **The latch is a property of the program**: before it, no block carries
    /// the mark; after it, every one does, and a recycled block comes back
    /// marked because it goes through `finish` like any other.
    ///
    /// This is the whole of the "err over-set" decision, asserted rather than
    /// argued: there is no per-value question, so there is no value the
    /// analysis can be wrong about.
    #[test]
    fn the_latch_marks_every_block_and_nothing_before_it() {
        let _alone = alone();
        // `alone()` first, then this; `memory::latch` states the order.
        let _latch = crate::memory::latch();
        assert!(!crate::memory::values_may_cross_tasks(), "the silent answer is the safe one");

        let plain = crate::memory::buri_rt_alloc(48);
        // SAFETY: live, just allocated.
        assert_eq!(unsafe { crate::memory::count_and_mark(plain) }.1, false);

        crate::memory::buri_rt_values_may_cross_tasks();
        assert!(crate::memory::values_may_cross_tasks());
        for size in [0u64, 1, 24, 48, 64, 4096] {
            let p = crate::memory::buri_rt_alloc(size);
            // SAFETY: live, just allocated.
            let (rc, marked) = unsafe { crate::memory::count_and_mark(p) };
            assert_eq!((rc, marked), (1, true), "a {size}-byte block was not marked");
            // SAFETY: the capacity is still the byte count it asked for, which
            // is what keeps the release glue's `cap / stride` walk honest.
            assert_eq!(unsafe { crate::memory::buri_rt_cap(p) }, size);
            // SAFETY: the only reference.
            unsafe { crate::memory::buri_rt_free(p) };
        }

        // The block freed before the latch comes back through the per-thread
        // cache, and it comes back marked.
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(plain) };
        let again = crate::memory::buri_rt_alloc(48);
        // SAFETY: live, just allocated.
        assert!(
            unsafe { crate::memory::count_and_mark(again) }.1,
            "a recycled block kept the mark it was made without",
        );
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(again) };
        crate::memory::forget_values_may_cross_tasks();
    }

    /// **A marked block is never unique**, so nothing writes through one.
    ///
    /// `buri_rt_unique_cap` is the licence for every in-place write in
    /// `list.rs` and `text.rs`, and G2 left it unforked on an argument whose
    /// premise the baton was keeping true. This is that premise closed: the
    /// same block, the same count of one, and the answer changes with the
    /// mark.
    #[test]
    fn a_marked_block_is_never_unique() {
        let _alone = alone();
        // `alone()` first, then this; `memory::latch` states the order.
        let _latch = crate::memory::latch();
        let plain = crate::memory::buri_rt_alloc(64);
        // SAFETY: live, count of one, unmarked.
        assert_eq!(unsafe { crate::memory::buri_rt_unique_cap(plain) }, Some(64));
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(plain) };

        crate::memory::buri_rt_values_may_cross_tasks();
        let marked = crate::memory::buri_rt_alloc(64);
        // SAFETY: live, count of one, marked.
        let (rc, is_marked) = unsafe { crate::memory::count_and_mark(marked) };
        assert_eq!((rc, is_marked), (1, true));
        assert_eq!(
            // SAFETY: live payload pointer.
            unsafe { crate::memory::buri_rt_unique_cap(marked) },
            None,
            "a block two carriers may reach was handed an in-place write",
        );
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(marked) };
        crate::memory::forget_values_may_cross_tasks();
    }

    #[test]
    fn a_parked_call_returns_the_right_value() {
        assert_eq!(park_on(async { "the answer" }), "the answer");
        assert_eq!(park_on(std::future::ready(7u64)) + 1, 8);
        assert_eq!(park_on(async { 41 + 1 }), 42);

        // Through the timer wheel, which is the reactor doing the waiting and
        // not the carrier spinning.
        let started = Instant::now();
        let slept: u8 = park_on(async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            9
        });
        assert_eq!(slept, 9);
        assert!(started.elapsed() >= Duration::from_millis(25), "the sleep did not wait");
    }

    /// A carrier that parks does not stop another one from running.
    ///
    /// Under the baton this was the whole of the concurrency story and the
    /// test had to be careful about it: the parking carrier held the baton, so
    /// the second could only run if the park gave it up. There is no baton
    /// now, and the case is kept because the *property* is still the one
    /// `park_on` exists for — a suspended carrier is not a stopped process —
    /// and because a `block_on` that had somehow become blocking of anything
    /// but its own thread would fail here rather than somewhere subtler. The
    /// timeout is what turns a regression into a message instead of a hang.
    #[test]
    fn a_parked_carrier_does_not_stop_the_next_one() {
        let _alone = alone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let second = on_carrier(move || {
            let _ = tx.send("the second carrier ran");
        });
        let arrived = park_on(async { tokio::time::timeout(Duration::from_secs(5), rx).await });
        second.join().expect("the second carrier did not finish");
        let arrived = arrived.expect("the parked carrier never let the second one run");
        assert_eq!(
            arrived.expect("the second carrier dropped its sender"),
            "the second carrier ran",
        );
    }

    /// A job runs off the calling thread, and the next one starts no thread.
    ///
    /// **The assertion used to name a `ThreadId`**: the carrier put its own
    /// channel back in the idle vector before signalling, so the next job
    /// landed on the very same thread. It still usually does, and the case no
    /// longer says so — a queue and a condvar hand the next task to *whichever*
    /// idle carrier the kernel wakes, and which one that is was never the
    /// property. What is the property is that no carrier was **started**, and
    /// that is exact: `arm` counts a finishing carrier as available before its
    /// joiner is told the answer is ready, which is the same ordering the
    /// vector gave and the reason it is written that way round.
    #[test]
    fn a_carrier_runs_the_job_and_is_reused() {
        let _alone = alone();
        let here = thread::current().id();
        let first = on_carrier(move || (thread::current().id(), 6 * 7)).join().unwrap();
        assert_eq!(first.1, 42);
        assert_ne!(first.0, here, "the job ran on the calling thread");

        let before = carriers();
        let second = on_carrier(move || thread::current().id()).join().unwrap();
        assert_ne!(second, here, "the second job ran on the calling thread");
        assert_eq!(carriers(), before, "a carrier was started for a job an idle one could take");
    }

    /// Four jobs that wait, all of them answering.
    ///
    /// **`four_carriers_all_answer` until B9**, and the rename is the slice:
    /// that case asserted `carriers() >= 4`, because four jobs in flight *were*
    /// four threads and anything less would have meant one job waiting for
    /// another. Four waiting tasks are now four saved stack pointers and no
    /// particular number of threads, so the assertion turned round — a task
    /// that waits must not cost a carrier, and four of them must not start
    /// more than four.
    #[test]
    fn four_jobs_that_wait_all_answer() {
        let _alone = alone();
        let before = carriers();
        let jobs: Vec<_> = (0..4u8)
            .map(|n| {
                on_carrier(move || {
                    park_on(async {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    });
                    n
                })
            })
            .collect();
        let mut answers: Vec<u8> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
        answers.sort_unstable();
        assert_eq!(answers, [0, 1, 2, 3]);
        assert!(
            carriers() - before <= 4,
            "four jobs that wait started {} carriers",
            carriers() - before,
        );
    }

    #[test]
    fn a_task_handle_names_a_position_and_is_tombstoned() {
        let _alone = alone();
        let (tx, rx) = channel();
        let handle = task_start(move || {
            let _ = tx.send("ran");
        });
        assert!(handle >= 0);
        assert_eq!(rx.recv().unwrap(), "ran");

        assert!(task_join(handle), "the task did not join");
        assert!(!task_is_live(handle), "a joined task is still live");
        assert!(!task_join(handle), "a second join answered as if it had work to wait for");

        // A handle that names nothing is inert rather than fatal.
        assert!(!task_join(9_999));
        assert!(!task_join(-1));
        assert!(!task_is_live(9_999));

        // Positions, and a tombstone is not reused.
        let a = task_start(|| {});
        let b = task_start(|| {});
        assert_ne!(a, b);
        assert!(task_join(a) && task_join(b));
    }

    /// A task runs **beside** the thread that started it, which is what
    /// `a_task_runs_as_a_carrier_and_gives_the_baton_back` became: the task
    /// used to take the run baton first, so it could not start until its
    /// starter had suspended.
    ///
    /// Asserted by having the task wait for a signal the starter only sends
    /// *after* `task_start` has returned. Under the baton this deadlocked;
    /// there is no ordering here that can pass by accident, and a regression
    /// is the five-second timeout rather than a hung suite.
    #[test]
    fn a_task_runs_beside_the_thread_that_started_it() {
        let _alone = alone();
        let (go, wait) = channel::<()>();
        let (done, ran) = channel::<&'static str>();
        let handle = task_start(move || {
            wait.recv().expect("the starter dropped its sender before the task ran");
            let _ = done.send("beside");
        });
        // The task is already running and is blocked on `wait`, which it could
        // not be if starting it had waited for this thread to suspend.
        go.send(()).expect("the task was not running to receive it");
        assert_eq!(
            ran.recv_timeout(Duration::from_secs(5)).expect("the task never ran"),
            "beside",
        );
        assert!(task_join(handle));
    }

    /// The number is a decision (§3) rather than a default, so it is asserted
    /// where a reader looking for it will find it.
    #[test]
    fn a_carrier_stack_is_the_stated_size() {
        assert_eq!(CARRIER_STACK_BYTES, 512 * 1024);
    }

    /// `Clock.sleepMillis` still waits and still answers nothing, having gone
    /// through the timer wheel instead of `thread::sleep`.
    #[test]
    fn the_sleep_intrinsic_still_sleeps() {
        let started = Instant::now();
        crate::buri_rt_host_clock_sleep_millis(30);
        assert!(started.elapsed() >= Duration::from_millis(25));

        // A duration that is not positive returns at once, exactly as before.
        // A second is a very loose bound and it is the right one: the failure
        // this guards against is the `millis > 0` test going away, and then
        // `-5` widens to a `u64` of eighteen quintillion milliseconds and the
        // process never comes back. A tight bound would buy nothing and would
        // fail on a loaded machine.
        let started = Instant::now();
        crate::buri_rt_host_clock_sleep_millis(0);
        crate::buri_rt_host_clock_sleep_millis(-5);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// `parallel` over four elements: every step run, once, in index order,
    /// told where it is, at the two strides it was given.
    ///
    /// The thunk is written by hand rather than generated, for the reason
    /// `list.rs`'s twin test gives: what an entry does with the index and the
    /// three pointers is a backend's business, and what this file promises is
    /// the walk.
    #[test]
    fn every_task_runs_once_in_index_order() {
        // This case drives the exported entry, which reads the artifact's
        // answer about its own frames — a global, so one case at a time.
        let _alone = alone();
        /// `(calls, indices seen)`. Written through `state`, which the runtime
        /// hands back untouched.
        struct Seen {
            calls: i64,
            order: Vec<u64>,
        }
        unsafe extern "C" fn step(state: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            // SAFETY: the test hands a live `Seen`, an `i64` source element and
            // an `i64` destination slot.
            unsafe {
                let seen = &mut *state.cast::<Seen>();
                seen.calls += 1;
                seen.order.push(index);
                out.cast::<i64>().write(arg.cast::<i64>().read() * 10 + index as i64);
            }
        }
        let src: [i64; 4] = [1, 2, 3, 4];
        let mut seen = Seen { calls: 0, order: Vec::new() };
        let mut got = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `src` covers four `i64`s and both out-pointers are live.
        unsafe {
            buri_rt_host_tasks_parallel(
                src.as_ptr().cast(),
                4,
                step,
                (&raw mut seen).cast(),
                8,
                8,
                &raw mut got,
            );
        }
        assert_eq!(seen.calls, 4, "one step per item, and no more");
        assert_eq!(seen.order, vec![0, 1, 2, 3], "the index is the item's own");
        assert_eq!(got.len, 4);
        // SAFETY: four `i64`s were written there, at the stride asked for.
        let answers: Vec<i64> =
            (0..4).map(|i| unsafe { got.ptr.add(i * 8).cast::<i64>().read() }).collect();
        // Results in the *items'* order, which is what the effect promises and
        // what a fan-out has to keep.
        assert_eq!(answers, vec![10, 21, 32, 43]);
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
    }

    /// The two strides are different, and both are honoured: an `i64` source
    /// read at eight, an `i32` answer written at four.
    #[test]
    fn the_two_strides_are_the_two_element_types() {
        // This case drives the exported entry, which reads the artifact's
        // answer about its own frames — a global, so one case at a time.
        let _alone = alone();
        unsafe extern "C" fn narrow(_: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            // SAFETY: an `i64` in, an `i32` out.
            unsafe { out.cast::<i32>().write((arg.cast::<i64>().read() + index as i64) as i32) }
        }
        let src: [i64; 3] = [100, 200, 300];
        let mut got = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `src` covers three `i64`s and `got` is a live local.
        unsafe {
            buri_rt_host_tasks_parallel(
                src.as_ptr().cast(),
                3,
                narrow,
                std::ptr::null_mut(),
                8,
                4,
                &raw mut got,
            );
        }
        assert_eq!(got.len, 3);
        // SAFETY: three `i32`s were written there.
        let answers: Vec<i32> =
            (0..3).map(|i| unsafe { got.ptr.add(i * 4).cast::<i32>().read() }).collect();
        assert_eq!(answers, vec![100, 201, 302]);
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
    }

    /// An empty list starts no task and allocates nothing — which is what
    /// keeps `parallel(ctx, [], f)` free rather than a null dereference.
    #[test]
    fn no_items_is_no_tasks() {
        // This case drives the exported entry, which reads the artifact's
        // answer about its own frames — a global, so one case at a time.
        let _alone = alone();
        unsafe extern "C" fn never(_: *mut u8, _: u64, _: *const u8, _: *mut u8) {
            unreachable!("a task ran on an empty list");
        }
        let mut got = BuriList { ptr: std::ptr::null_mut(), len: 9 };
        // SAFETY: the source is empty, so the pointer is never read.
        unsafe {
            buri_rt_host_tasks_parallel(
                std::ptr::null(),
                0,
                never,
                std::ptr::null_mut(),
                8,
                8,
                &raw mut got,
            );
        }
        assert_eq!(got.len, 0);
        assert!(got.ptr.is_null(), "an empty list allocates nothing");
    }

    /// A step that is itself a call site. `parallel` inside `parallel` is the
    /// shape a nested fan-out takes, and today it is a loop inside a loop —
    /// the assertion is that the inner walk does not disturb the outer one's
    /// index or its destination.
    #[test]
    fn a_task_may_itself_run_tasks() {
        // This case drives the exported entry, which reads the artifact's
        // answer about its own frames — a global, so one case at a time.
        let _alone = alone();
        unsafe extern "C" fn inner(_: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            // SAFETY: an `i64` in and an `i64` out.
            unsafe { out.cast::<i64>().write(arg.cast::<i64>().read() + index as i64) }
        }
        unsafe extern "C" fn outer(_: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            let src: [i64; 3] = [10, 20, 30];
            let mut nested = BuriList { ptr: std::ptr::null_mut(), len: 0 };
            // SAFETY: `src` is live for the length of this call.
            unsafe {
                buri_rt_host_tasks_parallel(
                    src.as_ptr().cast(),
                    3,
                    inner,
                    std::ptr::null_mut(),
                    8,
                    8,
                    &raw mut nested,
                );
            }
            // SAFETY: three `i64`s were just written there, and the block is
            // this call's to free.
            let sum: i64 =
                (0..3).map(|i| unsafe { nested.ptr.add(i * 8).cast::<i64>().read() }).sum();
            // SAFETY: the only reference.
            unsafe { crate::memory::buri_rt_free(nested.ptr) };
            // SAFETY: an `i64` in and an `i64` out.
            unsafe { out.cast::<i64>().write(sum + arg.cast::<i64>().read() + index as i64) }
        }
        let src: [i64; 2] = [1000, 2000];
        let mut got = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `src` covers two `i64`s and `got` is a live local.
        unsafe {
            buri_rt_host_tasks_parallel(
                src.as_ptr().cast(),
                2,
                outer,
                std::ptr::null_mut(),
                8,
                8,
                &raw mut got,
            );
        }
        // The inner walk answers 10 + 21 + 32 = 63 every time.
        // SAFETY: two `i64`s were written there.
        let answers: Vec<i64> =
            (0..2).map(|i| unsafe { got.ptr.add(i * 8).cast::<i64>().read() }).collect();
        assert_eq!(answers, vec![1063, 2064]);
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
    }

    // -----------------------------------------------------------------------
    // The fan-out
    // -----------------------------------------------------------------------

    /// **Two tasks that wait, wait at the same time.** The slice's whole point.
    ///
    /// Two steps that each sleep 100 ms answer in a little over 100 ms rather
    /// than in 200: the sleep is [`park_on`]'s and the two carriers are two
    /// threads. Sequentially this is 200 ms and the bound below fails, which is
    /// what makes it a test of the scheduler rather than of the clock.
    ///
    /// **D4's acceptance case, and its number moves in neither G3 nor B9.** It
    /// was green under the baton — a step that waits was already giving the
    /// baton up — so the ~100 ms it measures is the same ~100 ms before and
    /// after, and that is the point of re-running it here rather than of
    /// changing it. B9 changes what the two waiting steps *cost* — two saved
    /// stack pointers rather than two threads — and deliberately not what they
    /// take.
    ///
    /// The upper bound is generous — 100 ms of real waiting plus the whole of a
    /// loaded machine's scheduling — because the failure it guards against is
    /// *doubling*, and a bound tight enough to catch a millisecond of jitter
    /// would fail under `cargo test`'s own parallelism instead.
    #[test]
    fn two_tasks_that_wait_overlap() {
        // The fan-out draws on the carrier pool, which every other case
        // that reads it takes this lock for.
        let _alone = alone();
        unsafe extern "C" fn nap(_: *mut u8, index: u64, _: *const u8, out: *mut u8) {
            crate::buri_rt_host_clock_sleep_millis(100);
            // SAFETY: an `i64` slot of the answer.
            unsafe { out.cast::<i64>().write(index as i64) }
        }
        let src: [i64; 2] = [0, 0];
        let started = Instant::now();
        // SAFETY: two `i64`s in, two `i64`s out.
        let got =
            unsafe { steps_of(src.as_ptr().cast(), 2, nap, std::ptr::null_mut(), 8, 8, true) };
        let waited = started.elapsed();
        // SAFETY: two `i64`s were written there.
        assert_eq!(unsafe { i64s(&got, 2) }, vec![0, 1]);
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
        assert!(waited >= Duration::from_millis(100), "{waited:?} is not a sleep at all");
        assert!(waited < Duration::from_millis(190), "{waited:?}: the two sleeps did not overlap");
    }

    /// The answer is in the **items'** order even when the work is not.
    ///
    /// Each step sleeps for as long as its index is early, so the tasks finish
    /// in reverse; the case asserts both halves — that completion really was
    /// reversed, so the ordering claim is tested rather than accidentally
    /// satisfied, and that the `[B]` is in index order anyway.
    #[test]
    fn a_fan_out_answers_in_the_items_order() {
        // The fan-out draws on the carrier pool, which every other case
        // that reads it takes this lock for.
        let _alone = alone();
        /// The indices in the order their steps *finished*.
        struct Finished(Mutex<Vec<u64>>);
        unsafe extern "C" fn backwards(state: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            crate::buri_rt_host_clock_sleep_millis(20 * (4 - index as i64));
            // SAFETY: the test hands a live `Finished`, an `i64` element and an
            // `i64` slot.
            unsafe {
                let done = &*state.cast::<Finished>();
                match done.0.lock() {
                    Ok(mut seen) => seen.push(index),
                    Err(poisoned) => poisoned.into_inner().push(index),
                }
                out.cast::<i64>().write(arg.cast::<i64>().read() * 10 + index as i64);
            }
        }
        let src: [i64; 4] = [1, 2, 3, 4];
        let done = Finished(Mutex::new(Vec::new()));
        // SAFETY: four `i64`s in, four out, and `done` outlives the call.
        let got = unsafe {
            steps_of(
                src.as_ptr().cast(),
                4,
                backwards,
                (&raw const done).cast_mut().cast(),
                8,
                8,
                true,
            )
        };
        // SAFETY: four `i64`s were written there.
        assert_eq!(unsafe { i64s(&got, 4) }, vec![10, 21, 32, 43], "the items' order");
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
        let seen = match done.0.lock() {
            Ok(seen) => seen.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        assert_eq!(seen, vec![3, 2, 1, 0], "the work did not finish out of order");
    }

    /// **Two steps that compute run at the same time.** The case the baton
    /// made impossible, and the one sentence of behaviour G3 changes here.
    ///
    /// Each step spins on a shared counter until every other step has arrived,
    /// then leaves. Under the baton the first step would spin for ever, because
    /// the second cannot enter Buri code until the first has left — so this is
    /// a test that could not have been written before this slice and cannot
    /// pass by accident after it. The timeout is what makes a regression a
    /// message instead of a hung suite.
    #[test]
    fn the_steps_of_one_fan_out_compute_at_the_same_time() {
        let _alone = alone();
        const STEPS: usize = 4;
        struct Rendezvous {
            arrived: AtomicUsize,
            gave_up: AtomicUsize,
        }
        unsafe extern "C" fn meet(state: *mut u8, index: u64, _: *const u8, out: *mut u8) {
            // SAFETY: the test hands a live `Rendezvous` and an `i64` slot.
            unsafe {
                let r = &*state.cast::<Rendezvous>();
                r.arrived.fetch_add(1, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(10);
                while r.arrived.load(Ordering::SeqCst) < STEPS {
                    if Instant::now() > deadline {
                        r.gave_up.fetch_add(1, Ordering::SeqCst);
                        break;
                    }
                    thread::yield_now();
                }
                out.cast::<i64>().write(index as i64);
            }
        }
        let r = Rendezvous { arrived: AtomicUsize::new(0), gave_up: AtomicUsize::new(0) };
        let src = vec![0i64; STEPS];
        // SAFETY: `STEPS` `i64`s in and out, and `r` outlives the call.
        let got = unsafe {
            steps_of(
                src.as_ptr().cast(),
                STEPS,
                meet,
                (&raw const r).cast_mut().cast(),
                8,
                8,
                true,
            )
        };
        // SAFETY: `STEPS` `i64`s were written there.
        let answers = unsafe { i64s(&got, STEPS) };
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
        assert_eq!(answers, (0..STEPS as i64).collect::<Vec<i64>>(), "the items' order");
        assert_eq!(
            r.gave_up.load(Ordering::SeqCst),
            0,
            "a step waited ten seconds for another one: the steps are still serialised",
        );
    }

    /// **A block every step of one fan-out counts keeps an exact count.**
    ///
    /// The `Tasks.parallel`-shaped version of
    /// `the_count_of_a_marked_block_is_exact_under_every_carrier`: this one
    /// goes through the real scheduler rather than through `thread::scope`, so
    /// what it tests is that the fan-out only ever hands carriers *marked*
    /// blocks. Two hundred steps each take and give back a reference to one
    /// block; the count at the end is one, or the fan-out ran on something the
    /// latch had not marked.
    ///
    /// It also counts overlaps outright, which is the assertion that inverted
    /// in this slice: `the_baton_admits_one_step_at_a_time` asserted **zero**
    /// overlaps, and the whole of what the baton bought was that zero. The
    /// number now expected is "more than none", and the count under it is
    /// still exact — which is the trade the mark makes and this case states.
    #[test]
    fn a_shared_counter_survives_a_fan_out() {
        let _alone = alone();
        // `alone()` first, then this; `memory::latch` states the order.
        let _latch = crate::memory::latch();
        const STEPS: usize = 200;
        struct Shared {
            block: usize,
            inside: AtomicUsize,
            overlaps: AtomicUsize,
        }
        unsafe extern "C" fn touch(state: *mut u8, index: u64, _: *const u8, out: *mut u8) {
            // SAFETY: the test hands a live `Shared`, a live block and a slot.
            unsafe {
                let shared = &*state.cast::<Shared>();
                if shared.inside.fetch_add(1, Ordering::SeqCst) != 0 {
                    shared.overlaps.fetch_add(1, Ordering::SeqCst);
                }
                let p = shared.block as *mut u8;
                for _ in 0..50 {
                    crate::memory::buri_rt_incref(p);
                }
                thread::yield_now();
                for _ in 0..50 {
                    crate::memory::buri_rt_decref(p, None);
                }
                shared.inside.fetch_sub(1, Ordering::SeqCst);
                out.cast::<i64>().write(index as i64);
            }
        }

        crate::memory::buri_rt_values_may_cross_tasks();
        let p = crate::memory::buri_rt_alloc(32);
        // SAFETY: live, just allocated under the latch.
        assert_eq!(unsafe { crate::memory::count_and_mark(p) }, (1, true));
        let shared = Shared {
            block: p as usize,
            inside: AtomicUsize::new(0),
            overlaps: AtomicUsize::new(0),
        };
        let src = vec![0i64; STEPS];
        // SAFETY: `STEPS` `i64`s in and out; `shared` and the block outlive it.
        let got = unsafe {
            steps_of(
                src.as_ptr().cast(),
                STEPS,
                touch,
                (&raw const shared).cast_mut().cast(),
                8,
                8,
                true,
            )
        };
        // SAFETY: `STEPS` `i64`s were written there.
        let answers = unsafe { i64s(&got, STEPS) };
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
        assert_eq!(answers, (0..STEPS as i64).collect::<Vec<i64>>(), "the items' order");
        // SAFETY: `p` is still live at whatever the counts left it.
        let (rc, marked) = unsafe { crate::memory::count_and_mark(p) };
        assert_eq!(
            (rc, marked),
            (1, true),
            "the count did not come back to one: a reference operation was lost",
        );
        assert!(
            shared.overlaps.load(Ordering::SeqCst) > 0,
            "no two steps were ever in Buri code together, so this proved nothing",
        );
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(p) };
        crate::memory::forget_values_may_cross_tasks();
    }

    /// More items than may be in flight: the window slides, and every step
    /// still runs exactly once into its own slot.
    ///
    /// [`IN_FLIGHT`] bounds threads and not the answer, and this is where that
    /// is said out loud — the list is deliberately longer than it.
    #[test]
    fn a_list_longer_than_the_window_still_answers_every_item() {
        // The fan-out draws on the carrier pool, which every other case
        // that reads it takes this lock for.
        let _alone = alone();
        unsafe extern "C" fn triple(_: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            // SAFETY: an `i64` in and an `i64` out.
            unsafe { out.cast::<i64>().write(arg.cast::<i64>().read() * 2 + index as i64) }
        }
        let n = IN_FLIGHT * 2 + 3;
        assert!(n > IN_FLIGHT, "the case is only a case while the list outruns the window");
        let src: Vec<i64> = (0..n as i64).collect();
        // SAFETY: `n` `i64`s in and out.
        let got =
            unsafe { steps_of(src.as_ptr().cast(), n, triple, std::ptr::null_mut(), 8, 8, true) };
        // SAFETY: `n` `i64`s were written there.
        let answers = unsafe { i64s(&got, n) };
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
        assert_eq!(answers, (0..n as i64).map(|i| i * 3).collect::<Vec<i64>>());
    }

    /// A step that is itself a fan-out.
    ///
    /// `a_nested_fan_out_gives_the_baton_up_first` under the baton, where the
    /// giving-up was the point: a carrier's body began by taking the baton, so
    /// a nested call that kept it would have been a caller waiting for work
    /// that was waiting for the caller. With no baton the nesting is only
    /// nesting, and the case is kept for the property it always also had —
    /// that a fan-out from a carrier answers the same numbers a fan-out from
    /// the process thread does, and leaves the pool usable. `Handoff::join`
    /// has no timeout, so a failure here is still the suite stopping rather
    /// than a message.
    #[test]
    fn a_nested_fan_out_answers_the_same_numbers() {
        // The fan-out draws on the carrier pool, which every other case
        // that reads it takes this lock for.
        let _alone = alone();
        unsafe extern "C" fn inner(_: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            // SAFETY: an `i64` in and an `i64` out.
            unsafe { out.cast::<i64>().write(arg.cast::<i64>().read() + index as i64) }
        }
        unsafe extern "C" fn outer(_: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            let src: [i64; 3] = [10, 20, 30];
            // SAFETY: three `i64`s in and out, for the length of this call.
            let nested = unsafe {
                steps_of(src.as_ptr().cast(), 3, inner, std::ptr::null_mut(), 8, 8, true)
            };
            // SAFETY: three `i64`s were just written there.
            let sum: i64 = unsafe { i64s(&nested, 3) }.iter().sum();
            // SAFETY: the only reference.
            unsafe { crate::memory::buri_rt_free(nested.ptr) };
            // SAFETY: an `i64` in and an `i64` out.
            unsafe { out.cast::<i64>().write(sum + arg.cast::<i64>().read() + index as i64) }
        }
        let src: [i64; 2] = [1000, 2000];
        // SAFETY: two `i64`s in and out.
        let got =
            unsafe { steps_of(src.as_ptr().cast(), 2, outer, std::ptr::null_mut(), 8, 8, true) };
        // SAFETY: two `i64`s were written there.
        let answers = unsafe { i64s(&got, 2) };
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
        // The inner walk answers 10 + 21 + 32 = 63 every time, exactly as it
        // does in `a_task_may_itself_run_tasks` — the same nesting, the other
        // arm, the same number.
        assert_eq!(answers, vec![1063, 2064]);
    }

    /// **The data-race fixture, and it fails deterministically without the
    /// mark.**
    ///
    /// The counting cases above are stress tests: a lost update needs two
    /// carriers' load-and-store windows to interleave, so on the pre-fix tree
    /// they fail *often* rather than *always* — the numbers are in
    /// `reports/wave8-g3.md`. This one has no window to hit, because what it
    /// exercises is not the count but the **in-place write licence**, and that
    /// is a decision each carrier takes on its own and then acts on:
    ///
    ///  * `base` is one heap `Str` with a capacity floor of
    ///    [`crate::memory::BURI_RT_GROWTH_FLOOR`] bytes behind four bytes of
    ///    text, and a count of one, because this thread holds the only
    ///    reference and every step merely borrows it — which is the shape a
    ///    closure's captured value has.
    ///  * Every step concatenates **its own index** onto it. Unmarked, the
    ///    first step to look reads `rc == 1`, takes MEMORY.md §5.3's in-place
    ///    path, and writes its suffix into the borrowed block — and answers a
    ///    *view over it*. There is no window to hit: a step that reaches the
    ///    concat while the count says one does this, and on the pre-fix tree
    ///    one always does.
    ///  * Marked, `buri_rt_unique_cap` answers `None` whatever the count, so
    ///    every step allocates and nothing is written through.
    ///
    /// The two assertions are the two faces of the same fault: **no answer is
    /// a view over the caller's block**, and **the caller's block still holds
    /// what the caller put in it** — read over the whole capacity, because the
    /// suffix lands at offset four and a test that read only the four bytes it
    /// wrote would miss the write it is looking for. `[G3-RED]` in
    /// `reports/wave8-g3.md` is this case on a tree whose `finish` does not
    /// stamp the mark: it fails on every run, at both assertions.
    #[test]
    fn two_steps_never_append_to_one_buffer_in_place() {
        use crate::value::{BuriStr, BURI_RT_STR_ASCII, BURI_RT_STR_LEN_MASK};
        let _alone = alone();
        let _latch = crate::memory::latch();
        const STEPS: usize = 16;

        /// `(the shared base, the suffix bytes)`, read by every step.
        struct Base {
            base: usize,
            ptr: usize,
            len: u64,
        }
        unsafe extern "C" fn append(state: *mut u8, index: u64, _: *const u8, out: *mut u8) {
            // SAFETY: the test hands a live `Base` and a `BuriStr` slot.
            unsafe {
                let b = &*state.cast::<Base>();
                let digits = b"0123456789abcdef";
                let suffix = &digits[index as usize % 16..][..1];
                crate::buri_rt_str_concat(
                    b.base as *mut u8,
                    b.ptr as *const u8,
                    b.len,
                    std::ptr::null_mut(),
                    suffix.as_ptr(),
                    1 | BURI_RT_STR_ASCII,
                    out.cast::<BuriStr>(),
                );
            }
        }

        crate::memory::buri_rt_values_may_cross_tasks();
        // A four-byte `Str` in a block with room to grow, which is what a
        // captured accumulator looks like.
        let base = crate::memory::buri_rt_alloc_zeroed(crate::memory::BURI_RT_GROWTH_FLOOR);
        // SAFETY: `base` is a fresh block of at least four bytes.
        unsafe { std::ptr::copy_nonoverlapping(b"seed".as_ptr(), base, 4) };
        let state = Base {
            base: base as usize,
            ptr: base as usize,
            len: 4 | BURI_RT_STR_ASCII,
        };

        let src = vec![0i64; STEPS];
        let stride = std::mem::size_of::<BuriStr>();
        // SAFETY: `STEPS` `i64`s in, `STEPS` `BuriStr`s out, and `state` and
        // the block both outlive the call.
        let got = unsafe {
            steps_of(
                src.as_ptr().cast(),
                STEPS,
                append,
                (&raw const state).cast_mut().cast(),
                8,
                stride,
                true,
            )
        };
        // The caller's own block, read back **before** anything is freed. The
        // whole of it: an in-place append writes its suffix at offset four, so
        // a test that read only the four bytes it put there would miss exactly
        // the write it is looking for. The block was zeroed, so "seed" and
        // sixty zero bytes is the untouched picture.
        // SAFETY: `base` is this test's live block of `BURI_RT_GROWTH_FLOOR`.
        let base_after = unsafe {
            std::slice::from_raw_parts(base, crate::memory::BURI_RT_GROWTH_FLOOR as usize).to_vec()
        };
        // And whether any answer is a *view over it*, which is the same fault
        // stated as aliasing rather than as a write.
        let aliased = (0..STEPS)
            .filter(|i| {
                // SAFETY: `STEPS` `BuriStr`s were written there.
                unsafe { got.ptr.add(i * stride).cast::<BuriStr>().read().base == base }
            })
            .count();

        // SAFETY: `STEPS` `BuriStr`s were written there.
        let answers: Vec<String> = (0..STEPS)
            .map(|i| unsafe {
                let s = got.ptr.add(i * stride).cast::<BuriStr>().read();
                let n = (s.len & BURI_RT_STR_LEN_MASK) as usize;
                String::from_utf8_lossy(std::slice::from_raw_parts(s.ptr, n)).into_owned()
            })
            .collect();
        for i in 0..STEPS {
            // SAFETY: each answer owns its block unless the concat aliased.
            unsafe {
                let s = got.ptr.add(i * stride).cast::<BuriStr>().read();
                if !s.base.is_null() && s.base != base {
                    crate::memory::buri_rt_free(s.base);
                }
            }
        }
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(base) };
        crate::memory::forget_values_may_cross_tasks();

        let mut distinct = answers.clone();
        distinct.sort();
        distinct.dedup();
        let mut untouched = vec![0u8; crate::memory::BURI_RT_GROWTH_FLOOR as usize];
        untouched[..4].copy_from_slice(b"seed");
        assert_eq!(aliased, 0, "a step answered a view over the buffer it borrowed");
        assert_eq!(base_after, untouched, "a step wrote through a borrowed buffer");
        assert_eq!(
            distinct.len(),
            STEPS,
            "two steps appended to one buffer: {answers:?}",
        );
    }

    /// **The artifact decides, twice**, and silence on either question is the
    /// sequential answer.
    ///
    /// The only case that touches the two globals, which is why it puts both
    /// back. The observable difference is the thread a step runs on, and the
    /// table it walks is the whole truth table: a step fans out where the
    /// artifact has said *both* that its carriers have frames of their own and
    /// that its values may cross a task boundary, and runs on the caller's
    /// thread otherwise.
    ///
    /// The two are separate rows rather than one because they are separate
    /// facts (§2), and because the "frames yes, marking no" row is the one a
    /// bug would land in: it is a backend that can fan out over blocks nobody
    /// marked, which is the silent aliasing MEMORY.md §5.5 names. Asserting it
    /// stays sequential is asserting that the gate is a gate.
    #[test]
    fn the_artifact_says_whether_the_steps_may_fan_out() {
        static WHERE: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());
        unsafe extern "C" fn note(_: *mut u8, index: u64, _: *const u8, out: *mut u8) {
            match WHERE.lock() {
                Ok(mut seen) => seen.push(thread::current().id()),
                Err(poisoned) => poisoned.into_inner().push(thread::current().id()),
            }
            // SAFETY: an `i64` slot of the answer.
            unsafe { out.cast::<i64>().write(index as i64) }
        }
        fn ran_on() -> Vec<ThreadId> {
            let src: [i64; 4] = [0; 4];
            let mut got = BuriList { ptr: std::ptr::null_mut(), len: 0 };
            // SAFETY: four `i64`s in, four out, and `got` is a live local.
            unsafe {
                buri_rt_host_tasks_parallel(
                    src.as_ptr().cast(),
                    4,
                    note,
                    std::ptr::null_mut(),
                    8,
                    8,
                    &raw mut got,
                );
                crate::memory::buri_rt_free(got.ptr);
            }
            let mut seen = match WHERE.lock() {
                Ok(seen) => seen,
                Err(poisoned) => poisoned.into_inner(),
            };
            std::mem::take(&mut *seen)
        }

        let _alone = alone();
        let _latch = crate::memory::latch();
        let me = thread::current().id();
        let here = |seen: &[ThreadId]| seen.iter().all(|id| *id == me);

        assert!(!crate::frames_are_per_carrier(), "the silent answer is the safe one");
        assert!(!crate::memory::values_may_cross_tasks(), "the silent answer is the safe one");
        assert!(here(&ran_on()), "neither statement made, and a step left the calling carrier");

        // Marking without frames: the blocks are safe to share and there is
        // still nowhere for a second carrier to put a frame.
        crate::memory::buri_rt_values_may_cross_tasks();
        assert!(here(&ran_on()), "a shared Buri stack ran a step off the calling carrier");

        // Frames without marking: **the row a bug lands in.** A backend that
        // can fan out, over blocks nothing marked, must not.
        crate::memory::forget_values_may_cross_tasks();
        crate::buri_rt_frames_are_per_carrier();
        let unmarked = ran_on();
        assert!(
            here(&unmarked),
            "steps fanned out over unmarked blocks: {unmarked:?}",
        );

        // Both: the steps fan out.
        crate::memory::buri_rt_values_may_cross_tasks();
        let fanned = ran_on();
        crate::forget_frames_are_per_carrier();
        crate::memory::forget_values_may_cross_tasks();
        assert!(
            fanned.iter().any(|id| *id != me),
            "the steps stayed on the calling carrier: {fanned:?}"
        );
    }


    // === B9: the switch ===

    /// **A parked task holds no carrier**, which is the slice in one
    /// assertion.
    ///
    /// A thousand tasks park at the same time and are then let go. Before B9
    /// each of them was an OS thread for the whole of that wait, so this case
    /// could not have been written: at ten thousand the thread-per-task pool
    /// does not merely cost more, it **fails** — `pthread_create` answers
    /// `EAGAIN` on the 8 192nd thread of this process and `spawn_carrier`
    /// turns that into an abort. `reports/wave8-b9.md` has the run.
    ///
    /// What is asserted is the ratio rather than a number: a thousand tasks in
    /// flight cost fewer than a quarter as many threads. The carriers that do
    /// get started are the ones the dispatch loop outruns — a task is queued
    /// before the previous one has reached its park — and [`MAX_CARRIERS`] is
    /// the ceiling under all of it.
    #[test]
    fn a_thousand_parked_tasks_do_not_cost_a_thousand_carriers() {
        let _alone = alone();
        const N: usize = 1000;
        let before = carriers();
        let (_, _, started) = park_n(N);
        assert!(
            started * 4 < N,
            "{N} parked tasks started {started} carriers, which is not a saving worth the switch",
        );
        assert!(
            carriers() - before <= MAX_CARRIERS,
            "the pool went past its own ceiling",
        );
    }

    /// **A task resumed on a different carrier keeps its frames.**
    ///
    /// The red-proof of the machine-stack half, and it is written so that the
    /// migration is *observed* rather than hoped for: every task records the
    /// thread it parked on and the thread it woke on, and the case fails if no
    /// task ever moved. What it then checks is that four kilobytes of frame,
    /// written before the park and read after it, came back byte for byte, and
    /// that the frame was at the same address both times — a task whose stack
    /// had been the carrier's would find somebody else's bytes there.
    ///
    /// `[B9-RED]` for this case is a `spawn_task` that hands every task the
    /// same mapping instead of one of its own; on that tree it fails on the
    /// first pair of tasks that overlap, deterministically, because the second
    /// task's frame is written over the first's. The number in
    /// `reports/wave8-b9.md` is from that run.
    #[test]
    fn a_task_resumed_on_another_carrier_keeps_its_frames() {
        let _alone = alone();
        const TASKS: usize = 24;
        /// Deep enough that a frame cannot be a register spill, and written
        /// with a value that depends on the task so two tasks' frames differ.
        const WORDS: usize = 512;

        struct Seen {
            parked_on: ThreadId,
            woke_on: ThreadId,
            frame: usize,
            frame_again: usize,
            intact: bool,
        }

        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let arrived = std::sync::Arc::new(AtomicUsize::new(0));
        let seen: std::sync::Arc<Mutex<Vec<Seen>>> = std::sync::Arc::new(Mutex::new(Vec::new()));

        let handles: Vec<i64> = (0..TASKS)
            .map(|i| {
                let gate = std::sync::Arc::clone(&gate);
                let arrived = std::sync::Arc::clone(&arrived);
                let seen = std::sync::Arc::clone(&seen);
                task_start(move || {
                    let mut frame = [0u64; WORDS];
                    for (n, slot) in frame.iter_mut().enumerate() {
                        *slot = (i as u64) << 32 | n as u64;
                    }
                    let where_before = frame.as_ptr() as usize;
                    let parked_on = thread::current().id();
                    arrived.fetch_add(1, Ordering::SeqCst);

                    let _ = park_on(gate.acquire());

                    let intact =
                        frame.iter().enumerate().all(|(n, w)| *w == (i as u64) << 32 | n as u64);
                    let mine = Seen {
                        parked_on,
                        woke_on: thread::current().id(),
                        frame: where_before,
                        frame_again: frame.as_ptr() as usize,
                        intact,
                    };
                    match seen.lock() {
                        Ok(mut s) => s.push(mine),
                        Err(poisoned) => poisoned.into_inner().push(mine),
                    }
                })
            })
            .collect();

        // Every task is parked before any of them is let go, so the carriers
        // that pick them back up are whichever the queue hands them to.
        let deadline = Instant::now() + Duration::from_secs(30);
        while arrived.load(Ordering::SeqCst) < TASKS {
            assert!(Instant::now() < deadline, "only {} of {TASKS} tasks parked", arrived.load(Ordering::SeqCst));
            thread::sleep(Duration::from_millis(2));
        }
        gate.add_permits(TASKS);
        for h in handles {
            assert!(task_join(h), "a task did not finish");
        }

        let seen = match seen.lock() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert_eq!(seen.len(), TASKS);
        for (i, s) in seen.iter().enumerate() {
            assert!(s.intact, "task {i}'s frame was not what it left there");
            assert_eq!(s.frame, s.frame_again, "task {i}'s frame moved across the park");
        }
        assert!(
            seen.iter().any(|s| s.parked_on != s.woke_on),
            "no task changed carrier, so this case did not test what it is for",
        );
    }

    /// **A task's Buri data stack is its own, and it is still its own after a
    /// park.**
    ///
    /// The other stack. B7 kept the free list on the thread; a parked task
    /// outlives the carrier that started it, so the list moved onto the task
    /// (`memory::stack_list`) and this is what says so: every task acquires a
    /// block, writes its index into the first word and into the last usable
    /// one, parks, wakes somewhere else, and finds both bytes where it left
    /// them. Then the blocks are compared pairwise — **no two live tasks were
    /// handed the same block**, which is `two_carriers_do_not_share_a_stack`
    /// restated for the thing that owns a stack now.
    #[test]
    fn a_tasks_buri_stack_is_its_own_across_a_park() {
        use crate::memory::{buri_rt_stack_acquire, buri_rt_stack_release, BURI_RT_STACK_USABLE};
        let _alone = alone();
        const TASKS: usize = 16;

        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let arrived = std::sync::Arc::new(AtomicUsize::new(0));
        let blocks: std::sync::Arc<Mutex<Vec<(usize, bool)>>> =
            std::sync::Arc::new(Mutex::new(Vec::new()));

        let handles: Vec<i64> = (0..TASKS)
            .map(|i| {
                let gate = std::sync::Arc::clone(&gate);
                let arrived = std::sync::Arc::clone(&arrived);
                let blocks = std::sync::Arc::clone(&blocks);
                task_start(move || {
                    let base = buri_rt_stack_acquire();
                    // SAFETY: a block this task now owns, `BURI_RT_STACK_USABLE`
                    // bytes wide, with a guard above it.
                    unsafe {
                        base.write(i as u8);
                        base.add(BURI_RT_STACK_USABLE - 1).write(i as u8);
                    }
                    arrived.fetch_add(1, Ordering::SeqCst);
                    let _ = park_on(gate.acquire());
                    // SAFETY: the same block; nothing else may have touched it.
                    let kept = unsafe {
                        base.read() == i as u8
                            && base.add(BURI_RT_STACK_USABLE - 1).read() == i as u8
                    };
                    match blocks.lock() {
                        Ok(mut b) => b.push((base as usize, kept)),
                        Err(poisoned) => poisoned.into_inner().push((base as usize, kept)),
                    }
                    // SAFETY: this task's own block, and no entry is inside it.
                    unsafe { buri_rt_stack_release(base) };
                })
            })
            .collect();

        let deadline = Instant::now() + Duration::from_secs(30);
        while arrived.load(Ordering::SeqCst) < TASKS {
            assert!(Instant::now() < deadline, "only {} of {TASKS} tasks parked", arrived.load(Ordering::SeqCst));
            thread::sleep(Duration::from_millis(2));
        }
        gate.add_permits(TASKS);
        for h in handles {
            assert!(task_join(h), "a task did not finish");
        }

        let mut blocks = match blocks.lock() {
            Ok(b) => b,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert_eq!(blocks.len(), TASKS);
        assert!(blocks.iter().all(|(_, kept)| *kept), "a task's buri stack was written over");
        blocks.sort_unstable();
        let mut addresses: Vec<usize> = blocks.iter().map(|(a, _)| *a).collect();
        addresses.dedup();
        assert_eq!(addresses.len(), TASKS, "two live tasks were handed the same buri stack");
    }

    /// **A task recurses far past what a carrier thread could hold**, which is
    /// `reports/wave6-b7b8.md` §5.2 closed.
    ///
    /// That report recorded the asymmetry: the frame-threaded backend gave a
    /// carrier 64 MiB of *Buri* stack from `buri_rt_stack_acquire`, while
    /// under LLVM a Buri frame is a machine frame and a carrier had
    /// [`CARRIER_STACK_BYTES`] — 512 KiB — of thread stack to put it on. A
    /// task's machine stack is now mapped by `memory::buri_rt_task_stack_
    /// acquire` at the same `BURI_RT_STACK_BYTES` as its data stack, so the
    /// two backends have one number.
    ///
    /// Sixty thousand non-tail frames is several megabytes, which is an order
    /// of magnitude past 512 KiB and an order of magnitude short of the
    /// mapping — deep enough that the old number would fault and shallow
    /// enough that the new one is not being probed for its edge, which is what
    /// the guard is for and what `stencil::a_runaway_recursion_on_a_carrier_
    /// faults_at_its_own_guard` asks.
    #[test]
    fn a_task_recurses_past_what_a_carrier_thread_would_hold() {
        let _alone = alone();
        const DEPTH: u64 = 60_000;

        #[inline(never)]
        fn down(n: u64) -> u64 {
            // A frame with something in it, and a use of the answer afterwards
            // so the call is not a tail call.
            let mut pad = [0u64; 8];
            pad[(n % 8) as usize] = n;
            if n == 0 {
                return pad.iter().sum();
            }
            down(n - 1) + pad.iter().sum::<u64>()
        }

        let answer = on_carrier(|| down(DEPTH)).join().expect("the task did not finish");
        assert_eq!(answer, (0..=DEPTH).sum::<u64>());
    }

    /// The two mappings a task is given are the two the constants name, and
    /// the machine one is writable at both ends of its usable range.
    ///
    /// The guard is **not** written to here — it is `PROT_NONE`, so a test
    /// that touched it would be a signal rather than a failure. What is
    /// asserted is its geometry: the usable range starts a whole guard above
    /// the base and runs to the top, which is where a downward-growing stack
    /// starts and where `switch::prepare` puts its frame.
    #[test]
    fn a_task_machine_stack_is_guarded_at_the_end_it_grows_towards() {
        use crate::memory::{
            buri_rt_task_stack_acquire, buri_rt_task_stack_release, BURI_RT_STACK_ALIGN,
            BURI_RT_STACK_BYTES, BURI_RT_STACK_GUARD, BURI_RT_STACK_USABLE,
        };
        let (base, top) = buri_rt_task_stack_acquire();
        assert_eq!(top as usize - base as usize, BURI_RT_STACK_BYTES);
        assert!((base as usize).is_multiple_of(BURI_RT_STACK_ALIGN));
        assert!((top as usize).is_multiple_of(16), "a stack top must be 16-byte aligned");
        // SAFETY: the usable range is `[base + GUARD, top)`, which the mapping
        // left readable and writable.
        unsafe {
            let low = base.add(BURI_RT_STACK_GUARD);
            low.write(0x5a);
            top.sub(1).write(0xa5);
            assert_eq!(low.read(), 0x5a);
            assert_eq!(top.sub(1).read(), 0xa5);
            assert_eq!(top as usize - low as usize, BURI_RT_STACK_USABLE);
        }
        // SAFETY: nothing is running on it.
        unsafe { buri_rt_task_stack_release(base) };
    }

    // === B9: the benchmark row ===

    /// This process's resident set in kibibytes, or `None` where the platform
    /// will not say.
    ///
    /// **Not `ps -o rss=`**, which is what `memory.rs`'s G6 cases use and what
    /// this was written as first. On a macOS host with a hardened runtime `ps`
    /// answers *"rss: requires entitlement"* on standard error and nothing at
    /// all on standard output, so the probe reads zero and every assertion
    /// built on it passes vacuously. That is a measurement that cannot fail,
    /// which is the one kind this slice must not have.
    ///
    /// So: the kernel's own counter on each platform. `task_info` with
    /// `MACH_TASK_BASIC_INFO` on Darwin — declared here the way `memory.rs`
    /// declares `mmap`, because the dependency set is closed by an exact list
    /// — and the resident field of `/proc/self/statm` on Linux.
    #[cfg(target_os = "macos")]
    fn rss_kib() -> Option<u64> {
        /// `<mach/task_info.h>`'s `mach_task_basic_info`, whose second word is
        /// the only one read. The whole struct is declared because the count
        /// below is its size, and a short one is a write past the end.
        #[repr(C)]
        struct MachTaskBasicInfo {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_time: [i32; 2],
            system_time: [i32; 2],
            policy: i32,
            suspend_count: i32,
        }
        unsafe extern "C" {
            fn task_info(target: u32, flavor: u32, out: *mut u32, count: *mut u32) -> i32;
            static mach_task_self_: u32;
        }
        /// `MACH_TASK_BASIC_INFO`.
        const FLAVOR: u32 = 20;
        let mut info = MachTaskBasicInfo {
            virtual_size: 0,
            resident_size: 0,
            resident_size_max: 0,
            user_time: [0; 2],
            system_time: [0; 2],
            policy: 0,
            suspend_count: 0,
        };
        // `MACH_TASK_BASIC_INFO_COUNT`: the struct in `natural_t`s, which is
        // twelve, computed rather than spelled so the two cannot drift.
        let mut count = (size_of::<MachTaskBasicInfo>() / size_of::<u32>()) as u32;
        // SAFETY: a struct of exactly `count` words, and the task port is this
        // process's own.
        let rc = unsafe {
            task_info(mach_task_self_, FLAVOR, (&raw mut info).cast::<u32>(), &raw mut count)
        };
        (rc == 0).then(|| info.resident_size / 1024)
    }

    #[cfg(not(target_os = "macos"))]
    fn rss_kib() -> Option<u64> {
        // `/proc/self/statm`: total, resident, shared, … in pages.
        let text = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096 / 1024)
    }

    /// **What `n` parked tasks cost**, printed rather than asserted.
    ///
    /// The design's B9 row asks for one number: the resident set of ten
    /// thousand parked tasks, before and after. This is the harness that
    /// produces it, and it is written against [`task_start`], [`park_on`] and
    /// [`task_join`] alone — the three that are the same functions before and
    /// after the switch — so the two columns of the report's table are two
    /// runs of *this* source rather than two different measurements.
    ///
    /// Every task parks on a semaphore with no permits, which is a wait with
    /// no timer in it: what is being measured is what a task costs while it is
    /// *not* running, and a task woken every few milliseconds by a sleep would
    /// be measuring a scheduler instead. Permits are stored rather than
    /// signalled, so adding `n` of them after the count has arrived cannot lose
    /// a wakeup however late a task reaches its first poll.
    ///
    /// Two cases run this: `a_thousand_parked_tasks_do_not_cost_a_thousand_carriers`
    /// at a thousand, and `the_resident_set_of_ten_thousand_parked_tasks` below
    /// at ten thousand. Both are ordinary tests. The second was `#[ignore]`d on
    /// the theory that ten thousand of anything is seconds of wall clock — it is
    /// three tenths of a second and a hundred and sixty megabytes, which the
    /// suite can afford on every edit, and an ignored test proves nothing.
    fn park_n(n: usize) -> (u64, u64, usize) {
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let arrived = std::sync::Arc::new(AtomicUsize::new(0));
        let before = rss_kib().unwrap_or(0);
        let carriers_before = carriers();

        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let sem = std::sync::Arc::clone(&sem);
            let arrived = std::sync::Arc::clone(&arrived);
            handles.push(task_start(move || {
                arrived.fetch_add(1, Ordering::SeqCst);
                let _ = park_on(sem.acquire());
            }));
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        while arrived.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < deadline, "only {} of {n} tasks ever parked", arrived.load(Ordering::SeqCst));
            thread::sleep(Duration::from_millis(5));
        }
        // Every task is inside `acquire`, or one instruction from it, and none
        // will come out until the permits below.
        thread::sleep(Duration::from_millis(200));
        let parked = rss_kib().unwrap_or(0);
        let carriers_now = carriers();

        sem.add_permits(n);
        for h in handles {
            assert!(task_join(h), "a parked task did not finish");
        }
        (before, parked, carriers_now - carriers_before)
    }


    /// **What a switch costs, and what the stack seam costs**, printed rather
    /// than asserted.
    ///
    /// Two numbers, and they are the two paths this slice put in front of
    /// something that was already there:
    ///
    /// * a **machine-stack switch**, measured as a ping-pong between two
    ///   contexts on one thread, which is the operation that replaced a
    ///   channel send and a thread wake-up;
    /// * `buri_rt_stack_acquire` + `buri_rt_stack_release`, which gained
    ///   `memory::stack_list`'s question — is a task running on this thread? —
    ///   an `#[inline(never)]` call and a null test, once per entry into Buri
    ///   code. G6 measured the pair at 14 ns and the row in
    ///   `reports/wave8-b9.md` is against that.
    ///
    /// **No threshold is asserted** — a suite that failed on a loaded machine's
    /// jitter would be worse than no number at all — but the case is not
    /// `#[ignore]`d for that. What it asserts instead is the property the two
    /// million switches underneath the number are worth having run: that the
    /// contexts on both sides are still stack pointers the ABI would accept
    /// (`switch.rs` §2.1) after all of them. `switch::tests` makes that
    /// statement about one switch; this makes it about a stack that has been
    /// left and re-entered until the numbers settle.
    #[test]
    fn the_cost_of_the_switch_and_the_stack_seam() {
        use std::sync::atomic::AtomicUsize;
        static ONE: Mutex<()> = Mutex::new(());
        static HOME: AtomicUsize = AtomicUsize::new(0);
        static AWAY: AtomicUsize = AtomicUsize::new(0);

        /// Switches back, for ever. The case below stops asking.
        ///
        /// The two statics are read **once**, outside the loop, so that what
        /// the loop times is the switch and not two atomic loads beside it.
        ///
        /// The argument is the stack pointer `switch::plant_return`'s entry pad
        /// was entered with, which this measurement does not want; it is named
        /// rather than dropped because the pad passes it either way.
        extern "C" fn pong(_entry_sp: *mut u8) {
            let home = HOME.load(Ordering::SeqCst) as *const *mut u8;
            let away = AWAY.load(Ordering::SeqCst) as *mut *mut u8;
            if home.is_null() || away.is_null() {
                std::process::abort();
            }
            loop {
                // SAFETY: two words the case owns and keeps alive throughout.
                // `*home` is written by the other side's switch on every round,
                // so it is read volatile for the reason stated there.
                unsafe { switch::buri_rt_task_switch(away, home.read_volatile()) };
            }
        }

        let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        const ROUNDS: usize = 200_000;

        let block = vec![0u8; 256 * 1024];
        let top = ((block.as_ptr() as usize + block.len()) & !15usize) as *mut u8;
        // SAFETY: the top of a live block nothing else is running on.
        let start = unsafe { switch::prepare(top, std::ptr::null_mut()) };
        // The launch pad calls `buri_rt_task_main`, which wants a task; this
        // measurement wants the switch alone, so the frame is pointed at `pong`
        // instead — behind `plant_return`'s own pad, which is the launch pad's
        // stack-alignment half without its task half (`switch.rs` §2.2).
        //
        // **The two saved contexts live in a heap cell and are read back
        // `read_volatile`**, which is not fussiness: they are written by the
        // assembly, through pointers, while the loop below reads them, and a
        // plain local would be one a compiler is entitled to keep in a
        // register across the switch. A stale `away` is a switch into a context
        // that is already running, which is a signal rather than a wrong
        // number. The volatile load is one instruction and it is inside the
        // measurement, so the figure is an upper bound.
        let cell: Box<[*mut u8; 2]> = Box::new([std::ptr::null_mut(), start]);
        let cell = Box::into_raw(cell).cast::<*mut u8>();
        // SAFETY: a live two-word block, and the two halves are distinct.
        let (home, away) = unsafe { (cell, cell.add(1)) };
        HOME.store(home as usize, Ordering::SeqCst);
        AWAY.store(away as usize, Ordering::SeqCst);
        // SAFETY: overwriting the prepared frame's return word.
        unsafe { switch::plant_return(start, pong as *const ()) };

        // **Best of five**, which is `design/PERFORMANCE.md` §2's protocol and
        // not an optimism: what is being measured is a few tens of nanoseconds
        // on a machine that is running other work, so the minimum is the run
        // that was least interrupted and the mean is a measurement of the
        // interruptions.
        fn best_of(reps: usize, mut round: impl FnMut()) -> Duration {
            (0..reps)
                .map(|_| {
                    let began = Instant::now();
                    round();
                    began.elapsed()
                })
                .min()
                .expect("at least one repetition")
        }

        let switches = best_of(5, || {
            for _ in 0..ROUNDS {
                // SAFETY: `home` and `away` are the two words of the live cell;
                // `*away` is the pong's saved context, which nothing else is
                // running.
                unsafe {
                    let there = away.read_volatile();
                    switch::buri_rt_task_switch(home, there);
                }
            }
        });

        let seam = best_of(5, || {
            for _ in 0..ROUNDS {
                let p = crate::memory::buri_rt_stack_acquire();
                // SAFETY: just acquired on this list, nothing inside it.
                unsafe { crate::memory::buri_rt_stack_release(p) };
            }
        });

        println!(
            "B9 switch={:.1} ns/switch ({ROUNDS} round trips, two switches each)  \
             stack_acquire+release={:.1} ns",
            switches.as_nanos() as f64 / (ROUNDS as f64 * 2.0),
            seam.as_nanos() as f64 / ROUNDS as f64,
        );

        // What the loops leave behind. Both words are written by the assembly
        // on every round, so a null one is a ping-pong that never happened and
        // a misaligned one is every frame either side pushes from here on
        // sitting eight bytes off an aligned pointer.
        // SAFETY: the two halves of the live cell, written by the switch.
        let (mine, theirs) = unsafe { (home.read_volatile(), away.read_volatile()) };
        for (whose, sp) in [("this thread's", mine), ("the pong's", theirs)] {
            assert!(!sp.is_null(), "{whose} context was never saved: the ping-pong did not run");
            assert!(
                (sp as usize).is_multiple_of(16),
                "{whose} saved stack pointer is {sp:?} after {} round trips, which is not \
                 16-byte aligned",
                ROUNDS * 5,
            );
        }
        assert!(
            switches > Duration::ZERO && seam > Duration::ZERO,
            "a measurement of zero is a loop the optimiser removed, not a fast one",
        );
    }

    /// **The design's B9 row, at the size the row names**: what ten thousand
    /// parked tasks cost in resident set, and what they cost in threads.
    ///
    /// The number is printed and no threshold is put on it — a resident set is
    /// a measurement of a machine as much as of a runtime. The **thread** half
    /// is asserted, and it is the one this size is for: at ten thousand,
    /// thread-per-task does not merely cost more, it fails, because
    /// `pthread_create` answers `EAGAIN` on the 8 192nd thread of a process and
    /// `spawn_carrier` turns that into an abort. So a green run here is the
    /// statement that ten thousand tasks in flight are not ten thousand
    /// threads, which is the whole of the slice;
    /// `a_thousand_parked_tasks_do_not_cost_a_thousand_carriers` is the same
    /// statement one order of magnitude down and under the ratio.
    ///
    /// `BURI_B9_PARKED` sets the size, for a report that wants another one.
    #[test]
    fn the_resident_set_of_ten_thousand_parked_tasks() {
        let _alone = alone();
        let n: usize = std::env::var("BURI_B9_PARKED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_000);
        let (before, parked, started) = park_n(n);
        println!(
            "B9 parked={n} rss_before={before} KiB rss_parked={parked} KiB \
             delta={} KiB per_task={} B carriers_started={started}",
            parked.saturating_sub(before),
            (parked.saturating_sub(before) * 1024) / (n as u64),
        );
        assert!(
            started <= MAX_CARRIERS,
            "{n} parked tasks started {started} carriers, past the pool's own ceiling of \
             {MAX_CARRIERS}",
        );
        assert!(
            started * 8 < n,
            "{n} parked tasks started {started} carriers: a task that parks is holding a \
             thread, which is the arrangement this slice replaced",
        );
    }

    // -- `core/actor` -------------------------------------------------------
    //
    // The nine entries, driven directly and at the shape a Buri program
    // reaches them in: a one-element `[T]` of eight-byte elements, which is
    // what `core/actor`'s `Carried<T>` guarantees is never empty. Nothing here
    // calls Buri code, because nothing in the actor boundary does — the step is
    // the caller's, and `core/actor::draining` is where it is entered.
    //
    // **The references are counted by hand, exactly as generated code would.**
    // A block handed *to* an entry is still the caller's, and the entry takes
    // a second reference; a block handed *back* carries the entry's reference
    // and the caller is what releases it. So every one of these tests calls
    // [`drop_ref`] once per reference it is holding, and a test that got that
    // wrong would read a freed block under a sanitizer rather than pass.

    /// One block of one eight-byte element, with a distinguishing value in it.
    fn carried(mark: u64) -> BuriList {
        let list = block(1, 8);
        // SAFETY: `block(1, 8)` answers eight writable, aligned payload bytes.
        unsafe { list.ptr.cast::<u64>().write(mark) };
        list
    }

    /// What a carrier was built with, read back out of a block handed over.
    ///
    /// # Safety
    /// `list` names a live block of at least eight payload bytes.
    unsafe fn mark_of(list: &BuriList) -> u64 {
        // SAFETY: forwarded to the caller's promise.
        unsafe { list.ptr.cast::<u64>().read() }
    }

    /// Give one reference back. `[u64]` holds no counted pointer, so there is
    /// no drop glue to pass.
    fn drop_ref(list: &BuriList) {
        // SAFETY: a live payload pointer this test still holds a count on.
        unsafe { crate::memory::buri_rt_decref(list.ptr, None) };
    }

    fn nothing() -> BuriList {
        BuriList { ptr: std::ptr::null_mut(), len: 0 }
    }

    /// A mailbox is a queue: what one sender posted comes back in the order it
    /// posted it, and an empty one answers `.None` rather than waiting.
    #[test]
    fn a_mailbox_hands_messages_back_in_the_order_they_were_posted() {
        let state = carried(0);
        // SAFETY: a live one-element block.
        let actor = unsafe { buri_rt_actor_mailbox_open(state.ptr, state.len, 8) };
        drop_ref(&state);
        for mark in 1..=3u64 {
            let message = carried(mark);
            let mut depth = 0i64;
            // SAFETY: a live one-element block, and a writable `i64`.
            let ok = unsafe {
                buri_rt_actor_mailbox_push(actor, message.ptr, message.len, &raw mut depth)
            };
            assert_eq!(ok, crate::BURI_OK);
            assert_eq!(depth, mark as i64, "the depth is the number waiting");
            drop_ref(&message);
        }
        for mark in 1..=3u64 {
            let mut out = nothing();
            // SAFETY: a writable, aligned destination.
            let ok = unsafe { buri_rt_actor_mailbox_pop(actor, &raw mut out) };
            assert_eq!(ok, crate::BURI_OK);
            // SAFETY: the block the runtime handed back, still counted here.
            assert_eq!(unsafe { mark_of(&out) }, mark, "out of order");
            drop_ref(&out);
        }
        let mut out = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_mailbox_pop(actor, &raw mut out) }, 0);
        // SAFETY: a writable, aligned destination.
        let ok = unsafe { buri_rt_actor_mailbox_close(actor, &raw mut out) };
        assert_eq!(ok, crate::BURI_OK);
        drop_ref(&out);
    }

    /// **A bounded mailbox actually blocks.**
    ///
    /// The acceptance case for the bound, and it is here rather than in Buri
    /// because `core/actor::send` deliberately never reaches this wait — it
    /// runs the mailbox down when a post fills it, so a single-task program
    /// cannot see it. What can is a second carrier posting into an actor
    /// somebody else drives, which is what this drives directly.
    ///
    /// **Bounded at both ends.** The "it is still waiting" half is a deadline
    /// of its own, so a post that wrongly succeeded fails this test instead of
    /// passing it; and the "it finished" half has a deadline too, so a post
    /// that never woke is a failure rather than a hang.
    #[test]
    fn a_full_mailbox_makes_a_post_wait_for_room() {
        let state = carried(0);
        // SAFETY: a live one-element block.
        let actor = unsafe { buri_rt_actor_mailbox_open(state.ptr, state.len, 1) };
        drop_ref(&state);

        let first = carried(1);
        let mut depth = 0i64;
        // SAFETY: a live one-element block, and a writable `i64`.
        let ok =
            unsafe { buri_rt_actor_mailbox_push(actor, first.ptr, first.len, &raw mut depth) };
        assert_eq!(ok, crate::BURI_OK);
        drop_ref(&first);

        let (posted, arrived) = channel();
        let waiter = thread::spawn(move || {
            let second = carried(2);
            let mut depth = 0i64;
            // SAFETY: a live one-element block, and a writable `i64`.
            let ok = unsafe {
                buri_rt_actor_mailbox_push(actor, second.ptr, second.len, &raw mut depth)
            };
            drop_ref(&second);
            posted.send((ok, depth)).ok();
        });

        // Still waiting: the mailbox holds its one message and nothing has
        // taken it, so the post cannot have been accepted.
        assert!(
            arrived.recv_timeout(Duration::from_millis(150)).is_err(),
            "a post into a full mailbox did not wait"
        );

        let mut out = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_mailbox_pop(actor, &raw mut out) }, crate::BURI_OK);
        // SAFETY: the block the runtime handed back, still counted here.
        assert_eq!(unsafe { mark_of(&out) }, 1);
        drop_ref(&out);

        let (ok, depth) = arrived
            .recv_timeout(Duration::from_secs(5))
            .expect("the post never woke after the room was given back");
        assert_eq!(ok, crate::BURI_OK);
        assert_eq!(depth, 1);
        waiter.join().expect("the posting carrier panicked");
    }

    /// A `stop` while the mailbox is full does not leave the poster waiting:
    /// closing the room answers it `.None`, which `core/actor` reads as
    /// `.Err(.Stopped)`.
    #[test]
    fn closing_a_full_mailbox_answers_the_post_that_was_waiting() {
        let state = carried(0);
        // SAFETY: a live one-element block.
        let actor = unsafe { buri_rt_actor_mailbox_open(state.ptr, state.len, 1) };
        drop_ref(&state);
        let first = carried(1);
        let mut depth = 0i64;
        // SAFETY: a live one-element block, and a writable `i64`.
        unsafe { buri_rt_actor_mailbox_push(actor, first.ptr, first.len, &raw mut depth) };
        drop_ref(&first);

        let (posted, arrived) = channel();
        let waiter = thread::spawn(move || {
            let second = carried(2);
            let mut depth = 0i64;
            // SAFETY: a live one-element block, and a writable `i64`.
            let ok = unsafe {
                buri_rt_actor_mailbox_push(actor, second.ptr, second.len, &raw mut depth)
            };
            drop_ref(&second);
            posted.send(ok).ok();
        });
        assert!(arrived.recv_timeout(Duration::from_millis(150)).is_err());

        let mut out = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_mailbox_close(actor, &raw mut out) }, crate::BURI_OK);
        // SAFETY: the block the runtime handed back is the state.
        assert_eq!(unsafe { mark_of(&out) }, 0);
        drop_ref(&out);

        assert_eq!(
            arrived.recv_timeout(Duration::from_secs(5)).expect("the post never woke"),
            0,
            "a post into a closed mailbox is `.None`"
        );
        waiter.join().expect("the posting carrier panicked");
        // The message the closed mailbox never took is still the poster's, and
        // the poster released it: nothing here holds a second reference to it.
        let mut left = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_mailbox_pop(actor, &raw mut left) }, crate::BURI_OK);
        drop_ref(&left);
    }

    /// The state comes out once and goes back once, and a closed actor hands
    /// its final state to exactly one `mailboxClose`.
    #[test]
    fn an_actor_answers_its_final_state_once() {
        let state = carried(7);
        // SAFETY: a live one-element block.
        let actor = unsafe { buri_rt_actor_mailbox_open(state.ptr, state.len, 4) };
        drop_ref(&state);

        let mut held = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_state_take(actor, &raw mut held) }, crate::BURI_OK);
        // SAFETY: the block the runtime handed back, still counted here.
        assert_eq!(unsafe { mark_of(&held) }, 7);
        // A second take finds the baton gone, and does not wait for it.
        let mut again = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_state_take(actor, &raw mut again) }, 0);

        let next = carried(8);
        let mut depth = 0i64;
        // SAFETY: a live one-element block, and a writable `i64`.
        assert_eq!(
            unsafe { buri_rt_actor_state_put(actor, next.ptr, next.len, &raw mut depth) },
            crate::BURI_OK
        );
        drop_ref(&next);
        drop_ref(&held);

        let mut out = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_mailbox_close(actor, &raw mut out) }, crate::BURI_OK);
        // SAFETY: the block the runtime handed back is what `statePut` stored.
        assert_eq!(unsafe { mark_of(&out) }, 8, "the close answers the *final* state");
        drop_ref(&out);

        // Once. A second close is `.None`, which is what makes `onStop` run
        // exactly once without `core/actor` having to remember.
        let mut twice = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_mailbox_close(actor, &raw mut twice) }, 0);
        // A post after it is `.None` too, and does not take the block.
        let late = carried(9);
        // SAFETY: a live one-element block, and a writable `i64`.
        assert_eq!(
            unsafe { buri_rt_actor_mailbox_push(actor, late.ptr, late.len, &raw mut depth) },
            0
        );
        drop_ref(&late);
    }

    /// A `stop` that races a step waits for it: the close does not answer
    /// until the state is back, which is "`stop` lets the current message
    /// finish" enforced by the runtime rather than asked of the caller.
    #[test]
    fn a_close_waits_for_the_step_in_flight() {
        let state = carried(1);
        // SAFETY: a live one-element block.
        let actor = unsafe { buri_rt_actor_mailbox_open(state.ptr, state.len, 4) };
        drop_ref(&state);
        let mut held = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_state_take(actor, &raw mut held) }, crate::BURI_OK);

        let (closed, arrived) = channel();
        let closer = thread::spawn(move || {
            let mut out = nothing();
            // SAFETY: a writable, aligned destination.
            let ok = unsafe { buri_rt_actor_mailbox_close(actor, &raw mut out) };
            if ok != crate::BURI_OK {
                closed.send((ok, 0u64)).ok();
                return;
            }
            // SAFETY: the block the runtime handed back, still counted here.
            let mark = unsafe { mark_of(&out) };
            drop_ref(&out);
            closed.send((ok, mark)).ok();
        });
        assert!(
            arrived.recv_timeout(Duration::from_millis(150)).is_err(),
            "a close answered while a step held the state"
        );

        let next = carried(2);
        let mut depth = 0i64;
        // SAFETY: a live one-element block, and a writable `i64`.
        unsafe { buri_rt_actor_state_put(actor, next.ptr, next.len, &raw mut depth) };
        drop_ref(&next);
        drop_ref(&held);

        let (ok, mark) = arrived
            .recv_timeout(Duration::from_secs(5))
            .expect("the close never woke after the state came back");
        assert_eq!(ok, crate::BURI_OK);
        assert_eq!(mark, 2, "the close answers what the step left, not what it started with");
        closer.join().expect("the closing carrier panicked");
    }

    /// A reply is answered once, read once, and its slot comes back at a new
    /// generation — so the handle that was just spent names nothing.
    #[test]
    fn a_reply_is_answered_once_and_its_handle_expires() {
        let slot = buri_rt_actor_reply_open();
        let mut out = nothing();
        // Nothing there yet, and the read that found nothing does not spend it.
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_reply_take(slot, &raw mut out) }, 0);

        let value = carried(42);
        let mut ack = 0i64;
        // SAFETY: a live one-element block, and a writable `i64`.
        assert_eq!(
            unsafe { buri_rt_actor_reply_put(slot, value.ptr, value.len, &raw mut ack) },
            crate::BURI_OK
        );
        drop_ref(&value);
        // A second answer is refused rather than overwriting the first.
        let other = carried(43);
        // SAFETY: a live one-element block, and a writable `i64`.
        assert_eq!(
            unsafe { buri_rt_actor_reply_put(slot, other.ptr, other.len, &raw mut ack) },
            0
        );
        drop_ref(&other);

        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_reply_take(slot, &raw mut out) }, crate::BURI_OK);
        // SAFETY: the block the runtime handed back, still counted here.
        assert_eq!(unsafe { mark_of(&out) }, 42);
        drop_ref(&out);

        // Spent, and the slot is reusable — but not through the old handle.
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_reply_take(slot, &raw mut out) }, 0);
        let fresh = buri_rt_actor_reply_open();
        assert_ne!(fresh, slot, "a reused slot answers a new handle");
        let late = carried(1);
        // SAFETY: a live one-element block, and a writable `i64`.
        assert_eq!(
            unsafe { buri_rt_actor_reply_put(slot, late.ptr, late.len, &raw mut ack) },
            0,
            "the spent handle names nothing"
        );
        // SAFETY: a live one-element block, and a writable `i64`.
        assert_eq!(
            unsafe { buri_rt_actor_reply_put(fresh, late.ptr, late.len, &raw mut ack) },
            crate::BURI_OK
        );
        drop_ref(&late);
        let mut back = nothing();
        // SAFETY: a writable, aligned destination.
        assert_eq!(unsafe { buri_rt_actor_reply_take(fresh, &raw mut back) }, crate::BURI_OK);
        drop_ref(&back);
    }

    /// A handle that names nothing answers `.None` from every entry rather
    /// than aborting: it cannot arise from a program, and a runtime that
    /// aborted on it would report a toolchain bug as a program error.
    #[test]
    fn a_handle_that_names_nothing_is_answered_and_not_aborted() {
        let mut list = nothing();
        let mut number = 0i64;
        let held = carried(1);
        for handle in [-1i64, i64::MAX] {
            // SAFETY: writable destinations, and a live one-element block.
            unsafe {
                assert_eq!(buri_rt_actor_mailbox_pop(handle, &raw mut list), 0);
                assert_eq!(buri_rt_actor_mailbox_close(handle, &raw mut list), 0);
                assert_eq!(buri_rt_actor_state_take(handle, &raw mut list), 0);
                assert_eq!(
                    buri_rt_actor_state_put(handle, held.ptr, held.len, &raw mut number),
                    0
                );
                assert_eq!(
                    buri_rt_actor_mailbox_push(handle, held.ptr, held.len, &raw mut number),
                    0
                );
                assert_eq!(buri_rt_actor_reply_take(handle, &raw mut list), 0);
                assert_eq!(
                    buri_rt_actor_reply_put(handle, held.ptr, held.len, &raw mut number),
                    0
                );
            }
        }
        drop_ref(&held);
    }
}
