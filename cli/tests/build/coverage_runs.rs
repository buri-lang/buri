//! `buri test --coverage` beside other runs of itself: a killed run takes its
//! test runners with it, and a run's report holds only its own run's counts
//! (buri-lang/buri#302).

use crate::harness::hang::CappedChild;
use crate::harness::{buri_command, ci, indent, tests_dir, Scratch};
use crate::held::Held;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

const SIGINT: i32 = 2;
const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;

unsafe extern "C" {
    fn dup2(from: i32, to: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn signal(sig: i32, handler: usize) -> usize;
}

/// A suite whose one test never ends.
fn endless(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write(
        "lib/endless/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/endless.buri\"]\n    }\n}\n",
    );
    scratch.write(
        "lib/endless/lib.buri",
        "/// Counts n down to zero, one step at a time.\n\
         export fn spin(n: Int): Int {\n    if (n == 0) { 0 } else { spin(n - 1) }\n}\n",
    );
    scratch.write(
        "lib/endless/test/endless.buri",
        "from \"core/testing/assert\" import * as assert;\n\
         from \"//lib/endless\" import { spin };\n\
         \n\
         test \"never ends\" {\n    assert.equal(spin(9000000000000000000), 0);\n}\n",
    );
    scratch
}

/// Kills `buri test --coverage` with `sig` once its test runner is running,
/// and answers whether every process it started ended with it.
///
/// Each process `buri` starts inherits a pipe's write end, and the read end
/// sees its end once the last of them is gone. The wait is bounded only so a
/// runner that outlives `buri` fails the test rather than hanging it.
fn killed_with(sig: i32, name: &str) {
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::coverage_runs", &gap);
        return;
    }
    const KEPT: i32 = 100;
    const F_SETFD: i32 = 2;
    const DEADLINE: Duration = Duration::from_secs(120);
    let scratch = endless(name);
    let held = Held::new(name);
    held.raise("handed", 1, &[u8::try_from(sig).unwrap()]);
    let (mut reader, writer) = std::io::pipe().unwrap();
    let mut command: Command = buri_command();
    command
        .args(["test", "//lib/endless", "--coverage"])
        .current_dir(&scratch.root)
        .env("BURI_HOLD", held.dir())
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(held.dir().join("stdout")).unwrap())
        .stderr(std::fs::File::create(held.dir().join("stderr")).unwrap())
        .process_group(0);
    let fd = std::os::fd::AsRawFd::as_raw_fd(&writer);
    // SAFETY: `signal`, `dup2` and `fcntl` are on POSIX's async-signal-safe
    // list. The signal is the default one whatever this process inherited, and
    // the pipe is kept across `exec`.
    unsafe {
        command.pre_exec(move || {
            signal(sig, 0);
            if dup2(fd, KEPT) < 0 || fcntl(KEPT, F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
    };
    let mut buri = command.spawn().expect("`buri` starts");
    drop(writer);
    let status: ExitStatus = buri.capped_wait().unwrap();
    assert_eq!(
        status.signal(),
        Some(sig),
        "`buri` was not killed once its runner ran: {status}\n{}",
        held.printed()
    );
    let (ended, ending) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = std::io::Read::read_to_end(&mut reader, &mut Vec::new());
        let _ = ended.send(());
    });
    if ending.recv_timeout(DEADLINE).is_err() {
        // SAFETY: an ordinary `kill` of the group `buri` was started in.
        unsafe { kill(-i32::try_from(buri.id()).unwrap(), SIGKILL) };
        panic!("a test runner outlived `buri test --coverage` killed by signal {sig}");
    }
}

/// `kill -INT` ends the runner with `buri`.
#[test]
fn an_interrupted_coverage_run_takes_its_runner_with_it() {
    killed_with(SIGINT, "coverage-runs-int");
}

/// `kill -TERM` ends the runner with `buri`.
#[test]
fn a_terminated_coverage_run_takes_its_runner_with_it() {
    killed_with(SIGTERM, "coverage-runs-term");
}

/// `kill -KILL`, which `buri` can't see, ends the runner with it too.
#[test]
fn a_killed_coverage_run_takes_its_runner_with_it() {
    killed_with(SIGKILL, "coverage-runs-kill");
}

const SHAPES: [&str; 3] = ["test", "//lib/shapes", "--coverage"];

fn shapes(name: &str) -> Scratch {
    Scratch::copy_of(
        name,
        &tests_dir().join("repositories/coverage/partly_covered/repo"),
    )
}

/// The report a run alone writes.
fn alone(scratch: &Scratch) -> String {
    let run = scratch.run(&SHAPES);
    assert_eq!(run.code, 0, "{}", indent(&run.all()));
    scratch.read(".buri/coverage/lcov.info")
}

/// Two runs at once in one checkout, the second started while the first's
/// runner waits and reporting after it: each report is its own run's.
#[test]
fn two_coverage_runs_at_once_each_report_their_own_counts() {
    let scratch = shapes("coverage-runs-beside");
    let want = alone(&scratch);
    let (first, second) = (
        Held::new("coverage-runs-first"),
        Held::new("coverage-runs-second"),
    );
    first.hold("handed", 1);
    second.hold("merging", 1);
    let mut one = first.start(&scratch, &SHAPES);
    let waiting = first.reached("handed", 1);
    let mut two = second.start(&scratch, &SHAPES);
    let merging = second.reached("merging", 1);
    waiting.go();
    assert!(one.capped_wait().unwrap().success(), "{}", first.printed());
    assert_eq!(
        scratch.read(".buri/coverage/lcov.info"),
        want,
        "the first run's report:\n{}",
        first.printed()
    );
    merging.go();
    assert!(two.capped_wait().unwrap().success(), "{}", second.printed());
    assert_eq!(
        scratch.read(".buri/coverage/lcov.info"),
        want,
        "the second run's report:\n{}",
        second.printed()
    );
}

/// A run killed before it reported leaves nothing behind once the next one
/// has run: `.buri/coverage` holds the report alone.
#[test]
fn a_killed_runs_counts_are_gone_after_the_next_run() {
    let scratch = shapes("coverage-runs-leftover");
    let held = Held::new("coverage-runs-leftover");
    held.raise("merging", 1, &[u8::try_from(SIGKILL).unwrap()]);
    let status = held.start(&scratch, &SHAPES).capped_wait().unwrap();
    assert_eq!(
        status.signal(),
        Some(SIGKILL),
        "`buri` was not killed before it reported: {status}\n{}",
        held.printed()
    );
    let want = alone(&scratch);
    let mut left: Vec<String> = std::fs::read_dir(scratch.path(".buri/coverage"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        ["lcov.info"],
        "what a coverage run leaves in .buri/coverage"
    );
    assert!(want.contains("SF:lib/shapes/shapes.buri"), "{want}");
}
