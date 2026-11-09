//! A record read out of a map and written back under the same key, with the
//! map's slot emptied first so the record is the only holder of its parts.
//!
//! ```text
//! match (c.nodes.get(to)) {
//!     .Some(t) => {
//!         let after = handle(ctx, t);
//!         C { ..c, nodes: c.nodes.insert(ctx, t.id, after) }
//!     },
//!     .None => c,
//! }
//! ```
//!
//! `t` is a second holder of the record the map still holds, so `handle`
//! finds every map inside it shared and copies a root-to-leaf path of each
//! one it changes, once per call. The write that replaces the record comes
//! too late to help: it is what releases the map's count.
//!
//! So the block starts with that write, done early with a placeholder:
//!
//! ```text
//! let k = t.id;
//! let c2 = C { ..c, nodes: c.nodes.insert(ctx, k, placeholder) };
//! let after = handle(ctx, t);
//! C { ..c2, nodes: c2.nodes.insert(ctx, k, after) }
//! ```
//!
//! Inserting a key twice leaves only the second value, so the block's result
//! is the same map whatever the placeholder is. The first insert releases the
//! map's hold on the record, and `t` reaches `handle` unshared.
//!
//! **Why it is correct.** The block reads `c.nodes` nowhere but as the final
//! insert's receiver, so no reader can see the placeholder; every other field
//! of `c2` is `c`'s own. The key is read off `t` before anything else runs,
//! and a value never changes. The placeholder is `t` with every counted part
//! replaced by an empty one, a value of the right type holding nothing of
//! `t`'s.
//!
//! **When it runs.** Only where it pays: `t` comes out of a match on a read of
//! `c.nodes`, the final insert runs whenever the block does, and neither `c`
//! nor `t` is read after the block, so both die in it.
//!
//! **The native branch only**, after `forward`, which would otherwise put
//! `t.id` back in place of `k`, and before `rc`, which is what the rewrite is
//! for.

use crate::compiler::middle::monomorphize::{ConShape, Func, Program, Shapes};
use crate::compiler::middle::rc::{self, Answer, Counted};
use crate::compiler::semantics::name::Name;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Pattern, Stmt};
use crate::compiler::semantics::types::{self, FieldId, FuncIdx, LocalId, Prim, TyKind};
use crate::hash::Set;

/// The one function this rewrites around.
const INSERT: &str = "core/orderedmap:OrderedMap.insert";

pub fn run(program: &mut Program) {
    let inserts: Set<FuncIdx> = program
        .funcs
        .entries()
        .filter(|(_, f)| f.debug_name == INSERT)
        .map(|(i, _)| i)
        .collect();
    if inserts.is_empty() {
        return;
    }
    let mut cx = Find {
        inserts: &inserts,
        shapes: &program.shapes.clone(),
        counted: rc::Syntactic::new(program),
    };
    for f in program.funcs.iter_mut() {
        // Each rewrite leaves nothing the next search accepts again: the
        // final insert's base is the new name, which no match read, and the
        // early insert's key is the new name, which is no field of `t`.
        while let Some(plan) = f.body().and_then(|body| cx.find(body, &mut Vec::new())) {
            // `None` only if the plan's path is gone, which `find` rules out.
            let _ = apply(f, plan);
        }
    }
}

/// An update `C { ..c, f: c.f.insert(ctx, t.g, value) }`, taken apart.
struct Anchor<'a> {
    con: types::TyConId,
    /// `c` itself, and every update.
    base: &'a Expr,
    updates: &'a [(FieldId, Expr)],
    c: LocalId,
    f: FieldId,
    insert: FuncIdx,
    receiver: &'a Expr,
    ctx: &'a Expr,
    /// `t.g`.
    key: &'a Expr,
    t: LocalId,
    g: FieldId,
    value: &'a Expr,
}

fn anchor<'a>(e: &'a Expr, inserts: &Set<FuncIdx>) -> Option<Anchor<'a>> {
    let ExprKind::StructUpdate { con, base, updates } = &e.kind else {
        return None;
    };
    let c = local(base)?;
    updates.iter().find_map(|(f, update)| {
        let ExprKind::CallFunc { func, args } = &update.kind else {
            return None;
        };
        let [receiver, ctx, key, value] = args.as_slice() else {
            return None;
        };
        let (t, g) = path(key)?;
        // The placeholder is built from `t`, so `t` has the value's type.
        let whole = matches!(&key.kind, ExprKind::Field { base, .. } if base.ty == value.ty);
        (whole && inserts.contains(func) && path(receiver) == Some((c, *f))).then_some(Anchor {
            con: *con,
            base,
            updates,
            c,
            f: *f,
            insert: *func,
            receiver,
            ctx,
            key,
            t,
            g,
            value,
        })
    })
}

/// `x.f`, as `(x, f)`.
fn path(e: &Expr) -> Option<(LocalId, FieldId)> {
    match &e.kind {
        ExprKind::Field { base, index } => Some((local(base)?, *index)),
        _ => None,
    }
}

fn local(e: &Expr) -> Option<LocalId> {
    match e.kind {
        ExprKind::Local(l) => Some(l),
        _ => None,
    }
}

/// An expression's children, in the order [`typed::children`] visits them.
fn kids(e: &Expr) -> Vec<&Expr> {
    let mut out = Vec::new();
    typed::children(e, &mut |c| out.push(c));
    out
}

struct Find<'b> {
    inserts: &'b Set<FuncIdx>,
    shapes: &'b Shapes,
    counted: rc::Syntactic,
}

/// One accepted rewrite.
struct Plan {
    /// Child indices from the body down to the block.
    to_block: Vec<usize>,
    /// `c` itself, `c.f`, `ctx` and `t.g`, as the update read them.
    c: Expr,
    receiver: Expr,
    ctx: Expr,
    key: Expr,
    /// The update's own constructor and type, which `c2`'s are too.
    con: types::TyConId,
    f: FieldId,
    insert: FuncIdx,
    c_id: LocalId,
    t: LocalId,
    g: FieldId,
    placeholder: Expr,
}

impl Find<'_> {
    /// The first update under `e` this rewrites. `stack` holds every
    /// enclosing expression, outermost first, with the index of the child the
    /// walk went down.
    fn find<'a>(&mut self, e: &'a Expr, stack: &mut Vec<(&'a Expr, usize)>) -> Option<Plan> {
        if let Some(plan) = anchor(e, self.inserts).and_then(|a| self.accept(e, &a, stack)) {
            return Some(plan);
        }
        kids(e).into_iter().enumerate().find_map(|(i, child)| {
            stack.push((e, i));
            let plan = self.find(child, stack);
            stack.pop();
            plan
        })
    }

    fn accept(&mut self, u: &Expr, a: &Anchor<'_>, stack: &[(&Expr, usize)]) -> Option<Plan> {
        // The block: the innermost one around the update that reads `t`
        // whole, with nothing between the two that may skip the update.
        let mut level = stack.len();
        let block = loop {
            level = level.checked_sub(1)?;
            let &(node, i) = stack.get(level)?;
            if skips(node, i) {
                return None;
            }
            if matches!(node.kind, ExprKind::Block { .. }) {
                let mut uses = Uses {
                    u,
                    a,
                    whole: false,
                    ok: true,
                };
                uses.walk(node);
                if !uses.ok {
                    return None;
                }
                if uses.whole {
                    break node;
                }
            }
        };
        let outside = stack.get(..level)?;
        // `c`, `t` and the context are in scope where the block starts. A
        // context is a name or, holding nothing, a unit, so reading it early
        // runs nothing.
        matches!(a.ctx.kind, ExprKind::Local(_) | ExprKind::Unit).then_some(())?;
        let names: Set<LocalId> = [Some(a.c), Some(a.t), local(a.ctx)]
            .into_iter()
            .flatten()
            .collect();
        if binds(block, &mut |l| names.contains(&l)) {
            return None;
        }
        // Only where it pays. `t` is what a match on a read of `c.f` bound,
        let from_map = outside.iter().any(|&(node, _)| match &node.kind {
            ExprKind::Match { scrutinee, arms } => {
                let mut reads = false;
                typed::walk(scrutinee, &mut |e| reads |= path(e) == Some((a.c, a.f)));
                reads
                    && arms
                        .iter()
                        .any(|arm| bound(&arm.pattern, &mut |l| l == a.t))
            }
            _ => false,
        });
        // and both die in the block.
        if !from_map
            || outside
                .iter()
                .any(|&(node, i)| later(node, i).any(|e| mentions(e, a.c) || mentions(e, a.t)))
        {
            return None;
        }
        let t = Expr::new(ExprKind::Local(a.t), a.value.ty, a.key.span);
        Some(Plan {
            to_block: outside.iter().map(|&(_, i)| i).collect(),
            c: a.base.clone(),
            receiver: a.receiver.clone(),
            ctx: a.ctx.clone(),
            key: a.key.clone(),
            con: a.con,
            f: a.f,
            insert: a.insert,
            c_id: a.c,
            t: a.t,
            g: a.g,
            placeholder: self.empty(&t)?,
        })
    }

    /// A value of `e`'s type holding none of `e`'s counted parts: each one
    /// empty, and everything else read off `e`.
    fn empty(&mut self, e: &Expr) -> Option<Expr> {
        if self.counted.counted(&e.ty) == Answer::No {
            return Some(e.clone());
        }
        let kind = match e.ty.kind() {
            TyKind::Array(elem) => ExprKind::Array {
                elem: *elem,
                elems: Vec::new(),
            },
            TyKind::Con(id, args) => match self.shapes.cons.get(id.index())? {
                ConShape::Prim(Prim::Str) => ExprKind::Str(String::new()),
                // A struct holds itself only through a list or an enum, so
                // this ends.
                ConShape::Struct(declared) => ExprKind::StructUpdate {
                    con: *id,
                    base: Box::new(e.clone()),
                    updates: declared
                        .iter()
                        .enumerate()
                        .map(|(k, d)| {
                            let read = Expr::new(
                                ExprKind::Field {
                                    base: Box::new(e.clone()),
                                    index: FieldId::declared(k),
                                },
                                types::substitute(d, args, None),
                                e.span,
                            );
                            Some((FieldId::declared(k), self.empty(&read)?))
                        })
                        .collect::<Option<_>>()?,
                },
                _ => return None,
            },
            _ => return None,
        };
        Some(Expr::new(kind, e.ty, e.span))
    }
}

/// Whether the child at `i` of `node` may not run when `node` does.
fn skips(node: &Expr, i: usize) -> bool {
    matches!(
        node.kind,
        ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::And { .. } | ExprKind::Or { .. }
    ) && i > 0
}

/// What may run after the child at `i` of `node`, within `node`.
fn later(node: &Expr, i: usize) -> impl Iterator<Item = &Expr> {
    let alternatives = match &node.kind {
        // The other branches are alternatives, and so are a loop's entries: a
        // `Continue` rebinds every parameter before one runs again.
        ExprKind::If { .. } | ExprKind::And { .. } | ExprKind::Or { .. } => i > 0,
        // A failed guard runs the arms after it.
        ExprKind::Match { arms, .. } => i > 0 && arms.iter().all(|a| a.guard.is_none()),
        ExprKind::Loop { .. } => true,
        _ => false,
    };
    let rest = if alternatives { Vec::new() } else { kids(node) };
    rest.into_iter().skip(i).skip(1)
}

fn mentions(e: &Expr, l: LocalId) -> bool {
    let mut hit = false;
    typed::walk(e, &mut |x| hit |= local(x) == Some(l));
    hit
}

/// Whether a pattern binds a local `hit` holds for.
fn bound(p: &Pattern, hit: &mut impl FnMut(LocalId) -> bool) -> bool {
    let mut any = false;
    p.each_bind(&mut |l| any |= hit(l));
    any
}

/// Whether anything under `e` binds a local `hit` holds for.
fn binds(e: &Expr, hit: &mut impl FnMut(LocalId) -> bool) -> bool {
    let mut any = false;
    typed::walk(e, &mut |x| match &x.kind {
        ExprKind::Block { stmts, .. } => {
            for s in stmts {
                match s {
                    Stmt::Let { pattern, .. } => any |= bound(pattern, hit),
                    // Legal only in a test source, and binds nothing.
                    Stmt::Expr(_) => {}
                }
            }
        }
        ExprKind::Match { arms, .. } => {
            for arm in arms {
                any |= bound(&arm.pattern, hit);
            }
        }
        _ => {}
    });
    any
}

/// How a block reads `c` and `t`: `c` only as the update's base and
/// receiver or through another field, `t` whole or through the key's field.
struct Uses<'a, 'b> {
    u: &'a Expr,
    a: &'a Anchor<'b>,
    whole: bool,
    ok: bool,
}

impl Uses<'_, '_> {
    fn walk(&mut self, e: &Expr) {
        if std::ptr::eq(e, self.u) {
            // The base and the receiver are the two reads of `c` the rewrite
            // is for, and the key is the read of `t` it names.
            for (f, value) in self.a.updates {
                if *f != self.a.f {
                    self.walk(value);
                }
            }
            self.walk(self.a.ctx);
            self.walk(self.a.value);
            return;
        }
        match path(e) {
            Some((x, f)) if x == self.a.c => self.ok &= f != self.a.f,
            Some((x, g)) if x == self.a.t => self.ok &= g == self.a.g,
            _ => match local(e) {
                Some(x) if x == self.a.c => self.ok = false,
                Some(x) if x == self.a.t => self.whole = true,
                _ => typed::children(e, &mut |child| self.walk(child)),
            },
        }
    }
}

fn apply(f: &mut Func, plan: Plan) -> Option<()> {
    let k = f.locals.push(typed::Local {
        name: Name::new("vacated.key"),
        ty: plan.key.ty,
        span: f.span,
    });
    let c2 = f.locals.push(typed::Local {
        name: Name::new("vacated"),
        ty: plan.c.ty,
        span: f.span,
    });
    let mut block = f.body_mut()?;
    for &i in &plan.to_block {
        let mut kids = Vec::new();
        typed::children_mut(block, &mut |c| kids.push(c));
        block = kids.into_iter().nth(i)?;
    }
    let span = block.span;
    let early = Expr::new(
        ExprKind::StructUpdate {
            con: plan.con,
            base: Box::new(plan.c.clone()),
            updates: vec![(
                plan.f,
                Expr::new(
                    ExprKind::CallFunc {
                        func: plan.insert,
                        args: vec![
                            plan.receiver.clone(),
                            plan.ctx,
                            Expr::new(ExprKind::Local(k), plan.key.ty, span),
                            plan.placeholder,
                        ],
                    },
                    plan.receiver.ty,
                    span,
                ),
            )],
        },
        plan.c.ty,
        span,
    );
    rename(block, &mut |e| {
        if local(e) == Some(plan.c_id) {
            Some(ExprKind::Local(c2))
        } else {
            (path(e) == Some((plan.t, plan.g))).then_some(ExprKind::Local(k))
        }
    });
    let stmts = match &mut block.kind {
        ExprKind::Block { stmts, .. } => stmts,
        _ => return None,
    };
    let bind = |local: LocalId, value: Expr| Stmt::Let {
        pattern: Pattern {
            kind: PatKind::Bind { local, sub: None },
            ty: value.ty,
            span,
        },
        value,
        span,
    };
    stmts.splice(0..0, [bind(k, plan.key), bind(c2, early)]);
    Some(())
}

/// Replaces every expression under `e` that `with` has a replacement for.
fn rename(e: &mut Expr, with: &mut impl FnMut(&Expr) -> Option<ExprKind>) {
    match with(e) {
        Some(kind) => e.kind = kind,
        None => typed::children_mut(e, &mut |child| rename(child, with)),
    }
}
