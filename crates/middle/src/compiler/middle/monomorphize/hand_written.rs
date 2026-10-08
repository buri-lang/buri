//! A derived `Show`, `Equal`, `Ordered` or `Hash` whose shape reaches a
//! hand-written one.
//!
//! A derive is a structural intrinsic over a type descriptor, and a descriptor
//! knows shapes, not `impl`s. So a field whose type has its own `impl Show` was
//! printed field by field, and a secret type's `"***"` never ran (#258).
//!
//! Where a derive reaches a hand-written `impl` of the same trait, this
//! generates the derive as an ordinary function instead, one level of the type
//! at a time. Each component is a trait call: a hand-written one calls the
//! `impl`, and anything else goes back through `structural_call`, so the parts
//! of the shape that are derived all the way down keep their intrinsic. Every
//! backend compiles the result as it compiles any other function.
//!
//! Two places read a value whose `T` carries no bound at all: the test report,
//! which prints it, and a signal's write, which compares it. Each goes through
//! the same generated functions, walking a type with no `impl` as well, so a
//! secret inside a struct with no `Show` still prints `***` in a failure.
//!
//! The renderings and orders are the intrinsic's: `Name { f: .. }`, variants in
//! declaration order, `None` before `Some`. A hash only has to agree with
//! itself, so it hashes a tuple of the components with each hand-written one
//! replaced by its own hash.

use super::{Key, Monomorphizer};
use crate::compiler::middle::derives::{EXPECTED_SHOWN, REPORT_SHOWN};
use crate::compiler::semantics::typed::{
    self, ArrayRest, ExprKind, FieldPat, PatKind, Pattern, TemplatePart,
};
use crate::compiler::semantics::types::*;
use crate::diagnostics::Span;

type Expr = typed::Expr;

/// The four traits this covers. `ToJson` and `FromJson` cannot be written by
/// hand, and the operator traits derive only on a primitive.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Show,
    Equal,
    Compare,
    Hash,
}

impl Op {
    fn of(name: &str) -> Option<Op> {
        match name {
            "Show" => Some(Op::Show),
            "Equal" => Some(Op::Equal),
            "Ordered" => Some(Op::Compare),
            "Hash" => Some(Op::Hash),
            _ => None,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Op::Show => "show",
            Op::Equal => "equal",
            Op::Compare => "compare",
            Op::Hash => "hash",
        }
    }

    fn binary(self) -> bool {
        matches!(self, Op::Equal | Op::Compare)
    }
}

/// `Order`'s variants, in its declaration order.
const LESS: usize = 0;
const EQUAL: usize = 1;
const GREATER: usize = 2;

/// One generated function being built.
struct Deriving {
    trait_id: TraitId,
    op: Op,
    ret: Ty,
    /// `Show`'s context, which every component's `show` is handed.
    ctx: Option<(LocalId, Ty)>,
    /// Walks a component with no `impl` of the trait to the hand-written ones
    /// inside it, for a `T` with no bound: the test report's `Show` and a
    /// signal's `Equal`. A derive's components all have an `impl`.
    unbounded: bool,
}

/// One component of a struct, tuple or variant.
struct Part {
    name: String,
    ty: Ty,
}

fn local(id: LocalId, ty: Ty) -> Expr {
    Expr::new(ExprKind::Local(id), ty, Span::NONE)
}

fn bind(id: LocalId, ty: Ty) -> Pattern {
    Pattern { kind: PatKind::Bind { local: id, sub: None }, ty, span: Span::NONE }
}

fn wild(ty: Ty) -> Pattern {
    Pattern { kind: PatKind::Wild, ty, span: Span::NONE }
}

fn arm(pattern: Pattern, body: Expr) -> typed::Arm {
    typed::Arm { pattern, guard: None, body, span: Span::NONE }
}

fn match_(scrutinee: Expr, arms: Vec<typed::Arm>, ty: Ty) -> Expr {
    Expr::new(ExprKind::Match { scrutinee: Box::new(scrutinee), arms }, ty, Span::NONE)
}

fn call(slot: usize, args: Vec<Expr>, ret: Ty) -> Expr {
    Expr::new(
        ExprKind::CallFn { func: typed::Callee::Func(FuncIdx(slot as u32)), args },
        ret,
        Span::NONE,
    )
}

fn project(base: Expr, index: usize, tuple: bool, ty: Ty) -> Expr {
    let kind = if tuple {
        ExprKind::TupleIndex { base: Box::new(base), index }
    } else {
        ExprKind::Field { base: Box::new(base), index }
    };
    Expr::new(kind, ty, Span::NONE)
}

fn array_pattern(ty: Ty, elems: Vec<Pattern>, rest: ArrayRest) -> Pattern {
    Pattern { kind: PatKind::Array { elems, rest }, ty, span: Span::NONE }
}

impl Monomorphizer<'_> {
    /// The derive at `recv`, as a call to a generated function, where its shape
    /// reaches a hand-written `impl` of the trait. `None` where it does not, and
    /// the structural intrinsic is the whole answer.
    pub(super) fn hand_written_derive(
        &mut self,
        trait_id: TraitId,
        recv: &Ty,
        args: &[Expr],
    ) -> Option<ExprKind> {
        let op = Op::of(&self.tables().trait_(trait_id).name)?;
        if !self.reaches_hand_written(trait_id, recv, false) {
            return None;
        }
        let ctx = if op == Op::Show { Some(args.get(1)?.ty) } else { None };
        let slot = self.request(Key::Derived { trait_id, ty: *recv, ctx, unbounded: false });
        Some(ExprKind::CallFn {
            func: typed::Callee::Func(FuncIdx(slot as u32)),
            args: args.to_vec(),
        })
    }

    /// `trait_name`'s method at `ty`, for a `T` that carries no bound on it:
    /// the type's own `impl` where that is written by hand, and otherwise a walk
    /// of its shape that calls each hand-written one it reaches. `None` where
    /// it reaches none, and walking by shape is the whole answer.
    ///
    /// `ctx` is `Show`'s context type. The test report and a signal's write are
    /// the two callers: neither may require the trait, so a type with no `impl`
    /// is walked as it always was. Answers the generated function.
    fn unbounded_fn(&mut self, trait_name: &str, ty: Ty, ctx: Option<Ty>) -> Option<FuncIdx> {
        let module_paths = &self.module_paths;
        let trait_id = self
            .tables()
            .traits
            .iter()
            .position(|t| {
                t.name == trait_name
                    && module_paths.get(t.module.index()).is_some_and(|m| m == "core/order")
            })
            .map(|i| TraitId(i as u32))?;
        if !self.hand_written(trait_id, &ty) && !self.reaches_hand_written(trait_id, &ty, true) {
            return None;
        }
        // A function has no shape to walk, so a value holding one keeps the
        // comparison and rendering it had.
        if self.meets_opaque(trait_id, &ty, &mut Vec::new()) {
            return None;
        }
        let slot = self.request(Key::Derived { trait_id, ty, ctx, unbounded: true });
        Some(FuncIdx(slot as u32))
    }

    /// Gives `testing_assert.report` or `failExpected` at `slot` a body that
    /// renders its values through `Show`, where their type reaches a
    /// hand-written one. A secret's `"***"` is then what a failure prints.
    ///
    /// The body is the one `middle::derives` gives the intrinsic natively, so
    /// every backend lowers it the same way. `Show` is handed a `Scope`, the
    /// allocator that charges nothing, because the report has no context.
    pub(super) fn report_through_show(&mut self, slot: usize, key: &str, params: &[Ty]) -> bool {
        // `report(passed, kind, actual, expected)` and `failExpected(kind, got)`.
        let (kind_at, values, shown_key): (u32, &[u32], &str) = match key {
            "testing_assert.report" => (1, &[2, 3], REPORT_SHOWN),
            "testing_assert.failExpected" => (0, &[1], EXPECTED_SHOWN),
            _ => return false,
        };
        let Some(scope) = self.checked.known_types.get("Scope").copied() else { return false };
        let (Some(str_ty), Some(ty)) = (
            params.get(kind_at as usize).copied(),
            values.first().and_then(|i| params.get(*i as usize)).copied(),
        ) else {
            return false;
        };
        let scope_ty = Ty::con(scope, []);
        let Some(show) = self.unbounded_fn("Show", ty, Some(scope_ty)) else { return false };
        let ctx = Expr::new(
            ExprKind::StructLit { con: scope, targs: Vec::new(), fields: vec![self.int_lit(0)] },
            scope_ty,
            Span::NONE,
        );
        let mut args = vec![local(LocalId(kind_at), str_ty)];
        args.extend(
            values.iter().map(|i| call(show.index(), vec![local(LocalId(*i), ty), ctx.clone()], str_ty)),
        );
        let ret = self.func_mut(slot).ret;
        let shown = Expr::new(
            ExprKind::Intrinsic { name: shown_key.into(), targs: Vec::new(), args },
            ret,
            Span::NONE,
        );
        // `report` renders only on the branch that fails.
        let body = if kind_at == 1 {
            Expr::new(
                ExprKind::If {
                    cond: Box::new(local(LocalId(0), self.tables().prim(Prim::Bool))),
                    then: Box::new(Expr::new(ExprKind::Unit, ret, Span::NONE)),
                    else_: Box::new(shown),
                },
                ret,
                Span::NONE,
            )
        } else {
            shown
        };
        self.func_mut(slot).set_body(body);
        true
    }

    /// Records the comparison a reactive cell of `ty` is written with, where
    /// `ty` reaches a hand-written `Equal`. A cell whose type reaches none
    /// compares by shape, as it always has.
    pub(super) fn cell_through_equal(&mut self, ty: Ty) {
        if self.cell_equal.contains_key(&ty) {
            return;
        }
        if let Some(f) = self.unbounded_fn("Equal", ty, None) {
            self.cell_equal.insert(ty, f);
        }
    }

    /// Whether `ty`'s own `impl` of the trait is written by hand, and applies
    /// at `ty`. `impl<K, V: Show> Show for OrderedMap<K, V>` is no `Show` for
    /// a map whose values have none, so that map is walked like a type with
    /// no `impl` at all (#269).
    fn hand_written(&self, trait_id: TraitId, ty: &Ty) -> bool {
        let TyKind::Con(con, _) = ty.kind() else { return false };
        self.tables().as_prim(ty).is_none()
            && self
                .tables()
                .impls
                .get(&(trait_id, *con))
                .is_some_and(|i| !i.is_derived() && self.bounds_hold(i, ty, &mut Vec::new()))
    }

    /// Whether a hand-written `impl`'s own bounds hold at `ty`.
    fn bounds_hold(&self, imp: &ImplInfo, ty: &Ty, seen: &mut Vec<TyConId>) -> bool {
        if imp.generics.iter().all(|g| g.bounds.is_empty()) {
            return true;
        }
        let mut bound = vec![None; imp.generics.len()];
        if !super::match_head(&imp.head, ty, &mut bound) {
            return false;
        }
        imp.generics.iter().zip(bound).all(|(g, arg)| {
            arg.is_none_or(|arg| g.bounds.iter().all(|b| self.satisfies(*b, &arg, seen)))
        })
    }

    /// Whether the concrete `ty` has the trait, as checking decides it: a
    /// derive where its components do, and a hand-written `impl` where its
    /// bounds hold. `seen` stops a recursive derive, as it does in checking.
    fn satisfies(&self, trait_id: TraitId, ty: &Ty, seen: &mut Vec<TyConId>) -> bool {
        let structural = Op::of(&self.tables().trait_(trait_id).name).is_some();
        match ty.kind() {
            TyKind::Unit => structural,
            TyKind::Array(e) => structural && self.satisfies(trait_id, e, seen),
            TyKind::Tuple(es) => structural && es.iter().all(|e| self.satisfies(trait_id, e, seen)),
            TyKind::Ctx(id) => self.tables().ctx_type(*id).has(trait_id),
            TyKind::Con(con, _) => match self.tables().impls.get(&(trait_id, *con)) {
                None => false,
                Some(imp) if !imp.is_derived() => self.bounds_hold(imp, ty, seen),
                Some(_) if seen.contains(con) || self.tables().as_prim(ty).is_some() => true,
                Some(_) => {
                    seen.push(*con);
                    let parts = self.parts_of(ty);
                    let ok = parts.iter().flatten().all(|p| self.satisfies(trait_id, &p.ty, seen));
                    seen.pop();
                    ok
                }
            },
            TyKind::Fn(..) => false,
            _ => true,
        }
    }

    /// `unbounded` walks through a type with no `impl` of the trait, as well as
    /// through a derived one.
    fn reaches_hand_written(&mut self, trait_id: TraitId, ty: &Ty, unbounded: bool) -> bool {
        if let Some(known) = self.reaches.get(&(trait_id, *ty, unbounded)) {
            return *known;
        }
        let answer = self.reaches_from(trait_id, ty, unbounded, &mut Vec::new());
        self.reaches.insert((trait_id, *ty, unbounded), answer);
        answer
    }

    /// `seen` stops a recursive type: a cycle reaches nothing the rest of the
    /// type does not.
    fn reaches_from(&self, trait_id: TraitId, ty: &Ty, unbounded: bool, seen: &mut Vec<Ty>) -> bool {
        match ty.kind() {
            TyKind::Array(e) => self.reaches_from(trait_id, e, unbounded, seen),
            TyKind::Tuple(es) => es.iter().any(|e| self.reaches_from(trait_id, e, unbounded, seen)),
            TyKind::Con(con, _) => {
                if self.tables().as_prim(ty).is_some() || seen.contains(ty) {
                    return false;
                }
                match self.tables().impls.get(&(trait_id, *con)) {
                    Some(imp) if !imp.is_derived() => {
                        self.hand_written(trait_id, ty)
                            || (unbounded && self.parts_reach(trait_id, ty, unbounded, seen))
                    }
                    Some(_) => self.parts_reach(trait_id, ty, unbounded, seen),
                    None if unbounded => self.parts_reach(trait_id, ty, unbounded, seen),
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// Whether walking `ty` meets a value with no shape, such as a function,
    /// before a hand-written `impl` of the trait answers for it.
    fn meets_opaque(&self, trait_id: TraitId, ty: &Ty, seen: &mut Vec<Ty>) -> bool {
        match ty.kind() {
            TyKind::Unit => false,
            TyKind::Array(e) => self.meets_opaque(trait_id, e, seen),
            TyKind::Tuple(es) => es.iter().any(|e| self.meets_opaque(trait_id, e, seen)),
            TyKind::Con(..) => {
                if self.tables().as_prim(ty).is_some()
                    || seen.contains(ty)
                    || self.hand_written(trait_id, ty)
                {
                    return false;
                }
                seen.push(*ty);
                let parts = self.parts_of(ty);
                let found = parts.iter().flatten().any(|p| self.meets_opaque(trait_id, &p.ty, seen));
                seen.pop();
                found
            }
            _ => true,
        }
    }

    fn parts_reach(&self, trait_id: TraitId, ty: &Ty, unbounded: bool, seen: &mut Vec<Ty>) -> bool {
        seen.push(*ty);
        let parts = self.parts_of(ty);
        let found = parts.iter().flatten().any(|p| self.reaches_from(trait_id, &p.ty, unbounded, seen));
        seen.pop();
        found
    }

    /// A struct's fields as one list, or an enum's variants' payloads as one
    /// list each, with the type's arguments substituted in.
    fn parts_of(&self, ty: &Ty) -> Vec<Vec<Part>> {
        let TyKind::Con(con, args) = ty.kind() else { return Vec::new() };
        let part = |name: &str, t: &Ty| Part { name: name.to_string(), ty: self.sub(t, args) };
        match &self.tables().tycon(*con).def {
            TyDef::Struct { fields, .. } => {
                vec![fields.iter().map(|f| part(&f.name, &f.ty)).collect()]
            }
            TyDef::Enum { variants } => variants
                .iter()
                .map(|v| v.fields.iter().map(|f| part(&f.name, &f.ty)).collect())
                .collect(),
            TyDef::Prim(_) => Vec::new(),
        }
    }

    pub(super) fn derived_name(
        &mut self,
        trait_id: TraitId,
        ty: Ty,
        ctx: Option<Ty>,
        items: bool,
        unbounded: bool,
    ) -> (String, String, Span) {
        let name = self.tables().trait_(trait_id).name.clone();
        let tag = Op::of(&name).map_or("derive", Op::tag);
        let base = format!(
            "$derived${tag}{}{}",
            if items { "$items" } else { "" },
            if unbounded { "$unbounded" } else { "" },
        );
        let mut targs = vec![ty];
        targs.extend(ctx);
        let symbol = self.instantiation(&base, &targs);
        let module = self.declaring_module(&ty).unwrap_or_else(|| "core".into());
        let owner = ty.head().map_or("[]".to_string(), |c| self.tables().tycon(c).name.clone());
        (symbol, format!("{module}:{owner}.{tag}"), Span::NONE)
    }

    fn deriving(&mut self, trait_id: TraitId, ctx: Option<Ty>, unbounded: bool) -> Option<Deriving> {
        let op = Op::of(&self.tables().trait_(trait_id).name)?;
        let ret = match op {
            Op::Show => self.tables().prim(Prim::Str),
            Op::Equal => self.tables().prim(Prim::Bool),
            Op::Hash => self.tables().prim(Prim::U64),
            Op::Compare => Ty::con(*self.checked.known_types.get("Order")?, []),
        };
        let ctx = ctx.map(|c| (self.new_local("ctx", c, Span::NONE), c));
        Some(Deriving { trait_id, op, ret, ctx, unbounded })
    }

    fn finish(&mut self, slot: usize, params: Vec<LocalId>, ret: Ty, body: Expr) {
        let locals = std::mem::take(&mut self.locals);
        let f = self.func_mut(slot);
        f.params = params;
        f.locals = locals;
        f.ret = ret;
        f.set_body(body);
    }

    /// `Key::Derived`: the derive at `ty`, one level deep.
    pub(super) fn build_derived(
        &mut self,
        trait_id: TraitId,
        ty: Ty,
        ctx: Option<Ty>,
        unbounded: bool,
        slot: usize,
    ) {
        self.locals = Vec::new();
        let x = self.new_local("x", ty, Span::NONE);
        let Some(d) = self.deriving(trait_id, ctx, unbounded) else { return };
        let y = d.op.binary().then(|| self.new_local("y", ty, Span::NONE));
        let mut params = vec![x];
        params.extend(y);
        params.extend(d.ctx.map(|(c, _)| c));
        let (a, b) = (local(x, ty), y.map(|y| local(y, ty)));
        let body = match ty.kind() {
            // At a hand-written type the `impl` is the whole answer, wrapped so
            // a caller that needs a function, such as a signal, has one.
            _ if self.hand_written(trait_id, &ty) => {
                let mut args = vec![a];
                args.extend(b);
                self.component(&d, ty, args)
            }
            TyKind::Array(e) => self.list(&d, *e, a, b, slot),
            TyKind::Tuple(es) => {
                let parts: Vec<Part> =
                    es.iter().map(|t| Part { name: String::new(), ty: *t }).collect();
                self.record(&d, "", false, true, &parts, a, b)
            }
            _ => self.nominal(&d, ty, a, b),
        };
        let ret = d.ret;
        self.finish(slot, params, ret, body);
    }

    /// `Key::DerivedItems`: the rest of a list, after its first element, with
    /// what `Show` or `Hash` has made of the elements so far.
    pub(super) fn build_derived_items(
        &mut self,
        trait_id: TraitId,
        elem: Ty,
        ctx: Option<Ty>,
        unbounded: bool,
        slot: usize,
    ) {
        self.locals = Vec::new();
        let list = Ty::array(elem);
        let xs = self.new_local("xs", list, Span::NONE);
        let Some(d) = self.deriving(trait_id, ctx, unbounded) else { return };
        let acc = self.new_local("acc", d.ret, Span::NONE);
        let mut params = vec![xs, acc];
        params.extend(d.ctx.map(|(c, _)| c));
        let h = self.new_local("h", elem, Span::NONE);
        let t = self.new_local("t", list, Span::NONE);
        let next = match d.op {
            Op::Show => {
                let shown = self.component(&d, elem, vec![local(h, elem)]);
                self.joined(vec![
                    TemplatePart::Hole(local(acc, d.ret)),
                    TemplatePart::Text(", ".into()),
                    TemplatePart::Hole(shown),
                ])
            }
            _ => {
                let proxy = self.hash_proxy(&d, elem, local(h, elem));
                self.hash_of(&d, vec![local(acc, d.ret), proxy])
            }
        };
        let mut again = vec![local(t, list), next];
        again.extend(d.ctx.map(|(c, ct)| local(c, ct)));
        let ret = d.ret;
        let body = match_(
            local(xs, list),
            vec![
                arm(array_pattern(list, Vec::new(), ArrayRest::None), local(acc, ret)),
                arm(
                    array_pattern(list, vec![bind(h, elem)], ArrayRest::Bound(t)),
                    call(slot, again, ret),
                ),
            ],
            ret,
        );
        self.finish(slot, params, ret, body);
    }

    /// One component's derive: its own `impl` where that is written by hand,
    /// and the derive at its type otherwise.
    fn component(&mut self, d: &Deriving, ty: Ty, mut args: Vec<Expr>) -> Expr {
        let mut targs = Vec::new();
        if let Some((c, ct)) = d.ctx {
            args.push(local(c, ct));
            targs.push(ct);
        }
        let kind = if self.hand_written(d.trait_id, &ty) {
            self.resolve_trait_call(d.trait_id, 0, ty, targs, args, Span::NONE)
        } else if d.unbounded && self.reaches_hand_written(d.trait_id, &ty, true) {
            let ctx = d.ctx.map(|(_, ct)| ct);
            let slot = self.request(Key::Derived { trait_id: d.trait_id, ty, ctx, unbounded: true });
            ExprKind::CallFn { func: typed::Callee::Func(FuncIdx(slot as u32)), args }
        } else {
            self.structural_call(d.trait_id, 0, &ty, args, Span::NONE)
        };
        Expr::new(kind, d.ret, Span::NONE)
    }

    fn str_lit(&self, s: &str) -> Expr {
        Expr::new(ExprKind::Str(s.into()), self.tables().prim(Prim::Str), Span::NONE)
    }

    fn bool_lit(&self, b: bool) -> Expr {
        Expr::new(ExprKind::Bool(b), self.tables().prim(Prim::Bool), Span::NONE)
    }

    fn int_lit(&self, n: usize) -> Expr {
        Expr::new(
            ExprKind::Int(typed::Magnitude::new(n as u128), false),
            self.tables().prim(Prim::I64),
            Span::NONE,
        )
    }

    fn order_lit(&self, d: &Deriving, variant: usize) -> Expr {
        let con = d.ret.head().unwrap_or(TyConId(0));
        Expr::new(
            ExprKind::EnumLit { con, targs: Vec::new(), variant, args: Vec::new() },
            d.ret,
            Span::NONE,
        )
    }

    /// Strings joined. Every hole is already a `Str`, so this is concatenation.
    fn joined(&self, parts: Vec<TemplatePart>) -> Expr {
        Expr::new(ExprKind::Template { parts }, self.tables().prim(Prim::Str), Span::NONE)
    }

    /// `[]` and `[h, ..t]`: `Equal` and `Ordered` recurse on the tails here,
    /// and `Show` and `Hash` hand the tail to `DerivedItems`.
    fn list(&mut self, d: &Deriving, elem: Ty, a: Expr, b: Option<Expr>, slot: usize) -> Expr {
        let list = Ty::array(elem);
        let empty = || array_pattern(list, Vec::new(), ArrayRest::None);
        let ret = d.ret;
        let h = self.new_local("h", elem, Span::NONE);
        let t = self.new_local("t", list, Span::NONE);
        let cons = |h, t| array_pattern(list, vec![bind(h, elem)], ArrayRest::Bound(t));
        match (d.op, b) {
            (Op::Equal | Op::Compare, Some(b)) => {
                let k = self.new_local("k", elem, Span::NONE);
                let u = self.new_local("u", list, Span::NONE);
                let first = self.component(d, elem, vec![local(h, elem), local(k, elem)]);
                let rest = call(slot, vec![local(t, list), local(u, list)], ret);
                let (both_empty, only_a_empty, only_b_empty, step) = if d.op == Op::Equal {
                    let step = Expr::new(
                        ExprKind::If {
                            cond: Box::new(first),
                            then: Box::new(rest),
                            else_: Box::new(self.bool_lit(false)),
                        },
                        ret,
                        Span::NONE,
                    );
                    (self.bool_lit(true), self.bool_lit(false), self.bool_lit(false), step)
                } else {
                    let step = self.then(d, first, rest);
                    (self.order_lit(d, EQUAL), self.order_lit(d, LESS), self.order_lit(d, GREATER), step)
                };
                let on_empty = match_(
                    b.clone(),
                    vec![arm(empty(), both_empty), arm(wild(list), only_a_empty)],
                    ret,
                );
                let on_cons = match_(
                    b,
                    vec![arm(empty(), only_b_empty), arm(cons(k, u), step)],
                    ret,
                );
                match_(a, vec![arm(empty(), on_empty), arm(cons(h, t), on_cons)], ret)
            }
            (Op::Show, _) => {
                let first = self.component(d, elem, vec![local(h, elem)]);
                let items = self.request(Key::DerivedItems {
                    trait_id: d.trait_id,
                    elem,
                    ctx: d.ctx.map(|(_, ct)| ct),
                    unbounded: d.unbounded,
                });
                let mut args = vec![local(t, list), first];
                args.extend(d.ctx.map(|(c, ct)| local(c, ct)));
                let body = self.joined(vec![
                    TemplatePart::Text("[".into()),
                    TemplatePart::Hole(call(items, args, ret)),
                    TemplatePart::Text("]".into()),
                ]);
                match_(a, vec![arm(empty(), self.str_lit("[]")), arm(cons(h, t), body)], ret)
            }
            _ => {
                let items = self.request(Key::DerivedItems {
                    trait_id: d.trait_id,
                    elem,
                    ctx: None,
                    unbounded: d.unbounded,
                });
                let seed = self.hash_of(d, vec![self.int_lit(0)]);
                call(items, vec![a, seed], ret)
            }
        }
    }

    /// `match (c) { .Equal => rest, c => c }`: the first difference decides.
    fn then(&mut self, d: &Deriving, c: Expr, rest: Expr) -> Expr {
        let con = d.ret.head().unwrap_or(TyConId(0));
        let held = self.new_local("c", d.ret, Span::NONE);
        let equal = Pattern {
            kind: PatKind::Variant { con, variant: EQUAL, fields: Vec::new() },
            ty: d.ret,
            span: Span::NONE,
        };
        match_(c, vec![arm(equal, rest), arm(bind(held, d.ret), local(held, d.ret))], d.ret)
    }

    /// A struct's or a tuple's components, or one variant's payload already
    /// bound: `a` and `b` are the values, or `None` where `binds` holds them.
    #[allow(clippy::too_many_arguments, reason = "one call per shape, each naming what it renders")]
    fn record(
        &mut self,
        d: &Deriving,
        name: &str,
        record: bool,
        tuple: bool,
        parts: &[Part],
        a: Expr,
        b: Option<Expr>,
    ) -> Expr {
        let a_parts: Vec<Expr> =
            parts.iter().enumerate().map(|(i, p)| project(a.clone(), i, tuple, p.ty)).collect();
        let b_parts: Option<Vec<Expr>> = b.map(|b| {
            parts.iter().enumerate().map(|(i, p)| project(b.clone(), i, tuple, p.ty)).collect()
        });
        self.combine(d, name, record, tuple, parts, a_parts, b_parts)
    }

    /// The derive over components already in hand.
    #[allow(clippy::too_many_arguments, reason = "one call per shape, each naming what it renders")]
    fn combine(
        &mut self,
        d: &Deriving,
        name: &str,
        record: bool,
        tuple: bool,
        parts: &[Part],
        a: Vec<Expr>,
        b: Option<Vec<Expr>>,
    ) -> Expr {
        match d.op {
            Op::Show => {
                let (open, close) = if tuple {
                    ("(".to_string(), ")")
                } else if record {
                    (format!("{name} {{ "), " }")
                } else {
                    (format!("{name}("), ")")
                };
                let mut out = vec![TemplatePart::Text(open)];
                for (k, (p, x)) in parts.iter().zip(a).enumerate() {
                    if k > 0 {
                        out.push(TemplatePart::Text(", ".into()));
                    }
                    if record && !tuple {
                        out.push(TemplatePart::Text(format!("{}: ", p.name)));
                    }
                    out.push(TemplatePart::Hole(self.component(d, p.ty, vec![x])));
                }
                out.push(TemplatePart::Text(close.into()));
                self.joined(out)
            }
            Op::Equal => {
                let b = b.unwrap_or_default();
                let mut each: Vec<Expr> = parts
                    .iter()
                    .zip(a.into_iter().zip(b))
                    .map(|(p, (x, y))| self.component(d, p.ty, vec![x, y]))
                    .collect();
                let Some(mut acc) = each.pop() else { return self.bool_lit(true) };
                while let Some(x) = each.pop() {
                    acc = Expr::new(
                        ExprKind::And { lhs: Box::new(x), rhs: Box::new(acc) },
                        d.ret,
                        Span::NONE,
                    );
                }
                acc
            }
            Op::Compare => {
                let b = b.unwrap_or_default();
                let mut each: Vec<Expr> = parts
                    .iter()
                    .zip(a.into_iter().zip(b))
                    .map(|(p, (x, y))| self.component(d, p.ty, vec![x, y]))
                    .collect();
                let Some(mut acc) = each.pop() else { return self.order_lit(d, EQUAL) };
                while let Some(x) = each.pop() {
                    acc = self.then(d, x, acc);
                }
                acc
            }
            Op::Hash => {
                let proxies: Vec<Expr> = parts
                    .iter()
                    .zip(a)
                    .map(|(p, x)| self.hash_proxy(d, p.ty, x))
                    .collect();
                self.hash_of(d, proxies)
            }
        }
    }

    /// What stands for a component in a hash: the component itself where the
    /// intrinsic hashes it correctly, and its own hash where it does not.
    fn hash_proxy(&mut self, d: &Deriving, ty: Ty, x: Expr) -> Expr {
        if self.hand_written(d.trait_id, &ty) || self.reaches_hand_written(d.trait_id, &ty, false) {
            self.component(d, ty, vec![x])
        } else {
            x
        }
    }

    /// The structural hash of these values as one tuple, or of the one value.
    fn hash_of(&mut self, d: &Deriving, mut values: Vec<Expr>) -> Expr {
        let value = if values.len() == 1 {
            values.remove(0)
        } else {
            let ty = Ty::tuple(values.iter().map(|v| v.ty));
            Expr::new(ExprKind::Tuple(values), ty, Span::NONE)
        };
        let ty = value.ty;
        let kind = self.structural_call(d.trait_id, 0, &ty, vec![value], Span::NONE);
        Expr::new(kind, d.ret, Span::NONE)
    }

    /// A struct or an enum.
    fn nominal(&mut self, d: &Deriving, ty: Ty, a: Expr, b: Option<Expr>) -> Expr {
        let Some(con) = ty.head() else { return Expr::new(ExprKind::Error, d.ret, Span::NONE) };
        let tycon = self.tables().tycon(con).clone();
        let parts = self.parts_of(&ty);
        match &tycon.def {
            TyDef::Struct { record, .. } => {
                let fields = parts.into_iter().next().unwrap_or_default();
                if fields.is_empty() && d.op == Op::Show {
                    let shown =
                        if *record { format!("{} {{}}", tycon.name) } else { format!("{}()", tycon.name) };
                    return self.str_lit(&shown);
                }
                self.record(d, &tycon.name, *record, false, &fields, a, b)
            }
            TyDef::Enum { variants } => {
                let option = self.tables().is_option(con);
                // `None` orders before `Some`, whatever the declaration says.
                let rank = |i: usize, name: &str| {
                    if option { usize::from(name != "None") } else { i }
                };
                let names: Vec<(String, bool)> =
                    variants.iter().map(|v| (v.name.clone(), v.record)).collect();
                let rb = match (&b, d.op) {
                    (Some(b), Op::Compare) => {
                        let rb = self.new_local("rank", self.tables().prim(Prim::I64), Span::NONE);
                        let arms = names
                            .iter()
                            .enumerate()
                            .map(|(j, (n, _))| {
                                arm(self.tag_pattern(ty, j), self.int_lit(rank(j, n)))
                            })
                            .collect();
                        let int = self.tables().prim(Prim::I64);
                        Some((rb, match_(b.clone(), arms, int)))
                    }
                    _ => None,
                };
                let mut arms = Vec::new();
                for (i, (vname, vrecord)) in names.iter().enumerate() {
                    let payload = parts.get(i).map_or(&[][..], Vec::as_slice);
                    let a_binds: Vec<LocalId> =
                        payload.iter().map(|p| self.new_local("a", p.ty, Span::NONE)).collect();
                    let a_pat = self.payload_pattern(ty, i, payload, &a_binds);
                    let a_vals: Vec<Expr> =
                        a_binds.iter().zip(payload).map(|(l, p)| local(*l, p.ty)).collect();
                    let body = match &b {
                        None => match d.op {
                            Op::Show if payload.is_empty() => self.str_lit(&format!(".{vname}")),
                            Op::Show => {
                                let label = format!(".{vname}");
                                self.combine(d, &label, *vrecord, false, payload, a_vals, None)
                            }
                            _ => {
                                let mut values = vec![self.int_lit(i)];
                                values.extend(
                                    payload
                                        .iter()
                                        .zip(a_vals)
                                        .map(|(p, x)| self.hash_proxy(d, p.ty, x)),
                                );
                                self.hash_of(d, values)
                            }
                        },
                        Some(b) => {
                            let b_binds: Vec<LocalId> = payload
                                .iter()
                                .map(|p| self.new_local("b", p.ty, Span::NONE))
                                .collect();
                            let b_pat = self.payload_pattern(ty, i, payload, &b_binds);
                            let b_vals: Vec<Expr> =
                                b_binds.iter().zip(payload).map(|(l, p)| local(*l, p.ty)).collect();
                            let same =
                                self.combine(d, vname, *vrecord, false, payload, a_vals, Some(b_vals));
                            let differ = match &rb {
                                Some((rb, _)) => {
                                    let int = self.tables().prim(Prim::I64);
                                    let bool_ty = self.tables().prim(Prim::Bool);
                                    let below = Expr::new(
                                        ExprKind::Prim {
                                            op: typed::PrimOp::Lt,
                                            prim: Prim::I64,
                                            args: vec![self.int_lit(rank(i, vname)), local(*rb, int)],
                                        },
                                        bool_ty,
                                        Span::NONE,
                                    );
                                    Expr::new(
                                        ExprKind::If {
                                            cond: Box::new(below),
                                            then: Box::new(self.order_lit(d, LESS)),
                                            else_: Box::new(self.order_lit(d, GREATER)),
                                        },
                                        d.ret,
                                        Span::NONE,
                                    )
                                }
                                None => self.bool_lit(false),
                            };
                            let mut inner = vec![arm(b_pat, same)];
                            if names.len() > 1 {
                                inner.push(arm(wild(ty), differ));
                            }
                            match_(b.clone(), inner, d.ret)
                        }
                    };
                    arms.push(arm(a_pat, body));
                }
                let body = match_(a, arms, d.ret);
                match rb {
                    None => body,
                    Some((rb, rank_of_b)) => Expr::new(
                        ExprKind::Block {
                            stmts: vec![typed::Stmt::Let {
                                pattern: bind(rb, rank_of_b.ty),
                                value: rank_of_b,
                                span: Span::NONE,
                            }],
                            tail: Some(Box::new(body)),
                        },
                        d.ret,
                        Span::NONE,
                    ),
                }
            }
            TyDef::Prim(_) => Expr::new(ExprKind::Error, d.ret, Span::NONE),
        }
    }

    fn tag_pattern(&self, ty: Ty, variant: usize) -> Pattern {
        let con = ty.head().unwrap_or(TyConId(0));
        Pattern { kind: PatKind::Variant { con, variant, fields: Vec::new() }, ty, span: Span::NONE }
    }

    fn payload_pattern(
        &self,
        ty: Ty,
        variant: usize,
        payload: &[Part],
        binds: &[LocalId],
    ) -> Pattern {
        let con = ty.head().unwrap_or(TyConId(0));
        let fields = payload
            .iter()
            .zip(binds)
            .enumerate()
            .map(|(index, (p, l))| FieldPat { index, pattern: bind(*l, p.ty) })
            .collect();
        Pattern { kind: PatKind::Variant { con, variant, fields }, ty, span: Span::NONE }
    }
}
