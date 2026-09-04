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
use crate::compiler::standard_library;
use crate::diagnostics::{self, Diagnostic, Invariant as _, Span};
use crate::parsing::flat::{
    self, ArmData, BlockId, CtxBodyId, ExprId, ExprView as V, InitData, LambdaParamData, PartData,
    PartView, TypeId,
};
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

    pub(crate) fn check_block(&mut self, b: BlockId, expected: Option<&Ty>) -> typed::Expr {
        let t = self.tree();
        let block = t.block(b);
        self.push_scope();
        let mut stmts = Vec::new();
        for s in t.stmts_at(block.stmts_start, block.stmts_len) {
            let span = t.span_of(s.span);
            match s.kind {
                flat::StmtKind::Let => {
                    let stmt = self.check_let(
                        flat::PatId(s.pattern),
                        t.opt_type(s.ty),
                        ExprId(s.value),
                        s.is_ctx,
                        span,
                    );
                    stmts.push(stmt);
                }
                flat::StmtKind::Expr => {
                    // An expression statement is legal only in a test source,
                    // and only when its type is `()`.
                    //
                    // The diagnostic is pushed here rather than after the
                    // expression is checked, so that it keeps its place among
                    // whatever the expression itself reports; its *fix* cannot
                    // be written yet, because the answer depends on a type
                    // nothing has inferred. So the slot is remembered and
                    // filled in below. A page's `fix` that reads
                    // "bind it: `let _ = ...;`" is the wrong edit for a
                    // `Result`, and a diagnostic whose fix provokes the next
                    // diagnostic is worse than one that offers none.
                    let slot = if self.role == Role::TestSource {
                        None
                    } else {
                        let at = self.c.diags.items.len();
                        self.templated("expression-statement", span);
                        Some(at)
                    };
                    let e = self.check_expr(ExprId(s.value), None);
                    let ty = self.resolve(&e.ty);
                    let dropped_result = self.is_known_result(&ty).then(result_discard_fix).flatten();
                    if !matches!(ty, Ty::Unit | Ty::Error) {
                        let shown = self.show_ty(&ty);
                        let d = self.templated("statement-not-unit", span).bind("type", shown);
                        if let Some(fix) = dropped_result {
                            d.fix(fix);
                        }
                    }
                    // Outside a test source a `Result` statement is *both*
                    // errors at once, and `.ignore()` alone answers only the
                    // second: it settles the type and leaves the statement
                    // standing. So the edit this one offers is the whole edit —
                    // the drop and the binding together — which is what a
                    // program prints a line with now that `println` answers a
                    // `Result`.
                    if let (true, Some(at)) = (dropped_result.is_some(), slot) {
                        if let Some(d) = self.c.diags.items.get_mut(at) {
                            d.fix(
                                "consume it and bind what is left: `let _ = ....ignore();` — or \
                                 `?` to propagate, `match` to handle both cases",
                            );
                        }
                    }
                    stmts.push(typed::Stmt::Expr(e));
                }
            }
        }
        let (tail, ty) = match t.opt(block.tail) {
            Some(e) => {
                let checked = self.check_expr(e, expected);
                let ty = checked.ty.clone();
                (Some(Box::new(checked)), ty)
            }
            // A block whose last item is a `let` has type `()`.
            None => (None, Ty::Unit),
        };
        self.pop_scope();
        typed::Expr::new(typed::ExprKind::Block { stmts, tail }, ty, t.span_of(block.span))
    }

    fn check_let(
        &mut self,
        pattern: flat::PatId,
        ann: Option<TypeId>,
        value: ExprId,
        is_ctx: bool,
        span: Span,
    ) -> typed::Stmt {
        // Binding a value to `ctx` builds nothing, so it is not an error here —
        // `context-not-allowed` guards the construction itself. The name is
        // still a convention, and `ctx-rebinding` reports it from `buri lint`.
        if is_ctx && !self.may_build_context() {
            self.c.ctx_rebindings.push(span);
        }
        let expected = ann.map(|id| self.c.elaborate(self.module, &self.generics, id));
        let value_span = self.tree().span(value);
        let value_hir = self.check_expr(value, expected.as_ref());
        if let Some(exp) = &expected {
            self.unify_at(value_span, &value_hir.ty.clone(), exp, "the annotation");
        }
        let ty = expected.unwrap_or_else(|| value_hir.ty.clone());

        self.pattern_names.clear();
        let pat = self.check_pattern(pattern, &ty);

        // A value of type `Result<T, E>` may not be discarded by a `_`
        // pattern (SPEC 5.7.1). The question is asked of the whole pattern
        // rather than of its head, because a wildcard one level down drops a
        // `Result` exactly as thoroughly as one at the top:
        //
        //   let (count, _) = (1, mayFail());
        //   let Job { name, outcome: _ } = job;
        //
        // Reading the *checked* pattern is what makes that affordable — every
        // node carries the type it matches, so the walk is a walk rather than
        // a second, parallel elaboration of the syntax against `ty`.
        //
        // It runs after `check_pattern` for the same reason: before it, only
        // the head's type is known. A wildcard at the top still reports at the
        // statement's span, because the edit a reader has to make is the
        // statement; a nested one reports at itself, because the statement is
        // otherwise fine and the `_` is the whole of what is wrong.
        self.report_discarded_results(&pat, span);
        // The pattern in a `let` must be irrefutable. Use `match` for anything
        // else.
        if !pat.is_irrefutable(&self.c.tables) {
            let pspan = self.tree().pspan(pattern);
            self.templated("refutable-pattern", pspan);
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

    /// Every `_` in a `let`'s pattern that throws a `Result` away, reported.
    ///
    /// `at` is where a wildcard standing as the whole pattern reports — the
    /// statement, because that is the line to edit. Everything nested reports
    /// at its own span.
    fn report_discarded_results(&mut self, pat: &typed::Pattern, at: Span) {
        let mut wildcards = Vec::new();
        wildcard_types(pat, at, &mut wildcards);
        for (span, ty) in wildcards {
            if self.is_known_result(&ty) {
                self.templated("result-discarded", span);
            }
        }
    }

    /// Whether `ty` is the `Result` the prelude registered under that name.
    ///
    /// Nominal, not structural: `Checker::result_con` holds the id
    /// `register_known_names` recorded, so a user type spelled `Result` in
    /// another module is not this one. The same-named [`Tables::is_option`]
    /// answers the *shape* question instead — a deliberately different
    /// question, which is why these four say `known`.
    fn is_known_result(&self, ty: &Ty) -> bool {
        matches!(self.resolve_ref(ty), Ty::Con(id, _) if self.c.result_con.as_ref() == Some(id))
    }

    /// Whether `ty` is the `Option` the prelude registered under that name —
    /// nominal, for the reason [`Infer::is_known_result`] gives.
    fn is_known_option(&self, ty: &Ty) -> bool {
        matches!(self.resolve_ref(ty), Ty::Con(id, _) if self.c.option_con.as_ref() == Some(id))
    }

    /// `T`, when `ty` is the registered `Option<T>`. Asking the question and
    /// reading the payload are one step, so the arity cannot be checked in one
    /// place and relied on in another.
    fn known_option_payload<'t>(&self, ty: &'t Ty) -> Option<&'t Ty> {
        match ty {
            Ty::Con(id, args) if self.c.option_con.as_ref() == Some(id) => {
                match args.as_slice() {
                    [inner] => Some(inner),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// `(T, E)`, when `ty` is the registered `Result<T, E>`.
    fn known_result_payload<'t>(&self, ty: &'t Ty) -> Option<(&'t Ty, &'t Ty)> {
        match ty {
            Ty::Con(id, args) if self.c.result_con.as_ref() == Some(id) => {
                match args.as_slice() {
                    [ok, err] => Some((ok, err)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    pub(crate) fn check_expr(&mut self, e: ExprId, expected: Option<&Ty>) -> typed::Expr {
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
                    parts: vec![typed::TemplatePart::Hole(e)],
                },
                ty,
                span,
            );
        }
        e
    }

    fn check_expr_inner(&mut self, e: ExprId, expected: Option<&Ty>) -> typed::Expr {
        match self.tree().expr(e) {
            V::Int { value, raw, span } => {
                // A numeric literal gets a fresh type variable constrained to
                // the integer types; ordinary unification then decides.
                let ty = self.subst.fresh_num(NumClass::Int, span);
                if let Some(exp) = expected {
                    let _ = self.subst.unify(&self.c.tables, &ty, exp);
                }
                self.lit_checks.push(LitCheck {
                    value,
                    negative: false,
                    raw: raw.to_string(),
                    ty: ty.clone(),
                    span,
                });
                typed::Expr::new(typed::ExprKind::Int(value, false), ty, span)
            }
            V::Float { value, span, .. } => {
                let ty = self.subst.fresh_num(NumClass::Float, span);
                if let Some(exp) = expected {
                    let _ = self.subst.unify(&self.c.tables, &ty, exp);
                }
                typed::Expr::new(typed::ExprKind::Float(value), ty, span)
            }
            V::Str { value, span } => {
                let ty = self.prim(Prim::Str);
                typed::Expr::new(typed::ExprKind::Str(value.to_string()), ty, span)
            }
            V::Char { value, span } => {
                let ty = self.prim(Prim::Char);
                typed::Expr::new(typed::ExprKind::Char(value), ty, span)
            }
            V::Bool { value, span } => {
                let ty = self.prim(Prim::Bool);
                typed::Expr::new(typed::ExprKind::Bool(value), ty, span)
            }
            V::Unit { span } => typed::Expr::new(typed::ExprKind::Unit, Ty::Unit, span),
            // The parser already reported why. `Ty::Error` unifies with
            // everything, so nothing downstream reports a second time.
            V::Error { span } => self.error_expr(span),
            V::Template { parts, span } => self.check_template(parts, span),
            V::Ident { name, span } => self.check_ident(name, span, expected),
            V::SelfValue { span } => match self.lookup_local("self") {
                Some(l) => typed::Expr::new(typed::ExprKind::Local(l), self.local_ty(l), span),
                None => {
                    self.templated("self-outside-a-method", span);
                    self.error_expr(span)
                }
            },
            V::Ctx { span } => match self.lookup_local("ctx") {
                Some(l) => typed::Expr::new(typed::ExprKind::Local(l), self.local_ty(l), span),
                None => {
                    // A name nothing in scope declares, reported as one. `ctx`
                    // is a keyword, so a parameter is the only thing that ever
                    // puts it in scope, and that is what the fix says.
                    self.templated("unresolved-name", span)
                        .bind("name", "ctx")
                        .fix("declare `ctx` as a parameter of this function");
                    self.error_expr(span)
                }
            },
            V::DotVariant { name, span, .. } => {
                self.check_dot_variant(name, &[], span, expected)
            }
            V::Array { elems, span } => self.check_array(elems, span, expected),
            V::Tuple { elems, span } => {
                let want: Vec<Option<Ty>> = match expected.map(|t| self.resolve(t)) {
                    Some(Ty::Tuple(ts)) if ts.len() == elems.len() => {
                        ts.into_iter().map(Some).collect()
                    }
                    _ => vec![None; elems.len()],
                };
                let checked: Vec<typed::Expr> = elems
                    .iter()
                    .zip(want)
                    .map(|(x, w)| self.check_expr(*x, w.as_ref()))
                    .collect();
                let ty = Ty::Tuple(checked.iter().map(|c| c.ty.clone()).collect());
                typed::Expr::new(typed::ExprKind::Tuple(checked), ty, span)
            }
            V::Block { block, .. } => self.check_block(block, expected),
            V::If { cond, then, else_, span } => {
                let bool_ty = self.prim(Prim::Bool);
                let c = self.check_expr(cond, Some(&bool_ty));
                // The condition must have type `Bool`. There is no truthiness.
                let cond_span = self.tree().span(cond);
                self.unify_at(cond_span, &c.ty.clone(), &bool_ty, "an `if` condition");
                let t = self.check_block(then, expected);
                let f = self.check_expr(else_, expected.or(Some(&t.ty.clone())));
                // Both branches must have the same type.
                let else_span = self.tree().span(else_);
                self.unify_at(else_span, &f.ty.clone(), &t.ty.clone(), "the other branch");
                let ty = t.ty.clone();
                typed::Expr::new(
                    typed::ExprKind::If { cond: Box::new(c), then: Box::new(t), else_: Box::new(f) },
                    ty,
                    span,
                )
            }
            V::Match { scrutinee, arms, span } => {
                self.check_match(scrutinee, arms, span, expected)
            }
            V::ContextExpr { body, span } => self.check_context_body(body, span),
            V::Lambda { params, ret, body, span } => {
                self.check_lambda(params, ret, body, span, expected)
            }
            V::Unary { op, operand, span } => self.check_unary(op, operand, span, expected),
            V::Binary { op, lhs, rhs, op_span, span } => {
                self.check_binary(op, lhs, rhs, op_span, span, expected)
            }
            V::Field { base, name, name_span, span } => {
                self.check_field(base, name, name_span, span, expected)
            }
            V::TupleIndex { base, index, index_span, span } => {
                let b = self.check_expr(base, None);
                let bty = self.resolve(&b.ty);
                match &bty {
                    Ty::Tuple(elems) => match elems.get(index as usize) {
                        Some(t) => typed::Expr::new(
                            typed::ExprKind::TupleIndex { base: Box::new(b), index: index as usize },
                            t.clone(),
                            span,
                        ),
                        None => {
                            let n = elems.len();
                            self.templated("no-such-tuple-element", index_span)
                                .bind("arity", n.to_string())
                                .bind("index", index.to_string())
                                .bind("last", n.saturating_sub(1).to_string());
                            self.error_expr(span)
                        }
                    },
                    // A tuple struct's fields are `.0`, `.1`, ...
                    Ty::Con(con, args) => {
                        let fields = self.c.tables.tycon(*con).fields().to_vec();
                        match fields.get(index as usize) {
                            Some(f) => {
                                self.check_field_visible(*con, index as usize, index_span);
                                let ty = substitute(&f.ty, args, None);
                                typed::Expr::new(
                                    typed::ExprKind::Field { base: Box::new(b), index: index as usize },
                                    ty,
                                    span,
                                )
                            }
                            None => {
                                let n = self.c.tables.tycon(*con).name.clone();
                                self.templated("no-such-positional-field", index_span)
                                    .bind("type", n)
                                    .bind("index", index.to_string())
                                    .bind("count", fields.len().to_string());
                                self.error_expr(span)
                            }
                        }
                    }
                    _ => {
                        if !bty.is_error() {
                            let shown = self.show_ty(&bty);
                            self.templated("not-a-tuple", span).bind("type", shown);
                        }
                        self.error_expr(span)
                    }
                }
            }
            V::Call { callee, args, span } => self.check_call(callee, args, span, expected),
            V::Index { base, index, span } => {
                let b = self.check_expr(base, None);
                let int_ty = self.prim(Prim::I64);
                let i = self.check_expr(index, Some(&int_ty));
                let index_span = self.tree().span(index);
                self.unify_at(index_span, &i.ty.clone(), &int_ty, "an index");
                let bty = self.resolve(&b.ty);
                let elem = match &bty {
                    Ty::Array(e) => (**e).clone(),
                    Ty::Error => Ty::Error,
                    other => {
                        let shown = self.show_ty(other);
                        self.templated("not-indexable", span).bind("type", shown);
                        Ty::Error
                    }
                };
                // Indexing yields `Option<T>`. There is no way to index out of
                // bounds and no way to panic by indexing.
                let ty = self.option_of(elem.clone());
                typed::Expr::new(
                    typed::ExprKind::Index { base: Box::new(b), index: Box::new(i), elem },
                    ty,
                    span,
                )
            }
            V::Try { base, span } => self.check_try(base, span),
            V::Generic { base, args, span } => {
                // Type arguments only ever qualify a callee; on their own
                // they are a function reference.
                let targs = self.elaborate_all(args);
                match self.static_ref(base) {
                    Some(Static::Fn(f)) => self.fn_ref(f, Some(targs), span),
                    _ => {
                        let function = self.written_name(base);
                        self.templated("type-args-on-a-value", span).bind("function", function);
                        self.error_expr(span)
                    }
                }
            }
            V::StructLit { head, spread, fields, span } => {
                self.check_struct_lit(head, spread, fields, span, expected)
            }
        }
    }

    fn elaborate_all(&mut self, args: &'b [TypeId]) -> Vec<Ty> {
        args.iter().map(|a| self.c.elaborate(self.module, &self.generics, *a)).collect()
    }

    fn option_of(&self, t: Ty) -> Ty {
        match self.c.option_con {
            Some(id) => Ty::Con(id, vec![t]),
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
        let sym = self.c.scope(self.module).names.get(name).cloned();
        match sym {
            Some(Sym::Fn(f)) => self.fn_ref(f, None, span),
            Some(Sym::Const(cid)) => {
                let ty = self.c.tables.const_(cid).ty.clone();
                typed::Expr::new(typed::ExprKind::Const(cid), ty, span)
            }
            Some(Sym::Context(cid)) => {
                self.templated("context-not-a-value", span).bind("name", name);
                let _ = cid;
                self.error_expr(span)
            }
            Some(Sym::Method(owner)) => {
                self.templated("method-not-a-value", span).bind("name", name).fix(format!(
                    "call it on a receiver: `x.{name}(...)`, where `x` is a `{owner}`; to \
                     pass it on, wrap it in a lambda"
                ));
                self.error_expr(span)
            }
            Some(Sym::Overloaded(fs)) => {
                let names: Vec<String> = fs
                    .iter()
                    .map(|f| {
                        let i = self.c.tables.fn_info(*f);
                        match &i.self_ty {
                            Some(c) => format!("{}.{}", self.c.tables.tycon(*c).name, i.name),
                            None => i.name.clone(),
                        }
                    })
                    .collect();
                self.templated("ambiguous-free-function", span).bind("name", name).note(format!(
                    "it is a method on several types: {}",
                    diagnostics::names(&names)
                ));
                self.error_expr(span)
            }
            Some(Sym::Ty(_))
            | Some(Sym::Trait(_))
            | Some(Sym::Namespace(_))
            | Some(Sym::Alias(..)) => {
                self.templated("type-not-a-value", span).bind("name", name);
                self.error_expr(span)
            }
            None => {
                let _ = expected;
                let mut note = None;
                if self.c.scope(self.module).namespaces.contains_key(name) {
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
                let d = self.templated("unresolved-name", span).bind("name", name);
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
        candidates.extend(self.c.scope(self.module).names.keys().cloned());
        let refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
        nearest(name, &refs).map(|s| s.to_string())
    }

    fn fn_ref(&mut self, f: FnId, explicit: Option<Vec<Ty>>, span: Span) -> typed::Expr {
        // Only the generics are copied, and a function usually has none.
        // `instantiate` is the one step that needs `&mut self`; everything
        // after it reads the declaration in place.
        let generics = self.c.tables.fn_info(f).generics.clone();
        let targs = self.instantiate(&generics, explicit, span);
        let info = self.c.tables.fn_info(f);
        let params: Vec<Ty> =
            info.params.iter().map(|p| substitute(&p.ty, &targs, None)).collect();
        let ret = substitute(&info.ret, &targs, None);
        let ty = Ty::Fn(params, Box::new(ret));
        typed::Expr::new(typed::ExprKind::FnRef(typed::Callee::Decl { id: f, targs }), ty, span)
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
                self.templated("type-argument-mismatch", span)
                    .bind("expected", want.to_string())
                    .bind("found", got.to_string())
                    .mismatch(want.to_string(), got.to_string());
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
    fn static_ref(&mut self, e: ExprId) -> Option<Static> {
        match self.tree().expr(e) {
            V::Ident { name, .. } => {
                if self.lookup_local(name).is_some() {
                    return None;
                }
                match self.c.scope(self.module).names.get(name).cloned()? {
                    Sym::Fn(f) => Some(Static::Fn(f)),
                    Sym::Context(c) => Some(Static::Context(c)),
                    Sym::Const(c) => Some(Static::Const(c)),
                    Sym::Ty(con) => self.tuple_struct(con),
                    _ => None,
                }
            }
            V::Field { base, name, .. } => self.static_ref_field(base, name),
            V::Generic { base, .. } => self.static_ref(base),
            _ => None,
        }
    }

    /// The name the source wrote for an expression, for a diagnostic that shows
    /// the reader their own code back. `f` where there is no name to quote —
    /// the type arguments were attached to a literal or a call result.
    fn written_name(&self, e: ExprId) -> String {
        match self.tree().expr(e) {
            V::Ident { name, .. } | V::Field { name, .. } => name.to_string(),
            _ => "f".to_string(),
        }
    }

    /// The `Field` case on its own, so that `check_field` can ask the question
    /// without building the field node it would otherwise have to hand
    /// `static_ref` — its first act is to require the base to be an identifier
    /// and give up otherwise.
    fn static_ref_field(&mut self, base: ExprId, name: &str) -> Option<Static> {
        let V::Ident { name: head, .. } = self.tree().expr(base) else { return None };
        if self.lookup_local(head).is_some() {
            return None;
        }
        // `list.map` — a member of a namespace import.
        if let Some(ns) = self.c.scope(self.module).namespaces.get(head).copied() {
            return match self.c.lookup_export(ns, name)? {
                Sym::Fn(f) => Some(Static::Fn(f)),
                Sym::Context(c) => Some(Static::Context(c)),
                Sym::Const(c) => Some(Static::Const(c)),
                Sym::Ty(con) => self.tuple_struct(con),
                _ => None,
            };
        }
        // `Shape.Circle` — a qualified variant.
        if let Some(Sym::Ty(con)) = self.c.scope(self.module).names.get(head).cloned() {
            let index = self.c.tables.variant_index(con, name)?;
            return Some(Static::Variant(con, index));
        }
        None
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
        args: &[ExprId],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let (arity, fields) = {
            let tycon = self.c.tables.tycon(con);
            (tycon.arity(), tycon.fields().to_vec())
        };
        let targs: Vec<Ty> = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Con(c, ts)) if c == con => ts,
            _ => (0..arity).map(|_| self.fresh(span)).collect(),
        };
        if args.len() != fields.len() {
            let n = self.c.tables.tycon(con).name.clone();
            let have = args.len();
            let want = fields.len();
            self.templated("wrong-value-count", span)
                .bind("name", n)
                .bind("expected", want.to_string())
                .bind("given", have.to_string())
                .mismatch(want.to_string(), have.to_string());
        }
        // A struct with any private field cannot be constructed from scratch
        // outside its module.
        for i in 0..fields.len() {
            self.check_field_visible(con, i, span);
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
        callee: ExprId,
        args: &[ExprId],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // `.Some(x)` — the inferred-type dot form as a constructor.
        if let V::DotVariant { name, span: dspan, .. } = self.tree().expr(callee) {
            return self.check_dot_variant_call(name, args, dspan, span, expected);
        }

        let explicit = match self.tree().expr(callee) {
            V::Generic { args: targs, .. } => Some(self.elaborate_all(targs)),
            _ => None,
        };

        // A method call has no production of its own: it is a `Field` whose
        // base is a value.
        // Once: `static_ref` walks the callee and hashes a name or two, and a
        // namespaced call asked it the same question twice.
        let statically = self.static_ref(callee);
        if statically.is_none() {
            let bare = self.tree().strip_type_args(callee);
            if matches!(self.tree().expr(bare), V::Field { .. }) {
                return self.check_method_call(bare, args, explicit, span, expected);
            }
        }

        match statically {
            Some(Static::Fn(f)) => self.call_fn(f, explicit, args, span, None, expected),
            Some(Static::Variant(con, index)) => {
                let head_span = self.tree().span(callee);
                self.construct_variant(con, index, args, span, expected, head_span)
            }
            Some(Static::TupleStruct(con)) => {
                self.construct_tuple_struct(con, args, span, expected)
            }
            Some(Static::Context(cid)) => {
                if !args.is_empty() {
                    self.templated("context-call-with-arguments", span);
                }
                if !self.may_build_context() {
                    self.templated("context-not-allowed", span);
                }
                // Checked here if checking has not reached it yet. A
                // declaration built from another reads the base's *recorded*
                // type, and the order the declarations are checked in is the
                // order their ids were minted — the order the modules were
                // discovered in, which says nothing about which spreads which.
                // So the use asks for it, and `ctx_decls_reached` makes that
                // happen once.
                if self.c.tables.ctx_decl(cid).checked.is_none() {
                    super::inference::check_context_decl(self.c, cid);
                }
                let ty = match self.c.tables.ctx_decl(cid).checked {
                    Some(c) => Ty::Ctx(c.ty),
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
                            self.templated("argument-count-mismatch", span)
                                .bind("expected", want.to_string())
                                .bind("found", got.to_string())
                                .mismatch(want.to_string(), got.to_string());
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
                        let callee_span = self.tree().span(callee);
                        self.templated("not-callable", callee_span).bind("type", shown);
                        self.error_expr(span)
                    }
                }
            }
        }
    }

    /// Arguments are evaluated left to right before the call, and each is
    /// checked against its parameter's type so that a literal is pinned by it.
    fn check_args(&mut self, args: &[ExprId], params: &[Ty]) -> Vec<typed::Expr> {
        args.iter()
            .enumerate()
            .map(|(i, a)| {
                let want = params.get(i).cloned();
                let checked = self.check_expr(*a, want.as_ref());
                if let Some(w) = &want {
                    let aspan = self.tree().span(*a);
                    self.unify_at(aspan, &checked.ty.clone(), w, "the parameter type");
                }
                checked
            })
            .collect()
    }

    fn call_fn(
        &mut self,
        f: FnId,
        explicit: Option<Vec<Ty>>,
        args: &[ExprId],
        span: Span,
        receiver: Option<typed::Expr>,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // The declaration is read, not owned: copying `FnInfo` here copied the
        // name, every parameter's name, and every parameter's type tree, once
        // per call site in the program. Only the generics are taken, because
        // `instantiate` wants `&mut self`, and a function usually has none.
        let generics = self.c.tables.fn_info(f).generics.clone();
        let targs = self.instantiate(&generics, explicit, span);
        let self_ty = receiver.as_ref().map(|r| self.resolve(&r.ty));
        let (params, ret) = {
            let info = self.c.tables.fn_info(f);
            let params: Vec<Ty> = info
                .params
                .iter()
                .map(|p| substitute(&p.ty, &targs, self_ty.as_ref()))
                .collect();
            (params, substitute(&info.ret, &targs, self_ty.as_ref()))
        };

        // Type information flows outside-in: unifying the return type with
        // what the call site wants *before* visiting the arguments is what
        // lets `xs.fold(f, .None)` know which enum `.None` names, and what
        // pins a numeric literal argument to the width the callee declared.
        if let Some(exp) = expected {
            let _ = self.subst.unify(&self.c.tables, &ret, exp);
        }

        // A method's receiver takes the first parameter. A trait may declare
        // a method with no `self` at all, and an `impl` of it is still reached
        // through the method table, so the subtraction is saturating rather
        // than a claim about the declaration.
        let expected_args =
            if receiver.is_some() { params.len().saturating_sub(1) } else { params.len() };
        if args.len() != expected_args {
            let (name, takes_ctx) = {
                let info = self.c.tables.fn_info(f);
                (info.name.clone(), info.params.iter().any(|p| p.role == ParamRole::Ctx))
            };
            let have = args.len();
            let mut d = Diagnostic::templated("wrong-argument-count", span)
                .with_bind("function", name.clone())
                .with_bind("expected", expected_args.to_string())
                .with_bind("given", have.to_string())
                .with_mismatch(expected_args.to_string(), have.to_string());
            // The most common cause is forgetting the context, which is always
            // the parameter right after the receiver.
            if takes_ctx && have.saturating_add(1) == expected_args {
                d = d.with_fix("pass the context: the convention is receiver first, context second, everything else after");
            }
            self.c.diags.push(d);
        }

        let mut hir_args = Vec::new();
        // The receiver takes the first slot and the arguments are checked
        // against what is left, which is a subslice rather than a copy of the
        // list with its head removed.
        let mut param_types: &[Ty] = &params;
        if let Some(r) = receiver {
            if let Some((want, rest)) = params.split_first() {
                self.unify_at(r.span, &r.ty.clone(), want, "the receiver");
                param_types = rest;
            }
            hir_args.push(r);
        }
        hir_args.extend(self.check_args(args, param_types));
        typed::Expr::new(
            typed::ExprKind::CallFn {
                func: typed::Callee::Decl { id: f, targs },
                args: hir_args,
            },
            ret,
            span,
        )
    }

    // -----------------------------------------------------------------------
    // Method calls
    // -----------------------------------------------------------------------

    /// `field` is the `x.f` node, not the receiver: a method call has no
    /// production of its own, and taking the one id rather than the receiver
    /// and the name separately keeps the receiver, the name and the name's
    /// span from being able to disagree.
    fn check_method_call(
        &mut self,
        field: ExprId,
        args: &[ExprId],
        explicit: Option<Vec<Ty>>,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let V::Field { base, name, name_span, .. } = self.tree().expr(field) else {
            crate::ice!("a method call is a field access, which is what the caller matched on")
        };
        // `fs.appendBytes(...)` — the base is a namespace, so what is missing
        // is the member. Asked before the receiver, which has no value to check.
        if self.namespace_member_missing(base, name, name_span) {
            return self.error_expr(span);
        }
        let recv = self.check_expr(base, None);
        // A literal that reaches a method call with nothing else constraining
        // it takes its default — `Int` for an integer literal, `Float` for a
        // float — so `5.abs()` resolves in `core/num` (SPEC 5.1.1).
        let recv_ty = self.default_numeric_receiver(&recv.ty);

        // A field of function type is called as `(x.f)(...)`.
        if let Ty::Con(con, targs) = &recv_ty {
            let found = self.c.tables.field_index(*con, name).and_then(|i| {
                self.c.tables.tycon(*con).fields().get(i).map(|f| (i, f.ty.clone()))
            });
            if let Some((i, decl_ty)) = found {
                self.check_field_visible(*con, i, name_span);
                let fty = substitute(&decl_ty, targs, None);
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
                        self.templated("field-not-callable", span)
                            .bind("field", name.to_string())
                            .bind("type", shown);
                        self.error_expr(span)
                    }
                };
            }
        }

        match self.resolve_method(&recv_ty, name, name_span) {
            Some(MethodTarget::Direct(f)) => {
                self.call_fn(f, explicit, args, span, Some(recv), expected)
            }
            Some(MethodTarget::Bound(tid, index)) => {
                self.call_trait_method((tid, index), recv, args, explicit, span, expected)
            }
            None => {
                self.report_no_method(&recv_ty, name, name_span);
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
                if let Some(f) = self.c.tables.method(*con, name) {
                    self.check_method_visible(f, span);
                    // A method an `impl` supplied lands in the type's ordinary
                    // namespace, so `host.stdout.println(...)` — the effect's
                    // implementation, named outright — is a `Direct` hit and
                    // never reaches `find_in_bounds`. It is the same call
                    // written the third way, and it is refused for the same
                    // reason.
                    if let Some((tid, _)) = self.c.tables.fn_info(f).impl_of {
                        self.report_effect_method(tid, name, span);
                    }
                    return Some(MethodTarget::Direct(f));
                }
                // Methods supplied by an `impl` land in the type's ordinary
                // method namespace — including the ones `derive` generates,
                // which have no function of their own to point at.
                let traits = self.c.tables.traits_of_con(*con).to_vec();
                self.find_in_bounds(&traits, name, span)
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
                if let Some((prev, prev_index)) = found {
                    let a = self.c.tables.trait_(prev).name.clone();
                    let c = self.c.tables.trait_(*b).name.clone();
                    self.templated("ambiguous-trait-method", span)
                        .bind("method", name.to_string())
                        .bind("first_trait", a.clone())
                        .bind("second_trait", c)
                        .fix(format!("call it as a function to say which: `{a}.{name}(x, y)`"));
                    self.report_effect_method(prev, name, span);
                    return Some(MethodTarget::Bound(prev, prev_index));
                }
                found = Some((*b, i));
            }
        }
        let (tid, index) = found?;
        self.report_effect_method(tid, name, span);
        Some(MethodTarget::Bound(tid, index))
    }

    /// An effect's methods are called through the module that wraps the
    /// effect, never on the value that carries it (SPEC 10.2).
    ///
    /// `ctx.println(t)` is `io.println(ctx, t)`, and this is what says so for
    /// all three shapes a reader can write. Two of them — a `context` value
    /// and a `ctx: C` bounded by an effect — reach a method through a *bound*,
    /// which is [`Self::find_in_bounds`]. The third is `host.stdout.println(t)`
    /// in `main`: an `impl`-supplied method lands in the type's ordinary
    /// namespace, so that one is a `Direct` hit in [`Self::resolve_method`]'s
    /// `Ty::Con` arm and is reported from there. Two call sites, one rule, one
    /// message.
    ///
    /// **Two layers are below the line and keep the method form.** The
    /// standard library is where the wrappers are, so its bodies are the only
    /// thing that reaches an effect at all; and the body of an `impl` that
    /// *supplies* an effect is where the operation is implemented, which is
    /// what keeps SPEC 10.8's attenuation wrapper writable —
    /// `impl<C: Fs> Fs for ReadOnly<C> { fn readFile(self, p) { self.0.readFile(p) } }`
    /// cannot be `fs.readText(self.0, p)`, because that wrapper is bounded
    /// `Alloc + Fs` and the impl carries only `C: Fs`. The carve-out grants no
    /// new authority: an implementor can only reach an inner context somebody
    /// already handed it.
    ///
    /// The target is still returned by the caller, so the rest of the body
    /// checks against the type the method really has and one wrong call does
    /// not cascade — the shape `ambiguous-trait-method` above already uses.
    fn report_effect_method(&mut self, tid: TraitId, name: &str, span: Span) {
        if !self.c.tables.trait_(tid).is_effect || self.may_call_effect_method() {
            return;
        }
        let effect = self.c.tables.trait_(tid).name.clone();
        let door = standard_library::wrapper(&effect, name);
        let d = self
            .templated("effect-method-call", span)
            .bind("effect", effect.clone())
            .bind("method", name.to_string());
        match door {
            Some(row) => {
                d.fix(row.fix());
                if let Some(import) = row.import() {
                    d.notes.push(import);
                }
            }
            // Unreachable for a declared effect —
            // `every_effect_method_has_a_door` is what makes it so — and the
            // sentence is here for the day somebody adds a method and runs the
            // compiler before running that test.
            None => d.notes.push(format!(
                "`{effect}` is an effect, and an effect is performed by handing the context to a \
                 function rather than by calling a method on it"
            )),
        }
    }

    /// Whether the body being checked is one of the two layers that may call an
    /// effect method directly.
    fn may_call_effect_method(&self) -> bool {
        matches!(self.role, Role::Std | Role::Platform) || self.in_effect_impl
    }

    /// The type arguments `x.f<A, B>(…)` supplies, as a whole trait-method
    /// list.
    ///
    /// `TraitMethod.generics` is the trait's own generics followed by the
    /// method's, flattened into one list (`resolve.rs`), because that is the
    /// order `substitute` reads `Ty::Param` indices in. A call can only write
    /// the method's tail — the trait's are settled by the receiver — so the
    /// written list fills the tail and the prefix stays inferred, and a count
    /// that disagrees is counted against the tail rather than against the
    /// flattened list, which is the only number the reader could have written.
    ///
    /// `None` back means "infer everything", which is both the no-arguments
    /// case and how a rejected list recovers.
    fn trait_method_targs(
        &mut self,
        tid: TraitId,
        generics: &[GenericInfo],
        explicit: Option<Vec<Ty>>,
        span: Span,
    ) -> Option<Vec<Ty>> {
        let explicit = explicit?;
        let owner = self.c.tables.trait_(tid).generics.len();
        let want = generics.len().saturating_sub(owner);
        if explicit.len() != want {
            self.templated("type-argument-mismatch", span)
                .bind("expected", want.to_string())
                .bind("found", explicit.len().to_string())
                .mismatch(want.to_string(), explicit.len().to_string());
            return None;
        }
        let mut targs: Vec<Ty> = (0..owner).map(|_| self.fresh(span)).collect();
        targs.extend(explicit);
        Some(targs)
    }

    /// What `Self` stands for in a call to one of `tid`'s methods on `recv`.
    ///
    /// `Self` is the **implementing** type — the one whose `impl` supplies the
    /// body — and for every receiver but one that is the receiver itself. The
    /// exception is a `context` value: a context satisfies an effect without
    /// implementing it, by naming a value that does, and `CtxType.bindings`
    /// records which. Monomorphization already reads that row to dispatch
    /// (`resolve_trait_call`, `middle/monomorphize.rs`, which rewrites the
    /// receiver into a `CtxGet` of exactly this type), so substituting the
    /// receiver here left a method whose signature names `Self` — `Listen`'s
    /// `onRequest: fn(Self, Request) => Response` — handing the caller's
    /// handler the **context's** layout while the `impl` body hands it the
    /// implementation's. Two different types, and nothing between here and the
    /// machine to notice: the front end had agreed with itself, so it
    /// typechecked, and the native backends read the wrong bytes and crashed
    /// with no output at all.
    ///
    /// The rule is one-directional and that is the point: `Self` is the
    /// implementation *everywhere*, and an effect whose callback is meant to
    /// receive the caller's context takes that context as a `ctx` parameter and
    /// names it, the way `Tasks.parallel` does (SPEC 10.6). Nothing here ever
    /// answers "the receiver", so there is no second reading to get wrong.
    ///
    /// This is the same answer A1 gives a written `Self` in an `impl`
    /// signature (the head's type) and A3 gives an impl head matched against a
    /// receiver — one rule, read at three points.
    ///
    /// A bounded type parameter is the half this cannot settle: a generic body
    /// is checked once for every instantiation at once
    /// (guides/compile-speed.md), so `Self` there is the parameter, which is
    /// right until the parameter turns out to be a context.
    /// `rewrite_call_args` in `middle/monomorphize.rs` is that correction,
    /// made where the instantiation is known.
    ///
    /// A row's value is a value, so it is not itself a context; the loop is
    /// belt and braces against a shape that cannot be written, and it stops
    /// rather than spinning if one ever can be.
    fn implementing_ty(&self, recv: &Ty, tid: TraitId) -> Ty {
        let mut ty = recv.clone();
        for _ in 0..8 {
            let Ty::Ctx(id) = &ty else { return ty };
            // An unbound effect is diagnosed at monomorphization, where the
            // whole context is known; there is nothing better to say here than
            // the receiver.
            let Some(next) = self.c.tables.ctx_type(*id).get(tid).cloned() else {
                return ty;
            };
            ty = next;
        }
        ty
    }

    /// `bound` is the trait and the position in its method list — one argument
    /// because `resolve_method` answers with the pair, and neither half means
    /// anything without the other.
    fn call_trait_method(
        &mut self,
        bound: (TraitId, usize),
        recv: typed::Expr,
        args: &[ExprId],
        explicit: Option<Vec<Ty>>,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let (tid, index) = bound;
        let method = self
            .c
            .tables
            .trait_(tid)
            .methods
            .get(index)
            .or_ice("the index is a position in this same trait's method list")
            .clone();
        let recv_ty = self.resolve(&recv.ty);
        // A trait method's own generics come after the trait's; the
        // *implementing* type supplies `Self`, which is the receiver everywhere
        // except through a context.
        let self_ty = self.implementing_ty(&recv_ty, tid);
        let explicit = self.trait_method_targs(tid, &method.generics, explicit, span);
        let targs = self.instantiate(&method.generics, explicit, span);
        let params: Vec<Ty> = method
            .params
            .iter()
            .map(|p| substitute(&p.ty, &targs, Some(&self_ty)))
            .collect();
        let ret = substitute(&method.ret, &targs, Some(&self_ty));
        if let Some(exp) = expected {
            let _ = self.subst.unify(&self.c.tables, &ret, exp);
        }

        let mut hir_args = vec![recv];
        // The receiver takes the first parameter, so the arguments are checked
        // against what is left.
        let rest = params.split_first().map_or(&[][..], |(_, rest)| rest);
        if args.len() != rest.len() {
            let name = method.name.clone();
            let want = rest.len();
            let have = args.len();
            self.templated("wrong-argument-count", span)
                .bind("function", name)
                .bind("expected", want.to_string())
                .bind("given", have.to_string())
                .mismatch(want.to_string(), have.to_string());
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
        // Four scalars, on the path every method call takes. Copying the whole
        // `FnInfo` to read them copied the parameter list and its types too.
        let (module, exported, from_impl) = {
            let info = self.c.tables.fn_info(f);
            (info.module, info.exported, info.impl_of.is_some())
        };
        if module.0 == u32::MAX || module == self.module {
            return;
        }
        // A method supplied by an `impl` is visible wherever the type is:
        // conformance is a property of the type.
        if from_impl {
            return;
        }
        if !exported {
            let name = self.c.tables.fn_info(f).name.clone();
            self.templated("private-to-module", span)
                .bind("declaration", format!("`{name}`"))
                .fix(format!("add `export` to `{name}`'s declaration, if it is meant to be part of the API"));
            return;
        }
        let (Some(here), Some(there)) =
            (self.c.module(self.module).pkg, self.c.module(module).pkg)
        else {
            return;
        };
        if here == there {
            return;
        }
        if let Some(surface) = self.c.surfaces.get(&there) {
            if !surface.contains(&self.c.tables.fn_info(f).name) {
                let name = self.c.tables.fn_info(f).name.clone();
                let label =
                    self.c.ws.map(|w| w.package(there).label()).unwrap_or_default();
                self.templated("not-on-the-surface", span)
                    .bind("name", name.clone())
                    .bind("owner", label)
                    .fix(format!("re-export `{name}` from that library's lib.buri, if it is part of the API"));
            }
        }
    }

    /// The field is named by its position rather than handed over, so that a
    /// caller on the field-access path need not own a copy of it — the check
    /// reads two fields of it and only on the error path.
    fn check_field_visible(&mut self, con: TyConId, index: usize, span: Span) {
        let owner = self.c.tables.tycon(con).module;
        if owner == self.module || owner.0 == u32::MAX {
            return;
        }
        let Some(f) = self.c.tables.tycon(con).fields().get(index) else { return };
        if f.exported {
            return;
        }
        let name = f.name.clone();
        let ty = self.c.tables.tycon(con).name.clone();
        self.templated("private-to-module", span)
            .bind("declaration", format!("field `{name}` of `{ty}`"))
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
                let refs: Vec<&str> = self.c.tables.method_names(*con).collect();
                if let Some(near) = nearest(name, &refs) {
                    notes.push(format!("did you mean `{near}`?"));
                }
                if self.c.tables.field_index(*con, name).is_some() {
                    notes.push(format!("`{name}` is a field; a field is not called"));
                }
                // A method that is missing because the type did not derive the
                // trait it comes from is a different mistake from one nobody
                // wrote, and the fix for it is a line rather than a body.
                if let Some(t) = self.derivable_trait_declaring(name, *con) {
                    notes.push(format!(
                        "`{name}` comes from `{t}`, which `{shown}` does not implement; \
                         `derive {t} for {shown};` generates it"
                    ));
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
        let fix = self.no_method_fix(recv, &shown, name);
        let d = self
            .templated("no-such-method", span)
            .bind("type", shown)
            .bind("method", name.to_string());
        d.notes.extend(notes);
        // After the binds: every `bind` re-renders the page's own fix over it.
        if let Some(fix) = fix {
            d.fix(fix);
        }
    }

    /// What to do about a missing method, which depends on who owns the
    /// receiver's type. The page's fix — "declare it in an `impl` block in
    /// that type's own module" — is an instruction only the owner can follow,
    /// and a `Result` or an `I64` is owned by the toolchain.
    fn no_method_fix(&self, recv: &Ty, shown: &str, name: &str) -> Option<String> {
        match recv {
            Ty::Param(_) => Some(format!(
                "add a bound to `{shown}` that declares `{name}`, or call one the bounds it \
                 already carries declare"
            )),
            Ty::Tuple(_) | Ty::Fn(..) => Some(format!(
                "there is no module to declare `{name}` in, so write a free function taking \
                 the value, or wrap it in a struct of yours and give that the method"
            )),
            Ty::Con(con, _) => {
                let info = self.c.tables.tycon(*con);
                // A primitive's table entry lives in a synthetic module; the
                // module its methods are declared in is SPEC 6.7.3's.
                let (path, role, header) = match &info.def {
                    TyDef::Prim(p) => {
                        (standard_library::defining_module(*p).to_string(), Role::Std, None)
                    }
                    _ => {
                        let module = self.c.module(info.module);
                        let params: Vec<&str> =
                            info.generics.iter().map(|g| g.name.as_str()).collect();
                        let header = if params.is_empty() {
                            format!("impl {}", info.name)
                        } else {
                            format!("impl<{0}> {1}<{0}>", params.join(", "), info.name)
                        };
                        (module.path.clone(), module.role, Some(header))
                    }
                };
                match (role, header) {
                    (Role::Std | Role::Platform, _) | (_, None) => Some(format!(
                        "check the spelling — `buri docs {path}` lists every method `{shown}` \
                         has, and one may not be added to it from outside `{path}`"
                    )),
                    (_, Some(header)) => Some(format!(
                        "check the spelling, or declare it in `{header} {{ ... }}` in \
                         `{path}`, where the type is declared — a method may not be added \
                         from anywhere else"
                    )),
                }
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Fields, variants, literals
    // -----------------------------------------------------------------------

    fn check_field(
        &mut self,
        base: ExprId,
        name: &str,
        name_span: Span,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // A namespace member, a qualified variant, or a constant.
        if let Some(s) = self.static_ref_field(base, name) {
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
                    self.templated("context-not-called", span);
                    self.error_expr(span)
                }
                Static::TupleStruct(_) => {
                    self.templated("tuple-struct-not-called", span);
                    self.error_expr(span)
                }
            };
        }

        // `fs.appendBytes`, or `host.ui` where this output's platform grants
        // no `Ui`. Asked here, between "is it a namespace member" and "is it a
        // field", because by here the name is known not to be a member and the
        // base is still a namespace rather than a value — which is the one
        // moment both halves of the answer are in hand.
        if self.namespace_member_missing(base, name, name_span) {
            return self.error_expr(span);
        }

        let b = self.check_expr(base, None);
        let bty = self.resolve(&b.ty);
        match &bty {
            Ty::Con(con, targs) => {
                let found = self.c.tables.field_index(*con, name).and_then(|i| {
                    self.c.tables.tycon(*con).fields().get(i).map(|f| (i, f.ty.clone()))
                });
                if let Some((i, decl_ty)) = found {
                    self.check_field_visible(*con, i, name_span);
                    let ty = substitute(&decl_ty, targs, None);
                    return typed::Expr::new(
                        typed::ExprKind::Field { base: Box::new(b), index: i },
                        ty,
                        span,
                    );
                }
                // A method is not a value: `x.f` must be immediately called.
                if self.c.tables.method(*con, name).is_some() {
                    let n = name.to_string();
                    self.templated("method-not-a-value", span).bind("name", n);
                    return self.error_expr(span);
                }
                self.report_no_field(&bty, name, name_span);
                self.error_expr(span)
            }
            Ty::Error => self.error_expr(span),
            _ => {
                self.report_no_field(&bty, name, name_span);
                self.error_expr(span)
            }
        }
    }

    /// Whether `base.name` names a `core/host` export this output's platform
    /// withholds, reporting it when it does.
    ///
    /// Without this the refusal reads "there is nothing named `host` in
    /// scope", pointing at the namespace rather than at the effect and telling
    /// a reader to check a spelling that is correct.
    /// Reports naming a member the namespace's module does not export,
    /// answering whether it did.
    ///
    /// Asked before the receiver is checked, because a namespace is not a
    /// value: checking it reports the base as an unresolved name and never
    /// says which member was missing, which is the whole of the question.
    fn namespace_member_missing(&mut self, base: ExprId, name: &str, name_span: Span) -> bool {
        let V::Ident { name: head, .. } = self.tree().expr(base) else { return false };
        let head = head.to_string();
        if self.lookup_local(&head).is_some() {
            return false;
        }
        let Some(ns) = self.c.scope(self.module).namespaces.get(&head).copied() else {
            return false;
        };
        if self.c.lookup_export(ns, name).is_some() {
            return false;
        }
        // A `core/host` name this platform withholds is missing on purpose,
        // and a list of what is left would not say why.
        if self.host_grant_refused(base, name, name_span) {
            return true;
        }
        let path = self.c.loaded.module(ns).path.clone();
        let mut exports: Vec<String> = self.c.scope(ns).exports.keys().cloned().collect();
        exports.sort();
        // A surface of a dozen names is worth reading here; `core/list`'s is
        // not, and the fix already points at the page that lists it.
        let listed = match exports.len() {
            0 => "nothing".to_string(),
            1..=12 => diagnostics::names(&exports),
            n => format!("{n} names"),
        };
        let near = self.c.nearest_export(ns, name);
        let d = self
            .templated("no-such-member", name_span)
            .bind("path", path)
            .bind("name", name)
            .bind("exports", listed);
        if let Some(n) = near {
            d.notes.push(format!("did you mean `{n}`?"));
        }
        true
    }

    fn host_grant_refused(&mut self, base: ExprId, name: &str, name_span: Span) -> bool {
        let V::Ident { name: head, .. } = self.tree().expr(base) else { return false };
        let head = head.to_string();
        if self.lookup_local(&head).is_some() {
            return false;
        }
        let Some(ns) = self.c.scope(self.module).namespaces.get(&head).copied() else {
            return false;
        };
        if self.c.loaded.module(ns).path != crate::compiler::standard_library::HOST_MODULE {
            return false;
        }
        self.c.report_host_not_granted(name_span, name)
    }

    /// The derivable trait that declares `name`, when `con` does not implement
    /// it. `None` when the method belongs to no such trait, or when the type
    /// already implements the one it belongs to — in which case the method is
    /// there and this is a different mistake.
    fn derivable_trait_declaring(&self, name: &str, con: TyConId) -> Option<String> {
        let implemented = self.c.tables.traits_of_con(con);
        crate::compiler::semantics::resolve::DERIVABLE.iter().find_map(|t| {
            let (id, info) = self
                .c
                .tables
                .traits
                .iter()
                .enumerate()
                .find(|(_, info)| info.name == **t)?;
            let id = TraitId(id as u32);
            if info.method_index(name).is_none() || implemented.contains(&id) {
                return None;
            }
            Some(info.name.clone())
        })
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
        let d = self
            .templated("no-such-field", span)
            .bind("type", shown)
            .bind("field", name.to_string());
        if let Some(n) = note {
            d.notes.push(n);
        }
    }

    fn check_dot_variant(
        &mut self,
        name: &str,
        args: &[ExprId],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        self.check_dot_variant_call(name, args, span, span, expected)
    }

    /// The dot form requires that the expected type is known from context
    /// (design/static-rules.md rule 12).
    fn check_dot_variant_call(
        &mut self,
        name: &str,
        args: &[ExprId],
        dot_span: Span,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let Some(exp) = expected.map(|t| self.resolve(t)) else {
            let n = name.to_string();
            self.templated("unannotated-variant", dot_span).bind("variant", n);
            return self.error_expr(span);
        };
        let Ty::Con(con, _) = &exp else {
            if !exp.is_error() {
                let shown = self.show_ty(&exp);
                self.templated("not-an-enum", dot_span)
                    .bind("type", shown)
                    .fix("the dot form names an enum variant; write the value another way");
            }
            return self.error_expr(span);
        };
        let Some(index) = self.c.tables.variant_index(*con, name) else {
            let ty = self.c.tables.tycon(*con).name.clone();
            let note = crate::compiler::semantics::patterns::no_variant_note(
                &self.c.tables,
                *con,
                name,
            );
            let d = self.templated("no-such-variant", dot_span)
                .bind("type", ty)
                .bind("variant", name.to_string());
            d.notes.extend(note);
            return self.error_expr(span);
        };
        self.construct_variant(*con, index, args, span, Some(&exp), dot_span)
    }

    fn construct_variant(
        &mut self,
        con: TyConId,
        index: usize,
        args: &[ExprId],
        span: Span,
        expected: Option<&Ty>,
        head_span: Span,
    ) -> typed::Expr {
        // One variant, not the type: copying the `TyCon` copied every variant
        // of the enum and every field of each of them, to write one of them
        // down.
        let (owner, arity, variant) = {
            let tycon = self.c.tables.tycon(con);
            (
                tycon.module,
                tycon.arity(),
                tycon
                    .variants()
                    .get(index)
                    .or_ice("the variant index came from this same type's variant list")
                    .clone(),
            )
        };

        // A variant is exported exactly when its enum is, so this fires only
        // where a private enum reached another module through a signature.
        if owner != self.module && !variant.exported && owner.0 != u32::MAX {
            let t = self.c.tables.tycon(con).name.clone();
            let v = variant.name.clone();
            self.templated("private-to-module", head_span)
                .bind("declaration", format!("variant `{v}` of `{t}`"))
                .fix(format!("add `export` to `{t}`, or build the value through a function `{t}`'s module provides"));
        }

        // Type arguments come from the expected type where there is one, and
        // from the payload otherwise.
        let targs: Vec<Ty> = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Con(c, ts)) if c == con => ts,
            _ => (0..arity).map(|_| self.fresh(span)).collect(),
        };

        let param_types: Vec<Ty> =
            variant.fields.iter().map(|f| substitute(&f.ty, &targs, None)).collect();
        if args.len() != param_types.len() {
            let v = variant.name.clone();
            let want = param_types.len();
            let have = args.len();
            self.templated("wrong-value-count", head_span)
                .bind("name", v)
                .bind("expected", want.to_string())
                .bind("given", have.to_string())
                .mismatch(want.to_string(), have.to_string());
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
        elems: &[ExprId],
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
                let c = self.check_expr(*e, Some(&elem_ty));
                let espan = self.tree().span(*e);
                self.unify_at(espan, &c.ty.clone(), &elem_ty, "the element type");
                c
            })
            .collect();
        // An array literal has a statically known length and is not, by
        // itself, an allocation the programmer must account for.
        typed::Expr::new(typed::ExprKind::Array(checked), Ty::Array(Box::new(elem_ty)), span)
    }

    fn check_struct_lit(
        &mut self,
        head: Option<ExprId>,
        spread: Option<ExprId>,
        fields: &[InitData],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // `{ hi: "hi" }` with no head at all. The type is whatever the literal
        // is checked against, read top-down and never solved for
        // (design/grammar-rationale.md 12.3).
        let Some(head) = head else {
            let Some((con, targs)) = self.anonymous_struct_type(span, expected) else {
                return self.error_expr(span);
            };
            return self.build_struct_lit(con, targs, spread, fields, span);
        };
        let head_span = self.tree().span(head);
        // The head must be a type path, optionally with type arguments, or the
        // dot form (design/static-rules.md rule 1).
        if !self.tree().is_type_path(head) {
            self.templated("struct-literal-head", head_span);
            return self.error_expr(span);
        }

        // A record-like enum variant: `.Rect { width, height }`.
        if let Some((con, index)) = self.struct_lit_variant(head, expected) {
            return self.build_record_variant(con, index, fields, span, expected, head_span);
        }

        let explicit = match self.tree().expr(head) {
            V::Generic { args, .. } => Some(self.elaborate_all(args)),
            _ => None,
        };
        let Some(con) = self.struct_lit_head(head) else {
            // `host.HostNet {}` where this output's platform grants no `Net`.
            // The implementation struct is withheld with the value it
            // implements, and this is the one other place a program can name
            // one — so it gets the refusal that names the platform rather than
            // one that says the type is not a type.
            if let V::Field { base, name, .. } = self.tree().expr(head) {
                let name = name.to_string();
                if self.host_grant_refused(base, &name, head_span) {
                    return self.error_expr(span);
                }
            }
            self.templated("not-a-struct-literal-head", head_span);
            return self.error_expr(span);
        };
        // The fields are what this needs; copying the `TyCon` for them copied
        // its name and its generics too, per literal.
        let (is_struct, arity) = {
            let tycon = self.c.tables.tycon(con);
            (matches!(tycon.def, TyDef::Struct { .. }), tycon.arity())
        };
        if !is_struct {
            let n = self.c.tables.tycon(con).name.clone();
            self.templated("enum-without-a-variant", head_span)
                .bind("type", n.clone())
                .fix(format!("write `{n}.Variant {{ ... }}`"));
            return self.error_expr(span);
        }

        let targs: Vec<Ty> = match explicit {
            Some(ts) if ts.len() == arity => ts,
            _ => match expected.map(|t| self.resolve(t)) {
                Some(Ty::Con(c, ts)) if c == con => ts,
                _ => (0..arity).map(|_| self.fresh(span)).collect(),
            },
        };
        self.build_struct_lit(con, targs, spread, fields, span)
    }

    /// The fields, once the struct and its type arguments are settled.
    ///
    /// Shared by the two ways a literal names its type — a head that is a type
    /// path, and an anonymous `{ ... }` that reads it from its surroundings —
    /// so elision, shorthand, visibility and the spread behave identically
    /// under both.
    fn build_struct_lit(
        &mut self,
        con: TyConId,
        targs: Vec<Ty>,
        spread: Option<ExprId>,
        fields: &[InitData],
        span: Span,
    ) -> typed::Expr {
        let ty = Ty::Con(con, targs.clone());
        let decl_fields = self.c.tables.tycon(con).fields().to_vec();

        let mut values: Vec<Option<typed::Expr>> = vec![None; decl_fields.len()];
        let mut seen: Vec<&str> = Vec::new();
        for init in fields {
            let t = self.tree();
            let iname = t.text(init.name);
            let ispan = t.span_of(init.name);
            let found = decl_fields
                .iter()
                .enumerate()
                .find(|(_, f)| f.name == iname)
                .map(|(i, f)| (i, f.clone()));
            let Some((i, decl)) = found else {
                let t = self.c.tables.tycon(con).name.clone();
                self.templated("no-such-field", ispan)
                    .bind("type", t)
                    .bind("field", iname.to_string());
                continue;
            };
            if seen.contains(&iname) {
                self.templated("duplicate-field-initializer", ispan)
                    .bind("field", iname.to_string());
            }
            seen.push(iname);
            self.check_field_visible(con, i, ispan);
            let want = substitute(&decl.ty, &targs, None);
            let value = match t.opt(init.value) {
                Some(v) => {
                    let c = self.check_expr(v, Some(&want));
                    let vspan = t.span(v);
                    self.unify_at(vspan, &c.ty.clone(), &want, "the field type");
                    c
                }
                // Field shorthand: `Point { x, y }` binds `x: x`.
                None => {
                    let c = self.check_ident(iname, ispan, Some(&want));
                    self.unify_at(ispan, &c.ty.clone(), &want, "the field type");
                    c
                }
            };
            if let Some(slot) = values.get_mut(i) {
                *slot = Some(value);
            }
        }

        match spread {
            Some(base) => {
                // Functional update never names the hidden fields, so it works
                // anywhere the type is visible.
                let b = self.check_expr(base, Some(&ty));
                let base_span = self.tree().span(base);
                self.unify_at(base_span, &b.ty.clone(), &ty, "the base of the update");
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
                let mut missing: Vec<String> = Vec::new();
                let mut filled: Vec<typed::Expr> = Vec::with_capacity(decl_fields.len());
                for (f, v) in decl_fields.iter().zip(values) {
                    let e = match v {
                        Some(e) => e,
                        None => {
                            let want = substitute(&f.ty, &targs, None);
                            match self.elided_none(&f.ty, &want, span) {
                                Some(none) => none,
                                None => {
                                    missing.push(f.name.clone());
                                    typed::Expr::new(typed::ExprKind::Error, want, span)
                                }
                            }
                        }
                    };
                    filled.push(e);
                }
                if !missing.is_empty() {
                    let t = self.c.tables.tycon(con).name.clone();
                    let missing = diagnostics::names(&missing);
                    self.templated("missing-field-value", span)
                        .bind("name", t.clone())
                        .bind("fields", missing.clone())
                        .fix(format!(
                            "give {missing} a value — only an `Option` field may be left out — \
                             or start from an existing one with `{t} {{ ..base, field: value }}`"
                        ));
                }
                typed::Expr::new(typed::ExprKind::StructLit { con, targs, fields: filled }, ty, span)
            }
        }
    }

    /// The `.None` a left-out `Option` field stands for (SPEC 5.6). Judged on
    /// the declared type, never the substituted one, so which fields a literal
    /// may leave out is a property of the struct rather than of one
    /// instantiation of it.
    fn elided_none(&self, declared: &Ty, want: &Ty, span: Span) -> Option<typed::Expr> {
        let con = match declared {
            Ty::Con(id, _) if self.c.option_con == Some(*id) => *id,
            _ => return None,
        };
        let variant = self.c.tables.tycon(con).variant_index("None")?;
        let targs = match want {
            Ty::Con(_, args) => args.clone(),
            _ => return None,
        };
        Some(typed::Expr::new(
            typed::ExprKind::EnumLit { con, targs, variant, args: Vec::new() },
            want.clone(),
            span,
        ))
    }

    /// The struct an anonymous `{ ... }` builds, and its type arguments.
    ///
    /// Strictly top-down. The expected type is *read*, never solved for: the
    /// claim the syntax makes is that the type is trivially inferable, and the
    /// only thing that can make it trivial is that something above the literal
    /// already said it. So a literal is refused rather than inferred wherever
    /// the answer would have had to come from the fields — which is what keeps
    /// this one lookup instead of a search, and keeps the type independent of
    /// the order the checker visits an expression in.
    ///
    /// A generic struct is accepted only where every argument is already
    /// settled. `Holder<Int>` is a type a reader can see; `Holder<?>` is one
    /// the fields would have to decide, and deciding it here is the inference
    /// this feature is not.
    fn anonymous_struct_type(
        &mut self,
        span: Span,
        expected: Option<&Ty>,
    ) -> Option<(TyConId, Vec<Ty>)> {
        let refuse = |me: &mut Self, fix: String| {
            me.templated("struct-literal-type", span).fix(fix);
            None
        };
        let name_it = "write the type before the `{`, as in `World { hi: \"hi\" }`".to_string();
        let Some(want) = expected else {
            return refuse(self, name_it);
        };
        let resolved = self.resolve(want);
        // Poison. One type error should not produce two.
        if matches!(resolved, Ty::Error) {
            return None;
        }
        let Ty::Con(con, targs) = resolved else {
            let fix = if matches!(self.resolve(want), Ty::Var(_)) {
                "write the type before the `{` — nothing here has settled it yet".to_string()
            } else {
                name_it
            };
            return refuse(self, fix);
        };
        match self.c.tables.tycon(con).def {
            TyDef::Struct { .. } => {}
            // A type alone does not say which variant a literal builds, so the
            // dot form is the answer here rather than the type name.
            TyDef::Enum { .. } => {
                let n = self.c.tables.tycon(con).name.clone();
                return refuse(self, format!("write `{n}.Variant {{ ... }}`"));
            }
            // A primitive. Naming it in the fix would be advice that cannot be
            // taken, and the spelling stored for it is the underlying one
            // (`I64` for `Int`), so the fix stays about the literal.
            TyDef::Prim(_) => return refuse(self, name_it),
        }
        if !targs.iter().all(|t| self.is_settled(t)) {
            let shown = self.show_ty(&Ty::Con(con, targs));
            return refuse(
                self,
                format!(
                    "write the type before the `{{` — `{shown}` still has a type argument \
                     nothing has settled"
                ),
            );
        }
        Some((con, targs))
    }

    fn struct_lit_head(&mut self, head: ExprId) -> Option<TyConId> {
        match self.tree().expr(head) {
            V::Ident { name, .. } => {
                match self.c.scope(self.module).names.get(name)? {
                    Sym::Ty(c) => Some(*c),
                    _ => None,
                }
            }
            V::Generic { base, .. } => self.struct_lit_head(base),
            V::Field { base, name, .. } => {
                let V::Ident { name: head, .. } = self.tree().expr(base) else { return None };
                let ns = self.c.scope(self.module).namespaces.get(head).copied()?;
                match self.c.lookup_export(ns, name)? {
                    Sym::Ty(c) => Some(c),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn struct_lit_variant(
        &mut self,
        head: ExprId,
        expected: Option<&Ty>,
    ) -> Option<(TyConId, usize)> {
        match self.tree().expr(head) {
            V::DotVariant { name, .. } => {
                let Ty::Con(con, _) = self.resolve(expected?) else { return None };
                let index = self.c.tables.variant_index(con, name)?;
                Some((con, index))
            }
            V::Field { .. } => match self.static_ref(head) {
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
        fields: &[InitData],
        span: Span,
        expected: Option<&Ty>,
        head_span: Span,
    ) -> typed::Expr {
        // The one variant, for the reason `construct_variant` gives.
        let (owner, arity, variant) = {
            let tycon = self.c.tables.tycon(con);
            (
                tycon.module,
                tycon.arity(),
                tycon
                    .variants()
                    .get(index)
                    .or_ice("the variant index came from this same type's variant list")
                    .clone(),
            )
        };
        if owner != self.module && !variant.exported && owner.0 != u32::MAX {
            let t = self.c.tables.tycon(con).name.clone();
            let v = variant.name.clone();
            self.templated("private-to-module", head_span)
                .bind("declaration", format!("variant `{v}` of `{t}`"))
                .fix(format!("add `export` to `{t}`, or build the value through a function `{t}`'s module provides"));
        }
        let targs: Vec<Ty> = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Con(c, ts)) if c == con => ts,
            _ => (0..arity).map(|_| self.fresh(span)).collect(),
        };
        let mut values: Vec<Option<typed::Expr>> = vec![None; variant.fields.len()];
        for init in fields {
            let t = self.tree();
            let iname = t.text(init.name);
            let ispan = t.span_of(init.name);
            let found = variant
                .fields
                .iter()
                .enumerate()
                .find(|(_, f)| f.name == iname)
                .map(|(i, f)| (i, f.ty.clone()));
            let Some((i, decl_ty)) = found else {
                let v = variant.name.clone();
                self.templated("no-such-field", ispan)
                    .bind("type", v)
                    .bind("field", iname.to_string());
                continue;
            };
            let want = substitute(&decl_ty, &targs, None);
            let value = match t.opt(init.value) {
                Some(v) => {
                    let c = self.check_expr(v, Some(&want));
                    let vspan = t.span(v);
                    self.unify_at(vspan, &c.ty.clone(), &want, "the field type");
                    c
                }
                None => {
                    let c = self.check_ident(iname, ispan, Some(&want));
                    self.unify_at(ispan, &c.ty.clone(), &want, "the field type");
                    c
                }
            };
            if let Some(slot) = values.get_mut(i) {
                *slot = Some(value);
            }
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
            self.templated("missing-field-value", span)
                .bind("name", v)
                .bind("fields", missing.clone())
                .fix(format!("give {missing} a value"));
        }
        let filled: Vec<typed::Expr> = values
            .into_iter()
            .zip(&variant.fields)
            .map(|(v, f)| {
                v.unwrap_or_else(|| {
                    typed::Expr::new(
                        typed::ExprKind::Error,
                        substitute(&f.ty, &targs, None),
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
        operand: ExprId,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // `-128` is one literal, not a negation of `128`: SPEC 5.1.1 gives
        // `let y: I8 = -129;` as the error, which makes `-128` legal, and it
        // is not if the magnitude is range-checked on its own.
        if op == tree::UnOp::Neg {
            if let V::Int { value, raw, .. } = self.tree().expr(operand) {
                let ty = self.subst.fresh_num(NumClass::Int, span);
                if let Some(exp) = expected {
                    let _ = self.subst.unify(&self.c.tables, &ty, exp);
                }
                self.lit_checks.push(LitCheck {
                    value,
                    negative: true,
                    raw: raw.to_string(),
                    ty: ty.clone(),
                    span,
                });
                return typed::Expr::new(typed::ExprKind::Int(value, true), ty, span);
            }
            if let V::Float { value, span: fspan, .. } = self.tree().expr(operand) {
                let ty = self.subst.fresh_num(NumClass::Float, fspan);
                if let Some(exp) = expected {
                    let _ = self.subst.unify(&self.c.tables, &ty, exp);
                }
                return typed::Expr::new(typed::ExprKind::Float(-value), ty, span);
            }
        }
        let e = match op {
            tree::UnOp::Not => {
                let b = self.prim(Prim::Bool);
                let e = self.check_expr(operand, Some(&b));
                let ospan = self.tree().span(operand);
                self.unify_at(ospan, &e.ty.clone(), &b, "`!` takes a `Bool`");
                return typed::Expr::new(
                    typed::ExprKind::Prim { op: typed::PrimOp::Not, prim: Prim::Bool, args: vec![e] },
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
                if let Some(p) = prim.filter(|p| p.is_signed() || p.is_float()) {
                    typed::Expr::new(
                        typed::ExprKind::Prim { op: typed::PrimOp::Neg, prim: p, args: vec![e] },
                        ty,
                        span,
                    )
                } else {
                    self.operator_trait_call("Neg", "neg", e, None, span)
                }
            }
            tree::UnOp::BitNot => {
                if let Some(p) = prim.filter(|p| p.is_integer()) {
                    typed::Expr::new(
                        typed::ExprKind::Prim { op: typed::PrimOp::BitNot, prim: p, args: vec![e] },
                        ty,
                        span,
                    )
                } else {
                    let shown = self.show_ty(&ty);
                    if !ty.is_error() {
                        self.templated("bitwise-on-a-non-integer", span)
                            .bind("operator", "~")
                            .bind("type", shown)
                            .fix("use `!` for a `Bool`");
                    }
                    self.error_expr(span)
                }
            }
            // `!` returned from the match above; it cannot arrive here.
            tree::UnOp::Not => crate::ice!("`!` is handled before the operand's type is known"),
        }
    }

    fn check_binary(
        &mut self,
        op: tree::BinOp,
        lhs: ExprId,
        rhs: ExprId,
        op_span: Span,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        use tree::BinOp as B;
        match op {
            B::And | B::Or => {
                let b = self.prim(Prim::Bool);
                let l = self.check_expr(lhs, Some(&b));
                let lspan = self.tree().span(lhs);
                self.unify_at(lspan, &l.ty.clone(), &b, "a logical operand");
                let r = self.check_expr(rhs, Some(&b));
                let rspan = self.tree().span(rhs);
                self.unify_at(rspan, &r.ty.clone(), &b, "a logical operand");
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
                let payload = self
                    .known_option_payload(&lty)
                    .map(|t| (t.clone(), typed::OptionOrResult::Option))
                    .or_else(|| {
                        self.known_result_payload(&lty)
                            .map(|(ok, _)| (ok.clone(), typed::OptionOrResult::Result))
                    });
                let (inner, kind) = match payload {
                    Some(found) => found,
                    None if lty.is_error() => return self.error_expr(span),
                    None => {
                        let shown = self.show_ty(&lty);
                        self.templated("coalesce-operand", op_span).bind("type", shown);
                        return self.error_expr(span);
                    }
                };
                let r = self.check_expr(rhs, Some(&inner));
                let rspan = self.tree().span(rhs);
                self.unify_at(rspan, &r.ty.clone(), &inner, "the default");
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
        let rspan = self.tree().span(rhs);
        self.unify_at(rspan, &r.ty.clone(), &lty, "the left operand's type");
        let ty = self.resolve(&l.ty);
        let prim = self.as_prim(&ty);
        let _ = expected;

        // `Eq` is not defined for function types, `Template`, or opaque
        // types, so comparing those is a compile error rather than a
        // representation accident.
        if matches!(prim, Some(Prim::Template))
            && matches!(op, B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge)
        {
            self.templated("missing-conformance", op_span)
                .bind("type", "Template")
                .bind("trait", "Eq")
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
            let bitwise = matches!(op, B::BitAnd | B::BitOr | B::BitXor);
            let usable = prim.filter(|p| {
                (p.is_integer() || p.is_float()) && (!bitwise || p.is_integer())
            });
            if let Some(p) = usable {
                return typed::Expr::new(
                    typed::ExprKind::Prim { op: prim_op, prim: p, args: vec![l, r] },
                    ty,
                    span,
                );
            }
            if bitwise {
                if !ty.is_error() {
                    let shown = self.show_ty(&ty);
                    self.templated("bitwise-on-a-non-integer", op_span)
                        .bind("operator", op.text())
                        .bind("type", shown)
                        .fix("use `&&` and `||` for a `Bool`");
                }
                return self.error_expr(span);
            }
            let Some((tr, method)) = op.trait_method() else {
                crate::ice!("every arithmetic operator names a trait method")
            };
            return self.operator_trait_call(tr, method, l, Some(r), span);
        }

        // Comparison.
        let bool_ty = self.prim(Prim::Bool);
        match op {
            B::Eq | B::Ne => {
                if let Some(pr) = prim {
                    let p = if op == B::Eq { typed::PrimOp::Eq } else { typed::PrimOp::Ne };
                    return typed::Expr::new(
                        typed::ExprKind::Prim { op: p, prim: pr, args: vec![l, r] },
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
                            prim: Prim::Bool,
                            args: vec![call],
                        },
                        bool_ty,
                        span,
                    )
                }
            }
            B::Lt | B::Le | B::Gt | B::Ge => {
                if let Some(pr) = prim {
                    let p = match op {
                        B::Lt => typed::PrimOp::Lt,
                        B::Le => typed::PrimOp::Le,
                        B::Gt => typed::PrimOp::Gt,
                        _ => typed::PrimOp::Ge,
                    };
                    return typed::Expr::new(
                        typed::ExprKind::Prim { op: p, prim: pr, args: vec![l, r] },
                        bool_ty,
                        span,
                    );
                }
                // `a < b` is `a.compare(b)` tested against an `Order`.
                let cmp = self.operator_trait_call("Ord", "compare", l, Some(r), span);
                self.order_test(cmp, op, span)
            }
            // `&&`, `||`, `??` and the arithmetic and bitwise operators all
            // returned above, so only the comparisons reach here.
            _ => crate::ice!("every other binary operator returns before this match"),
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
            let mut d = Diagnostic::templated("missing-conformance", span)
                .with_bind("type", shown.clone())
                .with_bind("trait", trait_name);
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
        let sig = self
            .c
            .tables
            .trait_(tid)
            .methods
            .get(index)
            .or_ice("an operator trait is one the prelude declares, with its method")
            .clone();
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
        let Some(order) = self.c.order_con else {
            return self.error_expr(span);
        };
        let bool_ty = self.prim(Prim::Bool);
        let (target, when_match) = match op {
            B::Lt => ("Less", true),
            B::Ge => ("Less", false),
            B::Gt => ("Greater", true),
            _ => ("Greater", false),
        };
        let index = self.c.tables.variant_index(order, target).unwrap_or(0);
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
    fn check_try(&mut self, base: ExprId, span: Span) -> typed::Expr {
        let b = self.check_expr(base, None);
        let bty = self.resolve(&b.ty);
        let ret = self.resolve(&self.ret.clone());
        let result = self.known_result_payload(&bty).map(|(ok, err)| (ok.clone(), err.clone()));
        let option = self.known_option_payload(&bty).cloned();
        let (inner, kind) = match (result, option) {
            (Some((ok_ty, err_ty)), _) => {
                // The enclosing function must return `Result<_, E>`. There is
                // no automatic error conversion; map the error explicitly.
                match self.known_result_payload(&ret).map(|(_, err)| err.clone()) {
                    Some(ret_err) => {
                        if self.subst.unify(&self.c.tables, &err_ty, &ret_err).is_err() {
                            let from = self.show_ty(&err_ty);
                            let to = self.show_ty(&ret_err);
                            self.templated("error-type-mismatch", span)
                                .bind("from", from)
                                .bind("to", to);
                        }
                    }
                    None => {
                        let shown = self.show_ty(&ret);
                        self.templated("question-mark-mismatch", span)
                            .bind("container", "a `Result`")
                            .bind("type", shown)
                            .fix("return a `Result` from this function, or handle the error here with `match` or `??`");
                    }
                }
                (ok_ty, typed::OptionOrResult::Result)
            }
            (None, Some(inner)) => {
                if !self.is_known_option(&ret) {
                    let shown = self.show_ty(&ret);
                    self.templated("question-mark-mismatch", span)
                        .bind("container", "an `Option`")
                        .bind("type", shown)
                        .fix("return an `Option` from this function, or turn absence into an error with `.okOr(e)?`");
                }
                (inner, typed::OptionOrResult::Option)
            }
            _ if bty.is_error() => return self.error_expr(span),
            _ => {
                let shown = self.show_ty(&bty);
                self.templated("try-operand", span).bind("type", shown);
                return self.error_expr(span);
            }
        };
        typed::Expr::new(typed::ExprKind::Try { base: Box::new(b), kind }, inner, span)
    }

    /// Hole expressions must have type `Int` (any width), `Float` (any width),
    /// `Bool`, `Char`, or `Str`. There is no user-extensible display mechanism
    /// in v0.3; convert explicitly.
    fn check_template(&mut self, parts: &[PartData], span: Span) -> typed::Expr {
        let mut out = Vec::new();
        for p in parts {
            match self.tree().part(*p) {
                PartView::Text(t) => out.push(typed::TemplatePart::Text(t.to_string())),
                PartView::Hole(e) => {
                    let checked = self.check_expr(e, None);
                    // Deferred: a hole holding `1 + 1` has an unresolved
                    // literal type until defaulting has run.
                    let espan = self.tree().span(e);
                    self.hole_checks.push((checked.ty.clone(), espan));
                    out.push(typed::TemplatePart::Hole(checked));
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
        params: &[LambdaParamData],
        ret: Option<TypeId>,
        body: ExprId,
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        // A lambda's parameter types come from the expected type at its call
        // site, which is known before the body is visited
        // (guides/compile-speed.md).
        let (want_params, want_ret) = match expected.map(|t| self.resolve(t)) {
            Some(Ty::Fn(ps, r)) if ps.len() == params.len() => (ps, Some(*r)),
            _ => (vec![Ty::Error; params.len()], None),
        };

        self.push_scope();
        self.lambda_depth = self.lambda_depth.saturating_add(1);
        let outer_scopes = self.scopes.len();
        // Locals are numbered in the order they are bound, so everything this
        // lambda introduces — its parameters, its `let`s, the names its match
        // arms bind — is at or past this mark, and everything a capture could
        // name is before it.
        let outer_locals = self.locals.len();
        let mut locals = Vec::new();
        let mut ptypes = Vec::new();
        for (i, p) in params.iter().enumerate() {
            let t = self.tree();
            let pname = t.text(p.name);
            let pspan = t.span_of(p.span);
            let ty = match t.opt_type(p.ty) {
                Some(id) => self.c.elaborate(self.module, &self.generics, id),
                None => match want_params.get(i) {
                    Some(t) if !t.is_error() => t.clone(),
                    _ => self.fresh(pspan),
                },
            };
            let local = self.new_local(pname, ty.clone(), pspan);
            self.bind(pname, local);
            // A lambda's parameter is a binding like any other, so a lambda
            // *inside* it may not close over one that carries authority. This
            // is the `fn(c, p) => { let g = fn() => fs.exists(c, p); g() }`
            // route: `c` is the context a `*Ctx` combinator passed in, and
            // without this the capture rule would stop at one level.
            self.note_capture_risk(local, &ty);
            locals.push(local);
            ptypes.push(ty);
        }
        let declared_ret = ret.map(|id| self.c.elaborate(self.module, &self.generics, id));
        let expect_ret = declared_ret.clone().or_else(|| want_ret.clone());
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
            let body_span = self.tree().span(body);
            self.unify_at(body_span, &body_hir.ty.clone(), r, "the declared return type");
        }
        // A lambda whose body answers a `Result` the callback's own return
        // type is not, reported — because `ret_ty` below would otherwise take
        // the *expected* type and throw the body's away, and a `Result` thrown
        // away is the one thing the language says can never happen.
        //
        // `xs.map(ctx, fn(n) => n.toU8())` is the case that named this: `[B]`
        // is unified with the expected `[U8]` before the argument is visited,
        // so `want_ret` is already `U8` when the body is, the expectation
        // `check_expr` carries is a hint a mismatch does not disturb, and the
        // lambda then claimed `fn(I64) => U8` while its body built a
        // `Result<U8, RangeError>`. Nothing downstream could see through that
        // claim: the argument check compared the lie against `fn(A) => B` and
        // agreed, and the program ran with a byte read out of a value that was
        // never one — every `.toU8()` silently a zero.
        //
        // Stated over the *must-use* family rather than over every mismatch,
        // and deliberately: a lambda body is still allowed to answer a
        // `Template` where a `Str` is wanted (§3.3 — there is no `Template` at
        // run time) and to answer a value where `()` is wanted (the reactive
        // callbacks in `ui/`, whose value is discarded). Those two are the
        // leniency this position has always had, they cost nothing at run
        // time, and neither is a value the language promises was consumed.
        // Dropping a `Result` is, so it is the one this refuses.
        if declared_ret.is_none() {
            if let Some(r) = want_ret.clone() {
                if self.is_known_result(&body_hir.ty) && !self.is_known_result(&r) {
                    let body_span = self.tree().span(body);
                    self.unify_at(
                        body_span,
                        &body_hir.ty.clone(),
                        &r,
                        "the expected return type",
                    );
                }
            }
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
        self.lambda_depth = self.lambda_depth.saturating_sub(1);
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
                let name = self.local(*c).name.clone();
                self.templated("lambda-captures-effect", span).bind("name", name);
                break;
            }
            if self.poly_locals.contains(c) {
                let name = self.local(*c).name.clone();
                let shown = self.show_ty(&self.local(*c).ty.clone());
                self.templated("lambda-captures-generic", span)
                    .bind("name", name)
                    .bind("type", shown);
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
        body: CtxBodyId,
        span: Span,
    ) -> typed::Expr {
        if !self.may_build_context() {
            self.templated("context-not-allowed", span);
        }

        let mut bindings: Vec<(TraitId, typed::Expr)> = Vec::new();
        // The effects bound explicitly here, as opposed to inherited from a
        // spread.
        let mut explicit: Vec<TraitId> = Vec::new();

        let t = self.tree();
        let ctx = t.ctx_body(body);

        // Either form may begin with a spread, which takes every binding from
        // another context and lets the ones that follow replace them.
        if let Some(base) = t.opt(ctx.spread) {
            let base_span = t.span(base);
            let b = self.check_expr(base, None);
            match self.resolve(&b.ty) {
                Ty::Ctx(id) => {
                    // The inherited binding keeps the *implementing type* the
                    // base recorded. Without it monomorphization has nothing
                    // to dispatch on, and every effect a spread supplies is
                    // unusable — the failure is `type-arguments-required` at
                    // the *use*, "`Clock` could not be resolved to a concrete
                    // type", far from the context that lost it. What pins that
                    // is the section "An effect INHERITED through a spread" in
                    // `cli/tests/conformance/lib/semantics/test/effects.buri`:
                    // every block there is a build-time claim as much as a
                    // runtime one, and both backends run them.
                    let base_bindings = self.c.tables.ctx_type(id).bindings.clone();
                    for (tr, impl_ty) in base_bindings {
                        let e = typed::Expr::new(
                            typed::ExprKind::CtxGet { base: Box::new(b.clone()), trait_id: tr },
                            impl_ty,
                            base_span,
                        );
                        bindings.push((tr, e));
                    }
                }
                Ty::Error => {}
                other => {
                    let shown = self.show_ty(&other);
                    self.templated("context-spread-operand", base_span).bind("type", shown);
                }
            }
        }

        for binding in t.bindings_at(ctx.bind_start, ctx.bind_len) {
            let effect_id = TypeId(binding.effect);
            let effect = t.ty(effect_id);
            let effect_span = effect.span();
            let binding_span = t.span_of(binding.span);
            let value_span = t.span(ExprId(binding.value));
            let flat::TypeView::Named { path, .. } = effect else {
                self.templated("context-binding-not-an-effect", effect_span);
                continue;
            };
            let Some(Sym::Trait(tid)) = self.c.resolve_path(self.module, path) else {
                let shown = t.type_head(effect_id).unwrap_or("?").to_string();
                self.templated("not-an-effect", effect_span).bind("name", shown);
                continue;
            };
            if !self.c.tables.trait_(tid).is_effect {
                let shown = self.c.tables.trait_(tid).name.clone();
                self.templated("trait-not-an-effect", effect_span).bind("name", shown);
                continue;
            }
            let value = self.check_expr(ExprId(binding.value), None);
            let vty = self.resolve(&value.ty);
            // Every binding's right side is a value whose type implements that
            // effect — ordinary nominal conformance.
            if !self.satisfies(&vty, tid) && !vty.is_error() {
                let shown = self.show_ty(&vty);
                let eff = self.c.tables.trait_(tid).name.clone();
                self.templated("missing-conformance", value_span)
                    .bind("type", shown)
                    .bind("trait", eff.clone())
                    .fix(format!(
                        "bind a value whose type has `impl {eff} for ...`; an effect is an \
                         ordinary interface, so a test double is a struct with those methods"
                    ));
            }
            // An explicit binding replaces a spread's rather than duplicating
            // it; two explicit bindings of one effect is an error.
            if explicit.contains(&tid) {
                let eff = self.c.tables.trait_(tid).name.clone();
                self.templated("duplicate-bound", binding_span).bind("effect", eff);
            }
            explicit.push(tid);
            if let Some(slot) = bindings.iter_mut().find(|(t, _)| *t == tid) {
                *slot = (tid, value);
            } else {
                bindings.push((tid, value));
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
        scrutinee: ExprId,
        arms: &[ArmData],
        span: Span,
        expected: Option<&Ty>,
    ) -> typed::Expr {
        let t = self.tree();
        let s = self.check_expr(scrutinee, None);
        let sty = self.resolve(&s.ty);
        let result = match expected {
            Some(ty) => ty.clone(),
            None => self.fresh(span),
        };
        let bool_ty = self.prim(Prim::Bool);
        let mut checked = Vec::new();
        for arm in arms {
            self.push_scope();
            self.pattern_names.clear();
            let pat = self.check_pattern(flat::PatId(arm.pattern), &sty);
            let guard = t.opt(arm.guard).map(|g| {
                let e = self.check_expr(g, Some(&bool_ty));
                let gspan = t.span(g);
                self.unify_at(gspan, &e.ty.clone(), &bool_ty, "a guard");
                e
            });
            let body = self.check_expr(ExprId(arm.body), Some(&result));
            let body_span = t.span(ExprId(arm.body));
            self.unify_at(body_span, &body.ty.clone(), &result, "the other arms");
            self.pop_scope();
            checked.push(typed::Arm { pattern: pat, guard, body, span: t.span_of(arm.span) });
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

fn collect_locals(e: &typed::Expr, out: &mut Vec<LocalId>) {
    typed::walk(e, &mut |x| {
        if let typed::ExprKind::Local(l) = &x.kind {
            out.push(*l);
        }
    });
}

/// Every `_` in `pat`, paired with the type it matches and the span to report
/// it at. The root is given `at`; everything below it reports at itself.
///
/// A `Bind` with no sub-pattern is a name, not a discard — `let _x = mayFail()`
/// is legal, and `unused-variable` is the rule that has an opinion about it.
fn wildcard_types(pat: &typed::Pattern, at: Span, out: &mut Vec<(Span, Ty)>) {
    match &pat.kind {
        typed::PatKind::Wild => out.push((at, pat.ty.clone())),
        typed::PatKind::Bind { sub: Some(sub), .. } => wildcard_types(sub, sub.span, out),
        typed::PatKind::Tuple(ps) | typed::PatKind::Or(ps) => {
            for p in ps {
                wildcard_types(p, p.span, out);
            }
        }
        typed::PatKind::Struct { fields, .. } | typed::PatKind::Variant { fields, .. } => {
            for f in fields {
                wildcard_types(&f.pattern, f.pattern.span, out);
            }
        }
        // The elements only. A `..` drops a *slice*, which is a list rather
        // than a `Result`, and no wildcard is written for it.
        typed::PatKind::Array { elems, .. } => {
            for p in elems {
                wildcard_types(p, p.span, out);
            }
        }
        _ => {}
    }
}

/// The fix the `result-discarded` page prints.
///
/// Borrowed rather than repeated, because a `Result` left in statement
/// position is the same mistake as one bound to `_`, and the two diagnostics
/// that shape earns — `expression-statement` and `statement-not-unit` — would
/// otherwise answer it with `let _ = ...;`, which is the error the reader hits
/// next. One rule, one sentence, and it lives on the page that explains it.
fn result_discard_fix() -> Option<&'static str> {
    crate::documentation::errors::page("result-discarded").and_then(|p| p.front.fix.as_deref())
}

#[cfg(test)]
mod tests {
    use crate::diagnostics::SourceMap;

    /// Every error one snippet reports, through the real front end.
    ///
    /// **`Role::Platform` rather than `Role::Entry`**, and it buys these tests
    /// two things at once. What they are about is `implementing_ty` — which
    /// type `Self` stands for when an effect method is reached *through a
    /// context value* — and the only way to write that call down is the method
    /// form, which is legal in the standard library and in an `impl` that
    /// supplies an effect, and nowhere else (`report_effect_method`). A
    /// platform module is one of those two, and it may build a context
    /// (SPEC 11.3), so the claim is written where it is still writable.
    ///
    /// The second thing is that a platform module may **declare an effect**,
    /// which is why the snippet mints its own `Accept` rather than borrowing
    /// one from `core/effect`. No standard-library effect spells `Self` in a
    /// callback any more — `Listen` used to, and its request handler moved
    /// into `core/net/server`'s own loop — so the rule outlived its last
    /// in-tree instance, and a test that depended on one would have died with
    /// it. The rule is about `Self`, not about servers.
    fn errors_of(src: &str) -> Vec<String> {
        let mut map = SourceMap::new();
        let analysis = crate::compiler::driver::analyze_snippet(
            &mut map,
            "self_through_a_context.buri",
            src,
            crate::compiler::modules::Role::Platform,
        );
        analysis
            .diagnostics
            .items
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect()
    }

    /// A `Listen` and a `Net`, each an ordinary struct, and a context binding
    /// both. `{handler}` is the `status` of the handler's `Response`, which is
    /// the one thing the tests below disagree about.
    ///
    /// The context is built in `main` because that is one of the three places
    /// authority may enter (`context-not-allowed`), and a snippet is a whole
    /// module rather than a body.
    fn snippet(handler: &str) -> String {
        format!(
            r#"
from "core/effect" import {{ Net, NetError, Request, Response }};

effect Accept {{
  fn accept(self, address: Str, onRequest: fn(Self, Request) => Response): Bool;
}}

struct Server {{ mark: Int }}

impl Accept for Server {{
  fn accept(self, address: Str, onRequest: fn(Self, Request) => Response): Bool {{
    true
  }}
}}

struct Caller {{}}

impl Net for Caller {{
  fn fetch(self, request: Request): Result<Response, NetError> {{
    .Err(.Refused)
  }}
}}

export fn main(): Result<(), Str> {{
  let ctx = context {{ Accept: Server {{ mark: 7 }}, Net: Caller {{}} }};
  match (ctx.accept("a", fn(acceptor, request) => Response {{
    status: {handler},
    headers: [],
    body: [],
  }})) {{
    true => .Ok(()),
    false => .Err("refused"),
  }}
}}
"#
        )
    }

    /// The handler's first parameter is the **implementation**, so a field of
    /// it is readable.
    ///
    /// A context has no fields at all, so this is the whole claim in one line:
    /// before `implementing_ty`, `Self` was the receiver and `acceptor.mark`
    /// reported `no-field` on a type with no name to print.
    #[test]
    fn a_self_parameter_through_a_context_is_the_implementing_type() {
        let errors = errors_of(&snippet("acceptor.mark"));
        assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
    }

    /// And it is **not** the context, which is the same claim from the other
    /// side.
    ///
    /// `fetch` is `Net`'s, the context binds `Net`, and `Server` — which is
    /// what implements `Listen` — does not. So a `Self` that was still the
    /// receiver would resolve this call and hand the handler a context at
    /// runtime; the implementation arrives instead, and the front end says so.
    #[test]
    fn a_self_parameter_through_a_context_is_not_the_context() {
        let errors = errors_of(&snippet(
            "match (acceptor.fetch(request)) { .Ok(r) => r.status, .Err(_) => 0 }",
        ));
        assert!(
            errors.iter().any(|m| m.contains("fetch")),
            "expected the handler's parameter to have no `fetch`, got {errors:?}"
        );
    }

    /// A written `Self` in the `impl` head's own signature and the one the
    /// call site substitutes are the same type — A1 fixed the first, and this
    /// is the pair agreeing.
    ///
    /// The `impl` above writes `fn(Self, Request) => Response` rather than
    /// `fn(Server, Request) => Response`, so the passing test above is already
    /// that agreement; this is the other spelling, which must compile too.
    #[test]
    fn the_impl_may_spell_the_handler_with_the_concrete_type_instead() {
        let src = snippet("acceptor.mark")
            .replace("onRequest: fn(Self, Request) => Response", "onRequest: fn(Server, Request) => Response");
        let errors = errors_of(&src);
        assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
    }
}
