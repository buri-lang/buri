//! The host capabilities: one native entry per `$host_*` in `backend/js/runtime.js`.
//!
//! `core/host` exports one zero-sized implementation per effect a platform can
//! grant, and `core/effect` declares what each of them grants. Every method of
//! every one a *native* platform grants has a counterpart here, named by the
//! rule in `lib.rs` §1: `host.HostFs.readFile` is `buri_rt_host_fs_read_file`.
//!
//! Five of the implementations have no counterpart *here*, and not one of them
//! is an omission. `HostUi` and `HostWatch` drive a document, and a native
//! binary has none, so those two have no native counterpart anywhere. The other
//! three are implemented natively and elsewhere in this crate, divided by what
//! they need rather than by what they are: `HostTasks` is in `rt.rs`, beside
//! the carrier pool `parallel` fans out onto, and `HostListen` and
//! `HostSockets` are in `net.rs` because they are the networking half — a bound
//! listener, an accepted connection, a deadline on every read and every write —
//! so they live with the reactor and the TLS stack, behind feature `net`, where
//! a toolchain built without one refuses the key with a sentence instead of
//! leaving a missing symbol for `cc`. This file is the synchronous world: a
//! syscall, a buffer, and a return.
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
//! `IoError`'s variants, in declaration order in `core/effect`, are the
//! integers this file returns: `NotFound` 0, `PermissionDenied` 1, `ReadOnly` 2,
//! `AlreadyExists` 3, `NotADirectory` 4, `CrossDevice` 5, `Other(Str)` 6.
//! `NetError`'s are in `http.rs`. [`crate::BURI_OK`] is the success arm.
//!
//! The one place the two backends' *text* differs is `Other(Str)`: node spells
//! a stray errno `"ENOENT: no such file or directory, open 'x'"` and Rust
//! spells it `"No such file or directory (os error 2)"`. The five named
//! variants — which is every error a Buri program can match on — agree exactly,
//! and a program that matches `Other` and compares its string is reading an
//! operating system's diagnostic, not a language's.

use crate::http;
use crate::rng;
use crate::value::{list_of_bytes, list_of_headers, list_of_strs, str_of, BuriList, BuriStr};
use crate::BURI_OK;
use std::io::{Read, Seek, Write};
use std::sync::Mutex;

/// Bytes buffered before a flush happens on its own. `$host` flushes after 64
/// pushed strings; a byte count is the same idea against a runtime that has
/// bytes rather than strings.
const FLUSH_AT: usize = 8 * 1024;

static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static ERR: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static ARGS: Mutex<Option<Vec<String>>> = Mutex::new(None);
static STDIN_LINES: Mutex<Option<(Vec<String>, usize)>> = Mutex::new(None);

/// The first stream failure nobody has been told about yet.
///
/// Output is buffered, so the write a program *calls* and the write the
/// platform *performs* are two different moments: `println` fills a buffer, and
/// only a full buffer — or the exit path — reaches the descriptor. A failure
/// discovered then belongs to some earlier print, and the honest thing a
/// buffered stream can promise is that it reaches the next caller rather than
/// being dropped. So the failure is held here and answered by the next write,
/// which is what makes `Result<(), IoError>` a claim the runtime can keep.
///
/// First rather than last: a closed pipe fails every write after the first, and
/// the one worth reporting is the one that says what went wrong before the
/// program was writing into nothing.
static PENDING: Mutex<Option<std::io::Error>> = Mutex::new(None);

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
/// `env.args(ctx)` falls back to `std::env` — but it is preferred, because
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
        note(stream.write_all(&out).and_then(|()| stream.flush()));
        out.clear();
    }
    drop(out);

    let mut err = lock(&ERR);
    if !err.is_empty() {
        let stream = std::io::stderr();
        let mut stream = stream.lock();
        note(stream.write_all(&err).and_then(|()| stream.flush()));
        err.clear();
    }
}

/// Remember a stream failure for the next writer to answer with. The first one
/// wins; see [`PENDING`].
fn note(r: std::io::Result<()>) {
    let Err(e) = r else { return };
    let mut slot = lock(&PENDING);
    if slot.is_none() {
        *slot = Some(e);
    }
}

/// The `Result` arm this write answers, and the point at which a held failure
/// stops being held.
///
/// **The variant and not the sentence**, which is `lib.rs` §2.1's message shape
/// declined for exactly these five entries. The pointer it would take is an
/// address *into the caller's destination*, so a function that prints would
/// stop being able to keep its `Result` in registers — `native/llvm.rs`'s
/// `a_hot_function_has_no_allocas` is what measures that, and printing is the
/// path it is measuring. What is given up is the text on an unclassified stream
/// failure: an `EPIPE` is `.Other("")` here and `.Other("EPIPE: …")` on the
/// JavaScript backend, while `PermissionDenied`, `ReadOnly` and the rest are the
/// variant they always were. A print's actionable half is which failure it was;
/// `Fs` keeps its message because `ENOTEMPTY` and `EISDIR` have no variant at
/// all, and `backend/runtime_table.rs`'s `Ret::ResMsg` is where the two are
/// told apart.
fn reported() -> i32 {
    let taken = lock(&PENDING).take();
    match taken {
        None => BURI_OK,
        Some(e) => io_error(&e).0,
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
pub(crate) unsafe fn text(ptr: *const u8, len: u64) -> String {
    // SAFETY: forwarded.
    String::from_utf8_lossy(unsafe { view(ptr, len) }).into_owned()
}

/// The four buffered text writers, which differ only in stream and newline.
///
/// Each answers `Result<(), IoError>` — `Ret::Res` with **both** out-pointers
/// omitted: `()` occupies no bytes, and these five are the entries
/// `backend/runtime_table.rs` deliberately does not give the message shape to
/// ([`reported`] says why). So the C signature is the `Str` view and an `i32`
/// discriminant, and nothing else. The value it answers is whatever [`PENDING`]
/// holds: a write that only filled a buffer has nothing to report, and one that
/// triggered a flush reports what the flush found.
macro_rules! writer {
    ($name:ident, $buffer:ident, $newline:expr) => {
        /// # Safety
        /// The three view parameters must describe a live `Str`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(_base: *mut u8, ptr: *const u8, len: u64) -> i32 {
            // SAFETY: forwarded to the caller.
            push(&$buffer, unsafe { view(ptr, len) }, $newline);
            reported()
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
/// Unbuffered, so unlike the four above this one answers its *own* failure
/// rather than a held one — and reports a held one first, because that failure
/// is older.
///
/// # Safety
/// `ptr` must be readable for `len` bytes, or null with `len == 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_stdout_write_bytes(ptr: *const u8, len: u64) -> i32 {
    buri_rt_flush();
    if !ptr.is_null() && len != 0 {
        // SAFETY: the caller promises `len` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
        let stream = std::io::stdout();
        let mut stream = stream.lock();
        note(stream.write_all(bytes).and_then(|()| stream.flush()));
    }
    reported()
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
        std::io::ErrorKind::CrossesDevices => (5, String::new()),
        _ => (6, e.to_string()),
    }
}

/// Write the error arm and return its tag.
///
/// This is `lib.rs` §2.1's **message** shape, and it is the whole of the
/// runtime's side of it: the tag names a variant of `IoError`, and the `Str` is
/// what `.Other` carries — empty for the six variants that carry nothing, which
/// is what the shape asks of an entry whose discriminant names a payload-less
/// one. The caller has zeroed those bytes already
/// (`backend/runtime_native.rs`'s `error_message_offset`), so the empty write
/// is a restatement rather than a requirement; what matters is that a
/// classified failure never leaves a message from a previous call behind.
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

/// `Fs::readFileBytes` — `Result<[U8], IoError>`, the octets unchanged.
///
/// The difference from [`buri_rt_host_fs_read_file`] is the decoding step it
/// does not do: a file that is not text comes back as it is on disk.
///
/// # Safety
/// The path must be a live `Str` view; both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_read_file_bytes(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_ok: *mut BuriList,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    match std::fs::read(&path) {
        Ok(bytes) => {
            let value = list_of_bytes(&bytes);
            // SAFETY: the caller promises a writable destination.
            unsafe { out_ok.write(value) };
            BURI_OK
        }
        // SAFETY: as above.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::writeFileBytes` — `Result<(), IoError>`. Truncates, or creates.
///
/// # Safety
/// The path must be a live `Str` view and `bptr`/`blen` a readable range;
/// `out_err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_write_file_bytes(
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
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

/// `Fs::appendFile` — `Result<(), IoError>`. Creates the file when it is absent.
///
/// `O_APPEND`, so the position is taken and the octets written as one
/// operation: two writers appending to one log interleave records rather than
/// overwriting each other's. `appendFileSync` opens with the same flag.
///
/// # Safety
/// As [`buri_rt_host_fs_write_file_bytes`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_append_file(
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
    bptr: *const u8,
    blen: u64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(pptr, plen) };
    // SAFETY: forwarded.
    let body = unsafe { view(bptr, blen) };
    let opened = std::fs::OpenOptions::new().create(true).append(true).open(&path);
    let mut file = match opened {
        Ok(file) => file,
        // SAFETY: the caller promises a writable destination.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    match file.write_all(body) {
        Ok(()) => BURI_OK,
        // SAFETY: as above.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::renameFile` — `Result<(), IoError>`, replacing `to` atomically.
///
/// `rename(2)`, whose atomicity is the whole reason "write a temporary, then
/// rename it over the real one" is a crash-safe checkpoint. Across two
/// filesystems there is no such operation and the kernel says `EXDEV`, which
/// [`io_error`] names `CrossDevice` rather than folding into `Other`.
///
/// # Safety
/// Both paths must be live `Str` views; `out_err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_rename_file(
    _fbase: *mut u8,
    fptr: *const u8,
    flen: u64,
    _tbase: *mut u8,
    tptr: *const u8,
    tlen: u64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let (from, to) = unsafe { (text(fptr, flen), text(tptr, tlen)) };
    match std::fs::rename(&from, &to) {
        Ok(()) => BURI_OK,
        // SAFETY: the caller promises a writable destination.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::removeFile` — `Result<(), IoError>`. `.Err(.NotFound)` where the path
/// names nothing, which is what `unlink(2)` and `unlinkSync` both answer.
///
/// # Safety
/// The path must be a live `Str` view; `out_err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_remove_file(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    match std::fs::remove_file(&path) {
        Ok(()) => BURI_OK,
        // SAFETY: the caller promises a writable destination.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::removeDir` — `Result<(), IoError>`, and the directory must be **empty**.
///
/// `rmdir(2)`, which is what `core/fs`'s `removeDir` promises and the whole of
/// what it promises: a directory that still holds something is `ENOTEMPTY`,
/// which [`io_error`] has no classified variant for and so reports as
/// `.Other(message)` carrying the platform's own sentence. A path naming a file
/// is `ENOTDIR`, which it does name — `.NotADirectory` — and a path naming
/// nothing is `.NotFound`.
///
/// Not `remove_dir_all`, deliberately: there is no recursive form on either
/// backend, and `core/fs` carries the argument.
///
/// # Safety
/// The path must be a live `Str` view; `out_err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_remove_dir(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    match std::fs::remove_dir(&path) {
        Ok(()) => BURI_OK,
        // SAFETY: the caller promises a writable destination.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::makeDir` — `Result<(), IoError>`, parents included.
///
/// An existing directory is `.Ok`, and a path already naming a file is
/// `.Err(.AlreadyExists)` — the same three answers `mkdirSync(p, {recursive:
/// true})` gives.
///
/// # Safety
/// The path must be a live `Str` view; `out_err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_make_dir(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    match std::fs::create_dir_all(&path) {
        Ok(()) => BURI_OK,
        // SAFETY: the caller promises a writable destination.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::syncFile` — `Result<(), IoError>`, the commit point.
///
/// `File::sync_all`, which is `fsync(2)` on Linux and **`fcntl(F_FULLFSYNC)`**
/// on macOS — so the native backend waits for the drive's own cache there,
/// which node's `fsyncSync` does not. `core/fs`'s module header is where that
/// difference is stated for a program that has to choose.
///
/// Opened read-only: `fsync(2)` needs no write access, and a directory — whose
/// entry is what makes a preceding rename durable — cannot be opened for
/// writing at all.
///
/// # Safety
/// The path must be a live `Str` view; `out_err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_sync_file(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        // SAFETY: the caller promises a writable destination.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    match file.sync_all() {
        Ok(()) => BURI_OK,
        // SAFETY: as above.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}


/// `Metadata` — `struct { kind: EntryKind, size: Int, modified: Instant }`.
///
/// `EntryKind` is four variants with no payload, which `middle/layout.rs` gives
/// a bare `i8` tag: the value *is* the index. `Instant` is a one-field struct
/// over an `I64`, so it is the eight bytes and no wrapper. `size` therefore
/// starts at offset 8, which is `#[repr(C)]`'s padding rule and the value
/// model's alignment rule agreeing — the same arrangement `net.rs`'s
/// `BuriRequest` has.
#[repr(C)]
pub struct BuriMetadata {
    kind: i8,
    size: i64,
    modified: i64,
}

/// A `Metadata` for a filesystem with no clock behind it — `testing.rs`'s.
///
/// Here rather than there so the layout has one definition: two transcriptions
/// of one struct is one of them being wrong.
pub(crate) fn metadata_of(kind: i8, size: i64) -> BuriMetadata {
    BuriMetadata { kind, size, modified: 0 }
}

/// What a `std::fs::Metadata` is, as `EntryKind`'s variant index.
///
/// A link is asked about first, because a link to a directory answers `true` to
/// both — and this is `symlink_metadata`, so a link is a link and never what it
/// points at.
fn entry_kind(m: &std::fs::Metadata) -> i8 {
    if m.file_type().is_symlink() {
        2
    } else if m.is_dir() {
        1
    } else if m.is_file() {
        0
    } else {
        3
    }
}

/// Milliseconds since the epoch, saturating at both ends.
///
/// A file whose timestamp is before 1970 — a restored archive, a clock that was
/// wrong — answers a negative number rather than failing, and one the platform
/// will not report answers 0. Neither is worth an `IoError` a caller would have
/// to match on to read a directory.
fn modified_millis(m: &std::fs::Metadata) -> i64 {
    let Ok(at) = m.modified() else { return 0 };
    match at.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// `Fs::metadata` — `Result<Metadata, IoError>`.
///
/// `symlink_metadata` and not `metadata`: a link is `.Symlink` rather than
/// whatever it points at, which is what makes `EntryKind` worth having and what
/// keeps `core/fs`'s `walk` out of a loop.
///
/// # Safety
/// The path must be a live `Str` view; both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_metadata(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_ok: *mut BuriMetadata,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    match std::fs::symlink_metadata(&path) {
        Ok(m) => {
            let value = BuriMetadata {
                kind: entry_kind(&m),
                size: i64::try_from(m.len()).unwrap_or(i64::MAX),
                modified: modified_millis(&m),
            };
            // SAFETY: the caller promises a writable destination.
            unsafe { out_ok.write(value) };
            BURI_OK
        }
        // SAFETY: as above.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::readRange` — `Result<[U8], IoError>`, a window of the file.
///
/// One `pread` rather than a read of the whole file, so the head of a large
/// file costs the head. An offset past the end is the empty list, which is what
/// a read at end of file is; a negative offset or count is the one failure this
/// produces that the platform would not.
///
/// # Safety
/// The path must be a live `Str` view; both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_read_range(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    at: i64,
    count: i64,
    out_ok: *mut BuriList,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    if at < 0 || count < 0 {
        // SAFETY: the caller promises a writable destination.
        unsafe { out_err.write(str_of("a negative offset or count")) };
        return 6;
    }
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        // SAFETY: as above.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    if let Err(e) = file.seek(std::io::SeekFrom::Start(at as u64)) {
        // SAFETY: as above.
        return unsafe { fail(&e, out_err) };
    }
    let mut body = Vec::new();
    if let Err(e) = file.take(count as u64).read_to_end(&mut body) {
        // SAFETY: as above.
        return unsafe { fail(&e, out_err) };
    }
    let value = list_of_bytes(&body);
    // SAFETY: the caller promises a writable destination.
    unsafe { out_ok.write(value) };
    BURI_OK
}

/// `Fs::realPath` — `Result<Str, IoError>`, every link and every `..` resolved.
///
/// Every component has to exist, which is `realpath(3)`'s own rule and the
/// reason `core/path` cannot answer this: `a/../b` and `b` are two files where
/// `a` is a link, and only the filesystem knows.
///
/// # Safety
/// The path must be a live `Str` view; both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_real_path(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out_ok: *mut BuriStr,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let path = unsafe { text(ptr, len) };
    match std::fs::canonicalize(&path) {
        Ok(resolved) => {
            let value = str_of(&resolved.to_string_lossy());
            // SAFETY: the caller promises a writable destination.
            unsafe { out_ok.write(value) };
            BURI_OK
        }
        // SAFETY: as above.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

/// `Fs::copyFile` — `Result<(), IoError>`, contents over `to`.
///
/// **Not atomic**, which is the difference from [`buri_rt_host_fs_rename_file`]:
/// a reader of the destination can see half of it. Contents only — permissions,
/// ownership and timestamps are not carried over, so the two backends promise
/// the same thing, and `std::fs::copy`'s permission copy is the one place they
/// would otherwise differ from node's.
///
/// # Safety
/// Both paths must be live `Str` views; `out_err` writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_fs_copy_file(
    _fbase: *mut u8,
    fptr: *const u8,
    flen: u64,
    _tbase: *mut u8,
    tptr: *const u8,
    tlen: u64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let (from, to) = unsafe { (text(fptr, flen), text(tptr, tlen)) };
    let body = match std::fs::read(&from) {
        Ok(body) => body,
        // SAFETY: the caller promises a writable destination.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    match std::fs::write(&to, body) {
        Ok(()) => BURI_OK,
        // SAFETY: as above.
        Err(e) => unsafe { fail(&e, out_err) },
    }
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// One `[Header]` element, as it is laid out in the spine.
///
/// `Header` is `struct { name: Str, value: Str }`, and VALUE-MODEL.md §5 lays a
/// struct out as its fields back to back — so an element of a `[Header]` is two
/// `BuriStr`s and the stride is 48. Declared here rather than inferred at the
/// cast site so the shape has a name a reader can check against the Buri
/// declaration.
#[repr(C)]
struct BuriHeader {
    name: BuriStr,
    value: BuriStr,
}

/// The name/value pairs of a `[Header]` argument.
///
/// # Safety
/// `ptr` must be the payload of a live `[Header]` of `len` elements, or null
/// with `len == 0`.
pub(crate) unsafe fn headers(ptr: *const u8, len: u64) -> Vec<(String, String)> {
    if ptr.is_null() || len == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len as usize {
        // SAFETY: the caller promises `len` elements at `ptr`, and the stride
        // is the one the layout gives `Header`.
        let element = unsafe { &*ptr.cast::<BuriHeader>().add(i) };
        // SAFETY: an element of a live list holds live `Str` views.
        out.push(unsafe {
            (element.name.as_str().into_owned(), element.value.as_str().into_owned())
        });
    }
    out
}

/// `Net::fetch` — `Result<Response, NetError>`.
///
/// The Buri signature is `fetch(self, request: Request)`, and `Request` is
/// `{ method: Method, url: Str, headers: [Header], body: [U8] }` — so per
/// `lib.rs` §2 rule 1 the argument arrives flattened: the method's variant
/// index (widened to a C `int`, for [`crate::Ret::Tag`]'s reason), then the
/// URL's three `Str` leaves, then two `(ptr, len)` pairs. `Response`'s three
/// fields leave through three out-pointers, per §2 rule 2.
///
/// On the error arm the returned tag is `NetError`'s variant index and
/// `out_err` carries the payload of the two variants that have one —
/// `BadUrl(Str)` and `Transport(Str)` — and the empty string for the three
/// that do not.
///
/// **No backend calls this yet.** `NetError` carries a payload on two of its
/// variants, and `lib.rs` §2.1's `Result` shape requires the error variant an
/// entry names to carry none — so neither runtime table has a row for
/// `host.HostNet.fetch`, and both name it in their absent-key list. This body
/// is what a row will call, and what `cli/tests/native/driver.c` calls today.
///
/// `timeout_millis` is `Request.withTimeout`'s, and **zero is the runtime's own
/// bound** — `http::DEADLINE`. It bounds every step of the exchange rather than
/// the whole of it, which is what `http.rs`'s `fetch_within` takes and what its
/// header says a deadline here is worth.
///
/// `http://` only; see `http.rs` for why, and for what would change it.
///
/// # Safety
/// The URL view, the `[Header]` and the `[U8]` must be live; all four
/// out-pointers writable and aligned.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_host_net_fetch(
    method: i32,
    _ubase: *mut u8,
    uptr: *const u8,
    ulen: u64,
    hptr: *const u8,
    hlen: u64,
    bptr: *const u8,
    blen: u64,
    timeout_millis: i64,
    out_status: *mut i64,
    out_headers: *mut BuriList,
    out_body: *mut BuriList,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let url = unsafe { text(uptr, ulen) };
    // SAFETY: forwarded.
    let sent = unsafe { headers(hptr, hlen) };
    let body: &[u8] = if bptr.is_null() || blen == 0 {
        &[]
    } else {
        // SAFETY: the caller promises `blen` readable bytes; a `[U8]`'s stride
        // is one, so the payload is the bytes themselves.
        unsafe { std::slice::from_raw_parts(bptr, blen as usize) }
    };
    // **Not a suspension point, though it is spelled like one.** `http.rs`'s
    // client is synchronous, so the `async` block runs to completion inside its
    // first poll and answers `Ready`, and `rt::park_on` only gives a carrier
    // back to a future that answers `Pending`. This carrier is held for as long
    // as the exchange takes — bounded, but by `http.rs`'s own deadlines and not
    // by anything here. `net.rs`'s `park` is the same two lines with the same
    // caveat written out at length. What the spelling buys is that the day the
    // client is asynchronous this becomes the suspension the name promises,
    // without this line or any caller moving.
    //
    // The Buri blocks below are built **after** it answers and on the carrier.
    // That used to be load-bearing: under the run baton the allocator and the
    // reference counts were single-threaded. G3 deleted the baton and made a
    // block two carriers can reach atomically counted (`rt.rs` §1), so what is
    // left is the ordinary rule — a runtime call builds no Buri value until it
    // has an answer to build one from.
    let bound = http::bound(timeout_millis);
    #[cfg(feature = "net")]
    let outcome = crate::rt::park_on(async { http::fetch(bound, method, &url, &sent, body) });
    #[cfg(not(feature = "net"))]
    let outcome = http::fetch(bound, method, &url, &sent, body);
    match outcome {
        Ok(response) => {
            let fields = list_of_headers(&response.headers);
            let bytes = list_of_bytes(&response.body);
            // SAFETY: the caller promises writable destinations.
            unsafe {
                out_status.write(response.status);
                out_headers.write(fields);
                out_body.write(bytes);
            }
            BURI_OK
        }
        Err(e) => {
            let message = str_of(e.message());
            // SAFETY: as above.
            unsafe { out_err.write(message) };
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
///
/// **A suspension point** (`rt.rs` §2): with the `net` feature the wait is the
/// reactor's timer wheel rather than the carrier, so the carrier is idle and
/// the processor is free. Without the feature there is no reactor and this is
/// `thread::sleep`, which is what it has always been.
///
/// The two are distinguishable, and `Tasks.parallel` is what distinguishes
/// them: two steps that each sleep here finish in one sleep's time rather than
/// two. Until G3 that was because the sleeping carrier gave the *run baton* to
/// the other one; there is no baton now, and the answer is simpler — they are
/// two threads, and both of them are asleep. Outside a fan-out the sleeping
/// thread is still the only one there is, and the two answer the same nothing
/// after the same wait.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_clock_sleep_millis(millis: i64) {
    if millis > 0 {
        let duration = std::time::Duration::from_millis(millis as u64);
        // The sleep is built *inside* the future, not handed to `park_on`
        // ready-made: `tokio::time::sleep` registers with the timer driver
        // where it is constructed, and this thread is in no runtime context
        // until `park_on` enters one.
        #[cfg(feature = "net")]
        crate::rt::park_on(async move { tokio::time::sleep(duration).await });
        #[cfg(not(feature = "net"))]
        std::thread::sleep(duration);
    }
}

/// `Clock::monotonicNanoseconds` — nanoseconds off a clock that only goes
/// forward.
///
/// The zero is this process's first reading of it, which is the whole of what
/// the effect promises: the number means nothing on its own and only a
/// difference of two of them does. `std::time::Instant` has no epoch to hand
/// out, so the baseline is taken once here and every reading is measured from
/// it.
///
/// It saturates at `i64::MAX`, which is 292 years of nanoseconds. Nothing is
/// going to reach it, and a wrap would make a later reading smaller than an
/// earlier one — the one thing this must never do.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_clock_monotonic_nanoseconds() -> i64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    i64::try_from(start.elapsed().as_nanos()).unwrap_or(i64::MAX)
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
pub unsafe extern "C" fn buri_rt_host_env_args(out: *mut BuriList) {
    let recorded = lock(&ARGS).clone();
    let args = recorded.unwrap_or_else(|| {
        std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect()
    });
    let value = list_of_strs(&args);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}


/// `Env::currentDirectory` — where the process is, as text.
///
/// A directory that has been removed under the process answers the empty
/// string rather than failing: `core/env` promises a `Path`, `path.of` turns
/// the empty string into `.`, and a program that cannot read its own directory
/// has a `.` that means the same thing.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_env_current_directory(out: *mut BuriStr) {
    let at = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let value = str_of(&at);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `Env::allVariables` — every variable, as `(name, value)` pairs.
///
/// `[(Str, Str)]` is `[Header]`'s layout — two `Str`s back to back — so
/// [`list_of_headers`] builds it, and the name says `Header` because that is
/// the shape rather than the type.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_env_all_variables(out: *mut BuriList) {
    let pairs: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .collect();
    let value = list_of_headers(&pairs);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `Env::operatingSystemName` — the lower-case short name.
///
/// `std::env::consts::OS`'s words, with the one that differs from node's
/// mapped: rust says `macos` and node says `darwin`, and `core/env` documents
/// `macos`. The other two this toolchain builds for already agree.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_env_operating_system_name(out: *mut BuriStr) {
    let value = str_of(std::env::consts::OS);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(value) }
}

/// `Output` — `struct { code: Int, stdout: [U8], stderr: [U8] }`.
///
/// `BuriMetadata`'s arrangement: a struct is its fields back to back, so this
/// is an `i64` and two 16-byte lists.
#[repr(C)]
pub struct BuriOutput {
    code: i64,
    stdout: BuriList,
    stderr: BuriList,
}

/// `Spawn::spawnProcess` — `Result<Output, IoError>`.
///
/// Four flat arguments rather than a `Command`, which `core/proc`'s
/// `effect Spawn` argues: `plan` is the program, then the working directory —
/// empty for this process's own — then the arguments, and `environment` is name
/// and value alternating, used only when `replaces` says so.
///
/// **Both streams are drained while the child runs.** `std::process::Child`'s
/// `wait_with_output` does exactly that, which is what keeps a child writing
/// more than a pipe holds from deadlocking — the same promise the JavaScript
/// half keeps by listening on both `data` events.
///
/// A child killed by a signal reports `128 + signal`, which is the number a
/// shell reports and the number node's half computes from the signal's name.
///
/// # Safety
/// Every view must be live: the two `[Str]` lists as `(ptr, len)`, and the
/// `[U8]` input. Both out-pointers writable and aligned.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_host_spawn_process(
    pptr: *const u8,
    plen: u64,
    eptr: *const u8,
    elen: u64,
    replaces: u8,
    iptr: *const u8,
    ilen: u64,
    out_ok: *mut BuriOutput,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let plan = unsafe { strs(pptr, plen) };
    // SAFETY: forwarded.
    let variables = unsafe { strs(eptr, elen) };
    // SAFETY: forwarded.
    let input = unsafe { view(iptr, ilen) }.to_vec();

    let program = plan.first().cloned().unwrap_or_default();
    let working = plan.get(1).cloned().unwrap_or_default();
    let mut command = std::process::Command::new(&program);
    command.args(plan.iter().skip(2));
    if !working.is_empty() {
        command.current_dir(&working);
    }
    if replaces != 0 {
        command.env_clear();
        for pair in variables.chunks_exact(2) {
            command.env(&pair[0], &pair[1]);
        }
    }
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        // SAFETY: the caller promises a writable destination.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    // **The input goes down its own thread.** `wait_with_output` is what drains
    // the child's two streams, and a pipe holds about 64 kilobytes: a program
    // that wrote the whole input first would stop as soon as the child had
    // written that much back, with each side waiting for the other to read.
    // `core/process`'s `run` promises both streams are read while the child
    // runs, and this is what keeps the promise.
    //
    // A child that never reads its input closes the pipe, and writing into a
    // closed pipe is that child's choice rather than a failure of the run — so
    // the write is dropped and the exit code is the answer.
    let feeding = child.stdin.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
            // Dropping the pipe closes it, which is the child's end of input.
        })
    });
    let done = match child.wait_with_output() {
        Ok(done) => done,
        // SAFETY: as above.
        Err(e) => {
            if let Some(handle) = feeding {
                let _ = handle.join();
            }
            return unsafe { fail(&e, out_err) };
        }
    };
    if let Some(handle) = feeding {
        // The child has gone, so the writer is finished or holding a pipe
        // nobody reads: either way the join is immediate.
        let _ = handle.join();
    }
    let value = BuriOutput {
        code: exit_code(&done.status),
        stdout: list_of_bytes(&done.stdout),
        stderr: list_of_bytes(&done.stderr),
    };
    // SAFETY: the caller promises a writable destination.
    unsafe { out_ok.write(value) };
    BURI_OK
}

/// The status a shell would report: the code, or `128 + signal`.
fn exit_code(status: &std::process::ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        return i64::from(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        return 128 + i64::from(status.signal().unwrap_or(0));
    }
    #[cfg(not(unix))]
    {
        128
    }
}

/// The strings of a `[Str]` argument.
///
/// # Safety
/// `ptr` must be the payload of a live `[Str]` of `len` elements, or null with
/// `len == 0`.
pub(crate) unsafe fn strs(ptr: *const u8, len: u64) -> Vec<String> {
    if ptr.is_null() || len == 0 {
        return Vec::new();
    }
    let stride = std::mem::size_of::<BuriStr>();
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len as usize {
        // SAFETY: the caller promises `len` elements at `ptr`, and the stride
        // is the one the layout gives a `Str`.
        let element = unsafe { &*ptr.add(i * stride).cast::<BuriStr>() };
        // SAFETY: an element of a live list holds a live `Str` view.
        out.push(unsafe { element.as_str().into_owned() });
    }
    out
}

/// `Proc::exitWith`. Flushes first, and does not return.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_proc_exit_with(code: i64) -> ! {
    // A program that chose where to stop is entitled to be holding values, so
    // the test-mode exit audit stays quiet (`memory.rs`'s heap-check section).
    crate::memory::quiet_heap_audit();
    buri_rt_flush();
    std::process::exit(code as i32)
}
