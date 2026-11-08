//! Places a test can hold `buri` at, so it decides a race instead of the
//! scheduler.
//!
//! ```sh
//! BURI_HOLD=/tmp/held buri test --watch   # the 3rd sweep reads /tmp/held/sweep-3
//! ```
//!
//! The `n`th time a process reaches a point it reads `<dir>/<point>-<n>`, or,
//! at a point whose reacher names itself, `<dir>/<point>-<name>`. A FIFO there
//! holds it until the test writes a byte to it, a file hands its contents back
//! to the point, and a missing file holds nothing.
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
        Point {
            name,
            reached: AtomicUsize::new(0),
        }
    }

    /// Waits for the test, and answers what it wrote: nothing without one.
    pub fn held(&self) -> Vec<u8> {
        let n = self
            .reached
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.held_as(&n.to_string())
    }

    /// The same, at `<dir>/<point>-<who>` whenever it is reached, for a point
    /// several threads reach in an order the test can't know: each names
    /// itself, and the test holds the one it means.
    pub fn held_as(&self, who: &str) -> Vec<u8> {
        let Some(dir) = std::env::var_os(VARIABLE) else {
            return Vec::new();
        };
        let Ok(mut file) =
            std::fs::File::open(std::path::Path::new(&dir).join(format!("{}-{who}", self.name)))
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
        String::from_utf8_lossy(&self.held())
            .trim()
            .parse()
            .unwrap_or(measured)
    }
}

/// Hands the progress block a clock and a hold on each side of a row's start,
/// read from the same directory.
///
/// The clock is `<dir>/clock-now`, whole seconds, rewritten by the test as it
/// moves time along; the last time read stays until the next one parses. A row
/// for work labelled `//lib/a` holds at `begin-test-lib_a` before it shows and
/// `begun-test-lib_a` after.
pub fn progress() {
    use crate::progress::{hook, Hooks, Kind};
    use std::sync::Mutex;
    static CLOCK: Point = Point::new("clock");
    static LAST: Mutex<Option<std::time::Duration>> = Mutex::new(None);
    static BEGIN: Point = Point::new("begin");
    static BEGUN: Point = Point::new("begun");
    fn clock() -> Option<std::time::Duration> {
        std::env::var_os(VARIABLE)?;
        let mut last = LAST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(secs) = String::from_utf8_lossy(&CLOCK.held_as("now"))
            .trim()
            .parse()
        {
            *last = Some(std::time::Duration::from_secs(secs));
        }
        *last
    }
    fn starting(kind: Kind, label: &str, begun: bool) {
        if std::env::var_os(VARIABLE).is_none() {
            return;
        }
        let mut who = String::new();
        for c in label.chars() {
            if c.is_ascii_alphanumeric() {
                who.push(c);
            } else if !who.is_empty() && !who.ends_with('_') {
                who.push('_');
            }
        }
        let who = format!("{}-{}", kind.name(), who.trim_end_matches('_'));
        if begun {
            BEGUN.held_as(&who);
        } else {
            BEGIN.held_as(&who);
        }
    }
    hook(Hooks { clock, starting });
}
