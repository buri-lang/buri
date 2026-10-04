//! `buri test`.
//!
//! Tests are ordinary build actions. Because there is no mutable global state,
//! no ambient I/O, and no observable ordering, the runner is free to shard
//! across processes and to run a suite's tests in any order. Nothing about a
//! suite's result may depend on that freedom, so the runner does not offer a
//! knob to turn it off — there is no `--shuffle`, and a suite that would need
//! one is a suite with a dependency it has not admitted to
//! (TESTING.md, "Running").
//!
//! `test` is the only action that leaves this process, so it is the only one
//! whose spawn has to be made deterministic: `build/spawn.rs` gives it an
//! explicit environment and a clock frozen at `1970-01-01T00:00:00Z`. That is
//! about determinism rather than confinement — what keeps a suite from reaching
//! the machine is that a test source is never handed a host: only an entry is,
//! and only a `platform.buri` may import `platform/host`. Its capabilities are
//! fakes this runner injects.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a failure, a diff, and the summary line are this command's output; \
              every diagnostic about the code still leaves through `Session::emit`"
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here counts cases the runner already produced, walks \
              offsets inside a string that bounds them, and juggles two line \
              vectors against a common prefix computed from both — all bounded by \
              a result that already exists in memory"
)]

use crate::build::actions;
use crate::build::buildfile::Platform;
use crate::build::session::{self, Session};
use crate::build::workspace::TargetId;
use crate::commands::arguments;
use crate::commands::watch;
use crate::compiler::backend::js::javascript;
use crate::compiler::modules::Unit;
use crate::compiler::middle::monomorphize;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use crate::json::Value;
use std::io::Write;
use std::time::{Duration, Instant};

/// The JavaScript runtime `buri run` and the test runner execute artifacts
/// with. `bun` unless `BURI_JS` says otherwise.
pub fn js_runtime() -> String {
    std::env::var("BURI_JS").unwrap_or_else(|_| "bun".to_string())
}

/// Whether a result was produced by this run or served from the cache.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Provenance {
    Ran,
    Cache,
}

/// The two sides of a failed comparison.
///
/// One value rather than two `Option`s: with two, `actual: Some` and
/// `expected: None` is representable, and a half diff is one there is nothing
/// to compare against.
struct Diff {
    actual: String,
    expected: String,
}

/// What one test did.
///
/// A message and a diff belong to a failure and to nothing else, so they live
/// inside the failing variant. A passing test carrying a diff is no longer a
/// value anything can build.
///
/// `order` is the same argument a third time: the sentence naming the order the
/// tasks completed in, and the seed that replays it, is something only a failure
/// has. A block that scheduled nothing — which is almost every block — has
/// `None`, and a passing one has nowhere to put it.
enum Verdict {
    Passed,
    Failed { message: String, diff: Option<Diff>, order: Option<String> },
}

struct Case {
    provenance: Provenance,
    name: String,
    module: String,
    /// `None` when the runner reported no span for the test — which is a
    /// different thing from a location that is the empty string.
    location: Option<String>,
    verdict: Verdict,
}

/// What one suite produced: the cases that ran, and the ones a `--filter` left
/// out. A skipped test is reported rather than silently absent, so a filter
/// that matches nothing looks different from a suite that holds nothing.
#[derive(Default)]
struct Outcome {
    cases: Vec<Case>,
    skipped: usize,
}

/// Where a run's platform came from.
///
/// Not a fallback rule any more — nothing gives way — but the two are still
/// different questions, and two sentences answer them. A platform the suite
/// wrote down, or the command line named, is a *request*, and a request this
/// toolchain cannot serve is refused in the words that name the request: drop
/// it from `test.platforms`. A platform nobody asked for is the *default*, and
/// a default this toolchain cannot serve is refused in the words that name the
/// toolchain: here is what a native run needs, and here is how to ask for
/// JavaScript instead.
///
/// **Neither of them re-routes a suite.** Running a suite on a backend nobody
/// chose is how a named gap becomes a wrong answer: the suite passes, and what
/// it proves is that the other backend agrees with itself.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chosen {
    /// `test { platforms: [...] }`, or `--output=`.
    Asked,
    /// Nobody said, so the host's own platform did.
    Default,
}

/// Where a pass's own output goes.
///
/// Without `--watch` a failure is printed the moment it is known, interleaved
/// with whatever `--explain` is streaming, which is what `buri test` has always
/// done and what the recorded cases hold it to. Under `--watch` the same text
/// is held instead, because the loop cannot know whether a pass is worth a run
/// separator until the pass has finished: a pass served entirely from the cache
/// prints nothing at all (BUILD-AND-WATCH.md §4.4).
enum Out {
    Direct,
    Held(String),
}

impl Out {
    fn line(&mut self, text: &str) {
        match self {
            Out::Direct => println!("{text}"),
            Out::Held(buffer) => {
                buffer.push_str(text);
                buffer.push('\n');
            }
        }
    }

    fn blank(&mut self) {
        self.line("");
    }

    fn take(self) -> String {
        match self {
            Out::Direct => String::new(),
            Out::Held(buffer) => buffer,
        }
    }
}

pub fn command_test(args: &arguments::Args) -> i32 {
    // A selector naming no platform is the thing you asked *with* being wrong,
    // and it is refused here rather than per suite: a run that silently used
    // the default because the selector matched nothing would report a pass for
    // a backend nobody chose. `buri build` refuses the same mistake in the same
    // shape, against the outputs a target declares; here the set is closed, so
    // the fix can name all of it.
    if args.flags.output.is_some() && selected_platform(&args.flags).is_none() {
        let selector = args.flags.output.as_deref().unwrap_or_default();
        eprintln!("error: no backend matches `--output={selector}`");
        eprintln!("  = a suite runs on one of: native, js");
        eprintln!("  = fix: name one of them, as in `--output=js`");
        return 2;
    }
    if !args.flags.watch {
        return one_pass(args, Asked::Once, None).code;
    }
    // The three combinations `--watch` refuses were refused at parsing, so by
    // here a watch loop is a loop: a pass, the declared set that pass computed,
    // and a sweep of it. The repository is opened once for the whole loop and
    // kept: `build::sources` re-reads and re-parses the files whose bytes
    // moved and nothing else, so a pass after a one-file edit is a pass that
    // parses one file.
    let root = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::build::workspace::find_root(&cwd))
        .unwrap_or_default();
    let mut sources = crate::build::sources::Sources::at(&root, args.flags.clone());
    watch::Watch::on(root, args.flags.explain)
        .drive(|_| one_pass(args, Asked::Watching, Some(&mut sources)))
}

/// One named test, run for the language server's `buri.runTest` lens.
///
/// The exit code and everything the pass printed, rather than a printed pass:
/// stdout carries protocol in that process, so a run whose output went there
/// would corrupt the stream. `--filter` is `contains`, which is what it means
/// at the terminal too — a name that is a substring of another test's runs both.
///
/// The root is named rather than found, because an editor may hold two
/// repositories open and the process's directory says nothing about which one a
/// request is about.
pub fn run_one(root: &std::path::Path, label: &str, name: &str) -> (i32, String) {
    let args = arguments::Args {
        command: "test".to_string(),
        targets: vec![label.to_string()],
        flags: arguments::Flags {
            filter: Some(name.to_string()),
            // The transcript is going into a JSON message; escape codes in one
            // are noise a client has to strip.
            color: Some(false),
            ..arguments::Flags::default()
        },
        passthrough: Vec::new(),
    };
    let pass = one_pass(&args, Asked::Served { root }, None);
    (pass.code, pass.output)
}

/// Who asked for a pass, which decides where its output goes, where its
/// repository is, and whether the declared input set is collected.
#[derive(Clone, Copy)]
enum Asked<'a> {
    /// `buri test` at a terminal: printed as it happens, in the repository the
    /// process is standing in.
    Once,
    /// `buri test --watch`: held for the loop to place, because the loop cannot
    /// know whether a pass is worth a run separator until it has finished — and
    /// the input set is collected from the session that just ran, which is what
    /// makes the watch set and the keys one enumeration rather than two kept in
    /// step.
    Watching,
    /// The language server: one repository named by the caller, and the output
    /// held because it is going into a protocol message.
    Served { root: &'a std::path::Path },
}

/// One `buri test` invocation, whole.
///
/// `sources` is the repository kept across the passes of a watch loop; without
/// one the repository is opened for this pass alone.
fn one_pass(
    args: &arguments::Args,
    asked: Asked,
    sources: Option<&mut crate::build::sources::Sources>,
) -> watch::Pass {
    let watching = matches!(asked, Asked::Watching);
    let mut out =
        if matches!(asked, Asked::Once) { Out::Direct } else { Out::Held(String::new()) };
    let mut sources = sources;
    let opened = match sources.as_deref_mut() {
        Some(sources) => {
            sources.begin_round();
            sources.session(&crate::build::sources::Overlay::new())
        }
        None => match asked {
            Asked::Served { root } => session::open_at(root, &args.flags),
            Asked::Once | Asked::Watching => session::open(&args.flags),
        },
    };
    let mut session = match opened {
        Ok(session) => session,
        Err(msg) => {
            eprintln!("error: {msg}");
            return watch::Pass { code: 2, inputs: Vec::new(), output: out.take(), quiet: false };
        }
    };
    // A graph with errors in it is still a graph: `Workspace::load` keeps every
    // package it found, whether or not its build file parsed. So the declared
    // set is collected before the refusal rather than after it — under `--watch`
    // an unparseable `BUILD.buri` shows its diagnostics and the loop carries on
    // watching, with the file that broke it in the set. An error state is a
    // state, not an exit (BUILD-AND-WATCH.md §4.3).
    let broken = session.report();
    if broken && !watching {
        return watch::Pass { code: 2, inputs: Vec::new(), output: out.take(), quiet: false };
    }
    let targets = match session.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            // Nothing to watch and nothing to run: a selection that names no
            // target is a mistake in the invocation, which no edit will fix.
            return watch::Pass { code: 2, inputs: Vec::new(), output: out.take(), quiet: false };
        }
    };
    let inputs = if watching { watch::inputs(&session, &targets) } else { Vec::new() };
    if broken {
        return watch::Pass { code: 2, inputs, output: out.take(), quiet: false };
    }

    let started = Instant::now();
    warm_linker(args);
    let mut pre = Prepass {
        lints: session.workspace.repo.lint.check_during_build,
        analyses: Vec::new(),
        promised: Vec::new(),
    };
    // Every suite's cache lookup first: on an unedited repository that is the
    // whole pass.
    let mut slots: Vec<Slot> = Vec::new();
    let mut plans: Vec<Plan> = Vec::new();
    let mut graph = None;
    for &target in &targets {
        if has_tests(&session, target) {
            plans.push(plan(&mut session, target, args, &mut graph, &mut slots));
        }
    }
    let suites = plans.len();
    let shared = Shared {
        root: session.root.clone(),
        flags: args.flags.clone(),
        painting: std::sync::Mutex::new(Vec::new()),
        workspace: std::sync::Arc::clone(&session.workspace),
    };
    let width = if slots.iter().all(|s| s.answer.is_some()) { 1 } else { jobs_of(&args.flags) };
    let tally = crate::parallel::pool(
        width,
        memory_budget(),
        |job, held, queue, tell| work(job, held, queue, tell, &shared),
        |queue, done| drive(&mut session, args, &mut pre, &plans, &mut slots, queue, done, &mut out),
    );
    let Tally { passed, failed, skipped, cached, uncompiled, printed, mut hard_error } = tally;

    // `check_during_build`: the catalogue runs over a test pass too, but only
    // one nothing already stopped — a suite that could not be built has an
    // answer, and it is not a lint finding.
    if !hard_error && session.workspace.repo.lint.check_during_build {
        let analyses = std::mem::take(&mut pre.analyses);
        let findings =
            crate::commands::lint::findings_reusing(&mut session, &targets, &args.flags, analyses);
        hard_error |= session.print(&findings);
        // The same line `buri lint` prints: a rule this repository turned off
        // is absent from the report, and an absence nothing explains reads as
        // a check that passed.
        if let Some(note) = crate::commands::lint::rules_note(&session) {
            out.line(&note);
        }
    }
    // Analyses the lint did not take are freed off this thread: a second or
    // two of `free` on a large repository, after the answer exists.
    crate::parallel::discard(std::mem::take(&mut pre.analyses));
    // Everything this pass read and parsed, kept for the next one. After the
    // compiling and before either exit below, which are the two the loop can
    // reach once a suite has been built.
    if let Some(sources) = sources {
        sources.keep(&session);
    }

    let elapsed = started.elapsed().as_secs_f64();
    if suites == 0 {
        out.line("no test suites");
        // Nothing ran, so the only thing that can have failed is the catalogue.
        let code = i32::from(hard_error);
        return watch::Pass { code, inputs, output: out.take(), quiet: false };
    }
    let note = if cached > 0 { format!(", {cached} cached") } else { String::new() };
    // Elided at zero, the way the cached note is: a clean run says nothing
    // about a suite that did not fail to compile.
    let uncompiled_note = if uncompiled > 0 {
        format!(", {uncompiled} failed to compile")
    } else {
        String::new()
    };
    if printed {
        out.blank();
    }
    // The diagnostics went to stderr and this line goes to stdout. Flushing
    // here is what fixes their order when both descriptors are one terminal.
    let _ = std::io::stderr().flush();
    out.line(&format!(
        "{passed} passed, {failed} failed, {skipped} skipped{uncompiled_note} ({elapsed:.1}s{note})"
    ));
    // Silent only when there was nothing to do: every case came out of the
    // cache, none failed, and nothing was asked for by name. `--explain` is
    // never silent — a transcript of what the cache did is exactly what
    // somebody running it wants to read.
    let quiet = !args.flags.explain
        && !hard_error
        && failed == 0
        && !printed
        && passed > 0
        && cached == passed;
    let code = if hard_error || failed > 0 { 1 } else { 0 };
    watch::Pass { code, inputs, output: out.take(), quiet }
}

/// Whether a run's verdicts may be written to the cache.
///
/// Only a clean run is worth remembering: a failure is what you are trying to
/// fix, and re-running it should re-run it. `--filter` is outside the cache in
/// both directions, because the verdicts of a subset are not the suite's. An
/// empty run is a run that produced nothing, and remembering it as "everything
/// passed" would serve a suite that never ran.
fn may_cache(cases: &[Case], flags: &arguments::Flags) -> bool {
    !cases.is_empty()
        && cases.iter().all(|c| matches!(c.verdict, Verdict::Passed))
        && flags.filter.is_none()
}

fn has_tests(session: &Session, target: TargetId) -> bool {
    session.workspace
        .package(target.package)
        .test_suite(target.kind)
        .is_some_and(|t| !t.sources.is_empty())
}

fn suite(session: &Session, target: TargetId) -> Option<crate::build::buildfile::TestSuite> {
    session.workspace.package(target.package).test_suite(target.kind).cloned()
}

// ---------------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------------
//
// A pass has four stages, and only the first two share the session:
//
// - **Plan** ([`plan`]): every suite's policy check, platforms, key and cache
//   lookup, in target order.
// - **Loading** ([`drive`]): this thread loads what the cache did not answer,
//   a batch at a time and then a suite at a time, and queues each for the
//   pool. Loading reads files and mints their ids in the session's source map,
//   so it stays on the thread that owns the map, and the ids come out in the
//   same order every run.
// - **Front ends** ([`front`], [`batch_job`]): the pool's workers type-check
//   and monomorphize, side by side.
// - **Back ends**: the same worker goes on to lower, generate and link, then
//   runs the binary. A batch's binary runs each member in a process of its
//   own, so its suites run side by side too.
//
// A front end and its back end are one heavy job, queued with the memory its
// size says it will hold ([`build_bytes`]), and the jobs holding a whole program
// at once stay within [`memory_budget`] (`parallel::Queue`).
//
// The report never depends on which worker finished first. Each run fills its
// own [`Slot`], `--explain` lines included, and a suite is printed only once
// every suite before it has been.

/// One run of one suite on one platform: what the report is written in.
struct Slot {
    target: TargetId,
    platform: Platform,
    chosen: Chosen,
    key: crate::build::cache::ActionKey,
    /// Where this run's build is recorded ([`actions::test_build_key`]), for a
    /// suite the verdict cache did not answer.
    build: Option<crate::build::cache::ActionKey>,
    /// The binary the last run linked for this suite, when the record had one
    /// and the binary is to be run again rather than built ([`rerun`]).
    served: Option<Box<Linked>>,
    /// This run's `--explain` lines, printed when the suite is reported.
    explain: String,
    /// Lines for standard error, such as a heap check's receipt.
    notes: String,
    answer: Option<Result<Outcome, Diagnostics>>,
    /// Handed to the pool, so nothing else should start it.
    queued: bool,
    /// The first member of a batch whose link has not reported yet. Its
    /// `--explain` lines belong in front of the suite's.
    awaiting_build: bool,
}

/// One suite: what refused it before anything ran, and its runs.
struct Plan {
    target: TargetId,
    refused: Diagnostics,
    slots: Vec<usize>,
}

/// One suite's policy check, platforms, keys and cache lookups.
///
/// A suite inherits its target's tags and platform restrictions, so a suite for
/// a `server` library is checked as server code without saying anything. A
/// suite that names no platforms runs once, natively, on the host it is checked
/// against; `test { backends: [JS] }` and `--output=js` are the two ways to ask
/// for JavaScript. A platform this toolchain cannot run is refused by name
/// ([`not_ready`]) rather than routed somewhere nobody chose.
///
/// A suite the verdict cache does not answer is looked up a second time, by
/// its build ([`recall`]): the errors that stopped it last time are its answer
/// again, and a binary it linked is run again rather than built. `graph` is
/// [`actions::graph_key`], worked out the first time a suite needs it.
fn plan(
    session: &mut Session,
    target: TargetId,
    args: &arguments::Args,
    graph: &mut Option<crate::build::cache::ActionKey>,
    slots: &mut Vec<Slot>,
) -> Plan {
    let mut refused = Diagnostics::new();
    let declared: Vec<Platform> = session.workspace.suite_platforms(target);
    let checked: Vec<Platform> = if declared.is_empty() {
        vec![crate::compiler::driver::host_native_platform()]
    } else {
        declared.clone()
    };
    // A platform a suite names must be one the target admits: asking for a JS
    // run of a `[LINUX, MACOS]` library is an error, not a skip
    // (TAGS.md, "Tags and tests").
    for p in &checked {
        let held = session.workspace.suite_output_platform(target, *p);
        actions::check_policy(session, target, &held, &mut refused);
    }
    if refused.has_errors() {
        return Plan { target, refused, slots: Vec::new() };
    }
    let runs: Vec<(Platform, Chosen)> = if !declared.is_empty() {
        declared.into_iter().map(|p| (p, Chosen::Asked)).collect()
    } else if let Some(p) = selected_platform(&args.flags) {
        // `--output` names the platform for the suites that have not named
        // one, and does not overrule a suite that has.
        vec![(p, Chosen::Asked)]
    } else {
        vec![(crate::compiler::driver::host_native_platform(), Chosen::Default)]
    };
    let mut mine = Vec::new();
    for (platform, chosen) in runs {
        if platform.is_native() && !native_ready(platform, &args.flags) {
            let span = suite(session, target).map(|x| x.span).unwrap_or(Span::NONE);
            refused.push(not_ready(platform, &args.flags, chosen, span));
            continue;
        }
        let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
        let key = actions::test_key(session, target, &output, &args.flags);
        let (cached, mut explain) =
            crate::build::cache::holding_explain(|| served(session, target, platform, &key, args));
        let mut answer = cached.map(Ok);
        let mut build = None;
        let mut linked = None;
        if answer.is_none() {
            let label = session.workspace.label(target);
            let say = |status, action, key: &crate::build::cache::ActionKey| {
                crate::build::cache::holding_explain(|| {
                    crate::build::cache::explain(args.flags.explain, status, action, &label, platform.slug(), key);
                })
                .1
            };
            explain = say(crate::build::cache::Status::Run, crate::build::cache::Action::Test, &key);
            let graph = graph.get_or_insert_with(|| actions::graph_key(session, &args.flags)).clone();
            let at = actions::test_build_key(session, target, &output, &args.flags, &graph);
            // `--force` builds again, as it links again.
            let recalled = if args.flags.force { None } else { recall(session, &at) };
            let status = match recalled {
                Some(_) => crate::build::cache::Status::Cached,
                None => crate::build::cache::Status::Run,
            };
            explain.push_str(&say(status, crate::build::cache::Action::Build, &at));
            match recalled {
                Some(Recalled::Refused(diagnostics)) => answer = Some(Err(diagnostics)),
                Some(Recalled::Nothing { skipped }) => {
                    answer = Some(Ok(Outcome { cases: Vec::new(), skipped }));
                }
                Some(Recalled::Linked(l)) => linked = Some(l),
                None => {}
            }
            build = Some(at);
        }
        mine.push(slots.len());
        slots.push(Slot {
            target,
            platform,
            chosen,
            key,
            build,
            served: linked,
            explain,
            notes: String::new(),
            answer,
            queued: false,
            awaiting_build: false,
        });
    }
    Plan { target, refused, slots: mine }
}

/// How many builds and runs the pool holds at once: `--jobs`, or one per core.
fn jobs_of(flags: &arguments::Flags) -> usize {
    flags.jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |c| c.get())).max(1)
}

/// The bytes the builds in flight may be expected to hold between them: half
/// this machine's memory, leaving the rest to the system, the suites' own
/// processes and the analyses the lint keeps. A suite's run holds no program,
/// so it never counts against this, and `jobs` runs can always go side by side.
/// A machine that cannot say how much memory it has is not limited.
fn memory_budget() -> u64 {
    std::env::var(MEMORY_VARIABLE)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .or_else(crate::parallel::memory_bytes)
        .map_or(u64::MAX, |bytes: u64| bytes / 2)
}

/// The environment variable that replaces this machine's memory, in bytes, in
/// [`memory_budget`].
const MEMORY_VARIABLE: &str = "BURI_TEST_MEMORY_BYTES";

/// The memory a build of `source` bytes of repository code is expected to
/// hold, from its check to its link.
///
/// An upper bound on what was measured, rather than a guess. On an 82-suite
/// repository, one suite at a time, a build grew the process's peak by 25 MB
/// for a suite of half a kilobyte, by 40 to 100 MB for most suites, and by at
/// most 440 MB for one loading 1.8 MB of source. The batch of all 58 suites
/// that could share a binary loaded 4.7 MB of source, and the whole run peaked
/// at 1.9 GB. The size of the source predicts a build only loosely, so this
/// bound is above every one of those, and as much as three times above some.
fn build_bytes(source: u64) -> u64 {
    BUILD_BASE.saturating_add(source.saturating_mul(BUILD_PER_SOURCE_BYTE))
}

/// What a build holds whatever its size.
const BUILD_BASE: u64 = 64 * 1024 * 1024;

/// What a build holds per byte of repository source it compiles.
const BUILD_PER_SOURCE_BYTE: u64 = 400;

/// What the reporting loop counted.
#[derive(Default)]
struct Tally {
    passed: usize,
    failed: usize,
    skipped: usize,
    cached: usize,
    uncompiled: usize,
    printed: bool,
    hard_error: bool,
}

/// Loads every suite the cache did not answer, on this thread, and queues it;
/// then the report, in target order, as the pool's answers arrive.
#[allow(
    clippy::too_many_arguments,
    reason = "the session, the invocation, the lint's analyses, the plans and their \
              slots, the pool's two ends and where output goes: none derivable from another"
)]
fn drive(
    session: &mut Session,
    args: &arguments::Args,
    pre: &mut Prepass,
    plans: &[Plan],
    slots: &mut [Slot],
    queue: &Queue,
    done: &std::sync::mpsc::Receiver<Option<Done>>,
    out: &mut Out,
) -> Tally {
    rerun(session, slots, queue);
    batch(session, args, slots, queue);
    for i in 0..slots.len() {
        solo(session, args, pre, slots, i, queue);
    }
    let mut tally = Tally::default();
    let mut next = 0;
    loop {
        while let Some(plan) = plans.get(next) {
            let ready = plan.slots.iter().all(|&i| {
                slots.get(i).is_some_and(|s| s.answer.is_some() && !s.awaiting_build)
            });
            if !ready {
                break;
            }
            report(session, plan, slots, &mut tally, out);
            next += 1;
        }
        if next >= plans.len() {
            return tally;
        }
        let Ok(Some(answer)) = done.recv() else {
            // A job panicked, or no worker is left. Which suite it was is
            // unknown, so every suite still waiting says so rather than hangs.
            for s in slots.iter_mut().filter(|s| s.answer.is_none() || s.awaiting_build) {
                let mut lost = Diagnostics::new();
                lost.push(
                    Diagnostic::error(Span::NONE, "internal error: a test job stopped without an answer")
                        .with_fix("report it: this is a toolchain bug"),
                );
                s.answer = Some(Err(lost));
                s.awaiting_build = false;
            }
            continue;
        };
        match answer {
            Done::Answer { slot, answer, explain, notes, built } => {
                if let (Some(built), Some(at)) = (built, slots.get(slot).and_then(|s| s.build.as_ref())) {
                    remember(session, at, &built, &answer);
                }
                let answer = answer.map(|ran| located(session, ran));
                if let Some(s) = slots.get_mut(slot) {
                    s.explain.push_str(&explain);
                    s.notes.push_str(&notes);
                    s.answer = Some(answer);
                }
            }
            Done::Progress => {}
            Done::Checked { target, analysis } => pre.analyses.push((target, *analysis)),
            Done::Linking { slot } => {
                if let Some(s) = slots.get_mut(slot) {
                    s.awaiting_build = true;
                }
            }
            // A batch whose type check failed: again without the members whose
            // code failed it, and those alone, where a diagnostic names them.
            Done::Broken { members, member_slots, diagnostics } => {
                let broken = broken_members(session, &members, &diagnostics);
                let kept: Vec<(TargetId, usize)> = members
                    .iter()
                    .copied()
                    .zip(member_slots.iter().copied())
                    .filter(|(m, _)| !broken.is_empty() && !broken.contains(m))
                    .collect();
                for &i in &member_slots {
                    if let Some(s) = slots.get_mut(i) {
                        s.queued = false;
                    }
                }
                if kept.len() >= 2 {
                    let (members, member_slots): (Vec<TargetId>, Vec<usize>) = kept.into_iter().unzip();
                    queue_batch(session, &members, &member_slots, slots, queue);
                }
                for &i in &member_slots {
                    solo(session, args, pre, slots, i, queue);
                }
            }
            Done::Built { slot, explain } => {
                if let Some(s) = slots.get_mut(slot) {
                    s.explain.push_str(&explain);
                    s.awaiting_build = false;
                }
            }
            // A batch that could not be trusted, or a member whose process
            // ended badly: each goes back to run alone, where a diagnostic
            // can name it.
            Done::Abandoned { slots: abandoned, explain } => {
                if let Some(s) = abandoned.first().and_then(|&i| slots.get_mut(i)) {
                    s.explain.push_str(&explain);
                }
                for &i in &abandoned {
                    if let Some(s) = slots.get_mut(i) {
                        s.queued = false;
                        s.awaiting_build = false;
                    }
                    solo(session, args, pre, slots, i, queue);
                }
            }
        }
    }
}

/// Prints one suite: its `--explain` lines, its notes and its failures.
fn report(session: &Session, plan: &Plan, slots: &mut [Slot], tally: &mut Tally, out: &mut Out) {
    let target = plan.target;
    let mut diagnostics = plan.refused.clone();
    let mut outcome = Outcome::default();
    for &i in &plan.slots {
        let Some(slot) = slots.get_mut(i) else { continue };
        print!("{}", slot.explain);
        eprint!("{}", slot.notes);
        match slot.answer.take() {
            Some(Ok(one)) => {
                outcome.cases.extend(one.cases);
                outcome.skipped += one.skipped;
            }
            Some(Err(d)) => diagnostics.extend(d.items),
            None => {}
        }
    }
    if diagnostics.has_errors() {
        // A suite that never compiled produced no cases, so it lands in no
        // other counter and the summary would say nothing about it.
        tally.uncompiled += 1;
        tally.hard_error |= session.print(&diagnostics);
        return;
    }
    tally.skipped += outcome.skipped;
    for c in &outcome.cases {
        if c.provenance == Provenance::Cache {
            tally.cached += 1;
        }
        match &c.verdict {
            Verdict::Passed => tally.passed += 1,
            Verdict::Failed { message, diff, order } => {
                tally.failed += 1;
                report_failure(session, target, c, message, diff.as_ref(), order.as_deref(), out);
                tally.printed = true;
            }
        }
    }
}

/// The pool's queue, of the jobs below.
type Queue = crate::parallel::Queue<Job>;

/// A job's claim on the pool's room for programs.
type Held<'a> = crate::parallel::Held<'a>;

/// What a worker needs that is the same for every job.
struct Shared {
    root: std::path::PathBuf,
    flags: arguments::Flags,
    /// Which suite is painting into each snapshot directory, and how many of
    /// its processes are. Two suites never write one directory's goldens at
    /// once; one suite's processes write different files, so they may.
    painting: std::sync::Mutex<Vec<Painter>>,
    /// The graph a front end is checked against.
    workspace: std::sync::Arc<crate::build::workspace::Workspace>,
}

/// A suite painting into a snapshot directory.
struct Painter {
    dir: String,
    slot: usize,
    processes: usize,
}

impl Shared {
    /// Takes `dir` for the suite in `slot`, unless another suite has it.
    fn claim(&self, dir: &str, slot: usize) -> bool {
        let mut painters = self.painting.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match painters.iter_mut().find(|p| p.dir == dir) {
            Some(p) if p.slot == slot => p.processes += 1,
            Some(_) => return false,
            None => painters.push(Painter { dir: dir.to_string(), slot, processes: 1 }),
        }
        true
    }

    /// Waits until [`Shared::claim`] succeeds. For a suite whose job can't be put back.
    fn claim_waiting(&self, dir: &str, slot: usize) {
        while !self.claim(dir, slot) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn release(&self, dir: &str, slot: usize) {
        let mut painters = self.painting.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(p) = painters.iter_mut().find(|p| p.dir == dir && p.slot == slot) {
            p.processes = p.processes.saturating_sub(1);
        }
        painters.retain(|p| p.processes > 0);
    }
}

/// Work for the pool. Each one owns what it compiles, so a worker needs no
/// session.
enum Job {
    /// One suite, loaded: checked, built and run.
    Front(Box<FrontJob>),
    /// One batch, loaded: checked once, then built as one binary or several.
    Batch(Box<BatchJob>),
    /// One binary of a batch that is several, from the batch's one check.
    Part(Box<PartJob>),
    /// One member's blocks, in a binary a batch linked.
    Member(MemberJob),
    /// A binary a run before this one linked, put back where it runs from.
    Served(Box<ServedJob>),
}

/// What a job hands back to [`drive`].
enum Done {
    /// A slot's answer. `explain` and `notes` are what the job printed, and
    /// `built` is what the next run may start from instead of building.
    Answer {
        slot: usize,
        answer: Result<Ran, Diagnostics>,
        explain: String,
        notes: String,
        built: Option<Built>,
    },
    /// A batch's binary is linked and its members are queued. `slot` is the
    /// first member, whose report carries the link's `--explain` lines.
    Built { slot: usize, explain: String },
    /// A batch's binary is about to be linked, and `slot` is its first member:
    /// that member waits for [`Done::Built`] before it is reported.
    Linking { slot: usize },
    /// These slots go back to run alone.
    Abandoned { slots: Vec<usize>, explain: String },
    /// A batch whose type check reported errors.
    Broken { members: Vec<TargetId>, member_slots: Vec<usize>, diagnostics: Diagnostics },
    /// A suite's analysis, for the lint ([`Prepass`]).
    Checked { target: TargetId, analysis: Box<crate::compiler::driver::Analysis> },
    /// One of a member's processes finished, and others haven't yet.
    Progress,
}

/// How a job hands [`drive`] a [`Done`] before its last.
type Tell = crate::parallel::Tell<Done>;

/// One test of a program, as the report locates it.
#[derive(Clone)]
struct Root {
    name: String,
    module: String,
    span: Span,
}

/// What a run produced, before its cases are located in the source.
struct Ran {
    cases: Vec<Case>,
    skipped: usize,
    roots: Vec<Root>,
}

/// Locates each case at the test it names ([`locate`]).
fn located(session: &Session, ran: Ran) -> Outcome {
    let Ran { mut cases, skipped, roots } = ran;
    locate(session, &roots, &mut cases);
    Outcome { cases, skipped }
}

/// The roots of a program's tests, for [`locate`].
fn roots_of(program: &monomorphize::Program) -> Vec<Root> {
    program
        .roots
        .tests()
        .iter()
        .map(|t| Root { name: t.name.clone(), module: t.module.clone(), span: t.span })
        .collect()
}

/// Runs one job. A job that builds drops `held` with its program, before it
/// runs anything, so a run never stands in the way of the next build.
fn work(job: Job, held: Held, queue: &Queue, tell: &Tell, shared: &Shared) -> Done {
    match job {
        Job::Front(job) => front(*job, held, tell, shared),
        Job::Batch(job) => batch_job(*job, held, queue, tell, shared),
        Job::Part(job) => part(*job, held, queue, tell, shared),
        Job::Member(job) => run_member(job, queue, shared),
        Job::Served(job) => {
            drop(held);
            serve(*job, queue, shared)
        }
    }
}

/// Loads slot `i` on its own and queues it, unless it is answered or already
/// queued.
///
/// `None`, not the platform, as the unit's platform: a test is never handed a
/// host, so there is no host to check it against (`Unit::platform`).
fn solo(
    session: &mut Session,
    args: &arguments::Args,
    pre: &mut Prepass,
    slots: &mut [Slot],
    i: usize,
    queue: &Queue,
) {
    let Some(slot) = slots.get_mut(i) else { return };
    if slot.answer.is_some() || slot.queued {
        return;
    }
    slot.queued = true;
    let (target, platform, chosen, key) = (slot.target, slot.platform, slot.chosen, slot.key.clone());
    let unit = Unit { target: Some(target), platform: None, entry: None, with_tests: true };
    let loading = crate::compiler::driver::load_all(
        Some(&session.workspace),
        &mut session.map,
        &mut session.parsed,
        std::slice::from_ref(&unit),
    );
    let bytes = build_bytes(loading.source_bytes(&session.map));
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    let limit = suite(session, target).and_then(|x| x.timeout_seconds);
    let js = session
        .root
        .join(".buri/out/node")
        .join(&session.workspace.package(target.package).path)
        .join(format!("test-{}.mjs", target.kind.name()));
    let job = FrontJob {
        slot: i,
        target,
        platform,
        chosen,
        key,
        loading,
        map: session.map.shared(),
        keep: pre.promise(target),
        label: session.workspace.label(target),
        limit,
        on_timeout: timed_out(session, target, limit),
        private: actions::private_test_binary(session, target, &output),
        output,
        js,
        snapshot_dir: snapshot_dir(session, target),
        filter: args.flags.filter.clone(),
    };
    queue.push(Job::Front(Box::new(job)), bytes);
}

/// One suite, loaded, and everything about it a worker would otherwise ask the
/// session.
struct FrontJob {
    slot: usize,
    target: TargetId,
    platform: Platform,
    chosen: Chosen,
    key: crate::build::cache::ActionKey,
    loading: crate::compiler::driver::Loading,
    /// The session's map as the load left it, which names every file the
    /// check can report on.
    map: std::sync::Arc<crate::diagnostics::SourceMap>,
    /// Whether the lint reads this suite's analysis.
    keep: bool,
    label: String,
    limit: Option<u32>,
    on_timeout: Diagnostics,
    private: std::path::PathBuf,
    output: crate::build::buildfile::Output,
    /// Where a JavaScript run writes its bundle.
    js: std::path::PathBuf,
    snapshot_dir: String,
    filter: Option<String>,
}

/// One suite's front end, and then its back end on the same worker.
fn front(job: FrontJob, held: Held, tell: &Tell, shared: &Shared) -> Done {
    let FrontJob {
        slot,
        target,
        platform,
        chosen,
        key,
        loading,
        map,
        keep,
        label,
        limit,
        on_timeout,
        private,
        output,
        js,
        snapshot_dir,
        filter,
    } = job;
    let answer = |answer, built| Done::Answer { slot, answer, explain: String::new(), notes: String::new(), built };
    let mut analysis = crate::compiler::driver::check(loading, Some(&shared.workspace), &map);
    drop(map);
    if analysis.diagnostics.has_errors() {
        return answer(Err(analysis.diagnostics), Some(Built::Refused));
    }
    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diagnostics,
        monomorphize::Roots::Tests,
    );
    if diagnostics.has_errors() {
        return answer(Err(diagnostics), Some(Built::Refused));
    }
    if program.roots.tests().is_empty() {
        if keep {
            tell.tell(Done::Checked { target, analysis: Box::new(analysis) });
        }
        let nothing = Built::Nothing { skipped: 0 };
        return answer(Ok(Ran { cases: Vec::new(), skipped: 0, roots: Vec::new() }), Some(nothing));
    }
    // Counted here rather than in the runner: the names are known before the
    // binary is built, so the summary can always print the count.
    let filter = filter.as_deref();
    let skipped = match filter {
        Some(f) => program.roots.tests().iter().filter(|t| !t.name.contains(f)).count(),
        None => 0,
    };
    // The gap, asked of the program before a second is spent on codegen, and
    // only for a platform nobody asked for.
    if platform.is_native() && chosen == Chosen::Default {
        if let Some(gap) = native_gap(platform, &shared.flags, &program, &analysis.checked.tables) {
            return answer(Err(gap_refusal(&label, gap)), None);
        }
    }
    let tables = std::sync::Arc::new(if keep {
        analysis.checked.tables.clone()
    } else {
        std::mem::take(&mut analysis.checked.tables)
    });
    if keep {
        tell.tell(Done::Checked { target, analysis: Box::new(analysis) });
    } else {
        drop(analysis);
    }
    if !platform.is_native() {
        let job = JsJob { slot, program, tables, key, path: js, limit, on_timeout, skipped };
        return run_js(job, held, shared);
    }
    // A filtered native run does not even generate the tests it leaves out.
    if let (Some(f), monomorphize::ProgramRoots::Tests(tests)) = (filter, &mut program.roots) {
        tests.retain(|t| t.name.contains(f));
    }
    let tests: Vec<(String, String)> =
        program.roots.tests().iter().map(|t| (t.name.clone(), t.module.clone())).collect();
    if tests.is_empty() {
        let nothing = Built::Nothing { skipped };
        return answer(Ok(Ran { cases: Vec::new(), skipped, roots: Vec::new() }), Some(nothing));
    }
    let paints = program.funcs.iter().any(|f| f.intrinsic_key() == Some(PAINT_KEY));
    let job = SoloJob {
        slot,
        label,
        private,
        output,
        roots: roots_of(&program),
        program,
        tables,
        key,
        limit,
        on_timeout,
        snapshot_dir,
        paints,
        tests,
        skipped,
    };
    run_solo(job, held, shared)
}

/// One suite's own native binary: link it, then run every block.
struct SoloJob {
    slot: usize,
    label: String,
    private: std::path::PathBuf,
    output: crate::build::buildfile::Output,
    program: monomorphize::Program,
    tables: std::sync::Arc<crate::compiler::semantics::types::Tables>,
    key: crate::build::cache::ActionKey,
    limit: Option<u32>,
    on_timeout: Diagnostics,
    snapshot_dir: String,
    paints: bool,
    /// Each block's title and module, in block order.
    tests: Vec<(String, String)>,
    roots: Vec<Root>,
    skipped: usize,
}

/// One suite, executed as a native binary.
///
/// The report is the JavaScript one, to the byte, because it is assembled from
/// the same record: this produces the array `$run` writes, and
/// [`report_failure`] states the format once for both backends.
///
/// **A failed assertion is still an abort.** SPEC 6.9 leaves nothing to catch,
/// so one process reports one failure, and [`run_blocks`] starts another at the
/// next block. A suite costs one process plus one per failure.
fn run_solo(job: SoloJob, held: Held, shared: &Shared) -> Done {
    let SoloJob { slot, label, private, output, mut program, tables, key, limit, on_timeout, snapshot_dir, paints, tests, roots, skipped } = job;
    // Taken before the link, which changes the program.
    let sheet = program.stylesheet.clone();
    let mut diagnostics = Diagnostics::new();
    let (built, explain) = crate::build::cache::holding_explain(|| {
        actions::link_test_binary(&shared.root, &label, private, &output, &shared.flags, &mut program, &tables, &mut diagnostics)
    });
    drop(program);
    drop(tables);
    drop(held);
    let answer = |answer, notes| Done::Answer { slot, answer, explain: explain.clone(), notes, built: None };
    let (binary, link) = match built {
        Ok(built) => built,
        // The gaps `missing_intrinsics` cannot see are named by the backend
        // while it emits, so the sentence saying what to do is added here.
        Err(d) if is_backend_gap(&d) => {
            let mut out = Diagnostics::new();
            for (i, d) in d.items.into_iter().enumerate() {
                out.push(if i == 0 { d.with_fix(GAP_FIX) } else { d });
            }
            return answer(Err(out), String::new());
        }
        Err(d) => return answer(Err(d), String::new()),
    };
    let snapshots = snapshot_env(&snapshot_dir, shared.flags.update, write_stylesheet(binary.path(), &sheet));
    if paints {
        shared.claim_waiting(&snapshot_dir, slot);
    }
    let mut notes = String::new();
    let ran = run_blocks(
        &binary.path().display().to_string(),
        limit,
        (0, tests.len()),
        &seed_of(&key).to_string(),
        &snapshots,
        &mut notes,
    );
    if paints {
        shared.release(&snapshot_dir, slot);
    }
    // A binary that ran to a verdict, or out of time, is worth running again
    // next time. One that died is built again, where the death is reported.
    let linked = |roots: &[Root]| {
        Some(Built::Linked(Box::new(Linked {
            link: link.clone(),
            sheet: sheet.clone(),
            paints,
            skipped,
            ranges: vec![(0, tests.len())],
            roots: roots.to_vec(),
        })))
    };
    let blocks = match ran {
        Ok(Verdicts::Blocks(blocks)) => blocks,
        Ok(Verdicts::TimedOut) => {
            let built = linked(&roots);
            return Done::Answer { slot, answer: Err(on_timeout), explain, notes, built };
        }
        Ok(Verdicts::HeapCheck(line)) => return answer(Err(heap_check_failed(&label, &line)), notes),
        Ok(Verdicts::Died(how)) => return answer(Err(the_binary_died(&label, &how)), notes),
        Ok(Verdicts::NotStarted(how)) => {
            return answer(Err(the_binary_did_not_start(&label, &how)), notes)
        }
        Err(e) => {
            let mut d = Diagnostics::new();
            d.push(
                Diagnostic::error(Span::NONE, format!("cannot run the test binary: {e}"))
                    .with_fix("the link produced it, so this is a toolchain bug"),
            );
            return answer(Err(d), notes);
        }
    };
    let cases = recorded(shared, &key, &tests, blocks.iter());
    let built = linked(&roots);
    Done::Answer { slot, answer: Ok(Ran { cases, skipped, roots }), explain, notes, built }
}

/// The environment that tells a test binary where its goldens are.
fn snapshot_env(dir: &str, update: bool, sheet: Option<String>) -> Vec<(&'static str, String)> {
    let mut env = vec![(SNAPSHOT_DIR, dir.to_string())];
    if update {
        env.push((SNAPSHOT_UPDATE, "1".to_string()));
    }
    if let Some(path) = sheet {
        env.push((SNAPSHOT_SHEET, path));
    }
    env
}

/// One suite's JavaScript bundle: emit it, write it, run it.
struct JsJob {
    slot: usize,
    program: monomorphize::Program,
    tables: std::sync::Arc<crate::compiler::semantics::types::Tables>,
    key: crate::build::cache::ActionKey,
    path: std::path::PathBuf,
    limit: Option<u32>,
    on_timeout: Diagnostics,
    skipped: usize,
}

fn run_js(job: JsJob, held: Held, shared: &Shared) -> Done {
    let JsJob { slot, mut program, tables, key, path, limit, on_timeout, skipped } = job;
    let answer =
        |answer| Done::Answer { slot, answer, explain: String::new(), notes: String::new(), built: None };
    let mut diagnostics = Diagnostics::new();
    let roots = roots_of(&program);
    let mut source =
        match actions::emit_test_bundle(&mut program, &tables, &shared.flags, &mut diagnostics) {
            Ok(source) => source,
            Err(d) => return answer(Err(d)),
        };
    drop(program);
    drop(tables);
    drop(held);
    // The order `anyOrder()` schedules with, and the action's clock, spliced in
    // after the runtime is defined and before a test could reach either.
    source.push_str(&format!("\n$t.seed={}n;\n", seed_of(&key)));
    source.push_str(crate::build::spawn::FIXED_CLOCK_JS);
    let filter = shared.flags.filter.as_ref().map(|f| javascript::quote(f)).unwrap_or_else(|| "null".into());
    // `$run` is `async`, and this is module top level of an `.mjs` file.
    source.push_str(&format!("$write(1,JSON.stringify(await $run({filter})));\n"));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, &source) {
        let mut d = Diagnostics::new();
        d.push(
            Diagnostic::error(Span::NONE, format!("cannot write {}: {e}", path.display()))
                .with_fix("check the directory exists and is writable"),
        );
        return answer(Err(d));
    }
    let out = match execute(&js_runtime(), Some(&path), limit, &[]) {
        Ok(Execution::Finished(out)) => out,
        Ok(Execution::TimedOut) => return answer(Err(on_timeout)),
        Err(e) => {
            let mut d = Diagnostics::new();
            d.push(
                Diagnostic::error(Span::NONE, format!("cannot run the test binary: {e}"))
                    .with_fix("install bun, or point BURI_JS at a JavaScript runtime"),
            );
            return answer(Err(d));
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let cases = parse_results(&stdout);
    if may_cache(&cases, &shared.flags) {
        crate::build::cache::Cache::open(&shared.root).put(&key, stdout.as_bytes());
    }
    if cases.is_empty() && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        let mut d = Diagnostics::new();
        d.push(
            Diagnostic::error(Span::NONE, "the test binary did not run")
                .with_fix("read the runtime's own message below; it is what failed")
                .with_note(err.trim().to_string()),
        );
        return answer(Err(d));
    }
    answer(Ok(Ran { cases, skipped, roots }))
}

/// Whether a suite may be *executed* on `platform`.
///
/// The same three questions a native build asks — a backend compiled in for
/// this target and profile, a runtime archive for this host, a host that can
/// link it — asked through the same function, so that `buri test` and
/// `buri build` cannot disagree about what this toolchain can do. The profile
/// comes from the flags rather than being pinned to `Debug`, so that
/// `buri test --release` on a toolchain without `backend-llvm` is refused
/// rather than quietly run through the development backend.
fn native_ready(platform: Platform, flags: &arguments::Flags) -> bool {
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    let target = actions::target_of(&output);
    // A test must *run*, so its artifact has to be one this host can execute —
    // its own. `buri build` cross-links a Linux artifact from a mac
    // (ARCHITECTURE.md §9), but a test suite for that artifact could not be run
    // where it was built, so `buri test` is refused for a cross platform even
    // though `buri build` is not. `is_host_target` is that narrower question.
    actions::native_ready(target, actions::profile_of(flags))
        && crate::build::link::is_host_target(target)
        && crate::build::spawn::resolve(&linker_name()).is_some()
}

/// Starts the linker-identity probe for the platform this pass will mostly run
/// on, before the first suite is compiled.
///
/// The probe is two `--version` spawns and its answer is a term in every `link`
/// key ([`crate::build::link::warm`]). It already ran on a thread, but the
/// thread was started inside the suite's own link step, and on a repository
/// whose suites compile in a millisecond there is nothing between the two to
/// hide it behind — so the whole pass paid the wait once per suite. Started
/// here, one probe runs beside the whole pass.
///
/// A guess, and one that costs nothing when it is wrong: a suite that declared
/// a different platform, or JavaScript, selects its own linker and the probe
/// that ran was for a linker nobody asked. What it must not do is *decide*
/// anything, and it does not: a pass this toolchain cannot serve is refused by
/// [`not_ready`], per suite and by name, rather than being turned away here.
fn warm_linker(args: &arguments::Args) {
    let platform = match selected_platform(&args.flags) {
        Some(p) => p,
        None => crate::compiler::driver::host_native_platform(),
    };
    if !platform.is_native() || !native_ready(platform, &args.flags) {
        return;
    }
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    crate::build::link::warm(actions::target_of(&output));
}

/// The C compiler the link is driven through, which is `cc` unless `CC` names
/// another (`build/link.rs::select`).
///
/// Asked here as well as there because the two questions are different: `select`
/// is where a link that was asked for finds its driver, and this is where a run
/// nobody asked for decides not to need one. A machine with a backend, an
/// archive and no C toolchain used to run its suites on JavaScript, and it still
/// does.
fn linker_name() -> String {
    std::env::var("CC").unwrap_or_else(|_| String::from("cc"))
}

/// The platform `--output=` names, if it names one.
///
/// On `buri test` the selector names a backend, `js` or `native`, because a
/// suite runs on a backend. A selector naming nothing is not an error at this
/// seam: `command_test` refuses it once, for the invocation, rather than once
/// per suite.
fn selected_platform(flags: &arguments::Flags) -> Option<Platform> {
    match flags.output.as_deref()? {
        "js" => Some(Platform::Js),
        "native" => Some(crate::compiler::driver::host_native_platform()),
        _ => None,
    }
}

/// A native run this toolchain cannot produce, refused.
///
/// The toolchain's half of the answer, and it is the same for every suite in a
/// pass: `--no-default-features`, a host outside macOS and Linux, a host the
/// development backend has no stencil library for (macOS on x86-64), a machine
/// with no C toolchain, and `--release` without `backend-llvm`. The last of
/// those is why the profile comes from the flags rather than being pinned to
/// `Debug`: the release profile routes to LLVM (`backend::select`), and a
/// toolchain that does not have it must not be quietly handed the debug
/// backend. The suite's half — whether the native backend has a body for
/// everything this program reaches — is [`native_gap`], asked once the program
/// exists.
///
/// It used to be a JavaScript run with a note on stderr, and the note was the
/// problem: a suite reported as passing had run on a backend nobody chose, and
/// the only record of that went into a stream a green run's reader does not
/// read. What replaced it is this, in both directions — a platform somebody
/// *asked* for is refused in the words that name the request, and the default
/// is refused in the words that name the toolchain, because "drop it from
/// `test.platforms`" is not an instruction a suite with no `platforms` can
/// follow (buri-lang/buri#4).
fn not_ready(
    platform: Platform,
    flags: &arguments::Flags,
    chosen: Chosen,
    span: Span,
) -> Diagnostic {
    match chosen {
        Chosen::Asked => {
            // The *reason*, from the one function `buri build` asks
            // (`actions::native_gap`), rather than "this toolchain emits
            // JavaScript" — which was false on a host that runs its other
            // suites natively, and is the same wrong sentence
            // buri-lang/buri#25 and buri-lang/buri#26 were about on the
            // build side.
            // A suite asking for `NATIVE` runs on this host's own variant, so
            // what is missing is the toolchain's half.
            let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
            let target = actions::target_of(&output);
            let why = actions::native_gap(target, actions::profile_of(flags))
                .map(|gap| gap.reason)
                .unwrap_or_else(|| "this host has no C toolchain to link one with".to_string());
            let backend = platform.backend().proto();
            let slug = platform.slug();
            Diagnostic::templated("test-run-unavailable", span)
                .with_bind("platform", slug)
                .with_note(format!("{why}, so this suite can be executed only on JavaScript"))
                .with_fix(format!(
                    "drop {backend} from `test.backends`, or run this suite where a {slug} \
                     artifact can be built"
                ))
        }
        Chosen::Default => Diagnostic::templated("test-run-unavailable", span)
            .with_bind("platform", platform.slug())
            .with_note(format!(
                "a native run in the {} profile needs a code generator for it compiled into this \
                 toolchain, a runtime archive for this host, and a C toolchain to link them with",
                flags.mode.name()
            ))
            .with_fix(
                "run the suite on JavaScript with `buri test --output=js`, or declare \
                 `test { backends: [JS] }` if that is where it belongs",
            ),
    }
}

/// Why a program cannot run natively, in the shape the refusal needs.
///
/// Three variants, because the causes are not the same kind of news. A key the
/// backend has no body for is a toolchain bug and a suite that could be
/// declared `platforms: [JS]`; an operation this toolchain has no networking or
/// no cryptography for is neither — the program is fine, and running it on
/// JavaScript instead would prove something else.
enum Gap {
    /// Operations only a runtime built with networking answers.
    Networking(Vec<String>),
    /// Operations only a runtime built with cryptography answers.
    Cryptography(Vec<String>),
    /// The keys this backend has no body for, all of them: a truncated list is
    /// a run that cannot show what it is missing (buri-lang/buri#199).
    NoBody { backend: &'static str, keys: Vec<String> },
}

/// What this program reaches that a native run cannot answer, or `None` when it
/// reaches nothing of the kind.
///
/// The answer is the backend's own, not a list kept here: keeping one would be
/// a second statement of the surface that drifts from the first the day a gap
/// closes, and a gap closing is the frequent event.
fn native_gap(
    platform: Platform,
    flags: &arguments::Flags,
    program: &monomorphize::Program,
    tables: &crate::compiler::semantics::types::Tables,
) -> Option<Gap> {
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    let backend =
        crate::compiler::backend::select(actions::target_of(&output), actions::profile_of(flags))
            .ok()?;
    // This prepass asks the backend of the *monomorphized* program, before
    // `middle::chunks` has run — where the real native build asks the same
    // question a step later, of the program that pass has already lowered
    // (`build::actions`). `core/lazy`'s `load` is the one intrinsic that
    // difference matters for: `middle::chunks` replaces every call to it with
    // its argument on a native build (`intrinsic_keys::LAZY_LOAD`), so no
    // backend is ever asked to emit it, and a suite that reaches one compiles
    // and runs natively exactly as three doc pages say it does. Counting it
    // here — and here alone, because it is gone by the time the build checks —
    // is what refused those suites onto JavaScript.
    let missing: Vec<String> = backend
        .missing_intrinsics(program, tables)
        .into_iter()
        .filter(|k| k != crate::compiler::backend::intrinsic_keys::LAZY_LOAD)
        .collect();
    let (networking, rest) = crate::compiler::backend::split_networking(&missing);
    if !networking.is_empty() {
        return Some(Gap::Networking(networking));
    }
    let (cryptography, rest) = crate::compiler::backend::split_cryptography(&rest);
    if !cryptography.is_empty() {
        return Some(Gap::Cryptography(cryptography));
    }
    if rest.is_empty() {
        return None;
    }
    Some(Gap::NoBody { backend: backend.name(), keys: rest })
}

/// Whether a native compilation failed because this backend has no body for
/// something, rather than because the program is wrong.
///
/// Matched on the sentence both spellings share — `actions::objects_of`'s, from
/// `missing_intrinsics`, and a backend's own, from a runtime key with no
/// entry. A failure with anything else among its errors is a failure, and is
/// reported: falling back on one would turn a toolchain bug into a suite that
/// quietly passes somewhere else.
/// What to do about a native gap, in the one sentence a reader needs.
///
/// Naming a platform is a statement in the build file rather than a decision
/// the runner takes on a program's behalf, which is the whole difference
/// between this and what it replaced.
const GAP_FIX: &str = "declare `test { backends: [JS] }` for this suite if it belongs \
                       on JavaScript, or report the gap: a program the front end \
                       accepted is one the backend should compile";

/// A native gap, reported rather than routed around.
///
/// Rerouting a suite onto JavaScript because the native backend has no body for
/// an operation is how a *named* gap becomes a wrong answer: the suite passes,
/// on a backend nobody chose, and what it proves is that the other backend
/// agrees with itself. `buri build` has always refused this; `buri test` does
/// now too (buri-lang/buri#4).
fn gap_refusal(label: &str, gap: Gap) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    match gap {
        // The page's own words, and its own fix: what this suite needs is a
        // toolchain, and [`GAP_FIX`]'s two suggestions — declare `platforms:
        // [JS]`, or report a bug — are both the wrong instruction for it.
        Gap::Networking(operations) => {
            diagnostics.push(crate::compiler::backend::no_networking(&operations, Span::NONE));
        }
        // The same argument, one page over: a suite that mints a token needs a
        // toolchain whose runtime has a cryptographic generator, and neither of
        // [`GAP_FIX`]'s suggestions is what to do about that.
        Gap::Cryptography(operations) => {
            diagnostics.push(crate::compiler::backend::no_cryptography(&operations, Span::NONE));
        }
        Gap::NoBody { backend, keys } => {
            // Every key, not a count: a run that is refused for a gap should be
            // able to show the whole gap, in the human sentence and — because
            // the message is what the JSON carries — in the JSON too
            // (buri-lang/buri#199). `build/actions.rs`'s sibling refusal lists
            // them all the same way.
            let reason =
                format!("the {backend} backend has no implementation of {}", keys.join(", "));
            diagnostics.push(
                Diagnostic::error(Span::NONE, format!("{label} cannot run natively: {reason}"))
                    .with_fix(GAP_FIX),
            );
        }
    }
    diagnostics
}

fn is_backend_gap(diagnostics: &Diagnostics) -> bool {
    !diagnostics.items.is_empty()
        && diagnostics.items.iter().all(|d| d.message.contains("has no implementation of"))
}

/// The analyses `check_during_build` reuses.
///
/// A suite compiled on its own is the same unit the lint asks about, so the
/// lint reads its analysis rather than analysing it again.
struct Prepass {
    /// Whether `check_during_build` will lint this pass's targets.
    lints: bool,
    /// The analyses the workers handed back ([`Done::Checked`]).
    analyses: Vec<(TargetId, crate::compiler::driver::Analysis)>,
    /// The targets whose analysis a queued front end will hand back.
    promised: Vec<TargetId>,
}

impl Prepass {
    /// Whether the lint wants the analysis of the suite about to be queued,
    /// and if so, notes that it is coming. A suite run on two platforms is one
    /// unit, so its first analysis is the one.
    fn promise(&mut self, target: TargetId) -> bool {
        if !self.lints || self.promised.contains(&target) {
            return false;
        }
        self.promised.push(target);
        true
    }
}

/// The verdicts the cache holds for this key, where it may serve them.
///
/// A suite whose inputs are unchanged is not re-run and reports as cached;
/// `--force` re-runs anyway, which is the honest way to check that a suite is
/// not accidentally depending on the cache.
fn served(
    session: &Session,
    target: TargetId,
    platform: Platform,
    key: &crate::build::cache::ActionKey,
    args: &arguments::Args,
) -> Option<Outcome> {
    // `--update` is a request to write the goldens, and a cached verdict writes
    // nothing. The sources did not move, so the key is the same and the record
    // a recording run stores is the record a later plain run wants.
    if args.flags.force || args.flags.update || args.flags.filter.is_some() {
        return None;
    }
    let bytes = crate::build::cache::Cache::open(&session.root).get(key)?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let mut cases = parse_results(&text);
    if cases.is_empty() {
        return None;
    }
    for c in &mut cases {
        c.provenance = Provenance::Cache;
    }
    crate::build::cache::explain(
        args.flags.explain,
        crate::build::cache::Status::Cached,
        crate::build::cache::Action::Test,
        &session.workspace.label(target),
        platform.slug(),
        key,
    );
    Some(Outcome { cases, skipped: 0 })
}

// ---------------------------------------------------------------------------
// What a build leaves for the next run
// ---------------------------------------------------------------------------
//
// A failing verdict is never cached: a failure is what somebody is fixing, and
// running the suite again has to run it. Building it again is another matter.
// Without an edit the binary is the same binary and a compile error is the
// same error, so each suite's build is recorded under a key known before the
// front end runs ([`actions::test_build_key`]), and a suite the verdict cache
// does not answer is looked up there next:
//
// - **Refused**: the errors the front end reported are printed again, from the
//   record, without checking anything.
// - **Nothing**: the suite had no test to run, or a `--filter` left it none.
// - **Linked**: the binary is still in the link cache under its `link` key, so
//   it is put back where it runs from ([`serve`]) and its blocks are run again.
//
// A batch's binary is recorded once per member, with the member's blocks in
// it, under the member's own key. That is sound for the reason a batched
// verdict is: a root neither changes another root's code nor can be reached
// from it, so a member's blocks behave the same in any binary that holds them.
// Members whose records name one binary share one copy of it again.
//
// A binary that ran again and then died, failed the heap check or never
// started is built again ([`Done::Abandoned`]), and the build reports it.

/// What a suite's build left for the next run, as a job hands it to [`drive`].
enum Built {
    /// The front end refused it, with the answer's diagnostics.
    Refused,
    /// It had no test to run.
    Nothing { skipped: usize },
    /// A binary in the link cache, and where the suite's tests are in it.
    Linked(Box<Linked>),
}

/// One suite's place in a test binary the link cache holds.
struct Linked {
    link: crate::build::cache::ActionKey,
    /// The program's stylesheet, written beside the binary for a snapshot.
    sheet: String,
    paints: bool,
    /// What the `--filter` left out.
    skipped: usize,
    /// The suite's blocks, in the binary's numbering.
    ranges: Vec<(usize, usize)>,
    /// The suite's tests, in block order.
    roots: Vec<Root>,
}

/// A record read back ([`recall`]).
enum Recalled {
    Refused(Diagnostics),
    Nothing { skipped: usize },
    Linked(Box<Linked>),
}

/// The shape of a build record, so that a change to the encoding is a miss
/// rather than a misreading.
const BUILD_FORMAT: &[u8] = b"buri-test-build-1\n";

/// Writes down what a suite's build left, under `at`.
///
/// On this thread rather than a worker's, because a span is written as the
/// name of its file and the session's map is what knows the names.
fn remember(
    session: &Session,
    at: &crate::build::cache::ActionKey,
    built: &Built,
    answer: &Result<Ran, Diagnostics>,
) {
    use crate::commands::lint_cache::{put_diagnostic, put_span, put_text, put_u32};
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let mut out = BUILD_FORMAT.to_vec();
    match built {
        Built::Refused => {
            let Err(diagnostics) = answer else { return };
            out.push(0);
            put_u32(&mut out, count(diagnostics.items.len()));
            for d in &diagnostics.items {
                put_diagnostic(&mut out, &session.map, d);
            }
        }
        Built::Nothing { skipped } => {
            out.push(1);
            put_u32(&mut out, count(*skipped));
        }
        Built::Linked(l) => {
            out.push(2);
            put_text(&mut out, l.link.as_str());
            put_text(&mut out, &l.sheet);
            out.push(u8::from(l.paints));
            put_u32(&mut out, count(l.skipped));
            put_u32(&mut out, count(l.ranges.len()));
            for &(from, to) in &l.ranges {
                put_u32(&mut out, count(from));
                put_u32(&mut out, count(to));
            }
            put_u32(&mut out, count(l.roots.len()));
            for root in &l.roots {
                put_text(&mut out, &root.name);
                put_text(&mut out, &root.module);
                put_span(&mut out, &session.map, root.span);
            }
        }
    }
    crate::build::cache::Cache::open(&session.root).put(at, &out);
}

/// What the build recorded under `at` left, with its spans in this run's map.
///
/// `None` for no record, one this toolchain can't read, or one naming a file
/// that isn't there. The suite is then built.
fn recall(session: &mut Session, at: &crate::build::cache::ActionKey) -> Option<Recalled> {
    use crate::commands::lint_cache::{read_diagnostic, read_span, Reader};
    let bytes = crate::build::cache::Cache::open(&session.root).get(at)?;
    let mut r = Reader::after(BUILD_FORMAT, &bytes)?;
    let root = session.root.clone();
    let count = |r: &mut Reader| r.u32().and_then(|n| usize::try_from(n).ok());
    match r.byte()? {
        0 => {
            let mut diagnostics = Diagnostics::new();
            for _ in 0..r.u32()? {
                diagnostics.push(read_diagnostic(&mut r, &mut session.map, &root)?);
            }
            Some(Recalled::Refused(diagnostics))
        }
        1 => Some(Recalled::Nothing { skipped: count(&mut r)? }),
        2 => {
            let link = crate::build::cache::ActionKey::parse(&r.text()?)?;
            let sheet = r.text()?;
            let paints = r.byte()? == 1;
            let skipped = count(&mut r)?;
            let mut ranges = Vec::new();
            for _ in 0..r.u32()? {
                ranges.push((count(&mut r)?, count(&mut r)?));
            }
            let mut roots = Vec::new();
            for _ in 0..r.u32()? {
                let name = r.text()?;
                let module = r.text()?;
                let span = read_span(&mut r, &mut session.map, &root)?;
                roots.push(Root { name, module, span });
            }
            Some(Recalled::Linked(Box::new(Linked { link, sheet, paints, skipped, ranges, roots })))
        }
        _ => None,
    }
}

/// Queues every slot whose binary is to be run again, one [`Job::Served`] per
/// binary, so members of one batch share one copy of it again.
fn rerun(session: &Session, slots: &mut [Slot], queue: &Queue) {
    let mut jobs: Vec<ServedJob> = Vec::new();
    for (i, slot) in slots.iter_mut().enumerate() {
        if slot.answer.is_some() || slot.queued {
            continue;
        }
        let Some(linked) = slot.served.take() else { continue };
        slot.queued = true;
        let Linked { link, sheet, paints, skipped, ranges, roots } = *linked;
        let target = slot.target;
        let output = crate::build::buildfile::Output::for_platform(slot.platform, Span::NONE);
        let limit = suite(session, target).and_then(|x| x.timeout_seconds);
        let seeds = std::sync::Arc::new(seeds_at(&ranges, seed_of(&slot.key)));
        let spec = MemberSpec {
            slot: i,
            key: slot.key.clone(),
            tests: roots.iter().map(|r| (r.name.clone(), r.module.clone())).collect(),
            ranges,
            roots,
            skipped,
            snapshot_dir: snapshot_dir(session, target),
            paints,
            limit,
            on_timeout: timed_out(session, target, limit),
            remember: None,
        };
        match jobs.iter_mut().find(|j| j.link == link) {
            Some(job) => job.members.push((spec, seeds)),
            None => jobs.push(ServedJob {
                private: actions::private_test_binary(session, target, &output),
                link,
                sheet,
                output,
                members: vec![(spec, seeds)],
            }),
        }
    }
    for job in jobs {
        queue.push(Job::Served(Box::new(job)), 0);
    }
}

/// The seeds a suite's processes are handed when its blocks are `ranges` of a
/// binary: its own seed at each of its blocks, and nothing that means anything
/// at the blocks of the suites it shares the binary with.
fn seeds_at(ranges: &[(usize, usize)], seed: u128) -> String {
    let len = ranges.iter().map(|&(_, to)| to).max().unwrap_or(0);
    let mut seeds = vec![String::from("0"); len];
    for &(from, to) in ranges {
        for s in seeds.iter_mut().take(to).skip(from) {
            *s = seed.to_string();
        }
    }
    seeds.join(",")
}

/// A binary an earlier run linked, and the suites to run in it.
struct ServedJob {
    link: crate::build::cache::ActionKey,
    sheet: String,
    output: crate::build::buildfile::Output,
    /// Where the binary goes when the shared runner file is taken: the first
    /// member's own.
    private: std::path::PathBuf,
    members: Vec<(MemberSpec, std::sync::Arc<String>)>,
}

/// Puts a recorded binary back where it runs from and queues its members'
/// processes. A binary the cache no longer holds sends its members to be built.
fn serve(job: ServedJob, queue: &Queue, shared: &Shared) -> Done {
    let ServedJob { link, sheet, output, private, members } = job;
    let Some(binary) = actions::place_test_binary(&shared.root, &output, private, &link) else {
        let slots = members.iter().map(|(m, _)| m.slot).collect();
        return Done::Abandoned { slots, explain: String::new() };
    };
    let sheet = write_stylesheet(binary.path(), &sheet);
    queue_members(binary, sheet, members, queue);
    Done::Progress
}

/// The environment variable a native test binary reads the block to start at
/// from. `cli/runtime/testing.rs` is the other half.
const RESUME: &str = "BURI_TEST_FROM";

/// The environment variable a native test binary reads the block to stop
/// before from, so several processes can share one binary's blocks.
const STOP: &str = "BURI_TEST_TO";

/// The environment variable a native test binary reads the seed
/// `tasks().anyOrder()` schedules with from. `cli/runtime/testing.rs` is the
/// other half, and the JavaScript path splices `$t.seed` instead of setting an
/// environment variable because it writes the artifact itself.
///
/// One number applies to every block. A comma-separated list is per block, in
/// the binary's own numbering, which is what a batched binary needs: its blocks
/// belong to several suites and a suite's order is its own.
const SEED: &str = "BURI_TEST_SEED";

/// The directory `platform/effect/testing`'s `snapshot` compares against and records into:
/// the package's own `test/__snapshots__`. `cli/runtime/snapshot.rs` is the
/// other half of this and of the two below.
const SNAPSHOT_DIR: &str = "BURI_SNAPSHOT_DIR";

/// Set to `1` by `--update`, which records what was painted instead of
/// comparing it.
const SNAPSHOT_UPDATE: &str = "BURI_SNAPSHOT_UPDATE";

/// The file the artifact's extracted stylesheet was written to.
///
/// `platform/effect/testing`'s `stylesheet()` is a JavaScript intrinsic: the sheet is a
/// string the JavaScript backend splices into the artifact, and a native binary
/// has nowhere to be handed one. A snapshot runs natively, so `buri test` hands
/// the sheet over the way it hands over the directory above. A program with no
/// static styles writes no file and sets nothing.
const SNAPSHOT_SHEET: &str = "BURI_SNAPSHOT_SHEET";

/// Where a package's goldens live, absolutely — the test binary runs from a
/// working directory of its own.
fn snapshot_dir(session: &Session, target: TargetId) -> String {
    session
        .root
        .join(&session.workspace.package(target.package).path)
        .join("test")
        .join("__snapshots__")
        .display()
        .to_string()
}

/// The intrinsic `platform/effect/testing`'s `snapshot` reaches, which is the whole of what
/// paints a golden.
///
/// Whether a suite takes a snapshot is asked of the monomorphized program
/// rather than of the sources ([`groups_of`]), for [`native_gap`]'s reason: what
/// a suite *reaches* is a property of the program and a list kept here would
/// drift from the runtime table the day the key moves.
const PAINT_KEY: &str = "host_testing.paint";

/// Writes the artifact's stylesheet beside the binary, and answers its path.
///
/// Beside the binary because that directory is one the build already owns:
/// nothing lands in a checked-in tree. An empty sheet writes nothing.
fn write_stylesheet(binary: &std::path::Path, sheet: &str) -> Option<String> {
    if sheet.is_empty() {
        return None;
    }
    let path = binary.with_extension("css");
    std::fs::write(&path, sheet).ok()?;
    Some(path.display().to_string())
}

/// The runtime's test-mode heap check, and its report knob
/// (`cli/runtime/memory.rs`).
///
/// **Both are handed to the test binary from this process's own environment**,
/// which is the one place `build/spawn.rs`'s cleared environment is deliberately
/// leaked through. The reason is that the audit is a question about the program
/// the toolchain *produced* — did it give back every block it took? — and there
/// is no other way to ask it of a binary this process spawned. A suite's record
/// does not depend on the answer: a clean run produces the same verdicts it
/// would have produced without the check, and a run that fails it produces no
/// record at all.
const HEAP_CHECK: [&str; 2] = ["BURI_RT_HEAP_CHECK", "BURI_RT_HEAP_REPORT"];

/// The status `cli/runtime/memory.rs` stops a program with when the heap check
/// fails. Not 1, which is what an ordinary abort exits with, so this number
/// means "the program finished and then failed the invariant".
const HEAP_CHECK_STATUS: i32 = 97;

/// The prefix every line the heap check prints carries.
const HEAP_CHECK_LINE: &str = "buri heap check:";

/// What the heap check said, out of a run's standard error.
fn heap_check_said(stderr: &str) -> Option<&str> {
    stderr.lines().find(|l| l.starts_with(HEAP_CHECK_LINE))
}

/// The order `anyOrder()` schedules with, for a suite whose key is `key`.
///
/// **The action key is the content hash** (D-10). It is a hash of every source
/// in the suite's closure and of the invocation that would build it, which is
/// exactly the question "is the record this run would produce still the record
/// the cache holds?" — so deriving the seed from it makes the two move together
/// by construction: an order changes exactly when the verdict that order
/// produced stops being reusable, and never on a run that changed nothing.
/// That is the whole of what keeps [`crate::build::cache::Cache::put`] pure
/// here; a clock or a generator in this position would store a record that the
/// next run could not have produced.
///
/// The first 32 hex digits, because a rank is taken modulo `n!` and a `u128`
/// holds `34!`. Which half of the digest is used is arbitrary and this is the
/// half `--explain` already prints the front of.
fn seed_of(key: &crate::build::cache::ActionKey) -> u128 {
    u128::from_str_radix(key.as_str().get(..32).unwrap_or("0"), 16).unwrap_or(0)
}

/// What one numbered block of a test binary did.
///
/// Numbered **per binary**, which is what makes this the same value whether the
/// binary holds one suite's tests or five: a block is a position in the entry
/// point the backend generated, and who owns it is the caller's question rather
/// than the runtime's.
enum Block {
    Passed,
    Failed { message: String, diff: Option<Diff>, order: Option<String> },
}

/// What running a test binary's blocks produced.
///
/// Four answers rather than one, because two of the ways a binary can end are
/// not a verdict on any block: the blocks all ran and every one of them may
/// have passed, and what failed is the *binary* — it did not give back every
/// block it allocated ([`Verdicts::HeapCheck`]), or it died with every block
/// already reported ([`Verdicts::Died`]). Attributing either to whichever block
/// happened to be last would name a test that is not the problem, so each comes
/// back as its own thing and the caller reports it against the suite.
enum Verdicts {
    /// One verdict per block, in the binary's own numbering.
    Blocks(Vec<Block>),
    /// The suite's `timeout_seconds` elapsed.
    TimedOut,
    /// The runtime's heap check stopped the binary, and this is the line it
    /// printed.
    HeapCheck(String),
    /// The binary ended badly with **no block of it left to blame**: every
    /// block it was asked to run wrote its `left` line and the process then
    /// died anyway — in a static initialiser after the last one, or on the way
    /// out through `exit`. This is how it ended.
    Died(String),
    /// The binary ended badly **before its first block**: it never wrote the
    /// line a process writes on reaching one (`cli/runtime/testing.rs`'s
    /// `note_started`). The operating system refusing to load it is the usual
    /// cause, and starting it again at the next block would fail the same way,
    /// once per block. This is how it ended.
    NotStarted(String),
}

/// Runs a native test binary until every block in `range` has a verdict, and
/// says what each did. `range` is `(from, to)` in the binary's numbering, `to`
/// exclusive, so several processes can share one binary.
///
/// One process per failure plus one. A block that aborted ended the process it
/// was in, so the blocks after it are run by the next: `RESUME` names the one to
/// start at and `buri_rt_test_enter` skips what is already reported. A clean run
/// is one process, which is the case that has to stay cheap.
///
/// `seeds` is [`SEED`]'s value, the same for every process this makes: the order
/// a block schedules in is a fact about the program and not about which process
/// happened to reach it, so a suite resumed after a failure is a suite scheduled
/// the way the run before it was.
///
/// A heap check's receipt goes to `notes`, for the report to print in order.
fn run_blocks(
    program: &str,
    limit: Option<u32>,
    range: (usize, usize),
    seeds: &str,
    snapshots: &[(&str, String)],
    notes: &mut String,
) -> std::io::Result<Verdicts> {
    let (first, count) = range;
    let mut blocks: Vec<Block> = Vec::with_capacity(count.saturating_sub(first));
    let mut from = first;
    let stop = count.to_string();
    while from < count {
        let start = from.to_string();
        // The snapshot entries are the same for every process this makes, for
        // `seeds`'s reason: where a golden lives is a fact about the package
        // and not about which process reached the block.
        let mut env: Vec<(&str, &str)> =
            vec![(RESUME, start.as_str()), (STOP, stop.as_str()), (SEED, seeds)];
        env.extend(snapshots.iter().map(|(name, value)| (*name, value.as_str())));
        let out = match execute(program, None, limit, &env)? {
            Execution::Finished(out) => out,
            Execution::TimedOut => return Ok(Verdicts::TimedOut),
        };
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        // The heap check's own line is **never swallowed**, whichever way the
        // run ended. A failure comes back as [`Verdicts::HeapCheck`] and is
        // reported against the suite; the other line it can print is the
        // `BURI_RT_HEAP_REPORT` receipt of a clean audit, which a reader asked
        // for and would not otherwise see, because a test binary's standard
        // error belongs to the runner rather than to the terminal.
        if out.status.code() == Some(HEAP_CHECK_STATUS) {
            return Ok(Verdicts::HeapCheck(
                heap_check_said(&stderr)
                    .unwrap_or(
                        "the test binary exited with the heap-check status and said nothing",
                    )
                    .to_string(),
            ));
        }
        if let Some(line) = heap_check_said(&stderr) {
            notes.push_str(line);
            notes.push('\n');
        }
        // A run that ended without aborting is a verdict for **every** block
        // from here on, because every one of them ran and none of them stopped
        // the process. Those verdicts are real, not assumed.
        if out.status.success() {
            while first + blocks.len() < count {
                blocks.push(Block::Passed);
            }
            break;
        }
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        // A process that never reached a block is a verdict on none of them,
        // and the next process would never reach one either. A failed
        // assertion is not this: its process started and says which block
        // ended it, so it is still one process per failing block.
        if !started(&stdout) {
            return Ok(Verdicts::NotStarted(how_it_ended(&out.status, &stderr)));
        }
        // The block the process was in, from the process itself. A run that
        // ended some other way — a signal — wrote no failure line, and the
        // block it was in is then the first one that never wrote a `left` line
        // either ([`died_after`]). A death with no block left to blame is not a
        // verdict on any test and is reported against the suite.
        let noted = noted_failure(&stdout).filter(|n| (from..count).contains(&n.at));
        let at = match &noted {
            Some(n) => n.at,
            None => match died_after(&stdout, from, count) {
                Some(at) => at,
                None => return Ok(Verdicts::Died(how_it_ended(&out.status, &stderr))),
            },
        };
        let message = match &noted {
            Some(n) => n.message.clone(),
            None => how_it_ended(&out.status, &stderr),
        };
        while first + blocks.len() < at {
            blocks.push(Block::Passed);
        }
        let (diff, order) = match noted {
            Some(n) => (n.diff, n.order),
            None => (None, None),
        };
        blocks.push(Block::Failed { message, diff, order });
        from = at + 1;
    }
    Ok(Verdicts::Blocks(blocks))
}

/// Whether a process reached its first block, which it says with one line on
/// standard output (`cli/runtime/testing.rs`'s `note_started`).
fn started(stdout: &str) -> bool {
    lines_of(stdout).any(|line| line.get("started").is_some())
}

/// Each line a native test binary wrote to standard output, as JSON.
///
/// The runtime writes one object per line. A line that is not one — the end of
/// a process killed mid-write — says nothing, and is skipped.
fn lines_of(stdout: &str) -> impl DoubleEndedIterator<Item = Value> + '_ {
    stdout.lines().filter_map(|line| crate::json::parse(line).ok())
}

/// A field of a record that is a string.
fn text_of(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_string)
}

/// A field of a record that is a block index.
fn index_of(value: &Value, name: &str) -> Option<usize> {
    match value.get(name) {
        Some(Value::Int(n)) => usize::try_from(*n).ok(),
        _ => None,
    }
}

/// The block a process that said nothing died in: the first one from `from` on
/// that never wrote its `left` line. `None` when every block wrote one, which
/// is a death outside all of them.
///
/// The runtime writes that line at the end of every block it runs
/// (`cli/runtime/testing.rs`'s `note_left`), so the blocks a dead process
/// finished are a *fact* rather than the guess this used to make. The guess was
/// "the block it was told to start at", and it cost a passing test its verdict
/// every time: a binary that died in its second block reported the first as
/// failing, then re-ran from the second and reported that one the same way, and
/// so on to the end of the suite — which is how one bad block became
/// `the run exited -1` against every test in a file.
fn died_after(stdout: &str, from: usize, count: usize) -> Option<usize> {
    let mut at = from;
    for line in lines_of(stdout).filter(|line| line.get("left").is_some()) {
        if let Some(i) = index_of(&line, "i") {
            at = at.max(i.saturating_add(1));
        }
    }
    (at < count).then_some(at)
}

/// How a run that reported no failure of its own ended, in the words the report
/// prints.
///
/// Whatever the binary wrote to standard error, where it wrote anything — that
/// is the program's own account and beats any of ours. Otherwise the status:
/// an exit code where there is one, and **a signal named as a signal** where
/// there is not, named by the signal that did it. `ExitStatus::code` is `None` for a process a signal killed, and
/// printing `-1` for it said the one thing that was certainly untrue.
fn how_it_ended(status: &std::process::ExitStatus, stderr: &str) -> String {
    let text = stderr.trim();
    if !text.is_empty() {
        return text.to_string();
    }
    match status.code() {
        Some(code) => format!("the run exited {code}"),
        None => format!("the run {}", crate::build::generators::how_it_ended(status)),
    }
}

/// One block's verdict as the runner's JSON, which is where a native record and
/// a JavaScript one become the same value.
///
/// A failure has the shape `$run` writes for a caught throw: the message and,
/// where the assertion had them, both rendered values. `order` is a sibling of
/// `error` rather than a field inside it, on both backends, because it is a
/// fact about the *run* and not about the throw. It is left out where there is
/// none.
fn record_of(name: &str, module: &str, block: &Block) -> Value {
    let mut fields =
        vec![("name", Value::str(name)), ("module", Value::str(module)), ("ms", Value::number(0))];
    match block {
        Block::Passed => fields.push(("ok", Value::Bool(true))),
        Block::Failed { message, diff, order } => {
            fields.push(("ok", Value::Bool(false)));
            let mut error = vec![("message", Value::str(message))];
            if let Some(d) = diff {
                error.push(("actual", Value::str(&d.actual)));
                error.push(("expected", Value::str(&d.expected)));
            }
            fields.push(("error", Value::object(error)));
            if let Some(note) = order {
                fields.push(("order", Value::str(note)));
            }
        }
    }
    Value::object(fields)
}

/// A native run's verdicts: recorded, cached where [`may_cache`] allows, and
/// read back.
///
/// Read back out of the record, so a verdict served from the cache and one just
/// produced are the same value by construction.
fn recorded<'b>(
    shared: &Shared,
    key: &crate::build::cache::ActionKey,
    tests: &[(String, String)],
    blocks: impl Iterator<Item = &'b Block>,
) -> Vec<Case> {
    let records =
        tests.iter().zip(blocks).map(|((name, module), block)| record_of(name, module, block));
    let record = Value::Array(records.collect()).to_string();
    let cases = parse_results(&record);
    if may_cache(&cases, &shared.flags) {
        crate::build::cache::Cache::open(&shared.root).put(key, record.as_bytes());
    }
    cases
}

// ---------------------------------------------------------------------------
// One binary for several suites
// ---------------------------------------------------------------------------
//
// A native suite's cost is almost none of it compilation. On the example
// monorepo — five suites, 19 tests, ~700 lines — the whole compiler is 1% of a
// cold `buri test //...`, and what is left is three charges paid *per suite*:
// one `cc` invocation (~100 ms, three quarters of it the C driver working out
// where libc is), one macOS first execution of a file nothing has run before
// (~200 ms, and it does not parallelise), and one front end. All three figures
// are measurements taken on that repository, on an M-series mac, rather than
// estimates: what makes the strategy below worth its complexity is that the
// per-suite charges are most of a cold run and the compiler is 1% of it.
//
// One binary for several suites collects the first two at once, and this is it.
// It is an **execution strategy and nothing else**: the same programs, the same
// verdicts, the same per-suite cache keys, and a suite that cannot join a batch
// runs exactly as it did before.
//
// # Which suites may share a binary
//
// A batch is one artifact, and `check_tags` is the rule about what may be in
// one: two tags that forbid each other may not appear anywhere in its closure
// (TAGS.md, and `actions::check_tags`). So the predicate is that rule applied to
// the *union* — a `client` suite and a `server` suite are two batches, however
// convenient one would have been. [`artifact_tags`] takes the union over the
// production closure **and** the test dependencies' closures, which is more than
// `check_tags` asks of a single suite and is the honest set for a binary that
// links both.
//
// Three more conditions, and each of them is a way for two suites to disagree
// about what building or running them means:
//
// - **The same platform.** Every member runs on [`default_platform`]'s answer,
//   which is one fact about the pass; a suite that *named* its platforms made a
//   request, and a request is served on its own.
// - **The same profile.** `--release` is the invocation's, so this is free — but
//   it is named because it is in every `codegen` key and a batch has one link.
// - **No `timeout_seconds`.** A limit is a suite's own, and a shared process
//   would make it a limit on everybody's tests together. A suite that declares
//   one keeps its own process.
//
// # Isolation, and where a verdict comes from
//
// A failed assertion is an abort and takes its process down, so the answer is
// the resuming runner the report-parity wave landed: blocks are numbered **per
// binary**, `BURI_TEST_FROM` names the one to start at, and `buri_rt_test_enter`
// skips what is already reported ([`run_blocks`]). Batching generalises the
// attribution rather than the mechanism — a block still names itself, and this
// maps the block to the suite that owns it through the module the test was
// declared in. One suite aborting therefore costs one process and no suite's
// report, which is exactly the isolation a binary per suite was buying.
//
// Each member runs in processes of its own, `BURI_TEST_TO` stopping each at the
// member's last block, so a batch's suites run side by side and a member whose
// process ends badly goes back to run alone without its neighbours.
//
// # Why no verdict can be served from the wrong place
//
// Nothing here is a cache key. `test_key` is unchanged, one per suite, and each
// suite's verdict is stored under its own — so an edit invalidates the suites it
// reaches and no others, and a suite whose verdict is already cached is not
// compiled into the batch at all. What is stored is the same JSON array the same
// suite would have produced alone, because it *is* the same monomorphized
// bodies: batching adds other suites' roots to the program, and a root neither
// changes another root's code nor can be reached from it. A suite's tests may
// not depend on the order they run in or on anything another test left behind
// (this module's header, TESTING.md "Running"), which is the same premise the
// resuming runner already rests on.
//
// The one thing a batch does change is the `link` key, which is the ordered list
// of the batch's `codegen` keys — so it is a different key rather than a wrong
// one, and two runs that batch different suites simply link twice.
//
// # Abandoning a batch
//
// Any reason at all to doubt the batch abandons it, silently, and every member
// then runs the way it would have. A failed front end, a failed monomorphization,
// an intrinsic the backend has no body for, a failed link: all four are answered
// per suite below, which is where the diagnostic can name one suite instead of
// five.
//
// # One batch, several binaries
//
// A batch that cannot be one binary is divided after monomorphization
// ([`groups_of`]): by snapshot directory, and by an estimate of its code size.
// Each group links on its own, and a group that fails to link sends only its
// own members back to run alone.

/// Loads the suites that can share binaries, a batch at a time, and queues a
/// [`Job::Batch`] for each.
///
/// Nothing at all is the answer that costs nothing and changes nothing: a pass
/// with one uncached suite, `--output=`, a toolchain with no native backend.
/// A slot this leaves unqueued is compiled on its own by [`solo`].
fn batch(session: &mut Session, args: &arguments::Args, slots: &mut [Slot], queue: &Queue) {
    // A batch is only ever the *default's* answer: `--output=` is a request,
    // and a request is served one suite at a time.
    if args.flags.output.is_some() {
        return;
    }
    let platform = crate::compiler::driver::host_native_platform();
    if !native_ready(platform, &args.flags) {
        return;
    }
    // Policy was checked when the slot was planned, so every suite here may be
    // compiled into a shared artifact as far as its own closure goes.
    let fresh: Vec<(TargetId, usize)> = slots
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.answer.is_none()
                && !s.queued
                && s.chosen == Chosen::Default
                && s.platform == platform
                && may_batch(session, s.target)
        })
        .map(|(i, s)| (s.target, i))
        .collect();
    if fresh.len() < 2 {
        return;
    }
    let targets: Vec<TargetId> = fresh.iter().map(|(t, _)| *t).collect();
    for members in batches_of(session, &targets) {
        // A batch of one is the path that already exists.
        if members.len() < 2 {
            continue;
        }
        let member_slots: Vec<usize> = members
            .iter()
            .filter_map(|m| fresh.iter().find(|(t, _)| t == m).map(|(_, i)| *i))
            .collect();
        queue_batch(session, &members, &member_slots, slots, queue);
    }
}

/// The members whose code holds one of `diagnostics`' errors, so a batch can go
/// on without them. Empty when an error is in nobody's code, or in everybody's.
fn broken_members(session: &Session, members: &[TargetId], diagnostics: &Diagnostics) -> Vec<TargetId> {
    let mut packages: Vec<crate::build::workspace::PackageId> = Vec::new();
    for d in diagnostics.items.iter().filter(|d| d.is_error()) {
        if d.span.file == crate::diagnostics::FileId::NONE {
            return Vec::new();
        }
        let file = session.map.get(d.span.file);
        match session.workspace.owning_package(&file.abs_path) {
            Some(p) => packages.push(p),
            None => return Vec::new(),
        }
    }
    let broken: Vec<TargetId> = members
        .iter()
        .copied()
        .filter(|&m| {
            let mut roots = vec![m];
            roots.extend(session.workspace.test_dep_edges(m).into_iter().map(|(dep, _)| dep));
            roots.iter().any(|&r| {
                session.workspace.closure(r).iter().any(|t| packages.contains(&t.package))
            })
        })
        .collect();
    if broken.len() == members.len() {
        return Vec::new();
    }
    broken
}

/// Whether a suite's *build file* leaves it free to share a binary.
///
/// Everything here is decidable before a byte is compiled, and each condition is
/// a way two suites would disagree about what building or running them means —
/// a declared platform is a request rather than a preference, and a declared
/// timeout has to bound one suite's process rather than several suites'.
fn may_batch(session: &Session, target: TargetId) -> bool {
    let Some(suite) = suite(session, target) else { return false };
    suite.backends.is_empty() && suite.timeout_seconds.is_none()
}
/// Every tag that would be carried by a suite's own test binary: its production
/// closure's, and its test dependencies' closures' too.
///
/// Wider than [`actions::check_tags`] asks of one suite, and deliberately: that
/// check is about what a target *ships*, and `test { dependencies }` is not
/// shipped — but it is linked into the suite's binary, so it is part of what a
/// shared binary would contain. A batch is refused on the wider set, which can
/// only ever refuse more.
fn artifact_tags(session: &Session, target: TargetId) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut roots = vec![target];
    roots.extend(session.workspace.test_dep_edges(target).into_iter().map(|(dep, _)| dep));
    for root in roots {
        for member in session.workspace.closure(root) {
            for tag in session.workspace.tags(member) {
                out.insert(tag.value.clone());
            }
        }
    }
    out
}

/// Whether two tags forbid each other. `forbids` is symmetric, so it is enough
/// for either declaration to name the other (TAGS.md).
fn tags_forbid(session: &Session, a: &str, b: &str) -> bool {
    let forbids = |one: &str, other: &str| {
        session
            .workspace
            .repo
            .tag(one)
            .is_some_and(|d| d.forbids_tags.iter().any(|f| f.value == other))
    };
    forbids(a, b) || forbids(b, a)
}

/// Partitions the candidates into batches no one of which could fail
/// `check_tags`.
///
/// Greedy and first-fit, over the pass's own target order, so the partition is a
/// function of the repository rather than of anything this run happened to do.
/// Each candidate is already internally consistent — `check_policy` said so —
/// so a union is consistent exactly when no tag of one member forbids a tag of
/// another, which is what the cross product below asks.
///
/// First-fit rather than optimal: the packing that minimises the number of
/// batches is the graph-colouring problem, and what this is for is turning five
/// links into one on a repository whose tags mostly do not forbid anything.
fn batches_of(session: &Session, candidates: &[TargetId]) -> Vec<Vec<TargetId>> {
    let mut batches: Vec<(std::collections::BTreeSet<String>, Vec<TargetId>)> = Vec::new();
    for &target in candidates {
        let tags = artifact_tags(session, target);
        let slot = batches.iter_mut().find(|(carried, _)| {
            !carried.iter().any(|a| tags.iter().any(|b| tags_forbid(session, a, b)))
        });
        match slot {
            Some((carried, members)) => {
                carried.extend(tags);
                members.push(target);
            }
            None => batches.push((tags, vec![target])),
        }
    }
    batches.into_iter().map(|(_, members)| members).collect()
}

/// The module path each of a suite's test sources becomes.
///
/// The loader's own rule (`modules.rs::load_package_source`), restated here
/// because this is the only thing that maps a block back to the suite that owns
/// it: a root records the module its `test` block was declared in, and a module
/// is named by its package-relative path from the repository root.
fn test_modules_of(session: &Session, target: TargetId) -> Vec<String> {
    let Some(suite) = suite(session, target) else { return Vec::new() };
    let pkg = session.workspace.package(target.package);
    suite.sources.iter().map(|src| pkg.module_path(&src.value)).collect()
}

/// Loads one batch as one compilation and queues its [`Job::Batch`].
///
/// A broken member is found once the batch is checked ([`Done::Broken`]), and
/// [`drive`] queues the batch again without it.
fn queue_batch(
    session: &mut Session,
    members: &[TargetId],
    member_slots: &[usize],
    slots: &mut [Slot],
    queue: &Queue,
) {
    // One unit per member, in the pass's order, which is the order their test
    // sources load in and therefore the order the binary's blocks come out in.
    let units: Vec<Unit> = members
        .iter()
        .map(|&target| Unit { target: Some(target), platform: None, entry: None, with_tests: true })
        .collect();
    let loading = crate::compiler::driver::load_all(
        Some(&session.workspace),
        &mut session.map,
        &mut session.parsed,
        &units,
    );
    let platform = crate::compiler::driver::host_native_platform();
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    let key_of = |i: usize| slots.get(i).map(|s| s.key.clone());
    let info = BatchInfo {
        members: members.to_vec(),
        member_slots: member_slots.to_vec(),
        // Which suite owns each module that declares tests. Built from the
        // build files rather than from the program, so a module the batch
        // loaded for some other reason cannot be mistaken for a suite's.
        owners: members
            .iter()
            .enumerate()
            .flat_map(|(i, &t)| test_modules_of(session, t).into_iter().map(move |m| (m, i)))
            .collect(),
        keys: member_slots.iter().filter_map(|&i| key_of(i)).collect(),
        labels: members.iter().map(|&t| session.workspace.label(t)).collect(),
        privates: members.iter().map(|&t| actions::private_test_binary(session, t, &output)).collect(),
        dirs: members.iter().map(|&t| snapshot_dir(session, t)).collect(),
        platform,
        output,
    };
    for &i in member_slots {
        if let Some(s) = slots.get_mut(i) {
            s.queued = true;
        }
    }
    let bytes = build_bytes(loading.source_bytes(&session.map));
    let job = BatchJob { info: std::sync::Arc::new(info), loading, map: session.map.shared(), bytes };
    queue.push(Job::Batch(Box::new(job)), bytes);
}

/// What every binary of one batch shares, each member's at its position.
struct BatchInfo {
    members: Vec<TargetId>,
    member_slots: Vec<usize>,
    /// Each test source's module and the member that owns it.
    owners: Vec<(String, usize)>,
    keys: Vec<crate::build::cache::ActionKey>,
    labels: Vec<String>,
    /// Where each member's own binary would go; a group's goes where its first
    /// member's would.
    privates: Vec<std::path::PathBuf>,
    /// Each member's snapshot directory.
    dirs: Vec<String>,
    platform: Platform,
    output: crate::build::buildfile::Output,
}

impl BatchInfo {
    fn owner_of(&self, module: &str) -> Option<usize> {
        self.owners.iter().find(|(m, _)| m == module).map(|(_, i)| *i)
    }

    fn slot_of(&self, member: usize) -> usize {
        self.member_slots.get(member).copied().unwrap_or(usize::MAX)
    }
}

/// One batch, loaded.
struct BatchJob {
    info: std::sync::Arc<BatchInfo>,
    loading: crate::compiler::driver::Loading,
    /// The session's map as the load left it.
    map: std::sync::Arc<crate::diagnostics::SourceMap>,
    /// What the build is expected to hold ([`build_bytes`]).
    bytes: u64,
}

/// One batch's front end: one type check, one program per group ([`groups_of`]),
/// and a binary for each.
///
/// Every abandonment says nothing. That is the whole of the safety argument: a
/// batch that is not certain is not a batch, and its suites are compiled on
/// their own by [`solo`], where a diagnostic — a gap the backend has no body
/// for, most of all — can name the one suite it belongs to.
///
/// A type check that fails goes back as [`Done::Broken`], so [`drive`] can try
/// again without the members whose code failed it ([`broken_members`]).
fn batch_job(job: BatchJob, held: Held, queue: &Queue, tell: &Tell, shared: &Shared) -> Done {
    let BatchJob { info, loading, map, bytes } = job;
    let abandoned = || Done::Abandoned { slots: info.member_slots.clone(), explain: String::new() };
    let mut analysis = crate::compiler::driver::check(loading, Some(&shared.workspace), &map);
    drop(map);
    if analysis.diagnostics.has_errors() {
        return Done::Broken {
            members: info.members.clone(),
            member_slots: info.member_slots.clone(),
            diagnostics: analysis.diagnostics,
        };
    }
    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        module_paths.clone(),
        &mut diagnostics,
        monomorphize::Roots::Tests,
    );
    if diagnostics.has_errors() || program.roots.tests().is_empty() {
        return abandoned();
    }
    // The gap probe cannot say *which* member reaches the intrinsic it names, so
    // a batch with a gap in it is abandoned and each member asks for itself.
    if native_gap(info.platform, &shared.flags, &program, &analysis.checked.tables).is_some() {
        return abandoned();
    }
    // What a `--filter` leaves out, per suite, counted before the roots are
    // narrowed.
    let mut skipped = vec![0usize; info.members.len()];
    if let Some(f) = &shared.flags.filter {
        for test in program.roots.tests() {
            if !test.name.contains(f.as_str()) {
                if let Some(n) = info.owner_of(&test.module).and_then(|i| skipped.get_mut(i)) {
                    *n += 1;
                }
            }
        }
        if let monomorphize::ProgramRoots::Tests(tests) = &mut program.roots {
            tests.retain(|t| t.name.contains(f.as_str()));
        }
    }
    let Some(selected) = selected_of(&program, &|m| info.owner_of(m)) else { return abandoned() };
    let groups = groups_of(&info.dirs, &program, &selected, batch_limit());
    if let [group] = groups.as_slice() {
        let tables = std::sync::Arc::new(std::mem::take(&mut analysis.checked.tables));
        drop(analysis);
        return submit_group(&info, &skipped, group, &selected, program, tables, held, queue, tell, shared);
    }
    // Each group is monomorphized again from the batch's one check, rooted at
    // its own tests, so each binary holds only its own code. Each is a heavy
    // job of its own, so the groups build side by side within the budget. The
    // batch's bytes are divided between them: together they hold its check,
    // and a program each.
    drop(program);
    let share = bytes / u64::try_from(groups.len()).unwrap_or(1).max(1);
    let tables = std::sync::Arc::new(analysis.checked.tables.clone());
    let analysis = std::sync::Arc::new(analysis);
    let skipped = std::sync::Arc::new(skipped);
    for group in groups {
        let modules: Vec<String> = info
            .owners
            .iter()
            .filter(|(_, i)| group.members.contains(i))
            .map(|(m, _)| m.clone())
            .collect();
        queue.push(
            Job::Part(Box::new(PartJob {
                info: std::sync::Arc::clone(&info),
                analysis: std::sync::Arc::clone(&analysis),
                tables: std::sync::Arc::clone(&tables),
                module_paths: module_paths.clone(),
                skipped: std::sync::Arc::clone(&skipped),
                group,
                modules,
            })),
            share,
        );
    }
    drop(held);
    Done::Progress
}

/// One binary of a batch that divides into several.
struct PartJob {
    info: std::sync::Arc<BatchInfo>,
    /// The batch's one check, which every part reads.
    analysis: std::sync::Arc<crate::compiler::driver::Analysis>,
    tables: std::sync::Arc<crate::compiler::semantics::types::Tables>,
    module_paths: Vec<String>,
    skipped: std::sync::Arc<Vec<usize>>,
    group: Group,
    /// The test modules of the group's members, which root its program.
    modules: Vec<String>,
}

/// Monomorphizes one group of a batch from the batch's check, and builds it.
/// A group that fails sends only its own members back to run alone.
fn part(job: PartJob, held: Held, queue: &Queue, tell: &Tell, shared: &Shared) -> Done {
    let PartJob { info, analysis, tables, module_paths, skipped, group, modules } = job;
    let mut diagnostics = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diagnostics,
        monomorphize::Roots::TestsIn(&modules),
    );
    drop(analysis);
    let abandoned = || Done::Abandoned {
        slots: group.members.iter().map(|&i| info.slot_of(i)).collect(),
        explain: String::new(),
    };
    if diagnostics.has_errors() {
        return abandoned();
    }
    if let (Some(f), monomorphize::ProgramRoots::Tests(tests)) =
        (&shared.flags.filter, &mut program.roots)
    {
        tests.retain(|t| t.name.contains(f.as_str()));
    }
    let Some(mine) = selected_of(&program, &|m| info.owner_of(m)) else { return abandoned() };
    submit_group(&info, &skipped, &group, &mine, program, tables, held, queue, tell, shared)
}

/// Links one group's binary and queues its members' processes, or answers its
/// members when a `--filter` left it nothing to run.
#[allow(
    clippy::too_many_arguments,
    reason = "the batch, what the filter skipped, the group, its tests, its program and \
              tables, the job's claim, the queue, the channel and the worker's shared \
              state: none derivable from another"
)]
fn submit_group(
    info: &BatchInfo,
    skipped: &[usize],
    group: &Group,
    selected: &[Selected],
    program: monomorphize::Program,
    tables: std::sync::Arc<crate::compiler::semantics::types::Tables>,
    held: Held,
    queue: &Queue,
    tell: &Tell,
    shared: &Shared,
) -> Done {
    let skipped_of = |i: usize| skipped.get(i).copied().unwrap_or(0);
    let nothing = |i: usize| Done::Answer {
        slot: info.slot_of(i),
        answer: Ok(Ran { cases: Vec::new(), skipped: skipped_of(i), roots: Vec::new() }),
        explain: String::new(),
        notes: String::new(),
        built: Some(Built::Nothing { skipped: skipped_of(i) }),
    };
    if selected.is_empty() {
        for &i in &group.members {
            tell.tell(nothing(i));
        }
        return Done::Progress;
    }
    let Some(&first) = group.members.first() else { return Done::Progress };
    // One seed per *block*, because a suite's order is its own key's: a suite
    // schedules the same way batched or alone.
    let seed = |i: usize| info.keys.get(i).map(seed_of).unwrap_or_default();
    let seeds: Vec<String> = selected.iter().map(|s| seed(s.owner).to_string()).collect();
    let tests = program.roots.tests();
    let members: Vec<(usize, MemberSpec)> = group
        .members
        .iter()
        .filter_map(|&i| {
            let mine: Vec<usize> =
                (0..selected.len()).filter(|&b| selected.get(b).is_some_and(|s| s.owner == i)).collect();
            Some((
                i,
                MemberSpec {
                    slot: info.slot_of(i),
                    key: info.keys.get(i)?.clone(),
                    ranges: ranges_of(&mine),
                    tests: mine
                        .iter()
                        .filter_map(|&b| selected.get(b))
                        .map(|s| (s.name.clone(), s.module.clone()))
                        .collect(),
                    roots: mine
                        .iter()
                        .filter_map(|&b| tests.get(b))
                        .map(|t| Root { name: t.name.clone(), module: t.module.clone(), span: t.span })
                        .collect(),
                    skipped: skipped_of(i),
                    snapshot_dir: info.dirs.get(i)?.clone(),
                    paints: group.painters.contains(&i),
                    // A suite that declared a limit is not in a batch.
                    limit: None,
                    on_timeout: Diagnostics::new(),
                    remember: None,
                },
            ))
        })
        .collect();
    // A member a `--filter` left nothing in is answered here, with no process.
    let (members, empty): (Vec<_>, Vec<_>) =
        members.into_iter().partition(|(_, m)| !m.ranges.is_empty());
    for (i, _) in &empty {
        tell.tell(nothing(*i));
    }
    let members: Vec<MemberSpec> = members.into_iter().map(|(_, m)| m).collect();
    // Told before the link queues anything, so `drive` knows the first member
    // waits for it before any member's answer can arrive.
    if let Some(m) = members.first() {
        tell.tell(Done::Linking { slot: m.slot });
    }
    let label = group
        .members
        .iter()
        .filter_map(|&i| info.labels.get(i).cloned())
        .collect::<Vec<_>>()
        .join(",");
    let job = GroupJob {
        label,
        private: info.privates.get(first).cloned().unwrap_or_default(),
        output: info.output.clone(),
        program,
        tables,
        seeds: seeds.join(","),
        members,
    };
    build_group(job, held, queue, shared)
}

/// Consecutive runs of block indices, as `(from, to)` with `to` exclusive.
fn ranges_of(blocks: &[usize]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &b in blocks {
        match out.last_mut() {
            Some((_, to)) if *to == b => *to = b + 1,
            _ => out.push((b, b + 1)),
        }
    }
    out
}

/// One binary for some of a batch's members.
struct GroupJob {
    label: String,
    private: std::path::PathBuf,
    output: crate::build::buildfile::Output,
    program: monomorphize::Program,
    tables: std::sync::Arc<crate::compiler::semantics::types::Tables>,
    /// `BURI_TEST_SEED`: one per block, in the binary's numbering.
    seeds: String,
    members: Vec<MemberSpec>,
}

/// One member of a group, as its own process will run it.
struct MemberSpec {
    slot: usize,
    key: crate::build::cache::ActionKey,
    /// The member's blocks, in the binary's numbering.
    ranges: Vec<(usize, usize)>,
    /// Each block's title and module, in block order.
    tests: Vec<(String, String)>,
    roots: Vec<Root>,
    skipped: usize,
    snapshot_dir: String,
    paints: bool,
    /// The suite's `timeout_seconds`. Only a binary run again has one: a
    /// suite that declared a limit is not in a batch.
    limit: Option<u32>,
    on_timeout: Diagnostics,
    /// The `link` key and stylesheet of a binary this run linked, so the
    /// member's answer can record it. `None` for a binary run again.
    remember: Option<(crate::build::cache::ActionKey, std::sync::Arc<String>)>,
}

/// Links a group's binary and queues a process per member, ahead of any new
/// build: those finish work already paid for.
///
/// A link that fails sends every member back to run alone.
fn build_group(job: GroupJob, held: Held, queue: &Queue, shared: &Shared) -> Done {
    let GroupJob { label, private, output, mut program, tables, seeds, members } = job;
    let sheet = program.stylesheet.clone();
    let mut diagnostics = Diagnostics::new();
    let (built, explain) = crate::build::cache::holding_explain(|| {
        actions::link_test_binary(&shared.root, &label, private, &output, &shared.flags, &mut program, &tables, &mut diagnostics)
    });
    drop(program);
    drop(tables);
    drop(held);
    let Ok((binary, link)) = built else {
        return Done::Abandoned { slots: members.iter().map(|m| m.slot).collect(), explain };
    };
    let written = write_stylesheet(binary.path(), &sheet);
    let sheet = std::sync::Arc::new(sheet);
    let seeds = std::sync::Arc::new(seeds);
    let first = members.first().map_or(usize::MAX, |m| m.slot);
    let members = members
        .into_iter()
        .map(|mut m| {
            m.remember = Some((link.clone(), std::sync::Arc::clone(&sheet)));
            (m, std::sync::Arc::clone(&seeds))
        })
        .collect();
    queue_members(binary, written, members, queue);
    Done::Built { slot: first, explain }
}

/// Queues a process per member of a linked binary, at the front of the queue:
/// they finish work already paid for. Each member comes with the seeds its
/// processes are handed.
fn queue_members(
    binary: actions::TestBinary,
    sheet: Option<String>,
    members: Vec<(MemberSpec, std::sync::Arc<String>)>,
    queue: &Queue,
) {
    let binary = std::sync::Arc::new(binary);
    // Reversed, because each goes to the front: the first member runs first.
    for (spec, seeds) in members.into_iter().rev() {
        // A limit bounds the suite's one process, so a suite with one is not
        // divided between several.
        let size = if spec.limit.is_some() { usize::MAX } else { BLOCKS_PER_PROCESS };
        let chunks = chunks_of(&spec.ranges, size);
        let spec = std::sync::Arc::new(spec);
        let gathered = std::sync::Arc::new(std::sync::Mutex::new(Gathered {
            left: chunks.len(),
            ..Gathered::default()
        }));
        for range in chunks.into_iter().rev() {
            queue.push_first(Job::Member(MemberJob {
                binary: std::sync::Arc::clone(&binary),
                seeds: std::sync::Arc::clone(&seeds),
                sheet: sheet.clone(),
                spec: std::sync::Arc::clone(&spec),
                range,
                gathered: std::sync::Arc::clone(&gathered),
            }));
        }
    }
}

/// The fewest blocks a member's process is given, when the member has more:
/// a process costs a launch, and a test usually costs less.
const BLOCKS_PER_PROCESS: usize = 4;

/// `ranges` cut into pieces of about `size` blocks, so one long suite's blocks
/// run in several processes at once. Each is `(from, to)`, `to` exclusive.
fn chunks_of(ranges: &[(usize, usize)], size: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &(from, to) in ranges {
        let mut at = from;
        while at < to {
            let end = to.min(at.saturating_add(size.max(1)));
            out.push((at, end));
            at = end;
        }
    }
    out
}

/// Some of one member's blocks, in its group's binary. The binary is released
/// when the last process has finished with it.
struct MemberJob {
    binary: std::sync::Arc<actions::TestBinary>,
    seeds: std::sync::Arc<String>,
    sheet: Option<String>,
    spec: std::sync::Arc<MemberSpec>,
    range: (usize, usize),
    gathered: std::sync::Arc<std::sync::Mutex<Gathered>>,
}

/// What a member's processes have reported so far.
#[derive(Default)]
struct Gathered {
    /// Each block's verdict with its index, and each process's notes with
    /// its first block, so the answer doesn't depend on which finished first.
    blocks: Vec<(usize, Block)>,
    notes: Vec<(usize, String)>,
    left: usize,
    failed: bool,
    timed_out: bool,
}

/// Runs some of one member's blocks in a process of its own, and answers for
/// the member once its last process has finished.
///
/// A process that ends any way but with a verdict per block — a heap check that
/// failed, a death after the last block, a binary that did not start — sends
/// the member back to run alone, where the same binary is built for it and the
/// problem is reported against it.
fn run_member(job: MemberJob, queue: &Queue, shared: &Shared) -> Done {
    if job.spec.paints && !shared.claim(&job.spec.snapshot_dir, job.spec.slot) {
        // Another suite is painting there. Back of the queue, rather than a
        // worker held waiting.
        std::thread::sleep(Duration::from_millis(20));
        queue.push(Job::Member(job), 0);
        return Done::Progress;
    }
    let MemberJob { binary, seeds, sheet, spec, range, gathered } = job;
    let snapshots = snapshot_env(&spec.snapshot_dir, shared.flags.update, sheet);
    let mut notes = String::new();
    let ran = run_blocks(&binary.path().display().to_string(), spec.limit, range, &seeds, &snapshots, &mut notes);
    if spec.paints {
        shared.release(&spec.snapshot_dir, spec.slot);
    }
    let mut all = gathered.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match ran {
        Ok(Verdicts::Blocks(mine)) => {
            all.blocks.extend((range.0..).zip(mine));
            all.notes.push((range.0, notes));
        }
        Ok(Verdicts::TimedOut) => {
            all.timed_out = true;
            all.notes.push((range.0, notes));
        }
        _ => all.failed = true,
    }
    all.left = all.left.saturating_sub(1);
    if all.left > 0 {
        return Done::Progress;
    }
    all.notes.sort_by_key(|(i, _)| *i);
    if all.timed_out {
        let notes: String = all.notes.iter().map(|(_, n)| n.as_str()).collect();
        let answer = Err(spec.on_timeout.clone());
        return Done::Answer { slot: spec.slot, answer, explain: String::new(), notes, built: linked_of(&spec) };
    }
    if all.failed {
        return Done::Abandoned { slots: vec![spec.slot], explain: String::new() };
    }
    all.blocks.sort_by_key(|(i, _)| *i);
    let notes: String = all.notes.iter().map(|(_, n)| n.as_str()).collect();
    let cases = recorded(shared, &spec.key, &spec.tests, all.blocks.iter().map(|(_, block)| block));
    Done::Answer {
        slot: spec.slot,
        answer: Ok(Ran { cases, skipped: spec.skipped, roots: spec.roots.clone() }),
        explain: String::new(),
        notes,
        built: linked_of(&spec),
    }
}

/// The record of a member's place in a binary this run linked.
fn linked_of(spec: &MemberSpec) -> Option<Built> {
    let (link, sheet) = spec.remember.as_ref()?;
    Some(Built::Linked(Box::new(Linked {
        link: link.clone(),
        sheet: sheet.to_string(),
        paints: spec.paints,
        skipped: spec.skipped,
        ranges: spec.ranges.clone(),
        roots: spec.roots.clone(),
    })))
}

/// Each of a program's tests with the member that owns it, in block order.
///
/// `None` when a test belongs to no member. That cannot arise — only a member's
/// test sources are loaded with `Role::TestSource` — and if it ever did, the
/// batch is abandoned rather than a test attributed to a suite that does not
/// own it.
fn selected_of(
    program: &monomorphize::Program,
    owner_of: &impl Fn(&str) -> Option<usize>,
) -> Option<Vec<Selected>> {
    program
        .roots
        .tests()
        .iter()
        .map(|test| {
            Some(Selected {
                owner: owner_of(&test.module)?,
                name: test.name.clone(),
                module: test.module.clone(),
                func: test.func.index(),
            })
        })
        .collect()
}

/// One test block of a batch: the member that owns it, its title and module,
/// and the function it is.
struct Selected {
    owner: usize,
    name: String,
    module: String,
    func: usize,
}

/// Some of a batch's members, which share one binary.
struct Group {
    /// Positions in the batch's member list, in that list's order.
    members: Vec<usize>,
    /// The snapshot directory of the members that take snapshots, where any
    /// do. One process is handed one directory, so a group holds the suites
    /// that paint into one package's directory at most.
    paints_into: Option<String>,
    /// The members whose tests take a snapshot.
    painters: Vec<usize>,
    /// The functions the members' tests reach, one row per function.
    reaches: Vec<bool>,
}

/// The most code one batched test binary may hold, in bytes, by default.
///
/// macOS cannot load an executable of about two gigabytes: dyld reports
/// `Library not loaded: libSystem.B.dylib`, intermittently from 1.96 GB and
/// always at 2.09 GB, and every test in the binary fails.
///
/// 128 MB is far below that, and it is about speed and memory. Groups build side
/// by side, so a gigabyte group was the slowest build in the pass, and it doubled
/// peak memory: 22 GB against 12 GB on an 80-suite repository, for no faster run.
const BATCH_BYTES: u64 = 128 * 1024 * 1024;

/// The environment variable that replaces [`BATCH_BYTES`].
const BATCH_BYTES_VARIABLE: &str = "BURI_TEST_BATCH_BYTES";

/// The most code one batched test binary may hold: `BURI_TEST_BATCH_BYTES`
/// where it is set to a number, and [`BATCH_BYTES`] otherwise.
fn batch_limit() -> u64 {
    std::env::var(BATCH_BYTES_VARIABLE).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(BATCH_BYTES)
}

/// Machine code per node of a function's tree, in bytes.
///
/// Measured rather than derived. A debug test binary of 42 suites from the
/// repository the limit was set against was 2.18 GB, for 11.9 million nodes of
/// monomorphized tree in the functions its tests reach: 183 bytes a node. This
/// rounds up.
const BYTES_PER_NODE: u64 = 192;

/// How much machine code a function is likely to become, in bytes.
///
/// An estimate, taken before the middle end and the backend have run, because
/// it decides which suites share a binary and so has to be known before any of
/// them is compiled. Inlining moves code between functions without changing
/// how much there is by much, so a count of the tree's nodes is a fair measure.
fn estimated_bytes(func: &monomorphize::Func) -> u64 {
    fn nodes(e: &crate::compiler::semantics::typed::Expr) -> u64 {
        let mut n = 1u64;
        crate::compiler::semantics::typed::children(e, &mut |c| n = n.saturating_add(nodes(c)));
        n
    }
    func.body().map_or(0, |b| nodes(b).saturating_mul(BYTES_PER_NODE))
}

/// Divides a batch's members between binaries.
///
/// Two things can stop two suites from sharing a binary once their tags allow
/// it. One process is handed one snapshot directory, so two suites that paint
/// into two packages' directories need two processes. And a binary's code has
/// a size past which it cannot be loaded ([`batch_limit`]). Every other suite
/// can go anywhere.
///
/// First fit, over the batch's own order, so the division is a function of the
/// repository rather than of anything this run happened to do. A group's size
/// is the code its members reach between them, each function counted once,
/// which is what its binary will hold. A suite larger than the limit on its own
/// is a group of one, which is what it would have been without batching.
///
/// `dirs` is each member's snapshot directory, in the batch's order.
fn groups_of(
    dirs: &[String],
    program: &monomorphize::Program,
    selected: &[Selected],
    limit: u64,
) -> Vec<Group> {
    let funcs = program.funcs.len();
    let callees: Vec<Vec<usize>> =
        crate::parallel::map(funcs, |f| crate::compiler::middle::dce::callees(program, f));
    let bytes: Vec<u64> = program.funcs.iter().map(estimated_bytes).collect();

    // What each member reaches from its own test blocks.
    let reach = |member: usize| -> Vec<usize> {
        let mut seen = vec![false; funcs];
        let mut work: Vec<usize> =
            selected.iter().filter(|s| s.owner == member).map(|s| s.func).collect();
        let mut out = Vec::new();
        while let Some(f) = work.pop() {
            match seen.get_mut(f) {
                Some(s) if !*s => *s = true,
                _ => continue,
            }
            out.push(f);
            if let Some(next) = callees.get(f) {
                work.extend(next.iter().copied());
            }
        }
        out
    };

    let mut groups: Vec<(Group, u64)> = Vec::new();
    for (member, dir) in dirs.iter().enumerate() {
        let reached = reach(member);
        let paints = reached
            .iter()
            .any(|&f| program.funcs.get(f).and_then(|f| f.intrinsic_key()) == Some(PAINT_KEY));
        let dir = paints.then(|| dir.clone());
        let added = |held: &[bool]| -> u64 {
            reached
                .iter()
                .filter(|&&f| !held.get(f).copied().unwrap_or(false))
                .map(|&f| bytes.get(f).copied().unwrap_or(0))
                .fold(0u64, u64::saturating_add)
        };
        let fits = groups.iter().position(|(group, size)| {
            let dir_ok = match (&dir, &group.paints_into) {
                (Some(mine), Some(theirs)) => mine == theirs,
                _ => true,
            };
            dir_ok && size.saturating_add(added(&group.reaches)) <= limit
        });
        let slot = fits.unwrap_or_else(|| {
            let empty = Group {
                members: Vec::new(),
                paints_into: None,
                painters: Vec::new(),
                reaches: vec![false; funcs],
            };
            groups.push((empty, 0));
            groups.len() - 1
        });
        if let Some((group, size)) = groups.get_mut(slot) {
            *size = size.saturating_add(added(&group.reaches));
            for &f in &reached {
                if let Some(h) = group.reaches.get_mut(f) {
                    *h = true;
                }
            }
            group.members.push(member);
            if paints {
                group.painters.push(member);
            }
            if dir.is_some() {
                group.paints_into = dir;
            }
        }
    }
    groups.into_iter().map(|(group, _)| group).collect()
}

/// What a native test binary said about the block that ended it.
struct Noted {
    at: usize,
    message: String,
    diff: Option<Diff>,
    /// The order sentence the runtime assembled, where the block scheduled
    /// anything. `cli/runtime/testing.rs`'s `task_order_note` writes it and
    /// `note_failure` puts it on the line; this is the field it arrives in.
    order: Option<String>,
}

/// The line a native test binary writes when a block aborts.
///
/// The last object on standard output that carries a message, because the
/// process writes one and then stops; reading the last rather than the first
/// means a suite that somehow produced two is reported by the one that ended
/// it.
fn noted_failure(stdout: &str) -> Option<Noted> {
    // The last object **carrying a message**: the stream also holds one line
    // per block that returned, and those carry an index and nothing else
    // (`cli/runtime/testing.rs`'s `note_left`). A block that aborted writes its
    // line after them, so the last message is still this process's failure.
    let line = lines_of(stdout).rev().find(|line| text_of(line, "message").is_some())?;
    Some(Noted {
        at: index_of(&line, "i")?,
        message: text_of(&line, "message").unwrap_or_default(),
        diff: diff_of(&line),
        order: text_of(&line, "order"),
    })
}

/// Both rendered values of a failed comparison. Half a diff is no diff: one
/// side alone is not something the other can be printed against.
fn diff_of(value: &Value) -> Option<Diff> {
    Some(Diff { actual: text_of(value, "actual")?, expected: text_of(value, "expected")? })
}

/// Attaches each case to the source location of the test it names.
///
/// By title *and module*. Two files of one suite may use one title — that is
/// legal, and each reports its own failure at its own line — so a match on the
/// title alone gives the second file's failure the first file's location, in
/// the first file. Two tests sharing a title inside one file cannot arise:
/// `duplicate-test` refuses them before anything is compiled.
fn locate(session: &Session, roots: &[Root], cases: &mut [Case]) {
    for c in cases.iter_mut() {
        if let Some(t) = roots.iter().find(|t| t.name == c.name && t.module == c.module) {
            if !t.span.is_none() {
                let f = session.map.get(t.span.file);
                let (line, col) = f.line_col(t.span.start);
                c.location = Some(format!("{}:{line}:{col}", f.name));
            }
        }
    }
}

/// The diagnostic a suite that ran past its own `timeout_seconds` gets.
fn timed_out(session: &Session, target: TargetId, limit: Option<u32>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    let span = suite(session, target).map(|x| x.span).unwrap_or(Span::NONE);
    let seconds = limit.unwrap_or(0);
    diagnostics.push(
        Diagnostic::templated("test-timeout", span)
            .with_bind("target", session.workspace.label(target))
            .with_bind("seconds", seconds.to_string()),
    );
    diagnostics
}

/// The diagnostic a suite whose binary failed the runtime's heap check gets.
///
/// **A leak is the toolchain's bug and not the suite's**, which is why this
/// says so rather than pointing at a test: the blocks a program allocates are
/// released by code `middle::rc` inserted, and a block that outlived the
/// program means that pass got the count wrong. `line` is the runtime's own
/// sentence — how many blocks, how many bytes, or which reference operation
/// reached a freed one — because the number is what a report needs and this
/// process has no better words for it.
fn heap_check_failed(label: &str, line: &str) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(
        Diagnostic::error(
            Span::NONE,
            format!("the test binary for {label} failed the heap check: {line}"),
        )
        .with_fix(
            "this is a toolchain bug rather than a bug in the suite: a program's memory is \
             released by code the compiler inserted. Please report it with the suite that \
             provoked it",
        ),
    );
    diagnostics
}

/// The diagnostic a suite gets whose binary died with every block of it already
/// reported.
///
/// **Against the suite and not against a test**, for [`heap_check_failed`]'s
/// reason: every block ran and said so, so there is no test whose behaviour
/// this is. What is left is a program that could not get out of its own `main`
/// — a static initialiser after the last block, or the way out through `exit` —
/// and naming a passing test for it is what this whole path exists to stop.
fn the_binary_died(label: &str, how: &str) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(
        Diagnostic::error(
            Span::NONE,
            format!("the test binary for {label} ran every block and then died: {how}"),
        )
        .with_fix(
            "this is a toolchain bug rather than a bug in the suite: every test in it \
             finished. Please report it with the suite that provoked it",
        ),
    );
    diagnostics
}

/// The diagnostic a suite gets whose binary ended before its first block.
///
/// Against the suite, for [`the_binary_died`]'s reason: no test ran, so no test
/// is to blame. `how` is what the process said on its way out, which for a
/// binary the operating system would not load is the loader's own sentence.
fn the_binary_did_not_start(label: &str, how: &str) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(
        Diagnostic::error(
            Span::NONE,
            format!("the test binary for {label} could not start: {how}"),
        )
        .with_fix(
            "no test in it ran. If the message is the loader's, the binary may be too large \
             to load; otherwise this is a toolchain bug, so please report it with the suite \
             that provoked it",
        ),
    );
    diagnostics
}

enum Execution {
    Finished(std::process::Output),
    TimedOut,
}

/// Runs the test binary, killing it if the suite declared a `timeout_seconds`
/// and it runs past one.
///
/// A test cannot block on I/O — every effect it can reach is one the runner
/// supplied — so the only way to run forever is a loop with no exit, and the
/// only thing to do about one is to stop it. The wait is a poll rather than a
/// thread because the suite is the only child and there is nothing else for
/// this process to do while it runs.
///
/// **The clock starts here, at the spawn.** Compiling and linking the suite
/// happened before this call and are not on the budget, which is what makes
/// `timeout_seconds` a bound on the run rather than on the build — a suite's
/// budget does not have to be re-argued because a machine got slower at
/// compiling. What the budget does cover, besides the tests, is what the
/// machine charges to start a process: a fork, an exec, and on macOS the
/// validation of a binary that was just written. That is hundreds of
/// milliseconds on a loaded machine, so a declared budget is a bound with room
/// in it rather than a measurement of the work.
///
/// The command comes from `build/spawn.rs` rather than from
/// `Command::new(js_runtime())`, which is the whole of what distinguishes an
/// action's process from any other: an explicit environment and a frozen clock,
/// so that the same suite produces the same record on a machine set to a
/// different time zone.
fn execute(
    program: &str,
    module: Option<&std::path::Path>,
    limit: Option<u32>,
    env: &[(&str, &str)],
) -> std::io::Result<Execution> {
    use std::process::Stdio;
    let Some(mut cmd) = crate::build::spawn::command(program) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("`{program}` is not on PATH"),
        ));
    };
    if let Some(module) = module {
        cmd.arg(module);
    }
    // Added to the explicit environment `spawn::command` built rather than
    // instead of it: what the runner tells a test binary is one more input to
    // the action, and everything else about the process — the frozen clock, the
    // rest of the environment — is unchanged by there being one.
    for (name, value) in env {
        cmd.env(name, value);
    }
    // The heap check, forwarded from this process rather than invented here.
    // `HEAP_CHECK`'s own comment is the argument; the shape of it is that a
    // harness — or a person — says `BURI_RT_HEAP_CHECK=1 buri test` and the
    // binary that runs the blocks is the process that answers.
    for name in HEAP_CHECK {
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        }
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(limit) = limit else {
        return Ok(Execution::Finished(child.wait_with_output()?));
    };
    // `timeout_seconds` comes from a build file, so the deadline is computed
    // from a number this process did not choose. One too large for the clock to
    // represent is one no run could reach anyway, and it means no deadline
    // rather than an instant one.
    let deadline = Instant::now().checked_add(Duration::from_secs(u64::from(limit)));
    loop {
        if child.try_wait()?.is_some() {
            return Ok(Execution::Finished(child.wait_with_output()?));
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Execution::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The runner's one JSON array of records, as `$run` writes it and as the
/// cache stores it.
///
/// The array is the last line of what the runner wrote, from its first `[`. A
/// record that is not a whole array is no record, so a run cut off part way
/// reads as a run that produced nothing rather than as the tests it got to.
fn parse_results(text: &str) -> Vec<Case> {
    let last = text.trim_end().lines().next_back().unwrap_or_default();
    let array = last.find('[').and_then(|i| last.get(i..)).unwrap_or_default();
    let Ok(Value::Array(records)) = crate::json::parse(array) else {
        return Vec::new();
    };
    records
        .iter()
        .map(|record| {
            let verdict = if record.get("ok") == Some(&Value::Bool(true)) {
                Verdict::Passed
            } else {
                let error = record.get("error");
                Verdict::Failed {
                    message: error.and_then(|e| text_of(e, "message")).unwrap_or_default(),
                    diff: error.and_then(diff_of),
                    order: text_of(record, "order"),
                }
            };
            Case {
                provenance: Provenance::Ran,
                name: text_of(record, "name").unwrap_or_default(),
                module: text_of(record, "module").unwrap_or_default(),
                verdict,
                location: None,
            }
        })
        .collect()
}

/// A test's title, as one quoted line.
///
/// The report is one line per `FAIL`, which is what makes it greppable, and a
/// title is whatever somebody typed between the quotes. A `"` in one would
/// close the quoting and a newline would end the line, so both are escaped —
/// the rendering is the source syntax the title was written in. So is any
/// other control character, which a terminal would act on rather than show.
fn quote_title(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for c in name.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A failure message, indented under the `FAIL` line it belongs to.
///
/// Every line, not just the first: a message spanning lines whose second line
/// is flush left reads as the report having ended, and an abort message is free
/// to span lines.
fn indented(message: &str) -> String {
    message.split('\n').map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
}

/// Output names the target, the file, and the test (TESTING.md, "Running").
fn report_failure(
    session: &Session,
    target: TargetId,
    c: &Case,
    message: &str,
    diff: Option<&Diff>,
    order: Option<&str>,
    out: &mut Out,
) {
    let label = session.workspace.label(target);
    let file = c.module.trim_start_matches("//");
    let file = file.strip_prefix(&session.workspace.package(target.package).path).unwrap_or(file);
    let file = file.trim_start_matches('/');
    // No `.buri` appended: a module path is the file, so the name is already
    // on it. Gluing one on produced `test/cents.buri.buri`, which this line
    // did and which the goldens had recorded.
    out.line(&format!("FAIL {label}  {file}  {}", quote_title(&c.name)));
    out.line(&indented(message));
    if let Some(d) = diff {
        out.line(&format!("    actual:   {}", d.actual));
        out.line(&format!("    expected: {}", d.expected));
    }
    // After the values and before the location, which is the order the three
    // answer a reader's questions: *what went wrong*, *between which two
    // values*, and *what would have to be true to see it again*. The location
    // stays last because it is the line the editor jumps to.
    //
    // Indented like the message rather than like the diff: it is a sentence
    // about this failure, not a third side of the comparison.
    if let Some(note) = order {
        out.line(&indented(note));
    }
    if let Some(loc) = &c.location {
        out.line(&format!("  --> {loc}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which block a process that said nothing died in, off the `left` lines
    /// the blocks that finished wrote.
    ///
    /// The whole-process half of this is
    /// `repositories/testing/a_binary_that_dies_mid_suite`, where a real binary
    /// really is killed. Here because the *last* row is the one no program can
    /// be asked for on purpose: a binary that ran every block and died anyway
    /// has no block to blame, and what the runner does with that is report it
    /// against the suite rather than invent a fourth verdict for a suite of
    /// three tests.
    #[test]
    fn a_dead_process_is_blamed_on_the_block_that_never_finished() {
        let left = |indices: &[usize]| {
            indices.iter().map(|i| format!("{{\"i\":{i},\"left\":1}}\n")).collect::<String>()
        };
        // Nothing finished: the block it was told to start at.
        assert_eq!(died_after("", 0, 3), Some(0));
        assert_eq!(died_after("", 2, 3), Some(2));
        // The first two finished, so it died in the third.
        assert_eq!(died_after(&left(&[0, 1]), 0, 3), Some(2));
        // A resumed process, whose earlier blocks are somebody else's verdict.
        assert_eq!(died_after(&left(&[1]), 1, 3), Some(2));
        // A block that ran more than once writes a line per run.
        assert_eq!(died_after(&left(&[0, 0, 0]), 0, 3), Some(1));
        // Every block finished, and the process died outside all of them.
        assert_eq!(died_after(&left(&[0, 1, 2]), 0, 3), None);
        // A failure line is not a `left` line, and does not count as one.
        assert_eq!(died_after("{\"i\":0,\"message\":\"boom\"}\n", 0, 3), Some(0));
    }

    /// A failure line is read past the `left` lines around it, and a process
    /// that wrote only `left` lines noted no failure at all.
    #[test]
    fn a_noted_failure_is_the_last_line_carrying_a_message() {
        let stdout = "{\"i\":0,\"left\":1}\n{\"i\":1,\"message\":\"boom\"}\n";
        let noted = noted_failure(stdout).expect("the failure");
        assert_eq!(noted.at, 1);
        assert_eq!(noted.message, "boom");
        assert!(noted_failure("{\"i\":0,\"left\":1}\n{\"i\":1,\"left\":1}\n").is_none());
        assert!(noted_failure("").is_none());
    }

    /// One line per `FAIL`, whatever the title holds.
    #[test]
    fn a_title_is_printed_as_the_quoted_string_it_was_written_as() {
        assert_eq!(quote_title("plain"), "\"plain\"");
        assert_eq!(quote_title("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_title("two\nlines"), "\"two\\nlines\"");
        assert_eq!(quote_title("a\\b\tc"), "\"a\\\\b\\tc\"");
        assert_eq!(quote_title("ring\u{7}\u{1b}"), "\"ring\\u{7}\\u{1b}\"");
        for title in ["plain", "say \"hi\"", "two\nlines", "a\\b\tc", "ring\u{7}"] {
            assert_eq!(quote_title(title).lines().count(), 1, "{title:?} broke the line");
        }
    }

    /// A message is part of the report, so all of it is indented under the
    /// `FAIL` line — a flush-left second line reads as the report ending.
    #[test]
    fn every_line_of_a_message_is_indented() {
        assert_eq!(indented("one"), "  one");
        assert_eq!(indented("a\nb\nc"), "  a\n  b\n  c");
        assert_eq!(indented("a\n\nb"), "  a\n  \n  b");
    }

    /// The gap net matches both spellings the toolchain has for "no body for
    /// this", and nothing else.
    ///
    /// Written against the two literal sentences rather than against a
    /// constructed compilation, because what the net is is a claim about those
    /// two strings: `build/actions.rs`'s, from `missing_intrinsics`, and a
    /// backend's own, from a runtime key with no entry. What it decides is
    /// whether the refusal gains the sentence about naming a platform, so a
    /// failure of another kind is reported as it stands.
    #[test]
    fn only_a_backend_gap_is_one() {
        let of = |messages: &[&str]| {
            let mut diagnostics = Diagnostics::new();
            for m in messages {
                diagnostics.push(Diagnostic::error(Span::NONE, (*m).to_string()));
            }
            is_backend_gap(&diagnostics)
        };
        assert!(of(&["the stencil backend has no implementation of character.isDigit"]));
        assert!(of(&["the native runtime has no implementation of `json.decode`"]));
        assert!(of(&[
            "the llvm backend has no implementation of testing_assert.report",
            "the native runtime has no implementation of `bytes.toUtf8`",
        ]));
        // Not a gap: an empty failure, a failure of another kind, and a
        // mixture with one of each.
        assert!(!of(&[]));
        assert!(!of(&["cannot declare the entry point: duplicate definition"]));
        assert!(!of(&[
            "the stencil backend has no implementation of character.isDigit",
            "cannot declare the entry point: duplicate definition",
        ]));
    }

    /// A native gap names **every** key it found, not a count
    /// (buri-lang/buri#199): a run refused for a gap should be able to show the
    /// whole gap, and the message is what the `--error-format=json` output
    /// carries, so listing them here fixes both surfaces at once.
    #[test]
    fn a_backend_gap_lists_every_missing_key() {
        let keys =
            vec!["number.I64.toI32".to_string(), "number.U32.toChar".to_string(), "z.z".to_string()];
        let gap = Gap::NoBody { backend: "llvm", keys };
        let diagnostics = gap_refusal("a suite", gap);
        let message = &diagnostics.items.first().expect("a refusal").message;
        assert!(message.contains("number.I64.toI32"), "{message}");
        assert!(message.contains("number.U32.toChar"), "{message}");
        assert!(message.contains("z.z"), "{message}");
        // The truncation #199 is about — never a count in place of the names.
        assert!(!message.contains("more"), "{message}");
    }

    /// The seed a suite schedules with is a function of its key and of nothing
    /// else.
    ///
    /// The point of the row in `DECISIONS.md` stated as a test: two calls agree,
    /// two different keys disagree, and nothing here reads a clock. A generator
    /// in this position would make `Cache::put` store a record the next run
    /// could not have produced.
    #[test]
    fn a_suites_seed_is_its_own_key_and_nothing_else() {
        let one = crate::build::cache::ActionKey::of(b"a suite");
        let two = crate::build::cache::ActionKey::of(b"another suite");
        assert_eq!(seed_of(&one), seed_of(&one));
        assert_ne!(seed_of(&one), seed_of(&two));
        // The first 32 hex digits of the digest, read as a `u128`.
        let front = one.as_str().get(..32).expect("a key is 64 hex digits");
        assert_eq!(seed_of(&one), u128::from_str_radix(front, 16).unwrap());
        // Every key is 64 hex digits by construction (`ActionKey`'s two
        // constructors both hash), so the fallbacks in `seed_of` are unreachable
        // and this is the claim that keeps them so.
        assert_eq!(two.as_str().len(), 64);
        assert!(two.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// A native record is read back by the parser that reads a JavaScript one.
    ///
    /// Written as a round trip rather than against a literal, because what has
    /// to hold is that the two producers and the one consumer agree — a record
    /// this file writes and cannot read is a verdict that silently becomes a
    /// suite of no tests.
    #[test]
    fn a_record_this_runner_writes_is_one_it_reads() {
        let diff = Diff { actual: String::from("\"a\\tb\""), expected: String::from("2") };
        let note = "the tasks completed in the order 0, 2, 1 — replay it with `tasks().seed(1)`";
        let failed = |diff, order: Option<&str>| Block::Failed {
            message: String::from("assert.equal failed"),
            diff,
            order: order.map(str::to_string),
        };
        let record = Value::Array(vec![
            record_of("a title", "//lib/x/test/x", &Block::Passed),
            record_of("say \"hi\"", "//lib/x/test/x", &failed(Some(diff), None)),
            record_of("scheduled", "//lib/x/test/x", &failed(None, Some(note))),
        ])
        .to_string();
        let cases = parse_results(&record);
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].name, "a title");
        assert_eq!(cases[0].module, "//lib/x/test/x");
        assert!(matches!(cases[0].verdict, Verdict::Passed));
        // The title's quotes survive the record, and so do the escapes inside
        // a rendered value: `$show` already escaped them, and the record
        // escapes what it is handed.
        assert_eq!(cases[1].name, "say \"hi\"");
        let Verdict::Failed { message, diff: Some(d), order } = &cases[1].verdict else {
            panic!("the failing record did not read back as a failure");
        };
        assert_eq!(message, "assert.equal failed");
        assert_eq!(d.actual, "\"a\\tb\"");
        assert_eq!(d.expected, "2");
        // A failure that scheduled nothing carries no order, and that is a
        // *missing* key rather than an empty one: the record of a suite that
        // never says `tasks()` is the bytes it was before this slice.
        assert_eq!(*order, None);
        assert!(!record.contains("\"order\":\"\""));
        // And the two travel independently — an order with no diff is the shape
        // a faulted task produces, which has a message and no pair.
        let Verdict::Failed { diff, order, .. } = &cases[2].verdict else {
            panic!("the scheduled record did not read back as a failure");
        };
        assert!(diff.is_none());
        assert_eq!(order.as_deref(), Some(note));
    }

    /// The line a native test binary writes when a block aborts.
    ///
    /// The literal is the contract with `cli/runtime/testing.rs`, so it is
    /// written out here rather than produced: the two are in different crates
    /// and nothing but this test compares them.
    #[test]
    fn a_native_binary_says_which_block_aborted() {
        let noted = noted_failure("{\"i\":3,\"message\":\"assert.equal failed\",\"actual\":\"1\",\"expected\":\"2\"}\n")
            .expect("a record with an index is a record");
        assert_eq!(noted.at, 3);
        assert_eq!(noted.message, "assert.equal failed");
        let diff = noted.diff.expect("both sides were there");
        assert_eq!((diff.actual.as_str(), diff.expected.as_str()), ("1", "2"));
        // A block that scheduled nothing says nothing about an order, which is
        // the case every block that never writes `tasks()` is in.
        assert_eq!(noted.order, None);
        // An abort that is not an assertion has a message and no pair, and a
        // run that said nothing at all has no record — which is the case the
        // caller attributes to the block it asked for.
        let plain = noted_failure("{\"i\":0,\"message\":\"division by zero\"}\n").unwrap();
        assert!(plain.diff.is_none());
        assert!(noted_failure("").is_none());
        assert!(noted_failure("assert.equal failed\n").is_none());
        // The order sentence, in the position `note_failure` writes it: after
        // the pair where there is one, and beside the message where there is
        // not. Both literals are the contract with the other crate.
        let scheduled = noted_failure(
            "{\"i\":1,\"message\":\"assert.equal failed\",\"actual\":\"1\",\"expected\":\"2\",\
             \"order\":\"the tasks completed in the order 1, 0 — replay it with `tasks().seed(1)`\"}\n",
        )
        .unwrap();
        assert_eq!(
            scheduled.order.as_deref(),
            Some("the tasks completed in the order 1, 0 — replay it with `tasks().seed(1)`")
        );
        let faulted = noted_failure(
            "{\"i\":2,\"message\":\"a task was failed by the plan: task(1): gone\",\
             \"order\":\"the tasks completed in the order 1, 0 — replay it with `tasks().seed(1)`\"}\n",
        )
        .unwrap();
        assert!(faulted.diff.is_none());
        assert!(faulted.order.is_some());
    }
}
