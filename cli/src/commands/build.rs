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
use crate::build::session::{self, open_or_exit};
use crate::build::workspace::RuleKind;
use crate::commands::arguments;

pub fn cmd_build(args: &arguments::Args) -> i32 {
    if args.flags.check_reproducible {
        return check_reproducible(args);
    }
    let (mut s, targets) = match session::open_and_resolve(&args.flags, &args.targets) {
        Ok(both) => both,
        Err(c) => return c as i32,
    };

    // `--output` selects one of the outputs a target declares. A selector that
    // matches none of them is the thing you asked *with* being wrong, and has
    // to say so: the build would otherwise report success having produced
    // nothing, which is the one answer a build system must never give.
    if let Some(sel) = &args.flags.output {
        let mut declared: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut matched = false;
        for t in &targets {
            if t.kind != RuleKind::Binary {
                continue;
            }
            for o in actions::selected_outputs(&s, *t, &arguments::Flags::default()) {
                declared.insert(o.dir());
                matched |= o.matches_selector(sel);
            }
        }
        // A target that declares no outputs at all has nothing for the fix to
        // name, and nothing the selector could have matched, so it is not this
        // mistake.
        let example = if matched { None } else { declared.first() };
        if let Some(example) = example {
            let names: Vec<&str> = declared.iter().map(String::as_str).collect();
            eprintln!("error: no declared output matches `--output={sel}`");
            eprintln!("  = declared: {}", names.join(", "));
            eprintln!(
                "  = fix: name one of them, as in `--output={example}`, or add the output to \
                 the rule"
            );
            return 2;
        }
    }

    let mut built = 0;
    let mut failed = false;
    for target in targets {
        // Only a binary produces an artifact; a library is checked, which is
        // what `buri build //lib/money` means.
        if target.kind == RuleKind::Library {
            let mut diags = crate::diagnostics::Diagnostics::new();
            // A library declares no outputs, so there is no platform this is
            // *for*; `Js` is the toolchain's default and is what this asked
            // before there was a fourth platform to pick wrongly. The only
            // thing it decides is a tag's `requires { platforms }`, which a
            // library is asked about again — per output — by every binary that
            // depends on it.
            actions::check_policy(&s, target, crate::build::buildfile::Platform::Js, &mut diags);
            if !diags.has_errors() {
                let unit = crate::compiler::modules::Unit {
                    target: Some(target),
                    // A library is checked, not built for an output, and it
                    // cannot import `core/host` at all. See `Unit::platform`.
                    platform: None,
                    with_tests: false,
                };
                let analysis = crate::compiler::driver::analyze(Some(&s.ws), &mut s.map, &mut s.parsed, &unit);
                diags.extend(analysis.diags.items);
            }
            failed |= s.print(&diags);
            continue;
        }
        for output in actions::selected_outputs(&s, target, &args.flags) {
            match actions::build_target(&mut s, target, &output, &args.flags) {
                Ok(a) => {
                    built += 1;
                    let rel = a.path.strip_prefix(&s.root).unwrap_or(&a.path);
                    let note = if a.cached { ", cached" } else { "" };
                    println!("{} ({} bytes{note})", rel.display(), a.bytes);
                }
                Err(diags) => {
                    failed |= s.print(&diags);
                }
            }
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
    let mut flags = arguments::Flags { force: true, ..arguments::Flags::default() };
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
        let Entry { label, path, pkg_path, artifact, platform, output } = entry;
        if platform.is_native() {
            if !actions::native_ready(actions::target_of(&output), actions::profile_of(&flags)) {
                eprintln!("error: the {} backend is not implemented", platform.slug());
                eprintln!("  = this toolchain emits JavaScript; check `--output=js`");
                return 1;
            }
            match check_native(&label, &path, &pkg_path, &artifact, &output, &flags, args) {
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
        let mut rounds: Vec<Vec<(String, Vec<u8>)>> = Vec::new();
        for round in ["a", "b"] {
            // Two directories, thrown away as soon as the bytes are read back.
            // Outside the repository, because a check must not double as a
            // build and must not leave a directory in a tree it was only asked
            // a question about; named for this process, so two checks running
            // at once do not compare each other's artifacts.
            let round_dir = std::env::temp_dir()
                .join(format!("buri-reproducible-{}-{round}", std::process::id()));
            let _ = std::fs::remove_dir_all(&round_dir);
            let mut s = match open_or_exit(&flags) {
                Ok(s) => s,
                Err(c) => return c as i32,
            };
            if s.report() {
                return 2;
            }
            let Some(target) = s.ws.targets().into_iter().find(|t| s.ws.label(*t) == label && t.kind == RuleKind::Binary)
            else {
                eprintln!("error: {label} disappeared between two builds of one tree");
                return 2;
            };
            let mut diags = crate::diagnostics::Diagnostics::new();
            let compiled = match actions::compile_artifact(&mut s, target, platform, &flags, &mut diags)
            {
                Ok(compiled) => compiled,
                Err(diags) => {
                    s.print(&diags);
                    return 1;
                }
            };
            // Written and read back rather than compared in memory, under two
            // different absolute paths, so that a path which leaked into an
            // artifact leaks differently in the two rounds.
            let written = round_dir.join(&pkg_path).join(&artifact);
            let mut files: Vec<(String, String)> =
                vec![(artifact.clone(), compiled.module.clone())];
            for (companion, text) in
                actions::web_companions(&written, &output, &compiled.stylesheet)
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
            rounds.push(staged);
            let _ = std::fs::remove_dir_all(&round_dir);
        }

        compared += 1;
        // The loop above either pushes one round's files or returns, so there
        // are exactly the two rounds' worth to compare.
        let [round_a, round_b] = rounds.as_slice() else {
            crate::ice!("each round of --check-reproducible pushes exactly one artifact set")
        };
        // A file present in one round and not the other is the same failure as
        // a byte that moved, and it is the one a companion artifact can newly
        // produce, so it is reported rather than skipped over.
        if round_a.len() != round_b.len() {
            differed = true;
            eprintln!("error: {label} is not reproducible");
            eprintln!(
                "  = two builds of the same tree wrote {} files and {} files",
                round_a.len(),
                round_b.len()
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
                eprintln!("error: {label} is not reproducible");
                eprintln!(
                    "  = two builds of the same tree wrote `{name_a}` and `{name_b}`"
                );
                continue;
            }
            if let Some(at) = actions::first_difference(a, b) {
                moved = true;
                differed = true;
                eprintln!("error: {label} is not reproducible");
                // `path` names the module; a companion sits beside it, so the
                // report names the sibling rather than the one file the plan
                // happened to record.
                let sibling = match path.rsplit_once('/') {
                    Some((dir, _)) => format!("{dir}/{name_a}"),
                    None => name_a.clone(),
                };
                eprintln!(
                    "  = {sibling} differs between two builds of the same tree, first at \
                     byte {at}"
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

/// One artifact `--check-reproducible` will build twice.
///
/// Plain data on purpose: the plan is resolved from its own session, which is
/// then dropped, so the two builds below share nothing but the repository on
/// disk.
struct Entry {
    label: String,
    /// What a failure names, repository-relative.
    path: String,
    pkg_path: std::path::PathBuf,
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
    pkg_path: &std::path::Path,
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
    struct Round {
        units: Vec<(String, Vec<u8>)>,
        exe: Vec<u8>,
    }
    let mut rounds: Vec<Round> = Vec::new();
    for round in ["a", "b"] {
        let round_dir =
            std::env::temp_dir().join(format!("buri-reproducible-{}-{round}", std::process::id()));
        let _ = std::fs::remove_dir_all(&round_dir);
        let mut s = open_or_exit(flags).map_err(i32::from)?;
        if s.report() {
            return Err(2);
        }
        let Some(target) = s
            .ws
            .targets()
            .into_iter()
            .find(|t| s.ws.label(*t) == label && t.kind == RuleKind::Binary)
        else {
            eprintln!("error: {label} disappeared between two builds of one tree");
            return Err(2);
        };
        let mut diags = crate::diagnostics::Diagnostics::new();
        let objects = match actions::compile_objects(&mut s, target, output, flags, &mut diags) {
            Ok(objects) => objects,
            Err(diags) => {
                s.print(&diags);
                return Err(1);
            }
        };
        let linker = match link::select(actions::target_of(output)) {
            Ok(l) => l.in_dir(round_dir.join("link")),
            Err(message) => {
                eprintln!("error: {message}");
                return Err(1);
            }
        };
        let written = round_dir.join(pkg_path).join(artifact);
        if let Some(parent) = written.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("error: cannot write {}: {e}", written.display());
                return Err(2);
            }
        }
        let prefix = s.ws.pkg(target.pkg).path.clone();
        let opts = crate::compiler::backend::LinkOptions {
            profile: actions::profile_of(flags),
            target: actions::target_of(output),
            unit_prefix: &prefix,
        };
        if let Err(diags) = link::run(&objects.units, &objects.rows, &linker, &written, &opts) {
            s.print(&diags);
            return Err(1);
        }
        let exe = match std::fs::read(&written) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("error: the link produced no {}: {e}", written.display());
                return Err(2);
            }
        };
        let units = objects.units.into_iter().map(|u| (u.name, u.bytes)).collect();
        rounds.push(Round { units, exe });
        let _ = std::fs::remove_dir_all(&round_dir);
    }

    let [a, b] = rounds.as_slice() else {
        crate::ice!("each round of --check-reproducible pushes exactly one artifact")
    };
    let (a_units, a_exe) = (&a.units, &a.exe);
    let (b_units, b_exe) = (&b.units, &b.exe);
    let mut differed = false;
    if a_units.len() != b_units.len() {
        differed = true;
        eprintln!("error: {label} is not reproducible");
        eprintln!(
            "  = two builds of the same tree produced {} and {} codegen units",
            a_units.len(),
            b_units.len()
        );
    }
    for ((name, a), (other, b)) in a_units.iter().zip(b_units) {
        if name != other {
            differed = true;
            eprintln!("error: {label} is not reproducible");
            eprintln!("  = two builds of the same tree named unit `{name}` and unit `{other}`");
            continue;
        }
        if let Some(at) = actions::first_difference(a, b) {
            differed = true;
            eprintln!("error: {label} is not reproducible");
            eprintln!(
                "  = {name} differs between two builds of the same tree, first at byte {at}"
            );
        }
    }
    match actions::first_difference(a_exe, b_exe) {
        Some(at) => {
            differed = true;
            eprintln!("error: {label} is not reproducible");
            eprintln!(
                "  = {path} differs between two builds of the same tree, first at byte {at}"
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
    if differed {
        eprintln!(
            "  = an artifact that is not a function of its declared inputs cannot be cached \
             safely; this is a toolchain bug, not a problem with your repository"
        );
    }
    Ok(differed)
}

type Plan = Vec<Entry>;

fn plan(args: &arguments::Args, flags: &arguments::Flags) -> Result<Plan, i32> {
    let s = open_or_exit(flags).map_err(i32::from)?;
    let targets = match s.resolve_targets(&args.targets) {
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
        for output in actions::selected_outputs(&s, target, flags) {
            let full = actions::artifact_path(&s, target, &output);
            let reported = full.strip_prefix(&s.root).unwrap_or(&full).display().to_string();
            let name = full
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "artifact".into());
            plan.push(Entry {
                label: s.ws.label(target),
                path: reported,
                pkg_path: std::path::PathBuf::from(&s.ws.pkg(target.pkg).path),
                artifact: name,
                platform: output.platform(),
                output,
            });
        }
    }
    Ok(plan)
}
