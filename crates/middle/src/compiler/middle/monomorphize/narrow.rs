//! One body for callers whose contexts bind effects the callee never names.
//!
//! A library function generic over its caller's context, `fn spin<C:
//! Random>(ctx: C, n: Int)`, can only call `Random` through `ctx`, or hand
//! `ctx` on. Yet it is instantiated once per context a caller holds:
//! `{Random: Mul, Log: Quiet}` and `{Log: Loud, Random: Mul}` make two copies
//! of one body.
//!
//! So where a type parameter `j` of a function is **narrowable**, a context
//! at `j` is replaced by the narrowed context of `j`'s bounds
//! (`Tables::add_narrowed_contexts`), read at the caller's implementations of
//! those effects. The caller builds that context from its own, one field per
//! effect, and hands it over where it handed its own. Both contexts above
//! narrow to `{Random: Mul}`, so the two callers share one body.
//!
//! A parameter `j` of a function with a body is narrowable when all of these
//! hold:
//!
//! - It is bounded, by effects only, so a value of it is good for nothing but
//!   calling those effects and being handed on.
//! - Every parameter of the function that mentions `j` is typed exactly `j`,
//!   and the result type doesn't mention it, so the caller converts each value
//!   that crosses and nothing else does.
//! - No type built from `j` reaches a runtime operation that keeps a value
//!   past the call, there or in anything it hands a type built from `j` on to.
//!   `core/tasks` queues a task typed `fn(C) => ()` for its drain to run, and
//!   a drain at the narrowed context would run a task queued at the caller's.
//!   The operations of the value modules ([`VALUE_MODULES`]) only compute a
//!   value from their arguments, so any type may reach them; any other takes
//!   `j` only as the whole context.
//!
//! Only `buri test` narrows, where each test holds a context of its own. A
//! program `buri build` makes mostly runs at one, and the walk for the last
//! rule would cost it 1.6% of a build. JavaScript makes a context a live
//! effect handle the runtime writes through, so only native tests narrow.
//! Coverage turns this off, as it does erasure: it reports each instantiation
//! on its own. A function used as a value keeps its full instantiation,
//! because its type is the caller's.

use super::erase::mentions;
use super::Monomorphizer;
use crate::compiler::semantics::typed::{self, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::*;
use crate::diagnostics::Invariant as _;

impl Monomorphizer<'_> {
    /// `f`'s type arguments with every context at a narrowable parameter
    /// replaced by its narrowed context, with each change, or `None` where none
    /// changes.
    pub(super) fn narrowed(&mut self, f: FnId, targs: &[Ty]) -> Option<(Vec<Ty>, Vec<Changed>)> {
        if !self.narrow || !targs.iter().any(|t| matches!(t.kind(), TyKind::Ctx(..))) {
            return None;
        }
        let at = f.index();
        let slots = match self.narrowable.get(at).cloned().flatten() {
            Some(known) => known,
            None => {
                let slots: std::rc::Rc<[Option<CtxTypeId>]> = self.narrowable(f).into();
                if let Some(entry) = self.narrowable.get_mut(at) {
                    *entry = Some(slots.clone());
                } else {
                    self.narrowable.resize_with(at, || None);
                    self.narrowable.push(Some(slots.clone()));
                }
                slots
            }
        };
        let mut out: Option<(Vec<Ty>, Vec<Changed>)> = None;
        for (j, t) in targs.iter().enumerate() {
            let (Some(Some(to_id)), TyKind::Ctx(from_id, cargs)) =
                (slots.get(j).copied(), t.kind())
            else {
                continue;
            };
            let tables = self.tables();
            let (to, from) = (tables.ctx_type(to_id), tables.ctx_type(*from_id));
            // Already the same effects in the same order: nothing to save.
            if to
                .bindings
                .iter()
                .map(|b| b.0)
                .eq(from.bindings.iter().map(|b| b.0))
            {
                continue;
            }
            let impls: Vec<Ty> = (to.bindings.iter())
                .map(|(e, _)| {
                    let ty = from
                        .get(*e)
                        .or_ice("a caller's context binds its callee's bounds");
                    self.sub(ty, cargs)
                })
                .collect();
            // Other effects, so another type than `t`.
            let narrowed = self.canonical(Ty::ctx_at(to_id, impls));
            // Asked last: it walks everything `f` hands `j` on to.
            if self.kept(f, j) {
                let (all, changes) = out.get_or_insert_with(|| (targs.to_vec(), Vec::new()));
                *all.get_mut(j).or_ice("one type argument per parameter") = narrowed;
                changes.push(Changed {
                    param: param(j),
                    from: *t,
                    from_id: *from_id,
                    cargs: cargs.to_vec(),
                    to: narrowed,
                    to_id,
                });
            }
        }
        out
    }

    /// The arguments of a call to `f` once [`Monomorphizer::narrowed`] made
    /// `changes` to its type arguments: at each parameter typed by a narrowed
    /// one, the narrowed context built from the caller's.
    pub(super) fn narrowed_args(
        &mut self,
        f: FnId,
        changes: &[Changed],
        mut args: Vec<typed::Expr>,
    ) -> Vec<typed::Expr> {
        let params = self.tables().fn_info(f).params.clone();
        for (arg, p) in args.iter_mut().zip(&params) {
            let Some(Changed {
                from: from_ty,
                from_id,
                cargs,
                to: to_ty,
                to_id,
                ..
            }) = changes.iter().find(|c| c.param == p.ty)
            else {
                continue;
            };
            let span = arg.span;
            let value = std::mem::replace(arg, typed::Expr::new(ExprKind::Unit, Ty::UNIT, span));
            let (local, bound) = match value.kind {
                ExprKind::Local(l) => (l, None),
                _ => {
                    let l = self.new_local("ctx", *from_ty, span);
                    let pattern = typed::Pattern {
                        kind: PatKind::Bind {
                            local: l,
                            sub: None,
                        },
                        ty: *from_ty,
                        span,
                    };
                    (
                        l,
                        Some(Stmt::Let {
                            pattern,
                            value,
                            span,
                        }),
                    )
                }
            };
            let tables = self.tables();
            let to_ctx = tables.ctx_type(*to_id);
            let from_ctx = tables.ctx_type(*from_id);
            let bindings: Vec<(TraitId, typed::Expr)> = to_ctx
                .bindings
                .iter()
                .map(|(e, _)| {
                    let (field, impl_ty) = from_ctx
                        .binding(*e)
                        .or_ice("a narrowed context's effects are ones its caller's binds");
                    let base = typed::Expr::new(ExprKind::Local(local), *from_ty, span);
                    let get = ExprKind::CtxGet {
                        base: Box::new(base),
                        trait_id: *e,
                        field,
                    };
                    (*e, typed::Expr::new(get, self.sub(&impl_ty, cargs), span))
                })
                .collect();
            let lit = typed::Expr::new(
                ExprKind::CtxLit {
                    bindings,
                    ctx: *to_id,
                },
                *to_ty,
                span,
            );
            *arg = match bound {
                None => lit,
                Some(stmt) => typed::Expr::new(
                    ExprKind::Block {
                        stmts: vec![stmt],
                        tail: Some(Box::new(lit)),
                    },
                    *to_ty,
                    span,
                ),
            };
        }
        args
    }

    /// Per type parameter of `f`, the narrowed context of its bounds where it
    /// may be narrowable: all but [`Monomorphizer::kept`]. Empty where none
    /// may be.
    fn narrowable(&self, f: FnId) -> Box<[Option<CtxTypeId>]> {
        if self.extracted.bodies.get(&f).is_none() {
            return Box::new([]);
        }
        let tables = self.tables();
        let info = tables.fn_info(f);
        let slots: Box<[Option<CtxTypeId>]> = (info.generics.iter().enumerate())
            .map(|(j, generic)| {
                let bounds = &generic.bounds;
                let effects_only =
                    !bounds.is_empty() && bounds.iter().all(|b| tables.trait_(*b).is_effect);
                if !(effects_only && takes_whole(info, j)) {
                    return None;
                }
                let to = if bounds.is_sorted_by(|a, b| a < b) {
                    self.narrowed_ctx.get(bounds.as_slice())
                } else {
                    let mut effects = bounds.clone();
                    effects.sort_unstable();
                    effects.dedup();
                    self.narrowed_ctx.get(&effects)
                };
                to.copied()
            })
            .collect();
        if slots.iter().all(Option::is_none) {
            return Box::new([]);
        }
        slots
    }

    /// Whether `f`'s type parameter `j` keeps every type built from it away
    /// from the runtime: whether no function rules out a parameter that
    /// `(f, j)` hands such a type on to, directly or not. The walk stops at the
    /// first one ruled out.
    fn kept(&mut self, f: FnId, j: usize) -> bool {
        if let Some(known) = self.ctx_kept.get(&(f, j)) {
            return *known;
        }
        let mut seen: crate::hash::Set<(FnId, usize)> = Default::default();
        let mut stack = vec![(f, j)];
        let mut kept = true;
        while let Some((g, t)) = stack.pop() {
            if !seen.insert((g, t)) {
                continue;
            }
            match self.ctx_kept.get(&(g, t)) {
                Some(true) => continue,
                Some(false) => kept = false,
                None => {}
            }
            let facts = self.kept_facts(g);
            if !kept || facts.ok.get(t) == Some(&false) {
                kept = false;
                break;
            }
            stack.extend(facts.deps.get(t).into_iter().flatten());
            for (trait_id, method, mt) in facts.methods.get(t).into_iter().flatten() {
                let all = self.impl_methods(*trait_id, *method);
                stack.extend(
                    all.iter()
                        .map(|(h, offset)| (*h, offset.saturating_add(*mt))),
                );
            }
        }
        // Every parameter a kept one reaches is kept too.
        if kept {
            self.ctx_kept
                .extend(seen.into_iter().map(|key| (key, true)));
        } else {
            self.ctx_kept.insert((f, j), false);
        }
        kept
    }

    /// What `f`'s declaration and body say on their own, worked out once.
    fn kept_facts(&mut self, f: FnId) -> std::rc::Rc<Facts> {
        if let Some(known) = self.kept_facts.get(&f) {
            return known.clone();
        }
        let info = self.tables().fn_info(f);
        let n = info.generics.len();
        let facts = match self.extracted.bodies.get(&f) {
            None => {
                let module = self.module_paths.get(info.module.index());
                let value = module.is_some_and(|m| VALUE_MODULES.contains(&m.as_str()));
                let ok = (0..n).map(|j| value || takes_whole(info, j));
                Facts {
                    ok: ok.collect(),
                    deps: vec![Vec::new(); n],
                    methods: vec![Vec::new(); n],
                }
            }
            Some(body) => {
                let mut facts = Facts {
                    ok: vec![true; n],
                    deps: vec![Vec::new(); n],
                    methods: vec![Vec::new(); n],
                };
                handed(&body.expr, &mut facts);
                facts
            }
        };
        let facts = std::rc::Rc::new(facts);
        self.kept_facts.insert(f, facts.clone());
        facts
    }

    /// The function supplying `method` in each written `impl` of the trait,
    /// with the number of the `impl`'s own type parameters, which come before
    /// the method's.
    fn impl_methods(&mut self, trait_id: TraitId, method: MethodId) -> Impls {
        if let Some(known) = self.impl_methods.get(&(trait_id, method)) {
            return known.clone();
        }
        let tables = self.tables();
        let all: Impls = tables
            .impls
            .iter()
            .filter(|((t, _), _)| *t == trait_id)
            .filter_map(|(_, imp)| Some((imp.method(method)?, imp.generics.len())))
            .collect();
        self.impl_methods.insert((trait_id, method), all.clone());
        all
    }
}

/// One context [`Monomorphizer::narrowed`] narrowed, at the type parameter
/// `param`.
pub(super) struct Changed {
    param: Ty,
    from: Ty,
    from_id: CtxTypeId,
    cargs: Vec<Ty>,
    to: Ty,
    to_id: CtxTypeId,
}

/// Type parameter `j`.
fn param(j: usize) -> Ty {
    Ty::param(u32::try_from(j).or_ice("a type parameter's index fits"))
}

/// Whether `f` takes values of its type parameter `j` only whole: its result
/// doesn't mention `j`, and each parameter that does is typed exactly `j`.
fn takes_whole(f: &FnInfo, j: usize) -> bool {
    let whole = param(j);
    !mentions(&f.ret, j)
        && f.params
            .iter()
            .all(|p| !mentions(&p.ty, j) || p.ty == whole)
}

/// Per written `impl` of a trait, the function supplying one of its methods,
/// with the number of the `impl`'s own type parameters.
pub(super) type Impls = std::rc::Rc<[(FnId, usize)]>;

/// The modules whose runtime operations only compute a value from their
/// arguments, and keep nothing past the call.
const VALUE_MODULES: [&str; 8] = [
    "core/list",
    "core/map",
    "core/option",
    "core/orderedmap",
    "core/orderedset",
    "core/result",
    "core/set",
    "core/str",
];

/// What one function's own body says about each of its type parameters.
pub(super) struct Facts {
    /// Whether nothing local rules the parameter out.
    ok: Vec<bool>,
    /// Per parameter, the callee type parameters handed a type built from it.
    deps: Vec<Vec<(FnId, usize)>>,
    /// Per parameter, the trait methods handed a type built from it, as
    /// `(trait, method, the method's type parameter)`. Their callees are
    /// known only per `impl`.
    methods: Vec<Vec<(TraitId, MethodId, usize)>>,
}

/// Records each callee type parameter `e` hands a type built from a parameter
/// to, and rules out a parameter a runtime operation might keep.
fn handed(e: &typed::Expr, facts: &mut Facts) {
    match &e.kind {
        ExprKind::CallFn { func, .. } | ExprKind::FnRef(func) => {
            for (t, targ) in func.targs.iter().enumerate() {
                for (j, deps) in facts.deps.iter_mut().enumerate() {
                    if mentions(targ, j) {
                        deps.push((func.id, t));
                    }
                }
            }
        }
        // An effect call on the context reaches the implementation it binds,
        // the same one at the narrowed context.
        ExprKind::CallTrait {
            trait_id,
            method,
            recv,
            targs,
            ..
        } => {
            for (j, (ok, methods)) in facts.ok.iter_mut().zip(&mut facts.methods).enumerate() {
                *ok &= !mentions(recv, j) || *recv == param(j);
                for (t, targ) in targs.iter().enumerate() {
                    if mentions(targ, j) {
                        methods.push((*trait_id, *method, t));
                    }
                }
            }
        }
        _ => {}
    }
    typed::children(e, &mut |c| handed(c, facts));
}

/// Each narrowed context ([`CtxType::narrowed`]) in `tables`, by its effects.
pub(super) fn narrowed_contexts(tables: &Tables) -> crate::hash::Map<Vec<TraitId>, CtxTypeId> {
    let entries = tables.ctx_types.entries();
    entries
        .filter_map(|(id, c)| c.narrowed().map(|effects| (effects, id)))
        .collect()
}
