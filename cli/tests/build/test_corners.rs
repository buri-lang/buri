//! `buri test`'s rarer runs, driven as a user reaches them: a suite with a
//! timeout whose blocks fail or die, suites with no test blocks, a missing C
//! compiler, a JavaScript runtime that is missing or misbehaves, a coverage
//! run with nowhere to write, and a watch loop over a broken build file.
use crate::harness::*;
use std::io::Read as _;
use std::time::{Duration, Instant};

const ONE: &str = "export fn one(): Int {\n    1\n}\n";

/// A suite of `n` passing blocks over `//lib/<name>`'s `one`.
fn blocks(name: &str, n: usize) -> String {
    let mut out = format!("from \"//lib/{name}\" import {{ one }};\nfrom \"core/testing/assert\" import * as assert;\n");
    for i in 0..n {
        out.push_str(&format!(
            "\ntest \"one {i}\" {{\n    assert.equal(one(), 1);\n}}\n"
        ));
    }
    out
}

/// `//lib/<name>`, with `test` as its suite's block body.
fn library(scratch: &Scratch, name: &str, test: &str, suite: &str) {
    scratch.write(
        &format!("lib/{name}/BUILD.buri"),
        &format!("library {{\n    test {{\n{test}    }}\n}}\n"),
    );
    scratch.write(&format!("lib/{name}/lib.buri"), ONE);
    scratch.write(&format!("lib/{name}/test/t.buri"), suite);
}

/// A suite with a timeout runs its blocks one process at a time. Each failing
/// block is reported, the last one included, and so is a block that kills the
/// process, with the blocks around it keeping their verdicts.
#[test]
fn a_suite_with_a_timeout_reports_each_failing_block() {
    let scratch = Scratch::repo("corners-timeout-failures");
    let suite =
        "from \"core/testing/assert\" import * as assert;\nfrom \"//lib/a\" import { deep };\n\n\
                 test \"first\" {\n    assert.isTrue(true);\n}\n\n\
                 test \"second\" {\n    assert.equal(1, 2);\n}\n\n\
                 test \"third\" {\n    assert.isTrue(true);\n}\n\n\
                 test \"last\" {\n    assert.equal(3, 4);\n}\n";
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n        timeout_seconds: 600\n",
        suite,
    );
    scratch.write(
        "lib/a/lib.buri",
        "export fn deep(n: Int): Int {\n    if (n == 0) { 0 } else { deep(n + 1) + 1 }\n}\n",
    );
    scratch
        .run(&["test", "//lib/a"])
        .exits(1)
        .says("FAIL //lib/a  test/t.buri  \"second\"")
        .says("FAIL //lib/a  test/t.buri  \"last\"")
        .says("2 passed, 2 failed, 0 skipped");

    scratch.edit(
        "lib/a/test/t.buri",
        "assert.equal(1, 2);",
        "assert.equal(deep(1), 1);",
    );
    scratch.edit(
        "lib/a/test/t.buri",
        "assert.equal(3, 4);",
        "assert.isTrue(true);",
    );
    scratch
        .run(&["test", "//lib/a"])
        .exits(1)
        .says("FAIL //lib/a  test/t.buri  \"second\"\n  the run was killed by ")
        .says("3 passed, 1 failed, 0 skipped");
}

/// Suites whose test files hold no `test` block pass with nothing run, alone,
/// side by side, and with the lint checking the build.
#[test]
fn suites_with_no_test_blocks_run_nothing() {
    let scratch = Scratch::repo("corners-no-blocks");
    for name in ["a", "b"] {
        let helper = format!(
            "from \"//lib/{name}\" import {{ one }};\n\nfn helper(): Int {{\n    one()\n}}\n"
        );
        library(
            &scratch,
            name,
            "        sources: [\"test/t.buri\"]\n",
            &helper,
        );
    }
    scratch
        .run(&["test", "//..."])
        .ok()
        .says("0 passed, 0 failed, 0 skipped");
    scratch.write("REPO.buri", "lint {\n    check_during_build: true\n}\n");
    scratch
        .run(&["test", "//lib/a"])
        .ok()
        .says("0 passed, 0 failed, 0 skipped");
}

/// With no C compiler to link with, a native suite is refused by name, and the
/// fix says how to run it on JavaScript instead.
#[test]
fn a_missing_c_compiler_refuses_native_suites() {
    let scratch = Scratch::repo("corners-no-cc");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n",
        &blocks("a", 2),
    );
    scratch
        .run_with_env(&["test", "//lib/a"], &[("CC", "/nonexistent/cc")])
        .exits(1)
        .says("error: this toolchain cannot build a native test binary [test-run-unavailable]")
        .says("fix: run the suite on JavaScript with `buri test --output=js`")
        .says("0 passed, 0 failed, 0 skipped, 1 failed to compile");
    scratch
        .run_with_env(
            &["test", "//lib/a", "--output=js"],
            &[("CC", "/nonexistent/cc")],
        )
        .ok()
        .says("2 passed, 0 failed, 0 skipped");
}

/// A JavaScript suite short enough for one process, and one long enough for
/// several, each run under `runtime`.
fn javascript_suites(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    let js = "        sources: [\"test/t.buri\"]\n        backends: [JS]\n";
    library(&scratch, "short", js, &blocks("short", 2));
    library(&scratch, "long", js, &blocks("long", 100));
    scratch
}

/// `BURI_JS` naming nothing, or a runtime that fails before running a test,
/// fails each suite with what the runtime said.
#[test]
fn a_javascript_runtime_that_cannot_run_fails_each_suite() {
    let scratch = javascript_suites("corners-js-missing");
    for suite in ["//lib/short", "//lib/long"] {
        scratch
            .run_with_env(&["test", suite], &[("BURI_JS", "/nonexistent/js")])
            .exits(1)
            .says("error: cannot run the test binary: `/nonexistent/js` is not on PATH")
            .says("fix: install bun, or point BURI_JS at a JavaScript runtime");
    }
    let quits =
        scratch.write_executable("quits", "#!/bin/sh\necho 'no runtime today' >&2\nexit 3\n");
    let quits = quits.display().to_string();
    for suite in ["//lib/short", "//lib/long"] {
        scratch
            .run_with_env(&["test", suite], &[("BURI_JS", &quits)])
            .exits(1)
            .says("error: the test binary did not run")
            .says("= no runtime today");
    }
}

/// A runtime that exits cleanly without reporting a test ran nothing, so the
/// suite fails rather than passing with no tests.
#[test]
fn a_javascript_runtime_that_reports_nothing_fails_the_suite() {
    let scratch = javascript_suites("corners-js-silent");
    let silent =
        scratch.write_executable("silent", "#!/bin/sh\necho 'nothing to say' >&2\nexit 0\n");
    let silent = silent.display().to_string();
    for suite in ["//lib/short", "//lib/long"] {
        scratch
            .run_with_env(&["test", suite], &[("BURI_JS", &silent)])
            .exits(1)
            .says("error: the test binary did not run")
            .says("= nothing to say");
    }
}

/// A runtime that dies holding a test it was handed fails the suite, rather
/// than the suite passing without it.
#[test]
fn a_javascript_runtime_that_dies_in_a_test_fails_the_suite() {
    let scratch = javascript_suites("corners-js-dies");
    let dies = scratch.write_executable(
        "dies",
        "#!/bin/sh\necho '{\"next\":0}'\nread test\necho \"died in test $test\" >&2\nexit 1\n",
    );
    scratch
        .run_with_env(
            &["test", "//lib/long"],
            &[("BURI_JS", &dies.display().to_string())],
        )
        .exits(1)
        .says("error: the test binary did not run")
        .says("= died in test ");
}

/// A coverage run that can't make its directory says why and exits 2.
#[test]
fn a_coverage_run_with_nowhere_to_write_says_why() {
    let scratch = Scratch::repo("corners-coverage-blocked");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n",
        &blocks("a", 1),
    );
    scratch.write(".buri/coverage", "a file where the directory goes\n");
    scratch
        .run(&["test", "//lib/a", "--coverage"])
        .exits(2)
        .says("error: cannot create ")
        .says(".buri/coverage/raw/");
}

/// Under `--watch`, a build file that doesn't parse is reported and the loop
/// keeps watching: fixing it runs the suite.
///
/// `--watch` refuses a pipe, so the loop runs under `script`, which hands it a
/// terminal.
#[test]
fn a_watch_loop_over_a_broken_build_file_runs_once_it_is_fixed() {
    let scratch = Scratch::repo("corners-watch-broken");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n",
        &blocks("a", 1),
    );
    library(
        &scratch,
        "b",
        "        sources: [\"test/t.buri\"]\n",
        &blocks("b", 1),
    );
    let fixed = scratch.read("lib/a/BUILD.buri");
    scratch.write("lib/a/BUILD.buri", "library {\n    test {\n");
    let buri = env!("CARGO_BIN_EXE_buri");
    let mut cmd = std::process::Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.args(["-q", "/dev/null", buri, "test", "//...", "--watch"]);
    } else {
        cmd.args(["-qec", &format!("{buri} test //... --watch"), "/dev/null"]);
    }
    let mut child = cmd
        .current_dir(&scratch.root)
        .env("BURI_HOME", sweep::kept::shared_cross_home())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("`script` runs");
    let mut pipe = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut repaired = false;
    while !seen.contains(" passed, ") && Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "the loop ended {status}:\n{}",
                indent(&seen.replace('\r', ""))
            );
        }
        if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        // The diagnostics are written as the pass runs, but the loop takes its
        // stamps before it prints the pass's line, so only an edit after that
        // line always wakes a second pass.
        if !repaired && seen.contains("lib/a/BUILD.buri") && seen.contains("run 1") {
            repaired = true;
            scratch.write("lib/a/BUILD.buri", &fixed);
        }
    }
    let status = ctrl_c(&mut child, "buri test --watch");
    let text = seen.replace('\r', "");
    assert_eq!(
        status.code(),
        Some(130),
        "the loop ended {status}:\n{}",
        indent(&text)
    );
    assert!(
        repaired,
        "the broken build file was never reported:\n{}",
        indent(&text)
    );
    assert!(
        text.contains("2 passed, 0 failed, 0 skipped"),
        "the fixed suites never ran:\n{}",
        indent(&text)
    );
}
