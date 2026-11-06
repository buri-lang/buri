//! One body for instantiations that differ only in a type they don't use.
//!
//! A library generic over its caller's context, `fn checkpointPath<C:
//! Allocator>(ctx: C, directory: Path)`, is instantiated once per handler stack
//! a program builds. Most such functions only hand `ctx` on to other functions
//! of the same kind, and at the bottom to a runtime call that drops it
//! (`runtime_table::Arg::Dropped`). Their instantiations are then one body
//! under many symbols, and none of them matches across programs.
//!
//! So a type parameter that is **erasable** is instantiated at `()`. Every
//! value of it is a `ctx` the body passes on, so at `()` that value is
//! zero-sized and leaves the signature (`layout::Layouts::zero_sized`). A
//! caller hands the erased instantiation `()` where it handed the context.
//!
//! A parameter `j` of a function is erasable when all of these hold:
//!
//! - Every local whose type mentions `j` is a parameter, of the function or of
//!   a lambda in it, typed exactly `j` or as a callback `fn(j, …) => R` that
//!   takes `j` only as a whole parameter and doesn't return it.
//! - Every read of one, and every lambda of a callback's type, is handed on: an
//!   argument of a direct call where the callee's parameter has the same
//!   shape with a bare type parameter in `j`'s place, which is itself
//!   erasable, or the callee or an argument of a callback call.
//! - No other expression, type argument, receiver or capture mentions `j`,
//!   except a captured callback, and the result type doesn't. So `j` reaches
//!   no layout, no glue, no trait or effect call, no template and no derive.
//! - A runtime operation is erasable at `j` only where `j` is bounded by
//!   `Allocator` alone and every parameter typed `j` is one the runtime drops,
//!   or one an open-coded list walk hands to its step or reads nowhere.
//!   JavaScript reads a `Tasks` context's identity, so nothing else qualifies.
//!
//! The rule is the greatest fixed point over the call graph: a recursive
//! function that only passes `ctx` to itself erases it. A caller can only
//! erase a callee that takes no callback typed by `j`, since its callback is
//! typed by the caller's `j`. So `xs.foldCtx(ctx, fn(c, acc, x) => …)` keeps
//! one body for its caller, and its step, when the step only hands `c` on.
//!
//! Debug names carry no type arguments, so backtraces and messages read the
//! same. A function used as a value keeps its full instantiation, because its
//! type is the caller's. Coverage turns this off: it reports each
//! instantiation on its own.

use super::Monomorphizer;
use crate::compiler::backend::{intrinsic_keys, runtime_table};
use crate::compiler::semantics::typed::{self, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::*;
use crate::hash::Map as HashMap;

/// What one function's own body says about each of its type parameters.
struct Facts {
    /// Whether nothing local rules the parameter out.
    ok: Vec<bool>,
    /// Per parameter, the callee parameters it is passed to.
    deps: Vec<Vec<(FnId, usize)>>,
}

/// Whether a type mentions parameter `j`, or `Self` or anything unsettled,
/// which could stand for it.
fn mentions(ty: &Ty, j: usize) -> bool {
    match ty.kind() {
        TyKind::Param(i) => *i as usize == j,
        TyKind::Con(_, args) | TyKind::Tuple(args) => args.iter().any(|a| mentions(a, j)),
        TyKind::Array(e) => mentions(e, j),
        TyKind::Fn(ps, r) => ps.iter().any(|p| mentions(p, j)) || mentions(r, j),
        TyKind::SelfTy | TyKind::Var(_) | TyKind::Error => true,
        TyKind::Unit | TyKind::Ctx(_) => false,
    }
}

/// Whether a value of type `ty`, which mentions `j`, given where a `to` is
/// wanted, hands `j` on and does nothing else with it: `j` itself at a bare
/// type parameter, or a callback that takes `j` only as a whole parameter, at
/// a callback type whose parameter there is bare.
fn hands_on(ty: &Ty, to: &Ty, j: usize) -> bool {
    let bare = |t: &Ty| matches!(t.kind(), TyKind::Param(_));
    match (ty.kind(), to.kind()) {
        (TyKind::Fn(ps, r), TyKind::Fn(qs, _)) => {
            !mentions(r, j)
                && ps
                    .iter()
                    .zip(qs.iter())
                    .all(|(p, q)| !mentions(p, j) || bare(p) && bare(q))
        }
        _ => bare(ty) && bare(to),
    }
}

impl Monomorphizer<'_> {
    /// `f`'s type arguments with every erasable one replaced by `()`, or
    /// `None` where none changes.
    pub(super) fn erased(&mut self, f: FnId, targs: &[Ty]) -> Option<Vec<Ty>> {
        if !self.erase || targs.is_empty() {
            return None;
        }
        let mask = self.erasable(f);
        let params = &self.tables().fn_info(f).params;
        let mut out = targs.to_vec();
        let mut changed = false;
        for (j, (t, &erase)) in out.iter_mut().zip(mask.iter()).enumerate() {
            // A callback typed by `j` is the caller's at its own `j`, so only
            // a caller erased itself can call `f` erased.
            let bare = params
                .iter()
                .all(|p| !mentions(&p.ty, j) || matches!(p.ty.kind(), TyKind::Param(_)));
            if erase && bare && *t != Ty::UNIT {
                *t = Ty::UNIT;
                changed = true;
            }
        }
        changed.then_some(out)
    }

    /// The arguments of a call to `f` once its type arguments are erased by
    /// [`Monomorphizer::erased`]: `()` at each parameter typed by an erased
    /// one, after evaluating what was there.
    pub(super) fn erased_args(
        &self,
        f: FnId,
        erased: &[Ty],
        mut args: Vec<typed::Expr>,
    ) -> Vec<typed::Expr> {
        let params = &self.tables().fn_info(f).params;
        for (arg, p) in args.iter_mut().zip(params) {
            let TyKind::Param(j) = p.ty.kind() else {
                continue;
            };
            if erased.get(*j as usize) != Some(&Ty::UNIT) || arg.ty == Ty::UNIT {
                continue;
            }
            let span = arg.span;
            let unit = typed::Expr::new(ExprKind::Unit, Ty::UNIT, span);
            let value = std::mem::replace(arg, unit);
            if !matches!(value.kind, ExprKind::Local(_)) {
                // Evaluated as before, and dropped by the caller.
                *arg = typed::Expr::new(
                    ExprKind::Block {
                        stmts: vec![Stmt::Let {
                            pattern: typed::Pattern {
                                kind: PatKind::Wild,
                                ty: value.ty,
                                span,
                            },
                            value,
                            span,
                        }],
                        tail: Some(Box::new(typed::Expr::new(ExprKind::Unit, Ty::UNIT, span))),
                    },
                    Ty::UNIT,
                    span,
                );
            }
        }
        args
    }

    /// Which of `f`'s type parameters are erasable.
    fn erasable(&mut self, f: FnId) -> std::rc::Rc<[bool]> {
        if let Some(known) = self.erasable.get(&f) {
            return known.clone();
        }
        // Every function the answer depends on, with its own facts.
        let mut found: HashMap<FnId, Facts> = HashMap::default();
        let mut stack = vec![f];
        while let Some(g) = stack.pop() {
            if found.contains_key(&g) || self.erasable.contains_key(&g) {
                continue;
            }
            let facts = self.facts(g);
            stack.extend(facts.deps.iter().flatten().map(|(h, _)| *h));
            found.insert(g, facts);
        }
        // Rule out whatever passes a parameter to one that is ruled out, until
        // nothing changes.
        loop {
            let next: Vec<Vec<bool>> = found
                .values()
                .map(|facts| {
                    let passed_on = |(h, t): &(FnId, usize)| {
                        let known = self.erasable.get(h).map(|m| m.get(*t).copied());
                        let open = found.get(h).map(|x| x.ok.get(*t).copied());
                        known.or(open).flatten() == Some(true)
                    };
                    let ok = facts.ok.iter().zip(&facts.deps);
                    ok.map(|(ok, deps)| *ok && deps.iter().all(passed_on))
                        .collect()
                })
                .collect();
            let mut changed = false;
            for (facts, ok) in found.values_mut().zip(next) {
                changed |= facts.ok != ok;
                facts.ok = ok;
            }
            if !changed {
                break;
            }
        }
        for (g, facts) in found {
            self.erasable.insert(g, facts.ok.into());
        }
        self.erasable
            .get(&f)
            .cloned()
            .unwrap_or_else(|| Vec::new().into())
    }

    /// What `f`'s declaration and body say on their own.
    fn facts(&self, f: FnId) -> Facts {
        let info = self.tables().fn_info(f);
        let n = info.generics.len();
        let mut facts = Facts {
            ok: (0..n).map(|j| !mentions(&info.ret, j)).collect(),
            deps: vec![Vec::new(); n],
        };
        // A runtime operation, or a trait method with no body, which is already
        // diagnosed.
        let Some(body) = self.checked.bodies.get(&f).filter(|_| !info.intrinsic) else {
            let key = info
                .intrinsic
                .then(|| self.runtime_key(info))
                .unwrap_or_default();
            let entry = runtime_table::entry(&key);
            // A list walk, which hands its context to its step or reads it
            // nowhere, and the open-coded operations that read it nowhere.
            let walk = intrinsic_keys::list_call(&key);
            let unread = matches!(
                key.as_str(),
                "list.zip" | "list.flatten" | "str.concat" | "str.format"
            );
            for (j, ok) in facts.ok.iter_mut().enumerate() {
                let bounds = info
                    .generics
                    .get(j)
                    .map_or(&[][..], |g| g.bounds.as_slice());
                let allocator_only = bounds.iter().all(|b| {
                    let t = self.tables().trait_(*b);
                    t.name == "Allocator"
                        && self.module_paths.get(t.module.index()).map(String::as_str)
                            == Some("platform/effect")
                });
                let dropped = info.params.iter().enumerate().all(|(i, p)| {
                    let bare = matches!(p.ty.kind(), TyKind::Param(_));
                    !mentions(&p.ty, j)
                        || bare && (unread || entry.is_some_and(|e| e.dropped(i)))
                        || walk.as_ref().is_some_and(|w| {
                            if w.func == i {
                                hands_on(&p.ty, &p.ty, j)
                            } else {
                                bare
                            }
                        })
                });
                *ok = *ok && allocator_only && dropped;
            }
            return facts;
        };
        // Only a parameter, of the function or of a lambda in it, may hold a
        // value of the type, typed as it or as a callback that takes it.
        let mut params = body.params.clone();
        lambda_params(&body.expr, &mut params);
        for (id, local) in body.locals.entries() {
            for (j, ok) in facts.ok.iter_mut().enumerate() {
                if mentions(&local.ty, j)
                    && (!hands_on(&local.ty, &local.ty, j) || !params.contains(&id))
                {
                    *ok = false;
                }
            }
        }
        if facts.ok.iter().any(|ok| *ok) {
            self.uses(&body.expr, None, &body.locals, &mut facts);
        }
        facts
    }

    /// Rules out every parameter `e` uses other than by handing it on. `at`
    /// is the type wanted where `e` is a callee's argument, or a callback's.
    fn uses(&self, e: &typed::Expr, at: Option<&Ty>, locals: &typed::Locals, facts: &mut Facts) {
        let rule_out = |ty: &Ty, facts: &mut Facts| {
            for (j, ok) in facts.ok.iter_mut().enumerate() {
                if mentions(ty, j) {
                    *ok = false;
                }
            }
        };
        let handed = matches!(e.kind, ExprKind::Local(_) | ExprKind::Lambda { .. });
        for (j, ok) in facts.ok.iter_mut().enumerate() {
            if mentions(&e.ty, j) && !(handed && at.is_some_and(|to| hands_on(&e.ty, to, j))) {
                *ok = false;
            }
        }
        match &e.kind {
            ExprKind::CallFn { func, args } => {
                for (t, targ) in func.targs.iter().enumerate() {
                    for (j, deps) in facts.deps.iter_mut().enumerate() {
                        if matches!(targ.kind(), TyKind::Param(k) if *k as usize == j) {
                            deps.push((func.id, t));
                        }
                    }
                    let bare = matches!(targ.kind(), TyKind::Param(_));
                    if !bare {
                        rule_out(targ, facts);
                    }
                }
                let params = &self.tables().fn_info(func.id).params;
                for (k, arg) in args.iter().enumerate() {
                    self.uses(arg, params.get(k).map(|p| &p.ty), locals, facts);
                }
                return;
            }
            // Each argument has the type the callback takes there.
            ExprKind::CallValue { callee, args } => {
                for c in std::iter::once(&**callee).chain(args) {
                    self.uses(c, Some(&c.ty), locals, facts);
                }
                return;
            }
            ExprKind::CallTrait { recv, targs, .. } => {
                rule_out(recv, facts);
                targs.iter().for_each(|t| rule_out(t, facts));
            }
            ExprKind::FnRef(callee) => callee.targs.iter().for_each(|t| rule_out(t, facts)),
            ExprKind::StructLit { targs, .. }
            | ExprKind::EnumLit { targs, .. }
            | ExprKind::Intrinsic { targs, .. } => targs.iter().for_each(|t| rule_out(t, facts)),
            ExprKind::Array { elem, .. } | ExprKind::Index { elem, .. } => rule_out(elem, facts),
            ExprKind::Lambda { captures, .. } => {
                // A callback may be captured, since a function value's layout
                // is one at every type.
                for c in captures {
                    let ty = &locals.get(*c).ty;
                    for (j, ok) in facts.ok.iter_mut().enumerate() {
                        if mentions(ty, j)
                            && !(matches!(ty.kind(), TyKind::Fn(..)) && hands_on(ty, ty, j))
                        {
                            *ok = false;
                        }
                    }
                }
            }
            _ => {}
        }
        typed::children(e, &mut |c| self.uses(c, None, locals, facts));
    }
}

/// Adds the parameters of every lambda in `e` to `params`.
fn lambda_params(e: &typed::Expr, params: &mut Vec<LocalId>) {
    if let ExprKind::Lambda { params: ps, .. } = &e.kind {
        params.extend(ps.iter().copied());
    }
    typed::children(e, &mut |c| lambda_params(c, params));
}
