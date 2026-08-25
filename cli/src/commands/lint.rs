//! `buri lint`.
//!
//! Checks that type checking does not cover. None of this is configurable:
//! there is no `lint` block in `REPO.buri`, no per-file suppression comment,
//! and no way to promote or silence a check for one repository. A lint that
//! cannot be turned off has to be one nobody wants to turn off, which is the
//! bar every check here is held to.
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
use crate::diagnostics::{Diagnostic, Diagnostics, Invariant as _, Span};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Checks that type checking does not cover. None of this is configurable:
/// there is no `lint` block in `REPO.buri`, no per-file suppression comment,
/// and no way to promote or silence a check for one repository.
pub fn cmd_lint(args: &arguments::Args) -> i32 {
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

    let diagnostics = findings_for(&mut session, &targets);
    Ok((session, diagnostics))
}

/// Every check `buri lint` runs, over the targets given.
///
/// Public because the language server reports the same findings the command
/// does — an editor that showed only type errors would be showing half of what
/// the toolchain knows, and the half that is easier to notice at the terminal.
pub fn findings_for(session: &mut Session, targets: &[TargetId]) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    let mut seen_packages = BTreeSet::new();
    for target in targets {
        if seen_packages.insert(target.package) {
            check_sources_declared(session, target.package, &mut diagnostics);
            check_test_suites(session, target.package, &mut diagnostics);
        }
        // `lint` is not building, so it checks a binary against the platforms
        // its own `outputs` name, and a library against the question TAGS.md
        // asks at the target: can it be built at all?
        crate::build::actions::check_visibility(session, *target, &mut diagnostics);
        crate::build::actions::check_tags(session, *target, &mut diagnostics);
        check_target_platforms(session, *target, &mut diagnostics);
        check_dependencies(session, *target, &mut diagnostics);
    }
    check_cycles(session, &mut diagnostics);

    diagnostics.sort(&session.map);
    diagnostics
}

fn report_findings(session: &mut Session, diagnostics: &Diagnostics) -> i32 {
    let mut errors = 0;
    let mut warnings = 0;
    for d in &diagnostics.items {
        session.emit(d);
        if d.is_error() {
            errors += 1;
        } else {
            warnings += 1;
        }
    }
    if errors == 0 && warnings == 0 {
        println!("no findings");
    }
    if errors > 0 {
        1
    } else {
        0
    }
}

/// The four findings whose answer is a build file that describes the code.
///
/// These are never byte-edited. `buri gen` already writes exactly this file,
/// preserving `tags`, `visibility`, `outputs` and comments, so calling it is
/// the only way `lint --fix` and `gen` cannot end up disagreeing about what a
/// package's `BUILD.buri` should say.
const REGENERABLE: &[&str] =
    &["missing-dep", "unused-dep", "undeclared-source", "duplicate-source"];

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

/// `platform-violation`. An unsatisfiable target is an error at the target
/// itself, before any binary asks for it: otherwise the mistake surfaces as a
/// confusing failure in whichever binary happens to reach it first.
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
        diagnostics.push(
            Diagnostic::error(span, format!("{label} can never be built"))
                .with_code("platform-violation")
                .with_fix("widen a tag's `requires { platforms }` in REPO.buri, or drop the dependency that narrows it to nothing")
                .with_note("its dependency closure admits no platform at all"),
        );
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
            Diagnostic::warning(
                suite.span,
                format!("this {rule}'s `test` block declares no sources"),
            )
            .with_code("empty-test-suite")
            .with_fix("list the suite's files in `test { sources }`, or drop the empty block")
            .with_note("an empty suite reads as coverage to anything that walks the build graph"),
        );
    };
    if let Some(l) = &p.build.library {
        report(l.test.as_ref(), "library");
    }
    if let Some(b) = &p.build.binary {
        report(b.test.as_ref(), "binary");
    }
}

/// `undeclared-source` and `duplicate-source`: every `.buri` file in a package
/// must appear in exactly one rule. A file that appears in none can be dropped
/// from the build by a typo and never noticed.
fn check_sources_declared(session: &Session, package: PackageId, diagnostics: &mut Diagnostics) {
    let p = session.workspace.package(package);
    let mut declared: Vec<(String, Span)> = Vec::new();
    let push = |list: &[crate::build::buildfile::Sp<String>], out: &mut Vec<(String, Span)>| {
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
                    Diagnostic::error(*span, format!("{name} is listed by two rules"))
                        .with_code("duplicate-source")
                        .with_fix("list it under one rule only")
                        .with_sub(*first_span, "first listed here"),
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
            Diagnostic::error(
                Span::point(p.build_file_id, 0),
                format!("{}/{rel} is not declared by any rule", p.path),
            )
            .with_code("undeclared-source")
            .with_fix(format!(
                "add it to a rule's `{field}`, or delete it — `buri gen //{}` does this \
                 automatically",
                p.path
            )),
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

/// `missing-dep` and `unused-dep`. Use is what requires a dep, and an import is
/// not the only way to use: a method resolving into a library counts too.
fn check_dependencies(session: &mut Session, target: TargetId, diagnostics: &mut Diagnostics) {
    // A lint is not a build, so it does not refuse a program for an output it
    // was not asked about. See `Unit::platform`.
    let unit = Unit { target: Some(target), platform: None, with_tests: true };
    let analysis = crate::compiler::driver::analyze(
        Some(&session.workspace),
        &mut session.map,
        &mut session.parsed,
        &unit,
    );
    if analysis.diags.has_errors() {
        diagnostics.extend(analysis.diags.items);
        return;
    }

    // The hygiene rules ask about the same modules this analysis already
    // loaded, so they ride along rather than paying for a second one.
    check_hygiene(session, target, &analysis, diagnostics);

    let declared: Vec<crate::build::buildfile::Sp<String>> =
        session.workspace.declared_deps(target).to_vec();
    let own = target.package;
    // Use is what requires a dep, and an import is not the only way to use: a
    // method resolves through its receiver's type rather than through scope,
    // so a call that lands in another library counts even though no import
    // names it (BUILD-FILES.md, "Dependencies").
    let resolved: BTreeSet<String> = reached_by_resolution(session, &analysis, own);
    let mut used: BTreeSet<String> = resolved.clone();
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
            used.insert(wanted.clone());
            if !declared.iter().any(|d| d.value == wanted)
                && !in_test_deps(session, target, &wanted)
            {
                let importer = session.map.name(m.file).to_string();
                let package_path = session.workspace.package(own).path.clone();
                reported.insert(wanted.clone());
                diagnostics.push(
                    Diagnostic::error(
                        span,
                        format!("{importer} imports {wanted}, which is not in dependencies"),
                    )
                    .with_code("missing-dep")
                    .with_fix(format!(
                        "add \"{wanted}\" to dependencies in {package_path}/BUILD.buri — \
                         `buri gen //{package_path}` does this automatically"
                    )),
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
            Diagnostic::error(
                Span::point(session.workspace.package(own).build_file_id, 0),
                format!("{own_label} uses {wanted}, which is not in dependencies"),
            )
            .with_code("missing-dep")
            .with_note(
                "a method resolves through its receiver's type rather than through scope, \
                 so no import names it",
            )
            .with_fix(format!(
                "add \"{wanted}\" to dependencies in {package_path}/BUILD.buri — \
                 `buri gen //{package_path}` does this automatically"
            )),
        );
    }

    for d in &declared {
        if !used.contains(&d.value) {
            diagnostics.push(
                Diagnostic::error(d.span, format!("{} is declared but nothing uses it", d.value))
                    .with_code("unused-dep")
                    .with_fix("remove it from `dependencies`")
                    .with_note("a dependencies entry no source uses makes the graph a description of something other than the code"),
            );
        }
    }
}

/// The hygiene rules: `unused-import`, `discarded-result`, and
/// `test-without-assertion`. All three ask about a package's own code rather
/// than about the build graph, so they share the analysis `check_dependencies`
/// has already paid for.
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

fn check_hygiene(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    let own = target.package;
    for m in &analysis.loaded.modules {
        if m.pkg == Some(own) && !is_generated(&m.path) {
            check_unused_imports(session, m, diagnostics);
        }
    }
    check_unreachable_exports(session, target, analysis, diagnostics);
    check_discarded_results(own, analysis, diagnostics);
    check_tests_assert(own, analysis, diagnostics);
    check_test_titles(own, analysis, diagnostics);
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
            Diagnostic::warning(case.span, format!("test {:?} has a newline in its title", case.name))
                .with_code("test-title-newline")
                .with_fix("write the title on one line; a sentence is enough, and the body says the rest")
                .with_note(
                    "a failure report is one line per test, so the runner prints the title \
                     escaped — the break shows up as `\\n` rather than as a line break",
                ),
        );
    }
}

/// `unreachable-export`. Inside a library, `export` means "visible to the rest
/// of this library"; `lib.buri` decides what leaves it. So an `export` that
/// `lib.buri` does not re-export and no sibling module imports is visible to
/// nobody — the word is there, and it does nothing.
///
/// Only module-level items. A field's or a variant's `export` is about the
/// shape of a type, and asking whether it is "reached" is a different question.
fn check_unreachable_exports(
    session: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    // A binary has no surface — nothing may import its modules — so the rule
    // does not apply to one.
    if target.kind != RuleKind::Library {
        return;
    }
    let own = target.package;
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
                Diagnostic::error(span, format!("`{name}` is exported and reaches nobody"))
                    .with_code("unreachable-export")
                    .with_fix(format!(
                        "re-export it from {lib} to put it on the library's surface, or drop the \
                         `export`"
                    ))
                    .with_note(
                        "inside a library `export` means visible to the rest of the library, and \
                         `lib.buri` decides what leaves it",
                    ),
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
        Item::Const(d) => (d.exported, d.name),
        Item::Trait(d) => (d.exported, d.name),
        _ => return None,
    };
    exported.then_some((t.name(name), name.span))
}

/// `unused-import`. Deliberately syntactic: a name counts as used if it appears
/// as an identifier token anywhere outside the import statements themselves.
///
/// That over-approximates use, which is the safe direction for a rule at error
/// severity — a shadowed binding or a field with the same spelling silences the
/// finding rather than producing a wrong one. Reading tokens rather than the
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
        if lexed.tokens.kind(i) != crate::parsing::lexer::TokKind::Ident {
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
            let mut d = Diagnostic::error(*span, format!("`{name}` is imported and never used"))
                .with_code("unused-import")
                .with_fix(format!("remove `{name}` from the import"))
                .with_note(
                    "an import that names something the module does not use makes the \
                     dependency graph describe something other than the code",
                );
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
        diagnostics.push(
            Diagnostic::warning(span, "this discards a `Result`")
                .with_code("discarded-result")
                .with_fix(
                    "handle the error with `match`, propagate it with `?`, or keep `ignore` if \
                     dropping it is deliberate",
                )
                .with_note(
                    "`ignore` is the one way to drop a `Result`, so every place a failure is \
                     deliberately unhandled is one of these",
                ),
        );
    }
}

/// `test-without-assertion`. Read syntactically — "the body contains no
/// `assert`" — this fires on every test that asserts through a helper, which
/// is most of the ones worth writing. So it is transitive: a test passes if
/// anything reachable from it calls into `core/testing/assert`.
fn check_tests_assert(
    own: PackageId,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) {
    use crate::compiler::semantics::types::FnId;
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
        while let Some(f) = queue.pop() {
            if !seen.insert(f) {
                continue;
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
        if found {
            continue;
        }
        diagnostics.push(
            Diagnostic::warning(case.span, format!("test {:?} asserts nothing", case.name))
                .with_code("test-without-assertion")
                .with_fix("assert what the test is for, or delete it")
                .with_note(
                    "nothing reachable from this test calls `core/testing/assert`, so it passes \
                     as long as it does not abort",
                ),
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
                Diagnostic::error(
                    span.unwrap_or(Span::point(
                        session.workspace.package(t.package).build_file_id,
                        0,
                    )),
                    format!("{a} and {b} depend on each other"),
                )
                .with_code("dep-cycle")
                .with_fix("break the cycle: move what both need into a third target"),
            );
        }
    }
}
