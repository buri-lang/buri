//! Who implements what.
//!
//! `textDocument/implementation` and the type hierarchy are one relation asked
//! in three directions, and the compiler already holds it. `Tables.impls` is
//! keyed by `(trait, type)` and every entry carries the span of the `impl` or
//! the `derive` that recorded it, so nothing here walks anything: the answers
//! are a filter over a table the checker filled in.
//!
//! The relation is read out of the whole-repository analysis rather than out of
//! one target's, for the reason `references` is: an `impl` lives in the module
//! that declares its type (SPEC rule 22), and which targets those are is not
//! something the file under the cursor knows.

use crate::compiler::semantics::types::TyDef;
use crate::diagnostics::Span;
use crate::json::Value;
use std::path::Path;
use super::convert::{self, Position};
use super::features;
use super::state::Analyzed;
use super::symbols::{self, Symbol};

/// Every `impl` that answers for the symbol under the cursor.
///
/// Three things can be implemented and each reads the same table: a trait, by
/// every type that conforms to it; a type, by every conformance it was given;
/// and a trait method, by the function each `impl` supplied for it.
///
/// Everything else answers nothing rather than an empty list. "Where is this
/// local implemented" is not a question with no answers — it is not a question,
/// and `null` is what the protocol spells that with.
pub fn implementations(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let found = symbols::at(analyzed, path, text, offset)?;
    let tables = &analyzed.analysis.checked.tables;
    let spans: Vec<Span> = match &found.symbol {
        Symbol::Trait(id) => {
            tables.impls.iter().filter(|((t, _), _)| t == id).map(|(_, i)| i.span).collect()
        }
        Symbol::Type(con) => {
            tables.impls.iter().filter(|((_, c), _)| c == con).map(|(_, i)| i.span).collect()
        }
        // The method the `impl` wrote. A `derive` has no function to point at,
        // so the `derive` itself is the answer.
        Symbol::TraitMethod { trait_id, method } => tables
            .impls
            .iter()
            .filter(|((t, _), _)| t == trait_id)
            .map(|(_, info)| {
                info.method(*method)
                    .map(|id| tables.fn_info(id).span)
                    .filter(|span| !span.is_none())
                    .unwrap_or(info.span)
            })
            .collect(),
        _ => return None,
    };
    Some(features::locations(analyzed, spans))
}

/// The symbol under the cursor as the root of a type hierarchy.
///
/// A hierarchy in this language relates types to traits, so a type and a trait
/// are the only two things that are a node in one. Anything else answers
/// `null`, and the editor does not open a panel it would have nothing to put
/// in.
pub fn prepare(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let found = symbols::at(analyzed, path, text, offset)?;
    Some(Value::Array(vec![item(analyzed, &found.symbol)?]))
}

/// What the item is under: for a type, every trait it implements.
///
/// A trait is under nothing. Buri has no trait inheritance — a `trait` declares
/// methods and names no other trait — so the answer for one is empty rather
/// than absent.
pub fn supertypes(analyzed: &Analyzed, symbol: &Symbol) -> Value {
    let tables = &analyzed.analysis.checked.tables;
    let found = match symbol {
        Symbol::Type(con) => tables
            .impls
            .keys()
            .filter(|(_, c)| c == con)
            .map(|(t, _)| Symbol::Trait(*t))
            .collect(),
        _ => Vec::new(),
    };
    items(analyzed, found)
}

/// What is under the item: for a trait, every type that implements it.
///
/// A type has nothing under it. Buri has no subtyping, so no type is beneath
/// another one — which is the reason the hierarchy is a trait relation here and
/// not the reason it is missing.
pub fn subtypes(analyzed: &Analyzed, symbol: &Symbol) -> Value {
    let tables = &analyzed.analysis.checked.tables;
    let found = match symbol {
        Symbol::Trait(id) => {
            tables.impls.keys().filter(|(t, _)| t == id).map(|(_, c)| Symbol::Type(*c)).collect()
        }
        _ => Vec::new(),
    };
    items(analyzed, found)
}

/// A list of hierarchy items, in a fixed order.
///
/// `Tables.impls` is a `HashMap` and its iteration order is not the same twice,
/// so a list built from it has to be sorted before an editor sees it.
fn items(analyzed: &Analyzed, symbols: Vec<Symbol>) -> Value {
    let mut out: Vec<(String, String, u32, Value)> = symbols
        .iter()
        .filter_map(|symbol| {
            let rendered = item(analyzed, symbol)?;
            let name = rendered.get("name")?.as_str()?.to_string();
            let uri = rendered.get("uri")?.as_str()?.to_string();
            let start = rendered.at("selectionRange.start.line")?.as_u32()?;
            Some((name, uri, start, rendered))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    out.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1 && a.2 == b.2);
    Value::Array(out.into_iter().map(|(_, _, _, rendered)| rendered).collect())
}

/// One `TypeHierarchyItem`.
///
/// A declaration compiled into the binary has no file to name, and an item with
/// no `uri` is not one the protocol allows — so the standard library's types
/// and traits are left out of a hierarchy rather than pointed at nowhere, which
/// is what `definition` already does with them.
fn item(analyzed: &Analyzed, symbol: &Symbol) -> Option<Value> {
    let tables = &analyzed.analysis.checked.tables;
    // The protocol's kinds, the same numbers `workspace/symbol` reports.
    let kind = match symbol {
        Symbol::Trait(_) => 5,
        Symbol::Type(id) => match tables.tycon(*id).def {
            TyDef::Enum { .. } => 10,
            _ => 23,
        },
        _ => return None,
    };
    let name = symbols::name(analyzed, symbol)?;
    let selection = symbols::declaration_name(analyzed, symbol);
    if selection.is_none() {
        return None;
    }
    let file = analyzed.session.map.get(selection.file);
    if file.abs_path.as_os_str().is_empty() {
        return None;
    }
    let uri = convert::uri_of(&file.abs_path);
    let position = convert::position_of(&file.text, selection.start);
    Some(Value::object(vec![
        ("name", Value::str(name)),
        ("kind", Value::number(kind)),
        ("uri", Value::str(&uri)),
        // The whole declaration, with the name inside it: the protocol asks
        // for both, and they differ.
        ("range", convert::range(&file.text, symbols::declaration_extent(analyzed, symbol))),
        ("selectionRange", convert::range(&file.text, selection)),
        // What the two walking requests resolve the item back from, since the
        // protocol hands them the item rather than a position.
        (
            "data",
            Value::object(vec![
                ("uri", Value::str(&uri)),
                ("position", position.to_json()),
            ]),
        ),
    ]))
}
