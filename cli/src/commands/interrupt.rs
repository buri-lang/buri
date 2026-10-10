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
    fn raise(sig: i32) -> i32;
    fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
}

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
const SIG_DFL: usize = 0;

/// The first signal that arrived, or 0.
static ASKED: AtomicI32 = AtomicI32::new(0);

const SAID: &[u8] = b"buri: stopping once the run in progress ends; signal again to stop now\n";

/// The handler. A `swap`, `write`, `signal` and `raise`, all on POSIX's
/// async-signal-safe list.
extern "C" fn noted(sig: i32) {
    if ASKED.swap(sig, Ordering::Relaxed) == 0 {
        // SAFETY: a constant buffer, to standard error.
        unsafe { write(2, SAID.as_ptr().cast(), SAID.len()) };
        return;
    }
    // SAFETY: the default disposition back, then the signal again, so the
    // process ends the way it would have without this module.
    unsafe {
        signal(sig, SIG_DFL);
        raise(sig);
    }
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
