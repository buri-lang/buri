//! The source behind `Entropy` — the operating system's, and nothing else.
//!
//! `rng.rs` is the other generator in this archive and the two are deliberately
//! not related. That one is xoshiro256++ over a seed read once, with a
//! clock-derived fallback for a sandbox that has no `/dev`, because `Rand`
//! promises a *distribution* and a program that cannot start is worse than a
//! weak shuffle. This one promises that nobody can guess the next octet, and
//! every one of those choices inverts: the source is the kernel's on every
//! call, there is no seed to hold, there is no fallback, and a failure aborts.
//!
//! **`getrandom` rather than twenty lines here**, which is the one dependency
//! decision this file embodies; `manifest.toml`'s entry for the crate is where
//! it is argued in full. The short version is that `getrandom(2)`,
//! `getentropy(2)`, the vDSO path, the pre-3.17 fallback, `EINTR` and a short
//! return are a per-platform interface with a decade of errata in it, and
//! getting one of them wrong produces octets that look exactly like the right
//! ones.
//!
//! Behind the `crypto` feature in full: without it this file is not compiled,
//! `buri_rt_host_entropy_bytes` is not in the archive, and a program that
//! reaches `Entropy` is refused by `backend::cryptography_gap` naming the
//! operation — rather than linked against a generator that would answer.

use crate::memory::buri_rt_alloc;
use crate::value::BuriList;

/// `Entropy.bytes` — `count` octets from the platform's generator.
///
/// The octets are written **into the Buri block itself** rather than into a
/// scratch buffer that is then copied. That is not an optimisation: a copy
/// would leave a second image of a key in a freed allocation, and the block
/// this returns is the one the program will hold.
///
/// A negative count and a generator that will not answer both abort, and
/// neither is a `Result`. `core/effect`'s `Entropy` states why: the only other
/// thing this could return is fewer octets than were asked for, and that is the
/// one answer that must never reach a caller.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_entropy_bytes(count: i64, out: *mut BuriList) {
    if count < 0 {
        crate::buri_rt_abort_entropy_count();
    }
    let len = count as usize;
    if len == 0 {
        // SAFETY: the caller promises a writable, aligned destination.
        unsafe { out.write(BuriList { ptr: std::ptr::null_mut(), len: 0 }) };
        return;
    }
    let ptr = buri_rt_alloc(len as u64);
    // SAFETY: `buri_rt_alloc` answered a block of `len` payload bytes, and
    // nothing else holds it yet.
    let block = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
    if getrandom::fill(block).is_err() {
        crate::buri_rt_abort_entropy_unavailable();
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriList { ptr, len: len as u64 }) }
}
