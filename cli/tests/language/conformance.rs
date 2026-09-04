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
use crate::harness::*;

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
        passed >= 1000,
        "expected the conformance suite to hold at least 1000 assertions, found {passed}:\n{}",
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
/// BURI_BLESS=1 cargo test -p buri --test language conformance::rejected_programs
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
        // A case that binds a UI effect declares `// PLATFORM: WEB`, so its
        // artifact lands under a different roof. It still crashes under the
        // JavaScript runtime: a page runs headlessly here, and an abort is an
        // abort wherever the document came from.
        cases.push((name, expect, output_dir_for(&text)));
    }

    // These must compile: a crash is a runtime claim, and a program that does
    // not build has not made it.
    scratch.run(&["build", "//..."]).ok();

    let mut failures = Vec::new();
    for (name, expect, out_dir) in &cases {
        let run = scratch.exec_js_in(out_dir, &format!("cmd/{name}"));
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
/// rendered by `Cents.format`, from `cli/tests/example/cmd/web/main.buri`.
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

/// The worked monorepo's page, built as the three files a WEB output is.
///
/// `//cmd/basket` is the example repository's application: a keyed list, a
/// form, both style tiers, a design-token vocabulary in one package themed by
/// another, and a request that answers through a callback. This is the test
/// that it is a *build artifact* and not only a program that checks — a page
/// with no stylesheet beside it, or with a stale one, is a page that does not
/// look like itself.
///
/// The three claims here are the three that no `.buri` suite can make, because
/// each is about what the build wrote rather than about what the program means.
#[test]
fn the_monorepo_page_builds_as_a_web_artifact() {
    let example = Scratch::copy_of("basket-web", &example_repo());

    example.run(&["build", "//cmd/basket", "--force"]).ok();

    // `--check-reproducible` builds twice, into two directories of its own, and
    // compares *every file each output wrote* by name. That is what covers a
    // companion artifact: a `.css` present in one round and not the other is
    // its own failure rather than a difference nobody looked for.
    example.run(&["build", "//cmd/basket", "--check-reproducible", "--force"]).ok();

    let module = example.artifact_in("web", "cmd/basket");
    let dir = module.parent().unwrap();
    let sheet = std::fs::read_to_string(dir.join("basket.css")).expect("the stylesheet is written");
    let shell = std::fs::read_to_string(dir.join("basket.html")).expect("the shell is written");

    // The shell is what makes "loadable in a browser" mean something. The link
    // carries the id the runtime's own installer looks for, so the rules are in
    // the page before the first paint and `mount` finds them there and does
    // nothing — no duplication and no flash of unstyled content.
    assert!(shell.contains(r#"<link id="buri-styles" rel="stylesheet" href="basket.css">"#));
    assert!(shell.contains(r#"<script type="module" src="./basket.mjs"></script>"#));

    // Two packages' tokens, each namespaced by the package that owns it, so a
    // library's `surface` and an app's could never collide.
    assert!(sheet.contains("var(--kit-surface)"), "the library's token is not in the sheet");
    assert!(sheet.contains("var(--basket-bg)"), "the app's token is not in the sheet");
    // Hover is a pseudo-class and a breakpoint is a media query: both exist
    // only in the static tier, and neither costs anything at run time.
    assert!(sheet.contains(":hover"), "hover did not reach the sheet");
    assert!(sheet.contains("@media (min-width:"), "a breakpoint did not reach the sheet");
    // And the computed tier is deliberately not there. The meter's width is
    // serialised onto its element; there is no class for "however wide it is".
    assert!(!sheet.contains("width: "), "an inline-tier value leaked into the sheet:\n{sheet}");

    // A page runs headlessly: there is no document, so the runtime supplies
    // one. It mounts, registers its listeners, and has nothing to say — the
    // budget in `main.buri` does not include `Stdout`, and could not print if
    // it wanted to.
    let run = example.exec_js_in("web", "cmd/basket");
    run.ok();
    assert_eq!("", run.stdout, "a mounted page has nothing to say on the way out");
    eprintln!("web: //cmd/basket built as .mjs + .css + .html, reproducibly");
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
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

fn countDown(n: Int, acc: Int): Int {
  if (n == 0) { acc } else { countDown(n - 1, acc + 1) }
}

fn pingA(n: Int): Bool {
  if (n == 0) { true } else { pingB(n - 1) }
}

fn pingB(n: Int): Bool {
  if (n == 0) { false } else { pingA(n - 1) }
}

fn everyBelow(n: Int): Bool {
  if (n == 0) { true } else { n > -1 && everyBelow(n - 1) }
}

fn anyBelow(n: Int): Bool {
  if (n == 0) { true } else { n < 0 || anyBelow(n - 1) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "self: ${countDown(10000000, 0)}").ignore();
  let _ = io.println(ctx, "mutual: ${pingA(10000001)}").ignore();
  // The right operand of a short-circuiting operator is a tail position too.
  // These two are the shapes of `all` and `any`, and both of them recursed on
  // the JavaScript stack until the backend learned to descend into `&&` and
  // `||`.
  let _ = io.println(ctx, "and: ${everyBelow(2000000)}").ignore();
  let _ = io.println(ctx, "or: ${anyBelow(2000000)}").ignore();
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
        assert_eq!(
            stdout,
            "self: 10000000\nmutual: false\nand: true\nor: true\n",
            "wrong answer on {engine}"
        );
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
// The worked monorepo
// ---------------------------------------------------------------------------

/// `cli/tests/example` is the build system's own corpus. It has to lint
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
// A test context names only what it needs
// ---------------------------------------------------------------------------

/// Neither corpus asks a caller for a capability its body never exercises.
///
/// The conformance corpus is not lint-clean and is not meant to be: it holds
/// unread fields, unconstructed variants and discarded results *on purpose*,
/// because those are what several of its cases are about. **`unused-context-bound`
/// is different**, and this is the one code held to zero over it.
///
/// The reason is the note's rule — a test context names only the effects the
/// function under test needs — and the chain that makes it a *corpus* property
/// rather than a signature one. A dead bound is a demand on every caller, so
/// `fn note<C: Alloc + Stdout>` forces `Alloc: alloc()` into the context of
/// every test that calls it, and that context is then wider than the test.
/// Removing the fifteen dead bounds this corpus carried is what let two
/// hundred and forty-seven of its contexts shrink; leaving one in would put
/// them back, one test at a time and invisibly.
///
/// `cli/tests/example` is held to the same line by
/// [`the_example_monorepo_is_clean`], which asks for `no findings` at all.
#[test]
fn no_conformance_context_asks_for_a_bound_it_does_not_use() {
    let corpus = Scratch::copy_of("conformance-bounds", &conformance_repo());
    // The corpus has findings, so the run exits 1; what is asserted is which
    // findings, not how many.
    corpus.run(&["lint", "//...", "--error-format=json"]).silent_about("unused-context-bound");
}

// ---------------------------------------------------------------------------
// Where a lint finding may point
// ---------------------------------------------------------------------------

/// A source a lint reads is not a source it may report, and the reason is the
/// **cache** rather than the caret.
///
/// `lint_cache.rs`'s `place` turns a recorded span's file name back into a
/// `FileId` for the run that reads the record, and answers `None` for a name
/// with no file behind it — an embedded standard library module, or one
/// generated from a schema. One such name makes the whole record unusable, so
/// a single finding pointing into `core/…` re-lints that target from scratch
/// for ever. `editable_modules_of` is what stops it, and this is the test that
/// holds the body-reading rules to it.
///
/// `unused-context-bound` is what makes the question live. `core/fs` declares
/// six functions whose `Alloc` bound the body never exercises — `readText` is
/// `ctx.readFile(path)`, which is an `Fs` method and nothing else — so a rule
/// that walked every body in the closure would report six findings inside the
/// standard library for a repository that merely reads a file. Measured with
/// the module filter lifted, it reports exactly those six.
///
/// The repository below reaches all six through one import, and the assertion
/// is in two halves so that neither can pass by the rule having gone silent:
/// nothing outside the repository is named, **and** the one dead bound the
/// repository itself wrote is reported.
#[test]
fn a_finding_never_names_a_file_the_author_cannot_edit() {
    let repo = Scratch::repo("lint-spans");
    repo.binary_package("cmd/app", FS_BOUNDS);
    let run = repo.run(&["lint", "//...", "--error-format=json"]);
    run.exits(1);

    let mut named: Vec<String> = Vec::new();
    let mut mine: Vec<String> = Vec::new();
    for line in run.all().lines() {
        let Some(file) = line.split("\"file\":\"").nth(1).and_then(|r| r.split('"').next()) else {
            continue;
        };
        named.push(file.to_string());
        if repo.path(file).exists() {
            mine.push(file.to_string());
        }
    }
    assert!(
        named.len() == mine.len(),
        "a finding names a file outside the repository: {named:?}\n{}",
        indent(&run.all())
    );
    assert!(
        run.all().contains("unused-context-bound"),
        "the repository's own dead bound went unreported, so this proves nothing:\n{}",
        indent(&run.all())
    );
}

/// `fs.readText<C: Alloc + Fs>` demands both of `read`'s bounds; `fs.exists<C:
/// Fs>` demands one of `touch`'s. So exactly one finding is this repository's,
/// and the six inside `core/fs` are nobody's.
const FS_BOUNDS: &str = r#"from "core/effect" import { Alloc, Fs };
from "core/fs" import * as fs;
from "core/host" import * as host;

fn read<C: Alloc + Fs>(ctx: C, path: Str): Bool {
  fs.readText(ctx, path).isOk()
}

fn touch<C: Alloc + Fs>(ctx: C, path: Str): Bool {
  fs.exists(ctx, path)
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Fs: host.fs };
  let _ = read(ctx, "a.txt");
  let _ = touch(ctx, "b.txt");
  .Ok(())
}
"#;

// ---------------------------------------------------------------------------
// The CLI contract
// ---------------------------------------------------------------------------

/// The rest of the exit-code contract is in `repositories/cli/exit_codes`, as a case.
/// This one cannot be: a repository case is a repository, and the thing being
/// checked here is what happens where there is not one.
#[test]
fn outside_a_repository_is_a_bad_invocation() {
    let nowhere = Scratch::empty("not-a-repo");
    let run = nowhere.run(&["build", "//..."]);
    run.exits(2).says("REPO.buri");
}

/// `buri version` is the one command with something to say outside a
/// repository, and CLI.md says so explicitly: it answers from the binary, and
/// there being no repository is not an error. It cannot be a repository case
/// for the same reason as the test above — a case *is* a repository. What it
/// prints *inside* one is `repositories/cli/version`.
///
/// `--verbose` is here rather than in that case because its second line is the
/// hash of whichever `buri` the suite just compiled, which no checked-in
/// golden can hold. It is the only way to learn which build of a version is
/// running, so a bug report can name one.
#[test]
fn version_works_outside_a_repository() {
    let nowhere = Scratch::empty("not-a-repo-version");
    let run = nowhere.run(&["version"]);
    run.ok().says("buri ");
    // A repository is what it has nothing to say about, so it must not claim
    // to have read one.
    run.silent_about("REPO.buri");

    let verbose = nowhere.run(&["version", "--verbose"]);
    verbose.ok().says("this executable: sha256 ");
    verbose.silent_about("unreadable");
}
