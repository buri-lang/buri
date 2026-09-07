//! **A CLI invocation that stops answering is a named failure, not a dead job.**
//!
//! `.config/nextest.toml` used to make this promise — `slow-timeout = { period
//! = "60s", terminate-after = 5 }`, a five-minute cap with the test's name on
//! it — and it could never keep it: nothing in this repository runs `nextest`,
//! and nothing it does run reads that file. Every suite in `ci.yml` is
//! `cargo test`, which has never opened `.config/`. Adopting the runner is not
//! the fix either — a second test runner is a second set of rules about what
//! counts as a skip, on top of the ones `cli/tests/ci.rs` already holds. The
//! config was deleted and this module is what replaced it.
//!
//! ## What it caps, exactly
//!
//! **One `buri` invocation made through [`super::run_in`] and its siblings.**
//! Every suite that drives the real binary goes through `run_in_full`, and this
//! is the wait inside it: the child is polled rather than joined, and a child
//! still running after the cap is killed, after which the wait **panics in the
//! test's own thread**. libtest prints `test <name> ... FAILED` for a panicking
//! test whatever else is in flight, so the report names the test, the argv, and
//! how long it waited — which is the whole of what the deleted config claimed.
//!
//! `fuzz.rs::run_watched` is the local precedent and stays where it is: it caps
//! the toolchain at thirty seconds and turns a hang into a *finding* about the
//! input rather than a failure of the suite, which is a different sentence from
//! the one below and belongs to that suite.
//!
//! **Not** an in-process deadlock, and not a `Command` a suite spawns for
//! itself: `native/` builds and links its own artifacts, `fuzz.rs` and
//! `build/hermeticity.rs` shell their own tools. Nothing in libtest tells a
//! watchdog which test is on which thread — the name is only knowable *inside*
//! the test, which is why the cap lives at the call and not in a thread above
//! it — so a cap over those would either name nothing or be one wrapper per
//! spawn site. The outer bound for everything this does not cover is the job's
//! `timeout-minutes`, which `cli/tests/ci.rs` now requires of every job.
//!
//! ## Stuck, not slow: what the cap actually measures
//!
//! **A child that is using the processor is working, whatever it has said.**
//! This was a plain wall clock — five minutes from the spawn, then kill — and a
//! wall clock cannot tell a stuck command from a slow one on a loaded machine.
//! Run 34121595426's arm64 Linux leg is what that costs: the leg runs sixteen
//! concurrent tests on four cores, and the cap killed a `buri build` that was
//! still minifying a fifty-thousand-arm match. The same build passed on every
//! idle host. A budget a busy machine can trip is measuring the machine.
//!
//! So the cap asks the operating system what the child's process *tree* has
//! spent on the processor, and kills only a child that has spent **nothing for
//! a whole cap period**. Five minutes of no CPU at all is a deadlock, a read
//! nothing is answering, or a wait on a child that already died. Five minutes
//! of a busy core is a slow build, and how slow a build may be is not this
//! module's question.
//!
//! The *tree*, because a build is mostly other people's processes: a `buri`
//! waiting on `bun`, on `cc`, on a linker has nothing of its own to spend, and
//! killing it for that would be the same mistake one level down. Output is not
//! counted separately: a child that writes a byte spends processor time to
//! write it, so "said nothing" is inside "spent nothing".
//!
//! [`cpu_time`] is the reading — `/proc/<pid>/stat` on Linux, `proc_pid_rusage`
//! and `proc_listchildpids` on macOS. On a host that will not say, the cap is
//! the wall clock it always was.
//!
//! What this gives up is the runaway that *spins*: a child looping forever
//! while burning a core is never killed here. The job's `timeout-minutes` ends
//! that one, as it already had to for an in-process deadlock, which this never
//! covered either.
//!
//! ## The numbers
//!
//! **Five minutes** with no processor time, the period the deleted file named,
//! overridable with `BURI_HANG_SECS`. The median invocation in this suite is a fraction of
//! a second and the slowest is seconds, so the margin is three orders of
//! magnitude. The override exists for the opposite direction — a test written
//! against a hang wants a cap it can reach — and
//! `the_cap_fires_and_names_what_it_killed` below reaches it with an argument
//! instead, so the ratchet is not itself a test of an environment variable.
//!
//! **One per cent of one core** over that period counts as spending it
//! ([`busy_enough`]). Low enough that a real build clears it by three orders of
//! magnitude even on a quarter of a starved core; high enough that a poll loop
//! is still a hang — the busiest wait inside `buri` wakes every five
//! milliseconds (`commands/test.rs`), which is a tenth of a per cent of one.

// Each test binary that includes this file uses a subset of it.
#![allow(dead_code)]

use std::io::Read;
use std::process::{Child, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

/// The cap one CLI invocation gets, from `BURI_HANG_SECS` or five minutes.
///
/// A value that does not parse is a typo in an environment variable, and
/// falling back to the default would hide it — so it is a panic and not a
/// shrug.
pub fn cap() -> Duration {
    match std::env::var("BURI_HANG_SECS") {
        Err(_) => Duration::from_secs(300),
        Ok(text) => match text.trim().parse::<u64>() {
            Ok(secs) => Duration::from_secs(secs),
            Err(e) => panic!("BURI_HANG_SECS={text:?} is not a number of seconds: {e}"),
        },
    }
}

/// How much processor time a tree has to spend inside one cap period to count
/// as working: one per cent of one core.
///
/// A fraction of the cap rather than a constant, so that a test which moves the
/// cap moves this with it and a short cap stays reachable.
fn busy_enough(cap: Duration) -> Duration {
    cap / 100
}

/// How often the tree's processor time is read: once a second, or four times
/// inside a cap shorter than four seconds.
///
/// A reading costs a directory of `/proc` or a handful of `libproc` calls, so
/// it is not something to do a hundred times a second — and nothing needs it
/// to be, since the answer is compared against a period measured in minutes.
/// The first reading is taken one interval in, so the invocations this suite is
/// made of — a fraction of a second, every one of them — take none at all.
fn sample_every(cap: Duration) -> Duration {
    (cap / 4).min(Duration::from_secs(1))
}

/// When the child was last seen to be working, and what it had spent by then.
struct Progress {
    /// The last moment the tree's processor time moved.
    at: Instant,
    /// The reading taken at that moment. `None` until the first one, and on a
    /// host that will not answer.
    spent: Option<Duration>,
    /// When to read again.
    next: Instant,
}

impl Progress {
    /// Fold one reading in.
    ///
    /// Three ways to be working, and the second is the one worth writing down.
    /// A reading that has *risen* by the threshold is the ordinary case. A
    /// reading that has *fallen* is a descendant that exited and took its time
    /// out of the sum — which is progress by any reading of the word, and
    /// treating it as silence would kill a build for finishing a subprocess.
    /// And a first reading is not evidence of anything, so it starts the clock
    /// rather than running against it.
    fn note(&mut self, now: Instant, reading: Option<Duration>, busy: Duration) {
        let Some(reading) = reading else { return };
        let moved = match self.spent {
            None => true,
            Some(before) => reading.checked_sub(before).is_none_or(|spent| spent >= busy),
        };
        if moved {
            self.at = now;
            self.spent = Some(reading);
        }
    }
}

/// Wait for a child, killing it and panicking once it has gone a whole `cap`
/// without spending processor time.
///
/// Polled with a backoff rather than blocked on, because a blocking `wait` is
/// exactly what has no deadline. From 200µs, doubling, topping out at 10ms: a
/// command that answers immediately pays microseconds, a command that answers
/// in a tenth of a second is noticed within a hundredth of one, and a command
/// that runs for a minute costs a hundred wakeups a second — nothing beside
/// the process it is waiting on. Measured, because the alternative is a tax on
/// every invocation in the suite: the `failing` corpus runs within a percent of
/// what it did before the cap went in.
pub fn wait_capped(child: &mut Child, what: &str, cap: Duration) -> ExitStatus {
    let pid = child.id();
    let start = Instant::now();
    let (busy, every) = (busy_enough(cap), sample_every(cap));
    let mut progress = Progress { at: start, spent: None, next: start + every };
    let mut nap = Duration::from_micros(200);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Err(e) => panic!("waiting for `{what}`: {e}"),
            Ok(None) => {}
        }
        let now = Instant::now();
        if now >= progress.next {
            progress.next = now + every;
            progress.note(now, cpu_time(pid), busy);
        }
        let idle = now.saturating_duration_since(progress.at);
        if idle >= cap {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{}", report(what, start.elapsed(), idle, progress.spent));
        }
        std::thread::sleep(nap.min(cap.saturating_sub(idle)));
        nap = (nap * 2).min(Duration::from_millis(10));
    }
}

/// Spawn, drain both pipes, and wait under the cap — `Command::output` with a
/// cap on it.
///
/// The pipes are read on threads of their own because a child that fills one
/// blocks writing to it, and a parent that waits before reading would then be
/// two processes waiting on each other — a deadlock the cap would report as a
/// hang, truthfully but unhelpfully. Standard input is closed, which is what
/// `output` does and what a CLI under test should see.
pub fn capped_output(mut cmd: std::process::Command, what: &str, cap: Duration) -> Output {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("`{what}` did not start: {e}"));
    let (out, err) = drain(&mut child);
    let status = wait_capped(&mut child, what, cap);
    Output { status, stdout: out.take(), stderr: err.take() }
}

/// Both of a child's pipes, being read on threads of their own.
pub fn drain(child: &mut Child) -> (Pipe, Pipe) {
    (Pipe::of(child.stdout.take()), Pipe::of(child.stderr.take()))
}

/// One pipe, read to the end on a thread, collected with [`Pipe::take`].
pub struct Pipe(Option<std::thread::JoinHandle<Vec<u8>>>);

impl Pipe {
    fn of<R: Read + Send + 'static>(stream: Option<R>) -> Self {
        Self(stream.map(|mut stream| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = stream.read_to_end(&mut bytes);
                bytes
            })
        }))
    }

    /// What the child wrote. Empty if the pipe was never piped, or if the
    /// reader died with it — a killed child's bytes are not the report.
    pub fn take(self) -> Vec<u8> {
        self.0.map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    }
}

/// The sentence a killed child leaves behind.
///
/// The test's name is read off the thread libtest runs it on, so it is in the
/// panic itself and not only in libtest's summary line — a `--nocapture` run
/// interleaves every binary's output, and "which test was that" is the first
/// thing a reader of one asks.
///
/// The evidence comes with it: how long the command ran, how long it went
/// without the processor, and what its tree had spent by the time it stopped
/// spending. A reader who thinks the verdict is wrong needs those three numbers
/// to say so.
fn report(what: &str, ran: Duration, idle: Duration, spent: Option<Duration>) -> String {
    let thread = std::thread::current();
    let who = thread.name().unwrap_or("an unnamed thread");
    let evidence = match spent {
        Some(spent) => format!(
            "It had been running {:.1}s, and its process tree had used {:.1}s of processor time \
             — none of it in the last {:.1}s.",
            ran.as_secs_f64(),
            spent.as_secs_f64(),
            idle.as_secs_f64()
        ),
        None => format!(
            "It had been running {:.1}s. This host does not report a process's processor time, so \
             the cap here is the wall clock it used to be everywhere, and a machine slow enough \
             can trip it.",
            ran.as_secs_f64()
        ),
    };
    format!(
        "the hang cap fired: `{what}` used no processor time for {:.1}s and was killed.\n\
         \n\
         The test is `{who}`. {evidence} A command that is not running is stuck rather than slow \
         — a deadlock, a read on a socket nothing is answering, or a wait on a child that already \
         died. Reproduce it with that argv alone; `BURI_HANG_SECS` moves the cap. Without this the \
         job would have run to its `timeout-minutes` and been killed by GitHub with no test named \
         at all.",
        idle.as_secs_f64()
    )
}

// ---------------------------------------------------------------------------
// What the machine says a process has spent
// ---------------------------------------------------------------------------

/// The processor time `pid` and its living descendants have used, where the
/// host will say.
///
/// **The whole tree, because a build is mostly other processes.** `buri`
/// waiting on `bun` or on a linker spends nothing itself, and a reading that
/// covered only the process this suite spawned would call every such wait a
/// hang.
///
/// `None` means the host would not answer — a platform with neither of the two
/// implementations below, or a process that exited between the wait and the
/// reading. A caller treats it as "no evidence" rather than as "no work":
/// [`Progress::note`] leaves its clock alone, so the cap degrades to the wall
/// clock it was before.
pub fn cpu_time(pid: u32) -> Option<Duration> {
    reading(pid)
}

/// `count` ticks, each worth `numer / denom` nanoseconds, as a duration.
///
/// Both hosts hand over a count of ticks and a fraction to scale it by — a
/// clock's hertz on Linux, mach's timebase on macOS — so the arithmetic is
/// written once. In `u128` because the multiplication is what overflows: a
/// tree's ticks times a billion leaves `u64` after a few years of processor
/// time, and a reading that silently wrapped would read as a hang.
fn scaled(count: u64, numer: u64, denom: u64) -> Option<Duration> {
    let nanos = u128::from(count).checked_mul(u128::from(numer))?.checked_div(u128::from(denom))?;
    Some(Duration::from_nanos(u64::try_from(nanos).ok()?))
}

/// How many processes one reading will walk before it gives up.
///
/// A build's tree is a handful. A number this size means the walk has found a
/// cycle that cannot exist, and a harness that spins inside a wait is worse
/// than one that reports nothing.
const TREE_LIMIT: usize = 4096;

/// Linux: `/proc/<pid>/stat`, summed over the subtree.
///
/// One pass over `/proc` reads every process's parent and its own times, and
/// the subtree is walked in memory afterwards — a second pass per generation
/// would be a different set of processes each time. `cutime`/`cstime` are in
/// the sum as well as `utime`/`stime`, so the time of a child that has already
/// been reaped stays counted where it landed: without them the sum would *fall*
/// every time a build finished a subprocess.
#[cfg(target_os = "linux")]
fn reading(pid: u32) -> Option<Duration> {
    // `_SC_CLK_TCK`. The times in `/proc` are in this clock's ticks — 100 a
    // second on every host this runs on, but asked rather than assumed.
    const SC_CLK_TCK: i32 = 2;
    unsafe extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    let hz = u64::try_from(unsafe { sysconf(SC_CLK_TCK) }).ok().filter(|hz| *hz > 0)?;

    // (the process, its parent, its own and its reaped children's ticks)
    let mut rows: Vec<(u32, u32, u64)> = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        // A process that exits mid-walk is ordinary, not an error.
        let Ok(text) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        let Some(row) = stat_row(id, &text) else { continue };
        rows.push(row);
    }

    let mut total: u64 = 0;
    let mut wanted = vec![pid];
    let mut walked = 0usize;
    while let Some(this) = wanted.pop() {
        walked = walked.saturating_add(1);
        if walked > TREE_LIMIT {
            break;
        }
        for (id, parent, spent) in &rows {
            if *id == this {
                total = total.saturating_add(*spent);
            }
            // `!= this` so that a process reported as its own parent — pid 1
            // under some sandboxes — is a leaf rather than a loop.
            if *parent == this && *id != this {
                wanted.push(*id);
            }
        }
    }
    scaled(total, 1_000_000_000, hz)
}

/// One line of `/proc/<pid>/stat`, as `(pid, ppid, utime + stime + cutime +
/// cstime)`.
///
/// Parsed from the **last** `)` rather than by splitting on spaces from the
/// left: the second field is the executable's name, in parentheses, and a
/// process is free to have a space or a bracket in it. Everything after that
/// bracket is fixed-width and positional, and the offsets here are the ones
/// `proc(5)` numbers 3, 4, 14, 15, 16 and 17.
#[cfg(target_os = "linux")]
fn stat_row(id: u32, text: &str) -> Option<(u32, u32, u64)> {
    let after = text.rsplit_once(')')?.1;
    // The state, the parent, and everything up to `cstime`. Index 0 here is
    // `proc(5)`'s field 3.
    let fields: Vec<&str> = after.split_whitespace().take(15).collect();
    // A negative count is not a thing a clock produces; it is a field this
    // reading has no use for, and zero is the honest answer for it.
    let number = |i: usize| {
        fields.get(i).and_then(|f| f.parse::<i64>().ok()).map(|n| u64::try_from(n).unwrap_or(0))
    };
    let parent = u32::try_from(number(1)?).ok()?;
    let spent = number(11)?
        .checked_add(number(12)?)?
        .checked_add(number(13)?)?
        .checked_add(number(14)?)?;
    Some((id, parent, spent))
}

/// macOS: `proc_pid_rusage` per process, with `proc_listchildpids` for the
/// tree.
///
/// The times come back in mach absolute ticks rather than in nanoseconds —
/// 24 MHz on Apple silicon, one tick per nanosecond on Intel — so
/// `mach_timebase_info` is what converts them, and a reading taken without it
/// is forty-one times too small on the machines this repository is written on.
/// Measured: a child spinning on one core for two seconds reports 48 million of
/// them.
///
/// Unlike Linux there is no reaped-children field here, so a tree's sum falls
/// when a subprocess is collected. [`Progress::note`] reads a fall as progress,
/// which is what it is.
#[cfg(target_os = "macos")]
fn reading(pid: u32) -> Option<Duration> {
    let mut total: u64 = 0;
    let mut wanted = vec![pid];
    let mut walked = 0usize;
    while let Some(this) = wanted.pop() {
        walked = walked.saturating_add(1);
        if walked > TREE_LIMIT {
            break;
        }
        if let Some(spent) = spent_by(this) {
            total = total.saturating_add(spent);
        }
        wanted.extend(children_of(this));
    }
    let (numer, denom) = timebase()?;
    scaled(total, u64::from(numer), u64::from(denom))
}

/// `RUSAGE_INFO_V0`'s buffer, in the order `<libproc.h>` declares it. Only the
/// first two numbers after the identifier are read; the rest is here so the
/// kernel writes into a buffer the size it expects.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Default)]
struct RusageInfoV0 {
    uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    pageins: u64,
    wired_size: u64,
    resident_size: u64,
    phys_footprint: u64,
    proc_start_abstime: u64,
    proc_exit_abstime: u64,
}

/// `mach_timebase_info_data_t`: the fraction that turns mach ticks into
/// nanoseconds.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Default)]
struct Timebase {
    numer: u32,
    denom: u32,
}

// The three calls this file makes into the system library, declared rather
// than depended on — `native/shared.rs`'s `kill` is the precedent here, and the
// argument is the same one: a dependency for a declaration is a dependency.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut std::ffi::c_void) -> i32;
    fn proc_listchildpids(ppid: i32, buffer: *mut std::ffi::c_void, size: i32) -> i32;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

/// One process's own mach ticks, or `None` if it is gone.
#[cfg(target_os = "macos")]
fn spent_by(pid: u32) -> Option<u64> {
    let pid = i32::try_from(pid).ok()?;
    let mut info = RusageInfoV0::default();
    let buffer: *mut RusageInfoV0 = &mut info;
    // `RUSAGE_INFO_V0`, the flavour every macOS since 10.9 answers.
    let answered = unsafe { proc_pid_rusage(pid, 0, buffer.cast()) };
    (answered == 0).then(|| info.user_time.saturating_add(info.system_time))
}

/// A process's immediate children.
///
/// The call answers with the **number of pids** it wrote, not a byte count, so
/// the return is clamped to the buffer rather than trusted: a version that
/// answered in bytes would otherwise walk three quarters of a buffer of zeros,
/// and pid 0 is not a process anything here can read.
#[cfg(target_os = "macos")]
fn children_of(pid: u32) -> Vec<u32> {
    let Ok(pid) = i32::try_from(pid) else { return Vec::new() };
    let mut kids = [0i32; 512];
    let size = i32::try_from(std::mem::size_of_val(&kids)).unwrap_or(i32::MAX);
    let answered = unsafe { proc_listchildpids(pid, kids.as_mut_ptr().cast(), size) };
    let found = usize::try_from(answered).unwrap_or(0).min(kids.len());
    kids.iter().take(found).filter_map(|k| u32::try_from(*k).ok()).filter(|k| *k > 0).collect()
}

/// The mach clock's fraction, asked once.
#[cfg(target_os = "macos")]
fn timebase() -> Option<(u32, u32)> {
    let mut info = Timebase::default();
    let answered = unsafe { mach_timebase_info(&mut info) };
    (answered == 0 && info.denom > 0 && info.numer > 0).then_some((info.numer, info.denom))
}

/// Any other host: no reading, and the cap is a wall clock.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn reading(_pid: u32) -> Option<Duration> {
    None
}

/// The cap, proved to fire on a child that will certainly not answer, and
/// proved not to on one that is working.
///
/// A gate that has never been seen to fire is a gate nobody can tell from a
/// no-op — which is the whole lesson of the file this module replaced. So:
/// `sleep 600` under a cap of a fifth of a second, and the progress rule fed by
/// hand. Every test here is fast and deterministic, and the set costs a fifth
/// of a second in each of the thirteen binaries that include this harness — so
/// the halves that need a real busy process live in `ci.rs`, which is one
/// binary, and are named from `work_is_a_reading_that_moved` below.
#[cfg(test)]
mod hang_tests {
    use super::*;

    #[test]
    fn the_cap_fires_and_names_what_it_killed() {
        let mut child = std::process::Command::new("sleep")
            .arg("600")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("`sleep` is on PATH");
        let id = child.id();
        let fired = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wait_capped(&mut child, "sleep 600", Duration::from_millis(200))
        }));

        let panic = fired.expect_err("a child that sleeps for ten minutes outlives a 200ms cap");
        let said = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(
            said.contains("the hang cap fired") && said.contains("sleep 600"),
            "the cap fired and did not say what it killed: {said:?}"
        );
        assert!(
            said.contains("the_cap_fires_and_names_what_it_killed"),
            "the cap fired and did not name the test it fired in, which is the one thing a \
             job-level timeout cannot do: {said:?}"
        );

        // Killed, not merely abandoned: `wait_capped` reaps the child before it
        // panics, so a second `wait` here would have nothing to reap. What is
        // asserted instead is that the process id is gone.
        let alive = std::process::Command::new("kill")
            .args(["-0", &id.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("`kill` is on PATH");
        assert!(!alive.success(), "the cap fired and left process {id} running");
    }

    /// A child that answers is waited for, not capped, and its status comes
    /// back — the other half of the claim, and the one every invocation in
    /// this suite depends on.
    #[test]
    fn a_child_that_answers_is_left_alone() {
        let mut cmd = std::process::Command::new("echo");
        cmd.arg("hello");
        let out = capped_output(cmd, "echo hello", cap());
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    /// **The rule the cap turns on, with the machine taken out of it.**
    ///
    /// A reading that rose by the threshold is work; one that stood still is
    /// not; one that *fell* is a descendant exiting, which is work too. Fed by
    /// hand rather than by a process, because a test that starves — and on a
    /// runner carrying sixteen tests on four cores, a spinning child can go a
    /// fifth of a second without the processor — would be the same flake one
    /// level down. `ci.rs::the_hang_cap_leaves_a_busy_child_alone` is the half
    /// that needs a real process, and it runs once rather than in every binary.
    #[test]
    fn work_is_a_reading_that_moved() {
        let cap = Duration::from_secs(300);
        let busy = busy_enough(cap);
        assert_eq!(busy, Duration::from_secs(3), "one per cent of five minutes");
        let start = Instant::now();
        let mut progress = Progress { at: start, spent: None, next: start };

        // The first reading starts the clock rather than running against it.
        let first = start + Duration::from_secs(1);
        progress.note(first, Some(Duration::from_secs(10)), busy);
        assert_eq!(progress.at, first);

        // Two seconds of work in a minute is a hang: the mark stays put.
        let quiet = first + Duration::from_secs(60);
        progress.note(quiet, Some(Duration::from_secs(12)), busy);
        assert_eq!(progress.at, first);

        // And the second that follows it counts, because what a reading is
        // compared against is the last mark rather than the last reading. A
        // tree spending a little the whole time is spending it.
        let later = quiet + Duration::from_secs(60);
        progress.note(later, Some(Duration::from_secs(13)), busy);
        assert_eq!(progress.at, later, "13s against a 10s mark is the 3s threshold, exactly");

        // A sum that fell is a subprocess that finished and took its time with
        // it. Reading that as silence would kill a build for making progress.
        let shrunk = later + Duration::from_secs(60);
        progress.note(shrunk, Some(Duration::from_secs(1)), busy);
        assert_eq!(progress.at, shrunk);

        // A host that will not say leaves the clock alone, so the cap falls
        // back to the wall clock it was before any of this.
        progress.note(shrunk + Duration::from_secs(60), None, busy);
        assert_eq!(progress.at, shrunk);
    }

    /// The default is the period the deleted `nextest.toml` named, and the
    /// override is read rather than remembered.
    ///
    /// Both arms assert. A test that returned quietly when the variable is set
    /// would be a skip, and this repository has a name for those.
    #[test]
    fn the_default_cap_is_the_period_the_deleted_config_named() {
        match std::env::var("BURI_HANG_SECS") {
            Err(_) => assert_eq!(cap(), Duration::from_secs(300)),
            Ok(text) => {
                let secs: u64 = text.trim().parse().expect("BURI_HANG_SECS is a number");
                assert_eq!(cap(), Duration::from_secs(secs));
            }
        }
    }
}
