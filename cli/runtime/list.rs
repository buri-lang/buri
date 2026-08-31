//! `core/list`, natively — the entries that are a block copy.
//!
//! # Why the element type arrives as two extra parameters
//!
//! `[T]` is one block of `n` elements at their own stride (VALUE-MODEL.md §4),
//! and the runtime does not know `T`. Two things follow, and both are passed in
//! rather than looked up:
//!
//! * **`stride`** — `middle::layout` computes it, so it is a compile-time
//!   constant at every call site and costs an immediate operand.
//! * **`retain`** — the per-element function that increfs whatever counted
//!   pointers one element holds, or **null** when the element type holds none,
//!   which is the common case (`[Int]`, `[U8]`, a struct of scalars). It exists
//!   because `lib.rs` §3 says a result is owned: a `[Str]` built by copying
//!   another one's bytes has `n` new references to `n` blocks, and something has
//!   to take them. `middle::rc` assumes exactly this — "a runtime intrinsic
//!   borrows its arguments and returns a fresh count" (`rc.rs:98`) — so a
//!   runtime that copied without retaining would produce a use-after-free the
//!   moment the source list was dropped.
//!
//! Both backends generate the retain glue the same way they already generate
//! the release glue (`stencil/glue.rs`'s `Helper::Elems`,
//! `llvm/emit.rs::release_elems_glue`), so this is a symmetric addition rather
//! than a new mechanism.
//!
//! The pair is appended **after** the flattened Buri arguments and before the
//! out-pointer, uniformly across every entry in this file, so the emitter is one
//! rule — spread the arguments, append the element pair, append the
//! out-pointer — rather than a parameter order remembered per symbol.
//!
//! # What is not here, and why
//!
//! * **Everything taking a closure** — `map`, `filter`, `fold`, `any`, `all`,
//!   `find`, `findIndex`, `count`, `sortBy` and the `Ctx` variants. A Buri
//!   closure is `{ code, env }` where `code` is a thunk whose signature is the
//!   *flattened* one of its own element type (VALUE-MODEL.md §5.1), so calling
//!   one from C would mean synthesizing a call whose parameter list depends on
//!   `T`. That is a backend's job by construction, and both backends emit the
//!   loop directly.
//! * **`zip` and `flatten`.** `zip` builds a `[(A, B)]`, whose element layout —
//!   the offset of the second field after alignment — is `middle::layout`'s
//!   answer and not a function of the two strides. `flatten` reads a `[[T]]`,
//!   whose elements are `BuriList` descriptors pointing at blocks of a stride
//!   this call does not carry. Both are one more descriptor parameter away and
//!   neither is needed by the conformance corpus that the native set runs, so
//!   they are named here rather than half-built.

use crate::memory::{buri_rt_alloc, buri_rt_grown_capacity, buri_rt_incref, buri_rt_unique_cap};
use crate::value::BuriList;
use crate::BURI_OK;

/// The per-element retain: increfs the counted pointers inside one element, in
/// place. Null where the element type holds none.
pub type Retain = Option<unsafe extern "C" fn(*mut u8)>;

/// The discriminant `list.get` answers for an index outside the list.
/// `.None` is `Option`'s only non-success arm; see `text.rs`'s `BURI_ABSENT`.
const BURI_ABSENT: i32 = 0;

/// Copy `count` elements from `src` to `dst` and take a reference on each.
///
/// # Safety
/// `src` and `dst` must each cover `count * stride` bytes and must not overlap;
/// `retain`, where non-null, must be the retain glue for the element type.
unsafe fn copy_retaining(
    dst: *mut u8,
    src: *const u8,
    count: usize,
    stride: usize,
    retain: Retain,
) {
    if count == 0 || stride == 0 {
        return;
    }
    // SAFETY: the caller promises both ranges and that they are disjoint.
    unsafe { std::ptr::copy_nonoverlapping(src, dst, count.saturating_mul(stride)) };
    let Some(retain) = retain else { return };
    for i in 0..count {
        // SAFETY: `i * stride` is inside the destination just written.
        unsafe { retain(dst.add(i.saturating_mul(stride))) }
    }
}

/// A fresh block of `count` elements, or the null descriptor when there are
/// none. An empty `[T]` allocates nothing, which is what makes `list.empty`
/// free.
pub(crate) fn block(count: usize, stride: usize) -> BuriList {
    if count == 0 || stride == 0 {
        return BuriList { ptr: std::ptr::null_mut(), len: count as u64 };
    }
    let ptr = buri_rt_alloc((count.saturating_mul(stride)) as u64);
    BuriList { ptr, len: count as u64 }
}

/// Where the `add` new elements of an append go, with the prefix already
/// there — MEMORY.md §5.3's in-place growth, and the two fallbacks under it.
///
/// Three outcomes, in the order they are tried:
///
///  1. **In place.** The block is uniquely owned ([`buri_rt_unique_cap`]) and
///     has the headroom. Nothing is copied and nothing is allocated: the
///     elements are written past the end and the block takes one more
///     reference, which the *result* holds. The caller's own reference is
///     untouched, so the `decref` the compiler already emits for it — this
///     runtime borrows its arguments (`rc.rs`'s header) — leaves the result
///     uniquely owned and the next append in a loop takes this path again.
///  2. **Grown.** Uniquely owned but out of capacity: a fresh block with
///     [`buri_rt_grown_capacity`] bytes, so the *next* append takes path 1 and
///     a loop of `n` appends allocates O(log n) times rather than O(n).
///  3. **Exact.** Shared, immortal, or empty: exactly what the result needs,
///     which is what this entry did unconditionally before.
///
/// # Why paths 1 and 2 are unobservable, and why `retain` gates them
///
/// The uniqueness half is [`buri_rt_unique_cap`]'s doc comment: `rc == 1` means
/// one observable value, every alias of it carries the same `len`, and a write
/// at `len` or beyond is therefore invisible to all of them. A `[T]` is never a
/// view (`value.rs`), so there is no second descriptor into the same block at a
/// different offset to worry about.
///
/// The `retain.is_none()` half is a *correctness* condition and not caution.
/// A null `retain` is the backend saying the element type holds no counted
/// references (`llvm/emit.rs::retain_glue`), and that is exactly what
/// makes both paths safe:
///
///  * **Path 1** writes over whatever those slots held. For a scalar element
///    that is nothing; for a counted one it would be a reference the block
///    still owned, dropped without a `decref` — a leak.
///  * **Path 2** leaves `cap / stride` above the element count, and the
///    generated release glue for a `[T]` block walks **`cap / stride`
///    elements** (`stencil/glue.rs`'s `Helper::Elems`). For a scalar
///    element the extra slots are bytes nobody reads; for a counted one they
///    are uninitialized memory the drop glue would decref.
///
/// Lifting the restriction means giving this ABI a per-element *release* glue
/// beside `retain`, and making the drop walk follow the element count rather
/// than the capacity. Both are backend changes; MEMORY.md §5.3 records them as
/// the growth path.
///
/// # Safety
/// `ptr` covers `n * stride` bytes and is null or a live payload pointer;
/// `retain`, where non-null, is the retain glue for the element type.
unsafe fn append_dest(
    ptr: *const u8,
    n: usize,
    add: usize,
    stride: usize,
    retain: Retain,
) -> BuriList {
    let total = n.saturating_add(add);
    if stride == 0 || total == 0 {
        return BuriList { ptr: std::ptr::null_mut(), len: total as u64 };
    }
    let needed = total.saturating_mul(stride) as u64;
    if retain.is_none() && !ptr.is_null() {
        // SAFETY: the caller promises a live payload pointer.
        if let Some(cap) = unsafe { buri_rt_unique_cap(ptr) } {
            if cap >= needed {
                // SAFETY: as above. The result is a second reference to the
                // block, so it takes a count of its own.
                unsafe { buri_rt_incref(ptr.cast_mut()) };
                return BuriList { ptr: ptr.cast_mut(), len: total as u64 };
            }
            let fresh = buri_rt_alloc(buri_rt_grown_capacity(needed, cap));
            // SAFETY: a fresh block of at least `needed` bytes, disjoint from
            // the source, which covers its own `n` elements.
            unsafe { copy_retaining(fresh, ptr, n, stride, retain) };
            return BuriList { ptr: fresh, len: total as u64 };
        }
    }
    let out = block(total, stride);
    // SAFETY: a fresh block of `total` elements, disjoint from the source.
    unsafe { copy_retaining(out.ptr, ptr, n, stride, retain) };
    out
}

/// `list.get(self, index) -> Option<T>` — `stride` bytes into `out`.
///
/// # Safety
/// `ptr` covers `len * stride` bytes; `out` covers `stride` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_get(
    ptr: *const u8,
    len: u64,
    index: i64,
    stride: u64,
    retain: Retain,
    out: *mut u8,
) -> i32 {
    if index < 0 || (index as u64) >= len || ptr.is_null() {
        return BURI_ABSENT;
    }
    let at = (index as u64).saturating_mul(stride) as usize;
    // SAFETY: `at` is inside the block, and `out` covers one element.
    unsafe { copy_retaining(out, ptr.add(at), 1, stride as usize, retain) };
    BURI_OK
}

/// `list.concat(self, ctx, other) -> [T]`.
///
/// # Safety
/// Both ranges cover their `len * stride` bytes; `out` is writable and aligned
/// for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_concat(
    ptr: *const u8,
    len: u64,
    optr: *const u8,
    olen: u64,
    stride: u64,
    retain: Retain,
    out: *mut BuriList,
) {
    let (a, b, s) = (len as usize, olen as usize, stride as usize);
    // SAFETY: the caller promises the receiver's range; `append_dest` leaves
    // the first `a` elements in place, however it got them there.
    let result = unsafe { append_dest(ptr, a, b, s, retain) };
    // SAFETY: the destination has room for `a + b` elements and the second
    // source covers its own count.
    //
    // The two ranges are disjoint, including on the in-place path. The only
    // way `optr` lands inside the receiver's block is `xs.concat(ctx, xs)` with
    // both arguments borrowing one value, and then `a == b`, so the
    // destination `[a, a + b)` begins exactly where the source `[0, b)` ends.
    // Two descriptors of *different* lengths over one block do exist — that is
    // what an in-place append produces — but each of them holds a count, so the
    // in-place path's `rc == 1` excludes the case.
    unsafe {
        if !result.ptr.is_null() {
            copy_retaining(result.ptr.add(a.saturating_mul(s)), optr, b, s, retain);
        }
        out.write(result);
    }
}

/// `list.push(self, ctx, item) -> [T]`.
///
/// `[T]` is immutable *in the language*; the implementation writes past the end
/// of a block nothing else can see, which is [`append_dest`]'s three paths and
/// MEMORY.md §5.3. The result is a new value either way.
///
/// The item arrives through a pointer for the same reason a `Str` result does:
/// `lib.rs` §2 rule 1 flattens an aggregate parameter into leaves, and a
/// generic `T` has no leaf list this signature could name. The caller spills it
/// to a stack slot and passes the address, which is one store it would have
/// made anyway.
///
/// # Safety
/// `ptr` covers `len * stride` bytes; `item` covers `stride` readable bytes;
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_push(
    ptr: *const u8,
    len: u64,
    item: *const u8,
    stride: u64,
    retain: Retain,
    out: *mut BuriList,
) {
    let (n, s) = (len as usize, stride as usize);
    // SAFETY: the caller promises the receiver's range. Where the receiver is
    // uniquely owned this writes nothing and allocates nothing — MEMORY.md
    // §5.3, and the reason an accumulate-in-a-loop `push` is amortized O(1).
    let result = unsafe { append_dest(ptr, n, 1, s, retain) };
    // SAFETY: the destination has room for `n + 1` elements, and `item` covers
    // one and is a stack slot the caller spilled, so it cannot be inside it.
    unsafe {
        if !result.ptr.is_null() {
            copy_retaining(result.ptr.add(n.saturating_mul(s)), item, 1, s, retain);
        }
        out.write(result);
    }
}

/// `list.reverse(self, ctx) -> [T]`.
///
/// # Safety
/// `ptr` covers `len * stride` bytes; `out` is writable and aligned for a
/// [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_reverse(
    ptr: *const u8,
    len: u64,
    stride: u64,
    retain: Retain,
    out: *mut BuriList,
) {
    let (n, s) = (len as usize, stride as usize);
    let result = block(n, s);
    for i in 0..n {
        let from = n.saturating_sub(1).saturating_sub(i);
        // SAFETY: both offsets are inside their own `n`-element blocks.
        unsafe {
            copy_retaining(
                result.ptr.add(i.saturating_mul(s)),
                ptr.add(from.saturating_mul(s)),
                1,
                s,
                retain,
            );
        }
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(result) }
}

/// `list.slice(self, ctx, start, end) -> [T]`, clamped at both ends.
///
/// A list slice **copies**, unlike a string slice: `slice` is `Alloc`-bounded
/// in `list.buri:120` and pure in `str.buri:26`, which is the language saying
/// which of the two is a view.
///
/// # Safety
/// As [`buri_rt_list_reverse`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_slice(
    ptr: *const u8,
    len: u64,
    start: i64,
    end: i64,
    stride: u64,
    retain: Retain,
    out: *mut BuriList,
) {
    let n = len as usize;
    let s = stride as usize;
    let from = start.max(0).min(len as i64) as usize;
    let to = end.max(0).min(len as i64).max(from as i64) as usize;
    let count = to.saturating_sub(from).min(n.saturating_sub(from));
    let result = block(count, s);
    // SAFETY: `from + count <= n`, so the source range is inside the block.
    unsafe {
        if !ptr.is_null() {
            copy_retaining(result.ptr, ptr.add(from.saturating_mul(s)), count, s, retain);
        }
        out.write(result);
    }
}

/// `list.take(self, ctx, n) -> [T]` — the first `n`, clamped.
///
/// A separate entry rather than a `slice(0, n)` at the call site, because the
/// emitter's one rule is "spread the Buri arguments, append the element pair,
/// append the out-pointer" (`stencil/runtime.rs`) and synthesising a missing
/// argument would be a second rule for two entries.
///
/// # Safety
/// As [`buri_rt_list_reverse`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_take(
    ptr: *const u8,
    len: u64,
    n: i64,
    stride: u64,
    retain: Retain,
    out: *mut BuriList,
) {
    // SAFETY: forwarded.
    unsafe { buri_rt_list_slice(ptr, len, 0, n, stride, retain, out) }
}

/// `list.drop(self, ctx, n) -> [T]` — everything after the first `n`.
///
/// # Safety
/// As [`buri_rt_list_reverse`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_drop(
    ptr: *const u8,
    len: u64,
    n: i64,
    stride: u64,
    retain: Retain,
    out: *mut BuriList,
) {
    // SAFETY: forwarded. `len` is an element count and fits an `i64` for any
    // list that exists, so the cast cannot change which elements are named.
    unsafe { buri_rt_list_slice(ptr, len, n, len as i64, stride, retain, out) }
}

/// `list.repeat(ctx, item, times) -> [T]`. A non-positive count is empty.
///
/// # Safety
/// `item` covers `stride` readable bytes; `out` is writable and aligned for a
/// [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_repeat(
    item: *const u8,
    times: i64,
    stride: u64,
    retain: Retain,
    out: *mut BuriList,
) {
    let n = times.max(0) as usize;
    let s = stride as usize;
    let result = block(n, s);
    for i in 0..n {
        // SAFETY: `i * s` is inside the `n`-element block just allocated.
        unsafe { copy_retaining(result.ptr.add(i.saturating_mul(s)), item, 1, s, retain) };
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(result) }
}

/// `list.range(ctx, start, end) -> [Int]` — half-open, empty when `end <=
/// start`. `Int` is `i64`, so the stride is fixed and no descriptor is needed.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_range(start: i64, end: i64, out: *mut BuriList) {
    let count = end.saturating_sub(start).max(0) as usize;
    let result = block(count, 8);
    for i in 0..count {
        // SAFETY: `i * 8` is inside the block, and the payload is 16-aligned so
        // every `i64` slot is aligned.
        unsafe {
            result
                .ptr
                .add(i.saturating_mul(8))
                .cast::<i64>()
                .write(start.saturating_add(i as i64));
        }
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(result) }
}

// ---------------------------------------------------------------------------
// The closure trampoline
// ---------------------------------------------------------------------------
//
// The header above says why nothing that takes a closure is in this file: "a
// Buri closure is `{ code, env }` where `code` is a thunk whose signature is
// the *flattened* one of its own element type, so calling one from C would mean
// synthesizing a call whose parameter list depends on `T`".
//
// That sentence is still true, and [`StepEntry`] is the way around it rather
// than an exception to it. The runtime does not call the closure; it calls a
// **function the backend generated at the call site**, where `T` is known, and
// hands it three pointers and a number — its own opaque state, which item this
// is, one element in, one element out. Nothing about the element type crosses
// but its stride, which is what every other entry in this file already takes.
//
// The entry in *this file* is a pilot: `list.mapCtxStep` is `list.mapCtx`
// spelled through the trampoline, so that the boundary a task scheduler needs
// was exercised — by a conformance fixture, and against JavaScript's answer —
// before the scheduler existed. The scheduler is `buri_rt_host_tasks_parallel`
// (`rt.rs`), which is the other user of [`StepEntry`] and the reason it is
// shaped this way. `map` itself is **not** routed here: both backends open-code
// that loop, and an indirect call per element is slower than the instructions
// it would replace.

/// The generated C-ABI entry thunk one step is reached through.
///
/// `state` is the backend's own record — a closure's `{ code, env }`, and
/// whatever else that backend needs to enter Buri code — and is never read
/// here. `arg` points at one source element at `in_stride`; `out` at where its
/// answer goes, at `out_stride`. Both are the element's *memory* layout, which
/// is what a stride describes.
///
/// `index` is **which item this is**, and it is a parameter rather than
/// something the thunk could work out. `effect Tasks` promises that the step
/// receives its item's own index (`sources/effect.buri`), and the caller is the
/// only side that knows one: the runtime drives the walk, so only the runtime
/// can say where in it a given call is. Deriving it inside the thunk would mean
/// dividing `arg - base` by the stride, which needs a base the record does not
/// carry and which stops being true the moment a scheduler hands out elements
/// out of order — which is exactly what `Tasks.parallel` becomes.
///
/// It is on **every** step and not only the ones that want it, so that there is
/// one C signature at this boundary rather than one per key: `list.mapCtxStep`'s
/// closure takes no index and its thunk ignores the register.
///
/// The step **owns** what it is handed and answers a fresh count: the thunk
/// takes its own reference on `arg`'s element before entering Buri code, and
/// what it writes through `out` belongs to the block being built. That is
/// `middle/rc.rs`'s "a call through a function value owns its arguments",
/// settled on the side of the boundary that knows the type — which is why there
/// is no `retain` parameter here beside the strides.
pub type StepEntry =
    unsafe extern "C" fn(state: *mut u8, index: u64, arg: *const u8, out: *mut u8);

/// `list.mapCtxStep(self, ctx, f) -> [B]` — `list.mapCtx` with its step reached
/// through [`StepEntry`].
///
/// The answer is `f` at every element in index order, which is `mapCtx`'s
/// answer: this entry is *compared* against it rather than merely run.
///
/// Nothing suspends. The entry thunk is called and returns before the next
/// element is read, exactly as an open-coded loop runs its step, so the only
/// new thing under test is the boundary itself.
///
/// # Safety
/// `ptr` covers `len * in_stride` bytes; `entry` is the thunk the backend
/// generated for this call and `state` the record it was generated against;
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_map_ctx_step(
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
    for i in 0..n {
        // SAFETY: `i * from` is inside the `n`-element source the caller
        // promised, and `i * to` is inside the block just allocated. The thunk
        // is the one the backend generated for these two element types.
        unsafe {
            entry(
                state,
                i as u64,
                ptr.add(i.saturating_mul(from)),
                result.ptr.add(i.saturating_mul(to)),
            );
        }
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(result) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trampoline, driven by a C thunk written here rather than generated:
    /// what an entry does with the index and the three pointers is a backend's
    /// business, and what this file promises is the walk — every element, in
    /// index order, at the two strides it was told.
    #[test]
    fn the_step_entry_sees_every_element_in_order() {
        unsafe extern "C" fn double_it(state: *mut u8, index: u64, arg: *const u8, out: *mut u8) {
            // SAFETY: the test hands a live counter, an `i64` source element
            // and an `i32` destination slot.
            unsafe {
                let seen = state.cast::<i64>();
                assert_eq!(seen.read(), index as i64, "the index counts the calls");
                seen.write(seen.read() + 1);
                out.cast::<i32>().write((arg.cast::<i64>().read() * 2) as i32);
            }
        }
        let src: [i64; 4] = [1, 2, 3, 4];
        let mut seen: i64 = 0;
        let mut got = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `src` covers four `i64`s and both out-pointers are live.
        unsafe {
            buri_rt_list_map_ctx_step(
                src.as_ptr().cast(),
                4,
                double_it,
                (&raw mut seen).cast(),
                8,
                4,
                &raw mut got,
            );
        }
        assert_eq!(seen, 4, "the step ran once per element");
        assert_eq!(got.len, 4);
        // SAFETY: four `i32`s were written there, at the stride asked for.
        let answers: Vec<i32> =
            (0..4).map(|i| unsafe { got.ptr.add(i * 4).cast::<i32>().read() }).collect();
        assert_eq!(answers, vec![2, 4, 6, 8]);
        // SAFETY: the only reference.
        unsafe { crate::memory::buri_rt_free(got.ptr) };
    }

    /// An empty list allocates nothing and enters nothing — `block`'s rule,
    /// which is what keeps `[].mapCtxStep(...)` free rather than a null
    /// dereference inside the loop.
    #[test]
    fn an_empty_list_never_enters_the_step() {
        unsafe extern "C" fn never(_: *mut u8, _: u64, _: *const u8, _: *mut u8) {
            unreachable!("the step ran on an empty list");
        }
        let mut got = BuriList { ptr: std::ptr::null_mut(), len: 7 };
        // SAFETY: the source is empty, so the pointer is never read.
        unsafe {
            buri_rt_list_map_ctx_step(
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
        assert!(got.ptr.is_null());
    }

    #[test]
    fn a_range_is_half_open() {
        let mut out = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `out` is a live local.
        unsafe { buri_rt_list_range(2, 5, &raw mut out) };
        assert_eq!(out.len, 3);
        // SAFETY: three `i64`s were just written there.
        let got: Vec<i64> = (0..3)
            .map(|i| unsafe { out.ptr.add(i * 8).cast::<i64>().read() })
            .collect();
        assert_eq!(got, vec![2, 3, 4]);
        // SAFETY: a fresh block with one reference.
        unsafe { crate::memory::buri_rt_free(out.ptr) };

        // SAFETY: `out` is a live local.
        unsafe { buri_rt_list_range(5, 2, &raw mut out) };
        assert_eq!(out.len, 0);
        assert!(out.ptr.is_null(), "an empty list allocates nothing");
    }

    #[test]
    fn a_slice_clamps_rather_than_aborting() {
        let src: [i64; 4] = [10, 20, 30, 40];
        let mut out = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        // SAFETY: `src` covers four `i64`s and `out` is a live local.
        unsafe {
            buri_rt_list_slice(src.as_ptr().cast(), 4, -3, 99, 8, None, &raw mut out);
        }
        assert_eq!(out.len, 4);
        // SAFETY: four `i64`s were just written there.
        let got: Vec<i64> =
            (0..4).map(|i| unsafe { out.ptr.add(i * 8).cast::<i64>().read() }).collect();
        assert_eq!(got, vec![10, 20, 30, 40]);
        // SAFETY: a fresh block with one reference.
        unsafe { crate::memory::buri_rt_free(out.ptr) };
    }

    /// One `push` per iteration over a uniquely-owned list allocates O(log n)
    /// times, not O(n) — MEMORY.md §5.3's amortized O(1).
    ///
    /// The loop is the shape a Buri `loop { .. continue(acc.push(c, i), ..) }`
    /// compiles to: the caller holds one reference, hands it to `push`
    /// borrowed, and drops it after — so the count seen here is `1` on entry
    /// and `2` on exit, and the `decref` standing in for the compiler's brings
    /// it back to `1`.
    #[test]
    fn a_unique_push_grows_in_place() {
        // The in-place licence is `buri_rt_unique_cap`'s, and it is refused
        // for a marked block — so this case and any case that sets the
        // marking latch must not run at once. `memory::latch` names both
        // sides of that rule.
        let _latch = crate::memory::latch();
        let mut acc = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        let mut allocations = 0u32;
        for i in 0i64..1000 {
            let mut out = BuriList { ptr: std::ptr::null_mut(), len: 0 };
            // SAFETY: `acc` is this test's own live list and `i` is a live
            // local of the element's stride.
            unsafe {
                buri_rt_list_push(acc.ptr, acc.len, (&raw const i).cast(), 8, None, &raw mut out);
            }
            if out.ptr != acc.ptr {
                allocations += 1;
            }
            // The compiler's own `decref` of the argument, which is what makes
            // the result unique again.
            // SAFETY: `acc` holds one reference, or is the null descriptor.
            unsafe { crate::memory::buri_rt_decref(acc.ptr, None) };
            acc = out;
        }
        assert_eq!(acc.len, 1000);
        // SAFETY: a thousand `i64`s were written there in order.
        let got: Vec<i64> =
            (0..1000).map(|i| unsafe { acc.ptr.add(i * 8).cast::<i64>().read() }).collect();
        assert_eq!(got, (0..1000).collect::<Vec<i64>>());
        // Doubling from a 64-byte floor over 8000 bytes: eight growths, plus
        // the first allocation. A linear implementation would report 1000.
        assert!(allocations <= 12, "one push per element allocated {allocations} times");
        // SAFETY: the last reference. Whether the *process* leaked is
        // `cli/tests/native/runtime.rs`'s question and not this one: the
        // counters are global and `cargo test` runs these in parallel, so a
        // reading taken here would be a reading of every other test as well.
        unsafe { crate::memory::buri_rt_free(acc.ptr) };
    }

    /// The observable-semantics guard: a `push` on a list something *else*
    /// still holds must not touch what that other holder can see.
    #[test]
    fn a_shared_push_leaves_the_original_alone() {
        let mut base = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        for i in 0i64..8 {
            let mut out = BuriList { ptr: std::ptr::null_mut(), len: 0 };
            // SAFETY: as above.
            unsafe {
                buri_rt_list_push(base.ptr, base.len, (&raw const i).cast(), 8, None, &raw mut out);
                crate::memory::buri_rt_decref(base.ptr, None);
            }
            base = out;
        }
        // A second binding onto the same block: this is what `rc == 1` is
        // testing for, and what the two pushes below must respect.
        // SAFETY: `base` is a live block.
        unsafe { crate::memory::buri_rt_incref(base.ptr) };
        let held = BuriList { ptr: base.ptr, len: base.len };

        let (mut one, mut two) = (
            BuriList { ptr: std::ptr::null_mut(), len: 0 },
            BuriList { ptr: std::ptr::null_mut(), len: 0 },
        );
        let (a, b) = (100i64, 200i64);
        // SAFETY: `base` is live and each item is a live local.
        unsafe {
            buri_rt_list_push(base.ptr, base.len, (&raw const a).cast(), 8, None, &raw mut one);
            buri_rt_list_push(base.ptr, base.len, (&raw const b).cast(), 8, None, &raw mut two);
        }
        assert_ne!(one.ptr, base.ptr, "a shared list was mutated in place");
        assert_ne!(two.ptr, base.ptr, "a shared list was mutated in place");
        assert_ne!(one.ptr, two.ptr, "two pushes answered one block");

        let read = |l: &BuriList| -> Vec<i64> {
            // SAFETY: `l.len` elements of eight bytes were written there.
            (0..l.len as usize).map(|i| unsafe { l.ptr.add(i * 8).cast::<i64>().read() }).collect()
        };
        assert_eq!(read(&held), (0..8).collect::<Vec<i64>>(), "the other holder's value changed");
        assert_eq!(read(&base), (0..8).collect::<Vec<i64>>());
        assert_eq!(read(&one).last().copied(), Some(100));
        assert_eq!(read(&two).last().copied(), Some(200));
        // SAFETY: one reference each, and two on `base`'s block.
        unsafe {
            crate::memory::buri_rt_free(one.ptr);
            crate::memory::buri_rt_free(two.ptr);
            crate::memory::buri_rt_decref(held.ptr, None);
            crate::memory::buri_rt_free(base.ptr);
        }
    }

    /// A counted element type keeps the old behaviour exactly, for the reason
    /// [`append_dest`] gives: the release glue for a `[T]` block walks
    /// `cap / stride`, so a block with headroom would have it walk slots
    /// nothing wrote.
    #[test]
    fn a_counted_element_type_still_allocates_exactly() {
        unsafe extern "C" fn nothing(_: *mut u8) {}
        let retain: Retain = Some(nothing);
        let mut acc = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        for i in 0i64..4 {
            let mut out = BuriList { ptr: std::ptr::null_mut(), len: 0 };
            // SAFETY: `acc` is this test's own live list.
            unsafe {
                buri_rt_list_push(acc.ptr, acc.len, (&raw const i).cast(), 8, retain, &raw mut out);
                assert_ne!(out.ptr, acc.ptr, "a counted element type took the in-place path");
                // SAFETY: `out` is a fresh block whose capacity is its length.
                assert_eq!(crate::memory::buri_rt_cap(out.ptr), out.len * 8);
                crate::memory::buri_rt_free(acc.ptr);
            }
            acc = out;
        }
        // SAFETY: the last reference.
        unsafe { crate::memory::buri_rt_free(acc.ptr) };
    }

    /// `concat` takes the same three paths, and the grown one keeps the
    /// elements in order across the seam.
    #[test]
    fn a_unique_concat_appends_in_place() {
        // The in-place licence is `buri_rt_unique_cap`'s, and it is refused
        // for a marked block — so this case and any case that sets the
        // marking latch must not run at once. `memory::latch` names both
        // sides of that rule.
        let _latch = crate::memory::latch();
        let src: [i64; 3] = [7, 8, 9];
        let mut acc = BuriList { ptr: std::ptr::null_mut(), len: 0 };
        let mut allocations = 0u32;
        for _ in 0..100 {
            let mut out = BuriList { ptr: std::ptr::null_mut(), len: 0 };
            // SAFETY: `src` covers three `i64`s and `acc` is this test's list.
            unsafe {
                buri_rt_list_concat(
                    acc.ptr,
                    acc.len,
                    src.as_ptr().cast(),
                    3,
                    8,
                    None,
                    &raw mut out,
                );
                crate::memory::buri_rt_decref(acc.ptr, None);
            }
            if out.ptr != acc.ptr {
                allocations += 1;
            }
            acc = out;
        }
        assert_eq!(acc.len, 300);
        // SAFETY: three hundred `i64`s were written there, three at a time.
        let got: Vec<i64> =
            (0..300).map(|i| unsafe { acc.ptr.add(i * 8).cast::<i64>().read() }).collect();
        assert_eq!(got, (0..300).map(|i| 7 + (i % 3) as i64).collect::<Vec<i64>>());
        assert!(allocations <= 10, "one concat per step allocated {allocations} times");
        // SAFETY: the last reference.
        unsafe { crate::memory::buri_rt_free(acc.ptr) };
    }

    #[test]
    fn get_is_bounds_checked_at_both_ends() {
        let src: [i64; 2] = [7, 8];
        let mut slot = 0i64;
        let out = (&raw mut slot).cast::<u8>();
        // SAFETY: `src` covers two `i64`s and `slot` is a live local.
        unsafe {
            assert_eq!(buri_rt_list_get(src.as_ptr().cast(), 2, 1, 8, None, out), BURI_OK);
            assert_eq!(slot, 8);
            assert_eq!(buri_rt_list_get(src.as_ptr().cast(), 2, 2, 8, None, out), BURI_ABSENT);
            assert_eq!(buri_rt_list_get(src.as_ptr().cast(), 2, -1, 8, None, out), BURI_ABSENT);
        }
    }
}
