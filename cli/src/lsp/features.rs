//! The requests that answer a question about a name.
//!
//! All four are reads of what the compiler already computed. `hir::Expr`
//! carries its own type and span, `Tables` carries every declaration's span,
//! and `ModuleScope` carries what a module exports — so hover, go-to-definition
//! and completion need no index of their own. That is the whole reason this
//! file is short: the answers were already in the analysis, waiting to be
//! asked for.

use super::convert::{self, Position};
use super::state::Analyzed;
use crate::ast::Item;
use crate::json::Value;
use crate::types::FnId;
use std::path::Path;

/// The innermost typed expression covering `offset`, and its rendered type.
pub fn hover(a: &Analyzed, path: &Path, text: &str, pos: Position) -> Option<Value> {
    let offset = convert::offset_of(text, pos);
    let file = a.session.map.find(&a.session.ws.rel_of(path))?;

    // A declaration's own name renders as its signature plus its doc comment,
    // which is what you want when you point at `fn parse` — the type of the
    // name is the least interesting thing about it.
    if let Some((sig, docs, span)) = declaration_at(a, path, offset) {
        let mut md = format!("```buri\n{sig}\n```");
        if !docs.is_empty() {
            md.push_str("\n\n");
            md.push_str(&docs.join("\n"));
        }
        return Some(Value::obj(vec![
            ("contents", markup(&md)),
            ("range", convert::range(text, span)),
        ]));
    }

    // Otherwise the innermost expression whose span contains the offset. It is
    // innermost because a smaller span is always the more specific answer.
    let mut best: Option<(u32, String, crate::diag::Span)> = None;
    for (fid, body) in &a.analysis.checked.bodies {
        if a.analysis.checked.tables.fun(*fid).span.file != file {
            continue;
        }
        crate::hir::walk(&body.expr, &mut |e| {
            if e.span.file != file || e.span.start > offset || e.span.end < offset {
                return;
            }
            let width = e.span.end - e.span.start;
            if best.as_ref().is_none_or(|(w, _, _)| width < *w) {
                let ty = crate::types::show(&a.analysis.checked.tables, None, &[], &e.ty);
                best = Some((width, ty, e.span));
            }
        });
    }
    let (_, ty, span) = best?;
    Some(Value::obj(vec![
        ("contents", markup(&format!("```buri\n{ty}\n```"))),
        ("range", convert::range(text, span)),
    ]))
}

fn markup(md: &str) -> Value {
    Value::obj(vec![("kind", Value::str("markdown")), ("value", Value::str(md))])
}

/// The declaration whose *name* covers `offset`, rendered the way the formatter
/// would print it.
fn declaration_at(
    a: &Analyzed,
    path: &Path,
    offset: u32,
) -> Option<(String, Vec<String>, crate::diag::Span)> {
    let file = a.session.map.find(&a.session.ws.rel_of(path))?;
    let m = a.analysis.loaded.modules.iter().find(|m| m.file == file)?;
    for item in &m.ast.items {
        // A function renders as its signature, which is what `buri format`
        // prints; everything else renders as the keyword and its name, because
        // a struct's whole body is not what you asked for by pointing at it.
        let (name, docs, sig) = match item {
            Item::Fn(d) => (&d.name, &d.docs, crate::format::signature(d)),
            Item::Struct(d) => (&d.name, &d.docs, format!("struct {}", d.name.name)),
            Item::Enum(d) => (&d.name, &d.docs, format!("enum {}", d.name.name)),
            Item::TypeAlias(d) => (&d.name, &d.docs, format!("type {}", d.name.name)),
            Item::Const(d) => (&d.name, &d.docs, format!("const {}", d.name.name)),
            Item::Trait(d) => (&d.name, &d.docs, format!("trait {}", d.name.name)),
            _ => continue,
        };
        if name.span.start <= offset && offset <= name.span.end {
            return Some((sig, docs.clone(), name.span));
        }
    }
    None
}

/// Where the name under the cursor was declared.
pub fn definition(a: &Analyzed, path: &Path, text: &str, pos: Position) -> Option<Value> {
    let offset = convert::offset_of(text, pos);
    let file = a.session.map.find(&a.session.ws.rel_of(path))?;

    // The innermost call or function reference covering the offset. Walking to
    // the innermost matters for `f(g(x))`, where both spans contain a cursor
    // inside `g`.
    let mut best: Option<(u32, FnId)> = None;
    for (fid, body) in &a.analysis.checked.bodies {
        if a.analysis.checked.tables.fun(*fid).span.file != file {
            continue;
        }
        crate::hir::walk(&body.expr, &mut |e| {
            if e.span.file != file || e.span.start > offset || e.span.end < offset {
                return;
            }
            let target = match &e.kind {
                crate::hir::ExprKind::CallFn { func, .. } => *func,
                crate::hir::ExprKind::FnRef(f, _) => *f,
                _ => return,
            };
            let width = e.span.end - e.span.start;
            if best.as_ref().is_none_or(|(w, _)| width < *w) {
                best = Some((width, target));
            }
        });
    }
    let (_, target) = best?;
    let span = a.analysis.checked.tables.fun(target).span;
    location(a, span)
}

fn location(a: &Analyzed, span: crate::diag::Span) -> Option<Value> {
    if span.is_none() {
        return None;
    }
    let f = a.session.map.get(span.file);
    // The standard library has no file on disk — it is `include_str!`d into the
    // binary — so there is nowhere to send the editor.
    if f.abs_path.as_os_str().is_empty() {
        return None;
    }
    let text = &f.text;
    Some(Value::obj(vec![
        ("uri", Value::str(convert::uri_of(&f.abs_path))),
        ("range", convert::range(text, span)),
    ]))
}

/// The outline. Built from the AST alone, so it still works in a file that does
/// not typecheck — which is when an outline is worth most.
pub fn document_symbols(text: &str, source: &str) -> Value {
    let parsed = crate::parse::parse(source, crate::diag::FileId(0));
    let mut out = Vec::new();
    for item in &parsed.module.items {
        // 12 function, 23 struct, 10 enum, 5 class (trait), 14 constant,
        // 26 type parameter (alias) — the protocol's SymbolKind numbers.
        let (name, kind) = match item {
            Item::Fn(d) => (&d.name, 12.0),
            Item::Struct(d) => (&d.name, 23.0),
            Item::Enum(d) => (&d.name, 10.0),
            Item::Trait(d) => (&d.name, 5.0),
            Item::Const(d) => (&d.name, 14.0),
            Item::TypeAlias(d) => (&d.name, 26.0),
            _ => continue,
        };
        out.push(Value::obj(vec![
            ("name", Value::str(&name.name)),
            ("kind", Value::num(kind)),
            ("range", convert::range(text, item.span())),
            ("selectionRange", convert::range(text, name.span)),
        ]));
    }
    Value::Arr(out)
}

/// Completion, for the two places that need no type information and are the
/// two the CLI reference promises: inside a module path, and inside the `{ … }`
/// of an import.
pub fn completion(a: &Analyzed, path: &Path, text: &str, pos: Position) -> Value {
    let offset = convert::offset_of(text, pos) as usize;
    let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line = &text[line_start..offset];

    // An odd number of quotes before the cursor means the cursor is inside
    // one. Counting is what distinguishes `from "core/st|` — still typing the
    // path — from `from "core/str" |`, where the string is closed and the
    // answer is a different one.
    let inside_string = line.bytes().filter(|c| *c == b'"').count() % 2 == 1;
    if inside_string && line.trim_start().starts_with("from") {
        let prefix = &line[line.rfind('"').map(|i| i + 1).unwrap_or(0)..];
        return module_paths(a, path, prefix);
    }

    // Inside the `{ … }` of `from "path" import { … }`.
    if !inside_string && line.contains("import") && line.rfind('{') > line.rfind('}') {
        if let Some(p) = line.split('"').nth(1) {
            return exported_names(a, p);
        }
    }

    Value::Arr(Vec::new())
}

/// Every module the file could legally import: the standard library, and the
/// packages its own target already declares. Offering a label the target does
/// not depend on would be offering a `missing-dep`.
fn module_paths(a: &Analyzed, path: &Path, prefix: &str) -> Value {
    let mut out: Vec<String> = crate::stdlib::MODULES.iter().map(|m| m.to_string()).collect();
    if let Some(pkg) = a.session.ws.owning_package(path) {
        for t in a.session.ws.targets().into_iter().filter(|t| t.pkg == pkg) {
            for d in a.session.ws.declared_deps(t) {
                out.push(d.value.clone());
            }
        }
        out.push(a.session.ws.pkg(pkg).label());
    }
    out.sort();
    out.dedup();
    Value::Arr(
        out.iter()
            .filter(|m| m.starts_with(prefix))
            .map(|m| {
                Value::obj(vec![
                    ("label", Value::str(m)),
                    // 9 module.
                    ("kind", Value::num(9.0)),
                ])
            })
            .collect(),
    )
}

/// What a module exports, for the `{ … }` half.
fn exported_names(a: &Analyzed, path: &str) -> Value {
    let Some(&id) = a.analysis.loaded.by_path.get(path) else {
        return Value::Arr(Vec::new());
    };
    let Some(scope) = a.analysis.checked.scopes.get(id.index()) else {
        return Value::Arr(Vec::new());
    };
    let mut names: Vec<&String> = scope.exports.keys().collect();
    names.sort();
    Value::Arr(
        names
            .iter()
            .map(|n| {
                Value::obj(vec![
                    ("label", Value::str(n.as_str())),
                    // 6 variable — the protocol has no "exported name".
                    ("kind", Value::num(6.0)),
                ])
            })
            .collect(),
    )
}
