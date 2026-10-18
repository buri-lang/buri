//! Inlining and folding, over the monomorphized tree. The folding is
//! interleaved with the inlining rather than a pass of its own, because
//! inlining a constructor into a projection is what makes most folding
//! possible and folding is what exposes the next round's call sites.
//!
//! This runs between monomorphization and the backend, which is the one point
//! where the whole program is present, every type is concrete, and nothing has
//! been committed to JavaScript yet. Because Buri has no dynamic dispatch, the
//! call graph here is exact (see `monomorphize.rs`), so a decision taken from it is a
//! fact rather than an estimate.
//!
//! **Function indices never move.** `Program::entry` and `TestEntry::func` are
//! `FuncIdx`, as are the callees of `CallFunc` and `FuncRef`. Everything
//! here rewrites bodies in place; a function nothing calls any more is left for
//! `javascript::eliminate_dead` to drop by name.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every operand is a count of things already in memory — nodes in a body, functions in the program, slots in one function's local table — and nothing here subtracts, so no sum or product can leave the range of the machine holding them"
)]

use crate::compiler::middle::monomorphize::{Func, Program};
use crate::compiler::middle::strongly_connected;
use crate::compiler::semantics::layered::{Dense, Shift};
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::{FuncIdx, LocalId};

/// How many times the pipeline may run.
///
/// Inlining exposes calls that were not visible before, so one round is not
/// enough; the loop also stops early once a round changes nothing, which is
/// why the ceiling can be a constant rather than something a caller tunes.
const ROUNDS: usize = 3;

/// What a run of the pipeline changed.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub inlined: usize,
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

pub fn run(program: &mut Program) -> Stats {
    let mut stats = Stats::default();
    let mut own: Dense<FuncIdx, Own> = program.funcs.map(Own::of);
    // Measured once, from the original bodies: the ceiling must not move as
    // inlining grows a function, or a chain of small functions compounds.
    let limits: Dense<FuncIdx, usize> = own.map(|o| o.size * 2 + GROWTH);

    // Which functions this round has to look at. The first round looks at
    // every one; see `revisit` for the rest.
    let mut dirty = program.funcs.map(|_| true);
    let mut before: Option<Facts> = None;
    for _ in 0..ROUNDS {
        let facts = Facts::collect(&own, &limits);
        if let Some(before) = &before {
            revisit(&mut dirty, &own, &facts, before);
        }
        let inlined = inline_round(program, &facts, &dirty, &own);
        // Inlining a constructor into a projection is what makes most of the
        // folding below possible, so it runs after rather than before.
        let folded = fold_round(program, &dirty, &inlined, &own);
        let total: usize = inlined.iter().sum();
        stats.inlined += total;
        if total == 0 {
            break;
        }
        for ((d, i), f) in dirty.iter_mut().zip(&inlined).zip(&folded) {
            *d = *i > 0 || *f > 0;
        }
        for ((o, f), d) in own.iter_mut().zip(&program.funcs).zip(&dirty) {
            if *d {
                *o = Own::of(f);
            }
        }
        before = Some(facts);
    }
    stats
}

/// Adds to `dirty` every function a round could now change.
///
/// `dirty` holds the functions whose body the last round changed. Everything
/// else came out of that round with no call inlined and nothing folded, and a
/// round decides from three things: the body, the facts of each callee, and
/// the caller's limit, which never moves. So a function whose body is the same
/// and whose callees' facts are the same would make every decision it made last
/// time, and inline and fold nothing again. Skipping it is the same answer.
fn revisit(
    dirty: &mut Dense<FuncIdx, bool>,
    own: &Dense<FuncIdx, Own>,
    now: &Facts,
    before: &Facts,
) {
    let moved: Dense<FuncIdx, bool> = now
        .per_func
        .iter()
        .zip(&before.per_func)
        .map(|(a, b)| a != b)
        .collect();
    for (d, o) in dirty.iter_mut().zip(own) {
        if !*d {
            *d = o.callees().any(|j| *moved.get(j));
        }
    }
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
        | ExprKind::FuncRef(..)
        | ExprKind::Lambda { .. } => true,
        ExprKind::StructLit { fields: xs, .. }
        | ExprKind::EnumLit { args: xs, .. }
        | ExprKind::Tuple(xs)
        | ExprKind::Array(xs) => xs.iter().all(discardable),
        ExprKind::Field { base, .. } | ExprKind::TupleIndex { base, .. } => discardable(base),
        _ => false,
    }
}

/// Whether [`fold_expr`] might rewrite this node. Every rewrite it makes is at
/// a node this answers `true` for, or above one it just rewrote.
fn foldable(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Field { base, .. } | ExprKind::TupleIndex { base, .. } => {
            matches!(base.kind, ExprKind::StructLit { .. } | ExprKind::Tuple(_))
        }
        ExprKind::StructUpdate { base, .. } => matches!(base.kind, ExprKind::StructLit { .. }),
        ExprKind::If { cond, .. } => matches!(cond.kind, ExprKind::Bool(_)),
        ExprKind::Block { stmts, .. } => stmts.is_empty(),
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
    typed::children_mut(e, &mut |child| n += fold_expr(child));

    let replacement = match &mut e.kind {
        // `S { a: x, b: y }.a` is `x`, as long as `y` had nothing to do.
        ExprKind::Field { base, index } | ExprKind::TupleIndex { base, index } => {
            let index = *index;
            let fields = match &mut base.kind {
                ExprKind::StructLit { fields, .. } | ExprKind::Tuple(fields) => fields,
                _ => return n,
            };
            // The checker keeps `index` inside the record or tuple it reads.
            if !fields
                .iter()
                .enumerate()
                .all(|(i, f)| i == index || discardable(f))
            {
                return n;
            }
            fields.swap_remove(index)
        }
        // `S { ..S { a: x, b: y }, b: z }` is `S { a: x, b: z }`, provided the
        // `y` it replaces had nothing to do.
        ExprKind::StructUpdate { base, updates, .. } => {
            let ExprKind::StructLit { con, targs, fields } = &mut base.kind else {
                return n;
            };
            if !updates
                .iter()
                .all(|(i, _)| fields.get(*i).is_some_and(discardable))
            {
                return n;
            }
            let (con, targs) = (*con, targs.clone());
            let mut fields = std::mem::take(fields);
            // Every index was just checked against this vector, which nothing
            // has resized since.
            for (i, v) in std::mem::take(updates) {
                #[expect(
                    clippy::indexing_slicing,
                    reason = "checked against `fields` just above"
                )]
                let slot = &mut fields[i];
                *slot = v;
            }
            Expr::new(ExprKind::StructLit { con, targs, fields }, e.ty, e.span)
        }
        ExprKind::If { cond, then, else_ } => match cond.kind {
            ExprKind::Bool(true) => (**then).clone(),
            ExprKind::Bool(false) => (**else_).clone(),
            _ => return n,
        },
        // A block that binds nothing is its own tail.
        ExprKind::Block { stmts, tail } if stmts.is_empty() => match tail.take() {
            Some(t) => *t,
            None => return n,
        },
        _ => return n,
    };
    *e = replacement;
    n + 1
}

// ---------------------------------------------------------------------------
// Facts
// ---------------------------------------------------------------------------

/// What the inliner knows about one function.
#[derive(Clone, PartialEq, Eq)]
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
struct Facts {
    per_func: Dense<FuncIdx, FuncFacts>,
}

/// What one function's own body says, from one walk of it.
///
/// Every row of [`Facts`] is a sum over these, so a round measures again only
/// the bodies the round before it changed, rather than every body there is.
struct Own {
    size: usize,
    has_try: bool,
    /// Every function this body names, in walk order, and whether by a call
    /// (`true`) or as a value.
    edges: Vec<(FuncIdx, bool)>,
    /// Holds a node [`fold_expr`] could rewrite. Without one, and with nothing
    /// pasted in since, a fold would walk the body and change nothing.
    folds: bool,
}

impl Own {
    fn of(func: &Func) -> Own {
        let mut own = Own {
            size: 0,
            has_try: false,
            edges: Vec::new(),
            folds: false,
        };
        let Some(body) = func.body() else { return own };
        typed::walk(body, &mut |e| {
            own.size += 1;
            match &e.kind {
                ExprKind::CallFunc { func, .. } => own.edges.push((*func, true)),
                ExprKind::FuncRef(func) => own.edges.push((*func, false)),
                ExprKind::Try { .. } => own.has_try = true,
                _ => {}
            }
            own.folds |= foldable(e);
        });
        own
    }

    /// The functions this body calls directly, which are the only ones the
    /// inliner can paste into it.
    fn callees(&self) -> impl Iterator<Item = FuncIdx> + '_ {
        self.edges.iter().filter(|(_, call)| *call).map(|(j, _)| *j)
    }
}

impl Facts {
    fn collect(own: &Dense<FuncIdx, Own>, limits: &Dense<FuncIdx, usize>) -> Facts {
        let mut f = Facts {
            per_func: own
                .iter()
                .zip(limits)
                .map(|(o, limit)| FuncFacts {
                    calls: 0,
                    refs: 0,
                    size: o.size,
                    limit: *limit,
                    recursive: false,
                    has_try: o.has_try,
                })
                .collect(),
        };
        // The whole call graph, not the tail-call subset `tail_calls` builds.
        for (i, o) in own.entries() {
            for (j, call) in &o.edges {
                let row = f.per_func.get_mut(*j);
                if *call {
                    row.calls += 1;
                } else {
                    row.refs += 1;
                }
            }
            if o.edges.iter().any(|(j, _)| *j == i) {
                f.per_func.get_mut(i).recursive = true;
            }
        }
        let edges: Dense<FuncIdx, Vec<FuncIdx>> =
            own.map(|o| o.edges.iter().map(|(j, _)| *j).collect());
        for group in strongly_connected(&edges).iter() {
            if group.len() > 1 {
                for &i in group {
                    f.per_func.get_mut(i).recursive = true;
                }
            }
        }
        f
    }

    /// Whether a call to `callee` may be replaced by its body, in a caller
    /// grown to `size_now`.
    ///
    /// A caller never inlines itself: its body is out of the table while it is
    /// walked, so [`inline_expr`] finds nothing to paste.
    ///
    /// The `has_try` condition is the one that is not a heuristic. `?`
    /// compiles to a `return` in the enclosing JavaScript function
    /// (`generate::expr`), so a body carrying one, pasted into a caller, would
    /// return from *that* caller — skipping everything it meant to do with the
    /// result, at a type the caller does not even return. Nothing catches that
    /// afterwards.
    fn may_inline(&self, callee: FuncIdx, size_now: usize, limit: usize) -> bool {
        size_now <= limit && self.inlinable(callee)
    }

    /// The half of [`Facts::may_inline`] that is about the callee alone, and
    /// so is fixed for a whole round.
    fn inlinable(&self, callee: FuncIdx) -> bool {
        let c = self.per_func.get(callee);
        if c.recursive || c.has_try {
            return false;
        }
        c.size <= TRIVIAL || (c.calls == 1 && c.refs == 0 && c.size <= SINGLE_USE)
    }
}

// ---------------------------------------------------------------------------
// Inlining
// ---------------------------------------------------------------------------

/// One round of inlining over the functions `dirty` names, in index order,
/// answering how many calls each one inlined.
///
/// Serial on purpose. The answer depends on the order — a caller pastes the
/// body its callee has at that moment — and cutting a round into levels that
/// respect it, one pool start per level, measured slower than this on every
/// corpus: thread start and stack teardown cost more than the work they
/// shared. `buri test` already prepares one program per job thread.
fn inline_round(
    program: &mut Program,
    facts: &Facts,
    dirty: &Dense<FuncIdx, bool>,
    own: &Dense<FuncIdx, Own>,
) -> Dense<FuncIdx, usize> {
    let mut done = program.funcs.map(|_| 0);
    for i in program.funcs.ids() {
        let func = program.funcs.get_mut(i);
        // Out of the table while it is walked, so it cannot paste itself.
        let Some(mut body) = func.take_body() else {
            continue;
        };
        // A body that calls nothing inlinable would be walked to inline nothing.
        if !*dirty.get(i) || !own.get(i).callees().any(|j| j != i && facts.inlinable(j)) {
            func.set_body(body);
            continue;
        }
        let mut locals = std::mem::take(&mut func.locals);
        let limit = facts.per_func.get(i).limit;
        let mut size = facts.per_func.get(i).size;
        let mut count = 0;
        inline_expr(
            &mut body,
            program,
            facts,
            limit,
            &mut locals,
            &mut size,
            &mut count,
        );
        let func = program.funcs.get_mut(i);
        func.set_body(body);
        func.locals = locals;
        *done.get_mut(i) = count;
    }
    done
}

/// The folds, over the functions `dirty` names. Answers how many rewrites
/// each made, which is how the next round knows whose body moved.
///
/// A body nothing was pasted into this round is still the body `own`
/// measured, so where that found nothing to fold the walk is skipped.
fn fold_round(
    program: &mut Program,
    dirty: &Dense<FuncIdx, bool>,
    inlined: &Dense<FuncIdx, usize>,
    own: &Dense<FuncIdx, Own>,
) -> Dense<FuncIdx, usize> {
    let mut folded = program.funcs.map(|_| 0);
    for (i, f) in program.funcs.entries_mut() {
        let Some(body) = f.body_mut() else { continue };
        if *dirty.get(i) && (*inlined.get(i) > 0 || own.get(i).folds) {
            *folded.get_mut(i) = fold_expr(body);
        }
    }
    folded
}

#[allow(clippy::too_many_arguments)]
fn inline_expr(
    e: &mut Expr,
    program: &Program,
    facts: &Facts,
    limit: usize,
    locals: &mut typed::Locals,
    size: &mut usize,
    done: &mut usize,
) {
    // Children first, so a call that only becomes visible after its own
    // arguments are rewritten is still seen this round.
    typed::children_mut(e, &mut |child| {
        inline_expr(child, program, facts, limit, locals, size, done);
    });

    let ExprKind::CallFunc { func, args } = &mut e.kind else {
        return;
    };
    let callee = *func;
    let target = program.funcs.get(callee);
    let Some(callee_body) = target.body() else {
        return;
    };
    // The checker gave the call one argument per parameter.
    if !facts.may_inline(callee, *size, limit) {
        return;
    }

    // Every local of the callee is appended to the caller's table and every
    // reference to one shifted by where they landed. `LocalId` is an index
    // into that table (`typed::Body`), so nothing else can capture: no two
    // bindings can end up with the same id.
    let shift = locals.append(&target.locals);
    let mut body = callee_body.clone();
    shift_expr(&mut body, &shift);

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
            let local = shift.of(*p);
            let span = arg.span;
            Stmt::Let {
                pattern: typed::Pattern {
                    kind: PatKind::Bind { local, sub: None },
                    ty: arg.ty,
                    span,
                },
                value: arg,
                span,
            }
        })
        .collect();

    *size += facts.per_func.get(callee).size;
    *done += 1;
    e.kind = ExprKind::Block {
        stmts,
        tail: Some(Box::new(body)),
    };
}

/// Every local id in an expression, moved to where `shift` put its table.
///
/// Shared with `tail_calls`, which appends a group member's locals to the
/// merged function's table for exactly the reason the inliner appends a
/// callee's to its caller's.
pub(crate) fn shift_expr(e: &mut Expr, shift: &Shift<LocalId>) {
    match &mut e.kind {
        ExprKind::Local(l) => *l = shift.of(*l),
        ExprKind::Lambda {
            params, captures, ..
        } => {
            params.iter_mut().for_each(|p| *p = shift.of(*p));
            captures.iter_mut().for_each(|c| *c = shift.of(*c));
        }
        _ => {}
    }
    // Patterns bind, so they carry ids too, and they are not sub-expressions.
    match &mut e.kind {
        ExprKind::Block { stmts, .. } => {
            for s in stmts.iter_mut() {
                if let Stmt::Let { pattern, .. } = s {
                    shift_pattern(pattern, shift);
                }
            }
        }
        ExprKind::Match { arms, .. } => {
            for a in arms.iter_mut() {
                shift_pattern(&mut a.pattern, shift);
            }
        }
        _ => {}
    }
    typed::children_mut(e, &mut |child| shift_expr(child, shift));
}

fn shift_pattern(p: &mut typed::Pattern, shift: &Shift<LocalId>) {
    match &mut p.kind {
        PatKind::Bind { local, sub } => {
            *local = shift.of(*local);
            if let Some(s) = sub {
                shift_pattern(s, shift);
            }
        }
        PatKind::Tuple(ps) => ps.iter_mut().for_each(|p| shift_pattern(p, shift)),
        PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => fields
            .iter_mut()
            .for_each(|f| shift_pattern(&mut f.pattern, shift)),
        PatKind::Array { elems, rest } => {
            elems.iter_mut().for_each(|p| shift_pattern(p, shift));
            if let typed::ArrayRest::Bound(l) = rest {
                *l = shift.of(*l);
            }
        }
        PatKind::Or(alts) => alts.iter_mut().for_each(|p| shift_pattern(p, shift)),
        _ => {}
    }
}

/// Every local id an expression mentions, for the tests below.
#[cfg(test)]
fn mentioned(e: &Expr, out: &mut std::collections::HashSet<u32>) {
    typed::walk(e, &mut |e| {
        if let ExprKind::Local(l) = &e.kind {
            out.insert(l.index() as u32);
        }
        if let ExprKind::Block { stmts, .. } = &e.kind {
            for s in stmts {
                if let Stmt::Let { pattern, .. } = s {
                    let mut b = Vec::new();
                    pattern.binds(&mut b);
                    out.extend(b.iter().map(|l| l.index() as u32));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::middle::func_id;
    use crate::compiler::middle::local_id;
    use crate::compiler::semantics::name::Name;
    use crate::compiler::semantics::types::Ty;
    use crate::diagnostics::{FileId, Span};
    use std::collections::HashSet;

    fn span() -> Span {
        Span {
            file: FileId(0),
            start: 0,
            end: 0,
        }
    }

    fn e(kind: ExprKind) -> Expr {
        Expr::new(kind, Ty::ERROR, span())
    }

    fn local(i: u32) -> Expr {
        e(ExprKind::Local(local_id(i)))
    }

    fn func(symbol: &str, params: Vec<u32>, local_count: usize, body: Option<Expr>) -> Func {
        Func {
            symbol: symbol.to_string(),
            debug_name: symbol.to_string(),
            params: params.iter().map(|i| local_id(*i)).collect(),
            locals: (0..local_count)
                .map(|i| typed::Local {
                    name: Name::new(&format!("l{i}")),
                    ty: Ty::ERROR,
                    span: span(),
                })
                .collect(),
            kind: match body {
                Some(e) => crate::compiler::middle::monomorphize::FuncKind::Body(e),
                None => crate::compiler::middle::monomorphize::FuncKind::Unbuilt,
            },
            ret: Ty::ERROR,
            desc: None,
            span: span(),
        }
    }

    fn program(funcs: Vec<Func>) -> Program {
        Program {
            funcs: funcs.into_iter().collect(),
            roots: crate::compiler::middle::monomorphize::ProgramRoots::Main(func_id(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: Default::default(),
            cell_equal: Default::default(),
            ctx_layouts: Default::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            inline_animations: false,
            icons: false,
            tooltips: false,
            themes: false,
            chunks: Vec::new(),
            hosted: Default::default(),
            instances: Default::default(),
        }
    }

    fn call(i: usize, args: Vec<Expr>) -> Expr {
        e(ExprKind::CallFunc {
            func: func_id(i as u32),
            args,
        })
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
                args: Box::new([local(0), local(0)]),
            })),
        );
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, double]);

        let stats = run(&mut p);
        assert!(stats.inlined >= 1, "nothing was inlined");
        let body = p.funcs.as_slice()[0].body().unwrap();
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

        run(&mut p);
        assert_eq!(
            p.funcs.as_slice()[0].locals.len(),
            5,
            "the callee's locals were not appended"
        );

        let mut ids = HashSet::new();
        mentioned(p.funcs.as_slice()[0].body().unwrap(), &mut ids);
        assert!(
            ids.iter()
                .all(|i| (*i as usize) < p.funcs.as_slice()[0].locals.len()),
            "an id escaped the caller's table: {ids:?}"
        );
        // The callee's parameter is now slot 3, not slot 0.
        assert!(
            ids.contains(&3),
            "the callee's parameter was not shifted: {ids:?}"
        );
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

        let stats = run(&mut p);
        assert_eq!(stats.inlined, 0, "a body with `?` was inlined");
    }

    #[test]
    fn a_self_recursive_function_is_never_inlined() {
        let loopy = func("loopy", vec![0], 1, Some(call(1, vec![local(0)])));
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, loopy]);

        let stats = run(&mut p);
        assert_eq!(stats.inlined, 0, "a self-recursive function was inlined");
    }

    #[test]
    fn a_mutually_recursive_pair_is_never_inlined() {
        let a = func("a", vec![0], 1, Some(call(2, vec![local(0)])));
        let b = func("b", vec![0], 1, Some(call(1, vec![local(0)])));
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, a, b]);

        let stats = run(&mut p);
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

        run(&mut p);
        let after: Vec<String> = p.funcs.iter().map(|f| f.symbol.clone()).collect();
        assert_eq!(before, after);
        assert!(matches!(
            p.roots,
            crate::compiler::middle::monomorphize::ProgramRoots::Main(f) if f == func_id(0)
        ));
    }

    fn tuple(xs: Vec<Expr>) -> Expr {
        e(ExprKind::Tuple(xs))
    }

    fn int(v: u128) -> Expr {
        e(ExprKind::Int(typed::Magnitude::new(v), false))
    }

    /// What inlining a one-line accessor leaves behind.
    #[test]
    fn a_field_read_out_of_the_value_being_built_is_that_field() {
        let mut x = e(ExprKind::TupleIndex {
            base: Box::new(tuple(vec![int(1), int(2)])),
            index: 1,
        });
        assert_eq!(fold_expr(&mut x), 1);
        assert!(
            matches!(x.kind, ExprKind::Int(m, false) if m.get() == 2),
            "{:?}",
            x.kind
        );
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
        assert!(matches!(x.kind, ExprKind::Int(m, false) if m.get() == 2));
    }

    #[test]
    fn a_block_that_binds_nothing_is_its_tail() {
        let mut x = e(ExprKind::Block {
            stmts: Vec::new(),
            tail: Some(Box::new(int(7))),
        });
        assert_eq!(fold_expr(&mut x), 1);
        assert!(matches!(x.kind, ExprKind::Int(m, false) if m.get() == 7));
    }
}
