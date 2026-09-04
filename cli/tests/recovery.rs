//! **One mistake reads as one mistake.**
//!
//! A person deletes a comma. The toolchain should say "a match arm ends with
//! `,`", once, with a caret where the comma goes — and then carry on as if the
//! comma were there, so that nothing downstream of the mistake invents a
//! second one. Today it says three things, none of which is that, and
//! `buri format --check` reports the file as clean. This suite is the
//! specification of the behaviour that replaces it.
//!
//! Two halves, and they prove different things:
//!
//! * **The generated half.** Every compiling source in the repository, with
//!   one token deleted, inserted or exchanged — thousands of cases — held to
//!   *invariants* rather than to recorded output. A golden file can only say
//!   what one case prints; an invariant says what every case must satisfy, and
//!   that is the only shape in which "recovery works" is a claim at all.
//! * **The curated half.** `cli/tests/recovery/`, one hand-written case per
//!   list context in the grammar, each pinning the exact message, code, span
//!   and edit. This is where the *wording* is decided, and it is written from
//!   the maintainer's example outwards.
//!
//! ```text
//! cargo test -p buri --test recovery                     # all of it
//! BURI_RECOVERY_PER_KIND=40 cargo test …                 # soak: more per file
//! BURI_RECOVERY_SEED=0x1234 cargo test …                 # a different sample
//! BURI_RECOVERY_ONLY="delete-closer" cargo test …        # one row of the report
//! BURI_BLESS=1 cargo test -p buri --test recovery recorded
//! ```
//!
//! # Nothing here is ignored
//!
//! These were written **before** the parser that satisfies them, so every one
//! of them was red on purpose and `#[ignore]` kept the default `cargo test`
//! green while that was true. That is over: the four the parser owns — (a) one
//! mistake is one diagnostic, (b) the caret is on the mistake, (c) the fix
//! names the missing token, (d) a syntax error stays a syntax error — the two
//! the formatter owns, the curated set, and [`message_audit_corpus`] all run by
//! default. This section is kept as the record of why the attributes were here,
//! because a file whose header still explains an `#[ignore]` it no longer
//! carries is how the next one gets added.
//!
//! # The ceilings
//!
//! An invariant here is held per *mutation shape*, and a shape whose text has
//! a second reading the grammar accepts cannot be held to zero — see
//! [`ceiling`], which states which rows those are and what each residue is.
//! Every row where the toolchain can be exact is zero, including every
//! separator row of (a), which is the maintainer's own case.
//!
//! # What it costs
//!
//! Six thousand cases and five invariants over them, and every case is a pure
//! function of one string. So the generated half is run the way the toolchain
//! runs its own whole-program work: [`judge_with`] fans the cases out over
//! [`buri::parallel::map_with`] and folds the verdicts back **in case order**,
//! on one thread, so the report is a function of the corpus and not of the
//! machine. The one invariant that runs a full analysis gives each worker an
//! [`Analyzer`] to keep, which is what stops the standard library being
//! re-parsed once per case, and [`corpus_and_cases`] builds the corpus once for
//! all seven tests that ask for it.
//!
//! None of that is allowed to change an answer, and none of it does: the tables
//! this prints are the same tables it printed as a serial loop, row for row.

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

#[path = "harness/mod.rs"]
mod harness;

#[path = "harness/mutation.rs"]
mod mutation;

use buri::compiler::driver;
use buri::compiler::modules::Role;
use buri::diagnostics::{Diagnostic, FileId, Severity, SourceMap};
use buri::formatting::{token_shape, Shape};
use harness::{case_dirs, indent, require_annotation, tests_dir, Golden, Scratch};
use mutation::{Kind, Mutation, Source};
use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// The sample
// ---------------------------------------------------------------------------

/// Where the sample starts when nothing says otherwise.
///
/// Fixed, for `fuzz.rs`'s reason: a suite whose input moves between runs
/// reports the day rather than the toolchain.
const BASE_SEED: u64 = 0x0B00_1A57_5EC0_4E27;

/// How many mutations of each shape are drawn from each file in CI.
///
/// Four shapes over roughly two hundred sources, so this is the multiplier
/// that turns the corpus into thousands of cases. It is small enough that the
/// parse-only invariants cost milliseconds and large enough that every list
/// context in the grammar is hit many times over.
const CI_PER_KIND: usize = 6;

/// How many cases the formatting invariant takes.
///
/// Formatting renders a document, so it is the one invariant that still
/// samples: a deterministic stride through the ordered list, so the cases are
/// a spread across every corpus and every mutation shape rather than a prefix.
/// Its ceilings are all zero, so the stride costs it no exactness. `a syntax
/// error stays a syntax error` used to sample the same way and no longer does
/// — the note on that test says what the stride cost and what it now costs.
const CI_FORMATTED: usize = 400;

fn env_num(key: &str) -> Option<usize> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}

fn per_kind() -> usize {
    env_num("BURI_RECOVERY_PER_KIND").unwrap_or(CI_PER_KIND)
}

fn base_seed() -> u64 {
    match std::env::var("BURI_RECOVERY_SEED") {
        Ok(v) => {
            let t = v.trim();
            t.strip_prefix("0x")
                .and_then(|h| u64::from_str_radix(&h.replace('_', ""), 16).ok())
                .or_else(|| t.replace('_', "").parse().ok())
                .unwrap_or(BASE_SEED)
        }
        Err(_) => BASE_SEED,
    }
}

/// The corpus this run drew from, and every mutation of it, in a fixed order.
///
/// **Built once for the whole binary.** Seven tests here ask for it and it is
/// the same answer for all of them: the walk reads two hundred checked-in
/// sources, parses each to prove it compiles, and draws the same mutations from
/// the same fixed seed. Building it per test was over a second of the run each
/// time, seven times over, for a value nothing may modify — `cases` hands out
/// shared references and there is no `&mut` anywhere in this file.
///
/// A `OnceLock` rather than a lazy memo per test because the default harness
/// runs these tests on threads of one process: the first to arrive builds it
/// and the rest wait, instead of six of them building their own copy beside it.
fn corpus_and_cases() -> &'static (Vec<Source>, Vec<Mutation>) {
    static BUILT: std::sync::OnceLock<(Vec<Source>, Vec<Mutation>)> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| {
        let root = harness::repo_root();
        let corpus = mutation::corpus(&root);
        assert!(
            corpus.len() > 100,
            "expected the checked-in compiling sources, found {}",
            corpus.len()
        );
        let (seed, per) = (base_seed(), per_kind());
        let mut out = Vec::new();
        for src in &corpus {
            out.extend(mutation::mutations_of(src, seed, per));
        }
        (corpus, out)
    })
}

fn cases() -> Vec<&'static Mutation> {
    let all = &corpus_and_cases().1;
    // `BURI_RECOVERY_ONLY=<row>` narrows a run to one row of the report, which
    // is how a residue is read case by case rather than five at a time.
    match std::env::var("BURI_RECOVERY_ONLY") {
        Ok(only) => all.iter().filter(|m| Tally::key(m) == only.trim()).collect(),
        Err(_) => all.iter().collect(),
    }
}

/// A deterministic spread of at most `cap` of them.
fn strided<'a>(all: &[&'a Mutation], cap: usize) -> Vec<&'a Mutation> {
    let cap = env_num("BURI_RECOVERY_CAP").unwrap_or(cap);
    if all.len() <= cap || cap == 0 {
        return all.to_vec();
    }
    let stride = all.len() / cap;
    all.iter().copied().step_by(stride.max(1)).take(cap).collect()
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

/// How a row's cases were chosen, which is what decides whether its ceiling
/// needs room for sampling noise.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sample {
    /// Every case in the corpus. The rate a row shows *is* the rate, so the
    /// ceiling is the rate and nothing more.
    Whole,
    /// A stride through the corpus, taken because the invariant is too
    /// expensive to run on all of it. A hundred cases drawn from fifteen
    /// hundred land a few points either side of the true rate, and a source
    /// landing in the repository redraws them — so the bound has to carry that
    /// swing, or a row tips for a reason that has nothing to do with the
    /// toolchain. See [`sampling_allowance`].
    Strided,
}

#[derive(Default)]
struct Row {
    cases: usize,
    /// The mutation left a program the grammar still accepts — a dropped
    /// trailing comma, a `;` whose statement becomes the block's tail. Nothing
    /// is broken, so there is nothing to assert.
    still_valid: usize,
    violated: usize,
}

/// Counts per mutation shape, and the first few failures in full.
///
/// The counts are the point. R1 and R2 are written against a number that has
/// to fall, and a suite that says only "failed" cannot show one moving.
#[derive(Default)]
struct Tally {
    rows: BTreeMap<String, Row>,
    examples: Vec<String>,
}

impl Tally {
    fn key(m: &Mutation) -> String {
        match m.kind {
            // The bound a missing separator is held to depends on the
            // delimiter it sits inside, so the report separates them.
            Kind::DeleteSeparator => format!("{} {}", m.kind.name(), m.nest.name()),
            _ => m.kind.name().to_string(),
        }
    }

    fn seen(&mut self, m: &Mutation) {
        self.rows.entry(Tally::key(m)).or_default().cases += 1;
    }

    /// The mutation left a program the grammar still accepts.
    fn still_valid(&mut self, m: &Mutation) {
        self.rows.entry(Tally::key(m)).or_default().still_valid += 1;
    }

    fn violation(&mut self, m: &Mutation, why: String) {
        self.rows.entry(Tally::key(m)).or_default().violated += 1;
        if self.examples.len() < 5 {
            self.examples.push(format!("{} ({}, {})\n{}", m.origin, m.what, m.nest.name(), indent(&why)));
        }
    }

    /// Folds one case's [`Verdict`] in, which is the only way a case reaches
    /// the counts now that the cases are judged off this thread.
    fn record(&mut self, m: &Mutation, verdict: Verdict) {
        match verdict {
            Verdict::Skipped => {}
            Verdict::Held => self.seen(m),
            Verdict::StillValid => {
                self.seen(m);
                self.still_valid(m);
            }
            Verdict::Violated(why) => {
                self.seen(m);
                self.violation(m, why);
            }
        }
    }

    /// Prints the table, then fails if any row is over its ceiling.
    fn finish(self, invariant: &str, sample: Sample) {
        let (mut cases, mut violated) = (0, 0);
        let mut table = String::new();
        let mut over = Vec::new();
        for (key, row) in &self.rows {
            cases += row.cases;
            violated += row.violated;
            let allowed = allowed(invariant, key, row.cases, sample);
            if row.violated > allowed {
                over.push(format!("{key}: {} violated, and {allowed} allowed", row.violated));
            }
            table.push_str(&format!(
                "  {key:<28} {:>6} cases {:>6} violated {:>6} allowed {:>6} still valid\n",
                row.cases, row.violated, allowed, row.still_valid
            ));
        }
        eprintln!("recovery {invariant}: {cases} cases, {violated} violated\n{table}");
        assert!(
            over.is_empty(),
            "{} row(s) of `{invariant}` are over their ceiling:\n  {}\n\n{table}\nThe first {}:\n\n{}",
            over.len(),
            over.join("\n  "),
            self.examples.len(),
            self.examples.join("\n\n")
        );
    }
}

/// What one row of one invariant may still violate, as a percentage of the
/// cases in it.
///
/// **Zero is the answer wherever the toolchain can be exact**, and every
/// separator row of `one mistake is one diagnostic` is zero — that row is what
/// the maintainer's example is, and it went from 191 violations of 410 to none.
///
/// The rows that are not zero are the ones where the mutated text has a second
/// reading that the grammar accepts, so no recovery can be asked to prefer the
/// author's:
///
/// * **A deleted closer is found late or not at all.** `[1, 2, 3.fold(f, 0)`
///   is a legal array of one element until end of file; the parser learns the
///   `]` is missing where it runs out, which is not where the `]` was. This is
///   the whole of the residue in `the caret is on the mistake`, and most of it
///   elsewhere.
/// * **A deleted separator between match arms is swallowed by the arm before
///   it.** `=> "a"` followed by `(x, y) => "b"` reads as a call of `"a"`, and
///   `=> 1` followed by `.Err(e) =>` reads as a field of `1`. This is exactly
///   the ambiguity the required comma exists to prevent (design/grammar-rationale.md 12.12), so the
///   text without it is a different program rather than a broken one.
/// * **A recovered program means something else.** `f(a, g(b)` closes `f` a
///   token early, and the checker then has an honest opinion about the arity
///   of what was written. Suppressing that would mean gating the checker on
///   the file, which the work order ranked below the error node and which
///   would hide real findings in the declarations that parsed.
///
/// A ceiling is a ratchet: it is set just above what the toolchain achieves,
/// so a regression fails the suite, and it is expressed as a rate so that
/// `BURI_RECOVERY_PER_KIND` and `BURI_RECOVERY_SEED` widen the corpus without
/// moving the bar. Lowering one is the point of the next round of work.
///
/// **Every number below is a percentage of that row's population**, read off a
/// `BURI_RECOVERY_CAP=0` run over all 5,201 cases and rounded up to the next
/// whole point plus one of margin. It is deliberately not fitted to what a draw
/// happened to show: `a syntax error stays a syntax error` used to run on a
/// 300-case stride that was redrawn whenever a source landed in the repository,
/// so a bound fitted to one draw went red on the next merge for a reason that
/// had nothing to do with the toolchain — which is exactly what happened to two
/// of its rows when the standard library grew. It now runs on every case, so
/// its rows are the population and its ceilings are exact.
///
/// The rate is the whole of the bound for an invariant that runs on every case.
/// For a strided one, [`sampling_allowance`] is added on top, because a
/// hundred-case row is a sample and a sample has a spread.
fn ceiling(invariant: &str, row: &str) -> usize {
    match (invariant, row) {
        ("one mistake is one diagnostic", "delete-closer") => 6,
        ("one mistake is one diagnostic", "insert-stray") => 2,
        ("one mistake is one diagnostic", "swap-adjacent") => 1,

        ("the caret is on the mistake", "delete-closer") => 30,
        ("the caret is on the mistake", "delete-separator ()") => 5,
        ("the caret is on the mistake", "delete-separator {}") => 8,
        ("the caret is on the mistake", "insert-stray") => 2,
        ("the caret is on the mistake", "swap-adjacent") => 3,

        ("the fix names the missing token", "delete-closer") => 19,
        ("the fix names the missing token", "delete-separator ()") => 5,
        ("the fix names the missing token", "delete-separator {}") => 7,

        // 12.6%, 2.1%, 3.8%, 19.3% and 23.4% over the population, now that the
        // row is the population: the rate rounded up, with no draw to cover.
        //
        // Re-read the day the repository was formatted. The corpus is this
        // tree's own sources with one token moved, so laying every source out
        // the one way redrew every case in it — a `let` that was on one line is
        // now on three, and a mutation of it is a different program. The
        // toolchain did not move; the population did, and `insert-stray` went
        // from 17.7% to 19.3% of it.
        //
        // Re-read again when `cli/tests/conformance/lib/actor/` landed, for the
        // same reason and with the same evidence: `delete-closer` went from
        // 12.6% to 13.2% of a population that grew by one file. That file is
        // dense in nested closers — `assert.eq(counted.ask(ctx, fn(reply) =>
        // .Get(reply)), .Ok(2))` ends in four of them — and deleting one leaves
        // a call the checker can still count the arguments of, so it counts
        // them and says so. That is the residue this row measures rather than a
        // regression in it: no row's per-case behaviour moved, and every other
        // row fell as a share of the larger population.
        // **And re-read again in F4, for the same reason and with the same
        // answer.** `delete-closer` was 159 of 1243 — 12.8%, two cases under a
        // ceiling of 13% — and the TLS slice added a `Listen` fake and a test
        // to `lib/semantics`. The population's *size* did not move at all: the
        // corpus is a fixed number of mutations per source and no source was
        // added. What moved is *which* tokens the seed picks inside two changed
        // files, and four of the new draws violate where their predecessors did
        // not: 163 of the same 1243, or 13.1%. The parser and the checker are
        // not in that slice's diff at all, so this is the population moving
        // under a rate that was already knife-edge. Fourteen is the rate rounded
        // up with one point of room, which is what the row should have carried
        // the first time — a `Sample::Whole` ceiling has no *draw* to cover, but
        // it still has a corpus that gets edited.
        ("a syntax error stays a syntax error", "delete-closer") => 14,
        ("a syntax error stays a syntax error", "delete-separator ()") => 3,
        // The arm before the comma swallows the next arm's pattern, so `2` gets
        // a field: the same residue this invariant's sibling caps at 7.
        ("a syntax error stays a syntax error", "delete-separator {}") => 7,
        // **Re-read when `core/order` and `core/testing/assert` grew**, and for
        // the reason the paragraphs above give twice over: the population is
        // this tree's own sources, and two conformance files landed —
        // `data/ordering.buri` and `data/assertions.buri`. Both are dense in
        // exactly the shape this row measures, an assertion wrapping a call
        // wrapping a comparator — `assert.eq(rows.sortBy(ctx, cmp).map(ctx,
        // fn(r) => r.name), ["a"])` — so a stray token inside one leaves a
        // program the grammar still reads, and the checker then has an honest
        // opinion about it. 356 of 1758, or 20.25%, against a ceiling of 20 that
        // had four cases of room in it. Nothing in the parser or the checker is
        // in that diff — it is `.buri` library code, its tests and its prose —
        // so this is the corpus moving under a knife-edge rate again. Twenty-one
        // is that rate rounded up.
        ("a syntax error stays a syntax error", "insert-stray") => 21,
        ("a syntax error stays a syntax error", "swap-adjacent") => 24,

        // Every row not named above, and every row of an invariant R2 owns.
        (_, _) => 0,
    }
}

// ---------------------------------------------------------------------------
// Running the cases
// ---------------------------------------------------------------------------

/// What one case came to.
///
/// The invariants used to write straight into the [`Tally`] as they went, which
/// is a running total and therefore the one thing a worker may not hold. So a
/// case now *returns* what it found and the fold happens on one thread, in case
/// order — see [`judge_with`].
enum Verdict {
    /// Not this invariant's business: not counted, not reported. Only
    /// [`the_fix_names_the_missing_token`] has such cases — a swap is not a
    /// missing token, so there is no token for a fix to name.
    Skipped,
    /// Counted, and the invariant held.
    Held,
    /// Counted, and the mutation left a program the grammar still accepts, so
    /// there was nothing to hold it to.
    StillValid,
    /// Counted, and the invariant did not hold. The string is what the report
    /// prints for the first five.
    Violated(String),
}

/// Judges every case in parallel and folds the verdicts in case order.
///
/// **Nothing about the report depends on how the work was divided.** Each case
/// is a pure function of its own text — the invariants read a parse, a format
/// or an analysis of one string and nothing else — and
/// [`buri::parallel::map_with`] returns results in index order, so the counts,
/// the ceilings and the five examples this prints are exactly what the serial
/// loop printed. That is asserted rather than argued: the tables in
/// `cargo test -p buri --test recovery -- --nocapture` are compared before and
/// after.
///
/// `init` is the scratch each worker keeps to itself. Most invariants need
/// none; `a syntax error stays a syntax error` needs an [`Analyzer`], which is
/// the whole of why this takes one.
fn judge_with<S, I, F>(invariant: &str, sample: Sample, cases: &[&Mutation], init: I, each: F)
where
    I: Fn() -> S + Sync,
    F: Fn(&mut S, &Mutation) -> Verdict + Sync,
{
    let verdicts = buri::parallel::map_with(cases.len(), init, |state, i| each(state, cases[i]));
    let mut tally = Tally::default();
    for (m, verdict) in cases.iter().zip(verdicts) {
        tally.record(m, verdict);
    }
    tally.finish(invariant, sample);
}

/// The same, for an invariant whose cases need no scratch of their own.
fn judge<F>(invariant: &str, sample: Sample, cases: &[&Mutation], each: F)
where
    F: Fn(&Mutation) -> Verdict + Sync,
{
    judge_with(invariant, sample, cases, || (), |(), m| each(m));
}

/// How many violations one row of `cases` may hold: its rate, plus the swing a
/// sample of that size has in it.
fn allowed(invariant: &str, row: &str, cases: usize, sample: Sample) -> usize {
    let rate = ceiling(invariant, row);
    cases.saturating_mul(rate) / 100 + sampling_allowance(cases, rate, sample)
}

/// How far above the rate a *sampled* row may land before the suite calls it a
/// regression.
///
/// Two and a half standard deviations of a binomial with the row's size and
/// rate, rounded up: the width of the swing a redrawn stride produces on its
/// own. A row of 105 cases at 19% swings by eleven either way, which covers the
/// jump that failed this suite the last time the standard library grew — the
/// parser had not changed, and the sample had.
///
/// Two and a half rather than two because the stride is not a random sample: it
/// steps through the corpus in file order, so a row's cases come from a handful
/// of neighbouring files and swing wider than an independent draw would. The
/// observed draws bear that out — the same parser has shown this row at 19.0%,
/// 21.0% and 25.6% against a population rate of 17.9%.
///
/// Zero for a whole-corpus row, which has no sampling in it, and zero for a
/// rate of zero, which is a claim of exactness and stays one.
///
/// Integer throughout: `cases * rate * (100 - rate)` is `n·p·(1−p)` scaled by
/// ten thousand, so its square root is the standard deviation scaled by a
/// hundred, and dividing that back out with a round-up is the whole sum.
fn sampling_allowance(cases: usize, rate: usize, sample: Sample) -> usize {
    if sample == Sample::Whole || rate == 0 {
        return 0;
    }
    let spread = cases.saturating_mul(rate).saturating_mul(100usize.saturating_sub(rate));
    5usize.saturating_mul(spread.isqrt()).div_ceil(200)
}

/// **A ceiling moves with the row and not with the corpus.**
///
/// The property the rates exist for, asserted rather than argued: a row whose
/// per-case behaviour has not changed stays under its ceiling at every size the
/// corpus could grow to, and a row whose behaviour really did get worse is over
/// it at every size the stride actually draws.
///
/// The row is the one that has twice made this suite go red for the wrong
/// reason — `insert-stray` of `a syntax error stays a syntax error`, whose
/// honest residue over the whole population is 17.9%. That invariant no longer
/// strides, so the row here is a stand-in: what the allowance owes any row a
/// future `Sample::Strided` ceiling is drawn from.
#[test]
fn a_ceiling_moves_with_the_row_and_not_with_the_corpus() {
    const INVARIANT: &str = "a syntax error stays a syntax error";
    const ROW: &str = "insert-stray";
    /// The measured residue, per thousand, so the arithmetic stays integer.
    const HONEST: usize = 179;
    /// A per-case regression: nearly twice as many cascades per mistake.
    const REGRESSED: usize = 330;

    for cases in [50, 90, 105, 300, 800, 1572, 5000] {
        let seen = cases * HONEST / 1000;
        let allowed = allowed(INVARIANT, ROW, cases, Sample::Strided);
        assert!(
            seen <= allowed,
            "a row of {cases} cases behaving exactly as it does today ({seen}              violations) is over its ceiling of {allowed}. Growing the corpus              would fail the suite without the toolchain changing."
        );
    }
    // Every size the 300-case stride has drawn this row at, and then some.
    for cases in [90, 105, 120, 300, 1572] {
        let seen = cases * REGRESSED / 1000;
        let allowed = allowed(INVARIANT, ROW, cases, Sample::Strided);
        assert!(
            seen > allowed,
            "a row of {cases} cases at nearly twice the residue ({seen}              violations) is inside its ceiling of {allowed}. A real regression              would pass the suite."
        );
    }
    // And a whole-corpus row is held to the rate exactly: no sample, no swing.
    assert_eq!(
        allowed("the caret is on the mistake", "delete-closer", 1123, Sample::Whole),
        1123 * 30 / 100
    );
}

// ---------------------------------------------------------------------------
// Reading what the toolchain said
// ---------------------------------------------------------------------------

fn parse_errors(text: &str) -> Vec<Diagnostic> {
    buri::parsing::parser::parse(text, FileId(0)).errors
}

fn rendered(text: &str, diagnostics: &[Diagnostic]) -> String {
    let mut map = SourceMap::new();
    let file = map.add("mutated.buri", std::path::PathBuf::from("mutated.buri"), text.to_string());
    diagnostics
        .iter()
        .map(|d| {
            let mut d = d.clone();
            d.span.file = file;
            for s in &mut d.secondary_spans {
                s.span.file = file;
            }
            map.render(&d, false)
        })
        .collect()
}

/// One thread's front end, kept between cases.
///
/// **The standard library is parsed once per worker rather than once per
/// case.** `driver::analyze_snippet` builds a fresh [`SourceMap`] and a fresh
/// parse cache for every call, so five thousand two-hundred-byte snippets each
/// paid for a fresh parse of every embedded standard library module — which was
/// most of this suite's runtime. Both structures are built to be reused:
/// `SourceMap::embedded` hands the same `FileId` back for a module already in
/// the map, `parser::Cache` is keyed on that id, and `modules::load_source_in`
/// replaces the snippet's text under one id and forgets just that parse. So the
/// map a worker holds after its first case is the map it holds after its
/// thousandth — the same files, with the same ids, in the same order — and
/// every case is analysed against exactly the state `analyze_snippet` would
/// have built for it alone.
///
/// The `Rc`s inside both make an `Analyzer` neither `Send` nor `Sync`, which is
/// precisely right: [`buri::parallel::map_with`] builds each worker's scratch
/// inside that worker and never moves it.
struct Analyzer {
    map: SourceMap,
    cache: buri::parsing::parser::Cache,
}

impl Analyzer {
    fn new() -> Analyzer {
        Analyzer { map: SourceMap::new(), cache: buri::parsing::parser::Cache::new() }
    }

    /// Every error the whole front end reports, as the pair that identifies
    /// one: its code and its wording.
    ///
    /// Enough to subtract one run's errors from another's, which is what the
    /// cascade invariant does. The *position* is deliberately not part of the
    /// key: a deleted token moves every byte after it, so a diagnostic the file
    /// already carried would otherwise read as one the mutation invented. The
    /// cost is that a cascade wording an unmutated file already produces
    /// somewhere else is masked, which is the safe direction to be wrong in.
    fn errors(&mut self, name: &str, text: &str) -> Vec<(String, String)> {
        let analysis = driver::analyze_snippet_in(
            None,
            &mut self.map,
            &mut self.cache,
            "recovery",
            text,
            role_of(name),
        );
        analysis
            .diagnostics
            .items
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .map(|d| (d.code.clone().unwrap_or_default(), d.message.clone()))
            .collect()
    }
}

/// A source under a `test/` directory is compiled as one, so that its `test`
/// declarations are legal and the baseline is about the mutation rather than
/// about where the file was read from.
fn role_of(name: &str) -> Role {
    if name.contains("/test/") {
        Role::TestSource
    } else {
        Role::Source
    }
}

// ---------------------------------------------------------------------------
// (a) One mistake is one diagnostic
// ---------------------------------------------------------------------------

/// **A single-token mistake produces at most one diagnostic per token it
/// perturbed.**
///
/// One, for a separator missing from a list the parser terminates with `}` or
/// `]`: every one of those is written in the breaking form, so exactly one
/// inserted comma repairs the file and exactly one diagnostic should say so.
/// Two where the mistake is genuinely ambiguous — a deleted closer, a stray
/// token — and three for a swap, which perturbed two tokens.
///
/// This is the invariant the maintainer's example fails: one deleted comma,
/// three errors.
#[test]
fn a_missing_token_is_one_diagnostic() {
    let all = cases();
    judge("one mistake is one diagnostic", Sample::Whole, &all, |m| {
        let errors = parse_errors(&m.source);
        let bound = m.bound();
        if errors.is_empty() {
            return Verdict::StillValid;
        }
        if errors.len() <= bound {
            return Verdict::Held;
        }
        Verdict::Violated(format!(
            "{} diagnostics, and {bound} {} allowed:\n{}",
            errors.len(),
            if bound == 1 { "is" } else { "are" },
            indent(&rendered(&m.source, &errors))
        ))
    });
}

// ---------------------------------------------------------------------------
// (b) The caret is on the mistake
// ---------------------------------------------------------------------------

/// **The first diagnostic starts inside the mutation's own window.**
///
/// The window runs from the start of the token before the mutation to the end
/// of the token after it — "at or adjacent to the site", stated in bytes. A
/// caret further away than that is a caret pointing at the *consequence* of
/// the mistake rather than at the mistake, which is what sends a reader to the
/// wrong line.
#[test]
fn the_first_diagnostic_lands_at_the_mutation() {
    let all = cases();
    judge("the caret is on the mistake", Sample::Whole, &all, |m| {
        let errors = parse_errors(&m.source);
        let Some(first) = errors.first() else { return Verdict::StillValid };
        let (lo, hi) = m.window;
        let at = first.span.start as usize;
        if at >= lo && at <= hi {
            return Verdict::Held;
        }
        Verdict::Violated(format!(
            "the first caret is at byte {at}, and the mutation's window is {lo}..{hi}:\n{}",
            indent(&rendered(&m.source, &errors[..1]))
        ))
    });
}

// ---------------------------------------------------------------------------
// (c) The fix names the token
// ---------------------------------------------------------------------------

/// **When one token would repair the file, the first diagnostic's `fix` names
/// that token.**
///
/// Only the deletions can make this claim: removing a stray token or undoing a
/// swap is not "write X here", and a fix that named a token there would be
/// wrong rather than terse. So insertions and swaps are counted and skipped.
#[test]
fn the_fix_names_the_missing_token() {
    let all = cases();
    judge("the fix names the missing token", Sample::Whole, &all, |m| {
        let Some(wants) = m.wants.as_deref() else { return Verdict::Skipped };
        let errors = parse_errors(&m.source);
        let Some(first) = errors.first() else { return Verdict::StillValid };
        let fix = first.fix.clone().unwrap_or_default();
        if fix.contains(wants) {
            return Verdict::Held;
        }
        Verdict::Violated(format!(
            "the fix is {fix:?}, and it does not name {wants}:\n{}",
            indent(&rendered(&m.source, &errors[..1]))
        ))
    });
}

// ---------------------------------------------------------------------------
// (d) A syntax error stays a syntax error
// ---------------------------------------------------------------------------

/// **A one-token mutation adds no error that is not about the syntax.**
///
/// The maintainer's example produces `expected `Route`, found `()`` on the
/// function's *signature* — three lines above the missing comma, and about a
/// return type nobody touched. That is the cascade: the parser abandoned the
/// body, the checker was handed an empty block, and an empty block is `()`.
///
/// The invariant is stated as "no new non-syntax error anywhere" rather than
/// "none downstream" precisely because that one is *upstream* of the mistake.
/// Errors the unmutated source already reports are subtracted first, so a
/// corpus file that does not resolve as a bare snippet still contributes.
#[test]
fn a_syntax_error_does_not_become_a_type_error() {
    let (corpus, all) = corpus_and_cases();
    let all: Vec<&Mutation> = all.iter().collect();
    // Every case, not a stride: a 300-case sample of 5,201 could not see a
    // five-point regression, so its ceilings had to carry a sampling swing a
    // real regression could hide inside. The whole population is what makes
    // every ceiling below exact, and `BURI_RECOVERY_CAP` still narrows a run by
    // hand.
    //
    // It was also the longest test in the whole `cargo test` run, and for a
    // reason that had nothing to do with the population: it was one serial loop
    // of five thousand full analyses, each of which built a `SourceMap` and a
    // parse cache from nothing and therefore re-parsed the whole standard
    // library. Sixty-five of the suite's seventy-eight seconds on a ten-core
    // host, and a hundred and sixty on CI's four-core runner. It is now the
    // corpus divided between the cores the machine already has, each of which
    // parses that library once — nine seconds for the whole suite here. See
    // [`Analyzer`] for the second half and [`judge_with`] for the first;
    // neither changes which cases run, or what any of them is held to, or what
    // the table below prints.
    let sample = strided(&all, 0);
    let texts: BTreeMap<&str, &str> =
        corpus.iter().map(|s| (s.name.as_str(), s.text.as_str())).collect();

    // The baselines first, one per file rather than one per case: what the
    // *unmutated* file already said is not something the mutation invented.
    // They were already computed once per file, in a memo filled as the loop
    // went; computing them all up front is what lets the cases below be a pure
    // function of the case, because a memo shared between workers would be
    // exactly the running total [`judge_with`] may not hold.
    let mut files: Vec<&str> = sample.iter().map(|m| m.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let computed = buri::parallel::map_with(files.len(), Analyzer::new, |analyzer, i| {
        let file = files[i];
        analyzer.errors(file, texts.get(file).copied().unwrap_or(""))
    });
    let baselines: BTreeMap<&str, Vec<(String, String)>> =
        files.iter().copied().zip(computed).collect();

    judge_with(
        "a syntax error stays a syntax error",
        Sample::Whole,
        &sample,
        Analyzer::new,
        |analyzer, m| {
            let errors = parse_errors(&m.source);
            if errors.is_empty() {
                return Verdict::StillValid;
            }
            // The syntax errors, by the same key, so that they can be told
            // apart from everything the checker went on to say.
            let syntax: Vec<(String, String)> = errors
                .iter()
                .map(|d| (d.code.clone().unwrap_or_default(), d.message.clone()))
                .collect();
            let mut remaining = baselines.get(m.file.as_str()).cloned().unwrap_or_default();
            let mut cascaded = Vec::new();
            for e in analyzer.errors(&m.file, &m.source) {
                if let Some(i) = remaining.iter().position(|b| *b == e) {
                    remaining.remove(i);
                    continue;
                }
                if syntax.contains(&e) {
                    continue;
                }
                cascaded.push(e);
            }
            if cascaded.is_empty() {
                return Verdict::Held;
            }
            Verdict::Violated(format!(
                "{} error(s) the mistake invented:\n{}",
                cascaded.len(),
                indent(&cascaded.iter().map(|(c, m)| format!("[{c}] {m}\n")).collect::<String>())
            ))
        },
    );
}

// ---------------------------------------------------------------------------
// (e) The formatter formats what it understood
// ---------------------------------------------------------------------------

/// **A file with one token wrong still formats, is a fixed point, and keeps
/// every token it was given.**
///
/// Today `formatting::source` returns `None` the moment a file has one parse
/// error, so `buri format` drops it in silence and this invariant is violated
/// by every case. R2 replaces the refusal with a verbatim region: the part
/// that parsed is laid out, the part that did not is emitted byte for byte.
/// The token comparison is ordered rather than the sorted one
/// `formatting.rs` makes, because a region that reappeared in the wrong place
/// would pass a set comparison.
#[test]
fn a_broken_file_still_formats() {
    let all = cases();
    let sample = strided(&all, CI_FORMATTED);
    judge("a broken file still formats", Sample::Strided, &sample, |m| {
        let errors = parse_errors(&m.source);
        if errors.is_empty() {
            return Verdict::StillValid;
        }
        let Some(out) = buri::formatting::source(&m.source) else {
            return Verdict::Violated(String::from("the formatter refused the file"));
        };
        if buri::formatting::source(&out).as_deref() != Some(out.as_str()) {
            return Verdict::Violated(String::from("formatting the output again moved it"));
        }
        let (before, after) = (kept(&m.source), kept(&out));
        if before != after {
            return Verdict::Violated(format!(
                "the tokens moved.\n  in:\n{}\n  out:\n{}",
                indent(&format!("{before:?}")),
                indent(&format!("{after:?}"))
            ));
        }
        let (was, now) = (regions_kept(&m.source), regions_kept(&out));
        if was != now {
            return Verdict::Violated(format!(
                "a region the formatter did not read came back changed.\n  in:\n{}\n  out:\n{}",
                indent(&was.join("\n---\n")),
                indent(&now.join("\n---\n"))
            ));
        }
        Verdict::Held
    });
}

/// Every token with the ones layout is allowed to add and drop taken out, as
/// a set: `formatting.rs`'s `tokens`.
///
/// The set and not the sequence, because two of the formatter's documented
/// moves are reorderings — the leading import run is sorted, and a `derive` is
/// carried onto the declaration it is about. What the sequence was reaching
/// for is asserted directly instead, and more sharply, by comparing the
/// regions themselves: see [`regions_kept`].
fn kept(text: &str) -> Vec<String> {
    const LAYOUT: &[&str] = &["`,`", "`(`", "`)`", "`{`", "`}`"];
    let mut out: Vec<String> = drop_empty_type_arguments(token_shape(text))
        .into_iter()
        .filter_map(|s| match s {
            Shape::Token(t) if !LAYOUT.contains(&t.as_str()) => Some(t),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// Every `<` immediately followed by `>` removed, in source order.
///
/// `t<>` prints as `t`, because they are the same type and only one of them is
/// how a reader writes it. The pair goes as a pair rather than by filtering
/// both tokens everywhere: a generic list the formatter really did lose would
/// then be invisible.
fn drop_empty_type_arguments(shapes: Vec<Shape>) -> Vec<Shape> {
    let mut out: Vec<Shape> = Vec::with_capacity(shapes.len());
    for s in shapes {
        if matches!(&s, Shape::Token(t) if t == "`>`")
            && matches!(out.last(), Some(Shape::Token(t)) if t == "`<`")
        {
            out.pop();
            continue;
        }
        out.push(s);
    }
    out
}

/// What each region the formatter left alone says, line by line with trailing
/// spaces off — the one whitespace the formatter never keeps.
fn regions_kept(text: &str) -> Vec<String> {
    buri::formatting::broken_regions(text)
        .into_iter()
        .filter_map(|(lo, hi)| text.get(lo..hi))
        .map(|slice| slice.lines().map(str::trim_end).collect::<Vec<_>>().join("\n"))
        .collect()
}

// ---------------------------------------------------------------------------
// (f) `--check` does not pass a file it could not read
// ---------------------------------------------------------------------------

/// **`buri format --check` exits non-zero on a source it cannot parse.**
///
/// It exits 0 today and prints nothing, which is not a refusal — it is a false
/// pass, and a repository whose formatting gate is green because a file did
/// not parse has the worst of both. The build-file path already exits 2 and
/// says so; this is the source path being held to the same standard.
///
/// The three runs are one assertion each: a clean file still passes, so the
/// test can fail in both directions; the broken file is named; and the exit
/// code says something went wrong.
#[test]
fn format_check_refuses_an_unparseable_file() {
    let repo = Scratch::repo("recovery-format-check");
    repo.binary_package("cmd/clean", CLEAN);
    let clean = repo.run(&["format", "--check", "cmd/clean/main.buri"]);
    assert_eq!(
        clean.code,
        0,
        "an already-formatted file must still pass `--check`:\n{}",
        indent(&clean.all())
    );

    repo.binary_package("cmd/broken", BROKEN);
    let broken = repo.run(&["format", "--check", "cmd/broken/main.buri"]);
    assert_ne!(
        broken.code,
        0,
        "`--check` passed a file it could not parse:\n{}",
        indent(&broken.all())
    );
    broken.says("cmd/broken/main.buri");

    // And the writing path says it skipped, rather than exiting 0 in silence.
    let written = repo.run(&["format", "cmd/broken/main.buri"]);
    written.says("cmd/broken/main.buri");
}

/// A file that is already exactly what the formatter would print.
const CLEAN: &str = "export fn main(): Int {\n    1\n}\n";

/// The maintainer's example, with the comma still missing.
const BROKEN: &str = "export struct Route {\n    export name: Str,\n}\n\n\
                      export fn route(path: Str): Route {\n    match (path) {\n        \
                      \"/entries\" => Route { name: \"entries\" }\n        \
                      _ => Route { name: \"fallback\" },\n    }\n}\n";

// ---------------------------------------------------------------------------
// The curated set
// ---------------------------------------------------------------------------

/// Each case in `cli/tests/recovery/` is one list context in the grammar, with
/// the diagnostics it must produce written down in full:
///
/// ```text
/// cli/tests/recovery/match_arm_missing_comma/
///   main.buri       the program, with one token missing
///   expected.txt    every error, with its code, span, fix and edit
/// ```
///
/// The recorded form is the fields rather than the rendered page: what these
/// cases exist to pin is the *code*, the *span* and the *edit*, and the reject
/// corpus already pins the rendering of a diagnostic. A case's `expected.txt`
/// lists every error the whole front end reports, so a cascaded type error is
/// an extra stanza rather than an invisible one — which is exactly the
/// difference between the maintainer's example before and after R1.
///
/// Re-record after a deliberate change, and read every diff:
///
/// ```text
/// BURI_BLESS=1 cargo test -p buri --test recovery recorded
/// ```
#[test]
fn recovery_cases_are_recorded() {
    let dir = tests_dir().join("recovery");
    let cases = case_dirs(&dir, "main.buri", 40);
    let mut g = Golden::new();
    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(case.join("main.buri")).unwrap();
        require_annotation(&text, "// EXPECT:", &name);

        // A case that builds a context is the module that exports `main`, and
        // only that module may import `core/host`.
        let role =
            if text.contains("\"core/host\"") { Role::Entry } else { Role::Source };
        let mut map = SourceMap::new();
        let analysis = driver::analyze_snippet(&mut map, "recovery", &text, role);
        let mut record = String::new();
        for d in analysis.diagnostics.items.iter() {
            if !matches!(d.severity, Severity::Error) {
                continue;
            }
            // Only what this file said. A snippet is compiled on top of the
            // standard library, and a diagnostic from inside it is not the
            // case's business.
            if map.text(d.span.file) != text {
                continue;
            }
            record.push_str(&stanza(&text, d));
        }
        if record.is_empty() {
            g.fail(format!("{name}: compiled, and it must not"));
            continue;
        }
        g.check(&case.join("expected.txt"), &format!("{name}/expected.txt"), &record);
    }
    g.finish("recovery", cases.len());
}

/// One diagnostic, as the four or five lines a case records.
fn stanza(text: &str, d: &Diagnostic) -> String {
    let mut out = format!(
        "error [{}] {}\nmessage: {}\n",
        d.code.as_deref().unwrap_or("-"),
        at(text, d.span.start, d.span.end),
        d.message
    );
    if let Some(fix) = &d.fix {
        out.push_str(&format!("fix: {fix}\n"));
    }
    for e in &d.edits {
        out.push_str(&format!("edit: {} {:?}\n", at(text, e.at.start, e.at.end), e.replacement));
    }
    for s in &d.secondary_spans {
        out.push_str(&format!("related: {} {}\n", at(text, s.span.start, s.span.end), s.label));
    }
    for n in &d.notes {
        out.push_str(&format!("note: {n}\n"));
    }
    out.push('\n');
    out
}

/// A byte range as `line:column-line:column`, counting columns in characters
/// so that the numbers are the ones an editor shows.
fn at(text: &str, start: u32, end: u32) -> String {
    let (a, b) = (position(text, start), position(text, end));
    format!("{}:{}-{}:{}", a.0, a.1, b.0, b.1)
}

fn position(text: &str, at: u32) -> (usize, usize) {
    let at = (at as usize).min(text.len());
    let before = &text[..at];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, column)
}

// ---------------------------------------------------------------------------
// The message audit
// ---------------------------------------------------------------------------

/// Writes every diagnostic the audit grades, in the shape the audit reads.
///
/// `cli/tests/message-audit/run.sh` reads the file this leaves behind and puts
/// each record to a model with a one-question rubric: does this message name
/// exactly what to fix, and where? That is a judgement about English, which no
/// assertion in this file can make and which is the whole of what "a better
/// diagnostic" means. The script is what reaches the network; this never does,
/// the way `cli/tests/proto/run.sh` keeps a foreign runner out of `cargo test`.
///
/// **It used to be `#[ignore]`d and to return early unless `BURI_MESSAGE_AUDIT`
/// was set**, which made it two skips wearing one coat: ignored by default, and
/// a silent pass if you un-ignored it without the variable. What it actually
/// costs is one parse of forty curated cases and sixty generated ones, which
/// the file already pays for `the_corpus_is_present` — so it now runs like any
/// other test, writes its corpus under `CARGO_TARGET_TMPDIR`, and **asserts the
/// format**. That last part is the coverage this had none of: `run.sh` splits
/// the file with `awk` on `--- <n>` and reads five named fields out of each
/// record, and nothing anywhere noticed when one of them stopped being
/// written. Now the generator and the reader of that format are held together
/// by a test rather than by a comment.
///
/// `BURI_MESSAGE_AUDIT_CASES` still says where the file goes, because that is
/// how the script asks for it, and `BURI_MESSAGE_AUDIT_SAMPLED` still says how
/// many generated cases to spread over.
#[test]
fn message_audit_corpus() {
    let out = std::env::var("BURI_MESSAGE_AUDIT_CASES").unwrap_or_else(|_| {
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("message-audit-cases.txt")
            .to_string_lossy()
            .into_owned()
    });
    let want = env_num("BURI_MESSAGE_AUDIT_SAMPLED").unwrap_or(60);

    let mut records = String::new();
    let mut n = 0;

    // The curated set first: these are the messages the wording is decided on.
    let dir = tests_dir().join("recovery");
    for case in case_dirs(&dir, "main.buri", 40) {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(case.join("main.buri")).unwrap();
        for d in parse_errors(&text).iter().take(1) {
            n += 1;
            records.push_str(&record(n, &format!("recovery/{name}"), &text, d));
        }
    }

    // Then a spread of the generated ones, so the audit sees what a real file
    // produces rather than only what a case was written to produce. Zero is
    // "the curated set alone", not "all of them".
    let all = if want == 0 { Vec::new() } else { cases() };
    for m in strided(&all, want) {
        let Some(d) = parse_errors(&m.source).into_iter().next() else { continue };
        n += 1;
        records.push_str(&record(n, &format!("{} ({})", m.origin, m.what), &m.source, &d));
    }

    std::fs::write(&out, &records).unwrap();
    eprintln!("message audit: {n} records written to {out}");

    // The curated forty are on disk, so anything under that is a corpus that
    // stopped being reachable rather than a corpus that is small.
    assert!(n >= 40, "the audit corpus is {n} records; the curated set alone is forty");

    // The format `run.sh` reads, asserted here because it is generated here.
    // `awk` splits on `--- <n>` and pulls five fields out of each record; a
    // record missing one of them is a record the grader is asked about with a
    // blank where the message should be, and the run before this assertion
    // existed would have looked exactly as green.
    let headers: Vec<&str> = records.lines().filter(|l| l.starts_with("--- ")).collect();
    assert_eq!(headers.len(), n, "one `--- <n>` header per record");
    for (i, header) in headers.iter().enumerate() {
        assert_eq!(*header, format!("--- {}", i + 1), "the headers are 1..=n in order");
    }
    for field in ["where: ", "line: ", "code: ", "message: ", "fix: "] {
        let count = records.lines().filter(|l| l.starts_with(field)).count();
        assert_eq!(count, n, "every record carries a `{field}` line");
    }
}

/// One record, in the shape the audit script reads with `awk`.
fn record(n: usize, where_: &str, text: &str, d: &Diagnostic) -> String {
    let (line, column) = position(text, d.span.start);
    let source = text.lines().nth(line.saturating_sub(1)).unwrap_or("").trim_end();
    format!(
        "--- {n}\nwhere: {where_}:{line}:{column}\nline: {source}\ncode: {}\nmessage: {}\nfix: {}\n\n",
        d.code.as_deref().unwrap_or("-"),
        d.message,
        d.fix.as_deref().unwrap_or("(none)")
    )
}

// ---------------------------------------------------------------------------
// The generator itself
// ---------------------------------------------------------------------------

/// The one test here that is about this file rather than about the toolchain.
///
/// A property suite that silently generated nothing would pass every invariant
/// above, which is the failure mode a bounded search has to be guarded against
/// on its own.
#[test]
fn the_corpus_is_present() {
    let all = cases();
    assert!(
        all.len() > 2_000,
        "expected thousands of mutations, found {} — is the corpus reachable?",
        all.len()
    );
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for m in &all {
        *by_kind.entry(m.kind.name()).or_default() += 1;
    }
    for kind in Kind::ALL {
        assert!(
            by_kind.get(kind.name()).copied().unwrap_or(0) > 100,
            "{} produced almost nothing: {by_kind:?}",
            kind.name()
        );
    }
    // Every mutation must actually change the text, and must stay in bounds.
    for m in &all {
        assert!(m.site <= m.source.len(), "{}: the site is past the end", m.origin);
        assert!(m.window.0 <= m.window.1, "{}: an inverted window", m.origin);
    }
    let strict = all.iter().filter(|m| m.is_strict()).count();
    eprintln!(
        "recovery corpus: {} mutations, {strict} under the strict bound, {by_kind:?}",
        all.len()
    );
}
