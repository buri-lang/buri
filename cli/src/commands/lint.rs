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

/// Applies every finding that carries a byte edit, and returns how many.
///
/// Per file, descending by offset so earlier edits keep their offsets, and
/// **refusing the whole file** on any overlap rather than guessing which of two
/// answers was meant. The result is run through `formatting::source`, which returns
/// `None` for anything that does not parse — the same guard the formatter uses
/// on itself — and a file that fails it is left exactly as it was.
fn apply_fixes(session: &mut Session, diagnostics: &Diagnostics) -> usize {
    use std::collections::BTreeMap;
    let mut by_file: BTreeMap<u32, Vec<crate::diagnostics::Edit>> = BTreeMap::new();
    for d in &diagnostics.items {
        for e in &d.edits {
            by_file.entry(e.at.file.0).or_default().push(e.clone());
        }
    }

    let mut applied = regenerate_build_files(session, diagnostics);
    for (file, edits) in by_file {
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
        applied += edits.len();
    }
    applied
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
/// unreadable when the *parser skipped a declaration* in it, because then the
/// list of what the module imports, declares and re-exports is short by an
/// unknown amount — and that list is what [`check_dead_code`] reads.
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
    /// Modules the parser did not read whole — an [`Item::Error`] among their
    /// items is a run of declarations it skipped over. What such a module
    /// imports and exports is not the whole account of it, so a rule that
    /// reads one module's imports to justify another module's export cannot
    /// run at all.
    ///
    /// An error that leaves the item list intact is not here, however far from
    /// a body it sits: an alias that closes a cycle, an import missing its
    /// `;`, a field whose type did not check. Each of those is one declaration
    /// the reader can see is wrong, and the rest of the module is as legible
    /// as it was before.
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

        // A module the parser did not read whole. `Item::Error` is the only
        // thing that says so: it is what the parser writes down where it gave
        // up on a declaration and skipped to the next one, so it is exactly
        // the run of source nothing in the tree accounts for. An error beside
        // an item the parser read is about that item, and the item list around
        // it is still the whole list.
        //
        // Only files something is already wrong with are looked at, which is
        // what keeps this a lookup per module rather than a walk of every item
        // in the standard library.
        for m in &analysis.loaded.modules {
            if !errors.contains_key(&m.file) {
                continue;
            }
            if m.ast.items.iter().any(|i| matches!(i, crate::parsing::tree::Item::Error(_))) {
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
    check_ctx_rebindings(own, analysis, &unchecked, diagnostics);
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

/// `discarded-result`. `let _ = <Result>` is already a hard type error
/// (`result-discarded`), so the only way a `Result` is dropped on purpose is
/// `core/result.ignore` — the greppable escape hatch. This is the grep,
/// promoted to a warning so it appears in a report rather than only when
/// somebody thinks to look.
fn check_discarded_results(
    own: PackageId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    for (span, _) in calls_into(analysis, own, "core/result", &["ignore"]) {
        diagnostics.push(Diagnostic::templated("discarded-result", span));
    }
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

/// Every call site in `own`'s code that lands on one of `names` in `module`.
fn calls_into(
    analysis: &crate::compiler::driver::Analysis,
    own: PackageId,
    module: &str,
    names: &[&str],
) -> Vec<(Span, String)> {
    let mine = modules_of(analysis, own);
    let mut out = Vec::new();
    for (fid, body) in &analysis.checked.bodies {
        if !mine.contains(&analysis.checked.tables.fn_info(*fid).module) {
            continue;
        }
        crate::compiler::semantics::typed::walk(&body.expr, &mut |e| {
            let called = match &e.kind {
                typed::ExprKind::CallFn { func, .. } => func.decl(),
                _ => return,
            };
            let Some(called) = called else { return };
            let info = analysis.checked.tables.fn_info(called);
            let Some(m) = analysis.loaded.modules.get(info.module.index()) else { return };
            if m.path == module && names.contains(&info.name.as_str()) {
                out.push((e.span, info.name.clone()));
            }
        });
    }
    out.sort_by_key(|(session, _)| (session.file.0, session.start));
    out
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
