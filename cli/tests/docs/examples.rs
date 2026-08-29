//! Compiling every example in every document.
//!
//! The examples are checked in the *topic* files under `cli/src/docs/`, not in
//! the assembled `SPEC.md`, because the topic is the file somebody edits — a
//! failure that points at a generated file points at the wrong place. Assembly
//! is concatenation, so checking the topics checks the assembled document
//! exactly (`docs/documents.rs::the_assembled_documents_are_not_stale` keeps
//! the two in step). The root `README.md` is hand-written and no topic's copy,
//! so `readme_examples` compiles it where it sits.
//!
//! There is no per-document registration here: the tests walk
//! `topics::TOPICS`, so a new topic is subject to all of this the moment it
//! is registered.
//!
//! When a block legitimately cannot be compiled, tag it `ignore why="..."`.
//! The reason is required — a fence without one does not extract — and
//! `untested_examples_say_why_and_do_not_multiply` puts a ceiling on how many
//! there may be, so an untested example is a reviewable line in a diff rather
//! than a silence.
use buri::documentation::{examples, topics};
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
fn check_kind(kind: topics::Kind) {
    let root = repo_root();
    let mut failures = Vec::new();
    let mut topics = 0;
    for t in topics::TOPICS.iter().filter(|t| t.kind == kind) {
        topics += 1;
        failures.extend(examples::run_file_at(&root, &topic_path(t.id), t.text));
    }
    assert!(topics > 0, "no topics of this kind");
    assert!(
        failures.is_empty(),
        "{} example(s) across {topics} {} topic(s) do not do what the document says:\n\n{}",
        failures.len(),
        kind.label(),
        examples::report(&failures)
    );
}

#[test]
fn language_reference_examples() {
    check_kind(topics::Kind::Lang);
}

#[test]
fn build_system_examples() {
    check_kind(topics::Kind::Build);
}

#[test]
fn guide_examples() {
    check_kind(topics::Kind::Guide);
}

/// The root `README.md`. It is hand-written rather than assembled, so nothing
/// above reaches its examples and this is what keeps them true.
#[test]
fn readme_examples() {
    let root = repo_root();
    let text = std::fs::read_to_string(root.join("README.md")).expect("the README exists");
    // A README whose every fence stopped extracting would pass the assertion
    // below over nothing at all.
    let compiled = examples::extract("README.md", &text)
        .blocks
        .iter()
        .filter(|b| !b.claim.is_ignored())
        .count();
    assert!(compiled > 0, "no example extracts from README.md; this test has gone vacuous");
    let failures = examples::run_file_at(&root, "README.md", &text);
    assert!(failures.is_empty(), "\n{}", examples::report(&failures));
}

/// The per-command pages. Their prose is hand-written, so it gets the same
/// treatment as everything else.
#[test]
fn cli_reference_examples() {
    let root = repo_root();
    let mut failures = Vec::new();
    for c in buri::commands::COMMANDS {
        let path = format!("cli/src/docs/cli/{}.md", c.name);
        failures.extend(examples::run_file_at(&root, &path, c.doc));
    }
    assert!(failures.is_empty(), "\n{}", examples::report(&failures));
}

/// How many examples may go untested. It may be lowered and not raised.
///
/// This used to be a checked-in list of every ignored fence by file and line,
/// which was a readout of what the suite had just found rather than anything
/// to compare against — it said nothing the documents do not already say, and
/// it went stale on every edit. What it was really buying was the ratchet, and
/// a ratchet is one number.
///
/// The other half of what it bought is already a property of the documents:
/// `ignore` without `why=` is an extraction failure
/// (`documentation::examples::parse_block`), so the reason for every one of
/// these is written where a reader of the diff can weigh it, in the `.md`.
const MAX_IGNORED_EXAMPLES: usize = 62;

/// An untested example is a claim nobody checks, so there is a ceiling on how
/// many of them there may be and each one says why in the document itself.
///
/// Converting one is a smaller number here. Adding one is a bigger number
/// here, in the same diff as the fence — which is the point: it should be
/// visible, not forbidden.
#[test]
fn untested_examples_say_why_and_do_not_multiply() {
    let mut ignored = Vec::new();
    let mut silent = Vec::new();
    for t in topics::TOPICS {
        for block in examples::extract(&topic_path(t.id), t.text).blocks {
            let examples::Claim::Ignore { why } = &block.claim else {
                continue;
            };
            let at = format!("{}:{}", block.origin.file, block.origin.line);
            // Belt and braces: `Claim::Ignore` is only ever built from a
            // non-empty reason, and it is cheap to say so here too rather than
            // to trust that the suite above is the only reader of these fences.
            if why.trim().is_empty() {
                silent.push(at);
            } else {
                ignored.push(at);
            }
        }
    }
    assert!(
        silent.is_empty(),
        "an `ignore` block must say why, and these do not:\n  {}",
        silent.join("\n  ")
    );
    assert!(
        ignored.len() <= MAX_IGNORED_EXAMPLES,
        "{} examples are untested, and the ceiling is {MAX_IGNORED_EXAMPLES}.\n\
         Compile the new one, or raise `MAX_IGNORED_EXAMPLES` in the same diff \
         as the fence so that the addition is what gets reviewed:\n  {}",
        ignored.len(),
        ignored.join("\n  ")
    );
    eprintln!("{} untested example(s), ceiling {MAX_IGNORED_EXAMPLES}", ignored.len());
}

/// A census, so that a harness regression shows up as a changed count rather
/// than as every suite passing vacuously over zero blocks.
#[test]
fn most_examples_are_actually_compiled() {
    let mut compiled = 0;
    let mut ignored = 0;
    for t in topics::TOPICS {
        for block in examples::extract(&topic_path(t.id), t.text).blocks {
            if block.claim.is_ignored() {
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

    for module in buri::compiler::standard_library::MODULES {
        // The source is a field of the entry rather than a second table keyed
        // by path, so a listed module with no source is unrepresentable.
        let (path, source) = (module.path, module.source);
        if !examples::has_examples(source) {
            continue;
        }
        // The name a failure reports: where the module actually lives, so the
        // line number is one an editor can open.
        let rel = format!("cli/src/compiler/standard_library/sources/{}.buri", path.trim_start_matches("core/"));
        let text = examples::doc_comments(source);
        let found = examples::extract(&rel, &text)
            .blocks
            .iter()
            .filter(|b| !b.claim.is_ignored())
            .count();
        if found == 0 {
            continue;
        }
        modules += 1;
        blocks += found;
        failures.extend(examples::run_file_at(&root, &rel, &text));
    }

    assert!(
        failures.is_empty(),
        "{} example(s) in standard library documentation comments do not do what they say:\n\n{}",
        failures.len(),
        examples::report(&failures)
    );
    // A corpus that discovers nothing passes every assertion.
    assert!(
        blocks >= 8 && modules >= 4,
        "only {blocks} example(s) across {modules} module(s); the extractor is missing them"
    );
}
