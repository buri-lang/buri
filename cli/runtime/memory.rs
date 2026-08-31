//! Allocation, the 16-byte header, and the reference-count slow paths.
//!
//! MEMORY.md §5. The fast paths — `incref`, `decref`, and one day the size-class
//! `alloc` — are open-coded by both backends and never reach this file; what is
//! here is the block itself, the free path, drop-glue dispatch, and the
//! non-inlined forms the runtime uses on its own values.

use crate::abort::{buri_rt_abort_alloc_budget, buri_rt_abort_oom};
use std::alloc::{alloc, alloc_zeroed, dealloc, realloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

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

/// Bit 62 of `cap`: the block lives in a **scoped arena** and is not the
/// platform allocator's to give back.
///
/// Set by [`finish_with`] for every block served out of a `core/alloc::scoped`
/// arena, and read by exactly two functions: [`buri_rt_free`], which does the
/// accounting and then returns rather than calling `dealloc`, and
/// [`buri_rt_realloc`], which grows such a block by allocating a new one rather
/// than by handing the old one to `realloc`.
///
/// **Bit 62 rather than a second word or a side table**, for
/// [`BURI_RT_CAP_SHARED`]'s reason and one more: the question "whose is this
/// block" has to be answerable on the `free` path, which is the hottest cold
/// path there is, and a header word that is already loaded answers it for
/// nothing. A side table would put a lookup in front of every free in the
/// process to serve the scopes.
///
/// A capacity that reached 2^62 bytes would collide with it, which is four
/// exabytes in one Buri value.
pub const BURI_RT_CAP_ARENA: u64 = 1 << 62;

/// The usable payload bytes of a block, once the two flag bits are off.
pub const BURI_RT_CAP_MASK: u64 = !(BURI_RT_CAP_SHARED | BURI_RT_CAP_ARENA);

/// The flag bits of a `cap` word, as a mask: what [`buri_rt_realloc`] carries
/// across and what [`BURI_RT_CAP_MASK`] takes off.
pub const BURI_RT_CAP_FLAGS: u64 = BURI_RT_CAP_SHARED | BURI_RT_CAP_ARENA;

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

/// Whether `h`'s block was served out of a scoped arena
/// ([`BURI_RT_CAP_ARENA`]) — the G5 fork on the free path.
///
/// # Safety
/// `h` points at a live block header.
#[inline]
unsafe fn is_arena(h: *const Header) -> bool {
    // SAFETY: the caller promises a live header.
    unsafe { (*h).cap & BURI_RT_CAP_ARENA != 0 }
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

/// What [`buri_rt_heap_stats`] writes. Eight `u64`s, in this order.
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
    /// G4: bytes every live scoped arena has mapped and not yet given back.
    ///
    /// **Not part of the heap the four counts above measure**, and it is here
    /// rather than added to them for that reason: an arena's blocks are its
    /// own `mmap`s, no Buri value is inside one, and a `live_bytes` that
    /// included them would stop meaning "what the program's blocks weigh".
    /// This is what `core/alloc`'s `scoped` has reserved on behalf of the
    /// scopes that are currently open.
    pub arena_bytes: u64,
    /// G4: bytes scoped arenas have `munmap`ed since the process started,
    /// cumulative. Never falls.
    ///
    /// The pair is the assertion `scoped` exists to make: `arena_bytes` back
    /// where it started **and** this one risen is a scope that reserved pages
    /// and gave them back, which neither number says on its own.
    pub arena_released_bytes: u64,
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
    // G5: a process that has opened a scope asks which one it is in; one that
    // has not takes the two lines below, which are the two lines it took
    // before this slice. [`scopes_exist`] is the whole of the difference.
    if scopes_exist() {
        if let Some(p) = scoped_alloc(payload, false) {
            return p;
        }
    } else if let Some(p) = cache_pop(payload) {
        // G2: this thread's cache first. A hit is a block of exactly `payload`
        // usable bytes, so `finish` writes the same header it would have
        // written over a fresh one.
        //
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
    // G5: as in `buri_rt_alloc`, and an arena block needs no zeroing — it is a
    // range of a fresh anonymous mapping handed out exactly once, because the
    // window only moves forward and a released arena's pages are unmapped
    // rather than reused, and POSIX guarantees `MAP_ANON` is zero.
    if scopes_exist() {
        if let Some(p) = scoped_alloc(payload, true) {
            return p;
        }
    } else if let Some(p) = cache_pop(payload) {
        // G2: a cached block holds whatever the last value in it held, so this
        // one zeroes what `alloc_zeroed` would have got from the allocator.
        //
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
    finish_with(raw, payload, 0)
}

/// [`finish`], with the block's own flag bits — G5's [`BURI_RT_CAP_ARENA`] for
/// a block a scope served, and nothing for one the platform allocator did.
fn finish_with(raw: *mut u8, payload: u64, flags: u64) -> *mut u8 {
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
        raw.cast::<Header>().write(Header { rc: 1, cap: payload | shared_mask() | flags });
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
    /// **G5: the `core/alloc::scoped` arena this carrier is inside, plus one;
    /// `0` is none.**
    ///
    /// It is *here*, in a struct about free lists, for one reason and it is a
    /// measured one. Both questions — "is this carrier inside a scope" and "has
    /// this carrier a block of this size" — are asked on every allocation, and
    /// on macOS a `thread_local!` access is a call to `tlv_get_addr`, not a
    /// register-relative load. Two thread-locals therefore cost two of those:
    /// the first draft of this slice put the arena in a `Cell<u64>` of its own
    /// and measured **+12 %** on G2's `allocs` row — 2.4 ns on an
    /// allocate-and-free pair — for a load that answers `0` in every program
    /// that never opens a scope. Folding it into the cache makes it one access
    /// and a branch on a word already in the register.
    ///
    /// The encoding is the handle plus one so that `0` means "no arena" and the
    /// hot test is a compare against zero. `buri_rt_alloc_arena_enter` hands
    /// the previous value back to `scoped`, which gives it to
    /// `buri_rt_alloc_arena_leave` unread — so nesting is the caller's local
    /// and this file keeps no stack.
    arena: u64,
    /// **The bump window this carrier is serving the scope's blocks out of**:
    /// the next free byte, and one past the end of the mapping it is in.
    ///
    /// Here rather than in the arena for the same reason [`Cache::arena`] is
    /// here, one step further on. The arena's own table is behind a `Mutex`, so
    /// a block allocated through it would cost a lock acquisition *per
    /// allocation* — twice what the whole allocate-and-free pair costs when it
    /// is uncontended, and a queue when it is not. With the window on the
    /// carrier, an allocation inside a scope is an add and a compare on a word
    /// [`take_block`] has already loaded, and the lock is taken once per 64 KiB
    /// to map the next one.
    ///
    /// The window is **abandoned rather than saved** when a scope is entered or
    /// left, which costs at most the tail of one block per nesting event and
    /// keeps what `scoped` carries between the two calls a single `I64`. The
    /// mapping itself is in the arena's `blocks` list either way, so the
    /// abandoned tail is still `munmap`ed with the rest.
    arena_at: usize,
    arena_end: usize,
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
            arena: 0,
            arena_at: 0,
            arena_end: 0,
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

/// The allocation path of a process that has opened a scope at some point:
/// **a finished payload pointer**, or `None` to fall through to the platform
/// allocator.
///
/// Out of line from [`buri_rt_alloc`] on purpose. A program that never calls
/// `core/alloc::scoped` never reaches here, and the cost of the feature to that
/// program is [`scopes_exist`]'s relaxed load and a predictable branch — the
/// same shape as G3's marking latch, and for the same reason.
///
/// Inside, **two questions and one `tlv_get_addr`**. See [`Cache::arena`]: the
/// first draft of this slice put the arena in a thread-local of its own and
/// measured 2.4 ns on an allocate-and-free pair, which is a third of what the
/// pair costs.
///
/// The bump window is checked before the cache and the cache is not consulted
/// at all inside a scope: a cached block is the platform allocator's, and
/// handing one to a scope that will `munmap` around it would be a
/// use-after-free at the *next* allocation of that size on some other carrier.
fn scoped_alloc(payload: u64, zeroed: bool) -> Option<*mut u8> {
    enum Take {
        Bumped(*mut u8),
        Arena(i64),
        Cached(*mut u8),
        Neither,
    }
    // `try_with` rather than `with`: an allocation while this thread's
    // destructors run must not panic, and the answer there is "no cache".
    let taken = CACHE
        .try_with(|c| {
            // SAFETY: `c` is this thread's own cell, and no reference derived
            // from it escapes this closure or crosses a call that could
            // re-enter.
            let cache = unsafe { &mut *c.get() };
            if cache.arena != 0 {
                let need = block_bytes(payload);
                if let Some(end) = cache.arena_at.checked_add(need) {
                    if end <= cache.arena_end {
                        let raw = cache.arena_at as *mut u8;
                        cache.arena_at = end;
                        return Take::Bumped(raw);
                    }
                }
                return Take::Arena(cache.arena.wrapping_sub(1) as i64);
            }
            if payload > CACHE_MAX_PAYLOAD {
                return Take::Neither;
            }
            let idx = payload as usize;
            let Some(slot) = cache.slots.get_mut(idx) else { return Take::Neither };
            let p = slot.head;
            if p.is_null() {
                return Take::Neither;
            }
            // SAFETY: every pointer in a slot is a block this file freed, of
            // exactly `payload` usable bytes, whose `rc` holds the next one.
            slot.head = unsafe { (*header(p)).rc as *mut u8 };
            // G6: the program wanted this size in this period, so the sweep
            // leaves the slot alone.
            slot.hit = true;
            cache.held = cache.held.saturating_sub(slot_bytes(payload));
            Take::Cached(p)
        })
        .unwrap_or(Take::Neither);
    match taken {
        Take::Bumped(raw) => {
            // **A pooled block holds the last scope's bytes.** `MAP_ANON` is
            // zero-filled and the window only moves forward, so a mapping
            // fresh from the kernel needs nothing here — but `ARENA_POOL`
            // hands the same pages to the next scope, so the zeroing caller
            // has to be given what it asked for.
            //
            // SAFETY: `raw` names `BURI_RT_HEADER + payload` bytes this
            // carrier has just reserved and nothing else holds.
            if zeroed {
                unsafe { std::ptr::write_bytes(raw.add(BURI_RT_HEADER), 0, payload as usize) };
            }
            Some(finish_with(raw, payload, BURI_RT_CAP_ARENA))
        }
        Take::Arena(handle) => {
            let p = arena_block(handle, payload)?;
            // SAFETY: as above; `p` is the payload pointer of a block this
            // call has just reserved.
            if zeroed {
                unsafe { std::ptr::write_bytes(p, 0, payload as usize) };
            }
            Some(p)
        }
        Take::Cached(p) => {
            // SAFETY: `p` is a payload pointer to a block with `payload` usable
            // bytes, which is exactly the range a zeroing caller wants written.
            unsafe {
                if zeroed {
                    std::ptr::write_bytes(p, 0, payload as usize);
                }
                Some(finish(p.sub(BURI_RT_HEADER), payload))
            }
        }
        Take::Neither => None,
    }
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
        (cap_of(h), (*h).rc, (*h).cap & BURI_RT_CAP_FLAGS)
    };
    // G5: a bump allocator cannot grow the block it handed out last but one, so
    // an arena block is grown the way a bump allocator grows anything —
    // allocate, copy, abandon. The new block comes from whichever allocator is
    // active *now*, which is the same rule `buri_rt_alloc` follows and the same
    // one that makes the copy at the scope's boundary the only way out.
    if flags & BURI_RT_CAP_ARENA != 0 {
        let fresh = buri_rt_alloc(payload);
        // SAFETY: `fresh` has `payload` usable bytes and `p` has `old_cap`;
        // the two blocks do not overlap, because `fresh` is an allocation
        // nothing else holds.
        unsafe {
            std::ptr::copy_nonoverlapping(p.cast_const(), fresh, old_cap.min(payload) as usize);
            // The count travels with the value, exactly as it does below.
            (*header(fresh)).rc = rc;
            // Not a `dealloc`: this only takes the old block out of `LIVE_*`,
            // because its pages belong to the scope.
            buri_rt_free(p);
        }
        return fresh;
    }
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
    let (cap, arena) = unsafe {
        let h = header(p);
        if (*h).rc == BURI_RT_IMMORTAL {
            return;
        }
        (cap_of(h), is_arena(h))
    };
    LIVE_BLOCKS.fetch_sub(1, Ordering::Relaxed);
    LIVE_BYTES.fetch_sub(cap, Ordering::Relaxed);
    // G5: a block a scope served is not the platform allocator's, and it is not
    // this thread's cache's either. The accounting above has already run — so
    // the block is not live and is not a leak — and the pages under it go back
    // in one `munmap` when the scope ends (`buri_rt_alloc_arena_release`).
    //
    // The *glue* has already run, above this in `buri_rt_decref`, which is what
    // makes this sound rather than a leak with a nicer name: a heap block held
    // by a value in the arena is released by the ordinary count on the ordinary
    // path, and the arena's bulk free reclaims only address space whose last
    // reference has already gone.
    if arena {
        return;
    }
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
            arena_bytes: ARENA_BYTES.load(Ordering::Relaxed),
            arena_released_bytes: ARENA_RELEASED_BYTES.load(Ordering::Relaxed),
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
// === G4 begin: the scoped arena ==========================================
// ---------------------------------------------------------------------------
//
// # `core/alloc`'s `scoped`, from the runtime's side
//
// `Scoped<C>` is the attenuating wrapper `core/alloc` declares: every effect
// forwards to the `C` it holds except `Alloc`, which is served here. What
// "served" means is the whole of this section, and it is narrower than the
// word usually implies — narrower on purpose, and the narrowness is what makes
// the release at the end of a scope sound today rather than after G5.
//
// **An arena is a bump allocator over its own mappings.** `arena_create` maps
// nothing; each `arena_allocate(handle, n)` reserves `n` bytes from the
// arena's current 64 KiB block, mapping another when the block is full and a
// right-sized one of its own when `n` is bigger than a block; `arena_release`
// unmaps every block the arena ever took. That is a real reservation with a
// real bulk free, and it is the first `Alloc` in this language that maps a
// page rather than only counting one.
//
// **What it does not do is hold Buri values, and that is deliberate.** A
// `[Str]` built inside a scope is a block from [`buri_rt_alloc`], on the
// reference-counted heap, exactly where it was before this slice. It could not
// be otherwise in this tree: the native ABI drops the context argument from
// every runtime call (`backend/runtime_table.rs`), so the function that builds
// a list never learns which allocator asked for it — `core/alloc`'s own module
// comment states that gap and this slice does not close it.
//
// **So the interim soundness rule is a short one: nothing that can escape a
// scope is ever inside the arena.** `allocate` answers `Region`, and a
// `Region` carries the *charge* — the byte count — and not a pointer; that is
// true of `HostAlloc`, of `GeneralPurpose`, of `FixedBuffer`, and it stays
// true here, so the number a scope hands out survives the scope by
// construction. Nothing G3 can mark, and nothing G6 can decommit, is in these
// mappings: they are this arena's own, they never enter the `malloc` heap or
// the carrier-stack pool, and the release below is an unconditional `munmap`
// of memory no Buri value has ever pointed into.
//
// **What G5 changes.** When `Helper::Copy` exists, a value that leaves a scope
// can be deep-copied out of it, and *then* it is safe to serve Buri blocks
// from these mappings — the copy at the boundary is what makes the bulk free
// sound for values as well as for reservations. Until that lands, the honest
// statement of what `scoped` buys is: a real, bounded, bulk-released
// reservation, and an allocator whose charges do not reach the caller's. The
// other reading — serve the blocks now, and keep the arena alive forever
// because a marked block might have escaped — was rejected: an arena that is
// never released is not an arena, and it would have made the acceptance
// property of this slice untrue.

/// The block an arena takes when it needs more room: 64 KiB.
///
/// A multiple of 16 KiB, so it is a whole number of pages on arm64 macOS as
/// well as everywhere else, and big enough that an ordinary scope's charges
/// cost one `mmap` between them.
const BURI_RT_ARENA_BLOCK: usize = 64 * 1024;

/// **Whether this process has ever opened a scope.**
///
/// Set by [`buri_rt_alloc_arena_create`] and never cleared, and read on every
/// allocation by [`take_block`] — one relaxed load of a word written at most
/// once in a process's life, so it is in this core's cache from the first block
/// onwards. A program with no `core/alloc::scoped` in it therefore allocates
/// exactly as it did before G5.
///
/// The same device as G3's marking latch, for the same reason: the question is
/// about the *program* and not about the allocation, so it is asked once and
/// the answer is a word rather than a branch through a thread-local.
static SCOPES_EXIST: AtomicBool = AtomicBool::new(false);

#[inline]
fn scopes_exist() -> bool {
    SCOPES_EXIST.load(Ordering::Relaxed)
}

/// Bytes an arena has mapped and not yet given back, across every live arena.
static ARENA_BYTES: AtomicU64 = AtomicU64::new(0);

/// Bytes arenas have given up since the process started, cumulative. Never
/// falls.
///
/// **Given up, not necessarily unmapped.** A standard block goes to
/// [`ARENA_POOL`] rather than to the kernel, up to [`ARENA_POOL_MAX`]; past
/// that bound, and for every mapping that is not a standard block, this is a
/// `munmap`. Either way no live arena holds the bytes any more, which is what
/// the pair with [`ARENA_BYTES`] is asserting — and the pool is bounded and
/// stated, exactly as G2's block caches and B7's stack pool are.
static ARENA_RELEASED_BYTES: AtomicU64 = AtomicU64::new(0);

/// One scope's arena.
///
/// `blocks` are `(base, len)` as `usize` so the whole struct is `Send`: what
/// travels is an address, and every use of it is inside this file.
struct Arena {
    blocks: Vec<(usize, usize)>,
    /// Bytes handed out of the last block.
    cursor: usize,
    allocations: i64,
    bytes: i64,
    /// Bumped every time this slot is released, and carried in the high half
    /// of every handle the slot issues. See [`arena_slot`].
    generation: u32,
    live: bool,
}

/// Every arena this process has issued a handle for, live or not.
///
/// A `Mutex` like `COUNTERS` next door, and unlike that one this may genuinely
/// be contended: G3's carriers run Buri code beside each other, and a scope
/// per task is the shape the note's server example has. It is taken once per
/// `allocate` and the body under it is arithmetic and at most one `mmap`, so
/// the lock is not the cost of a scope.
static ARENAS: Mutex<Vec<Arena>> = Mutex::new(Vec::new());

/// **Blocks a released arena hands to the next one instead of the kernel.**
///
/// A scope per request is the workload this whole feature is for, and a scope
/// that maps and unmaps a block is two system calls per request: measured at
/// **2.4 µs a scope** on macOS, which made a 500,000-scope run 2.2× the same
/// program without scopes — a pessimisation, not a feature. With the pool it
/// is a `Vec::pop`.
///
/// Bounded at [`ARENA_POOL_MAX`], which is the same shape as G2's per-thread
/// block caches and B7's carrier-stack pool next door: a small, stated amount
/// of memory this runtime holds so that the common path makes no system call.
/// Anything past the bound, and every mapping that is not a standard block, is
/// `munmap`ed.
static ARENA_POOL: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// How many standard blocks the pool keeps: 8, which is 512 KiB.
///
/// Small on purpose. What it has to cover is *concurrent* scopes plus a little
/// slack, not a working set — a scope gives its block back the moment it ends,
/// so a server answering one request per carrier needs one block per carrier.
const ARENA_POOL_MAX: usize = 8;

fn arena_pool<T>(f: impl FnOnce(&mut Vec<usize>) -> T) -> T {
    let mut guard = match ARENA_POOL.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

/// Slots whose arena has been released, for reuse.
///
/// **Reuse is what makes a scope per request affordable.** Without it the
/// table would grow by one entry for every `scoped` call a process ever makes,
/// which is a leak in exactly the workload the feature is for.
static ARENA_FREE: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn arenas<T>(f: impl FnOnce(&mut Vec<Arena>) -> T) -> T {
    let mut guard = match ARENAS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

fn arena_free<T>(f: impl FnOnce(&mut Vec<usize>) -> T) -> T {
    let mut guard = match ARENA_FREE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

/// A handle's slot index and the generation it was issued at.
///
/// **The generation is the answer to a `Scoped` that outlives its scope.**
/// `scoped(ctx, body)` releases the arena when `body` returns, and `body`'s
/// return type is an unbounded `T` — so a body may hand its `Scoped<C>` back,
/// and a caller may then allocate through a scope that has ended. Reusing the
/// slot without a tag would charge that request to whatever *other* scope took
/// the slot, which is a wrong number in somebody else's arena. With the tag,
/// the stale handle matches no slot, and the request is answered the way
/// [`index`] answers an unknown counter: quietly, charging nothing. Neither
/// reading can touch memory, because a `Region` is a byte count.
fn arena_slot(handle: i64) -> (usize, u32) {
    let bits = handle as u64;
    ((bits & 0xffff_ffff) as usize, (bits >> 32) as u32)
}

/// The handle for a slot at a generation.
fn arena_handle(slot: usize, generation: u32) -> i64 {
    let bits = (u64::from(generation) << 32) | (slot as u64 & 0xffff_ffff);
    bits as i64
}

/// `bytes` rounded up to a whole number of [`BURI_RT_ARENA_BLOCK`]s.
fn arena_block_len(bytes: usize) -> usize {
    let blocks = bytes.div_ceil(BURI_RT_ARENA_BLOCK).max(1);
    match blocks.checked_mul(BURI_RT_ARENA_BLOCK) {
        Some(l) => l,
        // A single request that cannot even be described in bytes is out of
        // memory by any useful definition, and `allocate` has no value to
        // report a failure with (`effect.buri`'s `Region`).
        None => buri_rt_abort_oom(bytes as u64),
    }
}

/// `len` bytes for an arena, counted into [`ARENA_BYTES`]: out of
/// [`ARENA_POOL`] where it is a standard block and the pool has one, and out of
/// the kernel otherwise.
fn arena_map(len: usize) -> usize {
    if len == BURI_RT_ARENA_BLOCK {
        if let Some(base) = arena_pool(Vec::pop) {
            ARENA_BYTES.fetch_add(len as u64, Ordering::Relaxed);
            return base;
        }
    }
    // SAFETY: a fresh anonymous private mapping; no fd, no fixed address.
    let p = unsafe {
        mmap(core::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0)
    };
    if p.is_null() || p as usize == MAP_FAILED {
        buri_rt_abort_oom(len as u64);
    }
    ARENA_BYTES.fetch_add(len as u64, Ordering::Relaxed);
    p as usize
}

/// Reserves `bytes` from `a`, mapping another block when the current one
/// cannot hold them.
///
/// A request larger than a block gets a right-sized mapping of its own and the
/// remainder of the previous block is abandoned, which is what every bump
/// allocator does with the tail and what makes the cursor a single number.
fn arena_reserve(a: &mut Arena, bytes: usize) {
    if bytes == 0 {
        return;
    }
    let room = a.blocks.last().map_or(0, |&(_, len)| len.saturating_sub(a.cursor));
    if bytes <= room {
        a.cursor = a.cursor.saturating_add(bytes);
        return;
    }
    let len = arena_block_len(bytes);
    a.blocks.push((arena_map(len), len));
    a.cursor = bytes;
}

/// `core/alloc`'s `arenaCreate()` — a fresh scope's arena, and its handle.
///
/// Maps nothing: a scope that charges nothing costs a table slot and no system
/// call, which is what keeps `scoped` cheap enough to wrap a request in.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_arena_create() -> i64 {
    // From here on this process's allocations ask which scope they are in.
    SCOPES_EXIST.store(true, Ordering::Relaxed);
    let reused = arena_free(Vec::pop).and_then(|slot| {
        arenas(|t| {
            let a = t.get_mut(slot)?;
            a.live = true;
            a.cursor = 0;
            a.allocations = 0;
            a.bytes = 0;
            a.blocks.clear();
            Some(arena_handle(slot, a.generation))
        })
    });
    if let Some(handle) = reused {
        return handle;
    }
    arenas(|t| {
        t.push(Arena {
            blocks: Vec::new(),
            cursor: 0,
            allocations: 0,
            bytes: 0,
            generation: 0,
            live: true,
        });
        let slot = t.len().saturating_sub(1);
        // A slot index that does not fit `u32` is a table with four billion
        // *live* arenas, which is not reachable: a released slot is reused.
        if slot > u32::MAX as usize {
            buri_rt_abort_oom(slot as u64);
        }
        arena_handle(slot, 0)
    })
}

/// `core/alloc`'s `arenaAllocate(handle, bytes)` — reserve, count, and answer
/// what was asked for.
///
/// The answer is the request, exactly as `HostAlloc.allocate` and the three
/// counting allocators answer it, so `Region` means the same number under a
/// scope as outside one and the JavaScript backend agrees with the native one
/// on every charge. A negative request reserves nothing; there is no value to
/// refuse it with, and the counter records what it was told.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_arena_allocate(handle: i64, bytes: i64) -> i64 {
    let (slot, generation) = arena_slot(handle);
    arenas(|t| {
        let Some(a) = t.get_mut(slot) else { return };
        if !a.live || a.generation != generation {
            return;
        }
        a.allocations = a.allocations.saturating_add(1);
        a.bytes = a.bytes.saturating_add(bytes);
        if let Ok(n) = usize::try_from(bytes) {
            arena_reserve(a, n);
        }
    });
    bytes
}

/// `core/alloc`'s `arenaRelease(handle)` — **unmap every page this arena took**
/// and retire the slot. Answers the bytes given back.
///
/// This is the operation the whole of `scoped` exists for, and it is
/// unconditional: there is nothing in these mappings for a reference count to
/// own, for G3 to mark, or for G6 to decommit — see this section's header.
/// Releasing twice releases nothing the second time, because the slot's
/// generation has already moved past the handle.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_arena_release(handle: i64) -> i64 {
    let (slot, generation) = arena_slot(handle);
    let Some(blocks) = arenas(|t| match t.get_mut(slot) {
        Some(a) if a.live && a.generation == generation => {
            a.live = false;
            a.cursor = 0;
            a.generation = a.generation.wrapping_add(1);
            Some(std::mem::take(&mut a.blocks))
        }
        _ => None,
    }) else {
        return 0;
    };
    let mut freed: u64 = 0;
    for (base, len) in blocks {
        freed = freed.saturating_add(len as u64);
        // A standard block goes to the pool for the next scope, up to the
        // bound; everything else, and everything past the bound, goes back to
        // the kernel.
        let kept = len == BURI_RT_ARENA_BLOCK
            && arena_pool(|pool| {
                if pool.len() >= ARENA_POOL_MAX {
                    return false;
                }
                pool.push(base);
                true
            });
        if kept {
            continue;
        }
        // SAFETY: every pointer here came from `arena_map` with exactly this
        // length, and nothing outside the arena points into it: the values it
        // held are dead — their counts reached zero before this — and the one
        // that left was deep-copied out (`buri_rt_copy_block`).
        unsafe { munmap(base as *mut core::ffi::c_void, len) };
    }
    if freed > 0 {
        ARENA_BYTES.fetch_sub(freed, Ordering::Relaxed);
        ARENA_RELEASED_BYTES.fetch_add(freed, Ordering::Relaxed);
    }
    arena_free(|f| f.push(slot));
    i64::try_from(freed).unwrap_or(i64::MAX)
}

/// `core/alloc`'s `arenaCount(handle)`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_arena_count(handle: i64) -> i64 {
    let (slot, generation) = arena_slot(handle);
    arenas(|t| t.get(slot).filter(|a| a.generation == generation).map_or(0, |a| a.allocations))
}

/// `core/alloc`'s `arenaTotal(handle)`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_arena_total(handle: i64) -> i64 {
    let (slot, generation) = arena_slot(handle);
    arenas(|t| t.get(slot).filter(|a| a.generation == generation).map_or(0, |a| a.bytes))
}


// ---------------------------------------------------------------------------
// === G5 begin: which arena a carrier is inside =============================
// ---------------------------------------------------------------------------
//
// # The context the ABI drops, restored as a property of the carrier
//
// `core/alloc`'s module header states the gap this closes: the native ABI drops
// the context argument from every `buri_rt_*` call, so the function that builds
// a `[Str]` never learns which allocator asked for it. G4 lived with it and
// held the arena to *charges*; G5 answers it, and not by putting the context
// back — that would be an argument on every runtime entry in the table, for a
// question all but one of them would ignore.
//
// **The answer is dynamic instead of passed.** `scoped` calls
// [`buri_rt_alloc_arena_enter`] before it calls `body` and
// [`buri_rt_alloc_arena_leave`] after, and for that dynamic extent — on that
// carrier — [`buri_rt_alloc`] serves out of the arena. Every allocation in the
// extent is the scope's, whoever asked for it and through whichever runtime
// entry, including the ones this runtime makes for itself.
//
// That is an *over*-approximation of "charged to the `Scoped`", and it is the
// safe end of the asymmetry MEMORY.md §5.5 states: a block that should have
// been on the heap and is in the arena is copied out with the answer or dies
// with the scope, which is correct and occasionally costs a copy; a block that
// should have been in the arena and is on the heap is also correct, and merely
// misses the optimisation. Neither direction dangles, because a value's
// lifetime never exceeds the extent it was made in except by being the answer,
// and the answer is deep-copied ([`buri_rt_copy_block`]).
//
// ## Per carrier, and what a task inside a scope gets
//
// The active arena is a thread-local, so a task started inside a scope and run
// by another carrier allocates on the platform heap. That is the safe
// direction again — a heap block outliving a scope is ordinary — and it is why
// this is a `Cell<u64>` and not a global. `rt.rs`'s carrier loop saves and
// restores it around a stack switch, because B9 runs two tasks on one thread
// and the arena belongs to the *task*, not to the thread it is on this turn.
//
// ## The encoding, which is one word and one test
//
// The cell holds **the handle plus one**, so that `0` is "no arena" and the
// hot path is a load and a branch on zero rather than two loads. `scoped` is
// handed the previous value back and gives it to `arenaLeave` unread, so
// nesting is the caller's local and this file keeps no stack. A handle of
// `u64::MAX` would encode as `0` and be read as no arena — a scope that
// allocated on the heap, which is slower and not wrong — and reaching it needs
// four billion live arenas at generation four billion.

/// The active arena's handle, or `None`. Off [`Cache::arena`], which is where
/// the encoding and the reason for its home are written down.
fn current_arena() -> Option<i64> {
    let biased = arena_slot_of_carrier().biased;
    (biased != 0).then(|| biased.wrapping_sub(1) as i64)
}

/// The scope a carrier is inside, as the three words that describe it.
///
/// `rt.rs` saves and restores one of these around a stack switch, because since
/// B9 two tasks share a carrier's thread and the arena — and the bump window
/// into it — belong to the task whose stack is running.
#[derive(Clone, Copy, Default)]
pub struct ArenaSlot {
    biased: u64,
    at: usize,
    end: usize,
}

impl ArenaSlot {
    /// A carrier that is inside no scope, which is what one between tasks is.
    pub const NONE: ArenaSlot = ArenaSlot { biased: 0, at: 0, end: 0 };
}

/// This carrier's scope, for `rt.rs` to put aside.
pub fn arena_slot_of_carrier() -> ArenaSlot {
    // SAFETY: this thread's own cell, and nothing derived from it escapes.
    CACHE
        .try_with(|c| unsafe {
            let cache = &*c.get();
            ArenaSlot { biased: cache.arena, at: cache.arena_at, end: cache.arena_end }
        })
        .unwrap_or(ArenaSlot::NONE)
}

/// Puts back what [`arena_slot_of_carrier`] answered.
pub fn set_arena_slot_of_carrier(slot: ArenaSlot) {
    // SAFETY: as above.
    let _ = CACHE.try_with(|c| unsafe {
        let cache = &mut *c.get();
        cache.arena = slot.biased;
        cache.arena_at = slot.at;
        cache.arena_end = slot.end;
    });
}

/// `core/alloc`'s `arenaEnter(handle)` — serve this carrier's blocks out of
/// `handle` until `arenaLeave` puts back what this answers.
///
/// Answers the *encoded* previous value rather than a handle, which is what
/// makes "there was no arena" expressible in the same `I64` the Buri side
/// carries. `scoped` never reads it; it hands it straight back.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_arena_enter(handle: i64) -> i64 {
    let previous = arena_slot_of_carrier().biased;
    // The window starts empty, so the first allocation of the new scope takes
    // the mapping path. The window the *outer* scope had is abandoned, which
    // is [`Cache::arena_at`]'s stated cost of nesting.
    set_arena_slot_of_carrier(ArenaSlot {
        biased: (handle as u64).wrapping_add(1),
        at: 0,
        end: 0,
    });
    previous as i64
}

/// `core/alloc`'s `arenaLeave(previous)` — the inverse, and the whole of it.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_arena_leave(previous: i64) -> i64 {
    set_arena_slot_of_carrier(ArenaSlot { biased: previous as u64, at: 0, end: 0 });
    previous
}

/// A block of `payload` usable bytes out of the arena this carrier is inside,
/// or `None` where it is inside none.
///
/// The header goes in the arena with the payload — one bump, one range — so
/// the block is bit-for-bit what `buri_rt_alloc` would have produced but for
/// [`BURI_RT_CAP_ARENA`] in its `cap`. Everything downstream of the pointer
/// therefore works unchanged: the walks, the glue, `incref`, `decref`, the
/// uniqueness test and both backends' open-coded copies of all of them.
///
/// A **retired** handle answers `None` and the request goes to the platform
/// allocator, which is the same quiet answer `index` gives an unknown counter
/// and the same one `buri_rt_alloc_arena_allocate` gives a stale scope.
///
/// `handle` is the one [`take_block`] read out of this carrier's cache, rather
/// than one read again here: the whole point of that function is that the
/// thread-local is touched once.
fn arena_block(handle: i64, payload: u64) -> Option<*mut u8> {
    let need = block_bytes(payload);
    let (slot, generation) = arena_slot(handle);
    let (base, len) = arenas(|t| {
        let a = t.get_mut(slot)?;
        if !a.live || a.generation != generation {
            return None;
        }
        let len = arena_block_len(need);
        let base = arena_map(len);
        a.blocks.push((base, len));
        // **The charge path's cursor is moved to the end of this mapping**, so
        // that `arena_reserve` maps one of its own rather than handing out
        // bytes this window is about to. One arena, two bump pointers, and no
        // overlap between them.
        a.cursor = len;
        Some((base, len))
    })?;
    // The rest of the mapping becomes this carrier's window.
    set_window(base.saturating_add(need), base.saturating_add(len));
    Some(finish_with(base as *mut u8, payload, BURI_RT_CAP_ARENA))
}

/// Points this carrier's bump window at `[at, end)`.
fn set_window(at: usize, end: usize) {
    // SAFETY: this thread's own cell, and nothing derived from it escapes.
    let _ = CACHE.try_with(|c| unsafe {
        let cache = &mut *c.get();
        cache.arena_at = at;
        cache.arena_end = end;
    });
}

/// The bytes one block of `payload` usable bytes occupies in an arena: the
/// header, the payload, and the rounding that keeps the next block aligned.
#[inline]
fn block_bytes(payload: u64) -> usize {
    (payload as usize).saturating_add(BURI_RT_HEADER).next_multiple_of(BURI_RT_ALIGN)
}


// === G5 end ===============================================================

// === G4 end ===============================================================

// ---------------------------------------------------------------------------
// === G5 begin: the copy out of a scope =====================================
// ---------------------------------------------------------------------------
//
// # A value that leaves a scope is deep-copied, and this is the block half
//
// `core/alloc`'s `scoped` answers `copyOut(inside)`, and `copyOut` is compiled
// rather than called: each backend generates a per-type **copy glue** the same
// way it generates the per-type release walk (`backend/stencil/glue.rs`'s
// `Helper::Copy`, `backend/llvm/emit.rs`'s `Job::Copy`), and that walk reaches
// a heap block through the two functions below and through nothing else.
//
// The division is deliberate. Everything that depends on a *type* is in the
// generated walk, where the type is known; everything that depends on a
// *block* is here, where the header is. This runtime is compiled once against
// no Buri type at all, so a copy that had to know a layout could not live in
// it — and a walk that had to know a header would be four backends' worth of
// arithmetic instead of one function.
//
// ## The copy is not a share, and that is the whole point
//
// [`buri_rt_copy_block`] does not increment anything. It allocates, memcpys
// the payload, and hands the fresh block to the type's own glue so that every
// pointer *inside* it is replaced in turn. So a value that has been copied out
// shares no block with the value it came from, at any depth — which is what
// makes the scope's bulk release sound, and what `a_copy_is_not_a_share`
// asserts by reading both counts.
//
// ## The mark is asked again rather than carried
//
// The new block goes through [`buri_rt_alloc`] and therefore through
// [`finish`], so its `cap` carries whatever `shared_mask()` says *now* — the
// program-wide answer G3 latched — and not the source block's bit. A source
// that was marked because it had crossed a task boundary does not hand that
// history to its copy: the copy is a new value, reachable so far from one
// place, and if the program can reach a task boundary at all it is marked for
// that reason and not for this one.

/// A fresh block holding the same payload bytes as `p`, with `rc == 1`.
///
/// `glue` is the type's **copy** glue — the generated walk that replaces every
/// counted pointer *inside* the new block with a copy of its own. It is null
/// for a block that holds only bytes (a `Str`'s allocation, an `[Int]`), which
/// is the same set of types [`buri_rt_decref`]'s `drop_glue` is null for.
///
/// A null `p` copies to a null `p`: a `Str` with no base is a static, and an
/// `Option`'s niche is a null pointer that means `.None`. An `IMMORTAL` block
/// copies to an ordinary counted one, which is right — the copy is a new value
/// and nothing about the original's immortality is a property of it.
///
/// # Safety
/// `p` is null or a live payload pointer, and `glue`, where non-null, is the
/// copy glue for the type `p` actually holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_copy_block(
    p: *mut u8,
    glue: Option<extern "C" fn(*mut u8)>,
) -> *mut u8 {
    if p.is_null() {
        return p;
    }
    // SAFETY: the caller promises a live payload pointer.
    let cap = unsafe { cap_of(header(p)) };
    let fresh = buri_rt_alloc(cap);
    // SAFETY: both blocks have `cap` usable bytes and they do not overlap —
    // `fresh` is an allocation nothing else holds.
    unsafe { std::ptr::copy_nonoverlapping(p.cast_const(), fresh, cap as usize) };
    if let Some(g) = glue {
        g(fresh);
    }
    fresh
}

/// The same for a `Str`, whose value is `{ base, ptr, len }` and whose `ptr`
/// points **into** `base` rather than at it (VALUE-MODEL.md §3).
///
/// One function rather than three instructions in each backend because the
/// rebase is the whole of the difference: a copied `Str` has to keep pointing
/// at the same *offset* of a different block, and a walk that only replaced
/// `base` would leave `ptr` addressing the block the scope is about to unmap.
///
/// A `Str` with no base is a static — a literal, or the empty string — and is
/// copied by leaving it alone: there is no block under it to release, so there
/// is none to duplicate either.
///
/// # Safety
/// `s` points at a readable, writable [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_copy_str(s: *mut u8) {
    if s.is_null() {
        return;
    }
    let v = s.cast::<crate::value::BuriStr>();
    // SAFETY: the caller promises a `BuriStr` here.
    unsafe {
        let base = (*v).base;
        if base.is_null() {
            return;
        }
        let offset = (*v).ptr.addr().wrapping_sub(base.addr());
        let fresh = buri_rt_copy_block(base, None);
        (*v).base = fresh;
        (*v).ptr = fresh.wrapping_add(offset).cast_const();
    }
}

// === G5 end ===============================================================


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
    /// The blocks **this thread** has mapped and is not currently inside.
    ///
    /// A **free list rather than a single pointer**, because entries nest: an
    /// entry thunk that calls Buri code which calls out and comes back in
    /// needs a second block, and handing it the first would put the inner
    /// frames on top of the outer ones' live locals. A list makes the nested
    /// case correct at the price of address space, which is the price this
    /// whole mechanism is already paying.
    ///
    /// **Thread-local was the whole answer until B9, and is now the fallback.**
    /// A block belongs to whoever is *inside* it, and since the carrier thread
    /// became a stack switch that is a **task**, not a thread: a task that
    /// parks holding a Buri stack is resumed on whichever carrier picks it up,
    /// and a list keyed by thread would hand its block to somebody else in the
    /// meantime. So a running task uses its own list ([`Blocks`] lives on
    /// `rt::Task`) and this one serves everything that is not a task — `main`'s
    /// own thread, a carrier between tasks, and every build without `net`,
    /// which has no tasks at all. [`stack_list`] is the two-line function that
    /// chooses, and B7's handover note is where this was predicted.
    static STACKS: core::cell::RefCell<Blocks> = const { core::cell::RefCell::new(Blocks::new()) };
}

/// A free list of idle Buri data stacks, and the release counter
/// [`STACK_DECOMMIT_EVERY`] is measured against.
///
/// One of these belongs to each running task and one to each thread; which is
/// in play is [`stack_list`]'s answer. Public to the crate because `rt::Task`
/// owns one.
pub(crate) struct Blocks {
    idle: Vec<*mut u8>,
    since_decommit: u32,
}

impl Blocks {
    pub(crate) const fn new() -> Self {
        Blocks { idle: Vec::new(), since_decommit: 0 }
    }

    /// A block nothing on this list is inside.
    fn acquire(&mut self) -> *mut u8 {
        match self.idle.pop() {
            Some(p) => p,
            None => map_stack(),
        }
    }

    /// Give `base` back, decommitting it where the three clauses
    /// [`buri_rt_stack_release`] states are met.
    ///
    /// # Safety
    /// `base` came from [`Blocks::acquire`] on **this** list and nothing is
    /// inside it.
    unsafe fn release(&mut self, base: *mut u8) {
        // SAFETY: the caller promises a live block nothing is inside.
        let deep = !unsafe { watermark_intact(base) };
        self.since_decommit += 1;
        // Three clauses, and the middle one is the retained set: the *first*
        // idle block a list has is the one its next entry will be handed, and
        // every block after it is a nested entry's, which is rare by
        // construction and not worth keeping warm.
        let go =
            deep || !self.idle.is_empty() || self.since_decommit >= STACK_DECOMMIT_EVERY;
        if go {
            self.since_decommit = 0;
        }
        if !go || decommit_stack(base) {
            self.idle.push(base);
        }
    }
}

impl Drop for Blocks {
    /// **A list that ends gives its blocks to the pool, not to the kernel.**
    ///
    /// Before B9 this was a `munmap` per block and that was right, because the
    /// only lists were threads' and a carrier thread never ended. A *task*
    /// ends constantly — one per step of a `Tasks.parallel` — so unmapping
    /// here would put an `mmap` and an `mprotect` in front of every step that
    /// enters Buri code, to give back address space that costs nothing. The
    /// blocks go to [`POOL`] instead, decommitted, and the pool's own cap is
    /// where the kernel eventually gets them back.
    fn drop(&mut self) {
        for p in self.idle.drain(..) {
            // SAFETY: every pointer in this list came from `map_stack`, was
            // mapped with exactly this length, and is one nothing is inside.
            unsafe { retire_stack(p) };
        }
    }
}

/// Idle Buri data stacks that belong to no list.
///
/// A `usize` rather than a pointer so the vector is `Send`: what travels is an
/// address, and every use of it is inside this file.
///
/// **Why a global pool exists at all**, when B7 was content with a per-thread
/// one: a task's list dies with the task, and the mapping in it is worth more
/// than the two system calls it would cost the next task to make a new one.
/// The pool is what carries a block from a task that has ended to a task that
/// is starting, across whatever carrier either ran on.
static POOL: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// How many idle blocks the pool holds before it starts unmapping them.
///
/// Sixty-four, and it is a **resident**-set number rather than an address-space
/// one: a pooled block has been decommitted, so it holds the retained prefix's
/// pages and no more — sixty-four of them is at most 16 MiB of reservation
/// that is warm and 4 GiB that is not. A cap is needed at all because a burst
/// of ten thousand tasks would otherwise leave ten thousand mappings behind
/// for a process that has gone quiet.
const STACK_POOL_MAX: usize = 64;

fn pool() -> MutexGuard<'static, Vec<usize>> {
    match POOL.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Hand a block nobody is inside to the pool, or to the kernel where the pool
/// is full.
///
/// # Safety
/// `base` came from [`map_stack`] and nothing is inside it.
unsafe fn retire_stack(base: *mut u8) {
    // SAFETY: the caller's promise. A block on its way out is decommitted
    // whatever the watermark says: there is no next entry on this list to keep
    // it warm for.
    if !decommit_stack(base) {
        // Retired by `decommit_stack` itself, which unmapped it.
        return;
    }
    let mut pool = pool();
    if pool.len() < STACK_POOL_MAX {
        pool.push(base as usize);
        return;
    }
    drop(pool);
    // SAFETY: `base` came from `map_stack` with exactly this length and is on
    // no list.
    unsafe {
        munmap(base.cast(), BURI_RT_STACK_BYTES);
    }
}

/// The list a `buri_rt_stack_acquire` on this thread draws on: **the running
/// task's own** where a task is running, and this thread's otherwise.
///
/// `#[inline(never)]` on `rt`'s side of it, and that is the load-bearing part:
/// the address of a thread-local is a value a compiler may cache, and a task
/// that switched carriers between the cache and the use would write through
/// the wrong thread's slot. Everything this function reaches for is read
/// through a call the compiler cannot see past.
fn stack_list<T>(f: impl FnOnce(&mut Blocks) -> T) -> T {
    #[cfg(feature = "net")]
    if let Some(mine) = crate::rt::running_task_blocks() {
        // SAFETY: `running_task_blocks` answers the list of the task running
        // on *this* thread, and a task runs on one carrier at a time, so this
        // is the only live reference to it.
        return f(unsafe { &mut *mine });
    }
    STACKS.with(|s| f(&mut s.borrow_mut()))
}

/// Maps one 64 MiB stack with its own 1 MiB `PROT_NONE` guard on top.
///
/// Zero-filled by the kernel and never faulted in until a frame lands on a
/// page, so the cost of a carrier that enters Buri code once and returns is
/// two system calls and the pages it actually used.
fn map_stack() -> *mut u8 {
    // A block the pool is holding is one that has already been mapped, given
    // its guard, and decommitted, so taking it is a lock and a pop against two
    // system calls.
    if let Some(p) = pool().pop() {
        return p as *mut u8;
    }
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

/// **The running task's Buri data stack**: the base a generated entry thunk
/// puts in `x0` (`rdi` on x86-64) before it calls into frame-threaded code.
///
/// The block is the caller's alone and carries its own `PROT_NONE` guard, so a
/// runaway recursion on a carrier faults at *its* boundary rather than walking
/// into whatever the linker placed after the process's static block.
///
/// **Whose block it is changed in B9** and the entry point did not, which is
/// what "behind the same ABI" means for this pair. B7 answered *this thread's*
/// free list; a parked task now outlives the carrier that started it, so the
/// answer is *this task's* list where a task is running and this thread's
/// where none is. [`stack_list`] is the whole of the difference.
///
/// Answers a block that is **not** in use by any other entry on the same list
/// — nested entries get different blocks — and it is the caller's business to
/// hand the same pointer back to [`buri_rt_stack_release`]. A caller that
/// enters Buri code repeatedly pays the mapping once: the block goes back on
/// the list rather than to the kernel.
///
/// `main` never calls this. The process's own carrier keeps the `__bss` block
/// `backend/stencil/asm.rs` emits, which is why a program that never starts a
/// second carrier makes no system call it did not make before.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_stack_acquire() -> *mut u8 {
    stack_list(Blocks::acquire)
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
/// The address space is **not** unmapped: the list is reused, so giving back
/// the reservation would cost an `mmap` and an `mprotect` per entry to buy
/// nothing — the reservation is free, and it is the *resident pages* that are
/// not. The mapping goes away when [`POOL`] is full, which since B9 is where a
/// block that outlives its list ends up.
///
/// A null pointer is ignored rather than aborting, for the reason
/// [`buri_rt_decref`]'s null check gives: a released nothing is a caller that
/// never acquired, which is a no-op and not a corruption.
///
/// # Safety
/// `base` is null, or a block from [`buri_rt_stack_acquire`] taken by **this
/// task, or by this thread while no task is running** — whichever
/// [`stack_list`] answered then must be the one it answers now, which is what
/// an entry thunk's acquire/release bracket guarantees — and that no entry
/// thunk is still inside. It became an `unsafe fn` in G6
/// and the reason is the watermark: this function now *reads* the block it is
/// handed, where before it only filed the pointer away, so a pointer that
/// names something else is a read of something else.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_stack_release(base: *mut u8) {
    if base.is_null() {
        return;
    }
    // SAFETY: the caller's promise, forwarded to the list that handed it out.
    stack_list(|blocks| unsafe { blocks.release(base) });
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

// ---------------------------------------------------------------------------
// The per-task machine stack (design/native track B, slice B9)
// ---------------------------------------------------------------------------
//
// The *other* stack. Everything above this line is the Buri data stack — the
// upward-growing block `middle::layout` addresses frames on. What follows is
// the ordinary downward-growing machine stack a task's own control flow lives
// on: the return-address chain, `cli/runtime`'s Rust frames, and — under the
// LLVM backend, where a Buri frame *is* a machine frame — the Buri frames too.
//
// **This closes `reports/wave6-b7b8.md` §5.2**, which recorded that a carrier's
// recursion depth was two different numbers on the two backends: 64 MiB of
// acquired Buri stack under the frame-threaded backend and a 512 KiB thread
// stack under LLVM. A task's machine stack is mapped here, at
// `BURI_RT_STACK_BYTES`, so the two are one number. `CARRIER_STACK_BYTES` in
// `rt.rs` still sizes the carrier *thread*, and no longer bounds any Buri code:
// a carrier's own stack now holds a scheduler loop and nothing else.

/// Map one task machine stack: [`BURI_RT_STACK_USABLE`] usable with a
/// [`BURI_RT_STACK_GUARD`] `PROT_NONE` guard **below** it.
///
/// Below, not above, and that is the one thing that differs from
/// [`map_stack`]: a machine stack grows *down*, so the guard goes where the
/// deepest frame would land. A runaway recursion on a task therefore faults on
/// its own guard, at its own boundary, exactly as
/// `a_runaway_recursion_on_a_carrier_faults_at_its_own_guard` requires of the
/// other stack.
fn map_task_stack() -> *mut u8 {
    if let Some(p) = task_pool().pop() {
        return p as *mut u8;
    }
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
    let p = p.cast::<u8>();
    assert!(
        (p as usize).is_multiple_of(BURI_RT_STACK_ALIGN),
        "a task stack was not mapped at a page boundary"
    );
    // SAFETY: `p` names `BURI_RT_STACK_BYTES` of this process's own mapping
    // and the guard is the bottom `BURI_RT_STACK_GUARD` of it.
    let rc = unsafe { mprotect(p.cast(), BURI_RT_STACK_GUARD, PROT_NONE) };
    assert!(rc == 0, "a task stack could not be given its guard");
    p
}

/// Idle task machine stacks, capped the way [`POOL`] is and for the same
/// reason.
static TASK_POOL: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn task_pool() -> MutexGuard<'static, Vec<usize>> {
    match TASK_POOL.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A machine stack for a task, and the address of its **top** — the high end,
/// which is where a downward-growing stack starts.
///
/// The pair `(base, top)`: `base` is what [`buri_rt_task_stack_release`] takes
/// back, and `top` is what `switch::prepare` builds a frame at.
pub(crate) fn buri_rt_task_stack_acquire() -> (*mut u8, *mut u8) {
    let base = map_task_stack();
    (base, base.wrapping_add(BURI_RT_STACK_BYTES))
}

/// Give a finished task's machine stack back, decommitted.
///
/// Called from the **carrier's** stack and never from the task's own, which is
/// the whole of why the carrier loop reaps a finished task rather than the task
/// reaping itself: a stack cannot free the ground it is standing on.
///
/// The retained prefix is [`BURI_RT_STACK_WARM`] at the **top** — a task that
/// stays shallow touches the top and nothing else, so what this releases is
/// never what the next task refaults, which is G6's argument at the other
/// stack's other end.
///
/// # Safety
/// `base` came from [`buri_rt_task_stack_acquire`] and no thread is running on
/// it.
pub(crate) unsafe fn buri_rt_task_stack_release(base: *mut u8) {
    if base.is_null() {
        return;
    }
    let low = base.wrapping_add(BURI_RT_STACK_GUARD);
    let len = BURI_RT_STACK_USABLE - BURI_RT_STACK_WARM;
    // SAFETY: `[base + GUARD, base + BYTES - WARM)` is inside the mapping
    // `map_task_stack` made, is a whole number of pages at a page boundary,
    // and nothing is running on it.
    let p = unsafe {
        mmap(
            low.cast(),
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON | MAP_FIXED,
            -1,
            0,
        )
    };
    if p.is_null() || p as usize == MAP_FAILED {
        // `decommit_stack`'s reasoning, at the other stack: a range this file
        // can no longer describe is retired whole rather than handed on.
        //
        // SAFETY: `base` came from `map_task_stack` with exactly this length.
        unsafe {
            munmap(base.cast(), BURI_RT_STACK_BYTES);
        }
        return;
    }
    DECOMMITTED_BYTES.fetch_add(len as u64, Ordering::Relaxed);
    let mut pool = task_pool();
    if pool.len() < STACK_POOL_MAX {
        pool.push(base as usize);
        return;
    }
    drop(pool);
    // SAFETY: as above; the pool is full and this block is on no list.
    unsafe {
        munmap(base.cast(), BURI_RT_STACK_BYTES);
    }
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
        assert_eq!(BURI_RT_CAP_ARENA, 1 << 62);
        assert_eq!(BURI_RT_CAP_MASK, u64::MAX >> 2);
        assert_eq!(BURI_RT_CAP_SHARED & BURI_RT_CAP_MASK, 0);
        assert_eq!(BURI_RT_CAP_ARENA & BURI_RT_CAP_MASK, 0);
        assert_eq!(BURI_RT_CAP_SHARED & BURI_RT_CAP_ARENA, 0);
        assert_eq!(BURI_RT_CAP_FLAGS, BURI_RT_CAP_SHARED | BURI_RT_CAP_ARENA);
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
            for flag in [
                0u64,
                BURI_RT_CAP_SHARED,
                BURI_RT_CAP_ARENA,
                BURI_RT_CAP_SHARED | BURI_RT_CAP_ARENA,
            ] {
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
    fn the_heap_stats_struct_is_eight_words_in_one_order() {
        assert_eq!(std::mem::size_of::<BuriHeapStats>(), 8 * 8);
        assert_eq!(std::mem::align_of::<BuriHeapStats>(), 8);
        let s = BuriHeapStats {
            live_blocks: 1,
            live_bytes: 2,
            total_blocks: 3,
            total_bytes: 4,
            retained_bytes: 5,
            decommitted_bytes: 6,
            arena_bytes: 7,
            arena_released_bytes: 8,
        };
        // SAFETY: `BuriHeapStats` is `repr(C)` and every field is a `u64`, so
        // it is eight `u64`s in declaration order and reading it as such is
        // exactly what the C mirrors do.
        let words = unsafe { std::slice::from_raw_parts(std::ptr::from_ref(&s).cast::<u64>(), 8) };
        assert_eq!(words, &[1, 2, 3, 4, 5, 6, 7, 8]);
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
            arena_bytes: 0,
            arena_released_bytes: 0,
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
            arena_bytes: 0,
            arena_released_bytes: 0,
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
            arena_bytes: 0,
            arena_released_bytes: 0,
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
            arena_bytes: 0,
            arena_released_bytes: 0,
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
            arena_bytes: 0,
            arena_released_bytes: 0,
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
            arena_bytes: 0,
            arena_released_bytes: 0,
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
            arena_bytes: 0,
            arena_released_bytes: 0,
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
            arena_bytes: 0,
            arena_released_bytes: 0,
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

    // === G4 begin: the scoped arena ========================================

    /// **The lock every case that opens a scope takes.**
    ///
    /// `arena_bytes` is process-wide and `cargo test` runs its cases on many
    /// threads at once, so "the number came back to where it started" is only
    /// an assertion while no other case has a scope open. That makes the rule
    /// wider than "cases that read the global": *opening* an arena moves the
    /// number too, so a case that only asserts about its own handle still has
    /// to hold this while it does — otherwise the block it maps is the block
    /// somebody else's delta is off by. Every case below that calls
    /// `buri_rt_alloc_arena_create` takes it; the ones that only do arithmetic
    /// on a handle do not.
    ///
    /// `memory::latch`'s shape, and independent of it: a scope has nothing to
    /// do with the marking latch, and a case that took both would be claiming
    /// they interact.
    fn arena_alone() -> std::sync::MutexGuard<'static, ()> {
        static ALONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        match ALONE.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// A snapshot of the two arena numbers, which is the only thing these
    /// cases assert about — `cargo test` runs on many threads and another
    /// case's scope may be open, so every claim here is a *difference*.
    fn arena_stats() -> (u64, u64) {
        let mut s = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
            arena_bytes: 0,
            arena_released_bytes: 0,
        };
        // SAFETY: `s` is a live, aligned `BuriHeapStats`.
        unsafe { buri_rt_heap_stats(&raw mut s) };
        (s.arena_bytes, s.arena_released_bytes)
    }

    /// **The acceptance property: a scope reserves pages and gives them back.**
    ///
    /// Both halves are needed and neither says it alone. `arena_bytes` back
    /// where it started is also what "never mapped anything" looks like, and
    /// `arena_released_bytes` rising is also what "released something it is
    /// still holding elsewhere" looks like. Together they are a reservation
    /// that was made and unmade.
    #[test]
    fn a_scope_maps_what_it_is_charged_and_unmaps_it_on_release() {
        let _alone = arena_alone();
        let (live_before, released_before) = arena_stats();
        let a = buri_rt_alloc_arena_create();
        // A create maps nothing: a scope that charges nothing costs no system
        // call at all.
        assert_eq!(arena_stats().0, live_before);

        assert_eq!(buri_rt_alloc_arena_allocate(a, 1024), 1024);
        let (live_open, _) = arena_stats();
        assert_eq!(
            live_open - live_before,
            BURI_RT_ARENA_BLOCK as u64,
            "a kilobyte inside a scope takes one block"
        );

        // The rest of the block is served without another mapping.
        assert_eq!(buri_rt_alloc_arena_allocate(a, 4096), 4096);
        assert_eq!(arena_stats().0, live_open);

        let freed = buri_rt_alloc_arena_release(a);
        assert_eq!(freed, BURI_RT_ARENA_BLOCK as i64);
        let (live_after, released_after) = arena_stats();
        assert_eq!(live_after, live_before, "the scope's pages went back");
        assert_eq!(released_after - released_before, BURI_RT_ARENA_BLOCK as u64);
    }

    /// A charge bigger than a block gets a mapping of its own, rounded up to a
    /// whole number of blocks — which is what keeps the cursor one number.
    #[test]
    fn a_charge_bigger_than_a_block_gets_its_own_mapping() {
        let _alone = arena_alone();
        let (live_before, _) = arena_stats();
        let a = buri_rt_alloc_arena_create();
        let big = (BURI_RT_ARENA_BLOCK * 3 + 1) as i64;
        assert_eq!(buri_rt_alloc_arena_allocate(a, big), big);
        assert_eq!(arena_stats().0 - live_before, (BURI_RT_ARENA_BLOCK * 4) as u64);
        assert_eq!(buri_rt_alloc_arena_release(a), (BURI_RT_ARENA_BLOCK * 4) as i64);
        assert_eq!(arena_stats().0, live_before);
    }

    /// A request for nothing is still a request. It maps nothing and counts
    /// one, which is `core/alloc`'s rule for every allocator it has.
    #[test]
    fn a_charge_for_nothing_maps_nothing_and_counts_once() {
        let _alone = arena_alone();
        let (live_before, _) = arena_stats();
        let a = buri_rt_alloc_arena_create();
        assert_eq!(buri_rt_alloc_arena_allocate(a, 0), 0);
        assert_eq!(arena_stats().0, live_before);
        assert_eq!(buri_rt_alloc_arena_count(a), 1);
        assert_eq!(buri_rt_alloc_arena_total(a), 0);
        assert_eq!(buri_rt_alloc_arena_release(a), 0);
    }

    /// **A `Scoped` that outlives its scope charges nothing**, and this is the
    /// generation tag doing it.
    ///
    /// `scoped`'s body returns an unbounded `T`, so a body may hand its
    /// wrapper back. The slot is reused — it has to be, or a scope per request
    /// leaks a table entry per request — so the tag is what keeps the stale
    /// handle from charging somebody else's arena.
    #[test]
    fn a_handle_does_not_survive_its_release() {
        let _alone = arena_alone();
        let a = buri_rt_alloc_arena_create();
        assert_eq!(buri_rt_alloc_arena_allocate(a, 32), 32);
        assert_eq!(buri_rt_alloc_arena_release(a), BURI_RT_ARENA_BLOCK as i64);

        // Charging a released arena moves nothing and maps nothing.
        let (live, _) = arena_stats();
        assert_eq!(buri_rt_alloc_arena_allocate(a, 64), 64);
        assert_eq!(arena_stats().0, live);
        assert_eq!(buri_rt_alloc_arena_count(a), 0);
        assert_eq!(buri_rt_alloc_arena_total(a), 0);
        // And releasing it twice releases nothing the second time.
        assert_eq!(buri_rt_alloc_arena_release(a), 0);
    }

    /// A reused slot is a *different* arena: its handle differs and its totals
    /// start at zero.
    #[test]
    fn a_reused_slot_issues_a_different_handle() {
        let _alone = arena_alone();
        let first = buri_rt_alloc_arena_create();
        assert_eq!(buri_rt_alloc_arena_allocate(first, 8), 8);
        assert_eq!(buri_rt_alloc_arena_count(first), 1);
        let _ = buri_rt_alloc_arena_release(first);
        let second = buri_rt_alloc_arena_create();
        assert_ne!(first, second, "a handle is never issued twice");
        assert_eq!(buri_rt_alloc_arena_count(second), 0);
        assert_eq!(buri_rt_alloc_arena_total(second), 0);
        let _ = buri_rt_alloc_arena_release(second);
    }

    /// Two scopes open at once are two arenas: neither sees the other's
    /// charges, which is what makes a scope per task expressible.
    #[test]
    fn two_open_scopes_count_separately() {
        let _alone = arena_alone();
        let a = buri_rt_alloc_arena_create();
        let b = buri_rt_alloc_arena_create();
        assert_eq!(buri_rt_alloc_arena_allocate(a, 10), 10);
        assert_eq!(buri_rt_alloc_arena_allocate(b, 20), 20);
        assert_eq!(buri_rt_alloc_arena_allocate(b, 2), 2);
        assert_eq!((buri_rt_alloc_arena_count(a), buri_rt_alloc_arena_total(a)), (1, 10));
        assert_eq!((buri_rt_alloc_arena_count(b), buri_rt_alloc_arena_total(b)), (2, 22));
        let _ = buri_rt_alloc_arena_release(a);
        let _ = buri_rt_alloc_arena_release(b);
    }

    /// **A burst of scopes leaves nothing mapped and nothing in the table.**
    ///
    /// The workload `scoped` is for is one scope per request, so the slot
    /// table has to be bounded rather than growing by an entry per call. A
    /// thousand scopes here take at most as many slots as are open at once,
    /// which is one.
    #[test]
    fn a_thousand_scopes_leave_no_pages_and_no_slots_behind() {
        let _alone = arena_alone();
        let (live_before, _) = arena_stats();
        let table_before = arenas(|t| t.len());
        for _ in 0..1000 {
            let a = buri_rt_alloc_arena_create();
            assert_eq!(buri_rt_alloc_arena_allocate(a, 4096), 4096);
            assert_eq!(buri_rt_alloc_arena_release(a), BURI_RT_ARENA_BLOCK as i64);
        }
        assert_eq!(arena_stats().0, live_before, "every scope gave its pages back");
        // Other cases on other threads may have grown the table meanwhile, so
        // the claim is that *this* loop did not grow it by a thousand.
        assert!(
            arenas(|t| t.len()) < table_before + 1000,
            "the slot table grew with the number of scopes"
        );
    }

    /// A handle this runtime never issued is answered quietly, the way
    /// [`index`] answers an unknown counter: a wrong number is a wrong number,
    /// and an abort here would be a runtime that crashes on a value the
    /// language says cannot exist.
    #[test]
    fn an_unknown_handle_is_quiet() {
        assert_eq!(buri_rt_alloc_arena_allocate(i64::MAX, 16), 16);
        assert_eq!(buri_rt_alloc_arena_count(i64::MAX), 0);
        assert_eq!(buri_rt_alloc_arena_total(i64::MAX), 0);
        assert_eq!(buri_rt_alloc_arena_release(i64::MAX), 0);
    }

    /// The packing is `cli/src/compiler/backend/js/runtime.js`'s too, so it is
    /// pinned rather than left to two readers of one comment.
    #[test]
    fn a_handle_packs_the_generation_above_the_slot() {
        assert_eq!(arena_slot(arena_handle(0, 0)), (0, 0));
        assert_eq!(arena_slot(arena_handle(7, 3)), (7, 3));
        assert_eq!(arena_slot(arena_handle(u32::MAX as usize, u32::MAX)), (u32::MAX as usize, u32::MAX));
    }

    /// The block length is a whole number of blocks and never zero: a mapping
    /// of no bytes is a failed `mmap` on every platform this runtime admits.
    #[test]
    fn a_block_length_is_a_whole_number_of_blocks() {
        assert_eq!(arena_block_len(0), BURI_RT_ARENA_BLOCK);
        assert_eq!(arena_block_len(1), BURI_RT_ARENA_BLOCK);
        assert_eq!(arena_block_len(BURI_RT_ARENA_BLOCK), BURI_RT_ARENA_BLOCK);
        assert_eq!(arena_block_len(BURI_RT_ARENA_BLOCK + 1), BURI_RT_ARENA_BLOCK * 2);
        // Sixteen kibibytes is arm64 macOS's page, so every block is a whole
        // number of pages on both platforms.
        assert_eq!(BURI_RT_ARENA_BLOCK % (16 * 1024), 0);
    }

    /// **Nothing an arena maps is a block the heap accounting knows about.**
    ///
    /// This is the interim soundness rule, as an assertion: an arena's pages
    /// are its own mapping, no `buri_rt_alloc` block is inside one, and the
    /// four counts above `arena_bytes` do not move when a scope reserves a
    /// megabyte. It is what makes the release unconditional — there is nothing
    /// in there for a reference count to own.
    #[test]
    fn an_arena_maps_nothing_the_heap_counts() {
        let _alone = arena_alone();
        let mut before = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
            arena_bytes: 0,
            arena_released_bytes: 0,
        };
        // SAFETY: a live, aligned destination.
        unsafe { buri_rt_heap_stats(&raw mut before) };
        let a = buri_rt_alloc_arena_create();
        let _ = buri_rt_alloc_arena_allocate(a, 1024 * 1024);
        let mut after = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
            arena_bytes: 0,
            arena_released_bytes: 0,
        };
        // SAFETY: as above.
        unsafe { buri_rt_heap_stats(&raw mut after) };
        // Not an equality: `cargo test`'s other threads are allocating while
        // this runs. The claim is about the *megabyte* — whatever else the
        // binary did meanwhile, none of it was this arena's reservation
        // arriving on the heap.
        assert!(
            after.total_bytes - before.total_bytes < 1024 * 1024,
            "an arena's reservation reached the heap: {} bytes of blocks",
            after.total_bytes - before.total_bytes
        );
        assert!(after.arena_bytes >= before.arena_bytes + 1024 * 1024);
        let _ = buri_rt_alloc_arena_release(a);
    }

    // === G4 end ============================================================

    // === G5 begin: the copy out of a scope =================================

    /// The blocks [`a_returning_scope_gives_its_pages_back_and_leaks_nothing`]
    /// puts inside a scope: 64 of a quarter-megabyte each, sixteen megabytes in
    /// all.
    ///
    /// **Big on purpose.** `live_bytes` is process-wide and `cargo test` runs
    /// its cases on many threads, so a claim about it is only as good as the
    /// margin between what this case allocates and what the rest of the binary
    /// is doing meanwhile. Sixteen megabytes against a suite whose other blocks
    /// are kilobytes is a margin no scheduling can close, and it is the same
    /// answer `an_arena_maps_nothing_the_heap_counts` gives to the same
    /// problem: assert a magnitude, not an equality.
    const SCOPE_BLOCKS: usize = 64;
    const SCOPE_BLOCK_BYTES: u64 = 256 * 1024;
    const SCOPE_TOTAL: i128 = (SCOPE_BLOCKS as i128) * (SCOPE_BLOCK_BYTES as i128);

    /// Counts the calls a copy glue makes, so that a case can say *whether*
    /// the type's own walk ran and not only that the bytes moved.
    static GLUE_CALLS: AtomicU64 = AtomicU64::new(0);

    /// A copy glue that records that it was called and copies nothing: a block
    /// of bytes has nothing inside it, and what is under test here is the
    /// dispatch.
    extern "C" fn counting_glue(_p: *mut u8) {
        GLUE_CALLS.fetch_add(1, Ordering::Relaxed);
    }

    /// Writes `n` bytes of a recognisable pattern into a payload.
    ///
    /// # Safety
    /// `p` names at least `n` writable bytes.
    unsafe fn fill(p: *mut u8, n: u64) {
        for i in 0..n {
            // SAFETY: the caller promises `n` writable bytes.
            unsafe { p.add(i as usize).write((i % 251) as u8) };
        }
    }

    /// **A copy is not a share**, which is the property the whole slice exists
    /// to have.
    ///
    /// Three assertions and each is one of the ways the alternative would show
    /// up: the two pointers differ, the *source's* count did not move — a
    /// `buri_rt_incref` in the copy path would be the obvious wrong
    /// implementation, and it would leave the source at three — and the copy's
    /// count is one, which is what makes it uniquely owned and therefore
    /// eligible for MEMORY.md §5.3's in-place write.
    #[test]
    fn a_copy_is_not_a_share() {
        let p = buri_rt_alloc(64);
        // SAFETY: live, just allocated.
        unsafe {
            fill(p, 64);
            buri_rt_incref(p);
            buri_rt_incref(p);
            assert_eq!(buri_rt_rc(p), 3);

            let q = buri_rt_copy_block(p, None);
            assert_ne!(p, q, "a copy answered the block it was given");
            assert_eq!(buri_rt_rc(p), 3, "the copy took a reference on the source");
            assert_eq!(buri_rt_rc(q), 1, "a copy is not uniquely owned");
            assert_eq!(buri_rt_cap(q), 64, "a copy did not keep the source's capacity");
            for i in 0..64u64 {
                assert_eq!(
                    q.add(i as usize).read(),
                    (i % 251) as u8,
                    "a copy did not hold the source's bytes"
                );
            }

            buri_rt_free(q);
            buri_rt_decref(p, None);
            buri_rt_decref(p, None);
            buri_rt_decref(p, None);
        }
    }

    /// The copy glue is what makes the copy *deep*, and it is called on the
    /// **fresh** block: the walk's job is to replace the pointers the memcpy
    /// duplicated, which are in the new block and not in the old one.
    #[test]
    fn the_copy_glue_runs_on_the_new_block() {
        let p = buri_rt_alloc(32);
        let before = GLUE_CALLS.load(Ordering::Relaxed);
        // SAFETY: live, just allocated.
        unsafe {
            let q = buri_rt_copy_block(p, Some(counting_glue));
            assert_eq!(
                GLUE_CALLS.load(Ordering::Relaxed) - before,
                1,
                "the copy glue was not called"
            );
            buri_rt_free(q);
            buri_rt_free(p);
        }
    }

    /// A null block copies to a null block, which is what lets an `Option`'s
    /// niche and a static `Str`'s absent base need no test in the emitted walk.
    #[test]
    fn a_null_block_copies_to_null() {
        // SAFETY: null is an accepted argument.
        let q = unsafe { buri_rt_copy_block(std::ptr::null_mut(), None) };
        assert!(q.is_null());
    }

    /// An `IMMORTAL` block copies to an ordinary counted one.
    ///
    /// Right, and worth a case: the copy is a *new value*, and nothing about
    /// the original's immortality is a property of it. A copy that inherited
    /// the sentinel would be a block that could never be freed, once per
    /// literal that ever left a scope.
    #[test]
    fn an_immortal_block_copies_to_a_counted_one() {
        let p = buri_rt_alloc(16);
        // SAFETY: live, just allocated.
        unsafe {
            buri_rt_make_immortal(p);
            assert_eq!(buri_rt_rc(p), BURI_RT_IMMORTAL);
            let q = buri_rt_copy_block(p, None);
            assert_eq!(buri_rt_rc(q), 1);
            buri_rt_free(q);
        }
    }

    /// **A `Str`'s `ptr` is rebased onto the copy.**
    ///
    /// `Str` is `{ base, ptr, len }` and `ptr` points *into* `base`
    /// (VALUE-MODEL.md §3), so a copy that replaced only the block would leave
    /// the value addressing the block the scope is about to unmap — a
    /// use-after-free that reads correctly until the pages go back, which is
    /// the worst kind.
    #[test]
    fn copying_a_str_rebases_the_pointer_into_the_copy() {
        let base = buri_rt_alloc(16);
        // SAFETY: live, just allocated, and sixteen bytes wide.
        unsafe { fill(base, 16) };
        let mut v = crate::value::BuriStr {
            base,
            // A view four bytes into its own block, which is what `slice` and
            // `splitOnce` hand back.
            ptr: base.wrapping_add(4).cast_const(),
            len: 8,
        };
        // SAFETY: `v` is a live, writable `BuriStr`.
        unsafe { buri_rt_copy_str((&raw mut v).cast::<u8>()) };
        assert_ne!(v.base, base, "the copy kept the source's block");
        assert_eq!(v.len, 8, "the copy changed the length");
        assert_eq!(
            v.ptr.addr() - v.base.addr(),
            4,
            "the copy did not keep the view's offset"
        );
        // SAFETY: the copy is live and this test holds the only reference.
        unsafe {
            for i in 0..8u64 {
                assert_eq!(v.ptr.add(i as usize).read(), (i + 4) as u8);
            }
            buri_rt_free(v.base);
            buri_rt_free(base);
        }
    }

    /// A `Str` with no base is a static — a literal, or the empty string — and
    /// is copied by being left alone.
    #[test]
    fn copying_a_static_str_leaves_it_alone() {
        static BYTES: [u8; 4] = [1, 2, 3, 4];
        let mut v = crate::value::BuriStr {
            base: std::ptr::null_mut(),
            ptr: BYTES.as_ptr(),
            len: 4,
        };
        // SAFETY: `v` is a live, writable `BuriStr`.
        unsafe { buri_rt_copy_str((&raw mut v).cast::<u8>()) };
        assert!(v.base.is_null());
        assert_eq!(v.ptr, BYTES.as_ptr(), "a static's bytes moved");
    }

    // -- the arena a carrier is inside --------------------------------------

    /// **A scope serves the blocks its body allocates**, which is the half of
    /// G4's interim rule this slice lifts.
    ///
    /// Three claims: the block carries [`BURI_RT_CAP_ARENA`], the arena mapped
    /// pages for it, and the platform allocator was not involved — which the
    /// last assertion says by releasing the arena and finding the block's bytes
    /// gone with it rather than returned to `malloc`.
    #[test]
    fn a_scope_serves_the_blocks_its_body_allocates() {
        let _alone = arena_alone();
        let (live_before, released_before) = arena_stats();
        let a = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(a);

        let p = buri_rt_alloc(128);
        // SAFETY: live, just allocated.
        unsafe {
            assert!(is_arena(header(p)), "a block inside a scope is not the scope's");
            assert_eq!(buri_rt_cap(p), 128, "the arena bit cost the block its capacity");
            assert_eq!(buri_rt_rc(p), 1);
            fill(p, 128);
        }
        let (live_open, _) = arena_stats();
        assert_eq!(
            live_open - live_before,
            BURI_RT_ARENA_BLOCK as u64,
            "a block inside a scope mapped no pages"
        );

        // A block made *outside* the scope again is the platform's.
        let _ = buri_rt_alloc_arena_leave(outer);
        let q = buri_rt_alloc(128);
        // SAFETY: live, just allocated.
        unsafe {
            assert!(!is_arena(header(q)), "a block outside a scope was charged to one");
            buri_rt_free(q);
        }

        // SAFETY: the only reference, and its pages are still mapped.
        unsafe { buri_rt_free(p) };
        let freed = buri_rt_alloc_arena_release(a);
        assert_eq!(freed, BURI_RT_ARENA_BLOCK as i64);
        let (live_after, released_after) = arena_stats();
        assert_eq!(live_after, live_before);
        assert_eq!(released_after - released_before, BURI_RT_ARENA_BLOCK as u64);
    }

    /// **The acceptance property, for a scope that answers a value**: the pages
    /// go back, and the platform heap is where it started.
    ///
    /// A megabyte of blocks inside the scope, one of them copied out, and then
    /// the release. `arena_bytes` back where it started is the pages; the live
    /// heap back where it started is the accounting — a block a scope served is
    /// not a leak, and `buri_rt_free`'s early return has to take it out of
    /// `live_blocks` on the way past or every scope in the process would look
    /// like one.
    #[test]
    fn a_returning_scope_gives_its_pages_back_and_leaks_nothing() {
        let _alone = arena_alone();
        let (live_before, released_before) = arena_stats();
        let mut heap = BuriHeapStats {
            live_blocks: 0,
            live_bytes: 0,
            total_blocks: 0,
            total_bytes: 0,
            retained_bytes: 0,
            decommitted_bytes: 0,
            arena_bytes: 0,
            arena_released_bytes: 0,
        };
        // SAFETY: a live, aligned destination.
        unsafe { buri_rt_heap_stats(&raw mut heap) };
        let total_before = i128::from(heap.total_bytes);

        let a = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(a);
        let mut inside = Vec::new();
        for _ in 0..SCOPE_BLOCKS {
            let p = buri_rt_alloc(SCOPE_BLOCK_BYTES);
            // SAFETY: live, just allocated.
            unsafe { fill(p, 64) };
            inside.push(p);
        }
        // A block a scope served is **counted like any other while it is
        // alive**, which is what keeps the leak checks honest: a scope is not a
        // way for sixteen megabytes to become invisible. `total_bytes` never
        // falls, so this direction needs no margin at all.
        // SAFETY: a live, aligned destination.
        unsafe { buri_rt_heap_stats(&raw mut heap) };
        let live_with = i128::from(heap.live_bytes);
        assert!(
            i128::from(heap.total_bytes) - total_before >= SCOPE_TOTAL,
            "the blocks a scope served were not counted at all"
        );
        let (live_open, _) = arena_stats();
        assert!(
            live_open - live_before >= SCOPE_TOTAL as u64,
            "sixteen megabytes of blocks inside a scope mapped {} bytes",
            live_open - live_before
        );

        // The answer leaves the scope, and it leaves it as a copy.
        let _ = buri_rt_alloc_arena_leave(outer);
        // SAFETY: `inside[0]` is live and this test holds the only reference.
        let answer = unsafe { buri_rt_copy_block(inside[0], None) };
        // SAFETY: live, just allocated outside the scope.
        unsafe { assert!(!is_arena(header(answer)), "the copy landed in the arena") };

        for p in inside {
            // SAFETY: each is live and this is its last reference.
            unsafe { buri_rt_free(p) };
        }
        let freed = buri_rt_alloc_arena_release(a);
        assert!(freed >= SCOPE_TOTAL as i64);
        let (live_after, released_after) = arena_stats();
        assert_eq!(live_after, live_before, "the scope's pages did not go back");
        assert!(released_after - released_before >= SCOPE_TOTAL as u64);

        // And it is **uncounted on the way out**, even though nothing was
        // handed back to `malloc`: `buri_rt_free`'s early return does the
        // accounting before it sees the arena bit, or every scope in the
        // process would read as a leak.
        //
        // Not an equality, for `an_arena_maps_nothing_the_heap_counts`'s
        // reason: `cargo test`'s other threads are allocating while this runs.
        // The claim is about the *megabyte* — whatever else the binary did
        // meanwhile, none of it was these 256 blocks staying live.
        //
        // SAFETY: a live, aligned destination.
        unsafe { buri_rt_heap_stats(&raw mut heap) };
        assert!(
            live_with - i128::from(heap.live_bytes) >= SCOPE_TOTAL / 2,
            "a scope's blocks stayed live after it released them: {} bytes went away",
            live_with - i128::from(heap.live_bytes)
        );
        // SAFETY: the only reference to the copy.
        unsafe { buri_rt_free(answer) };
    }

    /// **A scope's block never enters the platform allocator's free list.**
    ///
    /// The carrier cache is what would have taken it — `buri_rt_free` files a
    /// small block on a thread-local list rather than calling `dealloc` — and
    /// handing it a block whose pages are about to be unmapped would be a
    /// use-after-free at the *next* allocation of that size, on a thread that
    /// had nothing to do with the scope. The test is that a block of exactly
    /// the size the arena served comes back from `malloc` and not from the
    /// arena's mappings after the release.
    #[test]
    fn a_scope_block_does_not_reach_the_carrier_cache() {
        let _alone = arena_alone();
        let a = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(a);
        let p = buri_rt_alloc(200);
        let inside = p.addr();
        // SAFETY: the only reference.
        unsafe { buri_rt_free(p) };
        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(a);

        let q = buri_rt_alloc(200);
        assert_ne!(q.addr(), inside, "a released arena's block came back from the cache");
        // SAFETY: live, and the only reference.
        unsafe {
            assert!(!is_arena(header(q)));
            buri_rt_free(q);
        }
    }

    /// A bump allocator cannot grow what it handed out, so an arena block is
    /// grown by allocating, copying and abandoning — and the abandoned one is
    /// taken out of the live accounting rather than handed to `realloc`.
    #[test]
    fn growing_a_scope_block_allocates_a_new_one() {
        let _alone = arena_alone();
        let a = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(a);
        let p = buri_rt_alloc(64);
        // SAFETY: live, just allocated, and the only reference.
        let grown = unsafe {
            fill(p, 64);
            buri_rt_incref(p);
            buri_rt_realloc(p, 256)
        };
        assert_ne!(grown, p, "an arena block was grown in place");
        // SAFETY: live.
        unsafe {
            assert!(is_arena(header(grown)), "a grown arena block left the arena");
            assert_eq!(buri_rt_cap(grown), 256);
            assert_eq!(buri_rt_rc(grown), 2, "the count did not travel with the value");
            for i in 0..64u64 {
                assert_eq!(grown.add(i as usize).read(), (i % 251) as u8);
            }
            buri_rt_decref(grown, None);
            buri_rt_decref(grown, None);
        }
        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(a);
    }

    /// Scopes nest, and `arenaLeave` puts back exactly what `arenaEnter`
    /// answered — including "there was no scope", which is what makes the
    /// encoding a biased handle rather than a handle.
    #[test]
    fn scopes_nest_and_leave_puts_back_what_enter_answered() {
        let _alone = arena_alone();
        assert!(current_arena().is_none(), "a test began inside a scope");
        let outer_arena = buri_rt_alloc_arena_create();
        let inner_arena = buri_rt_alloc_arena_create();

        let none = buri_rt_alloc_arena_enter(outer_arena);
        assert_eq!(none, 0, "the encoding of `no arena` is not zero");
        assert_eq!(current_arena(), Some(outer_arena));

        let was_outer = buri_rt_alloc_arena_enter(inner_arena);
        assert_eq!(current_arena(), Some(inner_arena));

        let _ = buri_rt_alloc_arena_leave(was_outer);
        assert_eq!(current_arena(), Some(outer_arena), "leaving the inner scope lost the outer");

        let _ = buri_rt_alloc_arena_leave(none);
        assert!(current_arena().is_none(), "leaving the outer scope left one behind");

        let _ = buri_rt_alloc_arena_release(inner_arena);
        let _ = buri_rt_alloc_arena_release(outer_arena);
    }

    /// A **retired** scope serves nothing: the request goes to the platform
    /// allocator, which is the same quiet answer a stale handle gets from
    /// `buri_rt_alloc_arena_allocate`. A body that handed its `Scoped` back out
    /// and allocated through it afterwards is the shape.
    #[test]
    fn a_retired_scope_serves_no_blocks() {
        let _alone = arena_alone();
        let a = buri_rt_alloc_arena_create();
        let _ = buri_rt_alloc_arena_release(a);
        let outer = buri_rt_alloc_arena_enter(a);
        let p = buri_rt_alloc(48);
        // SAFETY: live, just allocated.
        unsafe {
            assert!(!is_arena(header(p)), "a retired scope served a block");
            buri_rt_free(p);
        }
        let _ = buri_rt_alloc_arena_leave(outer);
    }

    /// **The arena is a property of the carrier, not of the process.** A second
    /// thread inside no scope allocates from the platform heap while this one
    /// is inside one — which is what makes a scope per request safe, and what
    /// makes a task started inside a scope allocate on the heap.
    #[test]
    fn a_scope_is_this_carriers_and_no_other_threads() {
        let _alone = arena_alone();
        let a = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(a);
        let mine = buri_rt_alloc(80);
        // SAFETY: live, just allocated.
        unsafe { assert!(is_arena(header(mine))) };

        let elsewhere = std::thread::spawn(|| {
            assert!(current_arena().is_none(), "a fresh carrier began inside a scope");
            let p = buri_rt_alloc(80);
            // SAFETY: live, just allocated on this thread.
            let charged = unsafe { is_arena(header(p)) };
            // SAFETY: the only reference.
            unsafe { buri_rt_free(p) };
            charged
        })
        .join();
        assert_eq!(elsewhere.ok(), Some(false), "another carrier was inside this scope");

        // SAFETY: the only reference.
        unsafe { buri_rt_free(mine) };
        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(a);
    }

    // -- G3's mark, and what a copy inherits of it --------------------------

    /// **A copied-out value's mark is its own, not its source's.**
    ///
    /// The acceptance asks that a copy's mark reflect its *post*-copy
    /// reachability, and this is what that means given G3: the mark is answered
    /// per program by the latch, and the copy is a fresh allocation that goes
    /// through `finish` like any other — so it asks the latch again rather than
    /// inheriting a bit. A source marked because it had crossed a task boundary
    /// does not hand that history to a copy that stays where it was made.
    ///
    /// Both directions, because either alone would pass for the wrong reason:
    /// with the latch cold a copy of a marked block is **unmarked**, and with
    /// the latch set a copy is marked whatever the source was.
    #[test]
    fn a_copy_asks_the_latch_again_rather_than_inheriting_a_mark() {
        let _latch = latch();
        assert!(!values_may_cross_tasks(), "the silent answer is the safe one");

        // A source with the mark set by hand, as a value that had crossed a
        // task boundary would carry it.
        let source = buri_rt_alloc(64);
        // SAFETY: live, just allocated.
        unsafe {
            (*header(source)).cap |= BURI_RT_CAP_SHARED;
            assert_eq!(count_and_mark(source), (1, true));

            let cold = buri_rt_copy_block(source, None);
            assert_eq!(
                count_and_mark(cold),
                (1, false),
                "a copy that stays task-local inherited its source's mark"
            );
            assert_eq!(buri_rt_cap(cold), 64, "the mark cost the copy its capacity");
            // And it is therefore eligible for the in-place write its source is
            // not, which is the whole practical consequence of the bit.
            assert_eq!(buri_rt_unique_cap(cold), Some(64));
            assert_eq!(buri_rt_unique_cap(source), None);
            buri_rt_free(cold);
        }

        buri_rt_values_may_cross_tasks();
        // SAFETY: live.
        unsafe {
            let warm = buri_rt_copy_block(source, None);
            assert_eq!(
                count_and_mark(warm),
                (1, true),
                "a copy in a program whose values cross tasks came back cold"
            );
            buri_rt_free(warm);

            // An *unmarked* source in the same program copies to a marked
            // block too: the question is the program's, not the block's.
            (*header(source)).cap &= BURI_RT_CAP_MASK;
            let second = buri_rt_copy_block(source, None);
            assert_eq!(count_and_mark(second), (1, true));
            buri_rt_free(second);
        }

        forget_values_may_cross_tasks();
        // SAFETY: the only reference.
        unsafe { buri_rt_free(source) };
    }

    /// **The two flag bits are independent, and neither costs the other its
    /// meaning.**
    ///
    /// This is the one case that holds *both* locks, and it holds them because
    /// it is the one case claiming the two mechanisms interact — which G4's
    /// note about `arena_alone` says is the only reason to. The claim is that
    /// they interact in exactly one place and no other: `buri_rt_unique_cap`
    /// answers on `CAP_SHARED` and is blind to `CAP_ARENA`, so a scope's block
    /// in an ordinary program is uniquely owned and eligible for the in-place
    /// write, and the same block in a program whose values may cross tasks is
    /// not. The capacity reads back under either.
    #[test]
    fn the_arena_bit_and_the_mark_are_independent() {
        let _latch = latch();
        let _alone = arena_alone();
        assert!(!values_may_cross_tasks(), "the silent answer is the safe one");
        let a = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(a);

        // Cold: the scope's block is the scope's, and is unique.
        let cold = buri_rt_alloc(72);
        // SAFETY: live, just allocated.
        unsafe {
            assert!(is_arena(header(cold)), "a block inside a scope is not the scope's");
            assert!(!is_shared(header(cold)), "an unmarked program marked a block");
            assert_eq!(buri_rt_cap(cold), 72, "the arena bit cost the block its capacity");
            assert_eq!(
                buri_rt_unique_cap(cold),
                Some(72),
                "the arena bit was read as the multi-threaded mark"
            );
            buri_rt_free(cold);
        }

        // Latched: both bits, and the uniqueness test refuses.
        buri_rt_values_may_cross_tasks();
        let warm = buri_rt_alloc(72);
        // SAFETY: live, just allocated.
        unsafe {
            assert!(is_arena(header(warm)), "a block inside a scope is not the scope's");
            assert!(is_shared(header(warm)), "a block inside a scope escaped the mark");
            assert_eq!(buri_rt_cap(warm), 72, "two flags cost the block its capacity");
            assert_eq!(buri_rt_unique_cap(warm), None, "a marked block passed the unique test");
            buri_rt_free(warm);
        }

        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(a);
        forget_values_may_cross_tasks();
    }

    /// **A scope reuses the block the last scope gave back**, which is the
    /// difference between the feature and a pessimisation.
    ///
    /// Without the pool, a scope is an `mmap` and a `munmap` — 2.4 µs a scope
    /// measured, and 2.2× the run time of the same program without scopes over
    /// 500,000 of them. With it, the common path makes no system call at all.
    ///
    /// The assertion is the address: a second scope that mapped its own block
    /// would get one the kernel chose, and this one gets the block the first
    /// scope was using.
    #[test]
    fn a_scope_takes_the_block_the_last_one_gave_back() {
        let _alone = arena_alone();
        // Drain whatever other cases left, so the first scope below maps
        // rather than pops and the addresses are this case's own.
        arena_pool(Vec::clear);

        let first = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(first);
        let p = buri_rt_alloc(64);
        let was = p.addr();
        // SAFETY: the only reference.
        unsafe { buri_rt_free(p) };
        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(first);

        let second = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(second);
        let q = buri_rt_alloc(64);
        assert_eq!(q.addr(), was, "a scope mapped a block the pool was holding");
        // SAFETY: the only reference.
        unsafe { buri_rt_free(q) };
        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(second);
    }

    /// The pool is **bounded**, and past the bound a release is a `munmap`.
    ///
    /// Nine standard blocks in one arena: eight go to the pool and the ninth
    /// goes back to the kernel, so the pool holds what it says it holds and a
    /// scope that took a megabyte does not leave a megabyte behind.
    #[test]
    fn the_arena_pool_is_bounded() {
        let _alone = arena_alone();
        arena_pool(Vec::clear);
        let a = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(a);
        // One allocation just under a block each time, so each takes a block
        // of its own and none of them shares a window.
        let mut held = Vec::new();
        for _ in 0..(ARENA_POOL_MAX + 1) {
            held.push(buri_rt_alloc((BURI_RT_ARENA_BLOCK - 64) as u64));
        }
        for p in held {
            // SAFETY: each is live and this is its last reference.
            unsafe { buri_rt_free(p) };
        }
        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(a);
        assert_eq!(
            arena_pool(|pool| pool.len()),
            ARENA_POOL_MAX,
            "the pool kept more or fewer blocks than it says it does"
        );
        arena_pool(Vec::clear);
    }

    /// **A pooled block holds the last scope's bytes**, so the zeroing
    /// allocator has to zero it.
    ///
    /// The un-pooled case is why this is easy to get wrong: a mapping fresh
    /// from the kernel is zero-filled and an arena's window only moves forward,
    /// so before the pool existed `alloc_zeroed` in a scope could be a bump and
    /// nothing else — and it was.
    #[test]
    fn a_zeroed_block_in_a_scope_is_zero_even_out_of_the_pool() {
        let _alone = arena_alone();
        arena_pool(Vec::clear);

        let first = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(first);
        let p = buri_rt_alloc(64);
        // SAFETY: live, sixty-four writable bytes, and the only reference.
        unsafe {
            fill(p, 64);
            buri_rt_free(p);
        }
        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(first);

        let second = buri_rt_alloc_arena_create();
        let outer = buri_rt_alloc_arena_enter(second);
        let q = buri_rt_alloc_zeroed(64);
        // SAFETY: live, just allocated.
        unsafe {
            for i in 0..64usize {
                assert_eq!(q.add(i).read(), 0, "a zeroed block came back with byte {i} set");
            }
            buri_rt_free(q);
        }
        let _ = buri_rt_alloc_arena_leave(outer);
        let _ = buri_rt_alloc_arena_release(second);
        arena_pool(Vec::clear);
    }

    // === G5 end ============================================================


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
