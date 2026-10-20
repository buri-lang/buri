//! `buri test`'s rarer runs, driven as a user reaches them: a suite with a
//! timeout whose blocks fail or die, suites with no test blocks, a missing C
//! compiler, a JavaScript runtime that is missing or misbehaves, a coverage
//! run with nowhere to write, and a watch loop over a broken build file.
use crate::harness::hang::Capped;
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
        .says("FAIL //lib/a  test/t.buri  \"second\"\n  Maximum call stack size exceeded.")
        .says("3 passed, 1 failed, 0 skipped");
}

/// A test binary a signal kills, as an out-of-memory killer does, fails its
/// suite with the signal's name. The C driver swaps the runner for a script
/// that runs it, then kills itself.
#[test]
fn a_test_binary_a_signal_kills_fails_with_the_signal() {
    let scratch = Scratch::repo("corners-killed-runner");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n",
        &blocks("a", 2),
    );
    let real_cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let real_runner = scratch.path("runner");
    let driver = scratch.write_executable(
        "killing-cc",
        &format!(
            "#!/bin/sh\n\
             case \" $* \" in *\" -### \"*) exit 1 ;; esac\n\
             '{real_cc}' \"$@\" || exit $?\n\
             case \" $* \" in *\" -o artifact \"*)\n\
             mv artifact '{runner}'\n\
             printf '#!/bin/sh\\n\"%s\" \"$@\" 2>/dev/null\\nkill -KILL $$\\n' '{runner}' > artifact\n\
             chmod +x artifact ;;\n\
             esac\n",
            runner = real_runner.display()
        ),
    );
    // With no home there is no test host, so the suite links a runner.
    let out = buri_command()
        .current_dir(&scratch.root)
        .args(["test", "--color=never", "//lib/a"])
        .env_remove("HOME")
        .env_remove("BURI_HOME")
        .env("CC", &driver)
        .capped_output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(
        said.contains("ran every block and then died: the run was killed by SIGKILL (signal 9)"),
        "{said}"
    );
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

/// With no C compiler to link with, a native suite is refused by name, the note
/// says that the C compiler is what is missing, and the fix says how to run it
/// on JavaScript instead.
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
        .says(
            "runs natively in the debug profile, but this host has no C toolchain to link one with",
        )
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

/// A runtime whose records leave out a test of the suite, or name one it
/// doesn't have, didn't run the suite: it fails rather than passing without
/// the tests it left out. Lines that aren't the protocol's, and a record for a
/// test nobody handed out, are passed over on the way.
#[test]
fn a_javascript_runtime_that_reports_other_tests_fails_the_suite() {
    let scratch = javascript_suites("corners-js-others");
    let empty = scratch.write_executable("empty", "#!/bin/sh\necho '[]'\n");
    let stranger = scratch.write_executable(
        "stranger",
        "#!/bin/sh\necho '[{\"name\":\"stranger\",\"module\":\"m\",\"ok\":false}]'\n",
    );
    for runtime in [&empty, &stranger] {
        scratch
            .run_with_env(
                &["test", "//lib/short"],
                &[("BURI_JS", &runtime.display().to_string())],
            )
            .exits(1)
            .says("error: the test binary did not run")
            .says("= fix: the runtime wrote no message; check BURI_JS names a JavaScript runtime")
            .silent_about("  = \n")
            .says("0 passed, 0 failed, 0 skipped, 1 failed to run");
    }
    let pulling = scratch.write_executable(
        "pulling",
        "#!/bin/sh\n\
         echo 'a line that is not the protocol'\n\
         echo '{\"ran\":7,\"record\":{}}'\n\
         while :; do\n  \
           echo '{\"next\":true}'\n  \
           read test || exit 0\n  \
           echo \"{\\\"ran\\\":$test,\\\"record\\\":{\\\"name\\\":\\\"stranger\\\",\\\"module\\\":\\\"m\\\",\\\"ok\\\":true}}\"\n\
         done\n",
    );
    scratch
        .run_with_env(
            &["test", "//lib/long"],
            &[("BURI_JS", &pulling.display().to_string())],
        )
        .exits(1)
        .says("error: the test binary did not run")
        .says("0 passed, 0 failed, 0 skipped, 1 failed to run");
}

/// Bundles that can't be written where they run fail their suites, each
/// saying where it tried.
#[test]
fn javascript_bundles_with_nowhere_to_go_fail_their_suites() {
    let scratch = javascript_suites("corners-js-nowhere");
    for suite in ["short", "long"] {
        scratch.write(
            &format!(".buri/out/node/lib/{suite}/test-library.mjs/held"),
            "a directory where the bundle goes\n",
        );
    }
    scratch
        .run(&["test", "//lib/short", "//lib/long"])
        .exits(1)
        .says("error: cannot write ")
        .says("lib/short/test-library.mjs: ")
        .says("lib/long/test-library.mjs: ")
        .says("0 passed, 0 failed, 0 skipped, 2 failed to run");
}

/// A suite on both backends runs on each, with the lint checking during the
/// build.
#[test]
fn a_suite_on_both_backends_runs_on_each_while_linting() {
    let scratch = Scratch::repo("corners-two-backends-lint");
    scratch.write("REPO.buri", "lint {\n    check_during_build: true\n}\n");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n        backends: [NATIVE, JS]\n",
        &blocks("a", 1),
    );
    scratch
        .run(&["test", "//lib/a"])
        .ok()
        .says("2 passed, 0 failed, 0 skipped");
}

/// Suites sharing one test binary, split one per part, each run only the
/// tests `--filter` keeps.
#[test]
fn a_filter_reaches_each_part_of_a_split_binary() {
    let scratch = Scratch::repo("corners-split-filter");
    for name in ["a", "b"] {
        library(
            &scratch,
            name,
            "        sources: [\"test/t.buri\"]\n",
            &format!(
                "from \"//lib/{name}\" import {{ one }};\nfrom \"core/testing/assert\" import * as assert;\n\n\
                 test \"kept {name}\" {{\n    assert.equal(one(), 1);\n}}\n\n\
                 test \"left {name}\" {{\n    assert.equal(one(), 1);\n}}\n"
            ),
        );
    }
    scratch
        .run_with_env(
            &["test", "//lib/a", "//lib/b", "--filter=kept"],
            &[("BURI_TEST_BATCH_BYTES", "1")],
        )
        .ok()
        .says("2 passed, 0 failed, 2 skipped");
}

/// A suite with no test blocks, split into a part of its own, passes, and the
/// suite beside it still runs its blocks.
#[test]
fn a_blockless_suite_split_from_the_rest_passes() {
    let scratch = Scratch::repo("corners-split-blockless");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n",
        "from \"//lib/a\" import { one };\n\nfn two(): Int {\n    one() + one()\n}\n",
    );
    library(
        &scratch,
        "b",
        "        sources: [\"test/t.buri\"]\n",
        &blocks("b", 1),
    );
    scratch
        .run_with_env(
            &["test", "//lib/a", "//lib/b"],
            &[("BURI_TEST_BATCH_BYTES", "1")],
        )
        .ok()
        .says("1 passed, 0 failed, 0 skipped");
}

/// A link with nowhere to write its binary fails each suite, saying where.
#[test]
fn a_link_that_fails_fails_each_suite() {
    let scratch = Scratch::repo("corners-link-fails");
    for name in ["a", "b"] {
        library(
            &scratch,
            name,
            "        sources: [\"test/t.buri\"]\n",
            &blocks(name, 1),
        );
    }
    scratch.write(".buri/out/native", "a file where the directory goes\n");
    scratch
        .run_with_env(&["test", "//lib/a", "//lib/b"], &[LINKED])
        .exits(1)
        .says("error: cannot write ")
        .says("lib/a/test-library: ")
        .says("lib/b/test-library: ")
        .says("0 passed, 0 failed, 0 skipped, 2 failed to compile");
}

/// Suites with a timeout that failed, or ran out of time, run again from the
/// binary they ran in, and say again what they said. A binary that fails the
/// heap check on the second run is built again and reported.
#[test]
fn suites_with_a_timeout_run_again_from_their_binary() {
    let scratch = Scratch::repo("corners-timeout-again");
    library(
        &scratch,
        "f",
        "        sources: [\"test/t.buri\"]\n        timeout_seconds: 600\n",
        "from \"core/testing/assert\" import * as assert;\n\n\
         test \"first\" {\n    assert.isTrue(true);\n}\n\n\
         test \"second\" {\n    assert.equal(1, 2);\n}\n",
    );
    // A loop with no exit, so the one-second limit is certain to be reached.
    library(
        &scratch,
        "s",
        "        sources: [\"test/t.buri\"]\n        timeout_seconds: 1\n",
        "from \"core/testing/assert\" import * as assert;\nfrom \"//lib/s\" import { spin };\n\n\
         test \"returns eventually\" {\n    assert.equal(spin(0), 0);\n}\n",
    );
    scratch.write(
        "lib/s/lib.buri",
        "export fn spin(n: I64): I64 {\n    spin(n + 1)\n}\n",
    );
    for pass in 0..2 {
        let run = scratch.run_with_env(&["test", "//lib/f", "//lib/s", "--explain"], &[LINKED]);
        run.exits(1)
            .says("FAIL //lib/f  test/t.buri  \"second\"")
            .says("--> lib/f/test/t.buri:7:1")
            .says("error: //lib/s's test suite ran longer than 1s [test-timeout]")
            .says("1 passed, 1 failed, 0 skipped, 1 failed to run");
        if pass == 1 {
            run.says("cached build //lib/f native")
                .says("cached build //lib/s native");
        }
    }
    let leaks = scratch.write_executable("leaks", "#!/bin/sh\n\"$@\"\nexit 97\n");
    scratch
        .run_with_env(
            &["test", "//lib/f"],
            &[("BURI_TEST_RUNNER", &leaks.display().to_string()), LINKED],
        )
        .exits(1)
        .says("error: the test binary for //lib/f failed the heap check")
        .says("0 passed, 0 failed, 0 skipped, 1 failed to run");
}

/// A suite with a timeout that takes a snapshot records it under `--update`,
/// and passes against it after.
#[test]
fn a_suite_with_a_timeout_records_its_snapshot() {
    let scratch = Scratch::repo("corners-timeout-snapshot");
    scratch.write(
        "lib/p/BUILD.buri",
        "library {\n  sources: []\n  test {\n    sources: [\"test/p.buri\"]\n    timeout_seconds: 600\n  }\n}\n",
    );
    scratch.write(
        "lib/p/lib.buri",
        "// The package is its suite and nothing else.\n",
    );
    scratch.write(
        "lib/p/test/p.buri",
        "from \"platform/effect\" import { Allocator, Ui };\n\
         from \"platform/effect/testing\" import { alloc, headless, snapshot };\n\
         from \"ui/node\" import * as ui;\n\n\
         test \"the panel\" {\n  \
           let ctx = context { Allocator: alloc(), Ui: headless() };\n  \
           snapshot(\n    ctx,\n    \"panel\",\n    \
             ui.text({ content: .Const(\"p\"), headingLevel: .None, styles: .Some([]) }),\n    \
             .Hover,\n    [],\n  );\n}\n",
    );
    scratch
        .run_with_env(&["test", "//lib/p", "--update"], &[LINKED])
        .ok()
        .says("1 passed, 0 failed, 0 skipped");
    assert!(scratch.path("lib/p/test/__snapshots__/panel.png").is_file());
    scratch
        .run_with_env(&["test", "//lib/p", "--force"], &[LINKED])
        .ok()
        .says("1 passed, 0 failed, 0 skipped");
}

/// A suite that didn't build says why again on the next run, from what the
/// build recorded: an error in a file that doesn't lex, and one at the top of
/// a build file, before its first token.
#[test]
fn a_refused_suite_says_why_again() {
    let scratch = Scratch::repo("corners-refused-again");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n",
        "from \"core/testing/assert\" import * as assert;\n\n\
         test \"x\" {\n    assert.equal(\"abc, 1);\n}\n",
    );
    library(
        &scratch,
        "d",
        "        sources: [\"test/t.buri\"]\n",
        &blocks("d", 1),
    );
    scratch.edit(
        "lib/d/BUILD.buri",
        "library {",
        "# A testing directory, and no block for it.\nlibrary {",
    );
    scratch.write("lib/d/testing/lib.buri", ONE);
    for _ in 0..2 {
        scratch
            .run(&["test", "//lib/a", "//lib/d"])
            .exits(1)
            .says("error: unterminated string literal [unterminated-string]")
            .says("error: //lib/d has a testing/ directory and no `testing` block")
            .says("--> lib/d/BUILD.buri:1:1")
            .says("0 passed, 0 failed, 0 skipped, 2 failed to compile");
    }
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

/// A suite that failed runs again from the build the cache holds. When the
/// cache has lost that binary or bundle, the suite is built again and says what
/// it said before.
#[test]
fn a_failing_suite_whose_build_left_the_cache_is_built_again() {
    let failing =
        "from \"//lib/a\" import { one };\nfrom \"core/testing/assert\" import * as assert;\n\n\
                   test \"one\" {\n    assert.equal(one(), 2);\n}\n";
    for (name, backends, lost) in [
        ("native", "", Lost::Image),
        ("js", "        backends: [JS]\n", Lost::Bundle),
    ] {
        let scratch = Scratch::repo(&format!("corners-lost-build-{name}"));
        library(
            &scratch,
            "a",
            &format!("        sources: [\"test/t.buri\"]\n{backends}"),
            failing,
        );
        let first = scratch.run(&["test", "//lib/a", "--explain"]);
        first.exits(1).says("FAIL //lib/a  test/t.buri  \"one\"");
        let linked = first
            .all()
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                (f.len() == 5 && f[1] == "link").then(|| f[4].to_string())
            })
            .collect::<Vec<_>>();
        let mut files = Vec::new();
        cache_files(&scratch.path(".buri/cache"), &mut files);
        let gone: Vec<_> = match lost {
            Lost::Image => files
                .iter()
                .filter(|f| {
                    let name = f.parent().and_then(|p| p.file_name()).unwrap_or_default();
                    let rest = f.file_name().unwrap_or_default();
                    let key = format!("{}{}", name.to_string_lossy(), rest.to_string_lossy());
                    linked.iter().any(|link| key.starts_with(link.as_str()))
                })
                .collect(),
            Lost::Bundle => files
                .iter()
                .max_by_key(|f| std::fs::metadata(f).map_or(0, |m| m.len()))
                .into_iter()
                .collect(),
        };
        assert_eq!(
            gone.len(),
            1,
            "{name}: the cache held {} builds",
            gone.len()
        );
        std::fs::remove_file(gone[0]).unwrap();
        scratch
            .run(&["test", "//lib/a"])
            .exits(1)
            .says("FAIL //lib/a  test/t.buri  \"one\"")
            .says("expected: 2")
            .says("0 passed, 1 failed, 0 skipped");
    }
}

/// What a test makes the cache lose.
enum Lost {
    Image,
    Bundle,
}

/// Every entry under a cache directory.
fn cache_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            cache_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// A remembered verdict that holds no results is not served: the suite runs.
#[test]
fn a_remembered_verdict_with_no_results_is_run_again() {
    let scratch = Scratch::repo("corners-empty-verdict");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n",
        &blocks("a", 1),
    );
    scratch
        .run(&["test", "//lib/a"])
        .ok()
        .says("1 passed, 0 failed, 0 skipped");
    let mut files = Vec::new();
    cache_files(&scratch.path(".buri/cache"), &mut files);
    let mut emptied = 0;
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        if text.starts_with("reads ") && text.contains("\"ok\":true") {
            let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with('[')).collect();
            std::fs::write(&file, format!("{}\n[]\n", kept.join("\n"))).unwrap();
            emptied += 1;
        }
    }
    assert_eq!(emptied, 1, "the cache held {emptied} verdicts");
    scratch
        .run(&["test", "//lib/a", "--explain"])
        .ok()
        .says("run    test //lib/a")
        .says("1 passed, 0 failed, 0 skipped");
}

/// Under `--filter`, a suite whose source is missing is reported as missing,
/// not skipped as a suite the filter left nothing in.
#[test]
fn a_filtered_suite_with_a_missing_source_says_so() {
    let scratch = Scratch::repo("corners-filtered-missing");
    library(
        &scratch,
        "a",
        "        sources: [\"test/missing.buri\"]\n",
        &blocks("a", 1),
    );
    scratch
        .run(&["test", "//lib/a", "--filter=nothing"])
        .exits(1)
        .says("error: test/missing.buri does not exist [unknown-source]")
        .says("0 passed, 0 failed, 0 skipped, 1 failed to compile");
}

/// Under `--filter`, a suite whose source has no `.buri` extension, and so
/// can't be read ahead of the compile, still skips the blocks the filter
/// leaves out rather than building a binary with none in it.
#[test]
fn a_filter_that_leaves_nothing_in_a_txt_source_skips_it() {
    let scratch = Scratch::repo("corners-filtered-txt");
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.txt\"]\n",
        &blocks("a", 1),
    );
    scratch.write("lib/a/test/t.txt", &blocks("a", 1));
    scratch
        .run(&["test", "//lib/a"])
        .ok()
        .says("1 passed, 0 failed, 0 skipped");
    scratch
        .run(&["test", "//lib/a", "--filter=nothing"])
        .ok()
        .says("0 passed, 0 failed, 1 skipped");
}

/// A suite that asks for the native backend and reaches what it has no body
/// for is refused with the keys it lacks and what to do, rather than run
/// anywhere else.
#[test]
fn a_suite_that_asks_for_native_and_reaches_a_gap_is_refused() {
    let scratch = Scratch::repo("corners-asked-native-gap");
    library(
        &scratch,
        "g",
        "        sources: [\"test/t.buri\"]\n        backends: [NATIVE]\n",
        "from \"core/testing/assert\" import * as assert;\n\
         from \"core/math\" import * as math;\n\n\
         test \"logs\" {\n    assert.equal(math.ln(1.0), 0.0);\n}\n",
    );
    let run = scratch.run(&["test", "//lib/g"]);
    run.exits(1);
    if run.stderr.contains("test-run-unavailable") {
        return;
    }
    run.says("backend has no implementation of math.ln")
        .says("= fix: declare `test { backends: [JS] }` for this suite")
        .says("0 passed, 0 failed, 0 skipped, 1 failed to compile");
}

/// A refusal that points into a comment follows the comment when only the
/// whitespace before it changes, rather than repeating where it was.
#[test]
fn a_refusal_in_a_comment_follows_a_blank_line_added_before_it() {
    let scratch = Scratch::repo("corners-refusal-moves");
    let suite = |gap: &str| {
        format!(
            "from \"core/testing/assert\" import * as assert;\n{gap}\
             //! A late module doc.\n\n\
             test \"t\" {{\n    assert.equal(1, 1);\n}}\n"
        )
    };
    library(
        &scratch,
        "a",
        "        sources: [\"test/t.buri\"]\n",
        &suite(""),
    );
    scratch
        .run(&["test", "//lib/a"])
        .exits(1)
        .says("[module-doc-not-first]")
        .says("--> lib/a/test/t.buri:2:1");
    scratch.write("lib/a/test/t.buri", &suite("\n"));
    scratch
        .run(&["test", "//lib/a"])
        .exits(1)
        .says("[module-doc-not-first]")
        .says("--> lib/a/test/t.buri:3:1");
}

/// `//lib/p`: a library and a binary whose suites both paint goldens into the
/// package's one snapshot directory, with `extra` in each `test` block.
fn painting_package(name: &str, extra: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    let suite =
        |source: &str| format!("  test {{\n    sources: [\"test/{source}.buri\"]\n{extra}  }}\n");
    scratch.write(
        "lib/p/BUILD.buri",
        &format!(
            "library {{\n  sources: []\n{}}}\n\nbinary {{\n  outputs: [{{ platform: \"node\" }}]\n{}}}\n",
            suite("a"),
            suite("b"),
        ),
    );
    scratch.write("lib/p/lib.buri", "// The package is its suites.\n");
    scratch.write(
        "lib/p/main.buri",
        "from \"node\" import { NodeHost };\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n  .Ok(())\n}\n",
    );
    for source in ["a", "b"] {
        scratch.write(
            &format!("lib/p/test/{source}.buri"),
            &format!(
                "from \"platform/effect\" import {{ Allocator, Ui }};\n\
                 from \"platform/effect/testing\" import {{ alloc, headless, snapshot }};\n\
                 from \"ui/node\" import * as ui;\n\n\
                 test \"panel {source}\" {{\n  \
                   let ctx = context {{ Allocator: alloc(), Ui: headless() }};\n  \
                   snapshot(\n    ctx,\n    \"panel-{source}\",\n    \
                     ui.text({{ content: .Const(\"{source}\"), headingLevel: .None, styles: .Some([]) }}),\n    \
                     .Hover,\n    [],\n  );\n}}\n"
            ),
        );
    }
    scratch
}

/// Two suites that paint into one snapshot directory take turns: the second
/// waits while the first is painting, and both goldens are recorded.
///
/// The first test process to start waits at a FIFO, so the first suite is
/// still painting when the second is refused the directory.
fn painters_take_turns(name: &str, extra: &str) {
    let scratch = painting_package(name, extra);
    let held = crate::held::Held::new(name);
    held.hold("gate", 1);
    held.hold("refused", 1);
    let gate = held.dir().join("gate-1");
    let runner = scratch.write_executable(
        "gated",
        &format!(
            "#!/bin/sh\nif mkdir \"{0}.started\" 2>/dev/null; then read _ < \"{0}\"; fi\nexec \"$@\"\n",
            gate.display()
        ),
    );
    let runner = runner.display().to_string();
    let mut run = held.start_with_env(
        &scratch,
        &["test", "//...", "--update", "--jobs=4", "--color=never"],
        &[("BURI_TEST_RUNNER", &runner), LINKED],
    );
    let painting = held.reached("gate", 1);
    held.at("refused", 1, || {});
    painting.go();
    let status = hang::wait_capped(&mut run, "buri test --update", hang::cap());
    assert!(status.success(), "{}", held.printed());
    assert!(
        held.printed().contains("2 passed, 0 failed, 0 skipped"),
        "{}",
        held.printed()
    );
    for source in ["a", "b"] {
        assert!(
            scratch
                .path(&format!("lib/p/test/__snapshots__/panel-{source}.png"))
                .is_file(),
            "panel-{source} was not recorded:\n{}",
            held.printed()
        );
    }
}

/// Suites in one binary take turns at their snapshot directory.
#[test]
fn suites_in_one_binary_take_turns_painting_one_directory() {
    painters_take_turns("corners-painters-batched", "");
}

/// Suites with a timeout, each in a binary of its own, take turns at their
/// snapshot directory.
#[test]
fn suites_with_a_timeout_take_turns_painting_one_directory() {
    painters_take_turns("corners-painters-alone", "    timeout_seconds: 600\n");
}
