//! **What `buri lint` still finds in a file that did not parse whole**,
//! recorded case by case.
//!
//! A syntax error is not a reason to stop reading a repository. The rules that
//! are about a declaration the parser *did* read are still answerable, and the
//! ones whose evidence sat inside the declaration it could not read are not —
//! and saying either of those out loud is worth a corpus, because both failure
//! modes are silent. A rule that goes quiet for the whole file takes a real
//! finding with it; a rule that reads a region the parser dropped invents one.
//!
//! ```text
//! cli/tests/linting/<case>/
//!   main.buri       a lint fixture's source, with one token wrong
//!   expected.txt    every finding `buri lint` reports about it, rendered
//! ```
//!
//! # The seeds, and why they are the lint fixtures
//!
//! Every source under `cli/tests/repositories/linting/*/repo/` was written to
//! make exactly one rule fire. That is what a lint recovery corpus needs and
//! what the repository's own sources are not: mutating a file nothing has an
//! opinion about produces a case that pins the syntax error and nothing else,
//! which the checking corpus already does better. So a case here starts from a
//! file with a finding in it, and asks what the mistake did to that finding.
//!
//! The cost of that choice is a coupling worth knowing about: a lint fixture
//! landing in `repositories/linting/` is a new seed, so it changes which cases
//! this corpus holds and the tree has to be blessed again. That is the trade —
//! the seeds are the files somebody wrote a rule for, and there is no second
//! copy of them to drift.
//!
//! # The fixture
//!
//! One scratch repository, one package per case, and the case's source *is*
//! the package's `lib.buri`. That shape is the lightest one that is still a
//! real `buri lint` run: everything the file declares is on the library's
//! surface, so no `dead-code` finding is manufactured by the fixture, and no
//! second module can lend the case a finding that is not its own. One session
//! and one pass over the whole repository answers every case at once, which is
//! what makes several hundred of them affordable.
//!
//! # The two claims, over the generated population
//!
//! * **The mistake invents no finding.** A rule that fires on the mutated file
//!   and did not fire on the seed is a rule reading a gap: the use it could not
//!   see was inside the declaration the parser dropped. Stated as the
//!   difference between the two runs rather than as "the caret is inside a
//!   region", because a region is a whole declaration and an honest finding
//!   about a long function's length has its caret inside one.
//! * **A finding whose evidence survived is still reported.** The seed's own
//!   findings are mapped through the mutation's byte shift; the ones that land
//!   outside every broken region have to still be there. This is the half that
//!   catches a checker or a lint pass that bails on the first parse error.
//!
//! Both are held to rates rather than to counts, in the style `recovery.rs`
//! states, so growing the corpus does not move the bar.
//!
//! ```text
//! cargo test -p buri --test linting
//! BURI_BLESS=1 cargo test -p buri --test linting     # re-record the corpus
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::too_many_lines,
    reason = "test code, on the same grounds `fuzz.rs` states: the panic-free \
              lint set is a promise about the toolchain, and a harness that \
              drives the toolchain is not the toolchain."
)]

mod harness;

#[path = "harness/mutation.rs"]
mod mutation;

#[path = "harness/pinned.rs"]
mod pinned;

use buri::commands::arguments::Flags;
use buri::diagnostics::FileId;
use harness::{case_dirs, tests_dir, Golden, Scratch};
use mutation::Source;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// `recovery.rs`'s seed, so a case pinned here is a case that suite's
/// invariants also hold.
const BASE_SEED: u64 = 0x0B00_1A57_5EC0_4E27;

/// How many cases are checked in. **The one constant that scales the corpus.**
const TOTAL: usize = 600;

/// A floor on the sampler, not on the path.
const FLOOR: usize = 250;

/// What share of the cases may gain a finding the seed did not have, as a
/// percentage.
///
/// Not zero, and the cases behind the number are why: a stray token written
/// into an import list really is an unused import, and a deleted closer that
/// runs two functions together really does make one of them longer. Two
/// points is one above the 0.9% the whole population shows, so the bound is a
/// ratchet and not a description.
const INVENTED_CEILING: usize = 2;

/// What share of the cases may lose a finding whose evidence survived.
///
/// **Three points, down from nine, and the drop is a fix rather than a draw.**
/// `Unchecked::of` in `cli/src/commands/lint.rs` used to read an error that
/// was not inside any body as an error about the *module*, and mark every body
/// in that module unread — so a missing `;` on an import silenced
/// `unused-variable` in a function twenty lines below it, and a used alias
/// that closed a cycle silenced every lint beside it, both from files the
/// parser read whole. It now marks a module unread only where the parser
/// actually skipped a declaration, and a body unread only where an error
/// landed in it. The population rate fell from **145 of 2,000 (7.25%) to 47 of
/// 2,000 (2.35%)**, measured over the same seeds either side of the change;
/// the pinned corpus fell from 10 of 528 to **6 of 545 (1.1%)**.
///
/// The number is read off the **whole population** rather than off the pinned
/// corpus, which is one case per shape and whose shapes are not equally
/// common. That is `recovery.rs`'s rule for a ceiling, and the reason for it
/// is that a bound fitted to a sample goes red on the next draw for a reason
/// that has nothing to do with the toolchain.
///
/// # The residue, which is a different thing from the bug that was fixed
///
/// Forty-seven cases, and **none of them is a lint going quiet about code that
/// was read**. Two of the three lints in the residue — `duplicate-import` and
/// `discarded-result` — never consulted `Unchecked` at all, and the third is
/// stopped by the per-body rule, which is the rule doing its job:
///
/// * **`unused-variable`, 30 cases.** The mutation broke the import the body
///   depends on, so the body itself no longer checks — `not-an-effect`,
///   `unsatisfied-bound`, `unresolved-name`, reported *inside* it. A body the
///   checker stopped in has lost the reads under wherever it stopped, so what
///   the binding is read by is not something the report can claim to know.
/// * **`discarded-result`, 11 cases.** The rule looks for calls landing on
///   `core/result.ignore` in the typed tree. The mutation broke the callee's
///   declaration, so the call resolves to nothing and is not in the tree to
///   find.
/// * **`duplicate-import`, 6 cases.** The rule counts two statements naming
///   one module. The mutation destroyed one of the two, so there is no longer
///   a pair — the evidence is half gone, not overlooked.
///
/// Every one of the forty-seven has an error inside a body, checked rather
/// than asserted. What makes them show up here at all is the invariant's proxy
/// for "the evidence survived": a finding is counted as surviving when its
/// byte offset lands outside every region the *parser* skipped. That is a
/// coarser question than "the declaration this finding is about still checks",
/// and the gap between the two is exactly this residue. Narrowing the proxy
/// would need the harness to model what the checker stopped on, which is the
/// toolchain's own answer restated in the test — so the residue is described
/// here instead, and the ceiling sits above it.
///
/// Three points, against 2.35% measured: the ratchet is one point of headroom,
/// which is what makes it a bound and not a description. It is not zero and
/// cannot be while the proxy is a byte offset.
const LOST_A_FINDING_CEILING: usize = 3;

fn corpus_dir() -> PathBuf {
    tests_dir().join("linting")
}

// ---------------------------------------------------------------------------
// The seeds
// ---------------------------------------------------------------------------

/// Every lint fixture's source that stands on its own.
///
/// A fixture that imports `//lib/…` is one of a pair, and half of a pair is
/// not a package. The rest are single modules that reach only `core/…`, which
/// is exactly what the one-package fixture below can hold.
fn seeds() -> Vec<Source> {
    let root = tests_dir().join("repositories/linting");
    let mut out: Vec<Source> = Vec::new();
    walk(&root, &root, &mut out);
    out.retain(|s| {
        s.text.len() <= pinned::SEED_BYTES
            && !s.text.contains("from \"//")
            && buri::parsing::parser::parse(&s.text, FileId(0)).errors.is_empty()
    });
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<Source>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            walk(&p, root, out);
            continue;
        }
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if !name.ends_with(".buri") || name == "BUILD.buri" || name == "REPO.buri" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        out.push(Source {
            name: p.strip_prefix(root).unwrap_or(&p).display().to_string(),
            text,
        });
    }
}

// ---------------------------------------------------------------------------
// Running the whole corpus at once
// ---------------------------------------------------------------------------

/// A library package holding nothing but the source under test.
const LIBRARY: &str = "library {\n    visibility: [\"//visibility:public\"]\n}\n";

/// A package for a source that exports `main`.
///
/// A binary rather than a library, because a program is where a context may be
/// built and where `core/host` may be imported — a `main` filed as a library
/// would report three rules the fixture invented rather than the one the
/// fixture was written for.
const BINARY: &str = harness::JS_BINARY;

/// Which of the two a source is, and what its file is called.
fn shape_of(text: &str) -> (&'static str, &'static str) {
    if text.contains("export fn main(") || text.contains("fn main(") {
        (BINARY, "main.buri")
    } else {
        (LIBRARY, "lib.buri")
    }
}

/// What one `buri lint` pass said about one file.
///
/// `at` and `what` cover the *rules* alone. `buri lint` prints everything the
/// toolchain knows, so its report holds the syntax error too — and the caret
/// of a syntax error lands inside the declaration it is about by construction,
/// which is not a finding about anything. The recorded page keeps the whole
/// report, because that is what a reader sees.
struct Findings {
    /// The rendered pages, in the order the report printed them.
    text: String,
    /// Where each rule's caret landed, in that file's bytes.
    at: Vec<usize>,
    /// What each finding is, as the pair that identifies one across an edit.
    what: Vec<(String, String)>,
    /// How many of the report's diagnostics were errors, which is how a seed
    /// that does not fit this fixture is told from one that does.
    errors: usize,
}

/// Whether a code is one of the catalogue's rules rather than the front end's.
fn is_a_rule(code: &str) -> bool {
    buri::documentation::lints::find(code).is_some()
}

/// Lints a whole set of sources in one repository, one package each.
///
/// One session and one pass: opening a repository and analysing a package are
/// tens of milliseconds each, and a thousand of them one at a time would be
/// the whole test budget.
fn lint_all(label: &str, sources: &[(String, String)]) -> BTreeMap<String, Findings> {
    let scratch = Scratch::repo(label);
    for (name, text) in sources {
        let (build, file) = shape_of(text);
        scratch.write(&format!("lib/{name}/BUILD.buri"), build);
        scratch.write(&format!("lib/{name}/{file}"), text);
    }
    let flags = Flags::default();
    let mut session = buri::build::session::open_at(&scratch.path(""), &flags)
        .unwrap_or_else(|e| panic!("the fixture repository did not open: {e}"));
    let targets = session
        .resolve_targets(&[String::from("//...")])
        .unwrap_or_else(|e| panic!("the fixture repository has no targets: {e}"));
    let diagnostics = buri::commands::lint::findings_for(&mut session, &targets, &flags);

    let mut out: BTreeMap<String, Findings> = BTreeMap::new();
    for d in &diagnostics.items {
        let path = session.map.name(d.span.file).to_string();
        // `lib/<case>/lib.buri` — the case is the directory it is in.
        let Some(case) = path.strip_prefix("lib/").and_then(|p| p.split('/').next()) else {
            continue;
        };
        let row = out.entry(case.to_string()).or_insert_with(|| Findings {
            text: String::new(),
            at: Vec::new(),
            what: Vec::new(),
            errors: 0,
        });
        if matches!(d.severity, buri::diagnostics::Severity::Error) {
            row.errors = row.errors.saturating_add(1);
        }
        row.text.push_str(&session.map.render(d, false));
        let code = d.code.clone().unwrap_or_default();
        if is_a_rule(&code) {
            row.at.push(d.span.start as usize);
            row.what.push((code, d.message.clone()));
        }
    }
    out
}

/// Where a byte of the seed sits in the mutated text.
///
/// Every mutation is one token wide and at a known offset, so everything
/// before it is where it was and everything after it has moved by the change
/// in length. A swap keeps the length and permutes the bytes inside its own
/// two tokens, which is well inside the declaration a region is.
fn moved(offset: usize, site: usize, delta: isize) -> usize {
    if offset < site {
        return offset;
    }
    offset.saturating_add_signed(delta)
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// One checked-in case: the mutation, the page recorded beside it, and what
/// the two invariants found.
struct Case {
    name: String,
    cell: String,
    source: String,
    text: String,
    /// Findings the mutated file has and the seed did not.
    invented: Vec<String>,
    /// The seed's findings that survived the mistake and were not reported.
    lost: Vec<String>,
}

/// The seeds the one-package fixture can actually hold.
///
/// A fixture the seed does not fit reports errors that are the fixture's fault
/// — a `main` filed as a library cannot build a context — and every case made
/// from it would carry them. Run rather than reasoned about: the seeds go
/// through the same pass the cases do, and the ones the toolchain has an error
/// about are dropped.
fn usable_seeds() -> Vec<Source> {
    let all = seeds();
    let batch: Vec<(String, String)> = all
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("seed{i:04}"), s.text.clone()))
        .collect();
    let findings = lint_all("linting-seeds", &batch);
    all.into_iter()
        .enumerate()
        .filter(|(i, _)| findings.get(&format!("seed{i:04}")).is_none_or(|f| f.errors == 0))
        .map(|(_, s)| s)
        .collect()
}

fn corpus() -> &'static [Case] {
    static CORPUS: std::sync::OnceLock<Vec<Case>> = std::sync::OnceLock::new();
    CORPUS.get_or_init(|| {
        let cases = build_corpus();
        if std::env::var_os("BURI_BLESS").is_some() {
            write_corpus(&cases);
        }
        cases
    })
}

/// The population one run draws, with the seeds that are usable and a `keep`
/// that admits only a mutation the parser now refuses.
fn population(total: usize, per_kind: usize, per_cell: usize) -> (Vec<Source>, Vec<pinned::Pick>) {
    let sources = usable_seeds();
    assert!(
        sources.len() >= 15,
        "the lint fixtures are not reachable: {} seeds",
        sources.len()
    );
    let mut keep = |m: &mutation::Mutation| {
        !buri::parsing::parser::parse(&m.source, FileId(0)).errors.is_empty()
    };
    let picks = pinned::select_with(&sources, BASE_SEED, total, per_kind, per_cell, &mut keep);
    (sources, picks)
}

fn build_corpus() -> Vec<Case> {
    // A wider offer than the other corpora take, because this one draws from
    // twenty seeds rather than three hundred: the cells run out long before
    // the budget does, and widening is where the rest of the cases come from.
    let (sources, picks) = population(TOTAL, 40, 2);
    measured("linting-corpus", &sources, picks)
}

/// What one lint pass says about every case in a population, with the two
/// invariants read off it.
fn measured(label: &str, sources: &[Source], picks: Vec<pinned::Pick>) -> Vec<Case> {
    // The seeds go through the same fixture, so that "what this file said
    // before the mistake" is the same measurement as "what it says now".
    let by_name: BTreeMap<&str, &Source> = sources.iter().map(|s| (s.name.as_str(), s)).collect();
    let mut batch: Vec<(String, String)> = Vec::new();
    let mut seed_of: BTreeMap<String, String> = BTreeMap::new();
    for (i, s) in sources.iter().enumerate() {
        let name = format!("seed{i:04}");
        seed_of.insert(s.name.clone(), name.clone());
        batch.push((name, s.text.clone()));
    }
    for p in &picks {
        batch.push((p.name.clone(), p.mutation.source.clone()));
    }
    let findings = lint_all(label, &batch);
    let empty = Findings {
        text: String::new(),
        at: Vec::new(),
        what: Vec::new(),
        errors: 0,
    };

    let mut out = Vec::new();
    for p in picks {
        let m = &p.mutation;
        let here = findings.get(&p.name).unwrap_or(&empty);
        let seed = by_name.get(m.file.as_str()).copied().unwrap();
        let before = seed_of
            .get(&m.file)
            .and_then(|n| findings.get(n))
            .unwrap_or(&empty);
        let regions = buri::formatting::broken_regions(&m.source);
        let delta = m.source.len() as isize - seed.text.len() as isize;

        let invented: Vec<String> = here
            .what
            .iter()
            .filter(|what| !before.what.contains(what))
            .map(|(code, message)| format!("[{code}] {message}"))
            .collect();
        // No region at all means the parser could not say which declaration
        // the mistake was in, so the file's shape is unknown at the top level
        // and nothing in it counts as evidence that survived. That is the same
        // file `formatting::source` refuses whole, and it is a small minority.
        let lost: Vec<String> = if regions.is_empty() {
            Vec::new()
        } else {
            before
                .at
                .iter()
                .zip(&before.what)
                .filter(|(at, _)| {
                    let now = moved(**at, m.site, delta);
                    !regions.iter().any(|(lo, hi)| now >= *lo && now < *hi)
                })
                .filter(|(_, what)| !here.what.contains(what))
                .map(|(_, (code, message))| format!("[{code}] {message}"))
                .collect()
        };

        out.push(Case {
            name: p.name,
            cell: p.cell,
            source: m.source.clone(),
            text: here.text.clone(),
            invented,
            lost,
        });
    }
    out
}

fn write_corpus(cases: &[Case]) {
    let dir = corpus_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let wanted: BTreeSet<&str> = cases.iter().map(|c| c.name.as_str()).collect();
    for e in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
        let n = e.file_name().to_string_lossy().to_string();
        if e.path().is_dir() && !wanted.contains(n.as_str()) {
            std::fs::remove_dir_all(e.path()).unwrap();
        }
    }
    for c in cases {
        let case = dir.join(&c.name);
        std::fs::create_dir_all(&case).unwrap();
        std::fs::write(case.join("main.buri"), &c.source).unwrap();
        std::fs::write(case.join("expected.txt"), &c.text).unwrap();
    }
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// **Every case reports what is recorded beside it, and nothing else.**
#[test]
fn a_broken_file_reports_what_is_recorded() {
    let cases: &[Case] = corpus();
    let mut g = Golden::new();
    for c in cases {
        let case = corpus_dir().join(&c.name);
        if std::fs::read_to_string(case.join("main.buri")).unwrap_or_default() != c.source {
            g.fail(format!(
                "{}/main.buri is not what the mutator writes — \
                 `BURI_BLESS=1 cargo test -p buri --test linting` records it",
                c.name
            ));
            continue;
        }
        g.check(&case.join("expected.txt"), &format!("{}/expected.txt", c.name), &c.text);
    }
    g.finish("linting", cases.len());
}

/// **The mistake invents no finding.**
///
/// The false-positive half. A rule whose evidence is an *absence* —
/// `unused-import`, `unused-variable` — fires when the use it was looking for
/// was inside the declaration the parser dropped, and a reader has no way to
/// tell that from a real one. Read as the difference between the seed's
/// findings and the mutated file's, which is where a manufactured finding
/// shows up and an honest one does not.
#[test]
fn no_finding_is_invented_by_the_mistake() {
    let cases: &[Case] = corpus();
    let over: Vec<&Case> = cases.iter().filter(|c| !c.invented.is_empty()).collect();
    let rate = over.len().saturating_mul(100) / cases.len().max(1);
    eprintln!(
        "linting: {} of {} cases gain a finding the seed did not have",
        over.len(),
        cases.len()
    );
    assert!(
        rate <= INVENTED_CEILING,
        "{} of {} cases ({rate}%) report a finding the seed did not, against a \
         ceiling of {INVENTED_CEILING}%:\n  {}",
        over.len(),
        cases.len(),
        over.iter()
            .take(8)
            .map(|c| format!("{}: {}", c.name, c.invented.join(", ")))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// **A finding whose evidence survived the mistake is still reported.**
///
/// The other half, and the one a pass that bails on the first parse error
/// fails: the seed's findings are mapped through the mutation's byte shift,
/// and every one that lands outside the broken regions has to still be there.
#[test]
fn a_finding_outside_the_break_still_fires() {
    let cases: &[Case] = corpus();
    let over: Vec<&Case> = cases.iter().filter(|c| !c.lost.is_empty()).collect();
    let rate = over.len().saturating_mul(100) / cases.len().max(1);
    eprintln!(
        "linting: {} of {} cases lose a finding whose evidence survived",
        over.len(),
        cases.len()
    );
    assert!(
        rate <= LOST_A_FINDING_CEILING,
        "{} of {} cases ({rate}%) lose a finding the mistake did not touch, \
         against a ceiling of {LOST_A_FINDING_CEILING}%:\n  {}",
        over.len(),
        cases.len(),
        over.iter()
            .take(8)
            .map(|c| format!("{}: {}", c.name, c.lost.join(", ")))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// **The same two claims, over a population nothing is checked in for.**
///
/// The pinned corpus is one case per cell, which is what makes it readable and
/// is also what stops it being a measurement: a rate over five hundred cases
/// chosen for their variety is not the rate over the cases a repository has.
/// So the invariants run a second time over every candidate the sampler can
/// offer, with no cap on the cell — thousands of them, nothing recorded, the
/// same ceilings.
///
/// `BURI_LINT_POPULATION` widens it; the default is what fits the budget.
#[test]
fn the_invariants_hold_over_the_wider_population() {
    let cap = std::env::var("BURI_LINT_POPULATION")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(2_000);
    let (sources, picks) = population(cap, 400, 64);
    let cases = measured("linting-population", &sources, picks);
    let invented = cases.iter().filter(|c| !c.invented.is_empty()).count();
    let lost = cases.iter().filter(|c| !c.lost.is_empty()).count();
    let n = cases.len().max(1);
    eprintln!(
        "linting population: {} cases, {invented} invent a finding, {lost} lose one",
        cases.len()
    );
    assert!(cases.len() > 1_000, "the wider population is {} cases", cases.len());
    assert!(
        invented.saturating_mul(100) / n <= INVENTED_CEILING,
        "{invented} of {} cases invent a finding, over the {INVENTED_CEILING}% ceiling",
        cases.len()
    );
    assert!(
        lost.saturating_mul(100) / n <= LOST_A_FINDING_CEILING,
        "{lost} of {} cases lose a finding whose evidence survived, over the \
         {LOST_A_FINDING_CEILING}% ceiling:\n  {}",
        cases.len(),
        cases
            .iter()
            .filter(|c| !c.lost.is_empty())
            .take(8)
            .map(|c| format!("{}: {}", c.name, c.lost.join(", ")))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// **Regenerating the corpus produces the files that are checked in.**
#[test]
fn the_generated_corpus_is_what_the_generator_writes() {
    let cases: &[Case] = corpus();
    let dir = corpus_dir();
    let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{} does not exist: {e}", dir.display()))
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let wanted: BTreeSet<String> = cases.iter().map(|c| c.name.clone()).collect();
    let stale: Vec<&String> = on_disk.difference(&wanted).collect();
    let missing: Vec<&String> = wanted.difference(&on_disk).collect();
    assert!(
        stale.is_empty() && missing.is_empty(),
        "the corpus is not what the sampler chooses. \
         `BURI_BLESS=1 cargo test -p buri --test linting` rewrites it.\n  \
         no longer chosen ({}): {:?}\n  not checked in ({}): {:?}",
        stale.len(),
        stale.iter().take(8).collect::<Vec<_>>(),
        missing.len(),
        missing.iter().take(8).collect::<Vec<_>>()
    );
    assert!(cases.len() >= FLOOR, "the sampler chose {} cases, and the floor is {FLOOR}", cases.len());
    let rows = pinned::coverage_rows(cases.iter().map(|c| c.cell.as_str()));
    eprintln!(
        "linting: {} cases over {} shapes\n{}",
        cases.len(),
        rows.len(),
        rows.iter().map(|(row, n)| format!("  {row:<28} {n:>5} cases\n")).collect::<String>()
    );
}

/// A case is two files and no more.
#[test]
fn a_case_is_two_files() {
    let _ = corpus();
    for case in case_dirs(&corpus_dir(), "main.buri", FLOOR) {
        let mut names: Vec<String> = std::fs::read_dir(&case)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["expected.txt".to_string(), "main.buri".to_string()],
            "{} holds something other than the two files a case is made of",
            case.display()
        );
    }
}
