//! Type inference and the checking of function bodies.
//!
//! Inference is local to one body, because top-level signatures are mandatory
//! (SPEC 9). Name resolution and inference interleave in a single traversal —
//! the one place Buri gives something up — because resolving `x.f()` needs the
//! receiver's type. What keeps it a traversal rather than a fixpoint is that
//! method resolution needs only the receiver's *head type constructor*, that
//! type information flows outside-in and left-to-right, and that there is no
//! overloading (SPEC 13.2).

use crate::compiler::modules::Role;
use crate::compiler::semantics::resolve::Checker;
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::*;
use crate::diagnostics::{Diagnostic, Invariant as _, Span};
use crate::parsing::flat;
use crate::parsing::tree;
use crate::hash::Map as HashMap;

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

/// The declaration a function was written as. Borrowed from the loaded
/// modules — `'a`, not the checker — so that checking a body does not begin by
/// copying it. Every function in the compilation passed through here, so the
/// copy was one deep clone of every body in the standard library and the
/// repository, per analysis.
fn body_ast<'a>(c: &Checker<'a>, r: AstRef) -> Option<&'a tree::FnDecl> {
    match r {
        AstRef::Builtin => None,
        AstRef::Item { module, item } => match c.module(module).ast.items.get(item as usize)? {
            tree::Item::Fn(d) => Some(d),
            _ => None,
        },
        AstRef::Method { module, item, sub } => {
            match c.module(module).ast.items.get(item as usize)? {
                tree::Item::Impl(d) => d.methods.get(sub as usize),
                _ => None,
            }
        }
    }
}

fn check_fn(c: &mut Checker, fid: FnId) {
    let info = c.tables.fun(fid).clone();
    let Some(decl) = body_ast(c, info.ast) else { return };
    let Some(body) = decl.body else { return };

    let mut inf = Infer::new(c, info.module, info.generics.clone(), info.ret.clone());
    inf.self_con = info.self_ty;
    inf.in_main = info.name == "main" && info.exported && inf.role == Role::Entry;
    inf.push_scope();
    for p in &info.params {
        let local = inf.new_local(&p.name, p.ty.clone(), p.span);
        inf.bind(&p.name, local);
        inf.params.push(local);
        // The capture rule is scoped to *effect-carrying* values (SPEC 10.6,
        // SPEC 14 rule 8). `ctx` is one by construction — the `ctx` rule
        // admits nothing else there — but `self` is whatever the receiver
        // type is, and an ordinary struct's methods must still be able to
        // write `fn(x) => x > self.n`. So `self` is gated on its type, exactly
        // as a normal parameter is in `check_ctx_rule`.
        if p.role == ParamRole::Ctx {
            inf.effect_locals.insert(local);
        } else {
            let ty = p.ty.clone();
            inf.note_capture_risk(local, &ty);
        }
    }
    let expected = info.ret.clone();
    let body_span = inf.t.block_span(body);
    let expr = inf.check_block(body, Some(&expected));
    inf.unify_at(body_span, &expr.ty.clone(), &expected, "the declared return type");
    let hir_body = inf.finish(expr);
    c.bodies.insert(fid, hir_body);
}

fn check_const(c: &mut Checker, cid: ConstId) {
    let info = c.tables.const_(cid).clone();
    let Some((module, index)) = info.ast.item() else { return };
    let Some(tree::Item::Const(decl)) = c.module(module).ast.items.get(index as usize) else {
        return;
    };
    let mut inf = Infer::new(c, info.module, Vec::new(), info.ty.clone());
    inf.push_scope();
    let ty = info.ty.clone();
    let value_span = inf.t.span(decl.value);
    let value = inf.check_expr(decl.value, Some(&ty));
    inf.unify_at(value_span, &value.ty.clone(), &ty, "the declared type");
    let body = inf.finish(value);
    c.const_values.insert(cid, body.expr);
}

fn check_context_decl(c: &mut Checker, id: ContextDeclId) {
    let info = c.tables.ctx_decl(id).clone();
    let Some((decl_module, decl_index)) = info.ast.item() else { return };
    let Some(tree::Item::Context(decl)) = c.module(decl_module).ast.items.get(decl_index as usize)
    else {
        return;
    };
    let mut inf = Infer::new(c, info.module, Vec::new(), Ty::Unit);
    inf.in_main = true;
    inf.push_scope();
    let expr = inf.check_context_body(decl.body, decl.span);
    // A body that did not evaluate to a context type has no generated type,
    // and then its constructor is not usable either — so neither is recorded.
    let ctx_ty = match &expr.ty {
        Ty::Ctx(ct) => Some(*ct),
        _ => None,
    };
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
        ast: AstRef::Builtin,
        intrinsic: false,
    });
    if let Some(ty) = ctx_ty {
        c.tables.ctx_decl_mut(id).checked = Some(CheckedContext { ty, ctor });
    }
    c.bodies.insert(ctor, body);
}

fn check_tests(c: &mut Checker) {
    let mut cases = Vec::new();
    for m in 0..c.loaded.modules.len() {
        let module = ModuleId(m as u32);
        if c.module(module).role != Role::TestSource {
            continue;
        }
        let items = &c.module(module).ast.items;
        for (index, item) in items.iter().enumerate() {
            let tree::Item::Test(t) = item else { continue };
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
                ast: AstRef::Item { module, item: index as u32 },
                intrinsic: false,
            });
            let mut inf = Infer::new(c, module, Vec::new(), Ty::Unit);
            inf.in_main = true;
            inf.push_scope();
            let expr = inf.check_block(t.body, None);
            let body = inf.finish(expr);
            c.bodies.insert(fid, body);
            cases.push(crate::compiler::semantics::resolve::TestCase {
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

/// The state of the or-pattern currently being checked.
///
/// `current` and `first` were two `Option<HashMap<..>>` fields side by side.
/// Only one of them was saved and restored around a nested or-pattern, so an
/// or-pattern inside an alternative of an outer one cleared the outer's
/// first-alternative bindings on the way out — and the outer's remaining
/// alternatives then declared fresh locals for names the first had already
/// bound. As one value there is one thing to enter and one thing to leave, and
/// `(no scope, but a first alternative)` is unrepresentable.
#[derive(Default)]
pub(crate) struct OrScope {
    /// Bindings made by the alternative being checked right now.
    pub(crate) current: HashMap<String, LocalId>,
    /// Bindings made by the first alternative, which the rest reuse so that a
    /// name bound by both is one local.
    pub(crate) first: Option<HashMap<String, LocalId>>,
}

pub struct Infer<'a, 'b> {
    pub c: &'a mut Checker<'b>,
    /// The flat parse tree the body being checked lives in.
    ///
    /// It is `module`'s, and that is the module the body was written in: every
    /// `FnInfo`, `ConstInfo` and `ContextDeclInfo` is registered with the same
    /// `module` its `AstRef` names, so an id taken out of a declaration indexes
    /// this tree and no other. A body never reaches into another module's.
    ///
    /// Borrowed for `'b` — the modules live in `Checker::loaded`, which the
    /// checker only reads — so every id resolved through it is independent of
    /// the `&mut Checker` this holds, and a `match` arm may bind a name, a
    /// child list or a type expression out of the tree while the arm's body
    /// calls a `&mut self` method. That is the same reason `Checker::module`
    /// returns `&'a` rather than a `&self` borrow.
    pub(crate) t: &'b flat::Tree,
    pub(crate) module: ModuleId,
    pub(crate) generics: Vec<GenericInfo>,
    pub(crate) ret: Ty,
    pub(crate) subst: Subst,
    pub(crate) scopes: Vec<HashMap<String, LocalId>>,
    pub(crate) locals: Vec<typed::Local>,
    pub(crate) params: Vec<LocalId>,
    pub(crate) self_con: Option<TyConId>,
    /// Locals holding an effect-carrying value, which a lambda may not
    /// capture (SPEC 10.6).
    pub(crate) effect_locals: std::collections::HashSet<LocalId>,
    /// Locals whose type mentions a type parameter in a position that would
    /// hold an effect if the parameter were instantiated at a context type.
    /// A lambda may not capture one of these either: the body is checked once,
    /// polymorphically (SPEC 13.5), so this is the last point at which the
    /// question can be asked at all. See `Tables::may_carry_effect`.
    pub(crate) poly_locals: std::collections::HashSet<LocalId>,
    pub(crate) lambda_depth: u32,
    pub(crate) obligations: Vec<(Ty, TraitId, Span)>,
    pub(crate) lit_checks: Vec<LitCheck>,
    /// Template holes, checked after defaulting so `"${1 + 1}"` is fine.
    pub(crate) hole_checks: Vec<(Ty, Span)>,
    pub(crate) role: Role,
    /// Whether the body being checked is `main`'s. A context may be built in
    /// `main`'s body, not merely anywhere in the module that exports it.
    pub(crate) in_main: bool,
    /// The or-pattern being checked, if any.
    pub(crate) or_scope: Option<OrScope>,
    /// Names bound by the pattern currently being checked, so a duplicate
    /// within one pattern is caught (SPEC 14.6).
    pub(crate) pattern_names: Vec<String>,
}

impl<'a, 'b> Infer<'a, 'b> {
    fn new(c: &'a mut Checker<'b>, module: ModuleId, generics: Vec<GenericInfo>, ret: Ty) -> Self {
        let role = c.module(module).role;
        let t = &c.module(module).ast.tree;
        Infer {
            c,
            t,
            module,
            generics,
            ret,
            subst: Subst::default(),
            scopes: Vec::new(),
            locals: Vec::new(),
            params: Vec::new(),
            self_con: None,
            effect_locals: std::collections::HashSet::default(),
            poly_locals: std::collections::HashSet::default(),
            lambda_depth: 0,
            obligations: Vec::new(),
            lit_checks: Vec::new(),
            hole_checks: Vec::new(),
            role,
            in_main: false,
            or_scope: None,
            pattern_names: Vec::new(),
        }
    }

    /// The tree, detached from the `&self` borrow.
    ///
    /// `t` is a `&'b` reference and therefore `Copy`, so what comes back
    /// outlives this call and a checking method may hold a view, a child list
    /// or a name from it across its own `&mut self` recursion. Reading the
    /// field directly would reborrow `self`.
    pub(crate) fn tree(&self) -> &'b flat::Tree {
        self.t
    }

    /// Records what the capture rule needs to know about a newly bound local:
    /// whether it holds an effect, and — the part `is_effect_carrying` cannot
    /// see — whether it *would* hold one at some instantiation of the enclosing
    /// signature's generics (SPEC 10.6).
    ///
    /// Every binding form funnels through here, so the rule has no holes: a
    /// parameter, a `let`, a pattern binding, and a lambda's own parameters are
    /// all bindings an inner lambda could close over.
    pub(crate) fn note_capture_risk(&mut self, local: LocalId, ty: &Ty) {
        // `generics` and `c` are different fields, so neither predicate needs
        // a copy of the list — and this runs for every parameter, every `let`
        // and every pattern binding.
        let resolved = self.resolve(ty);
        if self.c.tables.is_effect_carrying(&resolved, &self.generics) {
            self.effect_locals.insert(local);
        } else if self.c.tables.may_carry_effect(&resolved, &self.generics) {
            self.poly_locals.insert(local);
        }
    }

    fn finish(mut self, expr: typed::Expr) -> typed::Body {
        // Only if nothing constrains a literal does the default apply.
        self.subst.default_numerics(&self.c.tables);
        self.discharge_obligations();
        self.check_literal_ranges();
        self.check_template_holes();
        let expr = self.resolve_expr(expr);
        let locals = self
            .locals
            .iter()
            .map(|l| typed::Local { name: l.name.clone(), ty: self.subst.resolve(&l.ty), span: l.span })
            .collect();
        typed::Body { locals, params: self.params, expr }
    }

    /// A callee is pre-monomorphization here, so only its type arguments need
    /// resolving.
    fn resolve_callee(&self, c: typed::Callee) -> typed::Callee {
        match c {
            typed::Callee::Decl { id, targs } => typed::Callee::Decl {
                id,
                targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
            },
            typed::Callee::Func(i) => typed::Callee::Func(i),
        }
    }

    fn resolve_expr(&self, mut e: typed::Expr) -> typed::Expr {
        e.ty = self.subst.resolve(&e.ty);
        let sub = |x: typed::Expr| self.resolve_expr(x);
        e.kind = match e.kind {
            typed::ExprKind::CallValue { callee, args } => typed::ExprKind::CallValue {
                callee: Box::new(sub(*callee)),
                args: args.into_iter().map(sub).collect(),
            },
            typed::ExprKind::CallFn { func, args } => typed::ExprKind::CallFn {
                func: self.resolve_callee(func),
                args: args.into_iter().map(sub).collect(),
            },
            typed::ExprKind::CallTrait { trait_id, method, recv, targs, args } => {
                typed::ExprKind::CallTrait {
                    trait_id,
                    method,
                    recv: self.subst.resolve(&recv),
                    targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                    args: args.into_iter().map(sub).collect(),
                }
            }
            typed::ExprKind::StructLit { con, targs, fields } => typed::ExprKind::StructLit {
                con,
                targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                fields: fields.into_iter().map(sub).collect(),
            },
            typed::ExprKind::StructUpdate { con, base, updates } => typed::ExprKind::StructUpdate {
                con,
                base: Box::new(sub(*base)),
                updates: updates.into_iter().map(|(i, e)| (i, sub(e))).collect(),
            },
            typed::ExprKind::EnumLit { con, targs, variant, args } => typed::ExprKind::EnumLit {
                con,
                targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                variant,
                args: args.into_iter().map(sub).collect(),
            },
            typed::ExprKind::Tuple(xs) => typed::ExprKind::Tuple(xs.into_iter().map(sub).collect()),
            typed::ExprKind::Array(xs) => typed::ExprKind::Array(xs.into_iter().map(sub).collect()),
            typed::ExprKind::Field { base, index } => {
                typed::ExprKind::Field { base: Box::new(sub(*base)), index }
            }
            typed::ExprKind::TupleIndex { base, index } => {
                typed::ExprKind::TupleIndex { base: Box::new(sub(*base)), index }
            }
            typed::ExprKind::Index { base, index, elem } => typed::ExprKind::Index {
                base: Box::new(sub(*base)),
                index: Box::new(sub(*index)),
                elem: self.subst.resolve(&elem),
            },
            typed::ExprKind::Block { stmts, tail } => typed::ExprKind::Block {
                stmts: stmts
                    .into_iter()
                    .map(|s| match s {
                        typed::Stmt::Let { pattern, value, span } => typed::Stmt::Let {
                            pattern: self.resolve_pattern(pattern),
                            value: sub(value),
                            span,
                        },
                        typed::Stmt::Expr(e) => typed::Stmt::Expr(sub(e)),
                    })
                    .collect(),
                tail: tail.map(|t| Box::new(sub(*t))),
            },
            typed::ExprKind::If { cond, then, else_ } => typed::ExprKind::If {
                cond: Box::new(sub(*cond)),
                then: Box::new(sub(*then)),
                else_: Box::new(sub(*else_)),
            },
            typed::ExprKind::Match { scrutinee, arms } => typed::ExprKind::Match {
                scrutinee: Box::new(sub(*scrutinee)),
                arms: arms
                    .into_iter()
                    .map(|a| typed::Arm {
                        pattern: self.resolve_pattern(a.pattern),
                        guard: a.guard.map(&sub),
                        body: sub(a.body),
                        span: a.span,
                    })
                    .collect(),
            },
            typed::ExprKind::Lambda { params, body, captures } => {
                typed::ExprKind::Lambda { params, body: Box::new(sub(*body)), captures }
            }
            typed::ExprKind::And { lhs, rhs } => {
                typed::ExprKind::And { lhs: Box::new(sub(*lhs)), rhs: Box::new(sub(*rhs)) }
            }
            typed::ExprKind::Or { lhs, rhs } => {
                typed::ExprKind::Or { lhs: Box::new(sub(*lhs)), rhs: Box::new(sub(*rhs)) }
            }
            typed::ExprKind::Coalesce { lhs, rhs, kind } => typed::ExprKind::Coalesce {
                lhs: Box::new(sub(*lhs)),
                rhs: Box::new(sub(*rhs)),
                kind,
            },
            typed::ExprKind::Try { base, kind } => {
                typed::ExprKind::Try { base: Box::new(sub(*base)), kind }
            }
            typed::ExprKind::Prim { op, prim, args } => {
                typed::ExprKind::Prim { op, prim, args: args.into_iter().map(sub).collect() }
            }
            typed::ExprKind::StructuralEq { negate, args } => typed::ExprKind::StructuralEq {
                negate,
                args: args.into_iter().map(sub).collect(),
            },
            typed::ExprKind::StructuralCmp { op, args } => typed::ExprKind::StructuralCmp {
                op,
                args: args.into_iter().map(sub).collect(),
            },
            typed::ExprKind::Template { parts } => typed::ExprKind::Template {
                parts: parts
                    .into_iter()
                    .map(|p| match p {
                        typed::TemplatePart::Text(t) => typed::TemplatePart::Text(t),
                        typed::TemplatePart::Hole(h) => typed::TemplatePart::Hole(sub(h)),
                    })
                    .collect(),
            },
            typed::ExprKind::CtxLit { bindings } => typed::ExprKind::CtxLit {
                bindings: bindings.into_iter().map(|(t, e)| (t, sub(e))).collect(),
            },
            typed::ExprKind::CtxGet { base, trait_id } => {
                typed::ExprKind::CtxGet { base: Box::new(sub(*base)), trait_id }
            }
            typed::ExprKind::Intrinsic { name, targs, args } => typed::ExprKind::Intrinsic {
                name,
                targs: targs.iter().map(|t| self.subst.resolve(t)).collect(),
                args: args.into_iter().map(sub).collect(),
            },
            typed::ExprKind::FnRef(c) => typed::ExprKind::FnRef(self.resolve_callee(c)),
            other => other,
        };
        e
    }

    fn resolve_pattern(&self, mut p: typed::Pattern) -> typed::Pattern {
        p.ty = self.subst.resolve(&p.ty);
        p.kind = match p.kind {
            typed::PatKind::Bind { local, sub } => typed::PatKind::Bind {
                local,
                sub: sub.map(|s| Box::new(self.resolve_pattern(*s))),
            },
            typed::PatKind::Tuple(ps) => {
                typed::PatKind::Tuple(ps.into_iter().map(|x| self.resolve_pattern(x)).collect())
            }
            typed::PatKind::Struct { con, fields } => typed::PatKind::Struct {
                con,
                fields: fields
                    .into_iter()
                    .map(|f| typed::FieldPat { index: f.index, pattern: self.resolve_pattern(f.pattern) })
                    .collect(),
            },
            typed::PatKind::Variant { con, variant, fields } => typed::PatKind::Variant {
                con,
                variant,
                fields: fields
                    .into_iter()
                    .map(|f| typed::FieldPat { index: f.index, pattern: self.resolve_pattern(f.pattern) })
                    .collect(),
            },
            typed::PatKind::Array { elems, rest } => typed::PatKind::Array {
                elems: elems.into_iter().map(|x| self.resolve_pattern(x)).collect(),
                rest,
            },
            typed::PatKind::Or(ps) => {
                typed::PatKind::Or(ps.into_iter().map(|x| self.resolve_pattern(x)).collect())
            }
            other => other,
        };
        p
    }

    // -- scopes -------------------------------------------------------------

    pub(crate) fn push_scope(&mut self) {
        self.scopes.push(HashMap::default());
    }

    pub(crate) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub(crate) fn new_local(&mut self, name: &str, ty: Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(typed::Local { name: name.to_string(), ty, span });
        id
    }

    pub(crate) fn bind(&mut self, name: &str, local: LocalId) {
        // Shadowing is permitted, both in nested scopes and within a block.
        self.scopes
            .last_mut()
            .or_ice("a body is checked inside a scope pushed by `push_scope`")
            .insert(name.to_string(), local);
    }

    pub(crate) fn lookup_local(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    pub(crate) fn local(&self, id: LocalId) -> &typed::Local {
        self.locals.get(id.index()).or_ice("every LocalId was minted by new_local on this body")
    }

    pub(crate) fn local_ty(&self, id: LocalId) -> Ty {
        self.local(id).ty.clone()
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

    /// The same, without the copy, for the callers that only look at the head.
    pub(crate) fn resolve_ref<'t>(&'t self, t: &'t Ty) -> &'t Ty {
        self.subst.shallow_ref(t)
    }

    pub(crate) fn prim(&self, p: Prim) -> Ty {
        self.c.tables.prim(p)
    }

    /// `as_prim` reads the head constructor and nothing else, and it runs
    /// twice per expression node through `coerce`, so it must not copy the
    /// type to ask.
    pub(crate) fn as_prim(&self, t: &Ty) -> Option<Prim> {
        self.c.tables.as_prim(self.resolve_ref(t))
    }

    pub(crate) fn show_ty(&self, t: &Ty) -> String {
        show(&self.c.tables, Some(&self.subst), &self.generics, t)
    }

    pub(crate) fn err(&mut self, span: Span, msg: impl Into<String>) -> &mut Diagnostic {
        self.c.diags.items.push(Diagnostic::error(span, msg));
        self.c.diags.items.last_mut().or_ice("the diagnostic just pushed is the last one")
    }

    pub(crate) fn error_expr(&self, span: Span) -> typed::Expr {
        typed::Expr::new(typed::ExprKind::Error, Ty::Error, span)
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
            // The one failure that is about the *kind* of type rather than a
            // missing implementation. Saying "add `derive Eq`" here would be
            // advice that cannot be taken.
            if !self.c.tables.trait_(tr).is_effect
                && self.c.tables.is_effect_carrying(&ty, &self.generics)
            {
                let d = self.err(
                    span,
                    format!("`{shown}` carries an effect, so it does not satisfy `{trait_name}`"),
                );
                d.code("missing-conformance");
                d.fix(format!(
                    "pass a type that holds no capability, or drop the `{trait_name}` bound"
                ));
                d.notes.push(
                    "a type is either part of the world or part of your data (SPEC 10.1), and \
                     that is what lets a lambda capture a `T: Ord` without laundering a context \
                     (SPEC 10.6)"
                        .into(),
                );
                continue;
            }
            if let Some(con) = ty.head() {
                let derived = self
                    .c
                    .tables
                    .impls
                    .get(&(tr, con))
                    .is_some_and(|i| i.is_derived());
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
                            crate::diagnostics::names(&has.into_iter().collect::<Vec<_>>())
                        ));
                    }
                    fix = Some(crate::compiler::semantics::types::conformance_fix(
                        &trait_name,
                        &shown,
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
        // A type is either part of the world or part of your data, and the
        // boundary is checked rather than assumed (SPEC 10.1). The nominal
        // half of that — no type implements both an effect and a trait — is
        // checked at the `impl`. This is the composite half: a type that
        // merely *mentions* an effect satisfies no ordinary bound either.
        //
        // It is what lets the capture rule exempt a bounded type parameter.
        // Without it, `struct Holder<C> { inner: C }` with a hand-written
        // `impl<C> Eq for Holder<C>` would let `Holder<Ctx>` through a
        // `T: Eq` bound, and a lambda in that function could capture the
        // capability inside it (SPEC 10.6).
        if !matches!(ty, Ty::Error | Ty::Var(_))
            && !self.c.tables.trait_(tr).is_effect
            && self.c.tables.is_effect_carrying(ty, &self.generics)
        {
            return false;
        }
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
                if let Some(imp) = self.c.tables.impls.get(&(tr, *id)) {
                    // A derived impl requires every field type to satisfy the
                    // trait too.
                    let derived = imp.is_derived();
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
        matches!(
            self.c.tables.trait_(tr).name.as_str(),
            "Eq" | "Ord" | "Show" | "Hash" | "ToJson" | "FromJson"
        )
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

