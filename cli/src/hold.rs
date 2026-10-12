//! Places a test can hold `buri` at, so it decides a race instead of the
//! scheduler.
//!
//! ```sh
//! BURI_HOLD=/tmp/held buri test --watch   # the 3rd sweep reads /tmp/held/sweep-3
//! ```
//!
//! The `n`th time a process reaches a point it reads `<dir>/<point>-<n>`. A
//! FIFO there holds it until the test writes a byte to it, a file hands its
//! contents back to the point, and a missing file holds nothing.
//!
//! The byte, not the writer closing, lets it go: on macOS a writer that opens
//! and closes while the reader is still opening can leave the read with no end.
//!
//! Only a build with the `test-hooks` feature has this module, so a shipped
//! `buri` ignores `BURI_HOLD`.

use std::io::Read as _;
use std::os::unix::fs::FileTypeExt as _;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The directory of held points. Unset outside tests.
pub const VARIABLE: &str = "BURI_HOLD";

/// One place a test can hold.
pub struct Point {
    name: &'static str,
    reached: AtomicUsize,
}

impl Point {
    pub const fn new(name: &'static str) -> Point {
        Point { name, reached: AtomicUsize::new(0) }
    }

    /// Waits for the test, and answers what it wrote: nothing without one.
    pub fn held(&self) -> Vec<u8> {
        let Some(dir) = std::env::var_os(VARIABLE) else { return Vec::new() };
        let n = self.reached.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let Ok(mut file) = std::fs::File::open(std::path::Path::new(&dir).join(format!("{}-{n}", self.name)))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if file.metadata().is_ok_and(|m| m.file_type().is_fifo()) {
            let _ = file.read_exact(&mut [0u8; 1]);
        } else {
            let _ = file.read_to_end(&mut out);
        }
        out
    }

    /// Waits for the test, and raises each byte it wrote as a signal, on this
    /// thread, so a handler has run before this returns.
    pub fn raised(&self) {
        unsafe extern "C" {
            fn raise(sig: i32) -> i32;
        }
        for sig in self.held() {
            // SAFETY: an ordinary `raise`.
            unsafe { raise(i32::from(sig)) };
        }
    }

    /// The nanoseconds the test wrote, or `measured` without them.
    pub fn time(&self, measured: u64) -> u64 {
        String::from_utf8_lossy(&self.held()).trim().parse().unwrap_or(measured)
    }
}
