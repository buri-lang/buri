//! A call through a function value whose function is known, made a direct
//! call.
//!
//! `let pick = fn(k) => xs[k]; pick(i) + pick(j)` builds a closure, and every
//! call of it goes through the closure's thunk. The thunk takes every argument
//! owned, because a code pointer's callers cannot know the callee's
//! convention, and releases the ones the lambda only borrows. So each call
//! costs a count and a release per counted argument, and each round costs the
//! environment's block, for a value nothing but calls.
//!
//! A name bound to a closure or a function reference, and read only as the
//! callee of a call, is never a value anybody holds. So each of its calls
//! becomes a `CallFunc` and the `let` goes. A call whose callee is written as
//! the closure or reference itself becomes one too.
//!
//! A lifted lambda takes its environment as a tuple, first. Once every
//! reference to it is a direct call, it takes the captured values as
//! parameters of their own instead: the prologue that read them out of the
//! tuple goes, and `middle::rc` infers each one's convention like any other
//! parameter's. A capture the lambda only reads is then borrowed, and the call
//! builds nothing.
//!
//! **The native branch only**, right after `closures`, which is what makes a
//! lambda a function a call can name. JavaScript calls a lambda directly
//! already.

use crate::compiler::middle::monomorphize::Program;
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::{FuncIdx, LocalId};
use crate::hash::Map;

pub fn run(program: &mut Program) {
    // How many times each function is named as a value. A lifted lambda is
    // named once, where it was written, so a call rewritten there is the only
    // way anything reaches it, and its parameters are this pass's to change.
    // Which bodies call through a function value at all: the rest have
    // nothing to rewrite, and this is the only walk they get.
    let mut named: Dense<FuncIdx, usize> = program.funcs.map(|_| 0);
    let mut calls: Vec<FuncIdx> = Vec::new();
    for i in program.funcs.ids() {
        let Some(body) = program.funcs.get(i).body() else {
            continue;
        };
        let mut any = false;
        typed::walk(body, &mut |e| match e.kind {
            ExprKind::FuncRef(g) | ExprKind::Closure { func: g, .. } => {
                *named.get_mut(g) = named.get(g).saturating_add(1);
            }
            ExprKind::CallValue { .. } => any = true,
            _ => {}
        });
        if any {
            calls.push(i);
        }
    }
    // How many captures each lifted lambda reads out of its environment, in
    // the shape `closures` writes: one `let` per capture, in order, first.
    let prologue: Dense<FuncIdx, Option<usize>> = program.funcs.map(|f| {
        let env = *f.params.first()?;
        let body = f.body()?;
        f.env.then(|| prologue_len(body, env))
    });
    let lifted: Dense<FuncIdx, bool> = program.funcs.map(|f| f.env);
    // Whether the function at a closure or reference can be called directly
    // with the environment's values in front of the arguments.
    let direct = |g: FuncIdx, captures: usize| match *prologue.get(g) {
        None => !*lifted.get(g) && captures == 0,
        Some(n) => *named.get(g) == 1 && n == captures,
    };

    let mut unpack: Vec<FuncIdx> = Vec::new();
    for i in calls {
        let Some(known) = program.funcs.get(i).body().map(|b| known_names(b, &direct)) else {
            continue;
        };
        let Some(body) = program.funcs.get_mut(i).body_mut() else {
            continue;
        };
        let mut rewritten = Vec::new();
        rewrite(body, &known, &direct, &mut rewritten);
        unpack.extend(rewritten.into_iter().filter(|g| prologue.get(*g).is_some()));
    }
    unpack.sort_unstable();
    unpack.dedup();
    for g in unpack {
        let n = prologue.get(g).unwrap_or(0);
        let f = program.funcs.get_mut(g);
        let Some(mut body) = f.take_body() else {
            continue;
        };
        let mut params: Vec<LocalId> = Vec::with_capacity(f.params.len().saturating_add(n));
        // `prologue_len` counted `n` statements, so a nonzero `n` is a block's.
        if let (true, ExprKind::Block { stmts, tail }) = (n > 0, &mut body.kind) {
            params.extend(stmts.drain(..n).filter_map(|s| bound(&s)));
            if stmts.is_empty() {
                if let Some(t) = tail.take() {
                    body = *t;
                }
            }
        }
        params.extend(f.params.iter().skip(1).copied());
        f.params = params;
        f.env = false;
        f.set_body(body);
    }
}

/// How many of the body's leading statements bind a capture out of `env`.
fn prologue_len(body: &Expr, env: LocalId) -> usize {
    let ExprKind::Block { stmts, .. } = &body.kind else {
        return 0;
    };
    stmts
        .iter()
        .take_while(|s| match s {
            Stmt::Let { value, .. } => {
                bound(s).is_some()
                    && matches!(&value.kind, ExprKind::TupleIndex { base, .. }
                        if matches!(base.kind, ExprKind::Local(l) if l == env))
            }
            Stmt::Expr(_) => false,
        })
        .count()
}

fn bound(s: &Stmt) -> Option<LocalId> {
    match s {
        Stmt::Let {
            pattern:
                typed::Pattern {
                    kind: PatKind::Bind { local, sub: None },
                    ..
                },
            ..
        } => Some(*local),
        _ => None,
    }
}

/// The closure or reference a `let` binds, when it can be called directly.
fn known_value(
    value: &Expr,
    direct: &impl Fn(FuncIdx, usize) -> bool,
) -> Option<(FuncIdx, Vec<Expr>)> {
    match &value.kind {
        ExprKind::FuncRef(g) if direct(*g, 0) => Some((*g, Vec::new())),
        ExprKind::Closure { func, env } if direct(*func, env.len()) => Some((*func, env.clone())),
        _ => None,
    }
}

/// Every name in `body` bound to a closure or reference it can call directly
/// and read only as a callee.
fn known_names(
    body: &Expr,
    direct: &impl Fn(FuncIdx, usize) -> bool,
) -> Map<LocalId, (FuncIdx, Vec<Expr>)> {
    let mut known: Map<LocalId, (FuncIdx, Vec<Expr>)> = Map::default();
    typed::walk(body, &mut |e| {
        if let ExprKind::Block { stmts, .. } = &e.kind {
            for s in stmts {
                if let (Some(l), Stmt::Let { value, .. }) = (bound(s), s) {
                    if let Some(k) = known_value(value, direct) {
                        known.insert(l, k);
                    }
                }
            }
        }
    });
    if known.is_empty() {
        return known;
    }
    // Every read of a name, against its reads as a callee: `walk` reaches a
    // callee as a child too, so a name read only as one has as many of each.
    let mut reads: Map<LocalId, (usize, usize)> = Map::default();
    typed::walk(body, &mut |e| match &e.kind {
        ExprKind::Local(l) if known.contains_key(l) => {
            let r = reads.entry(*l).or_default();
            r.0 = r.0.saturating_add(1);
        }
        ExprKind::CallValue { callee, .. } => {
            if let ExprKind::Local(l) = callee.kind {
                if known.contains_key(&l) {
                    let r = reads.entry(l).or_default();
                    r.1 = r.1.saturating_add(1);
                }
            }
        }
        _ => {}
    });
    known.retain(|l, _| reads.get(l).is_some_and(|(all, calls)| all == calls));
    known
}

/// Makes every call of a known function direct, and drops the `let`s that
/// named one.
fn rewrite(
    e: &mut Expr,
    known: &Map<LocalId, (FuncIdx, Vec<Expr>)>,
    direct: &impl Fn(FuncIdx, usize) -> bool,
    out: &mut Vec<FuncIdx>,
) {
    typed::children_mut(e, &mut |c| rewrite(c, known, direct, out));
    match &mut e.kind {
        ExprKind::CallValue { callee, args } => {
            let target = match &callee.kind {
                ExprKind::Local(l) => known.get(l).cloned(),
                _ => known_value(callee, direct),
            };
            if let Some((func, mut all)) = target {
                all.append(args);
                out.push(func);
                e.kind = ExprKind::CallFunc { func, args: all };
            }
        }
        ExprKind::Block { stmts, .. } if !known.is_empty() => {
            stmts.retain(|s| !bound(s).is_some_and(|l| known.contains_key(&l)));
        }
        _ => {}
    }
}
