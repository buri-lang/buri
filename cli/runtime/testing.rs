//! `core/testing/context` — the test runner's platform, natively — and the
//! runner's own protocol, at the bottom of this file.
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
//! ## `MemFs`'s four methods, and the one divergence they make readable
//!
//! Three of them answer a `Result<T, IoError>`, which was the shape neither
//! native backend had a `Ret` for and the reason all four were held back. It
//! has one now — `lib.rs` §2.1 — and the four are here.
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
    /// `TestStdin` — the lines, how many have been read, and the octets where
    /// it was built by `stdinBytes` rather than by `stdin`.
    Stdin { lines: Vec<String>, at: usize, bytes: Option<Vec<u8>> },
    /// `MemFs` — the entries, in insertion order.
    ///
    /// A `Vec` of pairs rather than a map, because the JavaScript side is an
    /// object and `readDir` reads its keys: a map would be a second ordering to
    /// reconcile, and there is no fixture large enough for the lookup cost to
    /// be the interesting number.
    Files(Vec<(String, String)>),
    /// `TestClock` — the current instant, in milliseconds.
    Clock(i64),
    /// `TestRand` — the xorshift32 state.
    Rand(u32),
    /// `TestEnv`.
    Env { vars: Vec<(String, String)>, args: Vec<String> },
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
    let handle = install(Slot::Stdin { lines, at: 0, bytes: None });
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
    let handle = install(Slot::Stdin { lines: Vec::new(), at: 0, bytes: Some(bytes) });
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
        Slot::Stdin { lines, at, bytes } if bytes.is_none() => {
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

/// `IoError.NotFound` — variant `0` of the enum `core/effect` declares, which is
/// the only error any of the four below produces.
///
/// `runtime.js`'s `$testing_context_MemFs_readFile` answers `$err([0])` and
/// nothing else; `writeFile` and `readDir` never fail at all. Named here rather
/// than written as a literal `0` because the number is `lib.rs` §2.1's "the
/// error variant's index in declaration order" and not an ordinary constant —
/// a reader who reorders `IoError` has to find this line.
const IO_NOT_FOUND: i32 = 0;

/// The entries a `MemFs` handle names, or an empty filesystem where the handle
/// names something else.
///
/// A handle that names no `Files` slot cannot arise from a program — `data()`
/// and `files()` are the only two constructors of the type — so the fallback is
/// "empty" rather than an abort, per [`with`]'s rule.
fn fs_read(handle: i64, path: &str) -> Option<String> {
    with(handle, None, |slot| match slot {
        Slot::Files(entries) => {
            entries.iter().find(|(k, _)| k == path).map(|(_, v)| v.clone())
        }
        _ => None,
    })
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
    let handle = install(Slot::Files(Vec::new()));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `files(entries)` — in-memory, containing exactly these.
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
    let handle = install(Slot::Files(entries));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) }
}

/// `MemFs.readFile(self, path) -> Result<Str, IoError>` — `lib.rs` §2.1's
/// shape, and the first entry in this archive to use it.
///
/// `$testing_context_MemFs_readFile` is `p in f ? $ok(f[p]) : $err([0])`, so
/// there is exactly one failure and it is `NotFound`. No path normalisation:
/// the key is the string a fixture wrote, compared as bytes, which is what
/// `p in f` is.
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
    let value = str_of(&body);
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
        (
            String::from_utf8_lossy(view(pptr, plen)).into_owned(),
            String::from_utf8_lossy(view(bptr, blen)).into_owned(),
        )
    };
    with(handle, (), |slot| {
        if let Slot::Files(entries) = slot {
            match entries.iter_mut().find(|(k, _)| *k == path) {
                Some(entry) => entry.1 = body,
                None => entries.push((path, body)),
            }
        }
    });
    BURI_OK
}

/// `MemFs.fileExists(self, path) -> Bool` — a plain scalar, and the one of the
/// four that was never about `Result`.
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
    u8::from(fs_read(handle, &path).is_some())
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
        if let Slot::Files(entries) = slot {
            for (key, _) in entries.iter() {
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
    let Some(from) = resume_at() else { return 1 };
    if index < from {
        return 0;
    }
    runner().at = index;
    1
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
}
