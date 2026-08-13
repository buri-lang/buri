//! `buri gen`.
//!
//! Rewrites the fields that *restate the sources* — where a file lives, what
//! it imports — and nothing that *constrains* them. `tags` and `platforms` are
//! the ones that matter most: a tag decides what a target may be linked with,
//! `platforms` decides where it may be built, neither is derivable from an
//! import graph, and a tool that dropped a `tags` entry while tidying
//! `sources` would turn `buri gen //...` into a way to quietly delete policy.

use crate::cli::Session;
use crate::diag::{Diagnostic, Span};
use crate::textproto::{Doc, Field, Msg, Value};
use crate::workspace::PkgId;
use std::collections::BTreeSet;
use std::path::Path;

pub struct Update {
    pub text: String,
    pub summary: Vec<String>,
}

pub fn regenerate(s: &mut Session, pkg: PkgId) -> Result<Option<Update>, Diagnostic> {
    let p = s.ws.pkg(pkg);
    let build_path = p.build_path.clone();
    let dir = p.dir.clone();
    let pkg_path = p.path.clone();
    let file_id = p.build_file_id;
    let original = std::fs::read_to_string(&build_path).unwrap_or_default();

    // A `BUILD.buri` must already exist, with the rule blocks. `buri gen`
    // never creates a build file and never adds a rule: deciding that a
    // directory is a library is a design decision.
    let parsed = crate::textproto::parse(&original, file_id);
    if !parsed.errors.is_empty() {
        return Err(parsed.errors[0].clone());
    }
    let mut doc = parsed.doc;

    let has_library = doc.get("library").is_some();
    let has_binary = doc.get("binary").is_some();

    // Every `.buri` file in the package, by category.
    let mut files = Vec::new();
    collect(&dir, &dir, &mut files);
    files.sort();

    let mut lib_sources = Vec::new();
    let mut bin_sources = Vec::new();
    let mut test_sources = Vec::new();
    let mut testing_sources = Vec::new();
    let mut unplaceable = Vec::new();

    // A file already listed in a rule's `sources` stays there.
    let existing_lib = listed(&doc, "library", "sources");
    let existing_bin = listed(&doc, "binary", "sources");

    for f in &files {
        if f == "lib.buri" || f == "main.buri" || f == "testing/lib.buri" {
            continue;
        }
        if f.starts_with("test/") {
            test_sources.push(f.clone());
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
            // A file reachable from neither, or from both, is an error that
            // names the file and asks you to place it. Guessing here would
            // silently move code across a boundary that exists to be explicit.
            (true, true) => unplaceable.push(f.clone()),
            (false, false) => {}
        }
    }

    if !unplaceable.is_empty() {
        return Err(Diagnostic::error(
            Span::point(file_id, 0),
            format!(
                "{} is in a package with both a library and a binary, and belongs to neither yet",
                unplaceable[0]
            ),
        )
        .with_note("add it to one rule's `sources`; guessing would move code across a boundary that exists to be explicit"));
    }

    let deps = derive_dependencies(s, pkg);
    let p = s.ws.pkg(pkg);
    let mut summary = Vec::new();

    if has_library {
        set_list(&mut doc, "library", &["sources"], &lib_sources, &mut summary, "sources");
        if let Some(d) = &deps {
            set_list(&mut doc, "library", &["dependencies"], &d.library, &mut summary, "dependencies");
        }
        if !test_sources.is_empty() || has_block(&doc, "library", "test") {
            set_list(&mut doc, "library", &["test", "sources"], &test_sources, &mut summary, "test.sources");
        }
        if p.build.library.as_ref().is_some_and(|l| l.testing.present) {
            set_list(
                &mut doc,
                "library",
                &["testing", "sources"],
                &testing_sources,
                &mut summary,
                "testing.sources",
            );
        }
    }
    if has_binary {
        set_list(&mut doc, "binary", &["sources"], &bin_sources, &mut summary, "sources");
        if let Some(d) = &deps {
            set_list(&mut doc, "binary", &["dependencies"], &d.binary, &mut summary, "dependencies");
        }
        if has_block(&doc, "binary", "test") {
            set_list(&mut doc, "binary", &["test", "sources"], &test_sources, &mut summary, "test.sources");
        }
    }

    // What `gen` may change everywhere is formatting: it leaves the file
    // exactly as `buri format` would, so the two never fight over a file.
    let text = crate::textproto::print(&doc);
    let _ = pkg_path;
    if text == original {
        return Ok(None);
    }
    Ok(Some(Update { text, summary }))
}

struct Derived {
    library: Vec<String>,
    binary: Vec<String>,
}

/// Every library the sources use: the `//` imports, plus the libraries reached
/// by method resolution, which the tool can compute because resolution is a
/// single lookup.
fn derive_dependencies(s: &mut Session, pkg: PkgId) -> Option<Derived> {
    let mut out = Derived { library: Vec::new(), binary: Vec::new() };
    for kind in [crate::workspace::RuleKind::Library, crate::workspace::RuleKind::Binary] {
        let has = match kind {
            crate::workspace::RuleKind::Library => s.ws.pkg(pkg).has_library(),
            crate::workspace::RuleKind::Binary => s.ws.pkg(pkg).has_binary(),
        };
        if !has {
            continue;
        }
        let target = crate::workspace::TargetId { pkg, kind };
        let unit = crate::compile::Unit {
            target: Some(target),
            platform: crate::buildfile::Platform::Js,
            with_tests: false,
        };
        let analysis = crate::driver::analyze(Some(&s.ws), &mut s.map, &unit);
        if analysis.diags.has_errors() {
            // Without a clean check there is no method-resolution information,
            // so the imports alone would be an incomplete answer.
            return None;
        }
        let mut labels: BTreeSet<String> =
            crate::tools::reached_by_resolution_pub(s, &analysis, pkg);
        for m in &analysis.loaded.modules {
            if m.pkg != Some(pkg) {
                continue;
            }
            for item in &m.ast.items {
                let path = match item {
                    crate::ast::Item::Import(i) => i.path.clone(),
                    crate::ast::Item::ReExport(r) => r.path.clone(),
                    _ => continue,
                };
                if !path.starts_with("//") {
                    continue;
                }
                let Ok(loc) = s.ws.resolve_module(&path) else { continue };
                let Some(other) = loc.pkg else { continue };
                if other == pkg {
                    continue;
                }
                let label = s.ws.pkg(other).label();
                labels.insert(if crate::workspace::is_test_only_path(&path) {
                    format!("{label}/testing")
                } else {
                    label
                });
            }
        }
        let list: Vec<String> = labels.into_iter().collect();
        match kind {
            crate::workspace::RuleKind::Library => out.library = list,
            crate::workspace::RuleKind::Binary => out.binary = list,
        }
    }
    Some(out)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    items.sort();
    for p in items {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            if p.join("BUILD.buri").is_file() {
                continue;
            }
            collect(root, &p, out);
        } else if p.extension().is_some_and(|x| x == "buri") && name != "BUILD.buri" {
            out.push(p.strip_prefix(root).unwrap().display().to_string().replace('\\', "/"));
        }
    }
}

fn listed(doc: &Doc, rule: &str, field: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(Value::Msg(m, _)) = doc.get(rule).map(|f| &f.value) {
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

fn has_block(doc: &Doc, rule: &str, block: &str) -> bool {
    matches!(doc.get(rule).map(|f| &f.value), Some(Value::Msg(m, _)) if m.get(block).is_some())
}

/// Replaces a managed field's contents whole rather than merging, so
/// hand-editing `sources` is pointless and hand-editing `tags` is expected.
fn set_list(
    doc: &mut Doc,
    rule: &str,
    path: &[&str],
    values: &[String],
    summary: &mut Vec<String>,
    label: &str,
) {
    let Some(rule_field) = doc.fields.iter_mut().find(|f| f.name == rule) else { return };
    let Value::Msg(msg, span) = &mut rule_field.value else { return };
    let mut target: &mut Msg = msg;
    for seg in &path[..path.len() - 1] {
        let Some(i) = target.fields.iter().position(|f| f.name == *seg) else { return };
        match &mut target.fields[i].value {
            Value::Msg(inner, _) => target = inner,
            _ => return,
        }
    }
    let leaf = path[path.len() - 1];
    let items: Vec<Value> =
        values.iter().map(|v| Value::Str(v.clone(), Span::NONE)).collect();
    let before = listed_from(target, leaf);
    let after: BTreeSet<String> = values.iter().cloned().collect();

    match target.fields.iter_mut().find(|f| f.name == leaf) {
        Some(f) => f.value = Value::List(items, Span::NONE),
        None => {
            if values.is_empty() {
                return;
            }
            // A new field goes at the top of the rule, above any block.
            let at = target
                .fields
                .iter()
                .position(|f| matches!(f.value, Value::Msg(..)))
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
    let _ = span;

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

fn listed_from(msg: &Msg, field: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(Value::List(items, _)) = msg.get(field).map(|f| &f.value) {
        for i in items {
            if let Value::Str(s, _) = i {
                out.insert(s.clone());
            }
        }
    }
    out
}
