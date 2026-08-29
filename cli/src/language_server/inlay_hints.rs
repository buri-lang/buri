//! The two things a Buri file leaves unwritten, written back beside it.
//!
//! A **type** hint goes after a name the source bound without saying what it
//! is — a `let` with no annotation, a closure parameter with no annotation —
//! and says what the checker inferred, rendered by `types::show`, which is the
//! same renderer hover uses. A **parameter** hint goes before an argument and
//! says which parameter it lands in.
//!
//! Both are read out of what the analysis already holds, and the whole file is
//! answered in **one** pass: the syntax is scanned once for the names that
//! wrote no type, each typed body is walked once for the call sites, and the
//! resolver is not asked anything at all. That is deliberate — `symbols::at`
//! re-lexes the buffer to decide whether an offset is inside a literal, so a
//! request that called it per hint would lex a file once per hint.
//!
//! What a hint does *not* carry is its tooltip. That is `inlayHint/resolve`,
//! and it is where the one `symbols::at` per hovered hint is paid.

use crate::compiler::modules::ModuleData;
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{self, ParamInfo, ParamRole};
use crate::diagnostics::FileId;
use crate::json::Value;
use crate::parsing::flat::{self, ExprId, ExprView, PatId, PatView, StmtKind};
use crate::parsing::tree::Item;
use std::collections::BTreeSet;
use std::path::Path;
use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols::{self, Symbol};

/// An inferred type, written after the name that did not say it.
const TYPE: i64 = 1;

/// A parameter's name, written before the argument that fills it.
const PARAMETER: i64 = 2;

/// One hint, before it is a protocol object.
struct Hint {
    /// Where it is painted, as a byte offset into the file.
    at: u32,
    kind: i64,
    /// The label, in the parts the protocol wants so that `resolve` has
    /// somewhere to hang a location.
    parts: Vec<String>,
    /// Which part a location may be attached to.
    part: usize,
    /// The declaration the hint is about: the type for a type hint, the
    /// callable for a parameter hint. `None` when it has no file to name.
    about: Option<Symbol>,
    /// Which of the callable's parameters a parameter hint names, so that
    /// clicking the name goes to that parameter rather than to the whole
    /// declaration.
    parameter: Option<usize>,
    padding_right: bool,
}

/// Every hint in the requested range, in source order.
///
/// The range is what the client can see, and the filter is on the hint's own
/// position: a hint has no extent in the buffer, so "intersects" and "is
/// inside" are the same test for one.
pub fn hints(
    analyzed: &Analyzed,
    path: &Path,
    text: &str,
    from: u32,
    to: u32,
) -> Option<Value> {
    let file = analyzed.session.map.find(&analyzed.session.workspace.rel_of(path))?;
    let module = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file)?;
    let unwritten = names_with_no_written_type(module);

    let tables = &analyzed.analysis.checked.tables;
    let checked = &analyzed.analysis.checked;
    let mut found: Vec<Hint> = Vec::new();
    for (fid, body) in &checked.bodies {
        if tables.fn_info(*fid).span.file != file {
            continue;
        }
        for local in &body.locals {
            if local.span.file != file || !unwritten.contains(&(local.span.start, local.span.end)) {
                continue;
            }
            // A type the checker could not work out is not one to paint. The
            // squiggle already says so, and a hint reading `?` beside it would
            // be the server repeating itself in a place nobody asked.
            if local.ty.is_error() {
                continue;
            }
            found.push(Hint {
                at: local.span.end,
                kind: TYPE,
                parts: vec![": ".to_string(), types::show(tables, None, &[], &local.ty)],
                part: 1,
                about: local.ty.head().map(Symbol::Type),
                parameter: None,
                padding_right: false,
            });
        }
        parameter_hints(analyzed, file, text, &body.expr, &mut found);
    }
    // A module-level `let`'s value is checked on its own, with no body around
    // it — and it writes its type, so only its call sites can be hinted.
    for (id, expr) in &checked.consts {
        if tables.const_(*id).span.file == file {
            parameter_hints(analyzed, file, text, expr, &mut found);
        }
    }

    found.retain(|h| from <= h.at && h.at <= to);
    found.sort_by(|a, b| a.at.cmp(&b.at).then(a.kind.cmp(&b.kind)));
    found.dedup_by(|a, b| a.at == b.at && a.kind == b.kind && a.parts == b.parts);
    Some(render(analyzed, text, &found))
}

/// The protocol objects, with every position counted in one pass over the text.
fn render(analyzed: &Analyzed, text: &str, found: &[Hint]) -> Value {
    let offsets: Vec<u32> = found.iter().map(|h| h.at).collect();
    let positions = convert::positions_of(text, &offsets);
    Value::Array(
        found
            .iter()
            .zip(positions)
            .map(|(hint, position)| {
                let mut fields = vec![
                    ("position", position.to_json()),
                    (
                        "label",
                        Value::Array(
                            hint.parts
                                .iter()
                                .map(|p| Value::object(vec![("value", Value::str(p.as_str()))]))
                                .collect(),
                        ),
                    ),
                    ("kind", Value::number(hint.kind)),
                    ("paddingLeft", Value::Bool(false)),
                    ("paddingRight", Value::Bool(hint.padding_right)),
                ];
                // What `resolve` resolves the hint back from. A declaration
                // compiled into the binary has no file, so a hint about `I64`
                // or about a primitive's method carries none — and resolving it
                // hands the hint straight back, which is the honest answer when
                // the label already says everything there is.
                if let Some(data) = about(analyzed, hint) {
                    fields.push(("data", data));
                }
                Value::object(fields)
            })
            .collect(),
    )
}

/// The `data` a hint round-trips: where the declaration it is about writes its
/// name, and which label part a location belongs on.
fn about(analyzed: &Analyzed, hint: &Hint) -> Option<Value> {
    let symbol = hint.about.as_ref()?;
    let span = symbols::declaration_name(analyzed, symbol);
    if span.is_none() {
        return None;
    }
    let file = analyzed.session.map.get(span.file);
    if file.abs_path.as_os_str().is_empty() {
        return None;
    }
    let mut fields = vec![
        ("uri", Value::str(convert::uri_of(&file.abs_path))),
        ("position", convert::position_of(&file.text, span.start).to_json()),
        ("part", Value::number(hint.part as i64)),
    ];
    if let Some(index) = hint.parameter {
        fields.push(("parameter", Value::number(index as i64)));
    }
    Some(Value::object(fields))
}

/// The tooltip and the navigation a hovered hint earns.
///
/// The hint comes back as the client received it and goes back out with two
/// things added: the declaration rendered the way hover renders it, and a
/// location on the label part that names it, so the name in the hint is a
/// place you can go to. Everything else the client sent is preserved — the
/// protocol says a resolved hint is the hint, filled in.
pub fn resolve(analyzed: &Analyzed, path: &Path, text: &str, hint: &Value) -> Value {
    let mut resolved = hint.clone();
    let Some(position) = hint.at("data.position").and_then(Position::from_json) else {
        return resolved;
    };
    let part = hint.at("data.part").and_then(|p| p.as_u32()).unwrap_or(0) as usize;
    let offset = convert::offset_of(text, position);
    let Some(found) = symbols::at(analyzed, path, text, offset) else { return resolved };
    let (signature, docs) = symbols::describe(analyzed, &found.symbol);
    let mut markdown = format!("```buri\n{signature}\n```");
    if !docs.is_empty() {
        markdown.push_str("\n\n");
        markdown.push_str(&docs.join("\n"));
    }
    let parameter = hint.at("data.parameter").and_then(|p| p.as_u32()).map(|i| i as usize);
    let location = navigation(analyzed, &found.symbol, parameter);

    let Value::Object(fields) = &mut resolved else { return resolved };
    fields.insert(
        "tooltip".to_string(),
        Value::object(vec![
            ("kind", Value::str("markdown")),
            ("value", Value::str(markdown)),
        ]),
    );
    if let (Some(location), Some(Value::Array(parts))) = (location, fields.get_mut("label")) {
        if let Some(Value::Object(named)) = parts.get_mut(part) {
            named.insert("location".to_string(), location);
        }
    }
    resolved
}

/// Where clicking the hinted name goes.
///
/// For a parameter hint it is that parameter as the signature wrote it, which
/// is the declaration the label is quoting. For everything else it is where
/// `textDocument/definition` would send the cursor, so the two cannot point at
/// different lines.
fn navigation(analyzed: &Analyzed, symbol: &Symbol, parameter: Option<usize>) -> Option<Value> {
    let tables = &analyzed.analysis.checked.tables;
    let declared = parameter.and_then(|index| match symbol {
        Symbol::Function(id) => tables.fn_info(*id).params.get(index).map(|p| p.span),
        Symbol::TraitMethod { trait_id, method } => {
            tables.trait_(*trait_id).methods.get(*method)?.params.get(index).map(|p| p.span)
        }
        _ => None,
    });
    let span = declared
        .filter(|s| !s.is_none())
        .unwrap_or_else(|| symbols::declaration(analyzed, symbol));
    if span.is_none() {
        return None;
    }
    let file = analyzed.session.map.get(span.file);
    if file.abs_path.as_os_str().is_empty() {
        return None;
    }
    Some(Value::object(vec![
        ("uri", Value::str(convert::uri_of(&file.abs_path))),
        ("range", convert::range(&file.text, span)),
    ]))
}

// ---------------------------------------------------------------------------
// The parameter half
// ---------------------------------------------------------------------------

/// Every argument in one body that is worth naming.
fn parameter_hints(
    analyzed: &Analyzed,
    file: FileId,
    text: &str,
    root: &typed::Expr,
    out: &mut Vec<Hint>,
) {
    let tables = &analyzed.analysis.checked.tables;
    typed::walk(root, &mut |e| {
        let (params, args, about): (&[ParamInfo], _, Symbol) = match &e.kind {
            ExprKind::CallFn { func, args } => match func.decl() {
                Some(id) => (&tables.fn_info(id).params, args, Symbol::Function(id)),
                None => return,
            },
            ExprKind::CallTrait { trait_id, method, args, .. } => {
                match tables.trait_(*trait_id).methods.get(*method) {
                    Some(declared) => (
                        &declared.params,
                        args,
                        Symbol::TraitMethod { trait_id: *trait_id, method: *method },
                    ),
                    None => return,
                }
            }
            _ => return,
        };
        for (index, (param, arg)) in params.iter().zip(args).enumerate() {
            if param.role != ParamRole::Normal || param.name.is_empty() {
                continue;
            }
            if arg.span.file != file || arg.span.is_none() {
                continue;
            }
            // A receiver written postfix begins where the whole call begins,
            // which is the rule `symbols::after_receiver` reads the same way.
            if arg.span.start == e.span.start {
                continue;
            }
            if !opens_an_argument(text, arg.span.start) || !worth_naming(text, param, arg) {
                continue;
            }
            out.push(Hint {
                at: arg.span.start,
                kind: PARAMETER,
                parts: vec![param.name.clone(), ":".to_string()],
                part: 0,
                about: Some(about.clone()),
                parameter: Some(index),
                padding_right: true,
            });
        }
    });
}

/// Whether the text before an argument is the punctuation an argument list
/// writes.
///
/// This is what keeps an operator out. `a + b` is a call to a trait method
/// with two arguments in the tables, and `b` there follows a `+` rather than a
/// `(` or a `,` — so the one rule that says "this is written as an argument"
/// says no, and `a + other: b` never appears.
fn opens_an_argument(text: &str, start: u32) -> bool {
    let before = text.get(..start as usize).unwrap_or("").trim_end();
    before.ends_with('(') || before.ends_with(',')
}

/// Whether naming this argument tells the reader anything.
///
/// A literal always does: `250` says nothing about which parameter it is. A
/// bare name does only when it differs from the parameter's — `total(count)`
/// filling `count` is already labelled by the source, and repeating it is
/// noise. Everything else — a nested call, an arithmetic expression — gets no
/// hint at all, because the argument is already long enough to read.
fn worth_naming(text: &str, param: &ParamInfo, arg: &typed::Expr) -> bool {
    if matches!(
        arg.kind,
        ExprKind::Int(..)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit
    ) {
        return true;
    }
    let Some(written) = text.get(arg.span.start as usize..arg.span.end as usize) else {
        return false;
    };
    let bare = !written.is_empty()
        && written.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && written.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    // The last segment, because `order.count` filling `count` reads as labelled
    // already even though the whole path is longer than the name.
    bare && written.rsplit('.').next() != Some(param.name.as_str())
}

// ---------------------------------------------------------------------------
// The names the source bound without saying what they are
// ---------------------------------------------------------------------------

/// Every name a `let` or a closure binds and writes no type for, as a byte
/// range.
///
/// The typed tree cannot answer this: it holds the type a name *has* and never
/// whether one was written, so the question is the syntax's. The scan is one
/// pass over the expression arena plus the blocks the declarations own —
/// statements live only in blocks, and a block is reached from a function
/// body, a `Block` expression or an `if`'s consequent and from nowhere else.
fn names_with_no_written_type(module: &ModuleData) -> BTreeSet<(u32, u32)> {
    let tree = &module.ast.tree;
    let mut out = BTreeSet::new();
    let mut blocks: Vec<flat::BlockId> = Vec::new();
    for item in &module.ast.items {
        match item {
            Item::Fn(d) => blocks.extend(d.body),
            Item::Impl(d) => blocks.extend(d.methods.iter().filter_map(|m| m.body)),
            Item::Trait(d) => blocks.extend(d.methods.iter().filter_map(|m| m.body)),
            Item::Test(d) => blocks.push(d.body),
            _ => {}
        }
    }
    for index in 0..tree.nodes().len() {
        match tree.expr(ExprId(index as u32)) {
            ExprView::Block { block, .. } => blocks.push(block),
            ExprView::If { then, .. } => blocks.push(then),
            ExprView::Lambda { params, .. } => {
                for p in params.iter().filter(|p| p.ty == flat::NONE) {
                    out.insert((p.name.start, p.name.end));
                }
            }
            _ => {}
        }
    }
    for block in blocks {
        let data = tree.block(block);
        for stmt in tree.stmts_at(data.stmts_start, data.stmts_len) {
            // `let ctx = ...` writes no pattern and takes no annotation, so
            // there is no name in it that a type could be hinted after.
            if stmt.kind != StmtKind::Let || stmt.is_ctx || stmt.ty != flat::NONE {
                continue;
            }
            if let Some(pattern) = tree.opt_pat(stmt.pattern) {
                bound_names(tree, pattern, &mut out);
            }
        }
    }
    out
}

/// Every name one pattern binds. A destructuring `let` binds several, and each
/// of them is a name the source wrote with no type beside it.
fn bound_names(tree: &flat::Tree, pattern: PatId, out: &mut BTreeSet<(u32, u32)>) {
    match tree.pat(pattern) {
        PatView::Bind { name_span, sub, .. } => {
            out.insert((name_span.start, name_span.end));
            if let Some(sub) = sub {
                bound_names(tree, sub, out);
            }
        }
        PatView::Tuple { elems, .. } | PatView::Array { elems, .. } | PatView::Or { alts: elems, .. } => {
            for elem in elems {
                bound_names(tree, *elem, out);
            }
        }
        PatView::Path { payload: Some(payload), .. } => {
            if payload.record {
                for field in tree.fpats_at(payload.start, payload.len) {
                    match tree.opt_pat(field.pattern) {
                        Some(sub) => bound_names(tree, sub, out),
                        // The shorthand `{ count }` binds the field's own name.
                        None => {
                            out.insert((field.span.start, field.span.end));
                        }
                    }
                }
            } else {
                for sub in tree.pkids_at(payload.start, payload.len) {
                    bound_names(tree, *sub, out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::semantics::types::{LocalId, Ty};
    use crate::diagnostics::Span;

    fn parameter(name: &str) -> ParamInfo {
        ParamInfo {
            name: name.to_string(),
            ty: Ty::Unit,
            role: ParamRole::Normal,
            span: Span::NONE,
        }
    }

    fn argument(text: &str, kind: ExprKind) -> typed::Expr {
        let span = Span { file: FileId(0), start: 0, end: text.len() as u32 };
        typed::Expr::new(kind, Ty::Unit, span)
    }

    /// The rule that keeps an operator out of the parameter hints. `a + b` is a
    /// trait call with two arguments in the tables and no argument list in the
    /// source, and this is the only thing that can tell the difference.
    #[test]
    fn an_operand_is_not_an_argument() {
        assert!(opens_an_argument("line(", 5));
        assert!(opens_an_argument("line(1, ", 8));
        // The list broken over lines is still a list.
        assert!(opens_an_argument("line(\n    ", 10));
        assert!(!opens_an_argument("running + ", 10));
        assert!(!opens_an_argument("-", 1));
        assert!(!opens_an_argument("", 0));
    }

    #[test]
    fn a_literal_is_worth_naming_and_a_name_that_repeats_the_parameter_is_not() {
        let literal = argument("250", ExprKind::Int(250, false));
        assert!(worth_naming("250", &parameter("price"), &literal));

        let name = argument("each", ExprKind::Local(LocalId(0)));
        assert!(worth_naming("each", &parameter("price"), &name));

        let same = argument("count", ExprKind::Local(LocalId(0)));
        assert!(!worth_naming("count", &parameter("count"), &same));

        // A path already ending in the parameter's name reads as labelled too.
        let path = argument("order.count", ExprKind::Local(LocalId(0)));
        assert!(!worth_naming("order.count", &parameter("count"), &path));

        // Anything with structure is long enough to read on its own.
        let call = argument("line(1, 2)", ExprKind::Tuple(Vec::new()));
        assert!(!worth_naming("line(1, 2)", &parameter("price"), &call));
    }
}
