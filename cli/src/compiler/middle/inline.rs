//! Inlining and folding, over the monomorphized tree. Was `optimize.rs`; the
//! name says what it does, and the folding is interleaved with the inlining
//! rather than a pass of its own, because inlining a constructor into a
//! projection is what makes most folding possible and folding is what exposes
//! the next round's call sites.
//!
//! This runs between monomorphization and the backend, which is the one point
//! where the whole program is present, every type is concrete, and nothing has
//! been committed to JavaScript yet. Because Buri has no dynamic dispatch, the
//! call graph here is exact (see `monomorphize.rs`), so a decision taken from it is a
//! fact rather than an estimate.
//!
//! **Function indices never move.** `Program::entry` and `TestEntry::func` are
//! `FuncIdx`, as are the `Callee::Func` inside `CallFn` and `FnRef`. Everything
//! here rewrites bodies in place; a function nothing calls any more is left for
//! `javascript::eliminate_dead` to drop by name.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every operand is a count of things already in memory — nodes in a body, functions in the program, slots in one function's local table — and nothing here subtracts, so no sum or product can leave the range of the machine holding them"
)]

use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::LocalId;
use crate::compiler::middle::monomorphize::{Func, Program};
use crate::compiler::middle::strongly_connected;

pub struct Options {
    pub inline: bool,
    /// How many times the pipeline may run. Inlining exposes calls that were
    /// not visible before, so one round is not enough; the loop also stops
    /// early once a round changes nothing.
    pub rounds: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options { inline: true, rounds: 3 }
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub rounds: usize,
    pub inlined: usize,
    pub folded: usize,
}

/// A body at or below this many nodes is inlined wherever it is called: a
/// projection, a literal, or a single call, where the call itself was most of
/// the cost. Higher than this and a function called from several places costs
/// more in duplicated code than it saves in frames.
const TRIVIAL: usize = 6;

/// A body with exactly one call site and no other reference is inlined up to
/// this size: moving it costs nothing, because the original becomes
/// unreachable and is dropped.
const SINGLE_USE: usize = 96;

/// A function stops accepting inlined bodies once it has grown past
/// `original * 2 + this`. Without a ceiling, a chain of small functions can
/// compound.
const GROWTH: usize = 96;

pub fn run(program: &mut Program, opts: &Options) -> Stats {
    let mut stats = Stats::default();
    if !opts.inline {
        return stats;
    }
    // Measured once, from the original bodies: the ceiling must not move as
    // inlining grows a function, or a chain of small functions compounds.
    let limits: Vec<usize> =
        program.funcs.iter().map(|f| body_size(f) * 2 + GROWTH).collect();

    for _ in 0..opts.rounds {
        stats.rounds += 1;
        let facts = Facts::collect(program, &limits);
        let n = inline_round(program, &facts);
        stats.inlined += n;
        // Inlining a constructor into a projection is what makes most of the
        // folding below possible, so it runs after rather than before.
        for f in program.funcs.iter_mut() {
            if let Some(body) = f.body_mut() {
                stats.folded += fold_expr(body);
            }
        }
        if n == 0 {
            break;
        }
    }
    stats
}

// ---------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------

/// Whether an expression can be dropped without changing what a program does.
///
/// Deliberately narrow: no call of any kind answers `true`, so this needs no
/// purity analysis over the call graph and cannot be wrong about one. It is
/// enough for the rewrites below, which only ever discard a field expression
/// that a projection or an update replaces.
fn discardable(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(..)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Local(_)
        | ExprKind::FnRef(..)
        | ExprKind::Lambda { .. } => true,
        ExprKind::StructLit { fields: xs, .. }
        | ExprKind::EnumLit { args: xs, .. }
        | ExprKind::Tuple(xs)
        | ExprKind::Array(xs) => xs.iter().all(discardable),
        ExprKind::Field { base, .. } | ExprKind::TupleIndex { base, .. } => discardable(base),
        _ => false,
    }
}

/// Rewrites that only pay off once a body has been pasted into its caller.
///
/// Reading a field straight out of the record being built, and updating a
/// record that was built on the spot, both look pointless in source and both
/// are what inlining a one-line accessor or constructor produces.
fn fold_expr(e: &mut Expr) -> usize {
    let mut n = 0;
    for child in children_mut(e) {
        n += fold_expr(child);
    }

    let replacement = match &mut e.kind {
        // `S { a: x, b: y }.a` is `x`, as long as `y` had nothing to do.
        ExprKind::Field { base, index } | ExprKind::TupleIndex { base, index } => {
            let index = *index;
            let fields = match &mut base.kind {
                ExprKind::StructLit { fields, .. } | ExprKind::Tuple(fields) => fields,
                _ => return n,
            };
            if index >= fields.len()
                || !fields.iter().enumerate().all(|(i, f)| i == index || discardable(f))
            {
                return n;
            }
            Some(fields.swap_remove(index))
        }
        // `S { ..S { a: x, b: y }, b: z }` is `S { a: x, b: z }`, provided the
        // `y` it replaces had nothing to do.
        ExprKind::StructUpdate { base, updates, .. } => {
            let ExprKind::StructLit { con, targs, fields } = &mut base.kind else { return n };
            if !updates.iter().all(|(i, _)| fields.get(*i).is_some_and(discardable)) {
                return n;
            }
            let (con, targs) = (*con, targs.clone());
            let mut fields = std::mem::take(fields);
            // Every index was just checked against this vector, which nothing
            // has resized since.
            for (i, v) in std::mem::take(updates) {
                if let Some(slot) = fields.get_mut(i) {
                    *slot = v;
                }
            }
            Some(Expr::new(
                ExprKind::StructLit { con, targs, fields },
                e.ty.clone(),
                e.span,
            ))
        }
        ExprKind::If { cond, then, else_ } => match cond.kind {
            ExprKind::Bool(true) => Some((**then).clone()),
            ExprKind::Bool(false) => Some((**else_).clone()),
            _ => return n,
        },
        // A block that binds nothing is its own tail.
        ExprKind::Block { stmts, tail } if stmts.is_empty() => match tail.take() {
            Some(t) => Some(*t),
            None => return n,
        },
        _ => return n,
    };

    if let Some(r) = replacement {
        *e = r;
        n += 1;
    }
    n
}

// ---------------------------------------------------------------------------
// Facts
// ---------------------------------------------------------------------------

/// What the inliner knows about one function.
#[derive(Clone)]
struct FuncFacts {
    /// Direct calls to it, across the whole program.
    calls: usize,
    /// Occurrences of it *as a value*. One of these keeps the declaration
    /// alive no matter what happens to the direct calls.
    refs: usize,
    size: usize,
    /// The most this function may grow by accepting inlined bodies.
    limit: usize,
    /// In a call-graph cycle, including a self-call.
    recursive: bool,
    /// Contains a `?` anywhere. See `may_inline`.
    has_try: bool,
}

/// One row per function. These were six `Vec`s that had to stay the same
/// length and index-aligned — five here and a `limits` vector built separately
/// in `run` — so an index was bounds-checked against one and then used on
/// another.
pub struct Facts {
    per_func: Vec<FuncFacts>,
}

impl Facts {
    pub fn collect(program: &Program, limits: &[usize]) -> Facts {
        let n = program.funcs.len();
        let mut f = Facts {
            per_func: program
                .funcs
                .iter()
                .enumerate()
                .map(|(i, func)| FuncFacts {
                    calls: 0,
                    refs: 0,
                    size: body_size(func),
                    limit: limits.get(i).copied().unwrap_or(0),
                    recursive: false,
                    has_try: false,
                })
                .collect(),
        };

        // The whole call graph, not the tail-call subset `tail_calls` builds.
        // A callee outside the table is dropped rather than recorded: the row
        // it would need is the bounds check.
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
        for ((i, func), es) in program.funcs.iter().enumerate().zip(edges.iter_mut()) {
            let Some(body) = func.body() else { continue };
            typed::walk(body, &mut |e| match &e.kind {
                ExprKind::CallFn { func, .. } => {
                    let Some(j) = func.func().map(|c| c.index()) else { return };
                    let Some(row) = f.per_func.get_mut(j) else { return };
                    row.calls += 1;
                    es.push(j);
                }
                ExprKind::FnRef(func) => {
                    let Some(j) = func.func().map(|c| c.index()) else { return };
                    let Some(row) = f.per_func.get_mut(j) else { return };
                    row.refs += 1;
                    es.push(j);
                }
                ExprKind::Try { .. } => {
                    if let Some(row) = f.per_func.get_mut(i) {
                        row.has_try = true;
                    }
                }
                _ => {}
            });
        }
        for (i, (row, es)) in f.per_func.iter_mut().zip(edges.iter()).enumerate() {
            if es.contains(&i) {
                row.recursive = true;
            }
        }
        for group in strongly_connected(&edges) {
            if group.len() > 1 {
                for i in group {
                    if let Some(row) = f.per_func.get_mut(i) {
                        row.recursive = true;
                    }
                }
            }
        }
        f
    }

    /// Whether a call to `callee`, made from `caller`, may be replaced by its
    /// body.
    ///
    /// The `has_try` condition is the one that is not a heuristic. `?`
    /// compiles to a `return` in the enclosing JavaScript function
    /// (`generate::expr`), so a body carrying one, pasted into a caller, would
    /// return from *that* caller — skipping everything it meant to do with the
    /// result, at a type the caller does not even return. Nothing catches that
    /// afterwards.
    fn may_inline(&self, caller: usize, callee: usize, size_now: usize, limit: usize) -> bool {
        let Some(c) = self.per_func.get(callee) else { return false };
        if caller == callee || c.recursive || c.has_try {
            return false;
        }
        if size_now > limit {
            return false;
        }
        c.size <= TRIVIAL || (c.calls == 1 && c.refs == 0 && c.size <= SINGLE_USE)
    }

    /// How far one function may grow by accepting inlined bodies.
    fn limit(&self, caller: usize) -> usize {
        self.per_func.get(caller).map_or(0, |f| f.limit)
    }

    /// The node count of one body, as measured at the start of this round.
    fn size(&self, f: usize) -> usize {
        self.per_func.get(f).map_or(0, |x| x.size)
    }
}

fn body_size(f: &Func) -> usize {
    let Some(body) = f.body() else { return 0 };
    let mut n = 0;
    typed::walk(body, &mut |_| n += 1);
    n
}

// ---------------------------------------------------------------------------
// Inlining
// ---------------------------------------------------------------------------

fn inline_round(program: &mut Program, facts: &Facts) -> usize {
    let mut done = 0;
    for i in 0..program.funcs.len() {
        let Some(func) = program.funcs.get_mut(i) else { continue };
        let Some(mut body) = func.take_body() else { continue };
        let mut locals = std::mem::take(&mut func.locals);
        let mut size = facts.size(i);

        let limit = facts.limit(i);
        inline_expr(&mut body, i, program, facts, limit, &mut locals, &mut size, &mut done);

        if let Some(func) = program.funcs.get_mut(i) {
            func.set_body(body);
            func.locals = locals;
        }
    }
    done
}

#[allow(clippy::too_many_arguments)]
fn inline_expr(
    e: &mut Expr,
    caller: usize,
    program: &Program,
    facts: &Facts,
    limit: usize,
    locals: &mut Vec<typed::Local>,
    size: &mut usize,
    done: &mut usize,
) {
    // Children first, so a call that only becomes visible after its own
    // arguments are rewritten is still seen this round.
    for child in children_mut(e) {
        inline_expr(child, caller, program, facts, limit, locals, size, done);
    }

    let ExprKind::CallFn { func, args } = &mut e.kind else { return };
    let Some(callee) = func.func().map(|i| i.index()) else { return };
    let Some(target) = program.funcs.get(callee) else { return };
    let Some(callee_body) = target.body() else { return };
    if args.len() != target.params.len() || !facts.may_inline(caller, callee, *size, limit) {
        return;
    }

    // Every local of the callee is appended to the caller's table and every
    // reference to one shifted by where they landed. `LocalId` is an index
    // into that table (`typed::Body`), so nothing else can capture: no two
    // bindings can end up with the same id.
    let offset = locals.len() as u32;
    locals.extend(target.locals.iter().cloned());
    let mut body = callee_body.clone();
    shift_expr(&mut body, offset);

    // Each argument is bound in turn, before the body runs. That is exactly
    // what a call does, so evaluation order (SPEC 8.2) is preserved without
    // any reasoning about the arguments themselves — and an argument used
    // twice, or not at all, needs no special case. The bindings that turn out
    // to be unnecessary are removed later, by the local cleanup in `javascript.rs`.
    let args = std::mem::take(args);
    let stmts: Vec<Stmt> = target
        .params
        .iter()
        .zip(args)
        .map(|(p, arg)| {
            let local = LocalId(p.0 + offset);
            let span = arg.span;
            Stmt::Let {
                pattern: typed::Pattern {
                    kind: PatKind::Bind { local, sub: None },
                    ty: arg.ty.clone(),
                    span,
                },
                value: arg,
                span,
            }
        })
        .collect();

    *size += facts.size(callee);
    *done += 1;
    e.kind = ExprKind::Block { stmts, tail: Some(Box::new(body)) };
}

/// Every local id in an expression, moved by `offset`.
///
/// Shared with `tail_calls`, which appends a group member's locals to the
/// merged function's table for exactly the reason the inliner appends a
/// callee's to its caller's.
pub(crate) fn shift_expr(e: &mut Expr, offset: u32) {
    match &mut e.kind {
        ExprKind::Local(l) => l.0 += offset,
        ExprKind::Lambda { params, captures, .. } => {
            params.iter_mut().for_each(|p| p.0 += offset);
            captures.iter_mut().for_each(|c| c.0 += offset);
        }
        _ => {}
    }
    // Patterns bind, so they carry ids too, and they are not sub-expressions.
    match &mut e.kind {
        ExprKind::Block { stmts, .. } => {
            for s in stmts.iter_mut() {
                if let Stmt::Let { pattern, .. } = s {
                    shift_pattern(pattern, offset);
                }
            }
        }
        ExprKind::Match { arms, .. } => {
            for a in arms.iter_mut() {
                shift_pattern(&mut a.pattern, offset);
            }
        }
        _ => {}
    }
    for child in children_mut(e) {
        shift_expr(child, offset);
    }
}

fn shift_pattern(p: &mut typed::Pattern, offset: u32) {
    match &mut p.kind {
        PatKind::Bind { local, sub } => {
            local.0 += offset;
            if let Some(s) = sub {
                shift_pattern(s, offset);
            }
        }
        PatKind::Tuple(ps) => ps.iter_mut().for_each(|p| shift_pattern(p, offset)),
        PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => {
            fields.iter_mut().for_each(|f| shift_pattern(&mut f.pattern, offset))
        }
        PatKind::Array { elems, rest } => {
            elems.iter_mut().for_each(|p| shift_pattern(p, offset));
            if let typed::ArrayRest::Bound(l) = rest {
                l.0 += offset;
            }
        }
        PatKind::Or(alts) => alts.iter_mut().for_each(|p| shift_pattern(p, offset)),
        _ => {}
    }
}

/// Every sub-expression, mutably. The mirror of `typed::walk`, which cannot be
/// used here because these passes rewrite what they visit.
///
/// Shared with the rest of the middle end — `tail_calls`, `decision` and
/// `closures` all rewrite what they visit too, and a second copy of this match
/// is a second chance to forget a form and silently skip the tree under it.
pub(crate) fn children_mut(e: &mut Expr) -> Vec<&mut Expr> {
    let mut out: Vec<&mut Expr> = Vec::new();
    match &mut e.kind {
        ExprKind::CallValue { callee, args } => {
            out.push(callee);
            out.extend(args.iter_mut());
        }
        ExprKind::CallFn { args, .. }
        | ExprKind::CallTrait { args, .. }
        | ExprKind::StructLit { fields: args, .. }
        | ExprKind::EnumLit { args, .. }
        | ExprKind::Tuple(args)
        | ExprKind::Array(args)
        | ExprKind::Prim { args, .. }
        | ExprKind::StructuralEq { args, .. }
        | ExprKind::StructuralCmp { args, .. }
        | ExprKind::Intrinsic { args, .. }
        | ExprKind::Continue { args, .. }
        | ExprKind::Closure { env: args, .. }
        | ExprKind::Loop { entries: args } => out.extend(args.iter_mut()),
        ExprKind::StructUpdate { base, updates, .. } => {
            out.push(base);
            out.extend(updates.iter_mut().map(|(_, e)| e));
        }
        ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. }
        | ExprKind::CtxGet { base, .. }
        | ExprKind::Try { base, .. } => out.push(base),
        ExprKind::Index { base, index, .. } => {
            out.push(base);
            out.push(index);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts.iter_mut() {
                match s {
                    Stmt::Let { value, .. } => out.push(value),
                    Stmt::Expr(e) => out.push(e),
                }
            }
            if let Some(t) = tail {
                out.push(t);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            out.push(cond);
            out.push(then);
            out.push(else_);
        }
        ExprKind::Match { scrutinee, arms } => {
            out.push(scrutinee);
            for a in arms.iter_mut() {
                if let Some(g) = &mut a.guard {
                    out.push(g);
                }
                out.push(&mut a.body);
            }
        }
        ExprKind::Lambda { body, .. } => out.push(body),
        ExprKind::And { lhs, rhs }
        | ExprKind::Or { lhs, rhs }
        | ExprKind::Coalesce { lhs, rhs, .. } => {
            out.push(lhs);
            out.push(rhs);
        }
        ExprKind::Template { parts } => out.extend(parts.iter_mut().filter_map(|p| match p {
            typed::TemplatePart::Text(_) => None,
            typed::TemplatePart::Hole(h) => Some(h),
        })),
        ExprKind::CtxLit { bindings } => out.extend(bindings.iter_mut().map(|(_, e)| e)),
        _ => {}
    }
    out
}

/// Every local id an expression mentions, for the tests below.
#[cfg(test)]
fn mentioned(e: &Expr, out: &mut std::collections::HashSet<u32>) {
    typed::walk(e, &mut |e| {
        if let ExprKind::Local(l) = &e.kind {
            out.insert(l.0);
        }
        if let ExprKind::Block { stmts, .. } = &e.kind {
            for s in stmts {
                if let Stmt::Let { pattern, .. } = s {
                    let mut b = Vec::new();
                    pattern.binds(&mut b);
                    out.extend(b.iter().map(|l| l.0));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{FileId, Span};
    use crate::compiler::semantics::types::Ty;
    use std::collections::HashSet;

    fn span() -> Span {
        Span { file: FileId(0), start: 0, end: 0 }
    }

    fn e(kind: ExprKind) -> Expr {
        Expr::new(kind, Ty::Error, span())
    }

    fn local(i: u32) -> Expr {
        e(ExprKind::Local(LocalId(i)))
    }

    fn func(symbol: &str, params: Vec<u32>, nlocals: usize, body: Option<Expr>) -> Func {
        Func {
            symbol: symbol.to_string(),
            debug_name: symbol.to_string(),
            params: params.iter().map(|i| LocalId(*i)).collect(),
            locals: (0..nlocals)
                .map(|i| typed::Local {
                    name: format!("l{i}"),
                    ty: Ty::Error,
                    span: span(),
                })
                .collect(),
            kind: match body {
                Some(e) => crate::compiler::middle::monomorphize::FuncKind::Body(e),
                None => crate::compiler::middle::monomorphize::FuncKind::Unbuilt,
            },
            ret: Ty::Error,
            desc: None,
            span: span(),
        }
    }

    fn program(funcs: Vec<Func>) -> Program {
        Program {
            funcs,
            roots: crate::compiler::middle::monomorphize::ProgramRoots::Main(crate::compiler::semantics::types::FuncIdx(0)),
            descriptors: Vec::new(),
            desc_index: Default::default(),
            ctx_layouts: Default::default(),
        }
    }

    fn call(i: usize, args: Vec<Expr>) -> Expr {
        e(ExprKind::CallFn { func: typed::Callee::Func(crate::compiler::semantics::types::FuncIdx(i as u32)), args })
    }

    /// `double(x) = x + x`, called once. The call becomes a block that binds
    /// the argument and then runs the body.
    #[test]
    fn a_small_body_replaces_its_call() {
        let double = func(
            "double",
            vec![0],
            1,
            Some(e(ExprKind::Prim {
                op: typed::PrimOp::Add,
                prim: crate::compiler::semantics::types::Prim::I64,
                args: vec![local(0), local(0)],
            })),
        );
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, double]);

        let stats = run(&mut p, &Options::default());
        assert!(stats.inlined >= 1, "nothing was inlined");
        let body = p.funcs[0].body().unwrap();
        assert!(
            matches!(body.kind, ExprKind::Block { .. }),
            "the call did not become a block: {:?}",
            body.kind
        );
    }

    /// Every local the callee had must land on a fresh id in the caller, or
    /// two bindings would share one slot.
    #[test]
    fn inlining_moves_every_local_to_a_fresh_slot() {
        // `id(a) = a`, with one local of its own.
        let callee = func("id", vec![0], 2, Some(local(0)));
        // The caller already has three locals, so the callee's must not be
        // 0 and 1 any more.
        let main = func("main", vec![0], 3, Some(call(1, vec![local(2)])));
        let mut p = program(vec![main, callee]);

        run(&mut p, &Options::default());
        assert_eq!(p.funcs[0].locals.len(), 5, "the callee's locals were not appended");

        let mut ids = HashSet::new();
        mentioned(p.funcs[0].body().unwrap(), &mut ids);
        assert!(
            ids.iter().all(|i| (*i as usize) < p.funcs[0].locals.len()),
            "an id escaped the caller's table: {ids:?}"
        );
        // The callee's parameter is now slot 3, not slot 0.
        assert!(ids.contains(&3), "the callee's parameter was not shifted: {ids:?}");
    }

    /// `?` compiles to a `return` in the enclosing function, so a body holding
    /// one would return from its caller if pasted there. This is a soundness
    /// condition, not a heuristic.
    #[test]
    fn a_body_containing_a_question_mark_is_never_inlined() {
        let risky = func(
            "risky",
            vec![0],
            1,
            Some(e(ExprKind::Try {
                base: Box::new(local(0)),
                kind: typed::OptionOrResult::Result,
            })),
        );
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, risky]);

        let stats = run(&mut p, &Options::default());
        assert_eq!(stats.inlined, 0, "a body with `?` was inlined");
    }

    #[test]
    fn a_self_recursive_function_is_never_inlined() {
        let loopy = func("loopy", vec![0], 1, Some(call(1, vec![local(0)])));
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, loopy]);

        let stats = run(&mut p, &Options::default());
        assert_eq!(stats.inlined, 0, "a self-recursive function was inlined");
    }

    #[test]
    fn a_mutually_recursive_pair_is_never_inlined() {
        let a = func("a", vec![0], 1, Some(call(2, vec![local(0)])));
        let b = func("b", vec![0], 1, Some(call(1, vec![local(0)])));
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, a, b]);

        let stats = run(&mut p, &Options::default());
        assert_eq!(stats.inlined, 0, "a member of a cycle was inlined");
    }

    /// `Program::entry` and `TestEntry::func` are slot indices, so a pass that
    /// compacted `funcs` would silently retarget them.
    #[test]
    fn the_function_table_keeps_its_shape() {
        let unused = func("unused", vec![0], 1, Some(local(0)));
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, unused]);
        let before: Vec<String> = p.funcs.iter().map(|f| f.symbol.clone()).collect();

        run(&mut p, &Options::default());
        let after: Vec<String> = p.funcs.iter().map(|f| f.symbol.clone()).collect();
        assert_eq!(before, after);
        assert!(matches!(
            p.roots,
            crate::compiler::middle::monomorphize::ProgramRoots::Main(
                crate::compiler::semantics::types::FuncIdx(0)
            )
        ));
    }

    fn tuple(xs: Vec<Expr>) -> Expr {
        e(ExprKind::Tuple(xs))
    }

    fn int(v: u128) -> Expr {
        e(ExprKind::Int(v, false))
    }

    /// What inlining a one-line accessor leaves behind.
    #[test]
    fn a_field_read_out_of_the_value_being_built_is_that_field() {
        let mut x = e(ExprKind::TupleIndex {
            base: Box::new(tuple(vec![int(1), int(2)])),
            index: 1,
        });
        assert_eq!(fold_expr(&mut x), 1);
        assert!(matches!(x.kind, ExprKind::Int(2, false)), "{:?}", x.kind);
    }

    /// The field being stepped over has to have nothing to do: a call there is
    /// work the program asked for.
    #[test]
    fn a_field_read_past_a_call_is_left_alone() {
        let mut x = e(ExprKind::TupleIndex {
            base: Box::new(tuple(vec![call(1, vec![]), int(2)])),
            index: 1,
        });
        assert_eq!(fold_expr(&mut x), 0);
        assert!(matches!(x.kind, ExprKind::TupleIndex { .. }));
    }

    #[test]
    fn a_constant_condition_keeps_only_its_branch() {
        let mut x = e(ExprKind::If {
            cond: Box::new(e(ExprKind::Bool(false))),
            then: Box::new(int(1)),
            else_: Box::new(int(2)),
        });
        assert_eq!(fold_expr(&mut x), 1);
        assert!(matches!(x.kind, ExprKind::Int(2, false)));
    }

    #[test]
    fn a_block_that_binds_nothing_is_its_tail() {
        let mut x =
            e(ExprKind::Block { stmts: Vec::new(), tail: Some(Box::new(int(7))) });
        assert_eq!(fold_expr(&mut x), 1);
        assert!(matches!(x.kind, ExprKind::Int(7, false)));
    }
}
