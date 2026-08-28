//! The requests that answer a question about a name.
//!
//! All five are reads of what the compiler already computed. `typed::Expr`
//! carries its own type and span, `Tables` carries every declaration's span,
//! and `ModuleScope` carries what a module exports — so hover, go-to-definition,
//! references and completion need no index of their own. That is the whole
//! reason this file is short: the answers were already in the analysis, waiting
//! to be asked for.

use crate::compiler::semantics::types;
use crate::compiler::standard_library;
use crate::json::Value;
use crate::parsing::tree::Item;
use std::path::Path;
use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols;

/// The symbol under the cursor, rendered; failing that, the type of the
/// innermost expression covering it.
pub fn hover(analyzed: &Analyzed, path: &Path, text: &str, position: Position) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;

    // A name renders as its signature plus its doc comment, at the use as well
    // as at the declaration — pointing at a call is the commonest way to ask
    // what a function is, and the type of the name is the least interesting
    // thing about it.
    if let Some(found) = symbols::at(analyzed, path, text, offset) {
        let (signature, docs) = symbols::describe(analyzed, &found.symbol);
        return Some(rendered(text, &signature, &docs, found.span));
    }

    // Otherwise the innermost expression whose span contains the offset. It is
    // innermost because a smaller span is always the more specific answer.
    let mut best: Option<(u32, String, crate::diagnostics::Span)> = None;
    for (fid, body) in &analyzed.analysis.checked.bodies {
        if analyzed.analysis.checked.tables.fn_info(*fid).span.file != file {
            continue;
        }
        crate::compiler::semantics::typed::walk(&body.expr, &mut |e| {
            if e.span.file != file || e.span.start > offset || e.span.end < offset {
                return;
            }
            let width = e.span.end.saturating_sub(e.span.start);
            if best.as_ref().is_none_or(|(w, _, _)| width < *w) {
                let ty = types::show(&analyzed.analysis.checked.tables, None, &[], &e.ty);
                best = Some((width, ty, e.span));
            }
        });
    }
    if let Some((_, ty, span)) = best {
        return Some(Value::object(vec![
            ("contents", markup(&format!("```buri\n{ty}\n```"))),
            ("range", convert::range(text, span)),
        ]));
    }

    // Above the first declaration there is nothing to point at but the file,
    // which is exactly where its `//!` lines are written.
    let module = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file)?;
    let first = module.ast.items.first().map_or(u32::MAX, |item| item.span().start);
    if module.ast.docs.is_empty() || offset > first {
        return None;
    }
    let at = crate::diagnostics::Span::point(file, offset as usize);
    Some(rendered(text, &format!("from \"{}\"", module.path), &module.ast.docs, at))
}

/// The hover payload: the signature in a fence, then the doc comment under it.
fn rendered(
    text: &str,
    signature: &str,
    docs: &[String],
    span: crate::diagnostics::Span,
) -> Value {
    let mut markdown = format!("```buri\n{signature}\n```");
    if !docs.is_empty() {
        markdown.push_str("\n\n");
        markdown.push_str(&docs.join("\n"));
    }
    Value::object(vec![("contents", markup(&markdown)), ("range", convert::range(text, span))])
}

fn markup(markdown: &str) -> Value {
    Value::object(vec![("kind", Value::str("markdown")), ("value", Value::str(markdown))])
}

/// Where the name under the cursor was declared.
pub fn definition(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    // An import's path names a file, and the workspace resolves it whether or
    // not the module behind it was loaded — a path whose package this target
    // does not yet depend on is exactly when a reader wants to go and look.
    if let Some(module_path) = import_path_at(analyzed, path, offset) {
        return import_target(analyzed, &module_path);
    }
    let found = symbols::at(analyzed, path, text, offset)?;
    location(analyzed, symbols::declaration(analyzed, &found.symbol))
}

/// The module path of the import or re-export whose path string covers the
/// offset. The span is the one every import diagnostic is anchored on, so
/// "inside the path" means the string with its quotes.
fn import_path_at(analyzed: &Analyzed, path: &Path, offset: u32) -> Option<String> {
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;
    let module = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file)?;
    module.ast.items.iter().find_map(|item| {
        let (written, span) = match item {
            Item::Import(i) => (&i.path, i.path_span),
            Item::ReExport(r) => (&r.path, r.path_span),
            _ => return None,
        };
        let covered = span.file == file && span.start <= offset && offset <= span.end;
        covered.then(|| written.clone())
    })
}

/// The file a module path resolves to — the top of it, since a module has no
/// name of its own to point at.
///
/// A `core/...` path resolves to a module that is `include_str!`d into the
/// binary and has no file to open, so it answers with nothing rather than with
/// a guess. So does a path that resolves to nothing at all.
fn import_target(analyzed: &Analyzed, module_path: &str) -> Option<Value> {
    let resolved = analyzed.session.workspace.resolve_module(module_path).ok()?;
    Some(convert::top_of(&resolved.in_package()?.file))
}

/// Every place the repository names the symbol under the cursor.
///
/// The cursor may be on the declaration or on any use — both are the same
/// question to [`symbols::at`], and the answer is the same list either way,
/// which is what makes "find every use of this" work from wherever you happen
/// to be reading.
///
/// `analyzed` here is the whole repository rather than one target, because a
/// name is referred to from wherever it is imported and nothing about the file
/// it was declared in bounds that set. See `State::analyze_workspace`.
pub fn references(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
    include_declaration: bool,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let found = symbols::at(analyzed, path, text, offset)?;
    let mut spans = symbols::references(analyzed, &found.symbol);
    if include_declaration {
        spans.push(symbols::declaration(analyzed, &found.symbol));
    }
    // One file is reachable through several targets and a name can be written
    // twice in one expression, so the same place arrives more than once. Sorted
    // by file and then by position: the protocol imposes no order, and an
    // editor's list should not depend on which body the scan met first.
    let mut out: Vec<(String, crate::diagnostics::Span)> = spans
        .into_iter()
        .filter_map(|span| Some((uri_of(analyzed, span)?, span)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.start.cmp(&b.1.start)).then(a.1.end.cmp(&b.1.end)));
    out.dedup_by(|a, b| a.0 == b.0 && a.1.start == b.1.start && a.1.end == b.1.end);
    Some(Value::Array(
        out.iter()
            .map(|(uri, span)| {
                let text = &analyzed.session.map.get(span.file).text;
                Value::object(vec![
                    ("uri", Value::str(uri.as_str())),
                    ("range", convert::range(text, *span)),
                ])
            })
            .collect(),
    ))
}

/// The file a span is in, or nothing when it has none — a span with no file, or
/// the embedded standard library, which is `include_str!`d into the binary.
fn uri_of(analyzed: &Analyzed, span: crate::diagnostics::Span) -> Option<String> {
    if span.is_none() {
        return None;
    }
    let f = analyzed.session.map.get(span.file);
    if f.abs_path.as_os_str().is_empty() {
        return None;
    }
    Some(convert::uri_of(&f.abs_path))
}

fn location(analyzed: &Analyzed, span: crate::diagnostics::Span) -> Option<Value> {
    if span.is_none() {
        return None;
    }
    let f = analyzed.session.map.get(span.file);
    // The standard library has no file on disk — it is `include_str!`d into the
    // binary — so there is nowhere to send the editor.
    if f.abs_path.as_os_str().is_empty() {
        return None;
    }
    let text = &f.text;
    Some(Value::object(vec![
        ("uri", Value::str(convert::uri_of(&f.abs_path))),
        ("range", convert::range(text, span)),
    ]))
}

/// The outline. Built from the AST alone, so it still works in a file that does
/// not typecheck — which is when an outline is worth most.
///
/// One parameter, not two. It took `text` and `source` separately, and the
/// ranges it returns are byte offsets into the AST parsed out of `source`
/// resolved against `text` — so the two had to be the same string for any
/// answer to be right, and nothing said so. The one caller passed the same
/// string twice.
pub fn document_symbols(text: &str) -> Value {
    let parsed = crate::parsing::parser::parse(text, crate::diagnostics::FileId(0));
    let mut out = Vec::new();
    for item in &parsed.module.items {
        // 12 function, 23 struct, 10 enum, 5 class (trait), 14 constant,
        // 26 type parameter (alias) — the protocol's SymbolKind numbers.
        let (name, kind) = match item {
            Item::Fn(d) => (d.name, 12),
            Item::Struct(d) => (d.name, 23),
            Item::Enum(d) => (d.name, 10),
            Item::Trait(d) => (d.name, 5),
            Item::Let(d) => (d.name, 14),
            Item::TypeAlias(d) => (d.name, 26),
            _ => continue,
        };
        out.push(Value::object(vec![
            ("name", Value::str(parsed.module.tree.name(name))),
            ("kind", Value::number(kind)),
            ("range", convert::range(text, item.span())),
            ("selectionRange", convert::range(text, name.span)),
        ]));
    }
    Value::Array(out)
}

/// Completion, for the two places that need no type information and are the
/// two the CLI reference promises: inside a module path, and inside the `{ … }`
/// of an import.
pub fn completion(analyzed: &Analyzed, path: &Path, text: &str, position: Position) -> Value {
    let offset = convert::offset_of(text, position) as usize;
    // `offset_of` clamps into `text` and onto a character boundary, so the only
    // way this fails is a caller that did not go through it.
    let Some(before_cursor) = text.get(..offset) else {
        return Value::Array(Vec::new());
    };
    let line = before_cursor.rsplit('\n').next().unwrap_or(before_cursor);

    // An odd number of quotes before the cursor means the cursor is inside
    // one. Counting is what distinguishes `from "core/st|` — still typing the
    // path — from `from "core/str" |`, where the string is closed and the
    // answer is a different one.
    let inside_string = line.bytes().filter(|c| *c == b'"').count() % 2 == 1;
    if inside_string && line.trim_start().starts_with("from") {
        let prefix = line.rsplit('"').next().unwrap_or(line);
        return module_paths(analyzed, path, prefix);
    }

    // Inside the `{ … }` of `from "path" import { … }`.
    if !inside_string && line.contains("import") && line.rfind('{') > line.rfind('}') {
        if let Some(p) = line.split('"').nth(1) {
            return exported_names(analyzed, p);
        }
    }

    Value::Array(Vec::new())
}

/// Every module the file could legally import: the standard library, and the
/// packages its own target already declares. Offering a label the target does
/// not depend on would be offering a `missing-dep`.
fn module_paths(analyzed: &Analyzed, path: &Path, prefix: &str) -> Value {
    let mut out: Vec<String> = standard_library::MODULES.iter().map(|m| m.path.to_string()).collect();
    if let Some(package) = analyzed.session.workspace.owning_package(path) {
        for t in analyzed.session.workspace.targets().into_iter().filter(|t| t.package == package) {
            for d in analyzed.session.workspace.declared_deps(t) {
                out.push(d.value.clone());
            }
        }
        out.push(analyzed.session.workspace.package(package).label());
    }
    out.sort();
    out.dedup();
    Value::Array(
        out.iter()
            .filter(|m| m.starts_with(prefix))
            .map(|m| {
                Value::object(vec![
                    ("label", Value::str(m)),
                    // 9 module.
                    ("kind", Value::number(9)),
                ])
            })
            .collect(),
    )
}

/// What a module exports, for the `{ … }` half.
fn exported_names(analyzed: &Analyzed, path: &str) -> Value {
    let Some(&id) = analyzed.analysis.loaded.by_path.get(path) else {
        return Value::Array(Vec::new());
    };
    let Some(scope) = analyzed.analysis.checked.scopes.get(id.index()) else {
        return Value::Array(Vec::new());
    };
    let mut names: Vec<&String> = scope.exports.keys().collect();
    names.sort();
    Value::Array(
        names
            .iter()
            .map(|n| {
                Value::object(vec![
                    ("label", Value::str(n.as_str())),
                    // 6 variable — the protocol has no "exported name".
                    ("kind", Value::number(6)),
                ])
            })
            .collect(),
    )
}
