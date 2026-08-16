//! Type inference and the checking of function bodies.
//!
//! Inference is local to one body, because top-level signatures are mandatory
//! (SPEC 9). Name resolution and inference interleave in a single traversal —
//! the one place Buri gives something up — because resolving `x.f()` needs the
//! receiver's type. What keeps it a traversal rather than a fixpoint is that
//! method resolution needs only the receiver's *head type constructor*, that
//! type information flows outside-in and left-to-right, and that there is no
//! overloading (SPEC 13.2).

use crate::ast;
use crate::check::Checker;
use crate::compile::Role;
use crate::diag::{Diagnostic, Span};
use crate::hir;
use crate::types::*;
use std::collections::HashMap;

pub fn check_all(c: &mut Checker) {
    // Constants first: a `const` may be referenced from any body.
    for i in 0..c.tables.consts.len() {
        check_const(c, ConstId(i as u32));
    }
    for i in 0..c.tables.ctx_decls.len() {
        check_context_decl(c, ContextDeclId(i as u32));
    }
    // Function bodies check independently and in any order (SPEC 13.3).
    for i in 0..c.tables.fns.len() {
        check_fn(c, FnId(i as u32));
    }
    check_tests(c);
}

fn body_ast(c: &Checker, r: AstRef) -> Option<ast::FnDecl> {
    if r.module.0 == u32::MAX {
        return None;
    }
    let item = c.module(r.module).ast.items.get(r.item as usize)?;
    match item {
        ast::Item::Fn(d) if r.sub == u32::MAX => Some(d.clone()),
        ast::Item::Impl(d) => d.methods.get(r.sub as usize).cloned(),
        _ => None,
    }
}

fn check_fn(c: &mut Checker, fid: FnId) {
    let info = c.tables.fun(fid).clone();
    let Some(decl) = body_ast(c, info.ast) else { return };
    let Some(body) = decl.body.clone() else { return };

    let mut inf = Infer::new(c, info.module, info.generics.clone(), info.ret.clone());
    inf.self_con = info.self_ty;
    inf.in_main = info.name == "main" && info.exported && inf.role == Role::Entry;
    inf.push_scope();
    for p in &info.params {
        let local = inf.new_local(&p.name, p.ty.clone(), p.span);
        inf.bind(&p.name, local);
        inf.params.push(local);
        if p.role == ParamRole::Ctx || p.role == ParamRole::SelfParam {
            inf.effect_locals.insert(local);
        }
    }
    let expected = info.ret.clone();
    let expr = inf.check_block(&body, Some(&expected));
    inf.unify_at(body.span, &expr.ty.clone(), &expected, "the declared return type");
    let hir_body = inf.finish(expr);
    c.bodies.insert(fid, hir_body);
}

fn check_const(c: &mut Checker, cid: ConstId) {
    let info = c.tables.const_(cid).clone();
    if info.ast.module.0 == u32::MAX {
        return;
    }
    let Some(ast::Item::Const(decl)) =
        c.module(info.ast.module).ast.items.get(info.ast.item as usize).cloned()
    else {
        return;
    };
    let mut inf = Infer::new(c, info.module, Vec::new(), info.ty.clone());
    inf.push_scope();
    let ty = info.ty.clone();
    let value = inf.check_expr(&decl.value, Some(&ty));
    inf.unify_at(decl.value.span(), &value.ty.clone(), &ty, "the declared type");
    let body = inf.finish(value);
    c.const_values.insert(cid, body.expr);
}

fn check_context_decl(c: &mut Checker, id: ContextDeclId) {
    let info = c.tables.ctx_decl(id).clone();
    let Some(ast::Item::Context(decl)) =
        c.module(info.ast.module).ast.items.get(info.ast.item as usize).cloned()
    else {
        return;
    };
    let mut inf = Infer::new(c, info.module, Vec::new(), Ty::Unit);
    inf.in_main = true;
    inf.push_scope();
    let expr = inf.check_context_body(&decl.body, decl.span);
    if let Ty::Ctx(ct) = expr.ty {
        inf.c.tables.ctx_decls[id.index()].ty = Some(ct);
    }
    let body = inf.finish(expr);
    // A named context is constructed by calling it, and each call builds a
    // fresh one, so the declaration becomes a nullary function.
    let ret = body.expr.ty.clone();
    let ctor = c.tables.add_fn(FnInfo {
        name: info.name.clone(),
        module: info.module,
        generics: Vec::new(),
        params: Vec::new(),
        ret,
        exported: info.exported,
        span: info.span,
        self_ty: None,
        impl_of: None,
        ast: AstRef::NONE,
        intrinsic: false,
    });
    c.tables.ctx_decls[id.index()].ctor = Some(ctor);
    c.bodies.insert(ctor, body);
}

fn check_tests(c: &mut Checker) {
    let mut cases = Vec::new();
    for m in 0..c.loaded.modules.len() {
        let module = ModuleId(m as u32);
        if c.module(module).role != Role::TestSource {
            continue;
        }
        let items = c.module(module).ast.items.clone();
        for (index, item) in items.iter().enumerate() {
            let ast::Item::Test(t) = item else { continue };
            // A test takes no parameters and returns nothing: it passes unless
            // an assertion in it fails.
            let fid = c.tables.add_fn(FnInfo {
                name: format!("test#{}", cases.len()),
                module,
                generics: Vec::new(),
                params: Vec::new(),
                ret: Ty::Unit,
                exported: false,
                span: t.span,
                self_ty: None,
                impl_of: None,
                ast: AstRef { module, item: index as u32, sub: u32::MAX },
                intrinsic: false,
            });
            let mut inf = Infer::new(c, module, Vec::new(), Ty::Unit);
            inf.in_main = true;
            inf.push_scope();
            let expr = inf.check_block(&t.body, None);
            let body = inf.finish(expr);
            c.bodies.insert(fid, body);
            cases.push(crate::check::TestCase {
                name: t.name.clone(),
                module,
                func: fid,
                span: t.span,
            });
        }
    }
    c.tests = cases;
}

// ---------------------------------------------------------------------------

/// A numeric literal whose type is not known until defaulting has run.
pub(crate) struct LitCheck {
    pub(crate) value: u128,
    pub(crate) negative: bool,
    pub(crate) raw: String,
    pub(crate) ty: Ty,
    pub(crate) span: Span,
}

pub struct Infer<'a, 'b> {
    pub c: &'a mut Checker<'b>,
    pub(crate) module: ModuleId,
    pub(crate) generics: Vec<GenericInfo>,
    pub(crate) ret: Ty,
    pub(crate) subst: Subst,
    pub(crate) scopes: Vec<HashMap<String, LocalId>>,
    pub(crate) locals: Vec<hir::Local>,
    pub(crate) params: Vec<LocalId>,
    pub(crate) self_con: Option<TyConId>,
    /// Locals holding an effect-carrying value, which a lambda may not
    /// capture (SPEC 10.6).
    pub(crate) effect_locals: std::collections::HashSet<LocalId>,
    pub(crate) lambda_depth: u32,
    pub(crate) obligations: Vec<(Ty, TraitId, Span)>,
    pub(crate) lit_checks: Vec<LitCheck>,
    /// Template holes, checked after defaulting so `"${1 + 1}"` is fine.
    pub(crate) hole_checks: Vec<(Ty, Span)>,
    pub(crate) role: Role,
    /// Whether the body being checked is `main`'s. A context may be built in
    /// `main`'s body, not merely anywhere in the module that exports it.
    pub(crate) in_main: bool,
    /// Bindings made by the alternative of an or-pattern being checked, so the
    /// next alternative reuses the same locals for the same names.
    pub(crate) or_bindings: Option<HashMap<String, LocalId>>,
    pub(crate) or_first: Option<HashMap<String, LocalId>>,
    /// Names bound by the pattern currently being checked, so a duplicate
    /// within one pattern is caught (SPEC 14.6).
    pub(crate) pattern_names: Vec<String>,
}

impl<'a, 'b> Infer<'a, 'b> {
    fn new(c: &'a mut Checker<'b>, module: ModuleId, generics: Vec<GenericInfo>, ret: Ty) -> Self {
        let role = c.module(module).role;
        Infer {
            c,
            module,
            generics,
            ret,
            subst: Subst::default(),
            scopes: Vec::new(),
            locals: Vec::new(),
            params: Vec::new(),
            self_con: None,
            effect_locals: std::collections::HashSet::new(),
            lambda_depth: 0,
            obligations: Vec::new(),
            lit_checks: Vec::new(),
            hole_checks: Vec::new(),
            role,
            in_main: false,
            or_bindings: None,
            or_first: None,
            pattern_names: Vec::new(),
        }
    }

    fn finish(mut self, expr: hir::Expr) -> hir::Body {
        // Only if nothing constrains a literal does the default apply.
        self.subst.default_numerics(&self.c.tables);
        self.discharge_obligations();
        self.check_literal_ranges();
        self.check_template_holes();
        let expr = self.resolve_expr(expr);
        let locals = self
            .locals
            .iter()
            .map(|l| hir::Local { name: l.name.clone(), ty: self.subst.resolve(&l.ty), span: l.span })
            .collect();
        hir::Body { locals, params: self.params, expr }
    }

    fn resolve_expr(&self, mut e: hir::Expr) -> hir::Expr {
        e.ty = self.subst.resolve(&e.ty);
        let sub = |x: hir::Expr| self.resolve_expr(x);
        e.kind = match e.kind {
            hir::ExprKind::CallValue { callee, args } => hir::ExprKind::CallValue {
                callee: Box::new(sub(*callee)),
                args: args.into_iter().map(sub).collect(),
            },
            hir::ExprKind::CallFn { func, targs, args } => hir::ExprKind::CallFn {
                func,
                targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                args: args.into_iter().map(sub).collect(),
            },
            hir::ExprKind::CallTrait { trait_id, method, recv, targs, args } => {
                hir::ExprKind::CallTrait {
                    trait_id,
                    method,
                    recv: self.subst.resolve(&recv),
                    targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                    args: args.into_iter().map(sub).collect(),
                }
            }
            hir::ExprKind::StructLit { con, targs, fields } => hir::ExprKind::StructLit {
                con,
                targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                fields: fields.into_iter().map(sub).collect(),
            },
            hir::ExprKind::StructUpdate { con, base, updates } => hir::ExprKind::StructUpdate {
                con,
                base: Box::new(sub(*base)),
                updates: updates.into_iter().map(|(i, e)| (i, sub(e))).collect(),
            },
            hir::ExprKind::EnumLit { con, targs, variant, args } => hir::ExprKind::EnumLit {
                con,
                targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                variant,
                args: args.into_iter().map(sub).collect(),
            },
            hir::ExprKind::Tuple(xs) => hir::ExprKind::Tuple(xs.into_iter().map(sub).collect()),
            hir::ExprKind::Array(xs) => hir::ExprKind::Array(xs.into_iter().map(sub).collect()),
            hir::ExprKind::Field { base, index } => {
                hir::ExprKind::Field { base: Box::new(sub(*base)), index }
            }
            hir::ExprKind::TupleIndex { base, index } => {
                hir::ExprKind::TupleIndex { base: Box::new(sub(*base)), index }
            }
            hir::ExprKind::Index { base, index, elem } => hir::ExprKind::Index {
                base: Box::new(sub(*base)),
                index: Box::new(sub(*index)),
                elem: self.subst.resolve(&elem),
            },
            hir::ExprKind::Block { stmts, tail } => hir::ExprKind::Block {
                stmts: stmts
                    .into_iter()
                    .map(|s| match s {
                        hir::Stmt::Let { pattern, value, span } => hir::Stmt::Let {
                            pattern: self.resolve_pattern(pattern),
                            value: sub(value),
                            span,
                        },
                        hir::Stmt::Expr(e) => hir::Stmt::Expr(sub(e)),
                    })
                    .collect(),
                tail: tail.map(|t| Box::new(sub(*t))),
            },
            hir::ExprKind::If { cond, then, else_ } => hir::ExprKind::If {
                cond: Box::new(sub(*cond)),
                then: Box::new(sub(*then)),
                else_: Box::new(sub(*else_)),
            },
            hir::ExprKind::Match { scrutinee, arms } => hir::ExprKind::Match {
                scrutinee: Box::new(sub(*scrutinee)),
                arms: arms
                    .into_iter()
                    .map(|a| hir::Arm {
                        pattern: self.resolve_pattern(a.pattern),
                        guard: a.guard.map(&sub),
                        body: sub(a.body),
                        span: a.span,
                    })
                    .collect(),
            },
            hir::ExprKind::Lambda { params, body, captures } => {
                hir::ExprKind::Lambda { params, body: Box::new(sub(*body)), captures }
            }
            hir::ExprKind::And { lhs, rhs } => {
                hir::ExprKind::And { lhs: Box::new(sub(*lhs)), rhs: Box::new(sub(*rhs)) }
            }
            hir::ExprKind::Or { lhs, rhs } => {
                hir::ExprKind::Or { lhs: Box::new(sub(*lhs)), rhs: Box::new(sub(*rhs)) }
            }
            hir::ExprKind::Coalesce { lhs, rhs, kind } => hir::ExprKind::Coalesce {
                lhs: Box::new(sub(*lhs)),
                rhs: Box::new(sub(*rhs)),
                kind,
            },
            hir::ExprKind::Try { base, kind } => {
                hir::ExprKind::Try { base: Box::new(sub(*base)), kind }
            }
            hir::ExprKind::Prim { op, prim, args } => {
                hir::ExprKind::Prim { op, prim, args: args.into_iter().map(sub).collect() }
            }
            hir::ExprKind::StructuralEq { negate, args } => hir::ExprKind::StructuralEq {
                negate,
                args: args.into_iter().map(sub).collect(),
            },
            hir::ExprKind::StructuralCmp { op, args } => hir::ExprKind::StructuralCmp {
                op,
                args: args.into_iter().map(sub).collect(),
            },
            hir::ExprKind::Template { parts } => hir::ExprKind::Template {
                parts: parts
                    .into_iter()
                    .map(|p| hir::TemplatePart { text: p.text, hole: p.hole.map(&sub) })
                    .collect(),
            },
            hir::ExprKind::CtxLit { bindings } => hir::ExprKind::CtxLit {
                bindings: bindings.into_iter().map(|(t, e)| (t, sub(e))).collect(),
            },
            hir::ExprKind::CtxGet { base, trait_id } => {
                hir::ExprKind::CtxGet { base: Box::new(sub(*base)), trait_id }
            }
            hir::ExprKind::Intrinsic { name, targs, args } => hir::ExprKind::Intrinsic {
                name,
                targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                args: args.into_iter().map(sub).collect(),
            },
            hir::ExprKind::FnRef(f, targs) => {
                hir::ExprKind::FnRef(f, targs.iter().map(|t| self.subst.resolve(t)).collect())
            }
            other => other,
        };
        e
    }

    fn resolve_pattern(&self, mut p: hir::Pattern) -> hir::Pattern {
        p.ty = self.subst.resolve(&p.ty);
        p.kind = match p.kind {
            hir::PatKind::Bind { local, sub } => hir::PatKind::Bind {
                local,
                sub: sub.map(|s| Box::new(self.resolve_pattern(*s))),
            },
            hir::PatKind::Tuple(ps) => {
                hir::PatKind::Tuple(ps.into_iter().map(|x| self.resolve_pattern(x)).collect())
            }
            hir::PatKind::Struct { con, fields } => hir::PatKind::Struct {
                con,
                fields: fields
                    .into_iter()
                    .map(|f| hir::FieldPat { index: f.index, pattern: self.resolve_pattern(f.pattern) })
                    .collect(),
            },
            hir::PatKind::Variant { con, variant, fields } => hir::PatKind::Variant {
                con,
                variant,
                fields: fields
                    .into_iter()
                    .map(|f| hir::FieldPat { index: f.index, pattern: self.resolve_pattern(f.pattern) })
                    .collect(),
            },
            hir::PatKind::Array { elems, rest } => hir::PatKind::Array {
                elems: elems.into_iter().map(|x| self.resolve_pattern(x)).collect(),
                rest,
            },
            hir::PatKind::Or(ps) => {
                hir::PatKind::Or(ps.into_iter().map(|x| self.resolve_pattern(x)).collect())
            }
            other => other,
        };
        p
    }

    // -- scopes -------------------------------------------------------------

    pub(crate) fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub(crate) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub(crate) fn new_local(&mut self, name: &str, ty: Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(hir::Local { name: name.to_string(), ty, span });
        id
    }

    pub(crate) fn bind(&mut self, name: &str, local: LocalId) {
        // Shadowing is permitted, both in nested scopes and within a block.
        self.scopes.last_mut().unwrap().insert(name.to_string(), local);
    }

    pub(crate) fn lookup_local(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    pub(crate) fn local_ty(&self, id: LocalId) -> Ty {
        self.locals[id.index()].ty.clone()
    }

    // -- unification --------------------------------------------------------

    pub(crate) fn fresh(&mut self, span: Span) -> Ty {
        self.subst.fresh(span)
    }

    pub(crate) fn unify_at(&mut self, span: Span, actual: &Ty, expected: &Ty, what: &str) {
        if let Err((a, b)) = self.subst.unify(&self.c.tables, actual, expected) {
            let a = show(&self.c.tables, Some(&self.subst), &self.generics, &a);
            let b = show(&self.c.tables, Some(&self.subst), &self.generics, &b);
            let mut d = Diagnostic::error(span, format!("expected `{b}`, found `{a}`")).with_code("type-mismatch")
                .with_mismatch(format!("`{b}`"), format!("`{a}`"));
            if !what.is_empty() {
                d = d.with_label(format!("{what} is `{b}`"));
            }
            // There is no implicit promotion of any kind, and the most common
            // way to hit this is expecting one. The conversion is named
            // explicitly, because which one it is depends on whether the value
            // can fail to fit.
            d = if is_numeric_mismatch(&self.c.tables, &a, &b) {
                d.with_note("there is no implicit promotion of any kind")
                    .with_fix(numeric_fix(&a, &b))
            } else {
                d.with_fix(format!(
                    "produce a `{b}` here, or change what surrounds it to accept a `{a}`"
                ))
            };
            self.c.diags.push(d);
        }
    }

    pub(crate) fn resolve(&self, t: &Ty) -> Ty {
        self.subst.shallow(t)
    }

    pub(crate) fn prim(&self, p: Prim) -> Ty {
        self.c.tables.prim(p)
    }

    pub(crate) fn as_prim(&self, t: &Ty) -> Option<Prim> {
        self.c.tables.as_prim(&self.resolve(t))
    }

    pub(crate) fn show_ty(&self, t: &Ty) -> String {
        show(&self.c.tables, Some(&self.subst), &self.generics, t)
    }

    pub(crate) fn err(&mut self, span: Span, msg: impl Into<String>) -> &mut Diagnostic {
        self.c.diags.items.push(Diagnostic::error(span, msg));
        self.c.diags.items.last_mut().unwrap()
    }

    pub(crate) fn error_expr(&self, span: Span) -> hir::Expr {
        hir::Expr::new(hir::ExprKind::Error, Ty::Error, span)
    }

    // -- obligations --------------------------------------------------------

    pub(crate) fn require(&mut self, ty: Ty, tr: TraitId, span: Span) {
        self.obligations.push((ty, tr, span));
    }

    fn discharge_obligations(&mut self) {
        let obligations = std::mem::take(&mut self.obligations);
        for (ty, tr, span) in obligations {
            let ty = self.subst.resolve(&ty);
            if self.satisfies(&ty, tr) {
                continue;
            }
            let trait_name = self.c.tables.trait_(tr).name.clone();
            let shown = self.show_ty(&ty);
            let mut note = None;
            let mut fix = None;
            if let Some(con) = ty.head() {
                let derived = self
                    .c
                    .tables
                    .impls
                    .get(&(tr, con))
                    .is_some_and(|i| i.derived);
                if derived {
                    // The `derive` is there; one of the components it folds
                    // over is what fails, and naming it is the useful part.
                    let culprit = self.failing_component(con, &ty, tr);
                    match culprit {
                        Some(c) => {
                            note = Some(format!(
                                "`{shown}` derives `{trait_name}`, but `{c}` does not satisfy \
                                 it, and a derived implementation is a fold over the type's \
                                 components"
                            ));
                            fix = Some(format!("make `{c}` satisfy `{trait_name}` first"));
                        }
                        None => {
                            note = Some(format!(
                                "`{shown}` derives `{trait_name}`, but one of its components \
                                 does not satisfy it"
                            ));
                            fix = Some(format!(
                                "make every component of `{shown}` satisfy `{trait_name}`"
                            ));
                        }
                    }
                } else {
                    let has = self.c.traits_of(con);
                    if !has.is_empty() {
                        note = Some(format!(
                            "`{shown}` implements {}, but not `{trait_name}`",
                            crate::diag::names(&has.into_iter().collect::<Vec<_>>())
                        ));
                    }
                    fix = Some(format!(
                        "add `derive {trait_name} for {shown};` in that type's own module, or \
                         write `impl {trait_name} for {shown} {{ ... }}` there"
                    ));
                }
            }
            let fix = fix.unwrap_or_else(|| {
                format!("bound the type parameter with `{trait_name}`, or use a type that has one")
            });
            let d = self.err(span, format!("`{shown}` does not satisfy `{trait_name}`"));
            d.code("missing-conformance");
            d.fix(fix);
            if let Some(n) = note {
                d.notes.push(n);
            }
        }
    }

    pub(crate) fn satisfies(&self, ty: &Ty, tr: TraitId) -> bool {
        self.satisfies_seen(ty, tr, &mut Vec::new())
    }

    /// `seen` is the type constructors whose components are already being
    /// checked further up this walk.
    ///
    /// A derived implementation is a fold over the type's components, so
    /// deciding whether one satisfies a trait means asking the same question
    /// of its fields — and a recursive type asks it of itself. Reaching a
    /// constructor that is already on the stack means the answer depends on
    /// itself, and the honest answer to that is yes: the recursion is what
    /// makes it satisfiable, not what makes it fail.
    ///
    /// This was a `head() == Some(con)` test on the immediate component, which
    /// caught `Cons(L)` and missed `Cons([L])` — an array's head is not a
    /// constructor — so a type that recursed through *any* container hung the
    /// compiler until it ran out of stack.
    fn satisfies_seen(&self, ty: &Ty, tr: TraitId, seen: &mut Vec<TyConId>) -> bool {
        match ty {
            Ty::Param(i) => self
                .generics
                .get(*i as usize)
                .is_some_and(|g| g.bounds.contains(&tr)),
            Ty::Error | Ty::Var(_) => true,
            Ty::Ctx(id) => self.c.tables.ctx_type(*id).has(tr),
            Ty::SelfTy => true,
            // `[T]`, tuples and function types satisfy the structural traits
            // when their components do (SPEC 5.11).
            Ty::Array(e) => self.structural_trait(tr) && self.satisfies_seen(e, tr, seen),
            Ty::Tuple(es) => {
                self.structural_trait(tr)
                    && es.iter().all(|e| self.satisfies_seen(e, tr, seen))
            }
            Ty::Unit => self.structural_trait(tr),
            Ty::Con(id, args) => {
                if self.c.tables.impls.contains_key(&(tr, *id)) {
                    // A derived impl requires every field type to satisfy the
                    // trait too.
                    let derived = self.c.tables.impls[&(tr, *id)].derived;
                    if derived {
                        if seen.contains(id) {
                            return true;
                        }
                        seen.push(*id);
                        let ok = self.derived_components_satisfy(*id, args, tr, seen);
                        seen.pop();
                        return ok;
                    }
                    return true;
                }
                false
            }
            Ty::Fn(..) => false,
        }
    }

    /// The first field or payload type of a derived type that does not itself
    /// satisfy the trait.
    fn failing_component(&self, con: TyConId, ty: &Ty, tr: TraitId) -> Option<String> {
        let args = match ty {
            Ty::Con(_, a) => a.clone(),
            _ => Vec::new(),
        };
        let tycon = self.c.tables.tycon(con);
        let components: Vec<Ty> = match &tycon.def {
            TyDef::Struct { fields, .. } => fields.iter().map(|f| f.ty.clone()).collect(),
            TyDef::Enum { variants } => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                .collect(),
            TyDef::Prim(_) => Vec::new(),
        };
        components.into_iter().find_map(|t| {
            let t = substitute(&t, &args, None);
            if t.head() == Some(con) || self.satisfies(&t, tr) {
                None
            } else {
                Some(self.show_ty(&t))
            }
        })
    }

    fn structural_trait(&self, tr: TraitId) -> bool {
        matches!(self.c.tables.trait_(tr).name.as_str(), "Eq" | "Ord" | "Show" | "Hash")
    }

    fn derived_components_satisfy(
        &self,
        con: TyConId,
        args: &[Ty],
        tr: TraitId,
        seen: &mut Vec<TyConId>,
    ) -> bool {
        let tycon = self.c.tables.tycon(con);
        let field_types: Vec<Ty> = match &tycon.def {
            TyDef::Struct { fields, .. } => fields.iter().map(|f| f.ty.clone()).collect(),
            TyDef::Enum { variants } => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                .collect(),
            TyDef::Prim(_) => Vec::new(),
        };
        // `con` is already on `seen`, so a component that reaches back to it —
        // directly, or through an array, a tuple, or another type that holds
        // one — stops there rather than asking the question again.
        field_types.iter().all(|t| {
            let t = substitute(t, args, None);
            self.satisfies_seen(&t, tr, seen)
        })
    }

    // -- literal ranges -----------------------------------------------------

    /// Because a literal's type is known before it is checked, a literal that
    /// does not fit its type is a compile error, not a runtime surprise.
    fn check_literal_ranges(&mut self) {
        let checks = std::mem::take(&mut self.lit_checks);
        for lit in checks {
            let ty = self.subst.resolve(&lit.ty);
            let Some(p) = self.c.tables.as_prim(&ty) else { continue };
            let Some((lo, hi)) = p.int_range() else { continue };
            let fits = if lit.negative {
                p.is_signed() && (lit.value <= (lo.unsigned_abs()))
            } else {
                lit.value <= hi
            };
            if !fits {
                let name = p.name();
                let raw = if lit.negative { format!("-{}", lit.raw) } else { lit.raw.clone() };
                let mut d = Diagnostic::error(
                    lit.span,
                    format!("{raw} is not representable in `{name}`"),
                ).with_code("literal-out-of-range");
                d = d
                    .with_mismatch(format!("a value `{name}` can hold"), raw.clone())
                    .with_fix(if lit.negative && !p.is_signed() {
                        format!("use a signed type, or drop the sign; `{name}` starts at 0")
                    } else {
                        format!(
                            "write a value inside `{name}`'s range, or annotate a wider type"
                        )
                    });
                if lit.negative && !p.is_signed() {
                    d = d.with_note(format!("`{name}` has no negative values"));
                } else {
                    d = d.with_note(format!("`{name}` holds {lo} to {hi}"));
                }
                self.c.diags.push(d);
            }
        }
    }
}

impl<'a, 'b> Infer<'a, 'b> {
    /// Hole expressions must have type `Int` (any width), `Float` (any width),
    /// `Bool`, `Char`, or `Str`. There is no user-extensible display mechanism
    /// in v0.3; convert explicitly.
    fn check_template_holes(&mut self) {
        let checks = std::mem::take(&mut self.hole_checks);
        for (ty, span) in checks {
            let resolved = self.subst.resolve(&ty);
            let ok = match self.c.tables.as_prim(&resolved) {
                Some(p) => {
                    p.is_integer()
                        || p.is_float()
                        || matches!(p, Prim::Bool | Prim::Char | Prim::Str)
                }
                None => matches!(resolved, Ty::Error | Ty::Var(_)),
            };
            if !ok {
                let shown = show(&self.c.tables, Some(&self.subst), &self.generics, &resolved);
                self.c.diags.push(
                    Diagnostic::error(span, format!("`{shown}` cannot be interpolated"))
                        .with_fix("render it first, for instance with `.show(ctx)`")
                        .with_note(
                            "a hole holds an `Int`, a `Float`, a `Bool`, a `Char`, or a `Str`",
                        ),
                );
            }
        }
    }
}

/// The conversion to reach for, named exactly. Which one it is depends on
/// whether the value can fail to fit, so a generic "convert it" would leave the
/// reader to work out the return type for themselves.
fn numeric_fix(actual: &str, expected: &str) -> String {
    // A literal has not been pinned to a type yet, so an annotation is the
    // edit, not a conversion.
    if actual.starts_with('{') {
        return format!("annotate the literal, as in `let x: {expected} = ...`");
    }
    if expected.starts_with('{') {
        return format!("write a literal of the right kind, or convert with `.to{actual}()`");
    }
    let exact = matches!(
        (actual, expected),
        ("I8", "I16" | "I32" | "I64" | "I128" | "F64")
            | ("I16", "I32" | "I64" | "I128" | "F64")
            | ("I32", "I64" | "I128" | "F64")
            | ("I64", "I128")
            | ("U8", "U16" | "U32" | "U64" | "U128" | "I16" | "I32" | "I64" | "I128" | "F64")
            | ("U16", "U32" | "U64" | "U128" | "I32" | "I64" | "I128" | "F64")
            | ("U32", "U64" | "U128" | "I64" | "I128" | "F64")
            | ("U64", "U128" | "I128")
            | ("F32", "F64")
    );
    if exact {
        format!("convert explicitly: `.to{expected}()`, which is exact for every `{actual}`")
    } else {
        format!(
            "convert explicitly with `.to{expected}()?`, which returns a \
             `Result<{expected}, RangeError>` because not every `{actual}` fits"
        )
    }
}

fn is_numeric_mismatch(tables: &Tables, a: &str, b: &str) -> bool {
    let _ = tables;
    let numericish = |s: &str| {
        s.starts_with('I')
            || s.starts_with('U')
            || s.starts_with('F')
            || s == "{integer}"
            || s == "{float}"
    };
    numericish(a) && numericish(b) && a != b
}

