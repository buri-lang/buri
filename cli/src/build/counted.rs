//! What one run spent its build on, counted rather than timed.
//!
//! An edit-and-rerun of `buri test` spends most of its time in the first
//! launch of each newly written runner, then in the link (`design/PERFORMANCE.md`
//! §6.79). Neither shows up in an instruction count, so these count the
//! operations themselves. Each is the same on every run of one tree, whatever
//! the load, so a test can pin it exactly. `BURI_PROFILE` prints them.
//!
//! Off, each count is one load of a flag that was read once.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, PoisonError};

use crate::build::cache::{Action, Status};

#[derive(Clone, Copy)]
pub enum Count {
    /// A suite's program compiled: `run build` in `--explain`.
    SuitesBuilt,
    /// A suite's binary or bundle taken from the cache and run again: `cached build`.
    SuitesRestored,
    /// A suite's verdict taken from the cache, nothing run: `cached test`.
    SuitesReused,
    /// A codegen unit emitted: `run codegen`.
    ObjectsCompiled,
    /// A codegen unit taken from the cache: `cached codegen`.
    ObjectsRestored,
    /// A linker run.
    Links,
    /// The first start of an executable this run wrote as a new file, which
    /// macOS checks before it runs.
    NewExecutables,
    /// A process running tests, native or JavaScript.
    TestProcesses,
}

const NAMES: [&str; 8] = [
    "suites built",
    "suites restored",
    "suites reused",
    "objects compiled",
    "objects restored",
    "links",
    "new executables launched",
    "test processes",
];

static COUNTS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Executables this run wrote as new files and has not started yet.
static FRESH: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub fn add(count: Count) {
    if crate::profile::enabled() {
        if let Some(n) = COUNTS.get(count as usize) {
            n.fetch_add(1, Relaxed);
        }
    }
}

/// Counts what an `--explain` line says was done or reused.
pub fn explained(status: Status, action: Action) {
    let count = match (status, action) {
        (Status::Run, Action::Build) => Count::SuitesBuilt,
        (Status::Cached, Action::Build) => Count::SuitesRestored,
        (Status::Cached, Action::Test) => Count::SuitesReused,
        (Status::Run, Action::Codegen) => Count::ObjectsCompiled,
        (Status::Cached, Action::Codegen) => Count::ObjectsRestored,
        _ => return,
    };
    add(count);
}

/// Notes that `path` is an executable just written as a new file.
pub fn placed(path: &Path) {
    if crate::profile::enabled() {
        if let Ok(path) = std::fs::canonicalize(path) {
            FRESH.lock().unwrap_or_else(PoisonError::into_inner).push(path);
        }
    }
}

/// Notes that the executable [`placed`] noted at `from` now starts as `to`, or,
/// with no `to`, as a file an earlier run already started.
pub fn moved(from: &Path, to: Option<&Path>) {
    if !crate::profile::enabled() {
        return;
    }
    let Ok(from) = std::fs::canonicalize(from) else { return };
    let mut fresh = FRESH.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(at) = fresh.iter().position(|p| *p == from) else { return };
    fresh.swap_remove(at);
    if let Some(to) = to.and_then(|to| std::fs::canonicalize(to).ok()) {
        fresh.push(to);
    }
}

/// Counts a start of `program` that is the first since [`placed`] noted it.
pub fn started(program: &std::ffi::OsStr) {
    if !crate::profile::enabled() {
        return;
    }
    let Ok(program) = std::fs::canonicalize(program) else { return };
    let mut fresh = FRESH.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(at) = fresh.iter().position(|p| *p == program) {
        fresh.swap_remove(at);
        drop(fresh);
        add(Count::NewExecutables);
    }
}

/// One `<name> <count>` line per count, for the profile report.
pub fn lines() -> String {
    NAMES.iter().zip(&COUNTS).map(|(name, n)| format!("{name} {}\n", n.load(Relaxed))).collect()
}
