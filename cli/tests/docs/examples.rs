//! Compiling every example in every document.
//!
//! The examples are checked in the *topic* files under `crates/docs/src/docs/`, not in
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
//! There is no way to leave a block out. One that needs more than a single
//! file to mean anything is a `file=` of a repository, and that repository is
//! built (`documentation::examples`).
use buri::documentation::{examples, layout, topics};
use crate::shard;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Where a topic's text lives on disk, for the location a failure reports.
///
/// The registry answers this rather than the id: a `build/*` topic's file is
/// under `reference/build/`, so concatenating the id would name nothing.
fn topic_path(topic: &topics::Topic) -> String {
    topic.path()
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
    // The standard library's sources, and the bundled platforms' `platform.buri`.
    const DIRS: [&str; 2] = ["crates/stdlib/src/compiler/standard_library/sources", "crates/stdlib/src/platforms"];
    let mut found = None;
    let mut stack: Vec<_> = DIRS.iter().map(|d| root.join(d)).collect();
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
        let path = topic_path(t);
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
fn getting_started_examples() {
    check_kind(topics::Kind::GettingStarted);
}

#[test]
fn language_reference_examples() {
    check_kind(topics::Kind::Language);
}

#[test]
fn build_system_examples() {
    check_kind(topics::Kind::Build);
}

#[test]
fn reference_examples() {
    check_kind(topics::Kind::Reference);
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
    let compiled = examples::extract("README.md", &text).blocks.len();
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
        let path = format!("crates/docs/src/docs/reference/cli/{}.md", c.name);
        let text = document(&root, &path, c.doc);
        failures.extend(examples::run_file_at(&root, &path, &text));
    }
    assert!(failures.is_empty(), "\n{}", examples::report(&failures));
}

/// A census, so that a harness regression shows up as a changed count rather
/// than as every suite passing vacuously over zero blocks.
#[test]
fn the_examples_are_actually_extracted() {
    let (mut compiled, mut files) = (0, 0);
    for t in topics::TOPICS {
        let found = examples::extract(&topic_path(t), t.text);
        compiled += found.blocks.len();
        files += found.files.len();
    }
    eprintln!("{compiled} Buri example(s), {files} of them or of textproto files of a repository");
    assert!(compiled > 40, "only {compiled} examples are compiled; is the harness running?");
    assert!(files > 3, "only {files} fences are files of a repository; is the harness running?");
}

/// Every example written in a `///` or `//!` comment in the standard library.
///
/// The prose pages were already compiled by the tests above; this closes the
/// other half, because a documentation comment is documentation and an example
/// in one has the same claim on being true. `doc_comments` turns a source file
/// into a document with the source's own line numbers, so a failure points at
/// the `.buri` line the example is written on rather than at an offset into
/// something synthetic.
///
/// The modules are four tests, `standard_library_doc_comments::shard_0` to
/// `shard_3`, so nextest can run them side by side (`harness/shard.rs`).
fn doc_comments_shard(at: usize, count: usize) {
    let root = repo_root();
    let all = documented_modules(&root);
    let mine = shard::of(&all, at, count);
    let mut failures = Vec::new();
    let mut blocks = 0;
    for (rel, text, found) in mine.iter().copied() {
        blocks += found;
        failures.extend(examples::run_file_at(&root, rel, text));
    }
    let modules = mine.len();

    assert!(
        failures.is_empty(),
        "{} example(s) in standard library documentation comments do not do what they say:\n\n{}",
        failures.len(),
        examples::report(&failures)
    );
    // A corpus that discovers nothing passes every assertion. The whole corpus
    // owes eight examples across four modules, and each shard its share of
    // that, rounded up.
    let (least_blocks, least_modules) = (8usize.div_ceil(count), 4usize.div_ceil(count));
    assert!(
        blocks >= least_blocks && modules >= least_modules,
        "shard {at} of {count} has only {blocks} example(s) across {modules} module(s), \
         under its share of {least_blocks} across {least_modules}; the extractor is missing them"
    );
}

shards! {
    standard_library_doc_comments(doc_comments_shard, documented_module_count) =
        shard_0 shard_1 shard_2 shard_3;
}

/// Every standard library module with an example in its doc comments: where it
/// lives, the document its comments make, and how many examples are compiled.
fn documented_modules(root: &Path) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    for module in buri::compiler::standard_library::MODULES {
        // The source is a field of the entry rather than a second table keyed
        // by path, so a listed module with no source is unrepresentable.
        let compiled_in = module.source;
        if !examples::has_examples(compiled_in) {
            continue;
        }
        // The name a failure reports: where the module actually lives, so the
        // line number is one an editor can open.
        let Some(rel) = source_path(root, compiled_in) else {
            continue;
        };
        let text = examples::doc_comments(&source_text(root, &rel, compiled_in));
        let extracted = examples::extract(&rel, &text);
        let found = extracted.blocks.len();
        // A fence that does not extract is a failure to report, not a module
        // with nothing in it.
        if found == 0 && extracted.failures.is_empty() {
            continue;
        }
        out.push((rel, text, found));
    }
    out
}

fn documented_module_count() -> usize {
    documented_modules(&repo_root()).len()
}

/// How many fences the formatter is allowed to have nothing to say about.
///
/// A page about a syntax error shows the syntax error, and the formatter
/// refuses text it could not read whole — so a silence is expected and a
/// *growing* number of them is not. The ceiling is one number: it may be
/// lowered and not raised, and raising it is a line in the same diff as the
/// fence that needed it.
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
        // A blessing run rewrites a page with `format_document`, textproto
        // fences included, so a page it would rewrite is one this run fails.
        if file.ends_with(".md") && layout::format_document(file, text).is_some() {
            drifted.push(format!("{file}: a fence `buri format` would rewrite"));
        }
    };

    for t in topics::TOPICS {
        census(&topic_path(t), &document(&root, &topic_path(t), t.text));
    }
    for c in buri::commands::COMMANDS {
        let rel = format!("crates/docs/src/docs/reference/cli/{}.md", c.name);
        census(&rel, &document(&root, &rel, c.doc));
    }
    // The catalogs. Their pages are markdown like any other, and the program on
    // an error page is the one a reader copies to reproduce the error.
    for e in buri::documentation::errors::ERRORS {
        let rel = format!("crates/docs/src/docs/reference/errors/{}.md", e.code);
        census(&rel, &document(&root, &rel, e.text));
    }
    for l in buri::documentation::lints::LINTS {
        let rel = format!("crates/docs/src/docs/reference/lints/{}.md", l.code);
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
