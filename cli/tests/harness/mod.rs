//! Shared machinery for the suites that drive the real `buri` binary.
//!
//! Everything a suite needs that is not about what it is proving: where the
//! binary is, a repository nobody else can see, running the CLI in it, and the
//! bless-or-compare loop that turns printed output into a diff.
//!
//! Nothing here writes inside a checked-in tree. A test that builds in
//! `build-system/example` leaves `.buri/` and an `out` symlink behind, and a
//! test that edits a fixture in place corrupts it if it panics first — so every
//! suite works on a copy under `CARGO_TARGET_TMPDIR` instead.

// Each test binary gets its own copy of this module and uses a subset of it.
#![allow(dead_code, unused_imports)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

mod case;
pub use case::{load_case, run_case, run_corpus, Case, Step, Stream};

// ---------------------------------------------------------------------------
// Where things are
// ---------------------------------------------------------------------------

pub fn buri() -> &'static str {
    env!("CARGO_BIN_EXE_buri")
}

pub fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

pub fn example_repo() -> PathBuf {
    repo_root().join("build-system/example")
}

pub fn js_runtime() -> String {
    std::env::var("BURI_JS").unwrap_or_else(|_| "bun".to_string())
}

/// The `REPO.buri` every scratch repository gets. The hash is deliberately not
/// a real one: nothing in the test suite fetches a toolchain, and a plausible
/// hash would invite someone to believe it was checked.
pub const REPO_BURI: &str = "toolchain {\n  version: \"0.3.0\"\n  sha256: \"00\"\n}\n";

/// The build file for a single-package JS binary, which is what most scratch
/// repositories are.
pub const JS_BINARY: &str = "binary {\n  outputs: [{ platform: JS }]\n}\n";

// ---------------------------------------------------------------------------
// Running the CLI
// ---------------------------------------------------------------------------

pub fn strip_ansi(s: &str) -> String {
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

pub fn indent(s: &str) -> String {
    s.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    /// What was run, so a failure can say so without the caller repeating it.
    pub what: String,
}

impl Run {
    /// stdout then stderr. Diagnostics go to stderr and summaries to stdout,
    /// and a case is usually about both.
    pub fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    pub fn ok(&self) -> &Run {
        self.exits(0)
    }

    pub fn exits(&self, want: i32) -> &Run {
        assert!(
            self.code == want,
            "`{}` exited {} rather than {want}:\n{}",
            self.what,
            self.code,
            indent(&self.all())
        );
        self
    }

    pub fn says(&self, needle: &str) -> &Run {
        assert!(
            self.all().contains(needle),
            "`{}` never said {needle:?}:\n{}",
            self.what,
            indent(&self.all())
        );
        self
    }

    pub fn silent_about(&self, needle: &str) -> &Run {
        assert!(
            !self.all().contains(needle),
            "`{}` said {needle:?} and should not have:\n{}",
            self.what,
            indent(&self.all())
        );
        self
    }

    /// `N` from the test runner's `N passed, M failed (1.2s)` summary line.
    ///
    /// A suite that compiled to nothing exits 0 having asserted nothing, so
    /// every caller of this pairs it with a floor.
    pub fn tests_passed(&self) -> usize {
        let out = self.all();
        let summary = out
            .lines()
            .rev()
            .find(|l| l.contains(" passed, "))
            .unwrap_or_else(|| panic!("no summary line from `{}`:\n{}", self.what, indent(&out)));
        summary
            .split_whitespace()
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("unreadable summary from `{}`: {summary:?}", self.what))
    }

    /// Output with the three fields that move between runs replaced, so it can
    /// be recorded. Everything else is already deterministic: file names are
    /// repository-relative, targets are sorted, and diagnostics are sorted.
    pub fn normalised(&self, root: &Path) -> String {
        let mut s = self.all();
        // An absolute path. Only a toolchain failure can print one, so it is
        // replaced rather than deleted: a golden holding `<scratch>` is a
        // visible bug rather than a silent one.
        s = s.replace(&root.display().to_string(), "<scratch>");
        s = replace_between(&s, "(", "s)", "(0.0s)");
        s = replace_between(&s, "(", " bytes)", "(N bytes)");
        s
    }
}

/// Replaces `(<digits and dots>s)` and `(<digits> bytes)` — elapsed time and
/// artifact size, the two things a golden must not depend on. Hand-rolled
/// because the toolchain has no regex and is not getting one.
fn replace_between(s: &str, open: &str, close: &str, with: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(close) else {
            out.push_str(&rest[..i + open.len()]);
            rest = after;
            continue;
        };
        let inner = &after[..j];
        if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == '.') {
            out.push_str(&rest[..i]);
            out.push_str(with);
            rest = &after[j + close.len()..];
        } else {
            out.push_str(&rest[..i + open.len()]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

pub fn run_in(dir: &Path, args: &[&str]) -> Run {
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
        what: format!("buri {}", args.join(" ")),
    }
}

// ---------------------------------------------------------------------------
// A repository nobody else can see
// ---------------------------------------------------------------------------

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A repository on disk that no other test can see.
///
/// Rooted under `CARGO_TARGET_TMPDIR` rather than the system temp directory,
/// and named with the process id and a counter, so two tests in one run and
/// two `cargo test` runs in two shells never share a directory.
pub struct Scratch {
    pub root: PathBuf,
    name: String,
}

impl Scratch {
    /// An empty directory, and deliberately *not* a repository: the cases that
    /// check what happens outside one need this.
    pub fn empty(name: &str) -> Scratch {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A `REPO.buri` in an ancestor would make `find_root` succeed where a
        // test needs it to fail, and that failure would be baffling.
        let mut up = root.as_path();
        while let Some(parent) = up.parent() {
            assert!(
                !parent.join("REPO.buri").is_file(),
                "{} holds a REPO.buri, so a scratch directory beneath it is not outside a repository",
                parent.display()
            );
            up = parent;
        }
        Scratch { root, name: name.to_string() }
    }

    /// An empty repository: a directory holding only a `REPO.buri`.
    pub fn repo(name: &str) -> Scratch {
        let s = Scratch::empty(name);
        s.write("REPO.buri", REPO_BURI);
        s
    }

    /// A copy of a checked-in tree, leaving behind what a previous build wrote.
    /// This is how a suite tests `build-system/example` or the conformance
    /// repository without writing into either.
    pub fn copy_of(name: &str, source: &Path) -> Scratch {
        let s = Scratch::empty(name);
        copy_tree(source, &s.root, &[".buri", "out"]);
        s
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        path
    }

    pub fn read(&self, rel: &str) -> String {
        let path = self.path(rel);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    /// Replaces the first occurrence of `from` with `to`, and panics if it is
    /// not there. A substitution that silently does nothing is a test that
    /// silently proves nothing.
    pub fn edit(&self, rel: &str, from: &str, to: &str) {
        let before = self.read(rel);
        let after = before.replacen(from, to, 1);
        assert!(
            after != before,
            "{rel} does not contain {from:?}, so the edit changed nothing:\n{}",
            indent(&before)
        );
        self.write(rel, &after);
    }

    /// A single-package JS binary at `<path>`: a `BUILD.buri` and a `main.buri`.
    pub fn binary_package(&self, path: &str, source: &str) {
        self.write(&format!("{path}/BUILD.buri"), JS_BINARY);
        self.write(&format!("{path}/main.buri"), source);
    }

    pub fn run(&self, args: &[&str]) -> Run {
        run_in(&self.root, args)
    }

    /// The artifact `//<pkg_path>` builds to, under the JS runtime.
    pub fn artifact(&self, pkg_path: &str) -> PathBuf {
        let leaf = pkg_path.rsplit('/').next().unwrap();
        self.path(&format!(".buri/out/js/{pkg_path}/{leaf}.mjs"))
    }

    /// Runs that artifact. Not through `buri run`, because several suites need
    /// the artifact's own exit code and streams rather than the CLI's.
    pub fn exec_js(&self, pkg_path: &str) -> Run {
        let artifact = self.artifact(pkg_path);
        let out = Command::new(js_runtime())
            .arg(&artifact)
            .output()
            .expect("the javascript runtime runs");
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            what: format!("{} {}", js_runtime(), artifact.display()),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A failing test's evidence is the directory it failed in.
        if std::thread::panicking() || std::env::var_os("BURI_KEEP").is_some() {
            eprintln!("scratch `{}` kept at {}", self.name, self.root.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Copies a source tree, leaving behind the named entries — anything a
/// previous build wrote.
pub fn copy_tree(from: &Path, to: &Path, skip: &[&str]) {
    std::fs::create_dir_all(to).unwrap();
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", from.display()))
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let name = e.file_name();
        if skip.iter().any(|s| name == *s) {
            continue;
        }
        let (src, dst) = (e.path(), to.join(&name));
        if src.is_dir() {
            copy_tree(&src, &dst, skip);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Corpus discovery
// ---------------------------------------------------------------------------

/// Directories under `dir` holding `marker`, sorted.
///
/// `floor` is not optional. A corpus that discovers nothing passes every
/// assertion anyone makes about it, so the count is itself an assertion.
pub fn case_dirs(dir: &Path, marker: &str, floor: usize) -> Vec<PathBuf> {
    let mut cases: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} does not exist: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join(marker).is_file())
        .collect();
    cases.sort();
    assert!(
        cases.len() >= floor,
        "expected at least {floor} cases in {}, found {}",
        dir.display(),
        cases.len()
    );
    cases
}

/// Files directly under `dir` with the given extension, sorted, same floor.
pub fn case_files(dir: &Path, ext: &str, floor: usize) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} does not exist: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .collect();
    files.sort();
    assert!(
        files.len() >= floor,
        "expected at least {floor} cases in {}, found {}",
        dir.display(),
        files.len()
    );
    files
}

/// The one-phrase expectation a corpus file carries on its first lines:
/// `// EXPECT:` in `tests/reject`, `// CRASH:` in `tests/crash`.
pub fn annotation(text: &str, prefix: &str) -> Option<String> {
    text.lines().find_map(|l| l.trim().strip_prefix(prefix).map(|s| s.trim().to_string()))
}

pub fn require_annotation(text: &str, prefix: &str, case: &str) -> String {
    annotation(text, prefix).unwrap_or_else(|| panic!("{case}: no `{prefix}` line"))
}

// ---------------------------------------------------------------------------
// Recorded output
// ---------------------------------------------------------------------------

/// The bless-or-compare half of every recorded-output suite.
///
/// `BURI_BLESS=1` records; otherwise a mismatch is a collected failure, so one
/// run reports every case that moved rather than only the first.
pub struct Golden {
    bless: bool,
    blessed: usize,
    failures: Vec<String>,
}

impl Golden {
    pub fn new() -> Golden {
        Golden {
            bless: std::env::var_os("BURI_BLESS").is_some(),
            blessed: 0,
            failures: Vec::new(),
        }
    }

    pub fn blessing(&self) -> bool {
        self.bless
    }

    pub fn fail(&mut self, msg: String) {
        self.failures.push(msg);
    }

    /// `label` is what a reader needs in order to find the case, as in
    /// `missing_dep/lint.txt`.
    pub fn check(&mut self, path: &Path, label: &str, actual: &str) {
        if self.bless {
            if let Some(p) = path.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            std::fs::write(path, actual).unwrap();
            self.blessed += 1;
            return;
        }
        match std::fs::read_to_string(path) {
            Ok(golden) if golden == actual => {}
            Ok(golden) => self.failures.push(format!(
                "{label} does not match what the toolchain prints.\n  recorded:\n{}\n  printed:\n{}",
                indent(&golden),
                indent(actual)
            )),
            Err(_) => self.failures.push(format!(
                "{label} is missing — run with BURI_BLESS=1 to record:\n{}",
                indent(actual)
            )),
        }
    }

    /// `what` names the corpus for the one line a passing run prints.
    ///
    /// Failures are asserted whether or not this run is blessing. Blessing
    /// records prose; it must not launder a case that stopped failing, a
    /// diagnostic that lost its `fix`, or a flipped exit code.
    pub fn finish(self, what: &str, cases: usize) {
        assert!(
            self.failures.is_empty(),
            "{} {what} case(s) wrong:\n\n{}",
            self.failures.len(),
            self.failures.join("\n\n")
        );
        if self.bless {
            eprintln!("{what}: recorded {} files", self.blessed);
            return;
        }
        eprintln!("{what}: {cases} cases");
    }
}

impl Default for Golden {
    fn default() -> Golden {
        Golden::new()
    }
}
