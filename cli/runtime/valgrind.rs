//! Valgrind client requests, so its tools see each block, each lock and each
//! hand-off between threads rather than a few big pages, a futex and an atomic
//! (`design/native/MEMORY.md` §8).
//!
//! Only with the `valgrind` feature. Without it [`memcheck`] and [`helgrind`]
//! are constant `false` and none of this is in the binary. With it, a program
//! asks once which tool runs it. Under memcheck or helgrind:
//!
//! - small blocks come from the system rather than from pages, `memory.rs`
//!   caches none, and the heap check holds no freed block back.
//!
//! Under memcheck, task stacks are registered as stacks, and on musl, whose
//! `malloc` memcheck can't replace in a static binary, the system blocks are
//! annotated with redzones, and a freed one waits behind 20 MB of later frees
//! before it's reused. Under helgrind each new
//! block is fresh memory, `sync.rs`'s locks are reported as locks, and the
//! runtime marks where it hands work from one thread to another.
//!
//! cachegrind and callgrind answer neither question, so the instruction gate
//! measures the allocator a program ships with.
//!
//! An in-process loader that maps code must also discard Valgrind's
//! translations of it (`VALGRIND_DISCARD_TRANSLATIONS`, request `0x1002`).

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
#[cfg(feature = "valgrind")]
const CHECK_MEM_IS_ADDRESSABLE: usize = 0x4d43_0004;
/// Helgrind's start at `'H' << 24 | 'G' << 16`. DRD answers the lock and
/// happens-before ones by the same numbers.
const HG_CLEAN_MEMORY: usize = 0x4847_0000;
const HG: usize = 0x4847_0100;
#[cfg(feature = "valgrind")]
const HG_RWLOCK_ACQUIRED: usize = HG + 17;
#[cfg(feature = "valgrind")]
const HG_RWLOCK_RELEASED: usize = HG + 18;
const HG_USERSO_SEND_PRE: usize = HG + 33;
const HG_USERSO_RECV_POST: usize = HG + 34;
const HG_ARANGE_MAKE_UNTRACKED: usize = HG + 39;
#[cfg(feature = "valgrind")]
const HG_GET_ABITS: usize = HG + 46;

/// Whether system blocks are handled here: only where the tool can't replace
/// `malloc`, which is a static musl binary.
pub(crate) const MUSL: bool = cfg!(all(feature = "valgrind", target_env = "musl", not(test)));

/// [`tool`]'s answers, kept in [`STATE`].
const UNDECIDED: u8 = 0;
const NONE: u8 = 1;
const MEMCHECK: u8 = 2;
const HELGRIND: u8 = 3;

static STATE: AtomicU8 = AtomicU8::new(UNDECIDED);

/// Which tool runs this process, asked once.
#[inline]
fn tool() -> u8 {
    #[cfg(not(feature = "valgrind"))]
    return NONE;
    #[cfg(feature = "valgrind")]
    match STATE.load(Relaxed) {
        UNDECIDED => decide(),
        known => known,
    }
}

/// Whether this process runs under memcheck.
#[inline]
pub fn memcheck() -> bool {
    tool() == MEMCHECK
}

/// Whether this process runs under helgrind.
#[inline]
pub fn helgrind() -> bool {
    tool() == HELGRIND
}

/// Whether a tool changes how this process allocates: pages, caches and the
/// heap check's quarantine all step aside. Not under `cfg(test)`, whose tests
/// are about those.
#[inline]
pub fn reroutes() -> bool {
    !cfg!(test) && matches!(tool(), MEMCHECK | HELGRIND)
}

/// Whether task stacks are registered: memcheck's definedness follows the
/// stack pointer. Helgrind's doesn't, and a thousand of them exhaust it.
#[inline]
pub fn registers_stacks() -> bool {
    memcheck()
}

/// Whether an existing block was annotated for memcheck: [`memcheck`] without
/// the call to ask, which the block's allocation already made.
#[inline(always)]
pub(crate) fn annotating() -> bool {
    MUSL && STATE.load(Relaxed) == MEMCHECK
}

/// Whether a block from the system is new memory to helgrind, likewise.
#[inline(always)]
pub(crate) fn freshening() -> bool {
    MUSL && STATE.load(Relaxed) == HELGRIND
}

// Every request is out of line: its arguments are an array on the stack,
// which would give the allocator's fast path a frame.
#[cfg(feature = "valgrind")]
#[cold]
#[inline(never)]
fn decide() -> u8 {
    let probe = 0u8;
    let at = std::ptr::addr_of!(probe).addr();
    // Each request is one only that tool answers.
    let tool = if request(1, [CHECK_MEM_IS_ADDRESSABLE, at, 1, 0, 0, 0]) == 0 {
        MEMCHECK
    } else if request(usize::MAX, [HG_GET_ABITS, at, 0, 1, 0, 0]) == 1 {
        HELGRIND
    } else {
        NONE
    };
    if tool == HELGRIND {
        // A latch every allocation reads with a plain load.
        untrack(std::ptr::from_ref(&STATE).addr(), 1);
    }
    STATE.store(tool, Relaxed);
    tool
}

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

/// Marks `[p, p + len)` as written, for a read memcheck can't follow: a task
/// stack's watermark, below where its stack pointer was.
#[cold]
#[inline(never)]
pub fn make_defined(p: *mut u8, len: usize) {
    mark(MAKE_MEM_DEFINED, p, len);
}

// ---------------------------------------------------------------------------
// Helgrind
// ---------------------------------------------------------------------------

/// Everything this thread did so far happens before what a thread does after
/// [`happens_after`] on the same `tag`.
#[inline]
pub fn happens_before<T: ?Sized>(tag: *const T) {
    if helgrind() {
        send(tag.cast::<u8>().addr());
    }
}

/// See [`happens_before`].
#[inline]
pub fn happens_after<T: ?Sized>(tag: *const T) {
    if helgrind() {
        receive(tag.cast::<u8>().addr());
    }
}

#[cold]
#[inline(never)]
fn send(tag: usize) {
    request(0, [HG_USERSO_SEND_PRE, tag, 0, 0, 0, 0]);
}

#[cold]
#[inline(never)]
fn receive(tag: usize) {
    request(0, [HG_USERSO_RECV_POST, tag, 0, 0, 0, 0]);
}

/// The lock at `lock` was just taken.
#[cfg(feature = "valgrind")]
#[cold]
#[inline(never)]
pub fn acquired(lock: usize) {
    request(0, [HG_RWLOCK_ACQUIRED, lock, 1, 0, 0, 0]);
}

/// The lock at `lock` is about to be let go.
#[cfg(feature = "valgrind")]
#[cold]
#[inline(never)]
pub fn released(lock: usize) {
    request(0, [HG_RWLOCK_RELEASED, lock, 1, 0, 0, 0]);
}

/// `at` is an atomic that a plain load or store reads or writes: helgrind
/// orders its read-modify-writes, but not those, so it isn't checked.
#[inline]
pub fn atomic<T>(at: &T) {
    if helgrind() {
        untrack(std::ptr::from_ref(at).cast::<u8>().addr(), size_of::<T>());
    }
}

#[cold]
#[inline(never)]
fn untrack(at: usize, len: usize) {
    request(0, [HG_ARANGE_MAKE_UNTRACKED, at, len, 0, 0, 0]);
}

/// `[p, p + len)` is new memory whose past accesses race with nothing.
#[cold]
#[inline(never)]
pub fn fresh(p: *mut u8, len: usize) {
    mark(HG_CLEAN_MEMORY, p, len);
}

// ---------------------------------------------------------------------------
// Memcheck's blocks on musl
// ---------------------------------------------------------------------------

/// Bytes on each side of an annotated block. An overrun lands in them.
const REDZONE: usize = 32;

/// Freed bytes held back from reuse: memcheck's own `--freelist-vol`.
const QUARANTINE: usize = 20 << 20;

fn redzone(align: usize) -> usize {
    REDZONE.max(align)
}

/// A system block for `chunk`, of which memcheck sees the first `size` bytes.
///
/// # Safety
/// `chunk` has a non-zero size.
#[cold]
#[inline(never)]
pub(crate) unsafe fn alloc(chunk: Layout, size: usize, zeroed: bool) -> *mut u8 {
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
pub(crate) fn resize(p: *mut u8, chunk: Layout, old: usize, new: usize) {
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
pub(crate) unsafe fn dealloc(p: *mut u8, chunk: Layout) {
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

/// Gives a block back to the system. Defined, as untracked memory is: musl's
/// `malloc` puts its own header inside a slot it hands out again.
unsafe fn release(base: *mut u8, link: Link) {
    mark(MAKE_MEM_DEFINED, base, link.size);
    // SAFETY: `base` came from `System` at this size and alignment.
    unsafe { System.dealloc(base, Layout::from_size_align_unchecked(link.size, link.align)) };
}

/// Tells the tool `[low, high)` is a stack, and answers its id.
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
