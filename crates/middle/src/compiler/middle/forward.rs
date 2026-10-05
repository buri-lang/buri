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

pub fn run(program: &mut Program) {
    for f in &mut program.funcs {
        if let Some(body) = f.body_mut() {
            forward(body);
        }
    }
}

fn forward(e: &mut Expr) {
    typed::children_mut(e, &mut forward);
    let ExprKind::Block { stmts, tail } = &mut e.kind else { return };
    // In order, so `let b = a.items;` after `let a = acc.inner;` is a path by
    // the time it is reached.
    let mut forwarded: Vec<(LocalId, Expr)> = Vec::new();
    for mut s in std::mem::take(stmts) {
        let value = match &mut s {
            Stmt::Let { value, .. } => value,
            Stmt::Expr(e) => e,
        };
        for (name, path) in &forwarded {
            replace(value, *name, path);
        }
        match path_let(&s) {
            Some(named) => forwarded.push(named),
            None => stmts.push(s),
        }
    }
    if let Some(t) = tail {
        for (name, path) in &forwarded {
            replace(t, *name, path);
        }
    }
}

/// `let name = path;` with `path` a chain of field and tuple reads off a local.
fn path_let(s: &Stmt) -> Option<(LocalId, Expr)> {
    let Stmt::Let { pattern, value, .. } = s else { return None };
    let PatKind::Bind { local, sub: None } = &pattern.kind else { return None };
    let projects = matches!(value.kind, ExprKind::Field { .. } | ExprKind::TupleIndex { .. });
    (projects && is_path(value)).then(|| (*local, value.clone()))
}

fn is_path(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Local(_) => true,
        ExprKind::Field { base, .. } | ExprKind::TupleIndex { base, .. } => is_path(base),
        _ => false,
    }
}

fn replace(e: &mut Expr, name: LocalId, path: &Expr) {
    if matches!(e.kind, ExprKind::Local(l) if l == name) {
        let span = e.span;
        *e = path.clone();
        e.span = span;
        return;
    }
    typed::children_mut(e, &mut |k| replace(k, name, path));
}
