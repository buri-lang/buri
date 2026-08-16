//! `buri lint`.
//!
//! Checks that type checking does not cover. None of this is configurable:
//! there is no `lint` block in `REPO.buri`, no per-file suppression comment,
//! and no way to promote or silence a check for one repository. A lint that
//! cannot be turned off has to be one nobody wants to turn off, which is the
//! bar every check here is held to.

use crate::build::buildfile::Platform;
use crate::build::session::{self, Session};
use crate::build::workspace::{PkgId, RuleKind, TargetId};
use crate::commands::arguments;
use crate::compiler::modules::{ModuleData, Unit};
use crate::compiler::semantics::typed;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Checks that type checking does not cover. None of this is configurable:
/// there is no `lint` block in `REPO.buri`, no per-file suppression comment,
/// and no way to promote or silence a check for one repository.
pub fn cmd_lint(args: &arguments::Args) -> i32 {
    let (mut s, diags) = match collect_findings(args) {
        Ok(v) => v,
        Err(code) => return code,
    };

    if args.flags.fix {
        let applied = apply_fixes(&mut s, &diags);
        if applied > 0 {
            println!("fixed {applied} finding{}", if applied == 1 { "" } else { "s" });
            // Everything is computed again from the files on disk rather than
            // subtracted from what was just reported: an edit can uncover a
            // finding the first pass could not see, and a count arrived at by
            // arithmetic is one nobody can check.
            let (mut s, diags) = match collect_findings(args) {
                Ok(v) => v,
                Err(code) => return code,
            };
            return report_findings(&mut s, &diags);
        }
        println!("nothing to fix");
    }

    report_findings(&mut s, &diags)
}

/// Opens the repository and runs every check, in the order a reader would.
fn collect_findings(args: &arguments::Args) -> Result<(Session, Diagnostics), i32> {
    let mut s = match session::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return Err(2);
        }
    };
    if s.report() {
        return Err(2);
    }
    let targets = match s.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            return Err(2);
        }
    };

    let diags = findings_for(&mut s, &targets);
    Ok((s, diags))
}

/// Every check `buri lint` runs, over the targets given.
///
/// Public because the language server reports the same findings the command
/// does — an editor that showed only type errors would be showing half of what
/// the toolchain knows, and the half that is easier to notice at the terminal.
pub fn findings_for(s: &mut Session, targets: &[TargetId]) -> Diagnostics {
    let mut diags = Diagnostics::new();
    let mut seen_packages = BTreeSet::new();
    for target in targets {
        if seen_packages.insert(target.pkg) {
            check_sources_declared(s, target.pkg, &mut diags);
            check_test_suites(s, target.pkg, &mut diags);
        }
        // `lint` is not building, so it checks a binary against the platforms
        // its own `outputs` name, and a library against the question TAGS.md
        // asks at the target: can it be built at all?
        crate::build::actions::check_visibility(s, *target, &mut diags);
        crate::build::actions::check_tags(s, *target, &mut diags);
        check_target_platforms(s, *target, &mut diags);
        check_dependencies(s, *target, &mut diags);
    }
    check_cycles(s, &mut diags);

    diags.sort(&s.map);
    diags
}

fn report_findings(s: &mut Session, diags: &Diagnostics) -> i32 {
    let mut errors = 0;
    let mut warnings = 0;
    for d in &diags.items {
        s.emit(d);
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

fn regenerate_build_files(s: &mut Session, diags: &Diagnostics) -> usize {
    // Which package a finding is about. `missing-dep` points at the import in
    // a source file, not at the build file, so this is by path prefix — the
    // longest package path the file sits under.
    let mut packages: BTreeSet<PkgId> = BTreeSet::new();
    for d in &diags.items {
        if !d.code.as_deref().is_some_and(|c| REGENERABLE.contains(&c)) {
            continue;
        }
        let name = s.map.name(d.span.file).to_string();
        let mut best: Option<(usize, PkgId)> = None;
        for t in s.ws.targets() {
            let p = s.ws.pkg(t.pkg);
            let owns = p.build_file_id == d.span.file
                || name.strip_prefix(&p.path).is_some_and(|r| r.starts_with('/'));
            if owns && best.is_none_or(|(len, _)| p.path.len() > len) {
                best = Some((p.path.len(), t.pkg));
            }
        }
        if let Some((_, pkg)) = best {
            packages.insert(pkg);
        }
    }

    let mut fixed = 0;
    for pkg in packages {
        match crate::build::regenerate::regenerate(s, pkg) {
            Ok(Some(update)) => {
                let path = s.ws.pkg(pkg).build_path.clone();
                if let Err(err) = std::fs::write(&path, &update.text) {
                    eprintln!("error: writing {}: {err}", path.display());
                    continue;
                }
                println!("updated {}/BUILD.buri", s.ws.pkg(pkg).path);
                for line in &update.summary {
                    println!("  {line}");
                }
                fixed += 1;
            }
            Ok(None) => {}
            Err(d) => s.emit(&d),
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
fn apply_fixes(s: &mut Session, diags: &Diagnostics) -> usize {
    use std::collections::BTreeMap;
    let mut by_file: BTreeMap<u32, Vec<&crate::diagnostics::Edit>> = BTreeMap::new();
    for d in &diags.items {
        for e in &d.edits {
            by_file.entry(e.file.0).or_default().push(e);
        }
    }

    let mut applied = regenerate_build_files(s, diags);
    for (file, mut edits) in by_file {
        let id = crate::diagnostics::FileId(file);
        edits.sort_by_key(|e| (e.start, e.end));
        if edits.windows(2).any(|w| w[0].end > w[1].start) {
            eprintln!(
                "warning: {} has overlapping fixes, so none were applied",
                s.map.name(id)
            );
            continue;
        }

        let mut text = s.map.text(id).to_string();
        for e in edits.iter().rev() {
            text.replace_range(e.start as usize..e.end as usize, &e.replacement);
        }
        // Parsed, not formatted. The guard a rewriting tool owes its user is
        // "what I wrote is still a program", and that is what parsing answers.
        // Running the result through the formatter would answer it too, and
        // would also reformat everything the fix did not touch — which turns
        // one deliberate edit into a diff nobody asked for.
        if !crate::parsing::parser::parse(&text, id).errors.is_empty() {
            eprintln!("warning: fixing {} would not parse, so it was left alone", s.map.name(id));
            continue;
        }
        let path = s.map.get(id).abs_path.clone();
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
fn check_target_platforms(s: &Session, target: TargetId, diags: &mut Diagnostics) {
    let allowed = s.ws.platforms(target);
    if allowed.is_empty() {
        let label = s.ws.label(target);
        let span = s
            .ws
            .tags(target)
            .first()
            .map(|t| t.span)
            .unwrap_or(Span::point(s.ws.pkg(target.pkg).build_file_id, 0));
        diags.push(
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
    let Some(bin) = &s.ws.pkg(target.pkg).build.binary else { return };
    for output in &bin.outputs {
        let Some(p) = output.platform.as_ref().map(|x| x.value) else { continue };
        if !allowed.contains(&p) {
            crate::build::actions::check_platform(s, target, p, diags);
        }
    }
}

/// `empty-test-suite`. A `test` block that declares no sources is a claim that
/// something is tested, backed by nothing — and it reads as coverage in every
/// tool that walks the build graph. Writing the block is the deliberate act, so
/// the empty one is a leftover rather than a decision.
fn check_test_suites(s: &Session, pkg: PkgId, diags: &mut Diagnostics) {
    let p = s.ws.pkg(pkg);
    let mut report = |suite: &crate::build::buildfile::TestSuite, rule: &str| {
        if !suite.present || !suite.sources.is_empty() {
            return;
        }
        diags.push(
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
        report(&l.test, "library");
    }
    if let Some(b) = &p.build.binary {
        report(&b.test, "binary");
    }
}

/// `undeclared-source` and `duplicate-source`: every `.buri` file in a package
/// must appear in exactly one rule. A file that appears in none can be dropped
/// from the build by a typo and never noticed.
fn check_sources_declared(s: &Session, pkg: PkgId, diags: &mut Diagnostics) {
    let p = s.ws.pkg(pkg);
    let mut declared: Vec<(String, Span)> = Vec::new();
    let push = |list: &[crate::build::buildfile::Sp<String>], out: &mut Vec<(String, Span)>| {
        for x in list {
            out.push((x.value.clone(), x.span));
        }
    };
    if let Some(lib) = &p.build.library {
        push(&lib.sources, &mut declared);
        push(&lib.test.sources, &mut declared);
        push(&lib.testing.sources, &mut declared);
    }
    if let Some(bin) = &p.build.binary {
        push(&bin.sources, &mut declared);
        push(&bin.test.sources, &mut declared);
    }

    for i in 0..declared.len() {
        for j in i + 1..declared.len() {
            if declared[i].0 == declared[j].0 {
                diags.push(
                    Diagnostic::error(
                        declared[j].1,
                        format!("{} is listed by two rules", declared[j].0),
                    )
                    .with_code("duplicate-source")
                    .with_fix("list it under one rule only")
                    .with_sub(declared[i].1, "first listed here"),
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
    collect_package_sources(&p.dir, &p.dir, s, pkg, &mut on_disk);
    for rel in on_disk {
        if known.contains(&rel) {
            continue;
        }
        diags.push(
            Diagnostic::error(
                Span::point(p.build_file_id, 0),
                format!("{}/{rel} is not declared by any rule", p.path),
            )
            .with_code("undeclared-source")
            .with_fix(format!(
                "add it to a rule's `sources`, or delete it — `buri gen //{}` does this \
                 automatically",
                p.path
            )),
        );
    }
}

fn collect_package_sources(
    root: &Path,
    dir: &Path,
    s: &Session,
    pkg: PkgId,
    out: &mut Vec<String>,
) {
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
            collect_package_sources(root, &p, s, pkg, out);
        } else if p.extension().is_some_and(|x| x == "buri") && name != "BUILD.buri" {
            let rel = p.strip_prefix(root).unwrap().display().to_string().replace('\\', "/");
            out.push(rel);
        }
    }
}

/// `missing-dep` and `unused-dep`. Use is what requires a dep, and an import is
/// not the only way to use: a method resolving into a library counts too.
fn check_dependencies(s: &mut Session, target: TargetId, diags: &mut Diagnostics) {
    let unit = Unit { target: Some(target), platform: Platform::Js, with_tests: true };
    let analysis = crate::compiler::driver::analyze(Some(&s.ws), &mut s.map, &unit);
    if analysis.diags.has_errors() {
        diags.extend(analysis.diags.items);
        return;
    }

    // The hygiene rules ask about the same modules this analysis already
    // loaded, so they ride along rather than paying for a second one.
    check_hygiene(s, target, &analysis, diags);

    let declared: Vec<crate::build::buildfile::Sp<String>> = s.ws.declared_deps(target).to_vec();
    let own = target.pkg;
    // Use is what requires a dep, and an import is not the only way to use: a
    // method resolves through its receiver's type rather than through scope,
    // so a call that lands in another library counts even though no import
    // names it (BUILD-FILES.md, "Dependencies").
    let resolved: BTreeSet<String> = reached_by_resolution(s, &analysis, own);
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
            if !path.starts_with("//") {
                continue;
            }
            let Ok(loc) = s.ws.resolve_module(&path) else { continue };
            let Some(other) = loc.pkg else { continue };
            if other == own {
                continue;
            }
            let label = s.ws.pkg(other).label();
            let testing = crate::build::workspace::is_test_only_path(&path);
            let wanted = if testing { format!("{label}/testing") } else { label.clone() };
            used.insert(wanted.clone());
            if !declared.iter().any(|d| d.value == wanted) && !in_test_deps(s, target, &wanted) {
                let importer = s.map.name(m.file).to_string();
                let pkg_path = s.ws.pkg(own).path.clone();
                reported.insert(wanted.clone());
                diags.push(
                    Diagnostic::error(
                        span,
                        format!("{importer} imports {wanted}, which is not in dependencies"),
                    )
                    .with_code("missing-dep")
                    .with_fix(format!(
                        "add \"{wanted}\" to dependencies in {pkg_path}/BUILD.buri — \
                         `buri gen //{pkg_path}` does this automatically"
                    )),
                );
            }
        }
    }

    // A library reached only through method resolution. No import names it, so
    // there is nothing in a source file to point at — the claim that is wrong
    // is the one the build file makes, and that is where the span goes
    // (BUILD-FILES.md, "Dependencies": "an import is not the only way to use").
    let pkg_path = s.ws.pkg(own).path.clone();
    let own_label = s.ws.pkg(own).label();
    for wanted in &resolved {
        if declared.iter().any(|d| &d.value == wanted)
            || in_test_deps(s, target, wanted)
            || reported.contains(wanted)
        {
            continue;
        }
        diags.push(
            Diagnostic::error(
                Span::point(s.ws.pkg(own).build_file_id, 0),
                format!("{own_label} uses {wanted}, which is not in dependencies"),
            )
            .with_code("missing-dep")
            .with_note(
                "a method resolves through its receiver's type rather than through scope, \
                 so no import names it",
            )
            .with_fix(format!(
                "add \"{wanted}\" to dependencies in {pkg_path}/BUILD.buri — \
                 `buri gen //{pkg_path}` does this automatically"
            )),
        );
    }

    for d in &declared {
        if !used.contains(&d.value) {
            diags.push(
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
fn check_hygiene(
    s: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    diags: &mut Diagnostics,
) {
    let own = target.pkg;
    for m in &analysis.loaded.modules {
        if m.pkg == Some(own) {
            check_unused_imports(s, m, diags);
        }
    }
    check_unreachable_exports(s, target, analysis, diags);
    check_discarded_results(s, own, analysis, diags);
    check_tests_assert(s, own, analysis, diags);
}

/// `unreachable-export`. Inside a library, `export` means "visible to the rest
/// of this library"; `lib.buri` decides what leaves it. So an `export` that
/// `lib.buri` does not re-export and no sibling module imports is visible to
/// nobody — the word is there, and it does nothing.
///
/// Only module-level items. A field's or a variant's `export` is about the
/// shape of a type, and asking whether it is "reached" is a different question.
fn check_unreachable_exports(
    s: &Session,
    target: TargetId,
    analysis: &crate::compiler::driver::Analysis,
    diags: &mut Diagnostics,
) {
    // A binary has no surface — nothing may import its modules — so the rule
    // does not apply to one.
    if target.kind != RuleKind::Library {
        return;
    }
    let own = target.pkg;
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
            let Ok(loc) = s.ws.resolve_module(path) else { continue };
            if loc.pkg != Some(own) {
                continue;
            }
            for sp in specs {
                wanted.insert(sp.name.name.as_str());
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
        if taken_whole.contains(m.path.as_str()) {
            continue;
        }
        for item in &m.ast.items {
            let Some((name, span)) = exported_name(item) else { continue };
            if surface.contains(name) || wanted.contains(name) {
                continue;
            }
            let lib = format!("{}/lib.buri", s.ws.pkg(own).path);
            diags.push(
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

/// The name a module-level item exports, when it exports one.
fn exported_name(item: &crate::parsing::tree::Item) -> Option<(&str, Span)> {
    use crate::parsing::tree::Item;
    let (exported, name) = match item {
        Item::Fn(d) => (d.exported, &d.name),
        Item::Struct(d) => (d.exported, &d.name),
        Item::Enum(d) => (d.exported, &d.name),
        Item::TypeAlias(d) => (d.exported, &d.name),
        Item::Const(d) => (d.exported, &d.name),
        Item::Trait(d) => (d.exported, &d.name),
        _ => return None,
    };
    exported.then(|| (name.name.as_str(), name.span))
}

/// `unused-import`. Deliberately syntactic: a name counts as used if it appears
/// as an identifier token anywhere outside the import statements themselves.
///
/// That over-approximates use, which is the safe direction for a rule at error
/// severity — a shadowed binding or a field with the same spelling silences the
/// finding rather than producing a wrong one. Reading tokens rather than the
/// AST is what makes it total: there is no expression form it can forget.
fn check_unused_imports(s: &Session, m: &ModuleData, diags: &mut Diagnostics) {
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

    let text = s.map.text(m.file);
    let lexed = crate::parsing::lexer::lex(text, m.file);
    let mut used: BTreeSet<&str> = BTreeSet::new();
    for t in &lexed.tokens {
        let crate::parsing::lexer::Tok::Ident(name) = &t.tok else { continue };
        if import_ranges.iter().any(|(a, b)| t.span.start >= *a && t.span.end <= *b) {
            continue;
        }
        used.insert(name.as_str());
    }

    for item in &m.ast.items {
        let crate::parsing::tree::Item::Import(i) = item else { continue };
        let specs: Vec<(&str, Span)> = match &i.clause {
            crate::parsing::tree::ImportClause::Named(specs) => {
                specs.iter().map(|sp| (sp.local().name.as_str(), sp.span)).collect()
            }
            crate::parsing::tree::ImportClause::Namespace(n) => vec![(n.name.as_str(), n.span)],
        };
        // One edit per statement, not per name. Two adjacent unused names have
        // overlapping deletions — `A, ` and `, B` share the comma — and the
        // applier refuses on overlap rather than guessing. Rewriting the whole
        // clause has one answer whatever the pattern of unused names is.
        let survivors: Vec<String> = match &i.clause {
            crate::parsing::tree::ImportClause::Named(specs) => specs
                .iter()
                .filter(|sp| used.contains(sp.local().name.as_str()))
                .map(|sp| match &sp.alias {
                    Some(a) => format!("{} as {}", sp.name.name, a.name),
                    None => sp.name.name.clone(),
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
                d = d.with_edit(m.file, rewrite.0, rewrite.1, &rewrite.2);
                first = false;
            }
            diags.push(d);
        }
    }
}

/// The offset just past the newline ending the line `at` sits on.
fn line_end(text: &str, at: u32) -> u32 {
    let b = text.as_bytes();
    let mut i = at as usize;
    while i < b.len() && b[i] != b'\n' {
        i += 1;
    }
    if i < b.len() {
        i += 1;
    }
    i as u32
}

/// `discarded-result`. `let _ = <Result>` is already a hard type error
/// (`result-discarded`), so the only way a `Result` is dropped on purpose is
/// `core/result.ignore` — the greppable escape hatch. This is the grep,
/// promoted to a warning so it appears in a report rather than only when
/// somebody thinks to look.
fn check_discarded_results(
    s: &Session,
    own: PkgId,
    analysis: &crate::compiler::driver::Analysis,
    diags: &mut Diagnostics,
) {
    for (span, _) in calls_into(analysis, own, "core/result", &["ignore"]) {
        diags.push(
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
    let _ = s;
}

/// `test-without-assertion`. Read syntactically — "the body contains no
/// `assert`" — this fires on every test that asserts through a helper, which
/// is most of the ones worth writing. So it is transitive: a test passes if
/// anything reachable from it calls into `core/testing/assert`.
fn check_tests_assert(
    s: &Session,
    own: PkgId,
    analysis: &crate::compiler::driver::Analysis,
    diags: &mut Diagnostics,
) {
    use crate::compiler::semantics::types::FnId;
    let mine: BTreeSet<crate::compiler::semantics::types::ModuleId> = analysis
        .loaded
        .modules
        .iter()
        .filter(|m| m.pkg == Some(own))
        .map(|m| m.id)
        .collect();

    let asserts = |f: FnId| -> bool {
        let info = analysis.checked.tables.fun(f);
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
                    typed::ExprKind::CallFn { func, .. } => queue.push(*func),
                    typed::ExprKind::FnRef(g, _) => queue.push(*g),
                    _ => {}
                };
            });
        }
        if found {
            continue;
        }
        diags.push(
            Diagnostic::warning(case.span, format!("test {:?} asserts nothing", case.name))
                .with_code("test-without-assertion")
                .with_fix("assert what the test is for, or delete it")
                .with_note(
                    "nothing reachable from this test calls `core/testing/assert`, so it passes \
                     as long as it does not abort",
                ),
        );
    }
    let _ = s;
}

/// Every call site in `own`'s code that lands on one of `names` in `module`.
fn calls_into(
    analysis: &crate::compiler::driver::Analysis,
    own: PkgId,
    module: &str,
    names: &[&str],
) -> Vec<(Span, String)> {
    let mine: BTreeSet<crate::compiler::semantics::types::ModuleId> = analysis
        .loaded
        .modules
        .iter()
        .filter(|m| m.pkg == Some(own))
        .map(|m| m.id)
        .collect();
    let mut out = Vec::new();
    for (fid, body) in &analysis.checked.bodies {
        if !mine.contains(&analysis.checked.tables.fun(*fid).module) {
            continue;
        }
        crate::compiler::semantics::typed::walk(&body.expr, &mut |e| {
            let called = match &e.kind {
                typed::ExprKind::CallFn { func, .. } => *func,
                _ => return,
            };
            let info = analysis.checked.tables.fun(called);
            let Some(m) = analysis.loaded.modules.get(info.module.index()) else { return };
            if m.path == module && names.contains(&info.name.as_str()) {
                out.push((e.span, info.name.clone()));
            }
        });
    }
    out.sort_by_key(|(s, _)| (s.file.0, s.start));
    out
}

/// The same computation, for `buri gen`.
pub(crate) fn reached_by_resolution_pub(
    s: &Session,
    analysis: &crate::compiler::driver::Analysis,
    own: PkgId,
) -> BTreeSet<String> {
    reached_by_resolution(s, analysis, own)
}

/// Every library a package's own code reaches through a resolved call, which
/// the tool can compute because resolution is a single lookup.
pub(crate) fn reached_by_resolution(
    s: &Session,
    analysis: &crate::compiler::driver::Analysis,
    own: PkgId,
) -> BTreeSet<String> {
    use crate::compiler::semantics::types::ModuleId;
    let mine: BTreeSet<ModuleId> = analysis
        .loaded
        .modules
        .iter()
        .filter(|m| m.pkg == Some(own))
        .map(|m| m.id)
        .collect();
    let mut out = BTreeSet::new();
    for (fid, body) in &analysis.checked.bodies {
        let info = analysis.checked.tables.fun(*fid);
        if !mine.contains(&info.module) {
            continue;
        }
        crate::compiler::semantics::typed::walk(&body.expr, &mut |e| {
            let called = match &e.kind {
                typed::ExprKind::CallFn { func, .. } => Some(*func),
                typed::ExprKind::FnRef(f, _) => Some(*f),
                _ => None,
            };
            let Some(f) = called else { return };
            let callee = analysis.checked.tables.fun(f);
            let Some(m) = analysis.loaded.modules.get(callee.module.index()) else { return };
            let Some(pkg) = m.pkg else { return };
            if pkg == own {
                return;
            }
            let label = s.ws.pkg(pkg).label();
            out.insert(if crate::build::workspace::is_test_only_path(&m.path) {
                format!("{label}/testing")
            } else {
                label
            });
        });
    }
    out
}

fn in_test_deps(s: &Session, target: TargetId, label: &str) -> bool {
    let p = s.ws.pkg(target.pkg);
    let suite = match target.kind {
        RuleKind::Library => p.build.library.as_ref().map(|l| &l.test),
        RuleKind::Binary => p.build.binary.as_ref().map(|b| &b.test),
    };
    let testing = p.build.library.as_ref().map(|l| &l.testing);
    suite.is_some_and(|t| t.dependencies.iter().any(|d| d.value == label))
        || testing.is_some_and(|t| t.dependencies.iter().any(|d| d.value == label))
}

/// `dep-cycle`: package cycles are the module rule one level up.
fn check_cycles(s: &Session, diags: &mut Diagnostics) {
    for t in s.ws.targets() {
        for (dep, span) in s.ws.dep_edges(t) {
            if dep == t {
                continue;
            }
            if s.ws.closure(dep).contains(&t) {
                let a = s.ws.label(t);
                let b = s.ws.label(dep);
                diags.push(
                    Diagnostic::error(
                        span.unwrap_or(Span::point(s.ws.pkg(t.pkg).build_file_id, 0)),
                        format!("{a} and {b} depend on each other"),
                    )
                    .with_code("dep-cycle")
                    .with_fix("break the cycle: move what both need into a third target"),
                );
            }
        }
    }
}
