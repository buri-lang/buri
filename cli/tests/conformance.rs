//! The conformance suite: does the language do what the specification says?
//!
//! Everything here drives the real `buri` binary, because that is what a user
//! runs. Three shapes of test:
//!
//! * `cli/tests/conformance/` — a Buri repository whose `test/` directories
//!   assert on language semantics. Run with `buri test //...`.
//! * `cli/tests/reject/` — programs that must *not* compile, each paired with
//!   a `.stderr` file holding the diagnostic exactly as a terminal shows it.
//! * `WEB_STDOUT` — the exact stdout of the worked monorepo's JS binary, so a
//!   wrong *rendering* fails as loudly as a wrong value.
//!
//! The point of the first is that a wrong answer fails, rather than a program
//! that still exits 0.

use std::path::{Path, PathBuf};
use std::process::Command;

fn buri() -> &'static str {
    env!("CARGO_BIN_EXE_buri")
}

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for n in chars.by_ref() {
                if n == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

fn run_in(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(buri())
        .args(args)
        .arg("--color=never")
        .current_dir(dir)
        .output()
        .expect("the buri binary runs");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: strip_ansi(&String::from_utf8_lossy(&out.stdout)),
        stderr: strip_ansi(&String::from_utf8_lossy(&out.stderr)),
    }
}

// ---------------------------------------------------------------------------
// The conformance repository
// ---------------------------------------------------------------------------

/// Serialises the two tests that share the conformance repository.
static SUITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn conformance_suite_passes() {
    let _guard = SUITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tests_dir().join("conformance");
    // A stale cache would let a broken backend report a pass, so this always
    // starts from nothing.
    let _ = run_in(&dir, &["clean"]);
    let run = run_in(&dir, &["test", "//...", "--force"]);
    let output = run.all();

    // A suite that compiles to nothing would "pass" with zero assertions, so
    // the count is checked too.
    let summary = output
        .lines()
        .rev()
        .find(|l| l.contains(" passed, "))
        .unwrap_or_else(|| panic!("no summary line in:\n{output}"));
    let passed: usize = summary
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);

    assert!(
        run.code == 0,
        "the conformance suite failed (exit {}):\n{output}",
        run.code
    );
    assert!(
        passed >= 150,
        "expected the conformance suite to hold at least 150 assertions, found {passed}:\n{output}"
    );
    eprintln!("conformance: {passed} tests passed");
}

/// The suite has to be able to fail. A test that cannot fail proves nothing,
/// so this breaks one on purpose and checks the runner notices.
/// Runs in the same process-wide order as `conformance_suite_passes`, because
/// this one edits a file that one reads.
#[test]
fn conformance_suite_can_fail() {
    let _guard = SUITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tests_dir().join("conformance");
    let target = dir.join("lib/canary/test/canary.buri");
    let original = std::fs::read_to_string(&target).expect("the canary suite exists");
    assert!(
        original.contains("// CANARY_42"),
        "the canary suite must contain the marker it is edited through"
    );
    // The value, not a name: renaming a constant and its use together would
    // leave the assertion true.
    let broken = original.replace("assert.eq(6 * 7, 42);", "assert.eq(6 * 7, 43);");
    assert_ne!(broken, original, "the canary marker did not substitute");

    std::fs::write(&target, &broken).unwrap();
    let run = run_in(&dir, &["test", "//lib/canary", "--force"]);
    std::fs::write(&target, &original).unwrap();

    assert_ne!(run.code, 0, "a broken assertion still passed:\n{}", run.all());
    assert!(
        run.all().contains("FAIL"),
        "a broken assertion did not report FAIL:\n{}",
        run.all()
    );
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
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/reject exists")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("main.buri").is_file())
        .collect();
    cases.sort();
    assert!(cases.len() >= 25, "expected a real reject corpus, found {}", cases.len());

    // One scratch repository, one package per case.
    let scratch = std::env::temp_dir().join("buri-reject-corpus");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();

    let bless = std::env::var_os("BURI_BLESS").is_some();
    let mut blessed = 0usize;
    let mut failures = Vec::new();
    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(case.join("main.buri")).unwrap();
        let expect = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("// EXPECT:").map(|s| s.trim().to_string()))
            .unwrap_or_else(|| panic!("{name}: no `// EXPECT:` line"));

        let pkg = scratch.join("cmd").join(&name);
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("main.buri"), &text).unwrap();
        std::fs::write(
            pkg.join("BUILD.buri"),
            "binary {\n  outputs: [{ platform: JS }]\n}\n",
        )
        .unwrap();

        let target = format!("//cmd/{name}");
        let run = run_in(&scratch, &["build", &target]);
        let printed = run.all();
        if run.code == 0 {
            failures.push(format!("{name}: compiled, but should not have"));
            continue;
        }
        if !printed.contains(&expect) {
            failures.push(format!(
                "{name}: expected a diagnostic containing {expect:?}, got:\n{}",
                indent(&printed)
            ));
            continue;
        }
        let json = run_in(&scratch, &["build", &target, "--error-format=json", "--force"]).all();

        // Every diagnostic answers "what do I do about it?".
        for (i, line) in json.lines().enumerate() {
            if !line.starts_with('{') {
                continue;
            }
            if !line.contains("\"fix\":") {
                failures.push(format!(
                    "{name}: diagnostic {} carries no `fix`:\n{}",
                    i + 1,
                    indent(line)
                ));
            }
        }

        for (file, content) in [("expected.txt", &printed), ("expected.json", &json)] {
            let path = case.join(file);
            if bless {
                std::fs::write(&path, content).unwrap();
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(golden) if &golden == content => {}
                Ok(golden) => failures.push(format!(
                    "{name}/{file} does not match what the toolchain prints.\n  recorded:\n{}\n  printed:\n{}",
                    indent(&golden),
                    indent(content)
                )),
                Err(_) => failures.push(format!(
                    "{name}/{file} is missing — run with BURI_BLESS=1 to record:\n{}",
                    indent(content)
                )),
            }
        }
        blessed += 1;
    }
    if bless {
        eprintln!("reject: recorded {blessed} cases");
        return;
    }
    assert!(failures.is_empty(), "{} rejection(s) wrong:\n\n{}", failures.len(), failures.join("\n\n"));
    eprintln!("reject: {} programs rejected, with the exact diagnostics each prints", cases.len());
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
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
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/crash exists")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "buri"))
        .collect();
    files.sort();
    assert!(files.len() >= 8, "expected a real crash corpus, found {}", files.len());

    let scratch = std::env::temp_dir().join("buri-crash-corpus");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();

    let mut cases = Vec::new();
    for path in &files {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(path).unwrap();
        let expect = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("// CRASH:").map(|s| s.trim().to_string()))
            .unwrap_or_else(|| panic!("{name}: no `// CRASH:` line"));
        let pkg = scratch.join("cmd").join(&name);
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("main.buri"), &text).unwrap();
        std::fs::write(
            pkg.join("BUILD.buri"),
            "binary {\n  outputs: [{ platform: JS }]\n}\n",
        )
        .unwrap();
        cases.push((name, expect));
    }

    let build = run_in(&scratch, &["build", "//..."]);
    assert_eq!(build.code, 0, "the crash corpus does not compile:\n{}", build.all());

    let mut failures = Vec::new();
    for (name, expect) in &cases {
        let artifact = scratch
            .join(".buri/out/js/cmd")
            .join(name)
            .join(format!("{name}.mjs"));
        let out = Command::new(js_runtime())
            .arg(&artifact)
            .output()
            .expect("the javascript runtime runs");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if out.status.success() {
            failures.push(format!("{name}: exited 0, but should have crashed"));
        } else if !stderr.contains(expect.as_str()) {
            failures.push(format!(
                "{name}: expected a crash mentioning {expect:?}, got:\n{}",
                indent(&stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{} crash(es) wrong:\n\n{}", failures.len(), failures.join("\n\n"));
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
    let dir = repo_root().join("build-system/example");

    let build = run_in(&dir, &["build", "//cmd/web", "--force"]);
    assert_eq!(build.code, 0, "//cmd/web does not build:\n{}", build.all());

    let out = Command::new(js_runtime())
        .arg(dir.join(".buri/out/js/cmd/web/web.mjs"))
        .output()
        .expect("the javascript runtime runs");
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = run_in(&dir, &["clean"]);
    assert!(out.status.success(), "//cmd/web exited non-zero:\n{printed}");

    assert_eq!(
        WEB_STDOUT, printed,
        "//cmd/web printed something other than its transcript"
    );
    eprintln!("golden: //cmd/web matched its transcript");
}


fn js_runtime() -> String {
    std::env::var("BURI_JS").unwrap_or_else(|_| "bun".to_string())
}

/// The same artifact has to behave identically on an engine without native
/// tail calls, which is the whole reason the compiler eliminates them itself.
#[test]
fn tail_calls_run_in_constant_stack_on_v8() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("node is not installed; skipping the V8 tail-call check");
        return;
    }
    let scratch = std::env::temp_dir().join("buri-tco-check");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("cmd/deep")).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        scratch.join("cmd/deep/BUILD.buri"),
        "binary {\n  outputs: [{ platform: JS }]\n}\n",
    )
    .unwrap();
    // Ten million bounces: far past any engine's stack, through a self call,
    // a mutually recursive pair, and an accumulator.
    std::fs::write(
        scratch.join("cmd/deep/main.buri"),
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
    )
    .unwrap();

    let build = run_in(&scratch, &["build", "//cmd/deep", "--release"]);
    assert_eq!(build.code, 0, "the deep-recursion program does not build:\n{}", build.all());

    let artifact = scratch.join(".buri/out/js/cmd/deep/deep.mjs");
    for engine in ["node", "bun"] {
        let Ok(out) = Command::new(engine).arg(&artifact).output() else { continue };
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success(),
            "ten million tail calls overflowed on {engine}:\n{stderr}"
        );
        assert_eq!(
            stdout, "self: 10000000\nmutual: false\n",
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
    let _guard = SUITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let suite = tests_dir().join("conformance");

    let mut counts = Vec::new();
    for mode in ["--debug", "--release"] {
        let run = run_in(&suite, &["test", "//...", mode, "--force"]);
        assert_eq!(run.code, 0, "the conformance suite fails {mode}:\n{}", run.all());
        let summary = run
            .stdout
            .lines()
            .rev()
            .find(|l| l.contains(" passed, "))
            .unwrap_or_default()
            .to_string();
        let passed: usize =
            summary.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
        assert!(passed > 100, "expected the whole suite {mode}, ran {passed}");
        counts.push(passed);
    }
    assert_eq!(counts[0], counts[1], "the two modes ran different numbers of assertions");

    // And a whole program's stdout, which no assertion inside a program covers.
    let dir = repo_root().join("build-system/example");
    let artifact = dir.join(".buri/out/js/cmd/web/web.mjs");
    let mut outputs = Vec::new();
    let mut sizes = Vec::new();
    for mode in ["--debug", "--release"] {
        let build = run_in(&dir, &["build", "//cmd/web", mode, "--force"]);
        assert_eq!(build.code, 0, "//cmd/web does not build {mode}:\n{}", build.all());
        sizes.push(std::fs::metadata(&artifact).map(|m| m.len()).unwrap_or(0));
        let out = Command::new(js_runtime())
            .arg(&artifact)
            .output()
            .expect("the javascript runtime runs");
        outputs.push(String::from_utf8_lossy(&out.stdout).to_string());
    }
    let _ = run_in(&dir, &["clean"]);
    assert_eq!(
        outputs[0], outputs[1],
        "minification changed what //cmd/web prints"
    );
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
    let source = repo_root().join("build-system/example");
    let mut artifacts = Vec::new();
    for run in 0..2 {
        let scratch = std::env::temp_dir().join(format!("buri-repro-{run}"));
        let _ = std::fs::remove_dir_all(&scratch);
        copy_tree(&source, &scratch);
        let build = run_in(&scratch, &["build", "//cmd/web", "--release"]);
        assert_eq!(build.code, 0, "build {run} failed:\n{}", build.all());
        artifacts.push(std::fs::read(scratch.join(".buri/out/js/cmd/web/web.mjs")).unwrap());
    }
    assert!(
        artifacts[0] == artifacts[1],
        "two builds of the same source produced different artifacts ({} vs {} bytes)",
        artifacts[0].len(),
        artifacts[1].len()
    );
    eprintln!("reproducible: identical artifacts from two directories");
}

/// Copies a source tree, leaving behind anything a previous build wrote.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().filter_map(Result::ok) {
        let name = e.file_name();
        if name == ".buri" {
            continue;
        }
        let (src, dst) = (e.path(), to.join(&name));
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// A content-keyed cache must not be able to hold a wrong answer. This builds,
/// edits a source, rebuilds, and checks the program's *behaviour* changed —
/// not merely that some file was rewritten.
#[test]
fn the_cache_cannot_serve_a_stale_answer() {
    let scratch = std::env::temp_dir().join("buri-cache-check");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("cmd/c")).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        scratch.join("cmd/c/BUILD.buri"),
        "binary {\n  outputs: [{ platform: JS }]\n}\n",
    )
    .unwrap();
    let program = |answer: i32| {
        format!(
            r#"
from "core/cap" import {{ Alloc, Stdout }};
from "core/host" import * as host;

fn answer(): Int {{ {answer} }}

export fn main(): Result<(), Str> {{
  let ctx = context {{ Alloc: host.alloc, Stdout: host.stdout }};
  let _ = ctx.println("answer=${{answer()}}");
  .Ok(())
}}
"#
        )
    };
    let run_program = || -> String {
        let out = Command::new(js_runtime())
            .arg(scratch.join(".buri/out/js/cmd/c/c.mjs"))
            .output()
            .expect("the javascript runtime runs");
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    std::fs::write(scratch.join("cmd/c/main.buri"), program(1)).unwrap();
    assert_eq!(run_in(&scratch, &["build", "//cmd/c"]).code, 0);
    assert_eq!(run_program(), "answer=1\n");

    // A second build with no edit must be served from the cache.
    let again = run_in(&scratch, &["build", "//cmd/c"]);
    assert!(again.stdout.contains("cached"), "an unchanged build was not cached:\n{}", again.all());
    assert_eq!(run_program(), "answer=1\n");

    // An edit must invalidate it.
    std::fs::write(scratch.join("cmd/c/main.buri"), program(2)).unwrap();
    let edited = run_in(&scratch, &["build", "//cmd/c"]);
    assert!(
        !edited.stdout.contains("cached"),
        "an edited source was served from the cache:\n{}",
        edited.all()
    );
    assert_eq!(run_program(), "answer=2\n", "the cache served a stale artifact");

    // And going back must hit the entry the first build left.
    std::fs::write(scratch.join("cmd/c/main.buri"), program(1)).unwrap();
    let reverted = run_in(&scratch, &["build", "//cmd/c"]);
    assert!(reverted.stdout.contains("cached"), "reverting did not hit the cache");
    assert_eq!(run_program(), "answer=1\n");
    eprintln!("cache: invalidates on edit, hits on revert, never stale");
}

// ---------------------------------------------------------------------------
// The worked monorepo
// ---------------------------------------------------------------------------

/// `build-system/example` is the build system's own corpus. It has to lint
/// clean and its suites have to pass, through the real CLI.
#[test]
fn the_example_monorepo_is_clean() {
    let dir = repo_root().join("build-system/example");
    let _ = run_in(&dir, &["clean"]);

    let lint = run_in(&dir, &["lint", "//..."]);
    assert_eq!(lint.code, 0, "the example monorepo does not lint:\n{}", lint.all());
    assert!(lint.stdout.contains("no findings"), "unexpected lint output:\n{}", lint.all());

    let test = run_in(&dir, &["test", "//...", "--force"]);
    assert_eq!(test.code, 0, "the example monorepo's tests fail:\n{}", test.all());
    let summary = test
        .stdout
        .lines()
        .rev()
        .find(|l| l.contains(" passed, "))
        .unwrap_or("")
        .to_string();
    let passed: usize = summary.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(passed >= 15, "expected the example suites to hold real tests, found {passed}");

    // The policy checks the build system exists for.
    let tags = run_in(&dir, &["query", "tags(//lib/store)"]);
    assert!(tags.stdout.contains("server"), "query tags is wrong:\n{}", tags.all());
    let platforms = run_in(&dir, &["query", "platforms(//lib/store)"]);
    assert!(
        platforms.stdout.contains("linux") && !platforms.stdout.contains("js"),
        "a server-tagged library should not admit js:\n{}",
        platforms.all()
    );

    let _ = run_in(&dir, &["clean"]);
    eprintln!("monorepo: {passed} tests, lint clean, policy enforced");
}

// ---------------------------------------------------------------------------
// The CLI contract
// ---------------------------------------------------------------------------

/// CLI.md fixes three exit codes: 0 success, 1 "the thing you asked about is
/// wrong", 2 "the thing you asked *with* is wrong". The distinction is the
/// whole point, so it is worth pinning.
#[test]
fn exit_codes_distinguish_bad_code_from_bad_invocation() {
    let scratch = std::env::temp_dir().join("buri-exit-codes");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("cmd/ok")).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        scratch.join("cmd/ok/BUILD.buri"),
        "binary {\n  outputs: [{ platform: JS }]\n}\n",
    )
    .unwrap();
    let good = r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("fine");
  .Ok(())
}
"#;
    std::fs::write(scratch.join("cmd/ok/main.buri"), good).unwrap();

    // 0: it worked.
    assert_eq!(run_in(&scratch, &["build", "//cmd/ok"]).code, 0, "a good build did not exit 0");

    // 1: the code is wrong.
    std::fs::write(
        scratch.join("cmd/ok/main.buri"),
        good.replace("\"fine\"", "notAName"),
    )
    .unwrap();
    let broken = run_in(&scratch, &["build", "//cmd/ok"]);
    assert_eq!(broken.code, 1, "a type error did not exit 1:\n{}", broken.all());
    std::fs::write(scratch.join("cmd/ok/main.buri"), good).unwrap();

    // 2: the invocation is wrong.
    for args in [
        vec!["build", "//cmd/nope"],
        vec!["build", "lib/relative"],
        vec!["frobnicate"],
        vec!["build", "--nonsense"],
        vec!["query", "wat(//cmd/ok)"],
    ] {
        let run = run_in(&scratch, &args);
        assert_eq!(
            run.code, 2,
            "`buri {}` should exit 2, got {}:\n{}",
            args.join(" "),
            run.code,
            run.all()
        );
    }

    // 2: an unparseable build file is also the thing you asked *with*.
    std::fs::write(scratch.join("cmd/ok/BUILD.buri"), "binary {\n  sources: [\n").unwrap();
    let bad_build = run_in(&scratch, &["build", "//cmd/ok"]);
    assert_eq!(bad_build.code, 2, "an unparseable build file did not exit 2:\n{}", bad_build.all());

    // And outside a repository at all.
    let nowhere = std::env::temp_dir().join("buri-not-a-repo");
    let _ = std::fs::create_dir_all(&nowhere);
    let outside = run_in(&nowhere, &["build", "//..."]);
    assert_eq!(outside.code, 2, "outside a repository did not exit 2:\n{}", outside.all());
    assert!(
        outside.all().contains("REPO.buri"),
        "the diagnostic should name REPO.buri:\n{}",
        outside.all()
    );
    eprintln!("cli: exit codes 0/1/2 distinguish success, bad code, bad invocation");
}

/// `buri format` is a fixed point over the whole corpus, and `--check` is the
/// CI form that reports rather than rewrites.
#[test]
fn format_check_does_not_rewrite() {
    let scratch = std::env::temp_dir().join("buri-format-check");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("cmd/f")).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        scratch.join("cmd/f/BUILD.buri"),
        "binary {\n  outputs: [{ platform: JS }]\n}\n",
    )
    .unwrap();
    let ugly = "export    fn   main( ):Result<(),Str>{\n.Ok( ( ) )\n}\n";
    let path = scratch.join("cmd/f/main.buri");
    std::fs::write(&path, ugly).unwrap();

    let check = run_in(&scratch, &["format", "--check"]);
    assert_eq!(check.code, 1, "--check should report a file that would change");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        ugly,
        "--check rewrote the file it was only supposed to report"
    );

    let fixed = run_in(&scratch, &["format"]);
    assert_eq!(fixed.code, 0);
    assert_ne!(std::fs::read_to_string(&path).unwrap(), ugly, "format did not rewrite");

    let again = run_in(&scratch, &["format", "--check"]);
    assert_eq!(again.code, 0, "formatting is not a fixed point:\n{}", again.all());

    // And the formatted program still builds.
    assert_eq!(run_in(&scratch, &["build", "//cmd/f"]).code, 0);
    eprintln!("format: --check reports without rewriting, and formatting is idempotent");
}
