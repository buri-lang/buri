//! Functions that never end, and arithmetic an optimizer could fold, on
//! JavaScript and every native backend.
//!
//! A recursion that never ends is not a value. One that isn't a tail call runs
//! out of stack and aborts (SPEC 8.3, 6.9). One that is a tail call runs in
//! constant stack (SPEC 8.3), so it runs forever. Neither may print a number
//! and exit 0, whatever the release build's optimizer folds.
//!
//! Every program here runs under a deadline, so a build that turns an abort
//! into an endless loop fails rather than hangs.
use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::agreement::{agree, analyze, emit_js, emitted, natives, prepared_native, skip_reason};

macro_rules! rows_or_skip {
    () => {
        if let Some(why) = skip_reason() {
            crate::ci::skipped("backend agreement", &why);
            return;
        }
    };
}

/// What bun says when JavaScript runs out of stack, and so what every backend says.
const STACK_EXHAUSTED: &str = "Maximum call stack size exceeded.";

/// `main`, printing `before` and then `value`, after `functions`.
fn printing(functions: &str, value: &str) -> String {
    format!(
        r#"{functions}
from "native" import {{ NativeHost }};
from "core/io" import * as io;

export fn main(host: NativeHost): Result<(), Str> {{
  let _ = io.println(host.stdout, "before").ignore();
  let _ = io.println(host.stdout, "${{{value}}}").ignore();
  .Ok(())
}}
"#
    )
}

/// One program, built for JavaScript and for every native backend here, as
/// the command line that starts each, by backend name.
fn built(row: &str, source: &str) -> Vec<(String, Vec<PathBuf>)> {
    let (checked, paths) = analyze(row, source);
    let engine = crate::shared::js_engine().expect("a JavaScript engine");
    let js = emit_js(row, &checked, &paths);
    let dir = js.parent().unwrap().to_path_buf();
    let mut programs = vec![(String::from("javascript"), vec![PathBuf::from(engine), js])];
    let program = prepared_native(row, &checked, &paths);
    for native in natives(row) {
        let units = emitted(native, &program, &checked)
            .unwrap_or_else(|why| panic!("{row}: `{}` refused the program: {why}", native.name));
        let objects: Vec<PathBuf> = units
            .iter()
            .map(|unit| {
                let path = dir.join(format!("{}-{}", native.name, unit.name));
                std::fs::write(&path, &unit.bytes).unwrap();
                path
            })
            .collect();
        let binary = dir.join(format!("program-{}", native.name));
        let linked = match native.name {
            "stencil" => crate::shared::place_program(&objects, &binary),
            _ => crate::shared::link_program(&objects, &binary),
        };
        assert!(
            linked.status.success(),
            "{row}: the `{}` link failed:\n{}",
            native.name,
            String::from_utf8_lossy(&linked.stderr)
        );
        crate::sweep::kept::settle(&binary);
        programs.push((native.name.to_string(), vec![binary]));
    }
    programs
}

/// How one run ended: its status, standard output and standard error.
struct Ended {
    status: i32,
    stdout: String,
    stderr: String,
}

/// Runs `line` and answers how it ended, or `None` if it was still running
/// after `deadline`, in which case it is killed.
fn ended(line: &[PathBuf], deadline: Duration) -> Option<Ended> {
    let mut child = Command::new(&line[0])
        .args(&line[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Both pipes drain on threads of their own, so a chatty program can't
    // block on a full pipe and look like one that runs forever.
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let stdout = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let stderr = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if start.elapsed() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
    status.map(|status| Ended {
        status: status.code().unwrap_or(-1),
        stdout,
        stderr,
    })
}

/// How long a program that should abort may take to: far longer than running
/// out of stack takes, so only a program that doesn't abort reaches it.
const ABORTS_WITHIN: Duration = Duration::from_secs(60);

/// Asserts `source` prints `before`, then aborts because it ran out of stack,
/// on every backend.
fn exhausts_the_stack(row: &str, source: &str) {
    for (name, line) in built(row, source) {
        let Some(run) = ended(&line, ABORTS_WITHIN) else {
            panic!("{row}: `{name}` was still running after {ABORTS_WITHIN:?}");
        };
        assert_eq!(
            (run.status, run.stdout.as_str(), run.stderr.lines().next()),
            (1, "before\n", Some(STACK_EXHAUSTED)),
            "{row}: `{name}` didn't run out of stack. Standard error:\n{}",
            run.stderr
        );
    }
}

/// `1 + deep(n + 1)` has no side effects and no end, and isn't a tail call.
#[test]
fn a_recursion_with_no_end_and_no_effects_aborts() {
    rows_or_skip!();
    exhausts_the_stack(
        "endless sum",
        &printing("fn deep(n: Int): Int { 1 + deep(n + 1) }", "deep(1)"),
    );
}

/// The same with an exit only overflow could reach: the stack runs out long
/// before `n` comes back round to `0`.
#[test]
fn a_recursion_whose_exit_only_overflow_reaches_aborts() {
    rows_or_skip!();
    exhausts_the_stack(
        "endless sum with an exit",
        &printing(
            "fn deep(n: Int): Int { if (n == 0) { 0 } else { 1 + deep(n + 1) } }",
            "deep(1)",
        ),
    );
}

/// A product rather than a sum, which an optimizer can accumulate the same way.
#[test]
fn a_recursion_with_no_end_that_multiplies_aborts() {
    rows_or_skip!();
    exhausts_the_stack(
        "endless product",
        &printing("fn deep(n: Int): Int { deep(n + 1) * 3 }", "deep(1)"),
    );
}

/// Two functions that recurse through each other, neither a tail call.
#[test]
fn a_mutual_recursion_with_no_end_aborts() {
    rows_or_skip!();
    exhausts_the_stack(
        "endless mutual sum",
        &printing(
            "fn ping(n: Int): Int { 1 + pong(n + 1) }\nfn pong(n: Int): Int { 2 + ping(n + 1) }",
            "ping(1)",
        ),
    );
}

/// How long a program that runs forever must still be running. A program
/// whose loop was folded away exits in milliseconds, so a slow machine can
/// only make this pass a wrong build, never fail a right one.
const FOREVER: Duration = Duration::from_millis(1500);

/// Asserts `source` is still running after [`FOREVER`] on every backend.
fn runs_forever(row: &str, source: &str) {
    for (name, line) in built(row, source) {
        if let Some(run) = ended(&line, FOREVER) {
            panic!(
                "{row}: `{name}` ended, exiting {} after printing {:?}",
                run.status, run.stdout
            );
        }
    }
}

/// A tail call to itself is a loop (SPEC 8.3), and this one has no exit.
#[test]
fn a_tail_recursion_with_no_end_and_no_effects_runs_forever() {
    rows_or_skip!();
    runs_forever(
        "endless tail recursion",
        &printing("fn spin(n: Int): Int { spin(n + 1) }", "spin(1)"),
    );
}

/// The same with an exit no argument reaches, so the loop has a way out on
/// paper: a step of `0` never takes `n` below zero.
#[test]
fn a_tail_recursion_whose_exit_is_never_reached_runs_forever() {
    rows_or_skip!();
    runs_forever(
        "endless tail recursion with an exit",
        &printing(
            "fn spin(n: Int, step: Int): Int { if (n < 0) { n } else { spin(n + step, step) } }",
            "spin(1, 0)",
        ),
    );
}

/// `wrappingAdd` and `wrappingMultiply` in loops the optimizer can turn into
/// a closed form. Both sums overflow `Int`, and their low bits are the same on
/// every backend (SPEC 6.2.2).
#[test]
fn wrapping_arithmetic_in_a_foldable_loop_keeps_the_low_bits() {
    rows_or_skip!();
    agree(
        "wrapping sum",
        &printing(
            r#"
fn sum(i: Int, n: Int, acc: Int): Int {
  if (i == n) { acc } else { sum(i + 1, n, acc.wrappingAdd(i.wrappingMultiply(3000000000000000000))) }
}
fn count(i: Int, n: Int, acc: Int): Int {
  if (i == n) { acc } else { count(i + 1, n, acc.wrappingAdd(3000000000000000000)) }
}
fn id(x: Int): Int { x }
"#,
            r#"sum(0, id(10), 0)} ${count(0, id(10), 0)"#,
        ),
        "before\n5872791484033138688 -6893488147419103232\n",
    );
}
