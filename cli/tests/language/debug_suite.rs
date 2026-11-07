//! The conformance suite run once per test run for each tree of sources, in
//! the default (debug) mode.
//!
//! Three tests ask about the corpus as checked in: [`crate::conformance`]'s
//! `conformance_suite_passes`, the debug half of its `release_and_debug_agree`,
//! and [`crate::corpus`]'s `formatting_the_corpus_preserves_what_it_means`,
//! whose "before" copy is the unmodified corpus. Each used to make its own
//! copy and run `buri test //... --force` in it, which is the same inputs three
//! times: the same tree, the same flags (`--debug` is the default mode, so
//! naming it changes nothing a build reads), and the same environment. So the
//! run happens here, once, and each test asserts what it always asserted on
//! the answer.
//!
//! The formatting test's "after" copy is a fourth run, of whatever formatting
//! wrote. Its question is whether that tree answers as the checked-in one
//! does, and when formatting gives back the bytes it started from, the two
//! trees are the same inputs too. So a run is keyed by what its tree holds —
//! every file's path and bytes — rather than by who asks: identical trees
//! share one run, and a tree that differs by a byte gets one of its own.
//!
//! Once per run, not once per process: nextest starts a process per test, so
//! the first process to ask leaves the answer on disk for the others
//! (`sweep::once_per_run`). That was 80 CPU-seconds a suite, and 20 more for
//! the formatted copy.
//!
//! The run is memoized even when it fails. A failure is recorded and handed to
//! every test that asks, rather than left out for the next test to try again,
//! so a flaky run cannot pass on its second attempt.
use crate::harness::*;

use std::path::Path;
use std::sync::OnceLock;

/// `buri test //... --force` over a fresh copy of `cli/tests/conformance`,
/// with no mode flag and nothing added to the environment.
///
/// The answer comes back whatever the exit status, because the tests that
/// read it differ in what they assert about that. A run that panicked the
/// harness itself — a leak, or a run past the hang cap — panics every caller
/// with the same message.
pub fn unmodified_conformance_run() -> &'static Run {
    static RUN: OnceLock<Result<Run, String>> = OnceLock::new();
    let answer = RUN.get_or_init(|| {
        let source = tests_dir().join("conformance");
        conformance_run(&source, || Scratch::copy_of("conformance", &source))
    });
    match answer {
        Ok(run) => run,
        Err(said) => panic!("the unmodified conformance suite could not be run:\n{said}"),
    }
}

/// `buri test //... --force` over a tree holding what `source` holds, shared
/// with every caller in this test run whose tree holds the same.
///
/// `copy` makes the tree the run happens in. Only the caller that computes
/// calls it, and its tree is kept when the run fails.
pub fn conformance_run(source: &Path, copy: impl FnOnce() -> Scratch) -> Result<Run, String> {
    read_back(&once_per_content("conformance-run", source, || {
        written(&run_in_copy(copy))
    }))
}

/// [`sweep::once_per_run`], keyed by what `tree` holds as well as by `name`.
fn once_per_content(name: &str, tree: &Path, compute: impl FnOnce() -> String) -> String {
    sweep::once_per_run(&format!("{name}-{}", content_digest(tree)), compute)
}

/// Every file under `tree`, by relative path and bytes, as one digest.
///
/// It skips what [`copy_tree`] skips for a [`Scratch`], `.buri` and `out` at
/// any depth: a tree with a build's output in it holds the same sources as
/// one without.
fn content_digest(tree: &Path) -> String {
    fn walk(dir: &Path, root: &Path, hash: &mut buri::build::sha256::Sha256) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.file_name().is_some_and(|n| n == ".buri" || n == "out") {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, hash);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                hash.text(&relative.to_string_lossy());
                hash.field(&std::fs::read(&path).unwrap());
            }
        }
    }
    let mut hash = buri::build::sha256::Sha256::new();
    walk(tree, tree, &mut hash);
    hash.finish()
}

fn run_in_copy(copy: impl FnOnce() -> Scratch) -> Result<Run, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let suite = copy();
        let run = suite.run(&["test", "//...", "--force"]);
        if run.code != 0 {
            // The directory is the evidence, and no test is panicking yet
            // for `Scratch`'s own drop to notice.
            eprintln!("scratch kept at {}", suite.root.display());
            std::mem::forget(suite);
        }
        run
    }))
    .map_err(|panic| {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_else(|| String::from("a panic with no message"))
    })
}

/// An answer as one process leaves it for the next: a header of the field
/// lengths, then the fields.
fn written(answer: &Result<Run, String>) -> String {
    match answer {
        Ok(run) => format!(
            "{} {} {} {}\n{}{}{}",
            run.code,
            run.what.len(),
            run.stdout.len(),
            run.stderr.len(),
            run.what,
            run.stdout,
            run.stderr
        ),
        Err(said) => format!("panicked\n{said}"),
    }
}

/// [`written`] read back.
fn read_back(text: &str) -> Result<Run, String> {
    let (head, rest) = text.split_once('\n').expect("an answer has a header");
    if head == "panicked" {
        return Err(rest.to_string());
    }
    let Some((code, lengths)) = head.split_once(' ') else {
        panic!("{head:?} is not an answer's header");
    };
    let lengths: Vec<usize> = lengths.split(' ').map(|n| n.parse().unwrap()).collect();
    let [what, stdout, stderr] = lengths[..] else {
        panic!("{head:?} is not an answer's header");
    };
    let (what, rest) = rest.split_at(what);
    let (stdout, rest) = rest.split_at(stdout);
    assert_eq!(rest.len(), stderr, "an answer's stderr is cut short");
    Ok(Run {
        code: code.parse().unwrap(),
        stdout: stdout.to_string(),
        stderr: rest.to_string(),
        what: what.to_string(),
    })
}

/// What another process wrote reads back as the same answer, with the bytes
/// a run's output can hold.
#[test]
fn an_answer_reads_back_as_it_was_written() {
    let run = Run {
        code: 3,
        stdout: String::from("é 🙂 1 passed\n"),
        stderr: String::from("error: one\n  |\n"),
        what: String::from("buri test //... --force"),
    };
    let Ok(back) = read_back(&written(&Ok(run))) else {
        panic!("a run read back as a panic");
    };
    assert_eq!(
        (
            back.code,
            back.stdout.as_str(),
            back.stderr.as_str(),
            back.what.as_str()
        ),
        (
            3,
            "é 🙂 1 passed\n",
            "error: one\n  |\n",
            "buri test //... --force"
        )
    );
    let panicked = Err(String::from("leaked a block\nin 🙂"));
    assert_eq!(read_back(&written(&panicked)).err(), panicked.err());
}

/// Trees that hold the same files share one computation; a byte or a name
/// changed is a tree of its own, and a build's output is not part of a tree.
#[test]
fn identical_trees_compute_once() {
    let root = sweep::owned("content-digest", "trees");
    let tree = |name: &str, files: &[(&str, &str)]| {
        let dir = root.join(name);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    };
    let corpus = [("REPO.buri", "repo"), ("lib/a/a.buri", "fn a() {}")];
    let first = tree("first", &corpus);
    let same = tree("same", &corpus);
    let built = tree(
        "built",
        &[
            ("REPO.buri", "repo"),
            ("lib/a/a.buri", "fn a() {}"),
            (".buri/out/a.js", "built"),
            ("lib/a/out/x", "built"),
        ],
    );
    let byte = tree(
        "byte",
        &[("REPO.buri", "repo"), ("lib/a/a.buri", "fn b() {}")],
    );
    let renamed = tree(
        "renamed",
        &[("REPO.buri", "repo"), ("lib/b/a.buri", "fn a() {}")],
    );

    let mut computed = 0;
    let name = format!("content-digest-{}", std::process::id());
    let mut answers = Vec::new();
    for dir in [&first, &same, &built, &byte, &renamed] {
        answers.push(once_per_content(&name, dir, || {
            computed += 1;
            format!("answer {computed}")
        }));
    }
    assert_eq!(
        answers,
        ["answer 1", "answer 1", "answer 1", "answer 2", "answer 3"]
    );
    assert_eq!(computed, 3);
}
