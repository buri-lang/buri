//! The site, built from the documentation that is actually in the tree.
//!
//! The command under test is the compiled `website` binary rather than a
//! nested `cargo run`: `cargo test` already holds the lock on the build
//! directory, so a test that shelled out to cargo would wait for itself. The
//! binary here is the one `cargo run -p website` runs, built from the same
//! sources by the same invocation, and it is given the same arguments.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the promise is about the toolchain, not about the harness that \
              drives it; a test that unwraps fails on the line that broke"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the workspace root").to_path_buf()
}

fn temporary(name: &str) -> PathBuf {
    let at = std::env::temp_dir().join(format!("buri-website-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&at);
    at
}

fn website(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_website"))
        .arg("--root")
        .arg(repository())
        .args(arguments)
        .output()
        .expect("the website binary runs")
}

fn succeeded(output: &Output, what: &str) {
    if !output.status.success() {
        panic!(
            "{what} failed with {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Every file under a directory, relative to it, in a stable order.
fn tree(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                found.push(relative.to_path_buf());
            }
        }
    }
    found.sort();
    found
}

/// `cargo run -p website`, on the documentation in this checkout.
#[test]
fn the_site_builds_from_the_documentation_in_the_tree() {
    let out = temporary("build");
    let output = website(&["--out", out.to_str().unwrap()]);
    succeeded(&output, "the build");

    for page in [
        "index.html",
        "assets/site.css",
        "guide/installing/index.html",
        "language/lexical/index.html",
        "language/specification/index.html",
        "build/build-files/index.html",
        "cli/build/index.html",
        "errors/circular-import/index.html",
        "lints/dead-code/index.html",
        "skills/buri-language/index.html",
        "reference/grammar/index.html",
    ] {
        assert!(out.join(page).is_file(), "`{page}` was not written");
    }

    let front = std::fs::read_to_string(out.join("index.html")).unwrap();
    assert!(front.starts_with("<!doctype html>"), "the front page is not a document");
    assert!(front.contains("<title>Buri"), "the front page has no title");
    assert!(front.contains("data-theme-picker"), "the theme picker is not on the page");
    assert!(
        front.contains("https://github.com/buri-lang/buri/blob/main/README.md"),
        "the front page does not offer to edit the README"
    );
    assert!(
        front.contains("<span class=\"keyword\">export</span>"),
        "the README's Buri example was not highlighted at build time"
    );
    assert!(!front.contains("<script src="), "the site loads no external script");

    let _ = std::fs::remove_dir_all(&out);
}

/// `--check`, which is the link checker and the reason it exists.
#[test]
fn every_link_the_site_writes_resolves() {
    let output = website(&["--check"]);
    succeeded(&output, "--check");
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(said.contains("every link resolves"), "{said}");
}

/// Same input, same bytes. A generator that stamped a build time would fail
/// here, and so would one that walked a `HashMap`.
#[test]
fn two_builds_of_one_tree_are_the_same_bytes() {
    let first = temporary("once");
    let second = temporary("twice");
    succeeded(&website(&["--out", first.to_str().unwrap()]), "the first build");
    succeeded(&website(&["--out", second.to_str().unwrap()]), "the second build");

    let files = tree(&first);
    assert_eq!(files, tree(&second), "the two builds wrote different files");
    assert!(files.len() > 200, "only {} files; the site is not the whole corpus", files.len());
    for file in &files {
        let before = std::fs::read(first.join(file)).unwrap();
        let after = std::fs::read(second.join(file)).unwrap();
        assert!(before == after, "`{}` differs between two builds", file.display());
    }

    let _ = std::fs::remove_dir_all(&first);
    let _ = std::fs::remove_dir_all(&second);
}

/// Nothing generated lands anywhere tracked, and the default output is the
/// one directory the repository already ignores.
#[test]
fn the_default_output_is_under_target() {
    let out = repository().join(website::options::DEFAULT_OUTPUT);
    assert!(out.starts_with(repository().join("target")));
}
