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

/// `phase`'s row of [`TOTALS`].
fn totals(phase: Phase) -> &'static [AtomicU64; 6] {
    let [other, parse, check, monomorphize, middle, emit, link, run] = &TOTALS;
    match phase {
        Phase::Other => other,
        Phase::Parse => parse,
        Phase::Check => check,
        Phase::Monomorphize => monomorphize,
        Phase::Middle => middle,
        Phase::Emit => emit,
        Phase::Link => link,
        Phase::Run => run,
    }
}

thread_local! {
    static CURRENT: Mutex<Option<(Phase, Reading)>> = const { Mutex::new(None) };
    static ALLOCATIONS: AtomicU64 = const { AtomicU64::new(0) };
}

/// This thread's phase and the reading taken when it started.
fn now_in() -> Option<(Phase, Reading)> {
    CURRENT
        .try_with(|c| *c.lock().unwrap_or_else(PoisonError::into_inner))
        .ok()
        .flatten()
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
            std::mem::replace(
                &mut *c.lock().unwrap_or_else(PoisonError::into_inner),
                next.map(|p| (p, now)),
            )
        })
        .ok()
        .flatten();
    collect::crossed(before.map(|(p, _)| p), next);
    if let Some((phase, then)) = before {
        let moved = [
            now.instructions.saturating_sub(then.instructions),
            now.cpu_ns.saturating_sub(then.cpu_ns),
            u64::try_from(now.at.duration_since(then.at).as_nanos()).unwrap_or(u64::MAX),
            now.allocations.saturating_sub(then.allocations),
        ];
        for (total, by) in totals(phase).iter().zip(moved) {
            total.fetch_add(by, Relaxed);
        }
    }
}

/// The source files this run lexed, each once, and their lines of code.
static SOURCES: Mutex<Option<std::collections::HashSet<(u64, usize)>>> = Mutex::new(None);
static SOURCE_LINES: AtomicU64 = AtomicU64::new(0);

/// Counts `text`'s lines of code toward the report, once per distinct text
/// however many times it's lexed. `spans` are its tokens' byte ranges. The
/// counting is charged to `other`, so it doesn't inflate `lex+parse`. Call it
/// only when [`enabled`].
pub fn lexed(text: &str, spans: impl Iterator<Item = (u32, u32)>) {
    let _counting = enter(Phase::Other);
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        (h.finish(), text.len())
    };
    let new = SOURCES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(Default::default)
        .insert(key);
    if new {
        SOURCE_LINES.fetch_add(code_lines(text, spans) as u64, Relaxed);
    }
}

/// The lines of `text` that hold part of a token: blank lines and lines of
/// nothing but comments don't count. `spans` are the tokens' byte ranges.
pub fn code_lines(text: &str, spans: impl Iterator<Item = (u32, u32)>) -> usize {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line_of = |at: usize| starts.partition_point(|&s| s <= at).saturating_sub(1);
    let mut code = vec![false; starts.len()];
    for (start, end) in spans.filter(|(s, e)| s < e) {
        let (first, last) = (line_of(start as usize), line_of(end as usize - 1));
        for line in code.get_mut(first..=last).into_iter().flatten() {
            *line = true;
        }
    }
    code.iter().filter(|&&c| c).count()
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
    let Some((instructions, cpu_ns)) = os::exited_child(pid) else {
        return;
    };
    let [.., child_instructions, child_cpu] = totals(phase);
    child_instructions.fetch_add(instructions, Relaxed);
    child_cpu.fetch_add(cpu_ns, Relaxed);
}

/// The instructions child `pid` retired, once it exits: macOS only, and `None`
/// elsewhere or where the kernel won't say. Waits like [`reaped`], and leaves
/// reaping to the caller.
pub fn exited_instructions(pid: u32) -> Option<u64> {
    os::exited_child(pid)
        .map(|(instructions, _)| instructions)
        .filter(|&n| n > 0)
}

thread_local! {
    /// What this thread's instructions are being added to, for [`counted`]. A
    /// thread no `counted` covers adds to one of its own that nothing reads.
    static TALLY: std::cell::RefCell<std::sync::Arc<AtomicU64>> = std::cell::RefCell::default();
}

/// `work`'s answer, and the instructions it retired on this thread and on
/// every `parallel` worker it started: macOS only, and 0 elsewhere or where
/// the kernel won't say.
pub fn counted<T>(work: impl FnOnce() -> T) -> (T, u64) {
    let tally = std::sync::Arc::new(AtomicU64::new(0));
    let out = tallied(std::sync::Arc::clone(&tally), work);
    (out, tally.load(Relaxed))
}

/// The tally this thread adds to, for a worker it starts to add to as well.
pub fn tally() -> std::sync::Arc<AtomicU64> {
    TALLY.with(|t| t.borrow().clone())
}

/// Runs `work`, adding the instructions this thread retires in it to `tally`.
pub fn tallied<T>(tally: std::sync::Arc<AtomicU64>, work: impl FnOnce() -> T) -> T {
    let previous = TALLY.with(|t| t.replace(std::sync::Arc::clone(&tally)));
    let before = thread_instructions();
    let out = work();
    tally.fetch_add(thread_instructions().saturating_sub(before), Relaxed);
    TALLY.with(|t| *t.borrow_mut() = previous);
    out
}

/// This thread's instructions retired so far: macOS only, and 0 elsewhere or
/// where the kernel won't say, such as in a virtual machine.
pub fn thread_instructions() -> u64 {
    os::thread_instructions()
}

/// The instructions each [`Steps::step`] of `round` retires on this thread,
/// the fewest over `rounds` rounds after a first round that counts nothing. 0s
/// where the kernel won't say.
///
/// The count includes the kernel's work on this thread, such as interrupts and
/// switching threads, and that work only ever adds. The first round takes lazy
/// setup and first touches, and a short step usually has one round the kernel
/// left alone (`design/PERFORMANCE.md` §8).
pub fn fewest(rounds: usize, mut round: impl FnMut(&mut Steps)) -> Vec<u64> {
    let mut steps = Steps::default();
    for counting in std::iter::once(false).chain(std::iter::repeat_n(true, rounds)) {
        (steps.counting, steps.at) = (counting, 0);
        round(&mut steps);
    }
    steps.fewest
}

/// One round of [`fewest`]: the steps it counts, in the same order each round.
#[derive(Default)]
pub struct Steps {
    counting: bool,
    at: usize,
    fewest: Vec<u64>,
}

impl Steps {
    /// Runs `work` as this round's next step.
    pub fn step<T>(&mut self, work: impl FnOnce() -> T) -> T {
        let before = thread_instructions();
        let out = work();
        let n = thread_instructions().saturating_sub(before);
        if self.counting {
            match self.fewest.get_mut(self.at) {
                Some(fewest) => *fewest = (*fewest).min(n),
                None => self.fewest.push(n),
            }
            self.at += 1;
        }
        out
    }
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
        return Guard {
            active: false,
            previous: None,
        };
    }
    let previous = now_in().map(|(p, _)| p);
    switch(Some(phase));
    Guard {
        active: true,
        previous,
    }
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
        .map(|&p| (p, totals(p).each_ref().map(|v| v.load(Relaxed))))
        .collect();
    let counted_instructions = os::thread_instructions() != 0;
    let counted_allocations = rows
        .iter()
        .any(|(_, [_, _, _, allocated, ..])| *allocated != 0);
    let counted_children = rows.iter().any(|(_, [.., cpu])| *cpu != 0);
    // A child's instructions come from the same counters as this thread's,
    // which a virtual machine doesn't have: there its CPU time is counted and
    // its instructions are not, and a 0 would read as a child that ran none.
    let counted_child_instructions = rows.iter().any(|(_, [.., n, _])| *n != 0);
    let columns = Columns {
        instructions: counted_instructions,
        allocations: counted_allocations,
        children: counted_children,
        child_instructions: counted_child_instructions,
    };
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
    out.push_str(&format!(
        "wall {}\n",
        seconds(u64::try_from(started.elapsed().as_nanos()).unwrap_or(0))
    ));
    out.push_str(&format!(
        "source: {} lines of code lexed, each file once\n",
        SOURCE_LINES.load(Relaxed)
    ));
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
    child_instructions: bool,
}

fn line(name: &str, row: &[u64; 6], columns: &Columns) -> String {
    let [count, cpu, busy, allocated, child_count, child_cpu] = *row;
    let count = if columns.instructions {
        mega(count)
    } else {
        "-".to_string()
    };
    let mut s = format!(
        "  {name:<14} {count:>17} {:>10} {:>10}",
        seconds(cpu),
        seconds(busy)
    );
    if columns.allocations {
        s.push_str(&format!(" {allocated:>13}"));
    }
    if columns.children {
        let child_count = if columns.child_instructions {
            mega(child_count)
        } else {
            "-".to_string()
        };
        s.push_str(&format!(" {child_count:>19} {:>10}", seconds(child_cpu)));
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

/// `BURI_PROFILE_COLLECT=<group>`: callgrind counts only while a thread is in
/// one of the group's phases, for `cargo bench --bench compiler -- --goals`
/// (`design/PERFORMANCE.md` §6.82). Run under
/// `valgrind --tool=callgrind --collect-atstart=no`, which starts every thread
/// not counting. The groups are `parse`, `check` and `other`, which takes in
/// `run`: the test runner's own work around the tests a `buri test` starts.
mod collect {
    use super::Phase;
    use std::sync::OnceLock;

    fn group() -> Option<&'static [Phase]> {
        static GROUP: OnceLock<Option<&'static [Phase]>> = OnceLock::new();
        *GROUP.get_or_init(
            || match std::env::var("BURI_PROFILE_COLLECT").ok()?.as_str() {
                "parse" => Some(&[Phase::Parse]),
                "check" => Some(&[Phase::Check]),
                "other" => Some(&[Phase::Other, Phase::Run]),
                _ => None,
            },
        )
    }

    /// Turns this thread's counting on or off as it moves from `from` to `to`.
    pub fn crossed(from: Option<Phase>, to: Option<Phase>) {
        let Some(group) = group() else {
            return;
        };
        let inside = |p: Option<Phase>| p.is_some_and(|p| group.contains(&p));
        if inside(from) != inside(to) {
            toggle();
        }
    }

    /// Callgrind's `CALLGRIND_TOGGLE_COLLECT`, which flips counting for the
    /// calling thread. Outside valgrind the sequence changes no register and
    /// does nothing.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn toggle() {
        let request: [u64; 6] = [TOGGLE_COLLECT, 0, 0, 0, 0, 0];
        // SAFETY: valgrind's client-request preamble. The four rotations
        // total 128 bits, so `rdi` ends as it began, and `xchg rbx, rbx`
        // swaps a register with itself. Valgrind reads `request` through
        // `rax` and writes its answer to `rdx`.
        unsafe {
            std::arch::asm!(
                "rol rdi, 3",
                "rol rdi, 13",
                "rol rdi, 61",
                "rol rdi, 51",
                "xchg rbx, rbx",
                in("rax") request.as_ptr(),
                inout("rdx") 0u64 => _,
                options(nostack),
            );
        }
    }

    /// [`toggle`]'s arm64 form: the answer is in `x3`, the request in `x4`.
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    fn toggle() {
        let request: [u64; 6] = [TOGGLE_COLLECT, 0, 0, 0, 0, 0];
        // SAFETY: as for x86_64. The rotations of `x12` total 128 bits and
        // `orr x10, x10, x10` writes `x10` back unchanged.
        unsafe {
            std::arch::asm!(
                "ror x12, x12, #3",
                "ror x12, x12, #13",
                "ror x12, x12, #51",
                "ror x12, x12, #61",
                "orr x10, x10, x10",
                in("x4") request.as_ptr(),
                inout("x3") 0u64 => _,
                options(nostack),
            );
        }
    }

    #[cfg(not(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )))]
    fn toggle() {}

    /// `VG_USERREQ_TOOL_BASE('C', 'T') + 2`, from valgrind's `callgrind.h`.
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    const TOGGLE_COLLECT: u64 = 0x4354_0002;
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
    /// Where it is missing, a stand-in writes nothing, as a failed call does.
    fn self_counts() -> SelfCounts {
        static FOUND: OnceLock<SelfCounts> = OnceLock::new();
        *FOUND.get_or_init(|| {
            const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
            // SAFETY: a lookup by a NUL-terminated name in the loaded images.
            let f = unsafe { dlsym(RTLD_DEFAULT, c"thread_selfcounts".as_ptr()) };
            // SAFETY: the kernel's `thread_selfcounts(int, void *, size_t)`.
            let found = (!f.is_null())
                .then(|| unsafe { std::mem::transmute::<*mut c_void, SelfCounts>(f) });
            found.unwrap_or(missing)
        })
    }

    extern "C" fn missing(_kind: i32, _counts: *mut u64, _size: usize) -> i32 {
        -1
    }

    /// This thread's instructions retired, or 0 where the kernel won't say.
    pub fn thread_instructions() -> u64 {
        let f = self_counts();
        let mut counts = [0u64; 2];
        // SAFETY: kind 1 fills instructions and cycles, two `u64`s. A call that
        // fails, as it does in a virtual machine, writes nothing and leaves 0.
        let _ = unsafe { f(1, counts.as_mut_ptr(), std::mem::size_of_val(&counts)) };
        let [instructions, _cycles] = counts;
        instructions
    }

    pub fn thread_cpu_ns() -> u64 {
        const CLOCK_THREAD_CPUTIME_ID: u32 = 16;
        // SAFETY: reads a clock; touches no memory of ours.
        unsafe { clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) }
    }

    /// `rusage_info_v4` for `pid`, and a converter from its ticks to
    /// nanoseconds. `None` where the kernel won't say.
    fn rusage(pid: i32) -> Option<([u64; 37], impl Fn(u64) -> u64)> {
        const RUSAGE_INFO_V4: i32 = 4;
        let mut info = [0u64; 37];
        // SAFETY: `rusage_info_v4` is 37 `u64`s (a 16-byte UUID, then 35 counters).
        let status = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V4, info.as_mut_ptr()) };
        (status == 0).then_some(())?;
        let mut base = [0u32; 2];
        // SAFETY: fills a `{ numer, denom }` pair.
        unsafe { mach_timebase_info(&mut base) };
        let [numer, denom] = base;
        let ns = move |ticks: u64| {
            (u128::from(ticks) * u128::from(numer) / u128::from(denom.max(1))) as u64
        };
        Some((info, ns))
    }

    /// Waits for child `pid` to exit without reaping it, then reads its
    /// instructions and CPU nanoseconds. Best effort: `None` where the wait
    /// or the read fails.
    pub fn exited_child(pid: u32) -> Option<(u64, u64)> {
        const P_PID: i32 = 1;
        const WEXITED: i32 = 4;
        const WNOWAIT: i32 = 32;
        let mut info = [0u64; 16];
        // SAFETY: `info` is larger than the 104-byte `siginfo_t`.
        let status = unsafe { waitid(P_PID, pid, &mut info, WEXITED | WNOWAIT) };
        (status == 0).then_some(())?;
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
        // SAFETY: fills a 64-bit `timespec`. A call that fails leaves it zero,
        // which reads as 0 below.
        unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut t) };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A virtual machine counts a child's CPU time and none of its
    /// instructions, and the row says so rather than reading 0.
    #[test]
    fn a_child_whose_instructions_went_uncounted_shows_a_dash() {
        let columns = Columns {
            instructions: false,
            allocations: false,
            children: true,
            child_instructions: false,
        };
        let row = line("run", &[0, 0, 0, 0, 0, 8_000_000], &columns);
        let words: Vec<&str> = row.split_whitespace().collect();
        assert_eq!(words[words.len() - 3..], ["-", "0.008", "s"], "{row}");
    }

    #[test]
    fn a_child_whose_instructions_were_counted_shows_them() {
        let columns = Columns {
            instructions: true,
            allocations: false,
            children: true,
            child_instructions: true,
        };
        let row = line("run", &[0, 0, 0, 0, 2_500_000, 8_000_000], &columns);
        assert!(row.contains(" 2.5 M "), "{row}");
    }
}
