//! How `buri test` schedules suites: side by side, reported in suite order, and
//! reused when an edit cannot change what they do.
use crate::harness::hang::{Capped, CappedChild};
use crate::harness::*;

/// A package `lib/<name>` with one suite asserting `answer() == want`.
fn suite(scratch: &Scratch, name: &str, want: i64) {
    scratch.write(
        &format!("lib/{name}/BUILD.buri"),
        &format!("library {{\n  test {{ sources: [\"test/{name}.buri\"] }}\n}}\n"),
    );
    scratch.write(
        &format!("lib/{name}/lib.buri"),
        "export fn answer(): I64 { 1 }\n",
    );
    scratch.write(
        &format!("lib/{name}/test/{name}.buri"),
        &format!(
            "from \"//lib/{name}\" import {{ answer }};\n\
             from \"core/testing/assert\" import * as assert;\n\
             \ntest \"{name} answers\" {{\n  assert.equal(answer(), {want});\n}}\n"
        ),
    );
}

/// The JavaScript runtime as an absolute path, for a script that runs with no `PATH`.
fn js_path() -> String {
    let found = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("command -v {}", js_runtime()))
        .capped_output()
        .expect("sh runs");
    String::from_utf8_lossy(&found.stdout).trim().to_string()
}

/// Writes an executable script standing in for the JavaScript runtime, and
/// answers its path for `BURI_JS`.
fn runtime_script(scratch: &Scratch, body: &str) -> String {
    let path = scratch.write_executable("runtime.sh", &format!("#!/bin/sh\n{body}"));
    path.display().to_string()
}

/// A shell loop that waits up to two minutes for `condition`, and gives up with
/// `why` on standard error.
fn wait_for(condition: &str, why: &str) -> String {
    format!(
        "n=0\nuntil {condition}; do\n  n=$((n+1))\n  if [ \"$n\" -gt 2400 ]; then echo '{why}' >&2; exit 3; fi\n  /bin/sleep 0.05\ndone\n"
    )
}

/// The memory of a host with room for one build at a time, so these tests run
/// the way they would on a small machine whatever this one has. Every build is
/// larger than a budget this small, and one larger than the budget runs alone.
const SMALL_HOST: (&str, &str) = ("BURI_TEST_MEMORY_BYTES", "1");

/// Two suites are in flight at once, even on a host with room for one build.
///
/// Each suite's runtime waits until the other's has started. Run one after the
/// other, the first would wait alone and fail; side by side, both go at once.
#[test]
fn two_suites_run_side_by_side() {
    let scratch = Scratch::repo("parallel-suites");
    suite(&scratch, "a", 1);
    suite(&scratch, "b", 1);
    let dir = scratch.path("arrived").display().to_string();
    let js = js_path();
    let script = runtime_script(
        &scratch,
        &format!(
            "/bin/mkdir -p '{dir}'\n: > '{dir}/'$$\n{}exec '{js}' \"$@\"\n",
            wait_for(
                &format!("[ \"$(/bin/ls '{dir}' | /usr/bin/wc -l)\" -ge 2 ]"),
                "the other suite never started while this one waited",
            ),
        ),
    );
    let run = scratch.run_with_env(
        &["test", "//...", "--output=js", "--jobs=2"],
        &[("BURI_JS", &script), SMALL_HOST],
    );
    run.ok();
    assert_eq!(
        run.tests_passed(),
        2,
        "the suites did not both run:\n{}",
        indent(&run.all())
    );
}

/// Suites are reported in label order whichever finishes first, and the same
/// tree reports the same way twice.
///
/// `//lib/a`'s runtime waits until `//lib/b`'s has finished, so `//lib/b`
/// always finishes first.
#[test]
fn the_report_is_in_suite_order_whichever_finishes_first() {
    let scratch = Scratch::repo("parallel-order");
    suite(&scratch, "a", 2);
    suite(&scratch, "b", 2);
    let done = scratch.path("b-done");
    let js = js_path();
    let script = runtime_script(
        &scratch,
        &format!(
            "case \"$1\" in\n*lib/a/*)\n{}exec '{js}' \"$@\" ;;\n*)\n'{js}' \"$@\"\nstatus=$?\n: > '{done}'\nexit $status ;;\nesac\n",
            wait_for(&format!("[ -e '{}' ]", done.display()), "the other suite never finished"),
            done = done.display(),
        ),
    );
    let report = || {
        let _ = std::fs::remove_file(&done);
        let run = scratch.run_with_env(
            &["test", "//...", "--output=js", "--jobs=2"],
            &[("BURI_JS", &script), SMALL_HOST],
        );
        run.exits(1);
        // Everything but the summary line, which carries the time taken.
        run.stdout
            .lines()
            .filter(|l| !l.contains(" passed, "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let first = report();
    let a = first
        .find("a answers")
        .unwrap_or_else(|| panic!("no report for //lib/a:\n{first}"));
    let b = first
        .find("b answers")
        .unwrap_or_else(|| panic!("no report for //lib/b:\n{first}"));
    assert!(
        a < b,
        "//lib/b, which finished first, was reported first:\n{first}"
    );
    assert_eq!(first, report(), "two runs of one tree reported differently");
}

/// The status `--explain` gave `test //lib/<name>`.
fn test_status(run: &Run, name: &str) -> String {
    let label = format!("//lib/{name}");
    run.stdout
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .find(|f| f.len() == 5 && f[1] == "test" && f[2] == label)
        .map(|f| f[0].to_string())
        .unwrap_or_else(|| panic!("no `test {label}` line in:\n{}", indent(&run.all())))
}

/// Two suites over one shared library.
fn shared_library(scratch: &Scratch) {
    scratch.write(
        "lib/shared/BUILD.buri",
        "library {\n  visibility: [\"//...\"]\n}\n",
    );
    scratch.write("lib/shared/lib.buri", "export fn base(): I64 {\n  20\n}\n");
    for name in ["a", "b"] {
        scratch.write(
            &format!("lib/{name}/BUILD.buri"),
            &format!(
                "library {{\n  dependencies: [\"//lib/shared\"]\n  test {{ sources: [\"test/{name}.buri\"] }}\n}}\n"
            ),
        );
        scratch.write(
            &format!("lib/{name}/lib.buri"),
            "from \"//lib/shared\" import { base };\n\nexport fn answer(): I64 { base() + 1 }\n",
        );
        scratch.write(
            &format!("lib/{name}/test/{name}.buri"),
            &format!(
                "from \"//lib/{name}\" import {{ answer }};\n\
                 from \"core/testing/assert\" import * as assert;\n\
                 \ntest \"{name} answers\" {{\n  assert.equal(answer(), 21);\n}}\n"
            ),
        );
    }
}

/// A comment and whitespace edit in a shared library re-runs no suite, and an
/// edit that changes what it computes re-runs every suite above it.
#[test]
fn only_an_edit_that_can_change_behaviour_re_runs_a_suite() {
    let scratch = Scratch::repo("cutoff");
    shared_library(&scratch);
    let first = scratch.run(&["test", "//...", "--explain"]);
    if first.stderr.contains("test-run-unavailable") {
        // This toolchain cannot run a suite on its own host.
        first.exits(1);
        return;
    }
    first.ok();
    assert_eq!(
        (test_status(&first, "a"), test_status(&first, "b")),
        ("run".into(), "run".into())
    );

    scratch.write(
        "lib/shared/lib.buri",
        "// The number every answer starts from.\nexport fn base(): I64 {\n        20 // twenty\n}\n",
    );
    let comment = scratch.run(&["test", "//...", "--explain"]);
    comment.ok();
    assert_eq!(
        (test_status(&comment, "a"), test_status(&comment, "b")),
        ("cached".into(), "cached".into()),
        "a comment edit re-ran a suite:\n{}",
        indent(&comment.all())
    );
    assert!(
        comment.stdout.contains("2 cached)"),
        "{}",
        indent(&comment.all())
    );

    scratch.write("lib/shared/lib.buri", "export fn base(): I64 {\n  30\n}\n");
    let behaviour = scratch.run(&["test", "//...", "--explain"]);
    assert_eq!(
        (test_status(&behaviour, "a"), test_status(&behaviour, "b")),
        ("run".into(), "run".into()),
        "an edit that changes behaviour was served from the cache:\n{}",
        indent(&behaviour.all())
    );
    behaviour.exits(1);
    assert_eq!(behaviour.tests_passed(), 0, "{}", indent(&behaviour.all()));
}

/// A suite that doesn't type check leaves its batch, and the rest still share
/// a binary.
#[test]
fn a_broken_suite_leaves_the_others_batched() {
    let scratch = Scratch::repo("batch-without-broken");
    c_broken_by(&scratch, "export fn answer(): I64 { missing() }\n");
}

/// So does a suite whose library imports a package that isn't there.
#[test]
fn a_suite_importing_a_missing_package_leaves_the_others_batched() {
    let scratch = Scratch::repo("batch-without-missing-import");
    c_broken_by(
        &scratch,
        "from \"//lib/nowhere\" import { gone };\n\nexport fn answer(): I64 { gone() }\n",
    );
}

/// Suites `a`, `b` and `c`, where `c`'s library is `lib`: `c` fails to
/// compile, and `a` and `b` pass in one binary.
fn c_broken_by(scratch: &Scratch, lib: &str) {
    suite(scratch, "a", 1);
    suite(scratch, "b", 1);
    suite(scratch, "c", 1);
    scratch.write("lib/c/lib.buri", lib);
    let run = scratch.run(&["test", "//...", "--explain"]);
    if run.stderr.contains("test-run-unavailable") {
        run.exits(1);
        return;
    }
    run.exits(1);
    assert_eq!(run.tests_passed(), 2, "{}", indent(&run.all()));
    assert!(
        run.stdout.contains("1 failed to compile"),
        "{}",
        indent(&run.all())
    );
    let shared = run.stdout.lines().any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() == 5 && f[1] == "link" && f[2] == "//lib/a,//lib/b"
    });
    assert!(
        shared,
        "the suites that compile did not share a binary:\n{}",
        indent(&run.all())
    );
}

/// Many suites, each in its own binary, linked and run side by side, all pass.
///
/// On Linux, a child forked while `buri` was writing one binary held it open
/// until its own `exec`, and running the binary then failed with `Text file
/// busy`. Forty suites hit that in most runs. Each round deletes `.buri` so
/// every binary is written again.
#[test]
fn suites_linked_and_run_side_by_side_all_start() {
    let scratch = Scratch::repo("link-and-run-side-by-side");
    let count = 40;
    for i in 0..count {
        suite(&scratch, &format!("s{i}"), 1);
    }
    for round in 1..=3 {
        let _ = std::fs::remove_dir_all(scratch.path(".buri"));
        // A batch limit of one byte puts every suite in a binary of its own.
        let run = scratch.run_with_env(
            &["test", "//...", "--force"],
            &[("BURI_TEST_BATCH_BYTES", "1")],
        );
        run.ok();
        assert_eq!(
            run.tests_passed(),
            count,
            "round {round}:\n{}",
            indent(&run.all())
        );
    }
}

/// A suite named on its own spreads its tests over several processes, as a
/// suite named beside another one does.
///
/// `buri` is held once it has handed a process its first test, which keeps
/// that process waiting for its next. Another process must be handed a test
/// before the first is let go. Run one process at a time, the second handing
/// never comes, and the wait fails the test.
#[cfg(unix)]
#[test]
fn a_suite_named_alone_runs_its_tests_side_by_side() {
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::scheduling", &gap);
        return;
    }
    let scratch = Scratch::repo("alone-side-by-side");
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n  test { sources: [\"test/a.buri\"] }\n}\n",
    );
    scratch.write("lib/a/lib.buri", "export fn one(): Int { 1 }\n");
    let names: Vec<String> = (0..8).map(|i| format!("test {i}")).collect();
    let tests: String = names
        .iter()
        .map(|name| format!("\ntest \"{name}\" {{\n  assert.equal(one(), 1);\n}}\n"))
        .collect();
    scratch.write(
        "lib/a/test/a.buri",
        &format!("from \"//lib/a\" import {{ one }};\nfrom \"core/testing/assert\" import * as assert;\n{tests}"),
    );
    let held = crate::held::Held::new("alone-side-by-side");
    held.hold("handed", 1);
    held.hold("handed", 2);
    let mut run = held.start(
        &scratch,
        &["test", "//lib/a", "--verbose", "--jobs=4", "--color=never"],
    );
    let first = held.reached("handed", 1);
    let second = held.reached("handed", 2);
    first.go();
    second.go();
    assert!(run.capped_wait().unwrap().success(), "{}", held.printed());
    assert!(
        !held.printed().contains(HEAP_CHECK_LINE),
        "{}",
        held.printed()
    );
    // Listed in the order they're declared, each with its own time.
    let stdout = std::fs::read_to_string(held.dir().join("stdout")).unwrap();
    let listed: Vec<String> = stdout
        .lines()
        .filter(|l| l.starts_with("  ok  "))
        .map(|l| {
            let blanked = blank_test_time(l);
            assert!(
                blanked.ends_with("<time>"),
                "an untimed test: {l:?}\n{}",
                indent(&stdout)
            );
            blanked
                .trim_end_matches("<time>")
                .split_whitespace()
                .skip(2)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_eq!(listed, names, "{}", indent(&stdout));
}

/// A suite named on its own reports exactly what it reported when its tests
/// ran one process at a time: the failures in block order, each with its
/// diff, the `--verbose` list, what `--filter` skips, and the status.
#[test]
fn a_suite_named_alone_reports_as_it_did_one_process_at_a_time() {
    let scratch = Scratch::repo("alone-report");
    scratch.write(
        "lib/mixed/BUILD.buri",
        "library {\n  test { sources: [\"test/a.buri\", \"test/b.buri\"] }\n}\n",
    );
    scratch.write("lib/mixed/lib.buri", "export fn answer(): I64 { 1 }\n");
    scratch.write(
        "lib/mixed/test/a.buri",
        "from \"//lib/mixed\" import { answer };\nfrom \"core/testing/assert\" import * as assert;\n\n\
         test \"a passes\" {\n  assert.equal(answer(), 1);\n}\n\n\
         test \"a compares wrong\" {\n  assert.equal(answer(), 2);\n}\n\n\
         test \"a passes after a failure\" {\n  assert.equal(answer() + 1, 2);\n}\n\n\
         test \"a fails again\" {\n  assert.isTrue(answer() > 1);\n}\n",
    );
    scratch.write(
        "lib/mixed/test/b.buri",
        "from \"core/testing/assert\" import * as assert;\n\n\
         test \"b passes\" {\n  assert.equal(\"x\", \"x\");\n}\n\n\
         test \"b fails\" {\n  assert.equal(\"x\", \"y\");\n}\n",
    );
    let report = |args: &[&str]| {
        let run = scratch.run(args);
        run.heap_ok();
        let lines: Vec<String> = run
            .stdout
            .lines()
            .map(|l| {
                if l.contains(" passed, ") {
                    l.split(" (").next().unwrap_or(l).to_string()
                } else {
                    blank_test_time(l)
                }
            })
            .collect();
        (run.code, lines.join("\n"), run.stderr.clone())
    };
    let first = report(&["test", "//lib/mixed", "--force"]);
    if first.2.contains("test-run-unavailable") {
        return;
    }
    let failures = "\
FAIL //lib/mixed  test/a.buri  \"a compares wrong\"
  assert.equal failed
    actual:   1
    expected: 2
  --> lib/mixed/test/a.buri:8:1
FAIL //lib/mixed  test/a.buri  \"a fails again\"
  assert.isTrue failed
    actual:   false
    expected: true
  --> lib/mixed/test/a.buri:16:1
FAIL //lib/mixed  test/b.buri  \"b fails\"
  assert.equal failed
    actual:   \"x\"
    expected: \"y\"
  --> lib/mixed/test/b.buri:7:1
";
    assert_eq!(
        first,
        (
            1,
            format!("{failures}\n3 passed, 3 failed, 0 skipped"),
            String::new()
        )
    );
    let verbose = report(&["test", "//lib/mixed", "--force", "--verbose"]);
    let list = "\
//lib/mixed  native  6 tests  <time>  built in <time>
  ok    a.buri  a passes                  <time>
  FAIL  a.buri  a compares wrong          <time>
  ok    a.buri  a passes after a failure  <time>
  FAIL  a.buri  a fails again             <time>
  ok    b.buri  b passes                  <time>
  FAIL  b.buri  b fails                   <time>
shared build  <time>
";
    assert_eq!(
        verbose,
        (
            1,
            format!("{list}\n{failures}\n3 passed, 3 failed, 0 skipped"),
            String::new()
        )
    );
    // A filtered binary holds only the tests it runs, and each is timed as itself.
    let filtered = report(&[
        "test",
        "//lib/mixed",
        "--force",
        "--verbose",
        "--filter=passes",
    ]);
    let list = "\
//lib/mixed  native  3 tests  <time>  built in <time>
  ok    a.buri  a passes                  <time>
  ok    a.buri  a passes after a failure  <time>
  ok    b.buri  b passes                  <time>
  skip  a.buri  a compares wrong
  skip  a.buri  a fails again
  skip  b.buri  b fails
shared build  <time>
";
    assert_eq!(
        filtered,
        (
            0,
            format!("{list}\n3 passed, 0 failed, 3 skipped"),
            String::new()
        )
    );
}

/// Every `test` line `--explain` printed, as `(label, platform)` and its status.
fn test_statuses(run: &Run) -> std::collections::BTreeMap<(String, String), String> {
    run.stdout
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|f| f.len() == 5 && f[1] == "test")
        .map(|f| ((f[2].to_string(), f[3].to_string()), f[0].to_string()))
        .collect()
}

/// **Each of many suites is reused or re-run on its own key** (PERFORMANCE.md
/// §6.76). `buri test` keys every suite on all cores before it looks any up.
/// Thirty-six suites, a third native, a third on JavaScript and a third on
/// both, run, then run again with nothing changed, then with three of them
/// broken, then fixed. A key handed to the wrong suite would serve one suite
/// another's verdict, or re-run one nobody touched.
#[test]
fn many_suites_are_each_reused_or_re_run_on_their_own_key() {
    let scratch = Scratch::repo("many-suites-keyed");
    let count = 36;
    for i in 0..count {
        let backends = match i % 3 {
            0 => "",
            1 => "\n    backends: [JS]",
            _ => "\n    backends: [JS, NATIVE]",
        };
        scratch.write(
            &format!("lib/s{i}/BUILD.buri"),
            &format!(
                "library {{\n  test {{\n    sources: [\"test/s{i}.buri\"]{backends}\n  }}\n}}\n"
            ),
        );
        scratch.write(
            &format!("lib/s{i}/lib.buri"),
            &format!("export fn answer(): I64 {{ {i} }}\n"),
        );
        scratch.write(
            &format!("lib/s{i}/test/s{i}.buri"),
            &format!(
                "from \"//lib/s{i}\" import {{ answer }};\n\
                 from \"core/testing/assert\" import * as assert;\n\
                 \ntest \"s{i} answers\" {{\n  assert.equal(answer(), {i});\n}}\n"
            ),
        );
    }
    let runs = count + count / 3;
    let explain = || scratch.run(&["test", "//...", "--explain"]);
    let first = explain();
    if first.stderr.contains("test-run-unavailable") {
        first.exits(1);
        return;
    }
    first.ok();
    assert_eq!(first.tests_passed(), runs, "{}", indent(&first.all()));
    let statuses = test_statuses(&first);
    assert_eq!(statuses.len(), runs, "{}", indent(&first.all()));
    assert!(
        statuses.values().all(|s| s == "run"),
        "{}",
        indent(&first.all())
    );

    let again = explain();
    again.ok();
    assert!(
        test_statuses(&again).values().all(|s| s == "cached"),
        "{}",
        indent(&again.all())
    );
    assert!(
        again
            .stdout
            .contains(&format!("{runs} passed, 0 failed, 0 skipped")),
        "{}",
        indent(&again.all())
    );
    assert!(
        again.stdout.contains(&format!("{runs} cached)")),
        "{}",
        indent(&again.all())
    );

    // 12 is native, 22 is on JavaScript, and 35 on both.
    let broken = [12, 22, 35];
    for i in broken {
        scratch.edit(
            &format!("lib/s{i}/test/s{i}.buri"),
            &format!("answer(), {i})"),
            &format!("answer(), {})", i + 1),
        );
    }
    let failing = explain();
    failing.exits(1);
    for ((label, platform), status) in test_statuses(&failing) {
        let touched = broken.iter().any(|i| label == format!("//lib/s{i}"));
        let want = if touched { "run" } else { "cached" };
        assert_eq!(
            status,
            want,
            "{label} on {platform}:\n{}",
            indent(&failing.all())
        );
    }
    assert!(
        failing
            .stdout
            .contains(&format!("{} passed, 4 failed, 0 skipped", runs - 4)),
        "{}",
        indent(&failing.all())
    );
    let failed: Vec<&str> = failing
        .stdout
        .lines()
        .filter_map(|l| l.strip_prefix("FAIL "))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(
        failed,
        ["//lib/s12", "//lib/s22", "//lib/s35", "//lib/s35"],
        "{}",
        indent(&failing.all())
    );

    for i in broken {
        scratch.edit(
            &format!("lib/s{i}/test/s{i}.buri"),
            &format!("answer(), {})", i + 1),
            &format!("answer(), {i})"),
        );
    }
    let fixed = explain();
    fixed.ok();
    assert!(
        fixed
            .stdout
            .contains(&format!("{runs} passed, 0 failed, 0 skipped")),
        "{}",
        indent(&fixed.all())
    );
}

/// A runner that dies once it has asked for a test and none is left has
/// nothing to blame: its suite runs again on its own, and passes.
///
/// Two runners are each handed one of two tests. The one handed the second ends
/// with it. `buri` is then held where it tells the other there is nothing more
/// to run, and that runner is killed there, before its input closes, with no
/// test in hand.
#[cfg(unix)]
#[test]
fn a_runner_killed_with_no_test_in_hand_is_run_again() {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGKILL: i32 = 9;
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::scheduling", &gap);
        return;
    }
    let scratch = Scratch::repo("runner-killed-drained");
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n  test { sources: [\"test/a.buri\"] }\n}\n",
    );
    scratch.write("lib/a/lib.buri", "export fn one(): Int { 1 }\n");
    scratch.write(
        "lib/a/test/a.buri",
        "from \"//lib/a\" import { one };\n\
         from \"core/testing/assert\" import * as assert;\n\
         \ntest \"first\" {\n  assert.equal(one(), 1);\n}\n\
         \ntest \"second\" {\n  assert.equal(one(), 1);\n}\n",
    );
    let held = crate::held::Held::new("runner-killed-drained");
    held.hold("handed", 1);
    held.hold("handed", 2);
    held.hold("drained", 1);
    let mut run = held.start_with_env(
        &scratch,
        &["test", "//lib/a", "--jobs=2", "--color=never"],
        &[LINKED],
    );
    let runners = || -> Vec<i32> {
        let found = std::process::Command::new("pgrep")
            .args(["-P", &run.id().to_string(), "-x", "test-library"])
            .capped_output()
            .expect("pgrep runs");
        String::from_utf8_lossy(&found.stdout)
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
            .collect()
    };
    let first = held.reached("handed", 1);
    let second = held.reached("handed", 2);
    second.go();
    let started = std::time::Instant::now();
    while runners().len() > 1 {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(240),
            "the second runner never ended:\n{}",
            held.printed()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    first.go();
    let draining = held.reached("drained", 1);
    for pid in runners() {
        // SAFETY: an ordinary `kill` of a child of the run.
        unsafe { kill(pid, SIGKILL) };
    }
    draining.go();
    assert!(run.capped_wait().unwrap().success(), "{}", held.printed());
    assert!(held.printed().contains("2 passed"), "{}", held.printed());
}

/// Three suites of uneven size, `//lib/many` with the most tests and
/// `//lib/few` with the fewest, and tests of uneven length in each. Two of
/// them fail, one of them the longest test of the largest suite.
fn uneven(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    let tests = [
        (
            "few",
            "test \"few passes\" {\n  assert.equal(one(), 1);\n}\n",
        ),
        (
            "many",
            "test \"many short\" {\n  assert.equal(one(), 1);\n}\n\n\
             test \"many longest fails\" {\n  let two = one() + one();\n  let three = two + one();\n  \
             assert.equal(two, 2);\n  assert.equal(three, 3);\n  assert.equal(three + one(), 5);\n}\n\n\
             test \"many middle\" {\n  let two = one() + one();\n  assert.equal(two, 2);\n}\n",
        ),
        (
            "some",
            "test \"some fails\" {\n  assert.equal(one(), 2);\n}\n\n\
             test \"some longer\" {\n  let two = one() + one();\n  assert.equal(two, 2);\n}\n",
        ),
    ];
    for (lib, body) in tests {
        scratch.write(
            &format!("lib/{lib}/BUILD.buri"),
            &format!("library {{\n  test {{ sources: [\"test/{lib}.buri\"] }}\n}}\n"),
        );
        scratch.write(
            &format!("lib/{lib}/lib.buri"),
            "export fn one(): Int { 1 }\n",
        );
        scratch.write(
            &format!("lib/{lib}/test/{lib}.buri"),
            &format!(
                "from \"//lib/{lib}\" import {{ one }};\n\
                 from \"core/testing/assert\" import * as assert;\n\n{body}"
            ),
        );
    }
    scratch
}

/// **Suites of uneven size report in label order, and each suite's tests in
/// the order they're declared**, whichever ran first and however many run at
/// once. The failures read the same as well, the one in the longest test of
/// the largest suite included.
#[test]
fn uneven_suites_report_in_label_and_declared_order() {
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::scheduling", &gap);
        return;
    }
    let scratch = uneven("uneven-order");
    let report = |args: &[&str]| {
        let run = scratch.run(args);
        run.heap_ok();
        let lines: Vec<String> = run
            .stdout
            .lines()
            .map(|l| {
                if l.contains(" passed, ") {
                    l.split(" (").next().unwrap_or(l).to_string()
                } else {
                    blank_test_time(l)
                }
            })
            .collect();
        (run.code, lines.join("\n"), run.stderr.clone())
    };
    let failures = "\
FAIL //lib/many  test/many.buri  \"many longest fails\"
  assert.equal failed
    actual:   4
    expected: 5
  --> lib/many/test/many.buri:8:1
FAIL //lib/some  test/some.buri  \"some fails\"
  assert.equal failed
    actual:   1
    expected: 2
  --> lib/some/test/some.buri:4:1
";
    let list = "\
//lib/few  native  1 test  <time>  built with others
  ok    few.buri  few passes  <time>
//lib/many  native  3 tests  <time>  built with others
  ok    many.buri  many short          <time>
  FAIL  many.buri  many longest fails  <time>
  ok    many.buri  many middle         <time>
//lib/some  native  2 tests  <time>  built with others
  FAIL  some.buri  some fails   <time>
  ok    some.buri  some longer  <time>
shared build  <time>
";
    for jobs in ["--jobs=1", "--jobs=3"] {
        let plain = report(&["test", "//...", "--force", "--color=never", jobs]);
        assert_eq!(
            plain,
            (
                1,
                format!("{failures}\n4 passed, 2 failed, 0 skipped"),
                String::new()
            ),
            "{jobs}"
        );
        let verbose = report(&[
            "test",
            "//...",
            "--force",
            "--verbose",
            "--color=never",
            jobs,
        ]);
        assert_eq!(
            verbose,
            (
                1,
                format!("{list}\n{failures}\n4 passed, 2 failed, 0 skipped"),
                String::new()
            ),
            "{jobs}"
        );
    }
}

/// **The suite with the most tests is handed out first, and each suite's
/// longest tests before its shorter ones**, so the tests likeliest to take
/// longest start while the other suites can still fill the slots beside them.
/// One job at a time, `BURI_PROFILE` lists the order exactly.
#[test]
fn the_largest_suite_and_its_longest_tests_are_handed_out_first() {
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::scheduling", &gap);
        return;
    }
    let scratch = uneven("uneven-handed");
    let run = scratch.run_with_env(&["test", "//...", "--jobs=1"], &[("BURI_PROFILE", "1")]);
    run.exits(1);
    let handed = run
        .stderr
        .lines()
        .find_map(|l| l.strip_prefix("first tests handed out: "))
        .unwrap_or_else(|| panic!("no order in the profile:\n{}", indent(&run.all())));
    assert_eq!(
        handed,
        "//lib/many \"many longest fails\", //lib/many \"many middle\", \
         //lib/many \"many short\", //lib/some \"some longer\", \
         //lib/some \"some fails\", //lib/few \"few passes\"",
        "{}",
        indent(&run.all())
    );
}

/// **The profile names the first ten tests handed out**, however many more a
/// run hands out after them.
#[test]
fn the_profile_names_only_the_first_ten_tests_handed_out() {
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::scheduling", &gap);
        return;
    }
    let scratch = Scratch::repo("twelve-handed");
    let tests: String = (0..12)
        .map(|i| format!("test \"t{i:02}\" {{\n  assert.equal(one(), 1);\n}}\n\n"))
        .collect();
    scratch.write(
        "lib/twelve/BUILD.buri",
        "library {\n  test { sources: [\"test/twelve.buri\"] }\n}\n",
    );
    scratch.write("lib/twelve/lib.buri", "export fn one(): Int { 1 }\n");
    scratch.write(
        "lib/twelve/test/twelve.buri",
        &format!(
            "from \"//lib/twelve\" import {{ one }};\n\
             from \"core/testing/assert\" import * as assert;\n\n{tests}"
        ),
    );
    let run = scratch.run_with_env(&["test", "//...", "--jobs=1"], &[("BURI_PROFILE", "1")]);
    run.ok().says("12 passed, 0 failed, 0 skipped");
    assert_eq!(handed_out(&run).len(), 10, "{}", indent(&run.all()));
}

/// A test body that adds up a list of a million numbers: milliseconds of work
/// in a few lines, where the other tests take microseconds.
const SLOW_BODY: &str = "  let ctx = context { Allocator: alloc() };\n  \
     assert.greaterThan(list.range(ctx, 0, 1000000).fold(fn(sum, i) => sum + i % 7, 0), 0);\n";

/// `//lib/many`, three quick tests, and `//lib/slow`, two: "slow short", the
/// shorter but the slow one ([`SLOW_BODY`]), then "quick longest".
fn timed(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    let tests = [
        (
            "many",
            "\ntest \"many one\" {\n  let two = one() + one();\n  let three = two + one();\n  \
             assert.equal(three, 3);\n}\n\n\
             test \"many two\" {\n  let two = one() + one();\n  assert.equal(two, 2);\n}\n\n\
             test \"many three\" {\n  let two = one() + one();\n  let three = two + one();\n  \
             assert.equal(three + one(), 4);\n}\n"
                .to_string(),
        ),
        (
            "slow",
            format!(
                "from \"core/list\" import * as list;\n\
                 from \"platform/effect\" import {{ Allocator }};\n\
                 from \"platform/effect/testing\" import {{ alloc }};\n\n\
                 test \"slow short\" {{\n{SLOW_BODY}}}\n\n\
                 test \"quick longest\" {{\n  let two = one() + one();\n  let three = two + one();\n  \
                 let four = three + one();\n  let five = four + one();\n  let six = five + one();\n  \
                 assert.equal(two, 2);\n  assert.equal(three, 3);\n  assert.equal(four, 4);\n  \
                 assert.equal(five, 5);\n  assert.equal(six, 6);\n}}\n"
            ),
        ),
    ];
    for (lib, body) in tests {
        scratch.write(
            &format!("lib/{lib}/BUILD.buri"),
            &format!("library {{\n  test {{ sources: [\"test/{lib}.buri\"] }}\n}}\n"),
        );
        scratch.write(
            &format!("lib/{lib}/lib.buri"),
            "export fn one(): Int { 1 }\n",
        );
        scratch.write(
            &format!("lib/{lib}/test/{lib}.buri"),
            &format!(
                "from \"//lib/{lib}\" import {{ one }};\n\
                 from \"core/testing/assert\" import * as assert;\n{body}"
            ),
        );
    }
    scratch
}

/// The `first tests handed out:` entries of a run under `BURI_PROFILE`.
fn handed_out(run: &Run) -> Vec<String> {
    let line = run
        .stderr
        .lines()
        .find_map(|l| l.strip_prefix("first tests handed out: "))
        .unwrap_or_else(|| panic!("no order in the profile:\n{}", indent(&run.all())));
    line.split(", ").map(str::to_string).collect()
}

/// **A test that took longest last time is handed out first the next time**,
/// and its suite before a suite with more tests, whatever the length of its
/// source. The first run has no times, so it goes by test count and source
/// length. One job at a time, `BURI_PROFILE` lists the order exactly.
#[test]
fn the_test_slowest_last_time_is_handed_out_first() {
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::scheduling", &gap);
        return;
    }
    let scratch = timed("timed-handed");
    let args = ["test", "//...", "--force", "--jobs=1"];
    let first = scratch.run_with_env(&args, &[("BURI_PROFILE", "1")]);
    first.heap_ok();
    assert_eq!(
        handed_out(&first),
        [
            "//lib/many \"many three\"",
            "//lib/many \"many one\"",
            "//lib/many \"many two\"",
            "//lib/slow \"quick longest\"",
            "//lib/slow \"slow short\"",
        ],
        "{}",
        indent(&first.all())
    );
    let again = scratch.run_with_env(&args, &[("BURI_PROFILE", "1")]);
    again.heap_ok();
    let handed = handed_out(&again);
    // `//lib/many`'s tests all take microseconds, so their order among
    // themselves is whichever happened to be slowest.
    let mut rest = handed[2..].to_vec();
    rest.sort();
    assert_eq!(
        (&handed[..2], rest),
        (
            &[
                "//lib/slow \"slow short\"".to_string(),
                "//lib/slow \"quick longest\"".to_string()
            ][..],
            vec![
                "//lib/many \"many one\"".to_string(),
                "//lib/many \"many three\"".to_string(),
                "//lib/many \"many two\"".to_string(),
            ]
        ),
        "{}",
        indent(&again.all())
    );
}

/// **A run that goes by the last run's times reports exactly as the run
/// before it did**: suites in label order, and each suite's tests in the order
/// they're declared.
#[test]
fn a_run_ordered_by_last_times_reports_as_before() {
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::scheduling", &gap);
        return;
    }
    let scratch = timed("timed-report");
    let report = || {
        let run = scratch.run(&[
            "test",
            "//...",
            "--force",
            "--verbose",
            "--color=never",
            "--jobs=2",
        ]);
        run.heap_ok();
        let lines: Vec<String> = run
            .stdout
            .lines()
            .map(|l| {
                if l.contains(" passed, ") {
                    l.split(" (").next().unwrap_or(l).to_string()
                } else {
                    blank_test_time(l)
                }
            })
            .collect();
        (lines.join("\n"), run.stderr.clone())
    };
    let list = "\
//lib/many  native  3 tests  <time>  built with others
  ok    many.buri  many one    <time>
  ok    many.buri  many two    <time>
  ok    many.buri  many three  <time>
//lib/slow  native  2 tests  <time>  built with others
  ok    slow.buri  slow short     <time>
  ok    slow.buri  quick longest  <time>
shared build  <time>

5 passed, 0 failed, 0 skipped";
    let first = report();
    assert_eq!(first, (list.to_string(), String::new()));
    assert_eq!(report(), first);
}

/// A suite `//lib/long` of forty tests, `t00` to `t39`, each one line longer
/// than the one before, so `t39` is the longest. `t03` and `t37` fail.
fn forty_tests(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    let tests: String = (0..40)
        .map(|i| {
            let lines: String = (0..i)
                .map(|line| {
                    let want = if line == 1 && (i == 3 || i == 37) {
                        2
                    } else {
                        1
                    };
                    format!("  assert.equal(one(), {want});\n")
                })
                .collect();
            format!("test \"t{i:02}\" {{\n{lines}}}\n\n")
        })
        .collect();
    scratch.write(
        "lib/long/BUILD.buri",
        "library {\n  test { sources: [\"test/long.buri\"] }\n}\n",
    );
    scratch.write("lib/long/lib.buri", "export fn one(): Int { 1 }\n");
    scratch.write(
        "lib/long/test/long.buri",
        &format!(
            "from \"//lib/long\" import {{ one }};\n\
             from \"core/testing/assert\" import * as assert;\n\n{tests}"
        ),
    );
    scratch
}

/// **A long JavaScript suite runs in one process**, with workers to spare, so
/// the runtime and the suite's modules load once. A second process loads them
/// again, which on this suite about doubles the CPU it takes.
#[test]
fn a_long_javascript_suite_runs_in_one_process() {
    let scratch = forty_tests("js-one-process");
    let run = scratch.run_with_env(
        &["test", "//...", "--output=js", "--jobs=3"],
        &[("BURI_PROFILE", "1")],
    );
    run.exits(1);
    assert!(
        run.stderr.lines().any(|l| l == "test processes 1"),
        "{}",
        indent(&run.all())
    );
}

/// **A long JavaScript suite reports its tests in the order they're
/// declared**, failures included, whatever `--jobs` allows.
#[test]
fn a_javascript_suite_reports_in_declared_order_whichever_ran_first() {
    let report = |jobs: &str| {
        let scratch = forty_tests(&format!("js-order{jobs}"));
        let run = scratch.run(&[
            "test",
            "//...",
            "--output=js",
            "--verbose",
            "--color=never",
            jobs,
        ]);
        run.exits(1);
        let lines: Vec<String> = run
            .stdout
            .lines()
            .map(|l| {
                if l.contains(" passed, ") {
                    l.split(" (").next().unwrap_or(l).to_string()
                } else {
                    blank_test_time(l)
                }
            })
            .collect();
        (lines.join("\n"), run.stderr.clone())
    };
    let (alone, alone_err) = report("--jobs=1");
    let (spread, spread_err) = report("--jobs=3");
    assert_eq!((&spread, &spread_err), (&alone, &alone_err));
    let listed: Vec<&str> = spread
        .lines()
        .filter(|l| l.starts_with("  ok  ") || l.starts_with("  FAIL  "))
        .filter_map(|l| l.split_whitespace().nth(2))
        .collect();
    let declared: Vec<String> = (0..40).map(|i| format!("t{i:02}")).collect();
    assert_eq!(listed, declared, "{spread}");
    let failed: Vec<&str> = spread
        .lines()
        .filter_map(|l| l.strip_prefix("FAIL //lib/long  test/long.buri  "))
        .collect();
    assert_eq!(failed, ["\"t03\"", "\"t37\""], "{spread}");
    assert!(
        spread.ends_with("38 passed, 2 failed, 0 skipped"),
        "{spread}"
    );
}

/// **A suite whose process waits with nothing to run doesn't hold its job's
/// place**, so another suite starts beside it. One job at a time, each suite's
/// runtime waits until the other's has started: held, the first would wait
/// alone and fail; let go while it waits, both go.
#[test]
fn a_suite_waiting_on_nothing_lets_another_start() {
    let scratch = Scratch::repo("idle-suites");
    suite(&scratch, "a", 1);
    suite(&scratch, "b", 1);
    let dir = scratch.path("arrived").display().to_string();
    let js = js_path();
    // A quarter of a second between looks, so the waiting shell is asleep
    // rather than starting a `sleep` twenty times a second.
    let wait = wait_for(
        &format!("[ \"$(/bin/ls '{dir}' | /usr/bin/wc -l)\" -ge 2 ]"),
        "the other suite never started while this one waited",
    )
    .replace("2400", "480")
    .replace("sleep 0.05", "sleep 0.25");
    let script = runtime_script(
        &scratch,
        &format!("/bin/mkdir -p '{dir}'\n: > '{dir}/'$$\n{wait}exec '{js}' \"$@\"\n"),
    );
    let run = scratch.run_with_env(
        &["test", "//...", "--output=js", "--jobs=1"],
        &[("BURI_JS", &script), SMALL_HOST],
    );
    run.ok();
    assert_eq!(
        run.tests_passed(),
        2,
        "the suites did not both run:\n{}",
        indent(&run.all())
    );
}

/// `//lib/wait`'s tests: the first sends into a mailbox its own step filled,
/// where nothing makes room, and waits there until a timer stops the actor
/// eight seconds later. It's the longer, so it's handed out first. A process
/// waiting on a send has no thread runnable, and the timer fires on the
/// waiting thread, so the wait is idle and bounded.
const WAIT_TESTS: &str = r#"enum Message<C> {
  Tick,
  Within(fn(C) => Int),
}

fn ticker<C: Allocator + Tasks>(): Actor<C, Int, Message<C>, Int> {
  Actor {
    state: 0,
    step: fn(c, count, message) => {
      match (message) {
        .Tick => Stepped { state: count + 1, answer: count + 1 },
        .Within(f) => {
          let call = f;
          Stepped { state: count, answer: call(c) }
        },
      }
    },
  }
}

test "a send into a mailbox its own step filled waits until a timer stops the actor" {
  let ctx = context {
    Allocator: alloc(),
    Tasks: testTasks(),
  };
  let ticks = actor.start(ctx, ticker());
  let inside = ticks.sendMessage(ctx, .Within(fn(c) => {
    let filled = list.range(c, 0, 64).foldCtx(c, fn(c2, sofar, _i) => {
      match (ticks.sendMessage(c2, .Tick)) {
        .Err(.WouldDeadlock) => sofar + 1,
        _ => sofar,
      }
    }, 0);
    let _ = tasks.after(c, time.seconds(8), fn(c2) => ticks.stop(c2).ignore());
    match (ticks.sendMessage(c, .Tick)) {
      .Err(.Stopped) => filled,
      _ => 0 - 1,
    }
  }));
  assert.equal(inside, .Ok(64));
  assert.equal(ticks.sendMessage(ctx, .Tick), .Err(.Stopped));
}

test "wait counts" {
  assert.equal(one(), 1);
}
"#;

/// Two suites, `//lib/wait`'s first test waiting on a timer.
fn waiting(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    let tests = [
        ("wait", WAIT_TESTS),
        (
            "quick",
            "test \"quick counts\" {\n  assert.equal(one(), 1);\n}\n",
        ),
    ];
    for (lib, body) in tests {
        scratch.write(
            &format!("lib/{lib}/BUILD.buri"),
            &format!("library {{\n  test {{ sources: [\"test/{lib}.buri\"] }}\n}}\n"),
        );
        scratch.write(
            &format!("lib/{lib}/lib.buri"),
            "export fn one(): Int { 1 }\n",
        );
        scratch.write(
            &format!("lib/{lib}/test/{lib}.buri"),
            &format!(
                "from \"//lib/{lib}\" import {{ one }};\n\
                 from \"core/actor\" import * as actor;\n\
                 from \"core/actor\" import {{ Actor, Stepped }};\n\
                 from \"core/list\" import * as list;\n\
                 from \"core/tasks\" import * as tasks;\n\
                 from \"core/time\" import * as time;\n\
                 from \"core/testing/assert\" import * as assert;\n\
                 from \"platform/effect\" import {{ Allocator, Tasks }};\n\
                 from \"platform/effect/testing\" import {{ alloc, tasks as testTasks }};\n\n{body}"
            ),
        );
    }
    scratch
}

/// **A test waiting on a timer lets another suite run meanwhile, and nothing
/// it reports changes.** One job at a time, `//lib/quick` is handed its test
/// while `//lib/wait` waits, rather than once `//lib/wait` is done; the send
/// still ends when the timer stops the actor, and the report reads as it
/// always has.
#[test]
fn a_test_waiting_on_a_timer_lets_another_suite_run() {
    if let Some(gap) = ci::native_host_gap() {
        ci::skipped("build::scheduling", &gap);
        return;
    }
    let scratch = waiting("idle-timer");
    let run = scratch.run_with_env(
        &["test", "//...", "--jobs=1", "--verbose", "--color=never"],
        &[("BURI_PROFILE", "1")],
    );
    run.heap_ok();
    let report: Vec<String> = run
        .stdout
        .lines()
        .map(|l| {
            if l.contains(" passed, ") {
                l.split(" (").next().unwrap_or(l).to_string()
            } else {
                blank_test_time(l)
            }
        })
        .collect();
    assert_eq!(
        report.join("\n"),
        "\
//lib/quick  native  1 test  <time>  built with others
  ok    quick.buri  quick counts  <time>
//lib/wait  native  2 tests  <time>  built with others
  ok    wait.buri  a send into a mailbox its own step filled waits until a timer stops the actor  <time>
  ok    wait.buri  wait counts                                                                    <time>
shared build  <time>

3 passed, 0 failed, 0 skipped",
        "{}",
        indent(&run.all())
    );
    let handed = run
        .stderr
        .lines()
        .find_map(|l| l.strip_prefix("first tests handed out: "))
        .unwrap_or_else(|| panic!("no order in the profile:\n{}", indent(&run.all())));
    assert_eq!(
        handed,
        "//lib/wait \"a send into a mailbox its own step filled waits until a timer stops the actor\", \
         //lib/quick \"quick counts\", //lib/wait \"wait counts\"",
        "{}",
        indent(&run.all())
    );
}
