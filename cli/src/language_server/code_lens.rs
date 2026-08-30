//! The lines an editor draws above a declaration.
//!
//! Two lenses, and they are the two things a reader of a Buri file wants from
//! one: run this test, and how many places use this name.
//!
//! **The full pass reads a parse and nothing else.** A client asks for the
//! lenses of every file it scrolls through, so anything the pass does is paid
//! per scroll — and counting references means analysing the whole repository,
//! which is the most expensive thing this server does. So the count is not
//! computed here. The reference lens goes out with a range and a `data`, no
//! command, and `codeLens/resolve` does the counting for the one lens an editor
//! is about to draw. That split is what `resolveProvider: true` claims.

use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols;
use crate::diagnostics::{FileId, Span};
use crate::json::Value;
use crate::parsing::parser;
use crate::parsing::tree::Item;
use std::path::Path;

/// Run the one test the lens sits above.
pub const RUN_TEST: &str = "buri.runTest";

/// Show the places the reference count counted.
pub const SHOW_REFERENCES: &str = "buri.showReferences";

/// Every lens in one file.
///
/// A `test` gets a command straight away: what it needs is the sentence the
/// test was written with, and the parse has that. An exported declaration gets
/// an unresolved lens, for the reason in the module docs.
pub fn lenses(path: &Path, text: &str) -> Value {
    let parsed = parser::parse(text, FileId(0));
    let uri = convert::uri_of(path);
    let mut out = Vec::new();
    for item in &parsed.module.items {
        if let Item::Test(d) = item {
            out.push(run_test(text, &uri, d.name_span, &d.name));
            continue;
        }
        if let Some(name) = exported_name(item) {
            out.push(unresolved(text, &uri, name));
        }
    }
    Value::Array(out)
}

/// The name span of a declaration this module exports, if it exports it.
///
/// A lens above something the rest of the repository cannot reach would be
/// counting uses inside one file, which is what `documentHighlight` already
/// paints. An `impl` and a `derive` have no name of their own to hang one on.
fn exported_name(item: &Item) -> Option<Span> {
    match item {
        Item::Fn(d) => d.exported.then_some(d.name.span),
        Item::Struct(d) => d.exported.then_some(d.name.span),
        Item::Enum(d) => d.exported.then_some(d.name.span),
        Item::TypeAlias(d) => d.exported.then_some(d.name.span),
        Item::Let(d) => d.exported.then_some(d.name.span),
        Item::Trait(d) => d.exported.then_some(d.name.span),
        Item::Context(d) => d.exported.then_some(d.name.span),
        Item::Import(_)
        | Item::ReExport(_)
        | Item::Impl(_)
        | Item::Derive(_)
        | Item::Test(_)
        | Item::Error(_) => None,
    }
}

/// The lens above a `test`, complete as it leaves.
///
/// The title quotes the sentence rather than saying "Run test", because a file
/// holds several and a column of identical lenses says nothing about which line
/// each one belongs to. The arguments are the file and that same sentence: the
/// file is what says which repository and which target, and the sentence is
/// what `--filter` takes.
fn run_test(text: &str, uri: &str, span: Span, name: &str) -> Value {
    Value::object(vec![
        ("range", convert::range(text, span)),
        (
            "command",
            Value::object(vec![
                ("title", Value::str(format!("Run \"{name}\""))),
                ("command", Value::str(RUN_TEST)),
                (
                    "arguments",
                    Value::Array(vec![Value::str(uri), Value::str(name)]),
                ),
            ]),
        ),
    ])
}

/// The lens above an exported declaration, with the count left out.
///
/// `data` names where the declaration writes its name — the same round trip a
/// `TypeHierarchyItem` and an inlay hint make, and for the same reason: a
/// server that remembered every lens it had produced would hold a table nothing
/// removes an entry from. The range is that same span, so the lens's line and
/// the symbol the resolve looks for cannot drift apart.
fn unresolved(text: &str, uri: &str, span: Span) -> Value {
    Value::object(vec![
        ("range", convert::range(text, span)),
        (
            "data",
            Value::object(vec![
                ("uri", Value::str(uri)),
                ("position", convert::position_of(text, span.start).to_json()),
            ]),
        ),
    ])
}

/// The count, and the command that shows what was counted.
///
/// The declaration itself is not in the list: the lens sits on it, and "3
/// references" that included the line the lens is drawn above would be counting
/// the reader's own cursor. That is the same `includeDeclaration: false` a
/// client sends when it asks "where else is this used".
///
/// A lens whose `data` resolves to nothing — a file in no open repository, a
/// declaration an edit has since moved — comes back as it went in. The
/// protocol's result for this request is a code lens, and an unresolved one is
/// still one; `null` is not a legal answer to it.
pub fn resolve(analyzed: &Analyzed, path: &Path, text: &str, lens: &Value) -> Value {
    let mut resolved = lens.clone();
    let Some(position) = lens.at("data.position").and_then(Position::from_json) else {
        return resolved;
    };
    let offset = convert::offset_of(text, position);
    let Some(found) = symbols::at(analyzed, path, text, offset) else { return resolved };
    let spans = symbols::references(analyzed, &found.symbol);
    let locations = super::features::locations(analyzed, spans);
    let count = match &locations {
        Value::Array(items) => items.len(),
        _ => 0,
    };
    let Value::Object(fields) = &mut resolved else { return resolved };
    fields.insert(
        "command".to_string(),
        Value::object(vec![
            (
                "title",
                Value::str(format!("{count} reference{}", if count == 1 { "" } else { "s" })),
            ),
            ("command", Value::str(SHOW_REFERENCES)),
            (
                "arguments",
                Value::Array(vec![
                    Value::str(convert::uri_of(path)),
                    position.to_json(),
                    locations,
                ]),
            ),
        ]),
    );
    resolved
}
