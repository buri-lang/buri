//! The conformance suite: does the language do what the specification says?
//!
//! Everything here drives the real `buri` binary, because that is what a user
//! runs. Three shapes of test:
//!
//! * `cli/tests/conformance/` — a Buri repository whose `test/` directories
//!   assert on language semantics. Run with `buri test //...`.
//! * `cli/tests/reject/` — programs that must *not* compile, each annotated
//!   with the diagnostic it must produce.
//! * `cli/tests/golden/` — the expected stdout of every example program.
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

/// Each file in `tests/reject/` is a whole program that must fail to compile.
/// The first line is `// EXPECT: <substring the diagnostic must contain>`.
#[test]
fn rejected_programs_are_rejected() {
    let dir = tests_dir().join("reject");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/reject exists")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "buri"))
        .collect();
    files.sort();
    assert!(files.len() >= 25, "expected a real reject corpus, found {}", files.len());

    // One scratch repository, one package per case.
    let scratch = std::env::temp_dir().join("buri-reject-corpus");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();

    let mut failures = Vec::new();
    for path in &files {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(path).unwrap();
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

        let run = run_in(&scratch, &["build", &format!("//cmd/{name}")]);
        let output = run.all();
        if run.code == 0 {
            failures.push(format!("{name}: compiled, but should not have"));
        } else if !output.contains(&expect) {
            failures.push(format!(
                "{name}: expected a diagnostic containing {expect:?}, got:\n{}",
                indent(&output)
            ));
        }
    }
    assert!(failures.is_empty(), "{} rejection(s) wrong:\n\n{}", failures.len(), failures.join("\n\n"));
    eprintln!("reject: {} programs correctly rejected", files.len());
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

/// Every example program, compiled and run, with its stdout compared against a
/// checked-in transcript. This is what catches a backend that produces a
/// *different* answer rather than no answer.
#[test]
fn examples_produce_their_golden_output() {
    let root = repo_root();
    let golden_dir = tests_dir().join("golden");
    let scratch = std::env::temp_dir().join("buri-golden-corpus");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();

    let mut examples: Vec<PathBuf> = std::fs::read_dir(root.join("examples"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "buri"))
        .collect();
    examples.sort();
    assert_eq!(examples.len(), 22, "expected the 22 example programs");

    for path in &examples {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let pkg = scratch.join("cmd").join(&name);
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::copy(path, pkg.join("main.buri")).unwrap();
        std::fs::write(
            pkg.join("BUILD.buri"),
            "binary {\n  outputs: [{ platform: JS }]\n}\n",
        )
        .unwrap();
    }

    let build = run_in(&scratch, &["build", "//..."]);
    assert_eq!(build.code, 0, "the examples do not build:\n{}", build.all());

    let mut mismatches = Vec::new();
    for path in &examples {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let golden_path = golden_dir.join(format!("{name}.txt"));
        let Ok(expected) = std::fs::read_to_string(&golden_path) else {
            mismatches.push(format!("{name}: no golden file at {}", golden_path.display()));
            continue;
        };
        // Arguments and input files an example needs are named in the first
        // line of its golden file, as `# ARGS: ...`.
        let args: Vec<String> = expected
            .lines()
            .find_map(|l| l.strip_prefix("# ARGS:"))
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        // A line whose content genuinely varies between runs — a clock
        // reading, a roll of the real `Rand`, a network result — is declared
        // volatile and compared only up to its prefix. Everything else is
        // compared exactly.
        let volatile: Vec<String> = expected
            .lines()
            .filter_map(|l| l.strip_prefix("# VOLATILE:").map(|s| s.trim().to_string()))
            .collect();
        let body: String = expected
            .lines()
            .filter(|l| !l.starts_with("# ARGS:") && !l.starts_with("# VOLATILE:"))
            .map(|l| format!("{l}\n"))
            .collect();

        // A program that reads a file gets its fixtures from
        // `tests/golden/<name>.files/`, copied in beside it.
        let cwd = scratch.join("cmd").join(&name);
        let fixtures = golden_dir.join(format!("{name}.files"));
        if fixtures.is_dir() {
            for e in std::fs::read_dir(&fixtures).unwrap().filter_map(Result::ok) {
                let _ = std::fs::copy(e.path(), cwd.join(e.file_name()));
            }
        }
        let artifact = scratch
            .join(".buri/out/js/cmd")
            .join(&name)
            .join(format!("{name}.mjs"));
        let out = Command::new(js_runtime())
            .arg(&artifact)
            .args(&args)
            .current_dir(&cwd)
            .output()
            .expect("the javascript runtime runs");
        let raw = String::from_utf8_lossy(&out.stdout).to_string();
        // Both sides are reduced to their prefixes on a volatile line, so the
        // line still has to be *present* and still has to say what it is.
        let mask = |text: &str| -> String {
            text.lines()
                .map(|l| {
                    match volatile.iter().find(|v| l.starts_with(v.as_str())) {
                        Some(v) => format!("{v}<volatile>\n"),
                        None => format!("{l}\n"),
                    }
                })
                .collect()
        };
        let actual = mask(&raw);
        let body = mask(&body);
        if actual != body {
            mismatches.push(format!(
                "{name}:\n  expected:\n{}\n  actual:\n{}",
                indent(&body),
                indent(&actual)
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} example(s) produced the wrong output:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
    eprintln!("golden: {} examples matched their transcripts", examples.len());
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
/// program prints. This runs the whole example corpus both ways and diffs.
#[test]
fn release_and_debug_agree() {
    let root = repo_root();
    let scratch = std::env::temp_dir().join("buri-minify-check");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        scratch.join("REPO.buri"),
        "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
    )
    .unwrap();

    let golden_dir = tests_dir().join("golden");
    let mut examples: Vec<PathBuf> = std::fs::read_dir(root.join("examples"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "buri"))
        .collect();
    examples.sort();

    for path in &examples {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let pkg = scratch.join("cmd").join(&name);
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::copy(path, pkg.join("main.buri")).unwrap();
        std::fs::write(
            pkg.join("BUILD.buri"),
            "binary {\n  outputs: [{ platform: JS }]\n}\n",
        )
        .unwrap();
        let fixtures = golden_dir.join(format!("{name}.files"));
        if fixtures.is_dir() {
            for e in std::fs::read_dir(&fixtures).unwrap().filter_map(Result::ok) {
                let _ = std::fs::copy(e.path(), pkg.join(e.file_name()));
            }
        }
    }

    let mut mismatches = Vec::new();
    let mut smaller = 0usize;
    for path in &examples {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let args: Vec<String> = std::fs::read_to_string(golden_dir.join(format!("{name}.txt")))
            .ok()
            .and_then(|g| {
                g.lines()
                    .find_map(|l| l.strip_prefix("# ARGS:"))
                    .map(|s| s.split_whitespace().map(str::to_string).collect())
            })
            .unwrap_or_default();
        let volatile: Vec<String> = std::fs::read_to_string(golden_dir.join(format!("{name}.txt")))
            .map(|g| {
                g.lines()
                    .filter_map(|l| l.strip_prefix("# VOLATILE:").map(|s| s.trim().to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let mut outputs = Vec::new();
        let mut sizes = Vec::new();
        for mode in ["--debug", "--release"] {
            let target = format!("//cmd/{name}");
            let build = run_in(&scratch, &["build", &target, mode, "--force"]);
            assert_eq!(build.code, 0, "{name} does not build {mode}:\n{}", build.all());
            let artifact = scratch
                .join(".buri/out/js/cmd")
                .join(&name)
                .join(format!("{name}.mjs"));
            sizes.push(std::fs::metadata(&artifact).map(|m| m.len()).unwrap_or(0));
            let out = Command::new(js_runtime())
                .arg(&artifact)
                .args(&args)
                .current_dir(scratch.join("cmd").join(&name))
                .output()
                .expect("the javascript runtime runs");
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            outputs.push(
                text.lines()
                    .map(|l| match volatile.iter().find(|v| l.starts_with(v.as_str())) {
                        Some(v) => format!("{v}<volatile>\n"),
                        None => format!("{l}\n"),
                    })
                    .collect::<String>(),
            );
        }
        if outputs[0] != outputs[1] {
            mismatches.push(format!(
                "{name}:\n  debug:\n{}\n  release:\n{}",
                indent(&outputs[0]),
                indent(&outputs[1])
            ));
        }
        if sizes[1] < sizes[0] {
            smaller += 1;
        }
    }
    assert!(
        mismatches.is_empty(),
        "minification changed the behaviour of {} example(s):\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
    assert!(
        smaller >= examples.len() - 1,
        "minification did not shrink {} of {} artifacts",
        examples.len() - smaller,
        examples.len()
    );
    eprintln!("minify: {} examples behave identically debug and release", examples.len());
}

// ---------------------------------------------------------------------------
// Reproducibility
// ---------------------------------------------------------------------------

/// Two builds of the same sources in the same configuration produce
/// byte-identical artifacts. Symbol names are derived from labels and module
/// paths rather than from compilation order, and iteration is by sorted key.
#[test]
fn builds_are_reproducible() {
    let root = repo_root();
    let example = root.join("examples/20-word-count.buri");
    let mut hashes = Vec::new();
    for run in 0..2 {
        // Two *different directories*, so a path leaking into the output would
        // show up as a difference.
        let scratch = std::env::temp_dir().join(format!("buri-repro-{run}"));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(scratch.join("cmd/w")).unwrap();
        std::fs::write(
            scratch.join("REPO.buri"),
            "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n",
        )
        .unwrap();
        std::fs::copy(&example, scratch.join("cmd/w/main.buri")).unwrap();
        std::fs::write(
            scratch.join("cmd/w/BUILD.buri"),
            "binary {\n  outputs: [{ platform: JS }]\n}\n",
        )
        .unwrap();
        let build = run_in(&scratch, &["build", "//cmd/w", "--release"]);
        assert_eq!(build.code, 0, "build {run} failed:\n{}", build.all());
        let bytes = std::fs::read(scratch.join(".buri/out/js/cmd/w/w.mjs")).unwrap();
        hashes.push(bytes);
    }
    assert!(
        hashes[0] == hashes[1],
        "two builds of the same source produced different artifacts ({} vs {} bytes)",
        hashes[0].len(),
        hashes[1].len()
    );
    eprintln!("reproducible: identical artifacts from two directories");
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
