//! The conformance suite: does the language do what the specification says?
//!
//! Everything here drives the real `buri` binary, because that is what a user
//! runs. Three shapes of test:
//!
//! * `cli/tests/conformance/` — a Buri repository whose `test/` directories
//!   assert on language semantics. Run with `buri test //...`.
//! * `cli/tests/reject/` — programs that must *not* compile, each paired with
//!   the diagnostics exactly as a terminal and `--error-format=json` print them.
//! * `WEB_STDOUT` — the exact stdout of the worked monorepo's JS binary, so a
//!   wrong *rendering* fails as loudly as a wrong value.
//!
//! The point of the first is that a wrong answer fails, rather than a program
//! that still exits 0.
//!
//! Diagnostics *about the graph* live next door in `repos.rs`, because a
//! single-package binary with no dependencies cannot express one.
//!
//! Every test here works on a copy under `CARGO_TARGET_TMPDIR`. Nothing writes
//! into a checked-in tree, so the suites hold no lock and run in parallel.

mod harness;
use harness::*;

use std::process::Command;

// ---------------------------------------------------------------------------
// The conformance repository
// ---------------------------------------------------------------------------

fn conformance_repo() -> std::path::PathBuf {
    tests_dir().join("conformance")
}

#[test]
fn conformance_suite_passes() {
    // A copy, so the suite cannot be disturbed by anything else running, and
    // starts from an empty cache rather than from whatever was left behind.
    let suite = Scratch::copy_of("conformance", &conformance_repo());
    let run = suite.run(&["test", "//...", "--force"]);
    run.ok();

    // A suite that compiled to nothing would "pass" with zero assertions, so
    // the count is checked too.
    let passed = run.tests_passed();
    assert!(
        passed >= 150,
        "expected the conformance suite to hold at least 150 assertions, found {passed}:\n{}",
        indent(&run.all())
    );
    eprintln!("conformance: {passed} tests passed");
}

/// The suite has to be able to fail. A test that cannot fail proves nothing,
/// so this breaks one on purpose and checks the runner notices.
#[test]
fn conformance_suite_can_fail() {
    let suite = Scratch::copy_of("canary", &conformance_repo());
    let canary = "lib/canary/test/canary.buri";
    assert!(
        suite.read(canary).contains("// CANARY_42"),
        "the canary suite must contain the marker it is edited through"
    );
    // The value, not a name: renaming a constant and its use together would
    // leave the assertion true. `edit` panics if the text is not there, so a
    // substitution that silently did nothing cannot pass for a passing test.
    suite.edit(canary, "assert.eq(6 * 7, 42);", "assert.eq(6 * 7, 43);");

    let run = suite.run(&["test", "//lib/canary", "--force"]);
    assert_ne!(run.code, 0, "a broken assertion still passed:\n{}", indent(&run.all()));
    run.says("FAIL");
}

// ---------------------------------------------------------------------------
// Programs that must not compile
// ---------------------------------------------------------------------------

/// Each case in `tests/reject/` is a directory holding a whole program that
/// must fail to compile, together with what the toolchain says about it:
///
/// ```text
/// cli/tests/reject/non_exhaustive_match/
///   main.buri       the program
///   expected.txt    the diagnostics, exactly as a terminal shows them
///   expected.json   the same, as `--error-format=json` emits them
/// ```
///
/// The `// EXPECT:` line in `main.buri` states the intent in one phrase. The
/// two recorded files pin the rest: the span, the carets, the notes, the order
/// of several diagnostics, and every word of the prose. A reworded message is a
/// change to what a user reads, so it should show up as a diff and be looked at
/// rather than pass silently because a substring survived.
///
/// The JSON file is also where the four-part contract is enforced. Every
/// diagnostic has to carry a `fix`, because a diagnostic that cannot say what
/// to do about it is not finished.
///
/// Regenerate both after a deliberate change:
///
/// ```text
/// BURI_BLESS=1 cargo test -p buri --test conformance rejected_programs
/// ```
#[test]
fn rejected_programs_are_rejected() {
    let dir = tests_dir().join("reject");
    let cases = case_dirs(&dir, "main.buri", 25);

    // One scratch repository, one package per case. The package is named after
    // the directory, because the recorded diagnostics name `cmd/<case>/main.buri`.
    let scratch = Scratch::repo("reject-corpus");
    let mut g = Golden::new();

    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(case.join("main.buri")).unwrap();
        let expect = require_annotation(&text, "// EXPECT:", &name);

        scratch.binary_package(&format!("cmd/{name}"), &text);
        let target = format!("//cmd/{name}");
        let run = scratch.run(&["build", &target]);
        let printed = run.all();
        if run.code == 0 {
            g.fail(format!("{name}: compiled, but should not have"));
            continue;
        }
        if !printed.contains(&expect) {
            g.fail(format!(
                "{name}: expected a diagnostic containing {expect:?}, got:\n{}",
                indent(&printed)
            ));
            continue;
        }
        let json = scratch
            .run(&["build", &target, "--error-format=json", "--force"])
            .all();

        // Every diagnostic answers "what do I do about it?".
        for (i, line) in json.lines().enumerate() {
            if line.starts_with('{') && !line.contains("\"fix\":") {
                g.fail(format!(
                    "{name}: diagnostic {} carries no `fix`:\n{}",
                    i + 1,
                    indent(line)
                ));
            }
        }

        for (file, content) in [("expected.txt", &printed), ("expected.json", &json)] {
            g.check(&case.join(file), &format!("{name}/{file}"), content);
        }
    }
    g.finish("reject", cases.len());
}

// ---------------------------------------------------------------------------
// Programs that must crash
// ---------------------------------------------------------------------------

/// A crash cannot be observed from inside a test — there is no catch — so the
/// things that are specified to crash get their own corpus. Each file is a
/// program that must compile, run, exit non-zero, and say why.
#[test]
fn crashing_programs_crash() {
    let dir = tests_dir().join("crash");
    let files = case_files(&dir, "buri", 8);

    let scratch = Scratch::repo("crash-corpus");
    let mut cases = Vec::new();
    for path in &files {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(path).unwrap();
        let expect = require_annotation(&text, "// CRASH:", &name);
        scratch.binary_package(&format!("cmd/{name}"), &text);
        cases.push((name, expect));
    }

    // These must compile: a crash is a runtime claim, and a program that does
    // not build has not made it.
    scratch.run(&["build", "//..."]).ok();

    let mut failures = Vec::new();
    for (name, expect) in &cases {
        let run = scratch.exec_js(&format!("cmd/{name}"));
        if run.code == 0 {
            failures.push(format!("{name}: exited 0, but should have crashed"));
        } else if !run.stderr.contains(expect.as_str()) {
            failures.push(format!(
                "{name}: expected a crash mentioning {expect:?}, got:\n{}",
                indent(&run.stderr)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} crash(es) wrong:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    eprintln!("crash: {} programs crashed as specified", cases.len());
}

// ---------------------------------------------------------------------------
// Golden output
// ---------------------------------------------------------------------------

/// What `//cmd/web` prints, to the byte. Two entries summed by `total` and
/// rendered by `Cents.format`, from `build-system/example/cmd/web/main.buri`.
/// Short enough to read here, so a change to it is a change in the diff rather
/// than in a file nobody opens.
const WEB_STDOUT: &str = "basket total: $36.50\n";

/// The worked monorepo's JS binary, built by the real build system and run,
/// with its stdout compared against the transcript above. This is what catches
/// a backend that produces a *different* answer rather than no answer: every
/// other suite here asserts from inside a program, and a wrong rendering on the
/// way out would not show up in any of them.
#[test]
fn monorepo_binaries_produce_their_golden_output() {
    let example = Scratch::copy_of("web-golden", &example_repo());
    example.run(&["build", "//cmd/web", "--force"]).ok();

    let run = example.exec_js("cmd/web");
    run.ok();
    assert_eq!(
        WEB_STDOUT, run.stdout,
        "//cmd/web printed something other than its transcript"
    );
    eprintln!("golden: //cmd/web matched its transcript");
}

/// The same artifact has to behave identically on an engine without native
/// tail calls, which is the whole reason the compiler eliminates them itself.
#[test]
fn tail_calls_run_in_constant_stack_on_v8() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("node is not installed; skipping the V8 tail-call check");
        return;
    }
    let scratch = Scratch::repo("tco-check");
    // Ten million bounces: far past any engine's stack, through a self call,
    // a mutually recursive pair, and an accumulator.
    scratch.binary_package(
        "cmd/deep",
        r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn countDown(n: Int, acc: Int): Int {
  if (n == 0) { acc } else { countDown(n - 1, acc + 1) }
}

fn pingA(n: Int): Bool {
  if (n == 0) { true } else { pingB(n - 1) }
}

fn pingB(n: Int): Bool {
  if (n == 0) { false } else { pingA(n - 1) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("self: ${countDown(10000000, 0)}");
  let _ = ctx.println("mutual: ${pingA(10000001)}");
  .Ok(())
}
"#,
    );

    scratch.run(&["build", "//cmd/deep", "--release"]).ok();

    let artifact = scratch.artifact("cmd/deep");
    for engine in ["node", "bun"] {
        let Ok(out) = Command::new(engine).arg(&artifact).output() else { continue };
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success(),
            "ten million tail calls overflowed on {engine}:\n{stderr}"
        );
        assert_eq!(stdout, "self: 10000000\nmutual: false\n", "wrong answer on {engine}");
    }
}

// ---------------------------------------------------------------------------
// The minifier does not change the answer
// ---------------------------------------------------------------------------

/// `--release` mangles every identifier, folds constants, drops unreachable
/// declarations, and tree-shakes the runtime. None of that may change what a
/// program computes or prints.
///
/// The conformance suite is the strongest thing to point at: a thousand
/// assertions on language semantics, every one of which has to hold after the
/// minifier has been through it. The monorepo's binary covers the other half —
/// what a whole program *prints* — and pins that the release artifact is
/// actually smaller, so "identical behaviour" cannot be bought by doing nothing.
#[test]
fn release_and_debug_agree() {
    let mut counts = Vec::new();
    for mode in ["--debug", "--release"] {
        let suite = Scratch::copy_of("agree-suite", &conformance_repo());
        let run = suite.run(&["test", "//...", mode, "--force"]);
        run.ok();
        let passed = run.tests_passed();
        assert!(passed > 100, "expected the whole suite {mode}, ran {passed}");
        counts.push(passed);
    }
    assert_eq!(counts[0], counts[1], "the two modes ran different numbers of assertions");

    // And a whole program's stdout, which no assertion inside a program covers.
    let example = Scratch::copy_of("agree-example", &example_repo());
    let mut outputs = Vec::new();
    let mut sizes = Vec::new();
    for mode in ["--debug", "--release"] {
        example.run(&["build", "//cmd/web", mode, "--force"]).ok();
        sizes.push(std::fs::metadata(example.artifact("cmd/web")).map(|m| m.len()).unwrap_or(0));
        outputs.push(example.exec_js("cmd/web").stdout);
    }
    assert_eq!(outputs[0], outputs[1], "minification changed what //cmd/web prints");
    assert!(
        sizes[1] < sizes[0],
        "the release artifact is not smaller than the debug one ({} vs {} bytes)",
        sizes[1],
        sizes[0]
    );
    eprintln!(
        "minify: {} assertions hold both ways; //cmd/web {} -> {} bytes",
        counts[0], sizes[0], sizes[1]
    );
}

// ---------------------------------------------------------------------------
// Reproducibility
// ---------------------------------------------------------------------------

/// Two builds of the same sources in the same configuration produce
/// byte-identical artifacts. Symbol names are derived from labels and module
/// paths rather than from compilation order, and iteration is by sorted key.
#[test]
fn builds_are_reproducible() {
    // The worked monorepo, copied to two different paths and built in each. A
    // real dependency graph rather than a toy, so an ordering that came from a
    // hash map would show up as a difference — and two directories, so would a
    // path that leaked into the output.
    let mut artifacts = Vec::new();
    for _ in 0..2 {
        let scratch = Scratch::copy_of("repro", &example_repo());
        scratch.run(&["build", "//cmd/web", "--release"]).ok();
        artifacts.push(std::fs::read(scratch.artifact("cmd/web")).unwrap());
    }
    assert!(
        artifacts[0] == artifacts[1],
        "two builds of the same source produced different artifacts ({} vs {} bytes)",
        artifacts[0].len(),
        artifacts[1].len()
    );
    eprintln!("reproducible: identical artifacts from two directories");
}

// ---------------------------------------------------------------------------
// The worked monorepo
// ---------------------------------------------------------------------------

/// `build-system/example` is the build system's own corpus. It has to lint
/// clean and its suites have to pass, through the real CLI.
///
/// What the policy checks *print* when they fire lives in `repos.rs`; this is
/// the other half — that on a repository which obeys them, they are silent.
#[test]
fn the_example_monorepo_is_clean() {
    let example = Scratch::copy_of("example-clean", &example_repo());

    example.run(&["lint", "//..."]).ok().says("no findings");

    let test = example.run(&["test", "//...", "--force"]);
    test.ok();
    let passed = test.tests_passed();
    assert!(passed >= 15, "expected the example suites to hold real tests, found {passed}");
    eprintln!("monorepo: {passed} tests, lint clean");
}

// ---------------------------------------------------------------------------
// The CLI contract
// ---------------------------------------------------------------------------

/// The rest of the exit-code contract is in `repos/cli/exit_codes`, as a case.
/// This one cannot be: a repository case is a repository, and the thing being
/// checked here is what happens where there is not one.
#[test]
fn outside_a_repository_is_a_bad_invocation() {
    let nowhere = Scratch::empty("not-a-repo");
    let run = nowhere.run(&["build", "//..."]);
    run.exits(2).says("REPO.buri");
}
