//! A `let` that names a field path, `let held = acc.items;`, replaced by the
//! path at every read of the name.
//!
//! Values are immutable and a `LocalId` is bound once, so `held` and
//! `acc.items` are the same value wherever `held` is in scope, and reading a
//! field of a struct or a tuple costs nothing: both are words of the local's
//! own value. What changes is what `middle::rc` can see. A functional update
//! over a dying `acc` moves a replaced field it reads exactly once
//! (`rc::moved_fields`), so `Acc { ..acc, items: acc.items.push(ctx, x) }`
//! pushes in place. Through a name, `held` took a count of its own at the
//! `let`, the update released the old field after it, and the push between
//! the two found the list at two and copied it, once per push.
//!
//! **The native branch only**, after `closures`: a lambda is a lifted function
//! by then, so a name is never read from inside another function's body, and
//! JavaScript stays the reference the agreement tests compare against.

use crate::compiler::middle::monomorphize::Program;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::LocalId;
use crate::hash::{Map, Set};

pub fn run(program: &mut Program) {
    for f in &mut program.funcs {
        if let Some(body) = f.body_mut() {
            forward_body(body);
        }
    }
}

/// One walk, outside in: a block's own names are forwarded before the blocks
/// inside it are reached, so the `let items = held;` the inliner writes inside
/// becomes `let items = acc.1;` and is forwarded in turn.
fn forward_body(body: &mut Expr) {
    // A name a functional update is written over stays a name. Over a dying
    // local the update takes the local's count and moves the fields it
    // replaces (`rc::update_dying`); over a path there is no local to take,
    // and `{ ..o.inner, items: o.inner.items.push(..) }` copies the list.
    // (`fields.rs`'s
    // `a_list_in_a_record_nested_in_a_record_grows_in_place_through_an_inlined_step`.)
    //
    // Once for the body: a local is bound once and read only in its scope, so
    // the bases of the whole body are, for every block's own names, the bases
    // under that block. Forwarding never makes or unmakes one: it replaces
    // only names that aren't bases.
    let mut bases: Set<LocalId> = Set::default();
    let mut lets = false;
    typed::walk(body, &mut |x| match &x.kind {
        ExprKind::StructUpdate { base, .. } => {
            if let ExprKind::Local(l) = base.kind {
                bases.insert(l);
            }
        }
        ExprKind::Block { stmts, .. } => lets |= stmts.iter().any(|s| path_let(s).is_some()),
        _ => {}
    });
    // A body binding no path has nothing to forward.
    if !lets {
        return;
    }
    let mut paths: Map<LocalId, Expr> = Map::default();
    forward(body, &bases, &mut paths);
}

/// Replaces every forwarded name under `e` by its path, and forwards the names
/// the blocks under it bind. A path holds no name forwarded after it, so one
/// table answers for every enclosing block at once.
fn forward(e: &mut Expr, bases: &Set<LocalId>, paths: &mut Map<LocalId, Expr>) {
    match &mut e.kind {
        ExprKind::Local(l) => {
            if let Some(path) = paths.get(l) {
                let span = e.span;
                *e = path.clone();
                e.span = span;
            }
        }
        ExprKind::Block { stmts, tail } => {
            // In order, so `let b = a.items;` after `let a = acc.inner;` is a
            // path by the time it is reached.
            for mut s in std::mem::take(stmts) {
                let value = match &mut s {
                    Stmt::Let { value, .. } => value,
                    Stmt::Expr(e) => e,
                };
                forward(value, bases, paths);
                match path_let(&s) {
                    Some((name, path)) if !bases.contains(&name) => {
                        paths.insert(name, path.clone());
                    }
                    _ => stmts.push(s),
                }
            }
            if let Some(t) = tail {
                forward(t, bases, paths);
            }
        }
        _ => typed::children_mut(e, &mut |k| forward(k, bases, paths)),
    }
}

/// `let name = path;` with `path` a chain of field and tuple reads off a local.
fn path_let(s: &Stmt) -> Option<(LocalId, &Expr)> {
    let Stmt::Let { pattern, value, .. } = s else { return None };
    let PatKind::Bind { local, sub: None } = &pattern.kind else { return None };
    let projects = matches!(value.kind, ExprKind::Field { .. } | ExprKind::TupleIndex { .. });
    (projects && is_path(value)).then_some((*local, value))
}

fn is_path(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Local(_) => true,
        ExprKind::Field { base, .. } | ExprKind::TupleIndex { base, .. } => is_path(base),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::forward_body;
    use crate::compiler::semantics::typed::{Expr, ExprKind, PatKind, Pattern, Stmt};
    use crate::compiler::semantics::types::{LocalId, Ty, TyConId};
    use crate::diagnostics::Span;

    fn e(kind: ExprKind) -> Expr {
        Expr::new(kind, Ty::ERROR, Span::default())
    }

    fn local(i: u32) -> Expr {
        e(ExprKind::Local(LocalId(i)))
    }

    fn field(base: Expr, index: usize) -> Expr {
        e(ExprKind::Field { base: Box::new(base), index })
    }

    fn let_(i: u32, value: Expr) -> Stmt {
        let pattern = Pattern {
            kind: PatKind::Bind { local: LocalId(i), sub: None },
            ty: Ty::ERROR,
            span: Span::default(),
        };
        Stmt::Let { pattern, value, span: Span::default() }
    }

    fn block(stmts: Vec<Stmt>, tail: Expr) -> Expr {
        e(ExprKind::Block { stmts, tail: Some(Box::new(tail)) })
    }

    fn update(base: Expr, value: Expr) -> Expr {
        e(ExprKind::StructUpdate { con: TyConId(0), base: Box::new(base), updates: vec![(0, value)] })
    }

    /// Paths reach every later read, through nested blocks and chained `let`s;
    /// a name an update is written over stays a name, wherever the update is.
    #[test]
    fn paths_reach_nested_blocks_and_update_bases_stay_names() {
        // p = 0, q = 1, a = 2, u = 3, r = 4, b = 5, c = 6
        let mut body = block(
            vec![
                let_(2, field(local(0), 0)),
                let_(3, field(local(1), 1)),
                let_(
                    4,
                    block(
                        vec![let_(5, field(local(2), 1)), let_(6, field(local(5), 0))],
                        e(ExprKind::Tuple(vec![local(6), update(local(3), local(5))])),
                    ),
                ),
            ],
            e(ExprKind::Tuple(vec![local(4), local(2)])),
        );
        forward_body(&mut body);
        let p_0 = field(local(0), 0);
        let expected = block(
            vec![
                let_(3, field(local(1), 1)),
                let_(
                    4,
                    block(
                        vec![],
                        e(ExprKind::Tuple(vec![
                            field(field(p_0.clone(), 1), 0),
                            update(local(3), field(p_0.clone(), 1)),
                        ])),
                    ),
                ),
            ],
            e(ExprKind::Tuple(vec![local(4), p_0])),
        );
        assert_eq!(format!("{body:?}"), format!("{expected:?}"));
    }
}
