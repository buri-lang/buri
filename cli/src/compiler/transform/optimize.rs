//! Optimisation of the monomorphized IR.
//!
//! This runs between monomorphization and the backend, which is the one point
//! where the whole program is present, every type is concrete, and nothing has
//! been committed to JavaScript yet. Because Buri has no dynamic dispatch, the
//! call graph here is exact (see `monomorphize.rs`), so a decision taken from it is a
//! fact rather than an estimate.
//!
//! **Function indices never move.** `Program::entry` and `TestEntry::func` are
//! indices into `Program::funcs`, as are `CallFn`'s `func` and `FnRef`'s first
//! field. Everything here rewrites bodies in place; a function nothing calls
//! any more is left for `javascript::eliminate_dead` to drop by name.

use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::LocalId;
use crate::compiler::transform::monomorphize::{Func, Program};

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
    let limits: Vec<usize> =
        program.funcs.iter().map(|f| body_size(f) * 2 + GROWTH).collect();

    for _ in 0..opts.rounds {
        stats.rounds += 1;
        let facts = Facts::collect(program);
        let n = inline_round(program, &facts, &limits);
        stats.inlined += n;
        // Inlining a constructor into a projection is what makes most of the
        // folding below possible, so it runs after rather than before.
        for f in program.funcs.iter_mut() {
            if let Some(body) = &mut f.body {
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
            if !updates.iter().all(|(i, _)| *i < fields.len() && discardable(&fields[*i])) {
                return n;
            }
            let (con, targs) = (*con, targs.clone());
            let mut fields = std::mem::take(fields);
            for (i, v) in std::mem::take(updates) {
                fields[i] = v;
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

pub struct Facts {
    /// Direct calls to each function, across the whole program.
    calls: Vec<usize>,
    /// Occurrences of each function *as a value*. One of these keeps the
    /// declaration alive no matter what happens to the direct calls.
    refs: Vec<usize>,
    size: Vec<usize>,
    /// In a call-graph cycle, including a self-call.
    recursive: Vec<bool>,
    /// Contains a `?` anywhere. See `may_inline`.
    has_try: Vec<bool>,
}

impl Facts {
    pub fn collect(program: &Program) -> Facts {
        let n = program.funcs.len();
        let mut f = Facts {
            calls: vec![0; n],
            refs: vec![0; n],
            size: program.funcs.iter().map(body_size).collect(),
            recursive: vec![false; n],
            has_try: vec![false; n],
        };

        // The whole call graph, not the tail-call subset `tco` builds.
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, func) in program.funcs.iter().enumerate() {
            let Some(body) = &func.body else { continue };
            typed::walk(body, &mut |e| match &e.kind {
                ExprKind::CallFn { func, .. } => {
                    let j = func.index();
                    if j < n {
                        f.calls[j] += 1;
                        edges[i].push(j);
                    }
                }
                ExprKind::FnRef(func, _) => {
                    let j = func.index();
                    if j < n {
                        f.refs[j] += 1;
                        edges[i].push(j);
                    }
                }
                ExprKind::Try { .. } => f.has_try[i] = true,
                _ => {}
            });
        }
        for (i, es) in edges.iter().enumerate() {
            if es.contains(&i) {
                f.recursive[i] = true;
            }
        }
        for group in strongly_connected(&edges) {
            if group.len() > 1 {
                for i in group {
                    f.recursive[i] = true;
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
        if caller == callee || self.recursive[callee] || self.has_try[callee] {
            return false;
        }
        if size_now > limit {
            return false;
        }
        let size = self.size[callee];
        size <= TRIVIAL
            || (self.calls[callee] == 1 && self.refs[callee] == 0 && size <= SINGLE_USE)
    }
}

fn body_size(f: &Func) -> usize {
    let Some(body) = &f.body else { return 0 };
    let mut n = 0;
    typed::walk(body, &mut |_| n += 1);
    n
}

/// Tarjan's algorithm, iteratively. Each group is sorted, and the groups come
/// out in a deterministic order, because build output is compared byte for
/// byte (`builds_are_reproducible`).
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let (mut index, mut low, mut on) = (vec![usize::MAX; n], vec![0usize; n], vec![false; n]);
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut out: Vec<Vec<usize>> = Vec::new();

    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        // (node, how many of its edges have been taken)
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some((v, ei)) = work.pop() {
            if ei == 0 {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on[v] = true;
            }
            let mut descended = false;
            for (k, &w) in edges[v].iter().enumerate().skip(ei) {
                if index[w] == usize::MAX {
                    work.push((v, k + 1));
                    work.push((w, 0));
                    descended = true;
                    break;
                } else if on[w] {
                    low[v] = low[v].min(index[w]);
                }
            }
            if descended {
                continue;
            }
            if low[v] == index[v] {
                let mut group = Vec::new();
                while let Some(w) = stack.pop() {
                    on[w] = false;
                    group.push(w);
                    if w == v {
                        break;
                    }
                }
                group.sort_unstable();
                out.push(group);
            }
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Inlining
// ---------------------------------------------------------------------------

fn inline_round(program: &mut Program, facts: &Facts, limits: &[usize]) -> usize {
    let mut done = 0;
    for i in 0..program.funcs.len() {
        let Some(mut body) = program.funcs[i].body.take() else { continue };
        let mut locals = std::mem::take(&mut program.funcs[i].locals);
        let mut size = facts.size[i];

        inline_expr(&mut body, i, program, facts, limits[i], &mut locals, &mut size, &mut done);

        program.funcs[i].body = Some(body);
        program.funcs[i].locals = locals;
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

    let ExprKind::CallFn { func, args, .. } = &mut e.kind else { return };
    let callee = func.index();
    if callee >= program.funcs.len() {
        return;
    }
    let target = &program.funcs[callee];
    let Some(callee_body) = &target.body else { return };
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

    *size += facts.size[callee];
    *done += 1;
    e.kind = ExprKind::Block { stmts, tail: Some(Box::new(body)) };
}

/// Every local id in an expression, moved by `offset`.
fn shift_expr(e: &mut Expr, offset: u32) {
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
            if let Some(Some(l)) = rest {
                l.0 += offset;
            }
        }
        PatKind::Or(alts) => alts.iter_mut().for_each(|p| shift_pattern(p, offset)),
        _ => {}
    }
}

/// Every sub-expression, mutably. The mirror of `typed::walk`, which cannot be
/// used here because these passes rewrite what they visit.
fn children_mut(e: &mut Expr) -> Vec<&mut Expr> {
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
        | ExprKind::Intrinsic { args, .. } => out.extend(args.iter_mut()),
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
        ExprKind::Template { parts } => {
            out.extend(parts.iter_mut().filter_map(|p| p.hole.as_mut()))
        }
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
    use crate::compiler::semantics::types::{FnId, Ty};
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
            body,
            intrinsic: None,
            param_types: Vec::new(),
            ret: Ty::Error,
            desc: None,
            span: span(),
        }
    }

    fn program(funcs: Vec<Func>) -> Program {
        Program {
            funcs,
            entry: Some(0),
            tests: Vec::new(),
            descriptors: Vec::new(),
            desc_index: Default::default(),
            ctx_layouts: Default::default(),
        }
    }

    fn call(i: usize, args: Vec<Expr>) -> Expr {
        e(ExprKind::CallFn { func: FnId(i as u32), targs: Vec::new(), args })
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
                prim: None,
                args: vec![local(0), local(0)],
            })),
        );
        let main = func("main", vec![0], 1, Some(call(1, vec![local(0)])));
        let mut p = program(vec![main, double]);

        let stats = run(&mut p, &Options::default());
        assert!(stats.inlined >= 1, "nothing was inlined");
        let body = p.funcs[0].body.as_ref().unwrap();
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
        mentioned(p.funcs[0].body.as_ref().unwrap(), &mut ids);
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
        assert_eq!(p.entry, Some(0));
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

    #[test]
    fn cycles_are_found_and_singletons_are_not_called_recursive() {
        // 0 -> 1 -> 2 -> 1, and 3 alone.
        let edges = vec![vec![1], vec![2], vec![1], vec![]];
        let mut groups = strongly_connected(&edges);
        groups.sort();
        let cycles: Vec<Vec<usize>> =
            groups.into_iter().filter(|g| g.len() > 1).collect();
        assert_eq!(cycles, vec![vec![1, 2]]);
    }
}
