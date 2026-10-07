//! Scalar indices into a long non-ASCII `Str`, without a walk from its start.
//!
//! `core/str` indexes in Unicode scalars and a `Str` is UTF-8 bytes, so
//! `slice`, `charAt` and `indexOf` each turn a scalar index into a byte offset
//! (`text.rs`'s header). VALUE-MODEL.md §3.1's ASCII flag makes that free on an
//! ASCII string. Without this file it was a walk from the start of the view on
//! any other, so a program that sliced one long string once per token, which is
//! what `core/buri/ast`'s lexer does to its source, was quadratic in the source
//! the moment the source held one `é`.
//!
//! # The index
//!
//! A view of [`KEPT_FROM`] bytes or more that is not ASCII is indexed the first
//! time it is asked about: one pass records how many scalars start in each
//! [`STRIDE`]-byte run of it. After that, the scalar count at any byte offset
//! is one lookup and a scan of under [`STRIDE`] bytes, and the offset of scalar
//! `i` is a binary search and the same scan. An index serves its own view and
//! every view inside it, so the lexer's per-token `source.slice(a, b)` and a
//! later slice of one of those slices both read the table the first call built.
//!
//! [`SLOTS`] indices are kept, replaced in turn. A program that alternates
//! between more long strings than that rebuilds one per question, which costs
//! the pass the walk would have cost anyway.
//!
//! # Why a side table, and why it cannot go stale
//!
//! The index is keyed by the block a view lives in (`base`), so it is right for
//! exactly as long as the bytes it summarised do not change and the address is
//! not given to another block. A `Str` is immutable, and the runtime enforces
//! that, but three things can still happen to a block's bytes while a table
//! about them is kept. Each one calls [`forget`] on the block first:
//!
//! * **It is freed.** `memory.rs`'s `buri_rt_free`, before the block goes back
//!   to a cache, the quarantine or the allocator, so its address can be reused
//!   only after its index is gone.
//! * **It grows in place.** `buri_rt_realloc`, which is only ever called on a
//!   unique block.
//! * **A concatenation writes into it.** MEMORY.md §5.3's in-place arm writes
//!   past the end of the one live view into the block's headroom, and that
//!   headroom can hold bytes a longer, now-dead view was indexed over.
//!   `text.rs`'s `buri_rt_str_concat` calls [`forget`] on that arm, and the
//!   LLVM backend's open-coded concatenation calls [`buri_rt_str_written`].
//!
//! A null `base` is a literal or a static, or a `Str` the runtime lent out of
//! its own memory for one call, and is never indexed: the last kind's bytes
//! are gone when the call returns, and there is no free to hear about it. A
//! block from a scoped arena is never indexed either, because its pages go
//! back in one `munmap` with no free per block.
//!
//! # Threads
//!
//! The tables are behind one [`Mutex`], taken only by a question about a long
//! non-ASCII string. The free path does not take it: [`KEYS`] mirrors each
//! slot's `base` in an atomic, and [`forget`] reads those four words and goes
//! on unless one of them is the block being freed.
//!
//! That read is enough because the indexing thread held a reference to the
//! block when it stored the key, and the freeing thread holds the last one. On
//! a block with an atomic count (MEMORY.md §5.1's shared fork) the decrements
//! are `AcqRel` and the last one synchronises with all the others, so the
//! store happens before the free and the free reads that key or a later one.
//! A later one means another thread replaced the slot under the lock, which
//! removed this block's index with it. On a block without an atomic count the
//! program is on one thread. A key read as stale only ever costs a lock.
//!
//! The concatenation and growth hooks run only on a unique, unmarked block,
//! and a marked block is the only kind two threads can reach, so those hooks
//! never race an indexing thread on the same block.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here is byte offsets and scalar counts inside one string, bounded by \
              its length"
)]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

/// The shortest view worth indexing, in bytes. A shorter one is walked: the
/// walk is short, and keeping it out means a burst of small strings cannot
/// push out the long one a program keeps slicing. It is the same 256 the
/// JavaScript runtime's `$startsKept` uses for the same job.
pub(crate) const KEPT_FROM: usize = 256;

/// Bytes per entry of an index. A lookup scans at most this many bytes; an
/// index costs four bytes per this many.
const STRIDE: usize = 64;

/// How many indices are kept at once.
const SLOTS: usize = 4;

/// What one pass over a view recorded.
struct Index {
    /// The block the view is in, as an address.
    base: usize,
    /// The address of the first byte indexed.
    start: usize,
    /// How many bytes were indexed.
    len: usize,
    /// `marks[k]` is how many scalars start in the first `k * STRIDE` bytes,
    /// for every `k` from `0` to `len / STRIDE`.
    marks: Vec<u32>,
}

struct Kept {
    slots: [Option<Index>; SLOTS],
    /// The slot the next new index replaces.
    next: usize,
}

static KEPT: Mutex<Kept> = Mutex::new(Kept { slots: [None, None, None, None], next: 0 });

/// Each slot's `base`, or `0` when the slot is empty, for [`forget`] to read
/// without the lock. Written only under it.
static KEYS: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];

/// Whether any index has ever been built. Set before the first key is stored,
/// so whatever reads a key reads this set too; until then [`forget`], which
/// every free calls, is one load rather than four.
static INDEXED: AtomicBool = AtomicBool::new(false);

/// Bytes read to find where scalars start, over the whole run: every walk, and
/// every pass that built an index. A count of work for a test to read through
/// [`buri_rt_str_scanned_bytes`], never an input to an answer.
///
/// A load and a store rather than an atomic add, so counting costs no locked
/// instruction. Two threads counting at once can lose one of the two counts;
/// the tests that read it run on one thread.
static SCANNED: AtomicU64 = AtomicU64::new(0);

fn scanned(bytes: usize) {
    SCANNED.store(SCANNED.load(Ordering::Relaxed).wrapping_add(bytes as u64), Ordering::Relaxed);
}

/// How many bytes the runtime has read to turn scalar indices into byte
/// offsets so far in this run.
///
/// A probe: `cli/tests/native/strings.rs` links a destructor that prints it,
/// so a test can say a loop of slices did linear work rather than quadratic
/// without timing anything.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_str_scanned_bytes() -> u64 {
    SCANNED.load(Ordering::Relaxed)
}

fn kept() -> MutexGuard<'static, Kept> {
    match KEPT.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Whether `b` is the first byte of a scalar, rather than a continuation byte.
fn starts(b: u8) -> bool {
    (b & 0xC0) != 0x80
}

fn count_starts(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| starts(**b)).count()
}

impl Index {
    fn build(base: usize, bytes: &[u8]) -> Index {
        scanned(bytes.len());
        let mut marks = Vec::with_capacity(bytes.len() / STRIDE + 1);
        let mut seen = 0u32;
        marks.push(0);
        for run in bytes.as_chunks::<STRIDE>().0 {
            // At most `STRIDE` per run, and the caller refused a view whose
            // byte count does not fit a `u32`.
            seen = seen.wrapping_add(count_starts(run) as u32);
            marks.push(seen);
        }
        Index { base, start: bytes.as_ptr() as usize, len: bytes.len(), marks }
    }

    fn covers(&self, base: usize, start: usize, len: usize) -> bool {
        self.base == base && self.start <= start && start + len <= self.start + self.len
    }

    /// The indexed bytes.
    ///
    /// # Safety
    /// The block is live: a caller is answering a question about a view inside
    /// it, so it holds a reference, and [`forget`] has not run since the index
    /// was built, so the bytes are the ones it was built over.
    unsafe fn bytes(&self) -> &[u8] {
        // SAFETY: as above.
        unsafe { std::slice::from_raw_parts(self.start as *const u8, self.len) }
    }

    /// How many scalars start before byte `at`, for `at <= len`.
    ///
    /// # Safety
    /// As [`Index::bytes`].
    unsafe fn scalars_before(&self, at: usize) -> usize {
        let k = at / STRIDE;
        let mark = self.marks.get(k).copied().unwrap_or(0) as usize;
        // SAFETY: forwarded.
        let bytes = unsafe { self.bytes() };
        mark + count_starts(bytes.get(k * STRIDE..at).unwrap_or(&[]))
    }

    /// The byte offset scalar `g` starts at, or `len` when there are not that
    /// many.
    ///
    /// # Safety
    /// As [`Index::bytes`].
    unsafe fn offset_of(&self, g: usize) -> usize {
        // The last run whose count before it is at most `g`: the run after it
        // has more than `g` scalars before it, so scalar `g` starts in this one.
        let k = self.marks.partition_point(|m| *m as usize <= g).saturating_sub(1);
        let mut seen = self.marks.get(k).copied().unwrap_or(0) as usize;
        // SAFETY: forwarded.
        let bytes = unsafe { self.bytes() };
        let from = k * STRIDE;
        let to = (from + STRIDE).min(self.len);
        for (at, b) in bytes.get(from..to).unwrap_or(&[]).iter().enumerate() {
            if starts(*b) {
                if seen == g {
                    return from + at;
                }
                seen += 1;
            }
        }
        self.len
    }
}

/// Whether a view is answered from an index rather than walked.
///
/// # Safety
/// `base` is null or the live payload pointer of the block `bytes` is in.
unsafe fn indexable(base: *mut u8, bytes: &[u8], ascii: bool) -> bool {
    !ascii
        && !base.is_null()
        && bytes.len() >= KEPT_FROM
        && u32::try_from(bytes.len()).is_ok()
        // SAFETY: the caller promises a live payload pointer.
        && !unsafe { crate::memory::in_arena(base) }
}

/// Run `answer` against the index covering `bytes`, building one first if no
/// kept index does. `answer` gets the index and the view's offset inside it.
fn with_index<T>(base: *mut u8, bytes: &[u8], answer: impl FnOnce(&Index, usize) -> T) -> T {
    let key = base as usize;
    let start = bytes.as_ptr() as usize;
    let mut kept = kept();
    let found = kept
        .slots
        .iter()
        .position(|s| s.as_ref().is_some_and(|ix| ix.covers(key, start, bytes.len())));
    let slot = match found {
        Some(slot) => slot,
        None => {
            let slot = kept.next;
            kept.next = (slot + 1) % SLOTS;
            if let Some(s) = kept.slots.get_mut(slot) {
                *s = Some(Index::build(key, bytes));
            }
            INDEXED.store(true, Ordering::Relaxed);
            if let Some(k) = KEYS.get(slot) {
                k.store(key, Ordering::Relaxed);
            }
            slot
        }
    };
    match kept.slots.get(slot).and_then(Option::as_ref) {
        Some(ix) => answer(ix, start - ix.start),
        // Unreachable: the slot was found full or filled just above.
        None => answer(&Index::build(key, bytes), 0),
    }
}

/// The byte offset of scalar `index` in `bytes`, or `bytes.len()` when it is
/// past the end.
///
/// O(1) when the ASCII flag is set, a lookup in a kept index for a long view,
/// and a walk otherwise.
///
/// # Safety
/// `base` is null or the live payload pointer of the block `bytes` is a view
/// into.
pub(crate) unsafe fn byte_offset(base: *mut u8, bytes: &[u8], ascii: bool, index: usize) -> usize {
    // SAFETY: forwarded.
    let [at] = unsafe { byte_offsets(base, bytes, ascii, [index]) };
    at
}

/// [`byte_offset`] for several indices, under one lock.
///
/// # Safety
/// As [`byte_offset`].
pub(crate) unsafe fn byte_offsets<const N: usize>(
    base: *mut u8,
    bytes: &[u8],
    ascii: bool,
    indices: [usize; N],
) -> [usize; N] {
    if ascii {
        return indices.map(|i| i.min(bytes.len()));
    }
    // SAFETY: forwarded.
    if unsafe { indexable(base, bytes, ascii) } {
        return with_index(base, bytes, |ix, at| {
            // SAFETY: the view is live and inside the index (module header).
            let before = unsafe { ix.scalars_before(at) };
            indices.map(|i| {
                // SAFETY: as above.
                let x = unsafe { ix.offset_of(before.saturating_add(i)) };
                x.min(at + bytes.len()) - at
            })
        });
    }
    indices.map(|i| walk(bytes, i))
}

/// The number of scalars in the first `at` bytes of `bytes`.
///
/// # Safety
/// As [`byte_offset`].
pub(crate) unsafe fn scalars_before(base: *mut u8, bytes: &[u8], ascii: bool, at: usize) -> usize {
    let at = at.min(bytes.len());
    if ascii {
        return at;
    }
    // SAFETY: forwarded.
    if unsafe { indexable(base, bytes, ascii) } {
        return with_index(base, bytes, |ix, from| {
            // SAFETY: the view is live and inside the index (module header).
            unsafe { ix.scalars_before(from + at) - ix.scalars_before(from) }
        });
    }
    scanned(at);
    count_starts(bytes.get(..at).unwrap_or(&[]))
}

/// The walk: the byte offset of the `index`th scalar start, or the length.
fn walk(bytes: &[u8], index: usize) -> usize {
    let mut seen = 0usize;
    for (at, b) in bytes.iter().enumerate() {
        if starts(*b) {
            if seen == index {
                scanned(at);
                return at;
            }
            seen = seen.saturating_add(1);
        }
    }
    scanned(bytes.len());
    bytes.len()
}

/// Drop every index of the block at `p`, because its bytes are about to change
/// or the block is about to go back. The module header lists the callers.
///
/// Inline, because `buri_rt_free` calls it on every block: the test is four
/// loads, and the call around them cost as much again.
#[inline]
pub(crate) fn forget(p: *mut u8) {
    let key = p as usize;
    if !INDEXED.load(Ordering::Relaxed) || key == 0 || KEYS.iter().all(|k| k.load(Ordering::Relaxed) != key) {
        return;
    }
    forget_slow(key);
}

/// [`forget`]'s rare half: some index is about this block.
#[cold]
#[inline(never)]
fn forget_slow(key: usize) {
    let mut kept = kept();
    for (slot, k) in kept.slots.iter_mut().zip(&KEYS) {
        if slot.as_ref().is_some_and(|ix| ix.base == key) {
            *slot = None;
            k.store(0, Ordering::Relaxed);
        }
    }
}

/// A backend wrote into `base`'s block in place, past the end of its one live
/// view: MEMORY.md §5.3's concatenation arm. The LLVM backend open-codes that
/// arm and calls this from it; the copy-and-patch backend calls
/// `buri_rt_str_concat`, which calls [`forget`] itself.
///
/// # Safety
/// `base` is null or a live payload pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_written(base: *mut u8) {
    forget(base);
}

/// The lock every runtime test that indexes a string, or asserts about the
/// kept indices or [`SCANNED`], takes: the tables are one process-wide
/// resource and `cargo test` runs cases on many threads, so one case's index
/// could otherwise push out the one another case is asserting about.
#[cfg(test)]
pub(crate) fn tests_alone() -> MutexGuard<'static, ()> {
    static ALONE: Mutex<()> = Mutex::new(());
    match ALONE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block of `text`'s bytes the tests can index, freed through the
    /// runtime so [`forget`] runs.
    struct Block(*mut u8);

    impl Block {
        fn of(text: &str) -> Block {
            let p = crate::memory::buri_rt_alloc(text.len() as u64);
            // SAFETY: a fresh block of `text.len()` bytes.
            unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len()) };
            Block(p)
        }

        fn bytes(&self, from: usize, to: usize) -> &[u8] {
            // SAFETY: inside the block, which lives as long as `self`.
            unsafe { std::slice::from_raw_parts(self.0.add(from), to - from) }
        }
    }

    impl Drop for Block {
        fn drop(&mut self) {
            // SAFETY: the only reference.
            unsafe { crate::memory::buri_rt_free(self.0) }
        }
    }

    fn walked(bytes: &[u8], i: usize) -> usize {
        walk(bytes, i)
    }

    /// Every scalar index, every byte offset and a run past the end, against
    /// the walk, over every view of a long mixed string that starts and ends on
    /// a scalar boundary at a handful of places.
    #[test]
    fn the_index_answers_what_the_walk_answers() {
        let _alone = super::tests_alone();
        let text: String = (0..400)
            .map(|i| match i % 7 {
                0 => "é",
                1 => "😀",
                2 => "語",
                _ => "a",
            })
            .collect();
        let block = Block::of(&text);
        let bounds: Vec<usize> = text.char_indices().map(|(at, _)| at).chain([text.len()]).collect();
        let cuts = [0usize, 1, 2, 63, 64, 65, 200];
        for &a in &cuts {
            for &b in &cuts {
                let from = bounds[a];
                let to = bounds[bounds.len() - 1 - b];
                if to < from {
                    continue;
                }
                let view = block.bytes(from, to);
                let scalars = text.get(from..to).expect("cuts on char boundaries").chars().count();
                for i in 0..scalars + 3 {
                    // SAFETY: `block` is live and `view` is inside it.
                    let got = unsafe { byte_offset(block.0, view, false, i) };
                    assert_eq!(got, walked(view, i), "view {from}..{to}, scalar {i}");
                }
                for at in 0..=view.len() {
                    // SAFETY: as above.
                    let got = unsafe { scalars_before(block.0, view, false, at) };
                    assert_eq!(got, count_starts(&view[..at]), "view {from}..{to}, byte {at}");
                }
            }
        }
    }

    /// A long view is read once to index it, and a question after that reads
    /// a run, not the view.
    #[test]
    fn a_second_question_does_not_read_the_view_again() {
        let _alone = super::tests_alone();
        let text = "é".repeat(4096);
        let block = Block::of(&text);
        let view = block.bytes(0, text.len());
        // SAFETY: `block` is live and `view` is inside it.
        let first = unsafe { byte_offset(block.0, view, false, 4000) };
        assert_eq!(first, 8000);
        let before = buri_rt_str_scanned_bytes();
        for i in 0..4096 {
            // SAFETY: as above.
            assert_eq!(unsafe { byte_offset(block.0, view, false, i) }, 2 * i);
        }
        // Other tests count too, so this is an upper bound, and a walk per
        // question would be thousands of times over it.
        assert!(buri_rt_str_scanned_bytes() - before < 64 * text.len() as u64);
    }

    /// The index is dropped when the block is, so a new block at the same
    /// address is indexed over its own bytes.
    #[test]
    fn a_freed_block_takes_its_index_with_it() {
        let _alone = super::tests_alone();
        let text = "é".repeat(300);
        let block = Block::of(&text);
        let p = block.0;
        // SAFETY: `block` is live.
        let _ = unsafe { byte_offset(p, block.bytes(0, text.len()), false, 10) };
        assert!(KEYS.iter().any(|k| k.load(Ordering::Relaxed) == p as usize));
        drop(block);
        assert!(KEYS.iter().all(|k| k.load(Ordering::Relaxed) != p as usize));
    }

    /// A concatenation that writes into the block drops its index, so a view
    /// over the new bytes is not answered from the old ones.
    #[test]
    fn a_write_into_the_block_takes_its_index_with_it() {
        let _alone = super::tests_alone();
        let text = "é".repeat(300);
        let block = Block::of(&text);
        // SAFETY: `block` is live.
        let _ = unsafe { byte_offset(block.0, block.bytes(0, text.len()), false, 10) };
        // SAFETY: as above.
        unsafe { buri_rt_str_written(block.0) };
        assert!(KEYS.iter().all(|k| k.load(Ordering::Relaxed) != block.0 as usize));
    }
}
