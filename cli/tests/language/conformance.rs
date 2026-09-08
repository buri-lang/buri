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
    suite.edit(canary, "assert.equal(6 * 7, 42);", "assert.equal(6 * 7, 43);");

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
from "core/effect" import { Allocator, Stdout };
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
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
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
/// `fn note<C: Allocator + Stdout>` forces `Allocator: alloc()` into the context of
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
// The grammar a generator writes
// ---------------------------------------------------------------------------

/// **Every node kind `core/buri/ast` exports has a test that prints it.**
///
/// The printer is what a generator's whole output goes through, and the one
/// way it can be wrong that no assertion catches is a construct nobody ever
/// asked it for: a variant added to `ExprKind` with no case beside it prints
/// whatever its author guessed, for ever, and the first person to meet it is
/// somebody whose generated repository does not compile.
///
/// So this is the corpus rule that makes `lib/buri_ast/` a *complete* set
/// rather than a sample. It reads the module's own `export enum`s and asks the
/// conformance package to construct each variant. A variant two enums share by
/// name — `Named` is a `TypeKind` and a `Rest` — is covered by whichever of
/// the two is written, which is the one place this is a lower bound rather
/// than an exact one.
///
/// Adding a variant is therefore adding a test, and the failure says which.
#[test]
fn every_ast_node_kind_the_printer_can_meet_has_a_test() {
    let source =
        repo_root().join("cli/src/compiler/standard_library/sources/buri_ast.buri");
    let text = std::fs::read_to_string(&source).expect("core/buri/ast");
    let corpus = read_corpus(&tests_dir().join("conformance/lib/buri_ast"));

    let mut missing: Vec<String> = Vec::new();
    let mut seen = 0usize;
    let mut inside: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("export enum ") {
            inside = rest.split_whitespace().next().map(str::to_string);
            continue;
        }
        let Some(name) = inside.clone() else { continue };
        if line == "}" {
            inside = None;
            continue;
        }
        let Some(variant) = line.strip_prefix("    ") else { continue };
        let variant: String =
            variant.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if variant.is_empty() {
            continue;
        }
        seen += 1;
        if !corpus.contains(&format!(".{variant}")) {
            missing.push(format!("{name}::{variant}"));
        }
    }

    assert!(seen > 50, "only {seen} variants were read out of {}", source.display());
    assert!(
        missing.is_empty(),
        "cli/tests/conformance/lib/buri_ast/ never constructs {} of the {seen} node kinds \
         `core/buri/ast` exports, so nothing says what the printer writes for them: {}",
        missing.len(),
        missing.join(", ")
    );
}

/// Every `.buri` under a directory, concatenated.
fn read_corpus(dir: &std::path::Path) -> String {
    let mut out = String::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "buri") {
                out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    out
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
/// several functions whose `Allocator` bound the body never exercises — `readText`
/// is `ctx.readFile(path)`, which is an `FileSystemRead` method and nothing else — so a
/// rule that walked every body in the closure would report findings inside the
/// standard library for a repository that merely reads a file.
///
/// The repository below reaches them through one import, and the assertion
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

/// `fs.readText<C: Allocator + FileSystemRead>` demands both of `read`'s bounds;
/// `fs.exists<C: FileSystemRead>` demands one of `touch`'s. So exactly one finding is
/// this repository's, and the ones inside `core/fs` are nobody's.
///
/// Both take a `Path` rather than the text of one, which is what keeps
/// `touch`'s `Allocator` dead: making a `Path` allocates, so a version of this
/// that called `filepath.of` inside `touch` would be a repository with no dead
/// bound to report.
const FS_BOUNDS: &str = r#"from "core/effect" import { Allocator };
from "core/fs" import { FileSystemRead, Path };
from "core/fs" import * as fs;
from "core/host" import * as host;
from "core/path" import * as filepath;

fn read<C: Allocator + FileSystemRead>(ctx: C, at: Path): Bool {
  fs.readText(ctx, at).isOk()
}

fn touch<C: Allocator + FileSystemRead>(ctx: C, at: Path): Bool {
  fs.exists(ctx, at)
}

export fn main(): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, FileSystemRead: host.fs };
  let _ = read(ctx, filepath.of(ctx, "a.txt"));
  let _ = touch(ctx, filepath.of(ctx, "b.txt"));
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

/// A worker artifact, driven by the platform's own `Request` and `Response`.
///
/// **The highest tier a worker can reach.** A `CLOUDFLARE_WORKER` output is
/// called by its runtime rather than started, so no `buri` command runs one and
/// no repository fixture can. What a driver module can do is exactly what the
/// platform does: import the artifact's default export, hand `fetch` a real
/// `Request`, and read a real `Response` back. Every JavaScript engine this
/// suite runs on has both globals.
///
/// Every field of the crossing, in one run: each of the seven methods HTTP has,
/// the path routed on, the query string beside it, a header read by name, a
/// header written on the way out, a body echoed, a body that is not text, and
/// the two answers that are not `200`. Each is a field of the bridge, and a
/// bridge that lost one would still answer the rest.
#[test]
fn a_worker_answers_the_platforms_request_with_the_platforms_response() {
    let scratch = Scratch::repo("worker-fetch");
    scratch.write(
        "cmd/site/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: CLOUDFLARE_WORKER, entry: \"fetch\" },\n    ]\n}\n",
    );
    scratch.write(
        "cmd/site/main.buri",
        r#"
from "core/effect" import { Allocator, Method, Request, Response };
from "core/host" import * as host;
from "core/net/http" import * as http;
from "core/str" import * as str;

export fn fetch(request: Request): Response {
  let ctx = context { Allocator: host.alloc };
  match (request.path()) {
    // What was sent, read back off the request the platform handed over.
    "/echo" => http.text(ctx, bodyOrExcuse(ctx, request)),
    "/query" => http.text(ctx, request.query()),
    "/header" => http.text(ctx, request.header("x-asked").withDefault("nothing")),
    // A header the worker writes, which the platform has to carry back.
    "/tagged" => http.text(ctx, "tagged").withHeader(ctx, "x-answered", "yes"),
    // The two answers that are not 200. A worker says so with a status, and
    // there is no exit code anywhere in it.
    "/missing" => http.status(404),
    "/broken" => http.status(500),
    other => http.text(ctx, str.format(ctx, "${verb(request.method)} ${other}")),
  }
}

/// Every method HTTP has, so a method the bridge drops is a line that changed.
fn verb(method: Method): Str {
  match (method) {
    .Get => "GET",
    .Head => "HEAD",
    .Post => "POST",
    .Put => "PUT",
    .Patch => "PATCH",
    .Delete => "DELETE",
    .Options => "OPTIONS",
  }
}

fn bodyOrExcuse<C: Allocator>(ctx: C, request: Request): Str {
  match (http.bodyText(ctx, request.body)) {
    .Ok(text) => text,
    .Err(_e) => "not utf-8",
  }
}
"#,
    );
    scratch.run(&["build", "//cmd/site"]).ok();

    // The driver is the platform's half, written the way a worker runtime calls
    // one. It prints a line per exchange, so a wrong answer names which.
    let driver = scratch.write(
        "drive.mjs",
        r#"
import worker from "./.buri/out/cloudflare-worker/cmd/site/fetch.mjs";

const say = async (request) => {
  const answer = await worker.fetch(request);
  const tag = answer.headers.get("x-answered");
  const said = `${answer.status} ${answer.headers.get("content-type")} ${await answer.text()}`;
  console.log(tag === null ? said : `${said} x-answered=${tag}`);
};

await say(new Request("https://example.com/about?ref=x#top"));

// Every method, on a path that answers with the one it was called by. A `GET`
// and a `HEAD` may carry no body, so none of these do.
for (const method of ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
  await say(new Request("https://example.com/", { method }));
}

await say(new Request("https://example.com/echo", { method: "POST", body: "hello" }));
// A body that is not text at all: the bridge carries octets, so what comes back
// is the entry's own excuse rather than a crash.
await say(
  new Request("https://example.com/echo", { method: "POST", body: new Uint8Array([0xff, 0xfe]) }),
);
await say(new Request("https://example.com/query?ref=x&page=2#top"));
await say(new Request("https://example.com/header", { headers: { "X-Asked": "please" } }));
await say(new Request("https://example.com/header"));
await say(new Request("https://example.com/tagged"));
await say(new Request("https://example.com/missing"));
await say(new Request("https://example.com/broken"));
"#,
    );

    let out = Command::new(js_runtime())
        .arg(&driver)
        .output()
        .expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the worker refused the platform's request:\n{stderr}");
    assert_eq!(
        stdout,
        "200 text/plain; charset=utf-8 GET /about\n\
         200 text/plain; charset=utf-8 GET /\n\
         200 text/plain; charset=utf-8 HEAD /\n\
         200 text/plain; charset=utf-8 POST /\n\
         200 text/plain; charset=utf-8 PUT /\n\
         200 text/plain; charset=utf-8 PATCH /\n\
         200 text/plain; charset=utf-8 DELETE /\n\
         200 text/plain; charset=utf-8 OPTIONS /\n\
         200 text/plain; charset=utf-8 hello\n\
         200 text/plain; charset=utf-8 not utf-8\n\
         200 text/plain; charset=utf-8 ref=x&page=2\n\
         200 text/plain; charset=utf-8 please\n\
         200 text/plain; charset=utf-8 nothing\n\
         200 text/plain; charset=utf-8 tagged x-answered=yes\n\
         404 null \n\
         500 null \n",
        "the crossing lost a field:\n{stderr}"
    );
}

/// How long anything in the bounded-fetch case below may take before the claim
/// it makes — that the *program's own* bound is what ends a request — is the
/// thing that failed. Fifty times the bound the program asks for, so a loaded
/// machine is not a failing one.
const PATIENTLY: std::time::Duration = std::time::Duration::from_secs(15);

/// One connection, or nothing by `PATIENTLY`. `TcpListener::accept` has no
/// deadline of its own, so this polls a non-blocking listener rather than
/// leaving a thread in `accept` for a client that never dials.
fn dialled_within(listener: &std::net::TcpListener) -> Option<std::net::TcpStream> {
    let until = std::time::Instant::now() + PATIENTLY;
    loop {
        match listener.accept() {
            Ok((socket, _from)) => {
                let _blocking = socket.set_nonblocking(false);
                let _read = socket.set_read_timeout(Some(PATIENTLY));
                let _written = socket.set_write_timeout(Some(PATIENTLY));
                return Some(socket);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= until {
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(_stopped) => return None,
        }
    }
}

/// A whole request head, or as much of one as arrived.
fn request_head(socket: &mut std::net::TcpStream) -> Vec<u8> {
    use std::io::Read;
    let mut head: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 512];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
        }
    }
    head
}

/// **`Request.withTimeout` is honoured by the JavaScript host call**, against a
/// server that accepts and then says nothing.
///
/// `fetch` is the one host call whose asynchrony the language has a word for,
/// and this bound is the only thing a program can put on it.
/// `cli/runtime/http.rs` holds the same pair for the native client, where a Buri
/// program cannot reach `fetch` at all (`native/shared.rs` says why); this is
/// the JavaScript half, as a whole program over a real socket.
///
/// Both halves in one run. The first request meets a listener that accepts,
/// reads the whole request head and then holds the connection open, so the only
/// thing that can end it is the program's own bound — and the program prints
/// `.Timeout` rather than a transport failure, because those are two different
/// things to a caller deciding whether to retry. The second request meets the
/// same listener answering at once under a bound thirty times larger, so a
/// bound that fired on everything would fail here too.
///
/// Nothing sleeps to make this happen: the stall is a peer that does not write
/// and the answer is a peer that writes at once. Every wait has a deadline, the
/// child is killed if it outlives one, and the peer thread is joined.
#[test]
fn a_request_with_a_bound_gives_up_on_a_server_that_never_answers() {
    use std::io::Write;

    // Bound before the source is written, so the port is one this machine
    // really handed out rather than a number the test picked and hoped for.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a bound port");
    let port = listener.local_addr().expect("the bound port").port();
    listener.set_nonblocking(true).expect("a listener that can be polled");

    let scratch = Scratch::repo("fetch-timeout");
    scratch.write("cmd/dial/BUILD.buri", "binary {\n    outputs: [{ platform: JS }]\n}\n");
    scratch.write(
        "cmd/dial/main.buri",
        &format!(
            r#"
from "core/effect" import {{ Allocator, Network, NetError, Response, Stdout }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/str" import * as str;

export fn main(): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc, Network: host.net, Stdout: host.stdout }};
  // A peer that accepts and never answers, under a bound of a third of a second.
  let stalling = http.request(.Get, "http://127.0.0.1:{port}/stall").withTimeout(300);
  let _ = io.println(ctx, said(ctx, http.send(ctx, stalling))).ignore();
  // The same peer, answering at once, under a bound it cannot reach.
  let answering = http.request(.Get, "http://127.0.0.1:{port}/answer").withTimeout(10000);
  let _ = io.println(ctx, said(ctx, http.send(ctx, answering))).ignore();
  .Ok(())
}}

fn said<C: Allocator>(ctx: C, answer: Result<Response, NetError>): Str {{
  match (answer) {{
    .Ok(reply) => str.format(ctx, "answered ${{reply.status}}"),
    .Err(e) => str.format(ctx, "gave up: ${{http.errorText(e)}}"),
  }}
}}
"#
        ),
    );
    scratch.run(&["build", "//cmd/dial"]).ok();

    // The peer. It holds the first connection for as long as it lives, which is
    // until it has answered the second one.
    let serving = std::thread::spawn(move || {
        let Some(mut stalled) = dialled_within(&listener) else { return false };
        let _asked = request_head(&mut stalled);
        let Some(mut answering) = dialled_within(&listener) else { return false };
        let _asked_again = request_head(&mut answering);
        let _sent = answering.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi",
        );
        let _flushed = answering.flush();
        true
    });

    // Streams to files rather than pipes, because this child is polled rather
    // than waited on and a full pipe would be a deadlock of this test's own.
    let out_path = scratch.path(".buri/dial.stdout");
    let err_path = scratch.path(".buri/dial.stderr");
    std::fs::create_dir_all(out_path.parent().expect("a parent")).expect("a directory");
    let mut child = Command::new(js_runtime())
        .arg(scratch.artifact("cmd/dial"))
        .stdout(std::fs::File::create(&out_path).expect("a file"))
        .stderr(std::fs::File::create(&err_path).expect("a file"))
        .spawn()
        .expect("the javascript runtime runs");
    let until = std::time::Instant::now() + PATIENTLY;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() >= until => {
                let _killed = child.kill();
                let _reaped = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(_broken) => break None,
        }
    };
    let both_dialled = serving.join().expect("the peer thread");
    let said = std::fs::read_to_string(&out_path).unwrap_or_default();
    let complained = std::fs::read_to_string(&err_path).unwrap_or_default();

    assert!(
        ended.is_some(),
        "the program outlived its own bound and had to be killed.\nit said:\n{said}\n{complained}"
    );
    assert!(both_dialled, "the peer was not dialled twice.\nthe program said:\n{said}");
    assert_eq!(
        said, "gave up: the request timed out\nanswered 200\n",
        "the bound was not the thing that ended the first request.\nstderr:\n{complained}"
    );
}

/// **When a `core/lazy` chunk is fetched**, asked of a running artifact.
///
/// `build::repositories`' `lazy_chunks` case reads the split off the two files
/// a build wrote. What it cannot see is the half the feature exists for: that
/// the chunk is not fetched by a request that never reaches the `load`. This is
/// the driver-module pattern
/// [`a_worker_answers_the_platforms_request_with_the_platforms_response`] uses,
/// for the same reason — a worker is called per request, so one process can
/// take two paths through one artifact.
///
/// **The chunk file is deleted between the two runs**, which is the only
/// evidence that cannot be faked by a program that merely did not print. A run
/// with no chunk on disk answers `/` and fails `/heavy`; with the chunk there,
/// both answer. An artifact that fetched its chunks at start-up would fail the
/// first run's `/` as well, and one that fetched nothing would pass `/heavy`
/// without the file.
///
/// A chunk that is on disk and is *not a module* is the same answer from the
/// other side: the request that needs it fails and the request that does not is
/// untouched, so a half-written file cannot take the whole artifact down with
/// it.
#[test]
fn a_chunk_is_fetched_only_where_the_program_asks_for_it() {
    let scratch = Scratch::repo("lazy-when-fetched");
    scratch.write(
        "cmd/site/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: CLOUDFLARE_WORKER, entry: \"fetch\" },\n    ]\n}\n",
    );
    scratch.write(
        "cmd/site/main.buri",
        r#"
from "core/effect" import { Allocator, Request, Response };
from "core/host" import * as host;
from "core/lazy" import * as lazy;
from "core/net/http" import * as http;
from "core/str" import * as str;

fn onlyTheChunkReachesThis<C: Allocator>(ctx: C, path: Str): Str {
  str.format(ctx, "heavy ${path}")
}

fn heavy<C: Allocator>(ctx: C, path: Str): Str {
  onlyTheChunkReachesThis(ctx, path)
}

export fn fetch(request: Request): Response {
  let ctx = context { Allocator: host.alloc };
  match (request.path()) {
    "/heavy" => http.text(ctx, lazy.load(heavy)(ctx, request.path())),
    other => http.text(ctx, other),
  }
}
"#,
    );
    scratch.run(&["build", "//cmd/site"]).ok();

    let artifact = scratch.path(".buri/out/cloudflare-worker/cmd/site/fetch.mjs");
    let chunk = artifact.with_file_name("fetch.0.mjs");
    let held = std::fs::read(&chunk).expect("the build wrote a chunk beside the module");

    let driver = scratch.write(
        "drive.mjs",
        r#"
import worker from "./.buri/out/cloudflare-worker/cmd/site/fetch.mjs";

const say = async (path) => {
  try {
    const answer = await worker.fetch(new Request("https://example.com" + path));
    console.log(await answer.text());
  } catch (e) {
    console.log("no chunk");
  }
};

await say("/home");
await say("/heavy");
"#,
    );

    let run = || {
        let out = Command::new(js_runtime())
            .arg(&driver)
            .output()
            .expect("the javascript runtime runs");
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    std::fs::remove_file(&chunk).expect("the chunk is removable");
    assert_eq!(
        run(),
        "/home\nno chunk\n",
        "a request that never reaches the `load` needs no chunk, and one that does needs it"
    );

    // A file that is there and is not a module. Half a download and a truncated
    // deploy both look like this, and neither may cost the requests that never
    // reach the `load`.
    std::fs::write(&chunk, b"export const $bind = (").expect("the chunk is writable");
    assert_eq!(
        run(),
        "/home\nno chunk\n",
        "a chunk that will not load must cost only the request that needed it"
    );

    std::fs::write(&chunk, &held).expect("the chunk goes back");
    assert_eq!(run(), "/home\nheavy /heavy\n", "and with the chunk there, both answer");
}

/// A page that mounts an interface and then dials a socket, driven by the
/// browser's own half.
///
/// **The claim the proposal makes about WEB, and the only tier that can check
/// it.** `connect` follows `ui.mount`: it suspends without holding the event
/// loop, so a page that mounted an interface and then dialled goes on running
/// while the socket is idle. Nothing about that is visible in a table of
/// generated code, and no `buri` command runs a page — so this is the platform's
/// own side of it, a `WebSocket` double whose events arrive on timers.
///
/// **The double's log is the evidence, and it is ordered by construction.** The
/// dial schedules everything that follows it, each on its own delay — a tick at
/// 10ms, the open at 20, a tick at 30, a message at 40, a tick at 50, the close
/// at 60 — so the order is the engine's timer queue rather than a race with
/// however long this host took to load the artifact. What the log then shows is
/// the page's own `send` sitting *between two ticks*: the hook ran and the event
/// loop had its other work back before the next one. A `connect` that held the
/// loop could not produce that log at all; it could not even reach the open,
/// because the timer that delivers it would never run.
///
/// The URL is the second half of the row. `wss://` reaches the constructor
/// unchanged, so the scheme a program wrote is the scheme the platform dialled.
#[test]
fn a_page_mounts_an_interface_and_then_dials_a_socket() {
    let scratch = Scratch::repo("page-dials");
    scratch.write(
        "cmd/page/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: WEB, entry: \"main\" },\n    ]\n}\n",
    );
    scratch.write(
        "cmd/page/main.buri",
        r#"
from "core/effect" import { Allocator, Sockets, Stdout, WebSocketClient };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "ui/effect" import { Ui };
from "ui/node" import * as ui;

export fn main(): Result<(), Str> {
  let ctx = context {
    Allocator: host.alloc,
    Sockets: host.sockets,
    Stdout: host.stdout,
    Ui: host.ui,
    WebSocketClient: host.websocketClient,
  };
  match (ui.mount(ctx, ui.region(.Main, [], [ui.heading(1, .Const("live"))]), [])) {
    .Err(why) => .Err(why),
    .Ok(_mounted) => {
      let _said = io.println(ctx, "mounted").ignore();
      match (websocket.connect(ctx, feed())) {
        .Err(e) => .Err(e.detail),
        .Ok(reason) => {
          let _ended = io.println(ctx, "page ended ${reason}").ignore();
          .Ok(())
        },
      }
    },
  }
}

fn feed<C: Allocator + Sockets + Stdout + WebSocketClient>(): Client<C, Int> {
  Client {
    url: "wss://example.test/feed",
    onOpen: fn(c, socket, response) => {
      // **What a page knows about its own handshake, which is two fields.**
      // The browser's `WebSocket` never shows the response, so `Connected`
      // carries the negotiated subprotocol and extensions and nothing else —
      // where a native program gets the head the server sent. This is the one
      // of the two a program actually reads.
      let spoke = response.header("sec-websocket-protocol").withDefault("none");
      let _said = io.println(c, "page opened ${response.status} ${spoke}").ignore();
      let _sent = socket.send(c, .Text("subscribe"));
      0
    },
    onMessage: fn(c, _socket, seen, message) => {
      let _said = match (message) {
        .Text(text) => io.println(c, "page heard text ${text.length()} ${text}").ignore(),
        .Binary(data) => io.println(c, "page heard binary ${data.length()}").ignore(),
      };
      seen + 1
    },
    onClose: fn(c, _socket, seen, reason) => {
      io.println(c, "page closed after ${seen} ${reason}").ignore()
    },
  }
}
"#,
    );
    scratch.run(&["build", "//cmd/page"]).ok();

    let driver = scratch.write(
        "drive.mjs",
        r#"
// The browser's `WebSocket`, as much of it as a page uses, with a log of its
// own. Every event arrives on a timer, so the page is awaiting a promise the
// event loop has to reach — a `connect` that held the loop would never see one.
//
// The whole schedule is set up inside the constructor, so the log is ordered
// relative to *the dial* rather than to however long this engine took to load
// the artifact. Timers scheduled together fire in delay order, so the sequence
// below is the sequence, on a loaded machine as much as an idle one.
const log = [];
let ticks = 0;
const tick = () => log.push(`tick ${(ticks += 1)}`);
globalThis.WebSocket = class {
  constructor(url) {
    log.push(`dialled ${url}`);
    this.protocol = "feed.v1";
    this.extensions = "";
    setTimeout(tick, 10);
    setTimeout(() => this.onopen({}), 20);
    setTimeout(tick, 30);
    setTimeout(() => this.onmessage({ data: "one" }), 40);
    setTimeout(tick, 50);
    // Nothing, and octets. A frame of length zero is a message a page is owed
    // like any other, and a binary one arrives as the `ArrayBuffer` the
    // runtime asked `binaryType` for.
    setTimeout(() => this.onmessage({ data: "" }), 60);
    setTimeout(() => this.onmessage({ data: new Uint8Array([1, 2, 3]).buffer }), 70);
    // Not 1000, so the reason the page reads had to come off the event.
    setTimeout(() => this.onclose({ code: 1001, wasClean: true }), 80);
  }
  send(data) {
    log.push(`sent ${data}`);
  }
  close(code) {
    log.push(`closed ${code}`);
  }
};

await import("./.buri/out/web/cmd/page/main.mjs");
console.log(log.join("\n"));
"#,
    );

    let out = Command::new(js_runtime())
        .arg(&driver)
        .output()
        .expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the page did not finish:\n{stdout}{stderr}");

    for line in [
        "mounted",
        // `feed.v1` is what the double negotiated, and the empty `extensions`
        // beside it is why there is exactly one header rather than two.
        "page opened 101 feed.v1",
        "page heard text 3 one",
        // A message of nothing and a message of octets, both of which a page
        // is owed and neither of which is a `.Text` of three characters.
        "page heard text 0 ",
        "page heard binary 3",
        "page closed after 3 .GoingAway",
        "page ended .GoingAway",
    ] {
        assert!(stdout.contains(line), "the page never said `{line}`:\n{stdout}{stderr}");
    }
    // The double's own log, in full. The `sent subscribe` between tick 1 and
    // tick 2 is the whole claim: the page's `onOpen` ran and the event loop had
    // the next timer before the socket was done. Nothing calls `close`, because
    // the far side is what ended this socket.
    let seen = "dialled wss://example.test/feed\n\
                tick 1\n\
                sent subscribe\n\
                tick 2\n\
                tick 3";
    assert!(
        stdout.contains(seen),
        "the page held the event loop, or dialled something else:\n{stdout}{stderr}"
    );
}

/// A one-shot server that answers one connection with `answer`, or a port with
/// nothing behind it when `answer` is empty.
///
/// Two of the ways a dial fails, as listeners a test holds: a port nobody is on,
/// and a server that answers something other than `101`. Neither is a handshake
/// a Buri server can be made to write, so the far side has to be written here.
fn one_answer(answer: &'static str) -> (u16, Option<std::thread::JoinHandle<()>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a bound port");
    let port = listener.local_addr().expect("the bound port").port();
    if answer.is_empty() {
        // Bound, read back, and dropped: a port this machine really did hand
        // out a moment ago, which is a stronger arrangement than picking a
        // number and hoping nothing holds it.
        return (port, None);
    }
    let serving = std::thread::spawn(move || {
        let patience = std::time::Duration::from_secs(20);
        let Ok((mut socket, _from)) = listener.accept() else { return };
        let _read = socket.set_read_timeout(Some(patience));
        let _written = socket.set_write_timeout(Some(patience));
        let mut head: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 512];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            }
        }
        let _sent = socket.write_all(answer.as_bytes());
        let _flushed = socket.flush();
    });
    (port, Some(serving))
}

/// **A dial that cannot open is `.Transport` on the JavaScript backend too, and
/// against real sockets.**
///
/// `agreement.rs` pins the refusal that needs no server — a scheme no platform
/// speaks — through every pipeline at once. These two need one: a port nobody
/// holds, and a server that answers `404` rather than switching protocols. The
/// natives run the same two as linked programs in `native::e2e`; this is them
/// through `node`'s or `bun`'s own `WebSocket`, which is the client a page and a
/// worker dial with too.
///
/// **A fact about the attempt is `.Transport`; a fact about the platform is
/// `.Unsupported`.** A browser reports both of these the same way — an `error`
/// before the socket opened — and what a program can act on is that the socket
/// never opened, which the silent hooks are the other half of.
///
/// **The third native refusal is deliberately not here.** A `101` signing
/// another handshake's key is refused by `node` and opened by `bun`, because the
/// check belongs to the engine's own `WebSocket` and not to anything this
/// runtime writes: a page never sees the key it sent. `design/native/
/// DECISIONS.md` carries that as a row rather than this file carrying it as an
/// assertion two engines disagree about.
#[test]
fn a_javascript_client_is_refused_when_a_dial_cannot_open() {
    let scratch = Scratch::repo("js-dial-refused");
    scratch.write(
        "cmd/dial/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: JS, entry: \"main\" },\n    ]\n}\n",
    );
    scratch.write(
        "cmd/dial/main.buri",
        r#"
from "core/effect" import { Allocator, Environment, Sockets, Stdout, WebSocketClient };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context {
    Allocator: host.alloc,
    Environment: host.env,
    Sockets: host.sockets,
    Stdout: host.stdout,
    WebSocketClient: host.websocketClient,
  };
  // Three calls rather than a `mapCtx`: a lambda that suspends is not a thing a
  // list combinator awaits today, and what this row is about is the answer each
  // dial gives.
  let ports = env.arguments(ctx);
  let _nobody = dialling(ctx, ports.get(0).withDefault("0"));
  let _not101 = dialling(ctx, ports.get(1).withDefault("0"));
  .Ok(())
}

/// One dial, and the one line it is worth. `errorText` is the constant per
/// variant, so what varies between the three is the cause and nothing else.
fn dialling<C: Allocator + Sockets + Stdout + WebSocketClient>(ctx: C, port: Str): () {
  let url = str.format(ctx, "ws://127.0.0.1:${port}/socket");
  match (websocket.connect(ctx, silent(url))) {
    .Err(e) => {
      let _said = io.println(ctx, "refused ${e.cause}").ignore();
      ()
    },
    .Ok(reason) => {
      let _said = io.println(ctx, "opened, and ended ${reason}").ignore();
      ()
    },
  }
}

/// Hooks that would announce themselves if they ran. None of them does.
fn silent<C: Allocator + Sockets + Stdout + WebSocketClient>(url: Str): Client<C, Int> {
  Client {
    url: url,
    onOpen: fn(c, _socket, _response) => {
      let _said = io.println(c, "a hook ran").ignore();
      0
    },
    onMessage: fn(_c, _socket, seen, _message) => seen + 1,
    onClose: fn(c, _socket, _seen, _reason) => io.println(c, "a hook ran").ignore(),
  }
}
"#,
    );
    scratch.run(&["build", "//cmd/dial"]).ok();

    let (nobody, none) = one_answer("");
    assert!(none.is_none(), "a port with nothing behind it has no server thread");
    let (not_101, answering) =
        one_answer("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");

    let out = Command::new(js_runtime())
        .arg(scratch.path(".buri/out/js/cmd/dial/main.mjs"))
        .arg(nobody.to_string())
        .arg(not_101.to_string())
        .output()
        .expect("the javascript runtime runs");
    for server in [answering].into_iter().flatten() {
        server.join().expect("the one-shot server finished");
    }
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the client did not finish:\n{stdout}{stderr}");
    assert_eq!(
        stdout,
        "refused .Transport\nrefused .Transport\n",
        "a dial that could not open answered something else, or a hook ran:\n{stderr}"
    );
}

/// **The JavaScript backend's client, against a real socket, with a real
/// engine's `WebSocket` under it.**
///
/// The row above is the refusals; this is the exchange. It matters that both
/// exist, because on `JS` and `WEB` this toolchain writes no framing at all —
/// the platform's own `WebSocket` owns the handshake, the masking, the ping and
/// the reassembly — so what `runtime.js` is responsible for is the *queue* over
/// it: a frame that lands while nobody is asking must be answered by the next
/// `connectReceive` rather than lost between two of them.
///
/// That is what the far side is built to break. `harness::websocket` writes
/// eight things back to back, faster than a program that awaits one at a time
/// can consume them, so a runtime that awaited the event instead of a queue
/// would lose every frame but the first.
///
/// The rest of the first session is the surface a page shares with a native
/// program:
///
/// * both framings, and payloads of nothing, of one octet, and of past the
///   point where a frame's length stops fitting in two;
/// * a ping the program is never told about, with the message after it as the
///   evidence that the socket lived through it;
/// * a fragmented message, cut mid-character, which arrives as one message with
///   its multi-octet characters intact;
/// * a close the **program** started, which is `.Normal` and which the far side
///   reads back off the wire as 1000.
///
/// The second session is a connection that breaks: the far side goes without a
/// close frame, and the program is told `.Abnormal`. The third is the one place
/// a page's close differs from a native program's: the program hangs up with
/// `.GoingAway`, and what the far side is told is **1000**, because the
/// browser's `WebSocket.close` throws on every code but 1000 and the private
/// range.
///
/// **Why the program closes rather than the server, unlike the native row.**
/// The two engines this suite runs on do not agree about a close they were
/// *sent*. Against the same server, `node` reports 1001 cleanly and `bun`
/// reports 1000 with `wasClean: false` — which this runtime reads as 1006 and a
/// program reads as `.Abnormal`. That is the engine's own `WebSocket` and
/// nothing this toolchain writes, it is the same shape as the accept-key
/// disagreement already recorded, and it lives in `design/native/DECISIONS.md`
/// as a row rather than here as an assertion two engines answer differently.
/// The two endings this row *does* assert are the two they agree on.
#[test]
fn a_javascript_client_carries_every_shape_over_a_real_socket() {
    use crate::harness::websocket::{Heard, Step};
    const LARGE: usize = 70000;
    /// Sixteen octets and twelve characters, two of which take more than one.
    const SPANS: &[u8] = "h\u{e9}llo \u{1f30a} done".as_bytes();
    let scratch = Scratch::repo("js-dial-exchange");
    scratch.write(
        "cmd/dial/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: JS, entry: \"main\" },\n    ]\n}\n",
    );
    scratch.write(
        "cmd/dial/main.buri",
        r#"
from "core/effect" import { Allocator, Environment, Sockets, Stdout, WebSocketClient };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/net/server" import { CloseReason };
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

/// Say six things of every shape, print every one that comes back, and hang up
/// once `hangUpAt` of them have.
fn feed<C: Allocator + Sockets + Stdout + WebSocketClient>(
  url: Str,
  large: Int,
  hangUpAt: Int,
  saying: CloseReason,
): Client<C, Int> {
  Client {
    url: url,
    onOpen: fn(c, socket, response) => {
      let _said = io.println(c, "opened ${response.status}").ignore();
      let _empty = socket.send(c, .Text(""));
      let _one = socket.send(c, .Text("x"));
      let _many = socket.send(c, .Text("a".repeat(c, large)));
      let _none = socket.send(c, .Binary([]));
      let _byte = socket.send(c, .Binary([7]));
      let _bytes = socket.send(c, .Binary(list.repeat(c, 7, large)));
      0
    },
    onMessage: fn(c, socket, seen, message) => {
      let _said = match (message) {
        .Text(text) => io.println(c, "text ${text.length()} ${text}").ignore(),
        .Binary(data) => io.println(c, "binary ${data.length()}").ignore(),
      };
      let next = seen + 1;
      let _closed = if (next == hangUpAt) { socket.close(c, saying) } else { () };
      next
    },
    onClose: fn(c, _socket, seen, reason) => {
      io.println(c, "closed after ${seen} ${reason}").ignore()
    },
  }
}

/// One dial, and the line that says how it ended.
fn dialling<C: Allocator + Sockets + Stdout + WebSocketClient>(
  ctx: C,
  url: Str,
  large: Int,
  hangUpAt: Int,
  saying: CloseReason,
): () {
  match (websocket.connect(ctx, feed(url, large, hangUpAt, saying))) {
    .Err(e) => {
      let _said = io.println(ctx, "refused ${e.cause}").ignore();
      ()
    },
    .Ok(reason) => {
      let _said = io.println(ctx, "ended ${reason}").ignore();
      ()
    },
  }
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Allocator: host.alloc,
    Environment: host.env,
    Sockets: host.sockets,
    Stdout: host.stdout,
    WebSocketClient: host.websocketClient,
  };
  let args = env.arguments(ctx);
  let port = args.get(0).withDefault("0");
  let large = args.get(1).withDefault("0").toInt().withDefault(0);
  let url = str.format(ctx, "ws://127.0.0.1:${port}/socket");
  // The first session hangs up on the seventh message; the second never
  // reaches its number, so what ends it is the far side going. The second
  // sends nothing large, because what it is about is the ending and a second
  // seventy-kilobyte round trip through an engine's own `WebSocket` is seconds
  // of this suite's budget for a claim the first session already made.
  let _first = dialling(ctx, url, large, 7, .Normal);
  let _second = dialling(ctx, url, 0, 7, .Normal);
  // And a third, which hangs up on the first message with a reason that is not
  // `.Normal`. What the far side is told is the platform's decision, and this
  // is where a page differs from a native program.
  let _third = dialling(ctx, url, 0, 1, .GoingAway);
  .Ok(())
}
"#,
    );
    scratch.run(&["build", "//cmd/dial"]).ok();

    // The six the program sends, read back off the wire masked and whole; then
    // six back the other way, written without waiting for anybody.
    let saying = || {
        vec![
            Step::Hear,
            Step::Hear,
            Step::Hear,
            Step::Hear,
            Step::Hear,
            Step::Hear,
        ]
    };
    let mut first = saying();
    first.extend([
        Step::Text(String::new()),
        Step::Text(String::from("x")),
        Step::Text("a".repeat(LARGE)),
        Step::Binary(Vec::new()),
        Step::Binary(vec![7]),
        Step::Binary(vec![7; LARGE]),
        Step::Ping(b"beat".to_vec()),
        // Cut at octet 2 and octet 9, which is the middle of the `\u{e9}` and
        // the middle of the `\u{1f30a}`: neither piece is text on its own.
        Step::Fragments(
            [&SPANS[..2], &SPANS[2..9], &SPANS[9..]]
                .iter()
                .map(|piece| piece.to_vec())
                .collect(),
        ),
        // The program hangs up on the seventh message; this reads that close
        // and answers it.
        Step::Bye(1000),
    ]);
    let mut second = saying();
    second.extend([Step::Text(String::from("bye")), Step::Drop]);
    let mut third = saying();
    third.extend([Step::Text(String::from("go")), Step::Bye(1000)]);
    // The second session's six are the same six shapes with nothing large in
    // them, which is what its `large` of zero collapses the two big ones to.
    let small = || {
        vec![
            Heard::Text(String::new()),
            Heard::Text(String::from("x")),
            Heard::Text(String::new()),
            Heard::Binary(Vec::new()),
            Heard::Binary(vec![7]),
            Heard::Binary(Vec::new()),
        ]
    };
    let serving = crate::harness::websocket::serving(vec![first, second, third]);
    let port = serving.port;

    let out = Command::new(js_runtime())
        .arg(scratch.path(".buri/out/js/cmd/dial/main.mjs"))
        .arg(port.to_string())
        .arg(LARGE.to_string())
        .output()
        .expect("the javascript runtime runs");
    let heard = serving.heard();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the client did not finish:\n{stdout}{stderr}");
    let large = "a".repeat(LARGE);
    // `Str::len` counts Unicode scalar values, so the fragmented message is
    // twelve of them and not the sixteen octets it took on the wire — and the
    // three fragments cut two of those characters in half.
    assert_eq!(
        stdout,
        format!(
            "opened 101\ntext 0 \ntext 1 x\ntext {LARGE} {large}\nbinary 0\nbinary 1\n\
             binary {LARGE}\ntext 12 h\u{e9}llo \u{1f30a} done\n\
             closed after 7 .Normal\nended .Normal\n\
             opened 101\ntext 3 bye\nclosed after 1 .Abnormal\nended .Abnormal\n\
             opened 101\ntext 2 go\nclosed after 1 .Normal\nended .Normal\n"
        ),
        "a frame was lost, a ping reached the program, or an ending was read as \
         the wrong one:\n{stderr}"
    );
    let said = || {
        vec![
            Heard::Text(String::new()),
            Heard::Text(String::from("x")),
            Heard::Text("a".repeat(LARGE)),
            Heard::Binary(Vec::new()),
            Heard::Binary(vec![7]),
            Heard::Binary(vec![7; LARGE]),
        ]
    };
    let mut first_heard = said();
    // `.Normal` is 1000 on the wire, and this is the far side reading it.
    first_heard.push(Heard::Closed(Some(1000)));
    let mut third_heard = small();
    // **And `.GoingAway` is 1000 here too, which is the divergence.** A page's
    // `WebSocket.close` accepts 1000 and the private range 3000–4999 and throws
    // on everything else, so `runtime.js` sends 1000 rather than losing the
    // close — and the program is handed `.Normal` back, because 1000 is what
    // the engine then reports. A native program's far side reads 1001, which
    // `native::e2e::a_client_carries_every_size_and_is_told_nothing_of_a_ping`
    // asserts; `design/native/DECISIONS.md` carries the row.
    third_heard.push(Heard::Closed(Some(1000)));
    assert_eq!(
        heard,
        vec![first_heard, small(), third_heard],
        "what the client wrote is not what the far side read.\nit said:\n{stdout}"
    );
}

/// A worker that dials a socket while answering a request — five times, and
/// each dial ends a different way.
///
/// `WebSocketClient` and `Sockets` are granted on `CLOUDFLARE_WORKER` like every
/// other platform, and this is what that grant buys: a worker handed a request
/// dials somebody else's socket, reads what comes back, and answers with it.
///
/// The driver is the platform's half — the worker's default export, a real
/// `Request`, a real `Response` — with the same `WebSocket` double the page row
/// uses. A worker parks on the dial exactly as a page does, which is what
/// `$fetchEntry` awaiting the entry is for.
///
/// **Five requests rather than one, because a browser has five endings and one
/// request only reaches the first of them.** The engine hands a page one `close`
/// event and one `error` event, and what they *mean* depends entirely on whether
/// the socket had opened yet — so the same two events are four different answers
/// and there is no other way to reach three of them:
///
/// | The event | Before the socket opened | After |
/// |---|---|---|
/// | `close` with a code | the handshake failed: `.Err` | that code's `CloseReason` |
/// | `close` with no code | the handshake failed: `.Err` | 1005, which is `.Abnormal` |
/// | `error` | the handshake failed: `.Err` | 1006, which is `.Abnormal` |
///
/// So the five sessions are: a clean close with `1000`, a close with no code, an
/// `error` on an open socket, an `error` before the handshake finished, and a
/// `close` before it finished. The last two are the two sentences a program is
/// given for a socket that never opened, and no hook runs for either — which is
/// the absence of a `sent` line in the log.
#[test]
fn a_worker_dials_a_socket_while_it_answers_a_request() {
    let scratch = Scratch::repo("worker-dials");
    scratch.write(
        "cmd/relay/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: CLOUDFLARE_WORKER, entry: \"fetch\" },\n    ]\n}\n",
    );
    scratch.write(
        "cmd/relay/main.buri",
        r#"
from "core/effect" import { Allocator, Request, Response, Sockets, WebSocketClient };
from "core/host" import * as host;
from "core/net/http" import * as http;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

export fn fetch(request: Request): Response {
  let ctx = context {
    Allocator: host.alloc,
    Sockets: host.sockets,
    WebSocketClient: host.websocketClient,
  };
  match (websocket.connect(ctx, relaying(request.path()))) {
    .Err(e) => http.text(ctx, str.format(ctx, "no socket: ${e.detail}")),
    .Ok(reason) => http.text(ctx, str.format(ctx, "ended ${reason}")),
  }
}

/// The three hooks, over a socket a worker dialled.
///
/// A worker has nowhere to print, so what the far side said reaches the world
/// the only way it can: `onMessage` pushes it back on the socket, and the
/// double's own log is where the test reads it.
fn relaying<C: Allocator + Sockets + WebSocketClient>(path: Str): Client<C, Int> {
  Client {
    url: "wss://example.test/relay",
    onOpen: fn(c, socket, _response) => {
      let _sent = socket.send(c, .Text(str.format(c, "asking ${path}")));
      0
    },
    onMessage: fn(c, socket, seen, message) => {
      let said = match (message) {
        .Text(text) => str.format(c, "heard text ${text.length()}:${text}"),
        .Binary(data) => str.format(c, "heard binary ${data.length()}"),
      };
      let _sent = socket.send(c, .Text(said));
      seen + 1
    },
    onClose: fn(_c, _socket, _seen, _reason) => (),
  }
}
"#,
    );
    scratch.run(&["build", "//cmd/relay"]).ok();

    let driver = scratch.write(
        "drive.mjs",
        r#"
import worker from "./.buri/out/cloudflare-worker/cmd/relay/fetch.mjs";

// Five dials, five endings, one script each. Everything is on a timer, so the
// worker parks on each step and nothing here runs unless the event loop is
// free — a `connect` that held it would not reach the open, let alone the
// close.
const scripts = [
  // A socket that opened, carried three messages of three shapes, and closed
  // normally. The empty text is a message like any other; the binary one
  // arrives as the `ArrayBuffer` the runtime asked `binaryType` for.
  (ws) => {
    setTimeout(() => ws.onopen({}), 5);
    setTimeout(() => ws.onmessage({ data: "pong" }), 10);
    setTimeout(() => ws.onmessage({ data: "" }), 15);
    setTimeout(() => ws.onmessage({ data: new Uint8Array([1, 2, 3]).buffer }), 20);
    setTimeout(() => ws.onclose({ code: 1000, wasClean: true }), 25);
  },
  // A close carrying no code at all, which RFC 6455 calls 1005.
  (ws) => {
    setTimeout(() => ws.onopen({}), 5);
    setTimeout(() => ws.onclose({ wasClean: true }), 10);
  },
  // An error on a socket that was open, which is the connection breaking:
  // 1006, and there is no close frame to say otherwise.
  (ws) => {
    setTimeout(() => ws.onopen({}), 5);
    setTimeout(() => ws.onerror({}), 10);
  },
  // The same two events before the handshake finished, which are not endings
  // at all — they are a socket that never opened.
  (ws) => {
    setTimeout(() => ws.onerror({}), 5);
  },
  (ws) => {
    setTimeout(() => ws.onclose({ code: 1006, wasClean: false }), 5);
  },
];

const log = [];
let at = 0;
globalThis.WebSocket = class {
  constructor(url) {
    log.push(`dialled ${url}`);
    this.protocol = "";
    this.extensions = "";
    scripts[at++](this);
  }
  send(data) {
    log.push(`sent ${data}`);
  }
  close(code) {
    log.push(`closed ${code}`);
  }
};

for (const path of ["/rooms/9", "/b", "/c", "/d", "/e"]) {
  const answer = await worker.fetch(new Request(`https://example.com${path}`));
  console.log(`${answer.status} ${await answer.text()}`);
}
console.log(log.join("\n"));
"#,
    );

    let out = Command::new(js_runtime())
        .arg(&driver)
        .output()
        .expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the worker did not answer:\n{stdout}{stderr}");
    assert_eq!(
        stdout,
        "200 ended .Normal\n\
         200 ended .Abnormal\n\
         200 ended .Abnormal\n\
         200 no socket: the connection failed before the handshake finished\n\
         200 no socket: the socket closed before the handshake finished\n\
         dialled wss://example.test/relay\n\
         sent asking /rooms/9\n\
         sent heard text 4:pong\n\
         sent heard text 0:\n\
         sent heard binary 3\n\
         dialled wss://example.test/relay\n\
         sent asking /b\n\
         dialled wss://example.test/relay\n\
         sent asking /c\n\
         dialled wss://example.test/relay\n\
         dialled wss://example.test/relay\n",
        "a worker did not dial, lost what it heard, or read an ending as the \
         wrong one:\n{stderr}"
    );
}

/// A website, both halves, driven the way the two platforms drive them.
///
/// **The top of what a website can be asked.** The repository is
/// `repositories/concurrency/website`, and the case beside it builds it and
/// reads the artifacts; what no `buri` command can do is *run* either half — a
/// worker is called by its platform, and a page needs a document. So this is
/// the platform's own side of both, in one JavaScript module: the worker's
/// `fetch` handed a real `Request`, and the page's `main` imported on top of a
/// document that already holds what the worker sent.
///
/// The document double parses that markup the way a browser does — one run of
/// text per run, however many the tree that wrote it had — and counts every
/// node it is asked to make. So "the page resumed on the markup" is a number
/// and a comparison rather than an impression.
///
/// Four claims, and the driver prints a line for each so a wrong answer names
/// which:
///
///  * the worker answers HTML, with the tree rendered into it and the state it
///    rendered from beside it;
///  * the page picks that state up, and the address it is at;
///  * it builds nothing — the markup after the resume is the markup that
///    arrived, node for node;
///  * and the button works. A press writes the signal the page made and the
///    label the *server* wrote changes, which is the whole of what resuming is
///    for.
///
/// Then the failure beside it: the same page resumed at an address the server
/// did not render, so the tree and the markup disagree. It answers `.Err`, and
/// the artifact exits 1 with the sentence.
#[test]
fn a_website_is_rendered_by_its_worker_and_resumed_by_its_page() {
    let site = tests_dir().join("repositories/concurrency/website/repo");
    let scratch = Scratch::copy_of("website", &site);
    scratch.run(&["build", "//cmd/site"]).ok();

    let driver = scratch.write("drive.mjs", &format!("{DOCUMENT_DOUBLE}\n{WEBSITE_DRIVER}"));
    let drive = |at: &str| {
        let out = Command::new(js_runtime())
            .arg(&driver)
            .arg(at)
            .output()
            .expect("the javascript runtime runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };

    let (code, stdout, stderr) = drive("/");
    assert_eq!(code, 0, "the website did not answer:\n{stdout}{stderr}");

    let sent = "<main><h1>Buri</h1>visitors: 3\
                <button type=\"button\">say thanks</button></main>\
                <script id=\"buri-state\" type=\"application/json\" data-path=\"/\">\
                {\"title\":\"Buri\",\"visitors\":3}</script>";
    let pressed = sent.replace(">say thanks<", ">thanks<");
    assert_eq!(
        stdout,
        format!(
            "200 text/html; charset=utf-8\n\
             {sent}\n\
             resumed / {{\"title\":\"Buri\",\"visitors\":3}}\n\
             made 0 elements and 0 runs of text\n\
             {sent}\n\
             {pressed}\n"
        ),
        "the website lost a half:\n{stderr}"
    );

    // The failure. `/about` is a page the server did not send, and `shell` wrote
    // down which page it did send, so this is refused before a node is walked.
    let (code, stdout, stderr) = drive("/about");
    assert_eq!(code, 1, "a resume at an address the server did not send must fail:\n{stdout}{stderr}");
    assert!(
        stderr.contains(
            "this page is not the page the server sent: it was rendered at / and the address \
             is /about"
        ),
        "and it must name both: {stderr}"
    );
}

/// A document a page can resume into: the markup a browser would have parsed,
/// and the handful of operations the runtime asks of one.
///
/// Shared by the two rows that drive a page —
/// [`a_website_is_rendered_by_its_worker_and_resumed_by_its_page`] and
/// [`a_resumed_page_takes_the_markup_a_browser_would_have_handed_it`] — so there
/// is one document here rather than two that can drift apart.
///
/// It parses the way a browser parses: one run of text per run however many the
/// tree that wrote it had, entities read back, an attribute that stands on its
/// own kept as one, and an element left **open** unless HTML says it holds
/// nothing — so `<div />` is not a closed `divide` here either, which is the whole
/// reason the renderer writes a closing tag.
///
/// `browser(sent, at, holds)` installs it; `holds: false` is a platform whose
/// document has no body yet. `navigate(to)` is the reader pressing Back. `made`
/// counts every node the page asked the document for, so "the page built
/// nothing" is a number and a comparison rather than an impression.
const DOCUMENT_DOUBLE: &str = r##"
// Everything this document is asked to build, counted.
const made = { elements: 0, text: 0 };

function node(nodeType, nodeName) {
  const self = {
    nodeType,
    nodeName,
    childNodes: [],
    parentNode: null,
    listeners: {},
    attributes: {},
    data: "",
    // `className` and `style` are attributes in a browser, so they are
    // attributes here: writing "" to either *adds* `class=""` to markup the
    // server wrote, and a resume that did that would not be leaving the
    // document alone.
    get className() {
      const held = this.attributes.class;
      return held === undefined || held === null ? "" : held;
    },
    set className(value) {
      this.attributes.class = value;
    },
    get firstChild() {
      return this.childNodes.length > 0 ? this.childNodes[0] : null;
    },
    get nextSibling() {
      const parent = this.parentNode;
      if (parent === null) return null;
      const where = parent.childNodes.indexOf(this);
      return where + 1 < parent.childNodes.length ? parent.childNodes[where + 1] : null;
    },
    get textContent() {
      if (this.nodeType === 3) return this.data;
      return this.childNodes.map((c) => c.textContent).join("");
    },
    insertBefore(child, before) {
      if (child.parentNode !== null) child.parentNode.removeChild(child);
      child.parentNode = this;
      const where = before === null ? this.childNodes.length : this.childNodes.indexOf(before);
      this.childNodes.splice(where, 0, child);
      return child;
    },
    appendChild(child) {
      return this.insertBefore(child, null);
    },
    removeChild(child) {
      const where = this.childNodes.indexOf(child);
      if (where >= 0) this.childNodes.splice(where, 1);
      child.parentNode = null;
      return child;
    },
    setAttribute(name, value) {
      this.attributes[name] = value;
    },
    getAttribute(name) {
      const held = this.attributes[name];
      return held === undefined ? null : held;
    },
    addEventListener(type, handler) {
      this.listeners[type] = handler;
    },
    splitText(where) {
      const tail = node(3, "#text");
      tail.data = this.data.slice(where);
      this.data = this.data.slice(0, where);
      this.parentNode.insertBefore(tail, this.nextSibling);
      return tail;
    },
  };
  self.style = {
    get cssText() {
      const held = self.attributes.style;
      return held === undefined || held === null ? "" : held;
    },
    set cssText(text) {
      self.attributes.style = text;
    },
    setProperty(name, value) {
      const held = self.style.cssText;
      self.attributes.style = held === "" ? `${name}: ${value}` : `${held}; ${name}: ${value}`;
    },
  };
  return self;
}

// HTML's own empty elements. A trailing slash closes one of these and nothing
// else: a parser reads `<div />` as an opening tag and puts what follows inside.
const VOID = new Set([
  "area", "base", "br", "col", "embed", "hr", "img",
  "input", "link", "meta", "param", "source", "track", "wbr",
]);

const NAMED = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };

function unescaped(text) {
  return text.replace(/&(#[xX][0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);/g, (whole, body) => {
    if (body[0] !== "#") return NAMED[body] === undefined ? whole : NAMED[body];
    const digits = body[1] === "x" || body[1] === "X" ? body.slice(2) : body.slice(1);
    const code = parseInt(digits, body[1] === "x" || body[1] === "X" ? 16 : 10);
    return Number.isNaN(code) ? whole : String.fromCodePoint(code);
  });
}

const escaped = (t) => t.split("&").join("&amp;").split("<").join("&lt;").split(">").join("&gt;");
const quoted = (t) => escaped(t).split('"').join("&quot;");

// The markup, parsed the way a browser parses it.
function parse(html, into) {
  const stack = [into];
  const text = (data) => {
    if (data === "") return;
    const run = node(3, "#text");
    run.data = unescaped(data);
    stack[stack.length - 1].appendChild(run);
  };
  let read = 0;
  while (read < html.length) {
    const lt = html.indexOf("<", read);
    if (lt < 0) {
      text(html.slice(read));
      break;
    }
    text(html.slice(read, lt));
    const gt = html.indexOf(">", lt);
    const tag = html.slice(lt + 1, gt);
    read = gt + 1;
    if (tag.startsWith("/")) {
      stack.pop();
      continue;
    }
    const written = tag.split(/[ /]/)[0];
    const name = written.toLowerCase();
    const element = node(1, name.toUpperCase());
    // An attribute may carry a value or stand on its own: `checked` and
    // `checked=""` are one attribute to a browser, and only the first is markup
    // anybody writes.
    for (const found of tag.slice(written.length).matchAll(/([a-zA-Z-]+)(?:="([^"]*)")?/g)) {
      element.attributes[found[1]] = found[2] === undefined ? null : unescaped(found[2]);
    }
    stack[stack.length - 1].appendChild(element);
    if (name === "script") {
      const close = html.indexOf("</script>", read);
      const held = node(3, "#text");
      held.data = html.slice(read, close);
      element.appendChild(held);
      read = close + "</script>".length;
      continue;
    }
    if (!VOID.has(name)) stack.push(element);
  }
}

// What a reader is looking at. Markers are the runtime's own bookkeeping and a
// browser shows none of them, so neither does this.
function markup(n) {
  if (n.nodeType === 8) return "";
  if (n.nodeType === 3) return escaped(n.data);
  const name = n.nodeName.toLowerCase();
  let out = "<" + name;
  for (const key of Object.keys(n.attributes)) {
    const value = n.attributes[key];
    out += value === null ? ` ${key}` : ` ${key}="${quoted(value)}"`;
  }
  if (VOID.has(name)) return out + " />";
  // What is inside a `script` or a `style` is raw text: a browser neither reads
  // entities in it nor writes them back out.
  if (name === "script" || name === "style") {
    return `${out}>${n.childNodes.map((c) => c.data).join("")}</${name}>`;
  }
  let inner = "";
  for (const child of n.childNodes) inner += markup(child);
  return `${out}>${inner}</${name}>`;
}

const body = node(1, "BODY");
const showing = () => body.childNodes.map(markup).join("");

const findFirst = (n, name) => {
  if (n.nodeType === 1 && n.nodeName === name) return n;
  for (const child of n.childNodes) {
    const found = findFirst(child, name);
    if (found !== null) return found;
  }
  return null;
};

// The window's own listeners. `popstate` is the one a page registers.
const heard = {};
globalThis.addEventListener = (type, handler) => {
  if (heard[type] === undefined) heard[type] = [];
  heard[type].push(handler);
};

function browser(sent, at, holds) {
  parse(sent, body);
  globalThis.document = {
    get body() {
      return holds === false ? null : body;
    },
    getElementById(id) {
      const walk = (n) => {
        if (n.nodeType === 1 && n.attributes.id === id) return n;
        for (const child of n.childNodes) {
          const found = walk(child);
          if (found !== null) return found;
        }
        return null;
      };
      return walk(body);
    },
    createElement(name) {
      made.elements++;
      return node(1, name.toUpperCase());
    },
    createTextNode(data) {
      made.text++;
      const run = node(3, "#text");
      run.data = data;
      return run;
    },
    createComment() {
      return node(8, "#comment");
    },
  };
  globalThis.location = { pathname: at };
}

// The reader going back or forward: the address changes, and the browser says
// so. Nothing else here touches the document.
function navigate(to) {
  globalThis.location.pathname = to;
  for (const handler of heard.popstate || []) handler({});
}

const press = (button) => button.listeners.click({ preventDefault() {}, target: button });
"##;

/// The site half of [`a_website_is_rendered_by_its_worker_and_resumed_by_its_page`]:
/// the worker called the way a worker runtime calls one, and the page imported
/// on top of the document it sent.
const WEBSITE_DRIVER: &str = r##"
import worker from "./.buri/out/cloudflare-worker/cmd/site/fetch.mjs";

const at = process.argv[2];
const answer = await worker.fetch(new Request("https://example.com/"));
const document_ = await answer.text();
const sent = document_.split("<body>")[1].split("</body>")[0];
console.log(`${answer.status} ${answer.headers.get("content-type")}`);
console.log(sent);

browser(sent, at, true);

await import("./.buri/out/web/cmd/site/main.mjs");

console.log(`made ${made.elements} elements and ${made.text} runs of text`);
console.log(showing());

// The press the server could not have handled.
press(findFirst(body, "BUTTON"));
console.log(showing());
"##;

/// **Every shape the markup can arrive in**, asked of one built website.
///
/// [`a_website_is_rendered_by_its_worker_and_resumed_by_its_page`] is the happy
/// path of a resume. This is the rest of the surface, and it is one build with
/// a driver run per scenario, because what varies is the *markup the browser
/// hands the page* rather than the program.
///
/// Three groups.
///
/// **Markup a browser would have normalised.** The document that reaches a page
/// is never the bytes the worker wrote: runs of text beside each other are one
/// node, an attribute may stand on its own, an empty element may be spelled
/// either way, and an entity may be numeric. A resume that only worked on the
/// server's own spelling would work in a test and fail in a browser.
///
/// **A tree the markup does not match.** One node too many, one too few, and an
/// element renamed are each refused with a sentence naming what was wanted and
/// what was there. A changed attribute and a changed run of text are *not*: they
/// are written, because what a page renders comes from a state that is allowed
/// to have moved on.
///
/// **What a page does after it has resumed.** It presses a button the server
/// wrote, navigates, and finds that the region which read the path is the only
/// one rebuilt — the node holding the title is the same object it was. Then it
/// fetches a lazy chunk, which is the other half of this feature meeting this
/// one.
///
/// And the two `.Err`s `resume` documents: a document with no body at all, and
/// a body with nothing in it.
#[test]
fn a_resumed_page_takes_the_markup_a_browser_would_have_handed_it() {
    let scratch = Scratch::repo("resume-edges");
    scratch.write(
        "cmd/edges/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: WEB, entry: \"main\" },\n        \
         { platform: CLOUDFLARE_WORKER, entry: \"fetch\" },\n    ]\n}\n",
    );
    scratch.write("cmd/edges/main.buri", RESUME_EDGES_PAGE);
    scratch.run(&["build", "//cmd/edges"]).ok();

    let driver = scratch.write("drive.mjs", &format!("{DOCUMENT_DOUBLE}\n{RESUME_EDGES_DRIVER}"));
    let drive = |how: &str, at: &str| {
        let out = Command::new(js_runtime())
            .arg(&driver)
            .arg(how)
            .arg(at)
            .output()
            .expect("the javascript runtime runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };

    // What the worker writes, and therefore what every scenario starts from.
    let sent = "<main><h1>Tom &amp; Jerry &lt;br&gt; ✓</h1>onetwo \
                <img src=\"/cat.png\" alt=\"a cat\" />\
                <aside><ul><li>alpha</li><li>beta</li></ul>\
                <button type=\"button\">press me</button></aside>\
                <article>home</article></main>";
    // Every `<` in the state is written `<`, which is how a string holding
    // `</script>` closes nothing — and it is a JSON escape, so what the page
    // reads back is the character.
    let state = "{\"title\":\"Tom & Jerry \\u003cbr> ✓\",\"gap\":\" \",\"picture\":\"/cat.png\",\
                 \"twice\":false,\"rows\":[{\"key\":\"a\",\"label\":\"alpha\"},\
                 {\"key\":\"b\",\"label\":\"beta\"}]}";
    // The whole body: the tree, and the state script `shell` put beside it —
    // carrying the address it rendered for.
    let showing = format!(
        "{sent}<script id=\"buri-state\" type=\"application/json\" \
         data-path=\"/\">{state}</script>"
    );

    // --- The whole of a page's life, in one run -------------------------------
    let (code, stdout, stderr) = drive("plain", "/");
    assert_eq!(code, 0, "the page did not resume:\n{stdout}{stderr}");
    let pressed = showing.replace(">press me<", ">pressed once<");
    let about = pressed
        .replace("<article>home</article>", "<article><h2>About</h2>about</article>");
    let again = about.replace(">pressed once<", ">pressed twice<");
    assert_eq!(
        stdout,
        format!(
            "sent {sent}\n\
             before {state}\n\
             after {state}\n\
             at / once\n\
             ONLY_IN_THE_CHUNK over 2 rows\n\
             made 0 elements and 0 runs of text\n\
             showing {showing}\n\
             pressed {pressed}\n\
             navigated 2 elements and 2 runs of text\n\
             showing {about}\n\
             the title is the node the server wrote: true\n\
             pressed {again}\n"
        ),
        "the page's life is not what it was:\n{stderr}"
    );

    // --- Markup a browser would have normalised -------------------------------
    //
    // Each of these is the same document said a different way, and every one of
    // them has to resume with nothing built. The pair is what makes the claim:
    // the run answers 0 nodes *and* the markup it leaves is what arrived.
    for (how, why) in [
        ("boolean", "an attribute that stands on its own"),
        ("open-void", "an empty element spelled without its slash"),
        ("numeric-entity", "an entity written by number"),
        ("upper-case", "a tag name in capitals"),
    ] {
        let (code, stdout, stderr) = drive(how, "/");
        assert_eq!(code, 0, "{why} stopped the resume:\n{stdout}{stderr}");
        assert!(
            stdout.contains("made 0 elements and 0 runs of text\n"),
            "{why} made the page build:\n{stdout}{stderr}"
        );
    }

    // --- A tree the markup does not match -------------------------------------
    for (how, said) in [
        ("missing", "wanted <img>, found <aside>"),
        ("renamed", "wanted <h1>, found <h2>"),
        ("extra", "<ul> holds more than the tree does"),
    ] {
        let (code, stdout, stderr) = drive(how, "/");
        assert_eq!(code, 1, "a mismatch must fail:\n{stdout}{stderr}");
        assert!(
            stderr.contains(&format!("this page is not the markup the server sent: {said}")),
            "and it must name where: {stderr}"
        );
    }

    // --- The address, checked before the shape --------------------------------
    //
    // `/` and `/about` here render the same *shape* — a region holding a heading
    // and a run of text — so the walk would adopt one into the other and find
    // nothing wrong, leaving the reader looking at one page's markup with
    // another page's handlers on it. The path `shell` wrote is what catches
    // that, and it is compared before anything is walked.
    let (code, stdout, stderr) = drive("wrong-address", "/");
    assert_eq!(code, 1, "a page resumed where the server did not render must fail");
    assert!(
        stderr.contains(
            "this page is not the page the server sent: it was rendered at / and the address \
             is /about"
        ),
        "and it must name both:\n{stdout}{stderr}"
    );

    // Answered at `/about` and read at `/about`: the same page. The markup is
    // not `/`'s, so this is the check saying yes rather than saying nothing.
    let (code, stdout, stderr) = drive("plain", "/about");
    assert_eq!(code, 0, "the address the server rendered must resume:\n{stdout}{stderr}");
    assert!(
        stdout.contains("made 0 elements and 0 runs of text\n"),
        "and build nothing:\n{stdout}{stderr}"
    );

    // A query string and a fragment are not part of a path. The worker was asked
    // for `/?ref=x#top` and `Request.path` answers `/`; the address bar has both
    // and `location.pathname` has neither. One page, not two.
    let (code, stdout, stderr) = drive("right-address", "/");
    assert_eq!(code, 0, "a query string must not be a different page:\n{stdout}{stderr}");
    assert!(
        stdout.contains("made 0 elements and 0 runs of text\n"),
        "and it must resume on the markup:\n{stdout}{stderr}"
    );

    // Markup no `shell` wrote carries no path, so there is nothing to compare
    // and the shape is the whole check — which is what it was before.
    let (code, stdout, stderr) = drive("no-path", "/");
    assert_eq!(code, 1, "a page with no embedded path still fails on its shape");
    assert!(
        stderr.contains("this page is not the markup the server sent"),
        "and by the shape rather than the address:\n{stdout}{stderr}"
    );

    // The two that are written rather than refused. A page's numbers come from a
    // state that is allowed to have moved on, so a value that differs is
    // corrected in place and the reader is left looking at the tree.
    let (code, stdout, stderr) = drive("changed-text", "/");
    assert_eq!(code, 0, "a run of text that differs must not stop a resume:\n{stdout}{stderr}");
    assert!(
        stdout.contains(&format!("showing {showing}\n")),
        "and the tree's own text must win:\n{stdout}{stderr}"
    );
    let (code, stdout, stderr) = drive("changed-attribute", "/");
    assert_eq!(code, 0, "an attribute that differs must not stop a resume:\n{stdout}{stderr}");
    assert!(
        stdout.contains(&format!("showing {showing}\n")),
        "and the tree's own attribute must win:\n{stdout}{stderr}"
    );

    // --- Resumed twice --------------------------------------------------------
    //
    // The state says so, so the tree and the markup are the ones above. The
    // second pass walks markup the first pass has already taken over and holds
    // markers the server never wrote, so it is refused like any other
    // disagreement rather than quietly registering everything a second time.
    let (code, stdout, stderr) = drive("twice", "/");
    assert_eq!(code, 0, "the first resume must still succeed:\n{stdout}{stderr}");
    assert!(
        stdout.contains("at / this page is not the markup the server sent"),
        "a second resume must answer .Err naming what it found:\n{stdout}{stderr}"
    );

    // --- Nowhere to resume ----------------------------------------------------
    let (code, stdout, stderr) = drive("no-body", "/");
    assert_eq!(code, 1, "a document with no body must fail:\n{stdout}{stderr}");
    assert!(
        stderr.contains("there is nowhere to resume: this platform has no document"),
        "and it must say which of the two it is: {stderr}"
    );

    // A body with nothing in it is the other one: there is somewhere to resume
    // and nothing there, so it reads as a tree the markup does not match.
    let (code, stdout, stderr) = drive("blank", "/");
    assert_eq!(code, 1, "a blank document must fail:\n{stdout}{stderr}");
    assert!(
        stdout.contains("before none\n"),
        "and `state` must answer `.None` where no server rendered:\n{stdout}{stderr}"
    );
    assert!(
        stderr.contains("this page is not the markup the server sent: wanted <main>, found nothing left"),
        "and the resume must name what it wanted: {stderr}"
    );
}

/// A page with one of everything a resume has to walk: nested regions, rows
/// keyed by a list, a handler on an element inside two of them, an empty
/// element, runs of text beside each other, a run of nothing but a space, text
/// carrying the three characters markup is made of, a region that reads the
/// address bar, and a chunk fetched after the resume.
///
/// One `main.buri`, two entries, and one `page` both of them call — which is
/// what makes the markup match, and is the shape the guide teaches.
const RESUME_EDGES_PAGE: &str = r#"
from "core/effect" import { Allocator, Request, Response, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { FromJson, ToJson };
from "core/lazy" import * as lazy;
from "core/net/http" import * as http;
from "core/str" import * as str;
from "ui/effect" import { Event, Location, Ui, Watch };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/prop" import { Prop };
from "ui/signal" import { signal };
from "ui/web" import * as web;

derive Equal, FromJson, ToJson for Row;
/// One row of the list, keyed by `key`.
struct Row {
    key: Str,
    label: Str,
}

derive FromJson, ToJson for Sent;
/// What the worker rendered from, and what it sends with the document.
struct Sent {
    title: Str,
    gap: Str,
    picture: Str,
    twice: Bool,
    rows: [Row],
}

fn sent(): Sent {
    Sent {
        title: "Tom & Jerry <br> ✓",
        gap: " ",
        picture: "/cat.png",
        twice: false,
        rows: [Row { key: "a", label: "alpha" }, Row { key: "b", label: "beta" }],
    }
}

/// What a page with no state to read builds from.
fn blank(): Sent {
    let none: [Row] = [];
    Sent { title: "no state", gap: "", picture: "", twice: false, rows: none }
}

/// The whole site, as one function of the path and the state.
fn page<C>(
    path: Prop<Str>,
    state: Sent,
    label: Prop<Str>,
    onPress: fn(C, Event) => (),
): Node<C> {
    ui.region(
        .Main,
        [],
        [
            ui.heading(1, .Const(state.title)),
            // Two runs of text side by side, and a third holding one space: a
            // browser parses all three into one node.
            ui.text(.Const("one")),
            ui.text(.Const("two")),
            ui.text(.Const(state.gap)),
            ui.image(.Const(state.picture), .Const("a cat")),
            ui.region(
                .Complementary,
                [],
                [
                    ui.region(
                        .List,
                        [],
                        [
                            ui.each(
                                .Const(state.rows),
                                fn(row) => row.key,
                                fn(_c, row, _i) => {
                                    ui.region(.ListItem, [], [ui.text(.Const(row.label))])
                                },
                            ),
                        ],
                    ),
                    ui.button(label, onPress),
                ],
            ),
            ui.computed(fn(scope) => at(path.read(scope))),
        ],
    )
}

/// Routing: an ordinary match on the path.
fn at<C>(path: Str): Node<C> {
    match (path) {
        "/" => ui.region(.Article, [], [ui.text(.Const("home"))]),
        "/about" => {
            ui.region(.Article, [], [ui.heading(2, .Const("About")), ui.text(.Const("about"))])
        },
        _other => ui.region(.Article, [], [ui.text(.Const("nowhere"))]),
    }
}

/// The label after a press. A function rather than a lambda because it is
/// handed to `update`, which takes what it answers.
fn nextLabel(old: Str): Str {
    match (old) {
        "press me" => "pressed once",
        _other => "pressed twice",
    }
}

/// Reached from the `load` and from nowhere else, so it ships in a chunk of its
/// own — and the worker, which never reaches it, has no chunk at all.
fn summary<C: Allocator>(ctx: C, rows: Int): Str {
    str.format(ctx, "ONLY_IN_THE_CHUNK over ${rows} rows")
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        Ui: host.ui,
        Watch: host.watch,
        Location: host.location,
    };
    // The state the worker sent, read *before* anything is built: the tree the
    // page resumes with is the tree the worker rendered, and this is what it
    // was rendered from.
    let before = web.state(ctx).withDefault("none");
    let _ = io.println(ctx, "before ${before}").ignore();
    let state = json
        .decode(ctx, json.parse(ctx, before).withDefault(.Null))
        .withDefault(blank());
    let label = signal(ctx, "press me");
    let tree = page(
        web.route(ctx),
        state,
        .Cell(label),
        fn(c, _event) => label.update(c, nextLabel),
    );
    match (web.resume(ctx, tree)) {
        .Err(why) => .Err(why),
        .Ok(_) => {
            // A second resume, where the state asks for one: the markup in
            // front of it is markup a resume has already taken over.
            let twice = if (state.twice) {
                match (web.resume(ctx, tree)) {
                    .Err(why) => why,
                    .Ok(_) => "the second resume answered .Ok",
                }
            } else {
                "once"
            };
            // And again after it: a resume reads the document, and does not
            // consume what it read.
            let after = web.state(ctx).withDefault("none");
            let _ = io.println(ctx, "after ${after}").ignore();
            let _ = io.println(ctx, "at ${web.path(ctx)} ${twice}").ignore();
            let open = lazy.load(summary);
            match (io.println(ctx, open(ctx, state.rows.length()))) {
                .Ok(_written) => .Ok(()),
                .Err(_e) => .Err("the page has nowhere to print"),
            }
        },
    }
}

export fn fetch(request: Request): Response {
    let ctx = context {
        Allocator: host.alloc,
    };
    let state = sent();
    http.html(
        ctx,
        web.shell(
            ctx,
            request.path(),
            web.render(
                page(.Const(request.path()), state, .Const("press me"), fn(_c, _event) => ()),
            ),
            state.toJson(ctx),
        ),
    )
}
"#;

/// The browser's half of
/// [`a_resumed_page_takes_the_markup_a_browser_would_have_handed_it`]: the
/// worker's document, said a different way per scenario, and the page imported
/// on top of it.
///
/// Every perturbation is a `change`, which fails loudly when the text it is
/// looking for is not there — so a scenario cannot quietly stop perturbing
/// anything when the page it is written against moves.
const RESUME_EDGES_DRIVER: &str = r##"
import worker from "./.buri/out/cloudflare-worker/cmd/edges/fetch.mjs";

const how = process.argv[2];
const at = process.argv[3];

// The address bar is not always the address the worker answered.
// `wrong-address` and `no-path` are the page resumed where the server did not
// render; `right-address` is the same route reached with a query string and a
// fragment on it, neither of which is part of a path.
const elsewhere = how === "wrong-address" || how === "no-path";
const address = elsewhere ? "/about" : at;
const asked = how === "right-address" ? at + "?ref=x#top" : at;
const answer = await worker.fetch(new Request("https://example.com" + asked));
const document_ = await answer.text();
let sent = document_.split("<body>")[1].split("</body>")[0];
console.log(`sent ${sent.split("<script")[0]}`);

const change = (from, to) => {
  if (!sent.includes(from)) throw new Error(`the markup holds no ${from}`);
  sent = sent.split(from).join(to);
};

// Markup a browser would have handed the page, said the way a browser may say
// it rather than the way the worker wrote it.
if (how === "boolean") change("<main>", "<main hidden>");
if (how === "open-void") change('alt="a cat" />', 'alt="a cat">');
if (how === "numeric-entity") change("&amp;", "&#38;");
if (how === "upper-case") change("<article>home</article>", "<ARTICLE>home</ARTICLE>");

// Markup the tree does not match.
if (how === "missing") change('<img src="/cat.png" alt="a cat" />', "");
if (how === "renamed") change("<h1>", "<h2>"), change("</h1>", "</h2>");
if (how === "extra") change("<li>beta</li>", "<li>beta</li><li>gamma</li>");
if (how === "changed-text") change("<li>alpha</li>", "<li>ALPHA</li>");
if (how === "changed-attribute") change('src="/cat.png"', 'src="/dog.png"');

// A page the state asks to resume twice.
if (how === "twice") change('"twice":false', '"twice":true');

// Markup no `shell` wrote: it carries no path, so only the shape is checked.
if (how === "no-path") change(' data-path="/"', "");

// Nowhere to resume, the two ways there are: a document with no body, and a
// body the server wrote nothing into.
if (how === "blank") sent = "";

browser(sent, address, how !== "no-body");
if (how === "right-address") {
  globalThis.location.search = "?ref=x";
  globalThis.location.hash = "#top";
}

const title = findFirst(body, "H1");
await import("./.buri/out/web/cmd/edges/main.mjs");

console.log(`made ${made.elements} elements and ${made.text} runs of text`);
console.log(`showing ${showing()}`);

// A handler the server could not have attached, on an element two regions in.
press(findFirst(body, "BUTTON"));
console.log(`pressed ${showing()}`);

// The reader navigates. Only what read the address bar may be rebuilt, which is
// one element and one run of text — and the title is still the very node the
// server wrote.
made.elements = 0;
made.text = 0;
navigate("/about");
console.log(`navigated ${made.elements} elements and ${made.text} runs of text`);
console.log(`showing ${showing()}`);
console.log(`the title is the node the server wrote: ${findFirst(body, "H1") === title}`);

// And the button still works, because navigating rebuilt no part of it.
press(findFirst(body, "BUTTON"));
console.log(`pressed ${showing()}`);
"##;

/// **A page spawns after `main` returned**, and a worker spawns inside its
/// `fetch` — the two platforms `core/tasks` reached when `Tasks` was granted
/// everywhere.
///
/// The conformance corpus says what a scope promises and says it on the
/// reference backend; the repository case beside it says what a person gets
/// from `buri test`. Neither can ask the question a page asks, because a page's
/// interesting spawn happens *after* `main` has returned — from a handler the
/// mounted tree is holding — and no `buri` command runs a page. So this is the
/// platform's own side of it, driven from one JavaScript module: a document
/// double the tree mounts into, a click fired at the button it registered, and
/// the worker's `fetch` called the way a worker runtime calls one.
///
/// Five claims, one printed line each:
///
///  * the worker's scope waits for the task it spawned, and answers after it;
///  * the page mounts and `main` returns, with nothing spawned yet;
///  * a click spawns into the `Scope` the handler captured, and the task runs —
///    which is the shape `design/native/DECISIONS.md`'s "a scope stays open for
///    the life of the program" row is about;
///  * and the timer inside that task **waited**: the wall clock moved by at
///    least the sleep, so `clock.sleepMilliseconds` is a wait on a page rather than a
///    number the runtime pretends to have honoured.
#[test]
fn a_page_spawns_from_a_handler_after_main_returned() {
    let scratch = Scratch::repo("web-spawn");
    scratch.write(
        "cmd/page/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: WEB, entry: \"main\" },\n        \
         { platform: CLOUDFLARE_WORKER, entry: \"fetch\" },\n    ]\n}\n",
    );
    scratch.write(
        "cmd/page/main.buri",
        r#"
from "core/bytes" import * as bytes;
from "core/effect" import { Allocator, Clock, Request, Response, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/tasks" import * as tasks;
from "core/tasks" import { Scope };
from "core/time" import * as time;
from "ui/effect" import { Ui, Watch };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import * as signal;
from "ui/signal" import { Signal };

/// The page: a button that opens a socket later, and the line the socket
/// writes when it does.
///
/// The handler captures the scope and the cell, which is what `Scope` being
/// inert buys — neither carries a context, so a lambda may hold both.
fn page<C: Allocator + Clock + Tasks + Ui>(ctx: C, here: Scope, status: Signal<Str>): Node<C> {
    ui.column([], [
        ui.button(.Const("open"), fn(c, event) => {
            let _ = tasks.spawn(c, here, fn(c2) => {
                let _ = time.sleep(c2, time.milliseconds(20));
                let _ = status.set(c2, "the socket opened");
                ()
            });
            ()
        }),
        ui.text(.Cell(status)),
    ])
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        Stdout: host.stdout,
        Tasks: host.tasks,
        Ui: host.ui,
        Watch: host.watch,
    };
    let status = signal.signal(ctx, "waiting");
    tasks.scope(ctx, fn(c, here) => {
        let _ = io.println(c, "mounted").ignore();
        ui.mount(c, page(c, here, status), [])
    })
}

/// The worker's half: one request, answered after the work spawned into the
/// scope has finished.
///
/// The line is written as bytes rather than printed, because a worker's
/// process does not end when `fetch` does and a buffered line would still be
/// waiting when the platform read the answer.
export fn fetch(request: Request): Response {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let _ = tasks.scope(ctx, fn(c, here) => {
        let _ = tasks.spawn(c, here, fn(c2) => {
            let _ = time.sleep(c2, time.milliseconds(20));
            let _ = io.writeBytes(c2, bytes.toUtf8(c2, "the worker's task ran\n")).ignore();
            ()
        });
        ()
    });
    http.text(ctx, "served")
}
"#,
    );
    scratch.run(&["build", "//cmd/page"]).ok();

    // The document is the browser's half, and the smallest one the renderer
    // works against: three constructors, the four moves a tree makes, and a
    // place to hang a listener. `$shim` is deliberately absent — this is the
    // real-document path, the one a browser runs.
    let driver = scratch.write(
        "drive.mjs",
        r##"
class Element_ {
  constructor(name) {
    this.nodeName = name;
    this.childNodes = [];
    this.parentNode = null;
    this.listeners = {};
    this.attributes = {};
    this.className = "";
    this.data = "";
    this.textContent = "";
    this.style = { cssText: "", setProperty() {} };
  }
  get nextSibling() {
    const parent = this.parentNode;
    if (parent === null) return null;
    const at = parent.childNodes.indexOf(this);
    return at < 0 || at + 1 >= parent.childNodes.length ? null : parent.childNodes[at + 1];
  }
  setAttribute(name, value) {
    this.attributes[name] = value;
  }
  addEventListener(type, handler) {
    this.listeners[type] = handler;
  }
  appendChild(node) {
    return this.insertBefore(node, null);
  }
  insertBefore(node, before) {
    if (node.parentNode !== null) node.parentNode.removeChild(node);
    node.parentNode = this;
    const at =
      before === null || before === undefined
        ? this.childNodes.length
        : this.childNodes.indexOf(before);
    this.childNodes.splice(at, 0, node);
    return node;
  }
  removeChild(node) {
    const at = this.childNodes.indexOf(node);
    if (at >= 0) this.childNodes.splice(at, 1);
    node.parentNode = null;
    return node;
  }
}

const text_ = (node) =>
  node.childNodes.length === 0 ? node.data : node.childNodes.map(text_).join("");
const find_ = (node, wanted) => {
  if (node.listeners[wanted] !== undefined) return node;
  for (const child of node.childNodes) {
    const hit = find_(child, wanted);
    if (hit !== null) return hit;
  }
  return null;
};

globalThis.document = {
  body: new Element_("body"),
  head: new Element_("head"),
  createElement: (name) => new Element_(name),
  createTextNode: (data) => Object.assign(new Element_("#text"), { data }),
  createComment: () => new Element_("#comment"),
  getElementById: () => null,
};

// The worker first, called the way its platform calls it.
const worker = await import("./.buri/out/cloudflare-worker/cmd/page/fetch.mjs");
const answer = await worker.default.fetch(new Request("https://example.com/"));
console.log(`${answer.status} ${await answer.text()}`);

// Then the page, on top of the document above. `main` mounts and returns.
await import("./.buri/out/web/cmd/page/main.mjs");
console.log(`before the click: ${text_(document.body)}`);

// The click. Nothing waits for the handler — a listener answers at once and
// the task it spawned is the event loop's, which is the whole claim.
const started = Date.now();
find_(document.body, "click").listeners.click({});

// Polled rather than slept for: the wait ends when the page says so, and the
// deadline is there to fail rather than to hang.
const deadline = started + 30000;
while (text_(document.body).indexOf("the socket opened") < 0 && Date.now() < deadline) {
  await new Promise((wake) => setTimeout(wake, 5));
}
console.log(`after the click: ${text_(document.body)}`);
console.log(`the timer waited: ${Date.now() - started >= 20}`);
"##,
    );

    let out = Command::new(js_runtime())
        .arg(&driver)
        .output()
        .expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the page did not answer:\n{stdout}{stderr}");
    assert_eq!(
        stdout,
        "the worker's task ran\n\
         200 served\n\
         mounted\n\
         before the click: openwaiting\n\
         after the click: openthe socket opened\n\
         the timer waited: true\n",
        "a spawn on a page or in a worker did not run:\n{stderr}"
    );
}
