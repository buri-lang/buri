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
//! the release glue (`cranelift/emit.rs`'s `Helper::ReleaseElems`), so this is a
//! symmetric addition rather than a new mechanism.
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

use crate::memory::buri_rt_alloc;
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
fn block(count: usize, stride: usize) -> BuriList {
    if count == 0 || stride == 0 {
        return BuriList { ptr: std::ptr::null_mut(), len: count as u64 };
    }
    let ptr = buri_rt_alloc((count.saturating_mul(stride)) as u64);
    BuriList { ptr, len: count as u64 }
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
    let result = block(a.saturating_add(b), s);
    // SAFETY: the destination is a fresh block of `a + b` elements, disjoint
    // from both sources, and each source covers its own count.
    unsafe {
        copy_retaining(result.ptr, ptr, a, s, retain);
        if !result.ptr.is_null() {
            copy_retaining(result.ptr.add(a.saturating_mul(s)), optr, b, s, retain);
        }
        out.write(result);
    }
}

/// `list.push(self, ctx, item) -> [T]` — a fresh block, because `[T]` is
/// immutable and `push` is `Alloc`-bounded rather than in-place.
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
    let result = block(n.saturating_add(1), s);
    // SAFETY: a fresh block of `n + 1` elements, disjoint from both sources.
    unsafe {
        copy_retaining(result.ptr, ptr, n, s, retain);
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
/// append the out-pointer" (`cranelift/runtime.rs`) and synthesising a missing
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

#[cfg(test)]
mod tests {
    use super::*;

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
