//! `buri format`, `buri lint`, `buri gen`, and `buri query`.

use crate::buildfile::Platform;
use crate::cli::{self, Session};
use crate::compile::Unit;
use crate::diag::{Diagnostic, Diagnostics, Span};
use crate::textproto;
use crate::workspace::{PkgId, RuleKind, TargetId};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// format
// ---------------------------------------------------------------------------

/// Formats `.buri` sources and `BUILD.buri` files, with no options and no
/// configuration file. A formatter with options is a formatter whose output is
/// a repository decision.
pub fn cmd_format(args: &cli::Args) -> i32 {
    let s = match cli::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let roots: Vec<PathBuf> = if args.targets.is_empty() {
        vec![s.root.clone()]
    } else {
        args.targets.iter().map(|t| s.root.join(t.trim_start_matches("//"))).collect()
    };

    let mut files = Vec::new();
    for r in &roots {
        collect(r, &mut files);
    }
    files.sort();

    let mut changed = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let formatted = if name == "BUILD.buri" || name == "REPO.buri" {
            let parsed = textproto::parse(&text, crate::diag::FileId(0));
            if !parsed.errors.is_empty() {
                eprintln!("error: {} does not parse", s.ws.rel_of(path));
                return 2;
            }
            textproto::print(&parsed.doc)
        } else {
            match crate::format::source(&text) {
                Some(f) => f,
                None => {
                    // A file that does not parse is left exactly as it is.
                    continue;
                }
            }
        };
        if formatted != text {
            changed.push(s.ws.rel_of(path));
            if !args.flags.check {
                let _ = std::fs::write(path, formatted);
            }
        }
    }

    if args.flags.check {
        // The CI form: exit non-zero on any file that would change.
        for c in &changed {
            println!("{c}");
        }
        return if changed.is_empty() { 0 } else { 1 };
    }
    for c in &changed {
        println!("formatted {c}");
    }
    0
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "buri") {
            out.push(p);
        }
    }
}

// ---------------------------------------------------------------------------
// lint
// ---------------------------------------------------------------------------

/// Checks that type checking does not cover. None of this is configurable:
/// there is no `lint` block in `REPO.buri`, no per-file suppression comment,
/// and no way to promote or silence a check for one repository.
pub fn cmd_lint(args: &cli::Args) -> i32 {
    let mut s = match cli::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
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

    let mut diags = Diagnostics::new();
    let mut seen_packages = BTreeSet::new();
    for target in &targets {
        if seen_packages.insert(target.pkg) {
            check_sources_declared(&s, target.pkg, &mut diags);
        }
        // `lint` is not building, so it checks a binary against the platforms
        // its own `outputs` name, and a library against the question TAGS.md
        // asks at the target: can it be built at all?
        crate::build::check_visibility(&s, *target, &mut diags);
        crate::build::check_tags(&s, *target, &mut diags);
        check_target_platforms(&s, *target, &mut diags);
        check_dependencies(&mut s, *target, &mut diags);
    }
    check_cycles(&s, &mut diags);

    diags.sort(&s.map);
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
            crate::build::check_platform(s, target, p, diags);
        }
    }
}

/// `undeclared-source` and `duplicate-source`: every `.buri` file in a package
/// must appear in exactly one rule. A file that appears in none can be dropped
/// from the build by a typo and never noticed.
fn check_sources_declared(s: &Session, pkg: PkgId, diags: &mut Diagnostics) {
    let p = s.ws.pkg(pkg);
    let mut declared: Vec<(String, Span)> = Vec::new();
    let push = |list: &[crate::buildfile::Sp<String>], out: &mut Vec<(String, Span)>| {
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
    let analysis = crate::driver::analyze(Some(&s.ws), &mut s.map, &unit);
    if analysis.diags.has_errors() {
        diags.extend(analysis.diags.items);
        return;
    }

    let declared: Vec<crate::buildfile::Sp<String>> = s.ws.declared_deps(target).to_vec();
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
                crate::ast::Item::Import(i) => (i.path.clone(), i.path_span),
                crate::ast::Item::ReExport(r) => (r.path.clone(), r.path_span),
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
            let testing = crate::workspace::is_test_only_path(&path);
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

/// The same computation, for `buri gen`.
pub(crate) fn reached_by_resolution_pub(
    s: &Session,
    analysis: &crate::driver::Analysis,
    own: PkgId,
) -> BTreeSet<String> {
    reached_by_resolution(s, analysis, own)
}

/// Every library a package's own code reaches through a resolved call, which
/// the tool can compute because resolution is a single lookup.
pub(crate) fn reached_by_resolution(
    s: &Session,
    analysis: &crate::driver::Analysis,
    own: PkgId,
) -> BTreeSet<String> {
    use crate::types::ModuleId;
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
        crate::hir::walk(&body.expr, &mut |e| {
            let called = match &e.kind {
                crate::hir::ExprKind::CallFn { func, .. } => Some(*func),
                crate::hir::ExprKind::FnRef(f, _) => Some(*f),
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
            out.insert(if crate::workspace::is_test_only_path(&m.path) {
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

// ---------------------------------------------------------------------------
// gen
// ---------------------------------------------------------------------------

/// Rewrites the fields that restate the sources, and no others. `tags`,
/// `platforms`, `timeout_seconds`, `visibility`, `outputs`, `test.data`, and
/// every comment come back saying exactly what they said.
pub fn cmd_gen(args: &cli::Args) -> i32 {
    let mut s = match cli::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
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

    let mut stale = Vec::new();
    let mut packages: Vec<PkgId> = targets.iter().map(|t| t.pkg).collect();
    packages.sort();
    packages.dedup();

    for pkg in packages {
        match crate::gen::regenerate(&mut s, pkg) {
            Ok(Some(update)) => {
                stale.push(s.ws.pkg(pkg).path.clone());
                if !args.flags.check {
                    let path = s.ws.pkg(pkg).build_path.clone();
                    if std::fs::write(&path, &update.text).is_ok() {
                        println!("updated {}/BUILD.buri", s.ws.pkg(pkg).path);
                        for line in &update.summary {
                            println!("  {line}");
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(d) => {
                s.emit(&d);
                return 1;
            }
        }
    }

    if args.flags.check {
        for p in &stale {
            println!("{p}/BUILD.buri is out of date");
        }
        return if stale.is_empty() { 0 } else { 1 };
    }
    0
}

// ---------------------------------------------------------------------------
// query
// ---------------------------------------------------------------------------

/// `deps`, `rdeps`, `path`, `tags`, `platforms`, `sources`.
pub fn cmd_query(args: &cli::Args) -> i32 {
    let s = match cli::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let Some(expr) = args.targets.first() else {
        eprintln!("error: `buri query` takes an expression, as in 'deps(//cmd/server)'");
        return 2;
    };
    let expr = expr.trim();
    let Some((func, rest)) = expr.split_once('(') else {
        eprintln!("error: `{expr}` is not a query");
        eprintln!("  = the forms are deps, rdeps, path, tags, platforms, sources");
        return 2;
    };
    let Some(inner) = rest.strip_suffix(')') else {
        eprintln!("error: `{expr}` is missing its closing parenthesis");
        return 2;
    };
    let arguments: Vec<&str> = inner.split(',').map(|a| a.trim()).collect();

    let lookup = |label: &str| -> Option<TargetId> {
        let path = label.strip_prefix("//")?;
        let id = s.ws.pkg_by_path(path)?;
        s.ws.targets().into_iter().find(|t| t.pkg == id)
    };

    match func.trim() {
        "deps" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            for m in s.ws.closure(t) {
                if m != t {
                    println!("{}", s.ws.label(m));
                }
            }
        }
        "rdeps" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            for other in s.ws.targets() {
                if other != t && s.ws.closure(other).contains(&t) {
                    println!("{}", s.ws.label(other));
                }
            }
        }
        // The one that earns its place: the answer to "why does the JS build
        // pull in the database layer" is an edge, and printing it is faster
        // than reading build files.
        "path" => {
            if arguments.len() != 2 {
                eprintln!("error: `path` takes two targets");
                return 2;
            }
            let (Some(from), Some(to)) = (lookup(arguments[0]), lookup(arguments[1])) else {
                eprintln!("error: no such target");
                return 2;
            };
            match s.ws.dep_path(from, to) {
                Some(path) => {
                    println!("{}", s.ws.label(path[0].0));
                    for (node, span) in path.iter().skip(1) {
                        let where_ = match span {
                            Some(sp) if !sp.is_none() => {
                                let f = s.map.get(sp.file);
                                let (line, _) = f.line_col(sp.start);
                                format!("{}:{}", f.name, line)
                            }
                            _ => "implicit".into(),
                        };
                        println!("  -> {:<22} {where_}", s.ws.label(*node));
                    }
                }
                None => println!("no path"),
            }
        }
        "tags" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            for (tag, by) in s.ws.closure_tags(t) {
                println!("{tag}  ({})", s.ws.label(by));
            }
        }
        "platforms" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            for p in s.ws.platforms(t) {
                println!("{}", p.slug());
            }
        }
        "sources" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            let p = s.ws.pkg(t.pkg);
            let mut all = Vec::new();
            if let Some(lib) = &p.build.library {
                all.push("lib.buri".to_string());
                all.extend(lib.sources.iter().map(|x| x.value.clone()));
            }
            if let Some(bin) = &p.build.binary {
                all.push("main.buri".to_string());
                all.extend(bin.sources.iter().map(|x| x.value.clone()));
            }
            all.sort();
            for a in all {
                println!("{}/{a}", p.path);
            }
        }
        other => {
            eprintln!("error: there is no query `{other}`");
            eprintln!("  = the forms are deps, rdeps, path, tags, platforms, sources");
            return 2;
        }
    }
    0
}
