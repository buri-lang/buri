//! The host capabilities: one native entry per `$host_*` in `backend/js/runtime.js`.
//!
//! `core/host` exports ten zero-sized implementations (`host.buri:18-87`) and
//! `core/cap` declares what each of them grants. Every method of every one of
//! them has a counterpart here, named by the rule in `lib.rs` §1:
//! `host.HostFs.readFile` is `buri_rt_host_fs_read_file`.
//!
//! ## Buffering, and why it matches JavaScript
//!
//! Standard output and standard error are both buffered, and both are flushed
//! together, which is precisely what `$host` does (`runtime.js:1224-1234`). It
//! is not only a speed decision: the interleaving a program observes between
//! the two streams is part of what a golden test captures, and two backends
//! that buffered differently would need two sets of expected output.
//!
//! `writeBytes` flushes the text buffer before writing octets, for the same
//! reason `$host_HostStdout_writeBytes` does — "the two orderings a program can
//! see are the one it wrote".
//!
//! ## Errors
//!
//! `IoError`'s variants, in declaration order in `cap.buri:82-89`, are the
//! integers this file returns: `NotFound` 0, `PermissionDenied` 1, `ReadOnly` 2,
//! `AlreadyExists` 3, `NotADirectory` 4, `Other(Str)` 5. `NetError`'s are in
//! `http.rs`. [`crate::BURI_OK`] is the success arm.
//!
//! The one place the two backends' *text* differs is `Other(Str)`: node spells
//! a stray errno `"ENOENT: no such file or directory, open 'x'"` and Rust
//! spells it `"No such file or directory (os error 2)"`. The five named
//! variants — which is every error a Buri program can match on — agree exactly,
//! and a program that matches `Other` and compares its string is reading an
//! operating system's diagnostic, not a language's.

use crate::http;
use crate::rng;
use crate::value::{list_of_bytes, list_of_strs, str_of, BuriList, BuriStr};
use crate::BURI_OK;
use std::io::{Read, Write};
use std::sync::Mutex;

/// Bytes buffered before a flush happens on its own. `$host` flushes after 64
/// pushed strings; a byte count is the same idea against a runtime that has
/// bytes rather than strings.
const FLUSH_AT: usize = 8 * 1024;

static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static ERR: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static ARGS: Mutex<Option<Vec<String>>> = Mutex::new(None);
static STDIN_LINES: Mutex<Option<(Vec<String>, usize)>> = Mutex::new(None);

/// Lock, recovering from poisoning.
///
/// The language has no threads (MEMORY.md §1), so a poisoned lock means a panic
/// already happened inside this runtime. Recovering the buffer is strictly
/// better than failing a second time on top of the first — in particular it is
/// what lets an abort still flush what the program had printed.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// Record `argc`/`argv` and install the panic hook. `lib.rs` §6.
///
/// The generated `main` calls this as its first statement. It is not required —
/// `env.arguments()` falls back to `std::env` — but it is preferred, because
/// `std::env::args` in a **staticlib** reaches the argument vector through a
/// platform startup hook (`.init_array` on Linux, `_NSGetArgv` on macOS) whose
/// survival across a `--gc-sections` link is not something this runtime should
/// have an opinion about. Being handed the vector removes the question.
///
/// # Safety
/// `argv` must be an array of `argc` NUL-terminated pointers, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_argv_init(argc: i32, argv: *const *const u8) {
    std::panic::set_hook(Box::new(|info| {
        let mut err = lock(&ERR);
        err.extend_from_slice(b"internal runtime error: ");
        err.extend_from_slice(info.to_string().as_bytes());
        err.push(b'\n');
        drop(err);
        buri_rt_flush();
    }));

    if argv.is_null() || argc <= 0 {
        return;
    }
    let mut args = Vec::new();
    for i in 0..argc as isize {
        // SAFETY: the caller promises `argc` readable pointers at `argv`.
        let p = unsafe { *argv.offset(i) };
        if p.is_null() {
            continue;
        }
        let mut n = 0_usize;
        // SAFETY: the caller promises a NUL-terminated string at `p`.
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: `n` bytes precede the NUL.
        let bytes = unsafe { std::slice::from_raw_parts(p, n) };
        args.push(String::from_utf8_lossy(bytes).into_owned());
    }
    // `process.argv.slice(2)` on JavaScript is the arguments *after* the script,
    // so the program's own name is dropped on both backends.
    if !args.is_empty() {
        args.remove(0);
    }
    *lock(&ARGS) = Some(args);
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Write both buffers through, output first. `lib.rs` §6: the generated entry
/// point calls this before every return path, and the abort paths call it for
/// themselves.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_flush() {
    let mut out = lock(&OUT);
    if !out.is_empty() {
        let stream = std::io::stdout();
        let mut stream = stream.lock();
        let _ = stream.write_all(&out);
        let _ = stream.flush();
        out.clear();
    }
    drop(out);

    let mut err = lock(&ERR);
    if !err.is_empty() {
        let stream = std::io::stderr();
        let mut stream = stream.lock();
        let _ = stream.write_all(&err);
        let _ = stream.flush();
        err.clear();
    }
}

fn push(buffer: &Mutex<Vec<u8>>, bytes: &[u8], newline: bool) {
    let mut buf = lock(buffer);
    buf.extend_from_slice(bytes);
    if newline {
        buf.push(b'\n');
    }
    let full = buf.len() >= FLUSH_AT;
    drop(buf);
    if full {
        buri_rt_flush();
    }
}

/// Borrow a `Str` argument's bytes. `lib.rs` §2 rule 1 flattens it to three
/// parameters, and `base` is unused because §3 says a parameter is borrowed —
/// the caller's reference keeps the bytes alive for the whole call.
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

/// # Safety
/// `ptr`/`len` must describe a live `Str` view.
unsafe fn text(ptr: *const u8, len: u64) -> String {
    // SAFETY: forwarded.
    String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned()
}

macro_rules! writer {
    ($name:ident, $buffer:ident, $newline:expr) => {
        /// # Safety
        /// The three parameters must describe a live `Str` view.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(_base: *mut u8, ptr: *const u8, len: u64) {
            // SAFETY: forwarded to the caller.
            push(&$buffer, unsafe { view(ptr, len) }, $newline)
        }
    };
}

writer!(buri_rt_host_stdout_print, OUT, false);
writer!(buri_rt_host_stdout_println, OUT, true);
writer!(buri_rt_host_stderr_eprint, ERR, false);
writer!(buri_rt_host_stderr_eprintln, ERR, true);

/// `Stdout::writeBytes` — octets, written through unchanged.
///
/// The buffered text stream is flushed first, so the two orderings a program
/// can see are the one it wrote.
///
/// # Safety
/// `ptr` must be readable for `len` bytes, or null with `len == 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_stdout_write_bytes(ptr: *const u8, len: u64) {
    buri_rt_flush();
    if ptr.is_null() || len == 0 {
        return;
    }
    // SAFETY: the caller promises `len` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let stream = std::io::stdout();
    let mut stream = stream.lock();
    let _ = stream.write_all(bytes);
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// `Stdin::readLine` — `.Some(line)` or `.None` at end of input.
///
/// Standard input is read to its end on the first call and split, which is what
/// `$host_HostStdin_readLine` does (`runtime.js:1300-1315`): a program using
/// `readLine` cannot answer before the other side has finished speaking, and
/// `readBytes` is the operation for a program that must.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_stdin_read_line(out: *mut BuriStr) -> i32 {
    let mut state = lock(&STDIN_LINES);
    let (lines, at) = state.get_or_insert_with(|| {
        let mut raw = Vec::new();
        let _ = std::io::stdin().lock().read_to_end(&mut raw);
        let text = String::from_utf8_lossy(&raw);
        let mut lines: Vec<String> = if text.is_empty() {
            Vec::new()
        } else {
            text.split('\n').map(str::to_string).collect()
        };
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        (lines, 0)
    });
    let Some(line) = lines.get(*at) else { return 0 };
    *at += 1;
    let value = str_of(line);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// `Stdin::readBytes` — exactly `n` octets, or fewer at end of input, and
/// `.None` when there were none at all.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_stdin_read_bytes(n: i64, out: *mut BuriList) -> i32 {
    if n <= 0 {
        // SAFETY: the caller promises a writable, aligned destination.
        unsafe { out.write(list_of_bytes(&[])) };
        return BURI_OK;
    }
    let mut buf = vec![0_u8; n as usize];
    let mut got = 0_usize;
    let stream = std::io::stdin();
    let mut stream = stream.lock();
    while got < buf.len() {
        // `read` on a pipe returns what is available rather than what was
        // asked for, so this loops.
        match stream.read(buf.get_mut(got..).unwrap_or(&mut [])) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if got == 0 {
        return 0;
    }
    let value = list_of_bytes(buf.get(..got).unwrap_or(&[]));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) };
    BURI_OK
}

// ---------------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------------

/// `IoError`'s variant index, and the `Other(Str)` payload where there is one.
fn io_error(e: &std::io::Error) -> (i32, String) {
    match e.kind() {
        std::io::ErrorKind::NotFound => (0, String::new()),
        std::io::ErrorKind::PermissionDenied => (1, String::new()),
        std::io::ErrorKind::ReadOnlyFilesystem => (2, String::new()),
        std::io::ErrorKind::AlreadyExists => (3, String::new()),
        std::io::ErrorKind::NotADirectory => (4, String::new()),
        _ => (5, e.to_string()),
    }
}

/// Write the error arm and return its tag.
///
/// # Safety
/// `out_err` must be writable and aligned for a [`BuriStr`].
unsafe fn fail(e: &std::io::Error, out_err: *mut BuriStr) -> i32 {
    let (tag, message) = io_error(e);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out_err.write(str_of(&message)) };
    tag
}

/// `Fs::readFile` — `Result<Str, IoError>`.
///
/// Invalid UTF-8 becomes U+FFFD rather than an error, because
/// `readFileSync(p, "utf8")` does the same and a `Str` that could hold invalid
/// UTF-8 would make `chars()` fallible on both backends.
///
/// # Safety
/// The path must be a live `Str` view; both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_read_file(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_ok: *mut BuriStr,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    match std::fs::read(&path) {
        Ok(bytes) => {
            let value = str_of(&String::from_utf8_lossy(&bytes));
            // SAFETY: the caller promises a writable destination.
            unsafe { out_ok.write(value) };
            BURI_OK
        }
        // SAFETY: as above.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::writeFile` — `Result<(), IoError>`.
///
/// # Safety
/// Both `Str` views must be live; `out_err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_write_file(
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
    _bbase: *mut u8,
    bptr: *const u8,
    blen: u64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(pptr, plen) };
    // SAFETY: forwarded.
    let body = unsafe { view(bptr, blen) };
    match std::fs::write(&path, body) {
        Ok(()) => BURI_OK,
        // SAFETY: the caller promises a writable destination.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::fileExists` — `Bool`, and never an error: an unreadable parent
/// directory answers `false`, as `existsSync` does.
///
/// # Safety
/// The path must be a live `Str` view.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_file_exists(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> u8 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    u8::from(std::path::Path::new(&path).exists())
}

/// `Fs::readDir` — `Result<[Str], IoError>`, of entry *names*.
///
/// Names rather than paths, and in the order the operating system reports them,
/// because that is what `readdirSync` returns. Sorting here would be a
/// divergence from the JavaScript backend rather than a determinism win: both
/// backends call the same `readdir` on the same filesystem.
///
/// # Safety
/// The path must be a live `Str` view; both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_read_dir(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_ok: *mut BuriList,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        // SAFETY: the caller promises a writable destination.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    let mut names = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
            // SAFETY: as above.
            Err(e) => return unsafe { fail(&e, out_err) },
        }
    }
    let value = list_of_strs(&names);
    // SAFETY: the caller promises a writable destination.
    unsafe { out_ok.write(value) };
    BURI_OK
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// `Net::fetch` — `Result<NetResponse, NetError>`.
///
/// `NetResponse` is `{ status: Int, body: Str }` (`cap.buri:98-101`) and its two
/// fields leave through two out-pointers, per `lib.rs` §2 rule 2. On the error
/// arm the tag is `NetError`'s variant index and `out_body` carries the payload
/// of the two variants that have one — `BadUrl(Str)` and `Transport(Str)` —
/// and the empty string for the two that do not.
///
/// `http://` only; see `http.rs` for why, and for what would change it.
///
/// # Safety
/// The three `Str` views must be live; both out-pointers writable and aligned.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_host_net_fetch(
    _mbase: *mut u8,
    mptr: *const u8,
    mlen: u64,
    _ubase: *mut u8,
    uptr: *const u8,
    ulen: u64,
    _bbase: *mut u8,
    bptr: *const u8,
    blen: u64,
    out_status: *mut i64,
    out_body: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let (method, url, body) =
        unsafe { (text(mptr, mlen), text(uptr, ulen), text(bptr, blen)) };
    match http::fetch(&method, &url, &body) {
        Ok((status, text)) => {
            let value = str_of(&text);
            // SAFETY: the caller promises writable destinations.
            unsafe {
                out_status.write(status);
                out_body.write(value);
            }
            BURI_OK
        }
        Err(e) => {
            let message = str_of(e.message());
            // SAFETY: as above.
            unsafe { out_body.write(message) };
            e.tag()
        }
    }
}

// ---------------------------------------------------------------------------
// Clock, randomness, environment, process
// ---------------------------------------------------------------------------

/// `Clock::nowMillis` — milliseconds since the Unix epoch, as `Date.now()` is.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_clock_now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `Clock::sleepMillis`. A negative or zero duration returns immediately.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_clock_sleep_millis(millis: i64) {
    if millis > 0 {
        std::thread::sleep(std::time::Duration::from_millis(millis as u64));
    }
}

/// `Rand::nextInt` — uniform in `lo ..< hi`.
///
/// An empty range aborts with `random range is empty`, byte for byte what
/// `runtime.js:1418` says, and pinned by `cli/tests/crash/random_range_empty`
/// and `random_range_inverted`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_rand_next_int(lo: i64, hi: i64) -> i64 {
    if hi <= lo {
        crate::buri_rt_abort_random_range();
    }
    rng::int_in(lo, hi)
}

/// `Rand::nextFloat` — uniform in `[0, 1)`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_rand_next_float() -> f64 {
    rng::float()
}

/// `Env::variable` — `.Some(value)` or `.None`.
///
/// # Safety
/// The name must be a live `Str` view; `out` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_env_variable(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let name = unsafe { text(ptr, len) };
    let Some(value) = std::env::var_os(&name) else { return 0 };
    let value = str_of(&value.to_string_lossy());
    // SAFETY: the caller promises a writable destination.
    unsafe { out.write(value) };
    BURI_OK
}

/// `Env::arguments` — the program's own arguments, without its name.
///
/// From [`buri_rt_argv_init`] where the entry point supplied them, and from
/// `std::env` where it did not.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_env_arguments(out: *mut BuriList) {
    let recorded = lock(&ARGS).clone();
    let args = recorded.unwrap_or_else(|| {
        std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect()
    });
    let value = list_of_strs(&args);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `Proc::exitWith`. Flushes first, and does not return.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_proc_exit_with(code: i64) -> ! {
    buri_rt_flush();
    std::process::exit(code as i32)
}
