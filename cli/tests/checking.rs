//! **What the whole front end says about a file with one token wrong**,
//! recorded case by case.
//!
//! `recovery.rs` holds the mutated population to invariants, and an invariant
//! is a claim about every case at once: it can say "no mistake becomes a type
//! error" and it cannot show what one file prints. `cli/tests/reject` is the
//! other half of that pairing for programs the checker refuses — a whole
//! program, and the diagnostics it draws, written down. This is the same
//! bargain for programs the *parser* could not read whole:
//!
//! ```text
//! cli/tests/checking/clean/<case>/
//!   main.buri       the seed, with one token deleted, inserted or exchanged
//!   expected.txt    every error the front end reports about it, rendered
//! ```
//!
//! # The two halves, and why the directory says which
//!
//! A recovered program is not always the program the author meant. `f(a, g(b)`
//! closes `f` a token early, and the checker then has an honest opinion about
//! the arity of what was written — so a case's errors are the syntax errors
//! *and sometimes* something the mistake led the checker to. Which of the two
//! a case is is not a judgement call and not a comment: it is the directory.
//!
//! * `clean/` — the errors are exactly the parser's. One mistake, one report,
//!   nothing invented. This is the claim the work order is about, and it is
//!   asserted per case: a `clean/` case that gains a checker error fails.
//! * `cascades/` — the mistake led somewhere else as well, and the recorded
//!   page shows where. Asserted per case in the other direction: a case here
//!   that stops cascading fails and asks to be moved, so an improvement to
//!   recovery shows up as files changing sides rather than as nothing.
//!
//! The number of them is bounded by [`CASCADE_CEILING`], as a rate over the
//! corpus rather than a count fitted to one draw — the same rule
//! `recovery.rs` states for its rows, and for the same reason.
//!
//! # The seeds
//!
//! Only sources the front end has *nothing* to say about unmutated. A seed
//! that already reports an error would put it in every one of its cases'
//! recorded pages, and "the errors are exactly the parser's" would stop being
//! readable off the page. The filter is run rather than assumed, which is what
//! the first few seconds of this suite are.
//!
//! ```text
//! cargo test -p buri --test checking
//! BURI_BLESS=1 cargo test -p buri --test checking     # re-record the corpus
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
    reason = "test code, on the same grounds `fuzz.rs` states: the panic-free \
              lint set is a promise about the toolchain, and a harness that \
              drives the toolchain is not the toolchain."
)]

mod harness;

#[path = "harness/mutation.rs"]
mod mutation;

#[path = "harness/pinned.rs"]
mod pinned;

use buri::compiler::driver;
use buri::compiler::modules::Role;
use buri::diagnostics::{Severity, SourceMap};
use harness::{case_dirs, tests_dir, Golden};
use mutation::Source;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Where the sample starts: `recovery.rs`'s seed, so a case pinned here is a
/// case that suite's invariants also hold.
const BASE_SEED: u64 = 0x0B00_1A57_5EC0_4E27;

/// How many cases are checked in. **The one constant that scales the corpus.**
const TOTAL: usize = 1_000;

/// A floor on the sampler, not on the path: a rule that stopped choosing cases
/// would otherwise leave a suite that passes by having nothing to run.
const FLOOR: usize = 250;

/// What share of the corpus may be a cascade, as a percentage.
///
/// `recovery.rs` measures the honest residue of `a syntax error stays a syntax
/// error` at 12.4%, 1.9%, 4.9%, 17.5% and 22.1% per mutation shape over the
/// whole population, and caps its widest row at 24. This corpus is drawn flat
/// across those shapes, so its rate is a blend of them and cannot exceed the
/// widest. Twenty-eight is that row's own ceiling with margin over the blend:
/// a ratchet on the recovery, and the number to lower after the next round of
/// work.
///
/// Measured at 21% (160 of 763) after the formatter style change and the
/// `circular-type-alias` diagnostic landed — down from 22% (161 of 732),
/// because the cases the style change added to the draw are struct and enum
/// declarations whose mistakes stay inside the declaration. The ceiling did
/// not move: it is derived from `recovery.rs`'s population rates above, not
/// fitted to this corpus's draw, which is the whole point of stating it as a
/// rate.
///
/// `Option`-field elision left it at 21%, to the case, and the five residues
/// above moved by under half a point for two separate reasons. The conformance
/// file that exercises elision is a new seed, so `recovery.rs`'s population
/// grew from 5,246 mutations to 5,270 — but it is 5,561 bytes and
/// [`pinned::SEED_BYTES`] is 4,000, so the samplers here never draw from it and
/// no pinned case declares an `Option`-typed struct field at all. On top of
/// that growth, elision itself moved exactly one mutation out of the residue
/// (783 of 5,270 under the old rule, 782 under the new): a `missing-field-value`
/// on an optional field is no longer an error, so that case's mistake stays a
/// syntax error. The direction is the only one elision can move a rate in — it
/// removes an error and never adds one.
const CASCADE_CEILING: usize = 28;

fn corpus_dir() -> PathBuf {
    tests_dir().join("checking")
}

fn clean_dir() -> PathBuf {
    corpus_dir().join("clean")
}

fn cascades_dir() -> PathBuf {
    corpus_dir().join("cascades")
}

// ---------------------------------------------------------------------------
// Reading what the front end said
// ---------------------------------------------------------------------------

/// A source under a `test/` directory is compiled as one, so that its `test`
/// declarations are legal.
fn role_of(name: &str) -> Role {
    if name.contains("/test/") {
        Role::TestSource
    } else {
        Role::Source
    }
}

/// Every error the front end reports about this file, rendered as a terminal
/// shows it, with what the code is.
///
/// The rendering and not the error page beneath it: the page is one document
/// per code, and `cli/tests/reject` and the documentation suite already pin
/// every word of it. Repeating a twenty-line explanation across seven hundred
/// cases would make a prose edit a seven-hundred-file diff and would bury the
/// three lines each case is actually about.
fn report(name: &str, text: &str) -> (String, Vec<String>) {
    let mut map = SourceMap::new();
    // Named for the file a case is, so a recorded page reads as a page about
    // `main.buri` rather than about the suite.
    let analysis = driver::analyze_snippet(&mut map, "main.buri", text, role_of(name));
    let mut out = String::new();
    let mut codes = Vec::new();
    for d in &analysis.diagnostics.items {
        if !matches!(d.severity, Severity::Error) {
            continue;
        }
        // A snippet is compiled on top of the standard library, and a
        // diagnostic from inside it is not the case's business.
        if map.text(d.span.file) != text {
            continue;
        }
        codes.push(d.code.clone().unwrap_or_default());
        out.push_str(&map.render(d, false));
    }
    (out, codes)
}

/// The codes the *parser* reports, which is what a `clean/` case's report must
/// be made of and nothing else.
fn syntax_codes(text: &str) -> Vec<String> {
    buri::parsing::parser::parse(text, buri::diagnostics::FileId(0))
        .errors
        .iter()
        .map(|d| d.code.clone().unwrap_or_default())
        .collect()
}

/// Whether a report holds anything the parser did not say.
///
/// Compared as a multiset of codes rather than of messages: what makes a case
/// a cascade is that the *checker* spoke, and which sentence it chose is the
/// recorded page's business.
fn cascaded(report_codes: &[String], syntax: &[String]) -> bool {
    let mut remaining: Vec<&String> = syntax.iter().collect();
    for code in report_codes {
        match remaining.iter().position(|s| *s == code) {
            Some(i) => {
                remaining.remove(i);
            }
            None => return true,
        }
    }
    false
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// Runs `f` over `items` on as many threads as the machine has.
///
/// A snippet analysis loads the standard library, so it is tens of
/// milliseconds and seven hundred of them are a minute. They share nothing —
/// each builds its own source map — so the minute is a minute of one core.
fn mapped<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(16);
    let chunk = items.len().div_ceil(threads.max(1));
    if chunk == 0 {
        return Vec::new();
    }
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(f).collect::<Vec<R>>()))
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

/// The seeds a case may be made from: small enough to read, and reporting
/// nothing at all before the mutation.
fn seeds() -> Vec<Source> {
    let all: Vec<Source> = mutation::corpus(&harness::repo_root())
        .into_iter()
        .filter(|s| s.text.len() <= pinned::SEED_BYTES)
        .collect();
    assert!(all.len() > 100, "the seed corpus is not reachable: {}", all.len());
    let quiet = mapped(&all, |s| report(&s.name, &s.text).1.is_empty());
    all.into_iter().zip(quiet).filter(|(_, ok)| *ok).map(|(s, _)| s).collect()
}

/// Every case, with the half of the corpus it belongs in.
///
/// The membership is computed, not remembered: a case moves between `clean/`
/// and `cascades/` when the toolchain changes, and that move is the diff the
/// next round of recovery work is read from.
fn picks() -> Vec<(pinned::Pick, bool, String)> {
    let sources = seeds();
    let mut keep = |m: &mutation::Mutation| !syntax_codes(&m.source).is_empty();
    let chosen = pinned::select(&sources, BASE_SEED, TOTAL, &mut keep);
    let reports = mapped(&chosen, |p| report(&p.mutation.file, &p.mutation.source));
    chosen
        .into_iter()
        .zip(reports)
        .map(|(p, (text, codes))| {
            let cascade = cascaded(&codes, &syntax_codes(&p.mutation.source));
            (p, cascade, text)
        })
        .collect()
}

/// The directory a case with this verdict lives in.
fn home(cascade: bool) -> PathBuf {
    if cascade {
        cascades_dir()
    } else {
        clean_dir()
    }
}

/// Writes the corpus, and deletes what the sampler no longer chooses.
fn write_corpus(picks: &[(pinned::Pick, bool, String)]) {
    let mut wanted: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for (p, cascade, _) in picks {
        wanted.entry(home(*cascade)).or_default().insert(p.name.clone());
    }
    for dir in [clean_dir(), cascades_dir()] {
        std::fs::create_dir_all(&dir).unwrap();
        let keep = wanted.get(&dir).cloned().unwrap_or_default();
        for e in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
            let n = e.file_name().to_string_lossy().to_string();
            if e.path().is_dir() && !keep.contains(&n) {
                std::fs::remove_dir_all(e.path()).unwrap();
            }
        }
    }
    for (p, cascade, text) in picks {
        let case = home(*cascade).join(&p.name);
        std::fs::create_dir_all(&case).unwrap();
        std::fs::write(case.join("main.buri"), &p.mutation.source).unwrap();
        std::fs::write(case.join("expected.txt"), text).unwrap();
    }
}

/// The corpus, computed once for the whole binary and written first if this
/// run is a blessing one.
///
/// Once, because the analysis behind it is seconds rather than milliseconds
/// and four tests ask for it — and because a blessing run must have finished
/// writing before any of them reads the tree.
fn corpus() -> &'static [(pinned::Pick, bool, String)] {
    static CORPUS: std::sync::OnceLock<Vec<(pinned::Pick, bool, String)>> =
        std::sync::OnceLock::new();
    CORPUS.get_or_init(|| {
        let picks = picks();
        if std::env::var_os("BURI_BLESS").is_some() {
            write_corpus(&picks);
        }
        picks
    })
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// **Every case reports what is recorded beside it, and nothing else.**
///
/// One test rather than one per case: a reworded diagnostic moves every case
/// that shows it, and reading the whole set of diffs at once is the point of
/// recording them.
#[test]
fn a_broken_file_reports_what_is_recorded() {
    let picks: &[(pinned::Pick, bool, String)] = corpus();
    let mut g = Golden::new();
    for (p, cascade, text) in picks {
        let case = home(*cascade).join(&p.name);
        let side = if *cascade { "cascades" } else { "clean" };
        if !case.join("main.buri").is_file() {
            g.fail(format!(
                "{side}/{}: chosen by the sampler and not checked in — \
                 `BURI_BLESS=1 cargo test -p buri --test checking` records it",
                p.name
            ));
            continue;
        }
        if std::fs::read_to_string(case.join("main.buri")).unwrap_or_default()
            != p.mutation.source
        {
            g.fail(format!("{side}/{}/main.buri is not what the mutator writes", p.name));
            continue;
        }
        g.check(&case.join("expected.txt"), &format!("{side}/{}/expected.txt", p.name), text);
    }
    g.finish("checking", picks.len());
}

/// **A `clean/` case's errors are exactly the parser's, and a `cascades/`
/// case's are not.**
///
/// The property, per case, in the only place it can be per case: which
/// directory the file is in. A recovery that stops a cascade moves a file, and
/// a recovery that starts one moves a file the other way — either is a diff to
/// read rather than a silent change of what the corpus means.
#[test]
fn a_case_is_in_the_half_its_report_belongs_in() {
    let picks: &[(pinned::Pick, bool, String)] = corpus();
    let mut moved = Vec::new();
    for (p, cascade, _) in picks {
        let here = home(*cascade).join(&p.name);
        let there = home(!*cascade).join(&p.name);
        if there.is_dir() && !here.is_dir() {
            moved.push(format!(
                "{} is checked in under {} and now belongs under {}",
                p.name,
                if *cascade { "clean" } else { "cascades" },
                if *cascade { "cascades" } else { "clean" }
            ));
        }
    }
    assert!(
        moved.is_empty(),
        "{} case(s) changed sides. If recovery got better, that is the diff to read; \
         `BURI_BLESS=1 cargo test -p buri --test checking` records it:\n  {}",
        moved.len(),
        moved.join("\n  ")
    );
}

/// **Regenerating the corpus produces the files that are checked in.**
///
/// The sampler run again from the same seed over the same sources chooses
/// exactly these cases; the goldens above recompute exactly these pages. Bless
/// is therefore an idempotent operation, and a case edited by hand fails here.
#[test]
fn the_generated_corpus_is_what_the_generator_writes() {
    let picks: &[(pinned::Pick, bool, String)] = corpus();
    let mut wanted: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (p, cascade, _) in picks {
        let side = if *cascade { "cascades" } else { "clean" };
        wanted.entry(side).or_default().insert(p.name.clone());
    }
    let mut wrong = Vec::new();
    for (side, dir) in [("clean", clean_dir()), ("cascades", cascades_dir())] {
        let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{} does not exist: {e}", dir.display()))
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        let empty = BTreeSet::new();
        let want = wanted.get(side).unwrap_or(&empty);
        for stale in on_disk.difference(want) {
            wrong.push(format!("{side}/{stale} is checked in and no longer chosen"));
        }
        for missing in want.difference(&on_disk) {
            wrong.push(format!("{side}/{missing} is chosen and not checked in"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} case(s) are not what the sampler chooses. \
         `BURI_BLESS=1 cargo test -p buri --test checking` rewrites the corpus:\n  {}",
        wrong.len(),
        wrong.iter().take(12).cloned().collect::<Vec<_>>().join("\n  ")
    );
    assert!(picks.len() >= FLOOR, "the sampler chose {} cases, and the floor is {FLOOR}", picks.len());

    let cascades = picks.iter().filter(|(_, c, _)| *c).count();
    let rows = pinned::coverage_rows(picks.iter().map(|(p, _, _)| p.cell.as_str()));
    eprintln!(
        "checking: {} cases over {} shapes, {cascades} of them cascades\n{}",
        picks.len(),
        rows.len(),
        rows.iter().map(|(row, n)| format!("  {row:<28} {n:>5} cases\n")).collect::<String>()
    );
    let rate = cascades.saturating_mul(100) / picks.len().max(1);
    assert!(
        rate <= CASCADE_CEILING,
        "{cascades} of {} cases cascade, which is {rate}% against a ceiling of \
         {CASCADE_CEILING}%. A mistake that reaches the checker is a mistake the \
         reader is told about twice.",
        picks.len()
    );
}

/// The corpus directories hold cases and nothing else.
#[test]
fn a_case_is_two_files() {
    for dir in [clean_dir(), cascades_dir()] {
        for case in case_dirs(&dir, "main.buri", 0) {
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
}
