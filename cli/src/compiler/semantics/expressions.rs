//! Expression checking.
//!
//! Bidirectional: an expected type flows in wherever there is one, which is
//! what makes the inferred-type dot form (`.Some(x)`), a lambda's parameter
//! types, and a numeric literal's width all decidable in one left-to-right
//! pass without a fixpoint.

use crate::build::buildfile::nearest;
use crate::compiler::modules::Role;
use crate::compiler::semantics::inference::{Infer, LitCheck};
use crate::compiler::semantics::resolve::Sym;
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::*;
use crate::diagnostics::{self, Diagnostic, Span};
use crate::parsing::tree;

/// What a callee names, when it names something statically.
enum Static {
    Fn(FnId),
    Variant(TyConId, usize),
    Context(ContextDeclId),
    Const(ConstId),
    /// A tuple struct's name, which constructs one: `Meters(9.8)`.
    TupleStruct(TyConId),
}

/// Where a method call dispatches.
enum MethodTarget {
    /// A concrete function: one type, one module, one lookup.
    Direct(FnId),
    /// Declared by a bound, so the receiver's type is not concrete yet.
    Bound(TraitId, usize),
}

impl<'a, 'b> Infer<'a, 'b> {
    // -----------------------------------------------------------------------
    // Blocks
    // -----------------------------------------------------------------------

    pub(crate) fn check_block(&mut self, b: &tree::Block, expected: Option<&Ty>) -> typed::Expr {
        self.push_scope();
        let mut stmts = Vec::new();
        for s in &b.stmts {
            match s {
                tree::Stmt::Let { pattern, ty, value, is_ctx, span } => {
                    let stmt = self.check_let(pattern, ty.as_ref(), value, *is_ctx, *span);
                    stmts.push(stmt);
                }
                tree::Stmt::Expr { expr, span } => {
                    // An expression statement is legal only in a test source,
                    // and only when its type is `()`.
                    if self.role != Role::TestSource {
                        self.err(*span, "an expression statement is legal only in a test source").code("expression-statement")
                            .fix("bind it: `let _ = ...;`, or make it the block's result expression")
                            .notes
                            .push("a block is `let`s followed by a result expression".into());
                    }
                    let e = self.check_expr(expr, None);
                    let ty = self.resolve(&e.ty);
                    if !matches!(ty, Ty::Unit | Ty::Error) {
                        let shown = self.show_ty(&ty);
                        self.err(*span, format!("this statement has type `{shown}`, not `()`"))
                            .fix("bind it with `let _ = ...;`")
                            .notes
                            .push(
                                "only a call whose type is `()` may stand alone; bind anything \
                                 else"
                                    .into(),
                            );
                    }
                    stmts.push(typed::Stmt::Expr(e));
                }
            }
        }
        let (tail, ty) = match &b.tail {
            Some(e) => {
                let checked = self.check_expr(e, expected);
                let ty = checked.ty.clone();
                (Some(Box::new(checked)), ty)
            }
            // A block whose last item is a `let` has type `()`.
            None => (None, Ty::Unit),
        };
        self.pop_scope();
        typed::Expr::new(typed::ExprKind::Block { stmts, tail }, ty, b.span)
    }

    fn check_let(
        &mut self,
        pattern: &tree::Pattern,
        ann: Option<&tree::TypeExpr>,
        value: &tree::Expr,
        is_ctx: bool,
        span: Span,
    ) -> typed::Stmt {
        // `let ctx = ...` is legal only where a context may be built.
        if is_ctx && !self.may_build_context() {
            self.err(span, "`ctx` may be bound only where a context may be built")
                .fix("rename the binding, or move the construction into `main` or a test source")
                .notes
                .push(
                "that is `main`'s body, a test source, or a test-only module (SPEC 11.3)".into(),
            );
        }
        let expected = ann.map(|t| {
            let generics = self.generics.clone();
            self.c.elaborate(self.module, &generics, t, None)
        });
        let value_hir = self.check_expr(value, expected.as_ref());
        if let Some(exp) = &expected {
            self.unify_at(value.span(), &value_hir.ty.clone(), exp, "the annotation");
        }
        let ty = expected.unwrap_or_else(|| value_hir.ty.clone());

        // A value of type `Result<T, E>` may not be discarded by a `_`
        // pattern. Since `let _ =` is the only place a value can be thrown
        // away, the rule has no holes (SPEC 5.7.1).
        if matches!(pattern, tree::Pattern::Wild { .. }) && self.is_result(&ty) {
            self.err(span, "a `Result` may not be discarded").code("result-discarded")
                .fix(
                    "consume it: `?` to propagate, `match` to handle both cases, \
                     `result.withDefault` to supply one — or, when you really mean to drop it, \
                     the explicit and greppable `result.ignore`",
                );
        }

        self.pattern_names.clear();
        let pat = self.check_pattern(pattern, &ty);
        // The pattern in a `let` must be irrefutable. Use `match` for anything
        // else.
        if !pat.is_irrefutable(&self.c.tables) {
            self.err(pattern.span(), "this pattern does not match every value of its type").code("refutable-pattern")
                .fix("use `match`, which makes you say what the other cases do")
                .notes
                .push("a `let` binds unconditionally, so its pattern has to be irrefutable".into());
        }
        if is_ctx {
            let mut locals = Vec::new();
            pat.binds(&mut locals);
            for l in locals {
                self.effect_locals.insert(l);
            }
        } else {
            // A binding of an effect-carrying value is itself effect-carrying,
            // so the capture rule follows it.
            let mut locals = Vec::new();
            pat.binds(&mut locals);
            for l in locals {
                self.note_capture_risk(l, &ty);
            }
        }
        typed::Stmt::Let { pattern: pat, value: value_hir, span }
    }

    pub(crate) fn may_build_context(&self) -> bool {
        // Never inside a lambda, even where both are otherwise legal: without
        // that, a closure could mint authority (SPEC 11.3). And in a program,
        // only inside `main`'s body — not merely anywhere in the module that
        // exports it.
        if self.lambda_depth > 0 {
            return false;
        }
        match self.role {
            Role::Entry => self.in_main,
            other => other.may_build_context(),
        }
    }

    fn is_result(&self, ty: &Ty) -> bool {
        matches!(self.resolve(ty), Ty::Con(id, _) if self.c.known_types.get("Result") == Some(&id))
    }

    fn is_option(&self, ty: &Ty) -> bool {
        matches!(self.resolve(ty), Ty::Con(id, _) if self.c.known_types.get("Option") == Some(&id))
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    pub(crate) fn check_expr(&mut self, e: &tree::Expr, expected: Option<&Ty>) -> typed::Expr {
        let out = self.check_expr_inner(e, expected);
        self.coerce(out, expected)
    }

    /// `Str` is implicitly widened to `Template` in argument position. This is
    /// the only implicit conversion in the language, and it exists so that
    /// `io.println(ctx, "hi")` and `io.println(ctx, "hi ${name}")` are both
    /// well-typed (SPEC 3.6).
    fn coerce(&mut self, e: typed::Expr, expected: Option<&Ty>) -> typed::Expr {
        let Some(exp) = expected else { return e };
        let want = self.as_prim(exp);
        let got = self.as_prim(&e.ty);
        if want == Some(Prim::Template) && got == Some(Prim::Str) {
            let span = e.span;
            let ty = self.prim(Prim::Template);
            return typed::Expr::new(
                typed::ExprKind::Template {
                    parts: vec![typed::TemplatePart { text: None, hole: Some(e) }],
                },
                ty,
                span,
            );
        }
        e
    }

    fn check_expr_inner(&mut self, e: &tree::Expr, expected: Option<&Ty>) -> typed::Expr {
        match e {
            tree::Expr::Int { value, raw, span } => {
                // A numeric literal gets a fresh type variable constrained to
                // the integer types; ordinary unification then decides.
                let ty = self.subst.fresh_num(NumClass::Int, *span);
                if let Some(exp) = expected {
                    let _ = self.subst.unify(&self.c.tables, &ty, exp);
                }
                self.lit_checks.push(LitCheck {
                    value: *value,
                    negative: false,
                    raw: raw.clone(),
                    ty: ty.clone(),
                    span: *span,
                });
                typed::Expr::new(typed::ExprKind::Int(*value, false), ty, *span)
            }
            tree::Expr::Float { value, span, .. } => {
                let ty = self.subst.fresh_num(NumClass::Float, *span);
                if let Some(exp) = expected {
                    let _ = self.subst.unify(&self.c.tables, &ty, exp);
                }
                typed::Expr::new(typed::ExprKind::Float(*value), ty, *span)
            }
            tree::Expr::Str { value, span } => {
                let ty = self.prim(Prim::Str);
                typed::Expr::new(typed::ExprKind::Str(value.clone()), ty, *span)
            }
            tree::Expr::Char { value, span } => {
                let ty = self.prim(Prim::Char);
                typed::Expr::new(typed::ExprKind::Char(*value), ty, *span)
            }
            tree::Expr::Bool { value, span } => {
                let ty = self.prim(Prim::Bool);
                typed::Expr::new(typed::ExprKind::Bool(*value), ty, *span)
            }
            tree::Expr::Unit { span } => typed::Expr::new(typed::ExprKind::Unit, Ty::Unit, *span),
            tree::Expr::Template { parts, span } => self.check_template(parts, *span),
            tree::Expr::Ident { name, span } => self.check_ident(name, *span, expected),
            tree::Expr::SelfValue { span } => match self.lookup_local("self") {
                Some(l) => typed::Expr::new(typed::ExprKind::Local(l), self.local_ty(l), *span),
                None => {
                    self.err(*span, "`self` is legal only in a method body")
                        .fix("name a parameter instead, or move this into an `impl` block");
                    self.error_expr(*span)
                }
            },
            tree::Expr::Ctx { span } => match self.lookup_local("ctx") {
                Some(l) => typed::Expr::new(typed::ExprKind::Local(l), self.local_ty(l), *span),
                None => {
                    self.err(*span, "there is no `ctx` in scope")
                        .fix("add a `ctx` parameter bounded by the effects this function needs")
                        .notes
                        .push(
                        "a function that needs an effect declares a `ctx` parameter bounded by \
                         the effects it needs"
                            .into(),
                    );
                    self.error_expr(*span)
                }
            },
            tree::Expr::DotVariant { name, span } => {
                self.check_dot_variant(name, &[], *span, expected)
            }
            tree::Expr::Array { elems, span } => self.check_array(elems, *span, expected),
            tree::Expr::Tuple { elems, span } => {
                let want: Vec<Option<Ty>> = match expected.map(|t| self.resolve(t)) {
                    Some(Ty::Tuple(ts)) if ts.len() == elems.len() => {
                        ts.into_iter().map(Some).collect()
                    }
                    _ => vec![None; elems.len()],
                };
                let checked: Vec<typed::Expr> = elems
                    .iter()
                    .zip(want)
                    .map(|(x, w)| self.check_expr(x, w.as_ref()))
                    .collect();
                let ty = Ty::Tuple(checked.iter().map(|c| c.ty.clone()).collect());
                typed::Expr::new(typed::ExprKind::Tuple(checked), ty, *span)
            }
            tree::Expr::Block(b) => self.check_block(b, expected),
            tree::Expr::If { cond, then, else_, span } => {
                let bool_ty = self.prim(Prim::Bool);
                let c = self.check_expr(cond, Some(&bool_ty));
                // The condition must have type `Bool`. There is no truthiness.
                self.unify_at(cond.span(), &c.ty.clone(), &bool_ty, "an `if` condition");
                let t = self.check_block(then, expected);
                let f = self.check_expr(else_, expected.or(Some(&t.ty.clone())));
                // Both branches must have the same type.
                self.unify_at(else_.span(), &f.ty.clone(), &t.ty.clone(), "the other branch");
                let ty = t.ty.clone();
                typed::Expr::new(
                    typed::ExprKind::If { cond: Box::new(c), then: Box::new(t), else_: Box::new(f) },
                    ty,
                    *span,
                )
            }
            tree::Expr::Match { scrutinee, arms, span } => {
                self.check_match(scrutinee, arms, *span, expected)
            }
            tree::Expr::ContextExpr { body, span } => self.check_context_body(body, *span),
            tree::Expr::Lambda { params, ret, body, span } => {
                self.check_lambda(params, ret.as_ref(), body, *span, expected)
            }
            tree::Expr::Unary { op, operand, span } => self.check_unary(*op, operand, *span, expected),
            tree::Expr::Binary { op, lhs, rhs, op_span, span } => {
                self.check_binary(*op, lhs, rhs, *op_span, *span, expected)
            }
            tree::Expr::Field { base, name, span } => self.check_field(base, name, *span, expected),
            tree::Expr::TupleIndex { base, index, index_span, span } => {
                let b = self.check_expr(base, None);
                let bty = self.resolve(&b.ty);
                match &bty {
                    Ty::Tuple(elems) => match elems.get(*index as usize) {
                        Some(t) => typed::Expr::new(
                            typed::ExprKind::TupleIndex { base: Box::new(b), index: *index as usize },
                            t.clone(),
                            *span,
                        ),
                        None => {
                            let n = elems.len();
                            self.err(*index_span, format!("a {n}-tuple has no element {index}"))
                                .fix(format!("the elements are `.0` through `.{}`", n - 1));
                            self.error_expr(*span)
                        }
                    },
                    // A tuple struct's fields are `.0`, `.1`, ...
                    Ty::Con(con, args) => {
                        let fields = self.c.tables.tycon(*con).fields().to_vec();
                        match fields.get(*index as usize) {
                            Some(f) => {
                                self.check_field_visible(*con, f, *index_span);
                                let ty = substitute(&f.ty, args, None);
                                typed::Expr::new(
                                    typed::ExprKind::Field { base: Box::new(b), index: *index as usize },
                                    ty,
                                    *span,
                                )
                            }
                            None => {
                                let n = self.c.tables.tycon(*con).name.clone();
                                self.err(*index_span, format!("`{n}` has no field {index}"))
                                    .fix(format!("`{n}` has {} fields", fields.len()));
                                self.error_expr(*span)
                            }
                        }
                    }
                    _ => {
                        if !bty.is_error() {
                            let shown = self.show_ty(&bty);
                            self.err(*span, format!("`{shown}` is not a tuple"))
                                .fix("index a tuple or a tuple struct; name a field otherwise");
                        }
                        self.error_expr(*span)
                    }
                }
            }
            tree::Expr::Call { callee, args, span } => self.check_call(callee, args, *span, expected),
            tree::Expr::Index { base, index, span } => {
                let b = self.check_expr(base, None);
                let int_ty = self.prim(Prim::I64);
                let i = self.check_expr(index, Some(&int_ty));
                self.unify_at(index.span(), &i.ty.clone(), &int_ty, "an index");
                let bty = self.resolve(&b.ty);
                let elem = match &bty {
                    Ty::Array(e) => (**e).clone(),
                    Ty::Error => Ty::Error,
                    other => {
                        let shown = self.show_ty(other);
                        self.err(*span, format!("`{shown}` cannot be indexed"))
                            .fix("index an array; for a tuple, write `.0`")
                            .notes
                            .push("indexing is defined on `[T]`, and yields `Option<T>`".into());
                        Ty::Error
                    }
                };
                // Indexing yields `Option<T>`. There is no way to index out of
                // bounds and no way to panic by indexing.
                let ty = self.option_of(elem.clone());
                typed::Expr::new(
                    typed::ExprKind::Index { base: Box::new(b), index: Box::new(i), elem },
                    ty,
                    *span,
                )
            }
            tree::Expr::Try { base, span } => self.check_try(base, *span),
            tree::Expr::TurboFish { base, args, span } => {
                // A turbofish only ever qualifies a callee; on its own it is a
                // function reference.
                let targs = self.elaborate_all(args);
                match self.static_ref(base) {
                    Some(Static::Fn(f)) => self.fn_ref(f, Some(targs), *span),
                    _ => {
                        self.err(*span, "explicit type arguments qualify a function or a call")
                        .fix("attach the turbofish to the call, as in `f::<Str>(x)`");
                        self.error_expr(*span)
                    }
                }
            }
            tree::Expr::StructLit { head, spread, fields, span } => {
                self.check_struct_lit(head, spread.as_deref(), fields, *span, expected)
            }
        }
    }

    fn elaborate_all(&mut self, args: &[tree::TypeExpr]) -> Vec<Ty> {
        let generics = self.generics.clone();
        args.iter().map(|a| self.c.elaborate(self.module, &generics, a, None)).collect()
    }

    fn option_of(&self, t: Ty) -> Ty {
        match self.c.known_types.get("Option") {
            Some(id) => Ty::Con(*id, vec![t]),
            None => Ty::Error,
        }
    }

    // -----------------------------------------------------------------------
    // Names
    // -----------------------------------------------------------------------

    fn check_ident(&mut self, name: &str, span: Span, expected: Option<&Ty>) -> typed::Expr {
        if let Some(local) = self.lookup_local(name) {
            return typed::Expr::new(typed::ExprKind::Local(local), self.local_ty(local), span);
        }
        let sym = self.c.scopes[self.module.index()].names.get(name).cloned();
        match sym {
            Some(Sym::Fn(f)) => self.fn_ref(f, None, span),
            Some(Sym::Const(cid)) => {
                let ty = self.c.tables.const_(cid).ty.clone();
                typed::Expr::new(typed::ExprKind::Const(cid), ty, span)
            }
            Some(Sym::Context(cid)) => {
                self.err(span, format!("`{name}` is a context; construct one by calling it"))
                    .fix(format!("write `{name}()`"))
                    .notes
                    .push(
                        "each call builds a fresh context, so two tests never share one's state"
                            .into(),
                    );
                let _ = cid;
                self.error_expr(span)
            }
            Some(Sym::Method(owner)) => {
                self.err(span, format!("`{name}` is a method, not a value"))
                    .fix(format!(
                        "call it on a receiver: `x.{name}(...)`, where `x` is a `{owner}`; to \
                         pass it on, wrap it in a lambda"
                    ));
                self.error_expr(span)
            }
            Some(Sym::Overloaded(fs)) => {
                let names: Vec<String> = fs
                    .iter()
                    .map(|f| {
                        let i = self.c.tables.fun(*f);
                        match &i.self_ty {
                            Some(c) => format!("{}.{}", self.c.tables.tycon(*c).name, i.name),
                            None => i.name.clone(),
                        }
                    })
                    .collect();
                self.err(span, format!("`{name}` is ambiguous as a free function"))
                    .fix(format!("call it on a receiver, which is what picks the one you mean"))
                    .notes
                    .push(format!(
                        "it is a method on several types: {}",
                        diagnostics::names(&names)
                    ));
                self.error_expr(span)
            }
            Some(Sym::Ty(_)) | Some(Sym::Trait(_)) | Some(Sym::Namespace(_)) => {
                self.err(span, format!("`{name}` is a type, not a value"))
                    .fix("construct one, as in `Point { x: 1, y: 2 }`, or name a value");
                self.error_expr(span)
            }
            None => {
                let _ = expected;
                let mut note = None;
                if self.c.scopes[self.module.index()].namespaces.contains_key(name) {
                    note = Some(format!("`{name}` is a module namespace; name a member of it"));
                } else if let Some(near) = self.nearest_value(name) {
                    note = Some(format!("did you mean `{near}`?"));
                }
                let fix = match &note {
                    Some(_) => "correct the spelling, or declare it".to_string(),
                    None => format!(
                        "declare `{name}`, or import it — a name is in scope only from this \
                         module's own declarations and its imports"
                    ),
                };
                let d = self.err(span, format!("there is nothing named `{name}` in scope")).code("unresolved-name");
                d.fix(fix);
                if let Some(n) = note {
                    d.notes.push(n);
                }
                self.error_expr(span)
            }
        }
    }

    fn nearest_value(&self, name: &str) -> Option<String> {
        let mut candidates: Vec<String> =
            self.scopes.iter().flat_map(|s| s.keys().cloned()).collect();
        candidates.extend(self.c.scopes[self.module.index()].names.keys().cloned());
        let refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
        nearest(name, &refs).map(|s| s.to_string())
    }

    fn fn_ref(&mut self, f: FnId, explicit: Option<Vec<Ty>>, span: Span) -> typed::Expr {
        let info = self.c.tables.fun(f).clone();
        let targs = self.instantiate(&info.generics, explicit, span);
        let params: Vec<Ty> =
            info.params.iter().map(|p| substitute(&p.ty, &targs, None)).collect();
        let ret = substitute(&info.ret, &targs, None);
        let ty = Ty::Fn(params, Box::new(ret));
        typed::Expr::new(typed::ExprKind::FnRef(f, targs), ty, span)
    }

    /// Fresh variables for a generic item's parameters, with each bound
    /// recorded as an obligation to discharge once inference has settled.
    fn instantiate(
        &mut self,
        generics: &[GenericInfo],
        explicit: Option<Vec<Ty>>,
        span: Span,
    ) -> Vec<Ty> {
        let targs: Vec<Ty> = match explicit {
            Some(ts) if ts.len() == generics.len() => ts,
            Some(ts) => {
                let want = generics.len();
                let got = ts.len();
                self.err(span, format!("expected {want} type arguments, found {got}"))
                    .mismatch(want.to_string(), got.to_string())
                    .fix(format!("supply exactly {want}"));
                (0..generics.len()).map(|_| self.fresh(span)).collect()
            }
            None => (0..generics.len()).map(|_| self.fresh(span)).collect(),
        };
        for (g, t) in generics.iter().zip(&targs) {
            for b in &g.bounds {
                self.require(t.clone(), *b, span);
            }
        }
        targs
    }

    /// Resolves a callee that names something statically: a function, an enum
    /// variant, a named context, or a constant.
    fn static_ref(&mut self, e: &tree::Expr) -> Option<Static> {
        match e {
            tree::Expr::Ident { name, .. } => {
                if self.lookup_local(name).is_some() {
                    return None;
                }
                match self.c.scopes[self.module.index()].names.get(name).cloned()? {
                    Sym::Fn(f) => Some(Static::Fn(f)),
                    Sym::Context(c) => Some(Static::Context(c)),
                    Sym::Const(c) => Some(Static::Const(c)),
                    Sym::Ty(con) => self.tuple_struct(con),
                    _ => None,
                }
            }
            tree::Expr::Field { base, name, .. } => {
                let tree::Expr::Ident { name: head, .. } = &**base else { return None };
                if self.lookup_local(head).is_some() {
                    return None;
                }
                // `list.map` — a member of a namespace import.
                if let Some(ns) = self.c.scopes[self.module.index()].namespaces.get(head).copied() {
                    return match self.c.lookup_export_pub(ns, &name.name)? {
                        Sym::Fn(f) => Some(Static::Fn(f)),
                        Sym::Context(c) => Some(Static::Context(c)),
                        Sym::Const(c) => Some(Static::Const(c)),
                        Sym::Ty(con) => self.tuple_struct(con),
                        _ => None,
                    };
                }
                // `Shape.Circle` — a qualified variant.
                if let Some(Sym::Ty(con)) =
                    self.c.scopes[self.module.index()].names.get(head).cloned()
                {
                    let index = self.c.tables.tycon(con).variant_index(&name.name)?;
                    return Some(Static::Variant(con, index));
                }
                None
            }
            tree::Expr::TurboFish { base, .. } => self.static_ref(base),
            _ => None,
        }
    }

    /// A tuple struct's name is also its constructor, and `struct Meters(F64)`
    /// is the newtype pattern the operator traits exist for.
    fn tuple_struct(&self, con: TyConId) -> Option<Static> {
        match &self.c.tables.tycon(con).def {
            TyDef::Struct { record: false, .. } => Some(Static::TupleStruct(con)),
            _ => None,
        }
    }

    fn construct_tuple_struct(
        &mut self,
        con: TyConId,
        args: &[tree::Expr],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let tycon = self.c.tables.tycon(con).clone();
        let fields = tycon.fields().to_vec();
        let targs: Vec<Ty> = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Con(c, ts)) if c == con => ts,
            _ => (0..tycon.arity()).map(|_| self.fresh(span)).collect(),
        };
        if args.len() != fields.len() {
            let n = tycon.name.clone();
            let have = args.len();
            let want = fields.len();
            self.err(
                span,
                format!("`{n}` holds {want} values, but {have} were given"),
            )
            .mismatch(want.to_string(), have.to_string())
            .fix(format!("pass exactly {want}"));
        }
        // A struct with any private field cannot be constructed from scratch
        // outside its module.
        for f in &fields {
            self.check_field_visible(con, f, span);
        }
        let param_types: Vec<Ty> =
            fields.iter().map(|f| substitute(&f.ty, &targs, None)).collect();
        let checked = self.check_args(args, &param_types);
        let ty = Ty::Con(con, targs.clone());
        typed::Expr::new(typed::ExprKind::StructLit { con, targs, fields: checked }, ty, span)
    }

    // -----------------------------------------------------------------------
    // Calls
    // -----------------------------------------------------------------------

    fn check_call(
        &mut self,
        callee: &tree::Expr,
        args: &[tree::Expr],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // `.Some(x)` — the inferred-type dot form as a constructor.
        if let tree::Expr::DotVariant { name, span: dspan } = callee {
            return self.check_dot_variant_call(name, args, *dspan, span, expected);
        }

        let explicit = match callee {
            tree::Expr::TurboFish { args: targs, .. } => Some(self.elaborate_all(targs)),
            _ => None,
        };

        // A method call has no production of its own: it is a `Field` whose
        // base is a value.
        if let tree::Expr::Field { base, name, .. } = strip_turbofish(callee) {
            if self.static_ref(callee).is_none() {
                return self.check_method_call(base, name, args, explicit, span, expected);
            }
        }

        match self.static_ref(callee) {
            Some(Static::Fn(f)) => self.call_fn(f, explicit, args, span, None, expected),
            Some(Static::Variant(con, index)) => {
                self.construct_variant(con, index, args, span, expected, callee.span())
            }
            Some(Static::TupleStruct(con)) => {
                self.construct_tuple_struct(con, args, span, expected)
            }
            Some(Static::Context(cid)) => {
                if !args.is_empty() {
                    self.err(span, "a context declaration takes no parameters")
                        .fix("call it with no arguments; override what varies with `context { ..base, ... }`")
                        .notes
                        .push(
                        "what varies between call sites is expressed by overriding, not by \
                         arguments"
                            .into(),
                    );
                }
                if !self.may_build_context() {
                    self.err(span, "a context may not be constructed here").code("context-not-allowed")
                        .fix(
                            "build it in `main` and pass it down as a `ctx` parameter, or make \
                             this a test source, where a context may be built per test",
                        )
                        .notes
                        .push(
                            "only in `main`'s body, a test source, or a test-only module — and \
                             never inside a lambda"
                                .into(),
                        );
                }
                let ty = match self.c.tables.ctx_decl(cid).ty {
                    Some(t) => Ty::Ctx(t),
                    None => Ty::Error,
                };
                typed::Expr::new(typed::ExprKind::CtxCall { decl: cid }, ty, span)
            }
            _ => {
                // A call through a value of function type.
                let c = self.check_expr(callee, None);
                let cty = self.resolve(&c.ty);
                match cty {
                    Ty::Fn(params, ret) => {
                        if params.len() != args.len() {
                            let want = params.len();
                            let got = args.len();
                            self.err(span, format!("expected {want} arguments, found {got}"))
                                .mismatch(want.to_string(), got.to_string())
                                .fix(format!("pass exactly {want}"));
                        }
                        let checked = self.check_args(args, &params);
                        typed::Expr::new(
                            typed::ExprKind::CallValue { callee: Box::new(c), args: checked },
                            *ret,
                            span,
                        )
                    }
                    Ty::Error => self.error_expr(span),
                    other => {
                        let shown = self.show_ty(&other);
                        self.err(callee.span(), format!("`{shown}` is not callable"))
                            .fix("call a function, a lambda, or a field holding one — `(x.f)(...)` for a field");
                        self.error_expr(span)
                    }
                }
            }
        }
    }

    /// Arguments are evaluated left to right before the call, and each is
    /// checked against its parameter's type so that a literal is pinned by it.
    fn check_args(&mut self, args: &[tree::Expr], params: &[Ty]) -> Vec<typed::Expr> {
        args.iter()
            .enumerate()
            .map(|(i, a)| {
                let want = params.get(i).cloned();
                let checked = self.check_expr(a, want.as_ref());
                if let Some(w) = &want {
                    self.unify_at(a.span(), &checked.ty.clone(), w, "the parameter type");
                }
                checked
            })
            .collect()
    }

    fn call_fn(
        &mut self,
        f: FnId,
        explicit: Option<Vec<Ty>>,
        args: &[tree::Expr],
        span: Span,
        receiver: Option<typed::Expr>,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let info = self.c.tables.fun(f).clone();
        let targs = self.instantiate(&info.generics, explicit, span);
        let self_ty = receiver.as_ref().map(|r| self.resolve(&r.ty));
        let params: Vec<Ty> = info
            .params
            .iter()
            .map(|p| substitute(&p.ty, &targs, self_ty.as_ref()))
            .collect();
        let ret = substitute(&info.ret, &targs, self_ty.as_ref());

        // Type information flows outside-in: unifying the return type with
        // what the call site wants *before* visiting the arguments is what
        // lets `xs.fold(f, .None)` know which enum `.None` names, and what
        // pins a numeric literal argument to the width the callee declared.
        if let Some(exp) = expected {
            let _ = self.subst.unify(&self.c.tables, &ret, exp);
        }

        let expected_args = if receiver.is_some() { params.len() - 1 } else { params.len() };
        if args.len() != expected_args {
            let name = info.name.clone();
            let have = args.len();
            let mut d = Diagnostic::error(
                span,
                format!("`{name}` takes {expected_args} arguments, but {have} were given"),
            )
            .with_mismatch(expected_args.to_string(), have.to_string())
            .with_fix(format!("pass exactly {expected_args}"));
            // The most common cause is forgetting the context, which is always
            // the parameter right after the receiver.
            if info.params.iter().any(|p| p.role == ParamRole::Ctx) && have + 1 == expected_args {
                d = d.with_fix("pass the context: the convention is receiver first, context second, everything else after");
            }
            self.c.diags.push(d);
        }

        let mut hir_args = Vec::new();
        let mut param_types = params.clone();
        if let Some(r) = receiver {
            if !param_types.is_empty() {
                let want = param_types.remove(0);
                self.unify_at(r.span, &r.ty.clone(), &want, "the receiver");
            }
            hir_args.push(r);
        }
        hir_args.extend(self.check_args(args, &param_types));
        typed::Expr::new(typed::ExprKind::CallFn { func: f, targs, args: hir_args }, ret, span)
    }

    // -----------------------------------------------------------------------
    // Method calls
    // -----------------------------------------------------------------------

    fn check_method_call(
        &mut self,
        base: &tree::Expr,
        name: &tree::Ident,
        args: &[tree::Expr],
        explicit: Option<Vec<Ty>>,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let recv = self.check_expr(base, None);
        // A literal that reaches a method call with nothing else constraining
        // it takes its default — `Int` for an integer literal, `Float` for a
        // float — so `5.abs()` resolves in `core/num` (SPEC 5.1.1).
        let recv_ty = self.default_numeric_receiver(&recv.ty);

        // A field of function type is called as `(x.f)(...)`.
        if let Ty::Con(con, targs) = &recv_ty {
            if let Some(i) = self.c.tables.tycon(*con).field_index(&name.name) {
                let f = self.c.tables.tycon(*con).fields()[i].clone();
                self.check_field_visible(*con, &f, name.span);
                let fty = substitute(&f.ty, targs, None);
                let base_hir =
                    typed::Expr::new(typed::ExprKind::Field { base: Box::new(recv), index: i }, fty.clone(), span);
                return match self.resolve(&fty) {
                    Ty::Fn(params, ret) => {
                        let checked = self.check_args(args, &params);
                        typed::Expr::new(
                            typed::ExprKind::CallValue { callee: Box::new(base_hir), args: checked },
                            *ret,
                            span,
                        )
                    }
                    other => {
                        let shown = self.show_ty(&other);
                        self.err(span, format!("field `{}` has type `{shown}`, which is not callable", name.name))
                            .fix("this is a field access, not a method call");
                        self.error_expr(span)
                    }
                };
            }
        }

        match self.resolve_method(&recv_ty, &name.name, name.span) {
            Some(MethodTarget::Direct(f)) => {
                self.call_fn(f, explicit, args, span, Some(recv), expected)
            }
            Some(MethodTarget::Bound(tid, index)) => {
                self.call_trait_method(tid, index, recv, args, span, expected)
            }
            None => {
                self.report_no_method(&recv_ty, &name.name, name.span);
                self.error_expr(span)
            }
        }
    }

    /// Resolving `x.f()` needs the receiver's head type constructor. An
    /// unpinned numeric literal has none yet, so this is where its default
    /// applies.
    fn default_numeric_receiver(&mut self, ty: &Ty) -> Ty {
        let resolved = self.resolve(ty);
        let Ty::Var(id) = resolved else { return resolved };
        let Some(class) = self.subst.class_of(id) else { return resolved };
        let default = match class {
            NumClass::Int => self.prim(Prim::I64),
            NumClass::Float => self.prim(Prim::F64),
        };
        let _ = self.subst.unify(&self.c.tables, &Ty::Var(id), &default);
        self.resolve(ty)
    }

    /// Three steps, each a lookup rather than a search (SPEC 6.7.3).
    fn resolve_method(&mut self, recv: &Ty, name: &str, span: Span) -> Option<MethodTarget> {
        match recv {
            // If the receiver's type is concrete, the method is in that type's
            // defining module.
            Ty::Con(con, _) => {
                if let Some(f) = self.c.tables.methods.get(&(*con, name.to_string())).copied() {
                    self.check_method_visible(f, span);
                    return Some(MethodTarget::Direct(f));
                }
                // Methods supplied by an `impl` land in the type's ordinary
                // method namespace — including the ones `derive` generates,
                // which have no function of their own to point at.
                let traits: Vec<TraitId> = self
                    .c
                    .tables
                    .impls
                    .keys()
                    .filter(|(_, c)| c == con)
                    .map(|(t, _)| *t)
                    .collect();
                let mut sorted = traits;
                sorted.sort();
                self.find_in_bounds(&sorted, name, span)
            }
            // The defining module of `[T]` is `core/list`.
            Ty::Array(_) => self
                .c
                .tables
                .array_methods
                .get(name)
                .copied()
                .map(MethodTarget::Direct),
            // If the receiver is a type parameter, the method must be declared
            // by one of its bounds. A bare parameter with no bounds has no
            // methods.
            Ty::Param(i) => {
                let bounds = self.generics.get(*i as usize)?.bounds.clone();
                self.find_in_bounds(&bounds, name, span)
            }
            // A context value satisfies exactly the effects it binds.
            Ty::Ctx(id) => {
                let bounds: Vec<TraitId> =
                    self.c.tables.ctx_type(*id).bindings.iter().map(|(t, _)| *t).collect();
                self.find_in_bounds(&bounds, name, span)
            }
            Ty::SelfTy => {
                let con = self.self_con?;
                self.resolve_method(&Ty::Con(con, Vec::new()), name, span)
            }
            _ => None,
        }
    }

    fn find_in_bounds(
        &mut self,
        bounds: &[TraitId],
        name: &str,
        span: Span,
    ) -> Option<MethodTarget> {
        let mut found: Option<(TraitId, usize)> = None;
        for b in bounds {
            if let Some(i) = self.c.tables.trait_(*b).method_index(name) {
                // Where two bounds declare the same method name, the call is
                // ambiguous and must be disambiguated by calling the trait
                // method as a function.
                if let Some((prev, _)) = found {
                    let a = self.c.tables.trait_(prev).name.clone();
                    let c = self.c.tables.trait_(*b).name.clone();
                    self.err(span, format!("`{name}` is declared by both `{a}` and `{c}`"))
                        .fix(format!("call it as a function to say which: `{a}.{name}(x, y)`"));
                    return Some(MethodTarget::Bound(prev, found.unwrap().1));
                }
                found = Some((*b, i));
            }
        }
        found.map(|(t, i)| MethodTarget::Bound(t, i))
    }

    fn call_trait_method(
        &mut self,
        tid: TraitId,
        index: usize,
        recv: typed::Expr,
        args: &[tree::Expr],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let method = self.c.tables.trait_(tid).methods[index].clone();
        let recv_ty = self.resolve(&recv.ty);
        // A trait method's own generics come after the trait's; the receiver
        // supplies `Self`.
        let targs = self.instantiate(&method.generics, None, span);
        let params: Vec<Ty> = method
            .params
            .iter()
            .map(|p| substitute(&p.ty, &targs, Some(&recv_ty)))
            .collect();
        let ret = substitute(&method.ret, &targs, Some(&recv_ty));
        if let Some(exp) = expected {
            let _ = self.subst.unify(&self.c.tables, &ret, exp);
        }

        let mut hir_args = vec![recv];
        let rest = if params.is_empty() { &[][..] } else { &params[1..] };
        if args.len() != rest.len() {
            let name = method.name.clone();
            let want = rest.len();
            let have = args.len();
            self.err(span, format!("`{name}` takes {want} arguments, but {have} were given"))
                .mismatch(want.to_string(), have.to_string())
                .fix(format!("pass exactly {want}"));
        }
        hir_args.extend(self.check_args(args, rest));
        typed::Expr::new(
            typed::ExprKind::CallTrait { trait_id: tid, method: index, recv: recv_ty, targs, args: hir_args },
            ret,
            span,
        )
    }

    /// A name is on a library's surface if its `lib.buri` exports it, and a
    /// method call from outside the library resolves only to names on the
    /// surface. Resolution itself does not change — one type, one module, one
    /// lookup — with a visibility filter applied after it.
    fn check_method_visible(&mut self, f: FnId, span: Span) {
        let info = self.c.tables.fun(f).clone();
        if info.module.0 == u32::MAX || info.module == self.module {
            return;
        }
        // A method supplied by an `impl` is visible wherever the type is:
        // conformance is a property of the type.
        if info.impl_of.is_some() {
            return;
        }
        if !info.exported {
            let name = info.name.clone();
            self.err(span, format!("`{name}` is private to its module"))
                .fix(format!("add `export` to `{name}`'s declaration, if it is meant to be part of the API"));
            return;
        }
        let (Some(here), Some(there)) =
            (self.c.module(self.module).pkg, self.c.module(info.module).pkg)
        else {
            return;
        };
        if here == there {
            return;
        }
        if let Some(surface) = self.c.surfaces.get(&there) {
            if !surface.contains(&info.name) {
                let name = info.name.clone();
                let label =
                    self.c.ws.map(|w| w.pkg(there).label()).unwrap_or_default();
                self.err(span, format!("`{name}` is not on {label}'s surface"))
                    .fix(format!("re-export `{name}` from that library's lib.buri, if it is part of the API"))
                    .notes
                    .push(
                        "a method call from outside a library resolves only to names its \
                         lib.buri exports"
                            .to_string(),
                    );
            }
        }
    }

    fn check_field_visible(&mut self, con: TyConId, f: &FieldInfo, span: Span) {
        let owner = self.c.tables.tycon(con).module;
        if owner == self.module || owner.0 == u32::MAX || f.exported {
            return;
        }
        let ty = self.c.tables.tycon(con).name.clone();
        let name = f.name.clone();
        self.err(span, format!("field `{name}` of `{ty}` is private to its module"))
            .fix(format!("add `export` to the field, or go through a method `{ty}` provides"))
            .notes
            .push(
                "a struct with any private field cannot be constructed or destructured \
                 elsewhere, though functional update still works because it never names the \
                 hidden fields"
                    .into(),
            );
    }

    fn report_no_method(&mut self, recv: &Ty, name: &str, span: Span) {
        if recv.is_error() {
            return;
        }
        let shown = self.show_ty(recv);
        let mut notes = Vec::new();
        match recv {
            Ty::Param(i) => {
                let g = self.generics.get(*i as usize).cloned();
                match g {
                    Some(g) if g.bounds.is_empty() => notes.push(format!(
                        "`{shown}` carries no bounds, and a bare type parameter has no methods; \
                         add a bound that declares `{name}`"
                    )),
                    Some(g) => {
                        let names: Vec<String> = g
                            .bounds
                            .iter()
                            .map(|b| self.c.tables.trait_(*b).name.clone())
                            .collect();
                        notes.push(format!(
                            "`{shown}` is bounded by {}, and none of them declares `{name}`",
                            names.join(" + ")
                        ));
                    }
                    None => {}
                }
            }
            Ty::Con(con, _) => {
                let names: Vec<String> = self
                    .c
                    .tables
                    .methods
                    .keys()
                    .filter(|(c, _)| c == con)
                    .map(|(_, n)| n.clone())
                    .collect();
                let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                if let Some(near) = nearest(name, &refs) {
                    notes.push(format!("did you mean `{near}`?"));
                }
                if self.c.tables.tycon(*con).field_index(name).is_some() {
                    notes.push(format!("`{name}` is a field; a field is not called"));
                }
            }
            Ty::Tuple(_) | Ty::Fn(..) => {
                notes.push(
                    "tuples, function types, and `Template` have no defining module, so they \
                     have no methods"
                        .into(),
                );
            }
            _ => {}
        }
        let d = self.err(span, format!("`{shown}` has no method `{name}`")).code("no-such-method");
        d.fix(format!(
            "check the spelling, or declare it in `impl {shown} {{ ... }}` in that type's own \
             module — a method may not be added from anywhere else"
        ));
        d.notes.extend(notes);
    }

    // -----------------------------------------------------------------------
    // Fields, variants, literals
    // -----------------------------------------------------------------------

    fn check_field(
        &mut self,
        base: &tree::Expr,
        name: &tree::Ident,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // A namespace member, a qualified variant, or a constant.
        if let Some(s) = self.static_ref(&tree::Expr::Field {
            base: Box::new(base.clone()),
            name: name.clone(),
            span,
        }) {
            return match s {
                Static::Fn(f) => self.fn_ref(f, None, span),
                Static::Const(c) => {
                    let ty = self.c.tables.const_(c).ty.clone();
                    typed::Expr::new(typed::ExprKind::Const(c), ty, span)
                }
                Static::Variant(con, index) => {
                    self.construct_variant(con, index, &[], span, expected, span)
                }
                Static::Context(_) => {
                    self.err(span, "construct a context by calling it")
                        .fix("add `()`");
                    self.error_expr(span)
                }
                Static::TupleStruct(_) => {
                    self.err(span, "a tuple struct's name constructs one; call it")
                        .fix("write `Name(value)`");
                    self.error_expr(span)
                }
            };
        }

        let b = self.check_expr(base, None);
        let bty = self.resolve(&b.ty);
        match &bty {
            Ty::Con(con, targs) => {
                if let Some(i) = self.c.tables.tycon(*con).field_index(&name.name) {
                    let f = self.c.tables.tycon(*con).fields()[i].clone();
                    self.check_field_visible(*con, &f, name.span);
                    let ty = substitute(&f.ty, targs, None);
                    return typed::Expr::new(
                        typed::ExprKind::Field { base: Box::new(b), index: i },
                        ty,
                        span,
                    );
                }
                // A method is not a value: `x.f` must be immediately called.
                if self.c.tables.methods.contains_key(&(*con, name.name.clone())) {
                    let n = name.name.clone();
                    self.err(span, format!("`{n}` is a method, and a method is not a value")).code("method-not-a-value")
                        .fix(format!(
                            "call it on a receiver: `x.{n}()`; to pass it on, wrap it in a \
                             lambda: `fn(x) => x.{n}()`"
                        ));
                    return self.error_expr(span);
                }
                self.report_no_field(&bty, &name.name, name.span);
                self.error_expr(span)
            }
            Ty::Error => self.error_expr(span),
            _ => {
                self.report_no_field(&bty, &name.name, name.span);
                self.error_expr(span)
            }
        }
    }

    fn report_no_field(&mut self, ty: &Ty, name: &str, span: Span) {
        let shown = self.show_ty(ty);
        let mut note = None;
        if let Ty::Con(con, _) = ty {
            let names: Vec<String> =
                self.c.tables.tycon(*con).fields().iter().map(|f| f.name.clone()).collect();
            let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            note = nearest(name, &refs).map(|n| format!("did you mean `{n}`?"));
        }
        let d = self.err(span, format!("`{shown}` has no field `{name}`"));
        d.fix("check the spelling, or name a field the type declares");
        if let Some(n) = note {
            d.notes.push(n);
        }
    }

    fn check_dot_variant(
        &mut self,
        name: &tree::Ident,
        args: &[tree::Expr],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        self.check_dot_variant_call(name, args, span, span, expected)
    }

    /// The dot form requires that the expected type is known from context
    /// (SPEC 14.12).
    fn check_dot_variant_call(
        &mut self,
        name: &tree::Ident,
        args: &[tree::Expr],
        dot_span: Span,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let Some(exp) = expected.map(|t| self.resolve(t)) else {
            let n = name.name.clone();
            self.err(dot_span, format!("`.{n}` needs a known expected type")).code("unannotated-variant")
                .fix(format!(
                    "write the qualified form, as in `Option.{n}(...)`, or annotate what this \
                     value is being used as"
                ));
            return self.error_expr(span);
        };
        let Ty::Con(con, _) = &exp else {
            if !exp.is_error() {
                let shown = self.show_ty(&exp);
                self.err(dot_span, format!("`{shown}` is not an enum"))
                    .fix("the dot form names an enum variant; write the value another way");
            }
            return self.error_expr(span);
        };
        let Some(index) = self.c.tables.tycon(*con).variant_index(&name.name) else {
            let ty = self.c.tables.tycon(*con).name.clone();
            let variants: Vec<String> =
                self.c.tables.tycon(*con).variants().iter().map(|v| v.name.clone()).collect();
            let refs: Vec<&str> = variants.iter().map(|s| s.as_str()).collect();
            let near = nearest(&name.name, &refs).map(|s| s.to_string());
            let n = name.name.clone();
            let d = self.err(dot_span, format!("`{ty}` has no variant `{n}`"));
            d.fix("name a variant the enum declares");
            match near {
                Some(x) => d.notes.push(format!("did you mean `.{x}`?")),
                None if !variants.is_empty() => {
                    d.notes.push(format!("its variants are {}", diagnostics::names(&variants)))
                }
                None => {}
            }
            return self.error_expr(span);
        };
        self.construct_variant(*con, index, args, span, Some(&exp), dot_span)
    }

    fn construct_variant(
        &mut self,
        con: TyConId,
        index: usize,
        args: &[tree::Expr],
        span: Span,
        expected: Option<&Ty>,
        head_span: Span,
    ) -> typed::Expr {
        let tycon = self.c.tables.tycon(con).clone();
        let variant = tycon.variants()[index].clone();

        // A type with any unexported variant cannot be constructed outside its
        // module.
        if tycon.module != self.module && !variant.exported && tycon.module.0 != u32::MAX {
            let t = tycon.name.clone();
            let v = variant.name.clone();
            self.err(head_span, format!("variant `{v}` of `{t}` is private to its module"))
                .fix(format!("add `export` to `{v}`, or build the value through a function `{t}`'s module provides"));
        }

        // Type arguments come from the expected type where there is one, and
        // from the payload otherwise.
        let targs: Vec<Ty> = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Con(c, ts)) if c == con => ts,
            _ => (0..tycon.arity()).map(|_| self.fresh(span)).collect(),
        };

        let param_types: Vec<Ty> =
            variant.fields.iter().map(|f| substitute(&f.ty, &targs, None)).collect();
        if args.len() != param_types.len() {
            let v = variant.name.clone();
            let want = param_types.len();
            let have = args.len();
            self.err(head_span, format!("`{v}` takes {want} values, but {have} were given"))
                .mismatch(want.to_string(), have.to_string())
                .fix(format!("pass exactly {want}"));
        }
        let checked = self.check_args(args, &param_types);
        let ty = Ty::Con(con, targs.clone());
        typed::Expr::new(
            typed::ExprKind::EnumLit { con, targs, variant: index, args: checked },
            ty,
            span,
        )
    }

    fn check_array(
        &mut self,
        elems: &[tree::Expr],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // Literals take their type from context, so `let c: [F32] = [1.5]`
        // makes every element an F32.
        let elem_ty = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Array(e)) => *e,
            _ => self.fresh(span),
        };
        let checked: Vec<typed::Expr> = elems
            .iter()
            .map(|e| {
                let c = self.check_expr(e, Some(&elem_ty));
                self.unify_at(e.span(), &c.ty.clone(), &elem_ty, "the element type");
                c
            })
            .collect();
        // An array literal has a statically known length and is not, by
        // itself, an allocation the programmer must account for.
        typed::Expr::new(typed::ExprKind::Array(checked), Ty::Array(Box::new(elem_ty)), span)
    }

    fn check_struct_lit(
        &mut self,
        head: &tree::Expr,
        spread: Option<&tree::Expr>,
        fields: &[tree::FieldInit],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // The head must be a type path, optionally with a turbofish, or the
        // dot form (SPEC 14.1).
        if !head.is_type_path() {
            self.err(head.span(), "the head of a struct literal must be a type").code("struct-literal-head")
                .fix("name the type, as in `Point { x: 1, y: 2 }`, or `.Variant { ... }` where the expected type is known")
                .notes
                .push("the grammar permits `f(x) { a: 1 }`; the checker does not".into());
            return self.error_expr(span);
        }

        // A record-like enum variant: `.Rect { width, height }`.
        if let Some((con, index)) = self.struct_lit_variant(head, expected) {
            return self.build_record_variant(con, index, fields, span, expected, head.span());
        }

        let explicit = match head {
            tree::Expr::TurboFish { args, .. } => Some(self.elaborate_all(args)),
            _ => None,
        };
        let Some(con) = self.struct_lit_head(head) else {
            self.err(head.span(), "this is not a struct or an enum variant")
                .fix("name a struct, or an enum variant that has fields");
            return self.error_expr(span);
        };
        let tycon = self.c.tables.tycon(con).clone();
        if !matches!(tycon.def, TyDef::Struct { .. }) {
            let n = tycon.name.clone();
            self.err(head.span(), format!("`{n}` is an enum; name a variant"))
                .fix(format!("write `{n}.Variant {{ ... }}`"));
            return self.error_expr(span);
        }

        let targs: Vec<Ty> = match explicit {
            Some(ts) if ts.len() == tycon.arity() => ts,
            _ => match expected.map(|t| self.resolve(t)) {
                Some(Ty::Con(c, ts)) if c == con => ts,
                _ => (0..tycon.arity()).map(|_| self.fresh(span)).collect(),
            },
        };
        let ty = Ty::Con(con, targs.clone());
        let decl_fields = tycon.fields().to_vec();

        let mut values: Vec<Option<typed::Expr>> = vec![None; decl_fields.len()];
        let mut seen: Vec<&str> = Vec::new();
        for init in fields {
            let Some(i) = decl_fields.iter().position(|f| f.name == init.name.name) else {
                let t = tycon.name.clone();
                let n = init.name.name.clone();
                self.err(init.name.span, format!("`{t}` has no field `{n}`"))
                    .fix("check the spelling, or name a field the struct declares");
                continue;
            };
            if seen.contains(&init.name.name.as_str()) {
                let n = init.name.name.clone();
                self.err(init.name.span, format!("field `{n}` is given twice"))
                    .fix("delete one of the two");
            }
            seen.push(&init.name.name);
            self.check_field_visible(con, &decl_fields[i], init.name.span);
            let want = substitute(&decl_fields[i].ty, &targs, None);
            let value = match &init.value {
                Some(v) => {
                    let c = self.check_expr(v, Some(&want));
                    self.unify_at(v.span(), &c.ty.clone(), &want, "the field type");
                    c
                }
                // Field shorthand: `Point { x, y }` binds `x: x`.
                None => {
                    let c = self.check_ident(&init.name.name, init.name.span, Some(&want));
                    self.unify_at(init.name.span, &c.ty.clone(), &want, "the field type");
                    c
                }
            };
            values[i] = Some(value);
        }

        match spread {
            Some(base) => {
                // Functional update never names the hidden fields, so it works
                // anywhere the type is visible.
                let b = self.check_expr(base, Some(&ty));
                self.unify_at(base.span(), &b.ty.clone(), &ty, "the base of the update");
                let updates: Vec<(usize, typed::Expr)> = values
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, v)| v.map(|e| (i, e)))
                    .collect();
                typed::Expr::new(
                    typed::ExprKind::StructUpdate { con, base: Box::new(b), updates },
                    ty,
                    span,
                )
            }
            None => {
                let missing: Vec<String> = decl_fields
                    .iter()
                    .zip(&values)
                    .filter(|(_, v)| v.is_none())
                    .map(|(f, _)| f.name.clone())
                    .collect();
                if !missing.is_empty() {
                    let t = tycon.name.clone();
                    let missing = diagnostics::names(&missing);
                    self.err(span, format!("`{t}` is missing {missing}"))
                        .fix(format!(
                            "give {missing} a value, or start from an existing one with \
                             `{t} {{ ..base, field: value }}`"
                        ));
                }
                let filled: Vec<typed::Expr> = values
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| {
                        v.unwrap_or_else(|| {
                            typed::Expr::new(
                                typed::ExprKind::Error,
                                substitute(&decl_fields[i].ty, &targs, None),
                                span,
                            )
                        })
                    })
                    .collect();
                typed::Expr::new(typed::ExprKind::StructLit { con, targs, fields: filled }, ty, span)
            }
        }
    }

    fn struct_lit_head(&mut self, head: &tree::Expr) -> Option<TyConId> {
        match head {
            tree::Expr::Ident { name, .. } => {
                match self.c.scopes[self.module.index()].names.get(name)? {
                    Sym::Ty(c) => Some(*c),
                    _ => None,
                }
            }
            tree::Expr::TurboFish { base, .. } => self.struct_lit_head(base),
            tree::Expr::Field { base, name, .. } => {
                let tree::Expr::Ident { name: head, .. } = &**base else { return None };
                let ns = self.c.scopes[self.module.index()].namespaces.get(head).copied()?;
                match self.c.lookup_export_pub(ns, &name.name)? {
                    Sym::Ty(c) => Some(c),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn struct_lit_variant(
        &mut self,
        head: &tree::Expr,
        expected: Option<&Ty>,
    ) -> Option<(TyConId, usize)> {
        match head {
            tree::Expr::DotVariant { name, .. } => {
                let Ty::Con(con, _) = self.resolve(expected?) else { return None };
                let index = self.c.tables.tycon(con).variant_index(&name.name)?;
                Some((con, index))
            }
            tree::Expr::Field { .. } => match self.static_ref(head) {
                Some(Static::Variant(con, index)) => Some((con, index)),
                _ => None,
            },
            _ => None,
        }
    }

    fn build_record_variant(
        &mut self,
        con: TyConId,
        index: usize,
        fields: &[tree::FieldInit],
        span: Span,
        expected: Option<&Ty>,
        head_span: Span,
    ) -> typed::Expr {
        let tycon = self.c.tables.tycon(con).clone();
        let variant = tycon.variants()[index].clone();
        if tycon.module != self.module && !variant.exported && tycon.module.0 != u32::MAX {
            let t = tycon.name.clone();
            let v = variant.name.clone();
            self.err(head_span, format!("variant `{v}` of `{t}` is private to its module"))
                .fix(format!("add `export` to `{v}`, or build the value through a function `{t}`'s module provides"));
        }
        let targs: Vec<Ty> = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Con(c, ts)) if c == con => ts,
            _ => (0..tycon.arity()).map(|_| self.fresh(span)).collect(),
        };
        let mut values: Vec<Option<typed::Expr>> = vec![None; variant.fields.len()];
        for init in fields {
            let Some(i) = variant.fields.iter().position(|f| f.name == init.name.name) else {
                let v = variant.name.clone();
                let n = init.name.name.clone();
                self.err(init.name.span, format!("`{v}` has no field `{n}`"))
                    .fix("check the spelling, or name a field the variant declares");
                continue;
            };
            let want = substitute(&variant.fields[i].ty, &targs, None);
            let value = match &init.value {
                Some(v) => {
                    let c = self.check_expr(v, Some(&want));
                    self.unify_at(v.span(), &c.ty.clone(), &want, "the field type");
                    c
                }
                None => {
                    let c = self.check_ident(&init.name.name, init.name.span, Some(&want));
                    self.unify_at(init.name.span, &c.ty.clone(), &want, "the field type");
                    c
                }
            };
            values[i] = Some(value);
        }
        let missing: Vec<String> = variant
            .fields
            .iter()
            .zip(&values)
            .filter(|(_, v)| v.is_none())
            .map(|(f, _)| f.name.clone())
            .collect();
        if !missing.is_empty() {
            let v = variant.name.clone();
            let missing = diagnostics::names(&missing);
            self.err(span, format!("`{v}` is missing {missing}"))
                .fix(format!("give {missing} a value"));
        }
        let filled: Vec<typed::Expr> = values
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.unwrap_or_else(|| {
                    typed::Expr::new(
                        typed::ExprKind::Error,
                        substitute(&variant.fields[i].ty, &targs, None),
                        span,
                    )
                })
            })
            .collect();
        let ty = Ty::Con(con, targs.clone());
        typed::Expr::new(
            typed::ExprKind::EnumLit { con, targs, variant: index, args: filled },
            ty,
            span,
        )
    }

    // -----------------------------------------------------------------------
    // Operators
    // -----------------------------------------------------------------------

    fn check_unary(
        &mut self,
        op: tree::UnOp,
        operand: &tree::Expr,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // `-128` is one literal, not a negation of `128`: SPEC 5.1.1 gives
        // `let y: I8 = -129;` as the error, which makes `-128` legal, and it
        // is not if the magnitude is range-checked on its own.
        if op == tree::UnOp::Neg {
            if let tree::Expr::Int { value, raw, .. } = operand {
                let ty = self.subst.fresh_num(NumClass::Int, span);
                if let Some(exp) = expected {
                    let _ = self.subst.unify(&self.c.tables, &ty, exp);
                }
                self.lit_checks.push(LitCheck {
                    value: *value,
                    negative: true,
                    raw: raw.clone(),
                    ty: ty.clone(),
                    span,
                });
                return typed::Expr::new(typed::ExprKind::Int(*value, true), ty, span);
            }
            if let tree::Expr::Float { value, span: fspan, .. } = operand {
                let ty = self.subst.fresh_num(NumClass::Float, *fspan);
                if let Some(exp) = expected {
                    let _ = self.subst.unify(&self.c.tables, &ty, exp);
                }
                return typed::Expr::new(typed::ExprKind::Float(-*value), ty, span);
            }
        }
        let e = match op {
            tree::UnOp::Not => {
                let b = self.prim(Prim::Bool);
                let e = self.check_expr(operand, Some(&b));
                self.unify_at(operand.span(), &e.ty.clone(), &b, "`!` takes a `Bool`");
                return typed::Expr::new(
                    typed::ExprKind::Prim { op: typed::PrimOp::Not, prim: Some(Prim::Bool), args: vec![e] },
                    b,
                    span,
                );
            }
            _ => self.check_expr(operand, expected),
        };
        let ty = self.resolve(&e.ty);
        let prim = self.as_prim(&ty);
        match op {
            tree::UnOp::Neg => {
                if prim.is_some_and(|p| p.is_signed() || p.is_float()) {
                    typed::Expr::new(
                        typed::ExprKind::Prim { op: typed::PrimOp::Neg, prim, args: vec![e] },
                        ty,
                        span,
                    )
                } else {
                    self.operator_trait_call("Neg", "neg", e, None, span)
                }
            }
            tree::UnOp::BitNot => {
                if prim.is_some_and(|p| p.is_integer()) {
                    typed::Expr::new(
                        typed::ExprKind::Prim { op: typed::PrimOp::BitNot, prim, args: vec![e] },
                        ty,
                        span,
                    )
                } else {
                    let shown = self.show_ty(&ty);
                    if !ty.is_error() {
                        self.err(span, format!("`~` is defined on integers, not `{shown}`"))
                            .fix("use `!` for a `Bool`");
                    }
                    self.error_expr(span)
                }
            }
            tree::UnOp::Not => unreachable!(),
        }
    }

    fn check_binary(
        &mut self,
        op: tree::BinOp,
        lhs: &tree::Expr,
        rhs: &tree::Expr,
        op_span: Span,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        use tree::BinOp as B;
        match op {
            B::And | B::Or => {
                let b = self.prim(Prim::Bool);
                let l = self.check_expr(lhs, Some(&b));
                self.unify_at(lhs.span(), &l.ty.clone(), &b, "a logical operand");
                let r = self.check_expr(rhs, Some(&b));
                self.unify_at(rhs.span(), &r.ty.clone(), &b, "a logical operand");
                let kind = if op == B::And {
                    typed::ExprKind::And { lhs: Box::new(l), rhs: Box::new(r) }
                } else {
                    typed::ExprKind::Or { lhs: Box::new(l), rhs: Box::new(r) }
                };
                return typed::Expr::new(kind, b, span);
            }
            // `??` is defined for `Option<T> ?? T` and `Result<T, E> ?? T`.
            B::Coalesce => {
                let l = self.check_expr(lhs, None);
                let lty = self.resolve(&l.ty);
                let (inner, kind) = match &lty {
                    Ty::Con(id, args)
                        if self.c.known_types.get("Option") == Some(id) && args.len() == 1 =>
                    {
                        (args[0].clone(), typed::OptionOrResult::Option)
                    }
                    Ty::Con(id, args)
                        if self.c.known_types.get("Result") == Some(id) && args.len() == 2 =>
                    {
                        (args[0].clone(), typed::OptionOrResult::Result)
                    }
                    Ty::Error => return self.error_expr(span),
                    other => {
                        let shown = self.show_ty(other);
                        self.err(op_span, format!("`??` takes an `Option` or a `Result`, found `{shown}`"))
                            .fix("`??` supplies a default for an absent or failed value; this one is neither");
                        return self.error_expr(span);
                    }
                };
                let r = self.check_expr(rhs, Some(&inner));
                self.unify_at(rhs.span(), &r.ty.clone(), &inner, "the default");
                return typed::Expr::new(
                    typed::ExprKind::Coalesce { lhs: Box::new(l), rhs: Box::new(r), kind },
                    inner,
                    span,
                );
            }
            _ => {}
        }

        let l = self.check_expr(lhs, None);
        let lty = self.resolve(&l.ty);
        // On the built-in numeric types the operators are defined on two
        // operands of the *same* type; passing the left type as the right's
        // expectation is what pins a literal on either side.
        let r = self.check_expr(rhs, Some(&lty));
        self.unify_at(rhs.span(), &r.ty.clone(), &lty, "the left operand's type");
        let ty = self.resolve(&l.ty);
        let prim = self.as_prim(&ty);
        let _ = expected;

        // `Eq` is not defined for function types, `Template`, or opaque
        // types, so comparing those is a compile error rather than a
        // representation accident.
        if matches!(prim, Some(Prim::Template))
            && matches!(op, B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge)
        {
            self.err(op_span, "`Template` does not implement `Eq`").code("missing-conformance")
                .fix("render both sides first: `str.format(ctx, a) == str.format(ctx, b)`")
                .note("a Template is a fixed-size view of literal fragments and evaluated holes, not the text it would produce");
            return self.error_expr(span);
        }

        let arith = |op: tree::BinOp| -> Option<typed::PrimOp> {
            Some(match op {
                B::Add => typed::PrimOp::Add,
                B::Sub => typed::PrimOp::Sub,
                B::Mul => typed::PrimOp::Mul,
                B::Div => typed::PrimOp::Div,
                B::Rem => typed::PrimOp::Rem,
                B::BitAnd => typed::PrimOp::BitAnd,
                B::BitOr => typed::PrimOp::BitOr,
                B::BitXor => typed::PrimOp::BitXor,
                _ => return None,
            })
        };

        if let Some(prim_op) = arith(op) {
            let numeric = prim.is_some_and(|p| p.is_integer() || p.is_float());
            let bitwise = matches!(op, B::BitAnd | B::BitOr | B::BitXor);
            if numeric && (!bitwise || prim.is_some_and(|p| p.is_integer())) {
                return typed::Expr::new(
                    typed::ExprKind::Prim { op: prim_op, prim, args: vec![l, r] },
                    ty,
                    span,
                );
            }
            if bitwise {
                if !ty.is_error() {
                    let shown = self.show_ty(&ty);
                    self.err(op_span, format!("`{}` is defined on integers, not `{shown}`", op.text()))
                        .fix("use `&&` and `||` for a `Bool`");
                }
                return self.error_expr(span);
            }
            let (tr, method) = op.trait_method().unwrap();
            return self.operator_trait_call(tr, method, l, Some(r), span);
        }

        // Comparison.
        let bool_ty = self.prim(Prim::Bool);
        match op {
            B::Eq | B::Ne => {
                if prim.is_some() {
                    let p = if op == B::Eq { typed::PrimOp::Eq } else { typed::PrimOp::Ne };
                    return typed::Expr::new(
                        typed::ExprKind::Prim { op: p, prim, args: vec![l, r] },
                        bool_ty,
                        span,
                    );
                }
                let call = self.operator_trait_call("Eq", "eq", l, Some(r), span);
                if op == B::Eq {
                    call
                } else {
                    typed::Expr::new(
                        typed::ExprKind::Prim {
                            op: typed::PrimOp::Not,
                            prim: Some(Prim::Bool),
                            args: vec![call],
                        },
                        bool_ty,
                        span,
                    )
                }
            }
            B::Lt | B::Le | B::Gt | B::Ge => {
                if prim.is_some() {
                    let p = match op {
                        B::Lt => typed::PrimOp::Lt,
                        B::Le => typed::PrimOp::Le,
                        B::Gt => typed::PrimOp::Gt,
                        _ => typed::PrimOp::Ge,
                    };
                    return typed::Expr::new(
                        typed::ExprKind::Prim { op: p, prim, args: vec![l, r] },
                        bool_ty,
                        span,
                    );
                }
                // `a < b` is `a.compare(b)` tested against an `Order`.
                let cmp = self.operator_trait_call("Ord", "compare", l, Some(r), span);
                self.order_test(cmp, op, span)
            }
            _ => unreachable!(),
        }
    }

    /// Operators are trait methods, which is what makes newtypes usable.
    fn operator_trait_call(
        &mut self,
        trait_name: &str,
        method: &str,
        lhs: typed::Expr,
        rhs: Option<typed::Expr>,
        span: Span,
    ) -> typed::Expr {
        let Some(&tid) = self.c.known_traits.get(trait_name) else {
            return self.error_expr(span);
        };
        let ty = self.resolve(&lhs.ty);
        if ty.is_error() {
            return self.error_expr(span);
        }
        if !self.satisfies(&ty, tid) {
            let shown = self.show_ty(&ty);
            let mut d = Diagnostic::error(
                span,
                format!("`{shown}` does not implement `{trait_name}`"),
            ).with_code("missing-conformance");
            if ty.head().is_some() {
                d = d
                    .with_note("conformance is nominal: a type satisfies a trait only where a declaration says so")
                    .with_fix(format!(
                        "add `derive {trait_name} for {shown};` in that type's own module, or \
                         write `impl {trait_name} for {shown} {{ ... }}` there"
                    ));
            } else {
                d = d.with_fix(format!(
                    "bound the type parameter with `{trait_name}`, so the method is available"
                ));
            }
            self.c.diags.push(d);
            return self.error_expr(span);
        }
        let index = self.c.tables.trait_(tid).method_index(method).unwrap_or(0);
        let sig = self.c.tables.trait_(tid).methods[index].clone();
        let ret = substitute(&sig.ret, &[], Some(&ty));
        let mut args = vec![lhs];
        if let Some(r) = rhs {
            args.push(r);
        }
        typed::Expr::new(
            typed::ExprKind::CallTrait {
                trait_id: tid,
                method: index,
                recv: ty,
                targs: Vec::new(),
                args,
            },
            ret,
            span,
        )
    }

    /// Lowers `a < b` and friends to a test on the `Order` a comparison
    /// produced.
    fn order_test(&mut self, cmp: typed::Expr, op: tree::BinOp, span: Span) -> typed::Expr {
        use tree::BinOp as B;
        let Some(&order) = self.c.known_types.get("Order") else {
            return self.error_expr(span);
        };
        let bool_ty = self.prim(Prim::Bool);
        let (target, when_match) = match op {
            B::Lt => ("Less", true),
            B::Ge => ("Less", false),
            B::Gt => ("Greater", true),
            _ => ("Greater", false),
        };
        let index = self.c.tables.tycon(order).variant_index(target).unwrap_or(0);
        let order_ty = Ty::Con(order, Vec::new());
        let arms = vec![
            typed::Arm {
                pattern: typed::Pattern {
                    kind: typed::PatKind::Variant { con: order, variant: index, fields: Vec::new() },
                    ty: order_ty.clone(),
                    span,
                },
                guard: None,
                body: typed::Expr::new(typed::ExprKind::Bool(when_match), bool_ty.clone(), span),
                span,
            },
            typed::Arm {
                pattern: typed::Pattern { kind: typed::PatKind::Wild, ty: order_ty, span },
                guard: None,
                body: typed::Expr::new(typed::ExprKind::Bool(!when_match), bool_ty.clone(), span),
                span,
            },
        ];
        typed::Expr::new(
            typed::ExprKind::Match { scrutinee: Box::new(cmp), arms },
            bool_ty,
            span,
        )
    }

    // -----------------------------------------------------------------------
    // `?`, templates, lambdas, contexts
    // -----------------------------------------------------------------------

    /// `?` is the only early exit in the language. There is no `return`.
    fn check_try(&mut self, base: &tree::Expr, span: Span) -> typed::Expr {
        let b = self.check_expr(base, None);
        let bty = self.resolve(&b.ty);
        let ret = self.resolve(&self.ret.clone());
        let (inner, kind) = match &bty {
            Ty::Con(id, args)
                if self.c.known_types.get("Result") == Some(id) && args.len() == 2 =>
            {
                // The enclosing function must return `Result<_, E>`. There is
                // no automatic error conversion; map the error explicitly.
                match &ret {
                    Ty::Con(rid, rargs)
                        if self.c.known_types.get("Result") == Some(rid) && rargs.len() == 2 =>
                    {
                        if let Err(_) = self.subst.unify(&self.c.tables, &args[1], &rargs[1]) {
                            let from = self.show_ty(&args[1]);
                            let to = self.show_ty(&rargs[1]);
                            self.err(
                                span,
                                format!("`?` would propagate `{from}`, but this function returns `{to}`"),
                            ).code("question-mark-mismatch")
                            .fix(format!(
                                "map the error first: `.mapErr(fn(e) => ...)?`, producing a \
                                 `{to}` — there is no automatic error conversion"
                            ));
                        }
                    }
                    _ => {
                        let shown = self.show_ty(&ret);
                        self.err(span, format!("`?` on a `Result` needs a `Result` return type, not `{shown}`")).code("question-mark-mismatch")
                            .fix("return a `Result` from this function, or handle the error here with `match` or `??`");
                    }
                }
                (args[0].clone(), typed::OptionOrResult::Result)
            }
            Ty::Con(id, args)
                if self.c.known_types.get("Option") == Some(id) && args.len() == 1 =>
            {
                if !self.is_option(&ret) {
                    let shown = self.show_ty(&ret);
                    self.err(span, format!("`?` on an `Option` needs an `Option` return type, not `{shown}`")).code("question-mark-mismatch")
                        .fix("return an `Option` from this function, or turn absence into an error with `.okOr(e)?`");
                }
                (args[0].clone(), typed::OptionOrResult::Option)
            }
            Ty::Error => return self.error_expr(span),
            other => {
                let shown = self.show_ty(other);
                self.err(span, format!("`?` takes a `Result` or an `Option`, found `{shown}`"))
                    .fix("`?` propagates a failure; this value is neither a `Result` nor an `Option`")
                    .notes
                    .push("note that `??` is a single token, so `x??y` is coalescing; write `(x?) ?? y`".into());
                return self.error_expr(span);
            }
        };
        typed::Expr::new(typed::ExprKind::Try { base: Box::new(b), kind }, inner, span)
    }

    /// Hole expressions must have type `Int` (any width), `Float` (any width),
    /// `Bool`, `Char`, or `Str`. There is no user-extensible display mechanism
    /// in v0.3; convert explicitly.
    fn check_template(&mut self, parts: &[tree::TemplatePart], span: Span) -> typed::Expr {
        let mut out = Vec::new();
        for p in parts {
            match p {
                tree::TemplatePart::Text(t) => {
                    out.push(typed::TemplatePart { text: Some(t.clone()), hole: None })
                }
                tree::TemplatePart::Hole(e) => {
                    let checked = self.check_expr(e, None);
                    // Deferred: a hole holding `1 + 1` has an unresolved
                    // literal type until defaulting has run.
                    self.hole_checks.push((checked.ty.clone(), e.span()));
                    out.push(typed::TemplatePart { text: None, hole: Some(checked) });
                }
            }
        }
        // Constructing a `Template` allocates nothing, which is why
        // `io.println(ctx, "hi ${name}")` needs only `Stdout`.
        let ty = self.prim(Prim::Template);
        typed::Expr::new(typed::ExprKind::Template { parts: out }, ty, span)
    }

    fn check_lambda(
        &mut self,
        params: &[tree::LambdaParam],
        ret: Option<&tree::TypeExpr>,
        body: &tree::Expr,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // A lambda's parameter types come from the expected type at its call
        // site, which is known before the body is visited (SPEC 13.2).
        let (want_params, want_ret) = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Fn(ps, r)) if ps.len() == params.len() => (ps, Some(*r)),
            _ => (vec![Ty::Error; params.len()], None),
        };

        self.push_scope();
        self.lambda_depth += 1;
        let outer_scopes = self.scopes.len();
        // Locals are numbered in the order they are bound, so everything this
        // lambda introduces — its parameters, its `let`s, the names its match
        // arms bind — is at or past this mark, and everything a capture could
        // name is before it.
        let outer_locals = self.locals.len();
        let mut locals = Vec::new();
        let mut ptypes = Vec::new();
        for (i, p) in params.iter().enumerate() {
            let ty = match &p.ty {
                Some(t) => {
                    let generics = self.generics.clone();
                    self.c.elaborate(self.module, &generics, t, None)
                }
                None => match want_params.get(i) {
                    Some(t) if !t.is_error() => t.clone(),
                    _ => self.fresh(p.span),
                },
            };
            let local = self.new_local(&p.name.name, ty.clone(), p.span);
            self.bind(&p.name.name, local);
            // A lambda's parameter is a binding like any other, so a lambda
            // *inside* it may not close over one that carries authority. This
            // is the `fn(c, p) => { let g = fn() => fs.exists(c, p); g() }`
            // route: `c` is the context a `*Ctx` combinator passed in, and
            // without this the capture rule would stop at one level.
            self.note_capture_risk(local, &ty);
            locals.push(local);
            ptypes.push(ty);
        }
        let declared_ret = ret.map(|t| {
            let generics = self.generics.clone();
            self.c.elaborate(self.module, &generics, t, None)
        });
        let expect_ret = declared_ret.clone().or(want_ret);
        // `?` is the only early exit, and it returns from the *enclosing
        // function* — a lambda is one, so its return type is what `?` is
        // checked against while its body is being visited.
        let lambda_placeholder = match expect_ret.clone() {
            Some(t) => t,
            None => self.fresh(span),
        };
        let outer_ret = std::mem::replace(&mut self.ret, lambda_placeholder);
        let body_hir = self.check_expr(body, expect_ret.as_ref());
        let lambda_ret = std::mem::replace(&mut self.ret, outer_ret);
        if let Some(r) = &declared_ret {
            self.unify_at(body.span(), &body_hir.ty.clone(), r, "the declared return type");
        }
        // A lambda with no annotation gets its type from its body, but if `?`
        // pinned the placeholder, that is what it is.
        let ret_ty = declared_ret.unwrap_or_else(|| {
            let resolved = self.resolve(&lambda_ret);
            if matches!(resolved, Ty::Var(_)) {
                body_hir.ty.clone()
            } else {
                resolved
            }
        });
        let _ = outer_scopes;
        self.lambda_depth -= 1;
        self.pop_scope();

        // Lambdas capture by value, and a lambda may not capture an
        // effect-carrying value: without this rule, a value of type
        // `fn(Str) => Str` could smuggle a file handle past a signature with
        // no `ctx` parameter, and the purity theorem would be false.
        let mut captures = Vec::new();
        collect_locals(&body_hir, &mut captures);
        // A name the body itself binds is not a capture. Filtering only the
        // lambda's parameters left `match (best) { .Some(b) => ... }` counting
        // `b` as one, which is harmless for a `[Int]` but not for a `[T]`.
        captures.retain(|l| l.index() < outer_locals);
        captures.sort();
        captures.dedup();
        for c in &captures {
            if self.effect_locals.contains(c) {
                let name = self.locals[c.index()].name.clone();
                self.err(span, format!("a lambda may not capture `{name}`, which carries an effect")).code("lambda-captures-effect")
                    .fix(
                        "thread the context through a `*Ctx` combinator, which passes it in as a \
                         parameter instead: `paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p))`",
                    )
                    .notes
                    .push(
                        "a lambda that could close over authority would make the capture rule \
                         meaningless"
                            .into(),
                    );
                break;
            }
            if self.poly_locals.contains(c) {
                let name = self.locals[c.index()].name.clone();
                let shown = self.show_ty(&self.locals[c.index()].ty.clone());
                self.err(
                    span,
                    format!(
                        "a lambda may not capture `{name}`, whose type `{shown}` could be a \
                         context"
                    ),
                )
                .code("lambda-captures-generic")
                .fix(
                    "take it as a parameter of the lambda instead — the `fn(C, A) => B` shape a \
                     `*Ctx` combinator passes — or return the value rather than a closure over it",
                )
                .notes
                .push(
                    "a generic body is checked once, for every instantiation at once (SPEC 13.5), \
                     so a type parameter here stands for a context type too — and `fn wrap<T>(x: \
                     T, f: fn(T) => ()): fn() => ()` would otherwise launder one into a closure \
                     whose type mentions no effect at all"
                        .into(),
                );
                break;
            }
        }

        let ty = Ty::Fn(ptypes, Box::new(ret_ty));
        typed::Expr::new(
            typed::ExprKind::Lambda { params: locals, body: Box::new(body_hir), captures },
            ty,
            span,
        )
    }

    pub(crate) fn check_context_body(
        &mut self,
        body: &tree::ContextBody,
        span: Span,
    ) -> typed::Expr {
        if !self.may_build_context() {
            self.err(span, "a context may not be constructed here").code("context-not-allowed")
                .fix(
                    "build it in `main` and pass it down as a `ctx` parameter, or make this a \
                     test source, where a context may be built per test",
                )
                .notes
                .push(
                    "only in `main`'s body, in a test source, or in a test-only module — and \
                     never inside a lambda, since a closure that could mint authority would \
                     make the capture rule meaningless"
                        .into(),
                );
        }

        let mut bindings: Vec<(TraitId, typed::Expr)> = Vec::new();
        let mut spans: Vec<(TraitId, Span)> = Vec::new();
        // The effects bound explicitly here, as opposed to inherited from a
        // spread.
        let mut explicit: Vec<TraitId> = Vec::new();

        // Either form may begin with a spread, which takes every binding from
        // another context and lets the ones that follow replace them.
        if let Some(base) = &body.spread {
            let b = self.check_expr(base, None);
            match self.resolve(&b.ty) {
                Ty::Ctx(id) => {
                    // The inherited binding keeps the *implementing type* the
                    // base recorded. Without it monomorphization has nothing
                    // to dispatch on, and every effect a spread supplies is
                    // unusable.
                    let base_bindings = self.c.tables.ctx_type(id).bindings.clone();
                    for (t, impl_ty) in base_bindings {
                        let e = typed::Expr::new(
                            typed::ExprKind::CtxGet { base: Box::new(b.clone()), trait_id: t },
                            impl_ty,
                            base.span(),
                        );
                        bindings.push((t, e));
                        spans.push((t, base.span()));
                    }
                }
                Ty::Error => {}
                other => {
                    let shown = self.show_ty(&other);
                    self.err(base.span(), format!("a spread takes another context, found `{shown}`"))
                        .fix("spread a value built by a `context` expression or a context declaration");
                }
            }
        }

        for binding in &body.bindings {
            let tree::TypeExpr::Named { path, .. } = &binding.effect else {
                self.err(binding.effect.span(), "a context binding names an effect")
                    .fix("write `Effect: implementation`, as in `Alloc: host.alloc`");
                continue;
            };
            let Some(Sym::Trait(tid)) = self.c.resolve_path(self.module, path) else {
                let shown = binding.effect.head_name().unwrap_or("?").to_string();
                self.err(binding.effect.span(), format!("`{shown}` is not a declared effect")).code("not-an-effect")
                    .fix(format!(
                        "name an effect the platform declares, as in `Alloc` or `Stdout`; \
                         `{shown}` is not one"
                    ));
                continue;
            };
            if !self.c.tables.trait_(tid).is_effect {
                let shown = self.c.tables.trait_(tid).name.clone();
                self.err(binding.effect.span(), format!("`{shown}` is a trait, not an effect"))
                    .fix("bind an effect here; pass a plain trait's implementations as ordinary arguments")
                    .notes
                    .push("a context binds effects; a trait is not one".into());
                continue;
            }
            let value = self.check_expr(&binding.value, None);
            let vty = self.resolve(&value.ty);
            // Every binding's right side is a value whose type implements that
            // effect — ordinary nominal conformance.
            if !self.satisfies(&vty, tid) && !vty.is_error() {
                let shown = self.show_ty(&vty);
                let eff = self.c.tables.trait_(tid).name.clone();
                self.err(binding.value.span(), format!("`{shown}` does not implement `{eff}`")).code("missing-conformance")
                    .fix(format!(
                        "bind a value whose type has `impl {eff} for ...`; an effect is an \
                         ordinary interface, so a test double is a struct with those methods"
                    ));
            }
            // An explicit binding replaces a spread's rather than duplicating
            // it; two explicit bindings of one effect is an error.
            if explicit.contains(&tid) {
                let eff = self.c.tables.trait_(tid).name.clone();
                self.err(binding.span, format!("`{eff}` is bound twice")).code("duplicate-bound")
                    .fix("delete one of the two bindings")
                    .note("a spread's binding is replaced by an explicit one, but two explicit bindings of one effect are a mistake");
            }
            explicit.push(tid);
            if let Some(i) = bindings.iter().position(|(t, _)| *t == tid) {
                bindings[i] = (tid, value);
            } else {
                bindings.push((tid, value));
                spans.push((tid, binding.span));
            }
        }

        // The constructed value satisfies exactly the effects bound and
        // nothing else.
        let ctx_ty = CtxType {
            bindings: bindings.iter().map(|(t, e)| (*t, self.subst.resolve(&e.ty))).collect(),
        };
        let id = self.c.tables.add_ctx_type(ctx_ty);
        typed::Expr::new(typed::ExprKind::CtxLit { bindings }, Ty::Ctx(id), span)
    }

    fn check_match(
        &mut self,
        scrutinee: &tree::Expr,
        arms: &[tree::MatchArm],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let s = self.check_expr(scrutinee, None);
        let sty = self.resolve(&s.ty);
        let result = match expected {
            Some(t) => t.clone(),
            None => self.fresh(span),
        };
        let bool_ty = self.prim(Prim::Bool);
        let mut checked = Vec::new();
        for arm in arms {
            self.push_scope();
            self.pattern_names.clear();
            let pat = self.check_pattern(&arm.pattern, &sty);
            let guard = arm.guard.as_ref().map(|g| {
                let e = self.check_expr(g, Some(&bool_ty));
                self.unify_at(g.span(), &e.ty.clone(), &bool_ty, "a guard");
                e
            });
            let body = self.check_expr(&arm.body, Some(&result));
            self.unify_at(arm.body.span(), &body.ty.clone(), &result, "the other arms");
            self.pop_scope();
            checked.push(typed::Arm { pattern: pat, guard, body, span: arm.span });
        }
        // The match must be exhaustive, and no arm may be unreachable.
        crate::compiler::semantics::exhaustiveness::check(self, &sty, &checked, span);
        typed::Expr::new(
            typed::ExprKind::Match { scrutinee: Box::new(s), arms: checked },
            result,
            span,
        )
    }
}

fn strip_turbofish(e: &tree::Expr) -> &tree::Expr {
    match e {
        tree::Expr::TurboFish { base, .. } => strip_turbofish(base),
        other => other,
    }
}

fn collect_locals(e: &typed::Expr, out: &mut Vec<LocalId>) {
    typed::walk(e, &mut |x| {
        if let typed::ExprKind::Local(l) = &x.kind {
            out.push(*l);
        }
    });
}

