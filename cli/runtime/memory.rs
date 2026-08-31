//! Allocation, the 16-byte header, and the reference-count slow paths.
//!
//! MEMORY.md §5. The fast paths — `incref`, `decref`, and one day the size-class
//! `alloc` — are open-coded by both backends and never reach this file; what is
//! here is the block itself, the free path, drop-glue dispatch, and the
//! non-inlined forms the runtime uses on its own values.

use crate::abort::{buri_rt_abort_alloc_budget, buri_rt_abort_oom};
use std::alloc::{alloc, alloc_zeroed, dealloc, realloc, Layout};
use std::sync::atomic::{AtomicU64, Ordering};

/// A reference count that is never decremented and never freed.
///
/// Every string literal, every interned constant aggregate, and every
/// zero-sized value has it (VALUE-MODEL.md §2). `incref` is a *saturating* add
/// so that `IMMORTAL` stays `IMMORTAL` without a branch on the increment side,
/// where the traffic is.
pub const BURI_RT_IMMORTAL: u64 = u64::MAX;

/// Bytes of header in front of every payload. Sixteen, not eight: the payload
/// stays 16-byte aligned for `core/simd`, and `cap` is what both the free path
/// and MEMORY.md §5.3's in-place reuse test read.
pub const BURI_RT_HEADER: usize = 16;

/// The alignment every payload is guaranteed. VALUE-MODEL.md §2.
pub const BURI_RT_ALIGN: usize = 16;

/// Bit 63 of `cap`: the multi-threaded mark. **Set means "this block may be
/// reached from more than one thread"** — the question `incref`/`decref`
/// branch on to choose an atomic count, and the question
/// [`buri_rt_unique_cap`] answers `None` to.
///
/// G1 reserved it and every reader masks; G2 grew the branch; **G3 sets it**,
/// through [`buri_rt_values_may_cross_tasks`] and [`shared_mask`]. A program
/// whose artifact never makes that call is bit-for-bit the program it was
/// before track G, because [`shared_mask`] is zero for the whole of its run.
///
/// **Why `cap` and not `rc`.** A tag bit in the count breaks the two things the
/// count does. [`BURI_RT_IMMORTAL`] is `u64::MAX` and `incref` is a saturating
/// add exactly so the sentinel is a fixed point with no branch on the hot side;
/// a tag bit makes that add wrong. And MEMORY.md §5.3's licence for in-place
/// reuse is the literal `rc == 1` of [`buri_rt_unique_cap`], which a tagged
/// count fails for a block that is genuinely unique. `cap` has neither problem,
/// and a capacity runs out of address space long before bit 63. It follows the
/// `Str::len` ASCII flag, which spends the same bit position in the word next
/// door (VALUE-MODEL.md §3.1); `middle::layout::CAP_SHARED_FLAG` is the
/// compiler's copy of this number.
pub const BURI_RT_CAP_SHARED: u64 = 1 << 63;

/// The usable payload bytes of a block, once [`BURI_RT_CAP_SHARED`] is off.
pub const BURI_RT_CAP_MASK: u64 = !BURI_RT_CAP_SHARED;

// ---------------------------------------------------------------------------
// === G3 begin: the marking latch ==========================================
// ---------------------------------------------------------------------------
//
// # The marking latch
//
// The escape bit asks *which blocks may be reached from more than one thread*.
// This runtime answers it **per program**, not per block: an artifact whose
// values can cross a task boundary marks every block it allocates, from its
// first one, and an artifact whose values cannot marks none.
//
// **Why the whole program and not the value.** MEMORY.md §5.5 states the
// direction to be wrong in — *an over-set bit costs one copy; an under-counted
// reference is a silent aliasing bug* — and a per-value mark is the direction
// that can be under-set. Marking has to be **transitive**: a `[Str]` handed to
// a step is a block whose *elements* the step increfs, and a `Str` inside a
// closure's environment is a block two carriers count. So a per-value mark is
// a type-directed recursive walk — the shape of `Helper::Walk`, which is G5's
// `Helper::Copy` machinery and does not exist yet — and a *shallow* per-value
// mark is exactly the under-count the design forbids. `middle::rc::sharing`
// answers where a second *reference* comes into existence, which is a question
// about sites; this one is about the transitive closure of a heap, and the
// program-wide answer is the only sound one this tree can spell today.
//
// **What it costs.** One relaxed load and one `or` per allocation — see
// [`finish`] — on every program, and atomic reference counting throughout a
// program that fans out. That second number is the one MEMORY.md §5.4 prices
// at 2–3× the reference operation, and it is the price of the feature rather
// than of this shape: a program that does not use `core/tasks` pays neither.
//
// **Where the answer comes from.** `middle::rc::crosses_tasks` computes it
// over the same exact post-monomorphization call graph `can_park` uses, both
// native backends emit [`buri_rt_values_may_cross_tasks`] into `main` when it
// is true, and `cli/runtime/lib.rs` §6 lists the call. **Silence is the safe
// answer**: an entry point that forgets it gets a single-threaded program,
// because `rt::fan_out` is gated on the same latch and falls back to running
// the steps in order. That is the same fail-safe shape D4 gave
// `buri_rt_frames_are_per_carrier`, and for the same reason.

/// [`BURI_RT_CAP_SHARED`] once the artifact has said its values may cross a
/// task boundary, and `0` before that — the whole of the marking policy, as
/// one word.
///
/// A mask rather than a `bool` so that [`finish`] is an `or` and not a branch.
static SHARED_MASK: AtomicU64 = AtomicU64::new(0);

/// The bit [`finish`] stamps into a fresh block's `cap`.
#[inline]
fn shared_mask() -> u64 {
    SHARED_MASK.load(Ordering::Relaxed)
}

/// **This artifact's values may cross a task boundary**, so every block it
/// allocates from here on is counted atomically.
///
/// Emitted into `main` by both native backends, immediately after
/// `buri_rt_argv_init` and before any Buri code runs, and only for a program
/// `middle::rc::crosses_tasks` says can reach one. `cli/runtime/lib.rs` §6 is
/// the contract; calling it twice is calling it once.
///
/// **The ordering requirement is the whole of the safety argument**: a block
/// allocated before this call carries no mark and would be counted
/// non-atomically on a carrier. Nothing allocates before it — `argv_init`
/// stores the arguments as Rust `Vec`s and builds no Buri block — and
/// `rt::fan_out` refuses to fan out at all unless the latch is set, so a
/// backend that got the order wrong is a slow program rather than a racing
/// one. `the_latch_marks_every_block_it_precedes_and_none_before` pins the
/// first half.
///
/// **Relaxed is the right ordering, and it is an argument rather than a
/// default.** This store publishes nothing but itself — there is no other
/// write a reader has to see with it — and every carrier that reads it was
/// started by `thread::Builder::spawn`, which is a synchronisation edge, from
/// a thread that had already made this call. So a carrier's first load happens
/// after this store on every path there is, and an `Acquire` on the allocation
/// path would buy an ordering nothing needs at the price of an `ldapr` per
/// block.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_values_may_cross_tasks() {
    SHARED_MASK.store(BURI_RT_CAP_SHARED, Ordering::Relaxed);
}

/// Whether [`buri_rt_values_may_cross_tasks`] has been called.
///
/// `rt::fan_out`'s gate: a carrier may run Buri code beside another one only
/// where the blocks they both reach are marked.
#[must_use]
pub fn values_may_cross_tasks() -> bool {
    shared_mask() != 0
}

/// The count and the mark of a live block, for a test that wants to say what
/// a reference operation did rather than what it returned.
///
/// `pub(crate)` and test-only: the count is not part of the C ABI — `rc.rs`
/// and both backends open-code their access to it — and an accessor that
/// shipped would be an invitation to read a number whose only correct use is
/// `buri_rt_unique_cap`'s. `rt.rs`'s carrier cases are the callers.
///
/// # Safety
/// `p` is a live payload pointer from [`buri_rt_alloc`].
#[cfg(test)]
pub(crate) unsafe fn count_and_mark(p: *const u8) -> (u64, bool) {
    // SAFETY: the caller promises a live payload pointer.
    unsafe {
        let h = header(p.cast_mut());
        (rc_atomic(h).load(Ordering::Relaxed), is_shared(h))
    }
}

/// Put the latch back, for a test that has just set it.
#[cfg(test)]
pub(crate) fn forget_values_may_cross_tasks() {
    SHARED_MASK.store(0, Ordering::Relaxed);
}

/// **The lock every case that touches the marking latch takes**, whichever
/// module it is in.
///
/// An artifact makes one statement about itself and never takes it back, so a
/// shipping program sets [`SHARED_MASK`] at most once and reads it for the
/// rest of its life. `cargo test` is the one place where both answers exist in
/// one process, and it runs its cases on many threads at once — so a case that
/// *sets* the latch and a case whose answer depends on it must not overlap.
///
/// Two kinds of case take it, and naming both is the point of putting it here
/// rather than in one module's test module:
///
///  * the ones that set it — `rt`'s carrier cases and this module's;
///  * the ones that assert an **in-place write happened**, because
///    [`buri_rt_unique_cap`] refuses a marked block and an in-place write is
///    exactly what a marked block does not get: `text`'s and `list`'s
///    `a_unique_concat_appends_in_place` and `a_unique_push_grows_in_place`.
///
/// Where a case takes both this and `rt`'s `alone()`, `alone()` comes first.
/// There is no other order in the crate, so there is no cycle.
#[cfg(test)]
pub(crate) fn latch() -> std::sync::MutexGuard<'static, ()> {
    static LATCH: std::sync::Mutex<()> = std::sync::Mutex::new(());
    match LATCH.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// === G3 end ===============================================================

/// `{ rc, cap }`, immediately before the payload. `cap` carries
/// [`BURI_RT_CAP_SHARED`] in its top bit, so it is read through [`cap_of`].
#[repr(C)]
struct Header {
    rc: u64,
    cap: u64,
}

/// The usable payload bytes `h` records, with the reserved bit masked off.
///
/// Every read of the capacity goes through here: the free path's layout, the
/// growth decision, the accounting, and the uniqueness test all want a byte
/// count, and none of them wants a flag.
///
/// # Safety
/// `h` must point at a live block's header.
#[inline]
unsafe fn cap_of(h: *const Header) -> u64 {
    // SAFETY: the caller promises a live header.
    unsafe { (*h).cap & BURI_RT_CAP_MASK }
}

/// Whether `h`'s block carries [`BURI_RT_CAP_SHARED`] — the G2 fork.
///
/// True for **every** block of a program whose artifact said its values may
/// cross a task boundary, and false for every block of one that did not: the
/// mark is applied in [`finish`], out of [`shared_mask`], so it is a property
/// of the program rather than of the block. §"The marking latch" is the
/// argument for that shape.
///
/// # Safety
/// `h` must point at a live block's header.
#[inline]
unsafe fn is_shared(h: *const Header) -> bool {
    // SAFETY: the caller promises a live header.
    unsafe { (*h).cap & BURI_RT_CAP_SHARED != 0 }
}

/// The count word of `h`, as the atomic it is on a shared block.
///
/// [`AtomicU64`] has the size and alignment of `u64` and no other
/// representation, so a header's `rc` field *is* one; naming it through this
/// rather than storing an `AtomicU64` in [`Header`] keeps the struct a plain
/// description of the sixteen bytes both backends open-code against.
///
/// # Safety
/// `h` must point at a live block's header, and the reference must not outlive
/// the block.
#[inline]
unsafe fn rc_atomic<'a>(h: *mut Header) -> &'a AtomicU64 {
    // SAFETY: the caller promises a live header, and `AtomicU64` is
    // layout-compatible with the `u64` at that address.
    unsafe { &*std::ptr::addr_of_mut!((*h).rc).cast::<AtomicU64>() }
}

/// How much an atomic reference operation adds or subtracts: `0` for an
/// `IMMORTAL` block, `1` for a counted one.
///
/// This is MEMORY.md §5.1's *saturation* carried onto the atomic path. A plain
/// `fetch_add(1)` would take `u64::MAX` to zero and free every string literal
/// in the program; a plain `fetch_sub(1)` would take it to `u64::MAX - 1` and
/// free it on the next 2^64 decrements. Both backends open-code the same
/// expression.
///
/// The load is relaxed. It may read a count another thread is in the middle of
/// changing, and that does not matter, because the only question asked of it
/// is whether the block is `IMMORTAL` — a property established before a block
/// is published to a second thread, and never revoked.
#[inline]
fn atomic_delta(rc: &AtomicU64) -> u64 {
    u64::from(rc.load(Ordering::Relaxed) != BURI_RT_IMMORTAL)
}

// The one place this runtime is atomic, and it is not on the reference-count
// path: the counts themselves are open-coded plain loads and stores, because
// the language has no threads (MEMORY.md §1). These four are here so that
// `cli/tests/native/memory.rs` can assert "every allocation is freed at exit" — the
// test MEMORY.md §2 asks for to defend the acyclicity lemma — and a relaxed
// add next to a `malloc` is not a cost anybody can measure.
static LIVE_BLOCKS: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL_BLOCKS: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES: AtomicU64 = AtomicU64::new(0);

// G6: two more, and they are about *resident memory* rather than about the
// program's blocks. The four above answer "did this program leak"; these two
// answer "is this runtime still holding what the program gave back", which is
// the question a burst-and-drain asks and which the four above cannot see —
// `live_bytes` reaches zero on a drained heap whether or not a byte of it went
// back to the system.
static RETAINED_BYTES: AtomicU64 = AtomicU64::new(0);
static DECOMMITTED_BYTES: AtomicU64 = AtomicU64::new(0);

/// What [`buri_rt_heap_stats`] writes. Six `u64`s, in this order.
///
/// **The order and the count are an ABI.** `cli/tests/native/driver.c` and
/// `cli/tests/native/shared.rs`'s `ALLOC_PROBE` each declare this struct by
/// hand, because the probe has to be linked into a Buri artifact and there is
/// no header to include; a field added here without adding it there would have
/// this function write past the end of their stack slot. Append only, and
/// append in all three places at once.
#[repr(C)]
pub struct BuriHeapStats {
    pub live_blocks: u64,
    pub live_bytes: u64,
    pub total_blocks: u64,
    pub total_bytes: u64,
    /// Bytes of dead block — payload plus header — this runtime is holding in
    /// its per-thread caches right now, allocated from the system and not yet
    /// given back to it. **Not live and not a leak**: the program does not own
    /// these, this file does. `live_bytes + retained_bytes` is what the
    /// runtime is charging the process for.
    ///
    /// **Accurate to within one sweep period per live carrier.** A carrier
    /// publishes its total at each sweep and when it ends, rather than on
    /// every push and pop, because a process-wide atomic on the allocator's
    /// fast path costs more than the number is worth — `Cache::published` has
    /// the measurement. A carrier that has ended has published, always, so an
    /// assertion taken after a `join` is exact.
    pub retained_bytes: u64,
    /// Bytes of carrier stack **range** decommitted since the process started,
    /// cumulative: one [`buri_rt_stack_release`] adds
    /// `BURI_RT_STACK_USABLE - BURI_RT_STACK_WARM`, whether or not the kernel
    /// had a resident page for every byte of it. It is a count of what this
    /// runtime gave back, not a measurement of what the process was holding —
    /// the second is the operating system's number and this file has no
    /// portable way to ask for it. Never falls.
    pub decommitted_bytes: u64,
}

#[inline]
fn layout_for(payload: u64) -> Layout {
    let bytes = (payload as usize).checked_add(BURI_RT_HEADER);
    match bytes.and_then(|b| Layout::from_size_align(b, BURI_RT_ALIGN).ok()) {
        Some(l) => l,
        // A request that cannot even be described is out of memory by any
        // useful definition, and there is no value to report it with:
        // `allocate` returns `Region`, not `Result` (`effect.buri:19`).
        None => buri_rt_abort_oom(payload),
    }
}

/// The header of the block whose payload starts at `p`.
///
/// # Safety
/// `p` must be a payload pointer returned by [`buri_rt_alloc`] and not yet
/// freed, or the payload pointer of a static the compiler laid out with the
/// same 16-byte header.
#[inline]
unsafe fn header(p: *mut u8) -> *mut Header {
    unsafe { p.sub(BURI_RT_HEADER).cast::<Header>() }
}

/// A fresh block with `payload` usable bytes, `rc == 1`, `cap == payload`.
///
/// Returns the **payload** pointer, 16-byte aligned; the header is at `p - 16`.
/// Never returns null: exhaustion aborts, because SPEC 10.5 says `Alloc` can
/// fail and SPEC 6.10 says a failure with no value to return is an abort.
///
/// The contents are uninitialized. Use [`buri_rt_alloc_zeroed`] where the
/// caller does not write every byte.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc(payload: u64) -> *mut u8 {
    // G2: this thread's cache first. A hit is a block of exactly `payload`
    // usable bytes, so `finish` writes the same header it would have written
    // over a fresh one.
    if let Some(p) = cache_pop(payload) {
        // SAFETY: `p` is a payload pointer, so `p - 16` is its block.
        return finish(unsafe { p.sub(BURI_RT_HEADER) }, payload);
    }
    let layout = layout_for(payload);
    // SAFETY: `layout` has a non-zero size — the header alone is 16 bytes.
    let raw = unsafe { alloc(layout) };
    finish(raw, payload)
}

/// [`buri_rt_alloc`], with the payload zeroed.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_zeroed(payload: u64) -> *mut u8 {
    // G2: a cached block holds whatever the last value in it held, so this one
    // zeroes what `alloc_zeroed` would have got from the allocator.
    if let Some(p) = cache_pop(payload) {
        // SAFETY: `p` is a payload pointer to a block with `payload` usable
        // bytes, which is exactly the range written.
        unsafe {
            std::ptr::write_bytes(p, 0, payload as usize);
            return finish(p.sub(BURI_RT_HEADER), payload);
        }
    }
    let layout = layout_for(payload);
    // SAFETY: as above.
    let raw = unsafe { alloc_zeroed(layout) };
    finish(raw, payload)
}

fn finish(raw: *mut u8, payload: u64) -> *mut u8 {
    if raw.is_null() {
        buri_rt_abort_oom(payload);
    }
    // SAFETY: `raw` is a fresh allocation of at least `BURI_RT_HEADER` bytes,
    // aligned to 16, so the header is in bounds and aligned.
    unsafe {
        // G3: the mark, out of the process-wide latch rather than out of
        // anything about this block. One relaxed load of a word that is written
        // at most once in a program's life and read on every allocation, so it
        // is in this core's cache from the first block onwards, and an `or`
        // into a store that was happening anyway. A recycled block comes
        // through here too, which is what keeps a cache hit and a fresh
        // `malloc` indistinguishable.
        raw.cast::<Header>().write(Header { rc: 1, cap: payload | shared_mask() });
    }
    LIVE_BLOCKS.fetch_add(1, Ordering::Relaxed);
    LIVE_BYTES.fetch_add(payload, Ordering::Relaxed);
    TOTAL_BLOCKS.fetch_add(1, Ordering::Relaxed);
    TOTAL_BYTES.fetch_add(payload, Ordering::Relaxed);
    // SAFETY: the block is `BURI_RT_HEADER + payload` bytes, so the payload
    // start is one-past-the-header and in bounds.
    unsafe { raw.add(BURI_RT_HEADER) }
}

// ---------------------------------------------------------------------------
// === G2 begin: the per-thread block caches =================================
// ---------------------------------------------------------------------------
//
// MEMORY.md §5.4 states the cost of threads in two clauses: "reference
// operations become atomic, and the allocator grows per-thread caches". The
// first is the fork above. This is the second, and it is the smallest form of
// it that is correct against the allocator this runtime actually has.
//
// **Why exact sizes and not size classes.** A class allocator rounds a request
// up, so `cap` comes back larger than the payload asked for — which MEMORY.md
// §5.4 anticipates *and* §5.3 forbids for one case: the release glue of a
// `[T]` walks `cap / stride` elements, so spare capacity in a block of counted
// elements is a walk over slots nothing wrote. `buri_rt_grown_capacity` is
// allowed to overshoot only because the fast paths that use it are restricted
// to element types holding no references. A cache is under no such
// restriction — every block in the program passes through it — so it must not
// change `cap` at all. Keying on the *exact* payload size gives a cache with
// no semantic footprint whatever: `cap` is what it always was, `layout_for`
// recovers the layout the block was made with, and the drop walk counts what
// it always counted.
//
// **What it costs, and where it is spent.** A hit is a load, a store and a
// subtraction instead of a `malloc`; MEMORY.md §5.4 prices those at about five
// cycles against about twenty. The slots reach 256 bytes because that is where
// this language's allocation histogram is — a `Str`'s bytes, a fixed-size
// aggregate, a list below the growth floor's first few doublings — and a block
// above it is rare enough that a `malloc` per block is the right answer.
//
// **What it does not change.** `buri_rt_heap_stats` counts *blocks the program
// asked for*, not calls this file made to `malloc`, so every number a test can
// read is the number it read before this cache existed: a hit still increments
// `LIVE_*` and `TOTAL_*`, and a free still decrements `LIVE_*`. That is what
// keeps `cli/tests/native`'s allocation-count assertions measuring the
// compiler's elision rather than this file's hit rate.

/// The largest payload a cached block holds, in bytes.
const CACHE_MAX_PAYLOAD: u64 = 256;

/// One free-list head per exact payload size, `0..=CACHE_MAX_PAYLOAD`.
const CACHE_SLOTS: usize = CACHE_MAX_PAYLOAD as usize + 1;

/// The whole process's cache budget in bytes, **before** it is divided between
/// carriers. Four mebibytes: large enough that a single-threaded program keeps
/// its whole working set of small blocks, small enough to be invisible beside
/// the heap of any program that allocates enough to care.
const CACHE_BYTES: u64 = 4 << 20;

/// The floor a carrier's share does not go under, however many carriers there
/// are. A cache too small to hold a loop's block is a cache that costs a branch
/// and buys nothing.
const CACHE_BYTES_FLOOR: u64 = 64 << 10;

/// One carrier's share of [`CACHE_BYTES`].
///
/// **This is the "sized for carrier count" of MEMORY.md §5.4.** The budget is
/// stated for the process and divided, so that the cache's total footprint is
/// a property of the program rather than of how wide the carrier pool happens
/// to be: sixteen carriers get a sixteenth each rather than sixteen times the
/// memory. `available_parallelism` is the carrier count the pool is built for
/// (design/native, track B, the `rt.rs` pool) and is asked once.
fn cache_budget() -> u64 {
    static BUDGET: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        let carriers = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        let share = CACHE_BYTES / (carriers as u64).max(1);
        share.max(CACHE_BYTES_FLOOR)
    })
}

/// How many cache operations pass between two sweeps of the decay below.
///
/// **G6.** Small enough that a program which stops allocating has given its
/// cache back within a thousand more frees, large enough that the sweep's walk
/// over [`CACHE_SLOTS`] slots amortises to well under a nanosecond of the
/// 7.1 ns an alloc-and-free pair costs. It counts *refused* pushes as well as
/// accepted ones, which is the clause that makes a drain past the budget
/// converge: once the cache is full every push is a refusal, and a clock that
/// only ticked on acceptance would stop exactly when there was most to give
/// back.
///
/// It also sets the residue: after the first sweep of a *pure* drain a slot
/// keeps only what the period since put into it, so a program that allocates a
/// hundred thousand blocks and gives them all back ends holding at most two
/// periods rather than the whole of `cache_budget()`. Measured on 200 000
/// 64-byte blocks: **419 360 bytes retained before this slice, 25 600
/// after.**
const CACHE_SWEEP_OPS: u32 = 1024;

/// One carrier's cache: a free-list head per exact payload size, the bytes
/// they hold between them, and (G6) the decay state that gives them back.
///
/// A dead block's own header carries the link — `rc` holds the next block's
/// payload pointer, `cap` is left alone — so the lists cost sixteen bytes of
/// list state per thread and not one byte per block.
struct Cache {
    /// **One array, not two.** A slot's head and its decay flag are written
    /// together on every pop, so two parallel arrays would cost two cache
    /// lines per allocation where one will do.
    slots: [Slot; CACHE_SLOTS],
    held: u64,
    /// What this carrier last told [`RETAINED_BYTES`] it was holding.
    ///
    /// **The counter is published at sweep boundaries, not per operation**,
    /// and that is a measured decision rather than a tidy one: an
    /// `AtomicU64::fetch_add` in `cache_push` and a `fetch_sub` in `cache_pop`
    /// measured **2.3 ns on an alloc-and-free pair that takes 7.4 ns in
    /// total** — a third of the fast path G2 exists to make fast, spent on a
    /// number nothing reads more than a few times in a program's life.
    /// Publishing at each sweep and at thread exit makes it one atomic per
    /// [`CACHE_SWEEP_OPS`] operations, and costs the counter's readers an
    /// accuracy of one sweep period per live carrier — which
    /// `BuriHeapStats::retained_bytes` says out loud.
    published: u64,
    /// Cache operations since the last sweep.
    since_sweep: u32,
}

/// One exact payload size's free list.
#[derive(Clone, Copy)]
struct Slot {
    /// The first dead block, or null. The rest are chained through their own
    /// `rc` words.
    head: *mut u8,
    /// **The whole of the decay state, and it is one byte set by `pop`.**
    ///
    /// True if the program has taken a block of this size since the last
    /// sweep. A sweep gives back the *whole* of every slot that has not been
    /// touched — blocks the workload demonstrably did not want across a full
    /// period — and leaves a touched slot entirely alone. So a ping-pong
    /// workload, which pops from its slot in every period, pays nothing at all
    /// for this mechanism, and a drained cache empties itself.
    ///
    /// **It replaced a length and a low-water mark, and that was worth 4.5% of
    /// an allocation-bound program.** Tracking how *many* blocks a slot could
    /// spare is a finer answer, and it costs a decrement, a saturation, a
    /// compare and a second store on the two hottest paths in this file: end
    /// to end on G2's `allocs` row it measured +4.5 points against this
    /// slice's +2.9 total. What the coarser rule gives up is that a slot
    /// popped once in a period keeps everything it holds rather than only what
    /// it needs — which is bounded by [`cache_budget`] either way, and is the
    /// bound G2 already chose to live with.
    hit: bool,
}

/// The bytes one block of `payload` usable bytes costs the process.
#[inline]
const fn slot_bytes(payload: u64) -> u64 {
    payload + BURI_RT_HEADER as u64
}

impl Cache {
    /// Register [`CACHE_DRAIN`]'s destructor for this carrier, and open the
    /// cache for business. Once, ever.
    #[cold]
    #[inline(never)]
    fn arm(&mut self) {
        self.held = 0;
        let _ = CACHE_DRAIN.try_with(|_| ());
    }

    /// Give every cached block back and refuse to keep another. Runs from
    /// [`CacheDrain::drop`], on the way out of the carrier.
    fn close(&mut self) {
        for idx in 0..CACHE_SLOTS {
            self.release_slot(idx);
        }
        self.publish();
        self.held = CACHE_CLOSED;
    }

    /// Tell [`RETAINED_BYTES`] what this carrier is holding now.
    fn publish(&mut self) {
        if self.held >= CACHE_UNARMED {
            return;
        }
        if self.held >= self.published {
            RETAINED_BYTES.fetch_add(self.held - self.published, Ordering::Relaxed);
        } else {
            RETAINED_BYTES.fetch_sub(self.published - self.held, Ordering::Relaxed);
        }
        self.published = self.held;
    }

    /// One cache operation has happened; sweep if enough of them have.
    ///
    /// **`sweep` is `#[cold]` and `#[inline(never)]` deliberately**, and this
    /// is the single largest thing G6 does for the allocator's fast path.
    /// What is left here is an increment and a compare; letting the sweep's
    /// loop inline into `cache_push` grows the free path enough that
    /// `cache_push` stops being inlined into `buri_rt_free`, and that decision
    /// alone measured **0.6 ns on a 7.1 ns alloc-and-free pair** — as much as
    /// the whole of the rest of this slice costs it.
    #[inline(always)]
    fn tick(&mut self) {
        self.since_sweep += 1;
        if self.since_sweep >= CACHE_SWEEP_OPS {
            self.sweep();
        }
    }

    /// **Give back every slot that went a whole sweep period untouched.**
    ///
    /// See [`Slot::hit`] for why that is the right set. A slot that survives
    /// this sweep starts the next period untouched again, so it is asked the
    /// same question every period rather than being kept forever because it
    /// was once popular.
    #[cold]
    #[inline(never)]
    fn sweep(&mut self) {
        self.since_sweep = 0;
        for idx in 0..CACHE_SLOTS {
            if self.slots[idx].hit {
                self.slots[idx].hit = false;
            } else if !self.slots[idx].head.is_null() {
                self.release_slot(idx);
            }
        }
        self.publish();
    }

    /// Return up to `n` blocks of slot `idx` to the system allocator.
    ///
    /// **`dealloc`, not `madvise`** — and that is the stated rule for a
    /// cached-but-empty chunk. A cached block's pages are not this file's to
    /// decommit: it came out of `std::alloc::alloc`, so the system allocator
    /// owns the chunk it was carved from and is the only thing that can decide
    /// whether that chunk is now empty. Handing the block back is therefore
    /// the whole of what this runtime can honestly do, and it is what puts the
    /// allocator in a position to do the rest. (The pages this file *does* own
    /// are the carrier stacks, and those are decommitted for real: see
    /// [`buri_rt_stack_release`].)
    fn release_slot(&mut self, idx: usize) {
        let payload = idx as u64;
        let bytes = slot_bytes(payload);
        let layout = layout_for(payload);
        loop {
            let p = self.slots[idx].head;
            if p.is_null() {
                return;
            }
            // SAFETY: every pointer in a slot is a dead block this file freed,
            // of exactly `payload` usable bytes, whose `rc` holds the next one.
            self.slots[idx].head = unsafe { (*header(p)).rc as *mut u8 };
            self.held = self.held.saturating_sub(bytes);
            // SAFETY: `p - 16` is the allocation and `layout` is the layout it
            // was created with — the slot index *is* the capacity, which is
            // the whole point of keying on exact sizes.
            unsafe { dealloc(p.sub(BURI_RT_HEADER), layout) }
        }
    }
}

/// The `held` of a carrier whose cache has been drained for the last time.
///
/// Every push tests `held + bytes > cache_budget()` already, and `u64::MAX`
/// fails it forever, so closing the cache costs the fast path no branch of its
/// own: a block freed after this carrier's destructor has run goes straight
/// back to the allocator instead of onto a list nothing will ever drain again.
const CACHE_CLOSED: u64 = u64::MAX;

/// The `held` of a carrier that has not yet registered [`CACHE_DRAIN`].
///
/// **A sentinel rather than a flag, so that arming costs the fast path
/// nothing.** Every push already tests `held + bytes > cache_budget()`; this
/// value fails that test, so a carrier's *first* free takes the refusal path,
/// which is where [`Cache::arm`] lives and which is out of line already; the
/// arm opens the cache and the block is kept after all. The accepted-push path
/// therefore has no have-I-armed branch in it at all.
const CACHE_UNARMED: u64 = u64::MAX - 1;

/// **A carrier that ends gives its cache back**, and this is the thing that
/// makes it happen.
///
/// Without it a thread's lists die with its thread-local storage and every
/// block in them is lost to the process for good — not a leak by
/// `buri_rt_heap_stats`'s reckoning, because `buri_rt_free` decremented
/// `live_blocks` before the block entered a list, but memory `malloc` can
/// never hand out again, once per carrier that ever ran. Measured: sixteen
/// carriers that each allocate and free a thousand 248-byte blocks ended
/// holding 4 224 000 bytes between them, and before this slice every one of
/// those bytes stayed held for the life of the process.
///
/// **Why a separate one-byte thread-local instead of `impl Drop for Cache`.**
/// A `thread_local!` whose type needs dropping is not the same thing as one
/// whose type does not: the second compiles to a bare `#[thread_local]` static
/// and the first to a lazily-registered one with a state byte tested on every
/// access — on `CACHE`, twice per allocation. Hanging the destructor on a
/// zero-sized neighbour that is touched **once per carrier** keeps `CACHE`
/// itself in the cheap form. [`Cache::arm`] is that one touch.
struct CacheDrain;

impl Drop for CacheDrain {
    fn drop(&mut self) {
        // `try_with` cannot fail here — `CACHE`'s type has no destructor, so
        // it has no destroyed state to be in — but asking is free and a panic
        // out of a thread-local destructor would abort the process.
        let _ = CACHE.try_with(|c| {
            // SAFETY: this thread's own cell, on the way out; nothing else can
            // hold a reference derived from it.
            let cache = unsafe { &mut *c.get() };
            cache.close();
        });
    }
}

thread_local! {
    static CACHE: std::cell::UnsafeCell<Cache> = const {
        std::cell::UnsafeCell::new(Cache {
            slots: [Slot { head: std::ptr::null_mut(), hit: false }; CACHE_SLOTS],
            held: CACHE_UNARMED,
            published: 0,
            since_sweep: 0,
        })
    };

    /// Touched once per carrier, by [`Cache::arm`], purely so that its
    /// destructor is registered.
    static CACHE_DRAIN: CacheDrain = const { CacheDrain };
}

/// A dead block of exactly `payload` usable bytes from this thread's cache.
fn cache_pop(payload: u64) -> Option<*mut u8> {
    if payload > CACHE_MAX_PAYLOAD {
        return None;
    }
    let idx = payload as usize;
    // `try_with` rather than `with`: a block freed while this thread's
    // destructors run must not panic, and the answer there is simply "no
    // cache".
    CACHE
        .try_with(|c| {
            // SAFETY: `c` is this thread's own cell, and no reference derived
            // from it escapes this closure or crosses a call that could
            // re-enter.
            let cache = unsafe { &mut *c.get() };
            let slot = cache.slots.get_mut(idx)?;
            let p = slot.head;
            if p.is_null() {
                return None;
            }
            // SAFETY: every pointer in a slot is a block this file freed, of
            // exactly `payload` usable bytes, whose `rc` holds the next one.
            slot.head = unsafe { (*header(p)).rc as *mut u8 };
            // G6: the program wanted this size in this period, so the sweep
            // leaves the slot alone.
            slot.hit = true;
            cache.held = cache.held.saturating_sub(slot_bytes(payload));
            Some(p)
        })
        .ok()
        .flatten()
}

/// Keep a dead block of `cap` usable bytes in this thread's cache, if there is
/// room for it. `false` means the caller must return it to the allocator.
///
/// # Safety
/// `p` is a payload pointer whose block is dead, has exactly `cap` usable
/// bytes, and is not reachable from anywhere else.
unsafe fn cache_push(p: *mut u8, cap: u64) -> bool {
    if cap > CACHE_MAX_PAYLOAD {
        return false;
    }
    let idx = cap as usize;
    CACHE
        .try_with(|c| {
            // SAFETY: as in `cache_pop`.
            let cache = unsafe { &mut *c.get() };
            let bytes = slot_bytes(cap);
            if idx >= CACHE_SLOTS {
                return false;
            }
            if cache.held.saturating_add(bytes) > cache_budget() {
                if cache.held != CACHE_UNARMED {
                    // G6: a refusal is still an operation. See
                    // `CACHE_SWEEP_OPS`.
                    cache.tick();
                    return false;
                }
                // The carrier's first free. Register the destructor that will
                // drain this cache, open it, and keep the block after all.
                cache.arm();
            }
            let slot = &mut cache.slots[idx];
            // SAFETY: the caller promises a dead block, so its header is this
            // file's to use as list storage.
            unsafe {
                (*header(p)).rc = slot.head as u64;
            }
            slot.head = p;
            cache.held = cache.held.saturating_add(bytes);
            cache.tick();
            true
        })
        .unwrap_or(false)
}

// === G2 end ================================================================

/// Grow or shrink a block in place where the allocator can, preserving `rc`.
///
/// This is the fallback half of MEMORY.md §5.3's in-place reuse: the *uniquely
/// owned, spare capacity* case never gets here, because the backend's `rc == 1`
/// test writes the element and returns the same pointer. This is what runs when
/// the capacity ran out and the value is still unique.
///
/// # Safety
/// `p` must be a live payload pointer from [`buri_rt_alloc`], and the caller
/// must hold the only reference to it (`rc == 1`). Aliased reallocation would
/// leave every other holder with a dangling pointer, which is why the backend
/// only emits this under the uniqueness test.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_realloc(p: *mut u8, payload: u64) -> *mut u8 {
    if p.is_null() {
        return buri_rt_alloc(payload);
    }
    // SAFETY: the caller promises a live payload pointer.
    let (old_cap, rc, flags) = unsafe {
        let h = header(p);
        (cap_of(h), (*h).rc, (*h).cap & BURI_RT_CAP_SHARED)
    };
    let old = layout_for(old_cap);
    let new = layout_for(payload);
    // SAFETY: `p - 16` is the allocation `old` describes, and `new.size()` is
    // non-zero.
    let raw = unsafe { realloc(p.sub(BURI_RT_HEADER), old, new.size()) };
    if raw.is_null() {
        buri_rt_abort_oom(payload);
    }
    // SAFETY: `raw` is a live allocation of at least the header's size.
    unsafe {
        raw.cast::<Header>().write(Header { rc, cap: payload | flags });
    }
    LIVE_BYTES.fetch_add(payload.wrapping_sub(old_cap), Ordering::Relaxed);
    TOTAL_BYTES.fetch_add(payload.saturating_sub(old_cap), Ordering::Relaxed);
    // SAFETY: in bounds, as in `finish`.
    unsafe { raw.add(BURI_RT_HEADER) }
}

/// Return a block to the allocator. Drop glue has already run.
///
/// This is the tail of `decref`, split out because the backend open-codes
/// everything up to it (MEMORY.md §5.1) and calls only the cold path. Freeing
/// an `IMMORTAL` block is a no-op rather than an error: a generic path can
/// reach one, and that is the whole reason the sentinel exists.
///
/// # Safety
/// `p` must be a live payload pointer from [`buri_rt_alloc`] with no remaining
/// references, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: the caller promises a live payload pointer.
    let cap = unsafe {
        let h = header(p);
        if (*h).rc == BURI_RT_IMMORTAL {
            return;
        }
        cap_of(h)
    };
    LIVE_BLOCKS.fetch_sub(1, Ordering::Relaxed);
    LIVE_BYTES.fetch_sub(cap, Ordering::Relaxed);
    // G2: this thread's cache keeps a small block rather than returning it.
    // The accounting above has already run, so a cached block is not live and
    // is not a leak — it is memory this runtime holds, exactly as the
    // allocator holds a free list.
    //
    // SAFETY: the block is dead, has `cap` usable bytes, and this is its last
    // reference.
    if unsafe { cache_push(p, cap) } {
        return;
    }
    // SAFETY: `p - 16` is the allocation, and `layout_for(cap)` is the layout
    // it was created with — `cap` is stored precisely so this is recoverable.
    unsafe { dealloc(p.sub(BURI_RT_HEADER), layout_for(cap)) }
}

/// The non-inlined `incref`. Saturating, so `IMMORTAL` is a fixed point.
///
/// Forks on [`BURI_RT_CAP_SHARED`] exactly as both backends' open-coded copies
/// do, so a block reached from a generic path is counted the same way as one
/// reached from emitted code. The unshared arm is the one every program takes
/// today, and it is untouched.
///
/// # Safety
/// `p` is null or a live payload pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_incref(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: the caller promises a live payload pointer.
    unsafe {
        let h = header(p);
        if is_shared(h) {
            let rc = rc_atomic(h);
            rc.fetch_add(atomic_delta(rc), Ordering::Relaxed);
        } else {
            (*h).rc = (*h).rc.saturating_add(1);
        }
    }
}

/// The non-inlined `decref`, with drop-glue dispatch.
///
/// `drop_glue` is the generated per-type `drop_T` (MEMORY.md §5.1) and is null
/// for a type whose payload holds no references — a `Str`'s bytes, an `[Int]`,
/// a struct of scalars — which is most of them. It is called with the payload
/// pointer, before the block goes back, and must not free the block itself.
///
/// # Safety
/// `p` is null or a live payload pointer, and `drop_glue`, where non-null, is
/// the drop glue for the type `p` actually holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_decref(p: *mut u8, drop_glue: Option<extern "C" fn(*mut u8)>) {
    if p.is_null() {
        return;
    }
    // SAFETY: the caller promises a live payload pointer.
    let dead = unsafe {
        let h = header(p);
        if is_shared(h) {
            // The atomic arm answers the count *before* the subtraction, so
            // the block is this thread's to free when that was `1`. Two
            // threads each reading `1` from a separate load would each free
            // it, which is why the test is on what the `fetch_sub` returned.
            // `AcqRel` publishes this thread's writes to the value to whoever
            // performs the last decrement, and makes them visible to it before
            // the glue runs. An `IMMORTAL` block subtracts nothing and answers
            // `u64::MAX`, so it is never the one.
            let rc = rc_atomic(h);
            rc.fetch_sub(atomic_delta(rc), Ordering::AcqRel) == 1
        } else {
            let rc = (*h).rc;
            if rc == BURI_RT_IMMORTAL {
                return;
            }
            if rc > 1 {
                (*h).rc = rc - 1;
            }
            rc <= 1
        }
    };
    if dead {
        if let Some(glue) = drop_glue {
            glue(p);
        }
        // SAFETY: the count reached zero, so no other reference exists.
        unsafe { buri_rt_free(p) }
    }
}

/// Mark a block as never counted and never freed.
///
/// The compiler knows about `IMMORTAL` statically for literals and interned
/// aggregates and emits no reference operations for them at all (MEMORY.md
/// §5.2). This is for a value that reached a generic path — and for the
/// runtime's own statics.
///
/// # Safety
/// `p` is null or a live payload pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_make_immortal(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: the caller promises a live payload pointer. The block is
    // deliberately taken out of `LIVE_*`: it is not a leak, and a leak check
    // that flagged every string literal would report nothing useful. Marking a
    // block that is already immortal is a no-op rather than a second
    // subtraction, so the operation is idempotent and a generic path may reach
    // it twice.
    unsafe {
        let h = header(p);
        if (*h).rc == BURI_RT_IMMORTAL {
            return;
        }
        LIVE_BLOCKS.fetch_sub(1, Ordering::Relaxed);
        LIVE_BYTES.fetch_sub(cap_of(h), Ordering::Relaxed);
        (*h).rc = BURI_RT_IMMORTAL;
    }
}

/// The current reference count. For tests and for a defensive build.
///
/// # Safety
/// `p` must be a live payload pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_rc(p: *mut u8) -> u64 {
    // SAFETY: the caller promises a live payload pointer.
    unsafe { (*header(p)).rc }
}

/// The usable payload bytes of a block — MEMORY.md §5.3's reuse test reads it.
///
/// # Safety
/// `p` must be a live payload pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_cap(p: *mut u8) -> u64 {
    // SAFETY: the caller promises a live payload pointer.
    unsafe { cap_of(header(p)) }
}

/// Heap accounting, for `cli/tests/native/memory.rs` and for `--explain`.
///
/// # Safety
/// `out` must be non-null and aligned for [`BuriHeapStats`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_heap_stats(out: *mut BuriHeapStats) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe {
        out.write(BuriHeapStats {
            live_blocks: LIVE_BLOCKS.load(Ordering::Relaxed),
            live_bytes: LIVE_BYTES.load(Ordering::Relaxed),
            total_blocks: TOTAL_BLOCKS.load(Ordering::Relaxed),
            total_bytes: TOTAL_BYTES.load(Ordering::Relaxed),
            retained_bytes: RETAINED_BYTES.load(Ordering::Relaxed),
            decommitted_bytes: DECOMMITTED_BYTES.load(Ordering::Relaxed),
        });
    }
}

/// Blocks allocated and not yet freed. Zero at exit is the property
/// MEMORY.md §2 asks a test to assert.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_live_blocks() -> u64 {
    LIVE_BLOCKS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Uniqueness, and the growth policy over it (MEMORY.md §5.3)
// ---------------------------------------------------------------------------

/// The smallest capacity a growing block is given, in bytes.
///
/// One 16-byte header's worth of payload for eight `i64`s: the point is that a
/// list built one element at a time does not reallocate for the first handful,
/// and a floor smaller than a cache line buys nothing. Named here rather than
/// spelled at each call site because MEMORY.md §5.3 documents the policy and a
/// policy in three places is three policies.
pub const BURI_RT_GROWTH_FLOOR: u64 = 64;

/// The capacity to give a block that needs `needed` bytes and had `old_cap`.
///
/// **Doubling with a floor.** `needed` is the lower bound the caller cannot go
/// under; `old_cap * 2` is what makes a sequence of appends amortized O(1) and
/// therefore O(log n) allocations rather than O(n); the floor keeps the first
/// few appends from reallocating at all.
///
/// Saturating rather than checked: a capacity that overflows a `u64` is a
/// request `layout_for` will refuse, and refusing it there keeps the arithmetic
/// here total.
///
/// `old_cap` is masked with [`BURI_RT_CAP_MASK`] first — a header word is a
/// capacity plus [`BURI_RT_CAP_SHARED`], and doubling the flag would ask for
/// the whole address space.
#[must_use]
pub fn buri_rt_grown_capacity(needed: u64, old_cap: u64) -> u64 {
    let old_cap = old_cap & BURI_RT_CAP_MASK;
    needed.max(old_cap.saturating_mul(2)).max(BURI_RT_GROWTH_FLOOR)
}

/// `Some(cap)` when `p` is a live block that **nothing else references**.
///
/// The capacity comes back masked ([`BURI_RT_CAP_MASK`]): callers spend it as a
/// byte count, and the reserved bit is not part of one.
///
/// This is MEMORY.md §5.3's test, and it is the whole licence for the in-place
/// writes in `list.rs` and `text.rs`. The reasoning it rests on, stated once
/// here because every caller depends on it:
///
/// * A reference count of one means exactly one live value in the program
///   refers to this block. Static elision never *duplicates* a reference
///   without an `incref` — a borrowed parameter aliases the caller's own
///   reference rather than adding one, and every operation that produces a new
///   view of a block (`str.slice`, `str.trim`, `splitOnce`) increfs the base
///   before answering. So `rc == 1` is not "one binding": it is "one
///   observable value", and the aliases elision leaves behind are copies of
///   *that* value, with the same `ptr` and the same `len`.
/// * Therefore a write **strictly past the length every such alias carries** is
///   unobservable. That is the only kind of write the callers make: an append
///   writes at `len` and beyond, never at an index any live descriptor covers.
///
/// `IMMORTAL` fails the test by construction (`u64::MAX != 1`), which is what
/// keeps a literal or an interned constant aggregate out of it.
///
/// # A marked block is never unique, and this is the G3 half
///
/// G2 left this function unforked and said why: *a thread holding no reference
/// cannot make a second one, so a count of one cannot move under the caller
/// who read it.* That argument has a premise — **the caller holds the
/// reference it is testing** — and it is the premise the baton was keeping
/// true. A borrowed parameter aliases somebody else's reference rather than
/// adding one, so a step of a `Tasks.parallel` that reads `rc == 1` off a list
/// its closure's environment owns is a carrier testing a block it does *not*
/// hold, and every other carrier is reading the same `1` at the same time.
/// Under the baton those carriers were serialised; without it they would each
/// take the licence and write through the same block.
///
/// So [`BURI_RT_CAP_SHARED`] is the second half of the test, and it is a
/// **conservative** half in the direction MEMORY.md §5.5 names: a marked block
/// answers `None`, the caller allocates and copies, and what an over-set mark
/// costs is that copy. `IMMORTAL` and the mark are the two ways to fail it and
/// neither can be un-failed, which is what makes the answer stable in a way a
/// count read on another carrier is not.
///
/// It is one load and one test more than G2's version — of the `cap` word this
/// function was already going to read for its answer.
///
/// # Safety
/// `p` is null or a live payload pointer from [`buri_rt_alloc`].
#[must_use]
pub unsafe fn buri_rt_unique_cap(p: *const u8) -> Option<u64> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller promises a live payload pointer, so the header is in
    // bounds and aligned.
    let h = unsafe { header(p.cast_mut()) };
    // SAFETY: as above. The mark is read first, because it is the half that
    // does not depend on the count being stable: a block reachable from a
    // second carrier fails here whatever its count says, and a block that is
    // not is this thread's alone, where G2's argument for the relaxed load
    // holds unchanged — `rc == 1` cannot move under the caller who read it.
    unsafe {
        if is_shared(h) {
            return None;
        }
        (rc_atomic(h).load(Ordering::Relaxed) == 1).then(|| cap_of(h))
    }
}

// ---------------------------------------------------------------------------
// `Alloc`, the effect
// ---------------------------------------------------------------------------

/// `host.alloc.allocate(bytes) -> Region(bytes)`.
///
/// MEMORY.md §7: the charge is a function of the *types*, computed by
/// `middle::layout`, so it is the same number on both backends and both
/// platforms. `allocate` is the one row that charges its own argument, and
/// `HostAlloc` is zero-sized and unbounded, so this returns what it was asked
/// for and the accounting is the caller's.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_alloc_allocate(bytes: i64) -> i64 {
    bytes
}

/// A `FixedBuffer(n)` overrun.
///
/// MEMORY.md §7.2: `allocate` returns `Region`, not `Result<Region, _>`, so
/// there is no value to report failure with and exceeding the budget aborts —
/// with the budget and the request in the message, which is the only way a
/// reader of the abort can tell what to change.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_budget_check(requested: i64, used: i64, budget: i64) -> i64 {
    if used.saturating_add(requested) > budget {
        buri_rt_abort_alloc_budget(requested, budget);
    }
    requested
}

// ---------------------------------------------------------------------------
// `core/alloc`'s counters
// ---------------------------------------------------------------------------
//
// `GeneralPurpose`, `Arena` and `FixedBuffer` are one counter with three
// policies over it (MEMORY.md §7.2), and each carries a handle into this table
// rather than its own totals: Buri has no mutation, so a running total cannot
// live in the struct that reports it.
//
// What is counted is what `core/alloc` says is counted — every `allocate`, at
// the bytes it asked for, which is the cost model's last row. This table is
// not the heap accounting above it and does not want to be: `LIVE_BYTES` is a
// measurement of `malloc`, and a charge is a *definition* evaluated from the
// types (MEMORY.md §7.1), so the two disagree by construction and only one of
// them is the same number on the JavaScript backend.
//
// A `Mutex` rather than a bare `static mut` for the same reason `rng.rs` uses
// one: the language has no threads, so it is never contended, and an
// uncontended lock is cheaper than a soundness argument.

/// One allocator's accounting. A negative `budget` is unbounded.
struct Counter {
    allocations: i64,
    bytes: i64,
    budget: i64,
}

static COUNTERS: std::sync::Mutex<Vec<Counter>> = std::sync::Mutex::new(Vec::new());

/// The table, with a poisoned lock recovered rather than propagated — the
/// language has no threads, so poisoning means this runtime already panicked
/// once and a second failure on top of the first tells nobody anything
/// (`rng.rs` takes the same line).
fn counters<T>(f: impl FnOnce(&mut Vec<Counter>) -> T) -> T {
    let mut guard = match COUNTERS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

/// `core/alloc`'s `newCounter(budget)` — a fresh counter, and its handle.
///
/// Handles are indices and are never reused, so a copy of an allocator value
/// reaches the same counter as the original, which is what `core/alloc` says
/// about copies.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_new_counter(budget: i64) -> i64 {
    counters(|c| {
        c.push(Counter { allocations: 0, bytes: 0, budget });
        // A `usize` index that does not fit an `i64` is a table with 2^63
        // entries, which is not reachable: every entry costs 24 bytes.
        i64::try_from(c.len().saturating_sub(1)).unwrap_or(i64::MAX)
    })
}

/// `core/alloc`'s `charge(handle, bytes)` — add, then answer what was added.
///
/// A bounded counter is checked *before* the charge lands, so the totals a
/// program can observe never include the request that ended the process. The
/// abort happens after the lock is released, because
/// [`buri_rt_abort_alloc_budget`] does not return.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_charge(handle: i64, bytes: i64) -> i64 {
    let over = counters(|c| {
        let Some(entry) = index(handle).and_then(|i| c.get_mut(i)) else {
            return false;
        };
        if entry.budget >= 0 && entry.bytes.saturating_add(bytes) > entry.budget {
            return true;
        }
        entry.allocations = entry.allocations.saturating_add(1);
        entry.bytes = entry.bytes.saturating_add(bytes);
        false
    });
    if over {
        let budget = counters(|c| index(handle).and_then(|i| c.get(i)).map_or(0, |e| e.budget));
        buri_rt_abort_alloc_budget(bytes, budget);
    }
    bytes
}

/// `core/alloc`'s `count(handle)`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_count(handle: i64) -> i64 {
    counters(|c| index(handle).and_then(|i| c.get(i)).map_or(0, |e| e.allocations))
}

/// `core/alloc`'s `total(handle)`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_total(handle: i64) -> i64 {
    counters(|c| index(handle).and_then(|i| c.get(i)).map_or(0, |e| e.bytes))
}

/// A handle as a table index, or `None` for one this runtime never issued.
///
/// Unreachable from a Buri program — the handle is private to `core/alloc` and
/// only `newCounter` produces one — so the answer for an invalid handle is
/// chosen to be *quiet* rather than fatal: a zero total is a wrong number, and
/// an abort here would be a runtime that crashes on a value the language says
/// cannot exist.
fn index(handle: i64) -> Option<usize> {
    usize::try_from(handle).ok()
}

// ---------------------------------------------------------------------------
// The per-carrier Buri data stack (design/native track B, slice B7)
// ---------------------------------------------------------------------------
//
// A stencil artifact runs on **two** stacks. The machine stack is the OS's and
// the kernel guards it. The *Buri* stack is the one `middle::layout` addresses
// every local against, it grows **upward** from a base a caller is handed in
// `x0`/`rdi`, and nothing in the kernel knows it exists.
//
// Until this slice there was exactly one of them: a 64 MiB `__bss` symbol
// (`backend/stencil/asm.rs`'s `buri$stencil$stack`) with a 1 MiB `PROT_NONE`
// guard `main` installs once. One block is right for one thread and wrong for
// two — a second carrier entering Buri code would write its frames into the
// first carrier's, and the fault a runaway recursion is *supposed* to take at
// the guard would instead be a silent overwrite of somebody else's locals.
//
// So a carrier asks for its own. `main` does not: it keeps the static block,
// which is why nothing about a single-threaded program's startup moves — no
// `mmap`, no page faulted in, no bytes in the executable.

/// How much Buri stack one carrier may use: 64 MiB.
///
/// **The same number as `backend/stencil/asm.rs`'s `STACK_USABLE`, and it has
/// to be.** A program's frames are the same size whichever stack they land on,
/// so a carrier that got less than the process's own would fault at a depth
/// the process's own survives — a concurrency bug that reads as a stack bug.
/// The two constants are in two crates because the compiler cannot link the
/// runtime, and each one's test names the number so a change to either is a
/// failure rather than a divergence.
pub const BURI_RT_STACK_USABLE: usize = 64 * 1024 * 1024;

/// The guard above the usable stack, turned `PROT_NONE`: 1 MiB.
///
/// Above, not below, for `asm.rs`'s reason — this stack grows upward, so the
/// address a runaway recursion reaches first is the top of the block. A
/// megabyte rather than a page for the other of `asm.rs`'s reasons: a guard
/// narrower than the widest frame can be *stepped over*.
pub const BURI_RT_STACK_GUARD: usize = 1024 * 1024;

/// The whole reservation: the usable stack, then the guard.
pub const BURI_RT_STACK_BYTES: usize = BURI_RT_STACK_USABLE + BURI_RT_STACK_GUARD;

/// The alignment a block is mapped at: 16 KiB, `asm.rs`'s `STACK_ALIGN`.
///
/// Two requirements and the second is the larger: every frame offset is
/// computed from a base of zero so the base must satisfy the widest alignment
/// a layout asks for (sixteen), and [`mprotect`] wants a page and arm64 macOS
/// pages are 16 KiB. `mmap` already answers page-aligned on both platforms;
/// asserting it is what makes that an observed fact rather than an assumption.
pub const BURI_RT_STACK_ALIGN: usize = 16 * 1024;

/// **The retained set**: how much of an idle carrier stack keeps its pages.
/// 256 KiB — sixteen of arm64 macOS's pages, sixty-four of everyone else's.
///
/// This is the whole of G6's hysteresis on the stack side, and it is stated as
/// a *prefix* rather than as a count of blocks because that is the shape the
/// churn has. A carrier that enters Buri code, returns, and enters again
/// touches the bottom of its stack and nothing else; if the decommit started
/// at the base, every one of those entries would give back pages the next
/// entry immediately faults in again — the ping-pong the row warns about. With
/// a retained prefix, the range handed back is a range a shallow entry never
/// touched, so **no page this policy releases is ever a page that gets
/// refaulted** on such a workload, and what the release costs is the system
/// call and nothing else.
///
/// The number is a frame-depth judgement: 256 KiB is far more than the frames
/// of an ordinary entry, and a rounding error against the 64 MiB a deep
/// recursion is allowed to reach. A carrier that goes deeper than this has by
/// definition done something big enough that one system call afterwards is not
/// the cost worth optimising.
pub const BURI_RT_STACK_WARM: usize = 256 * 1024;

/// The eight bytes written at `base + BURI_RT_STACK_WARM`, and read back on
/// release to ask whether the entry that just finished went past the retained
/// prefix at all.
///
/// **This is the second half of the hysteresis, and it is what keeps a shallow
/// entry free.** The retained prefix means no page a shallow entry touches is
/// ever released; the watermark means no *system call* is made for one either.
/// Measured, one `acquire`/`release` pair around a one-byte entry:
///
/// | | per pair |
/// |---|---|
/// | before this slice | 0.004 µs |
/// | decommit on every release | 0.391 µs |
/// | watermark, [`STACK_DECOMMIT_EVERY`] disabled | 0.005 µs |
/// | **watermark and the floor, this slice** | 0.014 µs |
///
/// **Its failure mode is a missed decommit, never a wrong answer.** A frame
/// that straddles the watermark and happens not to write these particular
/// eight bytes leaves them intact, and the release skips a decommit it could
/// have made; nothing is freed early and nothing is corrupted, because the
/// only thing the word decides is whether to give back pages of a block that
/// is empty either way. [`STACK_DECOMMIT_EVERY`] is the floor under that.
const BURI_RT_STACK_WATERMARK: u64 = 0x6275_7269_5f77_6d6b;

/// How many releases a carrier may make before one of them decommits whatever
/// the watermark said.
///
/// The floor under [`BURI_RT_STACK_WATERMARK`]'s failure mode, and the reason
/// the policy has a guarantee rather than a tendency: a carrier gives its
/// stack's pages back at least once every 1024 entries into Buri code,
/// whatever the probe thought.
///
/// **The number is the amortisation.** A decommit measured inside this
/// runtime's own multi-threaded test binary is **8.4 µs**, not the 0.4 µs a
/// single-threaded C program sees — a `MAP_FIXED` re-map has to shoot down
/// every other thread's TLB, so the cost is a property of how many carriers
/// the process is running rather than of the range. One in 1024 puts that at
/// **8 ns** on an entry that otherwise costs 5, which is a rounding error
/// against anything that crosses this door; one in 256, measured, put it at
/// 52 ns, which is not.
const STACK_DECOMMIT_EVERY: u32 = 1024;

// The three calls this file makes into the C library, declared rather than
// depended on.
//
// `cli/runtime/manifest.toml`'s dependency set is closed by an exact list and
// asserted as an equality, so `libc` is not a thing this crate may add for
// three declarations. Declaring them here is the same thing the *compiler*
// already does from the other side: `backend/stencil/asm.rs` emits a `bl` to
// `mprotect` and to `abort` by name into every artifact, so both symbols are
// already part of what a Buri program links against.
//
// Unix only, which is what this runtime is: `ARCHITECTURE.md` §9 admits
// `Linux` and `Macos` and refuses cross-linking, and `cli/build.rs` builds
// the archive for the host triple alone.
unsafe extern "C" {
    fn mmap(
        addr: *mut core::ffi::c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut core::ffi::c_void;
    fn mprotect(addr: *mut core::ffi::c_void, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut core::ffi::c_void, len: usize) -> i32;
}

const PROT_NONE: i32 = 0;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_PRIVATE: i32 = 0x2;
/// Replace whatever is mapped over the range rather than picking a free one.
/// `0x10` on both platforms, and it is how [`decommit_stack`] gives an idle
/// carrier stack's pages back without giving back its guard.
const MAP_FIXED: i32 = 0x10;
/// `MAP_ANONYMOUS`, whose value is the one thing here that is not the same
/// number on both platforms.
#[cfg(target_os = "macos")]
const MAP_ANON: i32 = 0x1000;
#[cfg(not(target_os = "macos"))]
const MAP_ANON: i32 = 0x20;
const MAP_FAILED: usize = usize::MAX;

thread_local! {
    /// The blocks this carrier has mapped and is not currently inside.
    ///
    /// A **free list rather than a single pointer**, because entries nest: an
    /// entry thunk that calls Buri code which calls out and comes back in
    /// needs a second block, and handing it the first would put the inner
    /// frames on top of the outer ones' live locals. A list makes the nested
    /// case correct at the price of address space, which is the price this
    /// whole mechanism is already paying.
    ///
    /// Thread-local, so no lock is taken on the path a carrier actually runs,
    /// and so a thread that ends takes its blocks with it: the destructor
    /// unmaps them.
    static STACKS: core::cell::RefCell<Blocks> =
        const { core::cell::RefCell::new(Blocks { idle: Vec::new(), since_decommit: 0 }) };
}

/// A carrier's idle blocks, unmapped when the thread ends, and the release
/// counter [`STACK_DECOMMIT_EVERY`] is measured against.
struct Blocks {
    idle: Vec<*mut u8>,
    since_decommit: u32,
}

impl Drop for Blocks {
    fn drop(&mut self) {
        for p in self.idle.drain(..) {
            // SAFETY: every pointer in this list came from `map_stack` and was
            // mapped with exactly this length; a block that is *in* the list is
            // one no entry thunk is inside.
            unsafe {
                munmap(p.cast(), BURI_RT_STACK_BYTES);
            }
        }
    }
}

/// Maps one 64 MiB stack with its own 1 MiB `PROT_NONE` guard on top.
///
/// Zero-filled by the kernel and never faulted in until a frame lands on a
/// page, so the cost of a carrier that enters Buri code once and returns is
/// two system calls and the pages it actually used.
fn map_stack() -> *mut u8 {
    // SAFETY: a fresh anonymous private mapping; no fd, no fixed address.
    let p = unsafe {
        mmap(
            core::ptr::null_mut(),
            BURI_RT_STACK_BYTES,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON,
            -1,
            0,
        )
    };
    if p.is_null() || p as usize == MAP_FAILED {
        buri_rt_abort_oom(BURI_RT_STACK_BYTES as u64);
    }
    // `mmap` answers page-aligned and a page is at least 4 KiB, but the frame
    // offsets this block is addressed with were computed against `STACK_ALIGN`
    // and the guard below has to start on a page. Both are the same question
    // and this is where it is asked.
    assert!(
        (p as usize).is_multiple_of(BURI_RT_STACK_ALIGN),
        "mmap answered a block that is not 16 KiB aligned"
    );
    // SAFETY: `p` names `BURI_RT_STACK_BYTES` of this process's own mapping and
    // the guard is the top `BURI_RT_STACK_GUARD` of it, which is a whole
    // number of pages at a page boundary.
    let rc = unsafe { mprotect(p.wrapping_add(BURI_RT_STACK_USABLE).cast(), BURI_RT_STACK_GUARD, PROT_NONE) };
    assert!(rc == 0, "a carrier stack could not be given its guard");
    let base: *mut u8 = p.cast();
    // SAFETY: `base + WARM` is inside the usable range and 8-byte aligned.
    unsafe { arm_watermark(base) };
    base
}

/// Writes [`BURI_RT_STACK_WATERMARK`] at `base + BURI_RT_STACK_WARM`, which
/// costs the one page it lands on and is what makes the next release's
/// question answerable without a system call.
///
/// # Safety
/// `base` names a live carrier stack block.
unsafe fn arm_watermark(base: *mut u8) {
    // SAFETY: the caller promises a live block, and `BURI_RT_STACK_WARM` is a
    // multiple of the page size and therefore of eight.
    unsafe { base.add(BURI_RT_STACK_WARM).cast::<u64>().write(BURI_RT_STACK_WATERMARK) }
}

/// Whether the entry that just finished left the watermark alone — that is,
/// whether it stayed inside the retained prefix.
///
/// # Safety
/// `base` names a live carrier stack block.
unsafe fn watermark_intact(base: *mut u8) -> bool {
    // SAFETY: as in `arm_watermark`.
    unsafe { base.add(BURI_RT_STACK_WARM).cast::<u64>().read() == BURI_RT_STACK_WATERMARK }
}

/// **This carrier's Buri data stack**: the base a generated entry thunk puts
/// in `x0` (`rdi` on x86-64) before it calls into frame-threaded code.
///
/// The block is this thread's alone and carries its own `PROT_NONE` guard, so
/// a runaway recursion on a carrier faults at *its* boundary rather than
/// walking into whatever the linker placed after the process's static block.
///
/// Answers a block that is **not** in use by any other entry on this thread —
/// nested entries get different blocks — and it is the caller's business to
/// hand the same pointer back to [`buri_rt_stack_release`]. A carrier that
/// enters Buri code repeatedly pays the mapping once: the block goes back on
/// this thread's free list rather than to the kernel.
///
/// `main` never calls this. The process's own carrier keeps the `__bss` block
/// `backend/stencil/asm.rs` emits, which is why a program that never starts a
/// second carrier makes no system call it did not make before.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_stack_acquire() -> *mut u8 {
    let idle = STACKS.with(|s| s.borrow_mut().idle.pop());
    match idle {
        Some(p) => p,
        None => map_stack(),
    }
}

/// **Gives an idle carrier stack's pages back to the kernel** and the block
/// back to this carrier's free list.
///
/// A released block is *fully empty* by construction — no entry thunk is
/// inside it, and nothing above the base is live — so this is the moment
/// MEMORY.md's chunk lifecycle has to answer for. Everything above
/// [`BURI_RT_STACK_WARM`] is decommitted; the retained prefix, the mapping and
/// its `PROT_NONE` guard all survive.
///
/// **Three clauses decide whether this release is one that decommits**, and
/// between them they are the hysteresis the row asks for:
///
/// 1. the entry went past [`BURI_RT_STACK_WARM`], which
///    [`BURI_RT_STACK_WATERMARK`] answers with a load rather than a system
///    call — so an entry that stayed shallow costs 6 ns and not 391;
/// 2. this is not the carrier's *retained* block. The first idle block is the
///    one the next entry gets handed; anything past it belongs to a nested
///    entry and is not worth keeping warm;
/// 3. [`STACK_DECOMMIT_EVERY`] releases have gone by, which is the floor under
///    clause 1's failure mode.
///
/// The address space is **not** unmapped: a carrier is reused
/// (`cli/runtime/rt.rs`'s pool never retires one), so giving back the
/// reservation would cost an `mmap` and an `mprotect` per entry to buy
/// nothing — the reservation is free, and it is the *resident pages* that are
/// not. The mapping goes away when the thread does.
///
/// A null pointer is ignored rather than aborting, for the reason
/// [`buri_rt_decref`]'s null check gives: a released nothing is a caller that
/// never acquired, which is a no-op and not a corruption.
///
/// # Safety
/// `base` is null, or a block from [`buri_rt_stack_acquire`] on **this**
/// thread that no entry thunk is still inside. It became an `unsafe fn` in G6
/// and the reason is the watermark: this function now *reads* the block it is
/// handed, where before it only filed the pointer away, so a pointer that
/// names something else is a read of something else.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_stack_release(base: *mut u8) {
    if base.is_null() {
        return;
    }
    // SAFETY: `base` is a block this carrier is releasing, so it is live and
    // nothing is inside it.
    let deep = !unsafe { watermark_intact(base) };
    let decommit = STACKS.with(|s| {
        let mut blocks = s.borrow_mut();
        blocks.since_decommit += 1;
        // Three clauses, and the middle one is the retained set: the *first*
        // idle block a carrier has is the one its next entry will be handed,
        // and every block after it is a nested entry's, which is rare by
        // construction and not worth keeping warm.
        let go = deep
            || !blocks.idle.is_empty()
            || blocks.since_decommit >= STACK_DECOMMIT_EVERY;
        if go {
            blocks.since_decommit = 0;
        }
        go
    });
    if !decommit || decommit_stack(base) {
        STACKS.with(|s| s.borrow_mut().idle.push(base));
    }
}

/// Hands `[base + WARM, base + USABLE)` back to the kernel, leaving the
/// mapping and the guard in place. `false` means the block was retired instead
/// and must not go back on the free list.
///
/// **Why a fixed anonymous re-map and not `madvise`.** The row says
/// "`madvise(MADV_FREE`/`DONTNEED)` or `munmap` per the allocator's chunk
/// lifecycle", and the honest answer on the two platforms this runtime
/// supports is neither of the first two. Measured on macOS 15, arm64, over a
/// 64 MiB block with 8 MiB dirtied:
///
/// | mechanism | resident afterwards | cost |
/// |---|---|---|
/// | `madvise(MADV_FREE)` | **unchanged**, 9.78 MB | 403 µs |
/// | `madvise(MADV_DONTNEED)` | **unchanged**, 9.78 MB | 111 µs |
/// | `mmap(MAP_FIXED\|MAP_ANON)` | 1.65 MB | 49 µs |
///
/// Darwin's `MADV_FREE` is an *offer*: the pages stay resident and become
/// reclaimable only under memory pressure, so a program that drains and then
/// idles never gets its RSS back — which is precisely the acceptance this
/// slice is measured against — and Darwin's `MADV_DONTNEED` does nothing at
/// all for private anonymous memory. A fixed re-map is the one operation that
/// is a *decommit* rather than a hint, and it means the same thing on Linux,
/// where `MADV_DONTNEED` would also have worked. One mechanism that is exact
/// on both beats two that are exact on one.
///
/// `munmap` is the third option the row offers and it is the wrong one here
/// for [`buri_rt_stack_release`]'s reason: it would take the guard and the
/// reservation with it, and the next entry on this carrier would pay for both
/// again.
///
/// On the ping-pong path the range is already unmapped-in-fact, and the same
/// measurement puts a re-map over a clean range at **0.38 µs** — one system
/// call per *entry into Buri code*, which is a boundary a carrier crosses once
/// per task and not once per call.
fn decommit_stack(base: *mut u8) -> bool {
    let tail = BURI_RT_STACK_USABLE - BURI_RT_STACK_WARM;
    // SAFETY: `[base + WARM, base + USABLE)` is inside the mapping `map_stack`
    // made, is a whole number of pages at a page boundary (both constants are
    // multiples of `BURI_RT_STACK_ALIGN`), and the block is empty — no frame
    // is live in it, so discarding its contents discards nothing.
    let p = unsafe {
        mmap(
            base.wrapping_add(BURI_RT_STACK_WARM).cast(),
            tail,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON | MAP_FIXED,
            -1,
            0,
        )
    };
    if p.is_null() || p as usize == MAP_FAILED {
        // The re-map failed, which on either platform means the range is now
        // in a state this file cannot describe. Retire the whole block rather
        // than hand a caller a stack that may be missing its middle: the next
        // `acquire` maps a fresh one, and the only cost is the reservation.
        //
        // SAFETY: `base` came from `map_stack` and was mapped with exactly
        // this length; it is on no free list, so no entry thunk is inside it.
        unsafe {
            munmap(base.cast(), BURI_RT_STACK_BYTES);
        }
        return false;
    }
    DECOMMITTED_BYTES.fetch_add(tail as u64, Ordering::Relaxed);
    // SAFETY: the mapping is intact and `base + WARM` is the first byte of the
    // range just replaced, so this both re-arms the probe and is the only page
    // of the tail the block goes back to the free list holding.
    unsafe { arm_watermark(base) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The two stack constants are the ones `backend/stencil/asm.rs` emits
    /// against.**
    ///
    /// They live in two crates because the compiler does not link the runtime,
    /// so nothing but a pair of tests naming the same numbers keeps them equal.
    /// The compiler's half is `asm::tests::the_carrier_stack_is_the_size_the_
    /// runtime_maps`.
    #[test]
    fn a_carrier_stack_is_the_size_the_static_block_is() {
        assert_eq!(BURI_RT_STACK_USABLE, 64 * 1024 * 1024);
        assert_eq!(BURI_RT_STACK_GUARD, 1024 * 1024);
        assert_eq!(BURI_RT_STACK_BYTES, 65 * 1024 * 1024);
        assert_eq!(BURI_RT_STACK_ALIGN, 16 * 1024);
        // The guard has to start on a page and be a whole number of them, at
        // the widest page any supported target has.
        assert!(BURI_RT_STACK_USABLE.is_multiple_of(BURI_RT_STACK_ALIGN));
        assert!(BURI_RT_STACK_GUARD.is_multiple_of(BURI_RT_STACK_ALIGN));
        // G6: the retained prefix is where a decommit starts, so it has to be
        // a page boundary too, and it has to leave something to give back.
        assert!(BURI_RT_STACK_WARM.is_multiple_of(BURI_RT_STACK_ALIGN));
        const { assert!(BURI_RT_STACK_WARM < BURI_RT_STACK_USABLE) };
    }

    /// **A carrier's stack is writable to its last usable byte, aligned, and
    /// reused.**
    ///
    /// The write at `USABLE - 1` is the assertion that matters: it is the
    /// address one below the guard, so a block whose guard had been installed
    /// a page low would fault here instead of answering.
    #[test]
    fn a_carrier_stack_is_writable_up_to_its_guard_and_comes_back() {
        let a = buri_rt_stack_acquire();
        assert!(!a.is_null());
        assert!((a as usize).is_multiple_of(BURI_RT_STACK_ALIGN), "not 16 KiB aligned");
        // SAFETY: `a` names `BURI_RT_STACK_USABLE` writable bytes.
        unsafe {
            a.write(0xab);
            a.add(BURI_RT_STACK_USABLE - 1).write(0xcd);
            assert_eq!(a.read(), 0xab);
            assert_eq!(a.add(BURI_RT_STACK_USABLE - 1).read(), 0xcd);
        }
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(a) };
        // The *mapping* is kept — only the pages above the warm prefix go back
        // (G6) — so the next acquire on this thread is the same block at the
        // same address, and it is still writable to its last usable byte.
        let b = buri_rt_stack_acquire();
        assert_eq!(a, b, "a released block was not reused");
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(b) };
    }

    /// **Nested entries get different blocks.**
    ///
    /// The case a single per-thread pointer would get wrong: an entry thunk
    /// inside another one would be handed the outer entry's base and would
    /// write its frames over the outer frames' live locals.
    #[test]
    fn a_nested_acquire_is_a_different_block() {
        let outer = buri_rt_stack_acquire();
        let inner = buri_rt_stack_acquire();
        assert_ne!(outer, inner, "a nested entry was handed the outer entry's stack");
        // Neither overlaps the other, which is the property the frames rest on.
        let (lo, hi) = if outer < inner { (outer, inner) } else { (inner, outer) };
        assert!(
            (hi as usize) - (lo as usize) >= BURI_RT_STACK_BYTES,
            "two live carrier stacks overlap"
        );
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(inner) };
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(outer) };
    }

    /// **Two carriers get two stacks**, which is the whole reason this exists.
    #[test]
    fn two_carriers_do_not_share_a_stack() {
        let mine = buri_rt_stack_acquire();
        let theirs = std::thread::spawn(|| {
            let p = buri_rt_stack_acquire();
            // SAFETY: a block this test acquired on this thread and is not inside.
            unsafe { buri_rt_stack_release(p) };
            p as usize
        })
        .join()
        .expect("the second carrier panicked");
        assert_ne!(mine as usize, theirs, "two carriers were handed one stack");
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(mine) };
    }

    /// **Releasing nothing is nothing**, for `buri_rt_decref`'s reason.
    #[test]
    fn releasing_a_null_stack_is_a_no_op() {
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(core::ptr::null_mut()) };
    }


    /// The reserved bit is bit 63, the mask is its complement, and neither
    /// touches a capacity a block can have.
    #[test]
    fn the_shared_bit_is_the_top_bit_of_the_capacity() {
        assert_eq!(BURI_RT_CAP_SHARED, 1 << 63);
        assert_eq!(BURI_RT_CAP_MASK, u64::MAX >> 1);
        assert_eq!(BURI_RT_CAP_SHARED & BURI_RT_CAP_MASK, 0);
    }

    /// A real block's capacity round-trips through the header at the three
    /// boundaries — empty, the largest capacity the low 63 bits can spell, and
    /// that capacity with the reserved bit forced on — and the readers answer
    /// the byte count in every case.
    ///
    /// The largest capacity is written into the header directly rather than
    /// allocated: the point of the test is the *encoding*, and asking `malloc`
    /// for 2^63 - 1 bytes is a different question.
    #[test]
    fn a_masked_capacity_round_trips_at_the_boundaries() {
        for cap in [0u64, 1, BURI_RT_GROWTH_FLOOR, BURI_RT_CAP_MASK] {
            for flag in [0u64, BURI_RT_CAP_SHARED] {
                let word = cap | flag;
                assert_eq!(word & BURI_RT_CAP_MASK, cap);
                // The growth policy reads a header word and must not double a
                // flag: `2 * (2^63 - 1)` saturates, `2 * flag` would too.
                assert_eq!(
                    buri_rt_grown_capacity(1, word),
                    buri_rt_grown_capacity(1, cap),
                    "the growth policy read the reserved bit",
                );
            }
        }
    }

    /// The same, through an allocated block: `buri_rt_cap` and
    /// `buri_rt_unique_cap` both answer the byte count with the bit on, and the
    /// free path hands the allocator the layout it was given.
    #[test]
    fn the_readers_mask_a_live_block() {
        // This case allocates a block and says what its mark is, so it must
        // not run beside a case that sets the marking latch. `latch` names
        // both sides of that rule.
        let _latch = latch();
        for payload in [0u64, 1, BURI_RT_GROWTH_FLOOR] {
            let p = buri_rt_alloc(payload);
            // SAFETY: `p` is a fresh live payload pointer from this crate.
            unsafe {
                assert_eq!(buri_rt_cap(p), payload);
                assert_eq!(buri_rt_unique_cap(p), Some(payload));
                // Force the bit on, as G3's latch does for a whole program,
                // and read again.
                (*header(p)).cap |= BURI_RT_CAP_SHARED;
                assert_eq!(buri_rt_cap(p), payload, "buri_rt_cap read the reserved bit");
                // **G3 changed this one line, and it is the only reader whose
                // answer the bit is supposed to change.** `buri_rt_cap` still
                // reports the byte count, `buri_rt_free` still recovers the
                // layout, `buri_rt_grown_capacity` still doubles the capacity —
                // masking, all three. The uniqueness test is not a byte count:
                // it is the in-place-write licence, and a block a second
                // carrier may reach does not get one. See `buri_rt_unique_cap`.
                assert_eq!(
                    buri_rt_unique_cap(p),
                    None,
                    "a block a second carrier may reach was handed an in-place write",
                );
                // `buri_rt_free` recovers `layout_for(cap)`, so a block freed
                // with the bit on must reach the allocator with the same layout
                // it was created with. Miri and the system allocator both
                // notice if it does not.
                buri_rt_free(p);
            }
        }
    }

    // === G2 begin: the atomic arm, and the per-thread cache ================

    /// The atomic increment saturates, exactly as the unshared one does.
    ///
    /// This is the property `layout::CAP_SHARED_FLAG`'s doc says the bit was
    /// put in `cap` to preserve, checked on the side that could lose it: a
    /// plain `fetch_add(1)` on `u64::MAX` wraps to zero, and the next `decref`
    /// would free a string literal.
    #[test]
    fn the_atomic_increment_keeps_immortal_a_fixed_point() {
        // This case allocates a block and says what its mark is, so it must
        // not run beside a case that sets the marking latch. `latch` names
        // both sides of that rule.
        let _latch = latch();
        let p = buri_rt_alloc(32);
        // SAFETY: `p` is this test's only reference to a live block.
        unsafe {
            (*header(p)).cap |= BURI_RT_CAP_SHARED;
            (*header(p)).rc = BURI_RT_IMMORTAL;
            buri_rt_incref(p);
            assert_eq!(buri_rt_rc(p), BURI_RT_IMMORTAL, "the atomic increment wrapped IMMORTAL");
            // And a decrement of one neither moves it nor frees it.
            buri_rt_decref(p, None);
            assert_eq!(buri_rt_rc(p), BURI_RT_IMMORTAL, "the atomic decrement moved IMMORTAL");
            // Freed by hand: an IMMORTAL block is `buri_rt_free`'s no-op.
            (*header(p)).rc = 1;
            buri_rt_free(p);
        }
    }

    /// The atomic arm counts: two increments, three decrements, and the block
    /// dies on the one that took the count to zero — not before, not twice.
    #[test]
    fn the_atomic_count_frees_on_the_last_decrement() {
        // This case allocates a block and says what its mark is, so it must
        // not run beside a case that sets the marking latch. `latch` names
        // both sides of that rule.
        let _latch = latch();
        static DROPPED: AtomicU64 = AtomicU64::new(0);
        extern "C" fn glue(_: *mut u8) {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        DROPPED.store(0, Ordering::Relaxed);
        let p = buri_rt_alloc(48);
        // SAFETY: `p` is this test's only reference to a live block.
        unsafe {
            (*header(p)).cap |= BURI_RT_CAP_SHARED;
            buri_rt_incref(p);
            buri_rt_incref(p);
            assert_eq!(buri_rt_rc(p), 3);
            buri_rt_decref(p, Some(glue));
            buri_rt_decref(p, Some(glue));
            assert_eq!(buri_rt_rc(p), 1, "an atomic decrement did not land");
            assert_eq!(DROPPED.load(Ordering::Relaxed), 0, "the glue ran while the block lived");
            buri_rt_decref(p, Some(glue));
            assert_eq!(DROPPED.load(Ordering::Relaxed), 1, "the glue did not run exactly once");
        }
    }

    /// **A marked block is never unique, at any count.** G2's
    /// `a_shared_block_is_still_unique_at_a_count_of_one`, inverted, which is
    /// the one behavioural invariant of that slice G3 had to change.
    ///
    /// G2 left the test unforked on an argument with a premise — *a thread
    /// holding no reference cannot make a second one, so a count of one cannot
    /// move under the caller who read it* — and the run baton was what kept
    /// that premise true. A borrowed parameter aliases somebody else's
    /// reference rather than adding one, so with the baton gone a step of a
    /// `Tasks.parallel` reading `rc == 1` off its closure's list is one of
    /// several carriers reading the same `1`. G2's own handover note said this
    /// function would need a second look on the day the bit was live; this is
    /// that look, taken in the over-set direction MEMORY.md §5.5 names.
    ///
    /// The count is still the *other* half of the test, and the case asserts
    /// both: a marked block fails at one and at two, and an unmarked one still
    /// distinguishes them.
    #[test]
    fn a_marked_block_is_never_unique_at_any_count() {
        // This case allocates a block and says what its mark is, so it must
        // not run beside a case that sets the marking latch. `latch` names
        // both sides of that rule.
        let _latch = latch();
        let p = buri_rt_alloc(64);
        // SAFETY: `p` is this test's only reference to a live block.
        unsafe {
            // Unmarked, the count decides, exactly as it always did.
            assert_eq!(buri_rt_unique_cap(p), Some(64));
            buri_rt_incref(p);
            assert_eq!(buri_rt_unique_cap(p), None);
            buri_rt_decref(p, None);
            assert_eq!(buri_rt_unique_cap(p), Some(64));

            // Marked, the count is not consulted.
            (*header(p)).cap |= BURI_RT_CAP_SHARED;
            assert_eq!(buri_rt_unique_cap(p), None, "a marked block at one passed the test");
            buri_rt_incref(p);
            assert_eq!(buri_rt_unique_cap(p), None, "a marked block at two passed the test");
            buri_rt_decref(p, None);
            assert_eq!(buri_rt_unique_cap(p), None);
            (*header(p)).cap &= BURI_RT_CAP_MASK;
            buri_rt_free(p);
        }
    }

    /// A freed small block comes back from this thread's cache, with the
    /// **same capacity** it was asked for.
    ///
    /// The capacity is the assertion that matters: a cache that rounded up to
    /// a size class would hand back a `cap` larger than the payload, and the
    /// release glue of a `[T]` walks `cap / stride` slots.
    #[test]
    fn the_cache_returns_a_block_of_exactly_the_capacity_asked_for() {
        for payload in [0u64, 1, 24, 64, CACHE_MAX_PAYLOAD] {
            let first = buri_rt_alloc(payload);
            // SAFETY: `first` is this test's only reference to a live block.
            unsafe {
                assert_eq!(buri_rt_cap(first), payload);
                buri_rt_free(first);
            }
            let second = buri_rt_alloc(payload);
            // SAFETY: as above.
            unsafe {
                assert_eq!(second, first, "the cache did not hand the block back");
                assert_eq!(buri_rt_cap(second), payload, "the cache changed the capacity");
                assert_eq!(buri_rt_rc(second), 1, "a recycled block did not start at one");
                buri_rt_free(second);
            }
        }
    }

    /// A block above the ceiling is returned to the allocator rather than
    /// cached, and a `alloc_zeroed` over a cached block is still zeroed.
    #[test]
    fn the_cache_has_a_ceiling_and_still_zeroes() {
        let big = buri_rt_alloc(CACHE_MAX_PAYLOAD + 1);
        // SAFETY: `big` is this test's only reference to a live block.
        unsafe {
            buri_rt_free(big);
        }

        let dirty = buri_rt_alloc(32);
        // SAFETY: `dirty` is this test's only reference to a live block, with
        // 32 usable bytes.
        unsafe {
            std::ptr::write_bytes(dirty, 0xAB, 32);
            buri_rt_free(dirty);
        }
        let clean = buri_rt_alloc_zeroed(32);
        // SAFETY: as above.
        unsafe {
            assert_eq!(clean, dirty, "the cache did not hand the block back to `alloc_zeroed`");
            for i in 0..32 {
                assert_eq!(*clean.add(i), 0, "byte {i} of a recycled block was not zeroed");
            }
            buri_rt_free(clean);
        }
    }

    /// A carrier's share of the cache is the process budget divided by the
    /// carriers, with a floor — the "sized for carrier count" of MEMORY.md
    /// §5.4.
    #[test]
    fn the_cache_budget_is_a_share_of_one_process_wide_number() {
        let b = cache_budget();
        assert!(b >= CACHE_BYTES_FLOOR, "a carrier's share fell below the floor: {b}");
        assert!(b <= CACHE_BYTES, "a carrier's share exceeded the whole budget: {b}");
        assert_eq!(b, cache_budget(), "the budget is not stable across calls");
    }

    // === G2 end ============================================================

    /// `realloc` preserves the reserved bit rather than clearing it, so that
    /// growing a block does not silently un-share it.
    #[test]
    fn realloc_preserves_the_reserved_bit() {
        // This case allocates a block and says what its mark is, so it must
        // not run beside a case that sets the marking latch. `latch` names
        // both sides of that rule.
        let _latch = latch();
        let p = buri_rt_alloc(16);
        // SAFETY: `p` is this test's only reference to a live block.
        let p = unsafe {
            (*header(p)).cap |= BURI_RT_CAP_SHARED;
            buri_rt_realloc(p, 64)
        };
        // SAFETY: `p` is the live block `realloc` answered.
        unsafe {
            assert_eq!((*header(p)).cap & BURI_RT_CAP_SHARED, BURI_RT_CAP_SHARED);
            assert_eq!(buri_rt_cap(p), 64);
            buri_rt_free(p);
        }
    }

    // === G6 begin: decommit ================================================

    /// What this thread's block cache is holding, exactly.
    ///
    /// The process-wide `retained_bytes` cannot answer a question about *one*
    /// carrier while the test harness is running other tests on other threads,
    /// and the convergence claim below is a per-carrier claim.
    fn cache_held() -> u64 {
        // SAFETY: this thread's own cell, and the reference does not escape.
        CACHE.with(|c| unsafe { (*c.get()).held })
    }

    /// This process's resident set in kibibytes, or `None` where `ps` is not
    /// there to ask. `ps -o rss=` is POSIX and reports KiB on both platforms
    /// this runtime supports.
    fn rss_kib() -> Option<u64> {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }

    /// **The stats struct is six words in this order**, which is an ABI two
    /// hand-written C declarations mirror.
    ///
    /// `cli/tests/native/driver.c` and `cli/tests/native/shared.rs`'s
    /// `ALLOC_PROBE` each declare it by hand, and `buri_rt_heap_stats` writes
    /// the whole of it, so a field added here and not there is a write past
    /// the end of their stack slot. This test is the thing that fails when the
    /// three drift.
    #[test]
    fn the_heap_stats_struct_is_six_words_in_one_order() {
        assert_eq!(std::mem::size_of::<BuriHeapStats>(), 6 * 8);
        assert_eq!(std::mem::align_of::<BuriHeapStats>(), 8);
        let s = BuriHeapStats {
            live_blocks: 1,
            live_bytes: 2,
            total_blocks: 3,
            total_bytes: 4,
            retained_bytes: 5,
            decommitted_bytes: 6,
        };
        // SAFETY: `BuriHeapStats` is `repr(C)` and every field is a `u64`, so
        // it is six `u64`s in declaration order and reading it as such is
        // exactly what the C mirrors do.
        let words = unsafe { std::slice::from_raw_parts(std::ptr::from_ref(&s).cast::<u64>(), 6) };
        assert_eq!(words, &[1, 2, 3, 4, 5, 6]);
    }

    /// **A burst and a drain leave the cache holding a sweep period, not a
    /// budget.**
    ///
    /// This is the acceptance row's heap half. Before G6 the free lists only
    /// ever grew — a program that allocated a hundred thousand blocks and gave
    /// them all back kept `cache_budget()` bytes of them for the rest of the
    /// process, because nothing in the cache could shrink. The decay sweep
    /// makes the residue a property of the *sweep period* instead: after the
    /// first sweep a slot that is only being pushed into keeps at most one
    /// period's pushes, so two periods is the bound including whatever the
    /// last, unswept period accumulated.
    ///
    /// The leak counters are asserted unmoved on the same reading, because the
    /// point of the retained/live split is that returning cached blocks to the
    /// allocator is invisible to every number `cli/tests/native` reads.
    #[test]
    fn a_drained_cache_gives_its_blocks_back() {
        const PAYLOAD: u64 = 64;
        const N: usize = 20_000;

        let mut before = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
        };
        // SAFETY: a writable, aligned destination.
        unsafe { buri_rt_heap_stats(&raw mut before) };

        let mut held = Vec::with_capacity(N);
        for _ in 0..N {
            held.push(buri_rt_alloc(PAYLOAD));
        }
        for p in held {
            // SAFETY: each pointer is a live block this test holds alone.
            unsafe { buri_rt_free(p) };
        }

        let bound = 2 * u64::from(CACHE_SWEEP_OPS) * slot_bytes(PAYLOAD);
        let residue = cache_held();
        assert!(
            residue <= bound,
            "a drained cache kept {residue} bytes; the sweep bounds it at {bound}"
        );

        let mut after = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
        };
        // SAFETY: as above.
        unsafe { buri_rt_heap_stats(&raw mut after) };
        // Every counter here is *process*-wide and the harness is running
        // other tests on other threads, so what can be asserted about them is
        // what is true regardless of what else is allocating: this burst was
        // counted. The equality — that a cache hit is indistinguishable from a
        // `malloc` in these four numbers — is G2's property and is asserted
        // end-to-end by `cli/tests/native`'s allocation counts, where the
        // program under measurement is the only thing running.
        assert!(
            after.total_blocks >= before.total_blocks + N as u64,
            "a cache hit must still count as an allocation the program asked for"
        );
        assert!(after.total_bytes >= before.total_bytes + N as u64 * PAYLOAD);
    }

    /// **A carrier that ends gives its whole cache back**, asserted through
    /// `buri_rt_heap_stats`.
    ///
    /// Before G6 a thread's free lists died with its thread-local storage and
    /// every block in them was lost to the process for good — invisible to the
    /// leak counters, because `buri_rt_free` had already decremented them, and
    /// permanent. Sixteen carriers each filling a cache is a signal of
    /// megabytes against the kilobytes any concurrently running test in this
    /// crate can be holding, which is what makes a process-wide counter
    /// assertable here at all.
    #[test]
    fn a_carrier_that_ends_gives_its_cache_back() {
        const CARRIERS: usize = 16;
        // Under one sweep period each, so the *exit* rule is what is being
        // measured rather than the decay rule.
        const PER_CARRIER: usize = 1_000;
        const PAYLOAD: u64 = 248;

        let mut base = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
        };
        // SAFETY: a writable, aligned destination.
        unsafe { buri_rt_heap_stats(&raw mut base) };

        let carriers: Vec<_> = (0..CARRIERS)
            .map(|_| {
                std::thread::spawn(|| {
                    let mut blocks = Vec::with_capacity(PER_CARRIER);
                    for _ in 0..PER_CARRIER {
                        blocks.push(buri_rt_alloc(PAYLOAD));
                    }
                    for p in blocks {
                        // SAFETY: a live block this thread holds alone.
                        unsafe { buri_rt_free(p) };
                    }
                    cache_held()
                })
            })
            .collect();
        let filled: u64 = carriers.into_iter().map(|c| c.join().expect("a carrier panicked")).sum();

        // Each carrier was holding at least its floor share, so the signal is
        // megabytes; if it were not, the assertion below would prove nothing.
        assert!(
            filled > 4 * CACHE_BYTES_FLOOR,
            "the carriers only cached {filled} bytes between them; nothing was measured"
        );

        let mut after = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
        };
        // SAFETY: as above.
        unsafe { buri_rt_heap_stats(&raw mut after) };
        assert!(
            after.retained_bytes <= base.retained_bytes + CACHE_BYTES_FLOOR,
            "sixteen ended carriers left {} bytes cached, up from {}; \
             their lists were {filled} bytes",
            after.retained_bytes,
            base.retained_bytes
        );
    }

    /// **A ping-pong workload pays nothing for the decay.**
    ///
    /// The one property the row asks for by name. A slot that is popped as
    /// often as it is pushed has its watermark driven to zero on the first pop
    /// of every period, so a sweep returns nothing and the same block comes
    /// back every time — no `dealloc`, no `malloc`, no churn, however many
    /// sweep periods the loop runs through.
    #[test]
    fn a_ping_pong_workload_keeps_its_block_through_every_sweep() {
        const PAYLOAD: u64 = 32;
        let first = buri_rt_alloc(PAYLOAD);
        // SAFETY: a live block this test holds alone.
        unsafe { buri_rt_free(first) };
        for i in 0..(4 * CACHE_SWEEP_OPS) {
            let p = buri_rt_alloc(PAYLOAD);
            assert_eq!(p, first, "the cache gave up its block on iteration {i}");
            // SAFETY: as above.
            unsafe { buri_rt_free(p) };
        }
        assert_eq!(cache_held(), slot_bytes(PAYLOAD), "the cache is not holding exactly the one block");
        // SAFETY: drain it so the block does not outlive the test's thread on
        // a list nothing else will look at.
        unsafe { buri_rt_free(buri_rt_alloc(PAYLOAD)) };
    }

    /// **An idle carrier stack gives its pages back, and the resident set says
    /// so.**
    ///
    /// This is the acceptance row's stack half, and it is the one measured
    /// against the operating system rather than against a counter of this
    /// file's own: a deep excursion dirties 48 MiB, and after the release the
    /// process is holding tens of megabytes less. Both halves are asserted —
    /// the counter, which is exact, and the resident set, with a margin wide
    /// enough that no other test in this crate can close it.
    #[test]
    fn an_idle_carrier_stack_gives_its_pages_back() {
        const DIRTY: usize = 48 * 1024 * 1024;

        let mut before = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
        };
        // SAFETY: a writable, aligned destination.
        unsafe { buri_rt_heap_stats(&raw mut before) };

        let base = buri_rt_stack_acquire();
        // SAFETY: `base` names `BURI_RT_STACK_USABLE` writable bytes, and the
        // range written is inside it.
        unsafe {
            let mut off = BURI_RT_STACK_WARM;
            while off < BURI_RT_STACK_WARM + DIRTY {
                base.add(off).write(0x5a);
                off += 4096;
            }
            base.write(0xa5);
            base.add(BURI_RT_STACK_WARM - 1).write(0xa5);
        }
        let peak = rss_kib();
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(base) };
        let idle = rss_kib();

        let mut after = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
        };
        // SAFETY: as above.
        unsafe { buri_rt_heap_stats(&raw mut after) };
        let tail = (BURI_RT_STACK_USABLE - BURI_RT_STACK_WARM) as u64;
        let decommitted = after.decommitted_bytes - before.decommitted_bytes;
        assert!(
            decommitted >= tail,
            "a released stack did not report the range it gave back"
        );
        // Process-wide, so another test's release may be in the delta; what is
        // exact is that every contribution to it is one whole tail.
        assert_eq!(decommitted % tail, 0, "a release reported a partial range");

        if let (Some(peak), Some(idle)) = (peak, idle) {
            assert!(
                peak >= idle + 24 * 1024,
                "the resident set went {peak} KiB -> {idle} KiB across a release of 48 MiB of \
                 dirty stack; the pages were not given back"
            );
        }

        // The block is still usable, and the two halves of it read the way the
        // policy says they should: the retained prefix kept what was written
        // in it, and the decommitted tail came back zero-filled.
        let again = buri_rt_stack_acquire();
        assert_eq!(again, base, "a decommitted block was not the one handed back");
        // SAFETY: `again` is the live block just acquired.
        unsafe {
            assert_eq!(again.read(), 0xa5, "the retained prefix lost its first byte");
            assert_eq!(
                again.add(BURI_RT_STACK_WARM - 1).read(),
                0xa5,
                "the retained prefix lost its last byte"
            );
            assert!(
                watermark_intact(again),
                "a decommitted block came back with its watermark unarmed"
            );
            assert_eq!(
                again.add(BURI_RT_STACK_WARM + 4096).read(),
                0,
                "the decommitted tail kept its contents"
            );
            assert_eq!(
                again.add(BURI_RT_STACK_USABLE - 1).read(),
                0,
                "the byte below the guard kept its contents"
            );
            again.add(BURI_RT_STACK_USABLE - 1).write(0xcd);
            assert_eq!(
                again.add(BURI_RT_STACK_USABLE - 1).read(),
                0xcd,
                "a decommitted block is no longer writable to its last usable byte"
            );
        }
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(again) };
    }

    /// **A shallow entry costs a load and nothing else.**
    ///
    /// The stack half of "no churn on a ping-pong workload": an entry that
    /// stays inside the retained prefix leaves the watermark armed, and a
    /// release that sees it armed makes no system call at all — so the pages
    /// the entry touched are still there when the next one arrives, which is
    /// the observable form of "nothing was refaulted".
    #[test]
    fn a_shallow_entry_keeps_its_pages_and_makes_no_call() {
        let base = buri_rt_stack_acquire();
        // SAFETY: `base` names `BURI_RT_STACK_USABLE` writable bytes.
        unsafe {
            base.write(0x11);
            base.add(BURI_RT_STACK_WARM - 1).write(0x22);
            assert!(watermark_intact(base), "a fresh block came back unarmed");
        }
        for _ in 0..8 {
            // SAFETY: a block this test acquired on this thread and is not inside.
            unsafe { buri_rt_stack_release(base) };
            let again = buri_rt_stack_acquire();
            assert_eq!(again, base, "a shallow release gave up the block");
            // SAFETY: as above.
            unsafe {
                assert_eq!(again.read(), 0x11, "the retained prefix was decommitted");
                assert_eq!(
                    again.add(BURI_RT_STACK_WARM - 1).read(),
                    0x22,
                    "the last byte of the retained prefix was decommitted"
                );
            }
        }
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(base) };
    }

    /// **A carrier gives its stack's pages back at least once every
    /// [`STACK_DECOMMIT_EVERY`] entries**, whatever the watermark thought.
    ///
    /// The floor under the probe's one failure mode — a frame that straddles
    /// the watermark without writing it — and the reason the policy has a
    /// guarantee rather than a tendency. The assertion is a `>=` because
    /// `decommitted_bytes` is process-wide and other tests release stacks too;
    /// what is exact is that nothing else can make it *smaller*.
    #[test]
    fn a_carrier_decommits_on_a_floor_even_when_nothing_looks_deep() {
        let mut before = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
        };
        // SAFETY: a writable, aligned destination.
        unsafe { buri_rt_heap_stats(&raw mut before) };
        for _ in 0..=STACK_DECOMMIT_EVERY {
            let p = buri_rt_stack_acquire();
            // SAFETY: `p` names writable bytes and this write is shallow — it
            // leaves the watermark armed, which is the point.
            unsafe { p.write(1) };
            // SAFETY: a block this test acquired on this thread and is not inside.
            unsafe { buri_rt_stack_release(p) };
        }
        let mut after = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
        };
        // SAFETY: as above.
        unsafe { buri_rt_heap_stats(&raw mut after) };
        assert!(
            after.decommitted_bytes
                >= before.decommitted_bytes + (BURI_RT_STACK_USABLE - BURI_RT_STACK_WARM) as u64,
            "{} shallow releases in a row decommitted nothing",
            STACK_DECOMMIT_EVERY
        );
    }

    /// **A nested entry's block is not kept warm.** The retained set is one
    /// block per carrier; the second one a carrier is holding is a nested
    /// entry's and is decommitted the moment it comes back.
    #[test]
    fn only_the_first_idle_block_is_retained() {
        let outer = buri_rt_stack_acquire();
        let inner = buri_rt_stack_acquire();
        // SAFETY: both name `BURI_RT_STACK_USABLE` writable bytes, and both
        // writes are above the retained prefix but below the guard.
        unsafe {
            outer.add(BURI_RT_STACK_WARM + 4096).write(0x33);
            inner.add(BURI_RT_STACK_WARM + 4096).write(0x44);
        }
        // `outer` is released first, onto an empty list: retained, untouched.
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(outer) };
        // `inner` lands on a list that already has one: decommitted at once.
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(inner) };
        let a = buri_rt_stack_acquire();
        let b = buri_rt_stack_acquire();
        let (kept, dropped) = if a == outer { (a, b) } else { (b, a) };
        assert_eq!(kept, outer);
        assert_eq!(dropped, inner);
        // SAFETY: both are live blocks just acquired.
        unsafe {
            assert_eq!(
                kept.add(BURI_RT_STACK_WARM + 4096).read(),
                0x33,
                "the retained block was decommitted"
            );
            assert_eq!(
                dropped.add(BURI_RT_STACK_WARM + 4096).read(),
                0,
                "a nested entry's block was kept warm"
            );
        }
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(b) };
        // SAFETY: a block this test acquired on this thread and is not inside.
        unsafe { buri_rt_stack_release(a) };
    }

    // === G6 end ============================================================

    // === G3 begin: the marking latch =======================================

    /// **The latch is the whole of the marking policy**, and it is tested here
    /// rather than only in `rt.rs` because it is a property of this file and
    /// `rt.rs` is not compiled without `net`.
    ///
    /// Three claims: nothing is marked before the call, everything is after
    /// it, and the capacity a marked block reports is still the byte count it
    /// was asked for — which is what keeps the release glue's `cap / stride`
    /// walk and `buri_rt_free`'s layout recovery honest on a marked block.
    #[test]
    fn the_latch_marks_every_block_it_precedes_and_none_before() {
        let _latch = latch();
        assert!(!values_may_cross_tasks(), "the silent answer is the safe one");
        let before = buri_rt_alloc(96);
        // SAFETY: live, just allocated.
        assert_eq!(unsafe { count_and_mark(before) }, (1, false));

        buri_rt_values_may_cross_tasks();
        assert!(values_may_cross_tasks());
        // Idempotent: an artifact makes one statement about itself, and a
        // second call is the same statement.
        buri_rt_values_may_cross_tasks();

        for payload in [0u64, 1, 8, BURI_RT_GROWTH_FLOOR, 4096] {
            let p = buri_rt_alloc(payload);
            // SAFETY: live, just allocated under the latch.
            unsafe {
                assert_eq!(count_and_mark(p), (1, true), "a {payload}-byte block was not marked");
                assert_eq!(buri_rt_cap(p), payload, "the mark cost the block its capacity");
                assert_eq!(buri_rt_unique_cap(p), None, "a marked block passed the unique test");
                buri_rt_free(p);
            }
            let z = buri_rt_alloc_zeroed(payload);
            // SAFETY: live, just allocated under the latch.
            unsafe {
                assert_eq!(count_and_mark(z), (1, true), "a zeroed block was not marked");
                buri_rt_free(z);
            }
        }

        // The block made before the latch is freed into this thread's cache
        // and comes back through `finish`, so it comes back marked. A recycled
        // block is a *new* value and the mark is the program's, not its own.
        // SAFETY: the only reference.
        unsafe { buri_rt_free(before) };
        let again = buri_rt_alloc(96);
        // SAFETY: live, just allocated.
        assert_eq!(unsafe { count_and_mark(again) }, (1, true), "a recycled block came back cold");
        // SAFETY: the only reference.
        unsafe { buri_rt_free(again) };

        forget_values_may_cross_tasks();
        assert!(!values_may_cross_tasks());
    }

    /// The count of a marked block is exact under many threads, with no
    /// scheduler in the way.
    ///
    /// `rt.rs`'s `the_count_of_a_marked_block_is_exact_under_every_carrier` is
    /// the same claim through the carrier pool; this one is the same claim
    /// through `std::thread`, so that it is asserted in the build that has no
    /// `net` and therefore no pool at all.
    #[test]
    fn a_marked_count_is_exact_under_many_threads() {
        let _latch = latch();
        const THREADS: usize = 8;
        const ROUNDS: usize = 2000;
        buri_rt_values_may_cross_tasks();
        let p = buri_rt_alloc(32);
        // SAFETY: live, just allocated under the latch.
        assert_eq!(unsafe { count_and_mark(p) }, (1, true));
        let shared = p as usize;
        std::thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(move || {
                    for _ in 0..ROUNDS {
                        // SAFETY: the block outlives this scope, and every
                        // increment is matched by the decrement below it.
                        unsafe {
                            buri_rt_incref(shared as *mut u8);
                            buri_rt_decref(shared as *mut u8, None);
                        }
                    }
                });
            }
        });
        // SAFETY: still live at whatever the counts left it.
        assert_eq!(
            unsafe { count_and_mark(p) },
            (1, true),
            "a reference operation was lost: the marked arm is not atomic",
        );
        // SAFETY: the only reference.
        unsafe { buri_rt_free(p) };
        forget_values_may_cross_tasks();
    }

    // === G3 end ============================================================
}
