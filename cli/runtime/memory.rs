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

/// `{ rc, cap }`, immediately before the payload.
#[repr(C)]
struct Header {
    rc: u64,
    cap: u64,
}

// The one place this runtime is atomic, and it is not on the reference-count
// path: the counts themselves are open-coded plain loads and stores, because
// the language has no threads (MEMORY.md §1). These four are here so that
// `cli/tests/memory.rs` can assert "every allocation is freed at exit" — the
// test MEMORY.md §2 asks for to defend the acyclicity lemma — and a relaxed
// add next to a `malloc` is not a cost anybody can measure.
static LIVE_BLOCKS: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL_BLOCKS: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES: AtomicU64 = AtomicU64::new(0);

/// What [`buri_rt_heap_stats`] writes. Four `u64`s, in this order.
#[repr(C)]
pub struct BuriHeapStats {
    pub live_blocks: u64,
    pub live_bytes: u64,
    pub total_blocks: u64,
    pub total_bytes: u64,
}

#[inline]
fn layout_for(payload: u64) -> Layout {
    let bytes = (payload as usize).checked_add(BURI_RT_HEADER);
    match bytes.and_then(|b| Layout::from_size_align(b, BURI_RT_ALIGN).ok()) {
        Some(l) => l,
        // A request that cannot even be described is out of memory by any
        // useful definition, and there is no value to report it with:
        // `allocate` returns `Region`, not `Result` (`cap.buri:19`).
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
    let layout = layout_for(payload);
    // SAFETY: `layout` has a non-zero size — the header alone is 16 bytes.
    let raw = unsafe { alloc(layout) };
    finish(raw, payload)
}

/// [`buri_rt_alloc`], with the payload zeroed.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_alloc_zeroed(payload: u64) -> *mut u8 {
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
        raw.cast::<Header>().write(Header { rc: 1, cap: payload });
    }
    LIVE_BLOCKS.fetch_add(1, Ordering::Relaxed);
    LIVE_BYTES.fetch_add(payload, Ordering::Relaxed);
    TOTAL_BLOCKS.fetch_add(1, Ordering::Relaxed);
    TOTAL_BYTES.fetch_add(payload, Ordering::Relaxed);
    // SAFETY: the block is `BURI_RT_HEADER + payload` bytes, so the payload
    // start is one-past-the-header and in bounds.
    unsafe { raw.add(BURI_RT_HEADER) }
}

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
    let (old_cap, rc) = unsafe {
        let h = header(p);
        ((*h).cap, (*h).rc)
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
        raw.cast::<Header>().write(Header { rc, cap: payload });
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
        (*h).cap
    };
    LIVE_BLOCKS.fetch_sub(1, Ordering::Relaxed);
    LIVE_BYTES.fetch_sub(cap, Ordering::Relaxed);
    // SAFETY: `p - 16` is the allocation, and `layout_for(cap)` is the layout
    // it was created with — `cap` is stored precisely so this is recoverable.
    unsafe { dealloc(p.sub(BURI_RT_HEADER), layout_for(cap)) }
}

/// The non-inlined `incref`. Saturating, so `IMMORTAL` is a fixed point.
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
        (*h).rc = (*h).rc.saturating_add(1);
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
    let rc = unsafe {
        let h = header(p);
        let rc = (*h).rc;
        if rc == BURI_RT_IMMORTAL {
            return;
        }
        if rc > 1 {
            (*h).rc = rc - 1;
        }
        rc
    };
    if rc <= 1 {
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
        LIVE_BYTES.fetch_sub((*h).cap, Ordering::Relaxed);
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
    unsafe { (*header(p)).cap }
}

/// Heap accounting, for `cli/tests/memory.rs` and for `--explain`.
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
#[must_use]
pub fn buri_rt_grown_capacity(needed: u64, old_cap: u64) -> u64 {
    needed.max(old_cap.saturating_mul(2)).max(BURI_RT_GROWTH_FLOOR)
}

/// `Some(cap)` when `p` is a live block that **nothing else references**.
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
/// # Safety
/// `p` is null or a live payload pointer from [`buri_rt_alloc`].
#[must_use]
pub unsafe fn buri_rt_unique_cap(p: *const u8) -> Option<u64> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller promises a live payload pointer, so the header is in
    // bounds and aligned.
    let h = unsafe { &*header(p.cast_mut()) };
    (h.rc == 1).then_some(h.cap)
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
