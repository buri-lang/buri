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
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::{FuncIdx, LocalId};
use crate::hash::Map;

pub fn run(program: &mut Program) {
    // Every closure and reference can be called directly: `closures` names
    // each lifted lambda exactly once, where it was written, and its prologue
    // binds every capture in order. So a lifted lambda whose one reference
    // became a direct call is reached by nothing else, and its parameters are
    // this pass's to change.
    let mut unpack: Vec<FuncIdx> = Vec::new();
    for f in program.funcs.iter_mut() {
        let Some(body) = f.body_mut() else {
            continue;
        };
        let Some(known) = known_names(body) else {
            continue;
        };
        rewrite(body, &known, &mut unpack);
    }
    unpack.retain(|g| program.funcs.get(*g).env);
    unpack.sort_unstable();
    unpack.dedup();
    for g in unpack {
        let f = program.funcs.get_mut(g);
        let env = f.params.remove(0);
        let captured = f
            .body_mut()
            .map(|b| take_prologue(b, env))
            .unwrap_or_default();
        f.params.splice(0..0, captured);
        f.env = false;
    }
}

/// Takes the leading statements of a lifted lambda's body that bind a capture
/// out of `env`, and returns the captures they bound.
fn take_prologue(body: &mut Expr, env: LocalId) -> Vec<LocalId> {
    let ExprKind::Block { stmts, .. } = &mut body.kind else {
        return Vec::new();
    };
    let n = stmts
        .iter()
        .take_while(|s| match s {
            Stmt::Let { value, .. } => {
                bound(s).is_some()
                    && matches!(&value.kind, ExprKind::TupleIndex { base, .. }
                        if matches!(base.kind, ExprKind::Local(l) if l == env))
            }
            Stmt::Expr(_) => false,
        })
        .count();
    stmts.drain(..n).filter_map(|s| bound(&s)).collect()
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

/// The function a closure or reference names, with its environment's values.
fn known_value(value: &Expr) -> Option<(FuncIdx, Vec<Expr>)> {
    match &value.kind {
        ExprKind::FuncRef(g) => Some((*g, Vec::new())),
        ExprKind::Closure { func, env } => Some((*func, env.clone())),
        _ => None,
    }
}

/// Every name in `body` bound to a closure or reference and read only as a
/// callee, or `None` when the body calls through no function value at all.
fn known_names(body: &Expr) -> Option<Map<LocalId, (FuncIdx, Vec<Expr>)>> {
    let mut known: Map<LocalId, (FuncIdx, Vec<Expr>)> = Map::default();
    let mut any = false;
    typed::walk(body, &mut |e| match &e.kind {
        ExprKind::Block { stmts, .. } => {
            for s in stmts {
                if let (Some(l), Stmt::Let { value, .. }) = (bound(s), s) {
                    if let Some(k) = known_value(value) {
                        known.insert(l, k);
                    }
                }
            }
        }
        ExprKind::CallValue { .. } => any = true,
        _ => {}
    });
    if !any {
        return None;
    }
    if known.is_empty() {
        return Some(known);
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
    Some(known)
}

/// Makes every call of a known function direct, and drops the `let`s that
/// named one.
fn rewrite(e: &mut Expr, known: &Map<LocalId, (FuncIdx, Vec<Expr>)>, out: &mut Vec<FuncIdx>) {
    typed::children_mut(e, &mut |c| rewrite(c, known, out));
    match &mut e.kind {
        ExprKind::CallValue { callee, args } => {
            let target = match &callee.kind {
                ExprKind::Local(l) => known.get(l).cloned(),
                _ => known_value(callee),
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
