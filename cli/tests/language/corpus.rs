//! Parses every `.buri` source in the repository that is meant to compile: the
//! worked monorepo, the conformance suite, the abort corpus, and the small
//! programs the JavaScript goldens are recorded from. That is the body of
//! source the grammar was written against, so anything that fails to parse
//! here is either a parser bug or drift between the two.
//!
//! It is also where the formatter is held to its three properties: it is a
//! fixed point, it keeps every comment, and it preserves what the corpus
//! means.
//!
//! `tests/reject/` is left out on purpose — those files are supposed to be
//! turned away, some of them by the parser, and each one's expectation is
//! checked exactly by the reject harness in `language/conformance.rs`.
use crate::harness;

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/cli.
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn buri_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            buri_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "buri") {
            // BUILD.buri and REPO.buri are textproto, not source.
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name != "BUILD.buri" && name != "REPO.buri" {
                out.push(p);
            }
        }
    }
}

/// Every `.rs` file under `dir`, for the tests that read this repository's own
/// source rather than its corpora.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The files whose text is Buri source rather than textproto.
fn corpus(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    buri_sources(&root.join("cli/tests/example"), &mut files);
    buri_sources(&root.join("cli/tests/conformance"), &mut files);
    buri_sources(&root.join("cli/tests/crash"), &mut files);
    buri_sources(&root.join("cli/tests/golden_javascript"), &mut files);
    // The documentation's shared preambles are real source and are held to the
    // same standard: they parse, and `buri format` leaves them alone.
    buri_sources(&root.join("cli/src/docs/harness"), &mut files);
    files
}

#[test]
fn every_source_in_the_repository_parses() {
    let root = repo_root();
    let files = corpus(&root);
    assert!(files.len() > 30, "expected the corpus, found {} files", files.len());

    let mut failures = String::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let mut map = buri::diagnostics::SourceMap::new();
        let id = map.add(rel.clone(), path.clone(), text.clone());
        let parsed = buri::parsing::parser::parse(&text, id);
        for e in &parsed.errors {
            failures.push_str(&map.render(e, false));
        }
    }
    assert!(failures.is_empty(), "the corpus does not parse:\n{failures}");
}

fn proto_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            proto_files(&p, out);
        } else {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name == "BUILD.buri" || name == "REPO.buri" {
                out.push(p);
            }
        }
    }
}

#[test]
fn every_build_file_reads() {
    let root = repo_root();
    let mut files = Vec::new();
    proto_files(&root.join("cli/tests/example"), &mut files);
    assert!(files.len() >= 7, "expected the example build files, found {}", files.len());

    let mut failures = String::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let mut map = buri::diagnostics::SourceMap::new();
        let id = map.add(rel.clone(), path.clone(), text.clone());
        let errors = if path.file_name().unwrap() == "REPO.buri" {
            buri::build::buildfile::read_repo_config(&text, id).errors
        } else {
            buri::build::buildfile::read_build_file(&text, id).errors
        };
        for e in &errors {
            failures.push_str(&map.render(e, false));
        }
    }
    assert!(failures.is_empty(), "the example build files do not read:\n{failures}");
}

/// `buri format` and `buri gen` must never fight over a file, so formatting is
/// a fixed point: the example build files are already formatted.
#[test]
fn formatting_build_files_is_a_fixed_point() {
    let root = repo_root();
    let mut files = Vec::new();
    proto_files(&root.join("cli/tests/example"), &mut files);
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let file = buri::diagnostics::FileId(0);
        let format = |source: &str| {
            buri::build::textproto::print(&buri::build::textproto::parse(source, file).document)
        };
        let once = format(&text);
        let twice = format(&once);
        assert_eq!(once, twice, "formatting {} is not stable", path.display());
    }
}

/// Formatting is a fixed point and never produces something that does not
/// parse. That the *meaning* survives is
/// `formatting_the_corpus_preserves_what_it_means`, below, which formats the
/// conformance repository and runs it again — a token comparison cannot
/// express it, because the formatter is allowed to drop a redundant
/// parenthesis and an optional trailing comma.
#[test]
fn formatting_is_a_fixed_point() {
    let root = repo_root();
    let files = corpus(&root);

    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let Some(once) = buri::formatting::source(&text) else {
            panic!("{} does not format", path.display());
        };
        let twice = buri::formatting::source(&once).expect("formatted output re-formats");
        assert_eq!(once, twice, "formatting {} is not stable", path.display());
    }
}

/// Formatting keeps every comment.
///
/// A fixed point cannot see this on its own: `format(format(x)) == format(x)`
/// holds perfectly well when both sides have lost the same comment. So the
/// comments are compared against the *source*, as a set rather than a
/// sequence, because the leading import run is sorted and a comment travels
/// with the import it was written above.
///
/// This is what `token_shape` was built for. The token half of the shape is
/// deliberately not compared: the formatter may drop a redundant parenthesis
/// and add a trailing comma, and a test that forbade that would be a test
/// against formatting.
///
/// `source_unchecked` rather than `source`, on purpose. `source` refuses
/// output whose comments moved, so asking it this question can only ever get
/// the answer yes; the property belongs to the printer underneath.
#[test]
fn formatting_keeps_every_comment() {
    let root = repo_root();
    let files = corpus(&root);
    let mut comments = 0;

    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let once = buri::formatting::source_unchecked(&text);
        let (mut before, mut after) =
            (buri::formatting::comment_shape(&text), buri::formatting::comment_shape(&once));
        comments += before.len();
        before.sort();
        after.sort();
        assert_eq!(
            before,
            after,
            "formatting {} does not keep its comments",
            path.strip_prefix(&root).unwrap().display()
        );
    }
    assert!(comments > 500, "only {comments} comments in the corpus; the scan is not working");
}

/// Formatting preserves *meaning*: the conformance repository, formatted from
/// end to end, still compiles and still gets the same answers.
///
/// This is the property `language/corpus.rs` used to name a file for and never had. It
/// cannot be a comparison of text, because the whole point of the formatter is
/// to change text, and it cannot be a comparison of tokens, because a
/// redundant parenthesis and an optional trailing comma are the formatter's to
/// drop. So it is the same suite run twice, once against the source as checked
/// in and once against the source as formatted.
///
/// A difference rather than a success, deliberately: a suite that is failing
/// for a reason of its own fails the same way in both copies, and what this
/// asks is only that formatting did not change the answer. The floor under the
/// count is what stops that from being vacuous.
#[test]
fn formatting_the_corpus_preserves_what_it_means() {
    let root = repo_root();
    let source = root.join("cli/tests/conformance");
    let plain = harness::Scratch::copy_of("meaning-plain", &source);
    let formatted = harness::Scratch::copy_of("meaning-formatted", &source);

    let mut files = Vec::new();
    buri_sources(&formatted.root, &mut files);
    assert!(files.len() > 30, "expected the suite, found {} files", files.len());
    let mut changed = 0;
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let once = buri::formatting::source(&text)
            .unwrap_or_else(|| panic!("{} does not format", path.display()));
        if once != text {
            changed += 1;
            std::fs::write(path, &once).unwrap();
        }
    }
    assert!(changed > 10, "formatting rewrote only {changed} of the suite's files");

    let before = plain.run(&["test", "//...", "--force"]);
    let after = formatted.run(&["test", "//...", "--force"]);
    assert_eq!(
        after.code,
        before.code,
        "the formatted suite exits {} rather than {}:\n{}",
        after.code,
        before.code,
        harness::indent(&after.all())
    );
    let (want, got) = (before.tests_passed(), after.tests_passed());
    assert!(want > 500, "only {want} assertions ran; the comparison proves nothing");
    assert_eq!(got, want, "the formatted suite passes {got} assertions rather than {want}");
    eprintln!("formatting: {changed} files rewritten, {got} assertions unchanged");
}

/// The dependency **bar**, and that is a claim a test should hold rather than a
/// comment.
///
/// It used to be "no dependencies at all", and native code generation ended
/// that: a retargetable code generator is not something this repository can
/// write. So the promise is replaced by a bar rather than quietly weakened,
/// because a list is a thing people add to and a bar is not (the root
/// `Cargo.toml` states it in full):
///
///   A dependency is admissible only if it is a code generator or a platform
///   interface this repository could not reasonably write, it is behind a
///   cargo feature the default build can turn off, and its absence degrades
///   the toolchain rather than breaking it.
///
/// This checks the two halves a file can check. **The admitted set is closed**:
/// `inkwell`, and nothing else. And **it is optional**, so the default build
/// resolves nothing — which is the clause that makes the other two
/// enforceable, because a feature that could not be turned off is a dependency
/// by another name.
///
/// It stays load-bearing for the reason it was written: a Zed extension has no
/// choice about depending on `zed_extension_api`, so it lives outside the
/// workspace, and adding it to `members` is a one-line change that would make
/// `cargo test -p buri` resolve crates.io with nothing else noticing.
#[test]
fn dependencies_stay_behind_the_bar() {
    /// The whole admitted set, by prefix.
    const ADMITTED: &[&str] = &["inkwell"];
    const BAR: &str = "The bar is in the root Cargo.toml: a dependency is admissible only if \
                       it is a code generator or a platform interface this repository could not \
                       reasonably write, it is behind a cargo feature the default build can turn \
                       off, and its absence degrades the toolchain rather than breaking it.";

    let cli = std::fs::read_to_string(repo_root().join("cli/Cargo.toml")).expect("cli/Cargo.toml");
    let mut in_deps = false;
    let mut seen = 0;
    for line in cli.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // `[dependencies]` and every `[target.'...'.dependencies]`.
            in_deps = line.contains("dependencies");
            continue;
        }
        if !in_deps || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.split(['=', ' ']).next().unwrap_or(line).trim();
        assert!(
            ADMITTED.iter().any(|a| name.starts_with(a)),
            "cli/Cargo.toml declares `{name}`, which is not in the admitted set {ADMITTED:?}.\n{BAR}"
        );
        assert!(
            line.contains("optional = true"),
            "the dependency `{name}` is not optional, so the default build cannot turn it \
             off.\n{BAR}"
        );
        seen += 1;
    }
    assert!(seen > 0, "the admitted dependencies vanished; this test is now asserting nothing");

    // The default feature set is the one `cargo install buri` gets, and it must
    // not be the one that needs LLVM installed (BUILD-AND-WATCH.md §2). The
    // assertion is on the *property* rather than on the exact list: what has to
    // hold is that a default install can compile a native debug binary with no
    // crate behind it, which is `backend-stencil`, and that it does not need
    // LLVM.
    let default = cli
        .lines()
        .find_map(|l| l.trim().strip_prefix("default = "))
        .expect("cli/Cargo.toml declares a default feature set");
    assert!(
        default.contains("backend-stencil"),
        "the default build no longer has a native backend that needs no crate: {default}"
    );
    assert!(
        !default.contains("backend-llvm"),
        "the default feature set changed; `cargo install buri` must not require LLVM"
    );
    assert!(
        cli.contains("backend-llvm = [\"dep:inkwell\"]"),
        "`backend-llvm` no longer gates inkwell, so a default build may now need LLVM 21"
    );

    let root = std::fs::read_to_string(repo_root().join("Cargo.toml")).expect("Cargo.toml");
    assert!(
        root.contains("exclude = [\"editors/zed\"]"),
        "the root Cargo.toml no longer excludes editors/zed, so the Zed extension's \
         dependencies are about to become the toolchain's"
    );
}

/// The editor integration is one directory, and its pieces refer to each other
/// by path. A rename that misses one presents as an extension that installs and
/// then does nothing, which is the hardest kind of breakage to notice.
#[test]
fn the_editor_integration_is_whole() {
    for rel in [
        "editors/tree-sitter-buri/grammar.js",
        "editors/tree-sitter-buri/src/scanner.c",
        "editors/tree-sitter-buri/check.sh",
        "editors/zed/extension.toml",
        "editors/zed/src/lib.rs",
        "editors/zed/languages/buri/config.toml",
        "editors/zed/languages/buri/highlights.scm",
        "editors/zed/languages/buri/indents.scm",
        "editors/zed/languages/buri/outline.scm",
        // The second grammar and the second language: a BUILD.buri ends in
        // `.buri` and is textproto rather than Buri.
        "editors/tree-sitter-buri-build/grammar.js",
        "editors/tree-sitter-buri-build/check.sh",
        "editors/zed/languages/buri-build/config.toml",
        "editors/zed/languages/buri-build/highlights.scm",
        "editors/zed/languages/buri-build/indents.scm",
        "editors/zed/languages/buri-build/outline.scm",
    ] {
        assert!(repo_root().join(rel).is_file(), "{rel} is missing");
    }

    // The queries live once. `check.sh` compiles them from the Zed directory,
    // so a second copy under the grammar would be a second thing to keep in
    // step and nothing would report the drift.
    assert!(
        !repo_root().join("editors/tree-sitter-buri/queries").exists(),
        "there are two copies of the highlight queries; there must be one"
    );
}

/// **The `backend-llvm` delta is confined to the files the verification bar
/// names**, so the bar may run the feature suite as a *delta* rather than as a
/// second copy of the default one.
///
/// `cli/tests/README.md`'s "The five-minute budget" defines the bar, and its
/// feature leg runs only the LLVM-gated tests: the fifteen `backend::llvm` unit
/// tests, `native::llvm`, `native::agreement`, and the whole `fuzz` binary.
/// Everything else in a `--features backend-llvm` run is byte-for-byte the same
/// work the default leg just did, and running it twice buys nothing — which is
/// **dedup, not less coverage**, and only while this test passes.
///
/// The claim it holds is the one that makes that safe: a `cfg` on
/// `backend-llvm` exists nowhere but here. If it did exist somewhere else, some
/// test outside the delta would behave differently under the feature, the local
/// bar would stop exercising that behaviour, and nothing would say so. Now
/// something does, and the fix is either to widen the bar's feature leg or to
/// move the gate.
///
/// Two things this deliberately does **not** try to be:
///
///  * It is not a reachability analysis. `backend::select` returns the LLVM
///    backend for `(native, Release)` under the feature, so a test that drives
///    a native `--release` build through the CLI would differ without any `cfg`
///    of its own. Every `--release` in the suite today targets a `platform: JS`
///    output, and `native_ready` refuses `Js` before `select` is consulted, so
///    there is no such test — but that is an argument in the report and in the
///    README, not something this file can check.
///  * It is not CI's bar. **CI runs everything under both feature sets**; this
///    is the local wave loop, where the same run happening twice is the whole
///    of what is being removed.
#[test]
fn the_llvm_feature_is_confined_to_the_files_the_bar_names() {
    /// Every file allowed to gate on the feature. A prefix matches a directory:
    /// the backend's own modules are compiled only under it, so a `cfg` inside
    /// one cannot widen anything.
    const GATED: &[&str] = &[
        // `mod llvm`, and `select`'s release arm.
        "cli/src/compiler/backend/mod.rs",
        "cli/src/compiler/backend/llvm/",
        // The module declarations; `llvm` is the 59 tests.
        "cli/tests/native/main.rs",
        "cli/tests/native/llvm.rs",
        // Same-named tests that do *more* under the feature: both hold a
        // `NATIVES` table that gains an `llvm` row, so both are in the delta.
        "cli/tests/native/agreement.rs",
        "cli/tests/fuzz.rs",
    ];

    // Spelled in two pieces so that this file is not its own first hit, and so
    // the guard covers the file the guard is written in.
    let needle = format!("feature = {q}backend-llvm{q}", q = '"');

    let mut sources = Vec::new();
    rust_sources(&repo_root().join("cli/src"), &mut sources);
    rust_sources(&repo_root().join("cli/tests"), &mut sources);
    assert!(sources.len() > 50, "found {} Rust sources; the walk is broken", sources.len());

    let mut gates = 0;
    for path in &sources {
        let rel = path.strip_prefix(repo_root()).unwrap_or(path).to_string_lossy().to_string();
        let text = std::fs::read_to_string(path).unwrap();
        for (n, line) in text.lines().enumerate() {
            // Prose names the feature all over the repository, and prose
            // cannot change what a test does.
            if line.trim_start().starts_with("//") || !line.contains(&needle) {
                continue;
            }
            gates += 1;
            assert!(
                GATED.iter().any(|g| rel.starts_with(g)),
                "{rel}:{} gates on `backend-llvm`, and that file is not in the \
                 verification bar's feature leg. Either move the gate into one of \
                 {GATED:?}, or widen the leg in cli/tests/README.md's \
                 \"The five-minute budget\" and add the file here — but do not \
                 leave the local bar silently not running it.",
                n + 1
            );
        }
    }
    assert!(gates > 0, "no `backend-llvm` gate found at all; this test is asserting nothing");
    eprintln!("llvm delta: {gates} gates, all inside the bar's feature leg");
}

// ---------------------------------------------------------------------------
// The grammar has one source
// ---------------------------------------------------------------------------

/// `editors/tree-sitter-buri/grammar.js` is generated from `grammar.ebnf`, and
/// this is what makes that true rather than aspirational: the file is
/// regenerated in memory and compared byte for byte with the one on disk.
///
/// It is checked in rather than built on demand because an editor installs the
/// grammar without the toolchain — Zed fetches the directory and runs
/// `tree-sitter generate` over it. A build product that ships has to be pinned
/// by something, and here that is this test.
///
/// After a deliberate change to the grammar:
///
/// ```text
/// BURI_BLESS=1 cargo test -p buri --test language corpus::the_tree_sitter_grammar
/// ```
///
/// Then run `editors/tree-sitter-buri/check.sh`, which is the half of the
/// guarantee that needs the tree-sitter CLI.
#[test]
fn the_tree_sitter_grammar_is_generated_from_the_ebnf() {
    let path = repo_root().join("editors/tree-sitter-buri/grammar.js");
    let generated = buri::documentation::grammar::generate(buri::documentation::topics::GRAMMAR)
        .unwrap_or_else(|e| panic!("cli/src/docs/grammar.ebnf does not generate:\n{e}"));

    if std::env::var_os("BURI_BLESS").is_some() {
        std::fs::write(&path, &generated).unwrap();
        eprintln!("editors/tree-sitter-buri/grammar.js: recorded {} bytes", generated.len());
        return;
    }
    let recorded = std::fs::read_to_string(&path).unwrap_or_default();
    if recorded == generated {
        return;
    }
    panic!(
        "editors/tree-sitter-buri/grammar.js is not what the EBNF generates.\n{}\n\
         Edit `cli/src/docs/grammar.ebnf`, never this file. Then record it and run\n\
         editors/tree-sitter-buri/check.sh:\n  \
         BURI_BLESS=1 cargo test -p buri --test language corpus::the_tree_sitter_grammar",
        first_differences(&recorded, &generated)
    );
}

/// A diff short enough to read. `harness::Golden` prints both files whole,
/// which is right for a recorded diagnostic and useless for a five-hundred-
/// line one.
fn first_differences(recorded: &str, generated: &str) -> String {
    let mut out = String::new();
    let want: Vec<&str> = recorded.lines().collect();
    let got: Vec<&str> = generated.lines().collect();
    for i in 0..want.len().max(got.len()) {
        let (a, b) = (want.get(i).copied(), got.get(i).copied());
        if a != b {
            out.push_str(&format!(
                "  line {}:\n    on disk:   {}\n    generated: {}\n",
                i + 1,
                a.unwrap_or("<end of file>"),
                b.unwrap_or("<end of file>")
            ));
            if out.lines().count() > 40 {
                out.push_str("  ...\n");
                break;
            }
        }
    }
    out
}

/// Every non-terminal the EBNF names is a production the EBNF declares.
///
/// Nothing used to execute the EBNF, so a renamed production could leave a
/// reference behind and the file would still read as though it meant
/// something. Generation would fail on it, but this says which name and where
/// rather than failing somewhere downstream.
#[test]
fn every_reference_in_the_ebnf_resolves() {
    let ebnf = buri::documentation::grammar::parse(buri::documentation::topics::GRAMMAR)
        .unwrap_or_else(|e| panic!("cli/src/docs/grammar.ebnf does not parse:\n{e}"));
    let dangling = buri::documentation::grammar::dangling_references(&ebnf);
    assert!(dangling.is_empty(), "cli/src/docs/grammar.ebnf:\n  {}", dangling.join("\n  "));
    assert!(
        ebnf.productions.len() > 80,
        "only {} productions were read; the file is not being parsed",
        ebnf.productions.len()
    );
}

// ---------------------------------------------------------------------------
// The removed backend leaves no dangling pointers
// ---------------------------------------------------------------------------

/// Every file under `dir` whose bytes are text this repository writes by hand
/// or records as a golden, for the tests that read the tree itself.
fn text_files(dir: &Path, out: &mut Vec<PathBuf>) {
    const KINDS: &[&str] =
        &["rs", "c", "h", "md", "toml", "buri", "proto", "json", "jsonl", "txt", "sh", "js"];
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            text_files(&p, out);
        } else if p.extension().is_some_and(|x| KINDS.iter().any(|k| x == *k)) {
            out.push(p);
        }
    }
}

/// No file under `cli/` names the backend that was removed on 2026-08-29,
/// outside the allow-list below.
///
/// Its directory under `cli/src/compiler/backend/` and its document under
/// `design/native/` are both gone (`design/native/CODEGEN-STENCIL.md` §13).
/// Ninety-nine comments went on pointing at files inside them — a reader who
/// follows one opens nothing — and nothing held the count down, so it could
/// only grow. This is what holds it at the allow-list.
///
/// A comparison that has lost its subject is deleted; one that still has a
/// surviving twin points at the twin, under `stencil/` or `llvm/`; a fact
/// about the history is kept without the name. The history itself lives in
/// `design/`, which is outside this walk, and so is `reference/`, which cites
/// the upstream wasmtime tree rather than this one.
#[test]
fn the_removed_backend_is_not_cross_referenced() {
    /// Every file allowed to name it, and why.
    const ALLOWED: &[&str] = &[
        // The backend's name is a term of the codegen key, and the test that
        // pins that has to write down a name no toolchain answers to any more.
        "cli/src/build/actions.rs",
    ];

    // Spelled in two pieces so this file is not its own first hit.
    let needle = format!("crane{}", "lift");

    let mut files = Vec::new();
    text_files(&repo_root().join("cli"), &mut files);
    assert!(files.len() > 500, "found {} files under cli/; the walk is broken", files.len());

    let mut hits = 0;
    for path in &files {
        let rel = path.strip_prefix(repo_root()).unwrap_or(path).to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for (n, line) in text.lines().enumerate() {
            if !line.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            hits += 1;
            assert!(
                ALLOWED.contains(&rel.as_str()),
                "{rel}:{} names the removed backend, and that file is not on the \
                 allow-list {ALLOWED:?}. Say what is true now — the surviving \
                 counterpart under `stencil/` or `llvm/`, or the claim without the \
                 pointer — rather than citing a file the tree no longer has. \
                 `design/native/CODEGEN-STENCIL.md` §13 is where the history lives.",
                n + 1
            );
        }
    }
    eprintln!("removed-backend mentions under cli/: {hits}, all on the allow-list");
}
