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

/// Bit 63 of `cap`: **reserved** for the multi-threaded mark, and never set.
///
/// Set will mean "this block may be reached from more than one thread" — the
/// question `incref`/`decref` will branch on to choose an atomic count.
/// Nothing sets it yet. It is reserved here, and every reader masks, so that
/// turning it on later moves no code but the code that turns it on.
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
/// **It never does today.** G1 reserved the bit and nothing sets it; G3 is
/// what turns it on. Until then every reference operation below takes the
/// unshared arm, and the atomic arms are reachable only from this file's own
/// tests, which force the bit.
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

/// One carrier's cache: a free-list head per exact payload size, and the bytes
/// they hold between them.
///
/// A dead block's own header carries the link — `rc` holds the next block's
/// payload pointer, `cap` is left alone — so the lists cost sixteen bytes of
/// list state per thread and not one byte per block.
struct Cache {
    heads: [*mut u8; CACHE_SLOTS],
    held: u64,
}

thread_local! {
    static CACHE: std::cell::UnsafeCell<Cache> = const {
        std::cell::UnsafeCell::new(Cache { heads: [std::ptr::null_mut(); CACHE_SLOTS], held: 0 })
    };
}

/// A dead block of exactly `payload` usable bytes from this thread's cache.
fn cache_pop(payload: u64) -> Option<*mut u8> {
    if payload > CACHE_MAX_PAYLOAD {
        return None;
    }
    // `try_with` rather than `with`: a block freed while this thread's
    // destructors run must not panic, and the answer there is simply "no
    // cache".
    CACHE
        .try_with(|c| {
            // SAFETY: `c` is this thread's own cell, and no reference derived
            // from it escapes this closure or crosses a call that could
            // re-enter.
            let cache = unsafe { &mut *c.get() };
            let slot = cache.heads.get_mut(payload as usize)?;
            let p = *slot;
            if p.is_null() {
                return None;
            }
            // SAFETY: every pointer in a slot is a block this file freed, of
            // exactly `payload` usable bytes, whose `rc` holds the next one.
            *slot = unsafe { (*header(p)).rc as *mut u8 };
            cache.held = cache.held.saturating_sub(payload.saturating_add(BURI_RT_HEADER as u64));
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
    CACHE
        .try_with(|c| {
            // SAFETY: as in `cache_pop`.
            let cache = unsafe { &mut *c.get() };
            let bytes = cap.saturating_add(BURI_RT_HEADER as u64);
            if cache.held.saturating_add(bytes) > cache_budget() {
                return false;
            }
            let Some(slot) = cache.heads.get_mut(cap as usize) else {
                return false;
            };
            // SAFETY: the caller promises a dead block, so its header is this
            // file's to use as list storage.
            unsafe {
                (*header(p)).rc = *slot as u64;
            }
            *slot = p;
            cache.held = cache.held.saturating_add(bytes);
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
    // SAFETY: as above. The load is relaxed rather than plain so that the test
    // is defined on a block carrying [`BURI_RT_CAP_SHARED`], where another
    // thread's `atomicrmw` may be running on the same word. **The answer does
    // not change**: `rc == 1` means the caller holds the only reference, and a
    // thread that holds no reference cannot make a second one, so the count
    // cannot move under a caller who reads `1`. That is why G2's fork does not
    // reach this function — there is one test, and it is right on both sides.
    unsafe { (rc_atomic(h).load(Ordering::Relaxed) == 1).then(|| cap_of(h)) }
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
    static STACKS: core::cell::RefCell<Blocks> = const { core::cell::RefCell::new(Blocks(Vec::new())) };
}

/// A carrier's idle blocks, unmapped when the thread ends.
struct Blocks(Vec<*mut u8>);

impl Drop for Blocks {
    fn drop(&mut self) {
        for p in self.0.drain(..) {
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
    p.cast()
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
    let idle = STACKS.with(|s| s.borrow_mut().0.pop());
    match idle {
        Some(p) => p,
        None => map_stack(),
    }
}

/// Gives a block from [`buri_rt_stack_acquire`] back to this carrier's free
/// list.
///
/// It is **not** unmapped: a carrier is reused (`cli/runtime/rt.rs`'s pool
/// never retires one), so returning the address space would mean two system
/// calls per entry for no gain. The mapping goes away when the thread does.
///
/// A null pointer is ignored rather than aborting, for the reason
/// [`buri_rt_decref`]'s null check gives: a released nothing is a caller that
/// never acquired, which is a no-op and not a corruption.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_stack_release(base: *mut u8) {
    if base.is_null() {
        return;
    }
    STACKS.with(|s| s.borrow_mut().0.push(base));
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
        buri_rt_stack_release(a);
        // The mapping is kept, so the next acquire on this thread is the same
        // block and no system call.
        let b = buri_rt_stack_acquire();
        assert_eq!(a, b, "a released block was not reused");
        buri_rt_stack_release(b);
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
        buri_rt_stack_release(inner);
        buri_rt_stack_release(outer);
    }

    /// **Two carriers get two stacks**, which is the whole reason this exists.
    #[test]
    fn two_carriers_do_not_share_a_stack() {
        let mine = buri_rt_stack_acquire();
        let theirs = std::thread::spawn(|| {
            let p = buri_rt_stack_acquire();
            buri_rt_stack_release(p);
            p as usize
        })
        .join()
        .expect("the second carrier panicked");
        assert_ne!(mine as usize, theirs, "two carriers were handed one stack");
        buri_rt_stack_release(mine);
    }

    /// **Releasing nothing is nothing**, for `buri_rt_decref`'s reason.
    #[test]
    fn releasing_a_null_stack_is_a_no_op() {
        buri_rt_stack_release(core::ptr::null_mut());
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
        for payload in [0u64, 1, BURI_RT_GROWTH_FLOOR] {
            let p = buri_rt_alloc(payload);
            // SAFETY: `p` is a fresh live payload pointer from this crate.
            unsafe {
                assert_eq!(buri_rt_cap(p), payload);
                assert_eq!(buri_rt_unique_cap(p), Some(payload));
                // Force the reserved bit on, as G3 one day will, and read again.
                (*header(p)).cap |= BURI_RT_CAP_SHARED;
                assert_eq!(buri_rt_cap(p), payload, "buri_rt_cap read the reserved bit");
                assert_eq!(
                    buri_rt_unique_cap(p),
                    Some(payload),
                    "the uniqueness test read the reserved bit",
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

    /// The uniqueness test is not forked, and answers the same on both sides.
    #[test]
    fn a_shared_block_is_still_unique_at_a_count_of_one() {
        let p = buri_rt_alloc(64);
        // SAFETY: `p` is this test's only reference to a live block.
        unsafe {
            (*header(p)).cap |= BURI_RT_CAP_SHARED;
            assert_eq!(buri_rt_unique_cap(p), Some(64), "a shared unique block failed the test");
            buri_rt_incref(p);
            assert_eq!(buri_rt_unique_cap(p), None, "a shared block with two references passed");
            buri_rt_decref(p, None);
            assert_eq!(buri_rt_unique_cap(p), Some(64));
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
}
