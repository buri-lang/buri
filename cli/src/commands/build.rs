//! `buri build`.
//!
//! A binary is built; a library is checked. `buri build //lib/money` produces
//! no artifact and is not meant to — the library's outputs are whatever
//! depends on it — so what it reports is whether the code and the policy
//! around it hold.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the artifacts a build produced, and a refusal to start one, are this \
              command's output; every diagnostic about the code still leaves \
              through `Session::emit`"
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the only arithmetic here counts artifacts already produced, so it is \
              bounded by the number of targets the workspace holds"
)]

use crate::build::actions;
use crate::build::buildfile::Output;
use crate::build::session::Session;
use crate::build::session::{self, open_or_exit};
use crate::build::workspace::{RuleKind, TargetId};
use crate::commands::arguments;
use std::collections::BTreeMap;

pub fn command_build(args: &arguments::Args) -> i32 {
    if args.flags.check_reproducible {
        return check_reproducible(args);
    }
    // Whatever way the build ends, it waits for a cross runtime build it began.
    let _settle = Settle;
    let opened =
        session::open_and_resolve_to_build_with(&args.flags, &args.targets, |session, targets| {
            start_cross_runtimes(session, targets, &args.flags);
        });
    let (mut session, targets) = match opened {
        Ok(both) => both,
        Err(c) => return c as i32,
    };

    // `--output` selects one of the outputs a target declares. A selector that
    // matches none of them is the thing you asked *with* being wrong, and has
    // to say so: the build would otherwise report success having produced
    // nothing, which is the one answer a build system must never give.
    if let Some(selector) = &args.flags.output {
        let mut declared: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut matched = false;
        for t in &targets {
            if t.kind != RuleKind::Binary {
                continue;
            }
            for o in actions::selected_outputs(&session, *t, &arguments::Flags::default()) {
                declared.insert(o.dir());
                matched |= o.matches_selector(selector);
            }
        }
        // A target that declares no outputs at all has nothing for the fix to
        // name, and nothing the selector could have matched, so it is not this
        // mistake.
        let example = if matched { None } else { declared.first() };
        if let Some(example) = example {
            let names: Vec<&str> = declared.iter().map(String::as_str).collect();
            return no_output_matches(selector, &names, example);
        }
    }

    // Each library's own contribution to a check key, read once.
    let mut members = std::collections::HashMap::new();
    let mut rule_paths = actions::RulePaths::new();
    // What each library or tool asks before it is checked, and which of them
    // are checked: those are checked together below, each module once.
    let mut asked = std::collections::HashMap::new();
    for &target in targets.iter().filter(|t| t.kind != RuleKind::Binary) {
        let mut diagnostics = crate::diagnostics::Diagnostics::new();
        // A library declares no outputs, so there is no platform this is
        // *for*, and none to check. Every binary that depends on it asks
        // about its tags' platforms again, per output; a library that
        // admits none at all is `unsatisfiable-target`.
        actions::check_visibility(&session, target, &mut diagnostics);
        actions::check_tags(&session, target, &mut diagnostics);
        // A check that found nothing is recorded, and finds nothing again
        // while its sources and the graph stand still.
        let key = (!diagnostics.has_errors())
            .then(|| actions::library_check_key(&session, target, &args.flags, &mut members));
        let clean = !args.flags.force
            && key
                .as_ref()
                .is_some_and(|key| actions::clean_check_holds(&session, key));
        asked.insert(target, (diagnostics, key.filter(|_| !clean)));
    }
    // A library is checked, not built for an output, and it is handed no host
    // at all. See `Unit::platform`.
    let unit = |target| crate::compiler::modules::Unit {
        target: Some(target),
        platform: None,
        entry: None,
        with_tests: false,
    };
    let checked: Vec<crate::compiler::modules::Unit> = targets
        .iter()
        .filter(|t| asked.get(t).is_some_and(|(_, key)| key.is_some()))
        .map(|t| unit(*t))
        .collect();
    // `check_during_build`: what the catalogue will analyse is checked here
    // too, in the same compilation, so each module is checked once. Should
    // the two not share one, the libraries are checked alone, and the
    // catalogue checks its own once the build holds. Only the repository's
    // bodies are checked: the standard library's report nothing.
    let check_during_build = session.workspace.repo.lint.check_during_build;
    let linting = check_during_build
        .then(|| crate::commands::lint::Linting::before_build(&mut session, &targets, &args.flags))
        .flatten();
    let linted = linting
        .as_ref()
        .map(|l| l.units(&targets))
        .unwrap_or_default();
    let mut check = |units: &[crate::compiler::modules::Unit]| {
        crate::compiler::driver::shared(
            &session.workspace,
            &mut session.map,
            &mut session.parsed,
            units,
            crate::compiler::driver::Checking::Repository,
        )
    };
    let mut shared = check(&[linted.as_slice(), &checked].concat());
    let handed = shared.is_some() && !linted.is_empty();
    if shared.is_none() && !linted.is_empty() {
        shared = check(&checked);
    }
    // Each binary's outputs are built side by side, on the pool's workers, and
    // everything is printed in target order however the builds finish. This
    // thread checks the libraries, asks the cache about each output, and
    // loads each program a worker is to build — loading mints the file ids in
    // the session's map, so it happens here and only here.
    let workers = Workers {
        root: session.root.clone(),
        workspace: std::sync::Arc::clone(&session.workspace),
        rendering: session.rendering,
        flags: &args.flags,
    };
    let mut reports: Vec<Report> = Vec::new();
    // By slot: an output's place among the outputs, in target order.
    let mut outcomes: BTreeMap<usize, Outcome> = BTreeMap::new();
    let mut slots = 0;
    let mut printing = Printing::default();
    let mut crossing = Vec::new();
    let release = args.flags.mode.is_release();
    let budget = memory_budget();
    let mut most_held = (0, 0);
    let ran = crate::parallel::pool(
        crate::parallel::cores(),
        budget,
        |job: Job, held, _, _| job.build(held, &workers),
        |queue, done| {
            // By reference: the resolved list is the catalogue's input too, below.
            for &target in &targets {
                // Only a binary produces an artifact; a library or a tool is
                // checked, which is what `buri build //lib/money` means.
                if target.kind != RuleKind::Binary {
                    // Every library and tool in `targets` asked above.
                    let (mut diagnostics, key) = asked.remove(&target).unwrap_or_default();
                    let share = shared
                        .as_ref()
                        .and_then(|s| Some((s, s.shares.get(&(target, false))?)));
                    if let (Some(key), Some((shared, share))) = (&key, share) {
                        if share.reported.is_empty() {
                            actions::record_clean_share(
                                &session,
                                target,
                                key,
                                &shared.analysis,
                                share,
                                &mut rule_paths,
                            );
                        }
                        diagnostics.extend(share.reported.iter().cloned());
                    } else if let Some(key) = key {
                        let analysis = crate::compiler::driver::analyze(
                            Some(&session.workspace),
                            &mut session.map,
                            &mut session.parsed,
                            &unit(target),
                        );
                        if analysis.diagnostics.items.is_empty() {
                            actions::record_clean_check(
                                &session,
                                target,
                                &key,
                                &analysis,
                                &mut rule_paths,
                            );
                        }
                        diagnostics.extend(analysis.diagnostics.items);
                    }
                    reports.push(Report::Checked(diagnostics));
                    continue;
                }
                // One sink for every output of this target, because a mistake
                // in `main.buri` is one mistake however many artifacts are
                // built from it. Two outputs are two compilations of the same
                // sources, so a helper that will not typecheck otherwise
                // reports once per artifact.
                let first = slots;
                for output in actions::selected_outputs(&session, target, &args.flags) {
                    let slot = slots;
                    slots += 1;
                    let (prepared, explain) = crate::build::cache::holding_explain(|| {
                        actions::prepare_output(&session, target, &output, &args.flags)
                    });
                    match prepared {
                        actions::Prepared::Done(built) => {
                            let outcome =
                                outcome(&session, target, output, &args.flags, built, explain);
                            outcomes.insert(slot, outcome);
                        }
                        // Queued behind every other program, so the workers
                        // build those while its cross runtime is being built.
                        actions::Prepared::Front(front) if front.waits_for_cross() => {
                            crossing.push((slot, target, output, front, explain));
                        }
                        actions::Prepared::Front(front) => {
                            let (job, bytes) =
                                load(&mut session, slot, target, output, front, explain, release);
                            queue.push(job, bytes);
                            printing.pending += 1;
                        }
                    }
                }
                reports.push(Report::Built(first..slots));
            }
            for (slot, target, output, front, explain) in crossing {
                let (job, bytes) =
                    load(&mut session, slot, target, output, front, explain, release);
                queue.push(job, bytes);
                printing.pending += 1;
            }
            printing.flush(&session, &reports, &outcomes);
            while printing.pending > 0 {
                let (slot, outcome) = done.recv();
                outcomes.insert(slot, outcome);
                printing.pending -= 1;
                printing.flush(&session, &reports, &outcomes);
            }
            most_held = queue.most_held();
            true
        },
    )
    .unwrap_or_else(no_thread);
    if crate::profile::enabled() {
        let (programs, bytes) = most_held;
        eprintln!("programs held at once {programs}, expected {bytes} of {budget} bytes");
    }
    let Printing { built, failed, .. } = printing;
    let mut failed = failed | !ran;
    // `check_during_build`: the catalogue runs here too, but only over a build
    // that held — findings on code that does not compile are noise stacked on
    // top of the answer.
    if !failed && check_during_build {
        let findings = match linting {
            Some(linting) => {
                let shared = shared.filter(|_| handed);
                linting.finish(&mut session, &targets, Vec::new(), shared).0
            }
            None => crate::commands::lint::findings_for(&mut session, &targets, &args.flags),
        };
        failed |= session.print(&findings);
        // Which rules this repository turned off, wherever the catalogue runs.
        // A build that reports nothing because a rule is off says so here, or
        // the silence is indistinguishable from a clean tree.
        if let Some(note) = crate::commands::lint::rules_note(&session) {
            println!("{note}");
        }
    }
    if failed {
        return 1;
    }
    if built == 0 && args.flags.verbose {
        println!("nothing to build");
    }
    0
}

/// Whether an output failed only because this host cannot build it: a native
/// output for another machine, refused for the host or for a capability its
/// cross runtime lacks. A build that selects no output skips one of these
/// rather than failing (buri-lang/buri#194); `--output` naming it still fails.
/// The refusal for an `--output` that matches none of the outputs `declared`,
/// and the status it exits with. `buri run` says the same.
pub fn no_output_matches(selector: &str, declared: &[&str], example: &str) -> i32 {
    eprintln!("error: no declared output matches `--output={selector}`");
    eprintln!("  = declared: {}", declared.join(", "));
    eprintln!(
        "  = fix: name one of them, as in `--output={example}`, or add the output to the rule"
    );
    2
}

fn foreign(
    output: &crate::build::buildfile::Output,
    diagnostics: &crate::diagnostics::Diagnostics,
) -> bool {
    const HOST_REFUSALS: [&str; 3] = [
        "native-artifact-unavailable",
        "networking-unavailable",
        "cryptography-unavailable",
    ];
    let mut errors = diagnostics.items.iter().filter(|d| d.is_error());
    errors.all(|d| {
        d.code
            .as_deref()
            .is_some_and(|c| HOST_REFUSALS.contains(&c))
    }) && !crate::build::link::is_host_target(actions::target_of(output))
}

// ---------------------------------------------------------------------------
// Cross runtimes
// ---------------------------------------------------------------------------

/// Starts the cross runtime of every output the build will link for another
/// machine, before the generators run and long before the link: an output
/// the policy allows, for a target this host has a backend and a C driver
/// for. Those are the outputs that go on to ask for a linker.
fn start_cross_runtimes(session: &Session, targets: &[TargetId], flags: &arguments::Flags) {
    for &target in targets.iter().filter(|t| t.kind == RuleKind::Binary) {
        for output in actions::selected_outputs(session, target, flags) {
            let at = actions::target_of(&output);
            if !output.platform().is_native()
                || actions::native_gap(at, actions::profile_of(flags)).is_some()
                || !crate::build::link::reaches_cross_runtime(at)
            {
                continue;
            }
            let mut refused = crate::diagnostics::Diagnostics::new();
            actions::check_policy(session, target, &output.output_platform(), &mut refused);
            if !refused.has_errors() {
                crate::build::runtime_cross::start(at);
            }
        }
    }
}

/// Waits, when dropped, for the cross runtime builds the command began.
struct Settle;

impl Drop for Settle {
    fn drop(&mut self) {
        crate::build::runtime_cross::settle();
    }
}

// ---------------------------------------------------------------------------
// Programs side by side
// ---------------------------------------------------------------------------

/// What every worker building a program needs from the session, which stays
/// with the thread that loads.
struct Workers<'a> {
    root: std::path::PathBuf,
    workspace: std::sync::Arc<crate::build::workspace::Workspace>,
    rendering: session::Rendering,
    flags: &'a arguments::Flags,
}

/// One output whose program is loaded: checked, compiled and placed on a
/// worker.
struct Job {
    /// Where its [`Outcome`] goes among the outputs, in target order.
    slot: usize,
    target: TargetId,
    output: Output,
    front: Box<actions::Front>,
    loading: crate::compiler::driver::Loading,
    /// The session's map as the load left it, which names every file the
    /// build can report on.
    map: std::sync::Arc<crate::diagnostics::SourceMap>,
    /// The `--explain` lines its cache lookup printed.
    explain: String,
}

/// Loads the program `front` names, on the session's map, as a [`Job`] with
/// the bytes it is expected to hold.
fn load(
    session: &mut Session,
    slot: usize,
    target: TargetId,
    output: Output,
    front: Box<actions::Front>,
    explain: String,
    release: bool,
) -> (Job, u64) {
    let loading = crate::compiler::driver::load_all(
        Some(&session.workspace),
        &mut session.map,
        &mut session.parsed,
        std::slice::from_ref(&front.unit),
    );
    let bytes = build_bytes(loading.source_bytes(&session.map), release);
    let job = Job {
        slot,
        target,
        output,
        front,
        loading,
        map: session.map.shared(),
        explain,
    };
    (job, bytes)
}

/// Where a test holds `buri build` as a worker starts a program's front end
/// ([`crate::hold`]): two programs held there at once are two being built at
/// once.
#[cfg(feature = "test-hooks")]
static BUILDING: crate::hold::Point = crate::hold::Point::new("building");

impl Job {
    fn build(self, held: crate::parallel::Held<'_>, workers: &Workers<'_>) -> (usize, Outcome) {
        #[cfg(feature = "test-hooks")]
        BUILDING.held();
        let Job {
            slot,
            target,
            output,
            front,
            loading,
            map,
            explain,
        } = self;
        // A session of its own, over the map the load left: the build may
        // read a file more, and what it reports is printed against the map
        // that names it.
        let mut session = Session {
            root: workers.root.clone(),
            map: crate::diagnostics::SourceMap::clone(&map),
            parsed: crate::parsing::parser::Cache::new(),
            diagnostics: crate::diagnostics::Diagnostics::new(),
            workspace: std::sync::Arc::clone(&workers.workspace),
            rendering: workers.rendering,
        };
        drop(map);
        let analysis =
            crate::compiler::driver::check(loading, Some(&session.workspace), &session.map);
        let (built, finished) = crate::build::cache::holding_explain(|| {
            actions::finish_output(
                &mut session,
                target,
                &output,
                workers.flags,
                front,
                analysis,
            )
        });
        // The program is gone by now; only its artifact and its report are left.
        drop(held);
        let explain = explain + &finished;
        let mut outcome = outcome(&session, target, output, workers.flags, built, explain);
        outcome.map = Some(std::sync::Arc::new(session.map));
        (slot, outcome)
    }
}

/// One output's build, as it is to be printed.
struct Outcome {
    /// Its `--explain` lines, then the artifact's.
    stdout: String,
    reported: crate::diagnostics::Diagnostics,
    /// The map `reported` is printed against, when it isn't the session's.
    map: Option<std::sync::Arc<crate::diagnostics::SourceMap>>,
    /// Whether it produced an artifact.
    built: bool,
    /// The output `out` points at once this one is printed, when it built.
    linked: Option<Output>,
}

/// What an output's build reports: its artifact, or its diagnostics — or, for
/// an output this host can't build and nobody asked for by name, that it is
/// unavailable.
fn outcome(
    session: &Session,
    target: TargetId,
    output: Output,
    flags: &arguments::Flags,
    built: Result<actions::Artifact, crate::diagnostics::Diagnostics>,
    mut stdout: String,
) -> Outcome {
    let linked = built.is_ok();
    let mut reported = crate::diagnostics::Diagnostics::new();
    let placed = built.and_then(|a| actions::with_assets(session, target, &output, a));
    let made = placed.is_ok();
    match placed {
        Ok(a) => {
            let rel = a.path.strip_prefix(&session.root).unwrap_or(&a.path);
            let note = if a.cached { ", cached" } else { "" };
            stdout.push_str(&format!("{} ({} bytes{note})\n", rel.display(), a.bytes));
        }
        Err(diagnostics) if flags.output.is_none() && foreign(&output, &diagnostics) => {
            reported.push(
                crate::diagnostics::Diagnostic::templated("output-unavailable", output.span)
                    .with_bind("selector", output.dir())
                    .with_bind("target", session.workspace.label(target)),
            );
        }
        Err(diagnostics) => reported.extend(diagnostics.items),
    }
    Outcome {
        stdout,
        reported,
        map: None,
        built: made,
        linked: linked.then_some(output),
    }
}

/// A library's or tool's check, or a binary's outputs, in target order.
enum Report {
    Checked(crate::diagnostics::Diagnostics),
    Built(std::ops::Range<usize>),
}

/// How far printing has got through the reports.
#[derive(Default)]
struct Printing {
    next: usize,
    /// The jobs whose outcome hasn't come back.
    pending: usize,
    built: usize,
    failed: bool,
}

impl Printing {
    /// Prints every report whose outcomes are all in, up to the first that
    /// isn't. A binary's artifacts come first and its diagnostics after them,
    /// as one target's report. Printed here, in this order, rather than where
    /// each was built, because a code's explanation follows the first
    /// diagnostic printed with it.
    fn flush(
        &mut self,
        session: &Session,
        reports: &[Report],
        outcomes: &BTreeMap<usize, Outcome>,
    ) {
        while let Some(report) = reports.get(self.next) {
            match report {
                Report::Checked(diagnostics) => self.failed |= session.print(diagnostics),
                Report::Built(range) => {
                    let done: Option<Vec<&Outcome>> =
                        range.clone().map(|slot| outcomes.get(&slot)).collect();
                    let Some(done) = done else {
                        return;
                    };
                    for outcome in &done {
                        print!("{}", outcome.stdout);
                        if let Some(output) = &outcome.linked {
                            actions::link_out_symlink(session, output);
                        }
                        self.built += usize::from(outcome.built);
                    }
                    // One report for every output of the target, because a
                    // mistake in `main.buri` is one mistake however many
                    // artifacts are built from it. By file name: each output
                    // reports against a map of its own.
                    let mut seen = std::collections::HashSet::new();
                    for outcome in &done {
                        let map = outcome.map.as_deref().unwrap_or(&session.map);
                        for d in &outcome.reported.items {
                            let file = map.get(d.span.file).name.as_str();
                            if seen.insert((file, d.span.start, d.span.end, d.message.as_str())) {
                                session.emit_from(map, d);
                                self.failed |= d.is_error();
                            }
                        }
                    }
                }
            }
            self.next += 1;
        }
    }
}

/// What a build does on a machine with no thread to build on: says so, and
/// fails.
fn no_thread() -> bool {
    eprintln!("error: nothing could be built: the machine has no thread to build on");
    false
}

/// The bytes the programs being built may hold between them: half this
/// machine's memory, as `buri test` allows its builds. A machine that cannot
/// say how much memory it has is not limited.
fn memory_budget() -> u64 {
    machine_memory().map_or(u64::MAX, |bytes| bytes / 2)
}

/// Where a test says how much memory the machine has ([`crate::hold`]).
#[cfg(feature = "test-hooks")]
static MEMORY: crate::hold::Point = crate::hold::Point::new("memory");

/// The bytes of memory this machine has, or what a test says it has.
fn machine_memory() -> Option<u64> {
    #[cfg(feature = "test-hooks")]
    if let Some(bytes) = std::str::from_utf8(&MEMORY.held())
        .ok()
        .and_then(|text| text.trim().parse().ok())
    {
        return Some(bytes);
    }
    crate::parallel::memory_bytes()
}

/// The memory a build of `source` bytes of repository code is expected to
/// hold, from its check to its link.
///
/// An upper bound on what was measured. Building each of the 35 binaries of a
/// 100 MB repository on its own, the most a program's build grew the
/// process's footprint by was:
///
/// - `--release`: 822 MB for 4.1 MB of source, about 200 bytes a byte, and
///   up to 71 MB for programs under 300 KB.
/// - `--debug`, over the eight of those that grew most: 384 MB for the same
///   4.1 MB, about 94 bytes a byte, and under 20 MB for the small ones.
///
/// Each bound is at least 19% above every one of those.
fn build_bytes(source: u64, release: bool) -> u64 {
    const BASE: u64 = 64 * 1024 * 1024;
    let per_source_byte = if release { 240 } else { 120 };
    BASE.saturating_add(source.saturating_mul(per_source_byte))
}

// ---------------------------------------------------------------------------
// --check-reproducible
// ---------------------------------------------------------------------------

/// Builds every requested binary twice, into two separate directories, and
/// compares the artifacts byte for byte.
///
/// This is the check that carries the weight in this design. Hermeticity here
/// is a property of the type system rather than of a confinement the toolchain
/// applies — an action has no name for ambient state, so there is nothing to
/// confine — and the way a *toolchain* bug that leaked one would show up is as
/// two builds of one tree disagreeing. So the verification is here rather than
/// in a sandbox profile.
///
/// Three things make the two builds independent rather than the same build run
/// twice:
///
/// - **A fresh session each time.** The workspace, the source map, and every
///   cached parse are re-read from disk, so nothing an earlier build memoised
///   can carry a difference across.
/// - **The cache is not consulted.** A cache hit would compare an entry with
///   itself and pass unconditionally, which is the one way this check could be
///   worse than nothing.
/// - **Two different output directories.** An artifact that embedded the path
///   it was written to differs between them, which is the failure mode this is
///   most likely to catch.
fn check_reproducible(args: &arguments::Args) -> i32 {
    let mut flags = arguments::Flags {
        force: true,
        ..arguments::Flags::default()
    };
    // Everything that is part of the configuration has to be carried across,
    // because "the same configuration" is half of what is being claimed.
    flags.mode = args.flags.mode;
    flags.output = args.flags.output.clone();
    flags.verbose = args.flags.verbose;
    flags.color = args.flags.color;
    flags.error_format = args.flags.error_format;

    let mut differed = false;
    let mut compared = 0usize;
    // Resolved once, from a session that is then dropped: the two builds below
    // must each start from a repository nobody has looked at yet. This is also
    // where not being in one, or naming a target that is not there, is reported.
    let plan = match plan(args, &flags) {
        Ok(p) => p,
        Err(code) => return code,
    };

    for entry in plan {
        let Entry {
            label,
            path,
            package_path,
            artifact,
            platform,
            output,
        } = entry;
        if platform.is_native() {
            if let Some(gap) =
                actions::native_gap(actions::target_of(&output), actions::profile_of(&flags))
            {
                eprintln!("error: no native artifact for {}", gap.output);
                eprintln!("  = {}", gap.reason);
                eprintln!("  = fix: {}", gap.fix);
                return 1;
            }
            match check_native(
                &label,
                &path,
                &package_path,
                &artifact,
                &output,
                &flags,
                args,
            ) {
                Ok(moved) => {
                    compared += 1;
                    differed |= moved;
                    continue;
                }
                Err(code) => return code,
            }
        }
        // Every file the output writes, named, not just the module: a WEB
        // output is a page, a stylesheet and a module, and "reproducible" has
        // to mean all three or it means the interesting two thirds are
        // unchecked.
        let mut rounds: [Vec<(String, Vec<u8>)>; 2] = Default::default();
        for (slot, round) in rounds.iter_mut().zip(["a", "b"]) {
            // Two directories, thrown away as soon as the bytes are read back.
            // Outside the repository, because a check must not double as a
            // build and must not leave a directory in a tree it was only asked
            // a question about; named for this process, so two checks running
            // at once do not compare each other's artifacts.
            let round_dir = std::env::temp_dir()
                .join(format!("buri-reproducible-{}-{round}", std::process::id()));
            let _ = std::fs::remove_dir_all(&round_dir);
            let (mut session, target) = match round_session(&label, &flags) {
                Ok(both) => both,
                Err(code) => return code,
            };
            let mut diagnostics = crate::diagnostics::Diagnostics::new();
            let compiled = match actions::compile_artifact(
                &mut session,
                target,
                &output,
                &flags,
                &mut diagnostics,
            ) {
                Ok(compiled) => compiled,
                Err(diagnostics) => {
                    session.print(&diagnostics);
                    return 1;
                }
            };
            // Written and read back rather than compared in memory, under two
            // different absolute paths, so that a path which leaked into an
            // artifact leaks differently in the two rounds.
            let written = round_dir.join(&package_path).join(&artifact);
            let mut files: Vec<(String, String)> =
                vec![(artifact.clone(), compiled.module.clone())];
            let sheet = (!compiled.stylesheet.is_empty()).then(|| {
                (
                    actions::stylesheet_path(&written),
                    compiled.stylesheet.clone(),
                )
            });
            for (companion, text) in sheet
                .into_iter()
                // A `core/lazy` chunk is a file this build wrote, and the
                // module fetches it by name at run time — so it is as much
                // a part of "the artifact" as the stylesheet is.
                .chain(actions::chunk_paths(&written, &compiled.chunks))
            {
                let name = companion
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                files.push((name, text));
            }
            let mut staged: Vec<(String, Vec<u8>)> = Vec::new();
            for (name, text) in files {
                let at = written.with_file_name(&name);
                let read_back = at
                    .parent()
                    .map(std::fs::create_dir_all)
                    .unwrap_or(Ok(()))
                    .and_then(|()| std::fs::write(&at, &text))
                    .and_then(|()| std::fs::read(&at));
                match read_back {
                    Ok(b) => staged.push((name, b)),
                    Err(e) => {
                        eprintln!("error: cannot write {}: {e}", at.display());
                        return 2;
                    }
                }
            }
            *slot = staged;
            let _ = std::fs::remove_dir_all(&round_dir);
        }

        compared += 1;
        let [round_a, round_b] = &rounds;
        // A file present in one round and not the other is the same failure as
        // a byte that moved, and it is the one a companion artifact can newly
        // produce, so it is reported rather than skipped over.
        if round_a.len() != round_b.len() {
            differed = true;
            not_reproducible(
                &label,
                &format!(
                    "two builds of the same tree wrote {} files and {} files",
                    round_a.len(),
                    round_b.len()
                ),
            );
            continue;
        }
        let mut total = 0usize;
        let mut moved = false;
        for ((name_a, a), (name_b, b)) in round_a.iter().zip(round_b) {
            total = total.saturating_add(a.len());
            if name_a != name_b {
                moved = true;
                differed = true;
                not_reproducible(
                    &label,
                    &format!("two builds of the same tree wrote `{name_a}` and `{name_b}`"),
                );
                continue;
            }
            if let Some(at) = actions::first_difference(a, b) {
                moved = true;
                differed = true;
                // `path` names the module; a companion sits beside it, so the
                // report names the sibling rather than the one file the plan
                // happened to record.
                let sibling = match path.rsplit_once('/') {
                    Some((dir, _)) => format!("{dir}/{name_a}"),
                    None => name_a.clone(),
                };
                not_reproducible(
                    &label,
                    &format!(
                        "{sibling} differs between two builds of the same tree, first at byte \
                         {at}"
                    ),
                );
                eprintln!(
                    "  = an artifact that is not a function of its declared inputs cannot be \
                     cached safely; this is a toolchain bug, not a problem with your repository"
                );
            }
        }
        if !moved && args.flags.verbose {
            println!("{label} is reproducible ({total} bytes)");
        }
    }

    if differed {
        return 1;
    }
    if compared == 0 && args.flags.verbose {
        println!("nothing to build");
    }
    0
}

/// A round of `--check-reproducible` opens the repository for itself.
///
/// Each round has to start from a tree nobody has looked at yet — that is half
/// of what the check claims — so the session is per round rather than shared,
/// and the target is found again by label because the plan was resolved from a
/// session that has since been dropped.
fn round_session(
    label: &str,
    flags: &arguments::Flags,
) -> Result<(session::Session, crate::build::workspace::TargetId), i32> {
    let mut session = open_or_exit(flags).map_err(i32::from)?;
    if session.report() {
        return Err(2);
    }
    let Some(target) = session
        .workspace
        .targets()
        .into_iter()
        .find(|t| session.workspace.label(*t) == label && t.kind == RuleKind::Binary)
    else {
        eprintln!("error: {label} disappeared between two builds of one tree");
        return Err(2);
    };
    Ok((session, target))
}

/// The two lines every irreproducibility is reported as: the verdict, then
/// what moved.
///
/// One function because the verdict is one sentence. It was written out at
/// seven places, which is seven chances for the wording a user greps for to
/// stop being the same wording.
fn not_reproducible(label: &str, detail: &str) {
    eprintln!("error: {label} is not reproducible");
    eprintln!("  = {detail}");
}

/// One artifact `--check-reproducible` will build twice.
///
/// Plain data on purpose: the plan is resolved from its own session, which is
/// then dropped, so the two builds below share nothing but the repository on
/// disk.
struct Entry {
    label: String,
    /// What a failure names, repository-relative.
    path: String,
    package_path: std::path::PathBuf,
    artifact: String,
    platform: crate::build::buildfile::Platform,
    /// The declared output itself, because a native round needs the arch and
    /// the span as well as the platform, and re-deriving them from the
    /// platform would be inventing the arch the rule did not name.
    output: crate::build::buildfile::Output,
}

/// `--check-reproducible` for a native artifact.
///
/// ARCHITECTURE.md §7: **the objects are compared first, then the executable.**
/// A byte offset into a four-megabyte executable names nothing a person can act
/// on; per unit, the report is "`core_list.o` differs, first at byte 4192",
/// which names a module, and a module names a pass. The executable is compared
/// too, because a reproducible set of objects and an irreproducible link is a
/// real failure mode — link order, archive member ordering, a temporary path in
/// a debug section — and it is the one a per-object comparison would hide.
///
/// The two rounds link in two different directories for the same reason the
/// JavaScript rounds *write* in two: a path that leaked into a debug section
/// leaks differently, and that is the failure this design exists to catch.
///
/// `Ok(true)` means the artifact differed and has already been reported;
/// `Err(code)` means the check could not be run.
fn check_native(
    label: &str,
    path: &str,
    package_path: &std::path::Path,
    artifact: &str,
    output: &crate::build::buildfile::Output,
    flags: &arguments::Flags,
    args: &arguments::Args,
) -> Result<bool, i32> {
    use crate::build::link;
    /// One round's output: the objects, named, and the executable they linked
    /// into. Named rather than a tuple because the comparison below reads
    /// better than `rounds[0].1` does, and because the objects are the half
    /// that carries the useful report.
    #[derive(Default)]
    struct Round {
        units: Vec<(String, Vec<u8>)>,
        exe: Vec<u8>,
    }
    let mut rounds: [Round; 2] = Default::default();
    let mut chosen = None;
    for (slot, round) in rounds.iter_mut().zip(["a", "b"]) {
        let round_dir =
            std::env::temp_dir().join(format!("buri-reproducible-{}-{round}", std::process::id()));
        let _ = std::fs::remove_dir_all(&round_dir);
        let (mut session, target) = round_session(label, flags)?;
        let mut diagnostics = crate::diagnostics::Diagnostics::new();
        let objects =
            match actions::compile_objects(&mut session, target, output, flags, &mut diagnostics) {
                Ok(objects) => objects,
                Err(diagnostics) => {
                    session.print(&diagnostics);
                    return Err(1);
                }
            };
        let linker = match link::select(actions::target_of(output)) {
            Ok(l) => {
                chosen = l.chosen_outside();
                l.in_dir(round_dir.join("link"))
            }
            // `text()` and not the message alone: the hermetic-link refusal
            // carries the rule and the escape hatch in its notes, and this path
            // has no `Diagnostics` sink to render them through.
            Err(refusal) => {
                eprintln!("{}", refusal.text());
                return Err(1);
            }
        };
        let into = round_dir.join(package_path);
        let written = into.join(artifact);
        if let Err(e) = std::fs::create_dir_all(&into) {
            eprintln!("error: cannot write {}: {e}", written.display());
            return Err(2);
        }
        let prefix = session.workspace.package(target.package).path.clone();
        let opts = crate::compiler::backend::LinkOptions {
            profile: actions::profile_of(flags),
            target: actions::target_of(output),
            unit_prefix: &prefix,
        };
        if let Err(diagnostics) = link::run(&objects.units, &objects.rows, &linker, &written, &opts)
        {
            session.print(&diagnostics);
            return Err(1);
        }
        let exe = match std::fs::read(&written) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("error: the link produced no {}: {e}", written.display());
                return Err(2);
            }
        };
        let units = objects
            .units
            .into_iter()
            .map(|u| (u.name, u.bytes))
            .collect();
        *slot = Round { units, exe };
        let _ = std::fs::remove_dir_all(&round_dir);
    }

    let [a, b] = &rounds;
    let (a_units, a_exe) = (&a.units, &a.exe);
    let (b_units, b_exe) = (&b.units, &b.exe);
    let mut differed = false;
    if a_units.len() != b_units.len() {
        differed = true;
        not_reproducible(
            label,
            &format!(
                "two builds of the same tree produced {} and {} codegen units",
                a_units.len(),
                b_units.len()
            ),
        );
    }
    for ((name, a), (other, b)) in a_units.iter().zip(b_units) {
        if name != other {
            differed = true;
            not_reproducible(
                label,
                &format!("two builds of the same tree named unit `{name}` and unit `{other}`"),
            );
            continue;
        }
        if let Some(at) = actions::first_difference(a, b) {
            differed = true;
            not_reproducible(
                label,
                &format!("{name} differs between two builds of the same tree, first at byte {at}"),
            );
        }
    }
    // The objects are this toolchain's own; the link may not be.
    let units_differed = differed;
    match actions::first_difference(a_exe, b_exe) {
        Some(at) => {
            differed = true;
            not_reproducible(
                label,
                &format!("{path} differs between two builds of the same tree, first at byte {at}"),
            );
            eprintln!(
                "  = the objects above say whether this is a codegen difference or a link one"
            );
        }
        None if !differed && args.flags.verbose => {
            println!(
                "{label} is reproducible ({} bytes, {} objects)",
                a_exe.len(),
                a_units.len()
            );
        }
        None => {}
    }
    match chosen.filter(|_| !units_differed) {
        Some(linker) if differed => eprintln!(
            "  = the objects are the same in both builds, so the difference may come from the \
             linker, {linker}"
        ),
        _ if differed => eprintln!(
            "  = an artifact that is not a function of its declared inputs cannot be cached \
             safely; this is a toolchain bug, not a problem with your repository"
        ),
        _ => {}
    }
    Ok(differed)
}

type Plan = Vec<Entry>;

fn plan(args: &arguments::Args, flags: &arguments::Flags) -> Result<Plan, i32> {
    let session = open_or_exit(flags).map_err(i32::from)?;
    let targets = match session.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            return Err(2);
        }
    };
    let mut plan = Plan::new();
    for target in targets {
        if target.kind != RuleKind::Binary {
            continue;
        }
        for output in actions::selected_outputs(&session, target, flags) {
            // The rounds compile without the policy checks a build makes first,
            // so a target the build refuses is refused here, not called
            // reproducible.
            let mut refused = crate::diagnostics::Diagnostics::new();
            actions::check_policy(&session, target, &output.output_platform(), &mut refused);
            if session.print(&refused) {
                return Err(1);
            }
            let full = actions::artifact_path(&session, target, &output);
            let reported = full
                .strip_prefix(&session.root)
                .unwrap_or(&full)
                .display()
                .to_string();
            let name = full
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "artifact".into());
            plan.push(Entry {
                label: session.workspace.label(target),
                path: reported,
                package_path: std::path::PathBuf::from(
                    &session.workspace.package(target.package).path,
                ),
                artifact: name,
                platform: output.platform(),
                output,
            });
        }
    }
    Ok(plan)
}
