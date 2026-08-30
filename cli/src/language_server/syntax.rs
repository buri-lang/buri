//! The requests a parse alone can answer.
//!
//! An outline, a set of folds and a chain of selections are all questions about
//! shape rather than about meaning: none of them needs a workspace, a standard
//! library or a type. So all three re-parse the one buffer and read the tree —
//! which is what makes them work in a file that does not typecheck, and that is
//! exactly when an outline is worth most.
//!
//! The tree they read is the flat one. A declaration holds the id of its body
//! and of every type it names, and the arenas behind those ids are the only
//! place a span exists for an expression, a pattern or a written type — so
//! "every span covering this offset" is a scan over three arrays rather than a
//! walk over anything.

use crate::diagnostics::{FileId, Span};
use crate::json::Value;
use crate::parsing::flat::{ExprId, PatId, TypeId};
use crate::parsing::parser;
use crate::parsing::tree::{Item, Module, StructBody, VariantPayload};
use super::convert::{self, Position};

/// The outline, nested: an `impl` holds its methods, a struct its fields, an
/// enum its variants.
///
/// The protocol's two shapes for this answer are a flat list of
/// `SymbolInformation` and a tree of `DocumentSymbol`, and what this returned
/// before was already the second one — `range` and `selectionRange` are its
/// fields and a `SymbolInformation` has neither. So the nesting is the shape
/// being finished rather than swapped: a client reading it before reads it now.
pub fn document_symbols(text: &str) -> Value {
    let parsed = parser::parse(text, FileId(0));
    let module = &parsed.module;
    let mut out = Vec::new();
    for item in &module.items {
        if let Some(symbol) = item_symbol(text, module, item) {
            out.push(symbol);
        }
    }
    Value::Array(out)
}

/// The same outline as the protocol's other shape: a flat
/// `SymbolInformation[]`.
///
/// This is what a client that did not claim `hierarchicalDocumentSymbolSupport`
/// is entitled to read the reply as, and the two shapes differ in more than
/// nesting — a `SymbolInformation` requires a `location`, which a
/// `DocumentSymbol` does not carry at all. The nesting becomes `containerName`,
/// which is the only place the flat shape has to put it.
pub fn flattened(outline: &Value, uri: &str) -> Value {
    let mut out = Vec::new();
    flatten_into(outline, uri, None, &mut out);
    Value::Array(out)
}

fn flatten_into(symbols: &Value, uri: &str, container: Option<&str>, out: &mut Vec<Value>) {
    let Some(items) = symbols.as_array() else { return };
    for item in items {
        let Some(name) = item.get("name").and_then(|n| n.as_str()) else { continue };
        let mut fields = vec![
            ("name", Value::str(name)),
            ("kind", item.get("kind").cloned().unwrap_or(Value::Null)),
            (
                "location",
                Value::object(vec![
                    ("uri", Value::str(uri)),
                    ("range", item.get("range").cloned().unwrap_or(Value::Null)),
                ]),
            ),
        ];
        if let Some(container) = container {
            fields.push(("containerName", Value::str(container)));
        }
        out.push(Value::object(fields));
        if let Some(children) = item.get("children") {
            flatten_into(children, uri, Some(name), out);
        }
    }
}

/// The protocol's `SymbolKind` numbers, named where they are used rather than
/// spelled as bare integers at seven call sites.
mod kind {
    pub const METHOD: i64 = 6;
    pub const FIELD: i64 = 8;
    pub const ENUM: i64 = 10;
    pub const FUNCTION: i64 = 12;
    pub const CONSTANT: i64 = 14;
    pub const OBJECT: i64 = 19;
    pub const ENUM_MEMBER: i64 = 22;
    pub const STRUCT: i64 = 23;
    pub const TYPE_PARAMETER: i64 = 26;
    /// The protocol has no trait and no effect. `Class` is what every server
    /// for a language with traits reports, so it is what an editor's outline
    /// icons already expect.
    pub const CLASS: i64 = 5;
}

fn item_symbol(text: &str, module: &Module, item: &Item) -> Option<Value> {
    let tree = &module.tree;
    match item {
        Item::Fn(d) => {
            Some(symbol(text, tree.name(d.name), kind::FUNCTION, d.span, d.name.span, Vec::new()))
        }
        Item::Struct(d) => {
            let fields = match &d.body {
                StructBody::Record(fields) => fields
                    .iter()
                    .map(|f| {
                        let name = tree.name(f.name);
                        symbol(text, name, kind::FIELD, f.span, f.name.span, Vec::new())
                    })
                    .collect(),
                // A tuple field has no name, so there is nothing to list.
                StructBody::Tuple(_) => Vec::new(),
            };
            Some(symbol(text, tree.name(d.name), kind::STRUCT, d.span, d.name.span, fields))
        }
        Item::Enum(d) => {
            let variants = d
                .variants
                .iter()
                .map(|v| {
                    let fields = match &v.payload {
                        VariantPayload::Record(fields) => fields
                            .iter()
                            .map(|f| {
                                symbol(
                                    text,
                                    tree.name(f.name),
                                    kind::FIELD,
                                    f.span,
                                    f.name.span,
                                    Vec::new(),
                                )
                            })
                            .collect(),
                        VariantPayload::None | VariantPayload::Tuple(_) => Vec::new(),
                    };
                    symbol(text, tree.name(v.name), kind::ENUM_MEMBER, v.span, v.name.span, fields)
                })
                .collect();
            Some(symbol(text, tree.name(d.name), kind::ENUM, d.span, d.name.span, variants))
        }
        Item::Trait(d) => {
            let methods = d
                .methods
                .iter()
                .map(|m| {
                    symbol(text, tree.name(m.name), kind::METHOD, m.span, m.name.span, Vec::new())
                })
                .collect();
            Some(symbol(text, tree.name(d.name), kind::CLASS, d.span, d.name.span, methods))
        }
        Item::Impl(d) => {
            // An `impl` has no name of its own, so it is named by its head —
            // which is also the only part of it a reader scans for.
            let self_ty = crate::formatting::type_text(tree, d.self_ty);
            let name = match d.trait_ty {
                Some(t) => format!("impl {} for {self_ty}", crate::formatting::type_text(tree, t)),
                None => format!("impl {self_ty}"),
            };
            let methods = d
                .methods
                .iter()
                .map(|m| {
                    symbol(text, tree.name(m.name), kind::METHOD, m.span, m.name.span, Vec::new())
                })
                .collect();
            Some(symbol(text, &name, kind::OBJECT, d.span, tree.type_span(d.self_ty), methods))
        }
        Item::Let(d) => {
            Some(symbol(text, tree.name(d.name), kind::CONSTANT, d.span, d.name.span, Vec::new()))
        }
        Item::TypeAlias(d) => Some(symbol(
            text,
            tree.name(d.name),
            kind::TYPE_PARAMETER,
            d.span,
            d.name.span,
            Vec::new(),
        )),
        Item::Context(d) => {
            Some(symbol(text, tree.name(d.name), kind::STRUCT, d.span, d.name.span, Vec::new()))
        }
        // A test is named by a sentence rather than by an identifier, and it is
        // listed because finding one is the commonest reason to open the file
        // it is in.
        Item::Test(d) => {
            Some(symbol(text, &d.name, kind::FUNCTION, d.span, d.name_span, Vec::new()))
        }
        Item::Import(_) | Item::ReExport(_) | Item::Derive(_) | Item::Error(_) => None,
    }
}

fn symbol(
    text: &str,
    name: &str,
    kind: i64,
    range: Span,
    selection: Span,
    children: Vec<Value>,
) -> Value {
    let mut fields = vec![
        ("name", Value::str(name)),
        ("kind", Value::number(kind)),
        ("range", convert::range(text, range)),
        ("selectionRange", convert::range(text, selection)),
    ];
    if !children.is_empty() {
        fields.push(("children", Value::Array(children)));
    }
    Value::object(fields)
}

// ---------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------

/// What an editor may collapse: every declaration that spans more than a line,
/// the methods inside an `impl` or a `trait`, and the run of imports at the top.
///
/// A fold ends on the **last line of the body**, not on the closing brace's own
/// line, so that collapsing a function leaves its `}` visible. That is exact
/// rather than a guess because `buri format` is canonical and puts a closing
/// brace on a line of its own.
pub fn folding_ranges(text: &str) -> Value {
    let parsed = parser::parse(text, FileId(0));
    let module = &parsed.module;
    let mut out = Vec::new();

    // The imports, as one region. Consecutive because an import may only
    // appear above the first declaration, and folding them separately would
    // offer a fold per line.
    let imports: Vec<Span> = module
        .items
        .iter()
        .take_while(|i| matches!(i, Item::Import(_) | Item::ReExport(_)))
        .map(Item::span)
        .collect();
    if let (Some(first), Some(last)) = (imports.first(), imports.last()) {
        let (start, end) = (line_of(text, first.start), line_of(text, last.end));
        if end > start {
            out.push(fold(start, end, Some("imports")));
        }
    }

    for item in &module.items {
        if let Some(region) = body_fold(text, item.span()) {
            out.push(region);
        }
        let methods = match item {
            Item::Impl(d) => &d.methods,
            Item::Trait(d) => &d.methods,
            _ => continue,
        };
        for m in methods {
            if let Some(region) = body_fold(text, m.span) {
                out.push(region);
            }
        }
    }
    Value::Array(out)
}

/// A region for a span whose body is at least one line tall.
fn body_fold(text: &str, span: Span) -> Option<Value> {
    let start = line_of(text, span.start);
    let end = line_of(text, span.end).saturating_sub(1);
    (end > start).then(|| fold(start, end, None))
}

fn fold(start: u32, end: u32, kind: Option<&str>) -> Value {
    let mut fields =
        vec![("startLine", Value::number(start)), ("endLine", Value::number(end))];
    if let Some(k) = kind {
        fields.push(("kind", Value::str(k)));
    }
    Value::object(fields)
}

fn line_of(text: &str, offset: u32) -> u32 {
    convert::position_of(text, offset).line
}

// ---------------------------------------------------------------------------
// Selection ranges
// ---------------------------------------------------------------------------

/// Expand-selection, one chain per position the client asked about.
///
/// The chain is every span in the file that covers the offset, narrowest first
/// — the identifier under the cursor, then the expression holding it, then the
/// declaration, then the file. Ordering by width is enough to nest them,
/// because spans in a tree either contain one another or do not meet.
pub fn selection_ranges(text: &str, positions: &[Position]) -> Value {
    let parsed = parser::parse(text, FileId(0));
    let tree = &parsed.module.tree;

    Value::Array(
        positions
            .iter()
            .map(|position| {
                let offset = convert::offset_of(text, *position);
                let mut covering: Vec<(u32, u32)> = Vec::new();
                if let Some(word) = word_at(text, offset) {
                    covering.push(word);
                }
                for index in 0..tree.nodes().len() {
                    push_covering(&mut covering, tree.span(ExprId(index as u32)), offset);
                }
                for index in 0..tree.pat_nodes().len() {
                    push_covering(&mut covering, tree.pspan(PatId(index as u32)), offset);
                }
                for index in 0..tree.type_nodes().len() {
                    push_covering(&mut covering, tree.type_span(TypeId(index as u32)), offset);
                }
                for item in &parsed.module.items {
                    push_covering(&mut covering, item.span(), offset);
                }
                covering.push((0, text.len() as u32));

                covering.sort_by_key(|(start, end)| (end.saturating_sub(*start), *start));
                covering.dedup();
                chain(text, &covering)
            })
            .collect(),
    )
}

fn push_covering(out: &mut Vec<(u32, u32)>, span: Span, offset: u32) {
    if !span.is_none() && span.start <= offset && offset <= span.end {
        out.push((span.start, span.end));
    }
}

/// The nested `{ range, parent }` the protocol asks for, built outermost first
/// so that each step is the parent of the one before it.
///
/// A range that does not contain the one it would be the parent of is dropped:
/// two spans of the same width covering one offset are the same span, but a
/// tree that a syntax error left partial can still offer one that is not.
fn chain(text: &str, covering: &[(u32, u32)]) -> Value {
    let mut built: Option<Value> = None;
    let mut inner: Option<(u32, u32)> = None;
    for (start, end) in covering.iter().rev() {
        if let Some((s, e)) = inner {
            if *start < s || *end > e {
                continue;
            }
        }
        let span = Span { file: FileId(0), start: *start, end: *end };
        let mut fields = vec![("range", convert::range(text, span))];
        if let Some(parent) = built.take() {
            fields.push(("parent", parent));
        }
        built = Some(Value::object(fields));
        inner = Some((*start, *end));
    }
    built.unwrap_or(Value::Null)
}

/// The word the offset sits in, as a byte range.
fn word_at(text: &str, offset: u32) -> Option<(u32, u32)> {
    let bytes = text.as_bytes();
    let part = |b: &u8| b.is_ascii_alphanumeric() || *b == b'_';
    let mut start = offset as usize;
    let mut end = offset as usize;
    while let Some(previous) = start.checked_sub(1) {
        if !bytes.get(previous).is_some_and(part) {
            break;
        }
        start = previous;
    }
    while bytes.get(end).is_some_and(part) {
        end = end.saturating_add(1);
    }
    (end > start).then_some((start as u32, end as u32))
}
