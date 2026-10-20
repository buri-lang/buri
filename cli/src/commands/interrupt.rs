//! How a `--watch` loop and a page server stop on `SIGINT`, `SIGTERM` or
//! `SIGHUP`: the first signal lets the run in progress finish, and the command
//! then exits with 128 plus the signal, through the normal path. A second
//! signal ends the process at once.
//!
//! 128 plus the signal is what a shell shows for a command a signal killed, so
//! a script reads the same status it always did. What the normal path adds is
//! everything an exit does that a signal skips.

use std::sync::atomic::{AtomicI32, Ordering};

// Declared rather than depended on, as in `commands::run`.
unsafe extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
    fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
}

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
const SIG_DFL: usize = 0;

/// The signal that arrived, or 0.
static ASKED: AtomicI32 = AtomicI32::new(0);

const SAID: &[u8] = b"buri: stopping once the run in progress ends; signal again to stop now\n";

/// The handler, which runs once. A `signal`, `store` and `write`, all on
/// POSIX's async-signal-safe list.
extern "C" fn noted(sig: i32) {
    // SAFETY: the default disposition back for all three, first, so a second
    // signal ends the process the way it would have without this module. The
    // one being handled is held until this returns, so it is a second too.
    unsafe {
        signal(SIGHUP, SIG_DFL);
        signal(SIGINT, SIG_DFL);
        signal(SIGTERM, SIG_DFL);
    }
    ASKED.store(sig, Ordering::Relaxed);
    // SAFETY: a constant buffer, to standard error.
    unsafe { write(2, SAID.as_ptr().cast(), SAID.len()) };
}

/// Takes the three signals for the loop or server about to run.
pub fn listen() {
    for sig in [SIGHUP, SIGINT, SIGTERM] {
        // SAFETY: an ordinary `signal` call with a function this module owns.
        unsafe { signal(sig, noted as *const () as usize) };
    }
}

/// The status to exit with once asked to stop: 128 plus the signal.
pub fn asked() -> Option<i32> {
    match ASKED.load(Ordering::Relaxed) {
        0 => None,
        sig => Some(128_i32.saturating_add(sig)),
    }
}
