//! `Spawn`'s started children: `core/process`'s `start`, and the four calls
//! a `Child` makes afterwards.
//!
//! ## A reaped pid is never signalled
//!
//! Once a child has been waited on, the kernel may hand its pid to another
//! process. So the table below holds every child's status, and **a child is
//! reaped only while the table's lock is held**, and signalled only while it
//! is held and only if no status is recorded. An unreaped child is at worst a
//! zombie, whose pid nothing else can have.
//!
//! A wait cannot hold the lock while it blocks, or a signal from another task
//! would wait behind it. So it blocks on the child's exit **without reaping**
//! (`waitid` with `WNOWAIT` on Linux, a `kqueue` exit event on macOS, whose
//! `waitid` returns at once), then takes the lock and reaps.
//!
//! ## Blocking
//!
//! A wait holds its thread, as `run`'s does: `rt.rs`'s scheduler starts another
//! thread for the tasks queued behind one that blocks, so the rest of the
//! program keeps running.
//!
//! ## What is never done
//!
//! Nothing here reaps a child nobody asked about, and nothing kills one when
//! the program ends: an exited child stays a zombie until `wait` or `tryWait`
//! collects it, which is what keeps its status for them, and a running one
//! outlives the program, as in Rust's `std`.

use crate::host::{fail, strs, view};
use crate::value::{str_of, BuriStr};
use crate::BURI_OK;
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicI64, Ordering};
use crate::sync::{Mutex, MutexGuard};

/// One started child: its pid, and its raw wait status once reaped.
struct Started {
    pid: i32,
    status: Option<i32>,
}

/// Every started child, by handle. Entries are never removed, so a handle
/// never names a different child and a second wait finds the first's status.
static CHILDREN: Mutex<Option<HashMap<i64, Started>>> = Mutex::new(None);

/// The last handle given out. Zero, so the static is `__bss`, and the first
/// handle is one, so zero names nothing.
static LAST: AtomicI64 = AtomicI64::new(0);

fn table() -> MutexGuard<'static, Option<HashMap<i64, Started>>> {
    CHILDREN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
}

const WNOHANG: i32 = 1;

/// `core/process`'s numbering of a signal, as this platform numbers it.
fn signal_number(code: i64) -> Option<i32> {
    #[cfg(target_os = "macos")]
    const STOP_CONTINUE: (i32, i32) = (17, 19);
    #[cfg(not(target_os = "macos"))]
    const STOP_CONTINUE: (i32, i32) = (19, 18);
    match code {
        -1 => Some(2),
        -2 => Some(15),
        -3 => Some(9),
        -4 => Some(STOP_CONTINUE.0),
        -5 => Some(STOP_CONTINUE.1),
        n => i32::try_from(n).ok().filter(|n| *n >= 0),
    }
}

/// A raw wait status in `core/process`'s numbering: the exit code, or the
/// signal's number negated.
fn status_code(raw: i32) -> i64 {
    let signal = raw & 0x7f;
    if signal == 0 { i64::from((raw >> 8) & 0xff) } else { i64::from(signal).wrapping_neg() }
}

/// Reap `pid` if it has exited, without waiting. The caller holds the lock.
fn reap(child: &mut Started) -> std::io::Result<()> {
    if child.status.is_some() {
        return Ok(());
    }
    let mut raw = 0;
    loop {
        // SAFETY: a plain `waitpid` on our own child, into a local.
        let got = unsafe { waitpid(child.pid, &raw mut raw, WNOHANG) };
        if got == child.pid {
            child.status = Some(raw);
            return Ok(());
        }
        if got == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Block until `pid` has exited, leaving it unreaped.
#[cfg(target_os = "macos")]
fn exited(pid: i32) -> std::io::Result<()> {
    #[repr(C)]
    struct KEvent {
        ident: usize,
        filter: i16,
        flags: u16,
        fflags: u32,
        data: isize,
        udata: *mut u8,
    }
    unsafe extern "C" {
        fn kqueue() -> i32;
        fn kevent(
            kq: i32,
            changes: *const KEvent,
            count: i32,
            events: *mut KEvent,
            capacity: i32,
            timeout: *const u8,
        ) -> i32;
        fn close(fd: i32) -> i32;
    }
    const EVFILT_PROC: i16 = -5;
    const EV_ADD: u16 = 0x1;
    const EV_ONESHOT: u16 = 0x10;
    const NOTE_EXIT: u32 = 0x8000_0000;
    // SAFETY: a new descriptor, closed below.
    let kq = unsafe { kqueue() };
    if kq < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let change = KEvent {
        ident: pid as usize,
        filter: EVFILT_PROC,
        flags: EV_ADD | EV_ONESHOT,
        fflags: NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut event = KEvent { ident: 0, filter: 0, flags: 0, fflags: 0, data: 0, udata: std::ptr::null_mut() };
    let answer = loop {
        // SAFETY: one change in and room for one event out, both locals.
        let got = unsafe { kevent(kq, &raw const change, 1, &raw mut event, 1, std::ptr::null()) };
        if got >= 0 {
            // The exit, or `ESRCH` for a child that is a zombie already.
            const EV_ERROR: u16 = 0x4000;
            const ESRCH: isize = 3;
            if got == 1 && event.flags & EV_ERROR != 0 && event.data != ESRCH {
                break Err(std::io::Error::from_raw_os_error(event.data as i32));
            }
            break Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            break Err(e);
        }
    };
    // SAFETY: the descriptor opened above.
    unsafe { close(kq) };
    answer
}

/// Block until `pid` has exited, leaving it unreaped.
#[cfg(not(target_os = "macos"))]
fn exited(pid: i32) -> std::io::Result<()> {
    unsafe extern "C" {
        fn waitid(idtype: i32, id: u32, info: *mut u8, options: i32) -> i32;
    }
    const P_PID: i32 = 1;
    const WEXITED: i32 = 4;
    const WNOWAIT: i32 = 0x0100_0000;
    // `siginfo_t` is 128 bytes; nothing reads it.
    let mut info = [0u8; 128];
    loop {
        // SAFETY: a buffer the size of a `siginfo_t`.
        let got = unsafe { waitid(P_PID, pid as u32, info.as_mut_ptr(), WEXITED | WNOWAIT) };
        if got == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Write `IoError`'s variant `tag` with `message`, and answer the tag.
///
/// # Safety
/// `out_err` must be writable and aligned for a [`BuriStr`].
unsafe fn refuse(tag: i32, message: &str, out_err: *mut BuriStr) -> i32 {
    // SAFETY: forwarded.
    unsafe { out_err.write(str_of(message)) };
    tag
}

/// `.NotFound`, for a handle naming no child of this table: one a test double
/// minted, say.
///
/// # Safety
/// As [`refuse`].
unsafe fn unknown(out_err: *mut BuriStr) -> i32 {
    // SAFETY: forwarded.
    unsafe { refuse(0, "", out_err) }
}

/// `Spawn::startProcess` — `Result<Int, IoError>`: `spawnProcess`'s arguments,
/// and a handle back once the child is running.
///
/// # Safety
/// As `buri_rt_host_spawn_process`, with an `i64` destination for `.Ok`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_host_spawn_start_process(
    pptr: *const u8,
    plen: u64,
    eptr: *const u8,
    elen: u64,
    replaces: u8,
    iptr: *const u8,
    ilen: u64,
    out_ok: *mut i64,
    out_err: *mut BuriStr,
) -> i32 {
    // SAFETY: forwarded.
    let plan = unsafe { strs(pptr, plen) };
    // SAFETY: forwarded.
    let variables = unsafe { strs(eptr, elen) };
    // SAFETY: forwarded.
    let input = unsafe { view(iptr, ilen) }.to_vec();
    let mut command = match crate::host::command_of(&plan, &variables, replaces != 0, false) {
        Ok(command) => command,
        // SAFETY: the caller promises a writable destination.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    command.stdin(if input.is_empty() {
        std::process::Stdio::null()
    } else {
        std::process::Stdio::piped()
    });
    // Whatever this program printed reaches its streams before the child's.
    crate::host::about_to_block();
    let mut child = match command.spawn() {
        Ok(child) => child,
        // SAFETY: as above.
        Err(e) => return unsafe { fail(&e, out_err) },
    };
    // The input goes down a thread of its own, which ends once it is written
    // or the child closes its end.
    if let Some(mut pipe) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    let Ok(pid) = i32::try_from(child.id()) else {
        // SAFETY: as above.
        return unsafe { refuse(6, "the child's process id is out of range", out_err) };
    };
    // Dropping a `std::process::Child` neither waits nor kills: the table is
    // the child's only owner from here.
    drop(child);
    let handle = LAST.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    table().get_or_insert_with(HashMap::new).insert(handle, Started { pid, status: None });
    // SAFETY: the caller promises a writable destination.
    unsafe { out_ok.write(handle) };
    BURI_OK
}

/// `Spawn::processId` — the pid a handle names, or zero for none.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_spawn_process_id(handle: i64) -> i64 {
    table().as_ref().and_then(|t| t.get(&handle)).map_or(0, |c| i64::from(c.pid))
}

/// `Spawn::signalProcess` — `Result<(), IoError>`. A child that has exited is
/// sent nothing.
///
/// # Safety
/// `out_err` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_spawn_signal_process(
    handle: i64,
    signal: i64,
    out_err: *mut BuriStr,
) -> i32 {
    let Some(number) = signal_number(signal) else {
        // SAFETY: the caller promises a writable destination.
        return unsafe { refuse(6, &format!("{signal} is not a signal number"), out_err) };
    };
    let mut guard = table();
    let Some(child) = guard.as_mut().and_then(|t| t.get_mut(&handle)) else {
        // SAFETY: as above.
        return unsafe { unknown(out_err) };
    };
    // Reaping first means a child that has exited is never signalled, and the
    // lock held across both means nobody reaps it in between.
    if let Err(e) = reap(child) {
        // SAFETY: as above.
        return unsafe { fail(&e, out_err) };
    }
    if child.status.is_some() {
        return BURI_OK;
    }
    // SAFETY: an unreaped child of this process, so the pid is still its own.
    if unsafe { kill(child.pid, number) } != 0 {
        let e = std::io::Error::last_os_error();
        // SAFETY: as above.
        return unsafe { fail(&e, out_err) };
    }
    BURI_OK
}

/// `Spawn::pollProcess` — whether the child has exited, reaping it if it has.
/// A handle naming nothing answers `true`, so the wait that follows refuses it.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_spawn_poll_process(handle: i64) -> u8 {
    let mut guard = table();
    let Some(child) = guard.as_mut().and_then(|t| t.get_mut(&handle)) else {
        return 1;
    };
    // A failed `waitpid` on our own child is a status nobody can collect, so
    // the wait that follows is where it is reported.
    u8::from(reap(child).is_err() || child.status.is_some())
}

/// `Spawn::waitProcess` — `Result<Int, IoError>`: the exit code, or the
/// signal's number negated.
///
/// # Safety
/// Both out-pointers writable and aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_spawn_wait_process(
    handle: i64,
    out_ok: *mut i64,
    out_err: *mut BuriStr,
) -> i32 {
    let pid = {
        let mut guard = table();
        let Some(child) = guard.as_mut().and_then(|t| t.get_mut(&handle)) else {
            // SAFETY: the caller promises a writable destination.
            return unsafe { unknown(out_err) };
        };
        if let Err(e) = reap(child) {
            // SAFETY: as above.
            return unsafe { fail(&e, out_err) };
        }
        if let Some(raw) = child.status {
            // SAFETY: as above.
            unsafe { out_ok.write(status_code(raw)) };
            return BURI_OK;
        }
        child.pid
    };
    crate::host::about_to_block();
    if let Err(e) = exited(pid) {
        // SAFETY: as above.
        return unsafe { fail(&e, out_err) };
    }
    let mut guard = table();
    let Some(child) = guard.as_mut().and_then(|t| t.get_mut(&handle)) else {
        // SAFETY: as above.
        return unsafe { unknown(out_err) };
    };
    // The child has exited, so this `waitpid` answers as soon as the kernel has
    // made it a zombie, and holding the lock across it is brief.
    if let Err(e) = reap_exited(child) {
        // SAFETY: as above.
        return unsafe { fail(&e, out_err) };
    }
    let raw = child.status.unwrap_or(0);
    // SAFETY: as above.
    unsafe { out_ok.write(status_code(raw)) };
    BURI_OK
}

/// Reap a child known to have exited. The caller holds the lock.
fn reap_exited(child: &mut Started) -> std::io::Result<()> {
    if child.status.is_some() {
        return Ok(());
    }
    let mut raw = 0;
    loop {
        // SAFETY: a plain `waitpid` on our own child, into a local.
        let got = unsafe { waitpid(child.pid, &raw mut raw, 0) };
        if got == child.pid {
            child.status = Some(raw);
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}
