//! **The `Hermetic()` migration**: the script, and the cases that hold it to
//! what it claims.
//!
//! The rewriter itself is `harness/migrate.rs`, and its doc comment states the
//! two rules it is built on. This target is what runs it:
//!
//! ```text
//! cargo test -p buri --test migrate                                  # the cases
//! cargo test -p buri --test migrate -- --ignored --nocapture         # the migration
//! ```
//!
//! The migration is `#[ignore]`d because it is the one test in the tree that
//! edits a checked-in corpus. Everything else here is ordinary: unit cases over
//! synthetic sources, one per rewrite the table names.

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
              the line that broke, which is what a test is for."
)]

mod harness;
#[path = "harness/migrate.rs"]
mod migrate;

use harness::{indent, repo_root, Scratch};
use migrate::{plan, sources_under, Style};

/// The packages design slice E7 moves. Named here rather than passed in, so
/// that the idempotence sweep and the migration cannot drift apart.
const PACKAGES: &[&str] = &["lib/data", "lib/collections"];

/// The corpus both halves work in.
const CORPUS: &str = "cli/tests/conformance";

// ---------------------------------------------------------------------------
// The rewriter, case by case
// ---------------------------------------------------------------------------

/// Runs the structural half over one source and answers what it would write,
/// with every site bound to the effects named.
fn rewritten(source: &str, effects: &[&'static str]) -> String {
    let mut p = plan("test/case.buri", source);
    assert!(p.refused.is_none(), "{}", p.refused.unwrap());
    for site in &mut p.sites {
        site.effects = effects.iter().copied().collect();
    }
    p.render(Style::Final).0
}

const PREAMBLE: &str = "from \"core/testing/context/lib.buri\" import \
                        { Hermetic, alloc, captureOut, captureErr, stdin, stdinBytes, data, \
                        files, filesBytes, clockAt, randSeed, envOf, readOnly };\n";

#[test]
fn every_free_function_becomes_its_builder() {
    // One case per row of the table, written the way the corpus writes them.
    let cases: &[(&str, &str)] = &[
        ("alloc()", "alloc()"),
        ("captureOut()", "stdout()"),
        ("captureErr()", "stderr()"),
        ("stdin([\"a\", \"b\"])", "stdin().lines([\"a\", \"b\"])"),
        ("stdinBytes([1, 2])", "stdin().bytes([1, 2])"),
        ("data()", "fs()"),
        ("files([(\"a\", \"b\")])", "fs().files([(\"a\", \"b\")])"),
        ("filesBytes([(\"a\", [1])])", "fs().filesBytes([(\"a\", [1])])"),
        ("clockAt(1700)", "clock().at(1700)"),
        ("randSeed(7)", "rand().seed(7)"),
        ("envOf([(\"K\", \"v\")], [\"x\"])", "env().variables([(\"K\", \"v\")]).args([\"x\"])"),
        ("readOnly(files([(\"a\", \"b\")]))", "fs().files([(\"a\", \"b\")]).readOnly()"),
    ];
    for (old, new) in cases {
        let source = format!("{PREAMBLE}test \"t\" {{\n  let d = {old};\n}}\n");
        let out = rewritten(&source, &[]);
        assert!(
            out.contains(&format!("let d = {new};")),
            "`{old}` should become `{new}`, and the rewrite wrote:\n{}",
            indent(&out)
        );
    }
}

#[test]
fn hermetic_becomes_the_effects_the_compiler_asked_for() {
    let source = format!("{PREAMBLE}test \"t\" {{\n  let ctx = Hermetic();\n}}\n");

    let none = rewritten(&source, &[]);
    assert!(none.contains("let ctx = context { };"), "{}", indent(&none));

    let one = rewritten(&source, &["Alloc"]);
    assert!(one.contains("let ctx = context { Alloc: alloc() };"), "{}", indent(&one));
    assert!(one.contains("from \"core/host/testing/lib.buri\" import { alloc };"), "{}", indent(&one));
    assert!(one.contains("from \"core/effect/lib.buri\" import { Alloc };"), "{}", indent(&one));
    assert!(
        !one.contains("core/testing/context"),
        "a file with nothing left to import from the old module should not import it:\n{}",
        indent(&one)
    );

    // Bindings are written in `core/effect`'s own declaration order, whatever
    // order they were learned in, so the output is a function of the set.
    let two = rewritten(&source, &["Fs", "Alloc"]);
    assert!(
        two.contains("let ctx = context { Alloc: alloc(), Fs: fs() };"),
        "{}",
        indent(&two)
    );
    assert!(
        two.contains("from \"core/host/testing/lib.buri\" import { alloc, fs };"),
        "{}",
        indent(&two)
    );
}

#[test]
fn a_wide_context_is_written_over_several_lines() {
    let source = format!("{PREAMBLE}test \"t\" {{\n  let ctx = Hermetic();\n}}\n");
    let wide = rewritten(&source, &["Alloc", "Stdout", "Stderr", "Stdin", "Fs", "Clock"]);
    assert!(wide.contains("let ctx = context {\n    Alloc: alloc(),\n"), "{}", indent(&wide));
    assert!(wide.contains("\n    Clock: clock(),\n  };"), "{}", indent(&wide));
    for line in wide.lines() {
        assert!(line.chars().count() <= 100, "a line over a hundred columns: {line:?}");
    }
}

#[test]
fn an_effect_import_already_there_is_merged_rather_than_doubled() {
    let source = format!(
        "{PREAMBLE}from \"core/effect/lib.buri\" import {{ Stdin }};\n\
         test \"t\" {{\n  let ctx = Hermetic();\n}}\n"
    );
    let out = rewritten(&source, &["Alloc", "Stdin"]);
    assert_eq!(
        out.matches("core/effect/lib.buri").count(),
        1,
        "one import, not two:\n{}",
        indent(&out)
    );
    assert!(out.contains("import { Alloc, Stdin };"), "{}", indent(&out));
}

#[test]
fn an_argument_is_carried_over_exactly_as_it_was_written() {
    // The replacement is built out of the argument's *span*, so a list written
    // over several lines with a comment in it survives intact. A rewrite that
    // reprinted the tree would lose the comment and the shape.
    let source = format!(
        "{PREAMBLE}test \"t\" {{\n  \
         let d = files([\n    // the one that matters\n    (\"a/b.txt\", \"body\"),\n  ]);\n}}\n"
    );
    let out = rewritten(&source, &[]);
    assert!(
        out.contains("fs().files([\n    // the one that matters\n    (\"a/b.txt\", \"body\"),\n  ])"),
        "{}",
        indent(&out)
    );
}

#[test]
fn a_read_back_is_renamed_only_on_a_receiver_the_rewrite_bound() {
    let source = format!(
        "{PREAMBLE}test \"t\" {{\n  \
         let errors = captureErr();\n  \
         let other = 1;\n  \
         let a = errors.capturedErr();\n  \
         let b = other.capturedErr();\n}}\n"
    );
    let out = rewritten(&source, &[]);
    assert!(out.contains("let errors = stderr();"), "{}", indent(&out));
    assert!(out.contains("let a = errors.captured();"), "{}", indent(&out));
    assert!(
        out.contains("let b = other.capturedErr();"),
        "a method on a name the rewrite never bound is left alone:\n{}",
        indent(&out)
    );
}

#[test]
fn a_clock_advance_becomes_a_sleep() {
    let source =
        format!("{PREAMBLE}test \"t\" {{\n  let c = clockAt(5);\n  let d = c.advance(10);\n}}\n");
    let out = rewritten(&source, &[]);
    assert!(out.contains("let c = clock().at(5);"), "{}", indent(&out));
    assert!(out.contains("let d = c.sleepMillis(10);"), "{}", indent(&out));
}

#[test]
fn a_name_the_file_also_binds_is_refused_rather_than_guessed_at() {
    // `files` is both an import and a local here. Rewriting the second would
    // change what the program means, and the migration has no resolver of its
    // own to tell them apart — so it refuses the file and says which name.
    let source = format!("{PREAMBLE}test \"t\" {{\n  let files = 1;\n}}\n");
    let p = plan("test/case.buri", &source);
    let refused = p.refused.clone().expect("a shadowed name is refused");
    assert!(refused.contains("`files`"), "{refused}");
    assert_eq!(p.render(Style::Final).0, source, "a refused file is not touched");
}

#[test]
fn a_function_with_no_twin_is_left_alone_and_named() {
    let source = "from \"core/testing/context/lib.buri\" import { noNet };\n\
                  test \"t\" {\n  let n = noNet();\n}\n";
    let p = plan("test/case.buri", source);
    assert!(p.refused.is_none());
    assert!(
        p.notes.iter().any(|n| n.contains("noNet")),
        "the report has to name it: {:?}",
        p.notes
    );
    assert!(p.render(Style::Final).0.contains("let n = noNet();"));
}

// ---------------------------------------------------------------------------
// The migration
// ---------------------------------------------------------------------------

/// Runs the migration and writes it into the checked-in corpus.
///
/// Ignored, because it is the one test here that edits a tree somebody else
/// owns. It works in a scratch copy while the fixpoint runs — the compiler is
/// asked the same question a dozen times and none of those answers belongs in
/// the repository — and copies the settled text back only at the end.
///
/// `BURI_MIGRATE` overrides [`PACKAGES`], comma separated, so the batches after
/// this one need no edit here to be run.
#[test]
#[ignore = "edits the checked-in corpus; run it deliberately"]
fn migrate_the_corpus() {
    let root = repo_root();
    let corpus = root.join(CORPUS);
    let chosen: Vec<String> = match std::env::var("BURI_MIGRATE") {
        Ok(list) => list.split(',').map(|s| s.trim().to_string()).collect(),
        Err(_) => PACKAGES.iter().map(|s| (*s).to_string()).collect(),
    };

    let files: Vec<(String, String)> = chosen
        .iter()
        .flat_map(|package| sources_under(&corpus, package))
        .map(|rel| {
            let text = std::fs::read_to_string(corpus.join(&rel)).unwrap();
            (rel, text)
        })
        .collect();
    assert!(!files.is_empty(), "no sources under {chosen:?}");

    let scratch = Scratch::copy_of("migrate", &corpus);
    let targets: Vec<String> = chosen.iter().map(|p| format!("//{p}")).collect();
    let report = migrate::migrate(&files, |rendered| {
        for (rel, text) in rendered {
            scratch.write(rel, text);
        }
        let mut args: Vec<&str> = vec!["test", "--error-format=json", "--force"];
        args.extend(targets.iter().map(String::as_str));
        scratch.run(&args).all()
    });

    let mut moved = 0;
    for p in &report.plans {
        let (text, _) = p.render(Style::Final);
        if text != p.original {
            std::fs::write(corpus.join(&p.rel), &text).unwrap();
            moved += 1;
        }
        for note in &p.notes {
            println!("note: {note}");
        }
    }

    println!("migration: {} file(s) rewritten in {} round(s)", moved, report.rounds);
    println!(
        "  sites: {} derived, {} over-approximated, {} unmigrated",
        report.derived,
        report.approximated,
        report.unmigrated.len()
    );
    for (rel, why) in &report.unmigrated {
        println!("  unmigrated: {rel} — {why}");
    }

    // The shape of what was written, so a reader of the log can see the
    // narrowing rather than take it on trust.
    let mut shapes: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for p in &report.plans {
        for site in &p.sites {
            let key = if site.blocked.is_some() {
                "Hermetic() (unmigrated)".to_string()
            } else if site.effects.is_empty() {
                "context { }".to_string()
            } else {
                site.effects.iter().copied().collect::<Vec<_>>().join(", ")
            };
            *shapes.entry(key).or_default() += 1;
        }
    }
    for (shape, count) in &shapes {
        println!("  {count:>4} × {shape}");
    }
}
