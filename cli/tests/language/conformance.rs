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
/// several functions whose `Alloc` bound the body never exercises — `readText`
/// is `ctx.readFile(path)`, which is an `FsRead` method and nothing else — so a
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

/// `fs.readText<C: Alloc + FsRead>` demands both of `read`'s bounds;
/// `fs.exists<C: FsRead>` demands one of `touch`'s. So exactly one finding is
/// this repository's, and the ones inside `core/fs` are nobody's.
///
/// Both take a `Path` rather than the text of one, which is what keeps
/// `touch`'s `Alloc` dead: making a `Path` allocates, so a version of this
/// that called `filepath.of` inside `touch` would be a repository with no dead
/// bound to report.
const FS_BOUNDS: &str = r#"from "core/effect" import { Alloc };
from "core/fs" import { FsRead, Path };
from "core/fs" import * as fs;
from "core/host" import * as host;
from "core/path" import * as filepath;

fn read<C: Alloc + FsRead>(ctx: C, at: Path): Bool {
  fs.readText(ctx, at).isOk()
}

fn touch<C: Alloc + FsRead>(ctx: C, at: Path): Bool {
  fs.exists(ctx, at)
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, FsRead: host.fs };
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
/// Three crossings in one run: a path routed on, a method the entry reads, and
/// a body it echoes. Each one is a field of the bridge, and a bridge that lost
/// one would still answer the other two.
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
from "core/effect" import { Alloc, Request, Response };
from "core/host" import * as host;
from "core/net/http" import * as http;
from "core/str" import * as str;

export fn fetch(request: Request): Response {
  let ctx = context { Alloc: host.alloc };
  let verb = match (request.method) {
    .Get => "GET",
    .Post => "POST",
    _ => "OTHER",
  };
  match (request.path()) {
    "/echo" => http.text(ctx, bodyOrExcuse(ctx, request)),
    other => http.text(ctx, str.format(ctx, "${verb} ${other}")),
  }
}

fn bodyOrExcuse<C: Alloc>(ctx: C, request: Request): Str {
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
  console.log(`${answer.status} ${answer.headers.get("content-type")} ${await answer.text()}`);
};

await say(new Request("https://example.com/about?ref=x#top"));
await say(new Request("https://example.com/echo", { method: "POST", body: "hello" }));
await say(new Request("https://example.com/", { method: "PUT", body: "ignored" }));
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
         200 text/plain; charset=utf-8 hello\n\
         200 text/plain; charset=utf-8 OTHER /\n",
        "the crossing lost a field:\n{stderr}"
    );
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
from "core/effect" import { Alloc, Sockets, Stdout, WebSocketClient };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "ui/effect" import { Ui };
from "ui/node" import * as ui;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
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

fn feed<C: Alloc + Sockets + Stdout + WebSocketClient>(): Client<C, Int> {
  Client {
    url: "wss://example.test/feed",
    onOpen: fn(c, socket, response) => {
      let _said = io.println(c, "page opened ${response.status}").ignore();
      let _sent = socket.send(c, .Text("subscribe"));
      0
    },
    onMessage: fn(c, _socket, seen, message) => {
      match (message) {
        .Text(text) => {
          let _said = io.println(c, "page heard ${text}").ignore();
          seen + 1
        },
        .Binary(_data) => seen,
      }
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
    setTimeout(() => this.onclose({ code: 1000, wasClean: true }), 60);
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
        "page opened 101",
        "page heard one",
        "page closed after 1 .Normal",
        "page ended .Normal",
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
from "core/effect" import { Alloc, Env, Sockets, Stdout, WebSocketClient };
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Env: host.env,
    Sockets: host.sockets,
    Stdout: host.stdout,
    WebSocketClient: host.websocketClient,
  };
  // Three calls rather than a `mapCtx`: a lambda that suspends is not a thing a
  // list combinator awaits today, and what this row is about is the answer each
  // dial gives.
  let ports = env.args(ctx);
  let _nobody = dialling(ctx, ports.get(0).withDefault("0"));
  let _not101 = dialling(ctx, ports.get(1).withDefault("0"));
  .Ok(())
}

/// One dial, and the one line it is worth. `errorText` is the constant per
/// variant, so what varies between the three is the cause and nothing else.
fn dialling<C: Alloc + Sockets + Stdout + WebSocketClient>(ctx: C, port: Str): () {
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
fn silent<C: Alloc + Sockets + Stdout + WebSocketClient>(url: Str): Client<C, Int> {
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

/// A worker that dials a socket while answering a request.
///
/// `WebSocketClient` and `Sockets` are granted on `CLOUDFLARE_WORKER` like every
/// other platform, and this is what that grant buys: a worker handed a request
/// dials somebody else's socket, reads what comes back, and answers with it.
///
/// The driver is the platform's half — the worker's default export, a real
/// `Request`, a real `Response` — with the same `WebSocket` double the page row
/// uses. A worker parks on the dial exactly as a page does, which is what
/// `$fetchEntry` awaiting the entry is for.
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
from "core/effect" import { Alloc, Request, Response, Sockets, WebSocketClient };
from "core/host" import * as host;
from "core/net/http" import * as http;
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };
from "core/str" import * as str;

export fn fetch(request: Request): Response {
  let ctx = context {
    Alloc: host.alloc,
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
fn relaying<C: Alloc + Sockets + WebSocketClient>(path: Str): Client<C, Int> {
  Client {
    url: "wss://example.test/relay",
    onOpen: fn(c, socket, _response) => {
      let _sent = socket.send(c, .Text(str.format(c, "asking ${path}")));
      0
    },
    onMessage: fn(c, socket, seen, message) => {
      match (message) {
        .Text(text) => {
          let _sent = socket.send(c, .Text(str.format(c, "heard ${text}")));
          seen + 1
        },
        .Binary(_data) => seen,
      }
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

const log = [];
let live = null;
globalThis.WebSocket = class {
  constructor(url) {
    log.push(`dialled ${url}`);
    this.protocol = "";
    this.extensions = "";
    live = this;
    // The whole exchange, on timers: the worker parks on each step, so nothing
    // here runs unless the event loop is free.
    setTimeout(() => live.onopen({}), 5);
    setTimeout(() => live.onmessage({ data: "pong" }), 15);
    setTimeout(() => live.onclose({ code: 1000, wasClean: true }), 25);
  }
  send(data) {
    log.push(`sent ${data}`);
  }
  close(code) {
    log.push(`closed ${code}`);
  }
};

const answer = await worker.fetch(new Request("https://example.com/rooms/9"));
console.log(`${answer.status} ${await answer.text()}`);
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
         dialled wss://example.test/relay\n\
         sent asking /rooms/9\n\
         sent heard pong\n",
        "a worker did not dial, or lost what it heard:\n{stderr}"
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
/// Three claims, and the driver prints one line for each so a wrong answer
/// names which:
///
///  * the worker answers HTML, with the tree rendered into it and the state it
///    rendered from beside it;
///  * the page picks that state up, and the address bar it is at;
///  * and it re-renders nothing — the markup the worker sent is the markup the
///    reader is still looking at, and the document was never touched.
#[test]
fn a_website_is_rendered_by_its_worker_and_resumed_by_its_page() {
    let site = tests_dir().join("repositories/concurrency/website/repo");
    let scratch = Scratch::copy_of("website", &site);
    scratch.run(&["build", "//cmd/site"]).ok();

    // The document double is the browser's half: a body that already holds the
    // worker's markup, the state script the worker embedded, and an address.
    // Every way of changing a document counts what it was asked to do, so
    // "nothing was re-rendered" is a number rather than an impression.
    let driver = scratch.write(
        "drive.mjs",
        r#"
import worker from "./.buri/out/cloudflare-worker/cmd/site/fetch.mjs";

const answer = await worker.fetch(new Request("https://example.com/"));
const document_ = await answer.text();
console.log(`${answer.status} ${answer.headers.get("content-type")}`);
console.log(document_.split("<body>")[1].split("</body>")[0]);

const body = document_.split("<body>")[1].split("</body>")[0];
const embedded = body.split('type="application/json">')[1].split("</script>")[0];

let touched = 0;
const touch = () => (touched++, {});
globalThis.document = {
  body: { markup: body, appendChild: touch, insertBefore: touch, removeChild: touch },
  head: { appendChild: touch },
  getElementById: (id) => (id === "buri-state" ? { textContent: embedded } : null),
  createElement: touch,
  createTextNode: touch,
  createComment: touch,
};
globalThis.location = { pathname: "/about" };

await import("./.buri/out/web/cmd/site/main.mjs");

console.log(`touched ${touched}`);
console.log(document.body.markup);
"#,
    );

    let out = Command::new(js_runtime())
        .arg(&driver)
        .output()
        .expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the website did not answer:\n{stdout}{stderr}");

    let sent = "<main><h1>Buri</h1>visitors: 3</main>\
                <script id=\"buri-state\" type=\"application/json\">\
                {\"title\":\"Buri\",\"visitors\":3}</script>";
    assert_eq!(
        stdout,
        format!(
            "200 text/html; charset=utf-8\n\
             {sent}\n\
             resumed /about {{\"title\":\"Buri\",\"visitors\":3}}\n\
             touched 0\n\
             {sent}\n"
        ),
        "the website lost a half:\n{stderr}"
    );
}
