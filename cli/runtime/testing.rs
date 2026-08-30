//! `core/testing/context` and `core/host/testing` — the test runner's
//! platform, natively — and the runner's own protocol, at the bottom of this
//! file.
//!
//! The two modules are one handle table and two vocabularies.
//! `core/testing/context` — free constructors, `Hermetic()` — comes first and
//! is unchanged. `core/host/testing`, further down, is `core/host`'s names
//! *called* rather than referred to, with configuration as a builder that
//! answers a new handle; its own section states what is different.
//!
//! Two halves of one subject, and the second is the shorter: a native test
//! binary tells `buri test` which block it was in when it aborted, and with
//! what, so that the report a failing suite prints is the same report whichever
//! backend ran it. "The runner's side" below is that side.
//!
//! `testing_context.buri`'s header says why every implementation there carries
//! an `I64` handle rather than its state: a captured stdout and a seeded
//! generator accumulate what a test does to them, Buri has no mutation, and so
//! the state lives on the runner's side and the value in the program names it.
//! On JavaScript "the runner's side" is `runtime.js`'s `$t.h` array
//! (`$handle`/`$slot`); here it is [`TABLE`], and the two are the same design
//! written twice because the *program* is what has to behave identically.
//!
//! ## Why this is in the archive at all
//!
//! `lib.rs` §0 divides the intrinsic surface into what a generated program
//! cannot do for itself and what it can. A handle table is squarely the first:
//! it is mutable process state with a lifetime longer than any expression, and
//! open-coding it in two backends would be two allocators for one array. The
//! `Alloc` counters (`memory.rs`'s `buri_rt_alloc_new_counter`) are the same
//! shape and are already here for the same reason.
//!
//! `alloc()` and `TestAlloc::allocate` are the exception and are deliberately
//! **not** here: `$testing_context_TestAlloc_allocate` answers the byte count
//! it was handed and reads no state at all, so both backends open-code it and
//! the handle names nothing.
//!
//! ## `MemFs`'s eleven methods, and the one divergence they make readable
//!
//! Most answer a `Result<T, IoError>`, which was the shape neither native
//! backend had a `Ret` for and the reason the original four were held back. It
//! has one now — `lib.rs` §2.1 — and all eleven are here.
//!
//! The slot holds **octets**, not text, so `writeFileBytes` reads back through
//! `readFileBytes` unchanged; and it holds the directories `makeDir` was asked
//! for separately, because a flat map has no empty directory otherwise. Both
//! are `$t.h`'s shape written for a language that has statics, and `runtime.js`
//! stores exactly the same two things.
//!
//! `fileExists` was never the hard one; it was held back *with* the other three
//! on purpose, and the reason it was held back is still true and is now a
//! stated divergence instead of an absence. `data()` is "rooted at the package
//! directory, containing exactly `test { data: [...] }`", and a compiled test
//! binary has no runner to be handed those entries by, so `data()` here is
//! **empty**. On a package that declares no `data:`, that is not a divergence
//! at all — it is the specified answer, and
//! `conformance/lib/semantics/test/effects.buri` asserts it in those words. On
//! a package that declares one, a native test binary reads `.Err(.NotFound)`
//! where `buri test` reads the file.
//!
//! That was worth trading a gap for, and the trade is worth stating rather than
//! assuming: the alternative kept **183 conformance test blocks** out of the
//! native set — every one of them about effects and evaluation rather than
//! about files — to protect a case the corpus does not contain and that a
//! reader of `buri_rt_testing_context_data` is told about. Baking the runner's
//! `data:` entries into the binary is the real fix and it is a build-system
//! change, not a runtime one: nothing in this file can see a `BUILD.buri`.
//!
//! ## Ownership
//!
//! `lib.rs` §3, unchanged: every parameter is borrowed and every result is a
//! fresh block with `rc == 1`. A slot **copies** the text it is given rather
//! than keeping the caller's pointer, which is the same sentence as "a runtime
//! function never stores a pointer it was passed".

use crate::value::{list_of_bytes, list_of_strs, str_of, BuriList, BuriStr};
use crate::BURI_OK;
use std::sync::Mutex;

/// One live handle's state.
///
/// One table rather than one per implementation, as `$t.h` is one array: a
/// handle is a position in it, and no two implementations ever read each
/// other's, because the Buri type of the value carrying the handle says which
/// one made it.
enum Slot {
    /// `CaptureOut` and `CaptureErr` — the transcript, as text.
    Text(String),
    /// `TestStdin` — the lines, how many have been read, the octets where it
    /// was built by `stdinBytes` rather than by `stdin`, and the reads made
    /// through this handle.
    ///
    /// `calls` is `core/host/testing`'s and stays empty for a
    /// `core/testing/context` stdin, which has no `calls()` to read it with.
    /// One field costs a `Vec` that is never allocated until something is
    /// pushed to it; a second variant would cost every `readLine` a second arm
    /// that answered the same thing.
    Stdin { lines: Vec<String>, at: usize, bytes: Option<Vec<u8>>, calls: Vec<StdinLog> },
    /// `MemFs` — the files, in insertion order, and the directories `makeDir`
    /// has been asked for.
    ///
    /// A `Vec` of pairs rather than a map, because the JavaScript side is an
    /// object and `readDir` reads its keys: a map would be a second ordering to
    /// reconcile, and there is no fixture large enough for the lookup cost to
    /// be the interesting number.
    ///
    /// **Octets**, not text: `writeFileBytes` must read back through
    /// `readFileBytes` unchanged, and a `String` cannot hold what is not UTF-8.
    /// `readFile` decodes lossily on the way out, which is what a real
    /// filesystem's does. The directories are separate because a flat map has
    /// no empty one otherwise, and `readDir` after `makeDir` has to see one.
    Files { entries: Vec<(String, Vec<u8>)>, dirs: Vec<String> },
    /// `TestClock` — the current instant, in milliseconds.
    Clock(i64),
    /// `TestRand` — the xorshift32 state.
    Rand(u32),
    /// `TestEnv`.
    Env { vars: Vec<(String, String)>, args: Vec<String> },
    /// `core/host/testing`'s `TestFs` — a **view**: the handle of the
    /// [`Slot::Files`] store its files live in, and whether writes through this
    /// view are refused.
    ///
    /// The one shape that names another slot, and it is what folds
    /// `core/testing/context`'s `ReadOnly<C>` wrapper into a method. The
    /// wrapper holds the inner value, so a read through it sees whatever that
    /// filesystem holds now; `readOnly` here installs a second view onto the
    /// same store and keeps that property, where a flag inside `Slot::Files`
    /// would have forced either a copy or a builder that edited its receiver.
    ///
    /// `store` is always an index below the view's own, because the store is
    /// installed first — which is what makes reading a view one lookup rather
    /// than a walk.
    ///
    /// `plan` names the [`Slot::Plan`] this view fails through, or `-1` where
    /// nothing has called `faults`. It travels with the view exactly as
    /// `read_only` does — a builder is configuration and not a write — so the
    /// filesystem `fs().faults(p).files(x)` answers fails what `p` names.
    Fs { store: i64, read_only: bool, plan: i64, calls: Vec<FsLog> },
    /// `core/host/testing`'s `TestNet` — its log and the plan it fails through,
    /// and nothing else.
    ///
    /// The one slot that holds no state the double *reads*: a `TestNet` carries
    /// its responder as a value, because behaviour is what a runner cannot hold
    /// (`host_testing.buri`'s own documentation), and a log is state, which is
    /// what a runner is for. So the handle names the log and the responder
    /// travels in the program — and so does the plan, for the same reason read
    /// once more: a `NetError` carries a `Str` on two of its variants.
    Net { calls: Vec<NetLog>, plan: i64 },
    /// A fault plan's **promise**: what each of its entries reads like, and
    /// whether anything has fired it.
    ///
    /// The plan itself is a Buri value and stays in the program — an `IoError`
    /// is a value `lib.rs` §2.1 cannot hand back across a row, so matching is
    /// the `Eq` the `Call` records derive and happens there. What is here is the
    /// half a program cannot keep: a `test` block has returned by the time
    /// anyone could ask whether every fault it planned was used, so
    /// [`buri_rt_test_leave`] asks on its behalf.
    ///
    /// `retired` is set when `faults` is called a second time on a chain: that
    /// call replaces the plan, and a promise nothing can keep any more is not
    /// one to report.
    Plan { entries: Vec<PlanEntry>, retired: bool },
    /// `core/host/testing`'s `TestProc` — the code the first `exitWith` asked
    /// for, and `None` where nothing exited.
    ///
    /// The one shape `core/testing/context` has no counterpart for, because it
    /// has no `Proc` double at all. Everything else `core/host/testing` needs is
    /// one of the variants above: a captured stream is a transcript whichever
    /// module minted it, and one table with one shape per *state* beats one
    /// table with one shape per module.
    /// `core/host/testing`'s `TestTasks` — the order it schedules its tasks
    /// in, the plan it fails them through, and the tasks that have completed.
    ///
    /// `mode` is program order, one seeded order, or every order; `seed` is the
    /// order's own number, or `-1` for the mode's default. Both are numbers
    /// rather than a Rust enum because they are the numbers that cross:
    /// `host_testing.buri`'s `tasksOrdering` sends them, and one place spelling
    /// the mapping out is one place for the two sides to agree.
    ///
    /// `faults` is the plan itself, which the other two doubles keep in the
    /// program: a task's fault is an index, a count and a sentence, and all
    /// three of those cross, so the matching happens here beside the walk that
    /// needs it. `plan` still names the [`Slot::Plan`] holding the *promise*,
    /// entry for entry with this list.
    Tasks { mode: i64, seed: i64, plan: i64, log: Vec<i64>, faults: Vec<TaskFault> },
    Proc { code: Option<i64> },
}

/// One call to a `TestFs`, as `core/host/testing`'s `FsCall` records it: the
/// method's name, the path, and the second argument as text.
///
/// `name` is `&'static str` because the only names are the eleven this file
/// writes down; a call a program invented is not reachable.
struct FsLog {
    name: &'static str,
    path: String,
    body: String,
}

/// One request through a `TestNet`, as `NetCall` records it — `Request`'s four
/// fields, in the order `core/effect` declares them.
///
/// `method` is the variant's index, which is what crosses in either direction:
/// `host_testing.buri`'s `methodCode` sends it and [`BuriNetCall`] writes it
/// back.
struct NetLog {
    method: i8,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// One read from a `TestStdin`. `count` is what `readBytes` asked for, and zero
/// for `readLine`.
struct StdinLog {
    name: &'static str,
    count: i64,
}

/// One entry of a fault plan, as the runner sees it: the sentence a failure
/// would print, and whether the call it names has happened.
///
/// The text is assembled where the plan is installed rather than where it is
/// reported, because the pieces are the program's — a call's three fields and an
/// error's variant index and payload — and this side has no Buri value to render
/// from. `host_testing.buri`'s `describeFsPlan` hands them over one entry at a
/// time.
struct PlanEntry {
    shown: String,
    fired: bool,
}

static TABLE: Mutex<Vec<Slot>> = Mutex::new(Vec::new());

/// Lock, recovering from poisoning, for the reason `host.rs`'s `lock` gives:
/// the language has no threads, so a poisoned lock means this runtime already
/// panicked and failing a second time on top of the first helps nobody.
fn lock() -> std::sync::MutexGuard<'static, Vec<Slot>> {
    match TABLE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Record a slot and answer the handle that names it.
///
/// Fresh every call, which is what makes `Hermetic()` a *call*: what one test
/// writes is invisible to the next (`testing_context.buri`'s header).
fn install(slot: Slot) -> i64 {
    let mut table = lock();
    table.push(slot);
    (table.len() as i64) - 1
}

/// Read or update the slot a handle names.
///
/// A handle that names nothing answers the fallback rather than aborting: it
/// cannot arise from a program — every one of these values is built by a
/// constructor above — and a runtime that aborted on it would be reporting a
/// toolchain bug as a program error.
fn with<R>(handle: i64, fallback: R, f: impl FnOnce(&mut Slot) -> R) -> R {
    let mut table = lock();
    match usize::try_from(handle).ok().and_then(|i| table.get_mut(i)) {
        Some(slot) => f(slot),
        None => fallback,
    }
}

/// Borrow a `Str` argument's bytes. `lib.rs` §2 rule 1 flattens it to three
/// parameters, and `base` is unused because §3 says a parameter is borrowed.
///
/// # Safety
/// `ptr` must be readable for the byte length in `len`, or null with a zero
/// length.
unsafe fn view<'a>(ptr: *const u8, len: u64) -> &'a [u8] {
    let n = (len & crate::value::BURI_RT_STR_LEN_MASK) as usize;
    if ptr.is_null() || n == 0 {
        return &[];
    }
    // SAFETY: the caller promises `n` readable bytes at `ptr`.
    unsafe { std::slice::from_raw_parts(ptr, n) }
}

/// A `[Str]` argument, as text. The stride is a `BuriStr` because the element
/// type is fixed, which is the same reason `buri_rt_list_join` may assume it.
///
/// # Safety
/// `xs` points at `count` [`BuriStr`]s, or is null with a zero count.
unsafe fn strings(xs: *const u8, count: u64) -> Vec<String> {
    let stride = size_of::<BuriStr>();
    let mut out = Vec::new();
    if xs.is_null() {
        return out;
    }
    for i in 0..count {
        // SAFETY: the caller promises `count` elements at `xs`.
        let element = unsafe { &*xs.add((i as usize).saturating_mul(stride)).cast::<BuriStr>() };
        // SAFETY: an element of a live `[Str]` is a live view.
        out.push(unsafe { element.as_str() }.into_owned());
    }
    out
}

/// `(Str, Str)` — two `BuriStr`s end to end, which is what `middle::layout`
/// gives a tuple of two 24-byte, 8-aligned records.
#[repr(C)]
struct BuriPair {
    key: BuriStr,
    value: BuriStr,
}

/// `(Str, [U8])` — a 24-byte `Str` and a 16-byte list, both 8-aligned.
#[repr(C)]
struct BuriBytePair {
    key: BuriStr,
    value: BuriList,
}

/// A `[(Str, [U8])]` argument, as pairs.
///
/// # Safety
/// `xs` points at `count` [`BuriBytePair`]s, or is null with a zero count.
unsafe fn byte_pairs(xs: *const u8, count: u64) -> Vec<(String, Vec<u8>)> {
    let stride = size_of::<BuriBytePair>();
    let mut out = Vec::new();
    if xs.is_null() {
        return out;
    }
    for i in 0..count {
        // SAFETY: the caller promises `count` elements at `xs`.
        let element =
            unsafe { &*xs.add((i as usize).saturating_mul(stride)).cast::<BuriBytePair>() };
        // SAFETY: both halves of a live element are live.
        let key = unsafe { element.key.as_str() }.into_owned();
        // SAFETY: a `[U8]` element's payload is `len` readable bytes at `ptr`.
        let value = unsafe { view(element.value.ptr, element.value.len) }.to_vec();
        out.push((key, value));
    }
    out
}

/// A `[(Str, Str)]` argument, as pairs.
///
/// # Safety
/// `xs` points at `count` [`BuriPair`]s, or is null with a zero count.
unsafe fn pairs(xs: *const u8, count: u64) -> Vec<(String, String)> {
    let stride = size_of::<BuriPair>();
    let mut out = Vec::new();
    if xs.is_null() {
        return out;
    }
    for i in 0..count {
        // SAFETY: the caller promises `count` elements at `xs`.
        let element = unsafe { &*xs.add((i as usize).saturating_mul(stride)).cast::<BuriPair>() };
        // SAFETY: both halves of a live element are live views.
        let (key, value) = unsafe { (element.key.as_str(), element.value.as_str()) };
        out.push((key.into_owned(), value.into_owned()));
    }
    out
}

// ---------------------------------------------------------------------------
// Captured output
// ---------------------------------------------------------------------------

/// `captureOut()` — a fresh, empty transcript.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_capture_out(out: *mut i64) {
    let handle = install(Slot::Text(String::new()));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `captureErr()` — the same, for standard error.
///
/// # Safety
/// As [`buri_rt_testing_context_capture_out`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_capture_err(out: *mut i64) {
    let handle = install(Slot::Text(String::new()));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

fn append(handle: i64, bytes: &[u8], newline: bool) {
    with(handle, (), |slot| {
        if let Slot::Text(text) = slot {
            text.push_str(&String::from_utf8_lossy(bytes));
            if newline {
                text.push('\n');
            }
        }
    });
}

macro_rules! sink {
    ($name:ident, $newline:expr) => {
        /// # Safety
        /// The three `Str` parameters must describe a live view.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: i64, _base: *mut u8, ptr: *const u8, len: u64) {
            // SAFETY: forwarded to the caller.
            append(handle, unsafe { view(ptr, len) }, $newline)
        }
    };
}

sink!(buri_rt_testing_context_capture_out_print, false);
sink!(buri_rt_testing_context_capture_out_println, true);
sink!(buri_rt_testing_context_capture_err_eprint, false);
sink!(buri_rt_testing_context_capture_err_eprintln, true);

/// `Stdout::writeBytes`, into a captured stream.
///
/// Captured as the **text the octets spell**, so `captured` answers one
/// question rather than two — `testing_context.buri:39-41` states that and
/// `$testing_context_CaptureOut_writeBytes` implements it. Octets that are not
/// UTF-8 become one character per byte, which is what
/// `String.fromCharCode.apply(null, b)` does on the other backend.
///
/// # Safety
/// `ptr` must be readable for `len` bytes, or null with `len == 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_capture_out_write_bytes(
    handle: i64,
    ptr: *const u8,
    len: u64,
) {
    if ptr.is_null() || len == 0 {
        return;
    }
    // SAFETY: the caller promises `len` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|b| char::from(*b)).collect(),
    };
    with(handle, (), |slot| {
        if let Slot::Text(buffer) = slot {
            buffer.push_str(&text);
        }
    });
}

fn transcript(handle: i64) -> String {
    with(handle, String::new(), |slot| match slot {
        Slot::Text(text) => text.clone(),
        _ => String::new(),
    })
}

/// `CaptureOut::captured` — everything written to this sink so far.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_capture_out_captured(
    handle: i64,
    out: *mut BuriStr,
) {
    let text = transcript(handle);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&text)) }
}

/// `CaptureErr::capturedErr`.
///
/// # Safety
/// As [`buri_rt_testing_context_capture_out_captured`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_capture_err_captured_err(
    handle: i64,
    out: *mut BuriStr,
) {
    let text = transcript(handle);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&text)) }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// `stdin(lines)` — reads those lines, then end-of-input.
///
/// # Safety
/// `xs` points at `count` [`BuriStr`]s; `out` is writable and aligned for an
/// `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_stdin(
    xs: *const u8,
    count: u64,
    out: *mut i64,
) {
    // SAFETY: forwarded to the caller.
    let lines = unsafe { strings(xs, count) };
    let handle = install(Slot::Stdin { lines, at: 0, bytes: None, calls: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `stdinBytes(b)` — the binary twin of [`buri_rt_testing_context_stdin`].
///
/// A stdin built from octets answers `.None` to `readLine`, exactly as
/// `$testing_context_TestStdin_readLine`'s `if (s.bytes) return undefined`
/// does: the two are separate streams and a test picks one.
///
/// # Safety
/// `ptr` is readable for `len` bytes, or null with `len == 0`; `out` is
/// writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_stdin_bytes(
    ptr: *const u8,
    len: u64,
    out: *mut i64,
) {
    let bytes = if ptr.is_null() || len == 0 {
        Vec::new()
    } else {
        // SAFETY: the caller promises `len` readable bytes.
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec()
    };
    let handle = install(Slot::Stdin { lines: Vec::new(), at: 0, bytes: Some(bytes), calls: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestStdin::readLine` — `.Some(line)` or `.None` at end of input.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_test_stdin_read_line(
    handle: i64,
    out: *mut BuriStr,
) -> i32 {
    let line = with(handle, None, |slot| match slot {
        Slot::Stdin { lines, at, bytes, .. } if bytes.is_none() => {
            let line = lines.get(*at).cloned();
            if line.is_some() {
                *at = at.saturating_add(1);
            }
            line
        }
        _ => None,
    });
    let Some(line) = line else { return 0 };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&line)) };
    BURI_OK
}

/// `TestStdin::readBytes` — up to `n` octets, and `.None` when there were none.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_test_stdin_read_bytes(
    handle: i64,
    n: i64,
    out: *mut BuriList,
) -> i32 {
    let taken = with(handle, None, |slot| match slot {
        Slot::Stdin { at, bytes: Some(bytes), .. } => {
            if *at >= bytes.len() || n <= 0 {
                return None;
            }
            let end = at.saturating_add(n as usize).min(bytes.len());
            let chunk = bytes.get(*at..end).unwrap_or(&[]).to_vec();
            *at = end;
            Some(chunk)
        }
        _ => None,
    });
    let Some(chunk) = taken else { return 0 };
    let value = list_of_bytes(&chunk);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

// ---------------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------------

/// `IoError`'s variant indices, in declaration order in `core/effect`, for the
/// two errors this filesystem produces.
///
/// Named rather than written as literals because the numbers are `lib.rs`
/// §2.1's "the error variant's index in declaration order" and not ordinary
/// constants — a reader who reorders `IoError` has to find these lines.
const IO_NOT_FOUND: i32 = 0;
const IO_ALREADY_EXISTS: i32 = 3;

/// The bytes a `MemFs` handle holds at `path`, or `None` where there is no file
/// there.
///
/// A handle that names no `Files` slot cannot arise from a program — `data()`,
/// `files()` and `filesBytes()` are the only constructors of the type — so the
/// fallback is "empty" rather than an abort, per [`with`]'s rule.
fn fs_read(handle: i64, path: &str) -> Option<Vec<u8>> {
    with(handle, None, |slot| match slot {
        Slot::Files { entries, .. } => {
            entries.iter().find(|(k, _)| k == path).map(|(_, v)| v.clone())
        }
        _ => None,
    })
}

/// A path with its trailing slashes removed, which is how a directory is
/// recorded and compared. `""` and `"."` both name the root.
fn fs_clean(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed == "." {
        ""
    } else {
        trimmed
    }
}

/// Replace the file at `path`, or add it.
fn fs_put(handle: i64, path: String, body: Vec<u8>) {
    with(handle, (), |slot| {
        if let Slot::Files { entries, .. } = slot {
            match entries.iter_mut().find(|(k, _)| *k == path) {
                Some(entry) => entry.1 = body,
                None => entries.push((path, body)),
            }
        }
    });
}

/// `data()` — in-memory, and **empty**, because a compiled test binary has no
/// runner to be handed `test { data: [...] }` by.
///
/// This is the one place where a native test binary and `buri test` can answer
/// differently, and it is now readable rather than unreachable: `readFile` and
/// `fileExists` below will say a declared data file is missing. The module
/// header states the divergence and its bound — a package that declares no
/// `test { data: [...] }` cannot observe it, and `data()` on such a package is
/// specified to be empty, which is exactly what
/// `conformance/lib/semantics/test/effects.buri`'s "data is an empty package
/// filesystem when no test data is declared" asserts on both backends.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_data(out: *mut i64) {
    let handle = install(Slot::Files { entries: Vec::new(), dirs: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `files(entries)` — in-memory, containing exactly these, as the UTF-8 the
/// text spells.
///
/// # Safety
/// `xs` points at `count` `(Str, Str)` elements; `out` is writable and aligned
/// for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_files(
    xs: *const u8,
    count: u64,
    out: *mut i64,
) {
    // SAFETY: forwarded to the caller.
    let entries = unsafe { pairs(xs, count) };
    let entries = entries.into_iter().map(|(k, v)| (k, v.into_bytes())).collect();
    let handle = install(Slot::Files { entries, dirs: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `filesBytes(entries)` — the byte twin, for a fixture that is not text.
///
/// # Safety
/// `xs` points at `count` `(Str, [U8])` elements; `out` is writable and aligned
/// for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_files_bytes(
    xs: *const u8,
    count: u64,
    out: *mut i64,
) {
    // SAFETY: forwarded to the caller.
    let entries = unsafe { byte_pairs(xs, count) };
    let handle = install(Slot::Files { entries, dirs: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `MemFs.readFile(self, path) -> Result<Str, IoError>` — `lib.rs` §2.1's
/// shape, and the first entry in this archive to use it.
///
/// `$testing_context_MemFs_readFile` is `p in f ? $ok(...) : $err([0])`, so
/// there is exactly one failure and it is `NotFound`. No path normalisation:
/// the key is the string a fixture wrote, compared as bytes, which is what
/// `p in f` is.
///
/// The stored octets are decoded lossily, as a real filesystem's `readFile`
/// decodes them — a file that is not text is `readFileBytes`'s business.
///
/// # Safety
/// `ptr`/`len` describe a readable range; `out` is writable and aligned for a
/// [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_read_file(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let Some(body) = fs_read(handle, &path) else { return IO_NOT_FOUND };
    let value = str_of(&String::from_utf8_lossy(&body));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// `MemFs.writeFile(self, path, body) -> Result<(), IoError>`.
///
/// **No out-pointer**, because `()` occupies no bytes — `lib.rs` §2.1's second
/// bullet. It cannot fail: `$testing_context_MemFs_writeFile` is an assignment
/// followed by `$ok(0)`, and a `MemFs` that refused a write would be
/// `readOnly()`, which `testing_context.buri` writes in Buri rather than here.
///
/// A write to a path that is already there **replaces** it in place rather than
/// appending a second entry, so `files([("a","1")])` written twice reads back
/// once — the same statement as `files[p] = b` on an object.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_write_file(
    handle: i64,
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
    _bbase: *mut u8,
    bptr: *const u8,
    blen: u64,
) -> i32 {
    // SAFETY: the caller promises both ranges.
    let (path, body) = unsafe {
        (String::from_utf8_lossy(view(pptr, plen)).into_owned(), view(bptr, blen).to_vec())
    };
    fs_put(handle, path, body);
    BURI_OK
}

/// `MemFs.fileExists(self, path) -> Bool` — a plain scalar, and the one of the
/// four that was never about `Result`.
///
/// True for a file, and for a directory `makeDir` recorded: `existsSync`
/// answers both, and a `makeDir` a test could not then see would be a fake
/// diverging from what it stands in for.
///
/// # Safety
/// The range is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_file_exists(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> u8 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let found = with(handle, false, |slot| match slot {
        Slot::Files { entries, dirs } => {
            entries.iter().any(|(k, _)| *k == path) || dirs.contains(&path)
        }
        _ => false,
    });
    u8::from(found)
}

/// `MemFs.readDir(self, path) -> Result<[Str], IoError>`.
///
/// Transcribed from `$testing_context_MemFs_readDir`, including the two things
/// about it that are easy to get subtly different:
///
///   * **A directory that holds nothing is still not an error.** Only a path
///     that names nothing at all would be, and this filesystem has no way to
///     tell the two apart — so it never fails, and the `Result` is here because
///     the `Fs` effect declares it.
///   * **One entry per immediate child, deduplicated**, so `a/b/c` under `a`
///     lists `b` and not `b/c`. `""` and `"."` are the root; any other path
///     loses one trailing slash and gains one.
///
/// The directories `makeDir` recorded are listed alongside the files, so an
/// empty directory a test created is visible.
///
/// The order is `sort()`'s, which on JavaScript is **UTF-16 code-unit** order —
/// the same comparison [`crate::buri_rt_str_compare`] makes, and not byte
/// order, which differs where one name has an astral character exactly where
/// another has one in U+E000..U+FFFF.
///
/// # Safety
/// The range is readable; `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_read_dir(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriList,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let prefix = if path.is_empty() || path == "." {
        String::new()
    } else {
        format!("{}/", path.trim_end_matches('/'))
    };
    let mut names: Vec<String> = Vec::new();
    with(handle, (), |slot| {
        if let Slot::Files { entries, dirs } = slot {
            let keys = entries.iter().map(|(k, _)| k).chain(dirs.iter());
            for key in keys {
                let Some(rest) = key.strip_prefix(prefix.as_str()) else { continue };
                let Some(first) = rest.split('/').next().filter(|s| !s.is_empty()) else {
                    continue;
                };
                if !names.iter().any(|n| n == first) {
                    names.push(first.to_string());
                }
            }
        }
    });
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let value = list_of_strs(&names);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// `MemFs.readFileBytes(self, path) -> Result<[U8], IoError>` — the octets as
/// they were stored.
///
/// # Safety
/// The range is readable; `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_read_file_bytes(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriList,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let Some(body) = fs_read(handle, &path) else { return IO_NOT_FOUND };
    let value = list_of_bytes(&body);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// `MemFs.writeFileBytes(self, path, body) -> Result<(), IoError>` — replaces
/// the file, or creates it. Cannot fail.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_write_file_bytes(
    handle: i64,
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
    bptr: *const u8,
    blen: u64,
) -> i32 {
    // SAFETY: the caller promises both ranges.
    let (path, body) = unsafe {
        (String::from_utf8_lossy(view(pptr, plen)).into_owned(), view(bptr, blen).to_vec())
    };
    fs_put(handle, path, body);
    BURI_OK
}

/// `MemFs.appendFile(self, path, body) -> Result<(), IoError>` — adds the
/// octets to the end, creating the file when it is absent. Cannot fail.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_append_file(
    handle: i64,
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
    bptr: *const u8,
    blen: u64,
) -> i32 {
    // SAFETY: the caller promises both ranges.
    let (path, body) = unsafe {
        (String::from_utf8_lossy(view(pptr, plen)).into_owned(), view(bptr, blen).to_vec())
    };
    with(handle, (), |slot| {
        if let Slot::Files { entries, .. } = slot {
            match entries.iter_mut().find(|(k, _)| *k == path) {
                Some(entry) => entry.1.extend_from_slice(&body),
                None => entries.push((path, body)),
            }
        }
    });
    BURI_OK
}

/// `MemFs.renameFile(self, from, to) -> Result<(), IoError>` — replaces `to`.
///
/// `.Err(.NotFound)` where `from` names nothing, which is what `rename(2)`
/// answers. A move within one map is atomic for free, so the guarantee a real
/// filesystem works for is the one this gets by construction.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_rename_file(
    handle: i64,
    _fbase: *mut u8,
    fptr: *const u8,
    flen: u64,
    _tbase: *mut u8,
    tptr: *const u8,
    tlen: u64,
) -> i32 {
    // SAFETY: the caller promises both ranges.
    let (from, to) = unsafe {
        (
            String::from_utf8_lossy(view(fptr, flen)).into_owned(),
            String::from_utf8_lossy(view(tptr, tlen)).into_owned(),
        )
    };
    with(handle, IO_NOT_FOUND, |slot| {
        let Slot::Files { entries, .. } = slot else { return IO_NOT_FOUND };
        let Some(at) = entries.iter().position(|(k, _)| *k == from) else {
            return IO_NOT_FOUND;
        };
        let (_, body) = entries.remove(at);
        match entries.iter_mut().find(|(k, _)| *k == to) {
            Some(entry) => entry.1 = body,
            None => entries.push((to, body)),
        }
        BURI_OK
    })
}

/// `MemFs.removeFile(self, path) -> Result<(), IoError>`.
///
/// `.Err(.NotFound)` where the path names nothing, as `unlink(2)` answers:
/// `core/fs`'s `remove` says why that is not `.Ok`.
///
/// # Safety
/// The range is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_remove_file(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    with(handle, IO_NOT_FOUND, |slot| {
        let Slot::Files { entries, .. } = slot else { return IO_NOT_FOUND };
        let Some(at) = entries.iter().position(|(k, _)| *k == path) else {
            return IO_NOT_FOUND;
        };
        entries.remove(at);
        BURI_OK
    })
}

/// `MemFs.makeDir(self, path) -> Result<(), IoError>` — parents included, an
/// existing directory `.Ok`, and a path already naming a file
/// `.Err(.AlreadyExists)`.
///
/// # Safety
/// The range is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_make_dir(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let clean = fs_clean(&path).to_string();
    if clean.is_empty() {
        return BURI_OK;
    }
    with(handle, BURI_OK, |slot| {
        let Slot::Files { entries, dirs } = slot else { return BURI_OK };
        if entries.iter().any(|(k, _)| *k == clean) {
            return IO_ALREADY_EXISTS;
        }
        let parts: Vec<&str> = clean.split('/').collect();
        for i in 0..parts.len() {
            let at = parts.get(..=i).unwrap_or(&[]).join("/");
            if !at.is_empty() && !dirs.contains(&at) {
                dirs.push(at);
            }
        }
        BURI_OK
    })
}

/// `MemFs.syncFile(self, path) -> Result<(), IoError>` — nothing to flush.
///
/// So it answers whether there was anything to have flushed: `.Ok` for a file
/// or a directory this filesystem holds, `.Err(.NotFound)` otherwise, which is
/// what opening the path on a real one would say.
///
/// # Safety
/// The range is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_mem_fs_sync_file(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let clean = fs_clean(&path).to_string();
    if clean.is_empty() {
        return BURI_OK;
    }
    with(handle, IO_NOT_FOUND, |slot| match slot {
        Slot::Files { entries, dirs } => {
            let held =
                entries.iter().any(|(k, _)| *k == path) || dirs.contains(&clean);
            if held {
                BURI_OK
            } else {
                IO_NOT_FOUND
            }
        }
        _ => IO_NOT_FOUND,
    })
}

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// `clockAt(millis)` — starts there and advances only when a test advances it.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_clock_at(millis: i64, out: *mut i64) {
    let handle = install(Slot::Clock(millis));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestClock::nowMillis`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_testing_context_test_clock_now_millis(handle: i64) -> i64 {
    with(handle, 0, |slot| match slot {
        Slot::Clock(now) => *now,
        _ => 0,
    })
}

fn advance(handle: i64, millis: i64) {
    with(handle, (), |slot| {
        if let Slot::Clock(now) = slot {
            *now = now.wrapping_add(millis);
        }
    });
}

/// `TestClock::sleepMillis` — moves the clock without sleeping, which is what
/// `$testing_context_TestClock_sleepMillis` does and the whole point of a test
/// clock.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_testing_context_test_clock_sleep_millis(handle: i64, millis: i64) {
    advance(handle, millis);
}

/// `TestClock::advance`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_testing_context_test_clock_advance(handle: i64, millis: i64) {
    advance(handle, millis);
}

// ---------------------------------------------------------------------------
// Randomness
// ---------------------------------------------------------------------------

/// `randSeed(seed)` — seeded, so a failure reproduces.
///
/// The state is the seed's low 32 bits, and zero becomes one, which is
/// `(Math.trunc(seed) >>> 0) || 1` written in a language that has integers.
/// A zero state is a fixed point of xorshift and would answer zero forever.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_rand_seed(seed: i64, out: *mut i64) {
    let state = seed as u32;
    let handle = install(Slot::Rand(if state == 0 { 1 } else { state }));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// One xorshift32 step, byte for byte `$nextRand`'s: the sequence is part of
/// what a seeded test asserts, so the two backends have to be the same
/// generator and not merely two reproducible ones.
fn next(handle: i64) -> u32 {
    with(handle, 0, |slot| match slot {
        Slot::Rand(state) => {
            let mut x = *state;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *state = x;
            x
        }
        _ => 0,
    })
}

/// `TestRand::nextInt` — uniform enough for a fixture, in `lo ..< hi`.
///
/// An empty range aborts with the same message `host.HostRand.nextInt` and
/// `runtime.js` use, which `cli/tests/crash/random_range_empty` pins.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_testing_context_test_rand_next_int(
    handle: i64,
    lo: i64,
    hi: i64,
) -> i64 {
    if hi <= lo {
        crate::buri_rt_abort_random_range();
    }
    let span = hi.wrapping_sub(lo);
    lo.wrapping_add(i64::from(next(handle)) % span)
}

/// `TestRand::nextFloat` — `x / 2^32`, as `$testing_context_TestRand_nextFloat`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_testing_context_test_rand_next_float(handle: i64) -> f64 {
    f64::from(next(handle)) / 4294967296.0
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// `envOf(variables, arguments)` — these, and nothing the host has.
///
/// # Safety
/// `vars` points at `nvars` `(Str, Str)` elements and `args` at `nargs`
/// [`BuriStr`]s; `out` is writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_env_of(
    vars: *const u8,
    nvars: u64,
    args: *const u8,
    nargs: u64,
    out: *mut i64,
) {
    // SAFETY: forwarded to the caller.
    let (vars, args) = unsafe { (pairs(vars, nvars), strings(args, nargs)) };
    let handle = install(Slot::Env { vars, args });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestEnv::variable` — `.Some(value)` or `.None`.
///
/// The **last** binding of a name wins, because
/// `for (const e of vars) v[e[0]] = e[1]` is what builds the object on the
/// other backend and each assignment overwrites the one before it. A duplicate
/// in an `envOf` fixture is a mistake either way; agreeing about which mistake
/// costs one `rev`.
///
/// # Safety
/// The name must be a live `Str` view; `out` writable and aligned for a
/// [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_test_env_variable(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded to the caller.
    let name = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let found = with(handle, None, |slot| match slot {
        Slot::Env { vars, .. } => {
            vars.iter().rev().find(|(k, _)| *k == name).map(|(_, v)| v.clone())
        }
        _ => None,
    });
    let Some(value) = found else { return 0 };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&value)) };
    BURI_OK
}

/// `TestEnv::arguments`.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_testing_context_test_env_arguments(
    handle: i64,
    out: *mut BuriList,
) {
    let args = with(handle, Vec::new(), |slot| match slot {
        Slot::Env { args, .. } => args.clone(),
        _ => Vec::new(),
    });
    let value = list_of_strs(&args);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

// ---------------------------------------------------------------------------
// `core/host/testing` — the same doubles, under `core/host`'s names
// ---------------------------------------------------------------------------
//
// One table, not two: these share [`TABLE`] with the entries above, because a
// handle is a position in it and no two implementations ever read each other's
// — the Buri type of the value carrying the handle says which one made it.
// Two tables would be two allocators for one array.
//
// Two things are different from `core/testing/context`, and both are the point
// of the module rather than an accident of it:
//
//   * **Every constructor takes no arguments.** `clock()` is at zero and
//     `rand()` is at seed zero; a test that wants another says so with a
//     builder.
//   * **A builder answers a new handle.** `at`, `seed`, `variables` and `args`
//     each `install` rather than editing the slot they were called on, so the
//     value a test already holds is unchanged and two clocks built from one
//     are two clocks. That is what makes `let base = env(); base.args([..])`
//     safe to write twice.
//
// `alloc()` and `TestAlloc::allocate` are absent for the reason the module
// header gives about `core/testing/context`'s: both native backends open-code
// them, because the handle names nothing and `allocate` answers the count it
// was handed.
//
// One shape is genuinely new rather than a second spelling: `TestFs` is a
// *view* onto a store, so that `readOnly` can attenuate a filesystem without
// copying it. [`Slot::Fs`] says why.

/// `stdout()` — a fresh, empty transcript.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_stdout(out: *mut i64) {
    let handle = install(Slot::Text(String::new()));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `stderr()` — the same, for standard error.
///
/// # Safety
/// As [`buri_rt_host_testing_stdout`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_stderr(out: *mut i64) {
    let handle = install(Slot::Text(String::new()));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

sink!(buri_rt_host_testing_test_stdout_print, false);
sink!(buri_rt_host_testing_test_stdout_println, true);
sink!(buri_rt_host_testing_test_stderr_eprint, false);
sink!(buri_rt_host_testing_test_stderr_eprintln, true);

/// `TestStdout::writeBytes` — captured as the text the octets spell, so
/// `captured` answers one question rather than two.
///
/// # Safety
/// `ptr` must be readable for `len` bytes, or null with `len == 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_stdout_write_bytes(
    handle: i64,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller.
    unsafe { buri_rt_testing_context_capture_out_write_bytes(handle, ptr, len) }
}

/// `TestStdout::captured`.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_stdout_captured(
    handle: i64,
    out: *mut BuriStr,
) {
    let text = transcript(handle);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&text)) }
}

/// `TestStderr::captured` — the same question of the other stream, which is
/// why it is the same name and a different receiver.
///
/// # Safety
/// As [`buri_rt_host_testing_test_stdout_captured`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_stderr_captured(
    handle: i64,
    out: *mut BuriStr,
) {
    let text = transcript(handle);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&text)) }
}

/// `stdin()` — end of input, until a test says otherwise.
///
/// `lines` empty and `bytes` absent, so `readLine` runs off the end of the
/// lines and `readBytes` falls through to the `_` arm: both `.None`, which is
/// the empty fixture stated twice rather than a special case.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_stdin(out: *mut i64) {
    let handle = install(Slot::Stdin { lines: Vec::new(), at: 0, bytes: None, calls: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestStdin::lines` — a **new** stdin reading those lines.
///
/// It does not keep the receiver's octets, and `bytes` does not keep its
/// lines: a stream is one or the other, and the last builder in a chain is the
/// stream. `core/host/testing`'s header says so where a reader meets it.
///
/// # Safety
/// `xs` points at `count` [`BuriStr`]s; `out` is writable and aligned for an
/// `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_stdin_lines(
    _handle: i64,
    xs: *const u8,
    count: u64,
    out: *mut i64,
) {
    // SAFETY: forwarded to the caller.
    let lines = unsafe { strings(xs, count) };
    let handle = install(Slot::Stdin { lines, at: 0, bytes: None, calls: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestStdin::bytes` — a **new** stdin reading those octets.
///
/// # Safety
/// `ptr` is readable for `len` bytes, or null with `len == 0`; `out` is
/// writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_stdin_bytes(
    _handle: i64,
    ptr: *const u8,
    len: u64,
    out: *mut i64,
) {
    let bytes = if ptr.is_null() || len == 0 {
        Vec::new()
    } else {
        // SAFETY: the caller promises `len` readable bytes.
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec()
    };
    let handle = install(Slot::Stdin { lines: Vec::new(), at: 0, bytes: Some(bytes), calls: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestStdin::readLine` — `.Some(line)` or `.None` at end of input.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_stdin_read_line(
    handle: i64,
    out: *mut BuriStr,
) -> i32 {
    let _recorded = recording_stdin(handle, "readLine", 0);
    let line = with(handle, None, |slot| match slot {
        Slot::Stdin { lines, at, bytes, .. } if bytes.is_none() => {
            let line = lines.get(*at).cloned();
            if line.is_some() {
                *at = at.saturating_add(1);
            }
            line
        }
        _ => None,
    });
    let Some(line) = line else { return 0 };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&line)) };
    BURI_OK
}

/// `TestStdin::readBytes` — up to `n` octets, and `.None` when there were none.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_stdin_read_bytes(
    handle: i64,
    n: i64,
    out: *mut BuriList,
) -> i32 {
    let _recorded = recording_stdin(handle, "readBytes", n);
    let taken = with(handle, None, |slot| match slot {
        Slot::Stdin { at, bytes: Some(bytes), .. } => {
            if *at >= bytes.len() || n <= 0 {
                return None;
            }
            let end = at.saturating_add(n as usize).min(bytes.len());
            let chunk = bytes.get(*at..end).unwrap_or(&[]).to_vec();
            *at = end;
            Some(chunk)
        }
        _ => None,
    });
    let Some(chunk) = taken else { return 0 };
    let value = list_of_bytes(&chunk);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

// -- `core/host/testing`'s filesystem ---------------------------------------
//
// A `TestFs` handle is a **view**: [`Slot::Fs`] names the [`Slot::Files`] store
// its files live in and says whether writes through *this* view are refused.
// `readOnly` installs a second view onto the same store, which is what folds
// `ReadOnly<C>` into a method without turning it into a copy — the wrapper
// holds the inner value, so a read through the attenuated handle sees whatever
// the filesystem holds now.
//
// Two slots per `fs()` rather than one, and that is the price of the fold. The
// alternative — a flag inside `Slot::Files` — would make `readOnly` either
// copy the files (a different value, not an attenuation of this one) or
// attenuate the receiver as well (a builder that edited what it was called
// on, which no other builder in this module does).

/// `IoError::ReadOnly`'s index, in declaration order in `core/effect`.
///
/// `lib.rs` §2.1's "the error variant's index in declaration order", like
/// [`IO_NOT_FOUND`] and [`IO_ALREADY_EXISTS`] above.
const IO_READ_ONLY: i32 = 2;

/// The store a `TestFs` handle reads and writes, and whether writes through it
/// are refused.
///
/// A handle naming no view answers `-1`, which no `usize` conversion accepts,
/// so every caller below falls through to [`with`]'s fallback rather than
/// aborting — [`with`]'s rule, for the reason it gives.
fn fs_view(handle: i64) -> (i64, bool) {
    let table = lock();
    match usize::try_from(handle).ok().and_then(|i| table.get(i)) {
        Some(Slot::Fs { store, read_only, .. }) => (*store, *read_only),
        _ => (-1, false),
    }
}

/// The plan a handle fails through, or `-1` where it has none.
///
/// Separate from [`fs_view`] because only the three builders read it, and every
/// method below reads the view: a third element on that tuple would be a value
/// eleven callers destructure and discard.
fn slot_plan(handle: i64) -> i64 {
    let table = lock();
    match usize::try_from(handle).ok().and_then(|i| table.get(i)) {
        Some(
            Slot::Fs { plan, .. } | Slot::Net { plan, .. } | Slot::Tasks { plan, .. },
        ) => *plan,
        _ => -1,
    }
}

/// A store holding `entries`, and a view onto it with `read_only` and `plan`.
///
/// The order matters: the store is installed first so that a view's `store` is
/// always an index below its own, which is what makes [`fs_view`] one lookup
/// rather than a walk.
fn fs_install(
    entries: Vec<(String, Vec<u8>)>,
    dirs: Vec<String>,
    read_only: bool,
    plan: i64,
) -> i64 {
    let store = install(Slot::Files { entries, dirs });
    install(Slot::Fs { store, read_only, plan, calls: Vec::new() })
}

/// This view's files and directories, copied.
fn fs_contents(store: i64) -> (Vec<(String, Vec<u8>)>, Vec<String>) {
    with(store, (Vec::new(), Vec::new()), |slot| match slot {
        Slot::Files { entries, dirs } => (entries.clone(), dirs.clone()),
        _ => (Vec::new(), Vec::new()),
    })
}

/// A `[(Str, Str)]`, as one block of 48-byte elements.
///
/// Two `Str`s end to end is what `middle::layout` gives a tuple of two 24-byte,
/// 8-aligned records, and it is the shape [`BuriPair`] already describes on the
/// way in — `buri_rt_str_split_once` writes the same pair through an
/// out-pointer. Each string's bytes are their own block, as
/// [`crate::value::list_of_strs`]' comment says.
fn list_of_pairs(items: &[(String, String)]) -> BuriList {
    let stride = size_of::<BuriPair>();
    if items.is_empty() {
        return BuriList { ptr: std::ptr::null_mut(), len: 0 };
    }
    let bytes = items.len().saturating_mul(stride);
    let ptr = crate::memory::buri_rt_alloc(bytes as u64);
    for (i, (key, value)) in items.iter().enumerate() {
        // SAFETY: `i * stride` is within the `items.len() * stride` block, and
        // the destination is 8-aligned because the payload is 16-aligned and
        // the stride is a multiple of 8.
        unsafe {
            ptr.add(i.saturating_mul(stride))
                .cast::<BuriPair>()
                .write(BuriPair { key: str_of(key), value: str_of(value) })
        };
    }
    BuriList { ptr, len: items.len() as u64 }
}

/// `newFs()` — in-memory, empty, writable, and failing nothing.
///
/// A bare handle rather than a `TestFs`, for `newNet`'s reason: `fs()` is a Buri
/// body that builds the value around it, because the second field is a fault
/// plan and a plan is a list of Buri values this side cannot make.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_new_fs() -> i64 {
    fs_install(Vec::new(), Vec::new(), false, -1)
}

/// `TestFs::files` — a **new** filesystem holding this one's files and these as
/// well, as the UTF-8 the text spells.
///
/// Additive rather than replacing, so `files` and `filesBytes` compose in
/// either order: both write into the one map a file lives in, and a path
/// written twice is the later body — `fs_put`'s rule, and an object
/// assignment's.
///
/// The attenuation travels with the files, and so does the fault plan:
/// `fs().readOnly().files(..)` is a read-only filesystem with those files in it
/// and `fs().faults(p).files(..)` still fails what `p` names, because a builder
/// is configuration and not a write.
///
/// # Safety
/// `xs` points at `count` `(Str, Str)` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_files(
    handle: i64,
    xs: *const u8,
    count: u64,
) -> i64 {
    // SAFETY: forwarded to the caller.
    let added = unsafe { pairs(xs, count) };
    let added = added.into_iter().map(|(k, v)| (k, v.into_bytes()));
    fs_extended(handle, added)
}

/// `TestFs::filesBytes` — the byte twin, for a fixture that is not text.
///
/// # Safety
/// `xs` points at `count` `(Str, [U8])` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_files_bytes(
    handle: i64,
    xs: *const u8,
    count: u64,
) -> i64 {
    // SAFETY: forwarded to the caller.
    let added = unsafe { byte_pairs(xs, count) };
    fs_extended(handle, added)
}

/// The handle both builders answer: this view's files with `added` written over
/// them, in a store of its own, under this view's attenuation and plan.
fn fs_extended(handle: i64, added: impl IntoIterator<Item = (String, Vec<u8>)>) -> i64 {
    let (store, read_only) = fs_view(handle);
    let plan = slot_plan(handle);
    let (mut entries, dirs) = fs_contents(store);
    for (path, body) in added {
        match entries.iter_mut().find(|(k, _)| *k == path) {
            Some(entry) => entry.1 = body,
            None => entries.push((path, body)),
        }
    }
    fs_install(entries, dirs, read_only, plan)
}

/// `TestFs::readOnly` — a **new** handle onto the *same* files, through which
/// every write fails.
///
/// The same store, deliberately: `ReadOnly<C>` holds the inner value, so a read
/// through it answers whatever that filesystem holds now, and a method that
/// copied would be a snapshot wearing an attenuator's name. The plan travels
/// with it, as it does through the other two builders.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_fs_read_only(handle: i64) -> i64 {
    let (store, _) = fs_view(handle);
    let plan = slot_plan(handle);
    install(Slot::Fs { store, read_only: true, plan, calls: Vec::new() })
}

/// `TestFs::faults` — a **new** view onto the same store, with a fresh, empty
/// plan and this one's attenuation.
///
/// The plan the receiver was using is retired: `faults` replaces rather than
/// composing, and a promise that has been replaced is not one
/// [`buri_rt_test_leave`] should report. A fresh log, as every builder here
/// answers one, so the calls `failsOnCall` counts are the calls made through the
/// value the test put in its context.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_fs_with_plan(handle: i64) -> i64 {
    let (store, read_only) = fs_view(handle);
    retire(slot_plan(handle));
    let plan = install(Slot::Plan { entries: Vec::new(), retired: false });
    install(Slot::Fs { store, read_only, plan, calls: Vec::new() })
}

/// Marks a plan as one nothing can keep any more.
fn retire(plan: i64) {
    with(plan, (), |slot| {
        if let Slot::Plan { retired, .. } = slot {
            *retired = true;
        }
    });
}

/// `TestFs::read(self, path) -> Result<Str, IoError>` — the read-back, without
/// the effect.
///
/// The same answer `readFile` gives, including `.Err(.NotFound)` and the lossy
/// decode; what it does not need is an `Fs` bound, because asserting on what a
/// function wrote is reading an environment back rather than performing an
/// effect.
///
/// # Safety
/// `ptr`/`len` describe a readable range; `out` is writable and aligned for a
/// [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_read(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let (store, _) = fs_view(handle);
    let Some(body) = fs_read(store, &path) else { return IO_NOT_FOUND };
    let value = str_of(&String::from_utf8_lossy(&body));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// `TestFs::snapshot(self) -> [(Str, Str)]` — every file, as text, sorted by
/// path.
///
/// **UTF-16 code-unit order**, which is `sort()`'s on the other backend and the
/// comparison [`crate::buri_rt_str_compare`] makes: the same reason
/// `buri_rt_testing_context_mem_fs_read_dir` sorts that way, and the same two
/// strings it would differ on.
///
/// Files only. A directory `makeDir` recorded holds no octets and is not a
/// file, and `readDir` is the question it answers.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_snapshot(
    handle: i64,
    out: *mut BuriList,
) {
    let (store, _) = fs_view(handle);
    let (entries, _) = fs_contents(store);
    let mut items: Vec<(String, String)> = entries
        .into_iter()
        .map(|(k, v)| (k, String::from_utf8_lossy(&v).into_owned()))
        .collect();
    items.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    let value = list_of_pairs(&items);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `TestFs.readFile(self, path) -> Result<Str, IoError>`, forwarded through the
/// view. A read is never refused.
///
/// # Safety
/// As [`buri_rt_host_testing_fs_read`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_read_file(
    handle: i64,
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    // The recording is here and not in `read`, which the two share: `read` is
    // the read-back and a read-back is not a call.
    let _recorded = recording_fs(handle, "readFile", &path, "");
    // SAFETY: forwarded to the caller.
    unsafe { buri_rt_host_testing_fs_read(handle, base, ptr, len, out) }
}

/// `TestFs.writeFile(self, path, body) -> Result<(), IoError>`.
///
/// `.Err(.ReadOnly)` through an attenuated view, and otherwise it cannot fail:
/// a write to a path already there replaces it in place, so `files` written
/// twice reads back once.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_write_file(
    handle: i64,
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
    _bbase: *mut u8,
    bptr: *const u8,
    blen: u64,
) -> i32 {
    let (store, read_only) = fs_view(handle);
    // SAFETY: the caller promises both ranges.
    let (path, body) = unsafe {
        (String::from_utf8_lossy(view(pptr, plen)).into_owned(), view(bptr, blen).to_vec())
    };
    let _recorded =
        recording_fs(handle, "writeFile", &path, &String::from_utf8_lossy(&body));
    if read_only {
        return IO_READ_ONLY;
    }
    fs_put(store, path, body);
    BURI_OK
}

/// `TestFs.fileExists(self, path) -> Bool` — true for a file, and for a
/// directory `makeDir` recorded.
///
/// # Safety
/// The range is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_file_exists(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> u8 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let _recorded = recording_fs(handle, "fileExists", &path, "");
    let (store, _) = fs_view(handle);
    let found = with(store, false, |slot| match slot {
        Slot::Files { entries, dirs } => {
            entries.iter().any(|(k, _)| *k == path) || dirs.contains(&path)
        }
        _ => false,
    });
    u8::from(found)
}

/// `TestFs.readDir(self, path) -> Result<[Str], IoError>` — one entry per
/// immediate child, deduplicated, in UTF-16 code-unit order.
///
/// `buri_rt_testing_context_mem_fs_read_dir`'s two subtleties, unchanged: a
/// directory that holds nothing is still not an error, and the directories
/// `makeDir` recorded are listed alongside the files.
///
/// # Safety
/// The range is readable; `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_read_dir(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriList,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let _recorded = recording_fs(handle, "readDir", &path, "");
    let prefix = if path.is_empty() || path == "." {
        String::new()
    } else {
        format!("{}/", path.trim_end_matches('/'))
    };
    let (store, _) = fs_view(handle);
    let mut names: Vec<String> = Vec::new();
    with(store, (), |slot| {
        if let Slot::Files { entries, dirs } = slot {
            let keys = entries.iter().map(|(k, _)| k).chain(dirs.iter());
            for key in keys {
                let Some(rest) = key.strip_prefix(prefix.as_str()) else { continue };
                let Some(first) = rest.split('/').next().filter(|s| !s.is_empty()) else {
                    continue;
                };
                if !names.iter().any(|n| n == first) {
                    names.push(first.to_string());
                }
            }
        }
    });
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let value = list_of_strs(&names);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// `TestFs.readFileBytes(self, path) -> Result<[U8], IoError>` — the octets as
/// they were stored.
///
/// # Safety
/// The range is readable; `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_read_file_bytes(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriList,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let _recorded = recording_fs(handle, "readFileBytes", &path, "");
    let (store, _) = fs_view(handle);
    let Some(body) = fs_read(store, &path) else { return IO_NOT_FOUND };
    let value = list_of_bytes(&body);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// `TestFs.writeFileBytes(self, path, body) -> Result<(), IoError>` — replaces
/// the file, or creates it. `.Err(.ReadOnly)` through an attenuated view.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_write_file_bytes(
    handle: i64,
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
    bptr: *const u8,
    blen: u64,
) -> i32 {
    let (store, read_only) = fs_view(handle);
    // SAFETY: the caller promises both ranges.
    let (path, body) = unsafe {
        (String::from_utf8_lossy(view(pptr, plen)).into_owned(), view(bptr, blen).to_vec())
    };
    let _recorded =
        recording_fs(handle, "writeFileBytes", &path, &String::from_utf8_lossy(&body));
    if read_only {
        return IO_READ_ONLY;
    }
    fs_put(store, path, body);
    BURI_OK
}

/// `TestFs.appendFile(self, path, body) -> Result<(), IoError>` — adds the
/// octets to the end, creating the file when it is absent.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_append_file(
    handle: i64,
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
    bptr: *const u8,
    blen: u64,
) -> i32 {
    let (store, read_only) = fs_view(handle);
    // SAFETY: the caller promises both ranges.
    let (path, body) = unsafe {
        (String::from_utf8_lossy(view(pptr, plen)).into_owned(), view(bptr, blen).to_vec())
    };
    let _recorded =
        recording_fs(handle, "appendFile", &path, &String::from_utf8_lossy(&body));
    if read_only {
        return IO_READ_ONLY;
    }
    with(store, (), |slot| {
        if let Slot::Files { entries, .. } = slot {
            match entries.iter_mut().find(|(k, _)| *k == path) {
                Some(entry) => entry.1.extend_from_slice(&body),
                None => entries.push((path, body)),
            }
        }
    });
    BURI_OK
}

/// `TestFs.renameFile(self, from, to) -> Result<(), IoError>` — replaces `to`,
/// and `.Err(.NotFound)` where `from` names nothing.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_rename_file(
    handle: i64,
    _fbase: *mut u8,
    fptr: *const u8,
    flen: u64,
    _tbase: *mut u8,
    tptr: *const u8,
    tlen: u64,
) -> i32 {
    let (store, read_only) = fs_view(handle);
    // SAFETY: the caller promises both ranges.
    let (from, to) = unsafe {
        (
            String::from_utf8_lossy(view(fptr, flen)).into_owned(),
            String::from_utf8_lossy(view(tptr, tlen)).into_owned(),
        )
    };
    let _recorded = recording_fs(handle, "renameFile", &from, &to);
    if read_only {
        return IO_READ_ONLY;
    }
    with(store, IO_NOT_FOUND, |slot| {
        let Slot::Files { entries, .. } = slot else { return IO_NOT_FOUND };
        let Some(at) = entries.iter().position(|(k, _)| *k == from) else {
            return IO_NOT_FOUND;
        };
        let (_, body) = entries.remove(at);
        match entries.iter_mut().find(|(k, _)| *k == to) {
            Some(entry) => entry.1 = body,
            None => entries.push((to, body)),
        }
        BURI_OK
    })
}

/// `TestFs.removeFile(self, path) -> Result<(), IoError>` — `.Err(.NotFound)`
/// where the path names nothing, as `unlink(2)` answers.
///
/// # Safety
/// The range is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_remove_file(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> i32 {
    let (store, read_only) = fs_view(handle);
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let _recorded = recording_fs(handle, "removeFile", &path, "");
    if read_only {
        return IO_READ_ONLY;
    }
    with(store, IO_NOT_FOUND, |slot| {
        let Slot::Files { entries, .. } = slot else { return IO_NOT_FOUND };
        let Some(at) = entries.iter().position(|(k, _)| *k == path) else {
            return IO_NOT_FOUND;
        };
        entries.remove(at);
        BURI_OK
    })
}

/// `TestFs.makeDir(self, path) -> Result<(), IoError>` — parents included, an
/// existing directory `.Ok`, and a path already naming a file
/// `.Err(.AlreadyExists)`.
///
/// # Safety
/// The range is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_make_dir(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> i32 {
    let (store, read_only) = fs_view(handle);
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let _recorded = recording_fs(handle, "makeDir", &path, "");
    if read_only {
        return IO_READ_ONLY;
    }
    let clean = fs_clean(&path).to_string();
    if clean.is_empty() {
        return BURI_OK;
    }
    with(store, BURI_OK, |slot| {
        let Slot::Files { entries, dirs } = slot else { return BURI_OK };
        if entries.iter().any(|(k, _)| *k == clean) {
            return IO_ALREADY_EXISTS;
        }
        let parts: Vec<&str> = clean.split('/').collect();
        for i in 0..parts.len() {
            let at = parts.get(..=i).unwrap_or(&[]).join("/");
            if !at.is_empty() && !dirs.contains(&at) {
                dirs.push(at);
            }
        }
        BURI_OK
    })
}

/// `TestFs.syncFile(self, path) -> Result<(), IoError>` — nothing to flush, so
/// it answers whether there was anything to have flushed.
///
/// **Not** refused through an attenuated view: `sync` is not a write, and there
/// is nothing an attenuator could be hiding — whatever the filesystem already
/// holds is what gets flushed. `ReadOnly<C>::syncFile` forwards for the same
/// reason, in those words.
///
/// # Safety
/// The range is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_sync_file(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> i32 {
    // SAFETY: the caller promises the range.
    let path = String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned();
    let _recorded = recording_fs(handle, "syncFile", &path, "");
    let clean = fs_clean(&path).to_string();
    if clean.is_empty() {
        return BURI_OK;
    }
    let (store, _) = fs_view(handle);
    with(store, IO_NOT_FOUND, |slot| match slot {
        Slot::Files { entries, dirs } => {
            let held = entries.iter().any(|(k, _)| *k == path) || dirs.contains(&clean);
            if held {
                BURI_OK
            } else {
                IO_NOT_FOUND
            }
        }
        _ => IO_NOT_FOUND,
    })
}

/// `clock()` — at zero, and advancing only when a test advances it.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_clock(out: *mut i64) {
    let handle = install(Slot::Clock(0));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestClock::at` — a **new** clock at that instant. The receiver is read for
/// nothing, and that is the whole shape: a builder answers a handle rather than
/// editing one.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_clock_at(
    _handle: i64,
    millis: i64,
    out: *mut i64,
) {
    let handle = install(Slot::Clock(millis));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestClock::nowMillis`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_test_clock_now_millis(handle: i64) -> i64 {
    buri_rt_testing_context_test_clock_now_millis(handle)
}

/// `TestClock::sleepMillis` — moves the clock without sleeping.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_test_clock_sleep_millis(handle: i64, millis: i64) {
    advance(handle, millis);
}

/// `rand()` — seeded at zero, which is the state `randSeed(0)` produces: a
/// zero state is a fixed point of xorshift, so it becomes one.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rand(out: *mut i64) {
    let handle = install(Slot::Rand(1));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestRand::seed` — a **new** generator at that seed, drawing from the start
/// of its sequence.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_rand_seed(
    _handle: i64,
    seed: i64,
    out: *mut i64,
) {
    let state = seed as u32;
    let handle = install(Slot::Rand(if state == 0 { 1 } else { state }));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestRand::nextInt`, the same xorshift32 sequence `randSeed` draws.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_test_rand_next_int(handle: i64, lo: i64, hi: i64) -> i64 {
    buri_rt_testing_context_test_rand_next_int(handle, lo, hi)
}

/// `TestRand::nextFloat`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_test_rand_next_float(handle: i64) -> f64 {
    buri_rt_testing_context_test_rand_next_float(handle)
}

/// `env()` — no variables and no arguments.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_env(out: *mut i64) {
    let handle = install(Slot::Env { vars: Vec::new(), args: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// The arguments a handle holds, or none where it names no `Env` slot.
fn env_args(handle: i64) -> Vec<String> {
    with(handle, Vec::new(), |slot| match slot {
        Slot::Env { args, .. } => args.clone(),
        _ => Vec::new(),
    })
}

/// `TestEnv::variables` — a **new** environment with these variables and this
/// one's arguments, so the two builders compose in either order.
///
/// # Safety
/// `xs` points at `count` `(Str, Str)` elements; `out` is writable and aligned
/// for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_env_variables(
    handle: i64,
    xs: *const u8,
    count: u64,
    out: *mut i64,
) {
    // SAFETY: forwarded to the caller.
    let vars = unsafe { pairs(xs, count) };
    let args = env_args(handle);
    let fresh = install(Slot::Env { vars, args });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(fresh) }
}

/// `TestEnv::args` — a **new** environment with these arguments and this one's
/// variables.
///
/// `args` rather than `arguments` because `Env` already declares the reader of
/// that name and a Buri type has one method of each name; the module header
/// says so where a reader of the source will meet it.
///
/// # Safety
/// `xs` points at `count` [`BuriStr`]s; `out` is writable and aligned for an
/// `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_env_args(
    handle: i64,
    xs: *const u8,
    count: u64,
    out: *mut i64,
) {
    // SAFETY: forwarded to the caller.
    let args = unsafe { strings(xs, count) };
    let vars = with(handle, Vec::new(), |slot| match slot {
        Slot::Env { vars, .. } => vars.clone(),
        _ => Vec::new(),
    });
    let fresh = install(Slot::Env { vars, args });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(fresh) }
}

/// `TestEnv::variable`.
///
/// # Safety
/// As [`buri_rt_testing_context_test_env_variable`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_env_variable(
    handle: i64,
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded to the caller.
    unsafe { buri_rt_testing_context_test_env_variable(handle, base, ptr, len, out) }
}

/// `TestEnv::arguments`.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_env_arguments(
    handle: i64,
    out: *mut BuriList,
) {
    let value = list_of_strs(&env_args(handle));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `proc()` — nothing has exited.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_proc(out: *mut i64) {
    let handle = install(Slot::Proc { code: None });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `TestProc::exitWith` — **records** the exit rather than taking it.
///
/// A test that ended the process would take every block after it with it, and
/// the runner would report a suite that stopped rather than a function that
/// exited. The *first* code is kept, because a program that exits does not
/// carry on and a second call is one a real process could never have made.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_test_proc_exit_with(handle: i64, code: i64) {
    with(handle, (), |slot| {
        if let Slot::Proc { code: recorded } = slot {
            if recorded.is_none() {
                *recorded = Some(code);
            }
        }
    });
}

/// `TestProc::exited` — `.Some(code)` or `.None`.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_proc_exited(
    handle: i64,
    out: *mut i64,
) -> i32 {
    let code = with(handle, None, |slot| match slot {
        Slot::Proc { code } => *code,
        _ => None,
    });
    let Some(code) = code else { return 0 };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(code) };
    BURI_OK
}

// -- `core/host/testing`'s call log -----------------------------------------
//
// One append-only log per handle: `Slot::Fs`'s, `Slot::Net`'s and
// `Slot::Stdin`'s. What a double answers comes from its fixture and what it was
// *asked* comes from here, and the two are separate because a test asserting on
// a call is asking a different question from one asserting on an outcome.
//
// **In completion order**, which is what `calls()` promises. Nothing here
// suspends yet, so completion order is program order — but the recording is
// still done on the way *out* of a call rather than on the way in, by
// [`Recording`]'s `Drop`, so that the day a call can be in flight the order
// does not have to be revisited. That is also what makes one line per method
// enough: a refusal returns early and is recorded exactly as an answer is.
//
// The three `Buri*Call` records below are the Buri types' layouts, and they are
// `#[repr(C)]` for `value.rs`'s reason: VALUE-MODEL.md §5 lays a struct out in
// declaration order at natural alignment under C rules, so a `#[repr(C)]`
// record of the same fields *is* the layout rather than a description of it.

/// `FsCall` — `struct { name: Str, path: Str, body: Str }`, three `Str`s end to
/// end.
#[repr(C)]
struct BuriFsCall {
    name: BuriStr,
    path: BuriStr,
    body: BuriStr,
}

/// `NetCall(Request)` — a newtype *is* its field, and `Request` is
/// `{ method: Method, url: Str, headers: [Header], body: [U8] }`.
///
/// `Method` is seven payload-free variants, which VALUE-MODEL.md §6's first
/// niche makes the tag itself — an `i8`, since `middle::layout` gives every
/// enum of at most 256 variants one. It is the only discriminant this file
/// writes, and `cli/tests/conformance/lib/semantics/test/host_testing.buri`
/// asserts a round trip through all seven on every backend.
#[repr(C)]
struct BuriNetCall {
    method: i8,
    url: BuriStr,
    headers: BuriList,
    body: BuriList,
}

/// `StdinCall` — `struct { name: Str, count: Int }`.
#[repr(C)]
struct BuriStdinCall {
    name: BuriStr,
    count: i64,
}

/// A `[T]` of one of the three records above: one block of `size_of::<T>()`
/// strides, each written in place.
///
/// [`list_of_pairs`] with the element type abstracted, and the same two
/// promises: the stride is a multiple of 8 and the payload is 16-aligned, so
/// every element is 8-aligned; and each element's strings are their own blocks.
fn list_of<T, S>(items: &[S], element: impl Fn(&S) -> T) -> BuriList {
    let stride = size_of::<T>();
    if items.is_empty() {
        return BuriList { ptr: std::ptr::null_mut(), len: 0 };
    }
    let bytes = items.len().saturating_mul(stride);
    let ptr = crate::memory::buri_rt_alloc(bytes as u64);
    for (i, item) in items.iter().enumerate() {
        // SAFETY: `i * stride` is within the `items.len() * stride` block, and
        // the destination is 8-aligned because the payload is 16-aligned and
        // every one of the three strides is a multiple of 8.
        unsafe { ptr.add(i.saturating_mul(stride)).cast::<T>().write(element(item)) };
    }
    BuriList { ptr, len: items.len() as u64 }
}

/// One call, appended to `handle`'s log when it goes out of scope.
///
/// Which is where the call *completes*, whichever arm returned it — an early
/// refusal and a plain answer are one line at the top of the function rather
/// than two call sites that could drift apart.
struct Recording {
    handle: i64,
    call: Option<Call>,
}

/// What a [`Recording`] is holding until the call it names finishes.
enum Call {
    Fs(FsLog),
    Stdin(StdinLog),
}

impl Drop for Recording {
    fn drop(&mut self) {
        let Some(call) = self.call.take() else { return };
        with(self.handle, (), |slot| match (slot, call) {
            (Slot::Fs { calls, .. }, Call::Fs(one)) => calls.push(one),
            (Slot::Stdin { calls, .. }, Call::Stdin(one)) => calls.push(one),
            _ => {}
        });
    }
}

/// A `TestFs` call, recorded on the **view** it was made through.
///
/// The view and not the store: `calls()` is per handle, so a builder's new
/// filesystem and `readOnly`'s new view each start with an empty log, and the
/// calls a test reads back are the ones made through the value it put in the
/// context.
fn recording_fs(handle: i64, name: &'static str, path: &str, body: &str) -> Recording {
    Recording {
        handle,
        call: Some(Call::Fs(FsLog {
            name,
            path: path.to_string(),
            body: body.to_string(),
        })),
    }
}

/// A `TestStdin` read, recorded on the stream it was made through.
fn recording_stdin(handle: i64, name: &'static str, count: i64) -> Recording {
    Recording { handle, call: Some(Call::Stdin(StdinLog { name, count })) }
}

/// `TestFs::calls` — every call through this view, in completion order.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fs_calls(handle: i64, out: *mut BuriList) {
    let calls = with(handle, Vec::new(), |slot| match slot {
        Slot::Fs { calls, .. } => {
            calls.iter().map(|c| (c.name, c.path.clone(), c.body.clone())).collect()
        }
        _ => Vec::new(),
    });
    let value = list_of(&calls, |(name, path, body): &(&'static str, String, String)| BuriFsCall {
        name: str_of(name),
        path: str_of(path),
        body: str_of(body),
    });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `TestStdin::calls` — every read through this stream, in completion order.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_stdin_calls(handle: i64, out: *mut BuriList) {
    let calls = with(handle, Vec::new(), |slot| match slot {
        Slot::Stdin { calls, .. } => calls.iter().map(|c| (c.name, c.count)).collect(),
        _ => Vec::new(),
    });
    let value = list_of(&calls, |(name, count): &(&'static str, i64)| BuriStdinCall {
        name: str_of(name),
        count: *count,
    });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `newNet()` — a fresh, empty log, and the handle that names it.
///
/// A bare `I64` rather than a slot-shaped value, in
/// `buri_rt_alloc_new_counter`'s shape and for its reason: `net()` is a Buri
/// body that builds the `TestNet` itself, because the responder in the other
/// field is a value the archive cannot make.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_new_net() -> i64 {
    install(Slot::Net { calls: Vec::new(), plan: -1 })
}

/// `netRebind(handle)` — a fresh, empty log carrying this one's plan.
///
/// `respond` answers a new network and a new log, because a network that shared
/// its receiver's log would report calls made to a different one; what it does
/// not change is what the test said would fail.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_net_rebind(handle: i64) -> i64 {
    install(Slot::Net { calls: Vec::new(), plan: slot_plan(handle) })
}

/// `netWithPlan(handle)` — a fresh, empty log with a fresh, empty plan.
///
/// [`buri_rt_host_testing_fs_with_plan`] for the network, retiring the replaced
/// plan for the same reason.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_net_with_plan(handle: i64) -> i64 {
    retire(slot_plan(handle));
    let plan = install(Slot::Plan { entries: Vec::new(), retired: false });
    install(Slot::Net { calls: Vec::new(), plan })
}

/// `recordFetch(handle, method, url, headers, body)` — one request, recorded
/// after the responder has answered it.
///
/// The four pieces are `Request`'s four fields flattened by §2 rule 1, which is
/// exactly what `crate::buri_rt_host_net_fetch` is handed: the method's variant
/// index, the URL's three `Str` leaves, and two `(ptr, len)` pairs. They are
/// put back together by [`buri_rt_host_testing_net_calls`].
///
/// # Safety
/// The URL view, the `[Header]` and the `[U8]` must be live for the call.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_host_testing_record_fetch(
    handle: i64,
    method: i64,
    _ubase: *mut u8,
    uptr: *const u8,
    ulen: u64,
    hptr: *const u8,
    hlen: u64,
    bptr: *const u8,
    blen: u64,
) {
    // SAFETY: the caller promises the range.
    let url = String::from_utf8_lossy(unsafe { view(uptr, ulen) }).into_owned();
    // SAFETY: the caller promises `hlen` live `Header`s at `hptr`.
    let headers = unsafe { header_pairs(hptr, hlen) };
    // SAFETY: the caller promises `blen` readable bytes; a `[U8]`'s stride is
    // one, so the payload is the octets themselves.
    let body = unsafe { view(bptr, blen) }.to_vec();
    let call = NetLog { method: method as i8, url, headers, body };
    with(handle, (), |slot| {
        if let Slot::Net { calls, .. } = slot {
            calls.push(call);
        }
    });
}

/// The name/value pairs of a `[Header]` argument.
///
/// `Header` is two `Str`s end to end, which is [`BuriPair`]'s shape — the same
/// element [`list_of_pairs`] writes, read the other way.
///
/// # Safety
/// `ptr` is the payload of a live `[Header]` of `len` elements, or null with a
/// zero length.
unsafe fn header_pairs(ptr: *const u8, len: u64) -> Vec<(String, String)> {
    let stride = size_of::<BuriPair>();
    let mut out = Vec::new();
    if ptr.is_null() {
        return out;
    }
    for i in 0..len as usize {
        // SAFETY: the caller promises `len` elements at `ptr`.
        let element = unsafe { &*ptr.add(i.saturating_mul(stride)).cast::<BuriPair>() };
        // SAFETY: an element of a live list holds live `Str` views.
        out.push(unsafe {
            (element.key.as_str().into_owned(), element.value.as_str().into_owned())
        });
    }
    out
}

/// `netCalls(handle)` — every request through this network, in the order they
/// were answered.
///
/// By the handle and not by the `TestNet`: that value carries a responder as
/// well, and an argument crosses as its leaves, so `self` would arrive here as
/// a handle *and* a `{ code, env }` pair this side has no name for.
/// `TestNet.calls` is the Buri body that unwraps it.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_net_calls(handle: i64, out: *mut BuriList) {
    let calls = with(handle, Vec::new(), |slot| match slot {
        Slot::Net { calls, .. } => calls
            .iter()
            .map(|c| (c.method, c.url.clone(), c.headers.clone(), c.body.clone()))
            .collect(),
        _ => Vec::new(),
    });
    let value = list_of(
        &calls,
        |(method, url, headers, body): &(i8, String, Vec<(String, String)>, Vec<u8>)| {
            BuriNetCall {
                method: *method,
                url: str_of(url),
                headers: crate::value::list_of_headers(headers),
                body: list_of_bytes(body),
            }
        },
    );
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `spelled(body)` — the text these octets spell, replacing what is not UTF-8.
///
/// The decode `readFile` already does on the way out of a byte fixture, reached
/// by an `FsCall` constructor rather than by a filesystem: a test writing a call
/// down performs no effect and so has no context to decode with.
///
/// # Safety
/// `ptr` is readable for `len` bytes, or null with a zero length; `out` is
/// writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_spelled(
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises the range.
    let body = unsafe { view(ptr, len) };
    let value = str_of(&String::from_utf8_lossy(body));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

// ---------------------------------------------------------------------------
// The fault plan's promise
// ---------------------------------------------------------------------------
//
// The plan itself is a Buri value and is matched there, by the `Eq` the `Call`
// records derive: an `IoError` carries a `Str` on `.Other` and `lib.rs` §2.1
// cannot name an error variant that carries anything, so a plan the archive held
// could not hand its errors back. What is here is the half a program cannot
// keep — *which entries have fired* — because the block that would ask has
// returned by the time the answer matters.
//
// So `faults` installs a [`Slot::Plan`] and then tells this side, one entry at a
// time, what each fault would read like in a failure message. The pieces are the
// program's and the sentence is assembled here, which is the same division
// `note_failure` already makes: the compiler's `Show` renders the values and
// this file writes the line.

/// `IoError`'s variant names, in declaration order in `core/effect`, as a
/// failure message spells them.
///
/// The indices are `ioCode`'s, and the two lists are held together by
/// `conformance/lib/semantics/test/host_testing.buri`, which asserts the message
/// for a fault of each shape.
const IO_ERROR_NAMES: [&str; 7] = [
    ".NotFound",
    ".PermissionDenied",
    ".ReadOnly",
    ".AlreadyExists",
    ".NotADirectory",
    ".CrossDevice",
    ".Other",
];

/// `NetError`'s variant names, in declaration order — `netCode`'s indices.
const NET_ERROR_NAMES: [&str; 5] =
    [".Timeout", ".Refused", ".BadUrl", ".Transport", ".Aborted"];

/// One error, as a message names it: the variant, and the text it carries where
/// it carries any.
fn error_shown(names: &[&str], code: i64, payload: &str) -> String {
    let name = usize::try_from(code).ok().and_then(|i| names.get(i)).copied().unwrap_or("?");
    if payload.is_empty() {
        name.to_string()
    } else {
        format!("{name}(\"{payload}\")")
    }
}

/// Adds one entry to the plan the handle names, with only its call spelled.
///
/// The other half arrives in a second call, and the split is the frame-threaded
/// backend's: `stencil/abi.rs`'s `MAX_INT_ARGS` is ten and a `Str` is three of
/// them, so one row carrying a call *and* an error would be fifteen.
fn plan_push(handle: i64, shown: String) {
    let plan = slot_plan(handle);
    with(plan, (), |slot| {
        if let Slot::Plan { entries, .. } = slot {
            entries.push(PlanEntry { shown, fired: false });
        }
    });
}

/// `addFsFault(handle, name, path, body)` — the call half of one entry of a
/// filesystem's plan, as a failure message would spell it.
///
/// The call's three fields rather than an `FsCall`: this side does not read Buri
/// records, and `describeFsPlan` is already walking the plan to get here.
///
/// # Safety
/// Each pointer must address its byte length, or be null with a zero length.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_host_testing_add_fs_fault(
    handle: i64,
    _name_base: *mut u8,
    name: *const u8,
    name_len: u64,
    _path_base: *mut u8,
    path: *const u8,
    path_len: u64,
    _body_base: *mut u8,
    body: *const u8,
    body_len: u64,
) {
    // SAFETY: the caller promises each range.
    let (name, path, body) = unsafe {
        (
            String::from_utf8_lossy(view(name, name_len)).into_owned(),
            String::from_utf8_lossy(view(path, path_len)).into_owned(),
            String::from_utf8_lossy(view(body, body_len)).into_owned(),
        )
    };
    let call = if body.is_empty() {
        format!("{name}(\"{path}\")")
    } else {
        format!("{name}(\"{path}\", \"{body}\")")
    };
    plan_push(handle, call);
}

/// `addNetFault(handle, url)` — the call half of one entry of a network's plan.
///
/// The URL and not the whole request: matching is `NetCall`'s derived `Eq` and
/// reads every field of it, and a message that named every header would be a
/// paragraph where a reader wants a line.
///
/// # Safety
/// The pointer must address its byte length, or be null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_add_net_fault(
    handle: i64,
    _url_base: *mut u8,
    url: *const u8,
    url_len: u64,
) {
    // SAFETY: the caller promises the range.
    let url = unsafe { String::from_utf8_lossy(view(url, url_len)).into_owned() };
    plan_push(handle, format!("fetch(\"{url}\")"));
}

/// `faultFails(handle, nth, code, payload)` — the failure half of the entry the
/// call above just added, for whichever double added it.
///
/// One row for both, which is what the split turned out to be worth: a fault
/// fails the same way whatever it names, and the error's variant index is the
/// only thing that differs — `IoError`'s or `NetError`'s, told apart by the
/// slot the plan hangs from.
///
/// # Safety
/// The pointer must address its byte length, or be null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_fault_fails(
    handle: i64,
    nth: i64,
    code: i64,
    _payload_base: *mut u8,
    payload: *const u8,
    payload_len: u64,
) {
    // SAFETY: the caller promises the range.
    let payload = unsafe { String::from_utf8_lossy(view(payload, payload_len)).into_owned() };
    let names: &[&str] = match slot_kind(handle) {
        Kind::Net => &NET_ERROR_NAMES,
        Kind::Fs => &IO_ERROR_NAMES,
    };
    let error = error_shown(names, code, &payload);
    let plan = slot_plan(handle);
    with(plan, (), |slot| {
        if let Slot::Plan { entries, .. } = slot
            && let Some(entry) = entries.last_mut()
        {
            entry.shown.push_str(&format!(" fails {error}"));
            if nth != 0 {
                entry.shown.push_str(&format!(" on call {nth}"));
            }
        }
    });
}

/// Which double a handle names, which is which error enum its plan spells.
enum Kind {
    Fs,
    Net,
}

fn slot_kind(handle: i64) -> Kind {
    let table = lock();
    match usize::try_from(handle).ok().and_then(|i| table.get(i)) {
        Some(Slot::Net { .. }) => Kind::Net,
        _ => Kind::Fs,
    }
}

/// `noteFault(handle, index)` — the entry at `index` has fired.
///
/// Idempotent: a `fails` entry fires on every matching call and the promise is
/// kept by the first of them.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_note_fault(handle: i64, index: i64) {
    let plan = slot_plan(handle);
    with(plan, (), |slot| {
        if let Slot::Plan { entries, .. } = slot
            && let Some(entry) = usize::try_from(index).ok().and_then(|i| entries.get_mut(i))
        {
            entry.fired = true;
        }
    });
}

/// `noteFsCall(handle, name, path, body)` — one call on the path that never
/// reached the row that would have recorded it.
///
/// A call the plan failed is a call: the code under test asked the filesystem
/// for something and was answered. The eleven rows record through [`Recording`]
/// on their way out; this is the twelfth way in, and it is a plain push because
/// there is nothing to guard — the answer is already decided.
///
/// # Safety
/// Each pointer must address its byte length, or be null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_note_fs_call(
    handle: i64,
    _name_base: *mut u8,
    name: *const u8,
    name_len: u64,
    _path_base: *mut u8,
    path: *const u8,
    path_len: u64,
    _body_base: *mut u8,
    body: *const u8,
    body_len: u64,
) {
    // SAFETY: the caller promises each range.
    let (name, path, body) = unsafe {
        (
            String::from_utf8_lossy(view(name, name_len)).into_owned(),
            String::from_utf8_lossy(view(path, path_len)).into_owned(),
            String::from_utf8_lossy(view(body, body_len)).into_owned(),
        )
    };
    // The name is one of the eleven the constructors write, so it is matched
    // back onto the `&'static str` the log holds rather than leaked: a log
    // entry's name is not a string a program invented.
    let known = FS_CALL_NAMES.iter().find(|n| **n == name).copied().unwrap_or("");
    with(handle, (), |slot| {
        if let Slot::Fs { calls, .. } = slot {
            calls.push(FsLog { name: known, path, body });
        }
    });
}

/// The eleven names an `FsCall` can carry, which are the eleven methods of `Fs`.
const FS_CALL_NAMES: [&str; 11] = [
    "readFile",
    "writeFile",
    "fileExists",
    "readDir",
    "readFileBytes",
    "writeFileBytes",
    "appendFile",
    "renameFile",
    "removeFile",
    "makeDir",
    "syncFile",
];

// ---------------------------------------------------------------------------
// The runner's side of a native test binary
// ---------------------------------------------------------------------------
//
// `buri test` reports per test: which one failed, its message, and the two
// sides of the comparison. On JavaScript that report is assembled by
// `commands/test.rs` from the JSON array `runtime.js`'s `$run` writes, and the
// same function assembles the native one — so what a native binary owes the
// runner is the same *facts*, not a second format for them.
//
// A failed assertion is still an abort (SPEC 6.10: there is nothing to catch),
// so one process cannot report two failures. The runner runs the binary again
// from the block after the one that aborted, which is the sharding
// `commands/test.rs`'s header already permits — a suite's result may not depend
// on the order its blocks run in, nor on how many processes ran them.
//
// The protocol is three facts and nothing else:
//
//   * `BURI_TEST_FROM=<index>` — the block to start at. **Absent means nothing
//     is driving this process**: it runs every block, writes no record, and a
//     failure is the message on standard error and the exit status it always
//     was. A binary run by hand is unchanged by any of this.
//   * [`buri_rt_test_enter`] — before each block, answering whether to run it.
//   * one line on standard output when a block aborts, naming its index, the
//     message, and both rendered values where the assertion had them. A block
//     that returns writes nothing, so a passing suite pays one call per test
//     and no I/O at all.
//
// A test cannot reach real standard output — every effect it has is one the
// runner supplied, and `core/host` has no name inside a test source — so the
// line has the stream to itself.

/// The environment variable the runner starts a process with, holding the
/// index of the first block this process is to run.
const RESUME: &str = "BURI_TEST_FROM";

/// Where this process is in the suite, and what the assertion that ended it
/// had to say.
struct Runner {
    /// The block being run, or `-1` before the first — which is also what a
    /// process nothing is driving leaves it at.
    at: i64,
    /// The two sides of a failed comparison, rendered by the `Show` the derive
    /// pass generated for the type. `None` for a failure that has no pair:
    /// `assert.fail`, and every abort.
    shown: Option<(String, String)>,
}

static RUNNER: Mutex<Runner> = Mutex::new(Runner { at: -1, shown: None });

/// Lock, recovering from poisoning, for the reason [`lock`] gives.
fn runner() -> std::sync::MutexGuard<'static, Runner> {
    match RUNNER.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The index the runner asked this process to start at, or `None` when nothing
/// is driving it.
///
/// Read once: the environment cannot change under a process that has no way to
/// set one, and a failing suite reads this on the abort path where allocating
/// is the thing to avoid.
fn resume_at() -> Option<i64> {
    static ASKED: std::sync::OnceLock<Option<i64>> = std::sync::OnceLock::new();
    *ASKED.get_or_init(|| std::env::var(RESUME).ok().and_then(|v| v.parse().ok()))
}

/// Whether to run the `test` block at `index`, and a note of which one it is.
///
/// Answers 1 for every block where nothing is driving this process, so the
/// binary is the same program run by hand that it is under the runner.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_test_enter(index: i64) -> i32 {
    // Where the block's own slots begin. Unconditional, and before the two
    // early answers: a binary nothing is driving checks the post-condition too,
    // because it is the program's rule and not the runner's protocol.
    WATERMARK.store(lock().len(), std::sync::atomic::Ordering::Relaxed);
    // And this block has been run no times. `TestTasks.everyOrder` counts the
    // runs of one *body*, so the count belongs to the block and is reset where
    // the block is: `buri_rt_test_replay` advances it, and a block that never
    // schedules anything leaves it at the one run every block makes.
    *replay() = Replay { pass: 0, total: 1, note: None };
    let Some(from) = resume_at() else { return 1 };
    if index < from {
        return 0;
    }
    runner().at = index;
    1
}

/// The handle table's length when the current block started.
///
/// A `test` block's doubles are the slots installed after this point, which is
/// what makes [`buri_rt_test_leave`] a question about *this* block: the table
/// grows for the life of the process and the block before this one left its
/// filesystems in it.
static WATERMARK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The end of a `test` block: every fault the block planned has happened.
///
/// The other half of `core/host/testing`'s `faults`, and the half a program
/// cannot check for itself — the block has returned by the time the question can
/// be asked. `middle::monomorphize` emits this call after every test body, so it
/// runs on all three backends from one place; `runtime.js`'s `$test_leave` is
/// the same function for JavaScript, and `$run` marks the watermark there that
/// [`buri_rt_test_enter`] marks here.
///
/// A block that ended by aborting never reaches this, which is the right order
/// of report: a failed assertion is what went wrong, and an unused fault plan is
/// a consequence of stopping early rather than a second failure.
///
/// **One watermark per run of a block, and the reruns move it.**
/// `TestTasks.everyOrder` runs the body once per completion order, and every one
/// of those runs installs its own doubles; a plan the third run declared and
/// never reached is the third run's failure and not the block's. So this is
/// asked at the end of *every* run, and [`buri_rt_test_replay`] moves the
/// watermark up before the next one — which is the question this comment used to
/// leave open, answered in the direction that keeps each run's promise its own.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_test_leave(index: i64) {
    let unconsumed = unconsumed_since(WATERMARK.load(std::sync::atomic::Ordering::Relaxed));
    if unconsumed.is_empty() {
        return;
    }
    if resume_at().is_some() {
        runner().at = index;
    }
    let listed = unconsumed.join("; ");
    crate::abort::die(&[b"a fault was planned and never happened: ", listed.as_bytes()])
}

/// Every entry of every plan installed at or after `from` that nothing fired,
/// in the order the plans were installed and the entries were written.
///
/// A retired plan is skipped: `faults` replaces, and a promise that has been
/// replaced is not one to report. Separate from [`buri_rt_test_leave`] because
/// that function ends the process and this is the part with an answer to check.
fn unconsumed_since(from: usize) -> Vec<String> {
    let table = lock();
    let mut out: Vec<String> = Vec::new();
    for slot in table.iter().skip(from) {
        if let Slot::Plan { entries, retired: false } = slot {
            out.extend(entries.iter().filter(|e| !e.fired).map(|e| e.shown.clone()));
        }
    }
    out
}

/// A failed `assert` comparison, with both values already rendered.
///
/// The rendering is the *program*'s: `middle::derives` generates the `Show`
/// this backend calls, so the bytes here are the bytes `$show` produces for the
/// same value (VALUE-MODEL.md §12). Nothing in this file walks a type.
///
/// # Safety
/// Each pointer must address its byte length, or be null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_test_fail_compared(
    kind: *const u8,
    kind_len: u64,
    actual: *const u8,
    actual_len: u64,
    expected: *const u8,
    expected_len: u64,
) -> ! {
    // SAFETY: the caller promises each pointer addresses its length.
    let (kind, actual, expected) = unsafe {
        (view(kind, kind_len), view(actual, actual_len), view(expected, expected_len))
    };
    stash(actual, expected);
    crate::abort::die(&[b"assert.", kind, b" failed"])
}

/// `failExpected(kind, got)`: one rendered value, against the variant its
/// caller wanted.
///
/// The other side is the kind with an initial capital and a leading dot —
/// `.Ok`, `.Some` — which is `$testing_assert_failExpected`'s
/// `"." + kind[0].toUpperCase() + kind.slice(1)` and is the syntax the variant
/// is written in.
///
/// # Safety
/// Each pointer must address its byte length, or be null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_test_fail_expected(
    kind: *const u8,
    kind_len: u64,
    shown: *const u8,
    shown_len: u64,
) -> ! {
    // SAFETY: the caller promises each pointer addresses its length.
    let (kind, shown) = unsafe { (view(kind, kind_len), view(shown, shown_len)) };
    let mut want = String::with_capacity(kind.len() + 1);
    want.push('.');
    for (i, c) in String::from_utf8_lossy(kind).chars().enumerate() {
        if i == 0 {
            want.extend(c.to_uppercase());
        } else {
            want.push(c);
        }
    }
    stash(shown, want.as_bytes());
    crate::abort::die(&[b"assert.", kind, b" failed"])
}

/// Keeps both rendered sides for the line [`note_failure`] is about to write.
///
/// A separate step from writing it because the message is the abort's, and
/// every abort goes through one place.
fn stash(actual: &[u8], expected: &[u8]) {
    let (actual, expected) =
        (String::from_utf8_lossy(actual).into_owned(), String::from_utf8_lossy(expected).into_owned());
    runner().shown = Some((actual, expected));
}

/// The line an aborting block writes, from `abort::die` and from nowhere else.
///
/// Every way a `test` block can end other than by returning is an abort — a
/// failed assertion, a division by zero, an allocation past a budget — so one
/// call here attributes all of them, and the message the runner prints is the
/// one the process was going to print anyway.
pub(crate) fn note_failure(parts: &[&[u8]]) {
    if resume_at().is_none() {
        return;
    }
    // The guard is dropped before the write: `die` is not re-entered from here,
    // but a lock held across I/O is a lock held for no reason.
    let (at, shown) = {
        let mut r = runner();
        (r.at, r.shown.take())
    };
    if at < 0 {
        return;
    }
    let mut message = String::new();
    for part in parts {
        message.push_str(&String::from_utf8_lossy(part));
    }
    let mut line = format!("{{\"i\":{at},\"message\":");
    quote_into(&message, &mut line);
    if let Some((actual, expected)) = shown {
        line.push_str(",\"actual\":");
        quote_into(&actual, &mut line);
        line.push_str(",\"expected\":");
        quote_into(&expected, &mut line);
    }
    line.push_str("}\n");
    use std::io::Write;
    let stream = std::io::stdout();
    let mut stream = stream.lock();
    let _ = stream.write_all(line.as_bytes());
    let _ = stream.flush();
}

/// One JSON string literal, escaped the way `JSON.stringify` escapes it.
///
/// The same escapes, because the runner parses this line with the parser it
/// parses a JavaScript run's record with: a difference here would be a
/// difference in the report for a value holding a tab.
fn quote_into(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------
// `tasks()` — the order the work happens in
// ---------------------------------------------------------------------------
//
// The one double whose subject is scheduling rather than state.
// `Tasks.parallel` promises its results in the items' order and promises
// nothing about the order the work runs in; `TestTasks` makes that order a
// value a test writes down, and this file is where the choice is made.
//
// The walk is `cli/runtime/rt.rs`'s walk with a permutation in front of it and
// a log behind it, and it is the *same boundary* — the closure trampoline of
// [`crate::list::StepEntry`] — because a double that reached its steps some
// other way would be testing a different thing from the one that ships.
//
// A seed is the order's own number in the factorial number system, counted
// from zero over the orders in lexicographic order. That is what makes
// `everyOrder`'s `k`th run and `seed(k)` the same order, and it is what lets a
// failure report print one line that replays what failed.

/// Program order — the items' own.
const ORDER_PROGRAM: i64 = 0;
/// One seeded order per run of the body.
const ORDER_SEEDED: i64 = 1;
/// Every order, one run of the body each.
const ORDER_EVERY: i64 = 2;

/// The largest fan-out `everyOrder` will enumerate.
///
/// Six items are 720 runs of the block and seven are 5040. A test that wants a
/// wider fan-out wants `anyOrder`, and one that wanted every order of it wanted
/// something no machine is going to finish.
const EVERY_ORDER_CEILING: i64 = 6;

/// One planned failure, as the runner matches it: which task, which call of it,
/// and what the failure will say.
///
/// The whole fault, unlike `TestFs`' and `TestNet`'s, which keep the plan in the
/// program because an `IoError` cannot cross. A task's fault carries an index, a
/// count and a sentence — three things that cross — so the matching is here
/// beside the walk that needs it, and `Slot::Plan` holds the *promise* exactly as
/// it does for the other two.
struct TaskFault {
    index: i64,
    nth: i64,
    reason: String,
}

/// `tasks()` — program order, no plan, an empty log.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_tasks(out: *mut i64) {
    let handle = install(Slot::Tasks {
        mode: ORDER_PROGRAM,
        seed: -1,
        plan: -1,
        log: Vec::new(),
        faults: Vec::new(),
    });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// A **new** scheduler at that mode and seed, carrying this one's plan.
///
/// A fresh log, as every builder in this module answers one: the tasks a test
/// reads back are the tasks run through the value it put in its context. The
/// plan travels, exactly as `TestFs`' does through `files` — a builder is
/// configuration and not a write.
fn tasks_at(handle: i64, mode: i64, seed: i64) -> i64 {
    let (plan, faults) = with(handle, (-1, Vec::new()), |slot| match slot {
        Slot::Tasks { plan, faults, .. } => (
            *plan,
            faults
                .iter()
                .map(|f| TaskFault { index: f.index, nth: f.nth, reason: f.reason.clone() })
                .collect(),
        ),
        _ => (-1, Vec::new()),
    });
    install(Slot::Tasks { mode, seed, plan, log: Vec::new(), faults })
}

/// The mode and seed a scheduler handle names, or program order for a handle
/// naming nothing — [`with`]'s fallback rule.
fn tasks_ordering_of(handle: i64) -> (i64, i64) {
    let table = lock();
    match usize::try_from(handle).ok().and_then(|i| table.get(i)) {
        Some(Slot::Tasks { mode, seed, .. }) => (*mode, *seed),
        _ => (ORDER_PROGRAM, -1),
    }
}

/// `TestTasks::anyOrder` — one seeded order, at the default seed.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_tasks_any_order(handle: i64, out: *mut i64) {
    let next = tasks_at(handle, ORDER_SEEDED, -1);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(next) }
}

/// `TestTasks::everyOrder` — every order, one run of the body each.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_tasks_every_order(handle: i64, out: *mut i64) {
    let next = tasks_at(handle, ORDER_EVERY, -1);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(next) }
}

/// `TestTasks::seed` — one seeded order, at the seed the test named.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_tasks_seed(
    handle: i64,
    seed: i64,
    out: *mut i64,
) {
    let next = tasks_at(handle, ORDER_SEEDED, seed);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(next) }
}

/// `TestTasks::replan` — a **new** scheduler with a fresh, empty plan and this
/// one's ordering, retiring the plan it was using.
///
/// [`buri_rt_host_testing_fs_with_plan`]'s function at the third double, and its
/// reason read once more: `faults` replaces rather than composing, and a promise
/// that has been replaced is not one [`buri_rt_test_leave`] should report.
///
/// # Safety
/// `out` must be writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_tasks_replan(handle: i64, out: *mut i64) {
    let (mode, seed) = tasks_ordering_of(handle);
    retire(slot_plan(handle));
    let plan = install(Slot::Plan { entries: Vec::new(), retired: false });
    let next = install(Slot::Tasks { mode, seed, plan, log: Vec::new(), faults: Vec::new() });
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(next) }
}

/// `TestTasks::addFault(index, nth, reason)` — one entry of a scheduler's plan.
///
/// It lands in two places, which is one place more than the other two doubles
/// need: [`Slot::Plan`] holds what a failure message would say, exactly as
/// `addFsFault` leaves it, and the slot holds the three fields the walk matches
/// on — because for tasks the matching is here rather than in the program. The
/// two lists are appended together and are read by the same index, which is what
/// lets [`buri_rt_host_testing_note_fault`] mark the entry a walk fired.
///
/// # Safety
/// The pointer must address its byte length, or be null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_tasks_add_fault(
    handle: i64,
    index: i64,
    nth: i64,
    _reason_base: *mut u8,
    reason: *const u8,
    reason_len: u64,
) {
    // SAFETY: the caller promises the range.
    let reason = unsafe { String::from_utf8_lossy(view(reason, reason_len)).into_owned() };
    let mut shown = format!("task({index}) fails \"{reason}\"");
    if nth != 0 {
        shown.push_str(&format!(" on call {nth}"));
    }
    plan_push(handle, shown);
    with(handle, (), |slot| {
        if let Slot::Tasks { faults, .. } = slot {
            faults.push(TaskFault { index, nth, reason: reason.clone() });
        }
    });
}

/// `TestTasks::calls() -> [TaskCall]` — the tasks that completed, in the order
/// they completed.
///
/// `TaskCall` is one `Int`, so the element is a word and the list is the log
/// itself: there is no record to build the way `fsCalls` builds one.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_tasks_calls(handle: i64, out: *mut BuriList) {
    let log: Vec<i64> = with(handle, Vec::new(), |slot| match slot {
        Slot::Tasks { log, .. } => log.clone(),
        _ => Vec::new(),
    });
    let value = list_of(&log, |index: &i64| *index);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `TestTasks::runs()` — which run of the body this is, counted from one.
///
/// The receiver is read for nothing: the runs are the *block*'s, and a block
/// with two schedulers in it is still one block being run again.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_test_tasks_runs(_handle: i64) -> i64 {
    i64::try_from(replay().pass + 1).unwrap_or(i64::MAX)
}

/// `TestTasks::orders()` — how many runs of the body there will be.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_test_tasks_orders(_handle: i64) -> i64 {
    i64::try_from(replay().total).unwrap_or(i64::MAX)
}

/// `TestTasks.parallel(self, items, f) -> [B]` — `f` at every item, in the order
/// this scheduler chose, and the results in the items' order.
///
/// `buri_rt_host_tasks_parallel`'s walk with three things around it: the
/// permutation in front, the log behind, and the plan between them. The four
/// words after `len` are [`crate::list::StepEntry`]'s ABI, and the `[B]` block is
/// written at each item's **own** index rather than at the position the work was
/// done in — which is the whole of `Tasks.parallel`'s order promise, and is why
/// a scheduler is free to hand the work out in any order at all.
///
/// Every element of the result is written by exactly one step, because the order
/// is a permutation of `0..n`; a plan that fails one of them ends the process
/// before the block is handed back, so there is no arm here that leaves the
/// result half-filled and returns it.
///
/// # Safety
/// `ptr` covers `len * in_stride` bytes; `entry` is the thunk the backend
/// generated for this call and `state` the record it was generated against;
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_test_tasks_parallel(
    handle: i64,
    ptr: *const u8,
    len: u64,
    entry: crate::list::StepEntry,
    state: *mut u8,
    in_stride: u64,
    out_stride: u64,
    out: *mut BuriList,
) {
    let (n, from, to) = (len as usize, in_stride as usize, out_stride as usize);
    let result = crate::list::block(n, to);
    for index in task_order(handle, n) {
        let i = usize::try_from(index).unwrap_or(0);
        fail_planned_task(handle, index);
        // SAFETY: `i * from` is inside the `n`-element source the caller
        // promised, and `i * to` is inside the block just allocated. The thunk
        // is the one the backend generated for these two element types, and `i`
        // is a member of a permutation of `0..n`, so both are in range whatever
        // order the walk is in.
        unsafe {
            entry(
                state,
                index as u64,
                ptr.add(i.saturating_mul(from)),
                result.ptr.add(i.saturating_mul(to)),
            );
        }
        note_task(handle, index);
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(result) }
}

/// The order this run schedules `count` tasks in, and where `everyOrder` learns
/// how many runs there are to make.
///
/// The count is not known until a `parallel` says it, and this is the call that
/// says it. A second fan-out in the same run does not change the number of runs
/// — the first is the one being enumerated — and is scheduled by the same rank
/// at its own length.
fn task_order(handle: i64, count: usize) -> Vec<i64> {
    let (mode, seed) = tasks_ordering_of(handle);
    let orders = orders_of(count);
    if mode == ORDER_EVERY && count as i64 > EVERY_ORDER_CEILING {
        let named = format!("{count}");
        crate::abort::die(&[
            b"everyOrder over ",
            named.as_bytes(),
            b" tasks is more runs of this block than a suite can finish: ",
            b"a fan-out that wide is anyOrder's question",
        ]);
    }
    let rank = match mode {
        ORDER_EVERY => {
            let mut replay = replay();
            if replay.total <= 1 {
                replay.total = orders;
            }
            replay.pass
        }
        ORDER_SEEDED if seed < 0 => orders.saturating_sub(1),
        ORDER_SEEDED => (seed as u128) % orders.max(1),
        _ => 0,
    };
    let order = permutation(count, rank);
    note_order(mode, rank, &order);
    order
}

/// `n!`, saturating.
///
/// A `u128` holds `34!`. Past that the count is not a number any suite is going
/// to reach, and saturating is what keeps a seed legal at every length rather
/// than making one length a panic.
fn orders_of(n: usize) -> u128 {
    (1..=n as u128).try_fold(1u128, |acc, i| acc.checked_mul(i)).unwrap_or(u128::MAX)
}

/// The `rank`th permutation of `0..n`, counted from zero in lexicographic
/// order, wrapping past the last.
///
/// The factorial number system, read out digit by digit: the first digit is the
/// rank over `(n-1)!` and names which of the remaining items goes first. Written
/// this way rather than as a shuffle because a shuffle is not invertible — a
/// report that names a seed has to be a report a reader can replay, and this is
/// the mapping that makes `everyOrder`'s `k`th run and `seed(k)` the same
/// sentence.
fn permutation(n: usize, rank: u128) -> Vec<i64> {
    let mut pool: Vec<i64> = (0..n as i64).collect();
    let mut left = rank % orders_of(n).max(1);
    let mut out = Vec::with_capacity(n);
    for taken in 0..n {
        let remaining = orders_of(n - taken - 1);
        let digit = usize::try_from(left / remaining.max(1)).unwrap_or(0).min(pool.len() - 1);
        left %= remaining.max(1);
        out.push(pool.remove(digit));
    }
    out
}

/// One completed task, appended to the log.
fn note_task(handle: i64, index: i64) {
    with(handle, (), |slot| {
        if let Slot::Tasks { log, .. } = slot {
            log.push(index);
        }
    });
}

/// The plan's answer for the task about to run — and the end of the block where
/// there is one.
///
/// A task answers `B` and every `B` comes from the closure, so a double cannot
/// fail one and carry on: what a task that died would do to the run is end it,
/// and that is what this does, with the test's own words where a real one would
/// have had a stack. The tasks scheduled before it have already had their
/// effects, which is the state a reader of the report wants.
///
/// The count is of *matching* tasks — this index in the log — so
/// `failsOnCall(2, …)` names the second `parallel` that reached it, a walk
/// reaching each index once.
fn fail_planned_task(handle: i64, index: i64) -> Option<()> {
    let (at, reason) = chosen_task_fault(handle, index)?;
    buri_rt_host_testing_note_fault(handle, at as i64);
    let named = format!("task({index}): ");
    crate::abort::die(&[b"a task was failed by the plan: ", named.as_bytes(), reason.as_bytes()])
}

/// The first entry of the plan this task matches and its position satisfies —
/// which entry, and what the failure would say.
///
/// `chosenFsFault`'s rule, on this side rather than in the program: the first,
/// so a plan is read in the order it was written, and every entry sees the same
/// count of matching tasks. Separate from [`fail_planned_task`] because that
/// function ends the process and this is the half with an answer to check.
fn chosen_task_fault(handle: i64, index: i64) -> Option<(usize, String)> {
    let nth = with(handle, 0, |slot| match slot {
        Slot::Tasks { log, .. } => log.iter().filter(|entry| **entry == index).count() as i64,
        _ => 0,
    }) + 1;
    with(handle, None, |slot| match slot {
        Slot::Tasks { faults, .. } => faults
            .iter()
            .position(|f| f.index == index && (f.nth == 0 || f.nth == nth))
            .map(|at| (at, faults[at].reason.clone())),
        _ => None,
    })
}

/// Which run of the `test` body this is, and how many there are.
///
/// One per block rather than one per double, which is what `everyOrder` means:
/// the body is what re-runs, and every run of it builds its own doubles from the
/// same lines. [`buri_rt_test_enter`] resets it and [`buri_rt_test_replay`]
/// advances it.
struct Replay {
    /// The run being made, counted from zero. It is also the rank of the order
    /// this run schedules with, which is why `seed(runs() - 1)` replays it.
    pass: u128,
    /// How many runs there are. One until an `everyOrder` fan-out says
    /// otherwise.
    total: u128,
    /// The mode, the rank and the order of the first fan-out of this run — the
    /// datum a failure report names. **D6 renders it**; this file records it and
    /// [`task_order_note`] writes the sentence.
    note: Option<(i64, u128, Vec<i64>)>,
}

static REPLAY: Mutex<Replay> = Mutex::new(Replay { pass: 0, total: 1, note: None });

/// Lock, recovering from poisoning, for the reason [`lock`] gives.
fn replay() -> std::sync::MutexGuard<'static, Replay> {
    match REPLAY.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Records the order a fan-out was handed, for the report to name.
///
/// The **first** of the run, not the last: `everyOrder` enumerates the orders of
/// the first fan-out, so that is the one whose number a replay line has to
/// carry.
fn note_order(mode: i64, rank: u128, order: &[i64]) {
    let mut replay = replay();
    if replay.note.is_none() {
        replay.note = Some((mode, rank, order.to_vec()));
    }
}

/// What a failure report says about the order this run used, or `None` where
/// nothing ordered anything.
///
/// **The datum D6 renders.** The sentence is assembled here for
/// [`note_failure`]'s reason — the pieces are the runner's and this file writes
/// the line — and it is deliberately not printed by anything yet: the failure
/// report is one wire format shared by three backends, and widening it is a
/// slice of its own. A caller that has one prints it under the message.
pub fn task_order_note() -> Option<String> {
    let replay = replay();
    let (mode, rank, order) = replay.note.as_ref()?;
    if *mode == ORDER_PROGRAM {
        return None;
    }
    let listed: Vec<String> = order.iter().map(i64::to_string).collect();
    let run = if replay.total > 1 {
        format!(", order {} of {}", replay.pass + 1, replay.total)
    } else {
        String::new()
    };
    Some(format!(
        "the tasks completed in the order {}{run} — replay it with `tasks().seed({rank})`",
        listed.join(", ")
    ))
}

/// The end of one run of a `test` body: whether to run it again.
///
/// `middle::monomorphize` emits this after [`buri_rt_test_leave`], so the two
/// questions are asked in the order a reader wants — *did this run keep what it
/// promised*, and only then *is there another order to try*. Answering 1 makes
/// the body call itself, which is what "reruns the body" means on all three
/// backends from one lowering.
///
/// It moves the watermark up, which is the question
/// [`buri_rt_test_leave`]'s doc comment left for whoever landed `everyOrder`:
/// every run installs its own doubles, so a plan the *next* run declares is the
/// only plan the next `leave` is asking about. The runs before it have each been
/// asked already.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_test_replay(index: i64) -> u8 {
    let mut replay = replay();
    if replay.pass + 1 >= replay.total {
        return 0;
    }
    replay.pass += 1;
    replay.note = None;
    drop(replay);
    if resume_at().is_some() {
        runner().at = index;
    }
    WATERMARK.store(lock().len(), std::sync::atomic::Ordering::Relaxed);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

#[test]
fn probe_two_read_lines() {
    let lines = list_of_strs(&[String::from("one"), String::from("two")]);
    let mut handle = 0_i64;
    unsafe { buri_rt_testing_context_stdin(lines.ptr, lines.len, &raw mut handle) };
    let mut a = BuriStr::empty();
    let mut b = BuriStr::empty();
    let ra = unsafe { buri_rt_testing_context_test_stdin_read_line(handle, &raw mut a) };
    let rb = unsafe { buri_rt_testing_context_test_stdin_read_line(handle, &raw mut b) };
    assert_eq!(ra, BURI_OK);
    assert_eq!(rb, BURI_OK);
    assert_eq!(unsafe { a.as_str() }, "one");
    assert_eq!(unsafe { b.as_str() }, "two");
    let mut c = BuriStr::empty();
    assert_eq!(unsafe { buri_rt_testing_context_test_stdin_read_line(handle, &raw mut c) }, 0);
}

    #[test]
    fn a_transcript_accumulates_and_reads_back() {
        let mut handle = 0_i64;
        // SAFETY: `handle` is a live, aligned `i64`.
        unsafe { buri_rt_testing_context_capture_out(&raw mut handle) };
        append(handle, b"hello", true);
        append(handle, b"there", false);
        assert_eq!(transcript(handle), "hello\nthere");
    }

    #[test]
    fn two_sinks_are_independent() {
        let (mut a, mut b) = (0_i64, 0_i64);
        // SAFETY: both are live, aligned `i64`s.
        unsafe {
            buri_rt_testing_context_capture_out(&raw mut a);
            buri_rt_testing_context_capture_out(&raw mut b);
        }
        append(a, b"one", false);
        assert_eq!(transcript(a), "one");
        assert_eq!(transcript(b), "");
    }

    /// The generator is `$nextRand`'s, and a seed of zero is one — the two
    /// backends have to answer the same sequence, not merely a reproducible
    /// one.
    #[test]
    fn the_generator_is_the_javascript_one() {
        let mut handle = 0_i64;
        // SAFETY: `handle` is a live, aligned `i64`.
        unsafe { buri_rt_testing_context_rand_seed(0, &raw mut handle) };
        // xorshift32 from a state of 1: 270369, 67634689, 2647435461, …
        assert_eq!(next(handle), 270_369);
        assert_eq!(next(handle), 67_634_689);
        assert_eq!(next(handle), 2_647_435_461);
    }

    #[test]
    fn a_test_clock_moves_only_when_moved() {
        let mut handle = 0_i64;
        // SAFETY: `handle` is a live, aligned `i64`.
        unsafe { buri_rt_testing_context_clock_at(1_000, &raw mut handle) };
        assert_eq!(buri_rt_testing_context_test_clock_now_millis(handle), 1_000);
        buri_rt_testing_context_test_clock_sleep_millis(handle, 5);
        buri_rt_testing_context_test_clock_advance(handle, 5);
        assert_eq!(buri_rt_testing_context_test_clock_now_millis(handle), 1_010);
    }

    /// A handle nothing installed reads as empty rather than aborting, which is
    /// what keeps a toolchain bug from presenting as a program error.
    #[test]
    fn an_unknown_handle_is_inert() {
        assert_eq!(transcript(9_999_999), "");
        assert_eq!(buri_rt_testing_context_test_clock_now_millis(-1), 0);
    }

    /// The escapes `JSON.stringify` writes, because `commands/test.rs` reads
    /// this line with the parser it reads a JavaScript run's record with.
    #[test]
    fn a_record_field_is_escaped_as_json() {
        let mut out = String::new();
        quote_into("a\tb\nc\"d\\e", &mut out);
        assert_eq!(out, "\"a\\tb\\nc\\\"d\\\\e\"");
        out.clear();
        quote_into("\u{1}\u{8}\u{c}", &mut out);
        assert_eq!(out, "\"\\u0001\\b\\f\"");
        // Text outside ASCII is text, not an escape: `$show` puts a `Str`
        // through `JSON.stringify`, which leaves it alone.
        out.clear();
        quote_into("héllo 🎉", &mut out);
        assert_eq!(out, "\"héllo 🎉\"");
    }

    /// A process nothing is driving runs every block and writes no record: a
    /// binary run by hand is the program it always was.
    #[test]
    fn nothing_is_skipped_and_nothing_is_written_without_a_runner() {
        assert!(resume_at().is_none(), "the test process itself is not a test binary");
        assert_eq!(buri_rt_test_enter(0), 1);
        assert_eq!(buri_rt_test_enter(9), 1);
        note_failure(&[b"division by zero"]);
    }

    /// A fault plan, end to end on this side: what a message names it, what
    /// firing one does, and what a block would be told on the way out.
    ///
    /// [`buri_rt_test_leave`] itself ends the process, so what is checked here
    /// is [`unconsumed_since`] — the part with an answer. The call site is
    /// checked by `cli/tests/failing/unconsumed_fault/`, whose report the
    /// native backend and the JavaScript one both produce.
    #[test]
    fn a_plan_reports_the_entries_nothing_fired() {
        // `unconsumed_since` is a question about the whole table above a
        // watermark, so a case that installs a plan of its own on another thread
        // would answer it. There are two now.
        let _alone = one_runner_at_a_time();
        let from = lock().len();
        let files = buri_rt_host_testing_new_fs();
        let handle = buri_rt_host_testing_fs_with_plan(files);
        add_fault(handle, "readFile", "log", "", 0, 0, "");
        add_fault(handle, "writeFile", "log", "one", 2, 6, "disk full");
        assert_eq!(
            unconsumed_since(from),
            vec![
                String::from("readFile(\"log\") fails .NotFound"),
                String::from("writeFile(\"log\", \"one\") fails .Other(\"disk full\") on call 2"),
            ]
        );

        // Firing the first keeps its promise and leaves the second's standing.
        buri_rt_host_testing_note_fault(handle, 0);
        assert_eq!(
            unconsumed_since(from),
            vec![String::from(
                "writeFile(\"log\", \"one\") fails .Other(\"disk full\") on call 2"
            )]
        );

        // A builder carries the plan, so the promise is the same one.
        let derived = buri_rt_host_testing_fs_read_only(handle);
        buri_rt_host_testing_note_fault(derived, 1);
        assert!(unconsumed_since(from).is_empty());

        // And a plan `faults` replaced makes no claim at all.
        let again = buri_rt_host_testing_fs_with_plan(handle);
        add_fault(again, "readDir", "log", "", 0, 1, "");
        buri_rt_host_testing_fs_with_plan(again);
        assert!(unconsumed_since(from).is_empty());
    }

    /// The two rows one entry arrives in, spelled once for the test above.
    fn add_fault(handle: i64, name: &str, path: &str, body: &str, nth: i64, code: i64, payload: &str) {
        // SAFETY: every range is a live `&str`'s.
        unsafe {
            buri_rt_host_testing_add_fs_fault(
                handle,
                std::ptr::null_mut(),
                name.as_ptr(),
                name.len() as u64,
                std::ptr::null_mut(),
                path.as_ptr(),
                path.len() as u64,
                std::ptr::null_mut(),
                body.as_ptr(),
                body.len() as u64,
            );
            buri_rt_host_testing_fault_fails(
                handle,
                nth,
                code,
                std::ptr::null_mut(),
                payload.as_ptr(),
                payload.len() as u64,
            );
        }
    }

    /// A seed is the order's own number, and the numbering is lexicographic.
    ///
    /// This is the mapping the whole double rests on: it is what makes
    /// `everyOrder`'s `k`th run and `seed(k)` the same order, which is what lets
    /// a failure report print a line a reader can paste back. `runtime.js`'s
    /// `$tpermutation` is the same function for the other backend, and
    /// `conformance/lib/semantics/test/host_testing.buri` asserts three of these
    /// six through both of them.
    #[test]
    fn a_seed_names_the_order_it_replays() {
        let all: Vec<Vec<i64>> = (0..6).map(|rank| permutation(3, rank)).collect();
        assert_eq!(
            all,
            vec![
                vec![0, 1, 2],
                vec![0, 2, 1],
                vec![1, 0, 2],
                vec![1, 2, 0],
                vec![2, 0, 1],
                vec![2, 1, 0],
            ]
        );
        // Past the last one it wraps, so a seed derived from a program's content
        // is legal at every length.
        assert_eq!(permutation(3, 6), vec![0, 1, 2]);
        assert_eq!(permutation(3, 11), vec![2, 1, 0]);
        // The degenerate lengths, which every fan-out eventually is.
        assert_eq!(permutation(0, 4), Vec::<i64>::new());
        assert_eq!(permutation(1, 4), vec![0]);
        assert_eq!(orders_of(0), 1);
        assert_eq!(orders_of(6), 720);
    }

    /// The three modes, at the handles a program would reach them through.
    ///
    /// The default seed is the **reverse** of program order — the one order most
    /// likely to catch a program that depended on the order it was given — and a
    /// builder carries the plan while answering a new log.
    #[test]
    fn the_ordering_a_builder_answers() {
        let _alone = one_runner_at_a_time();
        let plain = tasks_handle();
        assert_eq!(task_order(plain, 3), vec![0, 1, 2]);

        let any = tasks_at(plain, ORDER_SEEDED, -1);
        assert_eq!(task_order(any, 3), vec![2, 1, 0]);

        let seeded = tasks_at(plain, ORDER_SEEDED, 3);
        assert_eq!(task_order(seeded, 3), vec![1, 2, 0]);

        // A builder answers a new handle: the receiver still schedules the way
        // it did.
        assert_eq!(task_order(plain, 3), vec![0, 1, 2]);
    }

    /// `everyOrder` counts the runs of the **block**, and the count is the one
    /// `buri_rt_test_replay` walks.
    ///
    /// The call site is `cli/tests/failing/every_order/`, whose first block is
    /// true on five runs and false on the sixth: a golden holding that failure is
    /// the body having run six times. What is checked here is the protocol under
    /// it — the total is the first fan-out's `n!`, the pass advances once per
    /// answer, and the last answer is no.
    #[test]
    fn the_body_is_run_once_per_order() {
        let _alone = one_runner_at_a_time();
        buri_rt_test_enter(0);
        let every = tasks_at(tasks_handle(), ORDER_EVERY, -1);

        // The first fan-out is what says how many orders there are.
        assert_eq!(buri_rt_host_testing_test_tasks_orders(every), 1);
        assert_eq!(task_order(every, 3), vec![0, 1, 2]);
        assert_eq!(buri_rt_host_testing_test_tasks_orders(every), 6);
        assert_eq!(buri_rt_host_testing_test_tasks_runs(every), 1);

        let mut seen = vec![vec![0, 1, 2]];
        for run in 2..=6 {
            assert_eq!(buri_rt_test_replay(0), 1, "run {run}");
            assert_eq!(buri_rt_host_testing_test_tasks_runs(every), run);
            seen.push(task_order(every, 3));
        }
        // Six runs, six orders, each of them once.
        assert_eq!(buri_rt_test_replay(0), 0);
        assert_eq!(seen, (0..6).map(|rank| permutation(3, rank)).collect::<Vec<_>>());

        // And a block that orders nothing is run once.
        buri_rt_test_enter(0);
        assert_eq!(buri_rt_test_replay(0), 0);
        assert_eq!(buri_rt_host_testing_test_tasks_runs(every), 1);
    }

    /// The datum a failure report names — **D6 renders it**, and this is the
    /// sentence it will render.
    ///
    /// Program order says nothing, because there is nothing about it to replay;
    /// a seeded order names its seed; and an `everyOrder` run names which of the
    /// orders it is as well, because the failing one is the last that ran.
    #[test]
    fn the_report_can_name_the_order_and_the_seed() {
        let _alone = one_runner_at_a_time();
        buri_rt_test_enter(0);
        let plain = tasks_handle();
        let _ = task_order(plain, 3);
        assert_eq!(task_order_note(), None);

        buri_rt_test_enter(0);
        let seeded = tasks_at(plain, ORDER_SEEDED, 3);
        let _ = task_order(seeded, 3);
        assert_eq!(
            task_order_note(),
            Some(String::from(
                "the tasks completed in the order 1, 2, 0 — replay it with `tasks().seed(3)`"
            ))
        );

        buri_rt_test_enter(0);
        let every = tasks_at(plain, ORDER_EVERY, -1);
        let _ = task_order(every, 3);
        assert_eq!(buri_rt_test_replay(0), 1);
        let _ = task_order(every, 3);
        assert_eq!(
            task_order_note(),
            Some(String::from(
                "the tasks completed in the order 0, 2, 1, order 2 of 6 \
                 — replay it with `tasks().seed(1)`"
            ))
        );
    }

    /// A fault is matched here rather than in the program, because a task's
    /// fault is three things that cross — and the promise is still
    /// [`Slot::Plan`]'s, entry for entry.
    ///
    /// The firing itself ends the process, so what is checked is the half with an
    /// answer: which entry a walk would choose. `cli/tests/failing/every_order/`
    /// is the call site, on both backends.
    #[test]
    fn a_fault_is_chosen_by_the_task_and_the_count() {
        let _alone = one_runner_at_a_time();
        let from = lock().len();
        let scheduler = replan_of(tasks_handle());
        add_task_fault(scheduler, 1, 0, "gone");
        add_task_fault(scheduler, 2, 2, "twice is too many");
        assert_eq!(
            unconsumed_since(from),
            vec![
                String::from("task(1) fails \"gone\""),
                String::from("task(2) fails \"twice is too many\" on call 2"),
            ]
        );

        // Task 0 is named by nothing; task 1 by an entry that fires every time;
        // task 2 by one that waits for the second fan-out to reach it.
        assert_eq!(chosen_task_fault(scheduler, 0), None);
        assert_eq!(chosen_task_fault(scheduler, 1), Some((0, String::from("gone"))));
        assert_eq!(chosen_task_fault(scheduler, 2), None);
        note_task(scheduler, 2);
        assert_eq!(
            chosen_task_fault(scheduler, 2),
            Some((1, String::from("twice is too many")))
        );

        // A builder carries the plan and its promise; a second `faults` retires
        // both.
        let carried = tasks_at(scheduler, ORDER_SEEDED, -1);
        assert_eq!(chosen_task_fault(carried, 1), Some((0, String::from("gone"))));
        replan_of(scheduler);
        assert!(unconsumed_since(from).is_empty());
    }

    /// Two things in this file are one per *process* rather than one per handle
    /// — the run counter `everyOrder` walks, and *"every plan installed since a
    /// watermark"* — and `cargo test` runs these cases on many threads at once.
    /// A case that asks about either takes this first. It is a lock over those
    /// cases and not over the crate: every other case here asks about a handle
    /// it minted itself.
    static ONE_RUNNER: Mutex<()> = Mutex::new(());

    fn one_runner_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        match ONE_RUNNER.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// `tasks()`, without the out-pointer the row is called through.
    fn tasks_handle() -> i64 {
        let mut handle = 0i64;
        // SAFETY: `handle` is a live, aligned `i64`.
        unsafe { buri_rt_host_testing_tasks(&raw mut handle) };
        handle
    }

    /// `TestTasks::replan`, likewise.
    fn replan_of(handle: i64) -> i64 {
        let mut next = 0i64;
        // SAFETY: `next` is a live, aligned `i64`.
        unsafe { buri_rt_host_testing_test_tasks_replan(handle, &raw mut next) };
        next
    }

    /// One entry of a plan, at the row's own signature.
    fn add_task_fault(handle: i64, index: i64, nth: i64, reason: &str) {
        // SAFETY: the range is a live `&str`'s.
        unsafe {
            buri_rt_host_testing_test_tasks_add_fault(
                handle,
                index,
                nth,
                std::ptr::null_mut(),
                reason.as_ptr(),
                reason.len() as u64,
            );
        }
    }

}
