//! `process.start`: a child started without waiting, signalled, and waited on
//! later (buri-lang/buri#277).
//!
//! One program, `started.buri`, run natively under the heap check and on the
//! JavaScript backend, and both must print the same lines. Its first argument
//! picks a scenario. Nothing in it or here sleeps to sequence anything: children
//! say they are ready through a FIFO, and the program lets them go through
//! another.
//!
//! The program's own streams go to files rather than pipes, so a child a failed
//! run left behind cannot hold this harness open.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

const SOURCE: &str = include_str!("started.buri");

/// Long enough for a loaded machine; a run that takes longer is hung.
const DEADLINE: Duration = Duration::from_secs(120);

/// The native executable, built once.
fn native() -> Option<&'static Path> {
    static BUILT: OnceLock<Option<PathBuf>> = OnceLock::new();
    BUILT.get_or_init(|| crate::e2e::ready().then(|| crate::e2e::built("started", SOURCE))).as_deref()
}

/// The JavaScript artifact and the engine to run it, built once.
fn javascript() -> Option<&'static (String, PathBuf)> {
    static BUILT: OnceLock<Option<(String, PathBuf)>> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let engine = crate::shared::js_engine()?;
            let (checked, paths) = crate::agreement::analyze("started", SOURCE);
            Some((engine, crate::agreement::emit_js("started", &checked, &paths)))
        })
        .as_ref()
}

fn scratch(name: &str) -> PathBuf {
    crate::sweep::once();
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("started-{}", std::process::id()))
        .join(format!("{name}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Which backend a run is on.
#[derive(Clone, Copy, Debug)]
enum On {
    Native,
    JavaScript,
}

/// Runs one scenario and answers its standard output, having asserted it
/// exited 0 within the deadline and, natively, gave back every block.
fn ran(on: On, mode: &str) -> Option<String> {
    let mut command = match on {
        On::Native => {
            let binary = native()?;
            let mut command =
                crate::valgrind::command(binary, "process.start").unwrap_or_else(|| Command::new(binary));
            crate::shared::heap_checked(&mut command);
            command
        }
        On::JavaScript => {
            let (engine, artifact) = javascript()?;
            let mut command = Command::new(engine);
            command.arg(artifact);
            command
        }
    };
    let dir = scratch(&format!("{mode}-{on:?}"));
    let out = dir.join("program.stdout");
    let err = dir.join("program.stderr");
    command
        .arg(mode)
        .arg(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&err).unwrap());
    let mut child = command.spawn().unwrap();
    let status = crate::shared::waited(&mut child, DEADLINE);
    let stdout = std::fs::read_to_string(&out).unwrap();
    let stderr = std::fs::read_to_string(&err).unwrap();
    assert_eq!(
        status.code(),
        Some(0),
        "{mode} on {on:?} failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Some(stdout)
}

/// The scenario's output on both backends, each asserted equal to `expected`.
fn both(mode: &str, expected: &str) {
    for on in [On::Native, On::JavaScript] {
        if let Some(stdout) = ran(on, mode) {
            assert_eq!(stdout, expected, "{mode} on {on:?}");
        }
    }
}

const HARNESS: &str = "\
a started, id matches true
b started, id matches true
c started, id matches true
a tryWait None
b tryWait None
c tryWait None
b signal Stop ok
b is stopped
b tryWait None
b signal Continue ok
b is running
a signal Kill ok
a wait Signaled(Kill)
b tryWait None
c tryWait None
b signal Terminate ok
b wait Signaled(Terminate)
c tryWait None
a again started, id matches true
a again has a new handle true
a again tryWait None
c signal Interrupt ok
c wait Signaled(Interrupt)
a again signal Other(1) ok
a again wait Signaled(Other(1))
a wait Signaled(Kill)
a tryWait Some(Signaled(Kill))
a signal Kill ok
";

const STUBBORN: &str = "\
stubborn started, id matches true
stubborn signal Terminate ok
stubborn tryWait None
stubborn signal Kill ok
stubborn wait Signaled(Kill)
";

const PAUSED: &str = "\
paused started, id matches true
paused signal Stop ok
paused tryWait None
paused signal Continue ok
paused wait Exited(5)
";

fn concurrent_expected() -> String {
    let statuses: Vec<String> = (0..16).map(|i| format!("Exited({i})")).collect();
    format!("released true {}\n", statuses.join(" "))
}

/// The issue's own harness: three long-running children, stopped, continued,
/// killed, terminated and interrupted, each waited on while the others run,
/// and one restarted. A second wait answers the first's status, and a signal
/// to a child already waited on is sent nowhere.
#[test]
fn started_children_are_signalled_and_waited_on_one_at_a_time() {
    both("harness", HARNESS);
}

/// A child that ignores `SIGTERM` is still running after one, and `SIGKILL`
/// ends it.
#[test]
fn a_child_that_ignores_terminate_is_ended_by_kill() {
    both("stubborn", STUBBORN);
}

/// A stopped child has not exited, and once continued it finishes on its own.
#[test]
fn a_stopped_child_continues_and_then_exits() {
    both("paused", PAUSED);
}

/// Exit codes, a status kept for a second wait, and a child nobody waited on
/// until later.
#[test]
fn a_child_that_exits_reports_its_code_however_late_it_is_asked() {
    both(
        "exits",
        "three wait Exited(3)\nthree wait Exited(3)\nthree tryWait Some(Exited(3))\n\
         true wait Exited(0)\nfour polled Exited(4)\nfour wait Exited(4)\n",
    );
}

/// `stdout` and `stderr` files, for `start` and for `run`, and a started
/// child's input, working directory and environment.
#[test]
fn a_started_child_writes_to_the_files_its_command_names() {
    both(
        "files",
        "both wait Exited(0)\nstdout file out\nstderr file err\nshared wait Exited(0)\n\
         shared file one two three\nrun captured 0 said file ran\n\
         cat wait Exited(0)\ncat wrote hello\nplaced wait Exited(0)\nplaced true true\n",
    );
}

/// With no file, the child writes to the program's own standard output, after
/// what the program printed first.
#[test]
fn a_started_child_inherits_the_programs_output() {
    both("inherits", "before\ninherited\nafter\n");
}

/// A program, a working directory or an output file that is not there.
#[test]
fn a_child_that_cannot_start_is_an_error() {
    both("refusals", "refused NotFound NotFound NotFound\n");
}

/// Sixteen children waited on from sixteen tasks while a seventeenth releases
/// them: a wait does not hold up the rest of the program.
#[test]
fn many_children_are_waited_on_at_once() {
    both("concurrent", &concurrent_expected());
}

/// The program ends with a child still running, and the child keeps running.
#[test]
fn a_child_outlives_the_program_that_started_it() {
    for on in [On::Native, On::JavaScript] {
        let Some(stdout) = ran(on, "outlives") else { continue };
        let pid: i32 = stdout
            .trim()
            .strip_prefix("pid ")
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("{on:?} printed no pid: {stdout}"));
        // SAFETY: signal 0 checks the process exists and sends nothing.
        let alive = unsafe { kill(pid, 0) } == 0;
        // SAFETY: the sleeper the program started; nothing else has its pid
        // while it is alive.
        unsafe { kill(pid, 9) };
        assert!(alive, "{on:?}: the child died with the program that started it");
    }
}

/// Every scenario that sequences on signals and FIFOs, many times over and
/// several at once, on both backends.
#[test]
fn the_scenarios_hold_under_repetition_and_concurrency() {
    let rounds = 8;
    let scenarios = [
        ("harness", HARNESS.to_string()),
        ("stubborn", STUBBORN.to_string()),
        ("paused", PAUSED.to_string()),
        ("concurrent", concurrent_expected()),
    ];
    for on in [On::Native, On::JavaScript] {
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..rounds {
                        for (mode, expected) in &scenarios {
                            if let Some(stdout) = ran(on, mode) {
                                assert_eq!(&stdout, expected, "{mode} on {on:?}");
                            }
                        }
                    }
                });
            }
        });
    }
}

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
