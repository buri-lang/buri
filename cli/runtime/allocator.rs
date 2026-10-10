//! The global allocator of the toolchain and of every program it compiles:
//! blocks of up to 1 KiB come from per-thread pages of one size class each,
//! everything else from the system allocator.
//!
//! The compiler is millions of small allocations, and so is a program that
//! builds a tree. Here a thread allocates and frees its own blocks with no
//! atomic operation at all, and successive allocations of a size sit next to
//! each other in one page. `design/PERFORMANCE.md` §6.54 and §6.67 have the
//! measurements.
//!
//! - **Size classes** are 16-byte steps to 128 bytes, then four to each
//!   doubling. Rust's `dealloc` passes the layout, so a block needs no header:
//!   its class is recomputed from the size.
//! - **Memory** is one address range reserved at first use and cut into 32 KiB
//!   pages. A page belongs to one heap and one class, and its header at the
//!   page's start holds its free list.
//! - **A heap** is one per thread, with a list of pages per class that may have
//!   room. It allocates from the first until that one is spent.
//! - **A free on the owning thread** pushes onto the page's list. A spent page
//!   gets room again this way and goes back on the heap's list.
//! - **A free on any other thread** pushes onto the page's remote stack with a
//!   compare-and-swap. The owner takes the whole stack with one swap, so there is
//!   no ABA. The first such free also queues the page on its heap, which is how
//!   a spent page's owner learns it has room again.
//! - **A page with no block out** goes to a shared pool, and any heap takes it
//!   from there for any class. Each remote free bumps a count on the page, so
//!   the owner knows when the last block is back.
//! - **A thread that exits** leaves its heap on an abandoned list, and the next
//!   new thread adopts it whole. The parallel passes start fresh workers each
//!   time, and this is how their memory comes back.
//! - **While a thread has no heap** (starting, exiting, or the range is used
//!   up) its small blocks come from the system. Those lie outside the range,
//!   which is how `dealloc` tells them apart.
//! - **`trim`** gives all but 8 MB of the pool back to the system. `buri lsp`
//!   calls it whenever no request is waiting, the watch loops after each pass,
//!   and a compiled program on each sweep of its block cache.
//!
//! Nothing here allocates or takes a `Mutex`: the pool and the abandoned list
//! are behind spin locks, and a heap is never freed.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "sizes are at most `MAX_SMALL`, and offsets are bounded by the reserved range"
)]
#![allow(
    clippy::indexing_slicing,
    reason = "a class is below `CLASSES` by construction in `class`"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr::{self, null_mut};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize};

const STEP: usize = 16;
/// 16-byte steps up to here, then four classes to each doubling.
const LINEAR: usize = 128;
const MAX_SMALL_SHIFT: u32 = 10;
const MAX_SMALL: usize = 1 << MAX_SMALL_SHIFT;
const CLASSES: usize = LINEAR / STEP + 4 * (MAX_SMALL_SHIFT - LINEAR.trailing_zeros()) as usize;
/// Each class's block size.
const SIZES: [usize; CLASSES] = {
    let mut sizes = [0; CLASSES];
    let mut c = 0;
    while c < CLASSES {
        sizes[c] = if c < LINEAR / STEP {
            (c + 1) * STEP
        } else {
            let k = c - LINEAR / STEP;
            let e = LINEAR.trailing_zeros() as usize + k / 4;
            (1 << e) + ((k % 4 + 1) << (e - 2))
        };
        c += 1;
    }
    sizes
};

const PAGE: usize = 32 << 10;
/// The page header's share of each page: two cache lines.
const HEADER: usize = 256;
/// Address space, not memory: pages are committed as they're touched.
const RANGE: usize = 32 << 30;

/// `BASE` before the range is reserved, and after reserving failed. Both are at
/// or above 2^63, so no real pointer is ever within `RANGE` of them.
const UNRESERVED: usize = 1 << 63;
const NO_RANGE: usize = UNRESERVED | PAGE;

static BASE: AtomicPtr<u8> = AtomicPtr::new(ptr::without_provenance_mut(UNRESERVED));
static NEXT_PAGE: AtomicUsize = AtomicUsize::new(0);

static ABANDONED_LOCK: AtomicBool = AtomicBool::new(false);
static ABANDONED: AtomicPtr<Heap> = AtomicPtr::new(null_mut());

/// Pages with no block out, for any heap and class. `trim` gives all but
/// `KEEP` of them back to the system and onto `RELEASED`.
static POOL_LOCK: AtomicBool = AtomicBool::new(false);
static POOL: AtomicPtr<Page> = AtomicPtr::new(null_mut());
static POOLED: AtomicUsize = AtomicUsize::new(0);
const KEEP: usize = KEEP_BYTES / PAGE;
const KEEP_BYTES: usize = 8 << 20;

/// A stack of released pages' indices. A released page's own bytes may be
/// gone, so the stack can't live in its header.
static RELEASED: [AtomicU32; RANGE / PAGE] = {
    // SAFETY: zero is a valid `AtomicU32`.
    unsafe { std::mem::zeroed() }
};
static RELEASED_TOP: AtomicUsize = AtomicUsize::new(0);

/// The installed allocator.
pub struct Allocator;

#[repr(C, align(128))]
struct Page {
    // The owner's line.
    free: *mut u8,
    /// Blocks from here to `end` have never been handed out.
    bump: *mut u8,
    end: *mut u8,
    /// The heap's list for this class, or the pool's.
    next: *mut Page,
    prev: *mut Page,
    owner: *mut Heap,
    class: usize,
    /// Blocks handed out, less those freed here and the remote frees counted so far.
    used: usize,
    /// Off the heap's list, with no room as of the last look.
    spent: bool,
    remote: PageRemote,
}

/// The line other threads write.
#[repr(C, align(128))]
struct PageRemote {
    blocks: AtomicPtr<u8>,
    /// On the heap's `queued` stack, or about to be.
    queued: AtomicBool,
    next_queued: AtomicPtr<Page>,
    /// Remote frees not yet subtracted from `used`.
    freed: AtomicUsize,
}

const _: () = assert!(size_of::<Page>() <= HEADER && HEADER.is_multiple_of(STEP));

#[repr(C, align(128))]
struct Heap {
    pages: [*mut Page; CLASSES],
    next_abandoned: *mut Heap,
    /// Pages that other threads freed into since the owner last looked.
    queued: Queued,
}

#[repr(C, align(128))]
struct Queued(AtomicPtr<Page>);

/// A thread whose small blocks come from the system for now.
const SYSTEM: *mut Heap = ptr::without_provenance_mut(1);

thread_local! {
    /// Null until the thread's first small allocation, then its heap or `SYSTEM`.
    static HEAP: Cell<*mut Heap> = const { Cell::new(null_mut()) };
    static EXIT: Exit = const { Exit };
}

/// Hands the thread's heap to the abandoned list when the thread exits.
struct Exit;

impl Drop for Exit {
    fn drop(&mut self) {
        let heap = HEAP.try_with(|h| h.replace(SYSTEM)).unwrap_or(SYSTEM);
        if heap != SYSTEM && !heap.is_null() {
            let _held = lock(&ABANDONED_LOCK);
            // SAFETY: the heap left this thread above, and the lock is held.
            unsafe { (*heap).next_abandoned = ABANDONED.load(Relaxed) };
            ABANDONED.store(heap, Relaxed);
        }
    }
}

struct Held(&'static AtomicBool);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.store(false, Release);
    }
}

fn lock(flag: &'static AtomicBool) -> Held {
    while flag.compare_exchange_weak(false, true, Acquire, Relaxed).is_err() {
        std::thread::yield_now();
    }
    Held(flag)
}

#[inline]
fn class(layout: Layout) -> Option<usize> {
    if layout.size() > MAX_SMALL || layout.align() > STEP {
        return None;
    }
    // `GlobalAlloc` is never asked for zero bytes.
    let s = layout.size().saturating_sub(1);
    if s < LINEAR {
        return Some(s / STEP);
    }
    let e = usize::BITS - 1 - s.leading_zeros();
    Some(LINEAR / STEP + (e - LINEAR.trailing_zeros()) as usize * 4 + (s >> (e - 2)) % 4)
}

#[inline]
fn class_layout(c: usize) -> Layout {
    // SAFETY: a multiple of 16 no larger than `MAX_SMALL`, at a power-of-two alignment.
    unsafe { Layout::from_size_align_unchecked(SIZES[c], STEP) }
}

/// The page holding `p`, if `p` is in the range.
#[inline]
fn page_of(p: *mut u8) -> Option<*mut Page> {
    let offset = p.addr().wrapping_sub(BASE.load(Relaxed).addr());
    (offset < RANGE).then(|| p.map_addr(|a| a & !(PAGE - 1)).cast::<Page>())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reserve() -> *mut u8 {
    use std::ffi::c_void;
    // `c_void`, as `memory.rs` declares them in the runtime.
    unsafe extern "C" {
        fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
    }
    const PROT_READ_WRITE: i32 = 1 | 2;
    #[cfg(target_os = "macos")]
    const FLAGS: i32 = 0x0002 | 0x1000 | 0x0040; // MAP_PRIVATE | MAP_ANON | MAP_NORESERVE
    #[cfg(target_os = "linux")]
    const FLAGS: i32 = 0x02 | 0x20 | 0x4000; // MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE

    // One page more than the range, so the range can start on a page boundary.
    // SAFETY: a fresh anonymous mapping, at an address of the kernel's choosing.
    let p = unsafe { mmap(null_mut(), RANGE + PAGE, PROT_READ_WRITE, FLAGS, -1, 0) }.cast::<u8>();
    let base = if p.addr() == usize::MAX {
        ptr::without_provenance_mut(NO_RANGE)
    } else {
        p.wrapping_add(p.addr().wrapping_neg() % PAGE)
    };
    let unreserved = ptr::without_provenance_mut(UNRESERVED);
    match BASE.compare_exchange(unreserved, base, Relaxed, Relaxed) {
        Ok(_) => base,
        Err(theirs) => {
            if base.addr() != NO_RANGE {
                // SAFETY: the mapping made above, which nothing else has seen.
                unsafe { munmap(p.cast(), RANGE + PAGE) };
            }
            theirs
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn reserve() -> *mut u8 {
    let unreserved = ptr::without_provenance_mut(UNRESERVED);
    let none = ptr::without_provenance_mut(NO_RANGE);
    match BASE.compare_exchange(unreserved, none, Relaxed, Relaxed) {
        Ok(_) => none,
        Err(theirs) => theirs,
    }
}

/// A fresh page of class `c` for `heap`, or null when the range is unavailable
/// or used up.
#[cold]
fn new_page(heap: *mut Heap, c: usize) -> *mut Page {
    let (pooled, released) = {
        let _held = lock(&POOL_LOCK);
        let page = POOL.load(Relaxed);
        if !page.is_null() {
            // SAFETY: a pooled page, owned by the pool, and the lock is held.
            POOL.store(unsafe { (*page).next }, Relaxed);
            POOLED.store(POOLED.load(Relaxed) - 1, Relaxed);
            (page, None)
        } else {
            let top = RELEASED_TOP.load(Relaxed);
            if top == 0 {
                (null_mut(), None)
            } else {
                RELEASED_TOP.store(top - 1, Relaxed);
                (null_mut(), Some(RELEASED[top - 1].load(Relaxed) as usize))
            }
        }
    };
    let page = if !pooled.is_null() {
        pooled.cast::<u8>()
    } else {
        let mut base = BASE.load(Relaxed);
        if base.addr() == UNRESERVED {
            base = reserve();
        }
        if base.addr() == NO_RANGE {
            return null_mut();
        }
        let index = match released {
            Some(index) => index,
            None => NEXT_PAGE.fetch_add(1, Relaxed),
        };
        if index >= RANGE / PAGE {
            return null_mut();
        }
        // SAFETY: `index` is below the range's page count, so the page lies inside the mapping.
        let page = unsafe { base.add(index * PAGE) };
        if released.is_some() {
            os::reuse(page);
        }
        page
    };
    let blocks = (PAGE - HEADER) / SIZES[c];
    // SAFETY: the page is fresh and ours, and the header fits before its first block.
    unsafe {
        page.cast::<Page>().write(Page {
            free: null_mut(),
            bump: page.add(HEADER),
            end: page.add(HEADER + blocks * SIZES[c]),
            next: null_mut(),
            prev: null_mut(),
            owner: heap,
            class: c,
            used: 0,
            spent: false,
            remote: PageRemote {
                blocks: AtomicPtr::new(null_mut()),
                queued: AtomicBool::new(false),
                next_queued: AtomicPtr::new(null_mut()),
                freed: AtomicUsize::new(0),
            },
        });
    }
    page.cast()
}

/// The calling thread's heap, adopted or made, or `SYSTEM`.
#[cold]
fn start_thread() -> *mut Heap {
    // Registering the exit hook may allocate, and that allocation comes from the system.
    let _ = HEAP.try_with(|h| h.set(SYSTEM));
    // For good: memcheck sees system blocks one by one, and pages as one mapping.
    if valgrind::reroutes() {
        return SYSTEM;
    }
    if EXIT.try_with(|_| ()).is_err() {
        return SYSTEM;
    }
    let adopted = {
        let _held = lock(&ABANDONED_LOCK);
        let heap = ABANDONED.load(Relaxed);
        if !heap.is_null() {
            // SAFETY: an abandoned heap, owned by the list, and the lock is held.
            ABANDONED.store(unsafe { (*heap).next_abandoned }, Relaxed);
        }
        heap
    };
    let heap = if adopted.is_null() {
        // SAFETY: a non-zero size; the heap is written in full before it's used.
        let fresh = unsafe { System.alloc(Layout::new::<Heap>()) }.cast::<Heap>();
        if fresh.is_null() {
            return SYSTEM;
        }
        // SAFETY: just allocated with this type's layout.
        unsafe {
            fresh.write(Heap {
                pages: [null_mut(); CLASSES],
                next_abandoned: null_mut(),
                queued: Queued(AtomicPtr::new(null_mut())),
            });
        }
        fresh
    } else {
        adopted
    };
    let _ = HEAP.try_with(|h| h.set(heap));
    heap
}

/// Puts a spent page back on its heap's list, second, so the page being
/// allocated from stays first.
///
/// # Safety
/// `heap` is the calling thread's, and owns `page`.
unsafe fn revive(heap: *mut Heap, page: *mut Page) {
    // SAFETY: both are this thread's.
    unsafe {
        (*page).spent = false;
        let head = &mut (*heap).pages[(*page).class];
        if head.is_null() {
            *head = page;
            return;
        }
        let after = (**head).next;
        (*page).prev = *head;
        (*page).next = after;
        if !after.is_null() {
            (*after).prev = page;
        }
        (**head).next = page;
    }
}

/// Takes `page` off its heap's list.
///
/// # Safety
/// `heap` is the calling thread's, and owns `page`, which is on the list.
unsafe fn unlink(heap: *mut Heap, page: *mut Page) {
    // SAFETY: both are this thread's.
    unsafe {
        let (prev, next) = ((*page).prev, (*page).next);
        if prev.is_null() {
            (*heap).pages[(*page).class] = next;
        } else {
            (*prev).next = next;
        }
        if !next.is_null() {
            (*next).prev = prev;
        }
        (*page).prev = null_mut();
        (*page).next = null_mut();
    }
}

/// Hands a page with no block out to the pool, unless it's the page being
/// allocated from or a remote free has queued it, in which case `drain` will
/// look again.
///
/// # Safety
/// `heap` is the calling thread's, and owns `page`, whose `used` is zero.
#[cold]
unsafe fn retire(heap: *mut Heap, page: *mut Page) {
    // SAFETY: both are this thread's, and no block of `page` is out, so no
    // other thread can touch it once it's off the queue.
    unsafe {
        if (*heap).pages[(*page).class] == page || (*page).remote.queued.load(Acquire) {
            return;
        }
        if !(*page).spent {
            unlink(heap, page);
        }
        let _held = lock(&POOL_LOCK);
        (*page).next = POOL.load(Relaxed);
        POOL.store(page, Relaxed);
        POOLED.store(POOLED.load(Relaxed) + 1, Relaxed);
    }
}

/// Gives the pool's pages past the first `KEEP` back to the system.
///
/// For a process about to sit idle, such as `buri lsp` between requests. A
/// build doesn't call it: its pages are about to be reused or the process is
/// about to exit, and releasing them cost it 1–2%.
pub fn trim() {
    if !os::RELEASES || POOLED.load(Relaxed) <= KEEP {
        return;
    }
    let mut excess = {
        let _held = lock(&POOL_LOCK);
        if POOLED.load(Relaxed) <= KEEP {
            return;
        }
        let mut page = POOL.load(Relaxed);
        // Keep the first `KEEP`, detach the rest.
        let mut kept = 1;
        while kept < KEEP && !page.is_null() {
            // SAFETY: pooled pages, and the lock is held.
            page = unsafe { (*page).next };
            kept += 1;
        }
        if page.is_null() {
            return;
        }
        // SAFETY: as above.
        let rest = unsafe { std::mem::replace(&mut (*page).next, null_mut()) };
        POOLED.store(KEEP, Relaxed);
        rest
    };
    while !excess.is_null() {
        // SAFETY: detached from the pool above, so these pages are this call's.
        let next = unsafe { (*excess).next };
        let index = (excess.addr() - BASE.load(Relaxed).addr()) / PAGE;
        os::release(excess.cast());
        {
            let _held = lock(&POOL_LOCK);
            let top = RELEASED_TOP.load(Relaxed);
            RELEASED[top].store(index as u32, Relaxed);
            RELEASED_TOP.store(top + 1, Relaxed);
        }
        excess = next;
    }
}

/// Giving a page's memory back to the system, and taking it again.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod os {
    use super::PAGE;

    unsafe extern "C" {
        fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    }

    pub const RELEASES: bool = true;

    /// `MADV_FREE_REUSABLE`, what the system allocator uses: the page leaves
    /// the footprint now, and the kernel takes it when it wants it.
    #[cfg(target_os = "macos")]
    const RELEASE: i32 = 7;
    /// `MADV_DONTNEED`: the page leaves RSS now and reads as zeros after.
    #[cfg(target_os = "linux")]
    const RELEASE: i32 = 4;

    pub fn release(page: *mut u8) {
        // SAFETY: a whole page of the range that no block is in.
        unsafe { madvise(page, PAGE, RELEASE) };
    }

    /// `MADV_FREE_REUSE`, so the page counts in the footprint again.
    #[cfg(target_os = "macos")]
    pub fn reuse(page: *mut u8) {
        // SAFETY: a whole page of the range, released earlier.
        unsafe { madvise(page, PAGE, 8) };
    }

    #[cfg(target_os = "linux")]
    pub fn reuse(_page: *mut u8) {}
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod os {
    pub const RELEASES: bool = false;
    pub fn release(_page: *mut u8) {}
    pub fn reuse(_page: *mut u8) {}
}

/// Valgrind client requests, so memcheck sees each block rather than a few big
/// pages (`design/native/MEMORY.md` §8).
///
/// Only with the `memcheck` feature. Without it [`valgrind::memcheck`] is a
/// constant `false` and none of this is in the binary. With it, a program that
/// isn't under memcheck asks once. Under memcheck:
///
/// - small blocks come from the system rather than from pages, and
///   `memory.rs` caches none, so every block is allocated and freed where
///   memcheck can see it;
/// - on musl, whose `malloc` memcheck can't replace in a static binary, those
///   system blocks are annotated here, with redzones, and a freed one waits
///   behind 20 MB of later frees before it's reused, as memcheck's own do;
/// - task stacks are registered as stacks.
///
/// cachegrind and callgrind see none of this, so the instruction gate measures
/// the allocator a program ships with.
///
/// An in-process loader that maps code must also discard Valgrind's
/// translations of it (`VALGRIND_DISCARD_TRANSLATIONS`, request `0x1002`).
pub mod valgrind {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::ptr::null_mut;
    use std::sync::atomic::Ordering::Relaxed;
    use std::sync::atomic::{AtomicBool, AtomicU8};

    const MALLOCLIKE_BLOCK: usize = 0x1301;
    const FREELIKE_BLOCK: usize = 0x1302;
    const RESIZEINPLACE_BLOCK: usize = 0x130b;
    const STACK_REGISTER: usize = 0x1501;
    const STACK_DEREGISTER: usize = 0x1502;
    /// Memcheck's own requests start at `'M' << 24 | 'C' << 16`.
    const MAKE_MEM_NOACCESS: usize = 0x4d43_0000;
    const MAKE_MEM_UNDEFINED: usize = 0x4d43_0001;
    const MAKE_MEM_DEFINED: usize = 0x4d43_0002;
    #[cfg(feature = "memcheck")]
    const CHECK_MEM_IS_ADDRESSABLE: usize = 0x4d43_0004;

    /// Whether system blocks are annotated here: only where memcheck can't
    /// replace `malloc`.
    pub(super) const ANNOTATES: bool = cfg!(all(feature = "memcheck", target_env = "musl", not(test)));

    /// Whether memcheck changes how this process allocates: pages, caches and
    /// the heap check's quarantine all step aside. Not under `cfg(test)`, whose
    /// tests are about those.
    pub fn reroutes() -> bool {
        !cfg!(test) && memcheck()
    }

    /// Marks `[p, p + len)` as written, for a read memcheck can't follow: a
    /// task stack's watermark, below where its stack pointer was.
    #[cold]
    #[inline(never)]
    pub fn make_defined(p: *mut u8, len: usize) {
        mark(MAKE_MEM_DEFINED, p, len);
    }

    /// Bytes on each side of an annotated block. An overrun lands in them.
    const REDZONE: usize = 32;

    /// Freed bytes held back from reuse: memcheck's own `--freelist-vol`.
    const QUARANTINE: usize = 20 << 20;

    /// The request's answer, or `default` outside Valgrind.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[inline(always)]
    fn request(default: usize, args: [usize; 6]) -> usize {
        let result;
        // SAFETY: the rotations total 128 bits, so `rdi` is unchanged, and
        // `xchg rbx, rbx` is a no-op. Valgrind reads `args` through `rax`.
        unsafe {
            std::arch::asm!(
                "rol rdi, 3", "rol rdi, 13", "rol rdi, 61", "rol rdi, 51", "xchg rbx, rbx",
                in("rax") args.as_ptr(),
                inout("rdx") default => result,
                options(nostack),
            );
        }
        result
    }

    /// The request's answer, or `default` outside Valgrind.
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    #[inline(always)]
    fn request(default: usize, args: [usize; 6]) -> usize {
        let result;
        // SAFETY: the rotations total 128 bits, so `x12` is unchanged, and
        // `orr x10, x10, x10` is a no-op. Valgrind reads `args` through `x4`.
        unsafe {
            std::arch::asm!(
                "ror x12, x12, #3", "ror x12, x12, #13", "ror x12, x12, #51", "ror x12, x12, #61",
                "orr x10, x10, x10",
                in("x4") args.as_ptr(),
                inout("x3") default => result,
                options(nostack),
            );
        }
        result
    }

    /// Valgrind doesn't run here.
    #[cfg(not(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))))]
    #[inline(always)]
    fn request(default: usize, _args: [usize; 6]) -> usize {
        default
    }

    fn mark(what: usize, p: *mut u8, len: usize) {
        request(0, [what, p.addr(), len, 0, 0, 0]);
    }

    /// Whether this process runs under memcheck, by a request only memcheck
    /// answers. Asked once.
    #[inline]
    pub fn memcheck() -> bool {
        #[cfg(not(feature = "memcheck"))]
        return false;
        #[cfg(feature = "memcheck")]
        match STATE.load(Relaxed) {
            1 => false,
            2 => true,
            _ => decide(),
        }
    }

    /// [`memcheck`]'s answer: `0` until it's asked, then `1` for no, `2` for yes.
    static STATE: AtomicU8 = AtomicU8::new(0);

    /// Whether a block that exists was annotated: [`memcheck`] without the
    /// call to ask, which the allocation of that block already made.
    #[inline(always)]
    pub(super) fn annotating() -> bool {
        ANNOTATES && STATE.load(Relaxed) == 2
    }

    // Every request is out of line: its arguments are an array on the stack,
    // which would give the allocator's fast path a frame.
    #[cfg(feature = "memcheck")]
    #[cold]
    #[inline(never)]
    fn decide() -> bool {
        let probe = 0u8;
        let at = std::ptr::addr_of!(probe).addr();
        let yes = request(1, [CHECK_MEM_IS_ADDRESSABLE, at, 1, 0, 0, 0]) == 0;
        STATE.store(if yes { 2 } else { 1 }, Relaxed);
        yes
    }

    fn redzone(align: usize) -> usize {
        REDZONE.max(align)
    }

    /// A system block for `chunk`, of which memcheck sees the first `size` bytes.
    ///
    /// # Safety
    /// `chunk` has a non-zero size.
    #[cold]
    #[inline(never)]
    pub(super) unsafe fn alloc(chunk: Layout, size: usize, zeroed: bool) -> *mut u8 {
        let rz = redzone(chunk.align());
        let Ok(outer) = Layout::from_size_align(chunk.size().saturating_add(2 * rz), chunk.align()) else {
            return null_mut();
        };
        // SAFETY: a non-zero size.
        let base = unsafe { if zeroed { System.alloc_zeroed(outer) } else { System.alloc(outer) } };
        if base.is_null() {
            return base;
        }
        let p = base.wrapping_add(rz);
        mark(MAKE_MEM_NOACCESS, base, outer.size());
        request(0, [MALLOCLIKE_BLOCK, p.addr(), size, rz, usize::from(zeroed), 0]);
        p
    }

    /// `p`, from [`alloc`] for `chunk`, now holds `new` bytes rather than `old`.
    #[cold]
    #[inline(never)]
    pub(super) fn resize(p: *mut u8, chunk: Layout, old: usize, new: usize) {
        request(0, [RESIZEINPLACE_BLOCK, p.addr(), old, new, redzone(chunk.align()), 0]);
    }

    /// What a held block's redzone records, to find the next one and free this.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Link {
        next: *mut u8,
        size: usize,
        align: usize,
    }

    const LINK: usize = size_of::<Link>();
    const _: () = assert!(LINK <= REDZONE);

    static LOCK: AtomicBool = AtomicBool::new(false);
    /// The oldest and newest held blocks, and the bytes held, behind `LOCK`.
    static mut HEAD: *mut u8 = null_mut();
    static mut TAIL: *mut u8 = null_mut();
    static mut HELD: usize = 0;

    /// Frees `p` for memcheck, and holds its memory back from reuse.
    ///
    /// # Safety
    /// `p` came from [`alloc`] for `chunk`, and nothing uses it again.
    #[cold]
    #[inline(never)]
    pub(super) unsafe fn dealloc(p: *mut u8, chunk: Layout) {
        let rz = redzone(chunk.align());
        request(0, [FREELIKE_BLOCK, p.addr(), rz, 0, 0, 0]);
        let base = p.wrapping_sub(rz);
        let size = chunk.size() + 2 * rz;
        if size > QUARANTINE {
            // SAFETY: the size and alignment `alloc` made the block with.
            unsafe { release(base, Link { next: null_mut(), size, align: chunk.align() }) };
            return;
        }
        let _held = super::lock(&LOCK);
        // SAFETY: the lock is held, and every link is in a held block's redzone.
        unsafe {
            write_link(base, Link { next: null_mut(), size, align: chunk.align() });
            if TAIL.is_null() {
                HEAD = base;
            } else {
                write_link(TAIL, Link { next: base, ..read_link(TAIL) });
            }
            TAIL = base;
            HELD += size;
            while HELD > QUARANTINE {
                let oldest = HEAD;
                let link = read_link(oldest);
                HEAD = link.next;
                if HEAD.is_null() {
                    TAIL = null_mut();
                }
                HELD -= link.size;
                release(oldest, link);
            }
        }
    }

    /// The link in a held block's redzone, opened for the read and closed again.
    unsafe fn read_link(base: *mut u8) -> Link {
        mark(MAKE_MEM_DEFINED, base, LINK);
        // SAFETY: the caller's held block, which starts with its link.
        let link = unsafe { base.cast::<Link>().read_unaligned() };
        mark(MAKE_MEM_NOACCESS, base, LINK);
        link
    }

    unsafe fn write_link(base: *mut u8, link: Link) {
        mark(MAKE_MEM_UNDEFINED, base, LINK);
        // SAFETY: the caller's held block, whose redzone is at least `LINK` bytes.
        unsafe { base.cast::<Link>().write_unaligned(link) };
        mark(MAKE_MEM_NOACCESS, base, LINK);
    }

    /// Gives a block back to the system. Defined, as untracked memory is:
    /// musl's `malloc` puts its own header inside a slot it hands out again.
    unsafe fn release(base: *mut u8, link: Link) {
        mark(MAKE_MEM_DEFINED, base, link.size);
        // SAFETY: `base` came from `System` at this size and alignment.
        unsafe { System.dealloc(base, Layout::from_size_align_unchecked(link.size, link.align)) };
    }

    /// Tells memcheck `[low, high)` is a stack, and answers its id.
    #[cold]
    #[inline(never)]
    pub fn stack_register(low: *mut u8, high: *mut u8) -> usize {
        request(0, [STACK_REGISTER, low.addr(), high.addr(), 0, 0, 0])
    }

    /// Forgets the stack [`stack_register`] answered `id` for.
    #[cold]
    #[inline(never)]
    pub fn stack_deregister(id: usize) {
        request(0, [STACK_DEREGISTER, id, 0, 0, 0, 0]);
    }
}

/// [`System`], or under memcheck on musl an annotated block for `chunk`, of
/// which the caller asked for `size` bytes.
///
/// Out of line where it annotates, so `alloc` and `dealloc` stay small enough
/// to inline into the cache's sweep.
///
/// # Safety
/// `chunk` has a non-zero size.
#[cfg_attr(all(feature = "memcheck", target_env = "musl", not(test)), inline(never))]
#[cfg_attr(not(all(feature = "memcheck", target_env = "musl", not(test))), inline)]
unsafe fn system_alloc(chunk: Layout, size: usize, zeroed: bool) -> *mut u8 {
    if valgrind::ANNOTATES && valgrind::memcheck() {
        // SAFETY: forwarded.
        return unsafe { valgrind::alloc(chunk, size, zeroed) };
    }
    // SAFETY: forwarded.
    unsafe { if zeroed { System.alloc_zeroed(chunk) } else { System.alloc(chunk) } }
}

/// Gives back a block [`system_alloc`] made for `chunk`.
///
/// # Safety
/// `p` came from [`system_alloc`] for `chunk`.
#[cfg_attr(all(feature = "memcheck", target_env = "musl", not(test)), inline(never))]
#[cfg_attr(not(all(feature = "memcheck", target_env = "musl", not(test))), inline)]
unsafe fn system_dealloc(p: *mut u8, chunk: Layout) {
    if valgrind::annotating() {
        // SAFETY: forwarded.
        return unsafe { valgrind::dealloc(p, chunk) };
    }
    // SAFETY: forwarded.
    unsafe { System.dealloc(p, chunk) }
}

impl Allocator {
    /// A block of class `c` when the first page on the list has nothing free.
    #[cold]
    #[inline(never)]
    unsafe fn refill(heap: *mut Heap, c: usize) -> *mut u8 {
        let mut drained = false;
        loop {
            // SAFETY: this thread's heap and its pages, used by no other thread
            // except through the atomics.
            unsafe {
                let page = (*heap).pages[c];
                if page.is_null() {
                    if drained {
                        break;
                    }
                    drained = true;
                    Self::drain(heap);
                    continue;
                }
                let p = &mut *page;
                if p.bump < p.end {
                    let block = p.bump;
                    p.bump = block.add(SIZES[c]);
                    p.used += 1;
                    return block;
                }
                if !p.remote.blocks.load(Relaxed).is_null() {
                    let block = p.remote.blocks.swap(null_mut(), Acquire);
                    // Each block was pushed with its link written, released by the push.
                    p.free = block.cast::<*mut u8>().read();
                    p.used = p.used - p.remote.freed.swap(0, Acquire) + 1;
                    return block;
                }
                p.spent = true;
                unlink(heap, page);
            }
        }
        let page = new_page(heap, c);
        if page.is_null() {
            // SAFETY: a non-zero size.
            return unsafe { system_alloc(class_layout(c), SIZES[c], false) };
        }
        // SAFETY: a fresh page, which has room for at least one block.
        unsafe {
            (*heap).pages[c] = page;
            let block = (*page).bump;
            (*page).bump = block.add(SIZES[c]);
            (*page).used = 1;
            block
        }
    }

    /// Takes the pages other threads have freed into back onto the lists.
    #[cold]
    unsafe fn drain(heap: *mut Heap) {
        // SAFETY: this thread's heap; a queued page is one of its own.
        unsafe {
            let mut page = (*heap).queued.0.swap(null_mut(), Acquire);
            while !page.is_null() {
                let next = (*page).remote.next_queued.load(Relaxed);
                // After reading `next`: a free that sees this may queue the page again.
                (*page).remote.queued.store(false, Release);
                (*page).used -= (*page).remote.freed.swap(0, Acquire);
                if (*page).used == 0 {
                    retire(heap, page);
                } else if (*page).spent {
                    revive(heap, page);
                }
                page = next;
            }
        }
    }
}

// SAFETY: a block of class `c` is `SIZES[c]` bytes at 16-byte alignment, from a
// page in the range or from the system, and is handed out once until it's freed
// again. A block in the range goes back to its page; one outside it goes back to
// the system with the layout it was allocated with.
unsafe impl GlobalAlloc for Allocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(c) = class(layout) else {
            // SAFETY: the caller's contract, passed through.
            return unsafe { system_alloc(layout, layout.size(), false) };
        };
        let mut heap = HEAP.try_with(Cell::get).unwrap_or(SYSTEM);
        if heap.is_null() {
            heap = start_thread();
        }
        if heap == SYSTEM {
            // SAFETY: a non-zero size.
            return unsafe { system_alloc(class_layout(c), layout.size(), false) };
        }
        // SAFETY: this thread's heap and page; the list holds free blocks of class `c`.
        unsafe {
            let page = (*heap).pages[c];
            if !page.is_null() {
                let block = (*page).free;
                if !block.is_null() {
                    (*page).free = block.cast::<*mut u8>().read();
                    (*page).used += 1;
                    return block;
                }
            }
            Self::refill(heap, c)
        }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let Some(c) = class(layout) else {
            // SAFETY: the caller's contract, passed through.
            return unsafe { system_dealloc(p, layout) };
        };
        let Some(page) = page_of(p) else {
            // SAFETY: a small block outside the range came from the system at its class's layout.
            return unsafe { system_dealloc(p, class_layout(c)) };
        };
        let heap = HEAP.try_with(Cell::get).unwrap_or(SYSTEM);
        // SAFETY: `p` is a free block of `page`, whose `owner` never changes.
        unsafe {
            if (*page).owner == heap {
                p.cast::<*mut u8>().write((*page).free);
                (*page).free = p;
                (*page).used -= 1;
                if (*page).used == 0 {
                    retire(heap, page);
                } else if (*page).spent {
                    revive(heap, page);
                }
                return;
            }
            let remote = &(*page).remote;
            let mut head = remote.blocks.load(Relaxed);
            loop {
                p.cast::<*mut u8>().write(head);
                match remote.blocks.compare_exchange_weak(head, p, Release, Relaxed) {
                    Ok(_) => break,
                    Err(now) => head = now,
                }
            }
            if !remote.queued.load(Relaxed) && !remote.queued.swap(true, AcqRel) {
                let queued = &(*(*page).owner).queued.0;
                let mut top = queued.load(Relaxed);
                loop {
                    remote.next_queued.store(top, Relaxed);
                    match queued.compare_exchange_weak(top, page, Release, Relaxed) {
                        Ok(_) => break,
                        Err(now) => top = now,
                    }
                }
            }
            // Last: once the owner counts this, nothing here touches the page again.
            remote.freed.fetch_add(1, Release);
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if class(layout).is_none() {
            // SAFETY: the caller's contract, passed through.
            return unsafe { system_alloc(layout, layout.size(), true) };
        }
        // SAFETY: the caller's contract.
        let p = unsafe { self.alloc(layout) };
        if !p.is_null() {
            // SAFETY: `p` holds at least `layout.size()` bytes.
            unsafe { p.write_bytes(0, layout.size()) };
        }
        p
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's contract makes this a valid layout.
        let new = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        match (class(layout), class(new)) {
            // A block holds its whole class, wherever it came from.
            (Some(a), Some(b)) if a == b => {
                if valgrind::annotating() {
                    valgrind::resize(p, class_layout(a), layout.size(), new_size);
                }
                p
            }
            // SAFETY: the caller's contract, passed through.
            (None, None) if !valgrind::annotating() => unsafe {
                System.realloc(p, layout, new_size)
            },
            _ => {
                // SAFETY: the caller's contract.
                let q = unsafe { self.alloc(new) };
                if !q.is_null() {
                    // SAFETY: both blocks hold the smaller size, and they're distinct.
                    unsafe {
                        ptr::copy_nonoverlapping(p, q, layout.size().min(new_size));
                        self.dealloc(p, layout);
                    }
                }
                q
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex, MutexGuard};

    /// One test at a time: they share the pool and the page counter. Nothing
    /// else in this binary uses `Allocator`, whose global is the system's.
    fn serial() -> MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size, 8).unwrap()
    }

    /// Fills a block with a pattern naming its seed, so a block handed out
    /// twice or overwritten by a list link shows up as a mismatch.
    unsafe fn fill(p: *mut u8, size: usize, seed: usize) {
        for i in 0..size {
            unsafe { p.add(i).write(seed.wrapping_mul(31).wrapping_add(i) as u8) };
        }
    }

    unsafe fn check(p: *mut u8, size: usize, seed: usize) {
        for i in 0..size {
            assert_eq!(unsafe { p.add(i).read() }, seed.wrapping_mul(31).wrapping_add(i) as u8);
        }
    }

    struct Block(*mut u8, usize, usize);
    unsafe impl Send for Block {}

    /// Mostly small, sometimes past `MAX_SMALL`, and often right at a boundary.
    fn size(seed: usize) -> usize {
        let x = seed.wrapping_mul(2_654_435_761) % 1000;
        match x {
            0..=699 => 1 + x % 128,
            700..=899 => 1 + x * 7 % MAX_SMALL,
            900..=969 => SIZES[x % CLASSES] + x % 3 - 1,
            _ => MAX_SMALL + 1 + x * 13 % 8000,
        }
    }

    #[test]
    fn blocks_freed_on_other_threads_are_reused_intact() {
        let _one = serial();
        const THREADS: usize = 16;
        const PER_THREAD: usize = 60_000;
        // Each round's threads adopt the heaps the last round's left behind.
        for round in 0..4 {
            let (senders, receivers): (Vec<_>, Vec<_>) =
                (0..THREADS).map(|_| mpsc::channel::<Block>()).unzip();
            std::thread::scope(|s| {
                for (t, rx) in receivers.into_iter().enumerate() {
                    let senders = senders.clone();
                    s.spawn(move || {
                        let mut kept = Vec::new();
                        for i in 0..PER_THREAD {
                            let seed = round * 1_000_003 + t * PER_THREAD + i;
                            let n = size(seed);
                            let p = unsafe { Allocator.alloc(layout(n)) };
                            assert!(!p.is_null());
                            assert_eq!(p as usize % 8, 0);
                            unsafe { fill(p, n, seed) };
                            match seed % 4 {
                                // Freed by another thread.
                                0 | 1 => senders[(t + 1 + seed % (THREADS - 1)) % THREADS]
                                    .send(Block(p, n, seed))
                                    .unwrap(),
                                // Freed here, now.
                                2 => unsafe {
                                    check(p, n, seed);
                                    Allocator.dealloc(p, layout(n));
                                },
                                // Grown or shrunk, and freed here at the end.
                                _ => {
                                    let to = size(seed + 7);
                                    let q = unsafe { Allocator.realloc(p, layout(n), to) };
                                    assert!(!q.is_null());
                                    unsafe { check(q, n.min(to), seed) };
                                    unsafe { fill(q, to, seed + 1) };
                                    kept.push(Block(q, to, seed + 1));
                                }
                            }
                            while let Ok(Block(p, n, seed)) = rx.try_recv() {
                                unsafe { check(p, n, seed) };
                                unsafe { Allocator.dealloc(p, layout(n)) };
                            }
                        }
                        for Block(p, n, seed) in kept {
                            unsafe { check(p, n, seed) };
                            unsafe { Allocator.dealloc(p, layout(n)) };
                        }
                        drop(senders);
                        // The rest, once every thread is done sending.
                        for Block(p, n, seed) in rx {
                            unsafe { check(p, n, seed) };
                            unsafe { Allocator.dealloc(p, layout(n)) };
                        }
                    });
                }
                drop(senders);
            });
        }
    }

    #[test]
    fn a_spent_page_freed_into_from_another_thread_is_used_again() {
        let _one = serial();
        // Fill whole pages of one class on one thread, free them all on
        // another, then allocate as many again: no new page should be needed.
        const N: usize = 4 * PAGE / 48;
        std::thread::scope(|s| {
            s.spawn(|| {
                let blocks: Vec<usize> =
                    (0..N).map(|_| unsafe { Allocator.alloc(layout(48)) } as usize).collect();
                std::thread::scope(|s| {
                    s.spawn(|| {
                        for &p in &blocks {
                            unsafe { Allocator.dealloc(p as *mut u8, layout(48)) };
                        }
                    });
                });
                let before = NEXT_PAGE.load(Relaxed);
                let again: Vec<usize> =
                    (0..N).map(|_| unsafe { Allocator.alloc(layout(48)) } as usize).collect();
                let mut sorted = again.clone();
                sorted.sort_unstable();
                sorted.dedup();
                assert_eq!(sorted.len(), N);
                assert_eq!(NEXT_PAGE.load(Relaxed), before);
                for p in again {
                    unsafe { Allocator.dealloc(p as *mut u8, layout(48)) };
                }
            });
        });
    }

    #[test]
    fn a_page_emptied_by_remote_frees_serves_another_class() {
        let _one = serial();
        const N: usize = 16 * PAGE / 48;
        std::thread::scope(|s| {
            s.spawn(|| {
                let blocks: Vec<usize> =
                    (0..N).map(|_| unsafe { Allocator.alloc(layout(48)) } as usize).collect();
                std::thread::scope(|s| {
                    s.spawn(|| {
                        for &p in &blocks {
                            unsafe { Allocator.dealloc(p as *mut u8, layout(48)) };
                        }
                    });
                });
                // The first allocation to find its list empty drains the queue
                // and pools the emptied pages.
                let before = NEXT_PAGE.load(Relaxed);
                let other: Vec<usize> = (0..8 * PAGE / 1000)
                    .map(|_| unsafe { Allocator.alloc(layout(1000)) } as usize)
                    .collect();
                assert_eq!(NEXT_PAGE.load(Relaxed), before);
                for p in other {
                    unsafe { Allocator.dealloc(p as *mut u8, layout(1000)) };
                }
            });
        });
    }

    #[test]
    fn trimmed_pages_are_released_and_used_again() {
        let _one = serial();
        let n = (KEEP + 64) * PAGE / 1024;
        std::thread::scope(|s| {
            s.spawn(|| {
                let blocks: Vec<usize> =
                    (0..n).map(|_| unsafe { Allocator.alloc(layout(1000)) } as usize).collect();
                for &p in &blocks {
                    unsafe { Allocator.dealloc(p as *mut u8, layout(1000)) };
                }
                assert!(POOLED.load(Relaxed) > KEEP);
                let released = RELEASED_TOP.load(Relaxed);
                trim();
                assert_eq!(POOLED.load(Relaxed), KEEP);
                assert!(RELEASED_TOP.load(Relaxed) >= released + 32);
                // Every page comes back from the pool or the released stack.
                let before = NEXT_PAGE.load(Relaxed);
                let again: Vec<usize> =
                    (0..n).map(|_| unsafe { Allocator.alloc(layout(1000)) } as usize).collect();
                assert_eq!(NEXT_PAGE.load(Relaxed), before);
                for (i, &p) in again.iter().enumerate() {
                    unsafe { fill(p as *mut u8, 1000, i) };
                }
                for (i, &p) in again.iter().enumerate() {
                    unsafe { check(p as *mut u8, 1000, i) };
                    unsafe { Allocator.dealloc(p as *mut u8, layout(1000)) };
                }
            });
        });
    }

    #[test]
    fn every_size_fits_its_class_and_no_smaller_one() {
        for n in 1..=MAX_SMALL {
            let c = class(layout(n)).unwrap();
            assert!(SIZES[c] >= n && SIZES[c].is_multiple_of(STEP), "{n}");
            assert!(c == 0 || SIZES[c - 1] < n, "{n}");
        }
        assert_eq!(SIZES[CLASSES - 1], MAX_SMALL);
        assert_eq!(class(layout(MAX_SMALL + 1)), None);
    }

    #[test]
    fn zeroed_blocks_are_zero_after_reuse() {
        let _one = serial();
        for n in [1, 15, 16, 17, 100, MAX_SMALL, MAX_SMALL + 1] {
            let p = unsafe { Allocator.alloc(layout(n)) };
            unsafe { p.write_bytes(0xAB, n) };
            unsafe { Allocator.dealloc(p, layout(n)) };
            let q = unsafe { Allocator.alloc_zeroed(layout(n)) };
            assert!((0..n).all(|i| unsafe { q.add(i).read() } == 0));
            unsafe { Allocator.dealloc(q, layout(n)) };
        }
    }

    #[test]
    fn over_aligned_blocks_come_from_the_system_aligned() {
        let _one = serial();
        let l = Layout::from_size_align(24, 64).unwrap();
        let p = unsafe { Allocator.alloc(l) };
        assert_eq!(p as usize % 64, 0);
        unsafe { Allocator.dealloc(p, l) };
    }
}
