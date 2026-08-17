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
use crate::build::workspace::{RuleKind, TargetId};
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
    ms: u64,
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

pub fn cmd_test(args: &arguments::Args) -> i32 {
    if !args.flags.watch {
        return one_pass(args, false).code;
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
    watch::Watch::on(root, args.flags.explain).drive(|_| one_pass(args, true))
}

/// One `buri test` invocation, whole.
///
/// `watching` decides two things and nothing else: whether the output is held
/// for the loop to place, and whether the declared input set is collected from
/// the session that just ran the suites — which is what makes the watch set and
/// the keys the same enumeration rather than two that have to be kept in step.
fn one_pass(args: &arguments::Args, watching: bool) -> watch::Pass {
    let mut out = if watching { Out::Held(String::new()) } else { Out::Direct };
    let mut s = match session::open(&args.flags) {
        Ok(s) => s,
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
    let broken = s.report();
    if broken && !watching {
        return watch::Pass { code: 2, inputs: Vec::new(), output: out.take(), quiet: false };
    }
    let targets = match s.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            // Nothing to watch and nothing to run: a selection that names no
            // target is a mistake in the invocation, which no edit will fix.
            return watch::Pass { code: 2, inputs: Vec::new(), output: out.take(), quiet: false };
        }
    };
    let inputs = if watching { watch::inputs(&s, &targets) } else { Vec::new() };
    if broken {
        return watch::Pass { code: 2, inputs, output: out.take(), quiet: false };
    }

    let started = Instant::now();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut cached = 0usize;
    let mut suites = 0usize;
    let mut printed = false;
    let mut hard_error = false;

    for &target in &targets {
        if !has_tests(&s, target) {
            continue;
        }
        suites += 1;
        match run_suite(&mut s, target, args, &mut out) {
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
                            report_failure(&s, target, c, message, diff.as_ref(), &mut out);
                            printed = true;
                        }
                    }
                }
            }
            Err(diags) => {
                hard_error |= s.print(&diags);
            }
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    if suites == 0 {
        out.line("no test suites");
        return watch::Pass { code: 0, inputs, output: out.take(), quiet: false };
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

fn has_tests(s: &Session, target: TargetId) -> bool {
    let pkg = s.ws.pkg(target.pkg);
    match target.kind {
        RuleKind::Library => {
            pkg.build.library.as_ref().is_some_and(|l| {
                l.test.as_ref().is_some_and(|t| !t.sources.is_empty())
            })
        }
        RuleKind::Binary => {
            pkg.build.binary.as_ref().is_some_and(|b| {
                b.test.as_ref().is_some_and(|t| !t.sources.is_empty())
            })
        }
    }
}

fn suite(s: &Session, target: TargetId) -> Option<crate::build::buildfile::TestSuite> {
    let pkg = s.ws.pkg(target.pkg);
    match target.kind {
        RuleKind::Library => pkg.build.library.as_ref().and_then(|l| l.test.clone()),
        RuleKind::Binary => pkg.build.binary.as_ref().and_then(|b| b.test.clone()),
    }
}

/// One suite, once per platform it runs on.
fn run_suite(
    s: &mut Session,
    target: TargetId,
    args: &arguments::Args,
    out: &mut Out,
) -> Result<Outcome, Diagnostics> {
    let mut diags = Diagnostics::new();
    // A suite inherits its target's tags and platform restrictions, so a suite
    // for a `server` library is checked as server code without saying
    // anything. A suite that names no platforms runs once on the host, and the
    // host here is the machine, not the JavaScript the backend emits.
    let declared: Vec<Platform> =
        suite(s, target).map(|x| x.platforms).unwrap_or_default().iter().map(|p| p.value).collect();
    let checked: Vec<Platform> = if declared.is_empty() {
        vec![crate::compiler::driver::host_native_platform()]
    } else {
        declared.clone()
    };
    // A platform a suite names must be one the target admits: asking for a JS
    // run of a `[LINUX, MACOS]` library is an error, not a skip
    // (TAGS.md, "Tags and tests").
    for p in &checked {
        actions::check_policy(s, target, *p, &mut diags);
    }
    if diags.has_errors() {
        return Err(diags);
    }

    // One run per declared platform. A native platform is executed natively
    // where this toolchain has a backend, a runtime archive and a linker for it
    // — the same three questions `buri build` asks — and refused in the same
    // words as a native build where it does not. A suite that names none is
    // checked against the host and executed as JavaScript, which is what
    // `buri test` has always done: the default is a statement about which
    // *runtime surface* every program can rely on, and that is still
    // JavaScript's (`design/native/BUILD-AND-WATCH.md` §5, wave 3c).
    let runs: Vec<Platform> = if declared.is_empty() { vec![Platform::Js] } else { declared };
    let mut outcome = Outcome::default();
    for platform in runs {
        if platform != Platform::Js && !native_ready(platform, &args.flags) {
            let span = suite(s, target).map(|x| x.span).unwrap_or(Span::NONE);
            diags.push(
                Diagnostic::error(
                    span,
                    format!("the {} backend is not implemented", platform.slug()),
                )
                .with_code("platform-not-implemented")
                .with_note("this toolchain emits JavaScript, so only a JS run can be executed")
                .with_fix(format!(
                    "drop {} from `test.platforms` until the backend exists",
                    platform.proto()
                )),
            );
            continue;
        }
        match run_on(s, target, platform, args, out) {
            Ok(one) => {
                outcome.cases.extend(one.cases);
                outcome.skipped += one.skipped;
                outcome.accepted += one.accepted;
            }
            Err(d) => diags.extend(d.items),
        }
    }
    if diags.has_errors() {
        return Err(diags);
    }
    Ok(outcome)
}

fn run_on(
    s: &mut Session,
    target: TargetId,
    platform: Platform,
    args: &arguments::Args,
    sink: &mut Out,
) -> Result<Outcome, Diagnostics> {
    let mut diags = Diagnostics::new();

    // A suite whose inputs are unchanged is not re-run and reports as cached;
    // `--force` re-runs anyway, which is the honest way to check that a suite
    // is not accidentally depending on the cache.
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    let key = actions::test_key(s, target, &output, &args.flags);
    let cache = crate::build::cache::Cache::open(&s.root);
    let label = s.ws.label(target);
    if !args.flags.force && args.flags.filter.is_none() && !args.flags.accept {
        if let Some(bytes) = cache.get(&key) {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let mut cached = parse_results(&text);
            for c in &mut cached {
                c.provenance = Provenance::Cache;
            }
            if !cached.is_empty() {
                crate::build::cache::explain(
                    args.flags.explain,
                    crate::build::cache::Status::Cached,
                    crate::build::cache::Action::Test,
                    &label,
                    platform,
                    &key,
                );
                return Ok(Outcome { cases: cached, skipped: 0, accepted: 0 });
            }
        }
    }
    crate::build::cache::explain(
        args.flags.explain,
        crate::build::cache::Status::Run,
        crate::build::cache::Action::Test,
        &label,
        platform,
        &key,
    );

    let unit = Unit { target: Some(target), platform, with_tests: true };
    let analysis = crate::compiler::driver::analyze(Some(&s.ws), &mut s.map, &mut s.parsed, &unit);
    if analysis.diags.has_errors() {
        return Err(analysis.diags);
    }

    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut program =
        monomorphize::run(&analysis.checked, module_paths, &mut diags, monomorphize::Roots::Tests);
    if diags.has_errors() {
        return Err(diags);
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

    if platform != Platform::Js {
        return run_native(s, target, platform, args, sink, &key, program, &analysis, skipped);
    }

    let mut source = actions::emit(
        &mut program,
        &analysis.checked.tables,
        crate::compiler::backend::Target { platform: Platform::Js, arch: None },
        &args.flags,
        &mut diags,
    )?;

    // The runner's in-memory `Fs` contains exactly `test { data: [...] }`, and
    // nothing else on disk is visible.
    let data = load_test_data(s, target);
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

    let dir = s.root.join(".buri/out/js").join(&s.ws.pkg(target.pkg).path);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("test.mjs");
    if let Err(e) = std::fs::write(&path, &source) {
        diags.push(
            Diagnostic::error(Span::NONE, format!("cannot write {}: {e}", path.display()))
                .with_fix("check the directory exists and is writable"),
        );
        return Err(diags);
    }

    let limit = suite(s, target).and_then(|x| x.timeout_seconds);
    let out = match execute(&js_runtime(), Some(&path), limit) {
        Ok(Execution::Finished(out)) => out,
        Ok(Execution::TimedOut) => return Err(timed_out(s, target, limit)),
        Err(e) => {
            diags.push(
                Diagnostic::error(Span::NONE, format!("cannot run the test binary: {e}"))
                    .with_fix("install bun, or point BURI_JS at a JavaScript runtime"),
            );
            return Err(diags);
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    // Only a clean run is worth remembering: a failure is what you are trying
    // to fix, and re-running it should re-run it. `--accept` is outside the
    // cache in both directions — it is the one mode that writes to the source
    // tree, and a mode that writes must not also be able to be served.
    let mut cases = parse_results(&stdout);
    if !cases.is_empty()
        && cases.iter().all(|c| matches!(c.verdict, Verdict::Passed))
        && args.flags.filter.is_none()
        && !args.flags.accept
    {
        cache.put(&key, stdout.as_bytes());
    }
    locate(s, &program, &mut cases);
    if cases.is_empty() && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        diags.push(
            Diagnostic::error(Span::NONE, "the test binary did not run")
                .with_fix("read the runtime's own message below; it is what failed")
                .with_note(err.trim().to_string()),
        );
        return Err(diags);
    }
    let accepted = if args.flags.accept { accept_goldens(s, target, &cases, sink) } else { 0 };
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
/// rather than quietly run through Cranelift.
fn native_ready(platform: Platform, flags: &arguments::Flags) -> bool {
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    actions::native_ready(actions::target_of(&output), actions::profile_of(flags))
}

/// One suite, executed as a native binary.
///
/// The differences from the JavaScript run are all consequences of one fact:
/// **there is no native test runner.** A failed assertion is
/// `buri_rt_abort_assert`, which prints and exits (SPEC 6.10: there is nothing
/// to catch), so the binary runs every `test` block in order and the first
/// failure is the last thing it does — `backend/cranelift/mod.rs`'s
/// `test_entry_point` is where that is decided and argued.
///
/// So:
///
/// - A clean run is a verdict for **every** test in it, because every block
///   ran and none of them aborted. Those cases are real, not assumed.
/// - A failing run names **one** failure. In a suite of one test that is the
///   test; in a suite of more, it says it cannot attribute it rather than
///   guessing, which is a worse report rather than a wrong answer.
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
    s: &mut Session,
    target: TargetId,
    platform: Platform,
    args: &arguments::Args,
    sink: &mut Out,
    key: &crate::build::cache::ActionKey,
    mut program: monomorphize::Program,
    analysis: &crate::compiler::driver::Analysis,
    skipped: usize,
) -> Result<Outcome, Diagnostics> {
    let mut diags = Diagnostics::new();
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
    let path = s
        .root
        .join(".buri/out")
        .join(output.dir())
        .join(&s.ws.pkg(target.pkg).path)
        .join("test");
    actions::native_test_binary(
        s,
        target,
        &output,
        &args.flags,
        &mut program,
        &analysis.checked.tables,
        &path,
        &mut diags,
    )?;

    let limit = suite(s, target).and_then(|x| x.timeout_seconds);
    let program_path = path.display().to_string();
    let out = match execute(&program_path, None, limit) {
        Ok(Execution::Finished(out)) => out,
        Ok(Execution::TimedOut) => return Err(timed_out(s, target, limit)),
        Err(e) => {
            diags.push(
                Diagnostic::error(Span::NONE, format!("cannot run the test binary: {e}"))
                    .with_fix("the link produced it, so this is a toolchain bug"),
            );
            return Err(diags);
        }
    };

    // The record is built as the same JSON a JavaScript run prints, and the
    // cases are parsed back out of it, so that a native verdict served from the
    // cache and a native verdict just produced are the same value by
    // construction rather than by two functions agreeing.
    if out.status.success() {
        let mut record = String::from("[");
        for (i, (name, module)) in selected.iter().enumerate() {
            if i > 0 {
                record.push(',');
            }
            record.push_str(&format!(
                "{{\"name\":{},\"module\":{},\"ms\":0,\"ok\":true}}",
                javascript::quote(name),
                javascript::quote(module)
            ));
        }
        record.push(']');
        let mut cases = parse_results(&record);
        if args.flags.filter.is_none() && !args.flags.accept {
            crate::build::cache::Cache::open(&s.root).put(key, record.as_bytes());
        }
        locate(s, &program, &mut cases);
        let accepted = if args.flags.accept { accept_goldens(s, target, &cases, sink) } else { 0 };
        return Ok(Outcome { cases, skipped, accepted });
    }

    let message = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let message = if message.is_empty() {
        format!("the run exited {}", out.status.code().unwrap_or(-1))
    } else {
        message
    };
    // A run with one test in it *does* know which one failed, and saying so is
    // the difference between a report that names a test and one that names a
    // suite. With more than one, the honest answer is the range.
    let (name, module, note) = match selected.as_slice() {
        [(name, module)] => (name.clone(), module.clone(), String::new()),
        _ => (
            format!("the {} run", platform.slug()),
            selected.first().map(|(_, m)| m.clone()).unwrap_or_default(),
            format!(
                " — a native run has no runner, so it stops at the first failure and cannot say \
                 which of the {} tests it was in",
                selected.len()
            ),
        ),
    };
    let mut cases = vec![Case {
        provenance: Provenance::Ran,
        name,
        module,
        location: None,
        ms: 0,
        verdict: Verdict::Failed { message: format!("{message}{note}"), diff: None },
    }];
    locate(s, &program, &mut cases);
    Ok(Outcome { cases, skipped, accepted: 0 })
}

/// Attaches each case to the source location of the test it names.
fn locate(s: &Session, program: &monomorphize::Program, cases: &mut [Case]) {
    for c in cases.iter_mut() {
        if let Some(t) = program.roots.tests().iter().find(|t| t.name == c.name) {
            if !t.span.is_none() {
                let f = s.map.get(t.span.file);
                let (line, col) = f.line_col(t.span.start);
                c.location = Some(format!("{}:{line}:{col}", f.name));
            }
        }
    }
}

/// The diagnostic a suite that ran past its own `timeout_seconds` gets.
fn timed_out(s: &Session, target: TargetId, limit: Option<u32>) -> Diagnostics {
    let mut diags = Diagnostics::new();
    let span = suite(s, target).map(|x| x.span).unwrap_or(Span::NONE);
    let seconds = limit.unwrap_or(0);
    diags.push(
        Diagnostic::error(span, format!("{}'s test suite ran longer than {seconds}s", s.ws.label(target)))
            .with_code("test-timeout")
            .with_label("the timeout this suite declares")
            .with_note(
                "the run was killed, so no test in this suite has a result — a timeout is \
                 the suite's, not one test's",
            )
            .with_fix(
                "raise `timeout_seconds`, or find the test that does not finish; a test that \
                 never returns is a loop with no exit, since nothing here blocks on I/O",
            ),
    );
    diags
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
fn accept_goldens(s: &Session, target: TargetId, cases: &[Case], out: &mut Out) -> usize {
    let Some(suite) = suite(s, target) else { return 0 };
    if suite.data.is_empty() {
        return 0;
    }
    let dir = s.ws.pkg(target.pkg).dir.clone();
    let label = s.ws.label(target);
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

fn load_test_data(s: &Session, target: TargetId) -> String {
    let Some(suite) = suite(s, target) else { return "{}".into() };
    let dir = s.ws.pkg(target.pkg).dir.clone();
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
        let ms = field_raw(&chunk, "ms").and_then(|s| s.parse().ok()).unwrap_or(0);
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
            ms,
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

/// Output names the target, the file, and the test (TESTING.md, "Running").
fn report_failure(
    s: &Session,
    target: TargetId,
    c: &Case,
    message: &str,
    diff: Option<&Diff>,
    out: &mut Out,
) {
    let label = s.ws.label(target);
    let file = c.module.trim_start_matches("//");
    let file = file.strip_prefix(&s.ws.pkg(target.pkg).path).unwrap_or(file);
    let file = file.trim_start_matches('/');
    out.line(&format!("FAIL {label}  {file}.buri  \"{}\"", c.name));
    out.line(&format!("  {message}"));
    if let Some(d) = diff {
        out.line(&format!("    actual:   {}", d.actual));
        out.line(&format!("    expected: {}", d.expected));
    }
    if let Some(loc) = &c.location {
        out.line(&format!("  --> {loc}"));
    }
    let _ = c.ms;
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
}
