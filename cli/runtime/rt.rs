//! The carrier runtime — the tokio handle, the carrier pool and the task
//! table, behind feature `net`.
//!
//! Design: `design/native` track B, §0.1 ("carrier threads with a run baton,
//! then stack switching") and §4 ("Runtime choices, concretely").
//!
//! ## 0. What a green task is, and what it is not
//!
//! A Buri task is **an OS thread from a pool**, not a state machine. Buri
//! machine code stays exactly what it is today — ordinary frame-threaded
//! synchronous code, no CPS, no coroutine, no `musttail` — and a suspending
//! host call is one [`park_on`] in an otherwise unremarkable `extern "C"` body.
//! That is the whole of the integration, and it is deliberate: a native CPS
//! transform is larger than both backends' relooper-shaped alternatives put
//! together, and `middle/mod.rs` and `design/native/CODEGEN-LLVM.md` have each
//! already rejected the shape once.
//!
//! Phase 2 (track B, B9) replaces the carrier *thread* with a hand-written
//! stack switch per `(arch, os)`, behind this same table and this same
//! [`park_on`]. Nothing above this file changes when it does.
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
//! There is also **no `buri_rt_task_*` C ABI here**, on purpose. The table and
//! the pool are Rust-visible only. An exported symbol is a contract both
//! backends emit calls into, and writing one before a backend has a call to
//! emit is a guess about a signature; track D is where the first one is a
//! call rather than a prediction.
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
//! Carrier stacks are [`CARRIER_STACK_BYTES`]. That is the *machine* stack; a
//! carrier additionally acquires a Buri data stack with its own guard page
//! from `memory::buri_rt_stack_acquire` (track B, B7), and `main`'s static
//! block is left exactly as it is.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::{self, ThreadId};

use crate::list::{block, StepEntry};
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

/// Wait for `future` on this thread.
///
/// **Three lines until G3, and one after it.** The two that went were the run
/// baton's: a suspending call used to give the baton up for the duration of
/// the wait and take it back before returning, which is what made I/O overlap
/// while two Buri functions still never ran at once. With the baton gone there
/// is nothing to give up — a carrier that waits is simply a carrier that is
/// not running, and the carriers that *are* running were already allowed to
/// (§1) — so what is left is the `block_on` that was always underneath.
///
/// The future is polled on **this** thread — `Handle::block_on` drives it here
/// and the reactor's threads only wake it — so it need be neither `Send` nor
/// `'static`, and a body that is still synchronous (`http.rs`'s client) costs
/// no copy and no thread hop to route through here. What it gains is that the
/// same call site becomes a real await when the client behind it does.
///
/// It stays a named function rather than becoming `handle().block_on(...)` at
/// its two call sites, because "this is a suspension point" is a fact
/// `host.rs` states about `sleepMillis` and `fetch` and the compiler's
/// `rc::suspends` list agrees with, and a wrapper is where that fact is
/// written down.
///
/// # Panics
/// If called from a tokio worker thread, which `Handle::block_on` refuses. No
/// carrier is one, and nothing in `host.rs` runs inside a task.
pub fn park_on<T>(future: impl Future<Output = T>) -> T {
    handle().block_on(future)
}

// ---------------------------------------------------------------------------
// The carrier pool
// ---------------------------------------------------------------------------

/// A carrier's machine stack, in bytes.
///
/// 512 KiB, from `design/native` track B §4. It is not the Buri data stack:
/// that one is `main`'s 64 MiB `__bss` block today, and becomes a per-carrier
/// block with its own `PROT_NONE` guard in B7. This number bounds the *Rust*
/// frames a carrier can hold — the runtime's own, and the platform's — which
/// is a shallow, bounded thing, so 512 KiB is generous rather than tight.
pub const CARRIER_STACK_BYTES: usize = 512 * 1024;

/// What a carrier is handed, and the channel it answers on when it is done.
///
/// The completion signal is the **carrier's**, not the job's, and that
/// ordering is the point: a carrier puts itself back in the pool and *then*
/// says the job is finished, so a caller that dispatches again the instant it
/// has an answer finds an idle carrier rather than starting a second one.
type Errand = (Job, Sender<()>);

/// The work itself, with its answer left where [`Handoff`] can pick it up.
type Job = Box<dyn FnOnce() + Send + 'static>;

/// The idle carriers, each named by the channel that reaches it.
///
/// A carrier puts *itself* back here when its job returns, so this vector is
/// the pool's whole state: [`on_carrier`] pops one or starts one, and the
/// balance is exactly one push per pop.
static IDLE: Mutex<Vec<Sender<Errand>>> = Mutex::new(Vec::new());

/// How many carrier threads this process has ever started.
///
/// A count and not a set, because carriers are never retired: a pool that
/// reaped idle threads would trade a thread for a thread creation on every
/// burst, and B9 deletes the threads outright.
static CARRIERS: AtomicUsize = AtomicUsize::new(0);

fn idle() -> MutexGuard<'static, Vec<Sender<Errand>>> {
    match IDLE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// How many carrier threads exist.
pub fn carriers() -> usize {
    CARRIERS.load(Ordering::Relaxed)
}

/// Start a carrier and answer the channel that reaches it.
///
/// The thread keeps its own sender, so the receive loop never ends and the
/// carrier lives as long as the process. That is the lifetime the table below
/// has and the one `testing.rs`'s table has: a runtime that tore its own
/// workers down would need a shutdown ordering, and a native Buri program's
/// shutdown is `buri_rt_flush` and a return.
fn spawn_carrier() -> Sender<Errand> {
    let (tx, rx): (Sender<Errand>, Receiver<Errand>) = channel();
    let mine = tx.clone();
    let id = CARRIERS.fetch_add(1, Ordering::Relaxed);
    let started = thread::Builder::new()
        .name(format!("buri-carrier-{id}"))
        .stack_size(CARRIER_STACK_BYTES)
        .spawn(move || {
            while let Ok((job, done)) = rx.recv() {
                job();
                idle().push(mine.clone());
                // After the push, so that "the answer is here" implies "the
                // carrier is available". A caller that has already dropped its
                // handoff is not an error: it asked for the work, not for the
                // answer.
                let _ = done.send(());
            }
        });
    if let Err(e) = started {
        panic!("the buri runtime could not start a carrier: {e}");
    }
    tx
}

/// What [`on_carrier`] answers: the value the job produced, once it has.
///
/// The answer travels in a slot rather than down the completion channel
/// because the carrier loop is the thing that signals and it does not know
/// `T`. `join` answers `None` where the carrier did not finish — which, under
/// `panic = "abort"`, cannot happen in a released runtime, and can happen
/// under a test harness that unwinds.
#[must_use = "dropping the handoff drops the answer the carrier is computing"]
pub struct Handoff<T> {
    done: Receiver<()>,
    answer: Arc<Mutex<Option<T>>>,
}

impl<T> Handoff<T> {
    /// Wait for the job and answer what it produced.
    pub fn join(self) -> Option<T> {
        self.done.recv().ok()?;
        match self.answer.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }
}

/// Run `f` on a carrier from the pool, reusing an idle one where there is one.
///
/// The job runs as it is written. Until G3 it was handed to `as_carrier`,
/// which took the run baton first, and [`task_start`] and [`fan_out`] were the
/// two callers that wrapped because a task and a step are both Buri code.
/// There is no baton to take now, so the wrapper is gone with it and this is
/// the whole of what starting a carrier means.
pub fn on_carrier<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Handoff<T> {
    let (done, finished) = channel();
    let answer = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&answer);
    let job: Job = Box::new(move || {
        let out = f();
        match slot.lock() {
            Ok(mut slot) => *slot = Some(out),
            Err(poisoned) => *poisoned.into_inner() = Some(out),
        }
    });
    let carrier = idle().pop().unwrap_or_else(spawn_carrier);
    // A carrier holds its own sender for the life of the process, so the only
    // way this fails is one that left its loop, which `panic = "abort"` makes
    // unreachable in a released runtime.
    if carrier.send((job, done)).is_err() {
        panic!("a buri carrier is gone");
    }
    Handoff { done: finished, answer }
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
/// A carrier is an OS thread with a [`CARRIER_STACK_BYTES`] stack, so this
/// number is address space: sixty-four of them is 32 MiB of carrier stacks per
/// level of nesting. Unbounded would be the shape that matches JavaScript's
/// `Promise.all` exactly, and it is the wrong shape while a task costs a
/// thread — a list of ten thousand items would ask the kernel for ten thousand
/// threads, which macOS refuses long before it runs out of memory, and
/// [`spawn_carrier`] turns that refusal into an abort. A program that fans out
/// wider than this simply waits: the window slides, so the (n + 1)-th step
/// starts when the first has finished, and the answer and its order are the
/// same either way.
///
/// B9 is what removes the bound rather than raises it — a parked task that
/// costs a stack switch and no thread has no reason to be counted.
const IN_FLIGHT: usize = 64;

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


#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn a_carrier_runs_the_job_and_is_reused() {
        let _alone = alone();
        let here = thread::current().id();
        let first = on_carrier(move || (thread::current().id(), 6 * 7)).join().unwrap();
        assert_eq!(first.1, 42);
        assert_ne!(first.0, here, "the job ran on the calling thread");

        // The carrier put itself back, so the next job — with nothing else in
        // flight — lands on the same thread rather than on a new one.
        let before = carriers();
        let second = on_carrier(move || thread::current().id()).join().unwrap();
        assert_eq!(second, first.0, "an idle carrier was not reused");
        assert_eq!(carriers(), before, "a carrier was started for a job an idle one could take");
    }

    #[test]
    fn four_carriers_all_answer() {
        let _alone = alone();
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
        assert!(carriers() >= 4, "four jobs in flight shared fewer than four carriers");
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
    /// **D4's acceptance case, and its number does not move in G3.** It was
    /// green under the baton — a step that waits was already giving the baton
    /// up — so the ~100 ms it measures is the same ~100 ms before and after,
    /// and that is the point of re-running it here rather than of changing it.
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
}
