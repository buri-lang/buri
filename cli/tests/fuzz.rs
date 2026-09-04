//! **Generated and mutated input, against the properties that must hold for
//! all of it.**
//!
//! Every other suite here asks a question somebody wrote down. This one asks
//! the same questions over input nobody wrote: a program drawn from the
//! benchmark generator's parameter space, or a checked-in source with bytes
//! moved, deleted and spliced into it. A corpus is a sample of the input
//! space and a property is a claim about all of it, and the gap between the
//! two is where the bugs are — `adversarial.rs` holds thirty hostile files
//! that each crashed the toolchain once, and the reason they are in a file is
//! that somebody thought of them.
//!
//! ```text
//! cargo test -p buri --test fuzz                       # CI mode: fixed seeds, bounded
//! BURI_FUZZ_SECONDS=600 cargo test -p buri --test fuzz # soak: per-property wall clock
//! BURI_FUZZ_ITERS=50000 cargo test -p buri --test fuzz # soak: per-property iterations
//! BURI_FUZZ_SEED=0x1234 cargo test -p buri --test fuzz # a different point to start from
//! BURI_FUZZ_RECORD=1 …                                 # write findings into the corpus
//! ```
//!
//! **CI mode is deterministic and bounded.** Every search starts from
//! [`BASE_SEED`] and stops after a fixed number of draws, so two runs on one
//! machine compile the same bytes and a failure is a failure of the toolchain
//! rather than of the day. The soak mode is the same code with the bound
//! moved, which is the only way a search that runs for ten minutes and a
//! search that runs for one second can be the same search.
//!
//! # The five properties
//!
//! A finding is only useful if it can be replayed, and it can only be replayed
//! if what it claims is one sentence. So every search reduces its finding to
//! one of five properties, each of which takes a file and answers "does this still
//! break":
//!
//! | Property | The claim |
//! |---|---|
//! | `safety` | The **binary** neither panics, nor overflows its stack, nor prints `internal compiler error`, nor fails to stop, on these bytes. |
//! | `roundtrip` | `format` accepts its own output, is a fixed point, and keeps every comment and token. |
//! | `compiles` | The source type-checks with no error diagnostic and exports a `main`. |
//! | `deterministic` | Two compilations of it print byte-identical diagnostics. |
//! | `output` | Every backend runs it and prints the manifest's `expects`, and they agree with each other. |
//!
//! `safety` is `adversarial.rs`'s promise — no input panics the toolchain —
//! asked of input nobody chose. `roundtrip` is `formatting.rs`'s four claims,
//! asked over generated shapes rather than over sixty hand-written cases.
//! `compiles` is the benchmark's `--validate` generalised off the twenty named
//! profiles onto the whole parameter space. `output` is
//! `native/agreement.rs`'s comparison — one analysis, two backends, stdout
//! compared byte for byte — with the program generated rather than written.
//!
//! # What each property catches
//!
//! `safety` catches the crash: a panic, a stack overflow, an allocation that
//! kills the process, a loop that does not end. `roundtrip` catches the
//! formatter losing or inventing something, which no assertion inside a
//! program can see. `compiles` catches a front end that refuses a program it
//! should accept — the class the reject corpus is structurally unable to hold,
//! since every file in it is supposed to be refused. `deterministic` catches
//! hash order leaking into output, which this repository has already shipped
//! once: `did you mean` picked an arbitrary winner among equally-close
//! candidates and printed a different name on each run. `output` catches the
//! miscompile, which is the only bug class here that a compiler can have while
//! passing every other suite.
//!
//! # The regression corpus
//!
//! A fuzzer that finds a bug and forgets it has found nothing. Every finding
//! is minimised — lines first, then tokens, then characters, the property
//! re-checked at each step — and written into `cli/tests/fuzz/` as a directory
//! holding a manifest and the input:
//!
//! ```text
//! cli/tests/fuzz/roundtrip_trailing_block_comment/
//!   CASE.textproto     doc, property, status, and where it came from
//!   input.buri         the minimised bytes, and nothing else
//! ```
//!
//! The manifest's `status` is what lets a fuzzer live in a suite that has to
//! stay green:
//!
//!   * `status: FIXED` — the property must **hold**. This is the ordinary
//!     regression: the bug was fixed and may never come back.
//!   * `status: OPEN` — the property must **still** fail, and `replay` prints
//!     it. A known-open finding is pinned rather than quarantined, so it
//!     cannot be forgotten, and the day somebody fixes it the suite fails and
//!     says to move the case to `FIXED`.
//!
//! That is Swift's `compiler_crashers` / `compiler_crashers_fixed` split and
//! rustc's `//@ known-bug:` header, in one field. The input file holds the
//! bytes and nothing else — no header comment — because a comment prepended to
//! a formatter repro is a change to the input the case exists to pin.
//!
//! **Nothing writes into the checked-in tree unless asked.** A search that
//! fires prints the minimised case and fails; `BURI_FUZZ_RECORD=1` writes it
//! into the corpus as well, which is how a soak run's findings become cases.

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
    reason = "test code. The lint set in `Cargo.toml` pins a promise about the \
              toolchain — that no input panics it — and a harness that drives \
              the toolchain is not the toolchain. A test that unwraps fails on \
              the line that broke, which is what a test is for, and threading \
              `?` through an assertion buys nothing. `clippy.toml` exempts \
              `#[test]` functions already; this covers the helpers around them."
)]

#[path = "harness/mod.rs"]
mod harness;

// The benchmark's generator, as a module rather than a copy. It is the
// repository's Csmith: seeded, parameterised over twenty-seven dimensions, and
// already required to emit programs that compile — which is exactly the
// contract the `compiles` property checks over the rest of the space.
#[path = "../benches/generate.rs"]
#[allow(dead_code)]
mod generate;

use buri::build::textproto;
use buri::compiler::driver;
use buri::compiler::modules::Role;
use buri::diagnostics::{FileId, Severity, SourceMap};
use buri::formatting::{comment_shape, source, source_unchecked, token_shape, Shape};
use harness::{indent, Scratch};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// The budget
// ---------------------------------------------------------------------------

/// Where every search starts when nothing says otherwise.
///
/// Fixed, for the reason `generate.rs`'s seed is fixed: a suite whose input
/// moves between runs reports the day rather than the toolchain, and a finding
/// nobody can reproduce is a rumour.
const BASE_SEED: u64 = 0x0B00_1A57_F0FF_0001;

/// How long shrinking one finding may take, as a wall-clock ceiling.
///
/// A minimiser is a convenience and the finding is the point, so this is
/// generous rather than tight and the failure is reported either way — a case
/// that is half-shrunk is a case a person can still read and replay. What it
/// rules out is the shape that made it necessary: a property whose step is a
/// compile and a link, over an input large enough that the character passes
/// alone are tens of thousands of them.
const MINIMISE_CEILING: Duration = Duration::from_secs(90);

/// How long one property's search may take in CI, as a wall-clock ceiling.
///
/// A bound in iterations alone is not enough: an iteration's cost is a
/// function of the input, and one draw that generates ten thousand lines
/// costs a hundred that generate two hundred. Both bounds apply and whichever
/// runs out first stops the search.
const CI_CEILING: Duration = Duration::from_secs(20);

/// What a search may spend.
struct Budget {
    iters: usize,
    deadline: Instant,
    /// True when an environment variable moved the bound, which is what
    /// distinguishes a soak run's report from a CI run's silence.
    soaking: bool,
}

impl Budget {
    /// `ci` is the iteration count a CI run makes. `BURI_FUZZ_ITERS` and
    /// `BURI_FUZZ_SECONDS` each replace one half of the bound and leave the
    /// other at a ceiling, so either one alone is enough to soak.
    fn new(ci: usize) -> Budget {
        let iters = env_num("BURI_FUZZ_ITERS");
        let seconds = env_num("BURI_FUZZ_SECONDS");
        let soaking = iters.is_some() || seconds.is_some();
        Budget {
            // Either variable alone is enough to soak: naming one lifts the
            // other end of the bound rather than leaving it where CI put it,
            // because `BURI_FUZZ_SECONDS=600` that stopped after the CI
            // iteration count would be a soak that ran for four seconds.
            iters: match (iters, seconds) {
                (Some(n), _) => n,
                (None, Some(_)) => usize::MAX,
                (None, None) => ci,
            },
            deadline: Instant::now()
                + match (iters, seconds) {
                    (_, Some(s)) => Duration::from_secs(s as u64),
                    // The CI deadline is a safety valve rather than a bound: a
                    // machine slow enough to reach it has a search that
                    // covered less ground, and a suite that took a minute
                    // instead of ten seconds would be worse.
                    (Some(_), None) => Duration::from_secs(86_400),
                    (None, None) => CI_CEILING,
                },
            soaking,
        }
    }

    fn spent(&self, done: usize) -> bool {
        done >= self.iters || Instant::now() >= self.deadline
    }

    /// The one line a search prints when it finds nothing, so that a soak run
    /// says how much ground it covered rather than passing silently.
    fn report(&self, what: &str, done: usize, started: Instant) {
        eprintln!(
            "fuzz {what}: {done} iterations in {:.1}s{}",
            started.elapsed().as_secs_f64(),
            if self.soaking { " (soak)" } else { "" }
        );
    }
}

fn env_num(key: &str) -> Option<usize> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}

/// The seed every search starts from, `BURI_FUZZ_SEED` or [`BASE_SEED`].
fn base_seed() -> u64 {
    match std::env::var("BURI_FUZZ_SEED") {
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

fn recording() -> bool {
    std::env::var_os("BURI_FUZZ_RECORD").is_some()
}

// ---------------------------------------------------------------------------
// The PRNG
// ---------------------------------------------------------------------------

/// SplitMix64, the same four lines `benches/generate.rs` uses and for the same
/// reason: no dependency, and the same sequence on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn span(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.below((hi - lo + 1) as usize) as u32
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

// ---------------------------------------------------------------------------
// The seed corpus
// ---------------------------------------------------------------------------

/// Every checked-in source a mutation search starts from.
///
/// Valid programs, because a mutation fuzzer that starts from noise never gets
/// past the lexer: the interesting states of a compiler are the ones a
/// well-formed program reaches, and a corpus of them is what lets a single
/// deleted brace land in the middle end rather than in the first diagnostic.
/// This is the seed-corpus half of grammar-aware fuzzing, bought for nothing —
/// the repository already holds nine hundred `.buri` files.
fn seed_corpus() -> Vec<(String, String)> {
    let root = harness::repo_root();
    let mut out: Vec<(String, String)> = Vec::new();
    for dir in [
        "cli/tests/conformance",
        "cli/tests/crash",
        "cli/tests/formatting",
        "cli/tests/golden_javascript",
        "cli/tests/example",
        "cli/tests/reject",
    ] {
        walk_sources(&root.join(dir), &root, &mut out);
    }
    out.sort();
    assert!(out.len() > 200, "expected the checked-in sources, found {}", out.len());
    out
}

fn walk_sources(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            walk_sources(&p, root, out);
            continue;
        }
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if !name.ends_with(".buri") || name == "BUILD.buri" || name == "REPO.buri" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        // A seed larger than a screenful minimises into a case nobody can
        // read, and the corpus has hundreds of files under that size.
        if text.len() > 8_000 {
            continue;
        }
        out.push((p.strip_prefix(root).unwrap_or(&p).display().to_string(), text));
    }
}

// ---------------------------------------------------------------------------
// The mutators
// ---------------------------------------------------------------------------

/// Fragments a random draw would never assemble.
///
/// Byte-level mutation reaches the lexer and stops; what reaches the parser and
/// the checker is a mutation that produces something *grammatical enough*. So
/// the insertion table is the language's own vocabulary — every keyword, the
/// punctuators that open and close, and the literal forms with their own
/// scanner path — which is the cheap half of grammar-aware fuzzing.
const VOCABULARY: &[&str] = &[
    "as", "const", "context", "ctx", "derive", "effect", "else", "enum", "export", "false",
    "fn", "for", "from", "if", "impl", "import", "let", "match", "self", "Self", "struct",
    "test", "trait", "true", "type", "{", "}", "(", ")", "[", "]", "<", ">", "=>", "->", "?",
    "??", "..", "...", ".", ",", ";", ":", "::", "=", "==", "!", "-", "+", "*", "/", "%", "&&",
    "||", "|", "&", "^", "~", "#", "@", "$", "\\", "\"", "'", "`", "${", "\\u{", "//", "/*",
    "*/", "///", "//!", "0x", "0b", "1e999", "'a'", "\"s\"", "Int", "Str", "Float", "Bool",
    "Option", "Result", ".Some", ".None", ".Ok", ".Err", "_", "\u{a0}", "\u{200f}", "\u{0}",
    "\u{1f600}", "×",
];

/// One edit to one file.
///
/// Each is a shape a real breakage has: a byte a copy-and-paste corrupted, a
/// line a merge dropped, a construct repeated until it is a tree, a fragment
/// spliced in from somewhere else. `repeat` is the one that finds depth bugs,
/// and it is why the table below is weighted rather than uniform.
fn mutate(text: &str, others: &[(String, String)], rng: &mut Rng) -> String {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return String::from("{");
    }
    let mut out: Vec<u8> = bytes.to_vec();
    // One to four edits: a single edit usually lands in a comment, and a dozen
    // is noise no seed survives.
    for _ in 0..1 + rng.below(4) {
        if out.is_empty() {
            out = b"{".to_vec();
        }
        match rng.below(11) {
            // Overwrite a byte.
            0 => {
                let at = rng.below(out.len());
                out[at] = rng.below(256) as u8;
            }
            // Delete a run.
            1 => {
                let at = rng.below(out.len());
                let len = (1 + rng.below(64)).min(out.len() - at);
                out.drain(at..at + len);
            }
            // Duplicate a run, which is how a chain becomes long enough to
            // matter.
            2 => {
                let at = rng.below(out.len());
                let len = (1 + rng.below(128)).min(out.len() - at);
                let run: Vec<u8> = out[at..at + len].to_vec();
                let to = rng.below(out.len());
                splice(&mut out, to, &run);
            }
            // Insert a fragment of the language.
            3 | 4 => {
                let at = rng.below(out.len());
                splice(&mut out, at, rng.pick(VOCABULARY).as_bytes());
            }
            // Repeat a fragment far enough to be a depth attack.
            5 => {
                let at = rng.below(out.len());
                let frag = *rng.pick(VOCABULARY);
                let n = *rng.pick(&[8usize, 64, 512, 4_096, 40_000]);
                splice(&mut out, at, frag.repeat(n).as_bytes());
            }
            // Splice a run in from another seed.
            6 | 7 => {
                let (_, donor) = rng.pick(others);
                let d = donor.as_bytes();
                if !d.is_empty() {
                    let from = rng.below(d.len());
                    let len = (1 + rng.below(200)).min(d.len() - from);
                    let at = rng.below(out.len());
                    splice(&mut out, at, &d[from..from + len]);
                }
            }
            // Delete a line, which is the edit a bad merge makes.
            8 => {
                let text = String::from_utf8_lossy(&out).to_string();
                let lines: Vec<&str> = text.lines().collect();
                if lines.len() > 1 {
                    let drop = rng.below(lines.len());
                    let kept: Vec<&str> = lines
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != drop)
                        .map(|(_, l)| *l)
                        .collect();
                    out = kept.join("\n").into_bytes();
                }
            }
            // Swap two words, which keeps the token count and moves the shape.
            9 => {
                let text = String::from_utf8_lossy(&out).to_string();
                let mut words: Vec<&str> = text.split_whitespace().collect();
                if words.len() > 1 {
                    let (a, b) = (rng.below(words.len()), rng.below(words.len()));
                    words.swap(a, b);
                    out = words.join(" ").into_bytes();
                }
            }
            // Truncate, which is what a file cut off mid-write looks like.
            _ => {
                let at = 1 + rng.below(out.len());
                out.truncate(at);
            }
        }
    }
    // The toolchain must survive bytes that are not UTF-8 — `adversarial.rs`
    // holds a case for exactly that — but a corpus entry that is not text is
    // a case nobody can read in a diff, and the searches here compare strings.
    // So the edit is made over bytes and the result is made text again.
    String::from_utf8_lossy(&out).to_string()
}

fn splice(out: &mut Vec<u8>, at: usize, what: &[u8]) {
    let at = at.min(out.len());
    let tail: Vec<u8> = out.split_off(at);
    out.extend_from_slice(what);
    out.extend_from_slice(&tail);
}

// ---------------------------------------------------------------------------
// The properties
// ---------------------------------------------------------------------------

/// Every property a recorded case can pin, by the name its manifest carries.
const PROPERTIES: &[&str] = &[
    "safety",
    "roundtrip",
    "compiles",
    "deterministic",
    "output",
    "generated",
];

/// What a case's input file is called, which is also what says how to read it.
///
/// Five of the six properties take Buri source. `generated` takes a point in the
/// benchmark generator's parameter space — `key=value`, one per line — because
/// the finding it reports is *what the generator emitted*, and recording the
/// emitted source instead would pin a claim about the language that nobody
/// makes: `derive  for Rec;` is not a program the compiler should accept, it
/// is a program the generator should not print.
fn input_name(property: &str) -> &'static str {
    if property == "generated" {
        "input.params"
    } else {
        "input.buri"
    }
}

/// Why this input breaks the named property, or `None` when it does not.
///
/// One function rather than five, because the minimiser and the corpus replay
/// both have to ask a question they were handed the name of. `expects` is the
/// `output` property's other half and is ignored by the rest.
fn fires(property: &str, input: &str, expects: Option<&str>) -> Option<String> {
    match property {
        "safety" => safety_fires(input),
        "roundtrip" => roundtrip_fires(input),
        "compiles" => compiles_fires(input),
        "deterministic" => deterministic_fires(input),
        "output" => output_fires(input, expects.unwrap_or_default()),
        "generated" => generated_fires(input),
        other => Some(format!("`{other}` is not a property; they are {PROPERTIES:?}")),
    }
}

// -- safety -----------------------------------------------------------------

/// How long the binary gets before "it has not stopped" becomes the finding.
///
/// Generous rather than tight: the suite's own binary is unoptimized, the
/// machine is running other tests beside this one, and a false hang is a
/// finding nobody can reproduce. A real hang is not a slow compile.
const WATCHDOG: Duration = Duration::from_secs(30);

/// The `safety` property: the toolchain, through the binary, on these bytes.
///
/// Through the binary rather than the library for `adversarial.rs`'s two
/// reasons — the binary is what a user runs, and the stack it runs on is
/// `main.rs`'s 256 MiB rather than a test thread's — and with a watchdog,
/// because a compiler that does not stop is a bug the library cannot report
/// about itself.
fn safety_fires(input: &str) -> Option<String> {
    let s = Scratch::repo("fuzz-safety");
    s.write("app/BUILD.buri", harness::JS_BINARY);
    s.write("app/main.buri", input);
    let all = match run_watched(&s.root, &["build", "//app"], WATCHDOG) {
        Ok(text) => text,
        Err(finding) => return Some(finding),
    };
    if all.contains("panicked at") {
        return Some(format!("the toolchain panicked:\n{}", indent(&all)));
    }
    if all.contains("overflowed its stack") || all.contains("stack overflow") {
        return Some(format!("the toolchain overflowed its stack:\n{}", indent(&all)));
    }
    if all.contains("internal compiler error") {
        return Some(format!(
            "an invariant the toolchain claims input cannot break was broken by input:\n{}",
            indent(&all)
        ));
    }
    None
}

/// Runs the binary with a deadline.
///
/// `Ok(text)` is "it stopped, and said this". `Err(finding)` is the two ways
/// stopping badly looks from outside: it did not stop at all, or it was killed
/// by a signal — which is what a stack overflow and an out-of-memory kill are,
/// and `adversarial.rs` reads them the same way through `Run::code == -1`.
/// Neither can be told from the output, so neither may be reported through it.
///
/// Output goes to files rather than pipes: a compiler writing megabytes into a
/// pipe nobody is draining deadlocks, and a deadlock is not the finding.
fn run_watched(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let out_path = dir.join("fuzz.stdout");
    let err_path = dir.join("fuzz.stderr");
    let (out_file, err_file) = (
        std::fs::File::create(&out_path).unwrap(),
        std::fs::File::create(&err_path).unwrap(),
    );
    let mut argv: Vec<&str> = args.to_vec();
    argv.push("--color=never");
    let mut child = Command::new(harness::buri())
        .args(&argv)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("the buri binary runs");
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => panic!("cannot wait on the toolchain: {e}"),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "the toolchain did not stop within {}s, so it was killed",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let text = format!(
        "{}{}",
        std::fs::read_to_string(&out_path).unwrap_or_default(),
        std::fs::read_to_string(&err_path).unwrap_or_default()
    );
    // A process killed by a signal has no code, which is what a stack overflow
    // and an out-of-memory kill look like from outside.
    if status.code().is_none() {
        return Err(format!("killed by a signal rather than exiting:\n{}", indent(&text)));
    }
    Ok(text)
}

/// The same question asked without a process, for the search rather than for
/// the record.
///
/// A subprocess costs tens of milliseconds and an in-process call costs
/// microseconds, and a fuzzer's whole economics is how many inputs it can ask
/// about. So the search runs the pipeline here, catching the panic itself, and
/// every finding is then confirmed and minimised through [`safety_fires`] — the
/// faithful one — before it is written down.
fn safety_in_process(input: &str, cache: &mut buri::parsing::parser::Cache) -> Option<String> {
    caught(|| {
        let mut map = SourceMap::new();
        let id = map.add(String::from("fuzz"), PathBuf::from("fuzz.buri"), input.to_string());
        let parsed = buri::parsing::parser::parse(input, id);
        // The formatter is on the hostile path too: `buri format` reads a file
        // nobody promised was valid, and `source_unchecked` is the printer with
        // its safety net taken off, which is where a panic would be.
        if parsed.errors.is_empty() {
            std::hint::black_box(source_unchecked(input));
        }
        std::hint::black_box(token_shape(input));
        // A build file is the other text the toolchain reads.
        std::hint::black_box(textproto::parse(input, FileId(0)));
        let mut map = SourceMap::new();
        let analysis =
            driver::analyze_snippet_in(None, &mut map, cache, "main", input, Role::Entry);
        for d in &analysis.diagnostics.items {
            std::hint::black_box(map.render(d, false));
        }
    })
}

/// Runs `f`, returning the panic message where it panicked.
///
/// The hook is installed once for the process and defers to the one it
/// replaced for any thread that is not inside this function, so a genuinely
/// failing test in this binary still prints its own message.
fn caught(f: impl FnOnce()) -> Option<String> {
    use std::cell::RefCell;
    use std::sync::OnceLock;
    thread_local! {
        static CAUGHT: RefCell<Option<String>> = const { RefCell::new(None) };
        static INSIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    static HOOK: OnceLock<()> = OnceLock::new();
    HOOK.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mine = INSIDE.with(std::cell::Cell::get);
            if !mine {
                previous(info);
                return;
            }
            CAUGHT.with(|c| *c.borrow_mut() = Some(format!("{info}")));
        }));
    });
    INSIDE.with(|i| i.set(true));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    INSIDE.with(|i| i.set(false));
    match result {
        Ok(()) => None,
        Err(_) => Some(
            CAUGHT
                .with(|c| c.borrow_mut().take())
                .unwrap_or_else(|| String::from("the toolchain panicked")),
        ),
    }
}

// -- roundtrip --------------------------------------------------------------

/// The layout edits the formatter is allowed to make, taken out of the token
/// comparison. Identical to `formatting.rs`'s list, and for its reasons.
fn tokens_modulo_layout(text: &str) -> Vec<Shape> {
    const LAYOUT: &[&str] = &["`,`", "`(`", "`)`", "`{`", "`}`"];
    let mut out: Vec<Shape> = drop_empty_type_arguments(token_shape(text))
        .into_iter()
        .filter(|s| !matches!(s, Shape::Token(t) if LAYOUT.contains(&t.as_str())))
        .collect();
    out.sort();
    out
}

/// Every `<` immediately followed by `>` removed, in source order.
///
/// The fifth allowed edit: `t<>` and `t` are the same type, and the formatter
/// prints the one a reader would write. The pair goes as a pair rather than by
/// filtering both tokens everywhere, so that a generic list the formatter
/// really did lose stays visible.
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

fn comments_sorted(text: &str) -> Vec<Shape> {
    let mut out = comment_shape(text);
    out.sort();
    out
}

/// The `roundtrip` property: `formatting.rs`'s four claims, over any source that
/// parses.
///
/// A file that does not parse is not a finding — there is nothing to format
/// and `buri format` leaves it alone — so the property is vacuously satisfied
/// there. That is what makes it usable over mutated input: most mutants do
/// not parse, and the ones that do are exactly the interesting ones.
fn roundtrip_fires(input: &str) -> Option<String> {
    if !buri::parsing::parser::parse(input, FileId(0)).errors.is_empty() {
        return None;
    }
    let Some(once) = source(input) else {
        // `source` refuses its own output when the output does not parse or
        // lost a comment, which is the formatter catching itself. It is still
        // a formatter bug, and this is the only place it is reported as one.
        let printed = source_unchecked(input);
        let reparsed = buri::parsing::parser::parse(&printed, FileId(0));
        let why = if reparsed.errors.is_empty() {
            format!(
                "it lost a comment.\n  before: {:?}\n  after:  {:?}",
                comments_sorted(input),
                comments_sorted(&printed)
            )
        } else {
            format!(
                "its output does not parse: {}",
                reparsed.errors.first().map(|e| e.message.clone()).unwrap_or_default()
            )
        };
        return Some(format!("`format` refused its own output — {why}\nit printed:\n{}", indent(&printed)));
    };
    let twice = source(&once)?;
    if twice != once {
        return Some(format!(
            "formatting the output again changes it, so the shape is not a fixed point.\n\
             once:\n{}\ntwice:\n{}",
            indent(&once),
            indent(&twice)
        ));
    }
    if comments_sorted(input) != comments_sorted(&once) {
        return Some(format!(
            "the comments are not the same set before and after.\n  before: {:?}\n  after:  {:?}",
            comments_sorted(input),
            comments_sorted(&once)
        ));
    }
    if tokens_modulo_layout(input) != tokens_modulo_layout(&once) {
        return Some(format!(
            "a token was invented or lost.\n  before: {:?}\n  after:  {:?}",
            tokens_modulo_layout(input),
            tokens_modulo_layout(&once)
        ));
    }
    None
}

// -- compiles ---------------------------------------------------------------

/// The `compiles` property: this source type-checks and exports a `main`.
///
/// The benchmark's `--validate`, over one module instead of a corpus. What it
/// is really asking is whether the front end accepts a program it should —
/// the claim the reject corpus cannot make, because everything in that corpus
/// is supposed to be turned away.
fn compiles_fires(input: &str) -> Option<String> {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", input, Role::Entry);
    let errors: Vec<String> = analysis
        .diagnostics
        .items
        .iter()
        .filter(|d| matches!(d.severity, Severity::Error))
        .map(|d| map.render(d, false))
        .collect();
    if !errors.is_empty() {
        return Some(format!("the source does not compile:\n{}", indent(&errors.join(""))));
    }
    if analysis.checked.entry.is_none() {
        return Some(String::from("the source exports no `main`"));
    }
    None
}

// -- deterministic ----------------------------------------------------------

/// The `deterministic` property: the same bytes twice, in one process and in
/// two, produce the same diagnostics.
///
/// Both halves, because they catch different things. Two calls in one process
/// share every hash map's random state, so what differs between them is
/// leftover mutable state; two processes get different `RandomState` seeds, so
/// what differs between them is hash order leaking into output — which this
/// repository has shipped, in the `did you mean` suggestion.
fn deterministic_fires(input: &str) -> Option<String> {
    let first = diagnostics_of(input);
    let second = diagnostics_of(input);
    if first != second {
        return Some(format!(
            "two compilations in one process disagree.\n  first:\n{}\n  second:\n{}",
            indent(&first),
            indent(&second)
        ));
    }
    // Two repositories rather than two runs in one. A second `build` in the
    // same tree is served by the cache and says so — `(N bytes, cached)` —
    // which is the cache working rather than the compiler disagreeing with
    // itself, and it is what this property reported as a finding until it did
    // not. Two trees at two paths also make the comparison the stronger one:
    // neither the cache nor the directory the build ran in may reach the
    // output. Both are normalised, because the scratch path is the one thing
    // that legitimately differs.
    let first = Scratch::repo("fuzz-deterministic-a");
    let second = Scratch::repo("fuzz-deterministic-b");
    let mut printed = Vec::new();
    for s in [&first, &second] {
        s.write("app/BUILD.buri", harness::JS_BINARY);
        s.write("app/main.buri", input);
        printed.push(s.run(&["build", "//app"]).normalised(&s.root));
    }
    let (a, b) = (&printed[0], &printed[1]);
    if a != b {
        return Some(format!(
            "two runs of the binary disagree.\n  first:\n{}\n  second:\n{}",
            indent(a),
            indent(b)
        ));
    }
    None
}

fn diagnostics_of(input: &str) -> String {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", input, Role::Entry);
    analysis.diagnostics.items.iter().map(|d| map.render(d, false)).collect()
}

// -- output -----------------------------------------------------------------

/// The `output` property: every backend runs this program and prints `expects`.
///
/// `native/agreement.rs`'s comparison, with the program handed in rather than
/// written down. Its three claims are kept: JavaScript prints the expected
/// bytes, every native backend prints them, and the two are identical — the
/// third alone would pass on a program that had rotted on both sides at once.
///
/// Where the host cannot answer — no JavaScript engine, no runtime archive, no
/// linker — the property reports nothing rather than a failure, which is the
/// same skip `native/agreement.rs` takes and for the same reason.
fn output_fires(input: &str, expects: &str) -> Option<String> {
    let js = match run_on_js(input) {
        Ok(Some(out)) => out,
        // No engine on this host: the property cannot answer, which is not the
        // same as answering yes.
        Ok(None) => return None,
        // A program the reference backend will not compile or will not run is
        // a finding in its own right: every input this property is handed —
        // generated here or recorded in the corpus — is one that did both.
        Err(why) => return Some(why),
    };
    if js.trim_end() != expects.trim_end() {
        return Some(format!(
            "JavaScript printed something else.\n  expected: {expects:?}\n  printed:  {js:?}"
        ));
    }
    #[cfg(any(feature = "backend-stencil", feature = "backend-llvm"))]
    if let Some(native) = native::run(input) {
        for (name, printed, trouble) in native {
            // How it ended, first: a program that printed the right answer and
            // did not give its blocks back is still a finding, and so is one
            // that crashed on the way — both are things this file used to skip
            // past. The message is the runtime's own line where the heap check
            // wrote one, so it names the number of blocks or the operation
            // that reached a freed block.
            if let Some(why) = trouble {
                return Some(format!("`{name}` did not finish cleanly: {why}"));
            }
            if printed.trim_end() != js.trim_end() {
                return Some(format!(
                    "`{name}` and JavaScript disagree.\n  javascript: {js:?}\n  {name}: {printed:?}"
                ));
            }
        }
    }
    None
}

/// How long the reference artifact gets before "it has not stopped" is the
/// answer.
///
/// The same generosity [`WATCHDOG`] is written with, and needed for the same
/// reason one step further along: **the minimiser writes programs nobody
/// drew**. A search's own draws terminate by construction, but a shrink step
/// deletes a token, and `walkFrom(octets, at + 1, ..)` with the `+ 1` deleted
/// is a loop that does not end. Before this bound that was a suite which did
/// not end either — the engine ran for ever inside a library call with no
/// process for the harness to kill.
const JS_DEADLINE: Duration = Duration::from_secs(20);

/// What the program prints under the JavaScript backend.
///
/// `Ok(None)` is "this host has no engine", which is a skip. `Err` is "the
/// reference backend would not produce an answer", which is a finding: this is
/// the pipeline `actions::prepare` composes for `Platform::Js`, so a program
/// it refuses is a program `buri run` refuses.
///
/// **Emitted and run here rather than through `driver::run_snippet`**, which
/// is what this used to call. `run_snippet` runs the engine with
/// `Command::output()` and no deadline, so an artifact that loops takes the
/// suite with it and there is no child for anything to kill. This is the same
/// three steps `native/agreement.rs::run_js` takes — one analysis,
/// `actions::prepare` for the JavaScript target, the backend `select`
/// answers — with [`JS_DEADLINE`] around the run.
fn run_on_js(input: &str) -> Result<Option<String>, String> {
    if !engine_present() {
        return Ok(None);
    }
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", input, Role::Entry);
    if analysis.diagnostics.has_errors() {
        return Err(format!(
            "the JavaScript backend produced no answer:\n{}",
            indent(&analysis.diagnostics.items.iter().map(|d| map.render(d, false)).collect::<String>())
        ));
    }
    let Some(entry) = analysis.checked.entry else {
        return Err(String::from("the program exports no `main`"));
    };
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = buri::diagnostics::Diagnostics::new();
    let mut program = buri::compiler::middle::monomorphize::run(
        &analysis.checked,
        paths,
        &mut diagnostics,
        buri::compiler::middle::monomorphize::Roots::Main(entry),
    );
    if diagnostics.has_errors() {
        return Err(String::from("monomorphization failed"));
    }
    let target = buri::compiler::backend::Target {
        platform: buri::build::buildfile::Platform::Js,
        arch: None,
    };
    buri::build::actions::prepare(&mut program, target);
    let profile = buri::compiler::backend::Profile::Debug;
    let opts =
        buri::compiler::backend::Options { profile, target, unit_prefix: "" };
    let mut backend = buri::compiler::backend::select(target, profile)
        .map_err(|e| format!("no JavaScript backend: {e}"))?;
    let units = backend
        .emit(&program, &analysis.checked.tables, &opts)
        .map_err(|d| {
            format!(
                "the JavaScript backend refused the program: {}",
                d.items.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("; ")
            )
        })?;
    // A directory of this call's own, because this binary runs its tests in
    // parallel threads of one process and an ES module has to come from a
    // file.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fuzz-js-{}", std::process::id()))
        .join(format!("case-{n}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let artifact = dir.join("main.mjs");
    let bytes = &units.first().ok_or("the backend emitted no unit")?.bytes;
    std::fs::write(&artifact, bytes).map_err(|e| format!("cannot write the artifact: {e}"))?;
    let mut cmd = std::process::Command::new(harness::js_runtime());
    cmd.arg(&artifact);
    let out = waited_out(cmd, JS_DEADLINE)
        .ok_or_else(|| format!("the artifact did not stop within {}s", JS_DEADLINE.as_secs()))?;
    if !out.status.success() {
        return Err(format!(
            "the artifact exited {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).to_string()))
}

/// Run a command to completion, killing it if it has not stopped in time.
///
/// `None` is "it did not stop", or "it could not be started". The pipes are
/// drained once the child has already exited, which is
/// `commands/test.rs::wait_for`'s shape; a child that filled one and blocked
/// is killed, which turns a deadlock into an answer.
fn waited_out(mut cmd: std::process::Command, within: Duration) -> Option<std::process::Output> {
    use std::process::Stdio;
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let deadline = Instant::now() + within;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn engine_present() -> bool {
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    static PRESENT: OnceLock<bool> = OnceLock::new();
    *PRESENT.get_or_init(|| {
        Command::new(harness::js_runtime())
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// The native half of the `output` property.
///
/// Its own module because it is the one part of this file that does not exist
/// under `--no-default-features`, and because the concurrent work on the two
/// backends is confined to it.
#[cfg(any(feature = "backend-stencil", feature = "backend-llvm"))]
mod native {
    use super::{Duration, Instant, Path, PathBuf};
    use buri::build::actions;
    use buri::build::buildfile::{Arch, Platform};
    use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME};
    use buri::compiler::backend::{self, Options, Profile, Target};
    use buri::compiler::driver;
    use buri::compiler::middle::monomorphize;
    use buri::compiler::modules::Role;
    use buri::diagnostics::{Diagnostics, SourceMap};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::OnceLock;

    /// Every native backend this binary was built with, named. Stencil is the
    /// debug-profile backend; LLVM arrives with its feature.
    const NATIVES: &[(&str, Profile)] = &[
        #[cfg(feature = "backend-stencil")]
        ("stencil", Profile::Debug),
        #[cfg(feature = "backend-llvm")]
        ("llvm", Profile::Release),
    ];

    pub fn host_target() -> Target {
        Target {
            platform: if cfg!(target_os = "macos") { Platform::Macos } else { Platform::Linux },
            arch: Some(if cfg!(target_arch = "aarch64") { Arch::Arm64 } else { Arch::X86_64 }),
        }
    }

    /// Why one backend cannot answer on this host, or `None`.
    ///
    /// The backend's own availability query, so the day a host grows the half
    /// it is missing this leg fires with no edit here. Stencil is the one that
    /// answers today: it has an x86-64 library and no entry point to put in
    /// front of it.
    fn unavailable(name: &str) -> Option<String> {
        #[cfg(feature = "backend-stencil")]
        if name == "stencil" {
            return buri::compiler::backend::stencil::unavailable_reason();
        }
        let _ = name;
        None
    }

    /// Why this host cannot compile, link and run a native binary, or `None`.
    ///
    /// `native_ready` answers for the backend `select` returns, which is not
    /// yet stencil, so each backend's own host question is asked beside it: a
    /// host where every native column is unavailable has nothing left to
    /// compare against, and one where a single column is keeps the rest.
    pub fn off_reason() -> Option<String> {
        if !actions::native_ready(host_target(), Profile::Debug) {
            return Some(String::from("`native_ready` is false on this host"));
        }
        let unavailable: Vec<String> = NATIVES
            .iter()
            .filter_map(|(name, _)| unavailable(name).map(|why| format!("`{name}`: {why}")))
            .collect();
        if unavailable.len() == NATIVES.len() {
            return Some(unavailable.join("; "));
        }
        None
    }

    /// [`off_reason`] as a `bool`, for the `output` property's own guard.
    pub fn ready() -> bool {
        off_reason().is_none()
    }

    /// How many times a native backend has actually produced an answer.
    ///
    /// A backend that refuses every program is indistinguishable from one that
    /// agrees with every program, and the difference is the whole comparison. The
    /// searches print this, so a run in which the native leg never fired says
    /// so rather than passing quietly.
    static ANSWERED: AtomicUsize = AtomicUsize::new(0);

    pub fn answered() -> usize {
        ANSWERED.load(Ordering::Relaxed)
    }

    fn archive() -> &'static Path {
        static WRITTEN: OnceLock<PathBuf> = OnceLock::new();
        WRITTEN.get_or_init(|| {
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("fuzz-native-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(ARCHIVE_NAME);
            std::fs::write(&path, ARCHIVE).unwrap();
            path
        })
    }

    /// The product's link line, staged once for this process.
    ///
    /// `build/link.rs::product_link_args` writes whatever the link needs —
    /// today an eleven-file musl sysroot, about 6.6 MB — into the directory it
    /// is handed, and returns arguments that name it *relatively*, so the
    /// driver has to run there. One directory for the whole fuzzer rather than
    /// one per case: every other path a case passes is absolute, and the
    /// searches below link as many programs as their budget buys.
    fn staged() -> &'static (PathBuf, Vec<String>) {
        static STAGED: OnceLock<(PathBuf, Vec<String>)> = OnceLock::new();
        STAGED.get_or_init(|| {
            crate::harness::sweep::once();
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("fuzz-native-{}", std::process::id()))
                .join("link-flags");
            std::fs::create_dir_all(&dir).unwrap();
            let args = buri::build::link::product_link_args(&dir);
            (dir, args)
        })
    }

    /// The backend, through `backend::select` where `select` answers.
    ///
    /// It answers for `(native, Debug)` and refuses `(native, Release)`, which
    /// is the selection LLVM is reached by — `native/agreement.rs` spells the
    /// same fallback for the same reason, gated on the feature that carries the
    /// backend, so that the `Ok` arm takes over the day `select` grows the arm.
    fn select(name: &str, target: Target, profile: Profile) -> Option<Box<dyn backend::Backend>> {
        // `select` does not answer with the copy-and-patch backend yet, so the
        // debug-profile backend is named here rather than selected.
        #[cfg(feature = "backend-stencil")]
        if name == "stencil" {
            return Some(Box::new(backend::stencil::Stencil::default()));
        }
        let _ = name;
        match backend::select(target, profile) {
            Ok(b) => Some(b),
            #[cfg(feature = "backend-llvm")]
            Err(_) if matches!(profile, Profile::Release) => {
                Some(Box::new(backend::llvm::Llvm))
            }
            Err(_) => None,
        }
    }

    fn workspace() -> PathBuf {
        crate::harness::sweep::once();
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("fuzz-native-{}", std::process::id()))
            .join(format!("case-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// How long a generated program gets before "it has not stopped" is the
    /// finding.
    ///
    /// Generous rather than tight, for the reason `WATCHDOG` next door gives:
    /// the machine is running other tests beside this one, and a false hang is
    /// a finding nobody can reproduce. A real hang is not a slow program —
    /// every program these searches emit prints eight lines and returns.
    const NATIVE_DEADLINE: Duration = Duration::from_secs(30);

    /// Run one linked program under the heap check, killing it if it has not
    /// stopped within `within`. `None` is "it did not stop", or "it could not
    /// be started".
    ///
    /// The pipes are drained by `wait_with_output` once the child has already
    /// exited, which is the shape `commands/test.rs::wait_for` uses. A program
    /// that filled a pipe and blocked would not be drained by that — it would
    /// be *killed*, which turns a deadlock into a finding and is the outcome
    /// this function exists for.
    fn watched(binary: &Path, within: Duration) -> Option<std::process::Output> {
        let mut child = Command::new(binary)
            .env("BURI_RT_HEAP_CHECK", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + within;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return child.wait_with_output().ok(),
                Ok(None) => {}
                Err(_) => return None,
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The exit status the runtime's heap check stops a program with.
    /// `cli/runtime/memory.rs`'s `BURI_RT_HEAP_CHECK_STATUS` is the other
    /// side, and `cli/tests/native/shared.rs` says the same number for the
    /// backend suites.
    const HEAP_CHECK_STATUS: i32 = 97;

    /// Why each native backend will not compile this program, or nothing.
    ///
    /// [`run`] treats a refusal as a fact about the *host's* surface rather
    /// than as a disagreement, which is right for a mutated input and wrong
    /// for a program the generator drew on purpose: `ownership.rs`'s shapes
    /// are all inside the native surface, so a backend that stops accepting
    /// one is a regression and not a gap. That is not a hypothetical — a
    /// merged tail-call group whose members' parameters disagree used to be
    /// *accepted* and miscompiled, and the fix made the malformed case a
    /// refusal, so the same defect coming back is a refusal here and nothing
    /// at all over in `run`.
    ///
    /// Emission only: no link and no execution, so asking is cheap enough to
    /// ask of every draw.
    pub fn refusals(input: &str) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if !ready() {
            return out;
        }
        let mut map = SourceMap::new();
        let analysis = driver::analyze_snippet(&mut map, "main", input, Role::Entry);
        if analysis.diagnostics.has_errors() {
            return out;
        }
        let Some(entry) = analysis.checked.entry else { return out };
        let paths: Vec<String> =
            analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
        for (name, profile) in NATIVES {
            if unavailable(name).is_some() {
                continue;
            }
            let mut diagnostics = Diagnostics::new();
            let mut program = monomorphize::run(
                &analysis.checked,
                paths.clone(),
                &mut diagnostics,
                monomorphize::Roots::Main(entry),
            );
            if diagnostics.has_errors() {
                out.push((*name, String::from("monomorphization failed")));
                continue;
            }
            let target = host_target();
            actions::prepare(&mut program, target);
            let opts = Options { profile: *profile, target, unit_prefix: "" };
            let Some(mut backend) = select(name, target, *profile) else { continue };
            let missing = backend.missing_intrinsics(&program, &analysis.checked.tables);
            if !missing.is_empty() {
                out.push((*name, format!("missing {missing:?}")));
                continue;
            }
            if let Err(d) = backend.emit(&program, &analysis.checked.tables, &opts) {
                out.push((
                    *name,
                    d.items.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("; "),
                ));
            }
        }
        out
    }

    /// What every native backend prints for this program, and what the heap
    /// check said about the run.
    ///
    /// `None` where the host cannot answer. A backend that refuses the
    /// program — a missing intrinsic, an unlowered pattern — is left out of
    /// the answer rather than reported: the native surface is admittedly
    /// narrower than the language, `native/conformance.rs` is what holds that
    /// gap open, and a refusal is not a disagreement.
    ///
    /// **Every run is under the heap check** (`BURI_RT_HEAP_CHECK`), so a
    /// program that printed the right bytes and leaked a block, or that
    /// reached a block it had already freed, comes back with the third element
    /// set. That is the whole of what the search needs to see an
    /// over-decrement: without the quarantine the freed block is recycled and
    /// the symptom is a wrong answer on the runs where the recycling happened
    /// to matter.
    pub fn run(input: &str) -> Option<Vec<(&'static str, String, Option<String>)>> {
        if !ready() {
            return None;
        }
        let mut map = SourceMap::new();
        let analysis = driver::analyze_snippet(&mut map, "main", input, Role::Entry);
        if analysis.diagnostics.has_errors() {
            return None;
        }
        let entry = analysis.checked.entry?;
        let paths: Vec<String> =
            analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
        let mut out = Vec::new();
        for (name, profile) in NATIVES {
            let mut diagnostics = Diagnostics::new();
            let mut program = monomorphize::run(
                &analysis.checked,
                paths.clone(),
                &mut diagnostics,
                monomorphize::Roots::Main(entry),
            );
            if diagnostics.has_errors() {
                return None;
            }
            let target = host_target();
            actions::prepare(&mut program, target);
            let opts = Options { profile: *profile, target, unit_prefix: "" };
            // A backend with no seat on this host refuses every program, which
            // is a fact about the host rather than a disagreement.
            if unavailable(name).is_some() {
                continue;
            }
            let Some(mut backend) = select(name, target, *profile) else { continue };
            if !backend.missing_intrinsics(&program, &analysis.checked.tables).is_empty() {
                continue;
            }
            let Ok(units) = backend.emit(&program, &analysis.checked.tables, &opts) else {
                continue;
            };
            let dir = workspace();
            let mut objects = Vec::new();
            for unit in &units {
                let path = dir.join(&unit.name);
                std::fs::write(&path, &unit.bytes).unwrap();
                objects.push(path);
            }
            let binary = dir.join("program");
            // `build/link.rs`'s own driver and trailing arguments. The old
            // hand-written `-lpthread -ldl -lm` was a second idea of the
            // product's link line, and on Linux the product's is now a
            // static-PIE musl link against a sysroot this binary carries —
            // which is not a thing a list of `-l`s can restate.
            let (link_dir, link_args) = staged();
            let driver = buri::build::link::product_link_driver().unwrap_or_else(|| {
                PathBuf::from(std::env::var("CC").unwrap_or_else(|_| String::from("cc")))
            });
            let mut link = Command::new(driver);
            link.current_dir(link_dir);
            link.arg("-o").arg(&binary);
            for object in &objects {
                link.arg(object);
            }
            link.arg(archive());
            link.args(link_args);
            let Ok(linked) = link.output() else { continue };
            if !linked.status.success() {
                continue;
            }
            // **With a watchdog**, because a miscompiled program is entitled
            // to loop for ever and this file used to wait for it. The test
            // that stood here — `started.elapsed()` *after* the call — could
            // not fire until the call returned, so a program that never
            // stopped was a search that never stopped. A reference-counting
            // defect is one of the things that produces one: a list walked
            // through a block whose length word is now poison does not
            // terminate, and that is a finding rather than a reason to wait.
            let Some(ran) = watched(&binary, NATIVE_DEADLINE) else {
                out.push((
                    *name,
                    String::new(),
                    Some(format!(
                        "it did not stop within {}s, and was killed",
                        NATIVE_DEADLINE.as_secs()
                    )),
                ));
                continue;
            };
            let status = ran.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&ran.stderr).to_string();
            // A heap-check failure is reported rather than skipped: it is the
            // only way this file can see an under- or over-decrement, and a
            // `continue` here would be the silence the whole layer exists to
            // remove. Every other nonzero status is a program that aborted for
            // a reason of its own, which the generator's programs do not do
            // and which a mutated input may well.
            let trouble = match status {
                0 => None,
                HEAP_CHECK_STATUS => Some(
                    stderr
                        .lines()
                        .find(|l| l.starts_with("buri heap check:"))
                        .unwrap_or("the heap-check status with nothing said")
                        .to_string(),
                ),
                // **Any other nonzero status is a finding too**, and this is
                // the line that used to be a `continue`. The property has
                // already established that the reference backend ran this
                // program and printed the expected bytes, so a native
                // artifact that aborted, or that died on a signal — status
                // `-1`, which is what a stale pointer through poisoned memory
                // looks like — is not a program with a reason of its own to
                // stop. It is the disagreement.
                other => Some(format!(
                    "the program exited {other} where JavaScript exited 0:\n{}",
                    stderr.trim_end()
                )),
            };
            ANSWERED.fetch_add(1, Ordering::Relaxed);
            out.push((*name, String::from_utf8_lossy(&ran.stdout).to_string(), trouble));
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// The minimiser
// ---------------------------------------------------------------------------

/// The finding, shrunk until nothing more can come out of it.
///
/// Three passes, coarse to fine, each greedy and each re-checking the property:
/// runs of lines, then runs of whitespace-separated tokens, then single
/// characters. That is delta debugging's shape (Zeller's `ddmin`) without its
/// generality — a compiler input is a line-oriented tree, and the three
/// granularities that matter are the ones a person would try.
///
/// Bounded, because a minimiser that runs longer than the search that fed it
/// is a minimiser nobody will leave switched on. `property` is re-checked at every
/// step against the *same* message class rather than merely "still fails", so
/// shrinking cannot silently walk from one bug to another.
fn minimize(property: &str, input: &str, expects: Option<&str>) -> String {
    let Some(first) = fires(property, input, expects) else { return input.to_string() };
    let signature = signature_of(&first);
    let deadline = Instant::now() + MINIMISE_CEILING;
    let still = |candidate: &str| -> bool {
        // The bound the doc comment above claims, made real. Every pass below
        // asks this question and treats `false` as "that step did not help",
        // so a deadline that has passed walks each of them to its end without
        // another compile and `best` is whatever had been reached. The
        // *finding* is never lost — it is `input`, and `report` re-checks what
        // this answers — only some of the shrinking is.
        //
        // It matters because a step of the `output` property is a compile, a
        // link and a run of a whole program on two backends, and the character
        // passes ask for one per character: a five-kilobyte program is tens of
        // thousands of them, which is hours. The searches that feed this used
        // to emit programs small enough for that not to show.
        if Instant::now() >= deadline {
            return false;
        }
        candidate != input
            && fires(property, candidate, expects)
                .is_some_and(|why| signature_of(&why) == signature)
    };

    let mut best = input.to_string();
    // Lines. Halving granularity, largest run first, which is what gets a
    // thousand-line seed down to ten in a handful of passes.
    let mut run = best.lines().count().max(1);
    while run >= 1 {
        let mut at = 0;
        loop {
            let lines: Vec<&str> = best.lines().collect();
            if at >= lines.len() {
                break;
            }
            let end = (at + run).min(lines.len());
            let candidate: String = lines
                .iter()
                .enumerate()
                .filter(|(i, _)| *i < at || *i >= end)
                .map(|(_, l)| format!("{l}\n"))
                .collect();
            if still(&candidate) {
                best = candidate;
            } else {
                at += run;
            }
        }
        if run == 1 {
            break;
        }
        run /= 2;
    }

    // Tokens, over the one line a line pass cannot get inside.
    let mut run = 8usize;
    while run >= 1 {
        let mut at = 0;
        loop {
            let words: Vec<&str> = best.split_whitespace().collect();
            if at >= words.len() {
                break;
            }
            let end = (at + run).min(words.len());
            let candidate: String = words
                .iter()
                .enumerate()
                .filter(|(i, _)| *i < at || *i >= end)
                .map(|(_, w)| *w)
                .collect::<Vec<_>>()
                .join(" ");
            if still(&candidate) {
                best = candidate;
            } else {
                at += run;
            }
        }
        if run == 1 {
            break;
        }
        run /= 2;
    }

    // Runs of one character, shortened whole.
    //
    // Deleting one character at a time cannot get through a thousand of the
    // same one, because every single deletion changes something the finding
    // depends on — the parity of an escape, the depth of a nest — so the
    // signature moves and the step is rejected, forty thousand times in a row.
    // Replacing the whole run keeps its shape, and the candidate lengths cover
    // both parities so a finding that needs an odd count can still shrink.
    // This is the inverse of the mutator's own repeat operator, and it is what
    // takes forty thousand backslashes down to four.
    for _ in 0..64 {
        let chars: Vec<char> = best.chars().collect();
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut at = 0;
        while at < chars.len() {
            let mut end = at;
            while end < chars.len() && chars[end] == chars[at] {
                end += 1;
            }
            if end - at >= 4 {
                runs.push((at, end - at));
            }
            at = end;
        }
        runs.sort_by_key(|(_, len)| std::cmp::Reverse(*len));
        let mut shrank = false;
        for (start, len) in runs {
            for target in [1usize, 2, 3, 4, len / 2, len / 2 + 1] {
                if target >= len {
                    continue;
                }
                let candidate: String = chars[..start]
                    .iter()
                    .chain(std::iter::repeat_n(&chars[start], target))
                    .chain(chars[start + len..].iter())
                    .collect();
                if still(&candidate) {
                    best = candidate;
                    shrank = true;
                    break;
                }
            }
            if shrank {
                break;
            }
        }
        if !shrank {
            break;
        }
    }

    // Characters, which is what takes `aaa` down to `aa` and trims the
    // ends of what is left.
    let mut at = 0;
    while at < best.chars().count() {
        let candidate: String =
            best.chars().enumerate().filter(|(i, _)| *i != at).map(|(_, c)| c).collect();
        if still(&candidate) {
            best = candidate;
        } else {
            at += 1;
        }
    }
    best
}

/// What makes two failures the same failure.
///
/// The headline plus every `error:` line under it, with the digits taken out.
/// Both halves are needed and each alone is wrong: the headline alone says
/// only "the source does not compile", which every broken file satisfies — so
/// a minimiser using it would shrink any finding down to a single `}` and
/// report the wrong bug. The diagnostics alone would let a panic and a clean
/// refusal share a signature. Digits go because a span and a count move as an
/// input shrinks while the bug does not, and a signature that kept them would
/// stop the minimiser at the first character it removed.
///
/// It is also the dedup key: two findings with one signature are one finding,
/// which is what keeps a soak run from recording the same bug two hundred
/// times. The case's directory name is derived from it, so the corpus dedups
/// itself by construction.
fn signature_of(why: &str) -> String {
    let head = why.lines().next().unwrap_or_default();
    let first = why
        .lines()
        .skip(1)
        .map(str::trim)
        .find(|t| t.starts_with("error:") || t.starts_with("panicked at"))
        .unwrap_or_default();
    // The *first* diagnostic rather than all of them. A shrinking input emits
    // fewer diagnostics as it shrinks, so a signature over the whole set would
    // change at almost every step and the minimiser would stop immediately.
    quoted_names_removed(&format!("{head} | {first}"))
}

/// The message with the names in it taken out: every digit, and everything
/// between a pair of backticks.
///
/// A diagnostic names what it is about — ``  `State0_7_xxxxxxxxxx` has no
/// method `show` `` — and those names come from the input rather than from the
/// bug. Two findings differing only in what the generator called a type are
/// one finding, and a minimiser holding a signature that kept the name would
/// stop at the first character it renamed.
fn quoted_names_removed(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut inside = false;
    for c in message.chars() {
        if c == '`' {
            if !inside {
                out.push_str("`_`");
            }
            inside = !inside;
            continue;
        }
        if !inside && !c.is_ascii_digit() {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The regression corpus
// ---------------------------------------------------------------------------

/// One recorded finding.
struct Recorded {
    name: String,
    doc: String,
    property: String,
    /// `true` where the finding is still open, and the property must still fail.
    open: bool,
    expects: Option<String>,
    input: String,
}

fn corpus_dir() -> PathBuf {
    harness::tests_dir().join("fuzz")
}

/// Every case in the corpus, in name order.
///
/// The corpus may legitimately be empty — a repository that has never run a
/// soak has nothing to replay — so there is no floor here, and
/// [`the_corpus_is_wired_up`] is what says the reader works.
fn recorded() -> Vec<Recorded> {
    let dir = corpus_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("CASE.textproto").is_file())
        .collect();
    dirs.sort();
    dirs.iter().map(|d| read_case(d)).collect()
}

fn read_case(dir: &Path) -> Recorded {
    let name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
    let text = std::fs::read_to_string(dir.join("CASE.textproto"))
        .unwrap_or_else(|e| panic!("{name}: cannot read CASE.textproto: {e}"));
    let parsed = textproto::parse(&text, FileId(0));
    assert!(
        parsed.errors.is_empty(),
        "{name}: CASE.textproto does not read: {:?}",
        parsed.errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
    );
    let field = |key: &str| -> Option<String> {
        parsed.document.fields.iter().find(|f| f.name == key).map(|f| match &f.value {
            textproto::Value::Str(s, _) => s.clone(),
            textproto::Value::Ident(s, _) => s.clone(),
            textproto::Value::Int(n, _) => n.to_string(),
            _ => String::new(),
        })
    };
    let status = field("status").unwrap_or_default();
    assert!(
        status == "OPEN" || status == "FIXED",
        "{name}: `status` is {status:?}; it is OPEN (the property must still fail) or \
         FIXED (it must hold)"
    );
    let property = field("property").unwrap_or_default();
    assert!(
        PROPERTIES.contains(&property.as_str()),
        "{name}: `{property}` is not one of {PROPERTIES:?}"
    );
    let file = input_name(&property);
    let input = std::fs::read_to_string(dir.join(file))
        .unwrap_or_else(|e| panic!("{name}: cannot read {file}: {e}"));
    Recorded {
        name,
        doc: field("doc").unwrap_or_default(),
        property,
        open: status == "OPEN",
        expects: field("expects"),
        input,
    }
}

/// Writes a finding into the corpus, minimised, with everything needed to read
/// it a year from now.
///
/// Only under `BURI_FUZZ_RECORD=1`. Every other suite here works on a copy
/// under `CARGO_TARGET_TMPDIR` and nothing writes into a checked-in tree, and a
/// fuzzer that silently grew the corpus on every CI run would be the loudest
/// possible exception to that.
fn record(name: &str, doc: &str, property: &str, input: &str, expects: Option<&str>, open: bool) {
    let dir = corpus_dir().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let mut manifest = format!(
        "doc: {doc:?}\nproperty: {property:?}\nstatus: {}\n",
        if open { "OPEN" } else { "FIXED" }
    );
    if let Some(e) = expects {
        manifest.push_str(&format!("expects: {e:?}\n"));
    }
    std::fs::write(dir.join("CASE.textproto"), manifest).unwrap();
    std::fs::write(dir.join(input_name(property)), input).unwrap();
    eprintln!("fuzz: recorded {}", dir.display());
}

/// A name a directory can carry and a person can read: the property, then what
/// broke.
///
/// Derived from the signature, so a recorded finding lands in a directory a
/// second sighting of the same bug would land in too. The half of the
/// signature that is worth reading is the diagnostic rather than the headline,
/// which the property's own name already says.
///
/// The name is a *label*, not the key. Renaming a case to something a person
/// would choose — which the checked-in cases have all had — costs nothing,
/// because [`known_signatures`] dedups by replaying the case rather than by
/// reading its directory.
fn case_name(property: &str, why: &str) -> String {
    let signature = signature_of(why);
    let readable = signature.split_once(" | ").map_or(signature.as_str(), |(head, first)| {
        if first.trim().is_empty() {
            head
        } else {
            first
        }
    });
    let head: String = readable
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let mut slug = String::new();
    let mut last_underscore = false;
    for c in head.chars() {
        if c == '_' {
            if !last_underscore && !slug.is_empty() {
                slug.push('_');
            }
            last_underscore = true;
        } else {
            slug.push(c);
            last_underscore = false;
        }
    }
    let slug: String = slug.trim_end_matches('_').chars().take(48).collect();
    format!("{property}_{}", slug.trim_end_matches('_'))
}

/// The signature of every finding the corpus already holds open.
///
/// This is what makes a fuzzer able to live in a suite that has to stay green.
/// A known-open finding is reproducible by construction — that is what `OPEN`
/// means — so a search that walks into it again has found nothing, and
/// stopping there would mean the search never gets past the first open bug.
/// `replay_the_regression_corpus` is where an open finding is asserted; here
/// it is only recognised.
fn known_signatures() -> Vec<String> {
    recorded()
        .iter()
        .filter(|c| c.open)
        .filter_map(|c| {
            fires(&c.property, &c.input, c.expects.as_deref()).map(|why| signature_of(&why))
        })
        .collect()
}

/// One finding, triaged.
///
/// Counted where the corpus already holds it — the signature is taken before
/// minimising, because minimising costs seconds and a known finding is worth
/// none of them — and reported, fatally, where it does not.
fn triage(
    seen: &mut Seen,
    property: &str,
    raw: &str,
    why: &str,
    expects: Option<&str>,
    provenance: &str,
) {
    if seen.known.contains(&signature_of(why)) {
        seen.skipped += 1;
        return;
    }
    report(property, raw, why, expects, provenance);
}

/// What a search knows before it starts, and what it met on the way.
struct Seen {
    known: Vec<String>,
    skipped: usize,
}

impl Seen {
    fn new() -> Seen {
        Seen { known: known_signatures(), skipped: 0 }
    }

    fn report(&self, what: &str) {
        if self.skipped > 0 {
            eprintln!(
                "fuzz {what}: {} finding(s) the corpus already holds open, skipped",
                self.skipped
            );
        }
    }
}

/// What a search does with a finding: minimise it, say so, record it where
/// asked, and fail.
fn report(property: &str, raw: &str, why: &str, expects: Option<&str>, provenance: &str) -> ! {
    let small = minimize(property, raw, expects);
    let confirmed = fires(property, &small, expects).unwrap_or_else(|| why.to_string());
    let name = case_name(property, &confirmed);
    if recording() {
        record(&name, provenance, property, &small, expects, true);
    }
    // Both messages, because the minimiser holds the signature and not the
    // wording: the first is what the search saw, the second is what the
    // recorded case will say, and a difference between them is worth reading.
    panic!(
        "fuzz {property}: {provenance}\n\nas found:\n{}\n\nminimised to {} bytes:\n{}\n\n\
         which still says:\n{}\n\n\
         Record it with BURI_FUZZ_RECORD=1, which writes cli/tests/fuzz/{name}/.",
        indent(why),
        small.len(),
        indent(&small),
        indent(&confirmed)
    );
}

// ---------------------------------------------------------------------------
// The searches
// ---------------------------------------------------------------------------

/// Every recorded finding, replayed.
///
/// This is the half of the suite that runs forever: a search finds a bug once,
/// and this is what keeps it found. `OPEN` cases assert the bug is still
/// there, so the corpus is a list of known-open findings that cannot rot into
/// a lie, and `FIXED` cases assert it is gone.
#[test]
fn replay_the_regression_corpus() {
    let cases = recorded();
    let mut failures = Vec::new();
    let mut open = 0;
    for case in &cases {
        let fired = fires(&case.property, &case.input, case.expects.as_deref());
        match (case.open, fired) {
            (false, Some(why)) => failures.push(format!(
                "{}: a FIXED case broke again — {}\n{}",
                case.name, case.doc, indent(&why)
            )),
            (true, None) => failures.push(format!(
                "{}: an OPEN case no longer fires, so it is fixed — {}\n  \
                 set `status: FIXED` in its CASE.textproto, and it is a regression test \
                 from now on.",
                case.name, case.doc
            )),
            (true, Some(_)) => open += 1,
            (false, None) => {}
        }
    }
    assert!(failures.is_empty(), "{} case(s):\n\n{}", failures.len(), failures.join("\n\n"));
    eprintln!("fuzz corpus: {} cases, {open} still open", cases.len());
}

/// Every case is a manifest and an input, and nothing else.
///
/// `failing.rs` makes the same check about its own cases and for the same
/// reason: a stray file is a leftover or somebody reaching for an option the
/// schema does not have.
#[test]
fn the_corpus_is_wired_up() {
    let mut wrong = Vec::new();
    let mut docs: Vec<(String, String)> = Vec::new();
    for case in recorded() {
        let dir = corpus_dir().join(&case.name);
        let mut entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        entries.sort();
        if entries != ["CASE.textproto", input_name(&case.property)] {
            wrong.push(format!(
                "{}: holds {entries:?}, not the manifest and the `{}` a {} case is",
                case.name,
                input_name(&case.property),
                case.property
            ));
        }
        if case.doc.len() < 20 {
            wrong.push(format!("{}: `doc` says almost nothing — {:?}", case.name, case.doc));
        }
        for (other, doc) in &docs {
            if *doc == case.doc {
                wrong.push(format!("{}: has {other}'s `doc` word for word", case.name));
            }
        }
        if case.property == "output" && case.expects.is_none() {
            wrong.push(format!(
                "{}: an `output` case with no `expects` asserts only that the backends \
                 agree, which they do on the wrong answer too",
                case.name
            ));
        }
        docs.push((case.name.clone(), case.doc.clone()));
    }
    assert!(wrong.is_empty(), "{} case(s) malformed:\n  {}", wrong.len(), wrong.join("\n  "));
}

/// **No input panics the toolchain**, over input nobody chose.
///
/// Mutations of the checked-in corpus, run through the pipeline in this
/// process for speed, and any finding confirmed through the binary before it
/// is minimised — because the promise is about the binary and the in-process
/// call is only how the search affords to ask so many times.
#[test]
fn mutation_never_panics_the_toolchain() {
    let seeds = seed_corpus();
    let budget = Budget::new(1_500);
    let mut rng = Rng(base_seed() ^ 0x5AFE_7900_0000_0001);
    let mut cache = buri::parsing::parser::Cache::new();
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let (from, text) = rng.pick(&seeds).clone();
        let mutant = mutate(&text, &seeds, &mut rng);
        if let Some(why) = safety_in_process(&mutant, &mut cache) {
            // In-process is the search; the binary is the promise. A finding
            // the binary does not reproduce is still a finding — the library
            // panicked — so it is reported either way, and which one saw it is
            // part of the report.
            let seen_by_binary = safety_fires(&mutant).is_some();
            triage(
                &mut seen,
                "safety",
                &mutant,
                &why,
                None,
                &format!(
                    "a mutation of {from} panicked the toolchain {}",
                    if seen_by_binary { "through the binary" } else { "in process" }
                ),
            );
        }
        done += 1;
    }
    budget.report("mutation", done, started);
    seen.report("mutation");
}

/// The same promise, asked of the binary, with a watchdog.
///
/// A separate search because it is a different question: the loop above cannot
/// see a stack overflow, an allocation that kills the process, or a compile
/// that never ends, and those are three of the five ways `adversarial.rs`
/// says the toolchain may not stop. A process costs a hundred times what a
/// call does, so this one runs a hundred times less often.
#[test]
fn the_binary_survives_mutated_input() {
    let seeds = seed_corpus();
    let budget = Budget::new(120);
    let mut rng = Rng(base_seed() ^ 0x5AFE_7900_0000_0002);
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let (from, text) = rng.pick(&seeds).clone();
        let mutant = mutate(&text, &seeds, &mut rng);
        if let Some(why) = safety_fires(&mutant) {
            triage(&mut seen, "safety", &mutant, &why, None, &format!("a mutation of {from}"));
        }
        done += 1;
    }
    budget.report("binary", done, started);
    seen.report("binary");
}

/// Structured garbage: a file assembled from the language's own vocabulary and
/// nothing else.
///
/// The other end of the mutation spectrum. A mutant of a valid program is
/// mostly valid and reaches the middle end; a draw from the token table is
/// mostly nonsense and reaches every error path in the parser, which is where
/// the recovery code nobody exercises lives.
#[test]
fn token_soup_never_panics_the_toolchain() {
    let budget = Budget::new(1_500);
    let mut rng = Rng(base_seed() ^ 0x5AFE_7900_0000_0003);
    let mut cache = buri::parsing::parser::Cache::new();
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let mut soup = String::new();
        for _ in 0..1 + rng.below(120) {
            soup.push_str(rng.pick::<&str>(VOCABULARY));
            if rng.chance(3) {
                soup.push(' ');
            }
            if rng.chance(9) {
                soup.push('\n');
            }
        }
        if let Some(why) = safety_in_process(&soup, &mut cache) {
            triage(&mut seen, "safety", &soup, &why, None, "a file drawn from the token table");
        }
        done += 1;
    }
    budget.report("token soup", done, started);
    seen.report("token soup");
}

/// **The formatter's four claims**, over generated programs and over every
/// mutant that still parses.
///
/// `formatting/` holds sixty cases, one per decision the formatter makes.
/// This asks the same four questions of shapes nobody chose, which is the only
/// way to find the shape nobody thought of.
#[test]
fn formatting_round_trips_on_generated_and_mutated_source() {
    let seeds = seed_corpus();
    let budget = Budget::new(3_000);
    let mut rng = Rng(base_seed() ^ 0xF0F1_A700_0000_0001);
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    let mut parsed = 0usize;
    while !budget.spent(done) {
        // Half generated, half mutated. A generated module is large and
        // regular and exercises layout at the margin; a mutant is small and
        // strange and exercises the shapes layout has no case for.
        let (from, text) = if rng.chance(2) {
            let params = random_params(&mut rng, 120);
            let program = generate::program(&params);
            let module = rng.pick(&program.modules);
            (format!("a generated module ({})", params.delta()), module.text.clone())
        } else {
            let (from, text) = rng.pick(&seeds).clone();
            (format!("a mutation of {from}"), mutate(&text, &seeds, &mut rng))
        };
        // Most mutants do not parse, and a file that does not parse is not
        // formatted at all — so the count that matters is how many reached the
        // formatter, not how many were drawn.
        if buri::parsing::parser::parse(&text, FileId(0)).errors.is_empty() {
            parsed += 1;
            if let Some(why) = roundtrip_fires(&text) {
                triage(&mut seen, "roundtrip", &text, &why, None, &from);
            }
        }
        done += 1;
    }
    budget.report("roundtrip", done, started);
    seen.report("roundtrip");
    eprintln!("fuzz roundtrip: {parsed} of {done} inputs reached the formatter");
}

/// **Every program the generator emits compiles.**
///
/// `generate.rs` says so in its own second paragraph — "it is not a fuzzer;
/// every program it emits is meant to compile, and one that does not is a bug
/// in this file" — and the benchmark's `--validate` checks it for the twenty
/// named profiles. A profile is three fields moved out of twenty-seven, so
/// twenty profiles are twenty points in a space this walks at random.
///
/// A failure here is a bug in the generator *or* in the front end, and the two
/// are told apart by reading the diagnostic. Either way it is a finding: a
/// benchmark that measures the error paths measures nothing.
#[test]
fn every_generated_program_compiles() {
    let budget = Budget::new(40);
    let mut rng = Rng(base_seed() ^ 0xC0FF_EE00_0000_0001);
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let params = random_params(&mut rng, 400);
        let point = point_of(&params);
        if let Some(why) = generated_fires(&point) {
            triage(
                &mut seen,
                "generated",
                &point,
                &why,
                None,
                "the generator emitted a program that does not compile",
            );
        }
        done += 1;
    }
    budget.report("validity", done, started);
    seen.report("validity");
}

/// A parameter point as the input file records it: `key=value`, one per line.
///
/// `Params::delta` already prints exactly the fields that are not at their
/// default, space-separated, which is the definition of a profile. One per
/// line rather than one line, so that the minimiser's line pass *is* parameter
/// minimisation: deleting a line resets a dimension to its default, and what
/// survives is the smallest set of dimensions that still reproduces.
fn point_of(params: &generate::Params) -> String {
    let mut out: Vec<String> =
        params.delta().split_whitespace().map(str::to_string).collect();
    if params.shape != generate::Shape::Mixed {
        out.push(format!("shape={}", shape_name(params.shape)));
    }
    out.push(String::new());
    out.join("\n")
}

fn shape_name(shape: generate::Shape) -> &'static str {
    match shape {
        generate::Shape::Mixed => "mixed",
        generate::Shape::DeepNesting => "deep-nesting",
        generate::Shape::WideMatch => "wide-match",
        generate::Shape::ManySmallFunctions => "many-small-fns",
        generate::Shape::FewLargeFunctions => "few-large-fns",
    }
}

/// The `generated` property: the benchmark generator emits a program that
/// compiles at this point in its parameter space.
///
/// `generate.rs` states the claim in its own opening — "it is not a fuzzer;
/// every program it emits is meant to compile, and one that does not is a bug
/// in this file" — and the benchmark's `--validate` checks it at the twenty
/// named profiles. A profile is two or three fields moved out of twenty-seven,
/// so twenty profiles are twenty points in a space this walks at random.
///
/// A finding is a bug in the generator or in the front end, and the
/// diagnostic says which. Either way the benchmark is affected: a corpus that
/// does not compile measures the error paths.
fn generated_fires(point: &str) -> Option<String> {
    let mut params = generate::Params::default();
    for line in point.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Some(format!("`{line}` is not a `key=value` parameter"));
        };
        if key == "shape" {
            let Some((_, named)) = generate::profile(value) else {
                return Some(format!("`{value}` is not a shape"));
            };
            params.shape = named.shape;
            continue;
        }
        if let Err(e) = params.set(key, value) {
            return Some(format!("`{line}` is not a parameter point: {e}"));
        }
    }
    program_compiles(&generate::program(&params))
}

/// A point in the generator's parameter space, drawn rather than named.
///
/// Every dimension is moved, within bounds a person would recognise: the aim
/// is the space between the named profiles rather than the space outside what
/// anybody would ask for. `lines` is bounded by the caller because the search
/// that compiles a program can afford a hundred times fewer lines than the one
/// that formats a module of it.
fn random_params(rng: &mut Rng, lines: usize) -> generate::Params {
    let mut p = generate::Params {
        lines: 40 + rng.below(lines),
        lines_per_module: *rng.pick(&[20usize, 60, 250, 1_000]),
        clusters: 1 + rng.below(6),
        cross_cluster: rng.span(0, 8),
        dep_span_pct: rng.span(1, 100),
        w_struct: rng.span(0, 6),
        w_enum: rng.span(0, 6),
        w_generic_fn: rng.span(0, 6),
        w_arith_fn: rng.span(0, 6),
        w_match_fn: rng.span(0, 6),
        w_string_fn: rng.span(0, 6),
        w_list_fn: rng.span(0, 6),
        methods_per_struct: rng.span(0, 8),
        derives: rng.span(0, 6),
        generic_args: rng.span(0, 9),
        nesting: rng.span(1, 6),
        doc_comment_pct: *rng.pick(&[0u32, 100]),
        comment_block_lines: rng.span(0, 4),
        blank_pct: rng.span(0, 40),
        ident_len: rng.span(0, 24),
        reach: !rng.chance(4),
        seed: rng.next(),
        ..Default::default()
    };
    let lo = rng.span(1, 6);
    p.fields_per_struct = (lo, lo + rng.span(0, 8));
    let lo = rng.span(1, 8);
    p.variants_per_enum = (lo, lo + rng.span(0, 12));
    let lo = rng.span(1, 8);
    p.body_lets = (lo, lo + rng.span(0, 12));
    if rng.chance(2) {
        let lo = rng.span(1, 8);
        p.match_arms = (lo, lo + rng.span(0, 16));
    }
    // Every weight at zero leaves nothing to emit, which `Params::set` refuses
    // on the command line and a draw has to refuse here.
    if p.w_struct
        + p.w_enum
        + p.w_generic_fn
        + p.w_arith_fn
        + p.w_match_fn
        + p.w_string_fn
        + p.w_list_fn
        == 0
    {
        p.w_arith_fn = 1;
    }
    // The stress shapes ignore most of the above and are already a named
    // profile each, so a draw reaches one occasionally rather than a third of
    // the time.
    if rng.chance(8) {
        p.shape = *rng.pick(&[
            generate::Shape::DeepNesting,
            generate::Shape::WideMatch,
            generate::Shape::ManySmallFunctions,
            generate::Shape::FewLargeFunctions,
        ]);
    }
    p
}

/// The whole multi-module program through the front end, the way the benchmark
/// validates its corpus.
fn program_compiles(program: &generate::Program) -> Option<String> {
    use buri::compiler::modules::Loader;
    use buri::compiler::semantics::resolve::Checker;
    use buri::diagnostics::Diagnostics;
    let mut map = SourceMap::new();
    let mut cache = buri::parsing::parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(None, &mut map, &mut diagnostics, &mut cache);
        loader.load_builtin_modules();
        let last = program.modules.len().saturating_sub(1);
        for (i, m) in program.modules.iter().enumerate() {
            let role = if i == last { Role::Entry } else { Role::Source };
            loader.load_source(&m.path, role, m.text.clone());
        }
        loader.finish()
    };
    let checked = Checker::new(&loaded, None, &mut diagnostics).run();
    let errors: Vec<String> = diagnostics
        .items
        .iter()
        .filter(|d| matches!(d.severity, Severity::Error))
        .map(|d| format!("error: {}: {}", map.name(d.span.file), d.message))
        .collect();
    if !errors.is_empty() {
        return Some(format!("the program does not compile:\n{}", indent(&errors.join("\n"))));
    }
    if checked.entry.is_none() {
        return Some(String::from("the program exports no `main`"));
    }
    None
}

/// **The same input twice says the same thing.**
///
/// Over mutants rather than over valid programs, because a diagnostic is what
/// this is about and a valid program has none. Both halves of
/// [`deterministic_fires`] apply: one process, then two.
#[test]
fn diagnostics_are_the_same_on_every_run() {
    let seeds = seed_corpus();
    let budget = Budget::new(200);
    let mut rng = Rng(base_seed() ^ 0xDE7E_8000_0000_0001);
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let (from, text) = rng.pick(&seeds).clone();
        let mutant = mutate(&text, &seeds, &mut rng);
        // The in-process half only, at this rate: the two-process half costs
        // two builds and is asked of a hundredth as many inputs below.
        let first = diagnostics_of(&mutant);
        let second = diagnostics_of(&mutant);
        if first != second {
            triage(
                &mut seen,
                "deterministic",
                &mutant,
                &format!(
                    "two compilations in one process disagree.\n  first:\n{}\n  second:\n{}",
                    indent(&first),
                    indent(&second)
                ),
                None,
                &format!("a mutation of {from}"),
            );
        }
        done += 1;
    }
    budget.report("determinism", done, started);
    seen.report("determinism");
}

/// The same, through two processes, which is the half that sees hash order.
///
/// A `RandomState` is seeded per process, so a map iterated into a diagnostic
/// prints a different order in each — and two calls in one process would agree
/// with each other every time. This is the shape of the bug the reject corpus
/// found in `did you mean`, and it is only visible from outside.
#[test]
fn two_processes_print_the_same_diagnostics() {
    let seeds = seed_corpus();
    let budget = Budget::new(12);
    let mut rng = Rng(base_seed() ^ 0xDE7E_8000_0000_0002);
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let (from, text) = rng.pick(&seeds).clone();
        let mutant = mutate(&text, &seeds, &mut rng);
        if let Some(why) = deterministic_fires(&mutant) {
            triage(&mut seen, "deterministic", &mutant, &why, None, &format!("a mutation of {from}"));
        }
        done += 1;
    }
    budget.report("determinism (two processes)", done, started);
    seen.report("determinism (two processes)");
}

// ---------------------------------------------------------------------------
// Programs whose answer is knowable
// ---------------------------------------------------------------------------

/// A generated program that prints, built so that its answer cannot depend on
/// anything a backend is allowed to disagree about.
///
/// This is the Csmith constraint, in this language's terms. Csmith's whole
/// engineering achievement is emitting C with no undefined and no unspecified
/// behaviour, because a differential test over a program whose answer is not
/// pinned by the language proves nothing. Buri's list of things two backends
/// may legally answer differently is written down — `design/native/VALUE-MODEL.md`
/// §12 — so the generator is built to reach none of it:
///
///   * **No overflow.** SPEC 6.2 makes integer overflow undefined. Every value
///     here is kept in `0..1000` by construction: the leaves are literals under
///     a hundred, the only operations are `+`, `*` and `%`, subtraction is
///     spelled `(a + 1000 - b) % 1000`, and a product of two values under a
///     thousand is under a million. Nothing can wrap, so there is no undefined
///     behaviour to be exploited differently on two backends.
///   * **No division and no shift.** Division by zero and a shift at or past
///     the width both abort (`cli/tests/crash/`), and an abort is not an
///     answer. `%` appears only with a literal divisor.
///   * **No floats.** Float *printing* is where the two backends' agreement is
///     hardest, and it is the one thing here already proved exhaustively:
///     `native/float_parity.rs` sweeps 3.8 million doubles through both
///     renderers. Generating floats would re-ask a settled question and would
///     report the §12 rows about precision as findings.
///   * **No `Checked`, `Saturating` or `wrapping*`.** §12 rows 2 and 3 are
///     documented divergences above 2^53 and above 32 bits. A generator that
///     could reach them would report the documentation as a bug.
///
/// What is left — bounded integer arithmetic, comparisons, `if`, `let` chains,
/// calls, a struct, an enum and a `match` — is where a miscompile would
/// actually be, and every backend must agree on all of it.
struct Printer {
    text: String,
    /// What it prints, computed here rather than by running it, so a
    /// disagreement between the two backends and a disagreement between both
    /// backends and the language are different findings.
    expected: String,
}

const MODULUS: i64 = 1_000;

/// One bounded expression over `x`, and its value.
///
/// The value is carried alongside the text because the alternative is trusting
/// one of the backends to say what the answer is, and then `output` would only
/// be asserting that the two agree — which they do on a wrong answer too.
fn bounded_expr(rng: &mut Rng, depth: u32, x: i64) -> (String, i64) {
    if depth == 0 || rng.chance(4) {
        return if rng.chance(2) {
            (String::from("x"), x)
        } else {
            let n = rng.below(100) as i64;
            (n.to_string(), n)
        };
    }
    let (a, av) = bounded_expr(rng, depth - 1, x);
    let (b, bv) = bounded_expr(rng, depth - 1, x);
    match rng.below(5) {
        0 => (format!("(({a} + {b}) % {MODULUS})"), (av + bv) % MODULUS),
        1 => (
            format!("(({a} + {MODULUS} - {b}) % {MODULUS})"),
            (av + MODULUS - bv) % MODULUS,
        ),
        2 => (format!("(({a} * {b}) % {MODULUS})"), (av * bv) % MODULUS),
        3 => {
            let (c, cv) = bounded_expr(rng, depth - 1, x);
            (
                format!("(if ({a} < {b}) {{ {a} }} else {{ {c} }})"),
                if av < bv { av } else { cv },
            )
        }
        _ => (
            format!("(if ({a} == {b}) {{ {a} }} else {{ {b} }})"),
            if av == bv { av } else { bv },
        ),
    }
}

/// A whole program: a handful of pure functions over the bounded expression
/// language, a struct, an enum with a `match`, and a `main` that prints each
/// of them.
fn printer(rng: &mut Rng) -> Printer {
    let funcs = 1 + rng.below(5);
    let mut text = String::from(
        "from \"core/effect\" import { Alloc, Stdout };\nfrom \"core/host\" import * as host;\n\
         from \"core/io\" import * as io;\n\n",
    );
    let mut lines: Vec<String> = Vec::new();
    let mut expected: Vec<String> = Vec::new();

    // An enum and the `match` over it, which is the shape a decision tree is
    // built from and the one place a backend has a choice to get wrong.
    let variants = 2 + rng.below(4);
    text.push_str("enum Tag {");
    for v in 0..variants {
        text.push_str(&format!(" V{v}(Int),"));
    }
    text.push_str(" }\n\n");
    text.push_str("fn weigh(t: Tag): Int {\n  match (t) {\n");
    let mut weights: Vec<i64> = Vec::new();
    for v in 0..variants {
        let k = rng.below(100) as i64;
        weights.push(k);
        text.push_str(&format!("    .V{v}(n) => ((n + {k}) % {MODULUS}),\n"));
    }
    text.push_str("  }\n}\n\n");

    // A struct, so field layout and construction are on the path too.
    let fields = 1 + rng.below(4);
    text.push_str("struct Rec {");
    for f in 0..fields {
        text.push_str(&format!(" f{f}: Int,"));
    }
    text.push_str(" }\n\n");
    let values: Vec<i64> = (0..fields).map(|_| rng.below(100) as i64).collect();
    let sum: i64 = values.iter().sum::<i64>() % MODULUS;
    let inits: Vec<String> =
        values.iter().enumerate().map(|(f, v)| format!("f{f}: {v}")).collect();
    let reads: Vec<String> = (0..fields).map(|f| format!("r.f{f}")).collect();
    text.push_str(&format!(
        "fn build(): Int {{\n  let r = Rec {{ {} }};\n  ({}) % {MODULUS}\n}}\n\n",
        inits.join(", "),
        reads.join(" + ")
    ));

    for i in 0..funcs {
        let arg = rng.below(100) as i64;
        // A chain of `let`s ending in the value returned, which is the shape
        // most lines in most programs have.
        let bindings = 1 + rng.below(4);
        let mut body = String::new();
        let mut last = arg;
        for b in 0..bindings {
            let depth = 2 + rng.below(2) as u32;
            let (e, v) = bounded_expr(rng, depth, last);
            body.push_str(&format!("  let v{b} = {e};\n"));
            last = v;
            // Every binding after the first reads the one before it, so the
            // chain cannot be dead-code-eliminated into nothing.
            body.push_str(&format!("  let x = v{b};\n"));
        }
        body.push_str("  x\n");
        text.push_str(&format!("fn f{i}(x: Int): Int {{\n{body}}}\n\n"));
        lines.push(format!("    let _ = io.println(ctx, \"${{f{i}({arg})}}\").ignore();\n"));
        expected.push(last.to_string());
    }

    let tag = rng.below(variants);
    let payload = rng.below(100) as i64;
    lines.push(format!("    let _ = io.println(ctx, \"${{weigh(Tag.V{tag}({payload}))}}\").ignore();\n"));
    expected.push(((payload + weights[tag]) % MODULUS).to_string());
    lines.push(String::from("    let _ = io.println(ctx, \"${build()}\").ignore();\n"));
    expected.push(sum.to_string());

    text.push_str(
        "export fn main(): Result<(), Str> {\n  \
         let ctx = context { Alloc: host.alloc, Stdout: host.stdout };\n",
    );
    for l in &lines {
        text.push_str(l);
    }
    text.push_str("  .Ok(())\n}\n");
    Printer { text, expected: format!("{}\n", expected.join("\n")) }
}

/// **The backends agree, and they agree on the right answer.**
///
/// `native/agreement.rs` asks this of fourteen written-down rows. This asks it
/// of programs nobody wrote, which is the only way to reach a shape the table
/// does not have a row for. Its three claims are the same three: JavaScript
/// prints what the generator computed, every native backend prints it, and the
/// two are identical.
///
/// Bounded low in CI because the native leg links and executes a fresh binary
/// per program, and macOS charges about 200 ms for that however small it is.
#[test]
fn generated_programs_print_the_same_answer_on_every_backend() {
    if !engine_present() {
        eprintln!("fuzz differential: skipped (no JavaScript engine on PATH)");
        return;
    }
    let budget = Budget::new(6);
    let mut rng = Rng(base_seed() ^ 0xD1FF_0000_0000_0001);
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let p = printer(&mut rng);
        if let Some(why) = output_fires(&p.text, &p.expected) {
            triage(
                &mut seen,
                "output",
                &p.text,
                &why,
                Some(&p.expected),
                "a generated program the backends do not agree on",
            );
        }
        done += 1;
    }
    budget.report("differential", done, started);
    seen.report("differential");
    report_native_participation(done);
}

/// What the native half of the `output` property actually did.
///
/// Printed rather than asserted, because a host with no backend, no runtime
/// archive or no linker is a legitimate place to run this suite, and
/// `native/agreement.rs` skips there for the same reason. What it must not do
/// is skip *silently*: a native leg that refused every program looks exactly
/// like one that agreed with every program from the outside.
fn report_native_participation(done: usize) {
    #[cfg(any(feature = "backend-stencil", feature = "backend-llvm"))]
    {
        if let Some(why) = native::off_reason() {
            eprintln!("fuzz differential: the native leg is off ({why})");
        } else {
            // A count for the process rather than for this search: the EMI
            // search runs the same property, and one counter says whether the
            // native leg fired at all, which is the question.
            eprintln!(
                "fuzz differential: {done} program(s) here; the native leg has answered {} \
                 time(s) in this process",
                native::answered()
            );
        }
    }
    #[cfg(not(any(feature = "backend-stencil", feature = "backend-llvm")))]
    {
        let _ = done;
        eprintln!("fuzz differential: built with no native backend, so JavaScript is the whole of it");
    }
}


// ---------------------------------------------------------------------------
// The ownership generator
// ---------------------------------------------------------------------------
//
// **The shapes `middle::rc` has actually got wrong**, drawn rather than
// written down.
//
// `printer` above generates arithmetic, enums and structs of scalars — a
// program whose every value fits in a register. Nothing in it can leak,
// because nothing in it is on the heap, so the property it feeds catches a
// wrong *number* and not a wrong *count*. The defect family this generator is
// for is the other one: a value that holds heap contents released while
// something is still reading words out of it. Five reports, and between them
// they name six shapes:
//
//  1. **A field projected off a compound tail.** `middle::inline` replaces a
//     call with the callee's *body*, so `identity(outer()).inner` is a
//     projection off a block whose tail is the block's own binding. Depth is a
//     dimension here because the report bisected on it.
//  2. **A `match` arm that reads a sibling field of the scrutinee's base.**
//     The arm's payload bindings are words copied out of the base's block with
//     no count of their own, and the sibling read is the base's last use.
//  3. **Mutual tail recursion whose members' parameters do not agree position
//     by position**, which is what put two types in one merged slot.
//  4. **An option whose payload holds a list**, read out through `withDefault`
//     and through a `match` — a count belonging to something the option does
//     not name.
//  5. **`sortBy` over an element whose type holds an enum**, which is the
//     value whose reference walk goes out of line.
//  6. **A closure over a list built outside it**, indexed back through an
//     `Option` — loop state captured by something that outlives the iteration.
//
// Every generated program prints an answer this file computed in Rust, so the
// three claims of the `output` property are all available: JavaScript prints
// what was computed, every native backend prints it, and the two are
// identical. And every native run is under the runtime's heap check, so a
// program that printed the right answer and leaked, or that touched a block it
// had already freed, is a finding too.
//
// **What is drawn and what is fixed.** The type declarations and the shape
// bodies are fixed, because a generator that also invented types would spend
// its budget on programs the front end refuses; what is drawn is which shapes
// `main` reaches, in what order, at what sizes, through which spelling — the
// let-bound and the inline reading of a projection are two spellings of one
// shape and the report's own bisection is that the `let` is what decides.

/// The fixed half of a generated ownership program: the types and the shape
/// bodies, which are the same in every draw.
///
/// One string rather than a builder because none of it varies: what varies is
/// the calls `main` makes, and a generator that rewrote the declarations would
/// be generating a different language rather than a different program.
const RC_PRELUDE: &str = r#"from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

struct Body { text: Str }
enum Held { Text(Body), Number(Int) }
struct Cell { held: Held }
struct Row { name: Str, cell: Cell, rank: Int }
struct Items { items: [Row] }
struct D1 { inner: Items }
struct D2 { inner: D1 }
struct D3 { inner: D2 }

struct Msg { body: Str }
enum Outcome {
  Attached { id: Str, status: Int, reply: Msg, flush: [Msg] },
  Refused { reply: Msg },
}
struct Manager { held: Int }
struct Step { manager: Manager, outcome: Outcome }

struct Wrapper { octets: [U8] }

enum Fault { Incomplete, Corrupt }
struct Walk { seen: [Int], total: Int }

fn identity<T>(value: T): T { value }

fn rows(count: Int): [Row] {
  list.range(alloc, 0, count).map(alloc, fn(n) => Row {
    name: "r".repeat(alloc, n + 1),
    cell: Cell { held: Held.Text(Body { text: "v".repeat(alloc, n + 1) }) },
    rank: count - n,
  })
}

fn deep(count: Int): D3 {
  D3 { inner: D2 { inner: D1 { inner: Items { items: rows(count) } } } }
}

// 1. A projection off the body `inline` pasted in, read three ways: one step
// and then a walk down locals, one step into a `let` and a walk from there,
// and one step into a call.
//
// **Each of these takes exactly one field off the compound base**, and that is
// deliberate. The *chained* spelling — `identity(deep(n)).inner.inner` — leaks
// its intermediate, which is pinned as its own row next door
// (`native/agreement.rs`'s `a_projection_off_a_generic_calls_result_agrees`,
// which asserts the exact number of blocks so a fix fails it). Drawing it here
// would make every draw report that one finding and never reach a second,
// which is the argument `known_signatures` makes for the recorded corpus.
fn projectedInline(count: Int): Int {
  let d2 = identity(deep(count)).inner;
  let d1 = d2.inner;
  let items = d1.inner;
  items.items.len()
}
fn projectedBound(count: Int): Int {
  let held = identity(deep(count)).inner;
  countOf(held.inner.inner)
}
fn projectedTwice(count: Int): Int {
  let outer = identity(deep(count)).inner;
  let inner = outer.inner;
  countOf(inner.inner)
}
fn countOf(items: Items): Int { items.items.len() }

fn step(k: Int, held: Int): Step {
  Step {
    manager: Manager { held: held },
    outcome: .Attached {
      id: "se".repeat(alloc, k),
      status: 0,
      reply: Msg { body: "re".repeat(alloc, k) },
      flush: [Msg { body: "fl".repeat(alloc, k) }],
    },
  }
}

fn sent(manager: Manager, messages: [Msg]): Str {
  let bodies = messages.map(alloc, fn(message) => message.body).join(alloc, ",");
  str.format(alloc, "${bodies}/${manager.held}")
}

// 2. The scrutinee's base is read inside the arm, which is its last use.
fn insideArm(k: Int, held: Int): Str {
  let s = step(k, held);
  match (s.outcome) {
    .Attached { id, reply, flush, .. } => {
      sent(s.manager, [Msg { body: id }].concat(alloc, [reply].concat(alloc, flush)))
    },
    .Refused { reply } => sent(s.manager, [reply]),
  }
}

// The same, with the base read before the match, which is the workaround the
// report found and which has to go on answering the same thing.
fn beforeMatch(k: Int, held: Int): Str {
  let s = step(k, held);
  let manager = s.manager;
  match (s.outcome) {
    .Attached { id, reply, flush, .. } => {
      sent(manager, [Msg { body: id }].concat(alloc, [reply].concat(alloc, flush)))
    },
    .Refused { reply } => sent(manager, [reply]),
  }
}

// 3. Two members of one tail-recursive group whose parameter lists differ.
fn walkFrom(octets: [U8], at: Int, state: Walk): Result<Walk, Fault> {
  match (octets[at]) {
    .None => .Ok(state),
    .Some(octet) => walkOne(octets, at, octet, state),
  }
}

fn walkOne(octets: [U8], at: Int, octet: U8, state: Walk): Result<Walk, Fault> {
  if (octet == 255) {
    .Err(.Corrupt)
  } else {
    walkFrom(octets, at + 1, Walk {
      seen: state.seen.push(alloc, octet.toI64()),
      total: state.total + octet.toI64(),
    })
  }
}

fn walk(octets: [U8]): Result<Int, Fault> {
  let walked = walkFrom(octets, 0, Walk { seen: list.empty<Int>(), total: 0 })?;
  .Ok(walked.total + walked.seen.len())
}

fn shownWalk(answer: Result<Int, Fault>): Str {
  match (answer) {
    .Ok(n) => str.format(alloc, "${n}"),
    .Err(.Corrupt) => "corrupt",
    .Err(.Incomplete) => "incomplete",
  }
}

// 4. An option whose payload holds a list, read out both ways. The list is a
// literal from the call site rather than a conversion, because `Int.toU8` is
// the inexact conversion that answers a `Result` (SPEC 6.2.1) and this shape
// is about the option and not about the conversion.
fn defaulted(held: Option<[U8]>): Int { held.withDefault(list.empty<U8>()).len() }
fn matched(held: Option<[U8]>): Int {
  match (held) { .None => 0, .Some(raw) => raw.len() }
}
fn wrapped(held: Option<Wrapper>): Int {
  held.withDefault(Wrapper { octets: list.empty<U8>() }).octets.len()
}
fn wrappedMatch(held: Option<Wrapper>): Int {
  match (held) { .None => 0, .Some(w) => w.octets.len() }
}

// 5. `sortBy` over an element whose type holds an enum two levels down.
fn label(row: Row): Str {
  let inner = match (row.cell.held) {
    .Text(body) => body.text,
    .Number(n) => str.format(alloc, "${n}"),
  };
  str.format(alloc, "${row.name}=${inner}:${row.rank}")
}

fn shownRows(xs: [Row]): Str { xs.map(alloc, fn(row) => label(row)).join(alloc, " ") }
fn byRank(xs: [Row]): [Row] { xs.sortBy(alloc, fn(a, b) => a.rank.compare(b.rank)) }
fn sorted(count: Int): Str { shownRows(byRank(rows(count))) }

// 6. A closure over a list built outside it, indexed back through an option.
fn gathered(count: Int): Str {
  let base = rows(count);
  let names = base.map(alloc, fn(row) => row.name);
  list.range(alloc, 0, count)
    .map(alloc, fn(i) => names[i].withDefault("-"))
    .join(alloc, "+")
}
"#;

/// How many `test`-sized calls one generated program makes.
///
/// Several rather than one, because a leak is a *whole-program* fact and a
/// program that makes one call gives the audit one chance to see one.
const RC_LINES: usize = 8;

/// A generated ownership program, and what it must print.
fn rc_shapes(rng: &mut Rng) -> Printer {
    let mut lines: Vec<String> = Vec::new();
    let mut expected: Vec<String> = Vec::new();
    for _ in 0..RC_LINES {
        let (call, answer) = rc_line(rng);
        lines.push(format!("  let _ = io.println(stdout, {call}).ignore();\n"));
        expected.push(answer);
    }
    let mut text = String::from(RC_PRELUDE);
    text.push_str("\nexport fn main(): Result<(), Str> {\n");
    for l in &lines {
        text.push_str(l);
    }
    text.push_str("  .Ok(())\n}\n");
    Printer { text, expected: format!("{}\n", expected.join("\n")) }
}

/// One call `main` makes, and the line it prints.
///
/// Each arm is one of the six shapes, at a size the draw chose, with the
/// answer computed here — so a native backend that got the count wrong is
/// caught by the *value* as well as by the heap check, and the two say
/// different things about the same bug.
fn rc_line(rng: &mut Rng) -> (String, String) {
    // Small on purpose. What these shapes are about is the *structure* of the
    // ownership, and a list of forty rows exercises the same structure as a
    // list of four while costing the JavaScript engine forty string builds.
    let n = 1 + rng.below(5) as i64;
    match rng.below(9) {
        0 => (
            format!("\"${{projectedInline({n})}}\""),
            n.to_string(),
        ),
        1 => (format!("\"${{projectedBound({n})}}\""), n.to_string()),
        2 => (format!("\"${{projectedTwice({n})}}\""), n.to_string()),
        3 => {
            let held = rng.below(100) as i64;
            let k = 1 + rng.below(3) as i64;
            let (id, reply, flush) = (
                "se".repeat(k as usize),
                "re".repeat(k as usize),
                "fl".repeat(k as usize),
            );
            let call = if rng.chance(2) {
                format!("insideArm({k}, {held})")
            } else {
                format!("beforeMatch({k}, {held})")
            };
            (call, format!("{id},{reply},{flush}/{held}"))
        }
        4 => {
            // A byte list that is sometimes corrupt, so both arms of the
            // group's `Result` are drawn.
            let corrupt = rng.chance(3);
            let mut octets: Vec<i64> = (0..n).map(|i| (i * 7 % 200) + 1).collect();
            if corrupt && !octets.is_empty() {
                let at = rng.below(octets.len());
                octets[at] = 255;
            }
            let list = octets.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
            let answer = if octets.contains(&255) {
                String::from("corrupt")
            } else {
                (octets.iter().sum::<i64>() + octets.len() as i64).to_string()
            };
            (format!("shownWalk(walk([{list}]))"), answer)
        }
        5 => (
            String::from("shownWalk(walk(list.empty<U8>()))"),
            String::from("0"),
        ),
        6 => {
            let present = rng.chance(4);
            // `1..=n`, which every element of fits a `U8` because `n` is at
            // most five.
            let bytes =
                (1..=n).map(|b| b.to_string()).collect::<Vec<_>>().join(", ");
            let (call, answer) = match rng.below(4) {
                0 if present => (format!("defaulted(.Some([{bytes}]))"), n),
                0 => (String::from("defaulted(.None)"), 0),
                1 if present => (format!("matched(.Some([{bytes}]))"), n),
                1 => (String::from("matched(.None)"), 0),
                // `wrapped(.Some(..))` is **not drawn**, and this is the one
                // place in this generator where a shape is held back. It
                // leaks — an `Option` whose payload is a *struct* holding a
                // list drops the payload's list when `withDefault` takes the
                // `Some` arm — and it is pinned as its own test next door
                // (`native/agreement.rs`'s
                // `an_option_of_a_struct_holding_a_list_still_leaks_its_payload`,
                // which asserts the exact number of blocks so a fix fails it).
                // A search that walked into a known finding on most draws
                // would report that one bug for ever and never reach a second,
                // which is the argument `known_signatures` makes for the
                // recorded corpus, applied to a finding the corpus cannot hold
                // because it needs a backend this host may not have.
                2 => (String::from("wrapped(.None)"), 0),
                _ if present => {
                    (format!("wrappedMatch(.Some(Wrapper {{ octets: [{bytes}] }}))"), n)
                }
                _ => (String::from("wrappedMatch(.None)"), 0),
            };
            (format!("\"${{{call}}}\""), answer.to_string())
        }
        7 => {
            // `rows(count)` ranks descending, so sorting by rank reverses the
            // order the rows were built in.
            let answer = (0..n)
                .rev()
                .map(|i| {
                    let k = (i + 1) as usize;
                    format!("{}={}:{}", "r".repeat(k), "v".repeat(k), n - i)
                })
                .collect::<Vec<_>>()
                .join(" ");
            (format!("sorted({n})"), answer)
        }
        _ => {
            let answer = (0..n)
                .map(|i| "r".repeat((i + 1) as usize))
                .collect::<Vec<_>>()
                .join("+");
            (format!("gathered({n})"), answer)
        }
    }
}

/// **The ownership shapes, on every backend, with the heap audited.**
///
/// The `output` property over programs drawn from [`rc_shapes`] rather than
/// from [`printer`]: the same three claims about what was printed, and — since
/// every native run in this file is under the runtime's heap check — the
/// fourth claim that the program gave back every block it took and touched
/// none it had freed.
///
/// This is the search that would have caught the corruption family
/// deterministically. Four of its five reports are shapes nobody would have
/// drawn from an arithmetic generator, because none of them has a wrong
/// *number* in it until a freed block happens to be recycled into something
/// that changes one.
///
/// Sixteen draws in CI, which is about eight seconds: the native leg links and
/// executes a fresh binary per program, and the wall-clock ceiling every search
/// here shares is what stops a slow machine from paying for the count.
#[test]
fn ownership_shapes_agree_on_every_backend_and_leak_nothing() {
    if !engine_present() {
        eprintln!("fuzz ownership: skipped (no JavaScript engine on PATH)");
        return;
    }
    let budget = Budget::new(16);
    let mut rng = Rng(base_seed() ^ 0x5C0F_E000_0000_0001);
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let p = rc_shapes(&mut rng);
        if let Some(why) = output_fires(&p.text, &p.expected) {
            triage(
                &mut seen,
                "output",
                &p.text,
                &why,
                Some(&p.expected),
                "a generated ownership program the backends do not agree on",
            );
        }
        done += 1;
    }
    budget.report("ownership", done, started);
    seen.report("ownership");
    report_native_participation(done);
}

/// The generator's own claim, checked without a backend: every draw is a
/// program the **front end** accepts.
///
/// The search above needs a JavaScript engine, a runtime archive and a linker,
/// so on a host without them it reports nothing — and a generator that had
/// rotted into emitting programs that do not parse would look exactly like one
/// that found no bugs. This runs everywhere and is what says the generator is
/// still generating.
#[test]
fn every_ownership_program_compiles() {
    let mut rng = Rng(base_seed() ^ 0x5C0F_E000_0000_0002);
    for i in 0..12 {
        let p = rc_shapes(&mut rng);
        assert!(
            p.text.contains("export fn main"),
            "draw {i} emitted no entry point"
        );
        if let Some(why) = compiles_fires(&p.text) {
            panic!("draw {i} does not compile:\n{why}\n\n{}", indent(&p.text));
        }
        // And every native backend this toolchain has still *accepts* it,
        // which is a different claim from the one above and catches a
        // different defect: the front end has no opinion about a merged
        // tail-call group, and a backend that has stopped being able to lower
        // one refuses rather than answering wrongly.
        #[cfg(any(feature = "backend-stencil", feature = "backend-llvm"))]
        {
            let refused = native::refusals(&p.text);
            assert!(
                refused.is_empty(),
                "draw {i} was refused by {refused:?}, and every shape it draws is inside \
                 the native surface:\n{}",
                indent(&p.text)
            );
        }
    }
}

/// **Equivalence modulo inputs**: code that cannot run may not change what runs.
///
/// Le, Afshari and Su's oracle (PLDI 2014), in the cheap direction Athena
/// (OOPSLA 2015) added: rather than profiling a program to find its dead code
/// and deleting it, the generator *knows* which region is dead, because it put
/// it there — a branch under a condition derived from `x * 0`, and a function
/// nothing calls. The program and its twin must print the same bytes.
///
/// The language makes this cheaper than it is in C. There is no `catch`, no
/// mutable global, and no address to observe, so a function nobody calls and a
/// branch nobody takes cannot be observed by any means except a compiler bug —
/// which means the technique needs no undefined-behaviour analysis at all, and
/// the whole apparatus Orion needs to be sure the mutant is still legal
/// collapses into two `format!` calls. What it catches is what EMI catches:
/// a pass whose analysis of what is reachable is wrong, monomorphization
/// pulling in a root it should not, or dead-code elimination removing
/// something live.
#[test]
fn dead_code_does_not_change_what_a_program_prints() {
    if !engine_present() {
        eprintln!("fuzz emi: skipped (no JavaScript engine on PATH)");
        return;
    }
    let budget = Budget::new(6);
    let mut rng = Rng(base_seed() ^ 0xE31A_0000_0000_0001);
    let mut seen = Seen::new();
    let started = Instant::now();
    let mut done = 0usize;
    while !budget.spent(done) {
        let p = printer(&mut rng);
        // The original has to be right before its twin can be interesting: a
        // program the backends already disagree about is the other property's
        // finding, not this one's.
        if let Some(why) = output_fires(&p.text, &p.expected) {
            triage(&mut seen, "output", &p.text, &why, Some(&p.expected), "the program EMI started from");
        }
        let twin = with_dead_code(&p.text, &mut rng);
        if let Some(why) = output_fires(&twin, &p.expected) {
            triage(
                &mut seen,
                "output",
                &twin,
                &why,
                Some(&p.expected),
                "inserting code that cannot run changed what the program prints",
            );
        }
        done += 1;
    }
    budget.report("emi", done, started);
    seen.report("emi");
}

/// The program with unreachable code in it, and nothing else changed.
///
/// Two injections, because they are dead for different reasons and a pass can
/// get one right and the other wrong: a function nothing calls is dead to
/// *monomorphization*, which walks from `main`; a branch under a condition
/// that is false for every input is dead to whatever the backend does with
/// control flow.
fn with_dead_code(text: &str, rng: &mut Rng) -> String {
    let (junk, _) = bounded_expr(rng, 3, 7);
    let (more, _) = bounded_expr(rng, 3, 11);
    let orphan = format!(
        "// Nothing calls this.\nfn unreachable{}(x: Int): Int {{\n  {junk}\n}}\n\n",
        rng.below(1_000_000)
    );
    // `x * 0` is zero for every `x` the program can pass, so the branch is
    // dead — and it is dead as a *fact about the program* rather than as a
    // literal the parser can fold, which is what makes the arm reach the
    // middle end at all.
    let guard = format!(
        "  let dead = (x * 0);\n  let _ = if (dead == 1) {{ {more} }} else {{ 0 }};\n"
    );
    let mut out = String::new();
    let mut injected = false;
    for line in text.lines() {
        out.push_str(line);
        out.push('\n');
        if !injected && line.starts_with("fn f0(x: Int): Int {") {
            out.push_str(&guard);
            injected = true;
        }
    }
    format!("{orphan}{out}")
}

// ---------------------------------------------------------------------------
// The machinery, held to the standard it holds the toolchain to
// ---------------------------------------------------------------------------

/// **This suite can fail.**
///
/// `lib/canary` exists in the conformance repository for this reason and says
/// so: a suite that cannot fail is not evidence. A fuzzer is worse than most
/// in that respect — every property here answers `None` on the overwhelming
/// majority of its input, and a property wired up wrongly answers `None` on all
/// of it and passes forever.
///
/// So each property that can be handed a *fabricated* failure is handed one. The
/// three that cannot — `safety`, `roundtrip` and `deterministic` fire only on
/// a real bug, and there is none to hand them — are covered instead by the
/// fact that they are the same function the corpus replay calls, and by
/// the corpus itself.
#[test]
fn the_properties_can_fail() {
    assert!(
        fires("compiles", "fn broken(", None).is_some(),
        "`compiles` accepted a file that is not a declaration"
    );
    assert!(
        fires("compiles", "fn ok(): Int { 1 }\n", None).is_some(),
        "`compiles` accepted a module with no `main`"
    );
    // The corpus this hands the property is the one `derives=0` used to emit.
    // The parameter point compiles now, so the canary is the source rather
    // than the point: without it nothing would hold the half of `generated`
    // that compiles what the generator wrote.
    assert!(
        program_compiles(&generate::Program {
            modules: vec![generate::Module {
                path: "//bench/m0000".into(),
                text: String::from(
                    "export struct Rec0_0 { export f0: Int }\n\nderive  for Rec0_0;\n"
                ),
            }],
        })
        .is_some(),
        "`generated` accepted a corpus the benchmark's own --validate refuses"
    );
    assert!(
        fires("generated", "derives=zero\n", None).is_some(),
        "`generated` accepted a value the dimension refuses"
    );
    assert!(
        fires("generated", "not_a_dimension=1\n", None).is_some(),
        "`generated` accepted a line that is not a parameter"
    );
    assert!(
        fires("generated", "", None).is_none(),
        "`generated` refused the default parameter point, which is the corpus \
         `design/PERFORMANCE.md` quotes its numbers over"
    );
    assert!(
        fires("output", "export fn main(): Result<(), Str> {\n  .Ok(())\n}\n", Some("nope"))
            .is_some()
            || !engine_present(),
        "`output` accepted a program that prints nothing as printing `nope`"
    );
    assert!(
        fires("nonsense", "", None).is_some(),
        "a property nobody implemented answered a question"
    );
}

/// **The watchdog fires**, and the hang it would see is a finding rather than a
/// silence.
///
/// The one branch of `safety` no input can be fabricated for: there is no known
/// program that hangs the toolchain, so the only way to know the timer works is
/// to set it to a millisecond. This test exists because the first version of
/// this file got the branch *inverted* — `run_watched` returned `None` on a
/// timeout into a caller whose `None` means "no finding", so a compiler that
/// never stopped would have passed. A watchdog nobody has watched bark is a
/// watchdog nobody should trust.
#[test]
fn the_watchdog_reports_a_toolchain_that_does_not_stop() {
    let s = Scratch::repo("fuzz-watchdog");
    s.write("app/BUILD.buri", harness::JS_BINARY);
    s.write("app/main.buri", "export fn main(): Result<(), Str> {\n  .Ok(())\n}\n");
    // Zero, not a millisecond: a fast machine can finish a no-op JS build
    // inside any positive deadline, and the question here is whether the
    // timer fires, not whether the build is slow.
    let hurried = run_watched(&s.root, &["build", "//app"], Duration::ZERO);
    assert!(
        hurried.is_err_and(|why| why.contains("did not stop")),
        "a zero deadline let a whole build through, so the deadline does nothing"
    );
    assert!(
        run_watched(&s.root, &["build", "//app"], WATCHDOG).is_ok(),
        "the same build did not finish inside the real watchdog either"
    );
}

/// The minimiser shrinks, and shrinks to the *same* bug.
///
/// Both halves matter and the second is the one that is easy to get wrong: a
/// minimiser holding "still fails" rather than "still fails the same way"
/// walks from the finding it was given to whatever the smallest failure in the
/// language is — for `compiles`, a single `}`. The signature is what stops it,
/// and this is where that is checked.
#[test]
fn the_minimiser_shrinks_to_the_same_finding() {
    let padded = format!(
        "{}\nexport fn main(): Result<(), Str> {{\n  .Ok(())\n}}\n",
        (0..40).map(|i| format!("fn pad{i}(): Int {{ {i} }}\n")).collect::<String>()
    );
    // One line the checker will refuse, buried in forty it will not.
    let broken = padded.replace("fn pad7(): Int { 7 }", "fn pad7(): Int { nosuchname }");
    let before = fires("compiles", &broken, None).expect("the case fails to begin with");
    let small = minimize("compiles", &broken, None);
    let after = fires("compiles", &small, None).expect("the minimised case still fails");
    assert!(
        small.len() < broken.len() / 2,
        "the minimiser shrank {} bytes to {}, which is not shrinking",
        broken.len(),
        small.len()
    );
    assert_eq!(
        signature_of(&before),
        signature_of(&after),
        "the minimiser walked from one finding to another:\n{}\n{}",
        indent(&before),
        indent(&after)
    );

    // The parameter point, whose minimisation is the line pass and nothing
    // else: every dimension not needed to reproduce is a line that comes out.
    let point = "not_a_dimension=1\nident_len=8\nnesting=3\nblank_pct=10\nclusters=4\n";
    let small = minimize("generated", point, None);
    assert!(
        fires("generated", &small, None).is_some(),
        "the minimised parameter point stopped reproducing"
    );
    assert!(
        small.lines().filter(|l| !l.trim().is_empty()).count() == 1,
        "five dimensions minimised to {small:?} rather than to the one that matters"
    );
}

/// A signature is about the bug rather than about the names in it.
#[test]
fn a_signature_survives_a_rename() {
    let a = "the program does not compile:\n  error: `State0_7_xxxx` has no method `show`";
    let b = "the program does not compile:\n  error: `State12_3` has no method `show`";
    assert_eq!(signature_of(a), signature_of(b));
    let other = "the program does not compile:\n  error: `Rec0_0` has no field `f1`";
    assert_ne!(
        signature_of(a),
        signature_of(other),
        "two different diagnostics share a signature, so the corpus would dedup them into one"
    );
}
