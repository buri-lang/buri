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
//! the machine is that a test source has no name for `core/host` at all, and
//! that its capabilities are fakes this runner injects.
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
use std::path::PathBuf;
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
/// `expected: None` is representable, and it makes `--accept` silently rewrite
/// nothing while the failure prints a half diff there is nothing to compare
/// against.
struct Diff {
    actual: String,
    expected: String,
}

/// What one test did.
///
/// A message and a diff belong to a failure and to nothing else, so they live
/// inside the failing variant. A passing test carrying a diff is no longer a
/// value anything can build.
enum Verdict {
    Passed,
    Failed { message: String, diff: Option<Diff> },
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
    /// Golden files `--accept` rewrote, for the blank line before the summary.
    accepted: usize,
}

/// Where a run's platform came from.
///
/// The distinction is the whole of the fallback rule: a platform the suite
/// wrote down, or the command line named, is a request, and a request this
/// toolchain cannot serve is refused in so many words. A platform nobody asked
/// for is a *preference*, and a preference gives way — to JavaScript, out loud
/// — rather than turning a suite that used to run into a suite that does not.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chosen {
    /// `test { platforms: [...] }`, or `--output=`.
    Asked,
    /// Nobody said, so [`default_platform`] did.
    Default,
}

/// The one-line notices a pass writes to standard error.
///
/// Standard error, because stdout is the record — the failures, the diffs and
/// the summary line — and which backend ran a suite is a fact about this
/// toolchain rather than a fact about the code. A golden that records what a
/// suite printed is unchanged by a note about how it was printed.
///
/// The toolchain's reason is stated once per pass and the suite's once per
/// suite, for the same reason: "this build has no native backend" is one fact
/// however many suites meet it, and "this suite reaches something the backend
/// has no body for" is a different fact about each one.
#[derive(Default)]
struct Notices {
    pass: bool,
}

impl Notices {
    /// A reason that belongs to the invocation: it is the same reason for every
    /// suite in the pass, so it is stated once and the suites are not
    /// enumerated.
    fn pass(&mut self, reason: &str) {
        if std::mem::replace(&mut self.pass, true) {
            return;
        }
        eprintln!("note: {reason}, so a suite that names no platform runs on javascript");
    }

    /// This suite reaches something the native backend has no body for.
    fn suite(&mut self, label: &str, reason: &str) {
        eprintln!("note: {label} runs on javascript — {reason}");
    }
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
        let names: Vec<&str> = Platform::ALL.iter().map(|p| p.slug()).collect();
        eprintln!("error: no platform matches `--output={selector}`");
        eprintln!("  = a suite runs on one of: {}", names.join(", "));
        eprintln!("  = fix: name one of them, as in `--output=js`");
        return 2;
    }
    if !args.flags.watch {
        return one_pass(args, Asked::Once).code;
    }
    // The three combinations `--watch` refuses were refused at parsing, so by
    // here a watch loop is a loop: a pass, the declared set that pass computed,
    // and a sweep of it. Nothing is shared between passes — each opens its own
    // `Session`, because the parse cache inside one is keyed on `FileId` and
    // would happily serve the text a file held before the edit that woke the
    // loop.
    let root = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::build::workspace::find_root(&cwd))
        .unwrap_or_default();
    watch::Watch::on(root, args.flags.explain).drive(|_| one_pass(args, Asked::Watching))
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
    let pass = one_pass(&args, Asked::Served { root });
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
fn one_pass(args: &arguments::Args, asked: Asked) -> watch::Pass {
    let watching = matches!(asked, Asked::Watching);
    let mut out =
        if matches!(asked, Asked::Once) { Out::Direct } else { Out::Held(String::new()) };
    let opened = match asked {
        Asked::Served { root } => session::open_at(root, &args.flags),
        Asked::Once | Asked::Watching => session::open(&args.flags),
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
    let mut notices = Notices::default();
    // One test binary per tag-compatible batch of suites, before the loop that
    // reports them. What comes back is a verdict per suite, and a suite that is
    // not in it — because it could not batch, or because the batch was
    // abandoned — is run below exactly as it was before any of this existed.
    let mut pre = run_batches(&mut session, &targets, args, &mut notices);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut cached = 0usize;
    let mut suites = 0usize;
    let mut printed = false;
    let mut hard_error = false;

    for &target in &targets {
        if !has_tests(&session, target) {
            continue;
        }
        suites += 1;
        match run_suite(&mut session, target, args, &mut out, &mut notices, &mut pre) {
            Ok(outcome) => {
                skipped += outcome.skipped;
                printed |= outcome.accepted > 0;
                for c in &outcome.cases {
                    if c.provenance == Provenance::Cache {
                        cached += 1;
                    }
                    match &c.verdict {
                        Verdict::Passed => passed += 1,
                        Verdict::Failed { message, diff } => {
                            failed += 1;
                            report_failure(&session, target, c, message, diff.as_ref(), &mut out);
                            printed = true;
                        }
                    }
                }
            }
            Err(diagnostics) => {
                hard_error |= session.print(&diagnostics);
            }
        }
    }

    // `check_during_build`: the catalogue runs over a test pass too, but only
    // one nothing already stopped — a suite that could not be built has an
    // answer, and it is not a lint finding.
    if !hard_error && session.workspace.repo.lint.check_during_build {
        let findings = crate::commands::lint::findings_for(&mut session, &targets);
        hard_error |= session.print(&findings);
    }

    let elapsed = started.elapsed().as_secs_f64();
    if suites == 0 {
        out.line("no test suites");
        // Nothing ran, so the only thing that can have failed is the catalogue.
        let code = i32::from(hard_error);
        return watch::Pass { code, inputs, output: out.take(), quiet: false };
    }
    let note = if cached > 0 { format!(", {cached} cached") } else { String::new() };
    if printed {
        out.blank();
    }
    out.line(&format!(
        "{passed} passed, {failed} failed, {skipped} skipped ({elapsed:.1}s{note})"
    ));
    // Silent only when there was nothing to do: every case came out of the
    // cache, none failed, nothing was accepted, and nothing was asked for by
    // name. `--explain` is never silent — a transcript of what the cache did is
    // exactly what somebody running it wants to read.
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
/// fix, and re-running it should re-run it. `--filter` and `--accept` are
/// outside the cache in both directions — `--accept` is the one mode that
/// writes to the source tree, and a mode that writes must not also be one that
/// can be served.
fn may_cache(cases: &[Case], flags: &arguments::Flags) -> bool {
    cases.iter().all(|c| matches!(c.verdict, Verdict::Passed))
        && flags.filter.is_none()
        && !flags.accept
}

/// The same, and additionally that there is something to remember.
///
/// The runners that parse a *process's* output take this one, because an empty
/// array from a process is a run that produced nothing, and remembering it as
/// "everything passed" would serve a suite that never ran. `run_native` builds
/// its record from the units it compiled rather than from a process, and takes
/// `may_cache` above.
///
/// Two functions rather than one with the guard folded in, so that the
/// divergence is a choice a reader can see rather than a conjunct missing from
/// one of three copies.
fn may_cache_produced(cases: &[Case], flags: &arguments::Flags) -> bool {
    !cases.is_empty() && may_cache(cases, flags)
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

/// One suite, once per platform it runs on.
///
/// `pre` holds the verdicts a shared test binary already produced
/// ([`run_batches`]). A suite in it has run — the policy checks below still
/// happen, because they are checks about the *graph* rather than about the run,
/// and they are the same pure function either way — and everything from the
/// platform decision down is skipped, because it has already been made.
fn run_suite(
    session: &mut Session,
    target: TargetId,
    args: &arguments::Args,
    out: &mut Out,
    notices: &mut Notices,
    pre: &mut Prepass,
) -> Result<Outcome, Diagnostics> {
    let mut diagnostics = Diagnostics::new();
    // A suite inherits its target's tags and platform restrictions, so a suite
    // for a `server` library is checked as server code without saying
    // anything. A suite that names no platforms runs once on the host, and the
    // host here is the machine, not the JavaScript the backend emits.
    let declared: Vec<Platform> = suite(session, target)
        .map(|x| x.platforms)
        .unwrap_or_default()
        .iter()
        .map(|p| p.value)
        .collect();
    let checked: Vec<Platform> = if declared.is_empty() {
        vec![crate::compiler::driver::host_native_platform()]
    } else {
        declared.clone()
    };
    // A platform a suite names must be one the target admits: asking for a JS
    // run of a `[LINUX, MACOS]` library is an error, not a skip
    // (TAGS.md, "Tags and tests").
    for p in &checked {
        actions::check_policy(session, target, *p, &mut diagnostics);
    }
    if diagnostics.has_errors() {
        return Err(diagnostics);
    }
    if let Some(outcome) = pre.take(target) {
        return Ok(outcome);
    }

    // One run per declared platform. A native platform is executed natively
    // where this toolchain has a backend, a runtime archive and a linker for it
    // — the same three questions `buri build` asks — and refused in the same
    // words as a native build where it does not.
    //
    // A suite that names none runs **natively**, on the host it is already
    // checked against, and falls back to JavaScript per suite where it cannot
    // (`default_platform`, and the gap probe in `run_on`). The default was
    // JavaScript for as long as the native runtime surface was too small to
    // carry an arbitrary program; it is native now because the dev loop is
    // measurably faster on it (`design/PERFORMANCE.md` §6) and because the
    // surface that made the old default right is now the exception rather than
    // the rule (`design/native/ARCHITECTURE.md` §4).
    let runs: Vec<(Platform, Chosen)> = if !declared.is_empty() {
        declared.into_iter().map(|p| (p, Chosen::Asked)).collect()
    } else if let Some(p) = selected_platform(&args.flags) {
        // `--output` names the platform for the suites that have not named
        // one. A suite that declares `platforms` has made the stronger
        // statement and the flag does not overrule it.
        vec![(p, Chosen::Asked)]
    } else {
        // The invocation's answer first, then the suite's, so that a toolchain
        // with no native backend states that once instead of giving every
        // `data:` suite a reason that is not the operative one.
        let wanted = default_platform(&args.flags, notices);
        // The suite's own fallback is a fact about the *build file* rather than
        // about the program, and the only one that can be answered before
        // anything is compiled. The JavaScript runner hands the suite its
        // `test { data: [...] }` entries as the in-memory filesystem `data()`
        // answers; a linked test binary has no runner to be handed them by, so
        // `data()` is empty there and every read of a declared file would
        // answer the wrong thing — silently, since an empty filesystem is a
        // filesystem (`cli/runtime/testing.rs`'s header states the divergence
        // and what would close it).
        if wanted.is_native() && suite(session, target).is_some_and(|x| !x.data.is_empty()) {
            notices.suite(
                &session.workspace.label(target),
                "a native test binary has no runner to hand it `test { data }`, so its \
                 `data()` filesystem would be empty",
            );
            vec![(Platform::Js, Chosen::Default)]
        } else {
            vec![(wanted, Chosen::Default)]
        }
    };
    let mut outcome = Outcome::default();
    for (platform, chosen) in runs {
        if platform.is_native() && !native_ready(platform, &args.flags) {
            let span = suite(session, target).map(|x| x.span).unwrap_or(Span::NONE);
            diagnostics.push(
                Diagnostic::templated("platform-not-implemented", span)
                    .with_bind("platform", platform.slug())
                    .with_bind("platform_in_build_file", platform.proto()),
            );
            continue;
        }
        match run_on(session, target, platform, chosen, args, out, notices, pre) {
            Ok(one) => {
                outcome.cases.extend(one.cases);
                outcome.skipped += one.skipped;
                outcome.accepted += one.accepted;
            }
            Err(d) => diagnostics.extend(d.items),
        }
    }
    if diagnostics.has_errors() {
        return Err(diagnostics);
    }
    Ok(outcome)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the selection is four independent facts — which suite, where it runs, \
              who asked, and what the invocation said — and the sink and the notice \
              log are the two places output goes; none is derivable from another"
)]
fn run_on(
    session: &mut Session,
    target: TargetId,
    mut platform: Platform,
    chosen: Chosen,
    args: &arguments::Args,
    sink: &mut Out,
    notices: &mut Notices,
    pre: &mut Prepass,
) -> Result<Outcome, Diagnostics> {
    let mut diagnostics = Diagnostics::new();

    let mut key = test_key_for(session, target, platform, args, pre);
    if let Some(cached) = served(session, target, platform, &key, args) {
        return Ok(cached);
    }
    crate::build::cache::explain(
        args.flags.explain,
        crate::build::cache::Status::Run,
        crate::build::cache::Action::Test,
        &session.workspace.label(target),
        platform,
        &key,
    );

    // `None`, not `platform`: the platform a suite *runs* on and the platform
    // whose host grant a program is checked against are different questions,
    // and a test never binds `core/host` — only the entry point a batched
    // binary happens to drag in does. See `Unit::platform`.
    let unit = Unit { target: Some(target), platform: None, with_tests: true };
    let analysis = crate::compiler::driver::analyze(
        Some(&session.workspace),
        &mut session.map,
        &mut session.parsed,
        &unit,
    );
    if analysis.diagnostics.has_errors() {
        return Err(analysis.diagnostics);
    }

    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut program = monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diagnostics,
        monomorphize::Roots::Tests,
    );
    if diagnostics.has_errors() {
        return Err(diagnostics);
    }
    if program.roots.tests().is_empty() {
        return Ok(Outcome::default());
    }

    // What a `--filter` leaves out is counted here rather than in the runner:
    // the names are known before the binary is built, and a count nobody has to
    // run a process to learn is one the summary can always print.
    let skipped = match &args.flags.filter {
        Some(f) => program.roots.tests().iter().filter(|t| !t.name.contains(f.as_str())).count(),
        None => 0,
    };

    // The fallback, asked before a second is spent on codegen and asked of the
    // program rather than of the source: `Backend::missing_intrinsics` is the
    // hook a native build already asks and `native/conformance.rs` already
    // pins, so a suite falls back here exactly when a native build of it would
    // be refused there. Nothing is remembered between runs — the answer is a
    // function of the program and of this backend, and the day the backend
    // grows the body the suite goes native with no cache to clear.
    //
    // Only a *defaulted* platform gives way. A suite that named one gets the
    // refusal it asked for, which is what `platform-not-implemented` and
    // `repositories/testing/suite_platforms` are for.
    if platform.is_native() && chosen == Chosen::Default {
        if let Some(reason) = native_gap(platform, &args.flags, &program, &analysis.checked.tables)
        {
            notices.suite(&session.workspace.label(target), &reason);
            platform = Platform::Js;
            key = test_key_for(session, target, platform, args, pre);
            if let Some(cached) = served(session, target, platform, &key, args) {
                return Ok(cached);
            }
        }
    }

    if platform.is_native() {
        let out =
            run_native(session, target, platform, args, sink, &key, program, &analysis, skipped);
        // The checked program is tens of milliseconds of `free` at a hundred
        // thousand lines, and by here the verdict already exists. `Loaded`
        // holds its modules behind `Rc` — shared with the session's parse
        // cache — so it is not one of the things that can be handed over.
        let crate::compiler::driver::Analysis { loaded, checked, diagnostics } = analysis;
        drop(loaded);
        drop(diagnostics);
        crate::parallel::discard(checked);
        // The second half of the same rule, for the gaps `missing_intrinsics`
        // cannot see: a `deriveArray*` is an intrinsic *expression* inside a
        // body `middle::derives` generated rather than a function the hook is
        // asked about, so the backend names it while emitting instead of
        // before. A defaulted run that failed for that reason and only that
        // reason is a run that should not have been native, so it is taken
        // again on JavaScript rather than reported.
        return match out {
            Err(diagnostics) if chosen == Chosen::Default && is_backend_gap(&diagnostics) => {
                let reason = diagnostics
                    .items
                    .first()
                    .map(|d| d.message.clone())
                    .unwrap_or_else(|| "the native backend refused it".to_string());
                notices.suite(&session.workspace.label(target), &reason);
                run_on(session, target, Platform::Js, Chosen::Default, args, sink, notices, pre)
            }
            out => out,
        };
    }

    let mut source = actions::emit(
        &mut program,
        &analysis.checked.tables,
        crate::compiler::backend::Target { platform: Platform::Js, arch: None },
        &args.flags,
        &mut diagnostics,
    )?;

    // The runner's in-memory `Fs` contains exactly `test { data: [...] }`, and
    // nothing else on disk is visible.
    let data = load_test_data(session, target);
    source.push_str(&format!("\n$t.data={data};\n"));
    // The action's clock, spliced in after the runtime is defined and before a
    // test could reach one. A test has no name for `core/host` to begin with;
    // this is what keeps a suite's *record* the same bytes twice, so that
    // reproducibility is a question worth asking about a suite.
    source.push_str(crate::build::spawn::FIXED_CLOCK_JS);
    let filter = args
        .flags
        .filter
        .as_ref()
        .map(|f| javascript::quote(f))
        .unwrap_or_else(|| "null".into());
    source.push_str(&format!(
        "$write(1,JSON.stringify($run({filter})));\n"
    ));

    let dir =
        session.root.join(".buri/out/js").join(&session.workspace.package(target.package).path);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("test.mjs");
    if let Err(e) = std::fs::write(&path, &source) {
        diagnostics.push(
            Diagnostic::error(Span::NONE, format!("cannot write {}: {e}", path.display()))
                .with_fix("check the directory exists and is writable"),
        );
        return Err(diagnostics);
    }

    let limit = suite(session, target).and_then(|x| x.timeout_seconds);
    let out = match execute(&js_runtime(), Some(&path), limit, &[]) {
        Ok(Execution::Finished(out)) => out,
        Ok(Execution::TimedOut) => return Err(timed_out(session, target, limit)),
        Err(e) => {
            diagnostics.push(
                Diagnostic::error(Span::NONE, format!("cannot run the test binary: {e}"))
                    .with_fix("install bun, or point BURI_JS at a JavaScript runtime"),
            );
            return Err(diagnostics);
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let mut cases = parse_results(&stdout);
    if may_cache_produced(&cases, &args.flags) {
        crate::build::cache::Cache::open(&session.root).put(&key, stdout.as_bytes());
    }
    locate(session, &program, &mut cases);
    if cases.is_empty() && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        diagnostics.push(
            Diagnostic::error(Span::NONE, "the test binary did not run")
                .with_fix("read the runtime's own message below; it is what failed")
                .with_note(err.trim().to_string()),
        );
        return Err(diagnostics);
    }
    let accepted =
        if args.flags.accept { accept_goldens(session, target, &cases, sink) } else { 0 };
    Ok(Outcome { cases, skipped, accepted })
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
    actions::native_ready(actions::target_of(&output), actions::profile_of(flags))
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
/// A guess, and one that costs nothing when it is wrong: a suite that ends up on
/// a different platform, or on JavaScript, selects its own linker and the probe
/// that ran was for a linker nobody asked. What it must not do is *decide*
/// anything, which is why it does not consult the fallbacks — a notice belongs
/// to the suite that earns it.
fn warm_linker(args: &arguments::Args) {
    if args.flags.accept {
        return;
    }
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
/// The same selector `buri build --output=` takes and the same matcher, so
/// `js`, `macos` and `linux/x86_64` mean here what they mean there. A selector
/// naming nothing is not an error at this seam: `command_test` refuses it once,
/// for the invocation, rather than once per suite.
fn selected_platform(flags: &arguments::Flags) -> Option<Platform> {
    let selector = flags.output.as_ref()?;
    Platform::ALL
        .into_iter()
        .find(|p| {
            crate::build::buildfile::Output::for_platform(*p, Span::NONE)
                .matches_selector(selector)
        })
}

/// Where a suite that names no platform runs.
///
/// The host's own platform where this toolchain can compile, link and run a
/// binary for it in this profile, and JavaScript where it cannot — which is
/// `--no-default-features`, a host outside macOS and Linux, a host the
/// development backend has no stencil library for (macOS on x86-64), a machine
/// with no C toolchain, and `--release` without `backend-llvm`. The last of
/// those is why the profile comes from the flags rather than being pinned to
/// `Debug`: the release profile routes to LLVM (`backend::select`), and a
/// toolchain that does not have it must not be quietly handed the debug
/// backend.
///
/// This is the *toolchain's* half of the answer, and it is the same for every
/// suite in a pass. The suite's half — whether the native backend has a body
/// for everything this program reaches — is [`native_gap`], asked once the
/// program exists.
fn default_platform(flags: &arguments::Flags, notices: &mut Notices) -> Platform {
    // `--accept` rewrites a golden file from the two sides of the comparison
    // that failed. A native run reports both now (`run_native`), so this is no
    // longer about what the runner can see — it is the *suite* rule arrived at
    // once for the invocation: the only file `accept_goldens` will rewrite is
    // one the suite declared in `test { data: [...] }`, and a suite that
    // declares any runs on JavaScript whatever else is true, because a native
    // test binary has no runner to be handed those entries by.
    if flags.accept {
        notices.pass("--accept rewrites a file the suite declared in `test { data }`, which \
                      only the JavaScript runner is handed");
        return Platform::Js;
    }
    let native = crate::compiler::driver::host_native_platform();
    if native_ready(native, flags) {
        return native;
    }
    notices.pass(&format!(
        "this toolchain cannot build a {} test binary in the {} profile",
        native.slug(),
        flags.mode.name()
    ));
    Platform::Js
}

/// What this program reaches that the native backend has no body for, in one
/// line, or `None` when it reaches nothing of the kind.
///
/// The answer is the backend's own, not a list kept here: keeping one would be
/// a second statement of the surface that drifts from the first the day a gap
/// closes, and a gap closing is the frequent event.
fn native_gap(
    platform: Platform,
    flags: &arguments::Flags,
    program: &monomorphize::Program,
    tables: &crate::compiler::semantics::types::Tables,
) -> Option<String> {
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    let backend =
        crate::compiler::backend::select(actions::target_of(&output), actions::profile_of(flags))
            .ok()?;
    let missing = backend.missing_intrinsics(program, tables);
    let (first, rest) = missing.split_first()?;
    let more = match rest.len() {
        0 => String::new(),
        1 => String::from(" and one more"),
        n => format!(" and {n} more"),
    };
    Some(format!("the {} backend has no implementation of {first}{more}", backend.name()))
}

/// Whether a native compilation failed because this backend has no body for
/// something, rather than because the program is wrong.
///
/// Matched on the sentence both spellings share — `actions::objects_of`'s, from
/// `missing_intrinsics`, and a backend's own, from a runtime key with no
/// entry. A failure with anything else among its errors is a failure, and is
/// reported: falling back on one would turn a toolchain bug into a suite that
/// quietly passes somewhere else.
fn is_backend_gap(diagnostics: &Diagnostics) -> bool {
    !diagnostics.items.is_empty()
        && diagnostics.items.iter().all(|d| d.message.contains("has no implementation of"))
}

/// What the batch prepass left for the loop that reports the suites.
///
/// Two things, and the second is why they are one value: the verdicts a shared
/// binary produced, and the `test` keys building them cost. A suite's key is a
/// hash of every source in its closure, and the prepass has to ask for one to
/// find out whether the suite needs compiling at all — so without the memo a
/// pass that batched nothing would hash every suite's sources twice, which is
/// the whole of what a `buri test` on an unchanged repository does.
#[derive(Default)]
struct Prepass {
    done: Vec<(TargetId, Outcome)>,
    keys: Vec<((TargetId, Platform), crate::build::cache::ActionKey)>,
}

impl Prepass {
    /// The verdict a shared binary produced for this suite, once.
    fn take(&mut self, target: TargetId) -> Option<Outcome> {
        let i = self.done.iter().position(|(t, _)| *t == target)?;
        Some(self.done.remove(i).1)
    }
}

/// The action key for one suite on one platform.
///
/// Memoised for the pass. A key is a pure function of the repository's bytes and
/// the invocation, and nothing a `buri test` pass does writes a source — with
/// one exception, `--accept`, which rewrites a golden a suite declared in
/// `test { data }`. That is why the memo is only ever *filled* by the batch
/// prepass, which `--accept` returns from before it looks at a suite.
fn test_key_for(
    session: &Session,
    target: TargetId,
    platform: Platform,
    args: &arguments::Args,
    pre: &mut Prepass,
) -> crate::build::cache::ActionKey {
    if let Some((_, key)) = pre.keys.iter().find(|((t, p), _)| *t == target && *p == platform) {
        return key.clone();
    }
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    actions::test_key(session, target, &output, &args.flags)
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
    if args.flags.force || args.flags.filter.is_some() || args.flags.accept {
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
        platform,
        key,
    );
    Some(Outcome { cases, skipped: 0, accepted: 0 })
}

/// One suite, executed as a native binary.
///
/// The report is the JavaScript one, to the byte, because it is assembled here
/// from the same record: this function produces the array `$run` writes and
/// [`report_failure`] states the format once for both backends. What differs is
/// only how the record is collected.
///
/// **A failed assertion is still an abort.** SPEC 6.10 leaves nothing to catch,
/// so one process can report one failure and no more — and the answer is not to
/// report less but to use more processes. `BURI_TEST_FROM` names the block a
/// process is to start at, `buri_rt_test_enter` skips the ones already
/// reported, and a suite costs one process plus one per failure. That is the
/// sharding this module's header already permits: a suite's result may not
/// depend on the order its blocks run in, and there is no mutable global state
/// for one to leave behind for the next.
///
/// So:
///
/// - A run that ends cleanly is a verdict for **every** block from the one it
///   started at, because each of them ran and none aborted.
/// - A run that aborts names the block it was in — the runtime knows which,
///   from `enter` — with the message it was going to print anyway and, where
///   the assertion had them, both values rendered by the `Show`
///   `middle::derives` generated at their type.
/// - A run that ends some other way (a signal) says nothing, and the block it
///   was told to start at is the honest attribution.
/// - `--filter` is applied to the program's *roots* before anything is
///   compiled, rather than to a runner that does not exist. That is arguably
///   the better place for it: a filtered native run does not even codegen the
///   tests it leaves out.
#[allow(
    clippy::too_many_arguments,
    reason = "the front end's output, the selection, the key and the sink are each \
              needed here and none of them is derivable from the others; bundling \
              them into a struct would name the arguments twice"
)]
fn run_native(
    session: &mut Session,
    target: TargetId,
    platform: Platform,
    args: &arguments::Args,
    sink: &mut Out,
    key: &crate::build::cache::ActionKey,
    mut program: monomorphize::Program,
    analysis: &crate::compiler::driver::Analysis,
    skipped: usize,
) -> Result<Outcome, Diagnostics> {
    let mut diagnostics = Diagnostics::new();
    if let Some(filter) = &args.flags.filter {
        if let monomorphize::ProgramRoots::Tests(tests) = &mut program.roots {
            tests.retain(|t| t.name.contains(filter.as_str()));
        }
    }
    let selected: Vec<(String, String)> = program
        .roots
        .tests()
        .iter()
        .map(|t| (t.name.clone(), t.module.clone()))
        .collect();
    if selected.is_empty() {
        return Ok(Outcome { cases: Vec::new(), skipped, accepted: 0 });
    }

    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    // The build names the file as well as writing it: which file a suite runs
    // from is a claim on a shared one, and `binary` is that claim
    // (`actions::test_binary_at`). It is held until this function returns,
    // which is exactly as long as the file it names is the one being executed.
    let binary = actions::native_test_binary(
        session,
        target,
        &output,
        &args.flags,
        &mut program,
        &analysis.checked.tables,
        &mut diagnostics,
    )?;

    let limit = suite(session, target).and_then(|x| x.timeout_seconds);
    let program_path = binary.path().display().to_string();

    let blocks = match run_blocks(&program_path, limit, selected.len()) {
        Ok(Some(blocks)) => blocks,
        Ok(None) => return Err(timed_out(session, target, limit)),
        Err(e) => {
            diagnostics.push(
                Diagnostic::error(Span::NONE, format!("cannot run the test binary: {e}"))
                    .with_fix("the link produced it, so this is a toolchain bug"),
            );
            return Err(diagnostics);
        }
    };
    let objects: Vec<String> = selected
        .iter()
        .zip(&blocks)
        .map(|((name, module), block)| record_of(name, module, block))
        .collect();

    // The record is the same JSON a JavaScript run prints, and the cases are
    // parsed back out of it, so that a native verdict served from the cache and
    // a native verdict just produced are the same value by construction rather
    // than by two functions agreeing — and so that one `report_failure` states
    // the format for both backends.
    let record = format!("[{}]", objects.join(","));
    let mut cases = parse_results(&record);
    if may_cache(&cases, &args.flags) {
        crate::build::cache::Cache::open(&session.root).put(key, record.as_bytes());
    }
    locate(session, &program, &mut cases);
    let accepted =
        if args.flags.accept { accept_goldens(session, target, &cases, sink) } else { 0 };
    crate::parallel::discard(program);
    Ok(Outcome { cases, skipped, accepted })
}

/// The environment variable a native test binary reads the block to start at
/// from. `cli/runtime/testing.rs` is the other half.
const RESUME: &str = "BURI_TEST_FROM";

/// What one numbered block of a test binary did.
///
/// Numbered **per binary**, which is what makes this the same value whether the
/// binary holds one suite's tests or five: a block is a position in the entry
/// point the backend generated, and who owns it is the caller's question rather
/// than the runtime's.
enum Block {
    Passed,
    Failed { message: String, diff: Option<Diff> },
}

/// Runs a native test binary until every one of its `count` blocks has a
/// verdict, and says what each did.
///
/// One process per failure plus one. A block that aborted ended the process it
/// was in, so the blocks after it are run by the next: `RESUME` names the one to
/// start at and `buri_rt_test_enter` skips what is already reported. A clean run
/// is one process, which is the case that has to stay cheap.
///
/// `Ok(None)` is the timeout, which belongs to whoever declared it — a limit is
/// a suite's, and the diagnostic naming the suite is the caller's to raise.
fn run_blocks(
    program: &str,
    limit: Option<u32>,
    count: usize,
) -> std::io::Result<Option<Vec<Block>>> {
    let mut blocks: Vec<Block> = Vec::with_capacity(count);
    let mut from = 0usize;
    while from < count {
        let start = from.to_string();
        let out = match execute(program, None, limit, &[(RESUME, start.as_str())])? {
            Execution::Finished(out) => out,
            Execution::TimedOut => return Ok(None),
        };
        // A run that ended without aborting is a verdict for **every** block
        // from here on, because every one of them ran and none of them stopped
        // the process. Those verdicts are real, not assumed.
        if out.status.success() {
            while blocks.len() < count {
                blocks.push(Block::Passed);
            }
            break;
        }
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        // The block the process was in, from the process itself. A run that
        // ended some other way — a signal, or a failure before the first block
        // — said nothing, and the honest attribution is then the block it was
        // told to start at, with whatever it wrote to standard error.
        let noted = noted_failure(&stdout).filter(|n| (from..count).contains(&n.at));
        let at = noted.as_ref().map_or(from, |n| n.at);
        let message = match &noted {
            Some(n) => n.message.clone(),
            None => {
                let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if text.is_empty() {
                    format!("the run exited {}", out.status.code().unwrap_or(-1))
                } else {
                    text
                }
            }
        };
        while blocks.len() < at {
            blocks.push(Block::Passed);
        }
        blocks.push(Block::Failed { message, diff: noted.and_then(|n| n.diff) });
        from = at + 1;
    }
    Ok(Some(blocks))
}

/// One block's verdict as the runner's JSON, which is where a native record and
/// a JavaScript one become the same value.
fn record_of(name: &str, module: &str, block: &Block) -> String {
    match block {
        Block::Passed => passing_record(name, module),
        Block::Failed { message, diff } => failing_record(name, module, message, diff.as_ref()),
    }
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
// Four more conditions, and each of them is a way for two suites to disagree
// about what building or running them means:
//
// - **The same platform.** Every member runs on [`default_platform`]'s answer,
//   which is one fact about the pass; a suite that *named* its platforms made a
//   request, and a request is served on its own.
// - **The same profile.** `--release` is the invocation's, so this is free — but
//   it is named because it is in every `codegen` key and a batch has one link.
// - **No `test { data }`.** Such a suite runs on JavaScript anyway
//   (`default_platform`'s rule), so it is never a candidate.
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
// # The fallback
//
// Any reason at all to doubt the batch abandons it, silently, and every member
// then runs the way it would have. A failed front end, a failed monomorphization,
// an intrinsic the backend has no body for, a failed link: all four are answered
// per suite below, which is where the diagnostic can name one suite instead of
// five, and where `run_on`'s two fallback steps already live.

/// The suites this pass ran in shared binaries, and what each one's tests did.
///
/// Empty is the answer that costs nothing and changes nothing: a repository with
/// one suite, a pass whose suites are all cached, `--accept`, `--output=`, a
/// toolchain with no native backend. The loop below neither knows nor cares —
/// a suite that is not in here is compiled, linked and run exactly as it was.
fn run_batches(
    session: &mut Session,
    targets: &[TargetId],
    args: &arguments::Args,
    notices: &mut Notices,
) -> Prepass {
    let mut pre = Prepass::default();
    // A batch is only ever the *default's* answer. `--accept` routes to
    // JavaScript for the whole pass, and `--output=` is a request — served the
    // way requests are, one suite at a time.
    if args.flags.accept || args.flags.output.is_some() {
        return pre;
    }
    // The build file's half of the question, before the platform is decided, so
    // that a repository which was never going to batch does not cause
    // `default_platform` to state a notice earlier than the suite that earns it.
    let possible: Vec<TargetId> = targets
        .iter()
        .copied()
        .filter(|&t| has_tests(session, t) && may_batch(session, t))
        .collect();
    if possible.len() < 2 {
        return pre;
    }
    let platform = default_platform(&args.flags, notices);
    if !platform.is_native() || !native_ready(platform, &args.flags) {
        return pre;
    }
    // Two filters, and the cache goes first: it is the answer for every suite
    // in a repository nobody has edited, and asking it first is what keeps a
    // `buri test` with nothing to do from walking five dependency closures to
    // find that out. The lookup is **silent** — `served` is what reports a hit,
    // in the loop that reports every other suite, in order.
    let cache = crate::build::cache::Cache::open(&session.root);
    let mut fresh: Vec<TargetId> = Vec::new();
    for target in possible {
        if already_cached(session, &cache, target, platform, args, &mut pre) {
            continue;
        }
        // The graph's own rules, asked before this suite is compiled into a
        // shared artifact. A suite that fails one of them is reported by
        // `run_suite` below, which checks the same three; what it must not do
        // first is contribute its closure to a binary the rule exists to
        // prevent.
        let mut diagnostics = Diagnostics::new();
        actions::check_policy(session, target, platform, &mut diagnostics);
        if diagnostics.has_errors() {
            continue;
        }
        fresh.push(target);
    }
    for members in batches_of(session, &fresh) {
        // A batch of one is the path that already exists, and taking it here
        // would be a second copy of it.
        if members.len() < 2 {
            continue;
        }
        run_batch(session, &members, platform, args, &mut pre);
    }
    pre
}

/// Whether a suite's *build file* leaves it free to share a binary.
///
/// Everything here is decidable before a byte is compiled, and each condition is
/// a way two suites would disagree about what building or running them means —
/// a declared platform is a request rather than a preference, `data` sends the
/// suite to JavaScript, and a declared timeout has to bound one suite's process
/// rather than several suites'.
fn may_batch(session: &Session, target: TargetId) -> bool {
    let Some(suite) = suite(session, target) else { return false };
    suite.platforms.is_empty() && suite.data.is_empty() && suite.timeout_seconds.is_none()
}

/// Whether this suite's verdict is already on disk under its own key.
///
/// The same three modes [`served`] refuses to serve, and the same test that its
/// bytes are a record rather than an empty one — asked without printing
/// anything, because printing it is the reporting loop's job and doing it twice
/// would put a suite in the transcript before its neighbours.
fn already_cached(
    session: &Session,
    cache: &crate::build::cache::Cache,
    target: TargetId,
    platform: Platform,
    args: &arguments::Args,
    pre: &mut Prepass,
) -> bool {
    if args.flags.force || args.flags.filter.is_some() || args.flags.accept {
        return false;
    }
    // Kept, because the loop below asks for the same key again to report the
    // suite — and building one reads every source in its closure.
    let key = test_key_for(session, target, platform, args, pre);
    pre.keys.push(((target, platform), key.clone()));
    cache.get(&key).is_some_and(|bytes| !parse_results(&String::from_utf8_lossy(&bytes)).is_empty())
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
    let path = session.workspace.package(target.package).path.clone();
    suite
        .sources
        .iter()
        .map(|src| {
            let stem = src.value.strip_suffix(".buri").unwrap_or(&src.value);
            if path.is_empty() {
                format!("//{stem}")
            } else {
                format!("//{path}/{stem}")
            }
        })
        .collect()
}

/// One batch: one front end, one link, one binary, one verdict per member.
///
/// Every early return abandons the batch and says nothing. That is the whole of
/// the safety argument: a batch that is not certain is not a batch, and the
/// suites in it are compiled and run below one at a time, where a diagnostic can
/// name the one suite it belongs to and where `run_on`'s two fallbacks to
/// JavaScript already live.
fn run_batch(
    session: &mut Session,
    members: &[TargetId],
    platform: Platform,
    args: &arguments::Args,
    pre: &mut Prepass,
) {
    // One unit per member, in the pass's order, which is the order their test
    // sources load in and therefore the order the binary's blocks come out in.
    let units: Vec<Unit> = members
        .iter()
        .map(|&target| Unit { target: Some(target), platform: None, with_tests: true })
        .collect();
    let analysis = crate::compiler::driver::analyze_all(
        Some(&session.workspace),
        &mut session.map,
        &mut session.parsed,
        &units,
    );
    if analysis.diagnostics.has_errors() {
        return;
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
    if diagnostics.has_errors() || program.roots.tests().is_empty() {
        return;
    }
    // The gap probe, asked of the batch. It cannot say *which* member reaches
    // the intrinsic it names, and a notice that named the wrong suite would be
    // worse than no batch — so a batch with a gap in it is abandoned and each
    // member asks the same question about itself.
    if native_gap(platform, &args.flags, &program, &analysis.checked.tables).is_some() {
        return;
    }

    // Which suite owns each module that declares tests. Built from the build
    // files rather than from the program, so a module the batch loaded for some
    // other reason cannot be mistaken for a suite's.
    let owners: Vec<(String, usize)> = members
        .iter()
        .enumerate()
        .flat_map(|(i, &t)| test_modules_of(session, t).into_iter().map(move |m| (m, i)))
        .collect();
    let owner_of = |module: &str| -> Option<usize> {
        owners.iter().find(|(m, _)| m == module).map(|(_, i)| *i)
    };

    // What a `--filter` leaves out, per suite, counted before the roots are
    // narrowed — the same count `run_on` takes, taken once for the batch.
    let mut skipped = vec![0usize; members.len()];
    if let Some(f) = &args.flags.filter {
        for test in program.roots.tests() {
            if !test.name.contains(f.as_str()) {
                if let Some(i) = owner_of(&test.module) {
                    if let Some(n) = skipped.get_mut(i) {
                        *n += 1;
                    }
                }
            }
        }
        if let monomorphize::ProgramRoots::Tests(tests) = &mut program.roots {
            tests.retain(|t| t.name.contains(f.as_str()));
        }
    }
    // Block index -> the suite that owns it. A root whose module belongs to no
    // member cannot arise — only a member's test sources are loaded with
    // `Role::TestSource` — and if it ever did, the batch is abandoned rather
    // than a test attributed to a suite that does not own it.
    let mut selected: Vec<(usize, String, String)> = Vec::new();
    for test in program.roots.tests() {
        let Some(i) = owner_of(&test.module) else { return };
        selected.push((i, test.name.clone(), test.module.clone()));
    }

    // A `--filter` that matched nothing leaves a program with no roots. Nothing
    // to link and nothing to run, and the report is the skipped count — which is
    // exactly what `run_native` answers in the same position.
    if selected.is_empty() {
        for (i, &target) in members.iter().enumerate() {
            pre.done.push((
                target,
                Outcome {
                    cases: Vec::new(),
                    skipped: skipped.get(i).copied().unwrap_or(0),
                    accepted: 0,
                },
            ));
        }
        return;
    }

    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    // Held until this function returns, which is exactly as long as the file it
    // names is the one being executed (`actions::claim_runner`).
    let binary = match actions::native_test_batch(
        session,
        members,
        &output,
        &args.flags,
        &mut program,
        &analysis.checked.tables,
        &mut diagnostics,
    ) {
        Ok(binary) => binary,
        Err(_) => return,
    };

    // Reported per suite, because the `test` action is per suite: the batch is
    // how the verdicts were produced, not what they are keyed on.
    for &target in members {
        let key = test_key_for(session, target, platform, args, pre);
        crate::build::cache::explain(
            args.flags.explain,
            crate::build::cache::Status::Run,
            crate::build::cache::Action::Test,
            &session.workspace.label(target),
            platform,
            &key,
        );
    }
    // No limit: a suite that declared one is not in a batch, so there is no
    // suite here whose `timeout_seconds` a shared process could misrepresent.
    let blocks = match run_blocks(&binary.path().display().to_string(), None, selected.len()) {
        Ok(Some(blocks)) => blocks,
        _ => return,
    };

    let mut records: Vec<Vec<String>> = vec![Vec::new(); members.len()];
    for ((i, name, module), block) in selected.iter().zip(&blocks) {
        if let Some(slot) = records.get_mut(*i) {
            slot.push(record_of(name, module, block));
        }
    }
    let cache = crate::build::cache::Cache::open(&session.root);
    for (i, &target) in members.iter().enumerate() {
        let record = format!("[{}]", records.get(i).map(|r| r.join(",")).unwrap_or_default());
        let mut cases = parse_results(&record);
        if may_cache_produced(&cases, &args.flags) {
            cache.put(&test_key_for(session, target, platform, args, pre), record.as_bytes());
        }
        locate(session, &program, &mut cases);
        pre.done.push((
            target,
            Outcome { cases, skipped: skipped.get(i).copied().unwrap_or(0), accepted: 0 },
        ));
    }

    let crate::compiler::driver::Analysis { loaded, checked, diagnostics: analysed } = analysis;
    drop(loaded);
    drop(analysed);
    crate::parallel::discard(checked);
    crate::parallel::discard(program);
}

/// One JSON string literal, escaped as `JSON.stringify` escapes it.
///
/// Not `javascript::quote`, which picks whichever quote character needs less
/// escaping: a single-quoted literal is JavaScript and is not JSON, and
/// [`parse_results`] — which reads what `$run` wrote — looks for a double one.
fn json_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One test that ran and did not abort, in the runner's JSON.
fn passing_record(name: &str, module: &str) -> String {
    format!(
        "{{\"name\":{},\"module\":{},\"ms\":0,\"ok\":true}}",
        json_quote(name),
        json_quote(module)
    )
}

/// One test that aborted, in the shape `$run` writes for a caught throw — the
/// message and, where the assertion had them, both rendered values.
fn failing_record(name: &str, module: &str, message: &str, diff: Option<&Diff>) -> String {
    let error = match diff {
        Some(d) => format!(
            "{{\"message\":{},\"actual\":{},\"expected\":{}}}",
            json_quote(message),
            json_quote(&d.actual),
            json_quote(&d.expected)
        ),
        None => format!("{{\"message\":{}}}", json_quote(message)),
    };
    format!(
        "{{\"name\":{},\"module\":{},\"ms\":0,\"ok\":false,\"error\":{error}}}",
        json_quote(name),
        json_quote(module)
    )
}

/// What a native test binary said about the block that ended it.
struct Noted {
    at: usize,
    message: String,
    diff: Option<Diff>,
}

/// The line a native test binary writes when a block aborts.
///
/// The last object on standard output, because the process writes one and then
/// stops; reading the last rather than the first means a suite that somehow
/// produced two is reported by the one that ended it.
fn noted_failure(stdout: &str) -> Option<Noted> {
    let chunk = split_objects(stdout).pop()?;
    let at = field_raw(&chunk, "i")?.parse().ok()?;
    let diff = match (field(&chunk, "actual"), field(&chunk, "expected")) {
        (Some(actual), Some(expected)) => Some(Diff { actual, expected }),
        _ => None,
    };
    Some(Noted { at, message: field(&chunk, "message").unwrap_or_default(), diff })
}

/// Attaches each case to the source location of the test it names.
///
/// By title *and module*. Two files of one suite may use one title — that is
/// legal, and each reports its own failure at its own line — so a match on the
/// title alone gives the second file's failure the first file's location, in
/// the first file. Two tests sharing a title inside one file cannot arise:
/// `duplicate-test-name` refuses them before anything is compiled.
fn locate(session: &Session, program: &monomorphize::Program, cases: &mut [Case]) {
    for c in cases.iter_mut() {
        if let Some(t) =
            program.roots.tests().iter().find(|t| t.name == c.name && t.module == c.module)
        {
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

// ---------------------------------------------------------------------------
// --accept
// ---------------------------------------------------------------------------

/// Rewrites the declared `data` files whose contents a failing `assert.eq`
/// expected, and prints a diff for each.
///
/// Rewriting a golden file is not something a hermetic action may do, so this
/// is a separate mode rather than a step in the normal path (TESTING.md, "Test
/// data and golden files"). Three things bound it, and all three are visible
/// here: only a file listed in `data` is considered, a file that does not exist
/// is never created, and the value written is the one the test actually
/// produced. Everything else about the run is unchanged — the failure is still
/// reported and still counted, because what `--accept` changes is the source
/// tree, not the verdict.
fn accept_goldens(session: &Session, target: TargetId, cases: &[Case], out: &mut Out) -> usize {
    let Some(suite) = suite(session, target) else { return 0 };
    if suite.data.is_empty() {
        return 0;
    }
    let dir = session.workspace.package(target.package).dir.clone();
    let label = session.workspace.label(target);
    let mut accepted = 0usize;
    for c in cases {
        let Verdict::Failed { diff: Some(diff), .. } = &c.verdict else { continue };
        // Only a text assertion can name a file's contents, and `$show` renders
        // a `Str` as a JSON string. Anything else is a failure `--accept` has
        // no opinion about.
        let (Some(actual), Some(expected)) = (unquote(&diff.actual), unquote(&diff.expected))
        else {
            continue;
        };
        for d in &suite.data {
            let path = dir.join(&d.value);
            // Never creates a file: a golden that is not there is a golden
            // nobody declared the contents of, and inventing one would make
            // `--accept` a way to add test data by running the tests.
            let Ok(body) = std::fs::read_to_string(&path) else { continue };
            if body != expected {
                continue;
            }
            if std::fs::write(&path, &actual).is_err() {
                continue;
            }
            print_diff(&label, &d.value, &body, &actual, out);
            accepted += 1;
            break;
        }
    }
    accepted
}

/// The diff `--accept` prints for one rewritten golden.
///
/// Line-oriented, with the common head and tail elided, because what a reader
/// checks is the lines that moved.
fn print_diff(label: &str, file: &str, before: &str, after: &str, out: &mut Out) {
    out.line(&format!("accepted {label}  {file}"));
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let head = old.iter().zip(new.iter()).take_while(|(a, b)| a == b).count();
    // The elided tail must not reach back past the elided head, or the two
    // would overlap and the middle would be printed inside out — which is what
    // the two caps are for.
    let tail = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(old.len() - head)
        .min(new.len() - head);
    for line in old.get(head..old.len() - tail).unwrap_or_default() {
        out.line(&format!("  -{line}"));
    }
    for line in new.get(head..new.len() - tail).unwrap_or_default() {
        out.line(&format!("  +{line}"));
    }
}

/// A JSON string literal back to the text it stands for, or `None` when the
/// rendering is not one.
fn unquote(shown: &str) -> Option<String> {
    let inner = shown.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                let code = u32::from_str_radix(&hex, 16).ok()?;
                out.push(char::from_u32(code)?);
            }
            other => out.push(other),
        }
    }
    Some(out)
}

fn load_test_data(session: &Session, target: TargetId) -> String {
    let Some(suite) = suite(session, target) else { return "{}".into() };
    let dir = session.workspace.package(target.package).dir.clone();
    let mut fields = Vec::new();
    for d in &suite.data {
        let p: PathBuf = dir.join(&d.value);
        let body = std::fs::read_to_string(&p).unwrap_or_default();
        fields.push(format!("{}:{}", javascript::quote(&d.value), javascript::quote(&body)));
    }
    format!("{{{}}}", fields.join(","))
}

/// The runner writes one JSON array; this reads it without a JSON library,
/// because the shape is fixed and known.
fn parse_results(text: &str) -> Vec<Case> {
    let json = match text.find('[').and_then(|i| text.get(i..)) {
        Some(json) => json,
        None => return Vec::new(),
    };
    let mut cases = Vec::new();
    for chunk in split_objects(json) {
        let name = field(&chunk, "name").unwrap_or_default();
        let module = field(&chunk, "module").unwrap_or_default();
        let verdict = if chunk.contains("\"ok\":true") {
            Verdict::Passed
        } else {
            // Half a diff is no diff: one side alone can neither be printed
            // against anything nor accepted into a golden.
            let diff = match (field(&chunk, "actual"), field(&chunk, "expected")) {
                (Some(actual), Some(expected)) => Some(Diff { actual, expected }),
                _ => None,
            };
            Verdict::Failed { message: field(&chunk, "message").unwrap_or_default(), diff }
        };
        cases.push(Case {
            provenance: Provenance::Ran,
            name,
            module,
            verdict,
            location: None,
        });
    }
    cases
}

fn split_objects(json: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut in_str = false;
    let bytes = json.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'{' if !in_str => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' if !in_str => {
                depth -= 1;
                // Both ends are the offsets of an ASCII brace, so the range is
                // a character boundary whatever text the object holds.
                if depth == 0 {
                    if let Some(object) = json.get(start..=i) {
                        out.push(object.to_string());
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn field(chunk: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":\"");
    let rest = chunk.get(chunk.find(&key)? + key.len()..)?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    Some(out)
}

fn field_raw(chunk: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":");
    let rest = chunk.get(chunk.find(&key)? + key.len()..)?;
    Some(rest.get(..rest.find([',', '}'])?)?.trim().to_string())
}

/// A test's title, as one quoted line.
///
/// The report is one line per `FAIL`, which is what makes it greppable, and a
/// title is whatever somebody typed between the quotes. A `"` in one would
/// close the quoting and a newline would end the line, so both are escaped —
/// the rendering is the source syntax the title was written in.
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
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A failure message, indented under the `FAIL` line it belongs to.
///
/// Every line, not just the first: a message spanning lines whose second line
/// is flush left reads as the report having ended, and an `assert.fail` message
/// and an abort message are both free to span lines.
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
    out: &mut Out,
) {
    let label = session.workspace.label(target);
    let file = c.module.trim_start_matches("//");
    let file = file.strip_prefix(&session.workspace.package(target.package).path).unwrap_or(file);
    let file = file.trim_start_matches('/');
    out.line(&format!("FAIL {label}  {file}.buri  {}", quote_title(&c.name)));
    out.line(&indented(message));
    if let Some(d) = diff {
        out.line(&format!("    actual:   {}", d.actual));
        out.line(&format!("    expected: {}", d.expected));
    }
    if let Some(loc) = &c.location {
        out.line(&format!("  --> {loc}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_string_comes_back_as_the_text_it_stands_for() {
        assert_eq!(unquote("\"zero\"").as_deref(), Some("zero"));
        assert_eq!(unquote("\"a\\tb\\n\"").as_deref(), Some("a\tb\n"));
        assert_eq!(unquote("\"say \\\"hi\\\"\"").as_deref(), Some("say \"hi\""));
        assert_eq!(unquote("\"\\u0041\"").as_deref(), Some("A"));
        // Not a string rendering: `--accept` has no opinion about these.
        assert_eq!(unquote("19"), None);
        assert_eq!(unquote(".Some(1)"), None);
    }

    /// One line per `FAIL`, whatever the title holds.
    #[test]
    fn a_title_is_printed_as_the_quoted_string_it_was_written_as() {
        assert_eq!(quote_title("plain"), "\"plain\"");
        assert_eq!(quote_title("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_title("two\nlines"), "\"two\\nlines\"");
        assert_eq!(quote_title("a\\b\tc"), "\"a\\\\b\\tc\"");
        for title in ["plain", "say \"hi\"", "two\nlines", "a\\b\tc"] {
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
    /// backend's own, from a runtime key with no entry. The
    /// negative cases are the ones that must never fall back — a failure that
    /// is not a gap is a toolchain bug, and running the suite somewhere else
    /// would bury it.
    #[test]
    fn only_a_backend_gap_falls_back() {
        let of = |messages: &[&str]| {
            let mut diagnostics = Diagnostics::new();
            for m in messages {
                diagnostics.push(Diagnostic::error(Span::NONE, (*m).to_string()));
            }
            is_backend_gap(&diagnostics)
        };
        assert!(of(&["the stencil backend has no implementation of char.isDigit"]));
        assert!(of(&["the native runtime has no implementation of `json.decode`"]));
        assert!(of(&[
            "the llvm backend has no implementation of testing_assert.report",
            "the native runtime has no implementation of `bytes.toUtf8`",
        ]));
        // Not a gap, and not a fallback: an empty failure, a failure of another
        // kind, and a mixture with one of each.
        assert!(!of(&[]));
        assert!(!of(&["cannot declare the entry point: duplicate definition"]));
        assert!(!of(&[
            "the stencil backend has no implementation of char.isDigit",
            "cannot declare the entry point: duplicate definition",
        ]));
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
        let record = format!(
            "[{},{}]",
            passing_record("a title", "//lib/x/test/x"),
            failing_record("say \"hi\"", "//lib/x/test/x", "assert.eq failed", Some(&diff))
        );
        let cases = parse_results(&record);
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].name, "a title");
        assert_eq!(cases[0].module, "//lib/x/test/x");
        assert!(matches!(cases[0].verdict, Verdict::Passed));
        // The title's quotes survive the record, and so do the escapes inside
        // a rendered value: `$show` already escaped them, and the record
        // escapes what it is handed.
        assert_eq!(cases[1].name, "say \"hi\"");
        let Verdict::Failed { message, diff: Some(d) } = &cases[1].verdict else {
            panic!("the failing record did not read back as a failure");
        };
        assert_eq!(message, "assert.eq failed");
        assert_eq!(d.actual, "\"a\\tb\"");
        assert_eq!(d.expected, "2");
    }

    /// The line a native test binary writes when a block aborts.
    ///
    /// The literal is the contract with `cli/runtime/testing.rs`, so it is
    /// written out here rather than produced: the two are in different crates
    /// and nothing but this test compares them.
    #[test]
    fn a_native_binary_says_which_block_aborted() {
        let noted = noted_failure("{\"i\":3,\"message\":\"assert.eq failed\",\"actual\":\"1\",\"expected\":\"2\"}\n")
            .expect("a record with an index is a record");
        assert_eq!(noted.at, 3);
        assert_eq!(noted.message, "assert.eq failed");
        let diff = noted.diff.expect("both sides were there");
        assert_eq!((diff.actual.as_str(), diff.expected.as_str()), ("1", "2"));
        // An abort that is not an assertion has a message and no pair, and a
        // run that said nothing at all has no record — which is the case the
        // caller attributes to the block it asked for.
        let plain = noted_failure("{\"i\":0,\"message\":\"division by zero\"}\n").unwrap();
        assert!(plain.diff.is_none());
        assert!(noted_failure("").is_none());
        assert!(noted_failure("assert.eq failed\n").is_none());
    }

    /// A JSON string literal is JSON: the runner's own parser looks for a
    /// double quote, and `javascript::quote` prefers whichever quote character
    /// needs less escaping.
    #[test]
    fn a_record_field_is_quoted_as_json_and_not_as_javascript() {
        assert_eq!(json_quote("it's"), "\"it's\"");
        assert_eq!(javascript::quote("it's"), "\"it's\"");
        assert_eq!(json_quote("plain"), "\"plain\"");
        assert_eq!(javascript::quote("plain"), "'plain'");
        assert_eq!(json_quote("a\tb\nc\"d\\e"), "\"a\\tb\\nc\\\"d\\\\e\"");
        assert_eq!(json_quote("\u{1}"), "\"\\u0001\"");
    }
}
