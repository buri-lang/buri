//! Who calls this, and what this calls.
//!
//! Both directions are one scan of the same thing: every call a checked body
//! writes, which `symbols::call_names` reads straight off the typed tree. The
//! incoming half runs it over every body and keeps the bodies that mention the
//! symbol; the outgoing half runs it over the one body and keeps what it
//! mentions. Nothing is indexed, and a call hierarchy costs the same whole-
//! repository analysis `references` already pays for.
//!
//! **A call is a written one.** An import clause naming a function is a
//! reference to it and not a call, and `a + b` is a trait method the source
//! never spelled. Neither is in the answer, which is what makes the panel a
//! list of call sites a reader can go and look at.

use crate::diagnostics::Span;
use crate::json::Value;
use std::path::Path;
use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols::{self, Symbol};

/// The symbol under the cursor as the root of a call hierarchy.
///
/// A function and a trait method are the two things a call names, so they are
/// the two that are a node in one. Everything else answers `null`, and the
/// editor does not open a panel with nothing to put in it.
pub fn prepare(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let found = symbols::at(analyzed, path, text, offset)?;
    match found.symbol {
        Symbol::Function(_) | Symbol::TraitMethod { .. } => {}
        _ => return None,
    }
    Some(Value::Array(vec![item(analyzed, &found.symbol)?]))
}

/// Every body that calls the symbol, with the call sites inside it.
///
/// A module-level `let` is a caller too: its value is checked on its own, with
/// no body around it, and a list of callers that quietly dropped one would be
/// wrong rather than shorter.
pub fn incoming(analyzed: &Analyzed, symbol: &Symbol) -> Value {
    let checked = &analyzed.analysis.checked;
    let mut callers: Vec<(Symbol, Vec<Span>)> = Vec::new();
    for (id, body) in &checked.bodies {
        let mut sites = Vec::new();
        symbols::call_names(analyzed, &body.locals, &body.expr, &mut |span, called| {
            if symbols::same(&called, symbol) {
                sites.push(span);
            }
        });
        if !sites.is_empty() {
            callers.push((Symbol::Function(*id), sites));
        }
    }
    for (id, expr) in &checked.consts {
        let mut sites = Vec::new();
        symbols::call_names(analyzed, &[], expr, &mut |span, called| {
            if symbols::same(&called, symbol) {
                sites.push(span);
            }
        });
        if !sites.is_empty() {
            callers.push((Symbol::Const(*id), sites));
        }
    }
    calls(analyzed, "from", callers)
}

/// Everything the symbol's own body calls.
///
/// A trait method answers `[]` rather than the union of what its
/// implementations call: the declaration under the cursor has no body — Buri
/// writes no default method — and the impls' calls belong to the impls.
pub fn outgoing(analyzed: &Analyzed, symbol: &Symbol) -> Value {
    let checked = &analyzed.analysis.checked;
    let mut callees: Vec<(Symbol, Vec<Span>)> = Vec::new();
    let mut take = |span: Span, called: Symbol| {
        match callees.iter_mut().find(|(seen, _)| symbols::same(seen, &called)) {
            Some((_, sites)) => sites.push(span),
            None => callees.push((called, vec![span])),
        }
    };
    match symbol {
        Symbol::Function(id) => {
            if let Some(body) = checked.bodies.get(id) {
                symbols::call_names(analyzed, &body.locals, &body.expr, &mut take);
            }
        }
        Symbol::Const(id) => {
            if let Some(expr) = checked.consts.get(id) {
                symbols::call_names(analyzed, &[], expr, &mut take);
            }
        }
        _ => {}
    }
    calls(analyzed, "to", callees)
}

/// A `CallHierarchyIncomingCall[]` or a `CallHierarchyOutgoingCall[]`, which
/// differ only in the name of the field holding the item.
///
/// `fromRanges` is always where the call is *written*, so each range is
/// converted against its own file's text rather than the item's: for an
/// incoming call the two are the same file, and for an outgoing one they are
/// not.
///
/// `bodies` is a `HashMap`, so its order is not the same twice and every list
/// is sorted before it leaves — by name, then file, then line, the order every
/// other list this server answers with is in.
fn calls(analyzed: &Analyzed, field: &'static str, groups: Vec<(Symbol, Vec<Span>)>) -> Value {
    let mut out: Vec<(String, String, u32, Value)> = groups
        .into_iter()
        .filter_map(|(symbol, mut sites)| {
            let rendered = item(analyzed, &symbol)?;
            let name = rendered.get("name")?.as_str()?.to_string();
            let uri = rendered.get("uri")?.as_str()?.to_string();
            let line = rendered.at("selectionRange.start.line")?.as_u32()?;
            sites.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
            sites.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.file == b.file);
            let ranges = sites
                .iter()
                .map(|span| convert::range(&analyzed.session.map.get(span.file).text, *span))
                .collect();
            let call = Value::object(vec![
                (field, rendered),
                ("fromRanges", Value::Array(ranges)),
            ]);
            Some((name, uri, line, call))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    Value::Array(out.into_iter().map(|(_, _, _, call)| call).collect())
}

/// One `CallHierarchyItem`.
///
/// A declaration compiled into the binary has no file to name, and the
/// protocol has no item without a `uri` — so a call of a standard-library
/// function contributes nothing to the panel rather than pointing at nowhere,
/// which is the silence `definition` already answers with for one.
fn item(analyzed: &Analyzed, symbol: &Symbol) -> Option<Value> {
    let tables = &analyzed.analysis.checked.tables;
    // The protocol's kinds, the same numbers `workspace/symbol` reports.
    let (kind, name, selection) = match symbol {
        Symbol::Function(id) => match symbols::test_title(analyzed, *id) {
            Some((title, span)) => (12, title, span),
            None => {
                let info = tables.fn_info(*id);
                let kind = if info.self_ty.is_some() { 6 } else { 12 };
                (kind, info.name.clone(), info.span)
            }
        },
        Symbol::TraitMethod { .. } => {
            (6, symbols::name(analyzed, symbol)?, symbols::declaration_name(analyzed, symbol))
        }
        Symbol::Const(_) => {
            (14, symbols::name(analyzed, symbol)?, symbols::declaration(analyzed, symbol))
        }
        _ => return None,
    };
    if selection.is_none() {
        return None;
    }
    let file = analyzed.session.map.get(selection.file);
    if file.abs_path.as_os_str().is_empty() {
        return None;
    }
    let uri = convert::uri_of(&file.abs_path);
    let position = convert::position_of(&file.text, selection.start);
    let mut fields = vec![
        ("name", Value::str(name)),
        ("kind", Value::number(kind)),
        ("uri", Value::str(&uri)),
        // The whole declaration, with the name inside it: the protocol asks
        // for both, and for a function they differ.
        ("range", convert::range(&file.text, symbols::declaration_extent(analyzed, symbol))),
        ("selectionRange", convert::range(&file.text, selection)),
        // What the two walking requests resolve the item back from, since the
        // protocol hands them the item rather than a position.
        (
            "data",
            Value::object(vec![("uri", Value::str(&uri)), ("position", position.to_json())]),
        ),
    ];
    if let Some(module) = module_of(analyzed, symbol) {
        // Two functions in a repository may share a name; the module they are
        // in is what tells a reader which panel row is which.
        fields.insert(2, ("detail", Value::str(module)));
    }
    Some(Value::object(fields))
}

/// The module path a declaration is written in.
fn module_of<'a>(analyzed: &'a Analyzed, symbol: &Symbol) -> Option<&'a str> {
    let tables = &analyzed.analysis.checked.tables;
    let module = match symbol {
        Symbol::Function(id) => tables.fn_info(*id).module,
        Symbol::TraitMethod { trait_id, .. } => tables.trait_(*trait_id).module,
        Symbol::Const(id) => tables.const_(*id).module,
        _ => return None,
    };
    analyzed.analysis.loaded.modules.get(module.index()).map(|m| m.path.as_str())
}
