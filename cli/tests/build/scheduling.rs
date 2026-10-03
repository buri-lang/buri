//! How `buri test` schedules suites: side by side, reported in suite order, and
//! reused when an edit cannot change what they do.
use crate::harness::*;

/// A package `lib/<name>` with one suite asserting `answer() == want`.
fn suite(scratch: &Scratch, name: &str, want: i64) {
    scratch.write(
        &format!("lib/{name}/BUILD.buri"),
        &format!("library {{\n  test {{ sources: [\"test/{name}.buri\"] }}\n}}\n"),
    );
    scratch.write(&format!("lib/{name}/lib.buri"), "export fn answer(): I64 { 1 }\n");
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
        .output()
        .expect("sh runs");
    String::from_utf8_lossy(&found.stdout).trim().to_string()
}

/// Writes an executable script standing in for the JavaScript runtime, and
/// answers its path for `BURI_JS`.
fn runtime_script(scratch: &Scratch, body: &str) -> String {
    let path = scratch.write("runtime.sh", &format!("#!/bin/sh\n{body}"));
    std::process::Command::new("/bin/chmod").arg("+x").arg(&path).status().expect("chmod runs");
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
/// the way they would on a small machine whatever this one has.
const SMALL_HOST: (&str, &str) = ("BURI_TEST_MEMORY_BYTES", "8589934592");

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
    assert_eq!(run.tests_passed(), 2, "the suites did not both run:\n{}", indent(&run.all()));
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
        let run = scratch
            .run_with_env(&["test", "//...", "--output=js", "--jobs=2"], &[("BURI_JS", &script), SMALL_HOST]);
        run.exits(1);
        // Everything but the summary line, which carries the time taken.
        run.stdout.lines().filter(|l| !l.contains(" passed, ")).collect::<Vec<_>>().join("\n")
    };
    let first = report();
    let a = first.find("a answers").unwrap_or_else(|| panic!("no report for //lib/a:\n{first}"));
    let b = first.find("b answers").unwrap_or_else(|| panic!("no report for //lib/b:\n{first}"));
    assert!(a < b, "//lib/b, which finished first, was reported first:\n{first}");
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
    assert_eq!((test_status(&first, "a"), test_status(&first, "b")), ("run".into(), "run".into()));

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
    assert!(comment.stdout.contains("2 cached)"), "{}", indent(&comment.all()));

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
    suite(&scratch, "a", 1);
    suite(&scratch, "b", 1);
    suite(&scratch, "c", 1);
    scratch.write("lib/c/lib.buri", "export fn answer(): I64 { missing() }\n");
    let run = scratch.run(&["test", "//...", "--explain"]);
    if run.stderr.contains("test-run-unavailable") {
        run.exits(1);
        return;
    }
    run.exits(1);
    assert_eq!(run.tests_passed(), 2, "{}", indent(&run.all()));
    assert!(run.stdout.contains("1 failed to compile"), "{}", indent(&run.all()));
    let shared = run.stdout.lines().any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() == 5 && f[1] == "link" && f[2] == "//lib/a,//lib/b"
    });
    assert!(shared, "the suites that compile did not share a binary:\n{}", indent(&run.all()));
}
