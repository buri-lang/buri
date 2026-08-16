//! Parses every `.buri` source in the repository that is meant to compile: the
//! worked monorepo, the conformance suite, and the abort corpus. That is the
//! body of source the grammar was written against, so anything that fails to
//! parse here is either a parser bug or drift between the two.
//!
//! `tests/reject/` is left out on purpose — those files are supposed to be
//! turned away, some of them by the parser, and each one's expectation is
//! checked exactly by the reject harness in `conformance.rs`.

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

/// The files whose text is Buri source rather than textproto.
fn corpus(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    buri_sources(&root.join("cli/tests/example"), &mut files);
    buri_sources(&root.join("cli/tests/conformance"), &mut files);
    buri_sources(&root.join("cli/tests/crash"), &mut files);
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
        let once = buri::build::textproto::print(&buri::build::textproto::parse(&text, buri::diagnostics::FileId(0)).doc);
        let twice = buri::build::textproto::print(&buri::build::textproto::parse(&once, buri::diagnostics::FileId(0)).doc);
        assert_eq!(once, twice, "formatting {} is not stable", path.display());
    }
}

/// Formatting is a fixed point and never produces something that does not
/// parse. That the *meaning* survives is checked by `tests/format_builds.rs`,
/// which formats the whole corpus and compiles it again — a token comparison
/// cannot express it, because the formatter is allowed to drop a redundant
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

/// The toolchain has no dependencies, and that is a claim a test should hold
/// rather than a comment.
///
/// It became load-bearing when `editors/zed` arrived: a Zed extension has no
/// choice about depending on `zed_extension_api`, so it lives outside the
/// workspace. Adding it to `members` is a one-line change that would make
/// `cargo test -p buri` resolve crates.io, and nothing else would notice.
#[test]
fn the_toolchain_still_has_no_dependencies() {
    let cli = std::fs::read_to_string(repo_root().join("cli/Cargo.toml")).expect("cli/Cargo.toml");
    // Everything after a `[dependencies]`-ish header, up to the next section.
    let mut in_deps = false;
    for line in cli.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_deps = line.contains("dependencies");
            continue;
        }
        if in_deps && !line.is_empty() && !line.starts_with('#') {
            panic!(
                "cli/Cargo.toml declares a dependency: `{line}`\n\
                 The toolchain is pinned by hash, and a dependency tree is a second thing to pin."
            );
        }
    }

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
