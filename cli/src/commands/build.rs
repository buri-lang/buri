//! `buri build`.
//!
//! A binary is built; a library is checked. `buri build //lib/money` produces
//! no artifact and is not meant to — the library's outputs are whatever
//! depends on it — so what it reports is whether the code and the policy
//! around it hold.

use crate::build::actions;
use crate::build::session::{self, open_or_exit};
use crate::build::workspace::RuleKind;
use crate::commands::arguments;

pub fn cmd_build(args: &arguments::Args) -> i32 {
    if args.flags.check_reproducible {
        return check_reproducible(args);
    }
    let mut s = match open_or_exit(&args.flags) {
        Ok(s) => s,
        Err(c) => return c as i32,
    };
    // An unparseable build file is a problem with the invocation, not with the
    // code, so it exits 2.
    if s.report() {
        return 2;
    }
    let targets = match s.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
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
        if !matched && !declared.is_empty() {
            let names: Vec<&str> = declared.iter().map(String::as_str).collect();
            eprintln!("error: no declared output matches `--output={sel}`");
            eprintln!("  = declared: {}", names.join(", "));
            eprintln!(
                "  = fix: name one of them, as in `--output={}`, or add the output to the rule",
                names[0]
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
            actions::check_policy(&s, target, crate::build::buildfile::Platform::Js, &mut diags);
            if !diags.has_errors() {
                let unit = crate::compiler::modules::Unit {
                    target: Some(target),
                    platform: crate::build::buildfile::Platform::Js,
                    with_tests: false,
                };
                let analysis = crate::compiler::driver::analyze(Some(&s.ws), &mut s.map, &unit);
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
    flags.release = args.flags.release;
    flags.debug = args.flags.debug;
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
        let Entry { label, path, pkg_path, artifact, platform } = entry;
        if platform != crate::build::buildfile::Platform::Js {
            eprintln!("error: the {} backend is not implemented", platform.slug());
            eprintln!("  = this toolchain emits JavaScript; check `--output=js`");
            return 1;
        }
        let mut bytes: Vec<Vec<u8>> = Vec::new();
        for round in ["a", "b"] {
            // Two directories, thrown away as soon as the bytes are read back.
            // Outside the repository, because a check must not double as a
            // build and must not leave a directory in a tree it was only asked
            // a question about; named for this process, so two checks running
            // at once do not compare each other's artifacts.
            let round_dir = std::env::temp_dir()
                .join(format!("buri-reproducible-{}-{round}", std::process::id()));
            let _ = std::fs::remove_dir_all(&round_dir);
            let mut s = match session::open(&flags) {
                Ok(s) => s,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    return 2;
                }
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
            let source = match actions::compile_artifact(&mut s, target, platform, &flags, &mut diags)
            {
                Ok(source) => source,
                Err(diags) => {
                    s.print(&diags);
                    return 1;
                }
            };
            // Written and read back rather than compared in memory, under two
            // different absolute paths, so that a path which leaked into an
            // artifact leaks differently in the two rounds.
            let written = round_dir.join(&pkg_path).join(&artifact);
            let staged = written
                .parent()
                .map(|p| std::fs::create_dir_all(p))
                .unwrap_or(Ok(()))
                .and_then(|()| std::fs::write(&written, source.as_bytes()))
                .and_then(|()| std::fs::read(&written));
            match staged {
                Ok(read_back) => bytes.push(read_back),
                Err(e) => {
                    eprintln!("error: cannot write {}: {e}", written.display());
                    return 2;
                }
            }
            let _ = std::fs::remove_dir_all(&round_dir);
        }

        compared += 1;
        match actions::first_difference(&bytes[0], &bytes[1]) {
            None => {
                if args.flags.verbose {
                    println!("{label} is reproducible ({} bytes)", bytes[0].len());
                }
            }
            Some(at) => {
                differed = true;
                eprintln!("error: {label} is not reproducible");
                eprintln!(
                    "  = {} differs between two builds of the same tree, first at byte {at}",
                    path
                );
                eprintln!(
                    "  = an artifact that is not a function of its declared inputs cannot be \
                     cached safely; this is a toolchain bug, not a problem with your repository"
                );
            }
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
}

type Plan = Vec<Entry>;

fn plan(args: &arguments::Args, flags: &arguments::Flags) -> Result<Plan, i32> {
    let s = match session::open(flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return Err(2);
        }
    };
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
                platform: output
                    .platform
                    .as_ref()
                    .map(|p| p.value)
                    .unwrap_or(crate::build::buildfile::Platform::Js),
            });
        }
    }
    Ok(plan)
}
