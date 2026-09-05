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
//! ## The number
//!
//! Five minutes, the period the deleted file named, overridable with
//! `BURI_HANG_SECS`. The median invocation in this suite is a fraction of a
//! second and the slowest is seconds, so the margin is three orders of
//! magnitude: a `buri` that has said nothing for five minutes is not slow, it
//! is stuck. The override exists for the opposite direction — a test written
//! against a hang wants a cap it can reach — and `the_cap_fires_and_names_what_it_killed`
//! below reaches it with an argument instead, so the ratchet is not itself a
//! test of an environment variable.

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

/// Wait for a child, killing it and panicking if it outlives `cap`.
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
    let start = Instant::now();
    let mut nap = Duration::from_micros(200);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Err(e) => panic!("waiting for `{what}`: {e}"),
            Ok(None) => {}
        }
        let waited = start.elapsed();
        if waited >= cap {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{}", report(what, waited));
        }
        std::thread::sleep(nap.min(cap.saturating_sub(waited)));
        nap = (nap * 2).min(Duration::from_millis(10));
    }
}

/// Spawn, drain both pipes, and wait under the cap — `Command::output` with a
/// deadline.
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
fn report(what: &str, waited: Duration) -> String {
    let thread = std::thread::current();
    let who = thread.name().unwrap_or("an unnamed thread");
    format!(
        "the hang cap fired: `{what}` said nothing for {:.0}s and was killed.\n\
         \n\
         The test is `{who}`. One CLI invocation in this suite takes a fraction of a second, so \
         this is a command that is stuck rather than one that is slow — a deadlock, a read on a \
         socket nothing is answering, or a wait on a child that already died. Reproduce it with \
         that argv alone; `BURI_HANG_SECS` moves the cap if the command really does need longer. \
         Without this the job would have run to its `timeout-minutes` and been killed by GitHub \
         with no test named at all.",
        waited.as_secs_f64()
    )
}

/// The cap, proved to fire, on a child that will certainly not answer.
///
/// A gate that has never been seen to fire is a gate nobody can tell from a
/// no-op — which is the whole lesson of the file this module replaced. So:
/// `sleep 600` under a cap of a fifth of a second, and three claims about what
/// comes back. Fast, deterministic, and it costs a fifth of a second in each
/// binary that includes this harness.
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
