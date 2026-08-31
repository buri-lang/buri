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
use buri::documentation::{examples, layout, topics};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Where a topic's text lives on disk, for the location a failure reports.
fn topic_path(id: &str) -> String {
    format!("cli/src/docs/{id}.md")
}

/// Every markdown document under `dir`, laid out, on a blessing run.
///
/// For the documents this suite enforces that are *not* compiled into the
/// binary: the worked monorepo's own pages, which
/// `documents::a_repository_can_test_its_own_documentation` runs `buri docs
/// test` over exactly as another repository would.
pub fn bless_documents_under(root: &Path, dir: &str) {
    if std::env::var_os("BURI_BLESS").is_none() {
        return;
    }
    let mut found = Vec::new();
    layout::documents_under(&root.join(dir), &mut found);
    for path in found {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
        if let Some(out) = layout::format_document(&rel, &text) {
            std::fs::write(&path, out).unwrap();
        }
    }
}

/// One document's text — and, on a blessing run, the document laid out.
///
/// A page's text is `include_str!`d, so what a check sees is what the last
/// build compiled in. `BURI_BLESS=1` is the other direction: read the file as
/// it stands, lay out every fence in it (`documentation::layout`, which is what
/// `buri format` does to a document), write it back, and check *that* — so a
/// blessing run is green and the diff is what gets read.
///
/// Blessing rewrites fence bodies and nothing else. The prose is the author's.
pub fn document(root: &Path, rel: &str, compiled_in: &str) -> String {
    if std::env::var_os("BURI_BLESS").is_none() {
        // The README is not compiled into anything, so it is read where it
        // lives; every other document's text is the one the binary carries.
        if compiled_in.is_empty() {
            return std::fs::read_to_string(root.join(rel)).expect("the document exists");
        }
        return compiled_in.to_string();
    }
    let path = root.join(rel);
    let Ok(text) = std::fs::read_to_string(&path) else { return compiled_in.to_string() };
    match layout::format_document(rel, &text) {
        Some(out) => {
            std::fs::write(&path, &out).unwrap();
            out
        }
        None => text,
    }
}

/// Where a standard-library module's text lives on disk.
///
/// The entry in `standard_library::MODULES` says what a module *is* and not
/// which file it was read from, and the two do not follow one another:
/// `core/net/http` is `sources/http.buri` and `ui/node` is
/// `sources/ui_node.buri`. Deriving a path from the module path named a file
/// that does not exist for eight of them, so a failure in one pointed at
/// nothing. This asks the only thing that cannot be wrong — the bytes.
fn source_path(root: &Path, text: &str) -> Option<String> {
    const DIR: &str = "cli/src/compiler/standard_library/sources";
    let mut found = None;
    let mut stack = vec![root.join(DIR)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()? {
            let path = entry.ok()?.path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
                found = Some(path.strip_prefix(root).ok()?.display().to_string());
            }
        }
    }
    found
}

/// The same, for a source file whose examples are written in its `///` and
/// `//!` comments.
fn source_text(root: &Path, rel: &str, compiled_in: &str) -> String {
    if std::env::var_os("BURI_BLESS").is_none() {
        return compiled_in.to_string();
    }
    let path = root.join(rel);
    let Ok(text) = std::fs::read_to_string(&path) else { return compiled_in.to_string() };
    match layout::format_doc_comments(&text) {
        Some(out) => {
            std::fs::write(&path, &out).unwrap();
            out
        }
        None => text,
    }
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
        let path = topic_path(t.id);
        let text = document(&root, &path, t.text);
        failures.extend(examples::run_file_at(&root, &path, &text));
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
    let text = document(&root, "README.md", "");
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
        let text = document(&root, &path, c.doc);
        failures.extend(examples::run_file_at(&root, &path, &text));
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
        let compiled_in = module.source;
        if !examples::has_examples(compiled_in) {
            continue;
        }
        // The name a failure reports: where the module actually lives, so the
        // line number is one an editor can open.
        let Some(rel) = source_path(&root, compiled_in) else {
            continue;
        };
        let text = examples::doc_comments(&source_text(&root, &rel, compiled_in));
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

/// How many fences the formatter is allowed to have nothing to say about.
///
/// A page about a syntax error shows the syntax error, and the formatter
/// refuses text it could not read whole — so a silence is expected and a
/// *growing* number of them is not. The ceiling is one number for the same
/// reason `MAX_IGNORED_EXAMPLES` is: it may be lowered and not raised, and
/// raising it is a line in the same diff as the fence that needed it.
const MAX_UNCHECKED_LAYOUTS: usize = 90;

/// **Every example is laid out the way `buri format` lays out source**, and the
/// ones that cannot be are counted.
///
/// The enforcement itself is inside `examples::run_file_at`, so every test
/// above already fails on a fence that has drifted, and so does `buri docs
/// test` in any repository. What this adds is the census: a check that passes
/// because it checked nothing is the failure mode a layout rule has, and the
/// floor and the ceiling here are what rule it out.
#[test]
fn every_example_is_laid_out_the_way_the_formatter_writes_source() {
    let root = repo_root();
    let mut clean = 0;
    let mut silent = Vec::new();
    let mut drifted = Vec::new();

    // On a blessing run this is also where the catalog pages are laid out: the
    // error pages are the only ones another test reads, and the lint pages are
    // read by this census alone.
    let mut census = |file: &str, text: &str| {
        for block in examples::extract(file, text).blocks {
            match layout::verdict(&block) {
                layout::Verdict::Clean => clean += 1,
                layout::Verdict::Silent(why) => silent.push(format!("{}: {why}", block.origin)),
                layout::Verdict::Drifted(_) => drifted.push(block.origin.to_string()),
            }
        }
    };

    for t in topics::TOPICS {
        census(&topic_path(t.id), &document(&root, &topic_path(t.id), t.text));
    }
    for c in buri::commands::COMMANDS {
        let rel = format!("cli/src/docs/cli/{}.md", c.name);
        census(&rel, &document(&root, &rel, c.doc));
    }
    // The catalogs. Their pages are markdown like any other, and the program on
    // an error page is the one a reader copies to reproduce the error.
    for e in buri::documentation::errors::ERRORS {
        let rel = format!("cli/src/docs/errors/{}.md", e.code);
        census(&rel, &document(&root, &rel, e.text));
    }
    for l in buri::documentation::lints::LINTS {
        let rel = format!("cli/src/docs/lints/{}.md", l.code);
        census(&rel, &document(&root, &rel, l.text));
    }
    census("README.md", &document(&root, "README.md", ""));
    for module in buri::compiler::standard_library::MODULES {
        if !examples::has_examples(module.source) {
            continue;
        }
        let rel = source_path(&root, module.source).expect("the module's file");
        census(&rel, &examples::doc_comments(module.source));
    }

    assert!(
        drifted.is_empty(),
        "{} example(s) are not laid out the way `buri format` lays out source:\n  {}\n\
         `BURI_BLESS=1 cargo test -p buri --test docs` rewrites the fence bodies, \
         and nothing else on the page.",
        drifted.len(),
        drifted.join("\n  ")
    );
    assert!(
        clean > 190,
        "only {clean} example(s) were laid out and checked; the census has gone vacuous"
    );
    assert!(
        silent.len() <= MAX_UNCHECKED_LAYOUTS,
        "{} example(s) have no canonical layout, and the ceiling is \
         {MAX_UNCHECKED_LAYOUTS}:\n  {}",
        silent.len(),
        silent.join("\n  ")
    );
    eprintln!("{clean} example(s) laid out, {} the formatter has nothing to say about", silent.len());
}
