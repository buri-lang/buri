//! `buri gen`.
//!
//! Rewrites the fields that *restate the sources* — where a file lives, what
//! it imports — and nothing that *constrains* them. `tags` and `platforms` are
//! the ones that matter most: a tag decides what a target may be linked with,
//! `platforms` decides where it may be built, neither is derivable from an
//! import graph, and a tool that dropped a `tags` entry while tidying
//! `sources` would turn `buri gen //...` into a way to quietly delete policy.

use crate::build::session::Session;
use crate::build::textproto::{Document, Field, Message, Value};
use crate::build::workspace::{PackageId, RuleKind};
use crate::diagnostics::{Diagnostic, Invariant as _, Span};
use std::collections::BTreeSet;
use std::path::Path;

pub struct Update {
    pub text: String,
    pub summary: Vec<String>,
}

#[expect(
    clippy::result_large_err,
    reason = "the error is the diagnostic itself, and `gen` returns at most one per package — \
              boxing it would put an allocation on a path that runs once and reads worse at \
              all three call sites"
)]
pub fn regenerate(session: &mut Session, package: PackageId) -> Result<Option<Update>, Diagnostic> {
    let p = session.workspace.package(package);
    let build_path = p.build_path.clone();
    let dir = p.dir.clone();
    let package_path = p.path.clone();
    let file_id = p.build_file_id;
    let original = std::fs::read_to_string(&build_path).unwrap_or_default();

    // A `BUILD.buri` must already exist, with the rule blocks. `buri gen`
    // never creates a build file and never adds a rule: deciding that a
    // directory is a library is a design decision.
    let parsed = crate::build::textproto::parse(&original, file_id);
    if let Some(first) = parsed.errors.first() {
        return Err(first.clone());
    }
    let mut document = parsed.document;

    let has_library = document.get("library").is_some();
    let has_binary = document.get("binary").is_some();

    // Every `.buri` and `.proto` file in the package, by category.
    let mut files = Vec::new();
    let mut schemas = Vec::new();
    collect(&dir, &dir, &mut files, &mut schemas);
    files.sort();
    schemas.sort();

    let mut lib_protos = Vec::new();
    let mut bin_protos = Vec::new();
    let mut lib_sources = Vec::new();
    let mut bin_sources = Vec::new();
    let mut lib_tests = Vec::new();
    let mut bin_tests = Vec::new();
    let mut testing_sources = Vec::new();
    // The two halves of rule 4, which read differently to whoever has to place
    // the file. Named rather than carried as a bool: the bool meant "reachable
    // from both" only because the match arms below happened to be in the order
    // they were in, and reordering them would have inverted the message.
    let mut unplaceable: Vec<Unplaceable> = Vec::new();

    // A file already listed in a rule's `sources` stays there.
    let existing_lib = listed(&document, "library", "sources");
    let existing_bin = listed(&document, "binary", "sources");
    let existing_lib_protos = listed(&document, "library", "proto_sources");
    let existing_bin_protos = listed(&document, "binary", "proto_sources");
    let existing_lib_tests = listed_at(&document, "library", &["test", "sources"]);
    let existing_bin_tests = listed_at(&document, "binary", &["test", "sources"]);

    // In a package with both rules, `gen` needs to know which rule a new file
    // belongs to, and the answer is which entry point reaches it. Computed
    // once, and only where the question can arise: a package with one rule has
    // one answer, and asking would be work with nothing to decide.
    let main_module = format!("//{package_path}/main");
    let (from_main, from_lib) = if has_library && has_binary {
        (reachable(&dir, &package_path, "main.buri"), reachable(&dir, &package_path, "lib.buri"))
    } else {
        (BTreeSet::new(), BTreeSet::new())
    };

    // A `.proto` is placed by the same question a `.buri` is — which entry
    // point reaches it — because the module it becomes belongs to a rule just
    // as a hand-written one does.
    for f in &schemas {
        if existing_lib_protos.contains(f) {
            lib_protos.push(f.clone());
            continue;
        }
        if existing_bin_protos.contains(f) {
            bin_protos.push(f.clone());
            continue;
        }
        match (has_library, has_binary) {
            (true, false) => lib_protos.push(f.clone()),
            (false, true) => bin_protos.push(f.clone()),
            (true, true) => match (from_main.contains(f), from_lib.contains(f)) {
                (true, false) => bin_protos.push(f.clone()),
                (false, true) => lib_protos.push(f.clone()),
                (true, true) => unplaceable.push(Unplaceable::ReachableFromBoth(f.clone())),
                (false, false) => {
                    unplaceable.push(Unplaceable::ReachableFromNeither(f.clone()))
                }
            },
            (false, false) => {}
        }
    }

    for f in &files {
        if f == "lib.buri" || f == "main.buri" || f == "testing/lib.buri" {
            continue;
        }
        if f.starts_with("test/") {
            // A suite names its target in an import: `//pkg/main` is the
            // binary's and anything else is the library's. The same question
            // as for sources — which entry point is this about — asked from
            // the other end, because nothing imports a test source. Rule 1
            // holds here too: a suite a rule already lists stays where it is.
            let tests_the_binary = if existing_bin_tests.contains(f) {
                true
            } else if existing_lib_tests.contains(f) {
                false
            } else {
                imports_of(&dir, f).contains(&main_module)
            };
            // A package with one rule has one suite, whichever the import
            // named, so the rule that exists takes it.
            if has_binary && (tests_the_binary || !has_library) {
                bin_tests.push(f.clone());
            } else if has_library {
                lib_tests.push(f.clone());
            }
            continue;
        }
        if f.starts_with("testing/") {
            testing_sources.push(f.clone());
            continue;
        }
        if existing_lib.contains(f) {
            lib_sources.push(f.clone());
            continue;
        }
        if existing_bin.contains(f) {
            bin_sources.push(f.clone());
            continue;
        }
        match (has_library, has_binary) {
            (true, false) => lib_sources.push(f.clone()),
            (false, true) => bin_sources.push(f.clone()),
            (true, true) => match (from_main.contains(f), from_lib.contains(f)) {
                // A file reachable by imports from `main.buri` and not from
                // `lib.buri` goes to the binary.
                (true, false) => bin_sources.push(f.clone()),
                // A file reachable from `lib.buri` goes to the library.
                (false, true) => lib_sources.push(f.clone()),
                // A file reachable from neither, or from both, is an error
                // that names the file and asks you to place it. Guessing here
                // would silently move code across a boundary that exists to be
                // explicit.
                (true, true) => unplaceable.push(Unplaceable::ReachableFromBoth(f.clone())),
                (false, false) => {
                    unplaceable.push(Unplaceable::ReachableFromNeither(f.clone()))
                }
            },
            (false, false) => {}
        }
    }

    if let Some(entry) = unplaceable.first() {
        let (file, reached) = match entry {
            Unplaceable::ReachableFromBoth(f) => (f, "both `lib.buri` and `main.buri`"),
            Unplaceable::ReachableFromNeither(f) => (f, "neither `lib.buri` nor `main.buri`"),
        };
        return Err(Diagnostic::templated("unplaceable-source", Span::point(file_id, 0))
            .with_bind("source", file.clone())
            .with_bind("reached", reached)
            .with_bind(
                "field",
                if file.ends_with(".proto") { "proto_sources" } else { "sources" },
            ));
    }

    let deps = derive_dependencies(session, package, &dir, &lib_tests, &bin_tests);
    let p = session.workspace.package(package);
    let mut summary = Vec::new();

    // In a package with both rules, `+ sources: ...` twice is two claims a
    // reader cannot tell apart, so the field is named by the rule that holds
    // it. In a package with one rule there is nothing to disambiguate, and
    // CLI.md's worked example prints the short form.
    let name = |rule: &str, field: &str| -> String {
        if has_library && has_binary {
            format!("{rule}.{field}")
        } else {
            field.to_string()
        }
    };

    // The two rules take the same managed fields and differ only in which
    // side of the source split each list came from, so this is one body run
    // twice rather than two bodies to keep in step. `testing` is the one
    // exception: only a library declares one.
    let lib_deps = deps.as_ref().and_then(|d| d.library.as_ref());
    let bin_deps = deps.as_ref().and_then(|d| d.binary.as_ref());
    for (rule, present, sources, protos, tests, rule_deps) in [
        (
            "library",
            has_library,
            &lib_sources,
            &lib_protos,
            &lib_tests,
            lib_deps.map(|d| (&d.production, &d.test)),
        ),
        (
            "binary",
            has_binary,
            &bin_sources,
            &bin_protos,
            &bin_tests,
            bin_deps.map(|d| (&d.production, &d.test)),
        ),
    ] {
        if !present {
            continue;
        }
        set_list(&mut document, rule, &["sources"], sources, &mut summary, &name(rule, "sources"));
        set_list(
            &mut document,
            rule,
            &["proto_sources"],
            protos,
            &mut summary,
            &name(rule, "proto_sources"),
        );
        if let Some((production, _)) = rule_deps {
            set_list(
                &mut document,
                rule,
                &["dependencies"],
                production,
                &mut summary,
                &name(rule, "dependencies"),
            );
        }
        if !tests.is_empty() || has_block(&document, rule, "test") {
            set_list(
                &mut document,
                rule,
                &["test", "sources"],
                tests,
                &mut summary,
                &name(rule, "test.sources"),
            );
            if let Some((_, test)) = rule_deps {
                set_list(
                    &mut document,
                    rule,
                    &["test", "dependencies"],
                    test,
                    &mut summary,
                    &name(rule, "test.dependencies"),
                );
            }
        }
        // A `testing/` directory is a surface only because a rule says so, so
        // this is the one managed field that waits to be asked for: `gen`
        // fills the block in and never decides the package has one.
        if rule == "library" && p.build.library.as_ref().is_some_and(|l| l.testing.is_some()) {
            set_list(
                &mut document,
                rule,
                &["testing", "sources"],
                &testing_sources,
                &mut summary,
                &name(rule, "testing.sources"),
            );
            if let Some(d) = lib_deps {
                set_list(
                    &mut document,
                    rule,
                    &["testing", "dependencies"],
                    &d.testing,
                    &mut summary,
                    &name(rule, "testing.dependencies"),
                );
            }
        }
    }

    // What `gen` may change everywhere is formatting: it leaves the file
    // exactly as `buri format` would, so the two never fight over a file.
    let text = crate::build::textproto::print(&document);
    if text == original {
        return Ok(None);
    }
    Ok(Some(Update { text, summary }))
}

/// A file in a package with both rules that `gen` will not place for you.
///
/// Two variants rather than a `(String, bool)`: the two are different findings
/// with different messages, and a bool that means "both" rather than "neither"
/// only by the order of the arms that produced it is one edit away from
/// printing the opposite of what happened.
enum Unplaceable {
    ReachableFromBoth(String),
    ReachableFromNeither(String),
}

/// What `gen` derived, per rule.
///
/// A rule's lists live behind that rule's `Option`, so "the binary's derived
/// dependencies, in a package with no binary" is nothing that can be written
/// down — where before it was five parallel vectors kept empty by a `has_binary`
/// guard at the one place they were read.
#[derive(Default)]
struct Derived {
    library: Option<LibraryDeps>,
    binary: Option<BinaryDeps>,
}

#[derive(Default)]
struct LibraryDeps {
    production: Vec<String>,
    test: Vec<String>,
    testing: Vec<String>,
}

#[derive(Default)]
struct BinaryDeps {
    production: Vec<String>,
    test: Vec<String>,
}

/// Every library the sources use: the `//` imports, plus the libraries reached
/// by method resolution, which the tool can compute because resolution is a
/// single lookup.
///
/// Four answers rather than one, because the four `dependencies` fields are
/// four different questions. A module's *role* is what separates them — a file
/// is a test source because a rule lists it in `test.sources`, and that is the
/// only thing that makes one — so the analysis is run with the tests loaded
/// and the answers are split by role afterwards.
fn derive_dependencies(
    session: &mut Session,
    package: PackageId,
    dir: &Path,
    lib_tests: &[String],
    bin_tests: &[String],
) -> Option<Derived> {
    use crate::compiler::modules::Role;
    let mut out = Derived::default();
    for kind in [RuleKind::Library, RuleKind::Binary] {
        let has = match kind {
            RuleKind::Library => session.workspace.package(package).has_library(),
            RuleKind::Binary => session.workspace.package(package).has_binary(),
        };
        if !has {
            continue;
        }
        let target = crate::build::workspace::TargetId { package, kind };
        let unit = crate::compiler::modules::Unit {
            target: Some(target),
            // Regeneration reads a target's imports; it builds nothing. See
            // `Unit::platform`.
            platform: None,
            with_tests: true,
        };
        let analysis = crate::compiler::driver::analyze(
            Some(&session.workspace),
            &mut session.map,
            &mut session.parsed,
            &unit,
        );
        if analysis.diagnostics.has_errors() {
            // Without a clean check there is no method-resolution information,
            // so the imports alone would be an incomplete answer.
            return None;
        }
        let mut production = BTreeSet::new();
        let mut test = BTreeSet::new();
        let mut testing = BTreeSet::new();
        // An import is not the only way to use a library: a method resolves
        // through its receiver's type rather than through scope, so a call
        // that lands in another library counts even though no import names it.
        // The role of the module the call sits in decides which field it
        // belongs to, exactly as the import loop below does.
        resolved_by_role(session, &analysis, package, &mut production, &mut test, &mut testing);
        for m in &analysis.loaded.modules {
            if m.pkg != Some(package) {
                continue;
            }
            let into = match m.role {
                Role::TestSource => &mut test,
                Role::TestOnly => &mut testing,
                _ => &mut production,
            };
            for item in &m.ast.items {
                let path = match item {
                    crate::parsing::tree::Item::Import(i) => i.path.clone(),
                    crate::parsing::tree::Item::ReExport(r) => r.path.clone(),
                    _ => continue,
                };
                if let Some(label) = session.workspace.dependency_label(package, &path) {
                    into.insert(label);
                }
            }
        }
        // A test source is loaded because a rule lists it, so on the run that
        // *writes* `test.sources` the analysis has not seen one. Their imports
        // are read off disk for that reason, and the two answers are merged:
        // otherwise `gen` would need two passes to reach a fixed point, and a
        // command whose second run differs from its first is a command whose
        // `--check` lies.
        let on_disk = match kind {
            RuleKind::Library => lib_tests,
            RuleKind::Binary => bin_tests,
        };
        test.extend(imported_labels(session, package, dir, on_disk));
        // `test.dependencies` is what the suite adds: the target under test is
        // this package and is already excluded, and its `dependencies` reach
        // the suite through it, so naming them again would be two claims about
        // one edge.
        let test: Vec<String> =
            test.into_iter().filter(|l| !production.contains(l)).collect();
        let testing: Vec<String> = testing.into_iter().collect();
        let production: Vec<String> = production.into_iter().collect();
        match kind {
            RuleKind::Library => {
                out.library = Some(LibraryDeps { production, test, testing });
            }
            RuleKind::Binary => {
                out.binary = Some(BinaryDeps { production, test });
            }
        }
    }
    Some(out)
}

/// The libraries a package's own code reaches through a resolved call, sorted
/// into the three fields that can hold one by the role of the module the call
/// is written in.
///
/// `commands::lint` asks the same question of a package as a whole, because
/// `missing-dep` is satisfied by a declaration in any of the three. `gen` has
/// to write one of them, so it needs the finer answer.
fn resolved_by_role(
    session: &Session,
    analysis: &crate::compiler::driver::Analysis,
    own: PackageId,
    production: &mut BTreeSet<String>,
    test: &mut BTreeSet<String>,
    testing: &mut BTreeSet<String>,
) {
    use crate::compiler::modules::Role;
    use crate::compiler::semantics::typed;
    for (fid, body) in &analysis.checked.bodies {
        let info = analysis.checked.tables.fn_info(*fid);
        let Some(from) = analysis.loaded.modules.get(info.module.index()) else { continue };
        if from.pkg != Some(own) {
            continue;
        }
        let role = from.role;
        let mut reached: Vec<String> = Vec::new();
        typed::walk(&body.expr, &mut |e| {
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
            reached.push(if crate::build::workspace::is_test_only_path(&m.path) {
                format!("{label}/testing")
            } else {
                label
            });
        });
        let into = match role {
            Role::TestSource => &mut *test,
            Role::TestOnly => &mut *testing,
            _ => &mut *production,
        };
        into.extend(reached);
    }
}

/// The files one entry point reaches by imports, transitively.
///
/// Only modules inside this package count: `//pkg/x` names one, and every
/// other path is another target's business. Read off the syntax rather than
/// off a checked analysis, because the file being placed is a file no rule
/// lists yet — the loader would not have it, so there is nothing to ask.
fn reachable(dir: &Path, package_path: &str, entry: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut queue = vec![entry.to_string()];
    while let Some(f) = queue.pop() {
        for path in imports_of(dir, &f) {
            let Some(rest) = path.strip_prefix("//") else { continue };
            let Some(rel) = rest.strip_prefix(package_path).and_then(|r| r.strip_prefix('/')) else {
                continue;
            };
            let file =
                if rel.ends_with(".proto") { rel.to_string() } else { format!("{rel}.buri") };
            if dir.join(&file).is_file() && seen.insert(file.clone()) {
                queue.push(file);
            }
        }
    }
    seen
}

/// The libraries a set of the package's files import, read off disk.
///
/// The syntactic half of the same question `derive_dependencies` asks of the
/// analysis, for the files the analysis has not been told about yet.
fn imported_labels(
    session: &Session,
    package: PackageId,
    dir: &Path,
    files: &[String],
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for f in files {
        for path in imports_of(dir, f) {
            if let Some(label) = session.workspace.dependency_label(package, &path) {
                out.insert(label);
            }
        }
    }
    out
}

/// The module paths one file imports or re-exports, in source order.
///
/// Parsed rather than scanned, and the parse errors are dropped: a file that
/// does not compile still says where it thinks its imports come from, and that
/// is the whole of what is being asked.
fn imports_of(dir: &Path, file: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join(file)) else { return Vec::new() };
    // A schema says where its types come from in its own dialect, and those
    // imports are module paths once `//` is in front of them.
    if file.ends_with(".proto") {
        let parsed = crate::build::protoschema::parse(&text, crate::diagnostics::FileId(0));
        return parsed
            .schema
            .imports
            .iter()
            .map(|i| crate::build::protogen::import_module_path(&i.path))
            .collect();
    }
    let parsed = crate::parsing::parser::parse(&text, crate::diagnostics::FileId(0));
    parsed
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            crate::parsing::tree::Item::Import(i) => Some(i.path.clone()),
            crate::parsing::tree::Item::ReExport(r) => Some(r.path.clone()),
            _ => None,
        })
        .collect()
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>, schemas: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    items.sort();
    for p in items {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        let rel = || {
            p.strip_prefix(root)
                .or_ice("this walk started at `root` and only ever descends, so every path is under it")
                .display()
                .to_string()
                .replace('\\', "/")
        };
        if p.is_dir() {
            if p.join("BUILD.buri").is_file() {
                continue;
            }
            collect(root, &p, out, schemas);
        } else if p.extension().is_some_and(|x| x == "buri") && name != "BUILD.buri" {
            out.push(rel());
        } else if p.extension().is_some_and(|x| x == "proto") {
            schemas.push(rel());
        }
    }
}

fn listed(document: &Document, rule: &str, field: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(Value::Message(m, _)) = document.get(rule).map(|f| &f.value) {
        if let Some(Value::List(items, _)) = m.get(field).map(|f| &f.value) {
            for i in items {
                if let Value::Str(s, _) = i {
                    out.insert(s.clone());
                }
            }
        }
    }
    out
}

/// The same, for a field inside a block: `test.sources`.
fn listed_at(document: &Document, rule: &str, path: &[&str]) -> BTreeSet<String> {
    let Some((leaf, blocks)) = path.split_last() else { return BTreeSet::new() };
    let Some(Value::Message(mut m, _)) = document.get(rule).map(|f| f.value.clone()) else {
        return BTreeSet::new();
    };
    for seg in blocks {
        match m.get(seg).map(|f| f.value.clone()) {
            Some(Value::Message(inner, _)) => m = inner,
            _ => return BTreeSet::new(),
        }
    }
    listed_from(&m, leaf)
}

fn has_block(document: &Document, rule: &str, block: &str) -> bool {
    matches!(
        document.get(rule).map(|f| &f.value),
        Some(Value::Message(m, _)) if m.get(block).is_some()
    )
}

/// Replaces a managed field's contents whole rather than merging, so
/// hand-editing `sources` is pointless and hand-editing `tags` is expected.
fn set_list(
    document: &mut Document,
    rule: &str,
    path: &[&str],
    values: &[String],
    summary: &mut Vec<String>,
    label: &str,
) {
    let Some((leaf, blocks)) = path.split_last() else { return };
    let Some(rule_field) = document.fields.iter_mut().find(|f| f.name == rule) else { return };
    let Value::Message(message, _) = &mut rule_field.value else { return };
    let mut target: &mut Message = message;
    for seg in blocks {
        let i = match target.fields.iter().position(|f| f.name == *seg) {
            Some(i) => i,
            None => {
                // CLI.md's worked example starts from `library {}` and comes
                // back with a `test.sources`, so the block a managed field
                // lives in is created when there is something to put in it —
                // and only then, because an empty `test {}` nobody wrote is a
                // claim the package has a suite.
                if values.is_empty() {
                    return;
                }
                let at = target.fields.len();
                target.fields.push(Field {
                    name: (*seg).to_string(),
                    name_span: Span::NONE,
                    value: Value::Message(Message::default(), Span::NONE),
                    comments: Vec::new(),
                    blank_before: true,
                    span: Span::NONE,
                });
                at
            }
        };
        match target.fields.get_mut(i).map(|f| &mut f.value) {
            Some(Value::Message(inner, _)) => target = inner,
            _ => return,
        }
    }
    let items: Vec<Value> =
        values.iter().map(|v| Value::Str(v.clone(), Span::NONE)).collect();
    let before = listed_from(target, leaf);
    let after: BTreeSet<String> = values.iter().cloned().collect();

    match target.fields.iter_mut().find(|f| f.name == *leaf) {
        Some(f) => f.value = Value::List(items, Span::NONE),
        None => {
            if values.is_empty() {
                return;
            }
            // A new field goes at the top of the rule, above any block.
            let at = target
                .fields
                .iter()
                .position(|f| matches!(f.value, Value::Message(..)))
                .unwrap_or(target.fields.len());
            target.fields.insert(
                at,
                Field {
                    name: leaf.to_string(),
                    name_span: Span::NONE,
                    value: Value::List(items, Span::NONE),
                    comments: Vec::new(),
                    blank_before: false,
                    span: Span::NONE,
                },
            );
        }
    }

    let added: Vec<&String> = after.difference(&before).collect();
    let removed: Vec<&String> = before.difference(&after).collect();
    if !added.is_empty() {
        summary.push(format!(
            "+ {label}: {}",
            added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    if !removed.is_empty() {
        summary.push(format!(
            "- {label}: {}",
            removed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
}

fn listed_from(message: &Message, field: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(Value::List(items, _)) = message.get(field).map(|f| &f.value) {
        for i in items {
            if let Value::Str(s, _) = i {
                out.insert(s.clone());
            }
        }
    }
    out
}
