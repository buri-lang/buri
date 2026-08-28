//! What the cursor is pointing at.
//!
//! Hover and go-to-definition ask one question and differ only in what they do
//! with the answer: one renders the declaration, the other returns its span. So
//! the question is asked once, here, and both read the same result. References
//! asks it once too and then asks the opposite one — where else is this name
//! written — off the same enumeration of what each piece of syntax names.
//! Everything read here was already computed: the flat tree holds every type
//! and pattern the source wrote, `Tables` holds every declaration's span, and a
//! checked body holds every resolved use, so there is still no index of our own.

use crate::compiler::modules::ModuleData;
use crate::compiler::semantics::resolve::Sym;
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{
    self, AstRef, ConstId, ContextDeclId, FnId, ModuleId, TraitId, Ty, TyConId, Tables,
};
use crate::formatting;
use crate::diagnostics::{FileId, Span};
use crate::parsing::flat::{self, Location, TypeId, TypeView};
use crate::parsing::tree::{self, ImportClause, Item};
use std::path::Path;
use super::state::Analyzed;

/// A declaration the source is able to name.
#[derive(Clone)]
pub enum Symbol {
    Function(FnId),
    Type(TyConId),
    Trait(TraitId),
    TraitMethod { trait_id: TraitId, method: usize },
    Const(ConstId),
    Context(ContextDeclId),
    /// A field of a struct, or of an enum variant's record payload.
    Field { con: TyConId, variant: Option<usize>, index: usize },
    Variant { con: TyConId, index: usize },
    /// A whole module, named by an import path or by a namespace alias.
    Module(ModuleId),
    /// A local or a parameter. Carried by value because it lives in a function
    /// body rather than in the tables, and there is no id that outlives the
    /// walk that found it.
    Local { name: String, ty: Ty, span: Span },
}

/// A symbol, and the span of the text that named it.
pub struct Found {
    pub symbol: Symbol,
    /// What to highlight: the name under the cursor, not the declaration.
    pub span: Span,
}

/// The symbol the offset names, wherever it was written.
///
/// The three sources are tried in the order that makes the narrower answer
/// win. A written type or an import clause is syntax the typed tree never
/// keeps, so it is asked first; a declaration's own name comes next; and a use
/// inside a body last, because a body's outermost expression covers the type
/// annotations written inside it.
pub fn at(analyzed: &Analyzed, path: &Path, text: &str, offset: u32) -> Option<Found> {
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;
    let module = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file)?;
    written_reference(analyzed, module, offset)
        .or_else(|| declared_here(analyzed, file, offset))
        .or_else(|| in_a_body(analyzed, file, text, offset))
}

/// Where the symbol was declared.
pub fn declaration(analyzed: &Analyzed, symbol: &Symbol) -> Span {
    let tables = &analyzed.analysis.checked.tables;
    match symbol {
        Symbol::Function(id) => tables.fn_info(*id).span,
        Symbol::Type(id) => tables.tycon(*id).span,
        Symbol::Trait(id) => tables.trait_(*id).span,
        Symbol::TraitMethod { trait_id, method } => {
            tables.trait_(*trait_id).methods.get(*method).map_or(Span::NONE, |m| m.span)
        }
        Symbol::Const(id) => tables.const_(*id).span,
        Symbol::Context(id) => tables.ctx_decls.get(id.index()).map_or(Span::NONE, |c| c.span),
        Symbol::Field { con, variant, index } => {
            field_info(tables, *con, *variant, *index).map_or(Span::NONE, |f| f.span)
        }
        Symbol::Variant { con, index } => {
            tables.tycon(*con).variants().get(*index).map_or(Span::NONE, |v| v.span)
        }
        // The top of the file: a module has no name of its own to point at.
        Symbol::Module(id) => match analyzed.analysis.loaded.modules.get(id.index()) {
            Some(m) => Span::point(m.file, 0),
            None => Span::NONE,
        },
        Symbol::Local { span, .. } => *span,
    }
}

/// The symbol as an editor should read it: the signature the formatter would
/// print, and the `///` lines written above it.
pub fn describe(analyzed: &Analyzed, symbol: &Symbol) -> (String, Vec<String>) {
    let tables = &analyzed.analysis.checked.tables;
    match symbol {
        Symbol::Function(id) => match function_syntax(analyzed, *id) {
            Some((t, d)) => (formatting::signature(t, d), d.docs.clone()),
            // The primitives' methods are supplied by the runtime and have no
            // syntax anywhere, so the table entry is the whole of what is known.
            None => (function_from_table(tables, *id), Vec::new()),
        },
        Symbol::Type(id) => {
            let con = tables.tycon(*id);
            match declaration_syntax(analyzed, con.module, con.span) {
                Some((t, Item::Struct(d))) => (
                    format!("struct {}{}", t.name(d.name), formatting::generics(t, &d.generics)),
                    d.docs.clone(),
                ),
                Some((t, Item::Enum(d))) => (
                    format!("enum {}{}", t.name(d.name), formatting::generics(t, &d.generics)),
                    d.docs.clone(),
                ),
                Some((t, Item::TypeAlias(d))) => (
                    format!("type {} = {}", t.name(d.name), formatting::type_text(t, d.ty)),
                    d.docs.clone(),
                ),
                _ => (con.name.clone(), Vec::new()),
            }
        }
        Symbol::Trait(id) => {
            let info = tables.trait_(*id);
            match declaration_syntax(analyzed, info.module, info.span) {
                Some((t, Item::Trait(d))) => (
                    format!(
                        "{} {}{}",
                        if d.is_effect { "effect" } else { "trait" },
                        t.name(d.name),
                        formatting::generics(t, &d.generics)
                    ),
                    d.docs.clone(),
                ),
                _ => (
                    format!("{} {}", if info.is_effect { "effect" } else { "trait" }, info.name),
                    Vec::new(),
                ),
            }
        }
        Symbol::TraitMethod { trait_id, method } => {
            let info = tables.trait_(*trait_id);
            let declared = match declaration_syntax(analyzed, info.module, info.span) {
                Some((t, Item::Trait(d))) => d.methods.get(*method).map(|m| (t, m)),
                _ => None,
            };
            match declared {
                Some((t, m)) => (formatting::signature(t, m), m.docs.clone()),
                None => match info.methods.get(*method) {
                    Some(m) => (
                        format!(
                            "fn {}({}): {}",
                            m.name,
                            parameters(tables, &m.params),
                            types::show(tables, None, &[], &m.ret)
                        ),
                        Vec::new(),
                    ),
                    None => (info.name.clone(), Vec::new()),
                },
            }
        }
        Symbol::Const(id) => {
            let info = tables.const_(*id);
            match item_syntax(analyzed, info.ast) {
                Some((t, Item::Let(d))) => (
                    format!("let {}: {}", t.name(d.name), formatting::type_text(t, d.ty)),
                    d.docs.clone(),
                ),
                _ => (
                    format!(
                        "let {}: {}",
                        info.name,
                        types::show(tables, None, &[], &info.ty)
                    ),
                    Vec::new(),
                ),
            }
        }
        Symbol::Context(id) => {
            let Some(info) = tables.ctx_decls.get(id.index()) else {
                return (String::new(), Vec::new());
            };
            match item_syntax(analyzed, info.ast) {
                Some((t, Item::Context(d))) => {
                    (format!("context {}", t.name(d.name)), d.docs.clone())
                }
                _ => (format!("context {}", info.name), Vec::new()),
            }
        }
        Symbol::Field { con, variant, index } => {
            let fallback = match field_info(tables, *con, *variant, *index) {
                Some(f) => format!(
                    "{}: {}",
                    f.name,
                    types::show(tables, None, &[], &f.ty)
                ),
                None => String::new(),
            };
            match field_syntax(analyzed, *con, *variant, *index) {
                Some((t, d)) => (formatting::field_decl(t, d), d.docs.clone()),
                None => (fallback, Vec::new()),
            }
        }
        Symbol::Variant { con, index } => {
            let info = tables.tycon(*con);
            let declared = match declaration_syntax(analyzed, info.module, info.span) {
                Some((t, Item::Enum(d))) => d.variants.get(*index).map(|v| (t, v)),
                _ => None,
            };
            match declared {
                Some((t, v)) => (formatting::variant(t, v), v.docs.clone()),
                None => (
                    info.variants().get(*index).map_or(String::new(), |v| v.name.clone()),
                    Vec::new(),
                ),
            }
        }
        Symbol::Module(id) => match analyzed.analysis.loaded.modules.get(id.index()) {
            // The `//!` lines, under the spelling an import would use.
            Some(m) => (format!("from \"{}\"", m.path), m.ast.docs.clone()),
            None => (String::new(), Vec::new()),
        },
        Symbol::Local { name, ty, .. } => (
            format!("{name}: {}", types::show(tables, None, &[], ty)),
            Vec::new(),
        ),
    }
}

/// Every place the compilation writes `symbol` down.
///
/// The two use sources [`at`] asks — the syntax a module writes, and the names
/// a checked body resolves — read the other way round: instead of keeping the
/// one name that covers an offset, keep every name that resolves to the symbol.
/// Nothing is indexed and nothing is cached; the caller hands in an analysis of
/// the whole repository and this is one pass over it.
///
/// [`at`]'s third source is the declaration, and it is deliberately not here:
/// [`declaration`] already knows where a symbol was declared, and the protocol
/// asks for it separately.
pub fn references(analyzed: &Analyzed, symbol: &Symbol) -> Vec<Span> {
    let tables = &analyzed.analysis.checked.tables;
    let mut out: Vec<Span> = Vec::new();
    let mut take = |span: Span, found: Symbol| {
        if !span.is_none() && same(&found, symbol) {
            out.push(span);
        }
    };
    for module in &analyzed.analysis.loaded.modules {
        written_names(analyzed, module, &mut take);
    }
    let checked = &analyzed.analysis.checked;
    for body in checked.bodies.values() {
        body_names(analyzed, tables, &body.locals, &body.expr, &mut take);
    }
    // A module-level `let`'s value is checked on its own, with no body and so
    // no locals around it.
    for expr in checked.consts.values() {
        body_names(analyzed, tables, &[], expr, &mut take);
    }
    out
}

/// Whether two symbols are the same declaration. A local is compared by its
/// binding span, which is what confines a local's references to the body that
/// declares it: no other body has a local bound at that position.
fn same(a: &Symbol, b: &Symbol) -> bool {
    match (a, b) {
        (Symbol::Function(x), Symbol::Function(y)) => x == y,
        (Symbol::Type(x), Symbol::Type(y)) => x == y,
        (Symbol::Trait(x), Symbol::Trait(y)) => x == y,
        (
            Symbol::TraitMethod { trait_id, method },
            Symbol::TraitMethod { trait_id: t, method: m },
        ) => trait_id == t && method == m,
        (Symbol::Const(x), Symbol::Const(y)) => x == y,
        (Symbol::Context(x), Symbol::Context(y)) => x == y,
        (
            Symbol::Field { con, variant, index },
            Symbol::Field { con: c, variant: v, index: i },
        ) => con == c && variant == v && index == i,
        (Symbol::Variant { con, index }, Symbol::Variant { con: c, index: i }) => {
            con == c && index == i
        }
        (Symbol::Module(x), Symbol::Module(y)) => x == y,
        (Symbol::Local { span, .. }, Symbol::Local { span: s, .. }) => span == s,
        _ => false,
    }
}

/// Every name a checked body writes, expressions and patterns alike.
fn body_names(
    analyzed: &Analyzed,
    tables: &Tables,
    locals: &[typed::Local],
    root: &typed::Expr,
    out: &mut impl FnMut(Span, Symbol),
) {
    typed::walk(root, &mut |e| {
        let from = after_receiver(e);
        expression_symbols(tables, locals, e, &mut |name, symbol| {
            if let Some(span) = name_span(analyzed, e.span, from, &name) {
                out(span, symbol);
            }
        });
        for pattern in patterns_of(e) {
            pattern_names(analyzed, tables, pattern, out);
        }
    });
}

fn pattern_names(
    analyzed: &Analyzed,
    tables: &Tables,
    pattern: &typed::Pattern,
    out: &mut impl FnMut(Span, Symbol),
) {
    pattern_symbols(tables, pattern, &mut |name, symbol| {
        if let Some(span) = name_span(analyzed, pattern.span, pattern.span.start, &name) {
            out(span, symbol);
        }
    });
    for sub in sub_patterns(pattern) {
        pattern_names(analyzed, tables, sub, out);
    }
}

/// Where to start looking for the name inside an expression: past the receiver,
/// when there is one.
///
/// Postfix syntax puts the receiver first — `total.of(x)` and `total.count` both
/// begin where the whole expression begins — so a sub-expression starting at the
/// same offset is text the name cannot be in.
fn after_receiver(expr: &typed::Expr) -> u32 {
    let mut from = expr.span.start;
    typed::children(expr, &mut |child| {
        if child.span.file == expr.span.file
            && child.span.start == expr.span.start
            && child.span.end > from
        {
            from = child.span.end;
        }
    });
    from
}

/// Where a name is written inside an expression's span.
///
/// The typed tree keeps a span for the whole expression and none for the name
/// in it, so the name is found in the text. An expression that never wrote it —
/// an operator standing for a trait method, a leading-dot variant standing for
/// its enum — is not a reference to it, and yields nothing.
fn name_span(analyzed: &Analyzed, span: Span, from: u32, name: &str) -> Option<Span> {
    if span.is_none() || name.is_empty() {
        return None;
    }
    let text = &analyzed.session.map.get(span.file).text;
    let start = (from.max(span.start) as usize).min(text.len());
    let end = (span.end as usize).min(text.len());
    let slice = text.get(start..end)?;
    let bytes = slice.as_bytes();
    let part = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut at = 0;
    let mut best: Option<usize> = None;
    while let Some(hit) = slice.get(at..).and_then(|rest| rest.find(name)) {
        let index = at.saturating_add(hit);
        let after = index.saturating_add(name.len());
        let whole = index
            .checked_sub(1)
            .is_none_or(|b| bytes.get(b).is_none_or(|c| !part(*c)))
            && bytes.get(after).is_none_or(|c| !part(*c));
        if whole {
            best = best.or(Some(index));
            // `f: v` in a struct literal or pattern: the label is the field,
            // and a value that happens to be spelled the same is not.
            let label = bytes
                .get(after..)
                .and_then(|rest| rest.iter().find(|b| **b != b' '))
                .is_some_and(|b| *b == b':');
            if label {
                best = Some(index);
                break;
            }
        }
        at = index.saturating_add(1);
    }
    let found = start.saturating_add(best?);
    Some(Span {
        file: span.file,
        start: found as u32,
        end: found.saturating_add(name.len()) as u32,
    })
}

// ---------------------------------------------------------------------------
// Names the typed tree does not keep
// ---------------------------------------------------------------------------

/// A written type, or a name inside an import clause.
fn written_reference(
    analyzed: &Analyzed,
    module: &ModuleData,
    offset: u32,
) -> Option<Found> {
    let mut found: Option<Found> = None;
    written_names(analyzed, module, &mut |span, symbol| {
        if found.is_none() && covers(span, offset) {
            found = Some(Found { symbol, span });
        }
    });
    found
}

/// Every name one module's syntax writes down, and what each one names.
///
/// Imports come first and written types second, which is the order the cursor
/// wants: an import clause is inside no type and a type is inside no import, so
/// the two never disagree about one offset — and a scan for every use of a
/// symbol wants them both regardless.
fn written_names(
    analyzed: &Analyzed,
    module: &ModuleData,
    out: &mut impl FnMut(Span, Symbol),
) {
    import_names(analyzed, module, out);
    written_type_names(analyzed, module, out);
}

/// The path of an import, and every name in its clause.
fn import_names(analyzed: &Analyzed, module: &ModuleData, out: &mut impl FnMut(Span, Symbol)) {
    let tree = &module.ast.tree;
    for item in &module.ast.items {
        let (path, path_span, specs, namespace) = match item {
            Item::Import(i) => match &i.clause {
                ImportClause::Named(specs) => (&i.path, i.path_span, specs.as_slice(), None),
                ImportClause::Namespace(n) => (&i.path, i.path_span, &[][..], Some(*n)),
            },
            Item::ReExport(r) => (&r.path, r.path_span, r.specs.as_slice(), None),
            _ => continue,
        };
        let Some(id) = analyzed.analysis.loaded.find(path) else { continue };
        out(path_span, Symbol::Module(id));
        if let Some(n) = namespace {
            out(n.span, Symbol::Module(id));
        }
        let Some(scope) = analyzed.analysis.checked.scopes.get(id.index()) else { continue };
        for spec in specs {
            let Some(symbol) = scope.exports.get(tree.name(spec.name)).and_then(symbol_of) else {
                continue;
            };
            // The alias and the name it renames stand for the same
            // declaration, so both are the same reference to it.
            out(spec.name.span, symbol.clone());
            if let Some(alias) = spec.alias {
                out(alias.span, symbol);
            }
        }
    }
}

/// Every type as it was written: an annotation, a bound, an `impl` head, a
/// `derive` list. None of these survive into a checked body.
fn written_type_names(
    analyzed: &Analyzed,
    module: &ModuleData,
    out: &mut impl FnMut(Span, Symbol),
) {
    let tree = &module.ast.tree;
    for index in 0..tree.type_nodes().len() {
        let TypeView::Named { path, .. } = tree.ty(TypeId(index as u32)) else { continue };
        for segment in 0..path.len() {
            let Some(symbol) = resolve_path(analyzed, module.id, tree, path, segment) else {
                continue;
            };
            let Some(at) = path.get(segment) else { continue };
            out(tree.span_of(*at), symbol);
        }
    }
}

/// A written path, resolved as far as the segment under the cursor.
///
/// Reconstructed against `ModuleScope` rather than replayed through the
/// checker: `resolve_path` there needs a `&mut Checker`, and the scopes it
/// filled in are still sitting in `Checked`.
fn resolve_path(
    analyzed: &Analyzed,
    module: ModuleId,
    tree: &flat::Tree,
    path: &[Location],
    segment: usize,
) -> Option<Symbol> {
    let scope = analyzed.analysis.checked.scopes.get(module.index())?;
    let at = path.get(segment)?;
    let name = tree.text(*at);
    let Some(previous) = segment.checked_sub(1) else {
        // A leading segment is a namespace only when something follows it.
        if path.len() > 1 {
            if let Some(id) = scope.namespaces.get(name) {
                return Some(Symbol::Module(*id));
            }
        }
        return symbol_of(scope.names.get(name)?);
    };
    let head = tree.text(*path.get(previous)?);
    if let Some(id) = scope.namespaces.get(head) {
        let inner = analyzed.analysis.checked.scopes.get(id.index())?;
        return symbol_of(inner.exports.get(name)?);
    }
    // `Color.Red` — the head names the enum and the tail one of its variants.
    if let Some(Sym::Ty(con)) = scope.names.get(head) {
        let index = analyzed.analysis.checked.tables.tycon(*con).variant_index(name)?;
        return Some(Symbol::Variant { con: *con, index });
    }
    None
}

fn symbol_of(sym: &Sym) -> Option<Symbol> {
    match sym {
        Sym::Ty(id) => Some(Symbol::Type(*id)),
        Sym::Fn(id) => Some(Symbol::Function(*id)),
        Sym::Trait(id) => Some(Symbol::Trait(*id)),
        Sym::Const(id) => Some(Symbol::Const(*id)),
        Sym::Context(id) => Some(Symbol::Context(*id)),
        Sym::Namespace(id) => Some(Symbol::Module(*id)),
        // Which of the overloads is meant is a question about a call, and this
        // is a name with no call around it. The first is the one an editor can
        // show without inventing an answer.
        Sym::Overloaded(ids) => ids.first().copied().map(Symbol::Function),
        // A method is reached through a receiver, never through this name.
        Sym::Method(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Declarations, and uses inside a body
// ---------------------------------------------------------------------------

/// A declaration's own name. Every table entry's span is the name it declares,
/// which is what makes this one scan rather than a walk over the syntax — and
/// what makes it cover an `impl` method, a field and a variant for free.
fn declared_here(analyzed: &Analyzed, file: FileId, offset: u32) -> Option<Found> {
    let tables = &analyzed.analysis.checked.tables;
    let mut best: Option<(u32, Span, Symbol)> = None;
    let mut offer = |span: Span, symbol: Symbol| {
        if span.file != file || span.start > offset || offset > span.end {
            return;
        }
        let width = span.end.saturating_sub(span.start);
        if best.as_ref().is_none_or(|(w, _, _)| width < *w) {
            best = Some((width, span, symbol));
        }
    };
    for (index, info) in tables.fns.iter().enumerate() {
        offer(info.span, Symbol::Function(FnId(index as u32)));
    }
    for (index, con) in tables.tycons.iter().enumerate() {
        let id = TyConId(index as u32);
        offer(con.span, Symbol::Type(id));
        for (i, f) in con.fields().iter().enumerate() {
            offer(f.span, Symbol::Field { con: id, variant: None, index: i });
        }
        for (v, variant) in con.variants().iter().enumerate() {
            offer(variant.span, Symbol::Variant { con: id, index: v });
            for (i, f) in variant.fields.iter().enumerate() {
                offer(f.span, Symbol::Field { con: id, variant: Some(v), index: i });
            }
        }
    }
    for (index, info) in tables.traits.iter().enumerate() {
        let id = TraitId(index as u32);
        offer(info.span, Symbol::Trait(id));
        for (i, m) in info.methods.iter().enumerate() {
            offer(m.span, Symbol::TraitMethod { trait_id: id, method: i });
        }
    }
    for (index, info) in tables.consts.iter().enumerate() {
        offer(info.span, Symbol::Const(ConstId(index as u32)));
    }
    for (index, info) in tables.ctx_decls.iter().enumerate() {
        offer(info.span, Symbol::Context(ContextDeclId(index as u32)));
    }
    let (_, span, symbol) = best?;
    Some(Found { symbol, span })
}

/// A use inside a checked body.
///
/// The innermost expression covering the offset is the answer or there is
/// none: an outer expression that happens to contain the cursor names
/// something the cursor is not on.
fn in_a_body(analyzed: &Analyzed, file: FileId, text: &str, offset: u32) -> Option<Found> {
    let checked = &analyzed.analysis.checked;
    for (fid, body) in &checked.bodies {
        if checked.tables.fn_info(*fid).span.file != file {
            continue;
        }
        if let Some(found) = in_one_body(analyzed, body, file, text, offset) {
            return Some(found);
        }
    }
    // A module-level `let`'s value is checked on its own, with no body and so
    // with no locals, around it.
    for (id, expr) in &checked.consts {
        if checked.tables.const_(*id).span.file != file {
            continue;
        }
        if let Some(found) = narrowest(analyzed, &[], expr, file, text, offset) {
            return Some(found);
        }
    }
    None
}

fn in_one_body(
    analyzed: &Analyzed,
    body: &typed::Body,
    file: FileId,
    text: &str,
    offset: u32,
) -> Option<Found> {
    // A binding site and a pattern come first: both sit inside the block or
    // the `match` that holds them, so no expression is ever narrower.
    let mut bound: Option<(u32, &typed::Local)> = None;
    for local in &body.locals {
        if !within(local.span, file, offset) {
            continue;
        }
        let width = local.span.end.saturating_sub(local.span.start);
        if bound.as_ref().is_none_or(|(w, _)| width < *w) {
            bound = Some((width, local));
        }
    }
    let mut matched: Option<(u32, &typed::Pattern)> = None;
    typed::walk(&body.expr, &mut |e| {
        for pattern in patterns_of(e) {
            narrowest_pattern(pattern, file, offset, &mut matched);
        }
    });
    // A pattern that binds a name is the name's declaration; the bind is the
    // narrower and more specific of the two, so it wins a tie.
    let pattern = matched.filter(|(w, _)| bound.as_ref().is_none_or(|(b, _)| w < b));
    if let Some((_, pattern)) = pattern {
        let word = identifier_at(text, offset);
        let named = word.as_ref().map(|(n, _)| n.as_str());
        if let Some(symbol) = symbol_of_pattern(analyzed, pattern, named) {
            return Some(Found { symbol, span: pattern.span });
        }
    }
    if let Some((_, local)) = bound {
        return Some(Found {
            symbol: Symbol::Local {
                name: local.name.clone(),
                ty: local.ty.clone(),
                span: local.span,
            },
            span: local.span,
        });
    }
    narrowest(analyzed, &body.locals, &body.expr, file, text, offset)
}

/// The patterns one expression holds directly. Every pattern in a body hangs
/// off a `match` arm or a `let`, so walking the expressions reaches them all.
fn patterns_of(e: &typed::Expr) -> Vec<&typed::Pattern> {
    match &e.kind {
        ExprKind::Match { arms, .. } => arms.iter().map(|a| &a.pattern).collect(),
        ExprKind::Block { stmts, .. } => stmts
            .iter()
            .filter_map(|s| match s {
                typed::Stmt::Let { pattern, .. } => Some(pattern),
                typed::Stmt::Expr(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn narrowest_pattern<'a>(
    pattern: &'a typed::Pattern,
    file: FileId,
    offset: u32,
    best: &mut Option<(u32, &'a typed::Pattern)>,
) {
    if within(pattern.span, file, offset) {
        let width = pattern.span.end.saturating_sub(pattern.span.start);
        if best.as_ref().is_none_or(|(w, _)| width < *w) {
            *best = Some((width, pattern));
        }
    }
    for sub in sub_patterns(pattern) {
        narrowest_pattern(sub, file, offset, best);
    }
}

fn sub_patterns(pattern: &typed::Pattern) -> Vec<&typed::Pattern> {
    match &pattern.kind {
        typed::PatKind::Bind { sub, .. } => sub.iter().map(std::convert::AsRef::as_ref).collect(),
        typed::PatKind::Tuple(elems)
        | typed::PatKind::Array { elems, .. }
        | typed::PatKind::Or(elems) => elems.iter().collect(),
        typed::PatKind::Struct { fields, .. } | typed::PatKind::Variant { fields, .. } => {
            fields.iter().map(|f| &f.pattern).collect()
        }
        _ => Vec::new(),
    }
}

/// What a pattern names. A `match` arm writes the variant and the fields it
/// destructures, and neither is anywhere else in the typed tree.
fn symbol_of_pattern(
    analyzed: &Analyzed,
    pattern: &typed::Pattern,
    word: Option<&str>,
) -> Option<Symbol> {
    let mut all = Vec::new();
    pattern_symbols(&analyzed.analysis.checked.tables, pattern, &mut |name, symbol| {
        all.push((name, symbol));
    });
    pick(word, all)
}

/// Every name a pattern writes, most specific first — so that a cursor sitting
/// on none of them still gets the pattern's own answer.
fn pattern_symbols(
    tables: &Tables,
    pattern: &typed::Pattern,
    out: &mut impl FnMut(String, Symbol),
) {
    match &pattern.kind {
        typed::PatKind::Struct { con, fields } => {
            out(tables.tycon(*con).name.clone(), Symbol::Type(*con));
            for f in fields {
                if let Some(info) = field_info(tables, *con, None, f.index) {
                    out(
                        info.name.clone(),
                        Symbol::Field { con: *con, variant: None, index: f.index },
                    );
                }
            }
        }
        typed::PatKind::Variant { con, variant, fields } => {
            let info = tables.tycon(*con);
            if let Some(v) = info.variants().get(*variant) {
                out(v.name.clone(), Symbol::Variant { con: *con, index: *variant });
            }
            out(info.name.clone(), Symbol::Type(*con));
            for f in fields {
                if let Some(field) = field_info(tables, *con, Some(*variant), f.index) {
                    out(
                        field.name.clone(),
                        Symbol::Field { con: *con, variant: Some(*variant), index: f.index },
                    );
                }
            }
        }
        // A bind is a declaration rather than a use, and `Body::locals` already
        // holds its span.
        _ => {}
    }
}

/// The written name the cursor is on, or — when it is on none of them — the
/// first, which is the enumeration's own answer for the construct.
fn pick(word: Option<&str>, all: Vec<(String, Symbol)>) -> Option<Symbol> {
    if let Some(w) = word {
        if let Some((_, symbol)) = all.iter().find(|(n, _)| n == w) {
            return Some(symbol.clone());
        }
    }
    all.into_iter().next().map(|(_, symbol)| symbol)
}

fn within(span: Span, file: FileId, offset: u32) -> bool {
    span.file == file && span.start <= offset && offset <= span.end
}

fn narrowest<'a>(
    analyzed: &Analyzed,
    locals: &[typed::Local],
    root: &'a typed::Expr,
    file: FileId,
    text: &str,
    offset: u32,
) -> Option<Found> {
    let mut best: Option<(u32, &'a typed::Expr)> = None;
    typed::walk(root, &mut |e| {
        if e.span.file != file || e.span.start > offset || offset > e.span.end {
            return;
        }
        let width = e.span.end.saturating_sub(e.span.start);
        if best.as_ref().is_none_or(|(w, _)| width < *w) {
            best = Some((width, e));
        }
    });
    let (_, expr) = best?;
    let word = identifier_at(text, offset);
    let named = word.as_ref().map(|(n, _)| n.as_str());
    let symbol = symbol_of_expr(analyzed, locals, expr, named)?;
    let span = match word {
        Some((_, span)) => Span { file, start: span.0, end: span.1 },
        None => expr.span,
    };
    Some(Found { symbol, span })
}

/// What a resolved expression names, if it names anything.
///
/// `word` is the identifier under the cursor. A struct literal and an enum
/// literal need it, because the typed tree keeps a field by index and a
/// variant by number: the name the source wrote is not in it.
fn symbol_of_expr(
    analyzed: &Analyzed,
    locals: &[typed::Local],
    expr: &typed::Expr,
    word: Option<&str>,
) -> Option<Symbol> {
    let mut all = Vec::new();
    expression_symbols(&analyzed.analysis.checked.tables, locals, expr, &mut |name, symbol| {
        all.push((name, symbol));
    });
    pick(word, all)
}

/// Every name one expression writes, most specific first.
///
/// One expression can write several: `Point { x: 1 }` names the type and the
/// field, and `Color.Red` names the enum and the variant. Which of them a
/// cursor means is decided by the word it is on; a scan for uses wants all of
/// them, so this is the one place that says what an expression names and both
/// readers take their answer from it.
fn expression_symbols(
    tables: &Tables,
    locals: &[typed::Local],
    expr: &typed::Expr,
    out: &mut impl FnMut(String, Symbol),
) {
    match &expr.kind {
        ExprKind::Local(id) => {
            if let Some(l) = locals.get(id.index()) {
                out(
                    l.name.clone(),
                    Symbol::Local { name: l.name.clone(), ty: l.ty.clone(), span: l.span },
                );
            }
        }
        ExprKind::Const(id) => out(tables.const_(*id).name.clone(), Symbol::Const(*id)),
        ExprKind::FnRef(func) | ExprKind::CallFn { func, .. } => {
            if let Some(id) = func.decl() {
                out(tables.fn_info(id).name.clone(), Symbol::Function(id));
            }
        }
        ExprKind::CallTrait { trait_id, method, .. } => {
            if let Some(m) = tables.trait_(*trait_id).methods.get(*method) {
                out(
                    m.name.clone(),
                    Symbol::TraitMethod { trait_id: *trait_id, method: *method },
                );
            }
        }
        ExprKind::StructLit { con, fields, .. } => {
            out(tables.tycon(*con).name.clone(), Symbol::Type(*con));
            for index in 0..fields.len() {
                if let Some(info) = field_info(tables, *con, None, index) {
                    out(info.name.clone(), Symbol::Field { con: *con, variant: None, index });
                }
            }
        }
        ExprKind::StructUpdate { con, updates, .. } => {
            out(tables.tycon(*con).name.clone(), Symbol::Type(*con));
            for (index, _) in updates {
                if let Some(info) = field_info(tables, *con, None, *index) {
                    out(
                        info.name.clone(),
                        Symbol::Field { con: *con, variant: None, index: *index },
                    );
                }
            }
        }
        ExprKind::EnumLit { con, variant, .. } => {
            // `Color.Red` — either half may be written, and the leading-dot
            // shorthand writes only the second.
            let info = tables.tycon(*con);
            if let Some(v) = info.variants().get(*variant) {
                out(v.name.clone(), Symbol::Variant { con: *con, index: *variant });
            }
            out(info.name.clone(), Symbol::Type(*con));
        }
        ExprKind::Field { base, index } => {
            if let Some(con) = base.ty.head() {
                if let Some(info) = field_info(tables, con, None, *index) {
                    out(info.name.clone(), Symbol::Field { con, variant: None, index: *index });
                }
            }
        }
        ExprKind::CtxCall { decl } => {
            if let Some(info) = tables.ctx_decls.get(decl.index()) {
                out(info.name.clone(), Symbol::Context(*decl));
            }
        }
        ExprKind::CtxGet { trait_id, .. } => {
            out(tables.trait_(*trait_id).name.clone(), Symbol::Trait(*trait_id));
        }
        ExprKind::CtxLit { bindings } => {
            for (id, _) in bindings {
                out(tables.trait_(*id).name.clone(), Symbol::Trait(*id));
            }
        }
        _ => {}
    }
}

/// The identifier the offset sits in, and its byte range.
fn identifier_at(text: &str, offset: u32) -> Option<(String, (u32, u32))> {
    let bytes = text.as_bytes();
    let word = |b: &u8| b.is_ascii_alphanumeric() || *b == b'_';
    let mut start = offset as usize;
    let mut end = offset as usize;
    while let Some(previous) = start.checked_sub(1) {
        if !bytes.get(previous).is_some_and(word) {
            break;
        }
        start = previous;
    }
    while bytes.get(end).is_some_and(word) {
        end = end.saturating_add(1);
    }
    let text = text.get(start..end)?;
    if text.is_empty() {
        return None;
    }
    Some((text.to_string(), (start as u32, end as u32)))
}

fn covers(span: Span, offset: u32) -> bool {
    !span.is_none() && span.start <= offset && offset <= span.end
}

// ---------------------------------------------------------------------------
// From a table entry back to the syntax that declared it
// ---------------------------------------------------------------------------

fn module_syntax(analyzed: &Analyzed, module: ModuleId) -> Option<&tree::Module> {
    analyzed.analysis.loaded.modules.get(module.index()).map(|m| m.ast.as_ref())
}

/// The item a declaration's name span belongs to. Every table entry's span is
/// the name it declares, so matching on it is exact.
fn declaration_syntax(
    analyzed: &Analyzed,
    module: ModuleId,
    name: Span,
) -> Option<(&flat::Tree, &Item)> {
    let ast = module_syntax(analyzed, module)?;
    let item = ast.items.iter().find(|item| {
        let declared = match item {
            Item::Fn(d) => d.name.span,
            Item::Struct(d) => d.name.span,
            Item::Enum(d) => d.name.span,
            Item::TypeAlias(d) => d.name.span,
            Item::Let(d) => d.name.span,
            Item::Trait(d) => d.name.span,
            Item::Context(d) => d.name.span,
            _ => return false,
        };
        declared == name
    })?;
    Some((&ast.tree, item))
}

/// The item an [`AstRef`] points at, for the tables that carry one.
fn item_syntax(analyzed: &Analyzed, ast: AstRef) -> Option<(&flat::Tree, &Item)> {
    let (module, index) = ast.item()?;
    let module = module_syntax(analyzed, module)?;
    let item = module.items.get(index as usize)?;
    Some((&module.tree, item))
}

/// The syntax of a function, whether it was written at the top level or inside
/// an `impl` or a `trait`.
fn function_syntax(analyzed: &Analyzed, id: FnId) -> Option<(&flat::Tree, &tree::FnDecl)> {
    let ast = analyzed.analysis.checked.tables.fn_info(id).ast;
    let (tree, item) = item_syntax(analyzed, ast)?;
    match (ast, item) {
        (AstRef::Item { .. }, Item::Fn(d)) => Some((tree, d)),
        (AstRef::Method { sub, .. }, Item::Impl(d)) => {
            d.methods.get(sub as usize).map(|m| (tree, m))
        }
        (AstRef::Method { sub, .. }, Item::Trait(d)) => {
            d.methods.get(sub as usize).map(|m| (tree, m))
        }
        _ => None,
    }
}

/// A signature for a declaration with no syntax: the primitives' methods.
fn function_from_table(tables: &Tables, id: FnId) -> String {
    let info = tables.fn_info(id);
    format!(
        "fn {}({}): {}",
        info.name,
        parameters(tables, &info.params),
        types::show(tables, None, &[], &info.ret)
    )
}

fn parameters(tables: &Tables, params: &[types::ParamInfo]) -> String {
    params
        .iter()
        .map(|p| {
            format!("{}: {}", p.name, types::show(tables, None, &[], &p.ty))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn field_info(
    tables: &Tables,
    con: TyConId,
    variant: Option<usize>,
    index: usize,
) -> Option<&types::FieldInfo> {
    let con = tables.tycon(con);
    match variant {
        Some(v) => con.variants().get(v)?.fields.get(index),
        None => con.fields().get(index),
    }
}

/// A field's declaration, for its doc comment. Only a record field has one —
/// a tuple field has no name to write a comment above.
fn field_syntax(
    analyzed: &Analyzed,
    con: TyConId,
    variant: Option<usize>,
    index: usize,
) -> Option<(&flat::Tree, &tree::FieldDecl)> {
    let info = analyzed.analysis.checked.tables.tycon(con);
    let (tree, item) = declaration_syntax(analyzed, info.module, info.span)?;
    match (variant, item) {
        (None, Item::Struct(d)) => match &d.body {
            tree::StructBody::Record(fields) => fields.get(index).map(|f| (tree, f)),
            tree::StructBody::Tuple(_) => None,
        },
        (Some(v), Item::Enum(d)) => match &d.variants.get(v)?.payload {
            tree::VariantPayload::Record(fields) => fields.get(index).map(|f| (tree, f)),
            _ => None,
        },
        _ => None,
    }
}
