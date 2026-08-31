//! **The `Hermetic()` migration**: the script, and the proof it has nothing
//! left to do.
//!
//! The rewriter itself is `harness/migrate.rs`, and its doc comment states the
//! two rules it is built on. This target is what runs it and what holds it to
//! them:
//!
//! ```text
//! cargo test -p buri --test migrate                                  # all of it
//! ```
//!
//! One invocation, and no `--ignored`: the rewriter-in-place, which was the one
//! test in the tree that edited a checked-in corpus, is gone (the note at the
//! bottom of this file says why, and what still covers it). What is left is
//! ordinary — unit cases over synthetic sources, one per rewrite the table
//! names, and one sweep over the packages that have already moved asserting
//! that a second run finds nothing, which is the whole of "the script is
//! idempotent" written down where it fails if it stops being true.

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

use harness::{indent, repo_root};
use migrate::{nothing_blocked, plan, sources_under, Style};

/// A repository the migration runs over, the directories inside it whose
/// sources are planned, and the targets `buri test` is asked for while the
/// fixpoint runs.
///
/// A corpus is a repository and not a package because the fixpoint compiles:
/// the diagnostics it reads name files by their *repository*-relative path,
/// and a label only means anything from a repository root. The conformance
/// tree is one repository holding many packages; every other corpus here is a
/// fixture repository holding one or two.
struct Corpus {
    /// The repository root, relative to this repository's root.
    root: &'static str,
    /// The directories under it whose `.buri` sources are planned. Every
    /// source is planned, not only the ones that name the old module — a file
    /// that names none of this is returned byte-identical, and planning it is
    /// what proves that.
    packages: &'static [&'static str],
    /// What `buri test` is asked for while the fixpoint runs. The migration
    /// reads compile errors, so all that matters is that these targets
    /// *compile* the planned sources; several of these fixtures are suites
    /// that then fail on purpose, which is not a compile error and so not this
    /// migration's business.
    targets: &'static [&'static str],
}

/// The corpora the migration has moved. Named here rather than passed in, so
/// that the idempotence sweep and the migration cannot drift apart.
///
/// E7 is the conformance repository's first two packages, E8 the next three
/// and E9 the seven after that; E10 is every row below the first.
const CORPORA: &[Corpus] = &[
    Corpus {
        root: "cli/tests/conformance",
        packages: &[
            // E7, batch 1.
            "lib/data",
            "lib/collections",
            // E8, batch 2.
            "lib/semantics",
            "lib/json",
            "lib/proto",
            // E9, batch 3.
            "lib/text",
            "lib/crypto",
            "lib/calendar",
            "lib/numbers",
            "lib/memory",
            "lib/canary",
            "lib/codegen",
        ],
        targets: &[
            "//lib/data",
            "//lib/collections",
            "//lib/semantics",
            "//lib/json",
            "//lib/proto",
            "//lib/text",
            "//lib/crypto",
            "//lib/calendar",
            "//lib/numbers",
            "//lib/memory",
            "//lib/canary",
            "//lib/codegen",
        ],
    },
    // The build system's own monorepo. `//...` rather than a list, because
    // `the_example_monorepo_is_clean` builds it that way, and a corpus the
    // migration compiles differently from its own driver is a corpus the
    // migration can be wrong about.
    Corpus { root: "cli/tests/example", packages: &["cmd", "lib", "tools"], targets: &["//..."] },
    Corpus {
        root: "cli/tests/repositories/cli/gen_both_rules/repo",
        packages: &["tools"],
        targets: &["//tools/report"],
    },
    Corpus {
        root: "cli/tests/repositories/testing/filesystem_in_the_suite/repo",
        packages: &["lib"],
        targets: &["//lib/report"],
    },
    // The three `cli/tests/failing` repositories. Each holds a suite that
    // fails on purpose, and each of them compiles.
    Corpus {
        root: "cli/tests/failing/captured_output/repo",
        packages: &["lib"],
        targets: &["//lib/noisy"],
    },
    Corpus {
        root: "cli/tests/failing/hand_written_show/repo",
        packages: &["lib"],
        targets: &["//lib/money"],
    },
    Corpus {
        root: "cli/tests/failing/long_values/repo",
        packages: &["lib"],
        targets: &["//lib/wide"],
    },
];

/// The files under [`CORPORA`] the migration is **not** run over, and why.
/// Paths are relative to this repository's root, so that one list spans every
/// corpus.
///
/// One entry, and it is not a limitation of the script: `effects.buri` is
/// `core/testing/context`'s own conformance file. Its subject is the module
/// being migrated away from — `readOnly`'s attenuator, `noNet`'s refusals,
/// `capturedErr`, `advance`, and `..Hermetic()` spread through three named
/// context declarations — and `core/host/testing`'s equivalent is the file
/// next door, `host_testing.buri`, written for E1–E4 precisely so that the two
/// spellings are covered side by side while they coexist. Rewriting it would
/// leave a module this repository still ships with no conformance file at all,
/// six waves before E12 deletes it. It moves with the module, in E12.
const HELD: &[(&str, &str)] = &[(
    "cli/tests/conformance/lib/semantics/test/effects.buri",
    "`core/testing/context`'s own conformance file — it moves with the module, in E12",
)];

/// The files the migration runs over and cannot finish, and what each is
/// waiting for.
///
/// Each is migrated as far as `core/host/testing` reaches and keeps importing
/// `core/testing/context` for the one name that has no twin.
/// [`the_migrated_packages_have_nothing_left_to_rewrite`] holds the list to
/// both halves of that: a file here that no longer names the old module is a
/// row to delete, and a file that names it and is not here is a migration that
/// stopped early without saying so.
const PARTIAL: &[(&str, &str)] = &[
    (
        "cli/tests/conformance/lib/semantics/test/http.buri",
        "`noNet()`, which E3's `net()` replaces; the rest of the file is migrated",
    ),
    (
        "cli/tests/example/cmd/basket/test/view.buri",
        "`noNet()`, which E3's `net()` replaces; `alloc` has moved and every context \
         in the file is written out already",
    ),
];

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
        ("envOf([(\"K\", \"v\")], [\"x\"])", "env().variables([(\"K\", \"v\")]).arguments([\"x\"])"),
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
fn a_file_that_names_none_of_this_is_left_exactly_as_it_is() {
    // Every file of a package is planned, including the ones that never named
    // `core/testing/context` — and what the migration owes those is nothing at
    // all. The `core/effect` import is the trap: it is a line this rewrite
    // knows how to write, and a plan that rewrote it here would reorder a
    // neighbour's imports for no reason. `lib/semantics/shapes.buri` and
    // `host_testing.buri` are the two real files that shape came from.
    let source = "from \"core/effect/lib.buri\" import { Stdout, Alloc, Fs };\n\
                  from \"core/host/testing/lib.buri\" import { alloc };\n\
                  test \"t\" {\n  let ctx = context { Alloc: alloc() };\n}\n";
    let p = plan("test/case.buri", source);
    assert!(!p.work_left(), "there is nothing here to do");
    assert!(!p.still_names_the_old_module());
    assert_eq!(p.render(Style::Final).0, source, "and so nothing is written");
}

/// The same, one step in: a file that *is* migrated but already imports every
/// effect its contexts name keeps its own import line, in its own order.
#[test]
fn an_effect_import_that_is_already_enough_is_left_alone() {
    let source = "from \"core/testing/context/lib.buri\" import { Hermetic };\n\
                  from \"core/effect/lib.buri\" import { Stdout, Alloc };\n\
                  test \"t\" {\n  let ctx = Hermetic();\n}\n";
    let out = rewritten(source, &["Alloc"]);
    assert!(
        out.contains("from \"core/effect/lib.buri\" import { Stdout, Alloc };"),
        "{}",
        indent(&out)
    );
}

#[test]
fn a_name_with_no_twin_keeps_its_import_and_nothing_else_does() {
    // `lib/semantics/test/http.buri`'s shape: two names from the old module,
    // one of which — `noNet` — this migration has nothing to put in place of.
    // The call stays, so the import of *that* name has to stay with it, and
    // the import of `alloc` has to move all the same. Dropping the line whole
    // would leave the file naming something it no longer imports; keeping it
    // whole would leave `alloc` imported from two modules at once.
    let source = "from \"core/testing/context/lib.buri\" import { alloc, noNet };\n\
                  from \"core/effect/lib.buri\" import { Alloc, Net };\n\
                  test \"t\" {\n  let ctx = context { Alloc: alloc(), Net: noNet() };\n}\n";
    let out = rewritten(source, &[]);
    assert!(
        out.contains("from \"core/testing/context/lib.buri\" import { noNet };"),
        "{}",
        indent(&out)
    );
    assert!(
        out.contains("from \"core/host/testing/lib.buri\" import { alloc };"),
        "{}",
        indent(&out)
    );
    assert!(out.contains("context { Alloc: alloc(), Net: noNet() }"), "{}", indent(&out));

    // And the inverse: a file with nothing left to keep loses the line.
    let clean = "from \"core/testing/context/lib.buri\" import { alloc };\n\
                 test \"t\" {\n  let ctx = context { Alloc: alloc() };\n}\n";
    let out = rewritten(clean, &[]);
    assert!(!out.contains("core/testing/context"), "{}", indent(&out));
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

/// A `Hermetic()` written in a *named context declaration* is answered by the
/// diagnostic its callers draw.
///
/// `context Inherited { ..Hermetic() }` is a declaration nothing is missing
/// from until somebody calls it, and the compiler reports the missing binding
/// where it is called — a test with no `Hermetic()` in it at all. So the
/// attribution follows the edge: a declaration with no site of its own hands
/// the diagnostic to the context declarations it names. `lib/semantics`'s
/// `evaluation.buri` is the file this came from, and `effects.buri` — held
/// back for E12 — is the one that chains three of them.
#[test]
fn a_context_declaration_is_answered_by_the_diagnostic_its_caller_drew() {
    let source = "from \"core/testing/context/lib.buri\" import { Hermetic };\n\
                  context Inherited {\n  ..Hermetic(),\n}\n\
                  test \"t\" {\n  let ctx = Inherited();\n  let _ = ticked(ctx);\n}\n";
    let files = vec![("test/case.buri".to_string(), source.to_string())];
    let mut round = 0;
    let report = migrate::migrate(&files, &nothing_blocked, |rendered| {
        round += 1;
        if round > 1 {
            return String::new();
        }
        // Reported at the call, which is the only place it can be.
        let line = rendered[0].1.lines().position(|l| l.contains("ticked(ctx)")).unwrap() + 1;
        format!(
            "{{\"code\": \"unsatisfied-bound\", \"severity\": \"error\", \
             \"message\": \"`a context` does not satisfy `Clock`\", \
             \"location\": {{\"file\": \"test/case.buri\", \"line\": {line}}}}}"
        )
    });
    assert_eq!(report.derived, 1, "one site, and the compiler named its effect");
    assert_eq!(report.approximated, 0, "and named it, so nothing was approximated");
    // The fixpoint is a loop and this is the only test that watches it turn:
    // one round that drew the diagnostic, and one that asked again and got
    // nothing back. `Report::rounds` had no reader at all once the in-place
    // migration was deleted, and a field nothing reads is a field that can say
    // anything.
    assert_eq!(report.rounds, 2, "one round to draw the diagnostic and one to confirm the fix");
    let out = report.plans[0].render(Style::Final).0;
    assert!(out.contains("..context { Clock: clock() },"), "{}", indent(&out));
}

// ---------------------------------------------------------------------------
// The proof the script is done
// ---------------------------------------------------------------------------

/// A second run over what has already moved rewrites nothing.
///
/// This is the structural half only — no compiler, so it costs one parse per
/// file — and it is enough: the migration's whole entry point is a file that
/// imports `core/testing/context`, so a package with no such import in it is a
/// package the script would walk past. The day one comes back, this fails.
#[test]
fn the_migrated_packages_have_nothing_left_to_rewrite() {
    let root = repo_root();
    let mut left = Vec::new();
    let mut residue = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut checked = 0;
    for corpus in CORPORA {
        let corpus_root = root.join(corpus.root);
        for package in corpus.packages {
            for rel in sources_under(&corpus_root, package) {
                let whole = format!("{}/{rel}", corpus.root);
                if HELD.iter().any(|(held, _)| *held == whole) {
                    continue;
                }
                let text = std::fs::read_to_string(corpus_root.join(&rel)).unwrap();
                let p = plan(&rel, &text);
                assert!(p.refused.is_none(), "{}", p.refused.unwrap());
                checked += 1;
                if p.work_left() {
                    left.push(format!("{whole}: {} site(s) and an import to move", p.sites.len()));
                }
                // A file that still names the old module has to say why, in a
                // list somebody reads — and the list has to still be true.
                if p.still_names_the_old_module() {
                    seen.push(whole.clone());
                    if !PARTIAL.iter().any(|(f, _)| *f == whole) {
                        residue
                            .push(format!("{whole} still imports the old module and is not in PARTIAL"));
                    }
                }
            }
        }
    }
    for (file, why) in PARTIAL {
        if !seen.iter().any(|rel| rel == file) {
            residue.push(format!("{file} no longer needs the old module — delete its row ({why})"));
        }
    }
    assert!(checked >= 40, "expected every corpus's sources, found {checked}");
    assert!(
        left.is_empty(),
        "the migration is not finished:\n  {}",
        left.join("\n  ")
    );
    assert!(residue.is_empty(), "the residue is not what it says it is:\n  {}", residue.join("\n  "));
}

/// Every file [`HELD`] and [`PARTIAL`] name exists and still names the old
/// module.
///
/// Both lists are prose about files, and prose about files rots. This is the
/// half of that which is checkable: a row naming a file that is gone, or one
/// that has nothing to do with `core/testing/context` any more, fails here.
#[test]
fn the_held_and_partial_lists_name_files_that_are_still_there() {
    let root = repo_root();
    for (file, why) in HELD.iter().chain(PARTIAL) {
        let path = root.join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{file} ({why}) cannot be read: {e}"));
        assert!(
            text.contains("core/testing/context"),
            "{file} no longer names the old module, so its row is stale: {why}"
        );
    }
}

/// Every corpus row names a repository that is there, and packages inside it
/// that hold sources.
///
/// [`CORPORA`] is the one place the two halves of this target agree on what
/// has moved, and a row pointing at a directory that has been renamed would
/// make the sweep above quietly check nothing.
#[test]
fn every_corpus_names_a_repository_with_sources_in_it() {
    let root = repo_root();
    for corpus in CORPORA {
        let corpus_root = root.join(corpus.root);
        assert!(
            corpus_root.join("REPO.buri").is_file(),
            "{} is not a repository root",
            corpus.root
        );
        assert!(!corpus.targets.is_empty(), "{} names no targets", corpus.root);
        for package in corpus.packages {
            assert!(
                !sources_under(&corpus_root, package).is_empty(),
                "{}/{package} holds no sources",
                corpus.root
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The migration itself is gone
// ---------------------------------------------------------------------------
//
// `migrate_the_corpus` used to live here: `#[ignore]`d, driven by hand, it ran
// the rewriter over every corpus in `CORPORA` and wrote the settled text back
// into the checked-in trees. It has been **deleted**, and the reason is that
// the behaviour it performed is no longer wanted rather than that it was
// inconvenient to run.
//
// The migration is finished. `the_migrated_packages_have_nothing_left_to_rewrite`
// above plans every source in every corpus on every run and asserts that not
// one of them has a site or an import left to move, and
// `the_held_and_partial_lists_name_files_that_are_still_there` asserts the two
// exception lists still describe files that exist and still name the old
// module. Running the rewriter again over trees a test says are already at the
// fixpoint writes nothing by construction; what it does do is compile a dozen
// packages a dozen times to prove it, which is minutes for an answer a parse
// per file already gives.
//
// So nothing was covered by it that is not covered now. The one capability
// that went with it is "re-run the rewrite in place", and the day a corpus
// needs one it needs a *different* rewrite — the table in `harness/migrate.rs`
// is about `Hermetic()` and about nothing else, and a second migration is a
// second script rather than this one pointed at new files.
//
// It is written down here rather than left to `git log` because an empty space
// under a heading invites somebody to fill it back in with the same thing.
