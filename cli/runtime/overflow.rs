//! Stack exhaustion, SPEC 6.9's third abort.
//!
//! A runaway recursion runs into a guard and the kernel raises `SIGSEGV` or
//! `SIGBUS`. [`on_fault`] runs on an alternate stack, because the thread's own
//! is full. When the fault was a stack's end it flushes what the program
//! printed, writes [`MESSAGE`] and exits 1, as every other abort does. Any
//! other fault is left to kill the process as before.
//!
//! A fault is a stack's end when it lands:
//!
//! - within a guard's width of the interrupted stack pointer, for every machine
//!   stack: the process's, a thread's, a task's;
//! - in a guard [`guarded`] was told about, for a thread's Buri data stack;
//! - in an image's zero-fill data, which holds the process's own Buri data
//!   stack: its pages are read-write except for the guard.
//!
//! Besides the flush, the handler calls only `write`, `sigaction`, `raise`,
//! `pause` and `_exit`. The flush never blocks on a lock: it reads the
//! published bytes when the faulting thread holds the lock, and waits at most
//! a second for another thread (`host::flush_in_handler`).
//!
//! The handler writes no `buri test` record: the runner names the block a
//! process died in from the blocks that returned, with what it wrote to stderr.

use crate::memory::BURI_RT_STACK_GUARD as GUARD;
use core::ffi::c_void as Void;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

/// What bun says, so a program says the same on every backend.
pub(crate) const MESSAGE: &[u8] = b"Maximum call stack size exceeded.";

const SIGSEGV: i32 = 11;
#[cfg(target_os = "macos")]
const SIGBUS: i32 = 10;
#[cfg(not(target_os = "macos"))]
const SIGBUS: i32 = 7;

#[cfg(target_os = "macos")]
const SA_ONSTACK: i32 = 0x1;
#[cfg(target_os = "macos")]
const SA_SIGINFO: i32 = 0x40;
#[cfg(not(target_os = "macos"))]
const SA_ONSTACK: i32 = 0x0800_0000;
#[cfg(not(target_os = "macos"))]
const SA_SIGINFO: i32 = 0x4;

#[cfg(target_os = "macos")]
const SS_DISABLE: i32 = 4;
#[cfg(not(target_os = "macos"))]
const SS_DISABLE: i32 = 2;

#[cfg(target_os = "macos")]
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: u32,
    flags: i32,
}

#[cfg(not(target_os = "macos"))]
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: i32,
    _restorer: usize,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct AltStackSpec {
    sp: *mut u8,
    size: usize,
    flags: i32,
}

#[cfg(not(target_os = "macos"))]
#[repr(C)]
struct AltStackSpec {
    sp: *mut u8,
    flags: i32,
    size: usize,
}

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

unsafe extern "C" {
    fn sigaction(sig: i32, act: *const u8, old: *mut u8) -> i32;
    fn sigaltstack(ss: *const AltStackSpec, old: *mut AltStackSpec) -> i32;
    fn mmap(addr: *mut Void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut Void;
    fn mprotect(addr: *mut Void, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut Void, len: usize) -> i32;
    fn getpagesize() -> i32;
    fn write(fd: i32, buf: *const Void, len: usize) -> isize;
    fn nanosleep(asked: *const Timespec, left: *mut Timespec) -> i32;
    fn raise(sig: i32) -> i32;
    fn pause() -> i32;
    fn _exit(code: i32) -> !;
}

const PROT_NONE: i32 = 0;
const PROT_READ_WRITE: i32 = 3;
#[cfg(target_os = "macos")]
const MAP_PRIVATE_ANON: i32 = 0x1002;
#[cfg(not(target_os = "macos"))]
const MAP_PRIVATE_ANON: i32 = 0x22;
const MAP_FAILED: usize = usize::MAX;

/// What the handler needs of its own stack, and of the guard under it.
const ALT_BYTES: usize = 64 * 1024;
const ALT_GUARD: usize = 16 * 1024;

/// The smallest stack the kernel delivers a signal on, which grows with a
/// machine's vector state.
#[cfg(target_os = "macos")]
fn kernel_minimum() -> usize {
    32 * 1024 // MINSIGSTKSZ
}

#[cfg(not(target_os = "macos"))]
fn kernel_minimum() -> usize {
    unsafe extern "C" {
        fn getauxval(kind: u64) -> u64;
    }
    const AT_MINSIGSTKSZ: u64 = 51;
    // SAFETY: reads the auxiliary vector; an absent entry is 0.
    unsafe { getauxval(AT_MINSIGSTKSZ) as usize }
}

/// An alternate stack's mapping: a guard at its low end, then the stack.
#[derive(Debug, PartialEq)]
struct AltLayout {
    guard: usize,
    usable: usize,
}

impl AltLayout {
    /// The layout on a host with `page`-byte pages, whose kernel needs at
    /// least `minimum` bytes to deliver a signal. Both parts are whole pages,
    /// so the guard never eats into the stack above it.
    fn new(page: usize, minimum: usize) -> AltLayout {
        let pages = |bytes: usize| bytes.div_ceil(page).wrapping_mul(page);
        AltLayout {
            guard: pages(ALT_GUARD),
            usable: pages(ALT_BYTES.max(minimum)),
        }
    }

    /// The layout on this host.
    fn here() -> AltLayout {
        // SAFETY: no preconditions.
        let page = unsafe { getpagesize() } as usize;
        AltLayout::new(page, kernel_minimum())
    }

    fn len(&self) -> usize {
        self.guard.wrapping_add(self.usable)
    }
}

/// Install the handler, once, and give the calling thread its alternate stack.
/// `buri_rt_argv_init` calls this on the process's own thread.
pub(crate) fn arm() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if !INSTALLED.swap(true, Ordering::AcqRel) {
        zerofill_of_this_executable();
        for sig in [SIGSEGV, SIGBUS] {
            // SAFETY: a zeroed `struct sigaction` is an empty mask and no flags.
            let mut act: SigAction = unsafe { std::mem::zeroed() };
            act.handler = on_fault as *const () as usize;
            act.flags = SA_SIGINFO | SA_ONSTACK;
            // SAFETY: a valid action for a signal that has one.
            unsafe { sigaction(sig, (&raw const act).cast(), std::ptr::null_mut()) };
        }
    }
    arm_thread();
}

/// The calling thread's alternate stack, mapped on first use and given back
/// when the thread ends.
struct AltStack(Cell<*mut u8>);

impl Drop for AltStack {
    fn drop(&mut self) {
        let mapped = self.0.get();
        if mapped.is_null() {
            return;
        }
        let off = AltStackSpec {
            sp: std::ptr::null_mut(),
            size: 0,
            flags: SS_DISABLE,
        };
        // SAFETY: the thread is ending, so it is not on this stack.
        unsafe {
            sigaltstack(&raw const off, std::ptr::null_mut());
            munmap(mapped.cast(), AltLayout::here().len());
        }
    }
}

thread_local! {
    static ALT: AltStack = const { AltStack(Cell::new(std::ptr::null_mut())) };
}

/// Give the calling thread an alternate stack, so a fault on its full stack
/// still has somewhere to run. Every thread that runs Buri code calls this.
pub(crate) fn arm_thread() {
    let _ = ALT.try_with(|alt| {
        if !alt.0.get().is_null() {
            return;
        }
        let layout = AltLayout::here();
        let len = layout.len();
        // SAFETY: a fresh anonymous mapping, then a guard at its low end and the
        // rest handed to the kernel as this thread's alternate stack.
        unsafe {
            let p: *mut u8 = mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ_WRITE,
                MAP_PRIVATE_ANON,
                -1,
                0,
            )
            .cast();
            if p.is_null() || p as usize == MAP_FAILED {
                return;
            }
            mprotect(p.cast(), layout.guard, PROT_NONE);
            let on = AltStackSpec {
                sp: p.wrapping_add(layout.guard),
                size: layout.usable,
                flags: 0,
            };
            sigaltstack(&raw const on, std::ptr::null_mut());
            alt.0.set(p);
        }
    });
}

// ---------------------------------------------------------------------------
// The guards a fault is compared against
// ---------------------------------------------------------------------------

/// The low ends of the data-stack guards, each [`GUARD`] wide. Zero is a free
/// slot. A full chunk links a mapped one, so the handler reads without a lock.
struct Chunk {
    slots: [AtomicUsize; SLOTS],
    next: AtomicPtr<Chunk>,
}

const SLOTS: usize = 511;

static FIRST: Chunk = Chunk {
    slots: [const { AtomicUsize::new(0) }; SLOTS],
    next: AtomicPtr::new(std::ptr::null_mut()),
};

/// Each chunk, first to last.
fn chunks(mut f: impl FnMut(&Chunk) -> bool) {
    let mut at: *const Chunk = &raw const FIRST;
    while !at.is_null() {
        // SAFETY: `FIRST`, or a chunk `guarded` mapped and never unmaps.
        let chunk = unsafe { &*at };
        if f(chunk) {
            return;
        }
        at = chunk.next.load(Ordering::Acquire);
    }
}

/// A [`GUARD`]-wide guard now starts at `low`.
pub(crate) fn guarded(low: usize) {
    let mut placed = false;
    let mut last: *const Chunk = &raw const FIRST;
    chunks(|chunk| {
        last = chunk;
        placed = chunk.slots.iter().any(|s| {
            s.compare_exchange(0, low, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        });
        placed
    });
    if placed {
        return;
    }
    // Every slot is taken: a zero-filled mapping is an empty chunk.
    let len = std::mem::size_of::<Chunk>();
    // SAFETY: a fresh anonymous mapping.
    let fresh = unsafe {
        mmap(
            std::ptr::null_mut(),
            len,
            PROT_READ_WRITE,
            MAP_PRIVATE_ANON,
            -1,
            0,
        )
    };
    if fresh.is_null() || fresh as usize == MAP_FAILED {
        return;
    }
    let fresh = fresh.cast::<Chunk>();
    // SAFETY: the empty chunk just mapped, which nobody else can see yet.
    unsafe { (*fresh).slots[0].store(low, Ordering::Relaxed) };
    let mut tail = last;
    loop {
        // SAFETY: a chunk in the list, which is never unmapped.
        let next = unsafe { &(*tail).next };
        match next.compare_exchange(
            std::ptr::null_mut(),
            fresh,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return,
            Err(other) => tail = other,
        }
    }
}

/// The guard starting at `low` is gone.
pub(crate) fn unguarded(low: usize) {
    chunks(|chunk| {
        chunk.slots.iter().any(|s| {
            s.compare_exchange(low, 0, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        })
    });
}

fn in_a_guard(addr: usize) -> bool {
    let mut found = false;
    chunks(|chunk| {
        found = chunk.slots.iter().any(|s| {
            let low = s.load(Ordering::Acquire);
            low != 0 && addr.wrapping_sub(low) < GUARD
        });
        found
    });
    found
}

/// The zero-fill data of each image: this executable, and the image the test
/// host loaded. `[low, high)`, zero where there is none.
static ZEROFILL: [(AtomicUsize, AtomicUsize); 2] = [
    (AtomicUsize::new(0), AtomicUsize::new(0)),
    (AtomicUsize::new(0), AtomicUsize::new(0)),
];

/// An image's zero-fill data now spans `[low, high)`.
pub(crate) fn zerofill(low: usize, high: usize) {
    for (lo, hi) in &ZEROFILL {
        if lo
            .compare_exchange(0, low, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            hi.store(high, Ordering::Release);
            return;
        }
    }
}

fn in_zerofill(addr: usize) -> bool {
    ZEROFILL.iter().any(|(lo, hi)| {
        let lo = lo.load(Ordering::Acquire);
        lo != 0 && addr >= lo && addr < hi.load(Ordering::Acquire)
    })
}

#[cfg(target_os = "macos")]
fn zerofill_of_this_executable() {
    unsafe extern "C" {
        fn _dyld_get_image_header(index: u32) -> *const u8;
        fn getsectiondata(
            header: *const u8,
            segment: *const std::ffi::c_char,
            section: *const std::ffi::c_char,
            size: *mut std::ffi::c_ulong,
        ) -> *mut u8;
    }
    let mut size: std::ffi::c_ulong = 0;
    // SAFETY: image 0 is this executable, and both names are C strings.
    let low = unsafe {
        getsectiondata(
            _dyld_get_image_header(0),
            c"__DATA".as_ptr(),
            c"__bss".as_ptr(),
            &raw mut size,
        )
    } as usize;
    if low != 0 {
        zerofill(low, low.wrapping_add(size as usize));
    }
}

#[cfg(not(target_os = "macos"))]
fn zerofill_of_this_executable() {
    #[repr(C)]
    struct Info {
        addr: usize,
        _name: *const u8,
        phdr: *const Phdr,
        phnum: u16,
    }
    #[repr(C)]
    struct Phdr {
        kind: u32,
        flags: u32,
        _offset: u64,
        vaddr: u64,
        _paddr: u64,
        filesz: u64,
        memsz: u64,
        _align: u64,
    }
    unsafe extern "C" {
        fn dl_iterate_phdr(
            f: unsafe extern "C" fn(*const Info, usize, *mut u8) -> i32,
            data: *mut u8,
        ) -> i32;
    }
    const PT_LOAD: u32 = 1;
    const PF_W: u32 = 2;
    /// The first object is the executable: its writable segment's tail past
    /// the file's bytes is the zero-fill.
    unsafe extern "C" fn first(info: *const Info, _size: usize, _data: *mut u8) -> i32 {
        // SAFETY: the C library hands a valid `dl_phdr_info` and its headers.
        let info = unsafe { &*info };
        // SAFETY: as above.
        let headers = unsafe { std::slice::from_raw_parts(info.phdr, usize::from(info.phnum)) };
        for h in headers {
            if h.kind == PT_LOAD && h.flags & PF_W != 0 && h.memsz > h.filesz {
                let start = info.addr.wrapping_add(h.vaddr as usize);
                zerofill(
                    start.wrapping_add(h.filesz as usize),
                    start.wrapping_add(h.memsz as usize),
                );
            }
        }
        1
    }
    // SAFETY: `first` reads only what it is handed.
    unsafe { dl_iterate_phdr(first, std::ptr::null_mut()) };
}

// ---------------------------------------------------------------------------
// The handler
// ---------------------------------------------------------------------------

/// Where the fault was, from the `siginfo_t`.
///
/// # Safety
/// `info` is the `siginfo_t` a handler installed with `SA_SIGINFO` is given.
unsafe fn fault_address(info: *const u8) -> usize {
    #[cfg(target_os = "macos")]
    const AT: usize = 24;
    #[cfg(not(target_os = "macos"))]
    const AT: usize = 16;
    // SAFETY: `si_addr` is at `AT` on this platform.
    unsafe { info.wrapping_add(AT).cast::<usize>().read_unaligned() }
}

/// The stack pointer the fault interrupted, from the `ucontext_t`.
///
/// # Safety
/// `context` is the `ucontext_t` a handler installed with `SA_SIGINFO` is given.
unsafe fn interrupted_sp(context: *const u8) -> usize {
    // SAFETY: each arm reads the field its platform's headers put there.
    unsafe {
        #[cfg(target_os = "macos")]
        {
            // `uc_mcontext` is a pointer; `__ss.__sp` follows `__es` and x0-x30.
            let machine = context
                .wrapping_add(48)
                .cast::<*const u8>()
                .read_unaligned();
            machine.wrapping_add(264).cast::<usize>().read_unaligned()
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            // `uc_mcontext.gregs[REG_RSP]`.
            context.wrapping_add(160).cast::<usize>().read_unaligned()
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            // `uc_mcontext.sp`, after `fault_address` and x0-x30.
            context.wrapping_add(432).cast::<usize>().read_unaligned()
        }
    }
}

/// Whether a fault at `addr`, with the stack pointer at `sp`, ran off a stack.
fn ran_off_a_stack(addr: usize, sp: usize) -> bool {
    addr.abs_diff(sp) < GUARD || in_a_guard(addr) || in_zerofill(addr)
}

extern "C" fn on_fault(sig: i32, info: *mut u8, context: *mut u8) {
    // SAFETY: the kernel hands this handler, installed with `SA_SIGINFO`, both.
    let (addr, sp) = unsafe { (fault_address(info), interrupted_sp(context)) };
    if ran_off_a_stack(addr, sp) {
        exhausted();
    }
    // Some other fault: the default action, as if this handler weren't here.
    // SAFETY: a zeroed action is `SIG_DFL`; the signal is blocked until this
    // handler returns, and then ends the process.
    unsafe {
        let default: SigAction = std::mem::zeroed();
        sigaction(sig, (&raw const default).cast(), std::ptr::null_mut());
        raise(sig);
    }
}

/// The abort, from inside the handler.
fn exhausted() -> ! {
    static REPORTING: AtomicBool = AtomicBool::new(false);
    if REPORTING.swap(true, Ordering::AcqRel) {
        // Another thread ran out first, and is ending the process.
        loop {
            // SAFETY: waits for the exit.
            unsafe { pause() };
        }
    }
    crate::memory::quiet_heap_audit();
    crate::host::flush_in_handler();
    write_all(2, MESSAGE);
    write_all(2, b"\n");
    // SAFETY: ends the process without running anything else.
    unsafe { _exit(1) }
}

/// `bytes` to `fd`, as far as `write` takes them.
pub(crate) fn write_all(fd: i32, bytes: &[u8]) {
    let mut left = bytes;
    while !left.is_empty() {
        // SAFETY: `left` is readable for its length.
        let n = unsafe { write(fd, left.as_ptr().cast(), left.len()) };
        let Ok(n) = usize::try_from(n) else { return };
        if n == 0 {
            return;
        }
        left = left.get(n..).unwrap_or_default();
    }
}

/// The calling thread, as `pthread_self` names it.
pub(crate) fn thread_id() -> usize {
    unsafe extern "C" {
        fn pthread_self() -> usize;
    }
    // SAFETY: always valid to call.
    unsafe { pthread_self() }
}

/// Sleep a millisecond.
pub(crate) fn nap() {
    let asked = Timespec {
        sec: 0,
        nsec: 1_000_000,
    };
    // SAFETY: a valid request and no remainder wanted.
    unsafe { nanosleep(&raw const asked, std::ptr::null_mut()) };
}

#[cfg(test)]
mod tests {
    use super::AltLayout;

    const K: usize = 1024;

    #[test]
    fn small_pages_keep_the_sizes_they_had() {
        for page in [4 * K, 16 * K] {
            let want = AltLayout {
                guard: 16 * K,
                usable: 64 * K,
            };
            assert_eq!(AltLayout::new(page, 32 * K), want);
        }
    }

    #[test]
    fn a_large_page_widens_the_guard_not_the_stack_over_it() {
        let want = AltLayout {
            guard: 64 * K,
            usable: 64 * K,
        };
        assert_eq!(AltLayout::new(64 * K, 5 * K), want);
    }

    #[test]
    fn every_part_is_whole_pages_and_the_stack_never_shrinks() {
        for page in [4 * K, 16 * K, 64 * K] {
            for minimum in [0, 2 * K, 5 * K, 32 * K, 64 * K + 1, 200 * K] {
                let layout = AltLayout::new(page, minimum);
                assert_eq!(layout.guard % page, 0);
                assert!(layout.guard >= 16 * K);
                assert_eq!(layout.usable % page, 0);
                assert!(layout.usable >= 64 * K);
                assert!(layout.usable >= minimum);
            }
        }
    }

    #[test]
    fn this_host_has_whole_pages() {
        let layout = AltLayout::here();
        // SAFETY: no preconditions.
        let page = unsafe { super::getpagesize() } as usize;
        assert_eq!(layout.guard % page, 0);
        assert_eq!(layout.usable % page, 0);
        assert!(layout.usable >= super::kernel_minimum());
    }
}
