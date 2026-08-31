//! `buri lint`.
//!
//! Checks that type checking does not cover. One catalogue, every check always
//! on, every finding a warning — a lint that cannot be turned off has to be one
//! nobody wants to turn off, which is the bar every check here is held to.
//!
//! A repository may only tighten. The `lint` block in `REPO.buri` can run the
//! catalogue during `buri build` and `buri test` (`check_during_build`) and can
//! make a finding fail the command that reported it (`fail_on_finding`). There
//! is no per-file suppression comment and no way to silence a check: the only
//! answers a repository is offered are "sooner" and "harder".
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "what `--fix` rewrote, and what it declined to rewrite, is this \
              command's output; every finding itself still leaves through \
              `Session::emit`"
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here counts findings and files that already exist, \
              and walks forward through a string that bounds the offset"
)]

use crate::build::session::{self, Session};
use crate::build::workspace::{PackageId, RuleKind, TargetId};
use crate::commands::arguments;
use crate::compiler::modules::{ModuleData, Unit};
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::{FnId, ModuleId};
use crate::diagnostics::{Diagnostic, Diagnostics, Invariant as _, Span};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Checks that type checking does not cover. Every check runs, every finding is
/// a warning, and nothing silences one; `REPO.buri`'s `lint` block may only ask
/// for them sooner or harder.
pub fn command_lint(args: &arguments::Args) -> i32 {
    let (mut session, diagnostics) = match collect_findings(args) {
        Ok(v) => v,
        Err(code) => return code,
    };

    if args.flags.fix {
        let applied = apply_fixes(&mut session, &diagnostics);
        if applied > 0 {
            println!("fixed {applied} finding{}", if applied == 1 { "" } else { "s" });
            // Everything is computed again from the files on disk rather than
            // subtracted from what was just reported: an edit can uncover a
            // finding the first pass could not see, and a count arrived at by
            // arithmetic is one nobody can check.
            let (mut session, diagnostics) = match collect_findings(args) {
                Ok(v) => v,
                Err(code) => return code,
            };
            return report_findings(&mut session, &diagnostics);
        }
        println!("nothing to fix");
    }

    report_findings(&mut session, &diagnostics)
}

/// Opens the repository and runs every check, in the order a reader would.
fn collect_findings(args: &arguments::Args) -> Result<(Session, Diagnostics), i32> {
    let (mut session, targets) =
        session::open_and_resolve(&args.flags, &args.targets).map_err(i32::from)?;

    let diagnostics = findings_for(&mut session, &targets, &args.flags);
    Ok((session, diagnostics))
}

/// Every check `buri lint` runs, over the targets given.
///
/// Public because the language server reports the same findings the command
/// does — an editor that showed only type errors would be showing half of what
/// the toolchain knows, and the half that is easier to notice at the terminal.
pub fn findings_for(
    session: &mut Session,
    targets: &[TargetId],
    flags: &arguments::Flags,
) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    let mut seen_packages = BTreeSet::new();
    let mut store = super::lint_cache::Store::open(&session.root, flags);
    for target in targets {
        // The package rules are asked once per package, so which of a record's
        // three lists is replayed depends on what the loop has already said.
        let first_in_package = !seen_packages.contains(&target.package);
        if let Some(parts) = store.recall(session, *target, first_in_package) {
            seen_packages.insert(target.package);
            replay(&parts, first_in_package, &mut diagnostics);
            continue;
        }
        let analysis = analysis_of(session, *target);
        let marks = one_target(session, *target, &analysis, &mut seen_packages, &mut diagnostics);
        store.remember(session, *target, &analysis, &marks.parts(&diagnostics));
    }
    check_cycles(session, &mut diagnostics);

    promote(session, &mut diagnostics);
    diagnostics.sort(&session.map);
    diagnostics
}

/// A recalled target's findings, put back in the order they were found in.
///
/// Through `push` rather than by concatenation, so that the deduplication a
/// cold run applied — one error in a shared module, reported by every target
/// whose closure holds it — applies here to the same effect.
fn replay(
    parts: &super::lint_cache::Parts,
    first_in_package: bool,
    diagnostics: &mut Diagnostics,
) {
    diagnostics.extend(parts.analysis.iter().cloned());
    if first_in_package {
        diagnostics.extend(parts.package.iter().cloned());
    }
    diagnostics.extend(parts.target.iter().cloned());
}

/// The same rules over one target, with its analysis already in hand.
///
/// The dependency and hygiene rules are read off a `driver::analyze` of the
/// target's closure, and a caller that has just run one — the language server
/// has, for the diagnostics it reports from the same pass — would otherwise pay
/// for the closure a second time.
///
/// The batch form above is not written in terms of this one, and that is the
/// point of having both: the package rules are asked once per *package*, so a
/// loop over targets would ask twice for a package holding a library and a
/// binary, and `buri lint` would print each of their findings twice.
pub fn findings_for_target(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    let mut seen_packages = BTreeSet::new();
    let _ = one_target(session, target, analysis, &mut seen_packages, &mut diagnostics);
    check_cycles(session, &mut diagnostics);

    promote(session, &mut diagnostics);
    diagnostics.sort(&session.map);
    diagnostics
}

/// The whole front end over one target's closure, as the rules below ask about
/// it: no output, and the tests included.
pub fn analysis_of(
    session: &mut Session,
    target: TargetId,
) -> crate::compiler::driver::Analysis {
    // A lint is not a build, so it does not refuse a program for an output it
    // was not asked about. See `Unit::platform`.
    let unit = Unit { target: Some(target), platform: None, with_tests: true };
    crate::compiler::driver::analyze(
        Some(&session.workspace),
        &mut session.map,
        &mut session.parsed,
        &unit,
    )
}

/// Where in the report one target's three kinds of finding ended up.
///
/// Positions rather than three sinks of their own, because the report
/// deduplicates: an error in a module two targets share is reported once, and
/// slicing what one pass actually added is the only way to write down the same
/// thing it added.
struct Marks {
    start: usize,
    analysis_end: usize,
    package_end: usize,
    /// Whether this pass asked the package rules, which is what tells an empty
    /// package list "nothing to report" from "somebody else reported it".
    asked_the_package: bool,
}

impl Marks {
    fn parts(&self, diagnostics: &Diagnostics) -> super::lint_cache::Parts {
        let cut = |from: usize, to: usize| {
            diagnostics.items.get(from..to).unwrap_or_default().to_vec()
        };
        super::lint_cache::Parts {
            analysis: cut(self.start, self.analysis_end),
            package: cut(self.analysis_end, self.package_end),
            target: cut(self.package_end, diagnostics.items.len()),
            asked_the_package: self.asked_the_package,
        }
    }
}

/// Every rule that is about one target, in the order they are asked.
fn one_target(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    seen_packages: &mut BTreeSet<crate::build::workspace::PackageId>,
    diagnostics: &mut Diagnostics,
) -> Marks {
    let start = diagnostics.items.len();
    // What the analysis found is part of the report, and it never displaces the
    // rest of it: a target that does not type check is still a target with an
    // import nothing uses, and the two are found by different questions.
    diagnostics.extend(analysis.diagnostics.items.clone());
    let analysis_end = diagnostics.items.len();
    let asked_the_package = seen_packages.insert(target.package);
    if asked_the_package {
        check_sources_declared(session, target.package, diagnostics);
        check_test_suites(session, target.package, diagnostics);
    }
    let package_end = diagnostics.items.len();
    // `lint` is not building, so it checks a binary against the platforms
    // its own `outputs` name, and a library against the question TAGS.md
    // asks at the target: can it be built at all?
    crate::build::actions::check_visibility(session, target, diagnostics);
    crate::build::actions::check_tags(session, target, diagnostics);
    check_target_platforms(session, target, diagnostics);
    check_dependencies(session, target, analysis, diagnostics);
    Marks { start, analysis_end, package_end, asked_the_package }
}

/// `fail_on_finding`: every finding from the catalogue becomes an error.
///
/// It happens here rather than at each caller so that the editor squiggles the
/// same colour the terminal prints, and only for codes the catalogue names —
/// the analysis errors that ride along in the same `Diagnostics` are already
/// errors and are nobody's to promote.
fn promote(session: &Session, diagnostics: &mut Diagnostics) {
    if !session.workspace.repo.lint.fail_on_finding {
        return;
    }
    for d in &mut diagnostics.items {
        let code = d.code.as_deref();
        let known = code.is_some_and(|c| crate::documentation::lints::find(c).is_some());
        if known && d.severity == crate::diagnostics::Severity::Warning {
            d.severity = crate::diagnostics::Severity::Error;
        }
    }
}

/// Prints every finding, and answers with the exit code.
///
/// Any finding at all is a nonzero exit. Severity does not gate it: running the
/// linter is itself the request to be told, and a report that exits zero is one
/// no script can act on. Whether a finding blocks `buri build` or `buri test` is
/// a different question, and the repository answers it in `REPO.buri`'s `lint`
/// block rather than here.
fn report_findings(session: &mut Session, diagnostics: &Diagnostics) -> i32 {
    for d in &diagnostics.items {
        session.emit(d);
    }
    if diagnostics.items.is_empty() {
        println!("no findings");
        return 0;
    }
    1
}

/// The three findings whose answer is a build file that describes the code.
///
/// These are never byte-edited. `buri gen` already writes exactly this file,
/// preserving `tags`, `visibility`, `outputs` and comments, so calling it is
/// the only way `lint --fix` and `gen` cannot end up disagreeing about what a
/// package's `BUILD.buri` should say.
const REGENERABLE: &[&str] = &["missing-dep", "unused-library", "duplicate-source"];

fn regenerate_build_files(session: &mut Session, diagnostics: &Diagnostics) -> usize {
    // Which package a finding is about. `missing-dep` points at the import in
    // a source file, not at the build file, so this is by path prefix — the
    // longest package path the file sits under.
    let mut packages: BTreeSet<PackageId> = BTreeSet::new();
    for d in &diagnostics.items {
        if !d.code.as_deref().is_some_and(|c| REGENERABLE.contains(&c)) {
            continue;
        }
        let name = session.map.name(d.span.file).to_string();
        let mut best: Option<(usize, PackageId)> = None;
        for t in session.workspace.targets() {
            let p = session.workspace.package(t.package);
            let owns = p.build_file_id == d.span.file
                || name.strip_prefix(&p.path).is_some_and(|r| r.starts_with('/'));
            if owns && best.is_none_or(|(len, _)| p.path.len() > len) {
                best = Some((p.path.len(), t.package));
            }
        }
        if let Some((_, package)) = best {
            packages.insert(package);
        }
    }

    let mut fixed = 0;
    for package in packages {
        match crate::build::regenerate::regenerate(session, package) {
            Ok(Some(update)) => {
                let path = session.workspace.package(package).build_path.clone();
                if let Err(err) = std::fs::write(&path, &update.text) {
                    eprintln!("error: writing {}: {err}", path.display());
                    continue;
                }
                println!("updated {}/BUILD.buri", session.workspace.package(package).path);
                for line in &update.summary {
                    println!("  {line}");
                }
                fixed += 1;
            }
            Ok(None) => {}
            Err(d) => session.emit(&d),
        }
    }
    fixed
}

/// Applies every finding that carries a byte edit, and returns how many
/// **findings** were answered.
///
/// Findings and not edits, which used to be the same number and is not any
/// more: `unused-context` deletes a parameter and the argument at every call
/// site, so one finding is one edit plus one per caller. Counting the edits
/// made `--fix` announce more findings than the report above it had listed.
///
/// Per file, descending by offset so earlier edits keep their offsets, and
/// **refusing the whole file** on any overlap rather than guessing which of two
/// answers was meant. The result is run through `formatting::source`, which returns
/// `None` for anything that does not parse — the same guard the formatter uses
/// on itself — and a file that fails it is left exactly as it was.
fn apply_fixes(session: &mut Session, diagnostics: &Diagnostics) -> usize {
    use std::collections::BTreeMap;
    // The finding each edit came from, so that a file written out can say which
    // findings it answered rather than how many byte ranges it moved.
    let mut by_file: BTreeMap<u32, (Vec<crate::diagnostics::Edit>, BTreeSet<usize>)> =
        BTreeMap::new();
    for (i, d) in diagnostics.items.iter().enumerate() {
        for e in &d.edits {
            let row = by_file.entry(e.at.file.0).or_default();
            // Two findings answered by one edit is one edit, not an overlap.
            // `unused-context-bound` is where that happens: a bound list is one
            // piece of text with shared separators, so removing two of its
            // elements is a single rewrite, and each of the findings about
            // that parameter carries it. Applying it once answers both, and
            // the count below says two because two findings went away.
            if !row.0.iter().any(|had| had.at == e.at && had.replacement == e.replacement) {
                row.0.push(e.clone());
            }
            row.1.insert(i);
        }
    }

    let mut answered: BTreeSet<usize> = BTreeSet::new();
    let applied = regenerate_build_files(session, diagnostics);
    for (file, (edits, findings)) in by_file {
        let id = crate::diagnostics::FileId(file);
        // Sorting, overlap, bounds and character boundaries are all settled
        // here, once, so that the application below cannot fail.
        let edits = match crate::diagnostics::EditSet::new(id, session.map.text(id), edits) {
            Ok(set) => set,
            Err(why) => {
                eprintln!("warning: {} has {why}, so none were applied", session.map.name(id));
                continue;
            }
        };

        let text = edits.apply(session.map.text(id));
        // Parsed, not formatted. The guard a rewriting tool owes its user is
        // "what I wrote is still a program", and that is what parsing answers.
        // Running the result through the formatter would answer it too, and
        // would also reformat everything the fix did not touch — which turns
        // one deliberate edit into a diff nobody asked for.
        if !crate::parsing::parser::parse(&text, id).errors.is_empty() {
            eprintln!(
                "warning: fixing {} would not parse, so it was left alone",
                session.map.name(id)
            );
            continue;
        }
        let path = session.map.get(id).abs_path.clone();
        if let Err(err) = std::fs::write(&path, &text) {
            eprintln!("error: writing {}: {err}", path.display());
            continue;
        }
        answered.extend(findings);
    }
    applied + answered.len()
}

/// `unsatisfiable-target`. A target whose closure admits no platform at all is
/// reported at the target itself, before any binary asks for it: otherwise the
/// mistake surfaces as a confusing failure in whichever binary happens to reach
/// it first. A target that merely refuses one platform is `platform-violation`,
/// reported by `actions::check_platform` below.
fn check_target_platforms(session: &Session, target: TargetId, diagnostics: &mut Diagnostics) {
    let allowed = session.workspace.platforms(target);
    if allowed.is_empty() {
        let label = session.workspace.label(target);
        let span = session
            .workspace
            .tags(target)
            .first()
            .map(|t| t.span)
            .unwrap_or(Span::point(session.workspace.package(target.package).build_file_id, 0));
        diagnostics
            .push(Diagnostic::templated("unsatisfiable-target", span).with_bind("target", label));
        return;
    }
    if target.kind != RuleKind::Binary {
        return;
    }
    let Some(bin) = &session.workspace.package(target.package).build.binary else { return };
    for output in &bin.outputs {
        let p = output.platform();
        if !allowed.contains(&p) {
            crate::build::actions::check_platform(session, target, p, diagnostics);
        }
    }
}

/// `empty-test-suite`. A `test` block that declares no sources is a claim that
/// something is tested, backed by nothing — and it reads as coverage in every
/// tool that walks the build graph. Writing the block is the deliberate act, so
/// the empty one is a leftover rather than a decision.
fn check_test_suites(session: &Session, package: PackageId, diagnostics: &mut Diagnostics) {
    let p = session.workspace.package(package);
    let mut report = |suite: Option<&crate::build::buildfile::TestSuite>, rule: &str| {
        let Some(suite) = suite else { return };
        if !suite.sources.is_empty() {
            return;
        }
        diagnostics.push(
            Diagnostic::templated("empty-test-suite", suite.span).with_bind("rule", rule),
        );
    };
    if let Some(l) = &p.build.library {
        report(l.test.as_ref(), "library");
    }
    if let Some(b) = &p.build.binary {
        report(b.test.as_ref(), "binary");
    }
}

/// `unused-library` and `duplicate-source`: every `.buri` file in a package
/// must appear in exactly one rule. A file that appears in none belongs to no
/// library and no binary, so nothing ever builds it.
fn check_sources_declared(session: &Session, package: PackageId, diagnostics: &mut Diagnostics) {
    let p = session.workspace.package(package);
    let mut declared: Vec<(String, Span)> = Vec::new();
    let push = |list: &[crate::build::buildfile::Spanned<String>], out: &mut Vec<(String, Span)>| {
        for x in list {
            out.push((x.value.clone(), x.span));
        }
    };
    if let Some(lib) = &p.build.library {
        push(&lib.sources, &mut declared);
        push(&lib.proto_sources, &mut declared);
        if let Some(t) = &lib.test {
            push(&t.sources, &mut declared);
        }
        if let Some(t) = &lib.testing {
            push(&t.sources, &mut declared);
        }
    }
    if let Some(bin) = &p.build.binary {
        push(&bin.sources, &mut declared);
        push(&bin.proto_sources, &mut declared);
        if let Some(t) = &bin.test {
            push(&t.sources, &mut declared);
        }
    }

    for (i, (first_name, first_span)) in declared.iter().enumerate() {
        for (name, span) in declared.iter().skip(i + 1) {
            if first_name == name {
                diagnostics.push(
                    Diagnostic::templated("duplicate-source", *span)
                        .with_bind("source", name.as_str())
                        .with_secondary_span(*first_span, "first listed here"),
                );
            }
        }
    }

    // The entry points are named by the rule kind rather than listed.
    let mut known: BTreeSet<String> = declared.iter().map(|(n, _)| n.clone()).collect();
    known.insert("lib.buri".into());
    known.insert("main.buri".into());
    known.insert("testing/lib.buri".into());

    let mut on_disk = Vec::new();
    collect_package_sources(&p.dir, &p.dir, &mut on_disk);
    for rel in on_disk {
        if known.contains(&rel) {
            continue;
        }
        // A `.proto` is declared in `proto_sources` rather than `sources`, and
        // the fix has to say which — the rule is the same rule ("everything is
        // declared"), so the code is the same code.
        let field = if rel.ends_with(".proto") { "proto_sources" } else { "sources" };
        diagnostics.push(
            Diagnostic::templated("unused-library", Span::point(p.build_file_id, 0))
                .with_bind("package_path", p.path.as_str())
                .with_bind("source", rel)
                .with_bind("field", field),
        );
    }
}

fn collect_package_sources(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    items.sort();
    for p in items {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            // A subdirectory with its own BUILD.buri is a different package.
            if p.join("BUILD.buri").is_file() {
                continue;
            }
            collect_package_sources(root, &p, out);
        } else if (p.extension().is_some_and(|x| x == "buri") && name != "BUILD.buri")
            || p.extension().is_some_and(|x| x == "proto")
        {
            let rel = p
                .strip_prefix(root)
                .or_ice("this walk only descends into `root`, so every path it reaches is under it")
                .display()
                .to_string()
                .replace('\\', "/");
            out.push(rel);
        }
    }
}

/// `missing-dep`. Use is what requires a dep, and an import is not the only way
/// to use: a method resolving into a library counts too.
fn check_dependencies(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    // The hygiene rules ask about the same modules this analysis already
    // loaded, so they ride along rather than paying for a second one.
    check_hygiene(session, target, analysis, diagnostics);

    let declared: Vec<crate::build::buildfile::Spanned<String>> =
        session.workspace.declared_deps(target).to_vec();
    let own = target.package;
    // Use is what requires a dep, and an import is not the only way to use: a
    // method resolves through its receiver's type rather than through scope,
    // so a call that lands in another library counts even though no import
    // names it (BUILD-FILES.md, "Dependencies").
    let resolved: BTreeSet<String> = reached_by_resolution(session, analysis, own);
    // What an import already complained about, so a library reached both ways
    // is reported once, at the import, where there is something to point at.
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for m in &analysis.loaded.modules {
        if m.pkg != Some(own) {
            continue;
        }
        for item in &m.ast.items {
            let (path, span) = match item {
                crate::parsing::tree::Item::Import(i) => (i.path.clone(), i.path_span),
                crate::parsing::tree::Item::ReExport(r) => (r.path.clone(), r.path_span),
                _ => continue,
            };
            let Some(wanted) = session.workspace.dependency_label(own, &path) else { continue };
            if !declared.iter().any(|d| d.value == wanted)
                && !in_test_deps(session, target, &wanted)
            {
                let importer = session.map.name(m.file).to_string();
                let package_path = session.workspace.package(own).path.clone();
                reported.insert(wanted.clone());
                diagnostics.push(
                    Diagnostic::templated("missing-dep", span)
                        .with_bind("user", importer)
                        .with_bind("reaches", "imports")
                        .with_bind("dependency", wanted.as_str())
                        .with_bind("package_path", package_path),
                );
            }
        }
    }

    // A library reached only through method resolution. No import names it, so
    // there is nothing in a source file to point at — the claim that is wrong
    // is the one the build file makes, and that is where the span goes
    // (BUILD-FILES.md, "Dependencies": "an import is not the only way to use").
    let package_path = session.workspace.package(own).path.clone();
    let own_label = session.workspace.package(own).label();
    for wanted in &resolved {
        if declared.iter().any(|d| &d.value == wanted)
            || in_test_deps(session, target, wanted)
            || reported.contains(wanted)
        {
            continue;
        }
        diagnostics.push(
            Diagnostic::templated(
                "missing-dep",
                Span::point(session.workspace.package(own).build_file_id, 0),
            )
            .with_bind("user", own_label.as_str())
            .with_bind("reaches", "uses")
            .with_bind("dependency", wanted.as_str())
            .with_bind("package_path", package_path.as_str())
            // Only this site: no import names the library, so the page cannot
            // carry the note — the other site would print it wrongly.
            .with_note(
                "a method resolves through its receiver's type rather than through scope, \
                 so no import names it",
            ),
        );
    }

}

/// The hygiene and shape rules — `unused-import`, `discarded-result`,
/// `test-without-assertion` and the rest. Every one of them asks about a
/// package's own code rather than about the build graph, so they share the
/// analysis `check_dependencies` has already paid for.
/// The modules this package owns, which is what separates "my code" from
/// everything the analysis loaded to check it.
///
/// Four checks ask this and every one of them wants the same set, so it is one
/// function: a lint that walked a dependency's bodies would report findings
/// against code the author cannot edit.
fn modules_of(
    analysis: &crate::compiler::driver::Analysis,
    own: PackageId,
) -> BTreeSet<crate::compiler::semantics::types::ModuleId> {
    analysis.loaded.modules.iter().filter(|m| m.pkg == Some(own)).map(|m| m.id).collect()
}

/// [`modules_of`], less the ones nobody can edit. The shape rules all end in
/// "rewrite this", which is not an instruction a generated module can be given.
fn editable_modules_of(
    analysis: &crate::compiler::driver::Analysis,
    own: PackageId,
) -> BTreeSet<crate::compiler::semantics::types::ModuleId> {
    analysis
        .loaded
        .modules
        .iter()
        .filter(|m| m.pkg == Some(own) && !is_generated(&m.path))
        .map(|m| m.id)
        .collect()
}

/// What an analysis could not vouch for, so that a target with a type error is
/// still linted everywhere the error cannot reach.
///
/// The checker stops where a body stops making sense: the expression it failed
/// on becomes a leaf and everything written under it is gone from the typed
/// tree. A rule reading that tree would then call a name unused because its
/// only use sat in the part that went missing. So the rules that read it ask
/// here first and stay silent for exactly the broken body, rather than the
/// whole target — which is what the early return this replaced did.
///
/// **The two questions are separate, and each is answered by the evidence that
/// belongs to it.** A body is unreadable when an error lands *in* it: that is
/// the thing that truncates the typed tree, and nothing else does. A module is
/// unreadable when its account of *what reaches what* is short: the parser
/// skipped a declaration, or an error landed on one of the imports and
/// re-exports that account is made of. That account is what [`check_dead_code`]
/// reads, and it is the only thing the module set is asked about.
///
/// Neither question is answered by where an error happens to sit. That is what
/// this used to do, and it was wrong twice over: a recovered missing `;` on an
/// import silenced `unused-variable` in a function twenty lines below it, and
/// `circular-type-alias` — an error on a declaration the parser read whole —
/// silenced every lint in its module. Both errors leave a complete AST and a
/// complete scope behind them, so neither is a reason to stop reading anything.
/// A position is not a kind, and only a kind can say what was lost.
#[derive(Default)]
struct Unchecked {
    /// Modules whose imports and re-exports are not the whole account of what
    /// they reach: an [`Item::Error`] among their items is a run of
    /// declarations the parser skipped, and an error landing on an import or a
    /// re-export is a line of that account that did not resolve. Either way a
    /// rule that reads one module's imports to justify another module's export
    /// cannot run at all.
    ///
    /// Nothing else is here, however far from a body it sits: an alias that
    /// closes a cycle, a field whose type did not check, a signature that did
    /// not. Each of those is one declaration the reader can see is wrong, and
    /// none of them says anything about what reaches what.
    ///
    /// This set answers `dead-code`'s question and no other. It does **not**
    /// reach the bodies: a module error is not a reason to stop reading a
    /// function that checked.
    ///
    /// [`Item::Error`]: crate::parsing::tree::Item::Error
    modules: BTreeSet<ModuleId>,
    /// Functions whose body cannot be read, which is an error inside it — in
    /// the body or in the signature above it.
    ///
    /// A name the parser skipped, or one an unresolved import never bound, is
    /// reached here the same way: a body that uses it gets `unresolved-name`
    /// at the use, which is an error inside that body. So the bodies that lost
    /// something say so themselves, and the ones that did not are read.
    bodies: BTreeSet<FnId>,
    /// Where those bodies are, for the rules that hold a span rather than a
    /// function.
    ranges: Vec<Span>,
}

impl Unchecked {
    fn of(analysis: &crate::compiler::driver::Analysis) -> Unchecked {
        use crate::diagnostics::FileId;
        let mut unchecked = Unchecked::default();
        // By file. Most of a compilation is code the repository merely reads,
        // so a body in a file nothing is wrong with has to cost a lookup
        // rather than a scan.
        let mut errors: std::collections::BTreeMap<FileId, Vec<Span>> =
            std::collections::BTreeMap::new();
        for d in &analysis.diagnostics.items {
            if d.is_error() && !d.span.is_none() {
                errors.entry(d.span.file).or_default().push(d.span);
            }
        }
        if errors.is_empty() {
            return unchecked;
        }

        // A function's whole declaration, from its name to the end of its
        // body: an error on a parameter's type is as much a reason not to read
        // the body as one inside it.
        for (fid, body) in &analysis.checked.bodies {
            let info = analysis.checked.tables.fn_info(*fid);
            let Some(in_file) = errors.get(&info.span.file) else { continue };
            if info.span.file != body.expr.span.file {
                continue;
            }
            let start = info.span.start.min(body.expr.span.start) as usize;
            let end = info.span.end.max(body.expr.span.end) as usize;
            let extent = Span::new(info.span.file, start, end);
            if in_file.iter().any(|e| e.start <= extent.end && e.end >= extent.start) {
                unchecked.bodies.insert(*fid);
                unchecked.ranges.push(extent);
            }
        }

        // A module whose account of what reaches what is in doubt. The set is
        // read by one rule, `check_dead_code`, and that rule reads exactly
        // three things in a module: its imports, its re-exports, and whatever
        // the parser could not turn into either. So those three are what is
        // asked about, and nothing else in the file is:
        //
        // * an `Item::Error` — the run of source the parser skipped, which
        //   could have held the import that reaches the export in question;
        // * an error landing on an import or a re-export — the list is there
        //   but one of its lines did not resolve, so what it reaches is a
        //   shorter list than the author wrote. A half-typed
        //   `import { Zz` in an editor is this, and without it the name it
        //   used to reach lights up as dead while the line is being written.
        //
        // An error on a function, a struct or an alias is not here however far
        // from a body it sits. None of those says anything about what reaches
        // anything, which is the only question this answers.
        //
        // Only files something is already wrong with are looked at, which is
        // what keeps this a lookup per module rather than a walk of every item
        // in the standard library.
        for m in &analysis.loaded.modules {
            let Some(in_file) = errors.get(&m.file) else { continue };
            let doubtful = m.ast.items.iter().any(|i| {
                use crate::parsing::tree::Item;
                match i {
                    Item::Error(_) => true,
                    Item::Import(_) | Item::ReExport(_) => {
                        let at = i.span();
                        in_file.iter().any(|e| e.start <= at.end && e.end >= at.start)
                    }
                    _ => false,
                }
            });
            if doubtful {
                unchecked.modules.insert(m.id);
            }
        }
        unchecked
    }

    fn body(&self, function: FnId) -> bool {
        self.bodies.contains(&function)
    }

    fn module(&self, module: ModuleId) -> bool {
        self.modules.contains(&module)
    }

    /// Whether a span sits in a body that did not check.
    fn at(&self, span: Span) -> bool {
        self.ranges
            .iter()
            .any(|r| r.file == span.file && span.start >= r.start && span.end <= r.end)
    }
}

fn check_hygiene(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    let own = target.package;
    let unchecked = Unchecked::of(analysis);
    for m in &analysis.loaded.modules {
        if m.pkg == Some(own) && !is_generated(&m.path) {
            check_unused_imports(session, m, diagnostics);
            check_duplicate_imports(m, diagnostics);
            check_warning_comments(session, m, diagnostics);
            check_function_shapes(session, m, diagnostics);
        }
    }
    check_dead_code(session, target, analysis, &unchecked, diagnostics);
    check_unused_declarations(session, target, analysis, &unchecked, diagnostics);
    check_ctx_rebindings(own, analysis, &unchecked, diagnostics);
    check_unused_contexts(session, target, analysis, &unchecked, diagnostics);
    check_unused_context_bounds(session, target, analysis, &unchecked, diagnostics);
    check_discarded_results(own, analysis, diagnostics);
    check_unused_variables(own, analysis, &unchecked, diagnostics);
    check_deep_nesting(own, analysis, diagnostics);
    check_tests_assert(own, analysis, &unchecked, diagnostics);
    check_test_titles(own, analysis, diagnostics);
}

/// The three limits the shape rules hold code to. None of them is configurable,
/// so each sits above the whole of this repository's Buri — standard library,
/// conformance corpus and worked monorepo — and a program that crosses one has
/// crossed it for a reason rather than by writing ordinary code.
///
/// The numbers reach the reader through the pages' `{limit}`, so there is one
/// of each rather than one here and one in a sentence that can drift from it.
const MAXIMUM_FUNCTION_LINES: usize = 40;
const MAXIMUM_PARAMETERS: usize = 5;
const MAXIMUM_NESTING: usize = 4;

/// `oversized-function` and `too-many-parameters`, both read off the
/// declaration rather than the body: the two questions are about the shape a
/// reader meets, and that is what is written down.
///
/// A `test` block is not a function for either rule. A suite is a list of
/// assertions, and its length is the number of cases rather than the number of
/// things it does.
fn check_function_shapes(session: &Session, m: &ModuleData, diagnostics: &mut Diagnostics) {
    use crate::parsing::tree::{Item, ParamKind};
    let file = session.map.get(m.file);
    let mut check = |d: &crate::parsing::tree::FnDecl| {
        let name = m.ast.tree.name(d.name);
        // `self` is the receiver and `ctx` is the effect budget. Neither is
        // data a caller assembled, so neither is counted.
        let parameters = d.params.iter().filter(|p| p.kind == ParamKind::Normal).count();
        if parameters > MAXIMUM_PARAMETERS {
            diagnostics.push(
                Diagnostic::templated("too-many-parameters", d.name.span)
                    .with_bind("name", name)
                    .with_bind("count", parameters.to_string())
                    .with_bind("limit", MAXIMUM_PARAMETERS.to_string()),
            );
        }
        let Some(body) = d.body else { return };
        let span = m.ast.tree.block_span(body);
        let lines = file.line_col(span.end).0 - file.line_col(span.start).0 + 1;
        if lines > MAXIMUM_FUNCTION_LINES {
            diagnostics.push(
                Diagnostic::templated("oversized-function", d.name.span)
                    .with_bind("name", name)
                    .with_bind("lines", lines.to_string())
                    .with_bind("limit", MAXIMUM_FUNCTION_LINES.to_string()),
            );
        }
    };
    for item in &m.ast.items {
        match item {
            Item::Fn(d) => check(d),
            Item::Impl(d) => d.methods.iter().for_each(&mut check),
            Item::Trait(d) => d.methods.iter().for_each(&mut check),
            _ => {}
        }
    }
}

/// `duplicate-import`. Two statements naming one module are a pair that drifts:
/// the second is easy to miss, so an edit lands on one of them and the top of
/// the file stops being the whole account of what the file borrows.
///
/// Only two named clauses. A statement carries one clause, so a namespace
/// import cannot be merged into a named one and there is no fix to offer.
fn check_duplicate_imports(m: &ModuleData, diagnostics: &mut Diagnostics) {
    use crate::parsing::tree::{ImportClause, Item};
    let mut first: std::collections::BTreeMap<&str, Span> = std::collections::BTreeMap::new();
    for item in &m.ast.items {
        let Item::Import(i) = item else { continue };
        if !matches!(i.clause, ImportClause::Named(_)) {
            continue;
        }
        match first.get(i.path.as_str()) {
            Some(at) => diagnostics.push(
                Diagnostic::templated("duplicate-import", i.path_span)
                    .with_bind("module", i.path.as_str())
                    .with_secondary_span(*at, "first imported here"),
            ),
            None => {
                first.insert(i.path.as_str(), i.path_span);
            }
        }
    }
}

/// The markers a comment uses to say the code is not finished.
///
/// `XXX` is deliberately not one of them: it is how a hexadecimal escape is
/// spelled in prose, and `\uXXXX` appears in this repository's own comments.
const WARNING_MARKERS: &[&str] = &["TODO", "FIXME", "HACK"];

/// `warning-comment`. The markers are found in the gaps between tokens, which
/// is what makes the rule about comments rather than about text: everything
/// between two tokens is whitespace or a comment, so a `TODO` inside a string
/// literal is inside a token and is never looked at.
fn check_warning_comments(session: &Session, m: &ModuleData, diagnostics: &mut Diagnostics) {
    let text = session.map.text(m.file);
    let lexed = crate::parsing::lexer::lex(text, m.file);
    let mut found: Vec<(usize, &'static str)> = Vec::new();
    let mut at = 0usize;
    for i in 0..lexed.tokens.len() {
        let span = lexed.tokens.span(i);
        markers_in(text, at, span.start as usize, &mut found);
        at = at.max(span.end as usize);
    }
    markers_in(text, at, text.len(), &mut found);

    found.sort_unstable();
    for (offset, marker) in found {
        diagnostics.push(
            Diagnostic::templated(
                "warning-comment",
                Span::new(m.file, offset, offset + marker.len()),
            )
            .with_bind("marker", marker),
        );
    }
}

/// Every marker in `text[from..to]`, as an offset into the whole file.
fn markers_in(text: &str, from: usize, to: usize, out: &mut Vec<(usize, &'static str)>) {
    let Some(gap) = text.get(from..to) else { return };
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    for marker in WARNING_MARKERS {
        let mut at = 0usize;
        while let Some(hit) = gap.get(at..).and_then(|rest| rest.find(marker)) {
            let start = at + hit;
            let end = start + marker.len();
            // A whole word, so `HACKATHON` is prose rather than a marker.
            if !is_word(gap.get(..start).and_then(|b| b.chars().next_back()))
                && !is_word(gap.get(end..).and_then(|a| a.chars().next()))
            {
                out.push((from + start, marker));
            }
            at = end;
        }
    }
}

/// `unused-variable`. A `let` names a value for the code below it, so a name
/// nothing below reads is a `let` that names nothing.
///
/// Only `let`. A binding in a `match` arm or a lambda's parameter list is part
/// of a shape being described rather than a name introduced for later, and
/// asking whether one is read is a different question.
fn check_unused_variables(
    own: PackageId,
    analysis: &crate::compiler::driver::Analysis,
    unchecked: &Unchecked,
    diagnostics: &mut Diagnostics,
) {
    use crate::compiler::semantics::types::LocalId;
    let mine = editable_modules_of(analysis, own);
    let mut found: Vec<(Span, String)> = Vec::new();
    for (fid, body) in &analysis.checked.bodies {
        // The reads are what this counts, and a body that did not check has
        // lost the ones under whatever it failed on.
        if !mine.contains(&analysis.checked.tables.fn_info(*fid).module) || unchecked.body(*fid) {
            continue;
        }
        let mut read: BTreeSet<LocalId> = BTreeSet::new();
        typed::walk(&body.expr, &mut |e| match &e.kind {
            typed::ExprKind::Local(local) => {
                read.insert(*local);
            }
            // A capture reads the local it captures, whatever the lambda's
            // body then does with it.
            typed::ExprKind::Lambda { captures, .. } => read.extend(captures.iter().copied()),
            _ => {}
        });

        let mut bound: Vec<LocalId> = Vec::new();
        typed::walk(&body.expr, &mut |e| {
            let typed::ExprKind::Block { stmts, .. } = &e.kind else { return };
            for s in stmts {
                if let typed::Stmt::Let { pattern, .. } = s {
                    pattern.binds(&mut bound);
                }
            }
        });

        for local in bound {
            if read.contains(&local) {
                continue;
            }
            let Some(l) = body.locals.get(local.index()) else { continue };
            found.push((l.span, l.name.clone()));
        }
    }
    // `bodies` is a map, so the order findings are met in is not the order they
    // are written in. Sorting here makes one run's report the same as the next.
    found.sort_by_key(|(span, _)| (span.file.0, span.start));
    for (span, name) in found {
        diagnostics.push(Diagnostic::templated("unused-variable", span).with_bind("name", name));
    }
}

/// `deep-nesting`. Every enclosing branch is a condition the reader has to hold
/// while reading the innermost one, and past a handful nobody holds them all.
fn check_deep_nesting(
    own: PackageId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    let mine = editable_modules_of(analysis, own);
    let mut found: Vec<(Span, usize)> = Vec::new();
    for (fid, body) in &analysis.checked.bodies {
        if !mine.contains(&analysis.checked.tables.fn_info(*fid).module) {
            continue;
        }
        nesting(&body.expr, 0, false, &mut found);
    }
    found.sort_by_key(|(span, _)| (span.file.0, span.start));
    for (span, depth) in found {
        diagnostics.push(
            Diagnostic::templated("deep-nesting", span)
                .with_bind("depth", depth.to_string())
                .with_bind("limit", MAXIMUM_NESTING.to_string()),
        );
    }
}

/// Walks a body counting the branch bodies wrapped around each expression, and
/// reports the outermost construct that sits past the limit.
///
/// `reported` is what keeps one deep nest from being one finding per level:
/// once a subtree has been reported, the levels under it are the same finding.
fn nesting(e: &typed::Expr, depth: usize, reported: bool, out: &mut Vec<(Span, usize)>) {
    let report = |out: &mut Vec<(Span, usize)>| {
        if depth >= MAXIMUM_NESTING && !reported {
            out.push((e.span, depth + 1));
            return true;
        }
        reported
    };
    match &e.kind {
        typed::ExprKind::If { cond, then, else_ } => {
            nesting(cond, depth, reported, out);
            let reported = report(out);
            nesting(then, depth + 1, reported, out);
            // `else if` continues a ladder rather than nesting inside one: it
            // is written at the same indentation and read as one decision.
            let deeper = !matches!(else_.kind, typed::ExprKind::If { .. });
            nesting(else_, depth + usize::from(deeper), reported, out);
        }
        typed::ExprKind::Match { scrutinee, arms } => {
            nesting(scrutinee, depth, reported, out);
            let reported = report(out);
            for a in arms {
                if let Some(g) = &a.guard {
                    nesting(g, depth, reported, out);
                }
                nesting(&a.body, depth + 1, reported, out);
            }
        }
        // A lambda is a unit of reading of its own — extracting one is the fix
        // this rule asks for — so its body starts again at nothing.
        typed::ExprKind::Lambda { body, .. } => nesting(body, 0, false, out),
        _ => typed::children(e, &mut |c| nesting(c, depth, reported, out)),
    }
}

/// `test-title-newline`. A title spanning lines is legal and reported — the
/// runner escapes it, so the one-line-per-`FAIL` shape holds — but the report
/// then shows `\n` where the author meant a line break, and nothing else in the
/// output is prose that wraps.
///
/// A warning rather than an error, because it is a matter of taste about the
/// text and not a rule about the program: `duplicate-test-name` is the rule.
fn check_test_titles(
    own: PackageId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    let mine = modules_of(analysis, own);
    for case in &analysis.checked.tests {
        if !mine.contains(&case.module) || !case.name.contains('\n') {
            continue;
        }
        diagnostics.push(
            Diagnostic::templated("test-title-newline", case.span)
                .with_bind("quoted_title", format!("{:?}", case.name)),
        );
    }
}

/// `dead-code`. Production code is reached from a library's surface or from a
/// binary's `main`. Inside a library, `export` means "visible to the rest of
/// this library" and `lib.buri` decides what leaves it, so an `export` that
/// `lib.buri` does not re-export and no sibling module imports is reached by
/// nothing at all — it is dead, whatever the word in front of it says.
///
/// Only module-level items. A field's or a variant's `export` is about the
/// shape of a type, and asking whether it is "reached" is a different question.
fn check_dead_code(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    unchecked: &Unchecked,
    diagnostics: &mut Diagnostics,
) {
    // A binary has no surface — nothing may import its modules — so the rule
    // does not apply to one.
    if target.kind != RuleKind::Library {
        return;
    }
    let own = target.package;
    // What reaches an export is written at the top of a module — `lib.buri`'s
    // re-exports, and what a sibling imports — so one module of the package
    // the parser did not read whole is enough to make a live name look
    // unreached: the import that reaches it may be in the run of declarations
    // the parser skipped, and a name nothing in the tree mentions is not a
    // name nothing uses.
    if modules_of(analysis, own).iter().any(|m| unchecked.module(*m)) {
        return;
    }
    let Some(surface) = analysis.checked.surfaces.get(&own) else { return };

    // What a sibling module inside the same package imports or re-exports, and
    // which sibling modules are taken whole by `import * as x`. A namespace
    // import reaches every name its target exports, so the rule cannot say
    // anything about that one module — but it still applies to the others.
    let mut wanted: BTreeSet<&str> = BTreeSet::new();
    let mut taken_whole: BTreeSet<&str> = BTreeSet::new();
    for m in &analysis.loaded.modules {
        if m.pkg != Some(own) {
            continue;
        }
        for item in &m.ast.items {
            let (path, specs) = match item {
                crate::parsing::tree::Item::Import(i) => match &i.clause {
                    crate::parsing::tree::ImportClause::Named(sp) => (&i.path, sp),
                    crate::parsing::tree::ImportClause::Namespace(_) => {
                        taken_whole.insert(i.path.as_str());
                        continue;
                    }
                },
                crate::parsing::tree::Item::ReExport(r) => (&r.path, &r.specs),
                _ => continue,
            };
            let Ok(loc) = session.workspace.resolve_module(path) else { continue };
            if loc.in_package().map(|m| m.package) != Some(own) {
                continue;
            }
            for sp in specs {
                wanted.insert(m.ast.tree.name(sp.name));
            }
        }
    }

    for m in &analysis.loaded.modules {
        if m.pkg != Some(own) {
            continue;
        }
        // `lib.buri` is the surface; what it exports is the answer, not the
        // question. Test sources are not importable at all.
        if m.path.ends_with("/lib") || !matches!(m.role, crate::compiler::modules::Role::Source) {
            continue;
        }
        // A generated module is not a file anybody can edit, so neither of the
        // two fixes this rule offers exists for one. A `.proto` schema exports
        // the whole of what it declares — that is what a schema *is* — and
        // `lib.buri` re-exports the part of it the library means to publish.
        if is_generated(&m.path) {
            continue;
        }
        if taken_whole.contains(m.path.as_str()) {
            continue;
        }
        for item in &m.ast.items {
            let Some((name, span)) = exported_name(&m.ast.tree, item) else { continue };
            if surface.contains(name) || wanted.contains(name) {
                continue;
            }
            let lib = format!("{}/lib.buri", session.workspace.package(own).path);
            diagnostics.push(
                Diagnostic::templated("dead-code", span)
                    .with_bind("name", name)
                    .with_bind("library_file", lib),
            );
        }
    }
}

/// `unused-type`, `unused-field` and `unused-variant`: the shapes a package
/// declares and never uses.
///
/// **A type is used two ways, and the union of them is the answer.** It is used
/// when the package's own code *writes its name down* somewhere that is not its
/// own declaration or an `impl`/`derive` block written about it, and it is used
/// when a body *builds or matches a value of it* without naming it at all.
/// Neither half is enough on its own: the name scan cannot see a `.Variant`
/// shorthand or an anonymous struct literal, which name nothing, and the typed
/// tree cannot see a type mentioned only in an alias nobody expands. Their
/// union over-approximates use, which is the safe direction for a rule nobody
/// can turn off — the trade [`check_unused_imports`] already makes, and for the
/// same reason.
///
/// A type's own `impl` and `derive` blocks are not uses of it. They describe
/// the shape rather than reach for it, and without that a dead type with a
/// method on it would never be reported. The exclusion is by name and by
/// extent, so the *other* names those blocks mention are ordinary uses.
///
/// **A field is used when something reads it**, which is `s.field` or a pattern
/// that binds it. Filling one in at a literal is a write: a field every literal
/// supplies and nothing ever consults is the finding, which is why
/// `Option`-field elision changes nothing here — a field the author no longer
/// has to write is still a field nobody reads. Two things read every field at
/// once and leave no projection behind to find, and both count: a `derive`,
/// which is a fold over the whole type definition (SPEC 5.12.3), and a value
/// handed to an intrinsic or compared structurally, which is the runtime
/// reading a value this rule cannot see into. `S { f: _ }` is a mention rather
/// than a read — that is how a reader says they do not need it, and it is what
/// `_` says in a `let`.
///
/// **A variant is used when something builds it or names it in a pattern.** A
/// `_` arm names no variant, so an enum matched only by wildcard has nothing
/// keeping its variants alive; `.Increment` names one without naming its enum,
/// which is why the evidence is the typed tree rather than the token. A
/// `derive` does *not* rescue a variant, and the asymmetry with a field is the
/// question rather than an inconsistency: a derived fold reads every field's
/// *value*, and it puts a value into no *case*.
///
/// **What is exempt is what the repository cannot see the whole of.** A name
/// `lib.buri` puts on the library's surface is public API — a consumer outside
/// this repository may name it, build it and read it — so a surfaced type, the
/// *exported* fields of one, and the variants of one are not reported. That is
/// the stance [`check_dead_code`] already takes, and it is what makes a
/// package-local census a complete one: a type the surface does not carry is
/// visible only inside its own package, and the package is entirely in front of
/// this analysis. A non-exported field is private to its declaring module even
/// on a published type, so it is still asked about. A binary has no surface at
/// all, and nothing in one is exempt.
///
/// **One finding per dead shape.** A type nothing uses has no read fields and
/// no matched variants by construction, so it is reported once rather than once
/// per member. For the same reason a declaration [`check_dead_code`] has
/// already reported is left alone: "nothing reaches this export" is the
/// stronger claim and the one carrying the `lib.buri` fix, and that rule is
/// asked first so that this one can see what it said.
///
/// **What silences it, and how far.** Two different doubts, and each is scoped
/// to what it actually costs.
///
/// A module whose account of what it reaches is short — an [`Unchecked`] module
/// — puts the library's *surface* in doubt, and the surface is the whole of
/// what an unresolved `export` line can cost this rule. A name that line may
/// have published is a name that may be exempt, so nothing is reported about
/// any **exported** declaration while the doubt stands. It reaches no further
/// than that, and the reason is worth stating: a declaration the parser skipped
/// and an import that did not resolve are both still *text*, and this rule's
/// other half reads text — a use inside either is counted like any other. What
/// cannot be recovered from the text is which names `lib.buri` meant to
/// publish, and a name with no `export` on it was never one of them.
///
/// A body that did not check is narrower than that, and this is where the rule
/// parts company with `check_dead_code`. The checker stops where a body stops
/// making sense, so the reads written under that point are gone from the typed
/// tree — but they are still *in the file*, and the lexer can see them. So the
/// doubt is per name rather than per package: an identifier appearing inside a
/// body that did not check, or in a run of declarations the parser skipped, is
/// a name that might be used there, and the type, field or variant it could be
/// naming is left alone. A name that appears nowhere in the unreadable text is
/// a name that text does not use, and the finding stands.
///
/// A type's own name doubts its members too: what a broken region holds might
/// be the `derive` that reads every field at once, and that line names the type
/// rather than any field of it.
fn check_unused_declarations(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    unchecked: &Unchecked,
    diagnostics: &mut Diagnostics,
) {
    use crate::compiler::semantics::types::{TyConId, TyDef};

    let own = target.package;
    let mine = modules_of(analysis, own);
    let tables = &analysis.checked.tables;
    let surface_in_doubt = mine.iter().any(|m| unchecked.module(*m));

    // What `dead-code` has already said, which is why it is asked first (see
    // `check_hygiene`). "Nothing reaches this export" is the stronger claim and
    // it is the one carrying the `lib.buri` fix, so a declaration it has spoken
    // about is left alone here — a reader meeting two warnings on one `struct`
    // has to work out that they are the same news.
    let spoken_for: BTreeSet<(u32, u32, u32)> = diagnostics
        .items
        .iter()
        .filter(|d| d.code.as_deref() == Some("dead-code"))
        .map(|d| (d.span.file.0, d.span.start, d.span.end))
        .collect();

    let names = Names::of(session, analysis, &mine, unchecked);
    let census = Census::of(analysis, &mine);
    let reportable = editable_modules_of(analysis, own);
    // A package with no library has no surface, and nothing in it is exempt.
    let surface = analysis.checked.surfaces.get(&own);

    for (index, con) in tables.tycons.iter().enumerate() {
        if !reportable.contains(&con.module) || con.span.is_none() {
            continue;
        }
        if spoken_for.contains(&(con.span.file.0, con.span.start, con.span.end))
            || names.doubted.contains(&con.name)
        {
            continue;
        }
        let id = TyConId(index as u32);
        // Published, or possibly published: either way this rule does not ask
        // about it, and neither does it ask about the exported fields and the
        // variants underneath, which are the shape a consumer reaches through.
        let exempt = con.exported
            && (surface_in_doubt || surface.is_some_and(|s| s.contains(&con.name)));
        if !census.built.contains(&id) && !names.written.contains(&con.name) {
            if !exempt {
                diagnostics.push(
                    Diagnostic::templated("unused-type", con.span)
                        .with_bind("name", con.name.as_str()),
                );
            }
            continue;
        }
        match &con.def {
            // A tuple struct's fields have no names to report and no
            // declarations of their own to point at; `.0` is the whole of what
            // a reader writes, and the type is the finding.
            TyDef::Struct { fields, record: true } => {
                if census.read_whole.contains(&id) {
                    continue;
                }
                for (i, f) in fields.iter().enumerate() {
                    if (exempt && f.exported)
                        || census.read.contains(&(id, i))
                        || names.doubted.contains(&f.name)
                    {
                        continue;
                    }
                    diagnostics.push(
                        Diagnostic::templated("unused-field", f.span)
                            .with_bind("type", con.name.as_str())
                            .with_bind("name", f.name.as_str()),
                    );
                }
            }
            TyDef::Enum { variants } if !exempt => {
                for (i, v) in variants.iter().enumerate() {
                    if census.variants.contains(&(id, i)) || names.doubted.contains(&v.name) {
                        continue;
                    }
                    diagnostics.push(
                        Diagnostic::templated("unused-variant", v.span)
                            .with_bind("type", con.name.as_str())
                            .with_bind("name", v.name.as_str()),
                    );
                }
            }
            _ => {}
        }
    }
}

/// What the package's own text says, in one pass of the lexer over it.
///
/// Deliberately syntactic, exactly as [`check_unused_imports`] is. Reading
/// tokens is what makes it total — there is no type position it can forget —
/// and a name that happens to be spelled the same as something else silences a
/// finding rather than producing a wrong one. It is also what survives a
/// mistake: a declaration the parser skipped and a body the checker stopped
/// inside are both still text, and the lexer reads them the same as the rest.
///
/// A generated module is not scanned. It declares and uses the types of one
/// schema and nothing else, so there is no hand-written type whose only use
/// could be inside one, and none of its own declarations is reported.
struct Names {
    /// Every name written down somewhere that is not the declaration of a type
    /// of that name, nor an `impl` or `derive` block about one.
    written: BTreeSet<String>,
    /// Every name written down inside text the checker or the parser could not
    /// read: a body that did not check, or a run of declarations the parser
    /// skipped. Nothing is reported about one of these, because the use that
    /// would have answered for it may be exactly what went missing.
    doubted: BTreeSet<String>,
}

impl Names {
    fn of(
        session: &Session,
        analysis: &crate::compiler::driver::Analysis,
        mine: &BTreeSet<ModuleId>,
        unchecked: &Unchecked,
    ) -> Names {
        use crate::parsing::tree::Item;
        let mut names = Names { written: BTreeSet::new(), doubted: BTreeSet::new() };
        // The bodies whose typed tree is short, by file. The *expression* and
        // not [`Unchecked`]'s declaration extent: a signature is text the
        // parser read whole and it holds no reads, so a type named in the
        // signature of a function whose body broke is not in doubt — only what
        // was written inside the body is.
        let mut broken: std::collections::BTreeMap<crate::diagnostics::FileId, Vec<Span>> =
            std::collections::BTreeMap::new();
        for (fid, body) in &analysis.checked.bodies {
            if unchecked.body(*fid) {
                broken.entry(body.expr.span.file).or_default().push(body.expr.span);
            }
        }
        for m in &analysis.loaded.modules {
            if !mine.contains(&m.id) || is_generated(&m.path) {
                continue;
            }
            // What the toolchain could not read: the bodies that did not check,
            // and the runs of source the parser skipped.
            let mut unreadable: Vec<Span> = broken.get(&m.file).cloned().unwrap_or_default();
            unreadable.extend(m.ast.items.iter().filter_map(|i| match i {
                Item::Error(at) => Some(**at),
                _ => None,
            }));
            let mut owned: Vec<(&str, Span)> = Vec::new();
            for item in &m.ast.items {
                match item {
                    Item::Struct(d) => owned.push((m.ast.tree.name(d.name), d.span)),
                    Item::Enum(d) => owned.push((m.ast.tree.name(d.name), d.span)),
                    // The whole block: a type's methods are part of the shape,
                    // so the constructor an unused type calls inside its own
                    // `impl` is not a use of it.
                    Item::Impl(d) => {
                        if let Some(name) = m.ast.tree.type_head(d.self_ty) {
                            owned.push((name, d.span));
                        }
                    }
                    // Only the type the `derive` is *for*. The traits it names
                    // are uses of those traits like any other mention.
                    Item::Derive(d) => {
                        if let Some(name) = m.ast.tree.type_head(d.self_ty) {
                            owned.push((name, m.ast.tree.type_span(d.self_ty)));
                        }
                    }
                    _ => {}
                }
            }
            let text = session.map.text(m.file);
            let lexed = crate::parsing::lexer::lex(text, m.file);
            for i in 0..lexed.tokens.len() {
                if lexed.tokens.kind(i) != crate::parsing::lexer::TokenKind::Ident {
                    continue;
                }
                let at = lexed.tokens.span(i);
                let name = lexed.tokens.text(i);
                if unreadable.iter().any(|r| at.start >= r.start && at.end <= r.end) {
                    names.doubted.insert(name.to_string());
                }
                if owned.iter().any(|(owner, range)| {
                    *owner == name && at.start >= range.start && at.end <= range.end
                }) {
                    continue;
                }
                names.written.insert(name.to_string());
            }
        }
        names
    }
}

/// What a package's own bodies do with the types the package declares.
///
/// Read by [`check_unused_declarations`], and the half of its question the
/// tokens cannot answer: what is built or matched without being written down,
/// and which fields are read.
#[derive(Default)]
struct Census {
    /// Types a value of which is built, matched, or held by an expression.
    built: BTreeSet<crate::compiler::semantics::types::TyConId>,
    /// `(type, field)` read: projected out, or bound by a pattern.
    read: BTreeSet<(crate::compiler::semantics::types::TyConId, usize)>,
    /// Types every field of which is read at once, by something with no
    /// projection to find: a `derive`, an intrinsic, a structural comparison.
    read_whole: BTreeSet<crate::compiler::semantics::types::TyConId>,
    /// `(type, variant)` built or named by a pattern.
    variants: BTreeSet<(crate::compiler::semantics::types::TyConId, usize)>,
    /// The type the body being walked belongs to, whose own mentions of itself
    /// are not uses of it. See [`check_unused_declarations`].
    owner: Option<crate::compiler::semantics::types::TyConId>,
    /// Reused across mentions so that walking a body is not one allocation per
    /// expression.
    scratch: Vec<crate::compiler::semantics::types::TyConId>,
}

impl Census {
    fn of(analysis: &crate::compiler::driver::Analysis, mine: &BTreeSet<ModuleId>) -> Census {
        let mut census = Census::default();
        let tables = &analysis.checked.tables;
        // A `derive` is a fold over one type definition, so a type that derives
        // anything has every field of it read.
        for ((_, con), imp) in &tables.impls {
            if imp.is_derived() {
                census.read_whole.insert(*con);
            }
        }
        for (fid, body) in &analysis.checked.bodies {
            let info = tables.fn_info(*fid);
            if !mine.contains(&info.module) {
                continue;
            }
            census.owner = info.self_ty;
            census.walk(&body.expr);
        }
        census.owner = None;
        for value in analysis.checked.consts.values() {
            census.walk(value);
        }
        census
    }

    fn walk(&mut self, root: &typed::Expr) {
        // `typed::walk` does not descend into patterns, so the arms and the
        // `let`s are taken here where the expression carrying them is met.
        typed::walk(root, &mut |e| {
            self.mention(&e.ty);
            match &e.kind {
                typed::ExprKind::StructLit { con, .. }
                | typed::ExprKind::StructUpdate { con, .. } => self.builds(*con),
                typed::ExprKind::EnumLit { con, variant, .. } => {
                    self.builds(*con);
                    self.variants.insert((*con, *variant));
                }
                typed::ExprKind::Field { base, index } => {
                    if let Some(con) = base.ty.head() {
                        self.read.insert((con, *index));
                    }
                }
                typed::ExprKind::Intrinsic { args, .. }
                | typed::ExprKind::StructuralEq { args, .. }
                | typed::ExprKind::StructuralCmp { args, .. } => {
                    for a in args {
                        let mut whole = Vec::new();
                        cons_in(&a.ty, &mut whole);
                        self.read_whole.extend(whole);
                    }
                }
                typed::ExprKind::Match { arms, .. } => {
                    for a in arms {
                        self.pattern(&a.pattern);
                    }
                }
                typed::ExprKind::Block { stmts, .. } => {
                    for s in stmts {
                        if let typed::Stmt::Let { pattern, .. } = s {
                            self.pattern(pattern);
                        }
                    }
                }
                _ => {}
            }
        });
    }

    fn pattern(&mut self, p: &typed::Pattern) {
        self.mention(&p.ty);
        match &p.kind {
            typed::PatKind::Struct { con, fields } => {
                self.builds(*con);
                for f in fields {
                    if !matches!(f.pattern.kind, typed::PatKind::Wild) {
                        self.read.insert((*con, f.index));
                    }
                    self.pattern(&f.pattern);
                }
            }
            typed::PatKind::Variant { con, variant, fields } => {
                self.builds(*con);
                self.variants.insert((*con, *variant));
                for f in fields {
                    self.pattern(&f.pattern);
                }
            }
            typed::PatKind::Tuple(ps) | typed::PatKind::Or(ps) => {
                ps.iter().for_each(|p| self.pattern(p));
            }
            typed::PatKind::Array { elems, .. } => elems.iter().for_each(|p| self.pattern(p)),
            typed::PatKind::Bind { sub: Some(s), .. } => self.pattern(s),
            _ => {}
        }
    }

    fn builds(&mut self, con: crate::compiler::semantics::types::TyConId) {
        if self.owner != Some(con) {
            self.built.insert(con);
        }
    }

    fn mention(&mut self, ty: &crate::compiler::semantics::types::Ty) {
        self.scratch.clear();
        let mut found = std::mem::take(&mut self.scratch);
        cons_in(ty, &mut found);
        for con in &found {
            if self.owner != Some(*con) {
                self.built.insert(*con);
            }
        }
        self.scratch = found;
    }
}

/// Every type constructor a type is written in terms of, itself included.
fn cons_in(
    ty: &crate::compiler::semantics::types::Ty,
    out: &mut Vec<crate::compiler::semantics::types::TyConId>,
) {
    use crate::compiler::semantics::types::Ty;
    match ty {
        Ty::Con(id, args) => {
            out.push(*id);
            args.iter().for_each(|a| cons_in(a, out));
        }
        Ty::Array(elem) => cons_in(elem, out),
        Ty::Tuple(elems) => elems.iter().for_each(|t| cons_in(t, out)),
        Ty::Fn(params, ret) => {
            params.iter().for_each(|t| cons_in(t, out));
            cons_in(ret, out);
        }
        Ty::Var(_) | Ty::Param(_) | Ty::Unit | Ty::Ctx(_) | Ty::SelfTy | Ty::Error => {}
    }
}

/// A module the toolchain wrote rather than a person: today, one generated
/// from a `.proto` schema. The hygiene rules ask a person to make an edit, and
/// there is no file here to edit.
fn is_generated(path: &str) -> bool {
    crate::build::protogen::is_proto_path(path)
}

/// The name a module-level item exports, when it exports one.
fn exported_name<'t>(
    t: &'t crate::parsing::flat::Tree,
    item: &crate::parsing::tree::Item,
) -> Option<(&'t str, Span)> {
    use crate::parsing::tree::Item;
    let (exported, name) = match item {
        Item::Fn(d) => (d.exported, d.name),
        Item::Struct(d) => (d.exported, d.name),
        Item::Enum(d) => (d.exported, d.name),
        Item::TypeAlias(d) => (d.exported, d.name),
        Item::Let(d) => (d.exported, d.name),
        Item::Trait(d) => (d.exported, d.name),
        _ => return None,
    };
    exported.then_some((t.name(name), name.span))
}

/// `unused-import`. Deliberately syntactic: a name counts as used if it appears
/// as an identifier token anywhere outside the import statements themselves.
///
/// That over-approximates use, which is the safe direction for a rule nobody
/// can turn off — a shadowed binding or a field with the same spelling silences
/// the finding rather than producing a wrong one. Reading tokens rather than the
/// AST is what makes it total: there is no expression form it can forget.
fn check_unused_imports(session: &Session, m: &ModuleData, diagnostics: &mut Diagnostics) {
    // The byte ranges the import statements occupy. An identifier inside one of
    // these is the binding, not a use of it.
    let mut import_ranges: Vec<(u32, u32)> = Vec::new();
    for item in &m.ast.items {
        match item {
            crate::parsing::tree::Item::Import(i) => import_ranges.push((i.span.start, i.span.end)),
            // A re-export names what it exports, so it *is* a use.
            crate::parsing::tree::Item::ReExport(_) => {}
            _ => {}
        }
    }

    let text = session.map.text(m.file);
    let lexed = crate::parsing::lexer::lex(text, m.file);
    let mut used: BTreeSet<&str> = BTreeSet::new();
    for i in 0..lexed.tokens.len() {
        if lexed.tokens.kind(i) != crate::parsing::lexer::TokenKind::Ident {
            continue;
        }
        let span = lexed.tokens.span(i);
        if import_ranges.iter().any(|(a, b)| span.start >= *a && span.end <= *b) {
            continue;
        }
        used.insert(lexed.tokens.text(i));
    }

    for item in &m.ast.items {
        let crate::parsing::tree::Item::Import(i) = item else { continue };
        let specs: Vec<(&str, Span)> = match &i.clause {
            crate::parsing::tree::ImportClause::Named(specs) => {
                specs.iter().map(|sp| (m.ast.tree.name(sp.local()), sp.span)).collect()
            }
            crate::parsing::tree::ImportClause::Namespace(n) => {
                vec![(m.ast.tree.name(*n), n.span)]
            }
        };
        // One edit per statement, not per name. Two adjacent unused names have
        // overlapping deletions — `A, ` and `, B` share the comma — and the
        // applier refuses on overlap rather than guessing. Rewriting the whole
        // clause has one answer whatever the pattern of unused names is.
        let survivors: Vec<String> = match &i.clause {
            crate::parsing::tree::ImportClause::Named(specs) => specs
                .iter()
                .filter(|sp| used.contains(m.ast.tree.name(sp.local())))
                .map(|sp| match sp.alias {
                    Some(a) => {
                        format!("{} as {}", m.ast.tree.name(sp.name), m.ast.tree.name(a))
                    }
                    None => m.ast.tree.name(sp.name).to_string(),
                })
                .collect(),
            crate::parsing::tree::ImportClause::Namespace(_) => Vec::new(),
        };
        let rewrite = if survivors.is_empty() {
            // Nothing is left, so the statement goes — and its line with it,
            // or `--fix` leaves a blank line where the import was.
            (i.span.start, line_end(text, i.span.end), String::new())
        } else {
            (
                i.span.start,
                i.span.end,
                format!("from \"{}\" import {{ {} }};", i.path, survivors.join(", ")),
            )
        };

        let mut first = true;
        for (name, span) in &specs {
            if used.contains(name) {
                continue;
            }
            let mut d = Diagnostic::templated("unused-import", *span).with_bind("name", *name);
            // The edit rides on the first finding in the statement; the rest
            // are reported and carry none, because there is one edit between
            // them and it has already been claimed.
            if first {
                d = d.with_edit(
                    Span::new(m.file, rewrite.0 as usize, rewrite.1 as usize),
                    &rewrite.2,
                );
                first = false;
            }
            diagnostics.push(d);
        }
    }
}

/// The offset just past the newline ending the line `at` sits on.
fn line_end(text: &str, at: u32) -> u32 {
    let from = at as usize;
    let Some(rest) = text.get(from..) else { return at };
    match rest.find('\n') {
        Some(offset) => (from + offset + 1) as u32,
        // The last line of a file need not end in a newline, and then the end
        // of the line is the end of the text.
        None => text.len() as u32,
    }
}

/// `ctx-rebinding`. `ctx` is the name a function's context arrives under, and
/// a `let` of that name where no context may be built is holding something
/// else — the binding creates no authority, so this is a convention rather
/// than a rule about what a program may do, and a convention is a lint.
///
/// The condition is the checker's: only it knows where a context may be built,
/// so it records the bindings and this reports the ones in the package's own
/// editable code (see [`Checked::ctx_rebindings`]).
///
/// [`Checked::ctx_rebindings`]: crate::compiler::semantics::resolve::Checked::ctx_rebindings
fn check_ctx_rebindings(
    own: PackageId,
    analysis: &crate::compiler::driver::Analysis,
    unchecked: &Unchecked,
    diagnostics: &mut Diagnostics,
) {
    let mine: BTreeSet<crate::diagnostics::FileId> = analysis
        .loaded
        .modules
        .iter()
        .filter(|m| m.pkg == Some(own) && !is_generated(&m.path))
        .map(|m| m.file)
        .collect();
    // Where a context may be built is a question about the function's bounds,
    // so a signature that did not check is one this cannot answer.
    let mut found: Vec<Span> = analysis
        .checked
        .ctx_rebindings
        .iter()
        .copied()
        .filter(|span| mine.contains(&span.file) && !unchecked.at(*span))
        .collect();
    found.sort_by_key(|span| (span.file.0, span.start));
    for span in found {
        diagnostics.push(Diagnostic::templated("ctx-rebinding", span));
    }
}

/// `unused-context`. A `ctx` parameter is the whole of what a function is
/// allowed to do to the world, so a body that never reads it is a signature
/// saying "this touches the world" over code that does not — and the cost is
/// paid by the callers, every one of which has to be holding a context to make
/// the call.
///
/// **The body is the whole of the evidence, and one question answers it.**
/// There are three ways to use a context — call a method it declares, hand it
/// to a callee that asks for one, hand it to a function-typed parameter — and
/// every one of the three reads the parameter's local. So the rule is a walk
/// for [`typed::ExprKind::Local`] naming `body.params[ctx_index]`, and it has
/// no other case to forget.
///
/// **There is no lambda-capture arm, and the invariant that makes one
/// unnecessary still holds.** A `ctx` parameter is put into
/// `Infer::effect_locals` by construction — `inference.rs`'s `check_body` does
/// it for every `ParamRole::Ctx` parameter unconditionally, over the comment
/// "`ctx` is one by construction — the `ctx` rule admits nothing else there" —
/// and `expressions.rs`'s lambda checker reports `lambda-captures-effect` for
/// any capture in that set. So a body whose `captures` list holds `ctx` has
/// already failed to check, and is skipped below with every other body that
/// did. [`typed::walk`] descends into a lambda's body regardless, so a read
/// written inside one would be found by the `Local` arm even if the language
/// ever allowed the capture.
///
/// **A body that did not check is answered from its text instead of going
/// quiet**, which is [`names_ctx`]: the tree is truncated at the failure and
/// the file is not, and a context has one spelling. `unused-variable` cannot do
/// that — a local's name is the author's — and this rule can, so it does.
///
/// Two declarations are not asked about:
///
/// * **one with no body.** A trait method's signature, an effect's operation
///   and the standard library's intrinsic declarations have no entry in
///   `checked.bodies` at all, so the loop over the bodies leaves them out by
///   construction rather than by a test.
/// * **one an `impl` supplies** (`impl_of.is_some()`). The signature is the
///   trait's, and an implementation cannot drop a parameter the trait
///   declared. Whether the *trait* needs it is a question about the trait.
fn check_unused_contexts(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    unchecked: &Unchecked,
    diagnostics: &mut Diagnostics,
) {
    use crate::compiler::semantics::types::ParamRole;
    let mine = editable_modules_of(analysis, target.package);
    let mut found: Vec<(Span, Vec<crate::diagnostics::Edit>)> = Vec::new();
    for (fid, body) in &analysis.checked.bodies {
        let info = analysis.checked.tables.fn_info(*fid);
        if !mine.contains(&info.module) {
            continue;
        }
        if info.impl_of.is_some() || !compiled_by(session, analysis, target, info.module) {
            continue;
        }
        let Some(index) = info.params.iter().position(|p| p.role == ParamRole::Ctx) else {
            continue;
        };
        let (Some(param), Some(ctx_local)) =
            (info.params.get(index), body.params.get(index).copied())
        else {
            continue;
        };
        // A body that did not check has lost the reads written under whatever
        // it failed on, so the tree is not the evidence there and the text is.
        let used = if unchecked.body(*fid) {
            names_ctx(session, extent_of(info, body), param.span)
        } else {
            let mut read = false;
            typed::walk(&body.expr, &mut |e| {
                if matches!(&e.kind, typed::ExprKind::Local(l) if *l == ctx_local) {
                    read = true;
                }
            });
            read
        };
        if used {
            continue;
        }
        let span = param.span;
        found.push((span, context_edits(session, analysis, unchecked, target, *fid, index)));
    }
    // `bodies` is a map, so the order findings are met in is not the order they
    // are written in. Sorting here makes one run's report the same as the next.
    found.sort_by_key(|(span, _)| (span.file.0, span.start));
    for (span, edits) in found {
        let mut d = Diagnostic::templated("unused-context", span);
        for e in edits {
            d = d.with_edit(e.at, &e.replacement);
        }
        diagnostics.push(d);
    }
}

/// A declaration's whole text, from its name to the end of its body.
///
/// The same extent [`Unchecked::of`] takes, and for the same reason: an error
/// on a parameter's type is as much a reason not to read the body as one
/// inside it, so the two questions have to be asked about one region.
fn extent_of(
    info: &crate::compiler::semantics::types::FnInfo,
    body: &typed::Body,
) -> Span {
    if info.span.file != body.expr.span.file {
        return info.span;
    }
    Span::new(
        info.span.file,
        info.span.start.min(body.expr.span.start) as usize,
        info.span.end.max(body.expr.span.end) as usize,
    )
}

/// Whether the text of a declaration writes `ctx` anywhere but at the parameter
/// itself.
///
/// This is what the rule asks of a body that did not check, and it is why a
/// mistake somewhere in a function costs almost none of this rule's findings.
/// The typed tree is truncated at the expression the checker stopped on, so a
/// use written below it is gone — but **the text is not**, and a context has
/// exactly one spelling. There is no alias for `ctx`, no way to reach it
/// through a name of your own, and no way to capture it into a lambda
/// (`lambda-captures-effect`); writing the word is the whole of what using one
/// looks like. So the token is total evidence where the tree is partial, which
/// is the trade [`check_unused_imports`] already makes and states: reading
/// tokens rather than the tree is what leaves no expression form to forget.
///
/// The parameter's own occurrence is excluded by extent rather than by
/// counting, so a signature written across lines is read like any other.
fn names_ctx(session: &Session, extent: Span, param: Span) -> bool {
    let lexed = crate::parsing::lexer::lex(session.map.text(extent.file), extent.file);
    (0..lexed.tokens.len()).any(|i| {
        let at = lexed.tokens.span(i);
        at.start >= extent.start
            && at.end <= extent.end
            && !(at.start >= param.start && at.end <= param.end)
            && lexed.tokens.text(i) == "ctx"
    })
}

/// Whether the target being linted is the rule that *compiles* this module.
///
/// A package holding a library and a binary is linted twice, and the two passes
/// see different halves of it: the binary's closure holds the library's
/// ordinary sources but not its test sources, and the library's pass holds
/// both. Every other body rule can ignore that, because a finding met twice is
/// deduplicated — but this one carries an edit, and the first pass to report is
/// the one whose edit survives. So the rule that compiles a declaration is the
/// one that answers for it, which is also the only pass that can see every call
/// the fix has to rewrite. The boundary is the rule's rather than the
/// directory's (BUILD-FILES.md: "rules inside a package do not reach into each
/// other"), which is the question [`Workspace::rule_of_file`] answers.
///
/// A file no rule lists is `unused-library`'s business rather than this one's:
/// every pass that can see it answers, and the report deduplicates.
///
/// [`Workspace::rule_of_file`]: crate::build::workspace::Workspace::rule_of_file
fn compiled_by(
    session: &Session,
    analysis: &crate::compiler::driver::Analysis,
    target: TargetId,
    module: ModuleId,
) -> bool {
    let Some(m) = analysis.loaded.modules.get(module.index()) else { return false };
    let Some(rel) = package_relative(session, target.package, &m.path) else { return true };
    match session.workspace.rule_of_file(target.package, &rel) {
        Some(kind) => kind == target.kind,
        None => true,
    }
}

/// A module path (`//lib/money/cents.buri`) as its package's rule lists it
/// (`cents.buri`).
fn package_relative(session: &Session, package: PackageId, path: &str) -> Option<String> {
    let rest = path.strip_prefix("//")?;
    let dir = session.workspace.package(package).path.clone();
    if dir.is_empty() {
        return Some(rest.to_string());
    }
    rest.strip_prefix(&format!("{dir}/")).map(str::to_string)
}

/// The bytes that delete a `ctx` parameter and the argument at every call site.
///
/// Empty — the finding stands with the page's sentence and nothing to apply —
/// wherever this pass cannot see the whole of the change. `--fix` and an
/// editor's quick fix both write without asking again, so half a rewrite is the
/// one outcome worth refusing: it leaves a repository that does not compile and
/// no longer holds the finding that would explain why.
///
/// The three refusals are each a call site this analysis cannot reach:
///
/// * **the library publishes the name.** A caller may be in a package this
///   analysis never loaded. That is [`check_dead_code`]'s stance and it is here
///   for the same reason — the surface is the whole of what makes a name
///   reachable from outside ("if it is not named in `lib.buri`, it is not
///   reachable from outside the library") — with `testing/lib.buri` counted
///   beside it, because any test source anywhere may import that one.
/// * **something takes the function as a value.** An [`typed::ExprKind::FnRef`]
///   is the function at its `Ty::Fn`, which is the type whatever asked for it
///   declared; deleting the parameter changes that type rather than a call.
/// * **the package did not check whole.** A body that failed has lost the calls
///   written under the failure, and a run of declarations the parser skipped may
///   hold one, so the list of call sites is short by an unknown amount.
fn context_edits(
    session: &Session,
    analysis: &crate::compiler::driver::Analysis,
    unchecked: &Unchecked,
    target: TargetId,
    func: FnId,
    index: usize,
) -> Vec<crate::diagnostics::Edit> {
    let own = target.package;
    let info = analysis.checked.tables.fn_info(func);
    if published_by(session, analysis, own).contains(info.name.as_str()) {
        return Vec::new();
    }
    let mine = editable_modules_of(analysis, own);
    if mine.iter().any(|m| unchecked.module(*m)) {
        return Vec::new();
    }
    let Some(param) = info.params.get(index) else { return Vec::new() };
    let Some(declaration) = dropped_from_list(session, param.span) else { return Vec::new() };

    let mut edits = vec![declaration];
    let mut refused = false;
    for (fid, body) in &analysis.checked.bodies {
        if !mine.contains(&analysis.checked.tables.fn_info(*fid).module) {
            continue;
        }
        if unchecked.body(*fid) {
            return Vec::new();
        }
        typed::walk(&body.expr, &mut |e| match &e.kind {
            typed::ExprKind::FnRef(callee) if callee.decl() == Some(func) => refused = true,
            typed::ExprKind::CallFn { func: callee, args } if callee.decl() == Some(func) => {
                match args.get(index).and_then(|a| dropped_from_list(session, a.span)) {
                    Some(edit) => edits.push(edit),
                    None => refused = true,
                }
            }
            _ => {}
        });
    }
    if refused { Vec::new() } else { edits }
}

/// Every name the package publishes: its `lib.buri` surface, and the
/// `testing/lib.buri` beside it, which a test source anywhere may import.
fn published_by(
    session: &Session,
    analysis: &crate::compiler::driver::Analysis,
    own: PackageId,
) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = analysis
        .checked
        .surfaces
        .get(&own)
        .map(|names| names.iter().cloned().collect())
        .unwrap_or_default();
    let testing = session.workspace.package(own).module_path("testing/lib.buri");
    for m in &analysis.loaded.modules {
        if m.pkg != Some(own) || m.path != testing {
            continue;
        }
        for item in &m.ast.items {
            if let crate::parsing::tree::Item::ReExport(r) = item {
                out.extend(r.specs.iter().map(|sp| m.ast.tree.name(sp.name).to_string()));
            }
        }
    }
    out
}

/// One element of a comma-separated list, and the separator that goes with it.
///
/// Deleting `ctx: C` on its own leaves `fn f(, a: Int)`, so the separator is
/// half of the edit — and which separator it is depends on where in the list
/// the element sits: the comma *after* it when there is one, the comma *before*
/// it when the element is last, and neither when it is the only element. The
/// whitespace past a trailing comma goes with it, which is what makes one rule
/// right both for a list written on one line and for a list written down a
/// column.
///
/// The answer is read out of the text rather than off the neighbouring spans,
/// and the reason is the receiver: in `x.f(ctx)` the parameter before `ctx` is
/// `self`, whose span is `x` — three tokens back from the comma this would have
/// to find, with the method's name in between. The text has the punctuation in
/// it and the span table does not.
///
/// `None` when what sits between the element and its separator is anything
/// other than whitespace. That is a comment, near enough always, and this
/// deletes text rather than reformatting it: the one thing it must not do is
/// take a reader's sentence out with the parameter it was written about.
fn dropped_from_list(session: &Session, at: Span) -> Option<crate::diagnostics::Edit> {
    let text = session.map.text(at.file);
    let (start, end) = (at.start as usize, at.end as usize);
    let after = text.get(end..)?;
    let ahead = after.len() - after.trim_start().len();
    let next = after.get(ahead..)?.chars().next()?;
    if next == ',' {
        let past = after.get(ahead + 1..)?;
        let gap = past.len() - past.trim_start().len();
        return Some(deletion(at, start, end + ahead + 1 + gap));
    }
    if next != ')' {
        return None;
    }
    let behind = text.get(..start)?.trim_end();
    if behind.ends_with(',') {
        return Some(deletion(at, behind.len() - 1, end));
    }
    if behind.ends_with('(') {
        return Some(deletion(at, start, end));
    }
    None
}

/// A deletion of `from..to` in the file `at` names.
fn deletion(at: Span, from: usize, to: usize) -> crate::diagnostics::Edit {
    crate::diagnostics::Edit { at: Span::new(at.file, from, to), replacement: String::new() }
}

/// `unused-context-bound`. A `ctx` parameter says a function touches the
/// world; its bounds say which parts of it. A bound the body never exercises
/// is a demand on every caller for a capability the code does not use — and it
/// spreads, because a caller's own signature has to carry the bound to satisfy
/// it.
///
/// **Three uses, and the reason there is no fourth.** The checker consults a
/// type parameter's bound list in exactly two places, and both of them are a
/// call that is written down in the checked tree:
///
/// * `expressions.rs`'s `resolve_method` at a `Ty::Param` receiver — a method
///   on a type parameter can only come from its bounds — which becomes a
///   [`typed::ExprKind::CallTrait`] whose `recv` is that parameter and whose
///   `trait_id` is the bound. That is `ctx.println(…)`.
/// * `inference.rs`'s `satisfies` at a `Ty::Param`, reached from the
///   obligations `expressions.rs`'s `instantiate` raises. `instantiate` has
///   three callers — `fn_ref`, `call_fn` and `call_trait_method` — and each
///   writes the type arguments it produced into the node it built, so a
///   callee's bound landing on `C` is a `targs` entry that mentions
///   `Ty::Param(i)`. That is `str.format(ctx, …)`.
///
/// The third use in the note's list is not a bound demand at all, which is why
/// it has to be stated rather than derived: handing the context to a
/// **function-typed parameter** ([`typed::ExprKind::CallValue`]) gives the
/// whole of `C` to code this function cannot see, so it uses *every* bound.
/// The callback was written against `C` as declared and nothing here can say
/// which parts of it the callback reaches.
///
/// A struct or enum literal carries type arguments too, and nothing today
/// raises an obligation for them — a `TyCon`'s generics are never instantiated
/// — but they are read anyway, so that the rule does not quietly depend on
/// that staying true.
///
/// **What is not asked**, in the order the loop below asks it.
///
/// Three of the five are [`check_unused_contexts`]'s, for its reasons: a
/// declaration with no body (there is no entry in `checked.bodies` at all), an
/// `impl` method (whose generics must match the trait's bound for bound, so a
/// dead bound there is dead on the *trait*), and a `ctx` whose type is not a
/// type parameter, which has no bound list to trim. The scope is narrowed once
/// more, to a parameter carrying at least one `effect`: that is what makes it
/// a context, and `T: Eq` on ordinary data is a different question with a
/// different answer.
///
/// The fourth is the one place this rule is *less* able than that one: **a
/// body that did not check is not asked**. A context has exactly one spelling
/// and a bound has none — it is used through method names it declares and
/// through callees whose own bounds name it — so where the typed tree is
/// truncated there is nothing for the lexer to answer with.
///
/// The fifth is a division of labour rather than a doubt: **a context the body
/// never reads at all is [`check_unused_contexts`]'s, and only its.** Every
/// bound on such a parameter is dead by construction, so reporting them here
/// would answer one mistake with one finding plus one per bound — and the edit
/// they carry is the wrong one, because what that signature needs is the
/// parameter taken out rather than its bounds trimmed.
fn check_unused_context_bounds(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    unchecked: &Unchecked,
    diagnostics: &mut Diagnostics,
) {
    use crate::compiler::semantics::types::{ParamRole, Ty};
    let tables = &analysis.checked.tables;
    let mine = editable_modules_of(analysis, target.package);
    let mut found: Vec<(Span, String, String, Vec<crate::diagnostics::Edit>)> = Vec::new();
    for (fid, body) in &analysis.checked.bodies {
        let info = tables.fn_info(*fid);
        if !mine.contains(&info.module) || info.impl_of.is_some() || unchecked.body(*fid) {
            continue;
        }
        let Some(index) = info.params.iter().position(|p| p.role == ParamRole::Ctx) else {
            continue;
        };
        let Some(Ty::Param(gi)) = info.params.get(index).map(|p| &p.ty) else { continue };
        let Some(generic) = info.generics.get(*gi as usize) else { continue };
        if !generic.bounds.iter().any(|b| tables.trait_(*b).is_effect) {
            continue;
        }
        if !body.params.get(index).copied().is_some_and(|local| reads_ctx(body, local)) {
            continue;
        }
        let used = bounds_used(analysis, body, *gi, &generic.bounds);
        let unused: Vec<usize> = generic
            .bounds
            .iter()
            .enumerate()
            .filter(|(_, b)| !used.contains(b))
            .map(|(k, _)| k)
            .collect();
        if unused.is_empty() {
            continue;
        }
        // The spans come from the declaring module's own syntax tree, so a
        // finding this rule makes always points at a file in `mine` — which is
        // what keeps it out of `core/…`, where a span would defeat the lint
        // cache for the target rather than merely misplace the caret.
        let Some(spans) = bound_spans(analysis, info, *gi as usize) else { continue };
        if spans.len() != generic.bounds.len() {
            continue;
        }
        let edits = dropped_bounds(session, &spans, &unused);
        for k in unused {
            let (Some(span), Some(bound)) = (spans.get(k).copied(), generic.bounds.get(k)) else {
                continue;
            };
            let bound = tables.trait_(*bound).name.clone();
            found.push((span, bound, generic.name.clone(), edits.clone()));
        }
    }
    // `bodies` is a map, so the order findings are met in is not the order they
    // are written in.
    found.sort_by_key(|(span, _, _, _)| (span.file.0, span.start));
    for (span, bound, param, edits) in found {
        let mut d = Diagnostic::templated("unused-context-bound", span)
            .with_bind("bound", bound)
            .with_bind("param", param);
        for e in edits {
            d = d.with_edit(e.at, &e.replacement);
        }
        diagnostics.push(d);
    }
}

/// Whether anything in the body names the context parameter's local.
///
/// The question [`check_unused_contexts`] asks, asked again here for the
/// opposite reason: that rule reports a `no`, and this one has nothing to say
/// about one. It is the tree alone, with no fallback to the text, because the
/// only caller has already declined to speak for a body that did not check.
fn reads_ctx(body: &typed::Body, ctx_local: crate::compiler::semantics::types::LocalId) -> bool {
    let mut read = false;
    typed::walk(&body.expr, &mut |e| {
        if matches!(&e.kind, typed::ExprKind::Local(l) if *l == ctx_local) {
            read = true;
        }
    });
    read
}

/// Which of a context parameter's bounds the body exercises.
///
/// The three arms are the note's three uses, in the same order, and each is
/// read off the node the checker wrote its own answer into — see
/// [`check_unused_context_bounds`] for why there is no fourth.
fn bounds_used(
    analysis: &crate::compiler::driver::Analysis,
    body: &typed::Body,
    gi: u32,
    bounds: &[crate::compiler::semantics::types::TraitId],
) -> BTreeSet<crate::compiler::semantics::types::TraitId> {
    use crate::compiler::semantics::types::Ty;
    let tables = &analysis.checked.tables;
    let mut used = BTreeSet::new();
    typed::walk(&body.expr, &mut |e| match &e.kind {
        // A method the bound declares, called on the context.
        typed::ExprKind::CallTrait { trait_id, method, recv, targs, .. } => {
            if matches!(recv, Ty::Param(i) if *i == gi) {
                used.insert(*trait_id);
            }
            if let Some(m) = tables.trait_(*trait_id).methods.get(*method) {
                note_targs(&m.generics, targs, gi, &mut used);
            }
        }
        // A callee that asks for a context of its own, and a callee taken as a
        // value, which instantiates its generics in the same way.
        typed::ExprKind::CallFn { func, .. } | typed::ExprKind::FnRef(func) => {
            if let typed::Callee::Decl { id, targs } = func {
                note_targs(&tables.fn_info(*id).generics, targs, gi, &mut used);
            }
        }
        // A function-typed parameter, which uses every bound.
        typed::ExprKind::CallValue { args, .. } => {
            if args.iter().any(|a| matches!(&a.ty, Ty::Param(i) if *i == gi)) {
                used.extend(bounds.iter().copied());
            }
        }
        typed::ExprKind::StructLit { con, targs, .. }
        | typed::ExprKind::EnumLit { con, targs, .. } => {
            note_targs(&tables.tycon(*con).generics, targs, gi, &mut used);
        }
        _ => {}
    });
    used
}

/// Every bound a generic item's own list demands of `Ty::Param(gi)`, at one
/// instantiation of it.
///
/// A type argument that merely *mentions* the parameter counts, because a
/// derived implementation is a fold over the type's components: `satisfies`
/// answers `Wrapper<C>: Show` by asking `C: Show`. Reading the mention rather
/// than the whole argument is the conservative direction — it can only hold a
/// bound alive that nothing needed, never delete one that something did.
fn note_targs(
    generics: &[crate::compiler::semantics::types::GenericInfo],
    targs: &[crate::compiler::semantics::types::Ty],
    gi: u32,
    used: &mut BTreeSet<crate::compiler::semantics::types::TraitId>,
) {
    for (g, t) in generics.iter().zip(targs) {
        if mentions_param(t, gi) {
            used.extend(g.bounds.iter().copied());
        }
    }
}

fn mentions_param(t: &crate::compiler::semantics::types::Ty, gi: u32) -> bool {
    use crate::compiler::semantics::types::Ty;
    match t {
        Ty::Param(i) => *i == gi,
        Ty::Con(_, args) | Ty::Tuple(args) => args.iter().any(|a| mentions_param(a, gi)),
        Ty::Array(e) => mentions_param(e, gi),
        Ty::Fn(params, ret) => {
            params.iter().any(|p| mentions_param(p, gi)) || mentions_param(ret, gi)
        }
        _ => false,
    }
}

/// The span of each bound a type parameter was written with, in order.
///
/// Read from the declaring module's syntax tree, because `GenericInfo` keeps
/// the resolved [`TraitId`]s and the span of the parameter as a whole — the
/// text `Fs` sits at is only in the tree the parser built.
///
/// [`TraitId`]: crate::compiler::semantics::types::TraitId
fn bound_spans(
    analysis: &crate::compiler::driver::Analysis,
    info: &crate::compiler::semantics::types::FnInfo,
    gi: usize,
) -> Option<Vec<Span>> {
    let (module, item) = info.ast.item()?;
    let m = analysis.loaded.modules.get(module.index())?;
    let crate::parsing::tree::Item::Fn(decl) = m.ast.items.get(item as usize)? else {
        return None;
    };
    let g = decl.generics.get(gi)?;
    let t = &m.ast.tree;
    Some(t.type_list(g.bounds).iter().map(|b| t.type_span(*b)).collect())
}

/// The bytes that take a set of bounds out of one type parameter's list.
///
/// **One rewrite, not one per bound**, and the separators are why: deleting
/// `Fs` from `<C: Alloc + Fs + Io>` has to take a `+` with it, and so does
/// deleting `Io` — the same `+`. Two findings whose edits both claim it are
/// refused as overlapping and neither is applied, so the whole removal is
/// computed once here and every finding about the parameter carries it.
///
/// The list is walked as runs of adjacent bounds. A run that starts after the
/// first bound takes the separator *before* it (`" + Fs + Io"`); a run that
/// starts at the first and does not reach the last takes the separator *after*
/// (`"Fs + Io + "`); a run that is the whole list takes the colon too, leaving
/// `<C>`. Runs are separated by a bound that stays, so no two of these ranges
/// meet.
///
/// Nothing but the separators may be swallowed. What else sits inside a bound
/// list is a comment — a reader's sentence about the code — and this deletes
/// text rather than reformatting it, so it refuses the whole rewrite instead.
fn dropped_bounds(
    session: &Session,
    spans: &[Span],
    removed: &[usize],
) -> Vec<crate::diagnostics::Edit> {
    let Some(first) = spans.first() else { return Vec::new() };
    let text = session.map.text(first.file);
    let mut edits = Vec::new();
    let mut rest: &[usize] = removed;
    while let Some((&a, after)) = rest.split_first() {
        // The run: this bound and every one adjacent to it, which is the group
        // whose separators are shared and so cannot be deleted independently.
        let mut b = a;
        rest = after;
        while let Some((&next, more)) = rest.split_first() {
            if next != b + 1 {
                break;
            }
            b = next;
            rest = more;
        }
        let (Some(head), Some(tail)) = (spans.get(a), spans.get(b)) else { return Vec::new() };
        let (from, to) = if a > 0 {
            match spans.get(a - 1) {
                Some(before) => (before.end as usize, tail.end as usize),
                None => return Vec::new(),
            }
        } else if let Some(after) = spans.get(b + 1) {
            (head.start as usize, after.start as usize)
        } else {
            // Every bound goes, so the `:` that introduced them goes as well.
            let Some(before) = text.get(..head.start as usize) else { return Vec::new() };
            let Some(colon) = before.trim_end().strip_suffix(':') else { return Vec::new() };
            (colon.trim_end().len(), tail.end as usize)
        };
        if !only_separators(text, from, to, spans) {
            return Vec::new();
        }
        edits.push(deletion(*first, from, to));
    }
    edits
}

/// Whether `from..to` holds nothing but bounds, the punctuation that joins
/// them, and space.
fn only_separators(text: &str, from: usize, to: usize, spans: &[Span]) -> bool {
    let Some(region) = text.get(from..to) else { return false };
    region.char_indices().all(|(offset, c)| {
        let at = from + offset;
        spans.iter().any(|s| at >= s.start as usize && at < s.end as usize)
            || c.is_whitespace()
            || c == '+'
            || c == ':'
    })
}

/// `discarded-result`. `let _ = <Result>` is already a hard type error
/// (`result-discarded`), so the only way a `Result` is dropped on purpose is
/// `core/result.ignore` — the greppable escape hatch. This is the grep,
/// promoted to a warning so it appears in a report rather than only when
/// somebody thinks to look.
///
/// **A print is the exception, and it is the exception that keeps the rule
/// usable.** `Stdout` and `Stderr` answer `Result<(), IoError>` because a
/// closed pipe is a thing that happens, so almost every line a program writes
/// is now a `Result` — and the failure a print reports is the one a program
/// can least act on, because the stream it would report the failure *on* is
/// the stream that failed. `io.println(ctx, x).ignore()` is therefore
/// punctuation rather than a decision, and a report dominated by it is a
/// report nobody reads. `fs.writeText(...).ignore()` is still reported: a
/// failed write is a failure somebody chose not to look at.
fn check_discarded_results(
    own: PackageId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    let mine = modules_of(analysis, own);
    let mut spans = Vec::new();
    for (fid, body) in &analysis.checked.bodies {
        if !mine.contains(&analysis.checked.tables.fn_info(*fid).module) {
            continue;
        }
        crate::compiler::semantics::typed::walk(&body.expr, &mut |e| {
            let typed::ExprKind::CallFn { func, args } = &e.kind else { return };
            let Some(called) = func.decl() else { return };
            if !names_function(analysis, called, "core/result", "ignore") {
                return;
            }
            // The receiver is `ignore`'s `self`, so a dropped print is a call
            // into `core/io` sitting in the first argument.
            if args.first().is_some_and(|recv| calls_core_io(analysis, recv)) {
                return;
            }
            spans.push(e.span);
        });
    }
    spans.sort_by_key(|s| (s.file.0, s.start));
    for span in spans {
        diagnostics.push(Diagnostic::templated("discarded-result", span));
    }
}

/// Whether `fid` is the named function of the named module.
fn names_function(
    analysis: &crate::compiler::driver::Analysis,
    fid: FnId,
    module: &str,
    name: &str,
) -> bool {
    let info = analysis.checked.tables.fn_info(fid);
    if info.name != name {
        return false;
    }
    analysis.loaded.modules.get(info.module.index()).is_some_and(|m| m.path == module)
}

/// Whether this expression is a call to one of `core/io`'s functions — the
/// receiver shape [`check_discarded_results`] exempts.
fn calls_core_io(analysis: &crate::compiler::driver::Analysis, e: &typed::Expr) -> bool {
    let typed::ExprKind::CallFn { func, .. } = &e.kind else { return false };
    let Some(called) = func.decl() else { return false };
    let info = analysis.checked.tables.fn_info(called);
    analysis.loaded.modules.get(info.module.index()).is_some_and(|m| m.path == "core/io")
}

/// `test-without-assertion`. Read syntactically — "the body contains no
/// `assert`" — this fires on every test that asserts through a helper, which
/// is most of the ones worth writing. So it is transitive: a test passes if
/// anything reachable from it calls into `core/testing/assert`.
fn check_tests_assert(
    own: PackageId,
    analysis: &crate::compiler::driver::Analysis,
    unchecked: &Unchecked,
    diagnostics: &mut Diagnostics,
) {
    let mine = modules_of(analysis, own);

    let asserts = |f: FnId| -> bool {
        let info = analysis.checked.tables.fn_info(f);
        analysis
            .loaded
            .modules
            .get(info.module.index())
            .is_some_and(|m| m.path == "core/testing/assert")
    };

    for case in &analysis.checked.tests {
        if !mine.contains(&case.module) {
            continue;
        }
        // Reachability from the test's body. The set is small — a test's
        // closure is its helpers — so a plain worklist is the whole algorithm.
        let mut seen: BTreeSet<FnId> = BTreeSet::new();
        let mut queue = vec![case.func];
        let mut found = false;
        // A body that did not check has lost whatever it called, so what is
        // reachable from it is not something this can claim to know.
        let mut unreadable = false;
        while let Some(f) = queue.pop() {
            if !seen.insert(f) {
                continue;
            }
            if unchecked.body(f) {
                unreadable = true;
                break;
            }
            if asserts(f) {
                found = true;
                break;
            }
            let Some(body) = analysis.checked.bodies.get(&f) else { continue };
            crate::compiler::semantics::typed::walk(&body.expr, &mut |e| {
                match &e.kind {
                    typed::ExprKind::CallFn { func, .. } | typed::ExprKind::FnRef(func) => {
                        queue.extend(func.decl())
                    }
                    _ => {}
                };
            });
        }
        if found || unreadable {
            continue;
        }
        diagnostics.push(
            Diagnostic::templated("test-without-assertion", case.span)
                .with_bind("quoted_title", format!("{:?}", case.name)),
        );
    }
}

/// Every library a package's own code reaches through a resolved call, which
/// the tool can compute because resolution is a single lookup.
pub(crate) fn reached_by_resolution(
    session: &Session,
    analysis: &crate::compiler::driver::Analysis,
    own: PackageId,
) -> BTreeSet<String> {
    let mine = modules_of(analysis, own);
    let mut out = BTreeSet::new();
    for (fid, body) in &analysis.checked.bodies {
        let info = analysis.checked.tables.fn_info(*fid);
        if !mine.contains(&info.module) {
            continue;
        }
        crate::compiler::semantics::typed::walk(&body.expr, &mut |e| {
            let called = match &e.kind {
                typed::ExprKind::CallFn { func, .. } | typed::ExprKind::FnRef(func) => func.decl(),
                _ => None,
            };
            let Some(f) = called else { return };
            let callee = analysis.checked.tables.fn_info(f);
            let Some(m) = analysis.loaded.modules.get(callee.module.index()) else { return };
            let Some(package) = m.pkg else { return };
            if package == own {
                return;
            }
            let label = session.workspace.package(package).label();
            out.insert(if crate::build::workspace::is_test_only_path(&m.path) {
                format!("{label}/testing")
            } else {
                label
            });
        });
    }
    out
}

fn in_test_deps(session: &Session, target: TargetId, label: &str) -> bool {
    let p = session.workspace.package(target.package);
    let suite = p.test_suite(target.kind);
    let testing = p.build.library.as_ref().and_then(|l| l.testing.as_ref());
    suite.is_some_and(|t| t.dependencies.iter().any(|d| d.value == label))
        || testing.is_some_and(|t| t.dependencies.iter().any(|d| d.value == label))
}

/// `dep-cycle`: package cycles are the module rule one level up.
///
/// One diagnostic per cycle, not one per edge in it. A cycle has no first end,
/// so reporting it from each is the same finding written twice — and
/// BUILD-FILES.md:389-390 describes one. The cycle's *members* are its
/// identity: every target mutually reachable with this edge's tail is in it,
/// and that set is the same set whichever edge of the cycle is walked first.
/// So the set is what deduplicates, and the first edge to reach it is the one
/// the diagnostic points at.
fn check_cycles(session: &Session, diagnostics: &mut Diagnostics) {
    let mut reported: BTreeSet<Vec<TargetId>> = BTreeSet::new();
    for t in session.workspace.targets() {
        for (dep, span) in session.workspace.dep_edges(t) {
            if dep == t {
                continue;
            }
            if !session.workspace.closure(dep).contains(&t) {
                continue;
            }
            let mut members: Vec<TargetId> = session
                .workspace
                .closure(t)
                .into_iter()
                .filter(|m| session.workspace.closure(*m).contains(&t))
                .collect();
            members.sort();
            if !reported.insert(members) {
                continue;
            }
            let a = session.workspace.label(t);
            let b = session.workspace.label(dep);
            diagnostics.push(
                Diagnostic::templated(
                    "dep-cycle",
                    span.unwrap_or(Span::point(
                        session.workspace.package(t.package).build_file_id,
                        0,
                    )),
                )
                .with_bind("target", a)
                .with_bind("other", b),
            );
        }
    }
}
