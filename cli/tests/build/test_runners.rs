//! What `buri test` says when a native test process misbehaves: fails the heap
//! check, never starts, dies after its last block, writes lines that aren't
//! the protocol's, or reports a block it wasn't running.
//!
//! Each process starts under a shell script named in `BURI_TEST_RUNNER`, which
//! stands in for a broken test binary. Each scenario runs a suite with a
//! timeout, which runs its blocks one process at a time, and one without.
#![cfg(unix)]
use crate::harness::*;

const ONE: &str = "export fn one(): Int {\n    1\n}\n";

const TWO_TESTS: &str = "from \"core/testing/assert\" import * as assert;\n\n\
     test \"first\" {\n    assert.isTrue(true);\n}\n\n\
     test \"second\" {\n    assert.isTrue(true);\n}\n";

/// `//lib/a`, a plain suite, and `//lib/t`, one with a timeout.
fn suites(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    for (package, extra) in [("a", ""), ("t", "        timeout_seconds: 600\n")] {
        scratch.write(
            &format!("lib/{package}/BUILD.buri"),
            &format!(
                "library {{\n    test {{\n        sources: [\"test/t.buri\"]\n{extra}    }}\n}}\n"
            ),
        );
        scratch.write(&format!("lib/{package}/lib.buri"), ONE);
        scratch.write(&format!("lib/{package}/test/t.buri"), TWO_TESTS);
    }
    scratch
}

/// Runs each suite under `runner`, and checks each run with `check`.
fn under(scratch: &Scratch, runner: &str, check: impl Fn(&Run, &str)) {
    for suite in ["//lib/a", "//lib/t"] {
        let run = scratch.run_with_env(
            &["test", suite, "--force"],
            &[("BURI_TEST_RUNNER", runner), LINKED],
        );
        check(&run, suite);
    }
}

fn script(scratch: &Scratch, name: &str, body: &str) -> String {
    scratch
        .write_executable(name, &format!("#!/bin/sh\n{body}"))
        .display()
        .to_string()
}

/// A binary that exits with the heap check's status fails its suite, though
/// it said nothing.
///
/// Silent, because the harness fails any run that prints the check's own
/// failure line ([`Run::heap_ok`]).
#[test]
fn a_test_binary_that_fails_the_heap_check_fails_its_suite() {
    let scratch = suites("runners-heap");
    let runner = script(&scratch, "leaks", "\"$@\"\nexit 97\n");
    under(&scratch, &runner, |run, suite| {
        run.exits(1)
            .says(&format!(
                "error: the test binary for {suite} failed the heap check: \
                 the test binary exited with the heap-check status and said nothing"
            ))
            .says("this is a toolchain bug")
            .says("0 passed, 0 failed, 0 skipped, 1 failed to compile");
    });
}

/// A binary whose heap check passed prints the check's receipt, and its
/// suite passes.
#[test]
fn a_test_binary_heap_receipt_is_printed() {
    let scratch = suites("runners-receipt");
    let runner = script(
        &scratch,
        "receipt",
        "\"$@\"\nstatus=$?\necho 'buri heap check: ok (allocated=1 live=0)' >&2\nexit $status\n",
    );
    under(&scratch, &runner, |run, _| {
        run.ok()
            .says("buri heap check: ok (allocated=1 live=0)")
            .says("2 passed, 0 failed, 0 skipped");
    });
}

/// A binary that ends before it starts says what stopped it.
#[test]
fn a_test_binary_that_never_starts_says_why() {
    let scratch = suites("runners-no-start");
    let runner = script(
        &scratch,
        "refused",
        "echo 'the loader refused it' >&2\nexit 1\n",
    );
    under(&scratch, &runner, |run, suite| {
        run.exits(1)
            .says(&format!(
                "error: the test binary for {suite} could not start: the loader refused it"
            ))
            .says("0 passed, 0 failed, 0 skipped, 1 failed to compile");
    });
}

/// A binary that fails after its last block fails its suite, though every
/// block passed.
#[test]
fn a_test_binary_that_dies_after_its_blocks_fails_its_suite() {
    let scratch = suites("runners-died");
    let runner = script(&scratch, "died", "\"$@\"\necho '{\"left\":1}'\nexit 3\n");
    under(&scratch, &runner, |run, suite| {
        run.exits(1)
            .says(&format!(
                "error: the test binary for {suite} ran every block and then died: the run exited 3"
            ))
            .says("0 passed, 0 failed, 0 skipped, 1 failed to compile");
    });
}

/// Lines that aren't the protocol's, and a block reported that the binary
/// wasn't running, are passed over.
#[test]
fn a_test_binary_stray_lines_are_passed_over() {
    let scratch = suites("runners-stray");
    let runner = script(
        &scratch,
        "stray",
        "\"$@\" | awk 'BEGIN { print \"{\\\"left\\\":1}\"; print \"{\\\"other\\\":1}\"; fflush() }\n\
         { print; if ($0 ~ /\"next\"/) print \"{\\\"i\\\":99,\\\"left\\\":1}\"; fflush() }'\n",
    );
    under(&scratch, &runner, |run, _| {
        run.ok().says("2 passed, 0 failed, 0 skipped");
    });
}

/// A failure reported for a block the binary wasn't running is the block it
/// was running failing, with the run's exit status as its message.
#[test]
fn a_test_binary_failure_elsewhere_fails_the_block_it_was_running() {
    let scratch = suites("runners-elsewhere");
    // The binary writes to a fifo, so the script's own end of buri's pipe
    // closes when it stops reading, however far the binary got.
    let runner = script(
        &scratch,
        "elsewhere",
        "dir=$(mktemp -d)\nmkfifo \"$dir/out\"\n\"$@\" <&0 >\"$dir/out\" &\n\
         awk '{ print; fflush() } /\"left\"/ { print \"{\\\"i\\\":99,\\\"message\\\":\\\"elsewhere\\\"}\"; exit 5 }' <\"$dir/out\"\n\
         status=$?\nkill $! 2>/dev/null\nwait $!\nrm -rf \"$dir\"\nexit $status\n",
    );
    under(&scratch, &runner, |run, suite| {
        run.exits(1)
            .says(&format!("FAIL {suite}  test/t.buri  \"second\""))
            .says("  the run exited 5")
            .says("test/t.buri:7:1")
            .says("1 passed, 1 failed, 0 skipped");
    });
}

/// A runner that isn't there, or can't be run, fails each suite with why.
#[test]
fn a_test_runner_that_cannot_run_fails_each_suite() {
    let scratch = suites("runners-cannot-run");
    under(&scratch, "/nonexistent/tool", |run, _| {
        run.exits(1)
            .says("cannot run the test binary: ")
            .says("is not on PATH");
    });
    let plain = scratch
        .write("plain", "not a program\n")
        .display()
        .to_string();
    under(&scratch, &plain, |run, _| {
        run.exits(1)
            .says("cannot run the test binary: ")
            .says("Permission denied");
    });
}
