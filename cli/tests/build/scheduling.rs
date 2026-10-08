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
        let run = scratch.run_with_env(&["test", "//...", "--force"], &[("BURI_TEST_BATCH_BYTES", "1")]);
        run.ok();
        assert_eq!(run.tests_passed(), count, "round {round}:\n{}", indent(&run.all()));
    }
}

/// A suite named on its own spreads its tests over several processes, as a
/// suite named beside another one does.
///
/// The test binary is wrapped by a C driver that links the real one and puts a
/// script in its place. The script writes `start` and `end` around the real
/// binary, and holds back everything after its first line until a second
/// process has started. Run one process at a time, the first waits alone and
/// no two ever overlap.
#[cfg(unix)]
#[test]
fn a_suite_named_alone_runs_its_tests_side_by_side() {
    use std::os::unix::fs::PermissionsExt;
    let real_cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let scratch = Scratch::repo("alone-side-by-side");
    scratch.write("lib/a/BUILD.buri", "library {\n  test { sources: [\"test/a.buri\"] }\n}\n");
    scratch.write("lib/a/lib.buri", "export fn one(): Int { 1 }\n");
    let names: Vec<String> = (0..8).map(|i| format!("test {i}")).collect();
    let tests: String =
        names.iter().map(|name| format!("\ntest \"{name}\" {{\n  assert.equal(one(), 1);\n}}\n")).collect();
    scratch.write(
        "lib/a/test/a.buri",
        &format!("from \"//lib/a\" import {{ one }};\nfrom \"core/testing/assert\" import * as assert;\n{tests}"),
    );
    let log = scratch.path("processes");
    let real = scratch.path("real-binary");
    let wrapper = scratch.write(
        "wrapper.sh",
        &format!(
            "#!/bin/sh\n\
             echo \"start $$\" >> '{log}'\n\
             starts() {{ c=0; while read -r l; do case \"$l\" in start*) c=$((c+1)) ;; esac; done < '{log}'; echo $c; }}\n\
             '{real}' \"$@\" | {{\n  \
               IFS= read -r first; printf '%s\\n' \"$first\"\n  \
               n=0\n  \
               until [ \"$(starts)\" -ge 2 ]; do\n    \
                 n=$((n+1)); if [ \"$n\" -gt 400 ]; then break; fi\n    \
                 /bin/sleep 0.05\n  \
               done\n  \
               while IFS= read -r line; do printf '%s\\n' \"$line\"; done\n\
             }}\n\
             echo \"end $$\" >> '{log}'\n",
            log = log.display(),
            real = real.display(),
        ),
    );
    let driver = scratch.write(
        "fake-cc",
        &format!(
            "#!/bin/sh\n\
             case \"$1\" in -###) exit 1 ;; esac\n\
             '{real_cc}' \"$@\" || exit $?\n\
             prev=\"\"\n\
             for a in \"$@\"; do\n  \
               if [ \"$prev\" = \"-o\" ] && [ \"$a\" = \"artifact\" ]; then\n    \
                 cp artifact '{real}' && cp '{wrapper}' artifact && chmod +x artifact\n  \
               fi\n  \
               prev=\"$a\"\n\
             done\n",
            real = real.display(),
            wrapper = wrapper.display(),
        ),
    );
    for script in [&wrapper, &driver] {
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let run = scratch.run_with_env(
        &["test", "//lib/a", "--verbose", "--jobs=4"],
        &[("CC", &driver.display().to_string())],
    );
    if run.stderr.contains("native-run-not-available") {
        run.exits(1);
        return;
    }
    run.heap_ok().ok();
    // Listed in the order they're declared, each with its own time.
    let listed: Vec<String> = run
        .stdout
        .lines()
        .filter(|l| l.starts_with("  ok  "))
        .map(|l| {
            let blanked = blank_test_time(l);
            assert!(blanked.ends_with("<time>"), "an untimed test: {l:?}\n{}", indent(&run.stdout));
            blanked.trim_end_matches("<time>").split_whitespace().skip(2).collect::<Vec<_>>().join(" ")
        })
        .collect();
    assert_eq!(listed, names, "{}", indent(&run.stdout));
    // Some process started while another was still running.
    let events = std::fs::read_to_string(&log).unwrap_or_default();
    let mut running = 0;
    let mut overlapped = false;
    for event in events.lines() {
        if event.starts_with("start") {
            overlapped |= running > 0;
            running += 1;
        } else {
            running -= 1;
        }
    }
    assert!(overlapped, "no two test processes ran at once:\n{}\n{}", indent(&events), indent(&run.all()));
}

/// A suite named on its own reports exactly what it reported when its tests
/// ran one process at a time: the failures in block order, each with its
/// diff, the `--verbose` list, what `--filter` skips, and the status.
#[test]
fn a_suite_named_alone_reports_as_it_did_one_process_at_a_time() {
    let scratch = Scratch::repo("alone-report");
    scratch.write("lib/mixed/BUILD.buri", "library {\n  test { sources: [\"test/a.buri\", \"test/b.buri\"] }\n}\n");
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
        let lines: Vec<String> =
            run.stdout.lines().map(|l| if l.contains(" passed, ") { l.split(" (").next().unwrap_or(l).to_string() } else { blank_test_time(l) }).collect();
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
    assert_eq!(first, (1, format!("{failures}\n3 passed, 3 failed, 0 skipped"), String::new()));
    let verbose = report(&["test", "//lib/mixed", "--force", "--verbose"]);
    let list = "\
//lib/mixed  native  6 tests  <time>
  ok    a.buri  a passes                  <time>
  FAIL  a.buri  a compares wrong          <time>
  ok    a.buri  a passes after a failure  <time>
  FAIL  a.buri  a fails again             <time>
  ok    b.buri  b passes                  <time>
  FAIL  b.buri  b fails                   <time>
";
    assert_eq!(verbose, (1, format!("{list}\n{failures}\n3 passed, 3 failed, 0 skipped"), String::new()));
    // A filtered binary holds only the tests it runs, and each is timed as itself.
    let filtered = report(&["test", "//lib/mixed", "--force", "--verbose", "--filter=passes"]);
    let list = "\
//lib/mixed  native  3 tests  <time>
  ok    a.buri  a passes                  <time>
  ok    a.buri  a passes after a failure  <time>
  ok    b.buri  b passes                  <time>
  skip  a.buri  a compares wrong
  skip  a.buri  a fails again
  skip  b.buri  b fails
";
    assert_eq!(filtered, (0, format!("{list}\n3 passed, 0 failed, 3 skipped"), String::new()));
}
