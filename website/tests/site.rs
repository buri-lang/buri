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
        "getting-started/index.html",
        "getting-started/installing/index.html",
        "guides/index.html",
        "guides/testing/index.html",
        "language/lexical/index.html",
        "reference/index.html",
        "reference/standard-library/index.html",
        "reference/build/build-files/index.html",
        "reference/cli/index.html",
        "reference/errors/index.html",
        "reference/errors/circular-import/index.html",
        "reference/lints/index.html",
        "reference/lints/dead-code/index.html",
        "reference/skills/buri-language/index.html",
        "reference/grammar/index.html",
        "reference/build-schema/index.html",
        "reference/repo-schema/index.html",
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
    assert!(
        front.contains("preloadOnHover"),
        "the front page does not preload the pages its links point at"
    );

    let _ = std::fs::remove_dir_all(&out);
}

/// The masthead is the four sections and nothing else, on every page.
#[test]
fn the_masthead_names_the_four_sections() {
    let out = temporary("masthead");
    succeeded(&website(&["--out", out.to_str().unwrap()]), "the build");

    for page in ["index.html", "language/lexical/index.html", "reference/errors/circular-import/index.html"]
    {
        let html = std::fs::read_to_string(out.join(page)).unwrap();
        let (_, rest) = html.split_once("<nav aria-label=\"Sections\">").expect("a masthead");
        let (bar, _) = rest.split_once("</nav>").expect("the masthead ends");
        for title in ["Getting started", "Guides", "The language", "Reference"] {
            assert!(bar.contains(&format!(">{title}<")), "`{page}` omits `{title}`:\n{bar}");
        }
        assert_eq!(bar.matches("<a href").count(), 4, "`{page}` has more than four sections");
    }

    let _ = std::fs::remove_dir_all(&out);
}

/// The reference navigates by group, and the two catalogues are represented by
/// their index pages rather than by two hundred and forty links.
#[test]
fn the_reference_is_grouped_and_the_catalogues_are_not_in_the_sidebar() {
    let out = temporary("reference");
    succeeded(&website(&["--out", out.to_str().unwrap()]), "the build");

    let hub = std::fs::read_to_string(out.join("reference/index.html")).unwrap();
    for group in
        ["Standard library", "Build", "The CLI", "Errors", "Lints", "Agent skills", "Normative"]
    {
        assert!(hub.contains(&format!(">{group}<")), "the reference index omits `{group}`");
    }

    // A code page's sidebar shows the groups, not its two hundred siblings.
    let page =
        std::fs::read_to_string(out.join("reference/errors/circular-import/index.html")).unwrap();
    let (_, rest) = page.split_once("class=\"sidebar\"").expect("a sidebar");
    let (sidebar, _) = rest.split_once("</nav>").expect("the sidebar ends");
    assert!(sidebar.contains(">Errors<"), "the sidebar has no Errors group:\n{sidebar}");
    assert!(
        !sidebar.contains("unresolved-name"),
        "the sidebar lists sibling codes:\n{sidebar}"
    );
    assert!(sidebar.matches("<li>").count() < 40, "the sidebar is a catalogue:\n{sidebar}");

    // The index page is where the whole catalogue is written down.
    let index = std::fs::read_to_string(out.join("reference/errors/index.html")).unwrap();
    for code in ["circular-import", "unresolved-name", "type-mismatch"] {
        assert!(index.contains(code), "the errors index omits `{code}`");
    }

    let _ = std::fs::remove_dir_all(&out);
}

/// One page for the CLI, generated from the table that dispatches.
#[test]
fn the_cli_page_holds_every_command() {
    let out = temporary("cli");
    succeeded(&website(&["--out", out.to_str().unwrap()]), "the build");
    let html = std::fs::read_to_string(out.join("reference/cli/index.html")).unwrap();

    for command in buri::commands::COMMANDS.iter().filter(|c| !c.hidden) {
        assert!(
            html.contains(&format!("buri {}</a>", command.name))
                || html.contains(&format!("buri {}<a", command.name)),
            "the CLI page has no block for `buri {}`",
            command.name
        );
    }
    assert!(html.contains("--error-format=json"), "the intro's global flags are missing");
    assert!(html.contains("<h2"), "the commands are not headings");
    assert!(!html.contains("<h1 id=\"buri-build\""), "a command is a second title on the page");

    let _ = std::fs::remove_dir_all(&out);
}

/// The standard library, module by module, generated from the API the compiler
/// read out of the sources it compiled.
///
/// Forty-odd modules are a listing rather than a navigation, so what is checked
/// here is both halves of that: every module has a page, the catalogue names
/// every one of them, and the sidebar names the catalogue instead.
#[test]
fn every_standard_library_module_has_a_page_the_catalogue_names() {
    let out = temporary("std");
    succeeded(&website(&["--out", out.to_str().unwrap()]), "the build");

    let catalogue = std::fs::read_to_string(out.join("reference/std/index.html")).unwrap();
    for module in buri::compiler::standard_library::MODULES {
        let page = out.join("reference/std").join(module.path).join("index.html");
        assert!(page.is_file(), "`{}` has no page", module.path);
        assert!(
            catalogue.contains(&format!(">{}</a>", module.path)),
            "the catalogue omits `{}`",
            module.path
        );
    }

    let list = std::fs::read_to_string(out.join("reference/std/core/list/index.html")).unwrap();
    let (_, rest) = list.split_once("class=\"sidebar\"").expect("a sidebar");
    let (sidebar, _) = rest.split_once("</nav>").expect("the sidebar ends");
    assert!(sidebar.contains(">Every module<"), "the sidebar omits the catalogue:\n{sidebar}");
    assert!(!sidebar.contains(">core/queue<"), "the sidebar lists the modules:\n{sidebar}");

    // An item's anchor is the item's own name, so `reference/std/core/list#map`
    // is a link somebody can write down.
    assert!(list.contains("id=\"map\""), "`core/list` has no anchor for `map`");
    assert!(
        list.contains("<span class=\"keyword\">fn</span> <span class=\"identifier\">map</span>"),
        "the signature was not highlighted at build time"
    );
    assert!(list.contains("A method on <code>[A]</code>"), "the receiver is not named");
    assert!(
        list.contains("standard_library/sources/list.buri"),
        "the page does not offer to edit the module's own source"
    );

    // And the prose map still points at them.
    let map = std::fs::read_to_string(out.join("reference/standard-library/index.html")).unwrap();
    assert!(
        map.contains("reference/std/core/list/"),
        "the standard library page does not link to `core/list`"
    );

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
