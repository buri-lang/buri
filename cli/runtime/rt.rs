//! The carrier runtime — the tokio handle, the run baton, the carrier pool and
//! the task table, behind feature `net`.
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
//! ## 1. The baton, and why concurrency lands before parallelism
//!
//! [`Baton`] is a token: **exactly one carrier runs Buri code at a time.** A
//! suspending call gives it up for the duration of the wait and takes it back
//! before returning, so I/O overlaps, a server can have many connections in
//! flight and `Tasks.parallel` can interleave — while two Buri *functions*
//! never execute simultaneously.
//!
//! That distinction is the staging device for the whole of track B. Non-atomic
//! refcounts, the single-threaded allocator and every `rc == 1` in-place update
//! (`memory.rs`) stay correct **untouched**, because the thing they are not
//! safe against does not happen yet. `design/native/MEMORY.md` prices atomic
//! refcounting at 2–3×; the baton lets that cost land as its own slice (track
//! G) rather than as a prerequisite for every other one.
//!
//! ## 2. What creates a second carrier, and what still does not
//!
//! [`buri_rt_host_tasks_parallel`] does: it fans a `Tasks.parallel` call's
//! steps onto the pool, one carrier each, and gives the baton up while it waits
//! for them. That is the only thing in a Buri program that starts a carrier
//! today — [`task_start`] and the table below are the shape track F's
//! `core/actor` needs and nothing calls them yet.
//!
//! And it does it **only where a carrier entering Buri code gets frames of its
//! own**, which is the artifact's statement about itself and not this file's
//! guess: `lib.rs`'s `buri_rt_frames_are_per_carrier`. Where a program has one
//! Buri stack — the frame-threaded backend's, until each carrier owns one
//! (track B, B7) — the steps run one after another on the calling carrier, in
//! index order, answering the same `[B]`. The order promise is what makes those
//! two the same program; the timing is not part of it.
//!
//! [`Clock::sleepMillis`][slp] and [`Net::fetch`][fch] route through
//! [`park_on`], so two steps that wait now genuinely overlap. Two steps that
//! *compute* do not: the baton admits one carrier at a time and track G is what
//! removes it.
//!
//! One consequence is worth stating rather than leaving to be discovered: the
//! **process carrier still does not hold the baton**. It has nothing to be
//! excluded from — while it is inside a `parallel` it is dispatching and
//! waiting, not running Buri code, and when it is not, it is the only thread
//! that runs any — so [`park_on`] and [`fan_out`] both give the baton up only
//! if the calling carrier is actually holding it, which a carrier running a
//! *nested* `parallel` is and the process carrier is not. Taking it at entry
//! would make those two conditionals unconditional and would buy nothing else;
//! it needs a runtime entry hook `main` calls, and the slice that wants one
//! (track F's detached tasks, which outlive the call that started them) is the
//! one that should add it. `parking_without_the_baton_takes_no_baton` pins
//! today's answer so that the change is a visible one.
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
//! Carrier stacks are [`CARRIER_STACK_BYTES`]. That is the *machine* stack; on
//! the stencil backend a carrier will additionally own a Buri data stack with
//! its own guard page (track B, B7), and `main`'s static block is left exactly
//! as it is.

use std::collections::VecDeque;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
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
// The run baton
// ---------------------------------------------------------------------------

/// The right to come back, handed out by [`Baton::release`] and consumed by
/// [`Baton::acquire`].
///
/// A zero-sized `#[must_use]` value rather than a bare pair of calls, and
/// deliberately **not** `Send`: the carrier that gave the baton up is the
/// carrier that takes it back, and a ticket that could cross a thread boundary
/// would make "release here, acquire there" a thing the type system permits.
#[must_use = "a released baton must be acquired again before running Buri code"]
pub struct Ticket(PhantomData<*const ()>);

/// The run baton: **exactly one carrier runs Buri code at a time.**
///
/// A `Mutex`/`Condvar` pair holding the thread that has it, rather than a
/// `Mutex` guard, because the hold is not lexical — [`park_on`] releases in
/// the middle of a call and reacquires before it returns, and a guard cannot
/// be dropped and revived across that.
///
/// The holder is recorded so that [`Baton::held_here`] can answer, which is
/// what makes a double release a panic with a name on it rather than a
/// silently over-released token and two carriers in Buri code at once.
pub struct Baton {
    holder: Mutex<Option<ThreadId>>,
    free: Condvar,
}

impl Baton {
    /// A baton nobody is holding.
    ///
    /// Public because a test wants one that is not the process's — the global
    /// baton is held for the life of whatever carrier took it, so a test that
    /// borrowed it would be a test that stopped every later one.
    pub const fn new() -> Self {
        Self { holder: Mutex::new(None), free: Condvar::new() }
    }

    /// The process's baton.
    pub fn global() -> &'static Baton {
        static BATON: Baton = Baton::new();
        &BATON
    }

    /// Take the baton, blocking until whoever has it gives it up.
    ///
    /// This is a carrier's *entry*: the first thing a thread does before it
    /// runs Buri code, and the counterpart of the [`Ticket`]-carrying pair
    /// below, which is the same operation in the middle of a call.
    ///
    /// # Panics
    /// If the calling thread already holds it. That is a runtime bug — a
    /// carrier entered twice — and it would otherwise deadlock against itself.
    pub fn hold(&self) {
        let mut holder = self.lock();
        let me = thread::current().id();
        assert!(
            *holder != Some(me),
            "a carrier tried to take the run baton twice; the second take would wait for the first"
        );
        while holder.is_some() {
            holder = match self.free.wait(holder) {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        *holder = Some(me);
    }

    /// Give the baton up. Another carrier may now run Buri code.
    ///
    /// # Panics
    /// If the calling thread is not holding it.
    pub fn release(&self) -> Ticket {
        let mut holder = self.lock();
        let me = thread::current().id();
        assert!(
            *holder == Some(me),
            "a carrier released the run baton it was not holding"
        );
        *holder = None;
        drop(holder);
        // One waiter, because the baton admits one: waking the rest would be a
        // thundering herd whose every member goes straight back to sleep.
        self.free.notify_one();
        Ticket(PhantomData)
    }

    /// Take the baton back, consuming the ticket [`Baton::release`] gave out.
    pub fn acquire(&self, ticket: Ticket) {
        let Ticket(_) = ticket;
        self.hold();
    }

    /// Whether the calling thread is the one holding it.
    pub fn held_here(&self) -> bool {
        *self.lock() == Some(thread::current().id())
    }

    /// Whether anybody is holding it.
    pub fn held(&self) -> bool {
        self.lock().is_some()
    }

    fn lock(&self) -> MutexGuard<'_, Option<ThreadId>> {
        match self.holder.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Default for Baton {
    fn default() -> Self {
        Self::new()
    }
}

/// A carrier's hold on the process baton, released when it is dropped.
///
/// `!Send` for [`Ticket`]'s reason, and it exists so that a panic on a carrier
/// releases the baton instead of wedging every other one behind a thread that
/// is no longer there.
#[must_use = "dropping the hold immediately gives the baton straight back"]
pub struct Held(PhantomData<*const ()>);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = Baton::global().release();
    }
}

/// Run `f` as a carrier: take the process baton, run, give it back.
///
/// This is what a spawned carrier's body is wrapped in, and what a test uses
/// to stand in for the process carrier that does not hold the baton yet (§2).
pub fn as_carrier<T>(f: impl FnOnce() -> T) -> T {
    Baton::global().hold();
    let _held = Held(PhantomData);
    f()
}

// ---------------------------------------------------------------------------
// Parking
// ---------------------------------------------------------------------------

/// Wait for `future` without holding the run baton.
///
/// The three lines the design sketches, plus the conditional §2 explains: the
/// process carrier does not hold the baton yet, so today the release and the
/// reacquire are both skipped and this is `block_on` with the same answer as
/// before. A carrier that *is* holding it gives it up for the whole of the
/// wait, which is what makes overlapped I/O real the moment a second carrier
/// exists.
///
/// The future is polled on **this** thread — `Handle::block_on` drives it here
/// and the reactor's threads only wake it — so it need be neither `Send` nor
/// `'static`, and a body that is still synchronous (`http.rs`'s client) costs
/// no copy and no thread hop to route through here. What it gains today is the
/// baton discipline; what it gains later is that the same call site becomes a
/// real await when the client behind it does.
///
/// # Panics
/// If called from a tokio worker thread, which `Handle::block_on` refuses. No
/// carrier is one, and nothing in `host.rs` runs inside a task.
pub fn park_on<T>(future: impl Future<Output = T>) -> T {
    let baton = Baton::global();
    let ticket = baton.held_here().then(|| baton.release());
    let out = handle().block_on(future);
    if let Some(ticket) = ticket {
        baton.acquire(ticket);
    }
    out
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
/// The job is **not** wrapped in [`as_carrier`]: a carrier that is going to
/// run Buri code takes the baton, and one doing runtime work of its own does
/// not, so the choice belongs to the caller. [`task_start`] and `fan_out` are
/// the two that wrap, because a task and a step are both Buri code; nothing
/// wraps nothing yet.
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
/// `f` runs **as a carrier** — it takes the baton before its first instruction
/// — because a task is a thing that runs Buri code. That is what makes a task
/// started from Buri code wait for the starter to suspend rather than run
/// beside it.
pub fn task_start(f: impl FnOnce() + Send + 'static) -> i64 {
    install(Slot::Running(on_carrier(move || as_carrier(f))))
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
// **Concurrent, on the carrier pool, answering in index order.** Every step is
// dispatched to a carrier of its own; the calling carrier gives the run baton up
// for as long as it is waiting and takes it back before it returns. So two steps
// that wait overlap, and two steps that *compute* still do not: the baton keeps
// exactly one carrier in Buri code at a time, which is what leaves the
// single-threaded allocator and the non-atomic refcounts correct untouched
// (§1). Track G is the slice that removes the baton; this one only makes it earn
// its keep.
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
// carriers touch the same byte, and the run baton keeps them out of Buri code
// at the same time.
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
/// What this file did before there was a fan-out, and still the answer for a
/// program whose artifact shares one Buri stack (`lib.rs`'s
/// `buri_rt_frames_are_per_carrier`) and for the one- and no-item cases, where a
/// carrier would be a thread started to do what this thread is already doing.
///
/// # Safety
/// `steps` describes `n` live elements and `n` writable slots.
unsafe fn in_order(steps: Steps, n: usize) {
    for i in 0..n {
        // SAFETY: `i < n`, and each index is run exactly once.
        unsafe { steps.run(i) };
    }
}

/// Every step on a carrier of its own, at most [`IN_FLIGHT`] at a time.
///
/// The baton is given up **before** the first carrier is dispatched, for two
/// reasons and not one. A carrier's body begins by taking it ([`as_carrier`]),
/// so a caller still holding it would be a caller waiting for work that is
/// waiting for the caller. And a step that finishes during the dispatch loop
/// puts its carrier back in the pool in time for the next index to reuse it, so
/// a `parallel` over items that do not wait costs a handful of threads rather
/// than one per item.
///
/// Between the release and the reacquire this thread runs no Buri code — it
/// dispatches, and it waits — which is the whole of what the baton excludes.
///
/// # Safety
/// As [`in_order`].
unsafe fn fan_out(steps: Steps, n: usize) {
    let baton = Baton::global();
    let ticket = baton.held_here().then(|| baton.release());
    let mut window: VecDeque<Handoff<()>> = VecDeque::with_capacity(n.min(IN_FLIGHT));
    for i in 0..n {
        if window.len() == IN_FLIGHT {
            finish(window.pop_front());
        }
        // SAFETY: `i < n`, each index dispatched once, and `Steps` is `Send`
        // for the reason stated at its `unsafe impl`.
        window.push_back(on_carrier(move || as_carrier(|| unsafe { steps.run(i) })));
    }
    while let Some(handoff) = window.pop_front() {
        finish(Some(handoff));
    }
    if let Some(ticket) = ticket {
        baton.acquire(ticket);
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

/// `Tasks.parallel(self, items, f) -> [B]` — `f` at every item, answering in
/// index order.
///
/// The `[B]` block is allocated here and every element of it is written by a
/// step, so the result is fully initialised before it is handed back — there is
/// no arm below that skips one.
///
/// Whether the steps run on carriers of their own or one after another on this
/// one is the artifact's answer, not this call's: see
/// [`crate::buri_rt_frames_are_per_carrier`]. Either way the `[B]` is the same
/// `[B]`, which is what `core/tasks`'s order promise is worth.
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
    unsafe {
        if n > 1 && crate::frames_are_per_carrier() {
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
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};

    /// The pool, the task table and the *process* baton are one shared thing,
    /// and `cargo test` runs this module's cases on many threads at once. A
    /// case that asks "was the idle carrier reused" or "is the process baton
    /// free" is asking about global state, so the cases that do take this
    /// first. Nothing outside this file touches any of the three, so it is a
    /// lock over this module and not over the crate.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn alone() -> MutexGuard<'static, ()> {
        match ONE_AT_A_TIME.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Deliberately a **relaxed load, a yield, and a relaxed store** rather
    /// than a `fetch_add`. A read-modify-write split in three is exactly what
    /// mutual exclusion has to make safe, and a lost update is then a wrong
    /// number rather than undefined behaviour — which is what lets this be a
    /// stress test rather than a thing only `loom` or a sanitizer could see.
    fn increment_slowly(counter: &AtomicU64) {
        let seen = counter.load(Ordering::Relaxed);
        thread::yield_now();
        counter.store(seen + 1, Ordering::Relaxed);
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

    #[test]
    fn the_baton_admits_one_carrier_at_a_time() {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        static INSIDE: AtomicUsize = AtomicUsize::new(0);
        static OVERLAPS: AtomicUsize = AtomicUsize::new(0);
        // Its own baton rather than the process's, so this case says nothing
        // about what any other case is doing with that one.
        static BATON: Baton = Baton::new();

        const CARRIERS: usize = 8;
        const ROUNDS: usize = 400;

        thread::scope(|scope| {
            for _ in 0..CARRIERS {
                scope.spawn(|| {
                    for _ in 0..ROUNDS {
                        BATON.hold();
                        if INSIDE.fetch_add(1, Ordering::SeqCst) != 0 {
                            OVERLAPS.fetch_add(1, Ordering::SeqCst);
                        }
                        increment_slowly(&COUNTER);
                        INSIDE.fetch_sub(1, Ordering::SeqCst);
                        // The ticket is the right to come back, and this round
                        // does not: dropping it is how a carrier leaves.
                        drop(BATON.release());
                    }
                });
            }
        });

        assert_eq!(OVERLAPS.load(Ordering::SeqCst), 0, "two carriers held the baton at once");
        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            (CARRIERS * ROUNDS) as u64,
            "an update was lost, so the increments were not mutually exclusive",
        );
        assert!(!BATON.held(), "the baton was left held");
    }

    /// The ordering the sketch in `design/native` promises: a carrier that
    /// releases hands the baton to whoever is waiting, and gets it back only
    /// once that one has given it up.
    #[test]
    fn a_released_baton_goes_to_a_waiting_carrier_and_comes_back() {
        static BATON: Baton = Baton::new();
        let order: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

        BATON.hold();
        order.lock().unwrap().push("first in");

        thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                BATON.hold();
                order.lock().unwrap().push("second in");
                order.lock().unwrap().push("second out");
                drop(BATON.release());
            });

            // Long enough for the waiter to reach `hold` and block there. It
            // cannot get in whether it has or not, which is why this sleep is
            // not what the assertions rest on.
            thread::sleep(Duration::from_millis(20));
            assert!(BATON.held_here(), "the waiter took the baton off its holder");

            let ticket = BATON.release();
            order.lock().unwrap().push("first parked");
            waiter.join().unwrap();
            BATON.acquire(ticket);
            order.lock().unwrap().push("first back");
        });

        drop(BATON.release());
        assert_eq!(
            *order.lock().unwrap(),
            ["first in", "first parked", "second in", "second out", "first back"],
            "the second carrier did not run inside the first one's park",
        );
        assert!(!BATON.held());
    }

    /// `assert!` with a bare message panics with a `&'static str` payload and
    /// with a formatted one with a `String`; a test that read only one of them
    /// would pass on an empty message.
    fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
        payload
            .downcast_ref::<&'static str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<the panic carried no message>")
    }

    #[test]
    fn a_double_take_is_named_rather_than_a_deadlock() {
        static BATON: Baton = Baton::new();
        let twice = thread::spawn(|| {
            BATON.hold();
            BATON.hold();
        });
        let panicked = twice.join().expect_err("taking the baton twice was allowed");
        let message = panic_message(&*panicked);
        assert!(message.contains("twice"), "the panic did not name the fault: {message}");
    }

    #[test]
    fn a_release_by_a_carrier_that_is_not_holding_is_named() {
        static BATON: Baton = Baton::new();
        let wrong = thread::spawn(|| drop(BATON.release()));
        let panicked = wrong.join().expect_err("an unheld baton was released");
        let message = panic_message(&*panicked);
        assert!(message.contains("not holding"), "the panic did not name the fault: {message}");
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

    /// The process carrier's situation today (§2): it holds nothing, so
    /// parking touches no baton and answers exactly as it did before.
    #[test]
    fn parking_without_the_baton_takes_no_baton() {
        assert!(!Baton::global().held_here());
        assert_eq!(park_on(async { "unchanged" }), "unchanged");
        assert!(!Baton::global().held_here(), "parking took a baton it had not been given");
    }

    /// The claim §1 is built on, and the only way to state it without a timing
    /// race: the parking carrier is *already* holding the baton when the
    /// second one is dispatched, so the second can only run if the park gave it
    /// up. If it never does, the timeout answers instead of the send, and the
    /// assertion names it rather than hanging the suite.
    #[test]
    fn a_parked_carrier_lets_the_next_one_in() {
        let _alone = alone();
        as_carrier(|| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let second = on_carrier(move || {
                as_carrier(move || {
                    let _ = tx.send("the second carrier ran");
                })
            });
            let arrived =
                park_on(async { tokio::time::timeout(Duration::from_secs(5), rx).await });
            second.join().expect("the second carrier did not finish");
            let arrived = arrived.expect("the parked carrier never released the baton");
            assert_eq!(
                arrived.expect("the second carrier dropped its sender"),
                "the second carrier ran",
            );
            assert!(Baton::global().held_here(), "the parked carrier did not take the baton back");
        });
        assert!(!Baton::global().held(), "a carrier kept the baton");
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
                    as_carrier(|| {
                        park_on(async {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        });
                        n
                    })
                })
            })
            .collect();
        let mut answers: Vec<u8> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
        answers.sort_unstable();
        assert_eq!(answers, [0, 1, 2, 3]);
        assert!(carriers() >= 4, "four jobs in flight shared fewer than four carriers");
        assert!(!Baton::global().held(), "a carrier kept the baton");
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

    #[test]
    fn a_task_runs_as_a_carrier_and_gives_the_baton_back() {
        let _alone = alone();
        let held = std::sync::Arc::new(AtomicUsize::new(0));
        let seen = std::sync::Arc::clone(&held);
        let handle = task_start(move || {
            seen.store(usize::from(Baton::global().held_here()), Ordering::SeqCst);
        });
        assert!(task_join(handle));
        assert_eq!(held.load(Ordering::SeqCst), 1, "a task ran without the baton");
        assert!(!Baton::global().held(), "the task kept the baton");
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
    /// than in 200: the sleep is [`park_on`]'s, so a carrier gives the run
    /// baton up for the length of it and the other carrier's step gets in.
    /// Sequentially this is 200 ms and the bound below fails, which is what
    /// makes it a test of the scheduler rather than of the clock.
    ///
    /// The upper bound is generous — 100 ms of real waiting plus the whole of a
    /// loaded machine's scheduling — because the failure it guards against is
    /// *doubling*, and a bound tight enough to catch a millisecond of jitter
    /// would fail under `cargo test`'s own parallelism instead.
    #[test]
    fn two_tasks_that_wait_overlap() {
        // The fan-out draws on the carrier pool and hands the process
        // baton around, both of which every other case that reads them
        // takes this lock for.
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
        // The fan-out draws on the carrier pool and hands the process
        // baton around, both of which every other case that reads them
        // takes this lock for.
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

    /// **The baton still admits one carrier at a time, through the fan-out.**
    ///
    /// `the_baton_admits_one_carrier_at_a_time` stresses [`Baton`] directly;
    /// this one stresses the thing that hands it out. The step is the same
    /// read-modify-write split in three, so a second step running beside this
    /// one is a wrong *number* rather than undefined behaviour, and the case
    /// counts overlaps outright as well.
    #[test]
    fn the_baton_admits_one_step_at_a_time() {
        // The fan-out draws on the carrier pool and hands the process
        // baton around, both of which every other case that reads them
        // takes this lock for.
        let _alone = alone();
        struct Shared {
            counter: AtomicU64,
            inside: AtomicUsize,
            overlaps: AtomicUsize,
        }
        const STEPS: usize = 200;
        unsafe extern "C" fn bump(state: *mut u8, index: u64, _: *const u8, out: *mut u8) {
            // SAFETY: the test hands a live `Shared` and an `i64` slot.
            unsafe {
                let shared = &*state.cast::<Shared>();
                if shared.inside.fetch_add(1, Ordering::SeqCst) != 0 {
                    shared.overlaps.fetch_add(1, Ordering::SeqCst);
                }
                increment_slowly(&shared.counter);
                shared.inside.fetch_sub(1, Ordering::SeqCst);
                out.cast::<i64>().write(index as i64);
            }
        }
        let shared = Shared {
            counter: AtomicU64::new(0),
            inside: AtomicUsize::new(0),
            overlaps: AtomicUsize::new(0),
        };
        let src = vec![0i64; STEPS];
        // SAFETY: `STEPS` `i64`s in and out, and `shared` outlives the call.
        let got = unsafe {
            steps_of(
                src.as_ptr().cast(),
                STEPS,
                bump,
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
        assert_eq!(answers, (0..STEPS as i64).collect::<Vec<i64>>());
        assert_eq!(
            shared.overlaps.load(Ordering::SeqCst),
            0,
            "two steps were in Buri code at once"
        );
        assert_eq!(
            shared.counter.load(Ordering::SeqCst),
            STEPS as u64,
            "a lost update: the steps were not excluding one another"
        );
    }

    /// More items than may be in flight: the window slides, and every step
    /// still runs exactly once into its own slot.
    ///
    /// [`IN_FLIGHT`] bounds threads and not the answer, and this is where that
    /// is said out loud — the list is deliberately longer than it.
    #[test]
    fn a_list_longer_than_the_window_still_answers_every_item() {
        // The fan-out draws on the carrier pool and hands the process
        // baton around, both of which every other case that reads them
        // takes this lock for.
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

    /// A step that is itself a fan-out, from a carrier that is holding the
    /// baton.
    ///
    /// The nested call has to give the baton up before it dispatches — a
    /// carrier's body begins by taking it — so this is the case that would
    /// deadlock if it did not. `Handoff::join` has no timeout, so a failure
    /// here is the suite stopping rather than a message.
    #[test]
    fn a_nested_fan_out_gives_the_baton_up_first() {
        // The fan-out draws on the carrier pool and hands the process
        // baton around, both of which every other case that reads them
        // takes this lock for.
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
        assert!(!Baton::global().held(), "the nested call kept the baton");
    }

    /// **The artifact decides**, and its silence is the sequential answer.
    ///
    /// The only case that touches `lib.rs`'s global, which is why it puts it
    /// back. The observable difference is the thread a step runs on: where an
    /// artifact has not said its carriers have frames of their own, every step
    /// runs on the caller's.
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
        assert!(!crate::frames_are_per_carrier(), "the silent answer is the safe one");
        let me = thread::current().id();
        assert!(
            ran_on().iter().all(|id| *id == me),
            "a shared Buri stack ran a step somewhere other than the calling carrier"
        );

        crate::buri_rt_frames_are_per_carrier();
        assert!(crate::frames_are_per_carrier(), "the artifact's statement did not stick");
        let fanned = ran_on();
        crate::forget_frames_are_per_carrier();
        assert!(
            fanned.iter().any(|id| *id != me),
            "the steps stayed on the calling carrier: {fanned:?}"
        );
    }
}
