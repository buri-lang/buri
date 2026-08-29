//! What could be written where the cursor is.
//!
//! Completion is the one request an editor sends on almost every keystroke, so
//! everything here is a read of what the analysis already holds and nothing
//! here resolves a name twice. `ModuleScope` is the list of what a module can
//! say unqualified; a checked `Body` is the list of what is bound inside a
//! function; `Tables` holds every field, variant and method of a type. Those
//! three are the whole of the answer, and each is one scan of one table rather
//! than a question asked once per candidate — the rule
//! [`super::symbols::Resolver`] exists for.
//!
//! **The buffer is mid-keystroke, always.** A file being typed into is a file
//! that does not parse and often does not check, so nothing here may depend on
//! either succeeding: a file the analysis could not load at all falls back to
//! the words the lexer can still see in it, and a context with no candidates
//! answers with an empty list rather than with a guess.
//!
//! Where the cursor is decides what is offered, and there are six places here:
//! a module path, an import clause, a member after a `.`, a variant after a
//! bare `.`, a written type, and ordinary code. A `BUILD.buri` has a seventh,
//! which is [`super::build_files`]'s answer rather than this one's.

use crate::compiler::semantics::resolve::{ModuleScope, Sym};
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::{self as types, ModuleId, Ty, TyConId};
use crate::compiler::standard_library;
use crate::diagnostics::Span;
use crate::json::Value;
use crate::parsing::flat::{TypeId, TypeView};
use crate::parsing::lexer::Keyword;
use std::path::Path;
use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols::{self, Symbol};

/// Everything the cursor could be completing.
///
/// The tests run in the order of how much is known: a string being typed is
/// unambiguous, a `.` says what kind of name follows it, and ordinary code is
/// what is left over.
pub fn completion(analyzed: &Analyzed, path: &Path, text: &str, position: Position) -> Value {
    let offset = convert::offset_of(text, position) as usize;
    // `offset_of` clamps into `text` and onto a character boundary, so the only
    // way this fails is a caller that did not go through it.
    let Some(before_cursor) = text.get(..offset) else {
        return Value::Array(Vec::new());
    };
    let line = before_cursor.rsplit('\n').next().unwrap_or(before_cursor);
    let cursor = offset as u32;

    // An odd number of quotes before the cursor means the cursor is inside
    // one. Counting is what distinguishes `from "core/st|` — still typing the
    // path — from `from "core/str" |`, where the string is closed and the
    // answer is a different one.
    let inside_string = line.bytes().filter(|c| *c == b'"').count() % 2 == 1;
    if inside_string && line.trim_start().starts_with("from") {
        let prefix = line.rsplit('"').next().unwrap_or(line);
        let from = cursor.saturating_sub(prefix.len() as u32);
        return module_paths(analyzed, path, text, prefix, (from, cursor));
    }

    // Inside the `{ … }` of `from "path" import { … }`.
    if !inside_string && line.contains("import") && line.rfind('{') > line.rfind('}') {
        if let Some(p) = line.split('"').nth(1) {
            let typed = partial_name(line);
            let from = cursor.saturating_sub(typed.len() as u32);
            return exported_names(analyzed, path, p, text, (from, cursor));
        }
    }
    // A literal is prose and a comment is prose. Neither is a place a name
    // goes, and completing inside one is offering to corrupt a sentence.
    if inside_string || in_a_comment(line) {
        return Value::Array(Vec::new());
    }

    Value::Array(in_source(analyzed, path, text, cursor))
}

/// The run of name characters immediately before the cursor — what an accepted
/// item replaces.
fn partial_name(line: &str) -> &str {
    let start = line
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(line.len(), |(i, _)| i);
    line.get(start..).unwrap_or("")
}

/// Whether a `//` opened a comment on this line before the cursor. A `//`
/// inside a string is a label — `from "//lib/money"` — and not one.
fn in_a_comment(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut quoted = false;
    for (i, c) in bytes.iter().enumerate() {
        match c {
            b'"' => quoted = !quoted,
            b'/' if !quoted && bytes.get(i.saturating_add(1)) == Some(&b'/') => return true,
            _ => {}
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Ordinary code
// ---------------------------------------------------------------------------

/// The candidates for a cursor in a `.buri` source.
fn in_source(analyzed: &Analyzed, path: &Path, text: &str, cursor: u32) -> Vec<Value> {
    let (prefix, from) = word_before(text, cursor);
    let replacing = (from, cursor);
    let uri = convert::uri_of(path);

    // `x.fie` and `.Var` are two questions, and which one this is depends on
    // whether anything precedes the dot.
    if let Some(dot) = dot_before(text, from) {
        let (receiver, receiver_at) = word_before(text, dot);
        let found = receiver_symbol(analyzed, path, text, (receiver, receiver_at), dot);
        let members = match found {
            // `list.map` — a namespace, whose members are what the module it
            // names publishes.
            Some(Symbol::Module(id)) => module_members(analyzed, id),
            // `Color.Red` — the type's own variants. A type is not a value, so
            // its fields and its methods are not reachable through it.
            Some(Symbol::Type(con)) => variants(analyzed, con),
            Some(symbol) => match symbols::type_of(analyzed, &symbol) {
                Some(con) => members_of(analyzed, con, module_id(analyzed, path)),
                None => Vec::new(),
            },
            // A bare `.` completes a variant of whatever type is expected
            // here — the inferred-variant form, which is the one place a name
            // is written with no receiver at all.
            None if receiver.is_empty() => match expected_enum(analyzed, path, text, dot) {
                Some(con) => variants(analyzed, con),
                None => Vec::new(),
            },
            // A receiver that resolves to nothing: the file does not check
            // that far yet, and every name in the repository is not the answer
            // to "what is on this value".
            None => Vec::new(),
        };
        return rendered(analyzed, members, prefix, text, replacing, &uri, None);
    }

    let Some((module_path, scope)) = module_scope(analyzed, path) else {
        // No analysis of this file at all: one too broken to load, or one no
        // target declares yet. The buffer itself is still a list of names.
        return lexical(text, prefix, replacing, &uri);
    };

    // A written type is the one context where a value would not typecheck, so
    // it is the one context that offers no values.
    if in_a_type(analyzed, path, cursor) {
        let named = scope
            .names
            .iter()
            .filter_map(|(name, sym)| Some((name.clone(), symbols::symbol_of(sym)?)))
            .filter(|(_, symbol)| {
                matches!(symbol, Symbol::Type(_) | Symbol::Trait(_) | Symbol::Module(_))
            })
            .chain(primitives(analyzed))
            .chain(namespaces(scope))
            .collect();
        let items = rendered(analyzed, named, prefix, text, replacing, &uri, Some(module_path));
        return or_lexical(items, text, prefix, replacing, &uri);
    }

    let mut candidates = locals_at(analyzed, path, cursor);
    candidates.extend(
        scope.names.iter().filter_map(|(name, sym)| Some((name.clone(), symbols::symbol_of(sym)?))),
    );
    candidates.extend(namespaces(scope));
    let mut items = rendered(analyzed, candidates, prefix, text, replacing, &uri, Some(module_path));
    items.extend(keywords(prefix, text, replacing));
    or_lexical(items, text, prefix, replacing, &uri)
}

/// The words in the buffer, where the analysis found nothing.
///
/// A file that loads and does not check has a scope with the module's own
/// declarations in it and nothing about the half-written function the cursor
/// is in — so a name bound two lines up is invisible exactly when it is being
/// referred to. The lexer still sees it.
fn or_lexical(
    items: Vec<Value>,
    text: &str,
    prefix: &str,
    replacing: (u32, u32),
    uri: &str,
) -> Vec<Value> {
    if items.is_empty() {
        return lexical(text, prefix, replacing, uri);
    }
    items
}

/// What the receiver of a `.` refers to.
///
/// The name is looked up before the typed tree is, and that order is the whole
/// point. `money.la` is a field access to a field that does not exist yet, so
/// the checker replaced the whole access — receiver included — with an error,
/// and asking the typed tree what `money` is answers nothing exactly when the
/// answer is wanted. A parameter's type is on its declaration and a module's
/// names are in its scope, and neither depends on the body checking.
///
/// The tree is still asked afterwards, because it is what knows the receivers
/// that are not a name: `fromCents(1).` and `money.label.` resolve there and
/// nowhere else.
fn receiver_symbol(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    receiver: (&str, u32),
    dot: u32,
) -> Option<Symbol> {
    let (name, at) = receiver;
    if name.is_empty() {
        return None;
    }
    // The innermost binding of the name wins, which for a shadowed one is the
    // last the body recorded.
    if let Some((_, symbol)) = locals_at(analyzed, path, dot).into_iter().rev().find(|(n, _)| n == name)
    {
        return Some(symbol);
    }
    if let Some((_, scope)) = module_scope(analyzed, path) {
        if let Some(symbol) = scope.names.get(name).and_then(symbols::symbol_of) {
            return Some(symbol);
        }
        // A namespace alias is kept in its own map rather than among the
        // names, because it is not one: nothing may be written unqualified.
        if let Some(id) = scope.namespaces.get(name) {
            return Some(Symbol::Module(*id));
        }
    }
    symbols::Resolver::of(analyzed, path, text)?.at(at).map(|f| f.symbol)
}

/// The run of `[A-Za-z0-9_]` ending at `at`, and where it starts.
fn word_before(text: &str, at: u32) -> (&str, u32) {
    let Some(before) = text.get(..at as usize) else { return ("", at) };
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(before.len(), |(i, _)| i);
    (before.get(start..).unwrap_or(""), start as u32)
}

/// The offset of the `.` immediately before `at`, if there is one.
///
/// Immediately: `x .f` is not a member access anybody writes, and a `.` after
/// a digit is a number rather than a receiver.
fn dot_before(text: &str, at: u32) -> Option<u32> {
    let before = text.get(..at as usize)?;
    let dot = before.strip_suffix('.')?;
    (!dot.ends_with(|c: char| c.is_ascii_digit())).then_some(dot.len() as u32)
}

/// The module the file holds, if the analysis loaded one.
fn module_id(analyzed: &Analyzed, path: &Path) -> Option<ModuleId> {
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;
    analyzed.analysis.loaded.modules.iter().find(|m| m.file == file).map(|m| m.id)
}

/// The path and the scope of the module the file holds, if the analysis loaded
/// one.
fn module_scope<'a>(analyzed: &'a Analyzed, path: &Path) -> Option<(&'a str, &'a ModuleScope)> {
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;
    let module = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file)?;
    let scope = analyzed.analysis.checked.scopes.get(module.id.index())?;
    Some((module.path.as_str(), scope))
}

/// Whether the cursor is inside a type the source wrote.
///
/// Asked of the syntax rather than guessed from the character before the
/// cursor: a `:` introduces a type in `let total: I64` and a value in
/// `Money { cents: 4 }`, and only the parser knows which of the two it read.
fn in_a_type(analyzed: &Analyzed, path: &Path, cursor: u32) -> bool {
    let Some(file) = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path)) else {
        return false;
    };
    let Some(module) = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file) else {
        return false;
    };
    let tree = &module.ast.tree;
    (0..tree.type_nodes().len()).any(|index| {
        let TypeView::Named { span, .. } = tree.ty(TypeId(index as u32)) else { return false };
        span.file == file && covers(span, cursor)
    })
}

/// Every local and parameter bound before the cursor in the function the
/// cursor is in.
///
/// "Before" rather than "in scope": a binding is written above its uses, and
/// deciding scope exactly would mean re-walking the block structure of a body
/// the checker already flattened. A name from a sibling block is a row a
/// reader ignores; leaving out the one they are about to type is the failure
/// this request has.
fn locals_at(analyzed: &Analyzed, path: &Path, cursor: u32) -> Vec<(String, Symbol)> {
    let Some(file) = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for body in analyzed.analysis.checked.bodies.values() {
        if body.expr.span.file != file || !covers(body.expr.span, cursor) {
            continue;
        }
        for (index, local) in body.locals.iter().enumerate() {
            // A parameter is in scope throughout, wherever its own span is.
            let parameter = body.params.iter().any(|p| p.index() == index);
            if !parameter && (local.span.file != file || local.span.start > cursor) {
                continue;
            }
            let symbol = Symbol::Local {
                name: local.name.clone(),
                ty: local.ty.clone(),
                span: local.span,
            };
            out.push((local.name.clone(), symbol));
        }
    }
    out
}

/// The enum a bare `.` is completing a variant of.
///
/// Two things say what is expected there and both are already computed: a
/// `match` says it with its scrutinee, and every other position says it with
/// the type the checker gave the expression the cursor is inside. Where
/// neither answers, the source may still have written the type down — and
/// where nothing has, there is no expected type, and every variant in the
/// repository is not one.
fn expected_enum(analyzed: &Analyzed, path: &Path, text: &str, cursor: u32) -> Option<TyConId> {
    checked_enum(analyzed, path, cursor).or_else(|| annotated_enum(analyzed, path, text, cursor))
}

/// The enum the type annotation on this line names.
///
/// `let paid: Status = .` does not parse — a `.` with no name after it is not
/// an expression — so the checker has nothing to say about it, and the whole
/// point of the inferred-variant form is that this is where it is written. The
/// annotation is still there in the syntax, and the nearest one that ends
/// before the cursor on the same line is the type being built.
fn annotated_enum(analyzed: &Analyzed, path: &Path, text: &str, cursor: u32) -> Option<TyConId> {
    let (_, scope) = module_scope(analyzed, path)?;
    let before = text.get(..cursor as usize)?;
    let line = before.rsplit('\n').next()?;
    // Read rather than parsed, because the statement this is in does not
    // parse: a `.` with no name after it is not an expression, so the syntax
    // that would hold the annotation was never built.
    let annotated = line.rsplit_once(':')?.1;
    let written = annotated.trim_start();
    let name: String =
        written.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    let Some(Sym::Ty(con)) = scope.names.get(&name) else { return None };
    let has_variants = !analyzed.analysis.checked.tables.tycon(*con).variants().is_empty();
    has_variants.then_some(*con)
}

/// The enum the checker expects here, where it checked this far.
fn checked_enum(analyzed: &Analyzed, path: &Path, cursor: u32) -> Option<TyConId> {
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;
    let tables = &analyzed.analysis.checked.tables;
    let mut best: Option<(u32, TyConId)> = None;
    let mut offer = |span: Span, ty: &Ty| {
        if span.file != file || !covers(span, cursor) {
            return;
        }
        let Some(con) = ty.head() else { return };
        if tables.tycon(con).variants().is_empty() {
            return;
        }
        let width = span.end.saturating_sub(span.start);
        if best.is_none_or(|(w, _)| width < w) {
            best = Some((width, con));
        }
    };
    for body in analyzed.analysis.checked.bodies.values() {
        if body.expr.span.file != file || !covers(body.expr.span, cursor) {
            continue;
        }
        typed::walk(&body.expr, &mut |e| {
            // A `match` arm is a pattern rather than an expression, so the
            // whole `match` is the narrowest node covering one — and its own
            // type is the type of the arms' results, not of what is matched.
            if let typed::ExprKind::Match { scrutinee, .. } = &e.kind {
                if scrutinee.span.end <= cursor {
                    offer(e.span, &scrutinee.ty);
                    return;
                }
            }
            offer(e.span, &e.ty);
        });
    }
    best.map(|(_, con)| con)
}

fn covers(span: Span, offset: u32) -> bool {
    span.start <= offset && offset <= span.end
}

// ---------------------------------------------------------------------------
// What a type and a module offer
// ---------------------------------------------------------------------------

/// The fields and the methods a value of this type has.
///
/// A field the declaring module did not export is left out of a list read
/// anywhere else: offering it would be offering a `private-field`, which is a
/// diagnostic rather than a completion. Inside that module it is an ordinary
/// field, and leaving it out there would be hiding what the file is about.
fn members_of(analyzed: &Analyzed, con: TyConId, from: Option<ModuleId>) -> Vec<(String, Symbol)> {
    let tables = &analyzed.analysis.checked.tables;
    let declaring = tables.tycon(con).module;
    let mut out: Vec<(String, Symbol)> = tables
        .tycon(con)
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| f.exported || from == Some(declaring))
        .map(|(index, f)| (f.name.clone(), Symbol::Field { con, variant: None, index }))
        .collect();
    let names: Vec<String> = tables.method_names(con).map(str::to_string).collect();
    for name in names {
        if let Some(id) = tables.method(con, &name) {
            out.push((name, Symbol::Function(id)));
        }
    }
    // What the traits and the effects add. A trait method is called through the
    // receiver like an inherent one, and leaving them out hides every operator.
    for trait_id in tables.traits_of_con(con) {
        for (method, m) in tables.trait_(*trait_id).methods.iter().enumerate() {
            out.push((m.name.clone(), Symbol::TraitMethod { trait_id: *trait_id, method }));
        }
    }
    out
}

/// The namespace aliases the file imported. Kept out of `names` by the
/// resolver, because nothing may be written under one unqualified — but the
/// alias itself is a name a reader types, and `list.` is what follows it.
fn namespaces(scope: &ModuleScope) -> impl Iterator<Item = (String, Symbol)> + '_ {
    scope.namespaces.iter().map(|(name, id)| (name.clone(), Symbol::Module(*id)))
}

/// The primitives, by every name the language spells them with.
///
/// They are in no module's scope — the checker maps the name straight onto a
/// `Prim` — so a list built from `ModuleScope` alone leaves out `I64`, `Str`
/// and `Bool`, which are the commonest annotations there are.
fn primitives(analyzed: &Analyzed) -> impl Iterator<Item = (String, Symbol)> + '_ {
    let tables = &analyzed.analysis.checked.tables;
    types::Prim::all()
        .iter()
        .map(|p| p.name())
        .chain(["Int", "Float", "Uint", "Byte"])
        .filter_map(|name| {
            Some((name.to_string(), Symbol::Type(builtin(tables, name)?)))
        })
}

/// The type constructor a primitive's name stands for. The same table
/// `Checker::builtin_type` reads, so the two agree about what `Int` is.
fn builtin(tables: &types::Tables, name: &str) -> Option<TyConId> {
    let prim = match name {
        "Int" => types::Prim::I64,
        "Float" => types::Prim::F64,
        "Uint" => types::Prim::U64,
        "Byte" => types::Prim::U8,
        other => *types::Prim::all().iter().find(|p| p.name() == other)?,
    };
    Some(tables.prim_id(prim))
}

/// An enum's variants, for `Color.` and for a bare `.`.
fn variants(analyzed: &Analyzed, con: TyConId) -> Vec<(String, Symbol)> {
    analyzed
        .analysis
        .checked
        .tables
        .tycon(con)
        .variants()
        .iter()
        .enumerate()
        .map(|(index, v)| (v.name.clone(), Symbol::Variant { con, index }))
        .collect()
}

/// What a namespace import publishes.
fn module_members(analyzed: &Analyzed, module: ModuleId) -> Vec<(String, Symbol)> {
    let Some(scope) = analyzed.analysis.checked.scopes.get(module.index()) else {
        return Vec::new();
    };
    scope
        .exports
        .iter()
        .filter_map(|(name, sym)| Some((name.clone(), symbols::symbol_of(sym)?)))
        .collect()
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// The candidates that match what has been typed, as protocol items.
///
/// Sorted by label so the array does not depend on a hash order, and
/// deduplicated: a name is one row however many tables it was found in.
///
/// `module` is the module whose scope holds these names, where one does. It is
/// what makes `completionItem/resolve` able to find the declaration again —
/// nothing here puts a table id on the wire, because an id means something
/// only inside the analysis that minted it and the next request may have a
/// newer one.
fn rendered(
    analyzed: &Analyzed,
    mut candidates: Vec<(String, Symbol)>,
    prefix: &str,
    text: &str,
    replacing: (u32, u32),
    uri: &str,
    module: Option<&str>,
) -> Vec<Value> {
    candidates.retain(|(name, _)| name.starts_with(prefix));
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates.dedup_by(|a, b| a.0 == b.0);
    candidates
        .iter()
        .map(|(name, symbol)| {
            let (detail, _) = symbols::describe(analyzed, symbol);
            let mut data = vec![("uri", Value::str(uri))];
            if let Some(module) = module {
                data.push(("module", Value::str(module)));
                data.push(("name", Value::str(name)));
            }
            item(
                name,
                completion_kind(analyzed, symbol),
                &detail,
                nearness(symbol),
                text,
                replacing,
                Value::object(data),
            )
        })
        .collect()
}

/// The keywords, offered wherever a name could go.
///
/// Last in the sort: a keyword is four letters a reader types faster than they
/// read a row, and the reason to offer them at all is the reader who does not
/// yet know which words the language has. Not offered for an empty prefix,
/// where they would be twenty-five rows in front of everything.
fn keywords(prefix: &str, text: &str, replacing: (u32, u32)) -> Vec<Value> {
    if prefix.is_empty() {
        return Vec::new();
    }
    Keyword::ALL
        .iter()
        .map(|k| k.text())
        .filter(|k| k.starts_with(prefix))
        // 14 keyword.
        .map(|k| item(k, 14, "", '6', text, replacing, Value::Null))
        .collect()
}

/// Every word the buffer holds, for a file the analysis could not load.
///
/// The answer of last resort, and it says so: no kind, no signature, nothing
/// but the words already written. A file mid-rename, or one no `BUILD.buri`
/// lists yet, is still a file somebody is typing in, and the names in front of
/// them beat an empty list.
fn lexical(text: &str, prefix: &str, replacing: (u32, u32), uri: &str) -> Vec<Value> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let mut words: Vec<&str> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.starts_with(prefix) && *w != prefix)
        .collect();
    words.sort_unstable();
    words.dedup();
    let mut items: Vec<Value> = words
        .iter()
        // 1 text: a word written here, and nothing more claimed about it.
        .map(|w| item(w, 1, "", '7', text, replacing, Value::object(vec![("uri", Value::str(uri))])))
        .collect();
    items.extend(keywords(prefix, text, replacing));
    items
}

// ---------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------

/// Every module the file could legally import: the standard library, and the
/// packages its own target already declares. Offering a label the target does
/// not depend on would be offering a `missing-dep`.
///
/// `detail` says which of those three a path is, because that is the thing a
/// reader cannot see in the path itself — `//lib/money` looks the same whether
/// the target already depends on it or it is this package's own label.
fn module_paths(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    prefix: &str,
    replacing: (u32, u32),
) -> Value {
    let mut out: Vec<(String, Origin)> = standard_library::MODULES
        .iter()
        .map(|m| (m.path.to_string(), Origin::StandardLibrary))
        .collect();
    if let Some(package) = analyzed.session.workspace.owning_package(path) {
        for t in analyzed.session.workspace.targets().into_iter().filter(|t| t.package == package) {
            for d in analyzed.session.workspace.declared_deps(t) {
                out.push((d.value.clone(), Origin::Dependency));
            }
        }
        out.push((analyzed.session.workspace.package(package).label(), Origin::ThisPackage));
    }
    // Sorted by path so the array does not depend on which target was met
    // first; the nearer origin wins where two agree on a path.
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.rank().cmp(&b.1.rank())));
    out.dedup_by(|a, b| a.0 == b.0);
    let uri = convert::uri_of(path);
    Value::Array(
        out.iter()
            .filter(|(m, _)| m.starts_with(prefix))
            .map(|(m, origin)| {
                item(
                    m,
                    // 9 module.
                    9,
                    origin.detail(),
                    origin.rank(),
                    text,
                    replacing,
                    Value::object(vec![("uri", Value::str(&uri)), ("module", Value::str(m))]),
                )
            })
            .collect(),
    )
}

/// Where a module an import could name comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    ThisPackage,
    Dependency,
    StandardLibrary,
}

impl Origin {
    /// What an editor shows in the row, and — as the leading character of
    /// `sortText` — the order the rows come in. Nearest first: the package you
    /// are in, then what it already depends on, then the standard library.
    fn detail(self) -> &'static str {
        match self {
            Origin::ThisPackage => "this package",
            Origin::Dependency => "dependency",
            Origin::StandardLibrary => "standard library",
        }
    }

    fn rank(self) -> char {
        match self {
            Origin::ThisPackage => '0',
            Origin::Dependency => '1',
            Origin::StandardLibrary => '2',
        }
    }
}

/// What a module exports, for the `{ … }` half.
///
/// `detail` is the signature the formatter would print, which is what makes an
/// import clause readable without opening the module. The `///` prose is not
/// here: see [`resolve_completion`].
fn exported_names(
    analyzed: &Analyzed,
    file: &Path,
    module_path: &str,
    text: &str,
    replacing: (u32, u32),
) -> Value {
    let Some(&id) = analyzed.analysis.loaded.by_path.get(module_path) else {
        return Value::Array(Vec::new());
    };
    let Some(scope) = analyzed.analysis.checked.scopes.get(id.index()) else {
        return Value::Array(Vec::new());
    };
    let mut names: Vec<&String> = scope.exports.keys().collect();
    names.sort();
    let uri = convert::uri_of(file);
    Value::Array(
        names
            .iter()
            .map(|n| {
                let sym = scope.exports.get(n.as_str());
                let symbol = sym.and_then(symbols::symbol_of);
                let (kind, rank, detail) = match (&symbol, sym) {
                    (Some(s), _) => {
                        (completion_kind(analyzed, s), imported(s), symbols::describe(analyzed, s).0)
                    }
                    // A method a module put on its surface so that a library's
                    // `lib.buri` can re-export it. It has no entry of its own
                    // to describe — the receiver it hangs off is the whole of
                    // what the name says. 2 is method.
                    (None, Some(Sym::Method(receiver))) => {
                        (2, '1', format!("method on {receiver}"))
                    }
                    // 6 variable — the protocol has no "exported name".
                    _ => (6, '3', String::new()),
                };
                item(
                    n,
                    kind,
                    &detail,
                    rank,
                    text,
                    replacing,
                    Value::object(vec![
                        ("uri", Value::str(&uri)),
                        ("module", Value::str(module_path)),
                        ("name", Value::str(n.as_str())),
                    ]),
                )
            })
            .collect(),
    )
}

/// The protocol's `CompletionItemKind` for a declaration, so that an editor's
/// icons say what a name is rather than that it is a name.
pub(super) fn completion_kind(analyzed: &Analyzed, symbol: &Symbol) -> i64 {
    match symbol {
        // 13 enum, 22 struct — the two shapes a type declaration has.
        Symbol::Type(id) => {
            if analyzed.analysis.checked.tables.tycon(*id).variants().is_empty() {
                22
            } else {
                13
            }
        }
        // 8 interface: the protocol has no trait and no effect, and every
        // server for a language with traits reports this one.
        Symbol::Trait(_) | Symbol::Context(_) => 8,
        // 3 function, 2 method — a function with a `self` is reached through a
        // receiver, and the icon should say so.
        Symbol::Function(id) => {
            if analyzed.analysis.checked.tables.fn_info(*id).self_ty.is_some() {
                2
            } else {
                3
            }
        }
        Symbol::TraitMethod { .. } => 2,
        // 21 constant, 20 enum member, 5 field, 9 module, 6 variable.
        Symbol::Const(_) => 21,
        Symbol::Variant { .. } => 20,
        Symbol::Field { .. } => 5,
        Symbol::Module(_) => 9,
        Symbol::Local { .. } => 6,
    }
}

/// The leading character of `sortText` inside an import clause: types before
/// the functions over them, then the constants, then everything else.
/// Alphabetical order puts every capitalized name above every lowercase one,
/// which is a fact about ASCII rather than about what a reader is looking for.
fn imported(symbol: &Symbol) -> char {
    match symbol {
        Symbol::Type(_) | Symbol::Trait(_) | Symbol::Context(_) | Symbol::Variant { .. } => '0',
        Symbol::Function(_) | Symbol::TraitMethod { .. } => '1',
        Symbol::Const(_) => '2',
        Symbol::Field { .. } | Symbol::Module(_) | Symbol::Local { .. } => '3',
    }
}

/// The same, in code: nearest first. What is bound in this function beats what
/// the module declares, which beats a type — because the nearer a name was
/// written, the likelier it is the one being typed.
fn nearness(symbol: &Symbol) -> char {
    match symbol {
        Symbol::Local { .. } => '0',
        Symbol::Field { .. } | Symbol::Variant { .. } => '1',
        Symbol::Function(_) | Symbol::TraitMethod { .. } => '2',
        Symbol::Type(_) | Symbol::Trait(_) | Symbol::Context(_) => '3',
        Symbol::Const(_) => '4',
        Symbol::Module(_) => '5',
    }
}

/// One item, with the range it replaces.
///
/// Every item carries its own `textEdit`, because the client's idea of what a
/// word is is not this language's: a module path holds a `/`, and a client
/// guessing the range from its own word rule replaces the last segment of one
/// and leaves the rest.
pub(super) fn item(
    label: &str,
    kind: i64,
    detail: &str,
    rank: char,
    text: &str,
    replacing: (u32, u32),
    data: Value,
) -> Value {
    let (from, to) = replacing;
    let mut fields = vec![
        ("label", Value::str(label)),
        ("kind", Value::number(kind)),
        ("sortText", Value::str(format!("{rank}{label}"))),
        (
            "textEdit",
            Value::object(vec![
                (
                    "range",
                    Value::object(vec![
                        ("start", convert::position_of(text, from).to_json()),
                        ("end", convert::position_of(text, to).to_json()),
                    ]),
                ),
                ("newText", Value::str(label)),
            ]),
        ),
    ];
    // A name the tables could not describe has no detail, and an empty string
    // is a row with a blank column in it rather than a row with one column.
    if !detail.is_empty() {
        fields.push(("detail", Value::str(detail)));
    }
    if !matches!(data, Value::Null) {
        fields.push(("data", data));
    }
    Value::object(fields)
}

/// `completionItem/resolve`: the `///` prose for the one item a reader is on.
///
/// The list already carries the signature, so what is left is the documentation
/// — and that is the half worth withholding, because attaching it to every item
/// would put every doc comment in a module on the wire to show one of them.
pub fn resolve_completion(analyzed: &Analyzed, item: &Value) -> Value {
    let mut resolved = item.clone();
    let docs = (|| {
        let module_path = item.at("data.module")?.as_str()?;
        let &id = analyzed.analysis.loaded.by_path.get(module_path)?;
        let symbol = match item.at("data.name").and_then(|n| n.as_str()) {
            // A name inside the braces is the export it stands for; a name in
            // code is whatever that module can say unqualified, which is a
            // wider list and includes what it imported.
            Some(name) => {
                let scope = analyzed.analysis.checked.scopes.get(id.index())?;
                let sym = scope.exports.get(name).or_else(|| scope.names.get(name))?;
                symbols::symbol_of(sym)?
            }
            // No name is the path itself, and what a path names is a module.
            None => Symbol::Module(id),
        };
        let (_, docs) = symbols::describe(analyzed, &symbol);
        (!docs.is_empty()).then(|| docs.join("\n"))
    })();
    let (Some(docs), Value::Object(fields)) = (docs, &mut resolved) else { return resolved };
    fields.insert(
        "documentation".to_string(),
        Value::object(vec![("kind", Value::str("markdown")), ("value", Value::str(docs))]),
    );
    resolved
}
