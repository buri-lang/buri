//! `core/map`'s three splices of a node's children: insert, replace and remove
//! one element of a `[T]`, in place where nothing else holds the list.
//!
//! Unlike every entry in `list.rs`, these **own** their receiver: `middle::rc`
//! hands each one a count of the list (`rc.rs`'s `TAKEN_NATIVELY`), and a
//! caller that still needs the list after the call takes a second count first.
//! So a count of one here means the caller gave up its only reference, and a
//! write into the block is invisible to anyone. A borrowed receiver couldn't
//! promise that: an append writes past every alias's end, and a splice writes
//! inside it.
//!
//! The result takes the receiver's count when the block is reused. When it
//! isn't, the splice copies and gives the count back ([`give_back`]).
//!
//! The rows carry the element's **release** glue as well as its retain, for
//! the element a splice writes over or removes, and the equality glue every
//! row of that shape carries, which nothing here reads.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here is element offsets, an index times a stride, inside a list block \
              allocated for `len * stride` bytes"
)]

use std::cell::Cell;

use crate::list::{block, copy_retaining, spare, Release, Retain};
use crate::memory::{
    buri_rt_alloc, buri_rt_cap, buri_rt_decref, buri_rt_grown_capacity, buri_rt_unique_cap,
};
use crate::value::BuriList;

thread_local! {
    /// The stride and release glue [`release_elements`] walks a dying block
    /// with, because a drop glue takes the block and nothing else.
    static RELEASING: Cell<(usize, Release)> = const { Cell::new((1, None)) };
}

/// The drop glue of a `[T]` block, from the stride and release in
/// [`RELEASING`]: every element of the block's capacity that isn't all zero,
/// which is the walk the backends' own list glue makes.
extern "C" fn release_elements(p: *mut u8) {
    let (stride, release) = RELEASING.get();
    let Some(release) = release else { return };
    // SAFETY: `buri_rt_decref` hands its glue a live payload pointer.
    let cap = unsafe { buri_rt_cap(p) } as usize;
    for i in 0..cap / stride.max(1) {
        // SAFETY: `i * stride` is inside the block's capacity.
        unsafe {
            let at = p.add(i.saturating_mul(stride));
            if !spare(at, stride) {
                release(at);
            }
        }
    }
}

/// Gives back the receiver's count, releasing its elements if it was the last.
///
/// # Safety
/// `ptr` is null or a live payload pointer of a `[T]` block this call owns a
/// count of; `release` is `T`'s release glue.
unsafe fn give_back(ptr: *const u8, stride: usize, release: Release) {
    if ptr.is_null() {
        return;
    }
    RELEASING.set((stride, release));
    let glue: Option<extern "C" fn(*mut u8)> =
        if release.is_some() { Some(release_elements) } else { None };
    // SAFETY: the caller owns a count of the live block.
    unsafe { buri_rt_decref(ptr.cast_mut(), glue) };
}

/// Releases the element at `at`, unless it holds nothing.
///
/// # Safety
/// `at` covers one element of `stride` bytes that the block owns.
unsafe fn release_one(at: *mut u8, stride: usize, release: Release) {
    if let Some(release) = release {
        // SAFETY: the caller promises the element.
        unsafe {
            if !spare(at, stride) {
                release(at);
            }
        }
    }
}

/// `core/map`'s `insertAt(ctx, xs, at, x) -> [T]`: `x` before index `at`, which
/// is clamped to the list.
///
/// # Safety
/// `ptr` covers `len * stride` bytes and the call owns a count of it; `item`
/// covers `stride` readable bytes; `out` is writable and aligned for a
/// [`BuriList`].
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments, reason = "the flattened row: list, index, item, element glue, out")]
pub unsafe extern "C" fn buri_rt_map_insert_at(
    ptr: *const u8,
    len: u64,
    at: i64,
    item: *const u8,
    stride: u64,
    retain: Retain,
    release: Release,
    _equal: *const u8,
    out: *mut BuriList,
) {
    let (n, s) = (len as usize, stride as usize);
    let at = (at.max(0) as usize).min(n);
    let total = n.saturating_add(1);
    if s == 0 {
        // SAFETY: the caller promises `out`.
        unsafe { out.write(BuriList { ptr: std::ptr::null_mut(), len: total as u64 }) };
        return;
    }
    let needed = total.saturating_mul(s) as u64;
    // SAFETY: the caller promises a live block or null.
    let unique = unsafe { buri_rt_unique_cap(ptr) };
    if let Some(cap) = unique {
        // SAFETY: the block is the caller's alone, so nothing else reads the
        // slots this moves; slot `n` is inside the capacity and spare.
        let fits = cap >= needed
            && (retain.is_none() || unsafe { spare(ptr.add(n.saturating_mul(s)), s) });
        if fits {
            let p = ptr.cast_mut();
            // SAFETY: `[at, n)` moves up by one inside `[0, n + 1)`, which the
            // capacity covers, and the item is a stack slot outside the block.
            unsafe {
                std::ptr::copy(p.add(at * s), p.add((at + 1) * s), (n - at) * s);
                copy_retaining(p.add(at * s), item, 1, s, retain);
                out.write(BuriList { ptr: p, len: total as u64 });
            }
            return;
        }
    }
    // A copy: grown where the list was the caller's alone, so the next insert
    // fits, and exact where it was shared.
    let fresh = match unique {
        Some(cap) => {
            let p = buri_rt_alloc(buri_rt_grown_capacity(needed, cap));
            BuriList { ptr: p, len: total as u64 }
        }
        None => block(total, s),
    };
    // SAFETY: the fresh block covers `total` elements and is disjoint from the
    // source and the item; the headroom past them is zeroed to the capacity
    // its glue walks.
    unsafe {
        let f = fresh.ptr;
        copy_retaining(f, ptr, at, s, retain);
        copy_retaining(f.add(at * s), item, 1, s, retain);
        copy_retaining(f.add((at + 1) * s), ptr.add(at * s), n - at, s, retain);
        if unique.is_some() && retain.is_some() {
            let used = total * s;
            let headroom = (buri_rt_cap(f) as usize).saturating_sub(used);
            std::ptr::write_bytes(f.add(used), 0, headroom);
        }
        give_back(ptr, s, release);
        out.write(fresh);
    }
}

/// `core/map`'s `replaceAt(ctx, xs, at, x) -> [T]`: the element at `at`
/// exchanged for `x`. An index outside the list changes nothing.
///
/// # Safety
/// As [`buri_rt_map_insert_at`].
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments, reason = "the flattened row: list, index, item, element glue, out")]
pub unsafe extern "C" fn buri_rt_map_replace_at(
    ptr: *const u8,
    len: u64,
    at: i64,
    item: *const u8,
    stride: u64,
    retain: Retain,
    release: Release,
    _equal: *const u8,
    out: *mut BuriList,
) {
    let (n, s) = (len as usize, stride as usize);
    if at < 0 || at as usize >= n || s == 0 {
        // SAFETY: the caller promises `out`; the result takes the count.
        unsafe { out.write(BuriList { ptr: ptr.cast_mut(), len }) };
        return;
    }
    let at = at as usize;
    // SAFETY: the caller promises a live block.
    if unsafe { buri_rt_unique_cap(ptr) }.is_some() {
        let slot = unsafe { ptr.cast_mut().add(at * s) };
        // SAFETY: the item takes its count before the element it replaces lets
        // go of its own, in case one reaches the other; then the bytes move in.
        unsafe {
            if let Some(retain) = retain {
                retain(item.cast_mut());
            }
            release_one(slot, s, release);
            std::ptr::copy_nonoverlapping(item, slot, s);
            out.write(BuriList { ptr: ptr.cast_mut(), len });
        }
        return;
    }
    let fresh = block(n, s);
    // SAFETY: the fresh block covers `n` elements, disjoint from the source.
    unsafe {
        let f = fresh.ptr;
        copy_retaining(f, ptr, at, s, retain);
        copy_retaining(f.add(at * s), item, 1, s, retain);
        copy_retaining(f.add((at + 1) * s), ptr.add((at + 1) * s), n - at - 1, s, retain);
        give_back(ptr, s, release);
        out.write(fresh);
    }
}

/// `core/map`'s `removeAt(ctx, xs, at) -> [T]`: everything but the element at
/// `at`. An index outside the list removes nothing.
///
/// # Safety
/// `ptr` covers `len * stride` bytes and the call owns a count of it; `out` is
/// writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_map_remove_at(
    ptr: *const u8,
    len: u64,
    at: i64,
    stride: u64,
    retain: Retain,
    release: Release,
    _equal: *const u8,
    out: *mut BuriList,
) {
    let (n, s) = (len as usize, stride as usize);
    if at < 0 || at as usize >= n {
        // SAFETY: the caller promises `out`; the result takes the count.
        unsafe { out.write(BuriList { ptr: ptr.cast_mut(), len }) };
        return;
    }
    let at = at as usize;
    let total = n - 1;
    if s == 0 {
        // SAFETY: the caller promises `out`.
        unsafe { out.write(BuriList { ptr: std::ptr::null_mut(), len: total as u64 }) };
        return;
    }
    // SAFETY: the caller promises a live block.
    if unsafe { buri_rt_unique_cap(ptr) }.is_some() {
        let p = ptr.cast_mut();
        // SAFETY: the block is the caller's alone. The element at `at` goes,
        // `(at, n)` moves down by one, and the slot that frees up is zeroed,
        // because the block's glue walks its whole capacity and would release
        // the copy left there a second time.
        unsafe {
            release_one(p.add(at * s), s, release);
            std::ptr::copy(p.add((at + 1) * s), p.add(at * s), (total - at) * s);
            std::ptr::write_bytes(p.add(total * s), 0, s);
            if total == 0 {
                give_back(p, s, release);
                out.write(BuriList { ptr: std::ptr::null_mut(), len: 0 });
            } else {
                out.write(BuriList { ptr: p, len: total as u64 });
            }
        }
        return;
    }
    let fresh = block(total, s);
    // SAFETY: the fresh block covers `total` elements, disjoint from the
    // source; an empty one is null and takes nothing.
    unsafe {
        if !fresh.ptr.is_null() {
            let f = fresh.ptr;
            copy_retaining(f, ptr, at, s, retain);
            copy_retaining(f.add(at * s), ptr.add((at + 1) * s), total - at, s, retain);
        }
        give_back(ptr, s, release);
        out.write(fresh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{buri_rt_free, buri_rt_incref, buri_rt_rc};

    /// A block of `xs` as `i64`s, with room for `cap` of them.
    fn ints(xs: &[i64], cap: usize) -> BuriList {
        let p = buri_rt_alloc((cap * 8) as u64);
        for (i, x) in xs.iter().enumerate() {
            // SAFETY: `i < cap`.
            unsafe { p.add(i * 8).cast::<i64>().write(*x) };
        }
        BuriList { ptr: p, len: xs.len() as u64 }
    }

    fn read(l: &BuriList) -> Vec<i64> {
        // SAFETY: `len` `i64`s live there.
        (0..l.len as usize).map(|i| unsafe { l.ptr.add(i * 8).cast::<i64>().read() }).collect()
    }

    fn empty() -> BuriList {
        BuriList { ptr: std::ptr::null_mut(), len: 0 }
    }

    /// Takes `memory::latch`, because an in-place write is what a marked block
    /// does not get, and a case beside this one may mark every block.
    #[test]
    fn a_list_held_once_is_spliced_in_place() {
        let _latch = crate::memory::latch();
        let xs = ints(&[1, 2, 3], 4);
        let (mut a, mut b, mut c) = (empty(), empty(), empty());
        let nine: i64 = 9;
        // SAFETY: each call owns the one count, and hands it to its result.
        unsafe {
            buri_rt_map_insert_at(xs.ptr, xs.len, 1, (&raw const nine).cast(), 8, None, None, std::ptr::null(), &raw mut a);
            assert_eq!(a.ptr, xs.ptr);
            assert_eq!(read(&a), [1, 9, 2, 3]);
            buri_rt_map_replace_at(a.ptr, a.len, 3, (&raw const nine).cast(), 8, None, None, std::ptr::null(), &raw mut b);
            assert_eq!(b.ptr, xs.ptr);
            assert_eq!(read(&b), [1, 9, 2, 9]);
            buri_rt_map_remove_at(b.ptr, b.len, 0, 8, None, None, std::ptr::null(), &raw mut c);
            assert_eq!(c.ptr, xs.ptr);
            assert_eq!(read(&c), [9, 2, 9]);
            // The slot the removal freed is spare again.
            assert_eq!(c.ptr.add(24).cast::<i64>().read(), 0);
            assert_eq!(buri_rt_rc(c.ptr), 1);
            buri_rt_free(c.ptr);
        }
    }

    #[test]
    fn a_list_held_twice_is_copied_and_left_as_it_was() {
        let xs = ints(&[1, 2, 3], 4);
        let mut out = empty();
        let nine: i64 = 9;
        // SAFETY: a second count stands for a second holder; the splice owns
        // one of the two and gives it back.
        unsafe {
            buri_rt_incref(xs.ptr);
            buri_rt_map_replace_at(xs.ptr, xs.len, 1, (&raw const nine).cast(), 8, None, None, std::ptr::null(), &raw mut out);
            assert_ne!(out.ptr, xs.ptr);
            assert_eq!(read(&out), [1, 9, 3]);
            assert_eq!(read(&xs), [1, 2, 3]);
            assert_eq!(buri_rt_rc(xs.ptr), 1);
            buri_rt_free(out.ptr);
            buri_rt_free(xs.ptr);
        }
    }

    #[test]
    fn an_insert_past_the_capacity_grows_and_frees_the_old_block() {
        let _latch = crate::memory::latch();
        let xs = ints(&[1, 2], 2);
        let mut out = empty();
        let nine: i64 = 9;
        // SAFETY: the call owns the one count.
        unsafe {
            buri_rt_map_insert_at(xs.ptr, xs.len, 5, (&raw const nine).cast(), 8, None, None, std::ptr::null(), &raw mut out);
            assert_eq!(read(&out), [1, 2, 9]);
            assert!(buri_rt_cap(out.ptr) >= 24);
            buri_rt_free(out.ptr);
        }
    }

    /// A **marked** list held once is copied rather than written into: a
    /// marked block's count of one may be borrowed by several threads
    /// (`buri_rt_unique_cap`), so it is no licence for a write inside it.
    #[test]
    fn a_marked_list_held_once_is_copied_and_given_back() {
        let _latch = crate::memory::latch();
        crate::memory::share_now();
        let xs = ints(&[1, 2, 3], 4);
        let mut out = empty();
        let nine: i64 = 9;
        // SAFETY: the call owns the one count, and gives it back on the copy.
        unsafe {
            buri_rt_map_replace_at(xs.ptr, xs.len, 1, (&raw const nine).cast(), 8, None, None, std::ptr::null(), &raw mut out);
            assert_ne!(out.ptr, xs.ptr);
            assert_eq!(read(&out), [1, 9, 3]);
            crate::memory::buri_rt_decref(out.ptr, None);
        }
        crate::memory::forget_values_may_cross_tasks();
    }

    #[test]
    fn removing_the_last_element_frees_the_block() {
        let xs = ints(&[7], 1);
        let mut out = ints(&[], 1);
        let spare_block = out.ptr;
        // SAFETY: the call owns the one count.
        unsafe {
            buri_rt_map_remove_at(xs.ptr, xs.len, 0, 8, None, None, std::ptr::null(), &raw mut out);
            assert!(out.ptr.is_null());
            assert_eq!(out.len, 0);
            buri_rt_free(spare_block);
        }
    }
}
