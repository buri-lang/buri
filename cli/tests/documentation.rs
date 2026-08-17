//! The documentation's own tests.
//!
//! `documentation_examples.rs` compiles what the documents *show*. This file checks what the
//! documents *are*: that every fence is scannable and tagged, that every link
//! resolves, and — once assembly lands — that the checked-in `SPEC.md` still
//! matches what `buri docs assemble` produces.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test code. The lint set in `Cargo.toml` pins a promise about the \
              toolchain — that no input panics it — and a harness that drives \
              the toolchain is not the toolchain. A test that unwraps fails on \
              the line that broke, which is what a test is for, and threading \
              `?` through an assertion buys nothing. `clippy.toml` exempts \
              `#[test]` functions already; this covers the helpers around them."
)]
use buri::documentation::{assemble, markdown, topics};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/cli.
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Every markdown document the toolchain is responsible for.
///
/// Adding a document here is the one-line registration that subjects it to
/// every test in this file.
const DOCUMENTS: &[&str] = &[
    "README.md",
    "SPEC.md",
    "TODO.md",
    "ECOSYSTEM-FEATURES.md",
    "STANDARD-LIBRARY.md",
    "cli/src/docs/build/overview.md",
    "cli/src/docs/build/build-files.md",
    "cli/src/docs/build/libraries.md",
    "cli/src/docs/build/tags.md",
    "cli/src/docs/build/testing.md",
    "cli/src/docs/build/repo-config.md",
    "cli/src/docs/build/cli.md",
    "cli/src/docs/build/proto.md",
    "cli/src/docs/build/hermeticity.md",
    "cli/tests/README.md",
];

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn every_document_is_scannable() {
    let mut problems = Vec::new();
    for doc in DOCUMENTS {
        let text = read(doc);
        if let Some(line) = markdown::unterminated_fence(&text) {
            problems.push(format!("{doc}:{line}: this ``` is never closed"));
        }
        for f in markdown::fences(&text) {
            if let Err(msg) = markdown::parse_info(f.raw_info) {
                problems.push(format!("{doc}:{}: {msg}", f.line));
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// A census, so that a scanner regression shows up as a changed count rather
/// than as every downstream suite passing vacuously over zero blocks.
#[test]
fn the_fence_census_is_what_we_think_it_is() {
    let mut buri = 0;
    let mut textproto = 0;
    let mut other = Vec::new();
    for doc in DOCUMENTS {
        for f in markdown::fences(&read(doc)) {
            match f.lang {
                "buri" => buri += 1,
                "textproto" => textproto += 1,
                "" => other.push(format!("{doc}:{}: fence with no language", f.line)),
                lang => other.push(format!("{doc}:{}: {lang}", f.line)),
            }
        }
    }
    assert!(buri >= 100, "only {buri} buri fences; the scanner is missing blocks");
    assert!(textproto >= 20, "only {textproto} textproto fences");
    eprintln!("buri: {buri}, textproto: {textproto}, other: {}", other.len());
}

/// Every `[text](target)` and `[text](#anchor)` points at something real.
///
/// This is the cheapest staleness catch in the repository: a renamed heading
/// or a moved file breaks it immediately, and nothing else notices.
#[test]
fn every_link_resolves() {
    let root = repo_root();
    let mut broken = Vec::new();
    for doc in DOCUMENTS {
        let text = read(doc);
        let anchors: Vec<String> =
            markdown::headings(&text).iter().map(|h| markdown::slug(h.title)).collect();
        // A topic under `cli/src/docs/` is a fragment of a document that lives
        // at the repository root, so its links are written relative to the
        // root. One rule, and it is the rule the assembled file needs.
        let dir = if doc.starts_with("cli/src/docs/") {
            root.clone()
        } else {
            root.join(doc).parent().unwrap().to_path_buf()
        };

        for link in markdown::links(&text) {
            match &link.dest {
                markdown::Dest::External { .. } => {}
                markdown::Dest::Nowhere => {
                    broken.push(format!("{doc}:{}: `[{}]()` points nowhere", link.line, link.text));
                }
                markdown::Dest::SameDoc { anchor } => {
                    if !anchors.contains(anchor) {
                        broken.push(format!(
                            "{doc}:{}: `#{anchor}` is not a heading in this document",
                            link.line
                        ));
                    }
                }
                markdown::Dest::File { path, anchor } => {
                    let target = dir.join(path);
                    if !target.exists() {
                        broken.push(format!("{doc}:{}: `{path}` does not exist", link.line));
                        continue;
                    }
                    let Some(anchor) = anchor.as_deref().filter(|_| path.ends_with(".md")) else {
                        continue;
                    };
                    let other = std::fs::read_to_string(&target).unwrap_or_default();
                    let other_anchors: Vec<String> = markdown::headings(&other)
                        .iter()
                        .map(|h| markdown::slug(h.title))
                        .collect();
                    if !other_anchors.iter().any(|a| a == anchor) {
                        broken.push(format!(
                            "{doc}:{}: `{path}` has no heading `#{anchor}`",
                            link.line
                        ));
                    }
                }
            }
        }
    }
    assert!(broken.is_empty(), "\n{}", broken.join("\n"));
}

/// The checked-in `SPEC.md` and `README.md` must be exactly what their topics
/// assemble to. This is the whole guarantee that there is one copy of every
/// sentence: edit a topic and forget to regenerate, and this fails.
#[test]
fn the_assembled_documents_are_not_stale() {
    let drifted = assemble::drifted(&repo_root());
    let names: Vec<&str> = drifted.iter().map(|(p, _)| *p).collect();
    assert!(
        drifted.is_empty(),
        "{} is stale.\n  Run `buri docs assemble` and commit the result.\n  \
         Edit the topics under cli/src/docs, never the assembled file.",
        names.join(", ")
    );
}

/// Every keyword the grammar names is a keyword the lexer has, and vice versa.
/// The grammar is hand-written and cannot be generated from the parser, so
/// this is what keeps the two honest about the one list they must share.
#[test]
fn every_grammar_keyword_is_a_keyword() {
    let grammar = topics::GRAMMAR;
    let mut missing = Vec::new();
    for kw in buri::parsing::lexer::Kw::ALL {
        let quoted = format!("\"{}\"", kw.text());
        if !grammar.contains(&quoted) {
            missing.push(kw.text());
        }
    }
    assert!(
        missing.is_empty(),
        "the grammar never mentions these keywords: {}",
        missing.join(", ")
    );
}

/// Every topic is served by the CLI under the id the registry gives it.
#[test]
fn every_topic_is_servable() {
    for source in buri::documentation::sources() {
        for entry in source.entries() {
            assert!(
                source.resolve(&entry.id).is_some(),
                "`{}` is listed but does not resolve",
                entry.id
            );
        }
    }
}

/// The manifest is the contract an agent reads before it asks for anything.
/// Every id it advertises must answer with exit 0 — otherwise the contract is
/// a lie and the agent's first request fails.
#[test]
fn every_manifest_id_is_fetchable() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_buri"))
        .args(["docs", "manifest"])
        .current_dir(std::env::temp_dir())
        .output()
        .expect("the buri binary runs");
    assert!(out.status.success(), "`buri docs manifest` failed");
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.lines().count(), 1, "the manifest must be one line");

    // Pull out every `"id":"..."` without a JSON parser — the toolchain has no
    // dependencies, and asserting the invariant matters more than re-parsing
    // our own output.
    let mut ids = Vec::new();
    let mut rest = text.as_ref();
    while let Some(at) = rest.find("\"id\":\"") {
        rest = &rest[at + 6..];
        let end = rest.find('"').expect("an id is terminated");
        ids.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    assert!(ids.len() > 30, "the manifest advertises only {} pages", ids.len());

    let mut broken = Vec::new();
    for id in &ids {
        let got = std::process::Command::new(env!("CARGO_BIN_EXE_buri"))
            .args(["docs", id, "--format=json"])
            .current_dir(std::env::temp_dir())
            .output()
            .expect("the buri binary runs");
        if !got.status.success() {
            broken.push(id.clone());
        }
    }
    assert!(broken.is_empty(), "the manifest advertises pages that do not exist: {broken:?}");
}

/// `buri docs` has to work where there is no repository — that is most of
/// where an agent will run it.
#[test]
fn docs_work_outside_a_repository() {
    for args in [
        vec!["docs"],
        vec!["docs", "lang/effects"],
        vec!["docs", "search", "effects"],
        vec!["docs", "grammar"],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_buri"))
            .args(&args)
            .arg("--color=never")
            .current_dir(std::env::temp_dir())
            .output()
            .expect("the buri binary runs");
        assert!(out.status.success(), "`buri {}` failed outside a repository", args.join(" "));
        assert!(!out.stdout.is_empty(), "`buri {}` printed nothing", args.join(" "));
    }
}

/// An unknown topic is a bad invocation, not a failure of the thing asked
/// about, so it exits 2 and suggests something.
#[test]
fn an_unknown_topic_exits_two_with_a_suggestion() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_buri"))
        .args(["docs", "lang/effect", "--color=never"])
        .current_dir(std::env::temp_dir())
        .output()
        .expect("the buri binary runs");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("lang/effects"), "expected a suggestion, got:\n{err}");
}

/// No document may name a flag the binary does not accept.
///
/// `--check-reproducible`, `query --output=proto`, and `lint --fix` were all
/// documented and all rejected by the parser. The command table is now the one
/// list of flags; this holds the prose to it.
#[test]
fn no_document_invents_a_flag() {
    let known: Vec<&str> = buri::commands::FLAGS.iter().map(|f| f.name).collect();
    let mut invented = Vec::new();
    for doc in DOCUMENTS {
        // `TODO.md` is an audit *of* the gaps, so naming an absent flag is its
        // job. `cli/tests/README.md` documents `cargo`, not `buri`.
        if *doc == "TODO.md" || *doc == "cli/tests/README.md" {
            continue;
        }
        let text = read(doc);
        for (n, line) in text.lines().enumerate() {
            // A line that invokes `cargo` documents cargo's flags, not ours
            // (the install-from-source instructions in the guide).
            if line.trim_start().starts_with("cargo ") {
                continue;
            }
            // Everything after a bare `--` is passed to the program `buri run`
            // executes, so those are its flags and not ours.
            let mut rest = match line.split_once(" -- ") {
                Some((before, _)) => before,
                None => line,
            };
            while let Some(at) = rest.find("--") {
                rest = &rest[at + 2..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '-')
                    .collect();
                // `--` alone (the passthrough marker) and prose em-dashes are
                // not flags.
                if name.len() < 3 || known.contains(&name.as_str()) {
                    continue;
                }
                invented.push(format!("{doc}:{}: `--{name}`", n + 1));
            }
        }
    }
    assert!(
        invented.is_empty(),
        "these flags are documented but not accepted by any command:\n{}",
        invented.join("\n")
    );
}

/// `buri docs lang/types | head` must not panic. A pipe closing early is the
/// reader saying it has enough, not an error — and it is the first thing
/// anybody does with a command that prints a page.
#[test]
fn a_closed_pipe_is_not_an_error() {
    use std::io::Read;
    use std::process::{Command, Stdio};

    for args in [
        vec!["docs", "lang/types"],
        vec!["docs", "search", "match"],
        vec!["docs", "manifest"],
        vec!["docs"],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_buri"))
            .args(&args)
            .arg("--color=never")
            .current_dir(std::env::temp_dir())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the buri binary runs");

        // Read a little, then drop the pipe while the child is still writing.
        let mut out = child.stdout.take().expect("stdout is piped");
        let mut head = [0u8; 64];
        let _ = out.read(&mut head);
        drop(out);

        let finished = child.wait_with_output().expect("the child exits");
        let err = String::from_utf8_lossy(&finished.stderr);
        assert!(
            !err.contains("panicked"),
            "`buri {}` panicked when its reader went away:\n{err}",
            args.join(" ")
        );
    }
}

/// Every exported item in the standard library has a page, and the page is
/// built from the same AST the compiler checked.
#[test]
fn the_standard_library_reference_is_complete() {
    let mut map = buri::diagnostics::SourceMap::new();
    let analysis = buri::compiler::driver::analyze_stdlib(&mut map);
    assert!(!analysis.diags.has_errors(), "the standard library must check");
    let modules = buri::documentation::reference::from_loaded(&analysis.loaded, &buri::documentation::reference::std_filter);

    assert_eq!(modules.len(), buri::compiler::standard_library::MODULES.len(), "a module is missing from the reference");
    let mut empty = Vec::new();
    for m in &modules {
        if m.items.is_empty() {
            empty.push(m.path.clone());
        }
    }
    assert!(empty.is_empty(), "these modules render no items: {empty:?}");
}

/// Every `///` on an exported standard library item ends up on its page. This
/// is what makes the doc comments load-bearing rather than decoration.
#[test]
fn documented_items_carry_their_documentation() {
    let mut map = buri::diagnostics::SourceMap::new();
    let analysis = buri::compiler::driver::analyze_stdlib(&mut map);
    let modules = buri::documentation::reference::from_loaded(&analysis.loaded, &buri::documentation::reference::std_filter);
    let documented: usize = modules.iter().flat_map(|m| &m.items).filter(|i| !i.docs.is_empty()).count();
    assert!(documented > 20, "only {documented} items carry documentation; is `///` still attached?");

    for m in &modules {
        assert!(!m.docs.is_empty(), "{} has no `//!` header", m.path);
        let page = buri::documentation::reference::render(m);
        for item in &m.items {
            for line in &item.docs {
                assert!(
                    page.contains(line.trim()),
                    "{}.{}'s documentation is not on its page: {line}",
                    m.path,
                    item.name
                );
            }
        }
    }
}

/// The same renderer, pointed at a repository, produces that repository's
/// public surface — following the re-exports in `lib.buri`, because the
/// re-export list *is* the API.
#[test]
fn a_workspace_library_renders_its_public_surface() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_buri"))
        .args(["docs", "//lib/money", "--format=markdown"])
        .current_dir(repo_root().join("cli/tests/example"))
        .output()
        .expect("the buri binary runs");
    assert!(out.status.success(), "`buri docs //lib/money` failed");
    let page = String::from_utf8_lossy(&out.stdout);

    // Everything `lib.buri` re-exports, and nothing it deliberately withholds.
    for name in ["Cents", "fromCents", "fromDollars", "parse", "ParseError"] {
        assert!(page.contains(name), "the surface omits `{name}`:\n{page}");
    }
    assert!(
        !page.contains("toCents"),
        "`toCents` stops at the library boundary and must not be listed:\n{page}"
    );
    // A re-exported type brings its methods, because conformance travels with
    // the type.
    assert!(page.contains("Cents.add"), "methods of a re-exported type should be listed");
}

/// `//!` documents the module and may only appear before the first item.
#[test]
fn a_module_doc_comment_must_come_first() {
    let mut map = buri::diagnostics::SourceMap::new();
    let text = "//! fine\n\nexport fn a(): Int { 1 }\n\n//! too late\nexport fn b(): Int { 2 }\n";
    let file = map.add("x".to_string(), std::path::PathBuf::new(), text.to_string());
    let parsed = buri::parsing::parser::parse(map.text(file), file);
    assert!(
        parsed.errors.iter().any(|d| d.message.contains("must come first")),
        "a late `//!` should be reported"
    );
    assert_eq!(parsed.module.docs, vec!["fine".to_string()]);
}

/// Every error page's reproduction still produces the code the page is about.
///
/// This is the invariant that makes the catalog worth having: a page that
/// describes an error the compiler no longer emits is worse than no page,
/// because it reads as current.
#[test]
fn every_error_page_is_provoked_by_its_own_example() {
    let mut failures = Vec::new();
    for e in buri::documentation::errors::ERRORS {
        let doc = format!("cli/src/docs/errors/{}.md", e.code);
        failures.extend(buri::documentation::examples::run_file_at(&repo_root(), &doc, e.text));
    }
    assert!(
        failures.is_empty(),
        "{} error page(s) do not provoke what they describe:\n\n{}",
        failures.len(),
        buri::documentation::examples::report(&failures)
    );
}

/// Every code the compiler can emit has a page, and every page names a code
/// the compiler can emit. Checked against the reject corpus, which is the set
/// of diagnostics we have a worked example of.
#[test]
fn the_catalog_and_the_compiler_agree() {
    let mut emitted = std::collections::BTreeSet::new();
    let dir = repo_root().join("cli/tests/reject");
    for entry in std::fs::read_dir(&dir).expect("the reject corpus exists") {
        let path = entry.expect("a corpus entry").path().join("expected.json");
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            // No JSON parser here on purpose; the field is unambiguous.
            if let Some(at) = line.find("\"code\":\"") {
                let rest = &line[at + 8..];
                let end = rest.find('"').expect("a code is terminated");
                emitted.insert(rest[..end].to_string());
            }
        }
    }
    assert!(emitted.len() > 30, "only {} codes in the corpus", emitted.len());

    let missing: Vec<&String> =
        emitted.iter().filter(|c| buri::documentation::errors::find(c).is_none()).collect();
    assert!(missing.is_empty(), "the compiler emits codes with no page: {missing:?}");
}

/// Every diagnostic in the reject corpus carries a code. An uncoded error is
/// one a reader cannot look up.
#[test]
fn every_rejected_program_names_the_rule_it_broke() {
    let mut uncoded = Vec::new();
    let dir = repo_root().join("cli/tests/reject");
    for entry in std::fs::read_dir(&dir).expect("the reject corpus exists") {
        let case = entry.expect("a corpus entry").path();
        let path = case.join("expected.json");
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if !line.contains("\"code\":\"") {
                uncoded.push(format!("{}", case.file_name().unwrap().to_string_lossy()));
            }
        }
    }
    uncoded.sort();
    uncoded.dedup();
    assert!(uncoded.is_empty(), "these cases produce an uncoded diagnostic: {uncoded:?}");
}

/// `buri docs test` gives any repository the whole documentation-testing
/// apparatus with nothing to configure: an example that imports the
/// repository's own packages just compiles, and one that prints something is
/// run and compared.
#[test]
fn a_repository_can_test_its_own_documentation() {
    let example = repo_root().join("cli/tests/example");

    let ok = std::process::Command::new(env!("CARGO_BIN_EXE_buri"))
        .args(["docs", "test", "--color=never"])
        .current_dir(&example)
        .output()
        .expect("the buri binary runs");
    assert!(
        ok.status.success(),
        "`buri docs test` failed in the worked monorepo:\n{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let said = String::from_utf8_lossy(&ok.stdout);
    assert!(said.contains("compile"), "expected a summary, got: {said}");

    // And it fails when the documentation stops being true. The example's
    // README pins what its program prints; a wrong transcript must be caught.
    let readme = example.join("README.md");
    let original = std::fs::read_to_string(&readme).expect("the README exists");
    let broken = original.replace("a latte costs $4.50", "a latte costs $9.99");
    assert_ne!(broken, original, "the transcript this test perturbs has moved");
    std::fs::write(&readme, &broken).expect("writing the README");

    let bad = std::process::Command::new(env!("CARGO_BIN_EXE_buri"))
        .args(["docs", "test", "--color=never"])
        .current_dir(&example)
        .output()
        .expect("the buri binary runs");
    std::fs::write(&readme, &original).expect("restoring the README");

    assert_eq!(bad.status.code(), Some(1), "a wrong transcript must fail");
    let err = String::from_utf8_lossy(&bad.stderr);
    assert!(
        err.contains("prints something else"),
        "expected the transcript mismatch to be named:\n{err}"
    );
}

/// Every diagnostic code the compiler can emit is documented somewhere.
///
/// `documentation/errors.rs` has said this test exists since it was written; it did not.
/// The loop it closes is the one that matters — `every_error_page_is_provoked_by_its_own_example`
/// checks that each page describes a real error, and this checks the other
/// direction: that no error goes undescribed.
///
/// Two catalogues, because there are two kinds of diagnostic. A compile error
/// can be provoked by one program, so it earns a page with that program on it.
/// A build-graph finding cannot — `dep-cycle` needs two packages — so those
/// live in the CLI reference's tables, next to the command that reports them.
#[test]
fn every_emitted_code_is_documented() {
    let root = repo_root();
    let mut sources = Vec::new();
    rust_sources(&root.join("cli/src"), &mut sources);
    assert!(sources.len() > 20, "only {} Rust sources found", sources.len());

    let catalogue = read("cli/src/docs/build/cli.md");
    let mut undocumented: Vec<String> = Vec::new();
    let mut found = 0;

    for path in &sources {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        for code in codes_in(&text) {
            found += 1;
            let has_page = buri::documentation::errors::find(&code).is_some();
            let in_catalogue = catalogue.contains(&format!("`{code}`"));
            if !has_page && !in_catalogue {
                undocumented.push(format!(
                    "{}: `{code}` is emitted and appears in neither catalogue",
                    path.strip_prefix(&root).unwrap_or(path).display()
                ));
            }
        }
    }
    undocumented.sort();
    undocumented.dedup();

    assert!(
        undocumented.is_empty(),
        "{} code(s) are emitted and documented nowhere:\n  {}\n\nAdd a page under \
         cli/src/docs/errors/ (and register it in documentation/errors.rs) for a diagnostic one \
         program can provoke, or a row in the cli/src/docs/build/cli.md tables for one \
         about the build graph.",
        undocumented.len(),
        undocumented.join("\n  ")
    );
    // A scan that finds nothing passes vacuously.
    assert!(found > 50, "only {found} code sites found; the scan is not working");
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The codes a Rust source attaches, read from `with_code` and `.code`.
/// Textual on purpose: the alternative is a registry every call site has to
/// remember to use, which is the thing that drifts.
fn codes_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["with_code(\"", ".code(\""] {
        let mut rest = text;
        while let Some(i) = rest.find(marker) {
            rest = &rest[i + marker.len()..];
            if let Some(end) = rest.find('"') {
                let code = &rest[..end];
                if !code.is_empty() && code.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
                    out.push(code.to_string());
                }
            }
        }
    }
    out
}
