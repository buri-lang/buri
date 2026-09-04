//! Aborts: the messages, and the exit status.
//!
//! SPEC 6.9 — an abort is a write to standard error and an exit, never an
//! unwind. There is nothing to unwind: the language has no exceptions, which is
//! also why the native backends emit no `.eh_frame` (CODEGEN-STENCIL.md §11).
//!
//! ## Parity with JavaScript
//!
//! VALUE-MODEL.md §12 row 14 says the abort message and the exit status must
//! agree between backends, and `cli/tests/crash/` is where that is pinned: each
//! file's `// CRASH:` line is a substring the process's standard error must
//! contain. Three messages are pinned today and all three are byte-identical
//! here to `runtime.js`:
//!
//! | Message | JavaScript | Corpus |
//! |---|---|---|
//! | `division by zero` | `runtime.js:45` | `{int,i8,u8}_*_by_zero`, four files |
//! | `shift out of range` | `runtime.js:925` | `shift_*`, six files |
//! | `random range is empty` | `runtime.js:1418` | `random_range_empty`, `random_range_inverted` |
//!
//! The others below are *not* pinned by the corpus, because the JavaScript
//! backend has no counterpart: a list index out of bounds is unrepresentable
//! there (`list.get` answers `Option`), and an unreachable default arm exists
//! only under `Profile::defensive_aborts`. Their wording is settled here so
//! that it is settled in one place when it does become observable.
//!
//! The exit status is 1, and the message is followed by a single newline, which
//! is what `generate.rs:336` writes on the JavaScript side.
//!
//! ## Inside a test binary
//!
//! An abort is also how a `test` block fails, and it is *every* way one can
//! fail: a failed assertion, a division by zero, an allocation past a budget.
//! So [`die`] is the one place that can attribute a failure to the block it
//! happened in, and it calls [`crate::testing::note_failure`] to do it — one
//! line naming the block, the message, and the values where the assertion had
//! them, which is what `buri test` turns into the same report a JavaScript run
//! prints. Outside a test binary nothing is driving the process and that call
//! is a load and a branch.
//!
//! ## What is missing
//!
//! CODEGEN-STENCIL.md §11 wants an abort to print a stack trace by walking the
//! frame-pointer chain and resolving each return address against a
//! `.buri_symbols` section. No native backend emits that section yet — §9 of
//! that document lists it as a gap — so the walk cannot be written before the
//! thing it reads exists. It is
//! additive when it arrives: it hangs off [`buri_rt_abort`] without changing
//! the message or the status, so nothing pinned above moves.

use crate::host::buri_rt_flush;
use std::io::Write;

/// Write `msg` and a newline to standard error, then exit 1. Never returns.
///
/// Buffered standard output is flushed first, so that the last thing a program
/// printed is on the terminal above the reason it stopped — the ordering
/// `$host.flush()` gives on JavaScript (`generate.rs:337`).
///
/// # Safety
/// `msg` must point at `len` readable bytes, or be null with `len == 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_abort(msg: *const u8, len: u64) -> ! {
    let text = if msg.is_null() {
        &[][..]
    } else {
        // SAFETY: the caller promises `len` readable bytes at `msg`.
        unsafe { std::slice::from_raw_parts(msg, len as usize) }
    };
    die(&[text])
}

/// The whole of an abort that is not the message: flush, write, exit.
///
/// One place, which is what lets a native test binary attribute *every* way a
/// `test` block can end other than by returning — a failed assertion, a
/// division by zero, an allocation past a budget — to the block it happened in
/// ([`crate::testing::note_failure`]). Outside a test binary that call is a
/// load and a branch: nothing is driving the process, so there is no record to
/// write.
pub(crate) fn die(parts: &[&[u8]]) -> ! {
    buri_rt_flush();
    crate::testing::note_failure(parts);
    let err = std::io::stderr();
    let mut err = err.lock();
    for part in parts {
        let _ = err.write_all(part);
    }
    let _ = err.write_all(b"\n");
    let _ = err.flush();
    std::process::exit(1)
}

/// Decimal digits of `n`, into a caller-owned buffer, no allocation.
///
/// An abort must work when the reason for it is that allocation failed, so the
/// message assembly cannot allocate. Twenty digits holds `u64::MAX`.
struct Digits {
    buf: [u8; 20],
    at: usize,
}

impl Digits {
    fn of(mut n: u64) -> Digits {
        let mut d = Digits { buf: [b'0'; 20], at: 20 };
        loop {
            d.at -= 1;
            d.buf[d.at] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 || d.at == 0 {
                break;
            }
        }
        d
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[self.at..]
    }
}

/// Signed decimal, as two parts: an optional `-` and the magnitude.
fn signed(n: i64) -> (&'static [u8], Digits) {
    if n < 0 {
        (b"-", Digits::of(n.unsigned_abs()))
    } else {
        (b"", Digits::of(n as u64))
    }
}

/// `division by zero` — SPEC 6.2, and `$divz` (`runtime.js:44-47`).
///
/// Overflow and underflow are undefined and go unchecked; division by zero
/// still aborts, because there is no answer to give.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_abort_div_zero() -> ! {
    die(&[b"division by zero"])
}

/// `shift out of range` — `runtime.js:925`.
///
/// A shift count that is negative, or at or beyond the operand's width.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_abort_shift() -> ! {
    die(&[b"shift out of range"])
}

/// `random range is empty` — `runtime.js:1418`, for `hi <= lo`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_abort_random_range() -> ! {
    die(&[b"random range is empty"])
}

/// An index outside `0 ..< len`.
///
/// Not pinned by the crash corpus: `core/list`'s indexing answers `Option`, so
/// a Buri program cannot reach this through the standard library. It is here
/// for the places a *backend* needs it — a slice whose bounds the middle end
/// could not prove, and `Profile::defensive_aborts`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_abort_bounds(index: i64, len: i64) -> ! {
    let (sign, idx) = signed(index);
    let (lsign, l) = signed(len);
    die(&[
        b"index out of bounds: the length is ",
        lsign,
        l.as_bytes(),
        b" but the index is ",
        sign,
        idx.as_bytes(),
    ])
}

/// An arm `exhaustiveness.rs` proved unreachable, reached.
///
/// Emitted only under `Profile::defensive_aborts` (on in debug, off in
/// release), which is the backend's own belt to the checker's braces. Reaching
/// it is a compiler bug and the message says so, because a user cannot act on
/// it.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_abort_unreachable() -> ! {
    die(&[b"internal compiler error: a case the checker proved unreachable was reached"])
}

/// A failed assertion whose values could not be rendered.
///
/// The message is the same sentence `$testing_assert_report` throws, and it is
/// the whole of the report where the two values are missing — which happens
/// only where `middle::derives` declined to generate a `Show` at the type, an
/// opaque one. Every other failed assertion arrives through
/// [`crate::testing::buri_rt_test_fail_compared`] with both values already
/// rendered, and `die`'s record carries them to `buri test`.
///
/// # Safety
/// `kind` must point at `len` readable bytes, or be null with `len == 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_abort_assert(kind: *const u8, len: u64) -> ! {
    let text = if kind.is_null() {
        &[][..]
    } else {
        // SAFETY: the caller promises `len` readable bytes at `kind`.
        unsafe { std::slice::from_raw_parts(kind, len as usize) }
    };
    die(&[b"assert.", text, b" failed"])
}

/// A `FixedBuffer(n)` budget exceeded. MEMORY.md §7.2.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_abort_alloc_budget(requested: i64, budget: i64) -> ! {
    let (rsign, r) = signed(requested);
    let (bsign, b) = signed(budget);
    die(&[
        b"allocation budget exhausted: ",
        rsign,
        r.as_bytes(),
        b" bytes requested against a budget of ",
        bsign,
        b.as_bytes(),
    ])
}

/// The allocator could not satisfy a request.
///
/// SPEC 10.5 says `Alloc` can fail; `effect.buri:19` gives `allocate` no value to
/// report a failure with; SPEC 6.9 says that combination is an abort.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_abort_oom(bytes: u64) -> ! {
    let n = Digits::of(bytes);
    die(&[b"out of memory: could not allocate ", n.as_bytes(), b" bytes"])
}
