//! The requests that answer a question about a name.
//!
//! Every one is a read of what the compiler already computed. `typed::Expr`
//! carries its own type and span, `Tables` carries every declaration's span,
//! and `ModuleScope` carries what a module exports — so hover, definition, type
//! definition, references, highlights and a workspace query need no index of
//! their own. That is the whole reason this file is short: the answers were
//! already in the analysis, waiting to be asked for.
//!
//! Completion asks the same tables a different question — what could be
//! written here — and is [`super::completion`].

use crate::compiler::semantics::types;
use crate::json::Value;
use crate::parsing::tree::Item;
use std::path::Path;
use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols;

/// Which `MarkupKind` the client said it can render, from its `initialize`.
///
/// A client that named its `contentFormat`s and left `markdown` out of them
/// draws a fence as three literal backticks, so the same content goes out
/// without one.
#[derive(Clone, Copy, PartialEq)]
pub enum Markup {
    Markdown,
    PlainText,
}

impl Markup {
    /// The protocol's name for it, which is what goes in `contents.kind`.
    fn named(self) -> &'static str {
        match self {
            Markup::Markdown => "markdown",
            Markup::PlainText => "plaintext",
        }
    }
}

/// The symbol under the cursor, rendered; failing that, the type of the
/// innermost expression covering it.
pub fn hover(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
    kind: Markup,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;

    // A name renders as its signature plus its doc comment, at the use as well
    // as at the declaration — pointing at a call is the commonest way to ask
    // what a function is, and the type of the name is the least interesting
    // thing about it.
    if let Some(found) = symbols::at(analyzed, path, text, offset) {
        let (signature, docs) = symbols::describe(analyzed, &found.symbol);
        return Some(rendered(text, "buri", &signature, &docs, found.span, kind));
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
        return Some(rendered(text, "buri", &ty, &[], span, kind));
    }

    // Above the first declaration there is nothing to point at but the file,
    // which is exactly where its `//!` lines are written.
    let module = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file)?;
    let first = module.ast.items.first().map_or(u32::MAX, |item| item.span().start);
    if module.ast.docs.is_empty() || offset > first {
        return None;
    }
    let at = crate::diagnostics::Span::point(file, offset as usize);
    Some(rendered(text, "buri", &format!("from \"{}\"", module.path), &module.ast.docs, at, kind))
}

/// The hover payload: the signature in a fence, then the doc comment under it.
///
/// Plain text is the same two parts without the fence — the client asked for a
/// kind that has no way to say "this is code".
///
/// `language` is what the fence is tagged with. A build file's answer comes
/// from a `.proto` schema rather than from Buri source, and it is rendered
/// here so that the two hovers are the same shape: a signature, then the prose
/// written above it.
pub(super) fn rendered(
    text: &str,
    language: &str,
    signature: &str,
    docs: &[String],
    span: crate::diagnostics::Span,
    kind: Markup,
) -> Value {
    let mut value = match kind {
        Markup::Markdown => format!("```{language}\n{signature}\n```"),
        Markup::PlainText => signature.to_string(),
    };
    if !docs.is_empty() {
        value.push_str("\n\n");
        value.push_str(&docs.join("\n"));
    }
    Value::object(vec![
        ("contents", markup(kind, &value)),
        ("range", convert::range(text, span)),
    ])
}

fn markup(kind: Markup, value: &str) -> Value {
    Value::object(vec![("kind", Value::str(kind.named())), ("value", Value::str(value))])
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
    Some(locations(analyzed, spans))
}

/// A `Location[]`, in the order every list of places this server answers with
/// is in.
///
/// One file is reachable through several targets and a name can be written
/// twice in one expression, so the same place arrives more than once. Sorted by
/// file and then by position: the protocol imposes no order, and an editor's
/// list should not depend on which body the scan met first. A span with no file
/// is dropped rather than guessed at.
pub(super) fn locations(analyzed: &Analyzed, spans: Vec<crate::diagnostics::Span>) -> Value {
    let mut out: Vec<(String, crate::diagnostics::Span)> = spans
        .into_iter()
        .filter_map(|span| Some((uri_of(analyzed, span)?, span)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.start.cmp(&b.1.start)).then(a.1.end.cmp(&b.1.end)));
    out.dedup_by(|a, b| a.0 == b.0 && a.1.start == b.1.start && a.1.end == b.1.end);
    Value::Array(
        out.iter()
            .map(|(uri, span)| {
                let text = &analyzed.session.map.get(span.file).text;
                Value::object(vec![
                    ("uri", Value::str(uri.as_str())),
                    ("range", convert::range(text, *span)),
                ])
            })
            .collect(),
    )
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

/// The same symbol, wherever else this one file names it.
///
/// The references scan narrowed to the buffer — which is why this analyses the
/// target owning the file rather than the repository: a highlight the editor
/// paints is a highlight in the file you are looking at, and every one of them
/// is inside a closure that target already compiles.
///
/// Every occurrence is reported as `Text`. The protocol distinguishes a read
/// from a write, and Buri has no assignment: a name is bound once and read
/// afterwards, so the distinction has nothing to mark.
pub fn document_highlight(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;
    let found = symbols::at(analyzed, path, text, offset)?;
    let mut spans = symbols::references(analyzed, &found.symbol);
    // The name rather than the whole declaration: a highlight is painted on
    // the identifier, and a field's declaration span is `export price: I64`.
    spans.push(symbols::declaration_name(analyzed, &found.symbol));
    spans.retain(|span| !span.is_none() && span.file == file);
    spans.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    spans.dedup_by(|a, b| a.start == b.start && a.end == b.end);
    Some(Value::Array(
        spans
            .iter()
            .map(|span| {
                Value::object(vec![
                    ("range", convert::range(text, *span)),
                    // 1 = Text.
                    ("kind", Value::number(1)),
                ])
            })
            .collect(),
    ))
}

/// Where the *type* of the thing under the cursor was declared.
///
/// A step sideways from definition rather than a different search: the cursor
/// names a local, a field or a call, and the answer is where that value's type
/// constructor was written. A primitive's is in the standard library and has no
/// file, so it answers with nothing.
pub fn type_definition(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let found = symbols::at(analyzed, path, text, offset)?;
    let con = symbols::type_of(analyzed, &found.symbol)?;
    location(analyzed, analyzed.analysis.checked.tables.tycon(con).span)
}

/// A name for the declaration under the cursor that an index somewhere else
/// could resolve.
///
/// The scheme is `buri` and the identifier is the two things that already name
/// a declaration in this language: what an import writes to reach the module,
/// and the dotted path to the declaration inside it —
/// `//lib/shop:catalog.Item.price`. The colon is where the package label ends,
/// which nothing else in the string can tell you: a package may hold a source
/// in a subdirectory, so `//lib/shop/catalog` alone does not say where the
/// package stops and the module starts.
///
/// A module that belongs to no package is the standard library, whose path is
/// what an import writes and so stands where the label would: `core/list:length`.
/// The package's own root module leaves the module half empty, and then the
/// separating dot goes with it: `//lib/shop:listing`.
///
/// `unique: "scheme"` is the honest level. The identifier is unique among
/// everything wearing this scheme and claims nothing beyond that — an external
/// repository is not a thing v0.3 has, so a wider claim would be about a world
/// that does not exist yet.
pub fn moniker(analyzed: &Analyzed, path: &Path, text: &str, position: Position) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let found = symbols::at(analyzed, path, text, offset)?;
    let (identifier, exported) = named(analyzed, &found.symbol)?;
    Some(Value::Array(vec![Value::object(vec![
        ("scheme", Value::str("buri")),
        ("identifier", Value::str(identifier)),
        ("unique", Value::str("scheme")),
        // The protocol's two answers for a name it can see: one the rest of
        // the world may resolve, and one that never leaves the project.
        ("kind", Value::str(if exported { "export" } else { "local" })),
    ])]))
}

/// A symbol's moniker identifier, and whether its declaration is exported.
///
/// Two symbols get neither. A local has no name outside the body that binds
/// it, so there is nothing for an index to resolve; and a module is named by
/// its path rather than by a name, which is why [`symbols::name`] has none for
/// it and why `rename` refuses one.
fn named(analyzed: &Analyzed, symbol: &symbols::Symbol) -> Option<(String, bool)> {
    use symbols::Symbol;
    let tables = &analyzed.analysis.checked.tables;
    let (module, declared, exported) = match symbol {
        Symbol::Function(id) => {
            let info = tables.fn_info(*id);
            // A method is reached through its type, and two types may both
            // declare a `cents` in one module.
            let declared = match info.self_ty {
                Some(con) => format!("{}.{}", tables.tycon(con).name, info.name),
                None => info.name.clone(),
            };
            (info.module, declared, info.exported)
        }
        Symbol::Type(id) => {
            let con = tables.tycon(*id);
            (con.module, con.name.clone(), con.exported)
        }
        Symbol::Trait(id) => {
            let info = tables.trait_(*id);
            (info.module, info.name.clone(), info.exported)
        }
        Symbol::TraitMethod { trait_id, method } => {
            let info = tables.trait_(*trait_id);
            let m = info.methods.get(*method)?;
            (info.module, format!("{}.{}", info.name, m.name), info.exported)
        }
        Symbol::Const(id) => {
            let info = tables.const_(*id);
            (info.module, info.name.clone(), info.exported)
        }
        Symbol::Context(id) => {
            let info = tables.ctx_decls.get(id.index())?;
            (info.module, info.name.clone(), info.exported)
        }
        Symbol::Field { con, variant, index } => {
            let info = tables.tycon(*con);
            let field = match variant {
                Some(v) => info.variants().get(*v)?.fields.get(*index)?,
                None => info.fields().get(*index)?,
            };
            let declared = match variant {
                Some(v) => {
                    format!("{}.{}.{}", info.name, info.variants().get(*v)?.name, field.name)
                }
                None => format!("{}.{}", info.name, field.name),
            };
            (info.module, declared, field.exported)
        }
        Symbol::Variant { con, index } => {
            let info = tables.tycon(*con);
            let variant = info.variants().get(*index)?;
            (info.module, format!("{}.{}", info.name, variant.name), variant.exported)
        }
        Symbol::Module(_) | Symbol::Local { .. } => return None,
    };
    Some((format!("{}{declared}", reached_by(analyzed, module)?), exported))
}

/// Everything in a moniker before the declaration's own name: what an import
/// writes to reach the module, and the module's path inside its package.
fn reached_by(
    analyzed: &Analyzed,
    module: crate::compiler::semantics::types::ModuleId,
) -> Option<String> {
    let data = analyzed.analysis.loaded.modules.get(module.index())?;
    let package = data.pkg.map(|p| analyzed.session.workspace.package(p).label());
    let inside = match &package {
        Some(label) => data.path.strip_prefix(label.as_str()).unwrap_or("").trim_start_matches('/'),
        // No package is the standard library, whose path is what an import
        // writes and so is the whole of the left-hand side.
        None => "",
    };
    let label = package.unwrap_or_else(|| data.path.clone());
    Some(if inside.is_empty() { format!("{label}:") } else { format!("{label}:{inside}.") })
}

/// Every declaration in the repository whose name contains `query`.
///
/// Read from the syntax of every module that has a file, for the reason the
/// outline is: a query answered from the tables would drop a `test`, and a
/// module that failed to check would vanish from a search that is most useful
/// when something is broken.
pub fn workspace_symbols(analyzed: &Analyzed, query: &str) -> Value {
    let wanted = query.to_lowercase();
    let mut out = Vec::new();
    for module in &analyzed.analysis.loaded.modules {
        let file = analyzed.session.map.get(module.file);
        // The standard library is compiled into the binary. It has declarations
        // and no file, and a search result you cannot open is not one.
        if file.abs_path.as_os_str().is_empty() {
            continue;
        }
        let uri = convert::uri_of(&file.abs_path);
        let tree = &module.ast.tree;
        for item in &module.ast.items {
            let (name, kind) = match item {
                Item::Fn(d) => (tree.name(d.name).to_string(), 12),
                Item::Struct(d) => (tree.name(d.name).to_string(), 23),
                Item::Enum(d) => (tree.name(d.name).to_string(), 10),
                Item::Trait(d) => {
                    // A trait method is a declaration of its own — the `impl`
                    // that supplies it is a second one — so both are findable.
                    let container = tree.name(d.name).to_string();
                    for m in &d.methods {
                        let name = tree.name(m.name).to_string();
                        if matches(&name, &wanted) {
                            out.push(found(&name, 6, &uri, &file.text, m.name.span, &container));
                        }
                    }
                    (container, 5)
                }
                Item::Let(d) => (tree.name(d.name).to_string(), 14),
                Item::TypeAlias(d) => (tree.name(d.name).to_string(), 26),
                Item::Context(d) => (tree.name(d.name).to_string(), 23),
                Item::Test(d) => (d.name.clone(), 12),
                Item::Impl(d) => {
                    // An `impl` is not itself a searchable name, but the
                    // methods it supplies are — and they are declared nowhere
                    // else.
                    let container = crate::formatting::type_text(tree, d.self_ty);
                    for m in &d.methods {
                        let name = tree.name(m.name).to_string();
                        if matches(&name, &wanted) {
                            out.push(found(&name, 6, &uri, &file.text, m.name.span, &container));
                        }
                    }
                    continue;
                }
                Item::Import(_) | Item::ReExport(_) | Item::Derive(_) => continue,
            };
            if matches(&name, &wanted) {
                let span = declared_name_span(item);
                out.push(found(&name, kind, &uri, &file.text, span, &module.path));
            }
        }
    }
    Value::Array(out)
}

fn matches(name: &str, wanted: &str) -> bool {
    wanted.is_empty() || name.to_lowercase().contains(wanted)
}

/// The span of the name a top-level item declares. `Item::span` is the whole
/// declaration; a search result should land the cursor on the name.
fn declared_name_span(item: &Item) -> crate::diagnostics::Span {
    match item {
        Item::Fn(d) => d.name.span,
        Item::Struct(d) => d.name.span,
        Item::Enum(d) => d.name.span,
        Item::Trait(d) => d.name.span,
        Item::Let(d) => d.name.span,
        Item::TypeAlias(d) => d.name.span,
        Item::Context(d) => d.name.span,
        Item::Test(d) => d.name_span,
        _ => item.span(),
    }
}

fn found(
    name: &str,
    kind: i64,
    uri: &str,
    text: &str,
    span: crate::diagnostics::Span,
    container: &str,
) -> Value {
    Value::object(vec![
        ("name", Value::str(name)),
        ("kind", Value::number(kind)),
        ("containerName", Value::str(container)),
        (
            "location",
            Value::object(vec![
                ("uri", Value::str(uri)),
                ("range", convert::range(text, span)),
            ]),
        ),
    ])
}
