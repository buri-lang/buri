//! `BURI_PROFILE=1`: where one run's work went, by compiler phase.
//!
//! Each thread keeps the phase it is in. Whenever that changes, the thread
//! reads its own counters and adds what they moved by to the phase it is
//! leaving. Reading per thread is what makes the split right when phases run
//! side by side: `buri test` checks one suite while it links another.
//!
//! The counters are the thread's instructions retired (macOS only), its CPU
//! time, the wall time it spent in the phase, and its allocations when the
//! binary was built with `--features alloc-counter`. A child process the phase
//! waited for adds its own instructions and CPU time (macOS only), since a
//! linker's or a test binary's work is none of this process's threads'. Instructions don't move
//! with the machine's load, which is the point: one run gives a figure that
//! two toolchains can be compared by (`design/PERFORMANCE.md` §8).
//!
//! Off, a phase change is one load of a flag that was read once.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "counter deltas and sums over one process's lifetime; a wrapped \
              figure is a wrong profile line, never a wrong build"
)]

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

/// What a thread is doing. `Other` is everything outside a named phase:
/// loading the workspace, hashing for the action cache, reading and writing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Other,
    Parse,
    Check,
    Monomorphize,
    Middle,
    Emit,
    Link,
    Run,
}

const PHASES: usize = 8;

impl Phase {
    const ALL: [Phase; PHASES] = [
        Phase::Parse,
        Phase::Check,
        Phase::Monomorphize,
        Phase::Middle,
        Phase::Emit,
        Phase::Link,
        Phase::Run,
        Phase::Other,
    ];

    fn name(self) -> &'static str {
        match self {
            Phase::Other => "other",
            Phase::Parse => "lex+parse",
            Phase::Check => "check",
            Phase::Monomorphize => "monomorphize",
            Phase::Middle => "middle",
            Phase::Emit => "emit",
            Phase::Link => "link",
            Phase::Run => "run",
        }
    }
}

/// Whether `BURI_PROFILE` is set to something other than empty or `0`.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BURI_PROFILE").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// One thread's counters at one moment.
#[derive(Clone, Copy)]
struct Reading {
    instructions: u64,
    cpu_ns: u64,
    at: Instant,
    allocations: u64,
}

/// Per phase: instructions, CPU nanoseconds, wall nanoseconds, allocations, and
/// the instructions and CPU nanoseconds of the child processes it waited for.
static TOTALS: [[AtomicU64; 6]; PHASES] = [const { [const { AtomicU64::new(0) }; 6] }; PHASES];

thread_local! {
    static CURRENT: Mutex<Option<(Phase, Reading)>> = const { Mutex::new(None) };
    static ALLOCATIONS: AtomicU64 = const { AtomicU64::new(0) };
}

/// This thread's phase and the reading taken when it started.
fn now_in() -> Option<(Phase, Reading)> {
    CURRENT.try_with(|c| *c.lock().unwrap_or_else(PoisonError::into_inner)).ok().flatten()
}

/// Counts one allocation on this thread, which is the only writer.
fn count_allocation() {
    let _ = ALLOCATIONS.try_with(|n| n.store(n.load(Relaxed).wrapping_add(1), Relaxed));
}

/// Adds what this thread did since its last change to the phase it was in, and
/// puts it in `next`.
fn switch(next: Option<Phase>) {
    let now = read();
    let before = CURRENT
        .try_with(|c| {
            std::mem::replace(&mut *c.lock().unwrap_or_else(PoisonError::into_inner), next.map(|p| (p, now)))
        })
        .ok()
        .flatten();
    if let Some((phase, then)) = before {
        if let Some(row) = TOTALS.get(phase as usize) {
            let moved = [
                now.instructions.saturating_sub(then.instructions),
                now.cpu_ns.saturating_sub(then.cpu_ns),
                u64::try_from(now.at.duration_since(then.at).as_nanos()).unwrap_or(u64::MAX),
                now.allocations.saturating_sub(then.allocations),
            ];
            for (total, by) in row.iter().zip(moved) {
                total.fetch_add(by, Relaxed);
            }
        }
    }
}

/// Charges child `pid`'s instructions and CPU time to this thread's phase.
///
/// Call it once the child's output is drained and before reaping it: it waits
/// for the child to exit, and leaves reaping to the caller.
pub fn reaped(pid: u32) {
    if !enabled() {
        return;
    }
    // A thread outside any phase, such as one `std::thread::spawn` started.
    let phase = current().unwrap_or(Phase::Other);
    let Some((instructions, cpu_ns)) = os::exited_child(pid) else { return };
    if let Some([.., child_instructions, child_cpu]) = TOTALS.get(phase as usize) {
        child_instructions.fetch_add(instructions, Relaxed);
        child_cpu.fetch_add(cpu_ns, Relaxed);
    }
}

/// This thread's instructions retired so far: macOS only, and 0 elsewhere or
/// where the kernel won't say, such as in a virtual machine.
pub fn thread_instructions() -> u64 {
    os::thread_instructions()
}

/// This process's instructions retired so far, every thread's, those that
/// have exited included: macOS only, and 0 elsewhere.
pub fn process_instructions() -> u64 {
    os::process().map_or(0, |p| p.instructions)
}

/// The phase this thread is in, for a worker to start in.
pub fn current() -> Option<Phase> {
    if !enabled() {
        return None;
    }
    now_in().map(|(p, _)| p)
}

/// Puts this thread in `phase` until the guard drops, then back in the phase
/// it was in before (or in none, on a worker's first).
#[must_use = "the phase ends when the guard drops"]
pub fn enter(phase: Phase) -> Guard {
    if !enabled() {
        return Guard { active: false, previous: None };
    }
    let previous = now_in().map(|(p, _)| p);
    switch(Some(phase));
    Guard { active: true, previous }
}

/// [`enter`] for a worker: the phase its spawner was in, or nothing.
pub fn adopt(phase: Option<Phase>) -> Option<Guard> {
    phase.map(enter)
}

pub struct Guard {
    active: bool,
    previous: Option<Phase>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.active {
            switch(self.previous);
        }
    }
}

/// The global allocator `buri` installs under `--features alloc-counter`. It
/// counts on the allocating thread, so the count lands in that thread's phase.
pub struct Counting;

// SAFETY: every call is forwarded unchanged to the system allocator. Bumping a
// thread-local `AtomicU64` allocates nothing and registers no destructor.
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        count_allocation();
        // SAFETY: the caller's contract, passed through.
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        count_allocation();
        // SAFETY: the caller's contract, passed through.
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        count_allocation();
        // SAFETY: the caller's contract, passed through.
        unsafe { std::alloc::System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: the caller's contract, passed through.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

fn read() -> Reading {
    Reading {
        instructions: os::thread_instructions(),
        cpu_ns: os::thread_cpu_ns(),
        at: Instant::now(),
        allocations: ALLOCATIONS.try_with(|n| n.load(Relaxed)).unwrap_or(0),
    }
}

/// The table, or `None` when profiling is off. Call it once, at the end of the
/// command, from the thread that ran it.
pub fn report(started: Instant) -> Option<String> {
    if !enabled() {
        return None;
    }
    // Close this thread's open phase so its last stretch is counted.
    let open = current();
    switch(open);
    let rows: Vec<(Phase, [u64; 6])> = Phase::ALL
        .iter()
        .map(|&p| {
            let row = TOTALS.get(p as usize).map(|r| r.each_ref().map(|v| v.load(Relaxed)));
            (p, row.unwrap_or([0; 6]))
        })
        .collect();
    let counted_instructions = os::thread_instructions() != 0;
    let counted_allocations = rows.iter().any(|(_, [_, _, _, allocated, ..])| *allocated != 0);
    let counted_children = rows.iter().any(|(_, [.., cpu])| *cpu != 0);
    let columns = Columns { instructions: counted_instructions, allocations: counted_allocations, children: counted_children };
    let mut out = String::from("buri profile           instructions        cpu       busy");
    if counted_allocations {
        out.push_str("   allocations");
    }
    if counted_children {
        out.push_str("  child instructions  child cpu");
    }
    out.push('\n');
    let mut sum = [0u64; 6];
    for (phase, row) in &rows {
        if row.iter().all(|&v| v == 0) {
            continue;
        }
        for (s, v) in sum.iter_mut().zip(row) {
            *s += v;
        }
        out.push_str(&line(phase.name(), row, &columns));
    }
    out.push_str(&line("all phases", &sum, &columns));
    out.push_str(&format!("wall {}\n", seconds(u64::try_from(started.elapsed().as_nanos()).unwrap_or(0))));
    if let Some(p) = os::process() {
        out.push_str(&format!(
            "process: {} instructions, {} cpu, {} runnable but not running, peak {} MB\n\
             child processes: {} cpu\n",
            giga(p.instructions),
            seconds(p.cpu_ns),
            seconds(p.runnable_ns),
            p.peak_bytes / 1_000_000,
            seconds(p.child_cpu_ns),
        ));
    }
    Some(out)
}

/// Which optional columns the table has.
struct Columns {
    instructions: bool,
    allocations: bool,
    children: bool,
}

fn line(name: &str, row: &[u64; 6], columns: &Columns) -> String {
    let [count, cpu, busy, allocated, child_count, child_cpu] = *row;
    let count = if columns.instructions { mega(count) } else { "-".to_string() };
    let mut s = format!("  {name:<14} {count:>17} {:>10} {:>10}", seconds(cpu), seconds(busy));
    if columns.allocations {
        s.push_str(&format!(" {allocated:>13}"));
    }
    if columns.children {
        s.push_str(&format!(" {:>19} {:>10}", mega(child_count), seconds(child_cpu)));
    }
    s.push('\n');
    s
}

fn mega(n: u64) -> String {
    format!("{:.1} M", n as f64 / 1e6)
}

fn giga(n: u64) -> String {
    format!("{:.3} G", n as f64 / 1e9)
}

fn seconds(ns: u64) -> String {
    format!("{:.3} s", ns as f64 / 1e9)
}

/// What the whole process did, from the kernel's own accounting.
struct Process {
    instructions: u64,
    cpu_ns: u64,
    child_cpu_ns: u64,
    runnable_ns: u64,
    peak_bytes: u64,
}

#[cfg(target_os = "macos")]
mod os {
    use std::ffi::{c_char, c_void};
    use std::sync::OnceLock;

    type SelfCounts = unsafe extern "C" fn(i32, *mut u64, usize) -> i32;

    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn clock_gettime_nsec_np(clock: u32) -> u64;
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u64) -> i32;
        fn getpid() -> i32;
        fn mach_timebase_info(info: *mut [u32; 2]) -> i32;
        fn waitid(idtype: i32, id: u32, info: *mut [u64; 16], options: i32) -> i32;
    }

    /// `thread_selfcounts`, looked up rather than linked: it isn't public API,
    /// and a binary that named it would fail to start on a system without it.
    fn self_counts() -> Option<SelfCounts> {
        static FOUND: OnceLock<Option<SelfCounts>> = OnceLock::new();
        *FOUND.get_or_init(|| {
            const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
            // SAFETY: a lookup by a NUL-terminated name in the loaded images.
            let f = unsafe { dlsym(RTLD_DEFAULT, c"thread_selfcounts".as_ptr()) };
            // SAFETY: the kernel's `thread_selfcounts(int, void *, size_t)`.
            (!f.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, SelfCounts>(f) })
        })
    }

    /// This thread's instructions retired, or 0 where the kernel won't say.
    pub fn thread_instructions() -> u64 {
        let Some(f) = self_counts() else { return 0 };
        let mut counts = [0u64; 2];
        // SAFETY: kind 1 fills instructions and cycles, two `u64`s.
        let status = unsafe { f(1, counts.as_mut_ptr(), std::mem::size_of_val(&counts)) };
        let [instructions, _cycles] = counts;
        if status == 0 { instructions } else { 0 }
    }

    pub fn thread_cpu_ns() -> u64 {
        const CLOCK_THREAD_CPUTIME_ID: u32 = 16;
        // SAFETY: reads a clock; touches no memory of ours.
        unsafe { clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) }
    }

    /// `rusage_info_v4` for `pid`, and a converter from its ticks to nanoseconds.
    fn rusage(pid: i32) -> Option<([u64; 37], impl Fn(u64) -> u64)> {
        const RUSAGE_INFO_V4: i32 = 4;
        let mut info = [0u64; 37];
        // SAFETY: `rusage_info_v4` is 37 `u64`s (a 16-byte UUID, then 35 counters).
        if unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V4, info.as_mut_ptr()) } != 0 {
            return None;
        }
        let mut base = [0u32; 2];
        // SAFETY: fills a `{ numer, denom }` pair.
        unsafe { mach_timebase_info(&mut base) };
        let [numer, denom] = base;
        Some((info, move |ticks: u64| (u128::from(ticks) * u128::from(numer) / u128::from(denom.max(1))) as u64))
    }

    /// Waits for child `pid` to exit without reaping it, then reads its
    /// instructions and CPU nanoseconds.
    pub fn exited_child(pid: u32) -> Option<(u64, u64)> {
        const P_PID: i32 = 1;
        const WEXITED: i32 = 4;
        const WNOWAIT: i32 = 32;
        let mut info = [0u64; 16];
        // SAFETY: `info` is larger than the 104-byte `siginfo_t`.
        if unsafe { waitid(P_PID, pid, &mut info, WEXITED | WNOWAIT) } != 0 {
            return None;
        }
        let (info, ns) = rusage(i32::try_from(pid).ok()?)?;
        let at = |i: usize| info.get(i).copied().unwrap_or(0);
        Some((at(31), ns(at(2) + at(3))))
    }

    pub(super) fn process() -> Option<super::Process> {
        // SAFETY: no arguments, no memory.
        let (info, ns) = rusage(unsafe { getpid() })?;
        let at = |i: usize| info.get(i).copied().unwrap_or(0);
        Some(super::Process {
            instructions: at(31),
            cpu_ns: ns(at(2) + at(3)),
            child_cpu_ns: ns(at(12) + at(13)),
            runnable_ns: ns(at(36)),
            peak_bytes: at(30),
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    unsafe extern "C" {
        fn clock_gettime(clock: i32, out: *mut [i64; 2]) -> i32;
    }

    pub fn thread_instructions() -> u64 {
        0
    }

    pub fn thread_cpu_ns() -> u64 {
        const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
        let mut t = [0i64; 2];
        // SAFETY: fills a 64-bit `timespec`.
        if unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut t) } != 0 {
            return 0;
        }
        let [secs, nanos] = t;
        u64::try_from(secs).unwrap_or(0) * 1_000_000_000 + u64::try_from(nanos).unwrap_or(0)
    }

    pub fn exited_child(_pid: u32) -> Option<(u64, u64)> {
        None
    }

    pub(super) fn process() -> Option<super::Process> {
        None
    }
}
