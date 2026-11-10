//! A value read out of a map and written back under the same key, with the
//! map's slot emptied first so the value is the only holder of its parts.
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
//! let c2 = C { ..c, nodes: c.nodes.insert((), k, placeholder) };
//! let after = handle(ctx, t);
//! C { ..c2, nodes: c2.nodes.insert(ctx, k, after) }
//! ```
//!
//! Inserting a key twice leaves only the second value, so the block's result
//! is the same map whatever the placeholder is. The first insert releases the
//! map's hold on the record, and `t` reaches `handle` unshared.
//!
//! The same goes for a map held in a local rather than a field, and a key
//! held in a name of its own, the one the match read. An arm that is no block,
//! such as `.Some(xs) => m.insert(ctx, k, xs.push(ctx, 1))`, becomes one.
//!
//! The early write costs a descent of its own, which pays only when the value
//! is all but sure to be copied otherwise. A value a match found is, but one a
//! `let` reads with `withDefault` is often the default, or an empty list that
//! a change allocates anyway: on `network.send` in the kvstore bench, the
//! extra descent cost 3.5% more instructions than the copies it saved.
//!
//! **Why it is correct.** After the early write, the map is read nowhere but
//! as the final insert's receiver, so no reader can see the placeholder, and
//! the record holding it is read only through its other fields, each `c`'s
//! own. The key is read before anything else runs, and a value never changes.
//! The placeholder is the value with every counted part replaced by an empty
//! one, a value of the right type holding nothing of the original's.
//!
//! **When it runs.** Only where it pays: the value comes out of a match on a
//! read of the map, under the key the insert writes or, for a record, one of
//! its own fields. The final insert runs whenever the early write does, and
//! neither the map's holder nor a matched value is read after the block, so
//! both die in it. The insert's context is erased to a unit, so the early
//! write passes one; `--coverage` keeps contexts, and turns the rewrite off.
//!
//! **The native branch only**, after `forward`, which would otherwise put
//! `t.id` back in place of `k`, and before `rc`, which is what the rewrite is
//! for.

use crate::compiler::middle::monomorphize::{ConShape, Func, Program, Shapes};
use crate::compiler::middle::rc::{self, Answer, Counted};
use crate::compiler::semantics::name::Name;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Pattern, Stmt};
use crate::compiler::semantics::types::{self, FieldId, FuncIdx, LocalId, Prim, Ty, TyKind};
use crate::hash::Set;

/// The functions this rewrites around: each stdlib map's `insert`.
const INSERTS: [&str; 2] = ["core/orderedmap:OrderedMap.insert", "core/map:Map.insert"];

pub fn run(program: &mut Program) {
    let inserts: Set<FuncIdx> = program
        .funcs
        .entries()
        .filter(|(_, f)| INSERTS.contains(&f.debug_name.as_str()))
        .map(|(i, _)| i)
        .collect();
    if inserts.is_empty() {
        return;
    }
    let mut cx = Find {
        inserts: &inserts,
        shapes: &program.shapes.clone(),
        counted: rc::Syntactic::new(program),
        keys: Set::default(),
    };
    for f in program.funcs.iter_mut() {
        // Both inserts a rewrite leaves take the key it names, which no
        // search accepts again.
        cx.keys.clear();
        while let Some(plan) = f.body().and_then(|body| cx.find(body, f, &mut Vec::new())) {
            // `None` only if the plan's path is gone, which `find` rules out.
            let _ = apply(f, plan, &mut cx.keys);
        }
    }
}

/// Where the map lives: `c.f`, or a local of its own.
#[derive(Clone, Copy, PartialEq)]
enum Holder {
    Field(LocalId, FieldId),
    Local(LocalId),
}

impl Holder {
    /// The local whose value holds the map.
    fn base(self) -> LocalId {
        match self {
            Holder::Field(c, _) | Holder::Local(c) => c,
        }
    }

    /// Whether `e` reads the map: `c.f`, or the local.
    fn is(self, e: &Expr) -> bool {
        match self {
            Holder::Field(c, f) => path(e) == Some((c, f)),
            Holder::Local(m) => local(e) == Some(m),
        }
    }

    /// Whether anything under `e` reads the map.
    fn read_in(self, e: &Expr) -> bool {
        let mut hit = false;
        typed::walk(e, &mut |x| hit |= self.is(x));
        hit
    }
}

/// A record update's constructor, its base and every update.
type Update<'a> = (types::TyConId, &'a Expr, &'a [(FieldId, Expr)]);

/// A write back, `C { ..c, f: c.f.insert(ctx, key, value) }` or
/// `m.insert(ctx, key, value)`, taken apart.
struct Anchor<'a> {
    holder: Holder,
    /// The record update, for a field.
    update: Option<Update<'a>>,
    insert: FuncIdx,
    receiver: &'a Expr,
    ctx: &'a Expr,
    key: &'a Expr,
    value: &'a Expr,
}

/// The insert's arguments, when `e` calls one of `inserts` on a key a
/// rewrite didn't name.
fn inserting<'a>(
    e: &'a Expr,
    inserts: &Set<FuncIdx>,
    keys: &Set<LocalId>,
) -> Option<(FuncIdx, &'a [Expr])> {
    let ExprKind::CallFunc { func, args } = &e.kind else {
        return None;
    };
    let [_, _, key, _] = args.as_slice() else {
        return None;
    };
    // A key read once early must not change, so it is a name or a field of
    // one.
    let stable = match local(key) {
        Some(k) => !keys.contains(&k),
        None => path(key).is_some(),
    };
    (stable && inserts.contains(func)).then_some((*func, args.as_slice()))
}

fn anchor<'a>(e: &'a Expr, inserts: &Set<FuncIdx>, keys: &Set<LocalId>) -> Option<Anchor<'a>> {
    let parts = |holder, update, (insert, args): (FuncIdx, &'a [Expr])| match args {
        [receiver, ctx, key, value] => Some(Anchor {
            holder,
            update,
            insert,
            receiver,
            ctx,
            key,
            value,
        }),
        _ => None,
    };
    match &e.kind {
        ExprKind::StructUpdate { con, base, updates } => {
            let c = local(base)?;
            updates.iter().find_map(|(f, update)| {
                let (insert, args) = inserting(update, inserts, keys)?;
                let holder = Holder::Field(c, *f);
                holder.is(args.first()?).then_some(())?;
                parts(
                    holder,
                    Some((*con, &**base, updates.as_slice())),
                    (insert, args),
                )
            })
        }
        ExprKind::CallFunc { .. } => {
            let (insert, args) = inserting(e, inserts, keys)?;
            let holder = Holder::Local(local(args.first()?)?);
            parts(holder, None, (insert, args))
        }
        _ => None,
    }
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

/// The local a key is read off: the key itself, or the base of its field.
fn key_base(key: &Expr) -> Option<LocalId> {
    local(key).or_else(|| path(key).map(|(x, _)| x))
}

/// Whether `a` and `b` read the same name or the same field of one.
fn same_read(a: &Expr, b: &Expr) -> bool {
    match (local(a), local(b)) {
        (Some(x), Some(y)) => x == y,
        (None, None) => path(a).is_some() && path(a) == path(b),
        _ => false,
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
    /// The keys this function's rewrites named.
    keys: Set<LocalId>,
}

/// One accepted rewrite.
struct Plan {
    /// Child indices from the body down to the expression the early write
    /// goes before.
    to_block: Vec<usize>,
    holder: Holder,
    /// The holder's own constructor and type, for a field.
    con: Option<types::TyConId>,
    /// The holder's base as the update read it, `c.f` or `m`, and the key.
    base: Expr,
    receiver: Expr,
    key: Expr,
    insert: FuncIdx,
    placeholder: Expr,
}

/// Whether a plan's region reads the holder only at the anchor, or through
/// the record's other fields.
struct Uses<'a, 'b> {
    u: &'a Expr,
    a: &'a Anchor<'b>,
    ok: bool,
}

impl Uses<'_, '_> {
    fn walk(&mut self, e: &Expr) {
        if std::ptr::eq(e, self.u) {
            // The base and the receiver are the reads of the holder the
            // rewrite is for.
            if let Some((_, _, updates)) = self.a.update {
                for (f, value) in updates {
                    if Holder::Field(self.a.holder.base(), *f) != self.a.holder {
                        self.walk(value);
                    }
                }
            }
            self.walk(self.a.ctx);
            self.walk(self.a.key);
            self.walk(self.a.value);
            return;
        }
        let base = self.a.holder.base();
        if self.a.holder.is(e) {
            self.ok = false;
        } else if path(e).is_some_and(|(x, _)| x == base) {
            // Another field of the record holding the map.
        } else if local(e) == Some(base) {
            self.ok = false;
        } else {
            typed::children(e, &mut |child| self.walk(child));
        }
    }
}

impl Find<'_> {
    /// The first write back under `e` this rewrites. `stack` holds every
    /// enclosing expression, outermost first, with the index of the child the
    /// walk went down.
    fn find<'a>(
        &mut self,
        e: &'a Expr,
        f: &Func,
        stack: &mut Vec<(&'a Expr, usize)>,
    ) -> Option<Plan> {
        if let Some(plan) =
            anchor(e, self.inserts, &self.keys).and_then(|a| self.accept(e, &a, f, stack))
        {
            return Some(plan);
        }
        kids(e).into_iter().enumerate().find_map(|(i, child)| {
            stack.push((e, i));
            let plan = self.find(child, f, stack);
            stack.pop();
            plan
        })
    }

    fn accept(
        &mut self,
        u: &Expr,
        a: &Anchor<'_>,
        f: &Func,
        stack: &[(&Expr, usize)],
    ) -> Option<Plan> {
        // An erased context is a unit wherever the early write goes, so the
        // write reads no name of it.
        (a.ctx.ty == Ty::UNIT).then_some(())?;
        // A value holding nothing counted is never copied.
        if self.counted.counted(&a.value.ty) == Answer::No {
            return None;
        }
        // The levels the insert runs whenever they do: up to the first branch,
        // arm, lambda or loop on the way out.
        let reach = stack
            .iter()
            .rposition(|&(node, i)| skips(node, i))
            .map_or(0, |at| at.saturating_add(1));
        let (level, wrap, value) = self.after_match(a, &f.locals, stack, reach)?;
        let (node, i) = *stack.get(level)?;
        // The expression the early write goes before.
        let region = if wrap {
            kids(node).into_iter().nth(i)?
        } else {
            node
        };
        let mut uses = Uses { u, a, ok: true };
        uses.walk(region);
        if !uses.ok {
            return None;
        }
        // Every name the early write reads is bound before the region: the
        // holder and the key where the match read them and the value by its
        // arm. A name is bound in one place only.
        // Neither the holder nor the value is read after the block, so both
        // die in it.
        let outside = stack.get(..level)?;
        let base = a.holder.base();
        if outside
            .iter()
            .any(|&(node, i)| later(node, i).any(|e| mentions(e, base) || mentions(e, value)))
        {
            return None;
        }
        let v = Expr::new(ExprKind::Local(value), a.value.ty, a.key.span);
        let mut to_block: Vec<usize> = outside.iter().map(|&(_, i)| i).collect();
        if wrap {
            to_block.push(i);
        }
        Some(Plan {
            to_block,
            holder: a.holder,
            con: a.update.map(|(con, _, _)| con),
            base: a
                .update
                .map_or_else(|| a.receiver.clone(), |(_, base, _)| base.clone()),
            receiver: a.receiver.clone(),
            key: a.key.clone(),
            insert: a.insert,
            placeholder: self.empty(&v)?,
        })
    }

    /// A match on a read of the map that binds a value of the insert's type,
    /// around the insert: the level of the block the early write starts, or of
    /// the expression holding what becomes one, whether it does, and the
    /// value's name. The key is the match's own, or a field of the value.
    fn after_match(
        &mut self,
        a: &Anchor<'_>,
        locals: &typed::Locals,
        stack: &[(&Expr, usize)],
        reach: usize,
    ) -> Option<(usize, bool, LocalId)> {
        let value = stack.iter().take(reach).rev().find_map(|&(node, i)| {
            let ExprKind::Match { scrutinee, arms } = &node.kind else {
                return None;
            };
            if !a.holder.read_in(scrutinee) {
                return None;
            }
            let arm = arm_at(arms, i)?;
            let mut found = None;
            arm.pattern.each_bind(&mut |l| {
                let own = key_base(a.key) == Some(l) && path(a.key).is_some();
                if locals.get(l).ty == a.value.ty && (own || reads_key(scrutinee, a.key)) {
                    found = found.or(Some(l));
                }
            });
            found
        })?;
        // The innermost block the insert always runs in that reads the value
        // whole, so the value is still to be handed on where the write goes.
        let block = (reach..stack.len()).rev().find(|&level| {
            stack.get(level).is_some_and(|&(node, _)| {
                matches!(node.kind, ExprKind::Block { .. }) && whole(node, value, a.key)
            })
        });
        match block {
            Some(level) => Some((level, false, value)),
            // No block: the expression the insert always runs in becomes one.
            None => {
                let up = reach.checked_sub(1)?;
                let &(node, i) = stack.get(up)?;
                let target = kids(node).into_iter().nth(i)?;
                whole(target, value, a.key).then_some((up, true, value))
            }
        }
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

/// The arm whose guard or body is child `i` of its match.
fn arm_at(arms: &[typed::Arm], i: usize) -> Option<&typed::Arm> {
    // A guard runs before the arm is chosen, so it answers `None`.
    arms.iter()
        .flat_map(|arm| {
            arm.guard
                .as_ref()
                .map(|_| None)
                .into_iter()
                .chain([Some(arm)])
        })
        .nth(i.checked_sub(1)?)?
}

/// Whether anything under `e` reads `key`.
fn reads_key(e: &Expr, key: &Expr) -> bool {
    let mut hit = false;
    typed::walk(e, &mut |x| hit |= same_read(x, key));
    hit
}

/// Whether `e` reads `value` whole somewhere, not only as the key's field.
fn whole(e: &Expr, value: LocalId, key: &Expr) -> bool {
    match local(e) {
        Some(l) => l == value,
        None if same_read(e, key) => false,
        None => kids(e).into_iter().any(|c| whole(c, value, key)),
    }
}

/// Whether the child at `i` of `node` may not run when `node` does, or may
/// run more than once.
fn skips(node: &Expr, i: usize) -> bool {
    match node.kind {
        ExprKind::If { .. }
        | ExprKind::Match { .. }
        | ExprKind::And { .. }
        | ExprKind::Or { .. } => i > 0,
        ExprKind::Lambda { .. } | ExprKind::Loop { .. } => true,
        _ => false,
    }
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

fn apply(f: &mut Func, plan: Plan, keys: &mut Set<LocalId>) -> Option<()> {
    let k = f.locals.push(typed::Local {
        name: Name::new("vacated.key"),
        ty: plan.key.ty,
        span: f.span,
    });
    keys.insert(k);
    let h2 = f.locals.push(typed::Local {
        name: Name::new("vacated"),
        ty: plan.base.ty,
        span: f.span,
    });
    let mut block = f.body_mut()?;
    for &i in &plan.to_block {
        let mut kids = Vec::new();
        typed::children_mut(block, &mut |c| kids.push(c));
        block = kids.into_iter().nth(i)?;
    }
    let span = block.span;
    let insert = Expr::new(
        ExprKind::CallFunc {
            func: plan.insert,
            args: vec![
                plan.receiver.clone(),
                Expr::new(ExprKind::Unit, Ty::UNIT, span),
                Expr::new(ExprKind::Local(k), plan.key.ty, span),
                plan.placeholder,
            ],
        },
        plan.receiver.ty,
        span,
    );
    let early = match (plan.holder, plan.con) {
        (Holder::Field(_, field), Some(con)) => Expr::new(
            ExprKind::StructUpdate {
                con,
                base: Box::new(plan.base.clone()),
                updates: vec![(field, insert)],
            },
            plan.base.ty,
            span,
        ),
        _ => insert,
    };
    let base = plan.holder.base();
    rename(block, &mut |e| {
        if local(e) == Some(base) {
            Some(ExprKind::Local(h2))
        } else {
            same_read(e, &plan.key).then_some(ExprKind::Local(k))
        }
    });
    let bind = |local: LocalId, value: Expr| Stmt::Let {
        pattern: Pattern {
            kind: PatKind::Bind { local, sub: None },
            ty: value.ty,
            span,
        },
        value,
        span,
    };
    let ty = block.ty;
    let inner = std::mem::replace(block, Expr::new(ExprKind::Unit, ty, span));
    *block = Expr::new(
        ExprKind::Block {
            stmts: vec![bind(k, plan.key), bind(h2, early)],
            tail: Some(Box::new(inner)),
        },
        ty,
        span,
    );
    Some(())
}

/// Replaces every expression under `e` that `with` has a replacement for.
fn rename(e: &mut Expr, with: &mut impl FnMut(&Expr) -> Option<ExprKind>) {
    match with(e) {
        Some(kind) => e.kind = kind,
        None => typed::children_mut(e, &mut |child| rename(child, with)),
    }
}
