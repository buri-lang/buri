//! Places a test can hold `buri` at, so it decides a race instead of the
//! scheduler.
//!
//! ```sh
//! BURI_HOLD=/tmp/held buri test --watch   # the 3rd sweep reads /tmp/held/sweep-3
//! ```
//!
//! The `n`th time a process reaches a point it reads `<dir>/<point>-<n>` to
//! the end. A FIFO there holds it until the test closes its writing end, and a
//! missing file holds nothing. What the file held goes back to the point.
//!
//! Only a build with the `test-hooks` feature has this module, so a shipped
//! `buri` ignores `BURI_HOLD`.

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
        std::fs::read(std::path::Path::new(&dir).join(format!("{}-{n}", self.name))).unwrap_or_default()
    }

    /// The nanoseconds the test wrote, or `measured` without them.
    pub fn time(&self, measured: u64) -> u64 {
        String::from_utf8_lossy(&self.held()).trim().parse().unwrap_or(measured)
    }
}
