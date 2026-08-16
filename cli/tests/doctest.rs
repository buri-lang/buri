//! Compiling every example in every document.
//!
//! The examples are checked in the *topic* files under `cli/src/docs/`, not in
//! the assembled `SPEC.md` and `README.md`, because the topic is the file
//! somebody edits — a failure that points at a generated file points at the
//! wrong place. Assembly is concatenation, so checking the topics checks the
//! assembled documents exactly (`docs.rs::the_assembled_documents_are_not_stale`
//! keeps the two in step).
//!
//! There is no per-document registration here: the tests walk
//! `doc_topics::TOPICS`, so a new topic is subject to all of this the moment it
//! is registered.
//!
//! When a block legitimately cannot be compiled, tag it `ignore why="..."`.
//! Those are listed by file and line in `doctest_ignores.txt`, and
//! `the_ignore_list_only_shrinks` fails when a new one appears without being
//! recorded — so an untested example is a reviewable line in a diff rather
//! than a silence.

use buri::{doc_topics, doctest};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Where a topic's text lives on disk, for the location a failure reports.
fn topic_path(id: &str) -> String {
    format!("cli/src/docs/{id}.md")
}

/// Compiles every block of every topic of one kind, and fails with all of them
/// at once — a fence at a time would make fixing a document a dozen round
/// trips.
fn check_kind(kind: doc_topics::Kind) {
    let root = repo_root();
    let mut failures = Vec::new();
    let mut topics = 0;
    for t in doc_topics::TOPICS.iter().filter(|t| t.kind == kind) {
        topics += 1;
        failures.extend(doctest::run_file_at(&root, &topic_path(t.id), t.text));
    }
    assert!(topics > 0, "no topics of this kind");
    assert!(
        failures.is_empty(),
        "{} example(s) across {topics} {} topic(s) do not do what the document says:\n\n{}",
        failures.len(),
        kind.label(),
        doctest::report(&failures)
    );
}

#[test]
fn language_reference_examples() {
    check_kind(doc_topics::Kind::Lang);
}

#[test]
fn build_system_examples() {
    check_kind(doc_topics::Kind::Build);
}

#[test]
fn guide_examples() {
    check_kind(doc_topics::Kind::Guide);
}

/// The per-command pages. Their prose is hand-written, so it gets the same
/// treatment as everything else.
#[test]
fn cli_reference_examples() {
    let root = repo_root();
    let mut failures = Vec::new();
    for c in buri::commands::COMMANDS {
        let path = format!("cli/src/docs/cli/{}.md", c.name);
        failures.extend(doctest::run_file_at(&root, &path, c.doc));
    }
    assert!(failures.is_empty(), "\n{}", doctest::report(&failures));
}

/// The untested examples, by file and line, with the reason each gives.
///
/// This may shrink and may not grow. `BURI_BLESS=1` rewrites it, which is the
/// escape hatch for a deliberate addition — the point is that adding one shows
/// up in review, not that it is forbidden.
#[test]
fn the_ignore_list_only_shrinks() {
    let mut lines = Vec::new();
    for t in doc_topics::TOPICS {
        for block in doctest::extract(&topic_path(t.id), t.text).blocks {
            if block.mode != doctest::Mode::Ignore {
                continue;
            }
            lines.push(format!(
                "{}:{}  {}",
                block.origin.file,
                block.origin.line,
                block.why.as_deref().unwrap_or("(no reason)")
            ));
        }
    }
    lines.sort();
    let actual = lines.join("\n");

    let path = repo_root().join("cli/tests/doctest_ignores.txt");
    if std::env::var_os("BURI_BLESS").is_some() {
        std::fs::write(&path, format!("{actual}\n")).expect("writing the ignore list");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        actual.trim(),
        expected.trim(),
        "the list of untested examples changed.\n\
         If that is deliberate, re-bless it:\n  \
         BURI_BLESS=1 cargo test -p buri --test doctest the_ignore_list"
    );
}

/// A census, so that a harness regression shows up as a changed count rather
/// than as every suite passing vacuously over zero blocks.
#[test]
fn most_examples_are_actually_compiled() {
    let mut compiled = 0;
    let mut ignored = 0;
    for t in doc_topics::TOPICS {
        for block in doctest::extract(&topic_path(t.id), t.text).blocks {
            if block.mode == doctest::Mode::Ignore {
                ignored += 1;
            } else {
                compiled += 1;
            }
        }
    }
    eprintln!("{compiled} compiled, {ignored} ignored");
    assert!(compiled > 40, "only {compiled} examples are compiled; is the harness running?");
}

/// Every example written in a `///` or `//!` comment in the standard library.
///
/// The prose pages were already compiled by the tests above; this closes the
/// other half, because a documentation comment is documentation and an example
/// in one has the same claim on being true. `doc_comments` turns a source file
/// into a document with the source's own line numbers, so a failure points at
/// the `.buri` line the example is written on rather than at an offset into
/// something synthetic.
#[test]
fn standard_library_doc_comments() {
    let root = repo_root();
    let mut failures = Vec::new();
    let mut blocks = 0;
    let mut modules = 0;

    for path in buri::stdlib::MODULES {
        let Some(source) = buri::stdlib::source(path) else {
            panic!("`{path}` is listed in MODULES and has no source");
        };
        if !doctest::has_examples(source) {
            continue;
        }
        // The name a failure reports: where the module actually lives, so the
        // line number is one an editor can open.
        let rel = format!("cli/src/std/{}.buri", path.trim_start_matches("core/"));
        let text = doctest::doc_comments(source);
        let found = doctest::extract(&rel, &text)
            .blocks
            .iter()
            .filter(|b| b.mode != doctest::Mode::Ignore)
            .count();
        if found == 0 {
            continue;
        }
        modules += 1;
        blocks += found;
        failures.extend(doctest::run_file_at(&root, &rel, &text));
    }

    assert!(
        failures.is_empty(),
        "{} example(s) in standard library documentation comments do not do what they say:\n\n{}",
        failures.len(),
        doctest::report(&failures)
    );
    // A corpus that discovers nothing passes every assertion.
    assert!(
        blocks >= 8 && modules >= 4,
        "only {blocks} example(s) across {modules} module(s); the extractor is missing them"
    );
}
