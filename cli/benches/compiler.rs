//! The compiler throughput benchmark.
//!
//! `design/PERFORMANCE.md` states three normative goals — 10,000,000 lines per
//! second through lexing and parsing, 1,000,000 through semantic analysis,
//! 100,000 through lowering and emission — and this is the thing that says
//! whether they hold. Every methodological choice here is written down there;
//! what follows is the implementation of it.
//!
//! Run it:
//!
//! ```text
//! cargo bench -p buri --bench compiler                      # the table
//! cargo bench -p buri --bench compiler -- --json            # machine-readable
//! cargo bench -p buri --bench compiler -- --quick           # fewer reps, small scales
//! cargo bench -p buri --bench compiler -- --validate        # compile everything, measure nothing
//! cargo bench -p buri --bench compiler -- --list            # the profile table
//! cargo bench -p buri --bench compiler -- --set=native      # native lowering rows
//! cargo bench -p buri --bench compiler -- --shape=mixed --param lines_per_module=40
//! ```
//!
//! No harness, no `criterion`, no `dev-dependencies`: the workspace's
//! dependency bar admits code generators and platform interfaces, and a
//! statistics library is neither. What a benchmark harness has to do is warm
//! up, repeat, and report a median with its spread, and that is the hundred
//! and fifty lines below.
//!
//! # The phases, and where their seams are
//!
//! ```text
//! lex              parsing::lexer::lex                      text    -> tokens
//! lex+parse        parsing::parser::parse                   text    -> tree     (goal 1)
//! sema             semantics::resolve::Checker::run         Loaded  -> Checked  (goal 2)
//! lower+js         monomorphize::run + actions::emit        Checked -> JavaScript    (goal 3)
//! lower+<triple>   monomorphize::run + actions::prepare
//!                                    + Backend::emit        Checked -> object bytes  (goal 3)
//! ```
//!
//! Each is timed against a value built *before* the timer starts, and each
//! rebuilds everything it produces, so a repetition measures the phase and not
//! a cache. `Checker::run` takes `&Loaded` and returns a fresh `Checked`, and
//! `monomorphize::run` takes `&Checked` and returns a fresh `Program`, so the
//! isolation is a property of the compiler's own signatures rather than
//! something this file arranges.
//!
//! The native rows go through the same two calls `actions::objects_of` makes
//! below the front end — `prepare`, which is the one place the middle-end
//! pipeline is chosen, and `Backend::emit` — and stop there. **Nothing is
//! linked and nothing is run**: the link is the only host-only step, because
//! the runtime archive `cli/build.rs` embeds is built for the host and for
//! nothing else, and goal 3 is stated over lowering rather than over producing
//! an executable.
//!
//! # Two kinds of corpus
//!
//! Most of it is generated per run from a profile and a seed (`generate.rs`);
//! a little of it is checked in with a manifest (`corpus.rs`,
//! `benches/corpora/`). Both are compiled before either is measured, both are
//! read into memory before any timer starts, and both produce the same
//! `Program`, so there is exactly one measurement path. `design/PERFORMANCE.md`
//! §3.1 says what each buys.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "benchmark harness. The lint set in `Cargo.toml` pins a promise \
              about the toolchain — that no input panics it — and a harness \
              that drives the toolchain is not the toolchain. The arithmetic \
              here is over durations and counters this file produced; the \
              printing *is* this target's output, exactly as it is for the \
              files under `commands/`; and a harness that cannot build its own \
              corpus should stop rather than report a number."
)]

mod calibrate;
mod corpus;
mod generate;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use buri::build::actions;
use buri::build::buildfile::{Arch, Platform};
use buri::commands::arguments::Flags;
use buri::compiler::backend::{self, Options, Profile, Target};
use buri::compiler::middle;
use buri::compiler::middle::lower;
use buri::compiler::middle::monomorphize::{self, Roots};
use buri::compiler::modules::{Loaded, Loader, Role};
use buri::compiler::semantics::resolve::Checker;
use buri::diagnostics::{Diagnostics, FileId, SourceMap};
use buri::parsing::lexer;
use buri::parsing::parser;

use generate::{Family, Params, Program};

/// The goals, in lines per second. One constant per row of the table in
/// `design/PERFORMANCE.md`, so that a change to a goal is a change to one line
/// in each of two files rather than a number nobody can find.
const TARGET_PARSE: f64 = 10_000_000.0;
const TARGET_SEMA: f64 = 1_000_000.0;
const TARGET_LOWER: f64 = 100_000.0;

/// The seed. Fixed, because two runs of the suite have to compile the same
/// bytes or the numbers are not comparable, and printed in the header so that
/// a future run can reproduce a past one. It lives in `generate.rs` because a
/// saved corpus's manifest records one too.
const SEED: u64 = generate::SEED;

/// The floor program: the imports every generated module carries and nothing
/// else. Semantic analysis of *any* program pays for the standard library
/// modules it pulls in, and at a thousand lines that fixed cost is most of the
/// measurement — so it is measured on its own and subtracted, and both figures
/// are reported. See `design/PERFORMANCE.md` §3, "The prelude floor".
const FLOOR: &str = "\
from \"core/str/lib.buri\" import * as str;
from \"core/list/lib.buri\" import * as list;
from \"core/effect/lib.buri\" import { Alloc };

export fn main(): Result<(), Str> {
  let xs: [Int] = list.empty<Int>();
  if (xs.len() == 0) { .Ok(()) } else { .Err(\"impossible\") }
}
";

const USAGE: &str = "\
usage: compiler [flags]

  --json                 one JSON document on stdout
  --quick                fewer reps, 1k only, core set
  --validate             compile everything, measure nothing
  --split                break lowering into its sub-phases (stderr)
  --calibrate            speed-of-light ceilings, before the table
  --alloc                allocations per line and per token (needs the
                         `alloc-counter` feature; noise-free)
  --rss                  peak resident set size per phase, untimed

  --set=<name>           core | realistic | stress | native | saved | scale |
                         scale-full | full          (default: core)
  --only=<substring>     keep corpora whose label contains it
  --list                 print the profile table and exit

  --shape=<profile>      run one profile ad hoc, instead of a set
  --param <k>=<v>        override a dimension (repeatable; with --shape)
  --scale=<n>            target lines for --shape
  --seed=<hex>           seed for --shape

  --targets=<list>       js,macos-arm64,macos-x86_64,linux-x86_64,linux-arm64
  --record[=<name>]      write the corpus into cli/benches/corpora/ and exit
  --pin[=<name>]         write a digest-pinned manifest into cli/benches/pinned/
";

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

/// Which corpora a run covers.
///
/// `--set` selects on the *family* for two of these, which is the doc's "stress
/// shapes, kept separate" rule made a property of a type rather than of a
/// convention.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    Core,
    Realistic,
    Stress,
    Native,
    Saved,
    Scale,
    ScaleFull,
    Full,
}

impl Set {
    fn parse(name: &str) -> Option<Set> {
        Some(match name {
            "core" => Set::Core,
            "realistic" => Set::Realistic,
            "stress" => Set::Stress,
            "native" => Set::Native,
            "saved" => Set::Saved,
            "scale" => Set::Scale,
            "scale-full" => Set::ScaleFull,
            "full" => Set::Full,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Set::Core => "core",
            Set::Realistic => "realistic",
            Set::Stress => "stress",
            Set::Native => "native",
            Set::Saved => "saved",
            Set::Scale => "scale",
            Set::ScaleFull => "scale-full",
            Set::Full => "full",
        }
    }

    /// Which pinned corpora this set covers.
    fn tier(self) -> Tier {
        match self {
            Set::Scale => Tier::Sample,
            Set::ScaleFull => Tier::All,
            _ => Tier::Anchor,
        }
    }
}

struct Args {
    json: bool,
    quick: bool,
    validate_only: bool,
    want_split: bool,
    want_calibrate: bool,
    want_alloc: bool,
    want_rss: bool,
    /// Set only in the child process an `--rss` run spawns: run this one phase
    /// once, measure nothing, print nothing.
    rss_child: Option<String>,
    list: bool,
    set: Set,
    only: Option<String>,
    shape: Option<String>,
    overrides: Vec<(String, String)>,
    scale: Option<usize>,
    seed: Option<u64>,
    targets: Vec<Target>,
    record: Option<String>,
    pin: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        json: false,
        quick: false,
        validate_only: false,
        want_split: false,
        want_calibrate: false,
        want_alloc: false,
        want_rss: false,
        rss_child: None,
        list: false,
        set: Set::Core,
        only: None,
        shape: None,
        overrides: Vec::new(),
        scale: None,
        seed: None,
        targets: default_targets(),
        record: None,
        pin: None,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let value = |rest: &str| rest.to_string();
        match arg {
            "--json" => a.json = true,
            "--quick" => a.quick = true,
            "--validate" => a.validate_only = true,
            "--split" => a.want_split = true,
            "--calibrate" => a.want_calibrate = true,
            "--alloc" => a.want_alloc = true,
            "--rss" => a.want_rss = true,
            "--list" => a.list = true,
            "--record" => a.record = Some(String::new()),
            "--pin" => a.pin = Some(String::new()),
            // `cargo bench` passes `--bench` to every target it runs; ignoring
            // it is what lets this file be a bench target at all.
            "--bench" => {}
            "--param" => {
                i += 1;
                let Some(pair) = argv.get(i) else {
                    fail("--param wants `key=value`");
                };
                push_param(&mut a, pair);
            }
            _ => {
                if let Some(v) = arg.strip_prefix("--set=") {
                    let Some(set) = Set::parse(v) else {
                        fail(&format!("unknown set `{v}`"));
                    };
                    a.set = set;
                } else if let Some(v) = arg.strip_prefix("--only=") {
                    a.only = Some(value(v));
                } else if let Some(v) = arg.strip_prefix("--shape=") {
                    if generate::profile(v).is_none() {
                        fail(&format!(
                            "unknown profile `{v}`; try --list. The profiles are: {}",
                            generate::PROFILES.join(", ")
                        ));
                    }
                    a.shape = Some(value(v));
                } else if let Some(v) = arg.strip_prefix("--param=") {
                    push_param(&mut a, v);
                } else if let Some(v) = arg.strip_prefix("--scale=") {
                    let Ok(n) = v.replace('_', "").parse::<usize>() else {
                        fail(&format!("`{v}` is not a line count"));
                    };
                    a.scale = Some(n);
                } else if let Some(v) = arg.strip_prefix("--seed=") {
                    let t = v.strip_prefix("0x").unwrap_or(v).replace('_', "");
                    let Ok(n) = u64::from_str_radix(&t, 16) else {
                        fail(&format!("`{v}` is not a hexadecimal seed"));
                    };
                    a.seed = Some(n);
                } else if let Some(v) = arg.strip_prefix("--targets=") {
                    let mut ts = Vec::new();
                    for name in v.split(',').filter(|s| !s.is_empty()) {
                        let Some(t) = parse_target(name) else {
                            fail(&format!(
                                "unknown target `{name}`; the targets are: js, macos-arm64, \
                                 macos-x86_64, linux-x86_64, linux-arm64"
                            ));
                        };
                        ts.push(t);
                    }
                    a.targets = ts;
                } else if let Some(v) = arg.strip_prefix("--record=") {
                    a.record = Some(value(v));
                } else if let Some(v) = arg.strip_prefix("--rss-child=") {
                    a.rss_child = Some(value(v));
                } else if let Some(v) = arg.strip_prefix("--pin=") {
                    a.pin = Some(value(v));
                } else {
                    eprintln!("unknown argument {arg}");
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                }
            }
        }
        i += 1;
    }
    a
}

fn push_param(a: &mut Args, pair: &str) {
    let Some((k, v)) = pair.split_once('=') else {
        fail(&format!("`{pair}` is not `key=value`"));
    };
    a.overrides.push((k.to_string(), v.to_string()));
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    eprintln!("{USAGE}");
    std::process::exit(2);
}

/// The two triples `design/PERFORMANCE.md` §4 names, plus JavaScript.
///
/// Both native triples are asked for on whichever machine the suite runs on:
/// the development backend bakes a stencil library per target into the
/// toolchain, so a cross triple costs no extra work at run time, and a cross
/// emission is *more* reproducible than the host's — nothing about it is
/// inferred from the running CPU.
///
/// `linux-arm64` is a third default rather than one of the two the doc names,
/// so the report always carries a cross triple that emits. All four defaults do
/// emit today; `macos-x86_64` is the one triple the development backend refuses,
/// for want of a stencil library, and it is not among them.
fn default_targets() -> Vec<Target> {
    vec![
        Target { platform: Platform::Js, arch: None },
        Target { platform: Platform::Macos, arch: Some(Arch::Arm64) },
        Target { platform: Platform::Linux, arch: Some(Arch::X86_64) },
        Target { platform: Platform::Linux, arch: Some(Arch::Arm64) },
    ]
}

fn parse_target(name: &str) -> Option<Target> {
    Some(match name {
        "js" => Target { platform: Platform::Js, arch: None },
        "macos-arm64" => Target { platform: Platform::Macos, arch: Some(Arch::Arm64) },
        "macos-x86_64" => Target { platform: Platform::Macos, arch: Some(Arch::X86_64) },
        "linux-x86_64" => Target { platform: Platform::Linux, arch: Some(Arch::X86_64) },
        "linux-arm64" => Target { platform: Platform::Linux, arch: Some(Arch::Arm64) },
        _ => return None,
    })
}

fn target_name(t: Target) -> String {
    let platform = match t.platform {
        // Neither carries an arch, so neither has a second half to name.
        Platform::Js => return "js".to_string(),
        Platform::Web => return "web".to_string(),
        Platform::Macos => "macos",
        Platform::Linux => "linux",
    };
    let arch = match t.arch {
        Some(Arch::Arm64) => "arm64",
        Some(Arch::X86_64) => "x86_64",
        None => "host",
    };
    format!("{platform}-{arch}")
}

/// The phase name a lowering row carries.
///
/// `lower+js` is unchanged, because a JSON consumer of today's output has to
/// still parse tomorrow's. A release row is an LLVM row and says so, because
/// `backend::select` maps `(native, Release)` to LLVM and `(native, Debug)` to
/// the stencil backend: they are not one backend at two settings, they are the two builds
/// the toolchain actually performs.
fn phase_name(t: Target, p: Profile) -> String {
    match (t.platform, p) {
        (Platform::Js, _) => "lower+js".to_string(),
        (_, Profile::Debug) => format!("lower+{}", target_name(t)),
        (_, Profile::Release) => format!("lower+{}-release", target_name(t)),
    }
}

// ---------------------------------------------------------------------------
// The work list
// ---------------------------------------------------------------------------

/// Where a corpus comes from. Every arm produces the same [`Program`].
enum Source {
    Generated(Params),
    Saved(PathBuf),
    /// A manifest with no source: regenerated per run and checked against its
    /// recorded digest before anything is measured.
    Pinned(PathBuf),
}

struct Plan {
    label: String,
    family: Family,
    source: Source,
    /// Whether this corpus gets native lowering rows. Native codegen is the
    /// expensive phase, so the matrix is deliberately narrow by default.
    native: bool,
    /// Whether its native rows may include the cross triples. False only in the
    /// scale tier, and there only away from the anchor: forty pinned corpora
    /// times three triples is an hour of cross-compiling to re-answer a question
    /// the anchor answers on the same bytes.
    cross: bool,
}

/// The profiles in each family, in report order.
fn family_profiles(family: Family) -> Vec<&'static str> {
    generate::PROFILES
        .iter()
        .copied()
        .filter(|n| generate::profile(n).is_some_and(|(f, _)| f == family))
        .collect()
}

fn generated(name: &str, lines: usize, seed: u64, native: bool) -> Plan {
    let (family, mut params) = generate::profile(name).expect("a name from PROFILES");
    params.lines = lines;
    params.seed = seed;
    Plan {
        label: format!("{name}/{}", human(lines)),
        family,
        source: Source::Generated(params),
        native,
        cross: true,
    }
}

fn saved_plans(native: bool) -> Vec<Plan> {
    let root = corpus::root();
    corpus::discover(&root)
        .into_iter()
        .filter_map(|dir| {
            let manifest = corpus::manifest(&dir).ok()?;
            Some(Plan {
                label: format!("saved:{}", manifest.name),
                family: manifest.family(),
                source: Source::Saved(dir),
                native,
                cross: true,
            })
        })
        .collect()
}

/// The parameter point the scale tier is anchored on.
///
/// Every other pinned corpus is a delta from it, so it is the one whose series
/// has to be unbroken: it keeps the cross triples, it is the pinned half a plain
/// `--validate` covers, and it is the only 1M corpus `--set=scale` measures.
const ANCHOR_POINT: &str = "mixed";

/// The parameter point a pinned corpus's name states, which is its name without
/// the scale suffix.
///
/// A pinned corpus is named `<point>-<scale>`, and the convention is already
/// load-bearing: `--pin=mixed-1M` reads the scale off the same suffix. The point
/// is what identifies a corpus *across* scales, which is what a tier and a
/// twenty-by-two sweep are both stated over. It is not the profile — four of the
/// points are the `mixed` profile with a parameter delta, and their manifests
/// say so.
fn pinned_point(name: &str) -> &str {
    name.rsplit_once('-').map_or(name, |(point, _)| point)
}

/// How much of the pinned half a run covers.
///
/// Forty pinned corpora are a parameter sweep, not a tier, and the sweep at a
/// million lines is twenty minutes. So the wall time is a property of the flag
/// rather than of the directory: `--set=scale-full` is the whole sweep,
/// `--set=scale` is the sample that fits in a coffee break, and every other set
/// — including a plain `--validate` — carries the anchor and nothing more.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tier {
    Anchor,
    /// Every pinned corpus the standard protocol applies to, plus the anchor
    /// above it. The threshold is [`REDUCED_ABOVE_LINES`] rather than a second
    /// number: the scale at which a row costs minutes is already named, and it
    /// is the same scale.
    Sample,
    All,
}

/// The pinned manifests a tier covers, as plans.
///
/// The tier is every `.txt` in the directory filtered by the manifest's own
/// fields, so a new scale point is still a new manifest and nothing else.
fn pinned_plans(tier: Tier) -> Vec<Plan> {
    let root = corpus::pinned_root();
    corpus::discover_pinned(&root)
        .into_iter()
        .filter_map(|path| {
            let manifest = corpus::pinned_manifest(&path).ok()?;
            let anchor = pinned_point(&manifest.name) == ANCHOR_POINT;
            let wanted = match tier {
                Tier::Anchor => anchor,
                Tier::Sample => anchor || manifest.lines <= REDUCED_ABOVE_LINES,
                Tier::All => true,
            };
            if !wanted {
                return None;
            }
            Some(Plan {
                label: format!("pinned:{}", manifest.name),
                family: manifest.family(),
                source: Source::Pinned(path),
                native: manifest.native,
                cross: anchor,
            })
        })
        .collect()
}

/// Which corpora a set covers, and which of them get native rows.
///
/// | Set | Native rows |
/// |---|---|
/// | `core` (default) | `mixed` at every scale, both triples, the debug backend. |
/// | `native` | Every realistic profile plus the three lowering-heavy stress ones. |
/// | `full` | Everything. |
fn build_work(set: Set, scales: &[usize], stress_scale: usize, seed: u64) -> Vec<Plan> {
    let mut work: Vec<Plan> = Vec::new();
    match set {
        Set::Core => {
            for &n in scales {
                work.push(generated("mixed", n, seed, true));
            }
            for name in family_profiles(Family::Realistic).into_iter().filter(|n| *n != "mixed") {
                work.push(generated(name, stress_scale, seed, false));
            }
            for name in family_profiles(Family::Stress) {
                // Two of the stress profiles carry the default run's native
                // rows. This was inherited from a backend with no body for
                // `list.filter`, `list.fold` or `list.mapCtx`, which is what
                // made `mixed`'s native rows skip; the stencil backend has all
                // six, so the set this list could take is now wider and is
                // worth revisiting against a measured run rather than widened
                // on the strength of a refusal that no longer happens.
                let native = matches!(name, "many-small-fns" | "enum-heavy");
                work.push(generated(name, stress_scale, seed, native));
            }
            // The pairing rule of `design/PERFORMANCE.md` §3.1: §6 records both
            // the generated and the saved reading of `mixed`, and the two
            // deltas are compared. When the compiler changes both move; when
            // the *generator* changes only the generated one does.
            work.extend(
                saved_plans(false)
                    .into_iter()
                    .filter(|p| p.label.starts_with("saved:mixed-1k") || p.label.starts_with("saved:mixed-10k")),
            );
        }
        Set::Realistic => {
            for name in family_profiles(Family::Realistic) {
                for &n in scales {
                    work.push(generated(name, n, seed, false));
                }
            }
        }
        Set::Stress => {
            for name in family_profiles(Family::Stress) {
                work.push(generated(name, stress_scale, seed, false));
            }
        }
        Set::Native => {
            for name in family_profiles(Family::Realistic) {
                work.push(generated(name, stress_scale, seed, true));
            }
            // `derive-heavy` earns its place specifically because
            // `middle::derives` runs only on the native branch, so a
            // regression in it is invisible in every JS row.
            //
            // The last four are here because they were the profiles the
            // previous debug backend could *take*: the realistic mix calls
            // `list.filter`, `list.fold` and `list.mapCtx`, which it had no
            // body for, so every realistic row skipped. The stencil backend
            // has all six, so this list is narrower than what the backend can
            // now be asked — the same revisit the `Stress` arm above notes.
            // `enum-heavy` is where the first native gap showed up.
            for name in [
                "many-small-fns",
                "few-large-fns",
                "derive-heavy",
                "struct-heavy",
                "enum-heavy",
                "wide-match",
                "deep-nesting",
            ] {
                work.push(generated(name, stress_scale, seed, true));
            }
        }
        Set::Saved => work.extend(saved_plans(true)),
        // The scale tier is pinned manifests and nothing else. It is
        // deliberately not in `core` and not in `full`: a million-line row
        // costs minutes, and the default run has to stay something a
        // contributor takes before a commit rather than over lunch.
        Set::Scale => work.extend(pinned_plans(Tier::Sample)),
        Set::ScaleFull => work.extend(pinned_plans(Tier::All)),
        Set::Full => {
            for name in family_profiles(Family::Realistic) {
                for &n in scales {
                    work.push(generated(name, n, seed, true));
                }
            }
            for name in family_profiles(Family::Stress) {
                work.push(generated(name, stress_scale, seed, true));
            }
            work.extend(saved_plans(true));
        }
    }
    work
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

/// Runs the benchmark on the stack the toolchain gives itself.
///
/// Every stage after parsing walks a tree by recursion, and `main.rs` reserves
/// `parallel::STACK` for exactly that; the process main thread's default is a
/// fraction of it, and `wide-match/10k` overflowed it.
fn main() {
    std::thread::Builder::new()
        .name("bench".into())
        .stack_size(buri::parallel::STACK)
        .spawn(run)
        .expect("a thread")
        .join()
        .expect("the benchmark does not panic");
}

fn run() {
    let args = parse_args();

    if args.list {
        print_profiles();
        return;
    }

    let seed = args.seed.unwrap_or(SEED);

    if let Some(name) = &args.record {
        do_record(&args, name, seed, false);
        return;
    }
    if let Some(name) = &args.pin {
        do_record(&args, name, seed, true);
        return;
    }

    let cfg = Config {
        min_reps: if args.quick { 5 } else { 10 },
        min_time: if args.quick {
            Duration::from_millis(200)
        } else {
            Duration::from_millis(750)
        },
        warmup: if args.quick {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(300)
        },
        warm_reps: 2,
        reduced: false,
    };

    let scales: Vec<usize> =
        if args.quick { vec![1_000] } else { vec![1_000, 10_000, 100_000] };
    let stress_scale = if args.quick { 1_000 } else { 10_000 };

    // `--shape` and `--set` are exclusive: one profile ad hoc, or a set.
    let mut work = if let Some(name) = &args.shape {
        let (family, mut params) = generate::profile(name).expect("checked at parse time");
        params.lines = args.scale.unwrap_or(stress_scale);
        params.seed = seed;
        for (k, v) in &args.overrides {
            if let Err(e) = params.set(k, v) {
                fail(&e);
            }
        }
        vec![Plan {
            label: format!("{name}/{}", human(params.lines)),
            family,
            source: Source::Generated(params),
            native: true,
            cross: true,
        }]
    } else {
        if !args.overrides.is_empty() {
            fail("--param needs --shape: a set is a table of profiles, not one point");
        }
        build_work(args.set, &scales, stress_scale, seed)
    };

    // `--validate` always covers the saved corpora, whatever the set: a saved
    // corpus that has stopped being valid Buri is a build failure exactly as a
    // drifted generator is, and CI runs `--validate --quick`.
    if args.validate_only {
        let have: Vec<String> = work.iter().map(|p| p.label.clone()).collect();
        for plan in saved_plans(false) {
            if !have.contains(&plan.label) {
                work.push(plan);
            }
        }
        // The pinned manifests too, and for the same reason — a pinned digest
        // that no longer matches is the staleness failure this scheme exists to
        // produce. How many of them is `--set`'s business and not `--validate`'s:
        // regenerating and digesting forty corpora, half of them a million
        // lines, is minutes, and a plain `--validate` has to stay the check
        // somebody takes before a commit. So the anchor by default, the sample
        // under `--set=scale`, all forty under `--set=scale-full`, and none
        // under `--quick`, which is the CI gate and has to stay under a second.
        if !args.quick {
            for plan in pinned_plans(args.set.tier()) {
                if !have.contains(&plan.label) {
                    work.push(plan);
                }
            }
        }
    }

    if let Some(filter) = &args.only {
        work.retain(|p| p.label.contains(filter.as_str()));
    }
    if work.is_empty() {
        eprintln!("no corpora selected");
        std::process::exit(2);
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut calibrations: Vec<(String, usize, usize, Vec<calibrate::Ceiling>)> = Vec::new();
    let mut flat_nodes = 0usize;
    let mut alloc_rows: Vec<AllocRow> = Vec::new();
    let mut memory_rows: Vec<MemoryRow> = Vec::new();

    if !args.json && !args.validate_only && args.rss_child.is_none() {
        header(&cfg, &scales, args.set, &args.targets);
    }

    // The floor, first, because every later row's net figure is relative to it.
    let floor =
        if args.validate_only || args.rss_child.is_some() { Floor::zero() } else { floor_costs() };
    if !args.json && !args.validate_only && args.rss_child.is_none() {
        println!(
            "  prelude floor  lex {:.3} ms   parse {:.3} ms   sema {:.3} ms   lower {:.3} ms",
            ms(floor.lex),
            ms(floor.parse),
            ms(floor.sema),
            ms(floor.lower)
        );
        println!();
    }

    if args.validate_only {
        validate_targets(&args.targets);
    }

    for plan in &work {
        let (program, revision, source_kind) = match &plan.source {
            Source::Generated(params) => (generate::program(params), 0, "generated"),
            Source::Saved(dir) => match corpus::load(dir) {
                Ok((manifest, program)) => {
                    if manifest.generator_revision != generate::GENERATOR_REVISION {
                        eprintln!(
                            "  note: {} was recorded at generator revision {} and this \
                             toolchain's generator is at {} — which is the point, not a problem",
                            plan.label,
                            manifest.generator_revision,
                            generate::GENERATOR_REVISION
                        );
                    }
                    (program, manifest.revision, "saved")
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            },
            // The digest is checked here, at work-list construction, which is
            // the same place a saved corpus's is and one step before anything
            // is timed. A mismatch is a staleness failure and stops the run.
            Source::Pinned(path) => match corpus::load_pinned(path) {
                Ok((manifest, program)) => {
                    if manifest.generator_revision != generate::GENERATOR_REVISION {
                        eprintln!(
                            "  note: {} was pinned at generator revision {} and this \
                             toolchain's generator is at {} — and the digest still matches, \
                             which is the whole claim",
                            plan.label,
                            manifest.generator_revision,
                            generate::GENERATOR_REVISION
                        );
                    }
                    (program, manifest.revision, "pinned")
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            },
        };
        let label = &plan.label;

        // The child of an `--rss` run: one phase, once, and out. Before the
        // validation below, because a validated corpus is a checked corpus and
        // the checker's arenas would be in every figure the parent reads.
        if let Some(phase) = &args.rss_child {
            let targets = lowering_targets(&args, plan, program.lines());
            run_one_phase(&program, phase, &targets);
            return;
        }

        // Carbon's rule, and the one that makes the rest of this file mean
        // something: a benchmark over source that does not compile is a
        // benchmark of the error paths. Nothing is timed until the real front
        // end has accepted the corpus.
        match validate(&program) {
            Ok(()) => {}
            Err(problems) => {
                eprintln!("the corpus for {label} does not compile:");
                for p in problems.iter().take(10) {
                    eprintln!("  {p}");
                }
                eprintln!("  ({} diagnostics)", problems.len());
                std::process::exit(1);
            }
        }
        if args.validate_only {
            let nodes = match check_flat_tree(&program) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("the flat parse tree for {label} is malformed:");
                    eprintln!("  {e}");
                    std::process::exit(1);
                }
            };
            flat_nodes += nodes;
            let (funcs, emitted) = reach(&program);
            println!(
                "ok  {label:24} {:>7} lines {:>9} bytes {:>4} modules  ->  {:>6} functions \
                 monomorphized, {:>9} bytes of JavaScript",
                program.lines(),
                program.bytes(),
                program.modules.len(),
                funcs,
                emitted
            );
            continue;
        }

        if args.want_calibrate {
            let c = calibration_corpus(&program);
            // There is no `parse-flat` row. It existed to price the flat tree
            // apart from the materializing shim that was built beside it; the
            // shim is gone and `parse` builds the flat tree alone, so the
            // `lex+parse` row of the default table is that measurement.
            let ceilings = calibrate::run(&c, |f| bench(&cfg, f));
            if !args.json {
                print_calibration(label, &c, &ceilings);
            }
            calibrations.push((label.clone(), c.lines, c.tokens, ceilings));
        }
        if args.want_alloc {
            let row = alloc_row(&program, label);
            if !args.json {
                print_alloc(&row);
            }
            alloc_rows.push(row);
        }

        let targets = lowering_targets(&args, plan, program.lines());
        // Untimed, and before the timers rather than after: the sampler forks
        // a process every twenty milliseconds, and a timed row must not be
        // taken beside one that does.
        if args.want_rss {
            let row = memory_row(label, program.lines(), &targets);
            if !args.json {
                print_memory(&row);
            }
            memory_rows.push(row);
        }
        let cfg = protocol_for(&cfg, program.lines());
        let before = rows.len();
        let (new_rows, new_skips) =
            measure(&program, plan, source_kind, revision, &cfg, &floor, &targets);
        rows.extend(new_rows);
        skipped.extend(new_skips);
        if !args.json {
            for row in &rows[before..] {
                print_row(row);
            }
            for skip in skipped.iter().filter(|s| s.label == *label) {
                println!("  {:<22} {:<20} skipped: {}", skip.label, skip.phase, skip.reason);
            }
        }
        if args.want_split {
            split_lowering(&program, label, &cfg, &targets);
        }
        if !args.json {
            println!();
        }
    }

    if args.validate_only {
        let root = corpus::root();
        let total = corpus::total_bytes(&root);
        println!();
        println!(
            "checked-in corpora: {} of {} KiB used ({} corpora, cap {} KiB each)",
            total / 1024,
            corpus::MAX_TOTAL_BYTES / 1024,
            corpus::discover(&root).len(),
            corpus::MAX_CORPUS_BYTES / 1024
        );
        if total > corpus::MAX_TOTAL_BYTES {
            eprintln!(
                "cli/benches/corpora is over its {} KiB cap; delete a corpus or generate it",
                corpus::MAX_TOTAL_BYTES / 1024
            );
            std::process::exit(1);
        }
        println!(
            "every flat parse tree is a well-formed post-order forest ({flat_nodes} nodes checked)."
        );
        println!("the whole corpus compiles.");
        return;
    }

    if args.json {
        print_json(&rows, &skipped, &floor, &calibrations, &alloc_rows, &memory_rows);
    } else {
        footer(&rows, &skipped);
    }
}

// ---------------------------------------------------------------------------
// Speed of light, and the allocation counter
// ---------------------------------------------------------------------------

/// Whether a flat parse tree's `subtree` counts describe a well-formed
/// post-order forest, and how many nodes were checked.
///
/// The rule the arenas promise is that a node's descendants are the contiguous
/// range that ends just before it, and that the range is exactly filled by the
/// complete subtrees of its children. Walking the array once with a stack of
/// finished subtrees checks both at the same time: greedily absorb the
/// subtrees that end where the current node's children must end, and the
/// remainder has to be empty.
///
/// Nothing in the compiler reads `subtree` yet — the consumers move onto the
/// flat tree one wave at a time — so this is what stops the field from being a
/// promise nobody has checked until the pass that first needs it.
fn well_formed(counts: &[(u32, u32)]) -> bool {
    let mut stack: Vec<(u32, u32)> = Vec::new();
    for &(i, subtree) in counts {
        if subtree == 0 || subtree > i + 1 {
            return false;
        }
        let start = i + 1 - subtree;
        let mut need = i;
        while need > start {
            match stack.last() {
                Some(&(cs, cr)) if cr + 1 == need && cs >= start => {
                    need = cs;
                    stack.pop();
                }
                _ => break,
            }
        }
        if need != start {
            return false;
        }
        stack.push((start, i));
    }
    true
}

fn check_flat_tree(program: &Program) -> Result<usize, String> {
    let mut total = 0usize;
    for (i, m) in program.modules.iter().enumerate() {
        let parsed = parser::parse(&m.text, FileId(i as u32));
        let t = &parsed.module.tree;
        let exprs: Vec<(u32, u32)> =
            t.nodes().iter().enumerate().map(|(j, n)| (j as u32, n.subtree)).collect();
        let pats: Vec<(u32, u32)> =
            t.pat_nodes().iter().enumerate().map(|(j, n)| (j as u32, n.subtree)).collect();
        total += exprs.len() + pats.len();
        if !well_formed(&exprs) {
            return Err(format!("{}: the expression arena is not a post-order forest", m.path));
        }
        if !well_formed(&pats) {
            return Err(format!("{}: the pattern arena is not a post-order forest", m.path));
        }
    }
    Ok(total)
}

/// The corpus a calibration run is normalized by: the same texts the timed
/// rows use, and the same token count, counted by lexing for real.
fn calibration_corpus(program: &Program) -> calibrate::Corpus<'_> {
    let texts: Vec<&str> = program.modules.iter().map(|m| m.text.as_str()).collect();
    let tokens: usize =
        program.modules.iter().map(|m| lexer::lex(&m.text, FileId(0)).tokens.len()).sum();
    calibrate::Corpus { texts, lines: program.lines(), bytes: program.bytes(), tokens }
}

fn print_calibration(label: &str, c: &calibrate::Corpus<'_>, rows: &[calibrate::Ceiling]) {
    // The arenas the corpus actually produces, beside the node count the
    // `node-write` ceiling assumed: a ceiling taken over the wrong number of
    // nodes is not a ceiling for anything.
    let mut real = [0usize; 6];
    for (i, t) in c.texts.iter().enumerate() {
        let parsed = parser::parse(t, FileId(i as u32));
        for (a, b) in real.iter_mut().zip(parsed.module.tree.counts()) {
            *a += b;
        }
    }
    println!(
        "  {label:<22} speed of light   {} lines, {} tokens, {} nodes assumed",
        c.lines,
        c.tokens,
        c.nodes()
    );
    println!(
        "  {:<22} arenas: {} expr nodes, {} pattern nodes, {} kids, {} stmts, {} types, {} strings",
        "", real[0], real[1], real[2], real[3], real[4], real[5]
    );
    for r in rows {
        println!(
            "  {:<22} {:<12} {:>8.3} ms  ±{:>5.1}%  {:>7.2} ns/line  {:>6.2} ns/{}   {}",
            "",
            r.name,
            ms(r.median),
            r.dispersion * 100.0,
            r.ns_per_line(c.lines),
            r.ns_per_unit(),
            r.unit,
            r.bounds
        );
    }
    let (sum, verdict) = calibrate::verdict(rows, c.lines);
    println!("  {:<22} front-end ceiling {sum:.1} ns/line ({:.2} M lines/s)", "", 1e9 / sum / 1e6);
    println!("  {:<22} {verdict}", "");
}

/// Allocations for one corpus, by phase. Exact rather than sampled: the
/// counter is a global allocator, so this is the one figure in the whole
/// benchmark that has no noise at all.
struct AllocRow {
    label: String,
    lines: usize,
    tokens: usize,
    /// `(phase, allocations)`, or empty when the binary was built without the
    /// counter.
    phases: Vec<(&'static str, u64)>,
}

/// A counting global allocator, behind a feature because the counter is two
/// atomic increments on every allocation in the process and every timed row
/// would pay them.
///
/// `cargo bench -p buri --features alloc-counter --bench compiler -- --alloc`.
#[cfg(feature = "alloc-counter")]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static ALLOCS: AtomicU64 = AtomicU64::new(0);

    pub struct Counting;

    // SAFETY-free by construction: every method forwards to `System` unchanged
    // and the counter is the only added effect.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            System.alloc(l)
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            System.dealloc(p, l);
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            System.realloc(p, l, n)
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            System.alloc_zeroed(l)
        }
    }

    pub fn count() -> u64 {
        ALLOCS.load(Ordering::Relaxed)
    }
}

#[cfg(feature = "alloc-counter")]
#[global_allocator]
static COUNTING_ALLOCATOR: counting::Counting = counting::Counting;

#[cfg(feature = "alloc-counter")]
fn allocations_of(f: impl FnOnce()) -> u64 {
    let before = counting::count();
    f();
    counting::count().saturating_sub(before)
}

#[cfg(not(feature = "alloc-counter"))]
fn allocations_of(_f: impl FnOnce()) -> u64 {
    0
}

fn alloc_row(program: &Program, label: &str) -> AllocRow {
    let lines = program.lines();
    let tokens: usize =
        program.modules.iter().map(|m| lexer::lex(&m.text, FileId(0)).tokens.len()).sum();
    let mut phases = Vec::new();
    if cfg!(feature = "alloc-counter") {
        let texts: Vec<&str> = program.modules.iter().map(|m| m.text.as_str()).collect();
        phases.push((
            "lex",
            allocations_of(|| {
                for (i, t) in texts.iter().enumerate() {
                    std::hint::black_box(lexer::lex(t, FileId(i as u32)));
                }
            }),
        ));
        phases.push((
            "lex+parse",
            allocations_of(|| {
                for (i, t) in texts.iter().enumerate() {
                    std::hint::black_box(parser::parse(t, FileId(i as u32)));
                }
            }),
        ));
        let mut map = SourceMap::new();
        let mut cache = parser::Cache::new();
        let mut diagnostics = Diagnostics::new();
        let loaded = load(program, &mut map, &mut cache, &mut diagnostics);
        phases.push((
            "sema",
            allocations_of(|| {
                let mut d = Diagnostics::new();
                std::hint::black_box(Checker::new(&loaded, None, &mut d).run());
            }),
        ));
    }
    AllocRow { label: label.to_string(), lines, tokens, phases }
}

fn print_alloc(row: &AllocRow) {
    if row.phases.is_empty() {
        println!(
            "  {:<22} allocations  built without the counter; rebuild with \
             `--features alloc-counter`",
            row.label
        );
        return;
    }
    for (phase, n) in &row.phases {
        println!(
            "  {:<22} {phase:<12} {n:>10} allocations  {:>8.1} per 1000 lines  {:>6.3} per token",
            row.label,
            *n as f64 * 1000.0 / row.lines.max(1) as f64,
            *n as f64 / row.tokens.max(1) as f64
        );
    }
}

// ---------------------------------------------------------------------------
// Peak memory
// ---------------------------------------------------------------------------

/// Peak resident set size, taken from a subprocess run of this same binary.
///
/// There is no dependency-free way for a process to ask for its own high-water
/// mark here. Linux has `/proc/self/status`'s `VmHWM`; macOS has no `/proc`,
/// `getrusage` is behind `libc` — not a dependency of this workspace, and not
/// one a memory column is worth buying — and `ps -o rss` requires an
/// entitlement on current macOS. What every platform does have is a program
/// whose whole job is to report the number, so `--rss` re-runs this binary once
/// per phase under `/usr/bin/time -l` and reads it back.
///
/// One phase per process is not a workaround, it is the measurement: a peak is
/// monotonic, so the peak of a process that stopped after `sema` *is* the cost
/// of everything up to and including `sema`, and the difference between two of
/// them is what a phase added. Sampling the current figure instead would miss
/// whatever a phase allocates and frees inside itself, which at these scales is
/// most of the question.
fn peak_rss_of_child(label: &str, phase: &str) -> Option<u64> {
    let exe = std::env::current_exe().ok()?;
    let mut argv: Vec<String> = vec![exe.to_string_lossy().into_owned()];
    argv.extend(
        std::env::args()
            .skip(1)
            .filter(|a| a != "--rss" && !a.starts_with("--only=") && !a.starts_with("--rss-child=")),
    );
    argv.push(format!("--only={label}"));
    argv.push(format!("--rss-child={phase}"));

    let out = std::process::Command::new("/usr/bin/time").arg("-l").args(&argv).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stderr);
    for line in text.lines() {
        let t = line.trim();
        // BSD `time -l`: "  1289109504  maximum resident set size", in bytes.
        if let Some(n) = t.strip_suffix("maximum resident set size") {
            if let Ok(bytes) = n.trim().parse::<u64>() {
                return Some(bytes);
            }
        }
        // GNU `time -v`, which some Linux distributions install as
        // /usr/bin/time: kibibytes, and after a colon.
        if let Some(n) = t.strip_prefix("Maximum resident set size (kbytes):") {
            if let Ok(kib) = n.trim().parse::<u64>() {
                return Some(kib * 1024);
            }
        }
    }
    None
}

/// Peak resident set size per phase, for one corpus.
struct MemoryRow {
    label: String,
    lines: usize,
    /// `(phase, peak bytes)`, cumulative in the way a compilation is: `corpus`
    /// is the program held in memory and nothing built, and every later phase
    /// holds what the ones before it produced.
    phases: Vec<(String, u64)>,
}

/// The phases an `--rss` pass takes a child process for.
fn memory_row(label: &str, lines: usize, targets: &[(Target, Profile)]) -> MemoryRow {
    let mut phases: Vec<(String, u64)> = Vec::new();
    let mut names: Vec<String> =
        vec!["corpus".to_string(), "lex".to_string(), "lex+parse".to_string(), "sema".to_string()];
    names.extend(targets.iter().map(|&(t, p)| phase_name(t, p)));
    for name in names {
        if let Some(peak) = peak_rss_of_child(label, &name) {
            phases.push((name, peak));
        }
    }
    MemoryRow { label: label.to_string(), lines, phases }
}

/// One phase, once, in a child process whose peak somebody else is watching.
///
/// Nothing is validated and nothing is timed: the parent is measuring the high
/// water mark of this process, so every allocation this function does not need
/// to make is one that would be attributed to the phase.
fn run_one_phase(program: &Program, phase: &str, targets: &[(Target, Profile)]) {
    if phase == "corpus" {
        std::hint::black_box(program.lines());
        return;
    }
    let texts: Vec<&str> = program.modules.iter().map(|m| m.text.as_str()).collect();
    if phase == "lex" {
        let lexed: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| lexer::lex(t, FileId(i as u32)))
            .collect();
        std::hint::black_box(&lexed);
        return;
    }
    if phase == "lex+parse" {
        let parsed: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| parser::parse(t, FileId(i as u32)))
            .collect();
        std::hint::black_box(&parsed);
        return;
    }

    let mut map = SourceMap::new();
    let mut cache = parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = load(program, &mut map, &mut cache, &mut diagnostics);
    let mut d = Diagnostics::new();
    let checked = Checker::new(&loaded, None, &mut d).run();
    if phase == "sema" {
        std::hint::black_box(&checked);
        return;
    }

    let Some(entry) = checked.entry else { return };
    let module_paths: Vec<String> = loaded.modules.iter().map(|m| m.path.clone()).collect();
    let flags = Flags::default();
    for &(target, profile) in targets {
        if phase_name(target, profile) != phase {
            continue;
        }
        let mut d = Diagnostics::new();
        let mut prog =
            monomorphize::run(&checked, module_paths.clone(), &mut d, Roots::Main(entry));
        if target.platform == Platform::Js {
            let out = actions::emit(&mut prog, &checked.tables, target, &flags, &mut d);
            std::hint::black_box(&out);
        } else {
            actions::prepare(&mut prog, target);
            let Ok(mut b) = backend::select(target, profile) else { return };
            let opts = Options { profile, target, unit_prefix: "" };
            let out = b.emit(&prog, &checked.tables, &opts);
            std::hint::black_box(out.is_ok());
        }
        return;
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn print_memory(row: &MemoryRow) {
    if row.phases.is_empty() {
        println!(
            "  {:<22} memory       no peak RSS on this platform: /usr/bin/time did not report one",
            row.label
        );
        return;
    }
    let base = row.phases.first().map_or(0, |(_, n)| *n);
    for (phase, peak) in &row.phases {
        println!(
            "  {:<22} {phase:<20} peak {:>8.1} MB   {:>6.0} B/line   {:>+8.1} MB over the corpus",
            row.label,
            mb(*peak),
            *peak as f64 / row.lines.max(1) as f64,
            mb(*peak) - mb(base)
        );
    }
}

/// Which `(target, profile)` pairs a corpus gets a lowering row for.
///
/// JavaScript always. The native triples only where the plan says so, because
/// native codegen is the expensive phase. LLVM — which is what
/// `(native, Release)` selects — only under `--set=native` and `--set=full`,
/// because a `core` run must take about the same time on every contributor's
/// machine and `backend-llvm` is not on every contributor's machine.
/// The host triple only: what a row takes beyond `REDUCED_ABOVE_LINES`, and
/// what every pinned corpus away from the anchor takes at any scale.
///
/// The cross triples are worth their seat where they cost two seconds a
/// repetition and settle whether a gap is codegen or cross-compilation; at a
/// million lines they cost twenty, and across a forty-corpus parameter sweep
/// they are three quarters of the wall time. Either way the question they answer
/// has already been answered on the anchor, over the same bytes. `--targets=`
/// overrides this, which is how somebody who wants the cross rows at scale asks
/// for them.
fn scale_targets(targets: &[Target]) -> Vec<Target> {
    let host = host_target();
    targets
        .iter()
        .copied()
        .filter(|t| t.platform == Platform::Js || Some(*t) == host)
        .collect()
}

/// The triple this machine is, as a `Target`, or `None` where the suite has no
/// name for it.
fn host_target() -> Option<Target> {
    let platform = match std::env::consts::OS {
        "macos" => Platform::Macos,
        "linux" => Platform::Linux,
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => Arch::Arm64,
        "x86_64" => Arch::X86_64,
        _ => return None,
    };
    Some(Target { platform, arch: Some(arch) })
}

fn lowering_targets(args: &Args, plan: &Plan, lines: usize) -> Vec<(Target, Profile)> {
    let mut out = Vec::new();
    let requested = if lines > REDUCED_ABOVE_LINES || !plan.cross {
        scale_targets(&args.targets)
    } else {
        args.targets.clone()
    };
    for &t in &requested {
        if t.platform == Platform::Js {
            out.push((t, Profile::Debug));
        } else if plan.native {
            out.push((t, Profile::Debug));
            if matches!(args.set, Set::Native | Set::Full) || args.shape.is_some() {
                out.push((t, Profile::Release));
            }
        }
    }
    out
}

fn do_record(args: &Args, name: &str, seed: u64, pinned: bool) {
    let root = if pinned { corpus::pinned_root() } else { corpus::root() };
    if let Err(e) = std::fs::create_dir_all(&root) {
        eprintln!("{}: {e}", root.display());
        std::process::exit(1);
    }
    let Some(profile_name) = args.shape.clone().or_else(|| {
        // `--record=mixed-10k` names the corpus; the profile is the longest
        // profile name that prefixes it, so `mixed-many-files-1k` records the
        // `mixed-many-files` profile rather than `mixed`.
        let mut best: Option<&str> = None;
        for p in generate::PROFILES {
            if name.starts_with(&format!("{p}-"))
                && best.is_none_or(|b: &str| p.len() > b.len())
            {
                best = Some(p);
            }
        }
        best.map(str::to_string)
    }) else {
        fail(&format!(
            "cannot tell which profile `{name}` is; name it with --shape=<profile>"
        ));
    };
    let (_, mut params) = generate::profile(&profile_name).unwrap_or_else(|| {
        fail(&format!("unknown profile `{profile_name}`"));
    });
    // A `--record=<profile>-<scale>` name carries the scale.
    let scale = args.scale.or_else(|| {
        let tail = name.rsplit('-').next()?;
        let (digits, mult) = match (tail.strip_suffix('k'), tail.strip_suffix('M')) {
            (Some(d), _) => (d, 1_000usize),
            (_, Some(d)) => (d, 1_000_000usize),
            _ => (tail, 1usize),
        };
        digits.parse::<usize>().ok().map(|n| n * mult)
    });
    params.lines = scale.unwrap_or(1_000);
    params.seed = seed;
    for (k, v) in &args.overrides {
        if let Err(e) = params.set(k, v) {
            fail(&e);
        }
    }

    let program = generate::program(&params);
    // Validated before it is written: a corpus that does not compile is not a
    // corpus, and `--record` is the one place that can still be fixed cheaply.
    if let Err(problems) = validate(&program) {
        eprintln!("refusing to record {name}: the generated corpus does not compile:");
        for p in problems.iter().take(10) {
            eprintln!("  {p}");
        }
        std::process::exit(1);
    }
    // A pin taken with `--targets=js` records a corpus the scale tier measures
    // through the JavaScript backend only.
    let native = args.targets.iter().any(|t| t.platform.is_native());
    let written = if pinned {
        corpus::pin(&root, name, &profile_name, &params, &program, native)
    } else {
        corpus::record(&root, name, &profile_name, &params, &program)
    };
    let verb = if pinned { "pinned" } else { "recorded" };
    match written {
        Ok(m) => println!(
            "{verb} {name}: profile {} revision {} — {} lines, {} bytes, {} modules, {} rows, \
             digest {}",
            m.profile,
            m.revision,
            m.lines,
            m.bytes,
            m.modules,
            if m.native { "native+js" } else { "js-only" },
            m.digest.chars().take(16).collect::<String>()
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn print_profiles() {
    println!("profiles (design/PERFORMANCE.md §4)");
    println!();
    println!("  {:<18} {:<10} parameters moved from the default", "profile", "family");
    println!("  {}", "-".repeat(94));
    for name in generate::PROFILES {
        let Some((family, params)) = generate::profile(name) else { continue };
        let delta = params.delta();
        let delta = if delta.is_empty() { "—".to_string() } else { delta };
        println!("  {:<18} {:<10} {}", name, family.name(), delta);
    }
    println!();
    println!("dimensions: {}", generate::KEYS.join(", "));
    println!();
    println!("  --shape=<profile> --param <key>=<value> runs a point that is not in the table.");
    let root = corpus::root();
    let saved = corpus::discover(&root);
    if !saved.is_empty() {
        println!();
        println!("checked-in corpora ({} KiB of {} KiB):", corpus::total_bytes(&root) / 1024, corpus::MAX_TOTAL_BYTES / 1024);
        for dir in saved {
            match corpus::load(&dir) {
                Ok((m, _)) => println!(
                    "  {:<24} profile {:<18} rev {}  {} lines, {} bytes",
                    m.name, m.profile, m.revision, m.lines, m.bytes
                ),
                Err(e) => println!("  {}", e),
            }
        }
    }
    let pinned = corpus::discover_pinned(&corpus::pinned_root());
    if !pinned.is_empty() {
        println!();
        println!(
            "digest-pinned corpora (--set=scale-full; regenerated per run, no source in git;\n\
             `s` marks the --set=scale sample, `n` a corpus that takes native rows):"
        );
        for path in pinned {
            match corpus::pinned_manifest(&path) {
                Ok(m) => println!(
                    "  {}{} {:<24} profile {:<18} rev {}  {} lines, {} bytes, {} modules",
                    if pinned_point(&m.name) == ANCHOR_POINT || m.lines <= REDUCED_ABOVE_LINES {
                        's'
                    } else {
                        ' '
                    },
                    if m.native { 'n' } else { ' ' },
                    m.name,
                    m.profile,
                    m.revision,
                    m.lines,
                    m.bytes,
                    m.modules
                ),
                Err(e) => println!("  {e}"),
            }
        }
    }
}

/// Which of the requested targets this toolchain can emit for at all, asked
/// once and before any timer.
fn validate_targets(targets: &[Target]) {
    println!("backends:");
    for &t in targets {
        for profile in [Profile::Debug, Profile::Release] {
            if t.platform == Platform::Js && profile == Profile::Release {
                continue;
            }
            match backend::select(t, profile) {
                Ok(b) => println!(
                    "  {:<22} {:<8} {} ({})",
                    target_name(t),
                    profile.name(),
                    b.name(),
                    b.identity()
                ),
                Err(e) => println!("  {:<22} {:<8} skipped: {e}", target_name(t), profile.name()),
            }
        }
    }
    println!();
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

struct Config {
    min_reps: usize,
    min_time: Duration,
    warmup: Duration,
    /// Repetitions before sampling starts, whatever the warmup clock says. Two
    /// everywhere except the scale tier's reduced protocol, where one call is a
    /// large fraction of a minute.
    warm_reps: usize,
    /// Whether this is the reduced protocol. Carried on the protocol rather
    /// than passed beside it, so that a row cannot be labelled with one and
    /// taken under the other.
    reduced: bool,
}

/// Above this many lines a row is taken under the reduced protocol.
///
/// `design/PERFORMANCE.md` §2 asks for at least ten repetitions and at least
/// three quarters of a second of sampling. The second half of that is never the
/// binding one at this scale — one repetition of anything at a million lines is
/// already past 750 ms — and the first half would put native lowering at
/// roughly twenty seconds a repetition, which is four minutes of one row. So
/// the deviation is exactly one rule: **at least three repetitions instead of
/// at least ten**, plus one warmup call instead of two. Rows taken under it say
/// so, in the table and in `--json`, because a deviation nobody can see in the
/// output is a deviation nobody can account for.
const REDUCED_ABOVE_LINES: usize = 500_000;
const REDUCED_MIN_REPS: usize = 3;

/// The protocol for a corpus of this size.
fn protocol_for(base: &Config, lines: usize) -> Config {
    if lines <= REDUCED_ABOVE_LINES || base.min_reps <= REDUCED_MIN_REPS {
        return Config {
            min_reps: base.min_reps,
            min_time: base.min_time,
            warmup: base.warmup,
            warm_reps: base.warm_reps,
            reduced: false,
        };
    }
    Config {
        min_reps: REDUCED_MIN_REPS,
        // Unchanged: the sampling-time floor is the rule that still buys
        // something here, and every cheap phase at this scale meets it with
        // repetitions to spare.
        min_time: base.min_time,
        warmup: base.warmup,
        warm_reps: 1,
        reduced: true,
    }
}

/// One measured phase at one scale and shape.
struct Row {
    label: String,
    phase: String,
    /// Realistic or stress. The goal column is printed only for the former,
    /// which is `design/PERFORMANCE.md` §3's rule made structural.
    family: Family,
    /// `generated` or `saved`.
    source: &'static str,
    /// The saved corpus's `revision`, or 0 for a generated one. A tracking
    /// script joins on it so that a re-record shows up as a break in the series
    /// rather than as a step in it.
    corpus_revision: u32,
    lines: usize,
    bytes: usize,
    tokens: usize,
    modules: usize,
    median: Duration,
    /// Median absolute deviation, as a fraction of the median. Reported rather
    /// than a standard deviation because a benchmark's distribution is
    /// one-sided — the machine can only ever make a run *slower* — so a
    /// symmetric spread is the wrong summary and an outlier-sensitive one is
    /// worse.
    dispersion: f64,
    fastest: Duration,
    reps: usize,
    /// Whether the row was taken under the scale tier's reduced protocol
    /// (`REDUCED_MIN_REPS` repetitions instead of ten). Printed beside the row
    /// and carried in `--json`, so a number taken under a deviation is never
    /// quoted as one taken under the rule.
    reduced: bool,
    /// The phase's cost on the floor program, subtracted to give the net rate.
    floor: Duration,
    target: f64,
}

/// A row this binary could not take, and why.
///
/// Printed rather than silently omitted, and carried in `--json` under its own
/// key rather than mixed into `rows`, so that a consumer of the row schema is
/// unaffected and a reader of the report still learns which rows are missing.
/// There is no `#[cfg]` anywhere in this file: the report says which rows this
/// binary could not take rather than changing shape depending on how it was
/// compiled.
struct Skipped {
    label: String,
    phase: String,
    reason: String,
}

impl Row {
    fn rate(&self) -> f64 {
        self.lines as f64 / self.median.as_secs_f64()
    }

    /// The rate with the fixed standard-library cost taken out. For lexing and
    /// parsing the two are the same by construction — only generated text is
    /// fed to them — and for semantic analysis they are very different at
    /// small scales.
    fn net_rate(&self) -> f64 {
        let net = self.median.as_secs_f64() - self.floor.as_secs_f64();
        if net <= 0.0 {
            f64::INFINITY
        } else {
            self.lines as f64 / net
        }
    }

    fn bytes_rate(&self) -> f64 {
        self.bytes as f64 / self.median.as_secs_f64()
    }

    /// Tokens per second. Reported beside the other two because the three
    /// diverge, and the divergence is the signal: a line rate is hostage to
    /// how dense the source is, a byte rate to how long its identifiers are,
    /// and only the token rate tracks what the lexer's inner loop actually
    /// does. Carbon reports all three for exactly this reason.
    fn tokens_rate(&self) -> f64 {
        self.tokens as f64 / self.median.as_secs_f64()
    }

    /// How far short of the goal, as a multiplier. 1.0 is on target.
    fn gap(&self) -> f64 {
        self.target / self.rate()
    }
}

/// Warm up, then repeat until both the repetition floor and the time floor are
/// met, and report the median with its dispersion.
///
/// The warmup is not a formality. The first call through a phase pays for page
/// faults on the freshly-mapped heap, for the branch predictor's ignorance,
/// and — on the machines that do it — for the CPU still being at its idle
/// frequency. None of those is a property of the compiler, and all of them
/// land on the first sample.
fn bench<F: FnMut()>(cfg: &Config, mut f: F) -> (Duration, f64, Duration, usize) {
    let warm_start = Instant::now();
    let mut warm_reps = 0;
    while warm_start.elapsed() < cfg.warmup || warm_reps < cfg.warm_reps {
        f();
        warm_reps += 1;
        // A phase that takes longer than the whole warmup budget on its first
        // call has warmed up as much as it is going to.
        if warm_reps >= cfg.warm_reps && warm_start.elapsed() >= cfg.warmup {
            break;
        }
        if warm_reps > 200 {
            break;
        }
    }

    let mut samples: Vec<Duration> = Vec::new();
    let start = Instant::now();
    while samples.len() < cfg.min_reps || start.elapsed() < cfg.min_time {
        let t = Instant::now();
        f();
        samples.push(t.elapsed());
        if samples.len() >= 10_000 {
            break;
        }
    }

    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    let mut devs: Vec<Duration> = samples
        .iter()
        .map(|s| if *s > median { *s - median } else { median - *s })
        .collect();
    devs.sort_unstable();
    let mad = devs[devs.len() / 2];
    let dispersion =
        if median.as_secs_f64() > 0.0 { mad.as_secs_f64() / median.as_secs_f64() } else { 0.0 };
    (median, dispersion, samples[0], samples.len())
}

/// The phases at one corpus, plus one lowering row per requested target.
fn measure(
    program: &Program,
    plan: &Plan,
    source_kind: &'static str,
    corpus_revision: u32,
    cfg: &Config,
    floor: &Floor,
    targets: &[(Target, Profile)],
) -> (Vec<Row>, Vec<Skipped>) {
    let label = &plan.label;
    let lines = program.lines();
    let bytes = program.bytes();
    let modules = program.modules.len();
    // Counted by lexing for real, outside any timer, rather than estimated.
    let tokens: usize =
        program.modules.iter().map(|m| lexer::lex(&m.text, FileId(0)).tokens.len()).sum();
    let mut rows = Vec::new();
    let mut skips = Vec::new();

    let row = |phase: String, median, dispersion, fastest, reps, floor, target| Row {
        label: label.clone(),
        phase,
        family: plan.family,
        source: source_kind,
        corpus_revision,
        lines,
        bytes,
        tokens,
        modules,
        median,
        dispersion,
        fastest,
        reps,
        reduced: cfg.reduced,
        floor,
        target,
    };

    // -- lex ----------------------------------------------------------------
    //
    // Straight over the source text, with no file loaded from disk: a saved
    // corpus was read at work-list construction and a generated one never
    // touched a filesystem, so nothing here is measuring one.
    let texts: Vec<&str> = program.modules.iter().map(|m| m.text.as_str()).collect();
    let (median, dispersion, fastest, reps) = bench(cfg, || {
        for (i, t) in texts.iter().enumerate() {
            let lexed = lexer::lex(t, FileId(i as u32));
            std::hint::black_box(&lexed);
        }
    });
    rows.push(row(
        "lex".to_string(),
        median,
        dispersion,
        fastest,
        reps,
        Duration::ZERO,
        TARGET_PARSE,
    ));

    // -- lex + parse --------------------------------------------------------
    //
    // `parser::parse` lexes as its first act, so this is the whole of the
    // first goal's budget in one call. The lex row above is the free
    // subdivision of it.
    let (median, dispersion, fastest, reps) = bench(cfg, || {
        for (i, t) in texts.iter().enumerate() {
            let parsed = parser::parse(t, FileId(i as u32));
            std::hint::black_box(&parsed);
        }
    });
    rows.push(row(
        "lex+parse".to_string(),
        median,
        dispersion,
        fastest,
        reps,
        Duration::ZERO,
        TARGET_PARSE,
    ));

    // -- sema ---------------------------------------------------------------
    //
    // `Loaded` is built once, outside the timer, so parsing is not being paid
    // for again here. The checker takes it by reference and hands back a fresh
    // `Checked` each time, so no repetition sees another's work.
    let mut map = SourceMap::new();
    let mut cache = parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = load(program, &mut map, &mut cache, &mut diagnostics);
    let (median, dispersion, fastest, reps) = bench(cfg, || {
        let mut d = Diagnostics::new();
        let checked = Checker::new(&loaded, None, &mut d).run();
        std::hint::black_box(&checked);
    });
    rows.push(row(
        "sema".to_string(),
        median,
        dispersion,
        fastest,
        reps,
        floor.sema,
        TARGET_SEMA,
    ));

    // -- lowering, one row per target ---------------------------------------
    let mut d = Diagnostics::new();
    let checked = Checker::new(&loaded, None, &mut d).run();
    let module_paths: Vec<String> = loaded.modules.iter().map(|m| m.path.clone()).collect();
    let entry = checked.entry.expect("the corpus exports `main`");
    let flags = Flags::default();

    for &(target, profile) in targets {
        let phase = phase_name(target, profile);
        if target.platform == Platform::Js {
            // JavaScript through `actions::emit`, which is the same call
            // `buri build --output=js` makes, `prepare` included.
            let (median, dispersion, fastest, reps) = bench(cfg, || {
                let mut d = Diagnostics::new();
                let mut prog =
                    monomorphize::run(&checked, module_paths.clone(), &mut d, Roots::Main(entry));
                let out = actions::emit(&mut prog, &checked.tables, target, &flags, &mut d);
                std::hint::black_box(&out);
            });
            rows.push(row(phase, median, dispersion, fastest, reps, floor.lower, TARGET_LOWER));
            continue;
        }

        // Everything that can honestly stop a native row is asked *before* any
        // timer, which is "validate before measure" extended to the backend: a
        // backend that would have failed must not be measured failing.
        let mut probe = {
            let mut d = Diagnostics::new();
            monomorphize::run(&checked, module_paths.clone(), &mut d, Roots::Main(entry))
        };
        actions::prepare(&mut probe, target);
        let mut selected = match backend::select(target, profile) {
            Ok(b) => b,
            Err(reason) => {
                skips.push(Skipped { label: label.clone(), phase, reason });
                continue;
            }
        };
        let missing = selected.missing_intrinsics(&probe, &checked.tables);
        if !missing.is_empty() {
            skips.push(Skipped {
                label: label.clone(),
                phase,
                reason: format!("the backend has no body for {}", missing.join(", ")),
            });
            continue;
        }
        let opts = Options { profile, target, unit_prefix: "" };
        if let Err(diagnostics) = selected.emit(&probe, &checked.tables, &opts) {
            let reason = diagnostics
                .items
                .first()
                .map_or_else(|| String::from("emission failed"), |d| d.message.clone());
            skips.push(Skipped { label: label.clone(), phase, reason });
            continue;
        }
        drop(probe);

        // Each repetition rebuilds the monomorphized program, because
        // `prepare` mutates in place and is not idempotent. Monomorphization
        // is therefore inside every lowering row's number, JS and native
        // alike, which is what keeps the rows comparable; `--split` subtracts
        // it.
        let (median, dispersion, fastest, reps) = bench(cfg, || {
            let mut d = Diagnostics::new();
            let mut prog =
                monomorphize::run(&checked, module_paths.clone(), &mut d, Roots::Main(entry));
            actions::prepare(&mut prog, target);
            let mut b = backend::select(target, profile).expect("checked above");
            let opts = Options { profile, target, unit_prefix: "" };
            let out = b.emit(&prog, &checked.tables, &opts);
            std::hint::black_box(&out);
        });
        rows.push(row(phase, median, dispersion, fastest, reps, Duration::ZERO, TARGET_LOWER));
    }

    (rows, skips)
}

/// The sub-phases of lowering, timed separately.
///
/// Not part of the table: the goal is stated over the whole of lowering, and
/// five more rows per target would bury it. This is what a bottleneck analysis
/// reads and what a profiler run is checked against. To stderr, so that
/// `--json --split` still emits one parseable document on stdout.
///
/// Every figure but the first is a difference of two measured prefixes, which
/// is the only honest way to time a pass that mutates its input: `prepare`
/// is not idempotent, so a repetition has to rebuild, and rebuilding costs the
/// prefix.
fn split_lowering(program: &Program, label: &str, cfg: &Config, targets: &[(Target, Profile)]) {
    let mut map = SourceMap::new();
    let mut cache = parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = load(program, &mut map, &mut cache, &mut diagnostics);
    let checked = &Checker::new(&loaded, None, &mut diagnostics).run();
    let module_paths: Vec<String> = loaded.modules.iter().map(|m| m.path.clone()).collect();
    let Some(entry) = checked.entry else { return };
    let flags = Flags::default();

    let build = || {
        let mut d = Diagnostics::new();
        monomorphize::run(checked, module_paths.clone(), &mut d, Roots::Main(entry))
    };
    let (mono, _, _, _) = bench(cfg, || {
        std::hint::black_box(build());
    });

    for &(target, profile) in targets {
        let phase = phase_name(target, profile);
        let (mono_a, _, _, _) = bench(cfg, || {
            let mut p = build();
            middle::run(&mut p, &middle::Options::default());
            std::hint::black_box(&p);
        });
        let layer_a = mono_a.saturating_sub(mono);

        if target.platform == Platform::Js {
            let (all, _, _, _) = bench(cfg, || {
                let mut d = Diagnostics::new();
                let mut p = build();
                let out = actions::emit(&mut p, &checked.tables, target, &flags, &mut d);
                std::hint::black_box(&out);
            });
            eprintln!(
                "  {label:<22} {phase:<24} mono {:>8.2}  middle-A {:>8.2}  emit {:>8.2} ms",
                ms(mono),
                ms(layer_a),
                ms(all.saturating_sub(mono_a))
            );
            continue;
        }

        let (mono_native, _, _, _) = bench(cfg, || {
            let mut p = build();
            middle::run(&mut p, &middle::Options::default());
            middle::native(&mut p);
            std::hint::black_box(&p);
        });
        let (mono_ir, _, _, _) = bench(cfg, || {
            let mut p = build();
            middle::run(&mut p, &middle::Options::default());
            middle::native(&mut p);
            std::hint::black_box(lower::run(&p, &checked.tables));
        });
        let Ok(_) = backend::select(target, profile) else { continue };
        let (all, _, _, _) = bench(cfg, || {
            let mut p = build();
            actions::prepare(&mut p, target);
            let mut b = backend::select(target, profile).expect("checked above");
            let opts = Options { profile, target, unit_prefix: "" };
            std::hint::black_box(b.emit(&p, &checked.tables, &opts).is_ok());
        });
        eprintln!(
            "  {label:<22} {phase:<24} mono {:>8.2}  middle-A {:>8.2}  middle-native {:>8.2}  \
             lower(IR) {:>8.2}  emit {:>8.2} ms",
            ms(mono),
            ms(layer_a),
            ms(mono_native.saturating_sub(mono_a)),
            ms(mono_ir.saturating_sub(mono_native)),
            ms(all.saturating_sub(mono_ir))
        );
    }
}

// ---------------------------------------------------------------------------
// Driving the real compiler
// ---------------------------------------------------------------------------

/// Loads the program the way a build would: the modules every compilation
/// needs, then the corpus, leaf first so that each module's imports are already
/// present when the loader reaches them.
///
/// `Loader::new(None, ..)` — no workspace — is what keeps the corpus in
/// memory. A `//bench/...` import resolves through `by_path` before anything
/// consults a repository, so no file is written and no `BUILD.buri` is needed.
fn load(
    program: &Program,
    map: &mut SourceMap,
    cache: &mut parser::Cache,
    diagnostics: &mut Diagnostics,
) -> Loaded {
    let mut loader = Loader::new(None, map, diagnostics, cache);
    loader.load_builtin_modules();
    let last = program.modules.len().saturating_sub(1);
    for (i, m) in program.modules.iter().enumerate() {
        let role = if i == last { Role::Entry } else { Role::Source };
        loader.load_source(&m.path, role, m.text.clone());
    }
    loader.finish()
}

/// Compiles the corpus through the real front end and reports every error.
fn validate(program: &Program) -> Result<(), Vec<String>> {
    let mut map = SourceMap::new();
    let mut cache = parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = load(program, &mut map, &mut cache, &mut diagnostics);
    let checked = Checker::new(&loaded, None, &mut diagnostics).run();
    let mut problems: Vec<String> = diagnostics
        .items
        .iter()
        .filter(|d| matches!(d.severity, buri::diagnostics::Severity::Error))
        .map(|d| {
            let file = map.name(d.span.file);
            format!("{file}: {}", d.message)
        })
        .collect();
    if checked.entry.is_none() {
        problems.push("the corpus exports no `main`".to_string());
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(())
}

/// How much of the corpus survives to the backend.
///
/// This is the guard against the trap the suite's first run fell into: a `main`
/// that reaches only a handful of the generated functions leaves the rest dead,
/// monomorphization never requests them, and lowering reports a throughput for
/// a program that was 95% discarded — a rate *faster than lexing*, which is the
/// tell. Reported by `--validate` beside the line count, for the generated and
/// the saved corpora alike, so that a corpus which stops being reachable is
/// visible rather than merely fast.
fn reach(program: &Program) -> (usize, usize) {
    let mut map = SourceMap::new();
    let mut cache = parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = load(program, &mut map, &mut cache, &mut diagnostics);
    let checked = Checker::new(&loaded, None, &mut diagnostics).run();
    let Some(entry) = checked.entry else { return (0, 0) };
    let module_paths: Vec<String> = loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut prog = monomorphize::run(&checked, module_paths, &mut diagnostics, Roots::Main(entry));
    let funcs = prog.funcs.len();
    let flags = Flags::default();
    let target = Target { platform: Platform::Js, arch: None };
    let emitted = actions::emit(&mut prog, &checked.tables, target, &flags, &mut diagnostics)
        .map(|s| s.len())
        .unwrap_or(0);
    (funcs, emitted)
}

/// What every measurement pays before it reaches a line of generated source.
struct Floor {
    lex: Duration,
    parse: Duration,
    sema: Duration,
    lower: Duration,
}

impl Floor {
    fn zero() -> Floor {
        Floor {
            lex: Duration::ZERO,
            parse: Duration::ZERO,
            sema: Duration::ZERO,
            lower: Duration::ZERO,
        }
    }
}

fn floor_costs() -> Floor {
    let cfg = Config {
        min_reps: 7,
        min_time: Duration::from_millis(400),
        warmup: Duration::from_millis(200),
        warm_reps: 2,
        reduced: false,
    };
    let program = Program {
        modules: vec![generate::Module {
            path: "//bench/main.buri".to_string(),
            text: FLOOR.to_string(),
        }],
    };
    let (lex, ..) = bench(&cfg, || {
        std::hint::black_box(lexer::lex(FLOOR, FileId(0)));
    });
    let (parse, ..) = bench(&cfg, || {
        std::hint::black_box(parser::parse(FLOOR, FileId(0)));
    });

    let mut map = SourceMap::new();
    let mut cache = parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = load(&program, &mut map, &mut cache, &mut diagnostics);
    let (sema, ..) = bench(&cfg, || {
        let mut d = Diagnostics::new();
        std::hint::black_box(Checker::new(&loaded, None, &mut d).run());
    });

    let mut d = Diagnostics::new();
    let checked = Checker::new(&loaded, None, &mut d).run();
    let module_paths: Vec<String> = loaded.modules.iter().map(|m| m.path.clone()).collect();
    let flags = Flags::default();
    let target = Target { platform: Platform::Js, arch: None };
    let lower = match checked.entry {
        Some(entry) => {
            let (t, ..) = bench(&cfg, || {
                let mut d = Diagnostics::new();
                let mut prog =
                    monomorphize::run(&checked, module_paths.clone(), &mut d, Roots::Main(entry));
                let out = actions::emit(&mut prog, &checked.tables, target, &flags, &mut d);
                std::hint::black_box(&out);
            });
            t
        }
        None => Duration::ZERO,
    };

    Floor { lex, parse, sema, lower }
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

fn human(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

fn rate(r: f64) -> String {
    if !r.is_finite() {
        return "     —   ".to_string();
    }
    if r >= 1_000_000.0 {
        format!("{:>7.2}M/s", r / 1_000_000.0)
    } else if r >= 1_000.0 {
        format!("{:>7.1}k/s", r / 1_000.0)
    } else {
        format!("{r:>7.0} /s")
    }
}

fn header(cfg: &Config, scales: &[usize], set: Set, targets: &[Target]) {
    println!("buri compiler throughput");
    println!("========================");
    println!();
    println!("  host        {} {}", std::env::consts::OS, std::env::consts::ARCH);
    println!(
        "  cores       {}",
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0)
    );
    println!("  toolchain   buri {}", env!("CARGO_PKG_VERSION"));
    println!(
        "  build       {}",
        if cfg!(debug_assertions) {
            "DEBUG — these numbers mean nothing; run `cargo bench`"
        } else {
            "release (opt-level 3, lto, 1 codegen unit)"
        }
    );
    println!("  seed        {SEED:#018x}");
    println!("  generator   revision {}", generate::GENERATOR_REVISION);
    println!(
        "  protocol    warm {:.0} ms, then >= {} reps and >= {:.0} ms; median of samples, \
         dispersion is MAD/median",
        ms(cfg.warmup),
        cfg.min_reps,
        ms(cfg.min_time)
    );
    println!("  set         {}", set.name());
    if matches!(set, Set::Scale | Set::ScaleFull) {
        // The scale tier's corpora carry their own line counts in their
        // manifests, so the generated scales are not what this run measures.
        println!("  scales      the pinned manifests (--list)");
        if set == Set::Scale {
            println!(
                "  sample      every pinned corpus at or below {} lines, plus `{}` above it; \
                 --set=scale-full runs all of them",
                commas(REDUCED_ABOVE_LINES as f64),
                ANCHOR_POINT
            );
        }
        println!(
            "  deviation   above {} lines: >= {} repetitions instead of >= 10, one warmup call \
             instead of two, and the host triple only for the native rows. Affected rows say so.",
            commas(REDUCED_ABOVE_LINES as f64),
            REDUCED_MIN_REPS
        );
    } else {
        println!(
            "  scales      {}",
            scales.iter().map(|s| human(*s)).collect::<Vec<_>>().join(", ")
        );
    }
    println!(
        "  targets     {}",
        targets.iter().map(|t| target_name(*t)).collect::<Vec<_>>().join(", ")
    );
    println!("  a line      a non-blank line of source, comments included");
    println!();
    println!(
        "  {:<22} {:<20} {:>7} {:>11} {:>11} {:>11} {:>8} {:>7}",
        "corpus", "phase", "lines", "median", "lines/s", "bytes/s", "vs goal", "±MAD"
    );
    println!("  {}", "-".repeat(104));
}

fn print_row(row: &Row) {
    // The goal column is printed only for the realistic family. A stress shape
    // is built to break one pass, so quoting it against a throughput goal
    // stated over representative source would be quoting a number against a
    // question it was never asked.
    let verdict = match row.family {
        Family::Stress => "   —   ".to_string(),
        Family::Realistic if row.rate() >= row.target => "  MET  ".to_string(),
        Family::Realistic => format!("{:>6.0}x", row.gap()),
    };
    // The deviation rides on the row it applies to, not in a footnote: §2's
    // ten-repetition rule is what the dispersion column is trustworthy under,
    // and a row taken over three has to carry that with it.
    let protocol = if row.reduced { format!("  [{} reps — reduced protocol]", row.reps) } else { String::new() };
    println!(
        "  {:<22} {:<20} {:>7} {:>9.2} ms {} {} {:>8} {:>6.1}%{protocol}",
        row.label,
        row.phase,
        row.lines,
        ms(row.median),
        rate(row.rate()),
        rate(row.bytes_rate()),
        verdict,
        row.dispersion * 100.0
    );
}

fn footer(rows: &[Row], skipped: &[Skipped]) {
    println!("  {}", "-".repeat(104));
    println!();
    println!("goals (design/PERFORMANCE.md), quoted against the realistic family only:");
    println!("  lex+parse   {:>12}  lines/s", commas(TARGET_PARSE));
    println!("  sema        {:>12}  lines/s", commas(TARGET_SEMA));
    println!("  lower       {:>12}  lines/s", commas(TARGET_LOWER));
    println!();

    for family in [Family::Realistic, Family::Stress] {
        let mine: Vec<&Row> = rows.iter().filter(|r| r.family == family).collect();
        if mine.is_empty() {
            continue;
        }
        println!("{} family, worst gap per phase:", family.name());
        let mut phases: Vec<&str> = Vec::new();
        for r in &mine {
            if !phases.contains(&r.phase.as_str()) {
                phases.push(r.phase.as_str());
            }
        }
        for phase in phases {
            let mut worst: Option<&&Row> = None;
            for r in mine.iter().filter(|r| r.phase == phase) {
                if worst.is_none_or(|w| r.rate() < w.rate()) {
                    worst = Some(r);
                }
            }
            if let Some(w) = worst {
                println!(
                    "  {:<20} {:<22} {} ({:.1}x off)",
                    phase,
                    w.label,
                    rate(w.rate()),
                    w.gap()
                );
            }
        }
        println!();
    }

    println!("net of the prelude floor (semantic analysis only; see PERFORMANCE.md §3):");
    for row in rows.iter().filter(|r| r.phase == "sema") {
        println!("  {:<22} {}", row.label, rate(row.net_rate()));
    }
    println!();
    println!("token rate, where the line rate is the misleading one (lex and parse):");
    for row in rows.iter().filter(|r| r.phase == "lex" || r.phase == "lex+parse") {
        println!(
            "  {:<22} {:<20} {} over {} tokens",
            row.label,
            row.phase,
            rate(row.tokens_rate()),
            row.tokens
        );
    }
    println!();
    println!("fastest sample, for the least-noise reading of the same thing:");
    for row in rows {
        println!(
            "  {:<22} {:<20} {:>9.2} ms over {} reps",
            row.label,
            row.phase,
            ms(row.fastest),
            row.reps
        );
    }
    if !skipped.is_empty() {
        println!();
        println!("rows this binary could not take:");
        for s in skipped {
            println!("  {:<22} {:<20} {}", s.label, s.phase, s.reason);
        }
    }
}

fn commas(x: f64) -> String {
    let s = format!("{x:.0}");
    let mut out = String::new();
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// One JSON object per line is deliberately *not* what this prints: a single
/// document is what a tracking script wants, and the toolchain already has a
/// JSON writer it does not need a dependency for.
///
/// Every key the previous version emitted is still here and still means the
/// same thing. `family`, `source` and `corpus_revision` are added to each row,
/// and skipped rows go in their own top-level array rather than into `rows`,
/// so that a consumer of today's schema parses tomorrow's document unchanged.
fn print_json(
    rows: &[Row],
    skipped: &[Skipped],
    floor: &Floor,
    calibrations: &[(String, usize, usize, Vec<calibrate::Ceiling>)],
    allocs: &[AllocRow],
    memory: &[MemoryRow],
) {
    println!("{{");
    println!("  \"host\": \"{}-{}\",", std::env::consts::OS, std::env::consts::ARCH);
    println!("  \"toolchain\": \"{}\",", env!("CARGO_PKG_VERSION"));
    println!("  \"seed\": \"{SEED:#018x}\",");
    println!("  \"generator_revision\": {},", generate::GENERATOR_REVISION);
    println!("  \"debug_build\": {},", cfg!(debug_assertions));
    println!(
        "  \"floor_ms\": {{ \"lex\": {:.4}, \"parse\": {:.4}, \"sema\": {:.4}, \"lower\": {:.4} }},",
        ms(floor.lex),
        ms(floor.parse),
        ms(floor.sema),
        ms(floor.lower)
    );
    println!("  \"rows\": [");
    for (i, row) in rows.iter().enumerate() {
        let comma = if i + 1 == rows.len() { "" } else { "," };
        println!(
            "    {{ \"corpus\": \"{}\", \"phase\": \"{}\", \"family\": \"{}\", \
             \"source\": \"{}\", \"corpus_revision\": {}, \"lines\": {}, \"bytes\": {}, \
             \"tokens\": {}, \"modules\": {}, \"median_ms\": {:.4}, \"fastest_ms\": {:.4}, \
             \"reps\": {}, \"dispersion\": {:.4}, \"lines_per_sec\": {:.1}, \
             \"net_lines_per_sec\": {:.1}, \"bytes_per_sec\": {:.1}, \"tokens_per_sec\": {:.1}, \
             \"target_lines_per_sec\": {:.0}, \"gap\": {:.2}, \"protocol\": \"{}\" }}{comma}",
            row.label,
            row.phase,
            row.family.name(),
            row.source,
            row.corpus_revision,
            row.lines,
            row.bytes,
            row.tokens,
            row.modules,
            ms(row.median),
            ms(row.fastest),
            row.reps,
            row.dispersion,
            row.rate(),
            if row.net_rate().is_finite() { row.net_rate() } else { 0.0 },
            row.bytes_rate(),
            row.tokens_rate(),
            row.target,
            row.gap(),
            if row.reduced { "reduced" } else { "standard" }
        );
    }
    println!("  ],");
    println!("  \"skipped\": [");
    for (i, s) in skipped.iter().enumerate() {
        let comma = if i + 1 == skipped.len() { "" } else { "," };
        println!(
            "    {{ \"corpus\": \"{}\", \"phase\": \"{}\", \"reason\": \"{}\" }}{comma}",
            s.label,
            s.phase,
            s.reason.replace('\\', "\\\\").replace('"', "\\\"")
        );
    }
    println!("  ],");
    // Additive keys: a consumer of `rows` is unaffected, and both are empty
    // unless `--calibrate` or `--alloc` asked for them.
    println!("  \"calibration\": [");
    for (i, (label, lines, tokens, ceilings)) in calibrations.iter().enumerate() {
        let comma = if i + 1 == calibrations.len() { "" } else { "," };
        let (sum, _) = calibrate::verdict(ceilings, *lines);
        print!(
            "    {{ \"corpus\": \"{label}\", \"lines\": {lines}, \"tokens\": {tokens}, \
             \"front_end_ceiling_ns_per_line\": {sum:.2}, \"rows\": ["
        );
        for (j, r) in ceilings.iter().enumerate() {
            let c = if j + 1 == ceilings.len() { "" } else { ", " };
            print!(
                "{{ \"name\": \"{}\", \"median_ms\": {:.4}, \"fastest_ms\": {:.4}, \
                 \"dispersion\": {:.4}, \"units\": {}, \"unit\": \"{}\", \
                 \"ns_per_line\": {:.3}, \"ns_per_unit\": {:.4} }}{c}",
                r.name,
                ms(r.median),
                ms(r.fastest),
                r.dispersion,
                r.units,
                r.unit,
                r.ns_per_line(*lines),
                r.ns_per_unit()
            );
        }
        println!("] }}{comma}");
    }
    println!("  ],");
    println!("  \"allocations\": [");
    for (i, a) in allocs.iter().enumerate() {
        let comma = if i + 1 == allocs.len() { "" } else { "," };
        print!("    {{ \"corpus\": \"{}\", \"lines\": {}, \"tokens\": {}, \"phases\": [", a.label, a.lines, a.tokens);
        for (j, (phase, n)) in a.phases.iter().enumerate() {
            let c = if j + 1 == a.phases.len() { "" } else { ", " };
            print!(
                "{{ \"phase\": \"{phase}\", \"allocations\": {n}, \
                 \"per_1000_lines\": {:.1}, \"per_token\": {:.4} }}{c}",
                *n as f64 * 1000.0 / a.lines.max(1) as f64,
                *n as f64 / a.tokens.max(1) as f64
            );
        }
        println!("] }}{comma}");
    }
    println!("  ],");
    println!("  \"memory\": [");
    for (i, m) in memory.iter().enumerate() {
        let comma = if i + 1 == memory.len() { "" } else { "," };
        print!(
            "    {{ \"corpus\": \"{}\", \"lines\": {}, \"phases\": [",
            m.label, m.lines
        );
        for (j, (phase, peak)) in m.phases.iter().enumerate() {
            let c = if j + 1 == m.phases.len() { "" } else { ", " };
            print!(
                "{{ \"phase\": \"{phase}\", \"peak_rss_bytes\": {peak}, \
                 \"bytes_per_line\": {:.1} }}{c}",
                *peak as f64 / m.lines.max(1) as f64
            );
        }
        println!("] }}{comma}");
    }
    println!("  ]");
    println!("}}");
}
