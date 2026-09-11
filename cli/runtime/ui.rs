//! The reactive graph, the themes, and the headless `Ui` platform.
//!
//! A port of three sections of `backend/js/runtime.js` — "The reactive graph",
//! "Themes" and "The headless user-interface platform" — so that a native
//! program's signals behave exactly as a JavaScript one's do. **The DOM shim
//! did not come with them.** Nothing here renders anything: `$tree_render` is
//! still JavaScript, and a native snapshot is painted from a scene document
//! rather than from a document.
//!
//! ## The graph
//!
//! One `Vec` of nodes, indexed by the `Int` a Buri `Signal<T>` carries. Four
//! kinds:
//!
//! ```text
//!   cell      a value, written from outside
//!   memo      a value, computed from other nodes, lazily
//!   watcher   run for its effect on the world, eagerly
//!   owner     runs nothing; exists so that something else can be disposed
//!             with it
//! ```
//!
//! [`Graph::tracking`] is what a read right now subscribes and
//! [`Graph::current`] is what a node created right now belongs to. A run drops
//! its edges before it calls its body, so what a body reads *this* time is
//! exactly what it is subscribed to — a read behind an `if` is tracked
//! exactly. A memo is lazy: it recomputes when something reads it. A watcher is
//! not: it goes on the queue and the queue drains at the end of the batch.
//!
//! ## A write inside a drain joins the pass
//!
//! `$ui_write` drains whenever no batch is open, and during a drain no batch
//! is. So a watcher that writes what it read starts a *second* drain on
//! JavaScript, and a runaway there is a stack overflow rather than the message
//! the budget exists to print. Here the drain is not re-entrant: a write while
//! one is running only queues, and the index walk already in progress picks the
//! work up. Every program that settles settles the same way; a runaway meets
//! [`STEPS`] and stops with a sentence.
//!
//! ## What a node holds
//!
//! `Vec<u8>` at a stride the caller names, and every entry that reads or writes
//! one takes that stride. The bytes are opaque: what they mean, and any
//! reference count inside them, belongs to the caller.
//!
//! **Which is why a write asks the caller whether two values are the same.**
//! `==` is structural (SPEC 7.2), so two strings with the same text are one
//! value wherever they live — and a cell holds a `Str` as a pointer, which
//! comparing bytes reads as two different values. So a write carries [`Equal`],
//! the type's own comparison generated where the type is known, beside the
//! retain and the release. Bytes are the fallback and the whole answer for a
//! scalar, which is the only shape they were ever right for.
//!
//! ## The flattened theme document
//!
//! A `Theme` holds closures and a closure cannot cross as data, so the caller
//! resolves the switch conditions and hands over the bindings as text. UTF-8,
//! line-oriented, one theme per `theme` line:
//!
//! ```text
//! buri-theme 1
//! theme
//! bind cardlib-surface token app-bg
//! bind cardlib-accent value rgb(29,78,216)
//! theme
//! bind app-bg value rgb(255,255,255)
//! ```
//!
//! * The first line is exactly `buri-theme 1`. A document that opens any other
//!   way resolves to nothing.
//! * `theme` opens a theme. Every `bind` after it belongs to that theme, in
//!   order.
//! * `bind <name> token <name>` is a value that is itself a token;
//!   `bind <name> value <text>` is one that is not. `<name>` is
//!   `namespace-name`, the custom property without its dashes, and `<text>` is
//!   the rest of the line — `rgb(1,2,3)`, `transparent`, whatever the caller
//!   rendered.
//! * Any other line is skipped.
//!
//! Resolution is `runtime.js`'s, unchanged: every binding of every theme in one
//! map keyed by name, a later one replacing an earlier; each value followed
//! while it is itself a token, with a step budget of the map's size; a chain
//! that leaves the map or closes on itself left out rather than guessed at; and
//! the answer one `:root{...}` block per theme in the order they were passed.

use crate::abort::die;
use crate::list::{Release, Retain};
use crate::memory::{buri_rt_stack_acquire, buri_rt_stack_release};
use crate::value::{list_of_strs, str_of, BuriList, BuriStr};
use std::sync::Mutex;

/// The per-value **equality** glue: answers whether two values of one type are
/// the same value, by the comparison `==` makes at that type. Null where the
/// backend generated none, and then two values are the same value when their
/// bytes are.
///
/// Four arguments, and the first is the one that is not obvious. The body it
/// reaches is *Buri code* — `middle::derives`'s generated `Equal`, called
/// through a thunk the backend emitted — so the frame-threaded backend needs a
/// Buri frame to run it in, exactly as a deferred body does
/// ([`Compute`]). The LLVM backend works on the machine stack and ignores the
/// word. `a` and `b` address one whole value each and `out` receives a byte:
/// non-zero for equal.
pub type Equal = Option<
    unsafe extern "C" fn(frame: *mut u8, a: *const u8, b: *const u8, out: *mut u8),
>;

/// A runaway is a program whose watchers write what they read. The limit is not
/// a policy, it is the difference between a diagnosis and a hung tab.
const STEPS: usize = 100_000;

/// `$ui_at`'s message, word for word, so a program that names a signal that was
/// never made says the same thing on both backends.
const NO_SIGNAL: &str = "this signal does not exist";

/// `$ui_drain`'s message, likewise.
const RUNAWAY: &str = "a reactive update did not settle";

/// The generated C-ABI thunk a memo's or a watcher's body is reached through.
///
/// **The same four words `list.rs`'s `StepEntry` declares**, and deliberately
/// so: a reactive body is `fn(Scope) => T`, which is a step of one element
/// whose element is the scope. `state` is the backend's own record, `index` is
/// the loop counter a step gets and a body ignores, `arg` points at the scope
/// the body is being run under, and `out` is where a memo's answer goes, at
/// the stride the memo was made with. One thunk shape means one thunk
/// generator per backend rather than two.
pub type ComputeEntry =
    unsafe extern "C" fn(state: *mut u8, index: i64, arg: *const u8, out: *mut u8);

/// A body, as the graph holds it.
///
/// # A deferred call needs a frame of its own
///
/// A step runs *during* the call that handed it over, so the record and the
/// working frame both sit past the caller's own and are gone when it returns.
/// A memo runs on the first read and a watcher on every change — long after —
/// so neither may point at a frame that has been left. Two things follow, and
/// they are the whole of what makes a deferred body work:
///
/// * the record is **copied** into a block of this crate's own, which lives as
///   long as the node does — which is the life of the program, because a cell
///   is never disposed;
/// * the working frame is **this crate's**, acquired per run from
///   [`crate::memory::buri_rt_stack_acquire`] and given back at the end of it.
///   `frame_at` is where in the record the backend wants that address written,
///   or `-1` for a backend whose thunk needs no such word. The frame-threaded
///   backend names its own `E_FRAME`; the LLVM one uses the machine stack and
///   names nothing.
#[derive(Clone, Copy)]
struct Compute {
    entry: ComputeEntry,
    state: *mut u8,
    frame_at: i64,
}

// SAFETY: `state` is the backend's record for one computation, handed back
// untouched and never read here. The graph is one per process and a Buri
// program drives it from one carrier, so the pointer is only ever called on the
// thread that installed it; the `Send` is what a `static Mutex` asks for and
// not a promise that two threads may run the same body.
unsafe impl Send for Compute {}

/// What a node is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cell,
    Memo,
    Watcher,
    Owner,
}

/// One node. `deps` and `subs` are the same edges from the two ends, and both
/// are `Vec`s because a computation reads a handful of cells and a linear scan
/// over three elements beats a hash.
struct Node {
    kind: Kind,
    value: Vec<u8>,
    stride: usize,
    compute: Option<Compute>,
    /// The release glue for the value this node holds, or `None`. A cell holds
    /// what it was written and a memo holds what it computed, so the write and
    /// the next run are where the reference goes back — and [`give_back`] is
    /// where the last one does.
    release: Release,
    /// The release glue for the **body**'s record, or `None`. The record's
    /// first two words are the closure `{ code, env }`, so this is the walk
    /// generated for the closure's own type: what takes back the reference
    /// [`buri_rt_ui_memo`]'s caller gave the graph.
    body: Release,
    deps: Vec<i64>,
    subs: Vec<i64>,
    dirty: bool,
    queued: bool,
    disposed: bool,
    children: Vec<i64>,
}

/// The whole graph — one per process, as `$ui` is one per artifact.
struct Graph {
    nodes: Vec<Node>,
    /// What a node created right now belongs to, or `-1`.
    current: i64,
    /// What a read right now subscribes, or `-1` for a read nobody is
    /// listening to. Separate from `current` because building a keyed list's
    /// row is two questions at once: the row belongs to the list, and what the
    /// row read is nobody's dependency.
    tracking: i64,
    queue: Vec<i64>,
    /// Open batches. A write inside one defers the drain, so N writes cause one
    /// pass rather than N.
    depth: i64,
    /// Whether a pass is running. A write during one joins it.
    draining: bool,
}

impl Graph {
    const fn new() -> Graph {
        Graph {
            nodes: Vec::new(),
            current: -1,
            tracking: -1,
            queue: Vec::new(),
            depth: 0,
            draining: false,
        }
    }

    fn get(&self, id: i64) -> Option<&Node> {
        usize::try_from(id).ok().and_then(|i| self.nodes.get(i))
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut Node> {
        usize::try_from(id).ok().and_then(|i| self.nodes.get_mut(i))
    }

    /// A fresh node, owned by whatever is running.
    fn make(&mut self, kind: Kind, value: Vec<u8>, stride: usize, compute: Option<Compute>) -> i64 {
        self.make_releasing(kind, value, stride, compute, None, None)
    }

    /// [`Graph::make`], for a node whose value or body the graph has to give
    /// back.
    fn make_releasing(
        &mut self,
        kind: Kind,
        value: Vec<u8>,
        stride: usize,
        compute: Option<Compute>,
        release: Release,
        body: Release,
    ) -> i64 {
        let owner = self.current;
        self.nodes.push(Node {
            kind,
            value,
            stride,
            compute,
            release,
            body,
            deps: Vec::new(),
            subs: Vec::new(),
            // A memo has never run, so it is out of date by construction.
            dirty: kind == Kind::Memo,
            queued: false,
            disposed: false,
            children: Vec::new(),
        });
        let id = (self.nodes.len() as i64) - 1;
        // Disposal is keyed on which computation was executing when the node
        // was created, so a nested computation dies with the run that made it.
        if let Some(o) = self.get_mut(owner) {
            o.children.push(id);
        }
        id
    }

    /// Drops every edge `id` reads through, from both ends.
    fn unsubscribe(&mut self, id: i64) {
        let deps = match self.get_mut(id) {
            Some(n) => std::mem::take(&mut n.deps),
            None => return,
        };
        for d in deps {
            if let Some(source) = self.get_mut(d) {
                source.subs.retain(|s| *s != id);
            }
        }
    }

    /// Disposes `id` and everything hanging off it.
    fn dispose(&mut self, id: i64) {
        let mut stack = vec![id];
        while let Some(next) = stack.pop() {
            let children = {
                let Some(n) = self.get_mut(next) else { continue };
                if n.disposed {
                    continue;
                }
                n.disposed = true;
                n.compute = None;
                std::mem::take(&mut n.children)
            };
            stack.extend(children);
            self.unsubscribe(next);
            if let Some(n) = self.get_mut(next) {
                n.subs.clear();
            }
        }
    }

    /// Marks dependents out of date, transitively. A memo is only marked — it
    /// recomputes when read — while a watcher is queued, since nothing will
    /// ever read it.
    fn notify(&mut self, id: i64) {
        let subs = match self.get(id) {
            Some(n) => n.subs.clone(),
            None => return,
        };
        for s in subs {
            let mark = {
                let Some(c) = self.get_mut(s) else { continue };
                if c.disposed {
                    continue;
                }
                match c.kind {
                    Kind::Memo if !c.dirty => {
                        c.dirty = true;
                        Mark::Deeper
                    }
                    Kind::Watcher if !c.queued => {
                        c.queued = true;
                        Mark::Queue
                    }
                    _ => Mark::Nothing,
                }
            };
            match mark {
                Mark::Deeper => self.notify(s),
                Mark::Queue => self.queue.push(s),
                Mark::Nothing => {}
            }
        }
    }
}

/// What [`Graph::notify`] does with one subscriber.
enum Mark {
    Deeper,
    Queue,
    Nothing,
}

static GRAPH: Mutex<Graph> = Mutex::new(Graph::new());

/// The custom-property block installed right now, without the `<style>` element
/// around it. Off a browser this is all there is, which is what `ui/testing`
/// reads.
static THEME: Mutex<String> = Mutex::new(String::new());

/// Lock, recovering from poisoning, for the reason `testing.rs`'s `lock` gives:
/// the language has no threads, so a poisoned lock means this runtime already
/// panicked and failing a second time on top of the first helps nobody.
fn lock() -> std::sync::MutexGuard<'static, Graph> {
    match GRAPH.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn theme_lock() -> std::sync::MutexGuard<'static, String> {
    match THEME.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// ---------------------------------------------------------------------------
// Running, reading, writing
// ---------------------------------------------------------------------------

/// One run of a computation: drop what the last run made, re-collect the
/// dependencies, call the body.
///
/// The lock is released for the call, because the body is Buri code and reads
/// and writes the graph on its way through.
fn run(id: i64) {
    let (compute, stride, outer_current, outer_tracking) = {
        let mut g = lock();
        let Some(n) = g.get(id) else { return };
        if n.disposed {
            return;
        }
        let Some(compute) = n.compute else { return };
        let stride = n.stride;
        // Everything the previous run created belongs to the previous run.
        let children = match g.get_mut(id) {
            Some(n) => std::mem::take(&mut n.children),
            None => return,
        };
        for c in children {
            g.dispose(c);
        }
        // Per-run dependency re-collection: the edges go before the body runs,
        // so what it reads this time is exactly what it is subscribed to.
        g.unsubscribe(id);
        let saved = (g.current, g.tracking);
        g.current = id;
        g.tracking = id;
        (compute, stride, saved.0, saved.1)
    };
    // A watcher answers `()` and writes nothing, but a destination it could
    // never write to would be a null pointer in a generated thunk's hands.
    let mut out = vec![0u8; stride.max(8)];
    // The scope the body is run under is the node itself, and it crosses as
    // the step's element: a pointer to the one word a Buri `Scope` carries.
    let scope = id;
    // The frame this body works in, and the whole reason a deferred call needs
    // one: the frame it was handed over in has been left.
    let frame = if compute.frame_at >= 0 { buri_rt_stack_acquire() } else { std::ptr::null_mut() };
    if let Ok(at) = usize::try_from(compute.frame_at) {
        // SAFETY: the backend asked for the frame at this offset in a record
        // of its own, and `keep` copied the whole of it.
        unsafe { compute.state.add(at).cast::<*mut u8>().write(frame) };
    }
    // SAFETY: `entry` is the thunk the backend generated for this computation
    // and `state` the copy of the record it was generated against; `out` is a
    // live buffer of at least the stride the memo was made with, and `scope`
    // one live word.
    unsafe {
        (compute.entry)(compute.state, id, std::ptr::addr_of!(scope).cast(), out.as_mut_ptr());
    }
    if !frame.is_null() {
        // SAFETY: this thread acquired it a few lines above and the thunk has
        // returned, so nothing is inside it.
        unsafe { buri_rt_stack_release(frame) };
    }
    let mut g = lock();
    g.current = outer_current;
    g.tracking = outer_tracking;
    let mut spent = Vec::new();
    if let Some(n) = g.get_mut(id) {
        if n.kind == Kind::Memo {
            out.truncate(stride);
            spent = std::mem::replace(&mut n.value, out);
        }
        n.dirty = false;
    }
    let glue = g.get(id).and_then(|n| n.release);
    drop(g);
    if !spent.is_empty() {
        // SAFETY: `spent` is the copy of a whole value of the memo's type that
        // the node held until the line above, and the graph no longer names it.
        unsafe { walk(glue, spent.as_mut_ptr()) };
    }
}

/// The queue, in the order it was scheduled. `false` when the step budget ran
/// out, which is the caller's cue to stop the program.
///
/// Index-walking rather than draining a snapshot: a watcher may schedule
/// another, and the one it schedules belongs to this pass.
fn drain() -> bool {
    {
        let mut g = lock();
        if g.draining {
            return true;
        }
        g.draining = true;
    }
    let mut steps = 0usize;
    let mut at = 0usize;
    let settled = loop {
        let next = {
            let g = lock();
            g.queue.get(at).copied()
        };
        let Some(id) = next else { break true };
        steps = steps.saturating_add(1);
        if steps > STEPS {
            break false;
        }
        {
            let mut g = lock();
            if let Some(n) = g.get_mut(id) {
                n.queued = false;
            }
        }
        run(id);
        at = at.saturating_add(1);
    };
    let mut g = lock();
    g.draining = false;
    if settled {
        g.queue.clear();
    }
    settled
}

/// Drains, and stops the program where it will not settle.
fn settle() {
    if !drain() {
        die(&[RUNAWAY.as_bytes()]);
    }
}

/// Whether a write should drain right now: no batch is open and no pass is
/// already walking the queue.
fn should_drain() -> bool {
    let g = lock();
    g.depth == 0 && !g.draining
}

/// `$ui_read` — the value, and the edge the read makes.
///
/// # Safety
/// `out` is writable for `stride` bytes, or null with a zero stride.
unsafe fn read_into(id: i64, stride: usize, out: *mut u8) {
    let stale = {
        let g = lock();
        let Some(n) = g.get(id) else { die(&[NO_SIGNAL.as_bytes()]) };
        // Reading is what makes a memo run: until then it has computed nothing,
        // and a memo nothing reads never runs at all.
        n.kind == Kind::Memo && n.dirty && !n.disposed
    };
    if stale {
        run(id);
    }
    let mut g = lock();
    let reader = g.tracking;
    if reader >= 0 && reader != id {
        if let Some(n) = g.get_mut(id) {
            if !n.subs.contains(&reader) {
                n.subs.push(reader);
            }
        }
        if let Some(r) = g.get_mut(reader) {
            if !r.deps.contains(&id) {
                r.deps.push(id);
            }
        }
    }
    let Some(n) = g.get(id) else { return };
    let count = n.value.len().min(stride);
    if count == 0 || out.is_null() {
        return;
    }
    // SAFETY: the caller promises `stride` writable bytes, and `count` is at
    // most that.
    unsafe { std::ptr::copy_nonoverlapping(n.value.as_ptr(), out, count) };
}

// ---------------------------------------------------------------------------
// The C ABI
// ---------------------------------------------------------------------------

/// `Ui.signal(initial)` — a fresh cell holding `stride` bytes, and the `Int` a
/// `Signal<T>` carries.
///
/// # Safety
/// `initial` points at `stride` readable bytes, or is null with a zero stride.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_signal(initial: *const u8, stride: usize) -> i64 {
    let value = if initial.is_null() || stride == 0 {
        Vec::new()
    } else {
        // SAFETY: the caller promises `stride` readable bytes.
        unsafe { std::slice::from_raw_parts(initial, stride) }.to_vec()
    };
    let mut g = lock();
    g.make(Kind::Cell, value, stride, None)
}

/// `Ui.read(id)` — the value, written through `out`, and the read subscribes
/// whatever computation is running.
///
/// A signal that was never made stops the program with `this signal does not
/// exist`.
///
/// # Safety
/// `out` is writable for `stride` bytes, or null with a zero stride.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_read(id: i64, stride: usize, out: *mut u8) {
    // SAFETY: forwarded to the caller's promise.
    unsafe { read_into(id, stride, out) }
}

/// `Ui.write(id, value)` — the new bytes, and the pass they cause.
///
/// Identical bytes are not a change, which is what makes "wrote the same value,
/// so nothing re-ran" a thing a test can assert.
///
/// # Safety
/// `value` points at `stride` readable bytes, or is null with a zero stride.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_write(id: i64, value: *const u8, stride: usize) {
    // SAFETY: forwarded to the caller's promise.
    unsafe { write_changed(id, value, stride) };
}

/// [`buri_rt_ui_write`], answering whether the bytes were new.
///
/// The answer is what the generic entry below needs and this one does not: a
/// value the graph did not store is a value the graph must not take a
/// reference on.
///
/// # Safety
/// As [`buri_rt_ui_write`].
unsafe fn write_changed(id: i64, value: *const u8, stride: usize) -> bool {
    let fresh = if value.is_null() || stride == 0 {
        Vec::new()
    } else {
        // SAFETY: the caller promises `stride` readable bytes.
        unsafe { std::slice::from_raw_parts(value, stride) }.to_vec()
    };
    {
        let mut g = lock();
        let Some(n) = g.get_mut(id) else { die(&[NO_SIGNAL.as_bytes()]) };
        if n.value == fresh {
            return false;
        }
        n.value = fresh;
        g.notify(id);
    }
    if should_drain() {
        settle();
    }
    true
}

/// `Ui.memo(compute)` — a lazy node, out of date by construction.
///
/// It runs on the first read and not before, and `stride` is how many bytes its
/// body writes through the thunk's `out`.
///
/// # Safety
/// `entry` is the thunk the backend generated for this body and `state` the
/// record it was generated against.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_memo(
    entry: ComputeEntry,
    state: *const u8,
    bytes: usize,
    frame_at: i64,
    stride: usize,
    release: Release,
    body: Release,
) -> i64 {
    // SAFETY: forwarded to the caller's promise.
    let state = unsafe { keep(state, bytes) };
    let mut g = lock();
    g.make_releasing(
        Kind::Memo,
        Vec::new(),
        stride,
        Some(Compute { entry, state, frame_at }),
        release,
        body,
    )
}

/// The backend's record, in a block of this crate's own that outlives the call
/// that handed it over.
///
/// Leaked on purpose: a memo and a watcher live for the life of the program
/// (`design/native/DECISIONS.md`, "a scope stays open for the life of the
/// program"), so the block that holds one's body has exactly that lifetime and
/// the process reclaims it.
///
/// # Safety
/// `state` points at `bytes` readable bytes, or is null with a zero count.
unsafe fn keep(state: *const u8, bytes: usize) -> *mut u8 {
    give_back_at_exit();
    if state.is_null() || bytes == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller promises `bytes` readable bytes.
    let copy = unsafe { std::slice::from_raw_parts(state, bytes) }.to_vec();
    Box::leak(copy.into_boxed_slice()).as_mut_ptr()
}

/// `Ui.watch(run)` — a node that runs for its effect, now and on every change.
///
/// Eager, and that is not an optimization: a watcher learns what it depends on
/// by running, so one that has never run is subscribed to nothing and would
/// never run again.
///
/// # Safety
/// As [`buri_rt_ui_memo`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_watch(
    entry: ComputeEntry,
    state: *const u8,
    bytes: usize,
    frame_at: i64,
    body: Release,
) {
    // SAFETY: forwarded to the caller's promise.
    let state = unsafe { keep(state, bytes) };
    let id = {
        let mut g = lock();
        g.make_releasing(
            Kind::Watcher,
            Vec::new(),
            0,
            Some(Compute { entry, state, frame_at }),
            None,
            body,
        )
    };
    run(id);
}

/// `Scope.read(id)` — the same read, reached through the `Scope` a reactive
/// closure was handed.
///
/// `scope` names the computation the body belongs to and is not what the edge
/// is drawn from: the graph's own tracking pointer is, exactly as
/// `$ui_effect_Scope_read` forwards to `$ui_read`. That is what makes a read
/// inside a keyed list's row the list's dependency and not the row's.
///
/// # Safety
/// As [`buri_rt_ui_read`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_scope_read(_scope: i64, id: i64, stride: usize, out: *mut u8) {
    // SAFETY: forwarded to the caller's promise.
    unsafe { read_into(id, stride, out) }
}

/// Opens a batch. Writes inside one defer the pass.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_flush_begin() {
    let mut g = lock();
    g.depth = g.depth.saturating_add(1);
}

/// Closes a batch, and runs the one pass N writes earned.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_flush_end() {
    let closed = {
        let mut g = lock();
        if g.depth > 0 {
            g.depth -= 1;
        }
        g.depth == 0
    };
    if closed {
        settle();
    }
}

// ---------------------------------------------------------------------------
// The renderer's closure trampolines (issue #53, phase 1)
// ---------------------------------------------------------------------------
//
// Three shapes the native renderer will drive, added ahead of the renderer so
// the maintainer can confirm against running code that invoking each is the
// SPEC 10.6 callback-with-context the language already blesses, not a new rule.
// None is reached from a Buri walk yet — the builder intrinsics and the
// reconciler that call them are a later phase — exactly as `rows` below is here
// before `$tree_each` is:
//
//   * `build:   fn(Scope) => Node`      a `computed`'s body, run under a scope.
//   * `rowAt:   fn(C, Scope, Int) => Node`  an `each`'s row, at a supplied index.
//   * `onPress: fn(C, Event) => ()`     a button's handler, fired with an event.
//
// Each is the very thunk [`ComputeEntry`] already spells — `(state, index, arg,
// out)`. `build` is [`buri_rt_ui_memo`]'s `fn(Scope) => T` with `T` a `Node`
// stride; `rowAt` is [`crate::list`]'s step with the loop index as the `Int`,
// the scope as the element, and the context dropped as a step already drops it;
// `onPress` is that step once more with the event as the element and nothing
// written back. No new thunk shape, no new argument the boundary cannot already
// carry — which is the whole of the "no SPEC change" claim, in code.

/// Mints a scope, runs `body` under it as [`run`] runs a reactive body, and
/// restores the cursors.
///
/// The scope is a [`Kind::Owner`] node so a later phase can hang a rebuilt
/// subtree's watchers off it; phase 1 needs only that it is a live id a read
/// inside the body subscribes through [`buri_rt_ui_scope_read`], the way a
/// `computed`'s reads become its dependencies.
fn under_fresh_scope<R>(body: impl FnOnce(i64) -> R) -> R {
    let (scope, outer) = {
        let mut g = lock();
        let scope = g.make(Kind::Owner, Vec::new(), 0, None);
        let outer = (g.current, g.tracking);
        g.current = scope;
        g.tracking = scope;
        (scope, outer)
    };
    let answer = body(scope);
    let mut g = lock();
    g.current = outer.0;
    g.tracking = outer.1;
    answer
}

/// `build(scope)` — a `computed`'s body, invoked once under a fresh scope.
///
/// The scope crosses as the thunk's element, a pointer to the one word a
/// `Scope` carries, exactly as [`run`] hands one to a memo's body; the `Node`
/// the body answers is written through `out` at the stride it was compiled for.
/// The runtime keeps nothing yet — a reactive rebuild is a later phase — so this
/// is the step half of a deferred body, invoked in place.
///
/// # Safety
/// `entry` is the thunk the backend generated for this body and `state` the
/// record it was generated against; `out` is writable for the body's `Node`
/// stride.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_build_node(
    entry: ComputeEntry,
    state: *mut u8,
    out: *mut u8,
) {
    under_fresh_scope(|scope| {
        // SAFETY: forwarded to the caller's promise; `scope` is one live word.
        unsafe { (entry)(state, 0, std::ptr::addr_of!(scope).cast(), out) };
    });
}

/// `rowAt(ctx, scope, at)` — an `each`'s row body, driven with the supplied
/// index and a fresh scope.
///
/// The context is dropped at the boundary as a step drops it: it allocates
/// through `buri_rt_alloc` and reads no capability, so it crosses nothing and
/// the thunk never names it. `at` is the loop index a step already carries, the
/// scope is its element, and the `Node` comes back through `out` at its stride.
///
/// # Safety
/// As [`buri_rt_ui_build_node`], with `at` any index the caller chose.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_row_at(
    entry: ComputeEntry,
    state: *mut u8,
    at: i64,
    out: *mut u8,
) {
    under_fresh_scope(|scope| {
        // SAFETY: forwarded to the caller's promise; `scope` is one live word.
        unsafe { (entry)(state, at, std::ptr::addr_of!(scope).cast(), out) };
    });
}

/// `onPress(ctx, event)` — a button's handler, fired with a runtime-minted
/// event.
///
/// A handler is not a computation — it writes signals freely — so this sets no
/// tracking cursor: it fires outside every scope, and a signal write inside it
/// drains as any write does. The context is dropped as `rowAt`'s is; the event
/// crosses as the element, a pointer to the one word an [`Event`] carries; and a
/// body that answers `()` still gets a non-null `out` to write through, as
/// [`run`] gives a watcher one.
///
/// # Safety
/// `entry` is the thunk the backend generated for this handler and `state` the
/// record it was generated against.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_fire_press(entry: ComputeEntry, state: *mut u8, event: i64) {
    let mut sink = [0u8; 8];
    // SAFETY: forwarded to the caller's promise; `event` is one live word and
    // `sink` a live destination a `()`-answering thunk writes nothing to.
    unsafe { (entry)(state, 0, std::ptr::addr_of!(event).cast(), sink.as_mut_ptr()) };
}

/// `Event(0)` — the one event the runtime mints, matching the JavaScript
/// renderer's `[0]`.
///
/// `Event`'s field is private so only the runtime may construct one; its native
/// shape is the one `Int` it wraps, and a press carries no data yet, so the
/// field is zero.
///
/// # Safety
/// `out` is writable and aligned for eight bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_event(out: *mut i64) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(0) };
}

/// `renderInto(builder, node)` — the walk that builds the document, driven once.
///
/// A fourth closure shape the native renderer crosses, beside the three above,
/// and the one `render` itself is: a `fn(Builder, Node) => ()` the runtime
/// invokes to walk a whole tree into the document `cli/runtime/document.rs`
/// holds. It is the [`ComputeEntry`] thunk once more — the builder handle is the
/// `index` a step already carries (it is `Ui.signal`'s `Int`, not a loop
/// counter, but the word is the same word), the node crosses as the element (a
/// pointer to the one field a `Node` wraps, which the thunk destructures and
/// this side never reads), and a body that answers `()` writes nothing through
/// a live `sink`.
///
/// Unlike a build or a row this sets no scope: `renderInto` reads a `Prop`
/// through `ui/node`'s own untracked `rootScope`, so the static walk subscribes
/// nothing, exactly as `describe` does. A reactive re-walk under a scope is a
/// later phase; this is the once-through the initial render is.
///
/// # Safety
/// `entry` is the thunk the backend generated for the walk and `state` the
/// record it was generated against; `builder` is a live document handle and
/// `node` points at one whole `Node`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_render_walk(
    entry: ComputeEntry,
    state: *mut u8,
    builder: i64,
    node: *const u8,
) {
    let mut sink = [0u8; 8];
    // SAFETY: forwarded to the caller's promise; `builder` is one live word,
    // `node` one whole `Node`, and `sink` a live destination a `()`-answering
    // thunk writes nothing to.
    unsafe { (entry)(state, builder, node, sink.as_mut_ptr()) };
}

// ---------------------------------------------------------------------------
// Owners, for the keyed list
// ---------------------------------------------------------------------------

/// `$ui_under`, `$ui_forget` and the owner node they work on.
///
/// Their one caller is `$tree_each`, which is still JavaScript, so nothing in
/// this archive reaches them yet. They are here because the graph is one port
/// and half a graph would be a second thing to get right later.
#[allow(dead_code)]
pub(crate) mod rows {
    use super::{lock, Kind};

    /// A node that runs nothing, so that something else can be disposed with
    /// it. A keyed list's rows hang off one, which is what lets a row outlive
    /// the run that decided it belongs.
    pub(crate) fn owner() -> i64 {
        let mut g = lock();
        g.make(Kind::Owner, Vec::new(), 0, None)
    }

    /// Runs `body` with everything it creates belonging to `owner`, and with
    /// what it reads subscribing nothing.
    ///
    /// Both halves are needed together exactly once: a keyed list builds a row
    /// that must outlive the run that decided to build it, and whose reads are
    /// the list's dependencies and not the row's.
    pub(crate) fn under<R>(owner: i64, body: impl FnOnce() -> R) -> R {
        let saved = {
            let mut g = lock();
            let saved = (g.current, g.tracking);
            g.current = owner;
            g.tracking = -1;
            saved
        };
        let answer = body();
        let mut g = lock();
        g.current = saved.0;
        g.tracking = saved.1;
        answer
    }

    /// Drops `id` from its owner's children, so that a list which adds and
    /// removes a row a thousand times holds a thousand disposed nodes for no
    /// longer than it holds the row.
    pub(crate) fn forget(owner: i64, id: i64) {
        let mut g = lock();
        if let Some(n) = g.get_mut(owner) {
            n.children.retain(|c| *c != id);
        }
    }

    /// Disposes a node and everything hanging off it.
    pub(crate) fn dispose(id: i64) {
        let mut g = lock();
        g.dispose(id);
    }
}

// ---------------------------------------------------------------------------
// Themes
// ---------------------------------------------------------------------------

/// One binding's value: another token, or something a browser can read.
#[derive(Clone)]
enum Bound {
    Token(String),
    Value(String),
}

/// One theme's bindings, in declaration order.
struct Block {
    bindings: Vec<(String, Bound)>,
}

/// The document, as blocks. Anything the format does not name is skipped, and a
/// document that does not open with `buri-theme 1` is nothing at all.
fn parse(doc: &str) -> Vec<Block> {
    let mut lines = doc.lines();
    if lines.next() != Some("buri-theme 1") {
        return Vec::new();
    }
    let mut blocks: Vec<Block> = Vec::new();
    for line in lines {
        if line == "theme" {
            blocks.push(Block { bindings: Vec::new() });
            continue;
        }
        let mut parts = line.splitn(4, ' ');
        if parts.next() != Some("bind") {
            continue;
        }
        let (Some(name), Some(kind), Some(rest)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let bound = match kind {
            "token" => Bound::Token(rest.to_owned()),
            "value" => Bound::Value(rest.to_owned()),
            _ => continue,
        };
        if let Some(block) = blocks.last_mut() {
            block.bindings.push((name.to_owned(), bound));
        }
    }
    blocks
}

/// One value, followed while it is a token. The step budget is the number of
/// bindings there are, so a chain that closes on itself stops instead of
/// hanging, and one that leaves the map names nothing rather than a guess.
fn resolve<'a>(bindings: &'a [(String, Bound)], start: &'a Bound) -> Option<String> {
    let mut steps = bindings.len();
    let mut value = start;
    loop {
        match value {
            Bound::Value(text) => return Some(text.clone()),
            Bound::Token(name) => {
                if steps == 0 {
                    return None;
                }
                steps -= 1;
                let next = bindings.iter().find(|(k, _)| k == name)?;
                value = &next.1;
            }
        }
    }
}

/// The whole custom-property text: one `:root` block per theme, in the order
/// they were passed — a theme *is* a block of values, so reading the installed
/// text shows which package each variable came from.
///
/// `crate::snapshot` resolves a snapshot's themes through here too, and it
/// installs nothing: a picture is painted once, so what it needs is the values,
/// not a document to leave them in.
pub(crate) fn render(doc: &str) -> String {
    let blocks = parse(doc);
    // Every binding, in declaration order, a later one for the same token
    // replacing an earlier one. This is what a chain is followed through.
    let mut bindings: Vec<(String, Bound)> = Vec::new();
    for block in &blocks {
        for (name, bound) in &block.bindings {
            match bindings.iter().position(|(k, _)| k == name) {
                Some(at) => {
                    if let Some(slot) = bindings.get_mut(at) {
                        slot.1 = bound.clone();
                    }
                }
                None => bindings.push((name.clone(), bound.clone())),
            }
        }
    }
    let mut out = String::new();
    for block in &blocks {
        let mut body: Vec<String> = Vec::new();
        for (name, bound) in &block.bindings {
            if let Some(value) = resolve(&bindings, bound) {
                body.push(format!("--{name}:{value}"));
            }
        }
        if !body.is_empty() {
            out.push_str(":root{");
            out.push_str(&body.join(";"));
            out.push_str("}\n");
        }
    }
    out
}

/// Installs a theme list the caller has already flattened, and answers the
/// custom-property block it resolved to.
///
/// The header of this file is the document's format. A switching theme is the
/// caller's business: it registers the watcher and installs again, which is why
/// nothing here holds a closure.
///
/// # Safety
/// `doc` points at `len` readable bytes, or is null with a zero length; `out`
/// is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_theme_install(doc: *const u8, len: usize, out: *mut BuriStr) {
    let bytes = if doc.is_null() || len == 0 {
        &[][..]
    } else {
        // SAFETY: the caller promises `len` readable bytes.
        unsafe { std::slice::from_raw_parts(doc, len) }
    };
    let text = render(&String::from_utf8_lossy(bytes));
    let answer = str_of(&text);
    *theme_lock() = text;
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(answer) };
}

/// `ui/theme`'s `rootScope()` — an untracked scope, which is `-1`.
///
/// The walk that flattens a theme list reads a `switching` theme's condition
/// through this, and a read through an untracked scope subscribes nothing.
///
/// # Safety
/// `out` is writable and aligned for eight bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_theme_root_scope(out: *mut i64) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(-1) };
}

/// The block installed right now.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_theme_variables(out: *mut BuriStr) {
    let answer = str_of(theme_lock().as_str());
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(answer) };
}

// ---------------------------------------------------------------------------
// The keys a runtime table names
// ---------------------------------------------------------------------------
//
// The entries above are the graph's own vocabulary. These are the six symbols
// the two runtime tables have rows for, named by §1's rule from the intrinsic
// key rather than from what the operation is called here: `ui_testing.headless`
// is `buri_rt_ui_testing_headless`, and `ui_effect.Scope.read` is
// `buri_rt_ui_effect_scope_read`. Each table asserts that spelling
// (`runtime_table.rs`'s `every_symbol_obeys_the_naming_rule`), so the two
// layers are one rename apart rather than one convention apart.
//
// **The retain glue, and what a cell owes the value in it.** Every generic
// entry here is emitted with §2 rule 4's pair — a stride and the per-element
// retain glue — and both halves are used. A cell holds the caller's bytes
// verbatim, so a `Str`-typed signal parks a 24-byte `BuriStr` naming a heap
// block. The glue is called **twice**: once on the bytes the graph keeps, so
// the block cannot be freed under it, and once on the bytes handed back, so
// the reader owns what it was given.
//
// The reference the graph takes on what it stores is one it **gives back**:
// `write` carries a third word, the per-value release glue of
// `runtime_table.rs`'s `Extra::Owned`, and calls it on the bytes it just
// replaced. A cell holds one value at a time and a write is the moment the old
// one stops being held, so the counts balance over any number of writes and a
// program that writes a `Str` signal in a loop leaks nothing.
//
// `core/list` needed no such word — nothing there holds a value past the call
// — which is why the retain travelled alone until the graph arrived.
//
// **And a fourth word, the equality glue**, for the same reason the first three
// exist: the question "is this the value already there" is one about the *type*
// and this side has none. `middle::derives` generates the comparison and the
// backend wraps it in [`Equal`]'s C shape, so a write of an equal `Str`, list
// or record re-runs nothing on either backend rather than only on the one where
// a value happens to fit in its own bytes.

/// Runs a per-value glue function — the retain or the release — over one
/// value, where there is one to run.
///
/// # Safety
/// `at` addresses a whole value of the type `glue` was generated for.
unsafe fn walk(glue: Retain, at: *mut u8) {
    if let Some(f) = glue
        && !at.is_null()
    {
        // SAFETY: the caller promises `at` is a whole value of that type.
        unsafe { f(at) };
    }
}

/// Two values of one type, compared by the comparison the language makes at
/// that type — [`Equal`], run in a frame of this crate's own.
///
/// The frame is acquired here for the reason [`run`] acquires one: what the
/// glue reaches is Buri code, and on the frame-threaded backend a Buri call
/// works in a frame the caller sets aside. The LLVM backend's thunk ignores the
/// word and uses the machine stack, so one shape serves both.
///
/// # Safety
/// `a` and `b` each address one whole value of the type `same` was generated
/// for.
unsafe fn equal_values(
    same: unsafe extern "C" fn(*mut u8, *const u8, *const u8, *mut u8),
    a: *const u8,
    b: *const u8,
) -> bool {
    let frame = buri_rt_stack_acquire();
    let mut out = [0u8; 8];
    // SAFETY: the caller promises two whole values; `frame` is a live Buri
    // frame and `out` eight writable bytes, of which the glue writes the first.
    unsafe { same(frame, a, b, out.as_mut_ptr()) };
    // SAFETY: this thread acquired it above and the glue has returned.
    unsafe { buri_rt_stack_release(frame) };
    out[0] != 0
}

/// `ui/testing`'s `headless()` — the handle a `Headless` carries.
///
/// The graph is the state, so the number is unused and every call answers the
/// same one. The entry exists because `Headless` is a struct, and a struct
/// comes back through an out-pointer (§2 rule 2).
///
/// # Safety
/// `out` is writable and aligned for eight bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_headless(out: *mut i64) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(0) };
}

/// `ui/node`'s `rootScope()` — an untracked scope, which is `-1`.
///
/// What `describe` reads props under. Nothing is running, so a read through it
/// records no dependency and a snapshot subscribes to nothing.
///
/// # Safety
/// As [`buri_rt_ui_testing_headless`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_root_scope(out: *mut i64) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(-1) };
}

/// `Headless.signal(initial)`.
///
/// # Safety
/// `initial` points at `stride` readable bytes, and `glue` is the retain glue
/// for that type or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_headless_signal(
    _self: i64,
    initial: *const u8,
    stride: usize,
    glue: Retain,
    drop: Release,
    _same: Equal,
) -> i64 {
    give_back_at_exit();
    // SAFETY: forwarded to the caller's promise.
    let id = unsafe { buri_rt_ui_signal(initial, stride) };
    let mut g = lock();
    if let Some(n) = g.get_mut(id) {
        // The cell holds this value until the next write, and the last one it
        // holds goes back at exit, so the glue that gives it back is kept here
        // rather than asked for again at every write.
        n.release = drop;
        let at = n.value.as_mut_ptr();
        // SAFETY: the cell holds one whole value of that type.
        unsafe { walk(glue, at) };
    }
    id
}

/// `Headless.read(id)`.
///
/// # Safety
/// `out` is writable for `stride` bytes, and `glue` is the retain glue for that
/// type or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_headless_read(
    _self: i64,
    id: i64,
    stride: usize,
    glue: Retain,
    out: *mut u8,
) {
    // SAFETY: forwarded to the caller's promise.
    unsafe {
        read_into(id, stride, out);
        walk(glue, out);
    }
}

/// `Headless.write(id, value)` — the new bytes retained, and the old ones
/// released.
///
/// The release is what keeps a cell from growing a reference per write. The
/// old bytes are copied out **before** the write, because the write is what
/// destroys them, and released **after** it, because a watcher the write woke
/// is entitled to see the new value first.
///
/// An equal value is not a change, so a write that stored nothing takes no
/// reference and gives none back.
///
/// **Equal, not identical.** `==` is structural, so two strings with the same
/// text are one value wherever they live — and a cell holds a `Str` as a
/// pointer, which comparing bytes reads as two. [`Equal`] is the type's own
/// comparison, generated where the type is known, and it is asked first. A type
/// the backend generated none for falls back to the bytes, which is the whole
/// of the value for a scalar.
///
/// The old value is what stays when the two are equal: nothing observable
/// separates them, and keeping the one already held means the write takes no
/// reference and the caller's argument is released as it always was.
///
/// # Safety
/// As [`buri_rt_ui_testing_headless_signal`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_headless_write(
    _self: i64,
    id: i64,
    value: *const u8,
    stride: usize,
    glue: Retain,
    drop: Release,
    same: Equal,
) {
    let mut old = {
        let g = lock();
        g.get(id).map(|n| n.value.clone()).unwrap_or_default()
    };
    if let Some(f) = same
        && old.len() == stride
        && stride > 0
        && !value.is_null()
        // SAFETY: `old` is a copy of one whole value of the cell's type and
        // `value` is one the caller promises; `f` was generated for it.
        && unsafe { equal_values(f, old.as_ptr(), value) }
    {
        return;
    }
    // SAFETY: forwarded to the caller's promise.
    let changed = unsafe { write_changed(id, value, stride) };
    if !changed {
        return;
    }
    {
        let mut g = lock();
        if let Some(n) = g.get_mut(id) {
            let at = n.value.as_mut_ptr();
            // SAFETY: the cell holds one whole value of that type.
            unsafe { walk(glue, at) };
        }
    }
    if !old.is_empty() {
        // SAFETY: `old` is the copy of a whole value of that type the cell held
        // until the write above, and the graph no longer names it.
        unsafe { walk(drop, old.as_mut_ptr()) };
    }
}

// ---------------------------------------------------------------------------
// What the graph gives back at exit
// ---------------------------------------------------------------------------

/// The graph holds a value per cell and a body per computation **for the life
/// of the program**, which is the design and not an oversight: a signal is
/// never disposed, so the reference it takes on what it holds is one nothing
/// gives back while the program is running. This is where it does.
///
/// It matters because the heap check is an exit audit over `live_blocks`, and
/// a graph that kept its references would read as a leak of one block per cell
/// and one per body — a real number, growing with the program, that nothing
/// could tell apart from a defect.
///
/// **Registered after the audit, so it runs before it.** `atexit` is
/// last-in-first-out, so [`crate::memory::arm_heap_audit`] is called first,
/// on the way to making the first node.
fn give_back_at_exit() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::memory::arm_heap_audit();
        // SAFETY: `give_back` is an `extern "C" fn()` taking no arguments and
        // returning normally, which is the whole of `atexit`'s contract.
        unsafe { atexit(give_back) };
    });
}

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

/// Every reference the graph is holding, given back.
///
/// **`try_lock`, and nothing if it is taken.** `abort::die` exits through the
/// handler list, and two of its callers are holding this lock when they call
/// it — a read and a write of a signal that does not exist. Blocking there
/// would turn a one-line refusal into a hang, and there is nothing to give
/// back on that path anyway: the audit is already quiet
/// (`memory::quiet_heap_audit`), because a program that stopped on its own
/// terms is holding whatever it was holding.
extern "C" fn give_back() {
    let Ok(mut g) = GRAPH.try_lock() else { return };
    for n in &mut g.nodes {
        let release = n.release;
        if !n.value.is_empty() {
            // SAFETY: the node holds one whole value of the type `release` was
            // generated for, and after this the graph names it no more.
            unsafe { walk(release, n.value.as_mut_ptr()) };
            n.value.clear();
        }
        let Some(compute) = n.compute.take() else { continue };
        // SAFETY: the record's first words are the closure `{ code, env }`,
        // which is what `body` was generated for.
        unsafe { walk(n.body, compute.state) };
    }
}

/// `ui/testing`'s `observer()` — the handle an `Observer` carries.
///
/// [`buri_rt_ui_testing_headless`]'s twin, and the same number: both are
/// windows onto the one graph the runtime holds, so there is nothing per
/// handle to name.
///
/// # Safety
/// As [`buri_rt_ui_testing_headless`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_observer(out: *mut i64) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(0) };
}

/// `Observer.read(id)` — a read from outside every computation.
///
/// The same read [`buri_rt_ui_testing_headless_read`] does, and it subscribes
/// nothing for the same reason it subscribes nothing there: the graph draws an
/// edge from whatever is *running*, and nothing is.
///
/// # Safety
/// As [`buri_rt_ui_testing_headless_read`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_observer_read(
    _self: i64,
    id: i64,
    stride: usize,
    glue: Retain,
    out: *mut u8,
) {
    // SAFETY: forwarded to the caller's promise.
    unsafe {
        read_into(id, stride, out);
        walk(glue, out);
    }
}

/// `Headless.memo(compute)`.
///
/// # Safety
/// `entry` is the thunk the backend generated for this body, `state` points at
/// `bytes` readable bytes of the record it was generated against, `frame_at`
/// is an offset inside that record or negative, and `release` is the release
/// glue for what the body answers or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_headless_memo(
    _self: i64,
    entry: ComputeEntry,
    state: *const u8,
    bytes: usize,
    frame_at: i64,
    stride: usize,
    release: Release,
    body: Release,
) -> i64 {
    // SAFETY: forwarded to the caller's promise.
    unsafe { buri_rt_ui_memo(entry, state, bytes, frame_at, stride, release, body) }
}

/// `Headless.watch(run)`.
///
/// It takes the stride and the release its sibling takes, and uses neither: a
/// watcher answers `()`, so there is nothing to keep and nothing to give back.
/// One shape for both keys, because `Extra::Compute` is one emission rule.
///
/// # Safety
/// As [`buri_rt_ui_testing_headless_memo`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_headless_watch(
    _self: i64,
    entry: ComputeEntry,
    state: *const u8,
    bytes: usize,
    frame_at: i64,
    _stride: usize,
    _release: Release,
    body: Release,
) {
    // SAFETY: forwarded to the caller's promise.
    unsafe { buri_rt_ui_watch(entry, state, bytes, frame_at, body) };
}

// ---------------------------------------------------------------------------
// The recorder
// ---------------------------------------------------------------------------

/// Every recorder a program has made: a tag log and a value log each.
///
/// One table rather than one allocation per handle, for the reason the graph
/// is one table: a `Recorder` is an index nothing else can produce, so a test
/// can only reach the one it made, and `recorder()` answering a fresh index is
/// the whole of the isolation `ui/testing`'s header promises.
static RECORDERS: Mutex<Vec<(Vec<String>, Vec<i64>)>> = Mutex::new(Vec::new());

fn recorders() -> std::sync::MutexGuard<'static, Vec<(Vec<String>, Vec<i64>)>> {
    match RECORDERS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A `[Int]` from a slice of them: one block at an eight-byte stride.
fn list_of_ints(items: &[i64]) -> BuriList {
    if items.is_empty() {
        return BuriList { ptr: std::ptr::null_mut(), len: 0 };
    }
    let ptr = crate::memory::buri_rt_alloc((items.len() * 8) as u64);
    for (i, item) in items.iter().enumerate() {
        // SAFETY: `i * 8` is inside the block just allocated, and the block is
        // 16-aligned so every eight-byte slot in it is aligned.
        unsafe { ptr.add(i * 8).cast::<i64>().write(*item) };
    }
    BuriList { ptr, len: items.len() as u64 }
}

/// `recorder()` — a fresh, empty log.
///
/// # Safety
/// `out` is writable and aligned for eight bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_recorder(out: *mut i64) {
    let mut all = recorders();
    all.push((Vec::new(), Vec::new()));
    let id = (all.len() as i64) - 1;
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(id) };
}

/// `Recorder.record(tag)`.
///
/// # Safety
/// `ptr` and `len` are a readable UTF-8 range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_recorder_record(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // The stored length carries VALUE-MODEL.md §3.1's ASCII flag in bit 63,
    // and every entry that takes a `Str` masks it off itself
    // (`cli/runtime/text.rs`'s header).
    let n = (len & crate::value::BURI_RT_STR_LEN_MASK) as usize;
    let tag = if ptr.is_null() || n == 0 {
        String::new()
    } else {
        // SAFETY: the caller promises `n` readable bytes at `ptr`.
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(ptr, n) }).into_owned()
    };
    let mut all = recorders();
    if let Some(entry) = usize::try_from(handle).ok().and_then(|i| all.get_mut(i)) {
        entry.0.push(tag);
    }
}

/// `Recorder.recorded()`.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_recorder_recorded(handle: i64, out: *mut BuriList) {
    let tags = {
        let all = recorders();
        usize::try_from(handle).ok().and_then(|i| all.get(i)).map(|e| e.0.clone())
    };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(list_of_strs(&tags.unwrap_or_default())) };
}

/// `Recorder.note(value)` — appended, and answered.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_testing_recorder_note(handle: i64, value: i64) -> i64 {
    let mut all = recorders();
    if let Some(entry) = usize::try_from(handle).ok().and_then(|i| all.get_mut(i)) {
        entry.1.push(value);
    }
    value
}

/// `Recorder.noted()`.
///
/// # Safety
/// As [`buri_rt_ui_testing_recorder_recorded`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_recorder_noted(handle: i64, out: *mut BuriList) {
    let values = {
        let all = recorders();
        usize::try_from(handle).ok().and_then(|i| all.get(i)).map(|e| e.1.clone())
    };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(list_of_ints(&values.unwrap_or_default())) };
}

/// `Scope.read(id)`, at the key the tables name.
///
/// # Safety
/// As [`buri_rt_ui_testing_headless_read`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_effect_scope_read(
    _scope: i64,
    id: i64,
    stride: usize,
    glue: Retain,
    out: *mut u8,
) {
    // SAFETY: forwarded to the caller's promise.
    unsafe {
        read_into(id, stride, out);
        walk(glue, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The graph and the installed block are one per process, and `cargo test`
    /// runs these cases on many threads at once. Every case here takes this
    /// first and starts from an empty graph.
    static ONE_GRAPH_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn alone() -> std::sync::MutexGuard<'static, ()> {
        let guard = match ONE_GRAPH_AT_A_TIME.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *lock() = Graph::new();
        theme_lock().clear();
        guard
    }

    /// A body written in Rust, so a case can say what a computation does.
    struct Body(Box<dyn Fn(i64) -> i64>);

    /// The four-word thunk shape the two backends generate, written by hand:
    /// the state is the `Body` box, the index is the step's and unused, and
    /// the argument is the scope.
    unsafe extern "C" fn call(state: *mut u8, _index: i64, arg: *const u8, out: *mut u8) {
        // SAFETY: every case hands a live `Body`, a live scope word and eight
        // writable bytes.
        unsafe {
            let body = &*state.cast::<*const Body>().read();
            let answer = (body.0)(arg.cast::<i64>().read());
            out.cast::<i64>().write(answer);
        }
    }

    /// What each run of a body recorded.
    type Log = Rc<RefCell<Vec<i64>>>;

    fn log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    fn noted(log: &Log) -> Vec<i64> {
        log.borrow().clone()
    }

    fn new_cell(value: i64) -> i64 {
        // SAFETY: `value` is a live, aligned `i64`.
        unsafe { buri_rt_ui_signal((&raw const value).cast(), 8) }
    }

    fn read_cell(id: i64) -> i64 {
        let mut value = 0i64;
        // SAFETY: `value` is a live, aligned `i64`.
        unsafe { buri_rt_ui_read(id, 8, (&raw mut value).cast()) };
        value
    }

    /// The same read, through the `Scope` a reactive closure is handed.
    fn scope_read(scope: i64, id: i64) -> i64 {
        let mut value = 0i64;
        // SAFETY: `value` is a live, aligned `i64`.
        unsafe { buri_rt_ui_scope_read(scope, id, 8, (&raw mut value).cast()) };
        value
    }

    fn write_cell(id: i64, value: i64) {
        // SAFETY: `value` is a live, aligned `i64`.
        unsafe { buri_rt_ui_write(id, (&raw const value).cast(), 8) };
    }

    /// A memo, and the box the graph points at — which the case must hold for
    /// as long as the graph may run it.
    fn memo(body: impl Fn(i64) -> i64 + 'static) -> (Box<Body>, i64) {
        let held = Box::new(Body(Box::new(body)));
        // A record of one word, the way a backend builds one: the graph copies
        // it, so the case's box has to be reachable *through* it rather than
        // be it.
        let record: [*const Body; 1] = [&raw const *held];
        // SAFETY: `call` is the thunk `held` was written for, `record` names
        // it, and `held` is a heap box the case keeps.
        let id = unsafe {
            buri_rt_ui_memo(call, (&raw const record).cast(), 8, -1, 8, None, None)
        };
        (held, id)
    }

    /// A watcher, likewise. It has already run once by the time this answers.
    fn watcher(body: impl Fn(i64) -> i64 + 'static) -> Box<Body> {
        let held = Box::new(Body(Box::new(body)));
        let record: [*const Body; 1] = [&raw const *held];
        // SAFETY: as `memo`.
        unsafe { buri_rt_ui_watch(call, (&raw const record).cast(), 8, -1, None) };
        held
    }

    fn subs_of(id: i64) -> Vec<i64> {
        lock().get(id).map(|n| n.subs.clone()).unwrap_or_default()
    }

    fn children_of(id: i64) -> Vec<i64> {
        lock().get(id).map(|n| n.children.clone()).unwrap_or_default()
    }

    fn is_disposed(id: i64) -> bool {
        lock().get(id).map(|n| n.disposed).unwrap_or(false)
    }

    /// The text an out-pointer entry answered, with its block freed.
    fn taken(answer: BuriStr) -> String {
        // SAFETY: the entry wrote a live `Str` there.
        let text = unsafe { answer.as_str() }.into_owned();
        if !answer.base.is_null() {
            // SAFETY: this is the only reference to the block.
            unsafe { crate::memory::buri_rt_free(answer.base) };
        }
        text
    }

    fn install(doc: &str) -> String {
        let mut answer = BuriStr { base: std::ptr::null_mut(), ptr: std::ptr::null(), len: 0 };
        // SAFETY: `doc` is a live view and `answer` a live local.
        unsafe { buri_rt_ui_theme_install(doc.as_ptr(), doc.len(), &raw mut answer) };
        taken(answer)
    }

    fn variables() -> String {
        let mut answer = BuriStr { base: std::ptr::null_mut(), ptr: std::ptr::null(), len: 0 };
        // SAFETY: `answer` is a live local.
        unsafe { buri_rt_ui_theme_variables(&raw mut answer) };
        taken(answer)
    }

    // --- signals ---------------------------------------------------------

    #[test]
    fn a_signal_reads_back_what_was_written() {
        let _alone = alone();
        let n = new_cell(1);
        assert_eq!(read_cell(n), 1);
        write_cell(n, 7);
        assert_eq!(read_cell(n), 7);
    }

    #[test]
    fn two_signals_are_two_cells() {
        let _alone = alone();
        let a = new_cell(1);
        let b = new_cell(2);
        write_cell(a, 100);
        assert_eq!(read_cell(a), 100);
        assert_eq!(read_cell(b), 2);
    }

    /// A cell holds bytes at whatever stride it was made with, so a `Str` or a
    /// struct is a cell exactly as an `Int` is.
    #[test]
    fn a_cell_holds_the_bytes_it_was_given_at_any_stride() {
        let _alone = alone();
        let initial: [u8; 24] = [7; 24];
        // SAFETY: `initial` covers twenty-four readable bytes.
        let id = unsafe { buri_rt_ui_signal(initial.as_ptr(), 24) };
        let mut got = [0u8; 24];
        // SAFETY: `got` covers twenty-four writable bytes.
        unsafe { buri_rt_ui_read(id, 24, got.as_mut_ptr()) };
        assert_eq!(got, initial);
    }

    #[test]
    fn writing_the_value_already_there_is_not_a_change() {
        let _alone = alone();
        let n = new_cell(3);
        let seen = log();
        let recording = Rc::clone(&seen);
        let _held = watcher(move |scope| {
            let value = scope_read(scope, n);
            recording.borrow_mut().push(value);
            value
        });
        write_cell(n, 3);
        assert_eq!(noted(&seen), vec![3], "the same bytes are the same value");
    }

    // --- watchers --------------------------------------------------------

    #[test]
    fn a_watcher_runs_when_it_is_registered_and_again_on_a_change() {
        let _alone = alone();
        let n = new_cell(3);
        let seen = log();
        let recording = Rc::clone(&seen);
        let _held = watcher(move |scope| {
            let value = scope_read(scope, n);
            recording.borrow_mut().push(value);
            value
        });
        assert_eq!(noted(&seen), vec![3]);
        write_cell(n, 4);
        write_cell(n, 5);
        assert_eq!(noted(&seen), vec![3, 4, 5]);
    }

    #[test]
    fn a_watcher_that_read_nothing_never_runs_again() {
        let _alone = alone();
        let n = new_cell(3);
        let seen = log();
        let recording = Rc::clone(&seen);
        let _held = watcher(move |_| {
            recording.borrow_mut().push(1);
            1
        });
        write_cell(n, 4);
        assert_eq!(noted(&seen), vec![1]);
    }

    #[test]
    fn watchers_run_in_the_order_they_were_registered() {
        let _alone = alone();
        let n = new_cell(0);
        let seen = log();
        let first = Rc::clone(&seen);
        let _one = watcher(move |scope| {
            let _ = scope_read(scope, n);
            first.borrow_mut().push(1);
            1
        });
        let second = Rc::clone(&seen);
        let _two = watcher(move |scope| {
            let _ = scope_read(scope, n);
            second.borrow_mut().push(2);
            2
        });
        assert_eq!(noted(&seen), vec![1, 2]);
        write_cell(n, 1);
        assert_eq!(noted(&seen), vec![1, 2, 1, 2]);
    }

    // --- memos -----------------------------------------------------------

    #[test]
    fn a_memo_nothing_reads_never_runs() {
        let _alone = alone();
        let n = new_cell(2);
        let seen = log();
        let recording = Rc::clone(&seen);
        let (_held, _doubled) = memo(move |scope| {
            let value = scope_read(scope, n) * 2;
            recording.borrow_mut().push(value);
            value
        });
        write_cell(n, 3);
        assert!(noted(&seen).is_empty());
    }

    #[test]
    fn reading_a_memo_twice_in_one_computation_runs_it_once() {
        let _alone = alone();
        let n = new_cell(2);
        let seen = log();
        let recording = Rc::clone(&seen);
        let (_held, doubled) = memo(move |scope| {
            let value = scope_read(scope, n) * 2;
            recording.borrow_mut().push(value);
            value
        });
        let _watching =
            watcher(move |scope| scope_read(scope, doubled) + scope_read(scope, doubled));
        assert_eq!(noted(&seen), vec![4]);
    }

    #[test]
    fn a_memo_recomputes_once_per_change_to_what_it_read() {
        let _alone = alone();
        let n = new_cell(2);
        let seen = log();
        let recording = Rc::clone(&seen);
        let (_held, doubled) = memo(move |scope| {
            let value = scope_read(scope, n) * 2;
            recording.borrow_mut().push(value);
            value
        });
        let _watching = watcher(move |scope| scope_read(scope, doubled));
        assert_eq!(noted(&seen), vec![4]);
        write_cell(n, 5);
        assert_eq!(noted(&seen), vec![4, 10]);
        write_cell(n, 5);
        assert_eq!(noted(&seen), vec![4, 10], "the same bytes are not a change");
    }

    #[test]
    fn a_memo_answers_the_value_it_computed() {
        let _alone = alone();
        let n = new_cell(6);
        let seen = log();
        let (_held, doubled) = memo(move |scope| scope_read(scope, n) * 2);
        let recording = Rc::clone(&seen);
        let _watching = watcher(move |scope| {
            let value = scope_read(scope, doubled);
            recording.borrow_mut().push(value);
            value
        });
        write_cell(n, 7);
        assert_eq!(noted(&seen), vec![12, 14]);
    }

    #[test]
    fn a_memo_of_a_memo_settles_in_one_pass() {
        let _alone = alone();
        let n = new_cell(1);
        let (_first, doubled) = memo(move |scope| scope_read(scope, n) * 2);
        let (_second, quadrupled) = memo(move |scope| scope_read(scope, doubled) * 2);
        let seen = log();
        let recording = Rc::clone(&seen);
        let _watching = watcher(move |scope| {
            let value = scope_read(scope, quadrupled);
            recording.borrow_mut().push(value);
            value
        });
        write_cell(n, 3);
        assert_eq!(noted(&seen), vec![4, 12]);
    }

    // --- exact tracking --------------------------------------------------

    #[test]
    fn a_read_behind_a_condition_is_tracked_exactly() {
        let _alone = alone();
        let use_left = new_cell(1);
        let left = new_cell(1);
        let right = new_cell(100);
        let seen = log();
        let recording = Rc::clone(&seen);
        let _held = watcher(move |scope| {
            let value = if scope_read(scope, use_left) != 0 {
                scope_read(scope, left)
            } else {
                scope_read(scope, right)
            };
            recording.borrow_mut().push(value);
            value
        });
        assert_eq!(noted(&seen), vec![1]);

        // `right` was not read on that run, so writing it is not a reason to
        // run again.
        write_cell(right, 200);
        assert_eq!(noted(&seen), vec![1]);

        // Flipping the condition re-collects: `right` is read now, `left` is
        // not.
        write_cell(use_left, 0);
        assert_eq!(noted(&seen), vec![1, 200]);
        write_cell(left, 2);
        assert_eq!(noted(&seen), vec![1, 200]);
        write_cell(right, 300);
        assert_eq!(noted(&seen), vec![1, 200, 300]);
    }

    /// Outside a computation nothing is listening, which is what makes the
    /// resolve walk a read of the graph rather than a subscriber to it.
    #[test]
    fn a_read_nobody_is_listening_to_subscribes_nothing() {
        let _alone = alone();
        let n = new_cell(1);
        assert_eq!(read_cell(n), 1);
        assert!(subs_of(n).is_empty());
    }

    // --- batching --------------------------------------------------------

    #[test]
    fn a_batch_of_writes_causes_one_pass() {
        let _alone = alone();
        let n = new_cell(0);
        let seen = log();
        let recording = Rc::clone(&seen);
        let _held = watcher(move |scope| {
            let value = scope_read(scope, n);
            recording.borrow_mut().push(value);
            value
        });
        assert_eq!(noted(&seen), vec![0]);
        buri_rt_ui_flush_begin();
        write_cell(n, 1);
        write_cell(n, 2);
        assert_eq!(noted(&seen), vec![0], "an open batch defers the pass");
        buri_rt_ui_flush_end();
        assert_eq!(noted(&seen), vec![0, 2], "one pass, at the value it settled on");
    }

    // --- disposal --------------------------------------------------------

    #[test]
    fn a_run_disposes_what_the_previous_run_created() {
        let _alone = alone();
        let n = new_cell(0);
        let made = log();
        let recording = Rc::clone(&made);
        let _held = watcher(move |scope| {
            let value = scope_read(scope, n);
            recording.borrow_mut().push(rows::owner());
            value
        });
        write_cell(n, 1);
        let made = noted(&made);
        assert_eq!(made.len(), 2, "one node per run");
        let first = made.first().copied().unwrap_or(-1);
        let second = made.get(1).copied().unwrap_or(-1);
        assert!(is_disposed(first), "the first run's node died with the run");
        assert!(!is_disposed(second));
    }

    #[test]
    fn disposing_a_node_disposes_its_children() {
        let _alone = alone();
        let owner = rows::owner();
        let child = rows::under(owner, rows::owner);
        assert_eq!(children_of(owner), vec![child]);
        rows::dispose(owner);
        assert!(is_disposed(owner));
        assert!(is_disposed(child));
    }

    #[test]
    fn a_forgotten_child_outlives_the_owner_that_made_it() {
        let _alone = alone();
        let owner = rows::owner();
        let kept = rows::under(owner, rows::owner);
        let dropped = rows::under(owner, rows::owner);
        rows::forget(owner, dropped);
        assert_eq!(children_of(owner), vec![kept]);
        rows::dispose(owner);
        assert!(is_disposed(kept));
        assert!(!is_disposed(dropped), "forgotten is not owned");
    }

    #[test]
    fn a_run_under_an_owner_subscribes_nothing_and_gives_it_the_children() {
        let _alone = alone();
        let tracked = new_cell(1);
        let untracked = new_cell(2);
        let owner = rows::owner();
        let made = log();
        let recording = Rc::clone(&made);
        let _held = watcher(move |scope| {
            let value = scope_read(scope, tracked);
            recording.borrow_mut().push(rows::under(owner, || {
                let _ = read_cell(untracked);
                rows::owner()
            }));
            value
        });
        assert_eq!(subs_of(tracked).len(), 1, "the watcher reads this one");
        assert!(subs_of(untracked).is_empty(), "a read under an owner is nobody's edge");
        assert_eq!(children_of(owner), noted(&made), "what it made belongs to the owner");
    }

    // --- the budget ------------------------------------------------------

    /// A watcher that writes what it read. The pass stops rather than running
    /// forever, and the sentence is `runtime.js`'s.
    ///
    /// The batch is what keeps this case in the process: a write at depth zero
    /// would drain, and the exported entry ends the program on a runaway.
    #[test]
    fn a_runaway_update_stops_at_the_step_budget() {
        assert_eq!(RUNAWAY, "a reactive update did not settle");
        let _alone = alone();
        let n = new_cell(0);
        buri_rt_ui_flush_begin();
        let _held = watcher(move |scope| {
            let value = scope_read(scope, n);
            write_cell(n, value + 1);
            value
        });
        assert!(!drain(), "the budget reports rather than hanging");
    }

    #[test]
    fn a_signal_that_was_never_made_has_its_own_sentence() {
        assert_eq!(NO_SIGNAL, "this signal does not exist");
        let _alone = alone();
        assert!(lock().get(7).is_none(), "a fresh graph holds no node");
    }

    // --- themes ----------------------------------------------------------

    /// The document `theme.buri`'s `cardThemed(cardTheme)` flattens to.
    const CARDLIB: &str = "theme\n\
        bind cardlib-surface token app-bg\n\
        bind cardlib-onSurface token app-fg\n\
        bind cardlib-accent value rgb(29,78,216)\n";

    const DAY: &str = "theme\n\
        bind app-bg value rgb(255,255,255)\n\
        bind app-fg value rgb(24,24,27)\n";

    const NIGHT: &str = "theme\n\
        bind app-bg value rgb(24,24,27)\n\
        bind app-fg value rgb(240,240,245)\n";

    fn document(themes: &[&str]) -> String {
        let mut doc = String::from("buri-theme 1\n");
        for theme in themes {
            doc.push_str(theme);
        }
        doc
    }

    #[test]
    fn a_theme_resolves_each_of_its_tokens_to_a_value() {
        let _alone = alone();
        assert_eq!(
            install(&document(&[DAY])),
            ":root{--app-bg:rgb(255,255,255);--app-fg:rgb(24,24,27)}\n"
        );
    }

    #[test]
    fn one_block_per_theme_in_the_order_they_were_passed() {
        let _alone = alone();
        assert_eq!(
            install(&document(&[CARDLIB, DAY])),
            ":root{--cardlib-surface:rgb(255,255,255);\
             --cardlib-onSurface:rgb(24,24,27);\
             --cardlib-accent:rgb(29,78,216)}\n\
             :root{--app-bg:rgb(255,255,255);--app-fg:rgb(24,24,27)}\n"
        );
    }

    #[test]
    fn a_chain_resolves_to_the_value_at_its_end() {
        let _alone = alone();
        let block = install(&document(&[CARDLIB, NIGHT]));
        assert!(block.contains("--cardlib-surface:rgb(24,24,27)"));
        assert!(block.contains("--cardlib-onSurface:rgb(240,240,245)"));
        assert!(!block.contains("var("), "the chain arrived, in one step");
    }

    #[test]
    fn a_token_mapped_straight_to_a_value_needs_no_chain() {
        let _alone = alone();
        let block = install(&document(&[CARDLIB, DAY]));
        assert!(block.contains("--cardlib-accent:rgb(29,78,216)"));
    }

    #[test]
    fn a_chain_that_ends_nowhere_names_nothing() {
        let _alone = alone();
        assert_eq!(
            install(&document(&[CARDLIB])),
            ":root{--cardlib-accent:rgb(29,78,216)}\n",
            "the two that lead nowhere are left out rather than guessed at"
        );
    }

    #[test]
    fn a_chain_that_closes_on_itself_names_nothing() {
        let _alone = alone();
        let doc = document(&["theme\nbind app-bg token app-fg\nbind app-fg token app-bg\n"]);
        assert_eq!(install(&doc), "");
    }

    #[test]
    fn no_themes_at_all_is_no_block_at_all() {
        let _alone = alone();
        assert_eq!(install(&document(&[])), "");
    }

    #[test]
    fn a_document_that_is_not_one_resolves_to_nothing() {
        let _alone = alone();
        assert_eq!(install("theme\nbind app-bg value rgb(1,2,3)\n"), "");
    }

    #[test]
    fn a_later_binding_of_one_token_replaces_an_earlier() {
        let _alone = alone();
        let block = install(&document(&[CARDLIB, DAY, NIGHT]));
        assert!(block.contains("--cardlib-surface:rgb(24,24,27)"), "the chain follows the last");
    }

    #[test]
    fn switching_a_theme_swaps_the_values_and_keeps_the_names() {
        let _alone = alone();
        let light = install(&document(&[CARDLIB, DAY]));
        assert!(light.contains("--app-bg:rgb(255,255,255)"));
        assert!(light.contains("--cardlib-surface:rgb(255,255,255)"));

        let dark = install(&document(&[CARDLIB, NIGHT]));
        assert!(dark.contains("--app-bg:rgb(24,24,27)"));
        assert!(dark.contains("--cardlib-surface:rgb(24,24,27)"), "the chain resolves again");
        assert_eq!(
            light.matches(":root{").count(),
            dark.matches(":root{").count(),
            "the same blocks, holding other values"
        );

        assert_eq!(install(&document(&[CARDLIB, DAY])), light, "switching back restores it");
    }

    #[test]
    fn variables_answers_the_block_installed_right_now() {
        let _alone = alone();
        assert_eq!(variables(), "", "nothing is installed yet");
        let block = install(&document(&[DAY]));
        assert_eq!(variables(), block);
    }
}
