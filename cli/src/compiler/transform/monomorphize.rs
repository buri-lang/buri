//! Monomorphization.
//!
//! A generic body is checked once, polymorphically; instantiating it is a
//! codegen concern (SPEC 13.5). This pass walks out from the entry point and
//! produces one concrete function per `(function, type arguments)` pair it
//! reaches, resolving every trait and effect call to a direct one on the way.
//!
//! Because Buri has no dynamic dispatch — no trait objects, no virtual calls —
//! the call graph of direct calls is fully known once this runs, which is what
//! makes the tail-call elimination in `tco.rs` exact.
//!
//! Reachability doubles as dead code elimination: an instance nothing calls is
//! never created, so the whole of `core/*` costs nothing in a program that
//! touches two functions of it.

use crate::compiler::semantics::resolve::Checked;
use crate::compiler::semantics::typed::{self, ExprKind, PatKind};
use crate::compiler::semantics::types::*;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use std::collections::HashMap;

/// One concrete function in the output. `typed::ExprKind::CallFn`'s `func` field
/// holds an index into `Program::funcs` after this pass, not a `FnId`.
pub struct Func {
    /// A stable, deterministic symbol, derived from the module path and name
    /// rather than from instantiation order.
    pub symbol: String,
    pub debug_name: String,
    pub params: Vec<LocalId>,
    pub locals: Vec<typed::Local>,
    pub body: Option<typed::Expr>,
    /// An operation the runtime supplies.
    pub intrinsic: Option<String>,
    /// The instantiated parameter and return types, which the backend needs
    /// for the numeric conversions and for `Bounded`.
    pub param_types: Vec<Ty>,
    pub ret: Ty,
    /// A descriptor the backend passes as an extra argument. Only the test
    /// runner's `report` needs one: it renders both values on a failure, and
    /// rendering is the runner's rather than the program's.
    pub desc: Option<usize>,
    pub span: Span,
}

pub struct TestEntry {
    pub name: String,
    pub func: usize,
    pub module: String,
    pub span: Span,
}

/// A runtime type descriptor, for the structural operations `derive` generates.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Desc {
    /// Compared and rendered by value.
    Prim(Prim),
    Unit,
    /// Rendered as `Name { field: .., .. }` or `Name(..)`.
    Struct { name: String, record: bool, fields: Vec<String>, types: Vec<usize> },
    Enum { name: String, variants: Vec<DescVariant>, payloadless: bool },
    Array(usize),
    Tuple(Vec<usize>),
    /// `Option<T>`, whose value is the payload itself.
    Option(usize),
    /// A type with no structural rendering.
    Opaque(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DescVariant {
    pub name: String,
    pub record: bool,
    pub fields: Vec<String>,
    pub types: Vec<usize>,
}

pub struct Program {
    pub funcs: Vec<Func>,
    pub entry: Option<usize>,
    pub tests: Vec<TestEntry>,
    pub descriptors: Vec<Desc>,
    /// Which descriptor describes a type, so the backend can compile a
    /// structural operation *at* a type instead of calling a runtime walker
    /// that rediscovers the shape on every element.
    pub desc_index: HashMap<Ty, usize>,
    /// Number of effect slots each context type carries, in binding order.
    pub ctx_layouts: HashMap<CtxTypeId, Vec<TraitId>>,
}

/// What a queued instance is.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Key {
    Fn(FnId, Vec<Ty>),
    /// A named `context` declaration's constructor.
    CtxCtor(ContextDeclId),
    /// A test body.
    Test(usize),
}

pub struct Monomorphizer<'a> {
    checked: &'a Checked,
    diags: &'a mut Diagnostics,
    funcs: Vec<Func>,
    index: HashMap<Key, usize>,
    queue: Vec<(Key, usize)>,
    descriptors: Vec<Desc>,
    desc_index: HashMap<Ty, usize>,
    ctx_layouts: HashMap<CtxTypeId, Vec<TraitId>>,
    module_paths: Vec<String>,
}

pub fn run(
    checked: &Checked,
    module_paths: Vec<String>,
    diags: &mut Diagnostics,
    roots: Roots,
) -> Program {
    let mut m = Monomorphizer {
        checked,
        diags,
        funcs: Vec::new(),
        index: HashMap::new(),
        queue: Vec::new(),
        descriptors: Vec::new(),
        desc_index: HashMap::new(),
        ctx_layouts: HashMap::new(),
        module_paths,
    };

    let entry = match roots {
        Roots::Main(f) => Some(m.request(Key::Fn(f, Vec::new()))),
        Roots::Tests => None,
    };
    let mut tests = Vec::new();
    if matches!(roots, Roots::Tests) {
        for (i, case) in m.checked.tests.iter().enumerate() {
            let idx = m.request(Key::Test(i));
            tests.push(TestEntry {
                name: case.name.clone(),
                func: idx,
                module: m.module_paths.get(case.module.index()).cloned().unwrap_or_default(),
                span: case.span,
            });
        }
    }

    // Instantiation order follows the worklist, which follows source order, so
    // two builds of the same commit produce identical output.
    while let Some((key, slot)) = m.queue.pop() {
        m.build(key, slot);
    }

    Program {
        funcs: m.funcs,
        entry,
        tests,
        descriptors: m.descriptors,
        desc_index: m.desc_index,
        ctx_layouts: m.ctx_layouts,
    }
}

#[derive(Clone, Copy)]
pub enum Roots {
    Main(FnId),
    Tests,
}

impl<'a> Monomorphizer<'a> {
    fn tables(&self) -> &Tables {
        &self.checked.tables
    }

    fn request(&mut self, key: Key) -> usize {
        if let Some(i) = self.index.get(&key) {
            return *i;
        }
        let slot = self.funcs.len();
        let (symbol, debug_name, span) = self.name_of(&key);
        self.funcs.push(Func {
            symbol,
            debug_name,
            params: Vec::new(),
            locals: Vec::new(),
            body: None,
            intrinsic: None,
            param_types: Vec::new(),
            ret: Ty::Unit,
            desc: None,
            span,
        });
        self.index.insert(key.clone(), slot);
        self.queue.push((key, slot));
        slot
    }

    fn name_of(&self, key: &Key) -> (String, String, Span) {
        match key {
            Key::Fn(f, targs) => {
                let info = self.tables().fun(*f);
                let module = self
                    .module_paths
                    .get(info.module.index())
                    .cloned()
                    .unwrap_or_else(|| "core".into());
                let owner = info
                    .self_ty
                    .map(|c| format!("{}.", self.tables().tycon(c).name))
                    .unwrap_or_default();
                let debug = format!("{module}:{owner}{}", info.name);
                let mut symbol = format!(
                    "{}${owner}{}",
                    module.replace(['/', '.'], "_").replace("//", ""),
                    info.name
                );
                if !targs.is_empty() {
                    symbol.push('$');
                    symbol.push_str(&short_hash(&format!("{targs:?}")));
                }
                (sanitize(&symbol), debug, info.span)
            }
            Key::CtxCtor(c) => {
                let info = self.tables().ctx_decl(*c);
                (sanitize(&format!("ctx${}", info.name)), info.name.clone(), info.span)
            }
            Key::Test(i) => {
                let case = &self.checked.tests[*i];
                (format!("test${i}"), case.name.clone(), case.span)
            }
        }
    }

    fn build(&mut self, key: Key, slot: usize) {
        match key {
            Key::Fn(f, targs) => self.build_fn(f, targs, slot),
            Key::CtxCtor(c) => {
                let Some(ctor) = self.tables().ctx_decl(c).ctor else { return };
                let Some(body) = self.checked.bodies.get(&ctor) else { return };
                let mut b = body.clone();
                b.expr = self.rewrite(b.expr, &[]);
                self.funcs[slot].params = b.params;
                self.funcs[slot].locals = b.locals;
                self.funcs[slot].body = Some(b.expr);
            }
            Key::Test(i) => {
                let fid = self.checked.tests[i].func;
                let Some(body) = self.checked.bodies.get(&fid) else { return };
                let mut b = body.clone();
                b.expr = self.rewrite(b.expr, &[]);
                self.funcs[slot].params = b.params;
                self.funcs[slot].locals = b.locals;
                self.funcs[slot].body = Some(b.expr);
            }
        }
    }

    fn build_fn(&mut self, f: FnId, targs: Vec<Ty>, slot: usize) {
        let info = self.tables().fun(f).clone();
        if info.intrinsic {
            let key = self.intrinsic_key(&info, &targs);
            self.funcs[slot].intrinsic = Some(key.clone());
            self.funcs[slot].locals = info
                .params
                .iter()
                .map(|p| typed::Local {
                    name: p.name.clone(),
                    ty: substitute(&p.ty, &targs, None),
                    span: p.span,
                })
                .collect();
            self.funcs[slot].params =
                (0..info.params.len()).map(|i| LocalId(i as u32)).collect();
            self.funcs[slot].param_types =
                info.params.iter().map(|p| substitute(&p.ty, &targs, None)).collect();
            self.funcs[slot].ret = substitute(&info.ret, &targs, None);
            if key == "testing_assert.report" || key == "testing_assert.failExpected" {
                if let Some(t) = self.funcs[slot].param_types.get(2).cloned() {
                    self.funcs[slot].desc = Some(self.descriptor(&t));
                } else if let Some(t) = self.funcs[slot].param_types.get(1).cloned() {
                    self.funcs[slot].desc = Some(self.descriptor(&t));
                }
            }
            // `json.decode` is the one operation whose subject is in neither a
            // parameter nor the receiver: it is asked for a `T` and handed a
            // `Json`. `T` is the first type argument, and the descriptor of it
            // is the whole of what the runtime needs to read one.
            if key == "json.decode" {
                if let Some(t) = targs.first().cloned() {
                    self.funcs[slot].desc = Some(self.descriptor(&t));
                }
            }
            return;
        }
        let Some(body) = self.checked.bodies.get(&f) else {
            // A trait method with no body and no impl: already diagnosed.
            return;
        };
        let mut b = body.clone();
        for l in &mut b.locals {
            l.ty = substitute(&l.ty, &targs, None);
        }
        b.expr = self.rewrite(b.expr, &targs);
        self.funcs[slot].params = b.params;
        self.funcs[slot].locals = b.locals;
        self.funcs[slot].body = Some(b.expr);
    }

    /// The name the backend looks up for an operation the runtime supplies.
    fn intrinsic_key(&self, info: &FnInfo, targs: &[Ty]) -> String {
        let module = self
            .module_paths
            .get(info.module.index())
            .cloned()
            .unwrap_or_else(|| "core/num".into());
        let short = module.strip_prefix("core/").unwrap_or(&module).replace('/', "_");
        match info.self_ty {
            // `core/str` exists for `Str`, so `str.Str.len` says it twice.
            // `core/num` is the defining module of a dozen types, so there the
            // type is what tells two conversions apart.
            Some(con)
                if is_prim(self.tables(), con)
                    && short != "num"
                    && info.impl_of.is_none() =>
            {
                format!("{short}.{}", info.name)
            }
            Some(con) if info.impl_of.is_some() || is_prim(self.tables(), con) => {
                format!("{short}.{}.{}", self.tables().tycon(con).name, info.name)
            }
            Some(con) if self.tables().tycon(con).module.0 != u32::MAX => {
                format!("{short}.{}.{}", self.tables().tycon(con).name, info.name)
            }
            _ => {
                let _ = targs;
                format!("{short}.{}", info.name)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Rewriting
    // -----------------------------------------------------------------------

    fn rewrite(&mut self, mut e: typed::Expr, targs: &[Ty]) -> typed::Expr {
        e.ty = substitute(&e.ty, targs, None);
        e.kind = match e.kind {
            ExprKind::CallFn { func, targs: call_targs, args } => {
                let resolved: Vec<Ty> =
                    call_targs.iter().map(|t| substitute(t, targs, None)).collect();
                let args = self.rewrite_all(args, targs);
                let slot = self.request(Key::Fn(func, resolved));
                ExprKind::CallFn { func: FnId(slot as u32), targs: Vec::new(), args }
            }
            ExprKind::CallTrait { trait_id, method, recv, targs: mt, args } => {
                let recv = substitute(&recv, targs, None);
                let mt: Vec<Ty> = mt.iter().map(|t| substitute(t, targs, None)).collect();
                let args = self.rewrite_all(args, targs);
                self.resolve_trait_call(trait_id, method, recv, mt, args, e.span, &e.ty)
            }
            ExprKind::FnRef(f, ft) => {
                let resolved: Vec<Ty> = ft.iter().map(|t| substitute(t, targs, None)).collect();
                let slot = self.request(Key::Fn(f, resolved));
                ExprKind::FnRef(FnId(slot as u32), Vec::new())
            }
            ExprKind::CtxCall { decl } => {
                let slot = self.request(Key::CtxCtor(decl));
                ExprKind::CallFn { func: FnId(slot as u32), targs: Vec::new(), args: Vec::new() }
            }
            ExprKind::Const(cid) => {
                // A constant is inlined: it has no address and nothing can
                // observe whether it was shared.
                match self.checked.consts.get(&cid) {
                    Some(v) => {
                        let inlined = self.rewrite(v.clone(), &[]);
                        return inlined;
                    }
                    None => ExprKind::Error,
                }
            }
            ExprKind::CtxLit { bindings } => {
                if let Ty::Ctx(id) = &e.ty {
                    let layout: Vec<TraitId> = bindings.iter().map(|(t, _)| *t).collect();
                    self.ctx_layouts.insert(*id, layout);
                }
                ExprKind::CtxLit {
                    bindings: bindings
                        .into_iter()
                        .map(|(t, v)| (t, self.rewrite(v, targs)))
                        .collect(),
                }
            }
            ExprKind::CtxGet { base, trait_id } => {
                let base = Box::new(self.rewrite(*base, targs));
                ExprKind::CtxGet { base, trait_id }
            }
            ExprKind::CallValue { callee, args } => ExprKind::CallValue {
                callee: Box::new(self.rewrite(*callee, targs)),
                args: self.rewrite_all(args, targs),
            },
            ExprKind::StructLit { con, targs: st, fields } => ExprKind::StructLit {
                con,
                targs: st.iter().map(|t| substitute(t, targs, None)).collect(),
                fields: self.rewrite_all(fields, targs),
            },
            ExprKind::StructUpdate { con, base, updates } => ExprKind::StructUpdate {
                con,
                base: Box::new(self.rewrite(*base, targs)),
                updates: updates
                    .into_iter()
                    .map(|(i, v)| (i, self.rewrite(v, targs)))
                    .collect(),
            },
            ExprKind::EnumLit { con, targs: et, variant, args } => ExprKind::EnumLit {
                con,
                targs: et.iter().map(|t| substitute(t, targs, None)).collect(),
                variant,
                args: self.rewrite_all(args, targs),
            },
            ExprKind::Tuple(xs) => ExprKind::Tuple(self.rewrite_all(xs, targs)),
            ExprKind::Array(xs) => ExprKind::Array(self.rewrite_all(xs, targs)),
            ExprKind::Field { base, index } => {
                ExprKind::Field { base: Box::new(self.rewrite(*base, targs)), index }
            }
            ExprKind::TupleIndex { base, index } => {
                ExprKind::TupleIndex { base: Box::new(self.rewrite(*base, targs)), index }
            }
            ExprKind::Index { base, index, elem } => ExprKind::Index {
                base: Box::new(self.rewrite(*base, targs)),
                index: Box::new(self.rewrite(*index, targs)),
                elem: substitute(&elem, targs, None),
            },
            ExprKind::Block { stmts, tail } => ExprKind::Block {
                stmts: stmts
                    .into_iter()
                    .map(|s| match s {
                        typed::Stmt::Let { pattern, value, span } => typed::Stmt::Let {
                            pattern: self.rewrite_pattern(pattern, targs),
                            value: self.rewrite(value, targs),
                            span,
                        },
                        typed::Stmt::Expr(x) => typed::Stmt::Expr(self.rewrite(x, targs)),
                    })
                    .collect(),
                tail: tail.map(|t| Box::new(self.rewrite(*t, targs))),
            },
            ExprKind::If { cond, then, else_ } => ExprKind::If {
                cond: Box::new(self.rewrite(*cond, targs)),
                then: Box::new(self.rewrite(*then, targs)),
                else_: Box::new(self.rewrite(*else_, targs)),
            },
            ExprKind::Match { scrutinee, arms } => ExprKind::Match {
                scrutinee: Box::new(self.rewrite(*scrutinee, targs)),
                arms: arms
                    .into_iter()
                    .map(|a| typed::Arm {
                        pattern: self.rewrite_pattern(a.pattern, targs),
                        guard: a.guard.map(|g| self.rewrite(g, targs)),
                        body: self.rewrite(a.body, targs),
                        span: a.span,
                    })
                    .collect(),
            },
            ExprKind::Lambda { params, body, captures } => ExprKind::Lambda {
                params,
                body: Box::new(self.rewrite(*body, targs)),
                captures,
            },
            ExprKind::And { lhs, rhs } => ExprKind::And {
                lhs: Box::new(self.rewrite(*lhs, targs)),
                rhs: Box::new(self.rewrite(*rhs, targs)),
            },
            ExprKind::Or { lhs, rhs } => ExprKind::Or {
                lhs: Box::new(self.rewrite(*lhs, targs)),
                rhs: Box::new(self.rewrite(*rhs, targs)),
            },
            ExprKind::Coalesce { lhs, rhs, kind } => ExprKind::Coalesce {
                lhs: Box::new(self.rewrite(*lhs, targs)),
                rhs: Box::new(self.rewrite(*rhs, targs)),
                kind,
            },
            ExprKind::Try { base, kind } => {
                ExprKind::Try { base: Box::new(self.rewrite(*base, targs)), kind }
            }
            ExprKind::Prim { op, prim, args } => {
                ExprKind::Prim { op, prim, args: self.rewrite_all(args, targs) }
            }
            ExprKind::StructuralEq { negate, args } => {
                let args = self.rewrite_all(args, targs);
                // The backend compiles this at the type rather than calling a
                // walker that rediscovers the shape per element, and the
                // descriptor is where the shape is written down. Nothing else
                // asks for one at this type, so ask here.
                if let Some(a) = args.first() {
                    let t = a.ty.clone();
                    self.descriptor(&t);
                }
                ExprKind::StructuralEq { negate, args }
            }
            ExprKind::StructuralCmp { op, args } => {
                ExprKind::StructuralCmp { op, args: self.rewrite_all(args, targs) }
            }
            ExprKind::Template { parts } => ExprKind::Template {
                parts: parts
                    .into_iter()
                    .map(|p| typed::TemplatePart {
                        text: p.text,
                        hole: p.hole.map(|h| self.rewrite(h, targs)),
                    })
                    .collect(),
            },
            ExprKind::Intrinsic { name, targs: it, args } => ExprKind::Intrinsic {
                name,
                targs: it.iter().map(|t| substitute(t, targs, None)).collect(),
                args: self.rewrite_all(args, targs),
            },
            other => other,
        };
        e
    }

    fn rewrite_all(&mut self, xs: Vec<typed::Expr>, targs: &[Ty]) -> Vec<typed::Expr> {
        xs.into_iter().map(|x| self.rewrite(x, targs)).collect()
    }

    fn rewrite_pattern(&mut self, mut p: typed::Pattern, targs: &[Ty]) -> typed::Pattern {
        p.ty = substitute(&p.ty, targs, None);
        p.kind = match p.kind {
            PatKind::Bind { local, sub } => PatKind::Bind {
                local,
                sub: sub.map(|s| Box::new(self.rewrite_pattern(*s, targs))),
            },
            PatKind::Tuple(ps) => {
                PatKind::Tuple(ps.into_iter().map(|x| self.rewrite_pattern(x, targs)).collect())
            }
            PatKind::Struct { con, fields } => PatKind::Struct {
                con,
                fields: fields
                    .into_iter()
                    .map(|f| typed::FieldPat {
                        index: f.index,
                        pattern: self.rewrite_pattern(f.pattern, targs),
                    })
                    .collect(),
            },
            PatKind::Variant { con, variant, fields } => PatKind::Variant {
                con,
                variant,
                fields: fields
                    .into_iter()
                    .map(|f| typed::FieldPat {
                        index: f.index,
                        pattern: self.rewrite_pattern(f.pattern, targs),
                    })
                    .collect(),
            },
            PatKind::Array { elems, rest } => PatKind::Array {
                elems: elems.into_iter().map(|x| self.rewrite_pattern(x, targs)).collect(),
                rest,
            },
            PatKind::Or(ps) => {
                PatKind::Or(ps.into_iter().map(|x| self.rewrite_pattern(x, targs)).collect())
            }
            other => other,
        };
        p
    }

    /// Turns a trait or effect call into a direct one. This is where the
    /// effect system stops costing anything: a context is a record of
    /// implementations, and by here the record's shape is known.
    fn resolve_trait_call(
        &mut self,
        trait_id: TraitId,
        method: usize,
        recv: Ty,
        method_targs: Vec<Ty>,
        mut args: Vec<typed::Expr>,
        span: Span,
        result_ty: &Ty,
    ) -> ExprKind {
        // A context value: read the implementation out of it, then dispatch on
        // that implementation's own type.
        if let Ty::Ctx(id) = &recv {
            let Some(impl_ty) = self.tables().ctx_type(*id).get(trait_id).cloned() else {
                self.diags.push(
                    Diagnostic::error(span, "this context does not bind that effect").with_fix(
                        "bind it where the context is built, or bound this function with the \
                         effect it needs",
                    ),
                );
                return ExprKind::Error;
            };
            if let Some(first) = args.first_mut() {
                let base = std::mem::replace(
                    first,
                    typed::Expr::new(ExprKind::Error, Ty::Error, span),
                );
                *first = typed::Expr::new(
                    ExprKind::CtxGet { base: Box::new(base), trait_id },
                    impl_ty.clone(),
                    span,
                );
            }
            return self.resolve_trait_call(trait_id, method, impl_ty, method_targs, args, span, result_ty);
        }

        // The structural traits are defined on `[T]`, tuples and unit by
        // their components (SPEC 5.11), with no `impl` to find.
        if matches!(recv, Ty::Array(_) | Ty::Tuple(_) | Ty::Unit) {
            return self.structural_call(trait_id, method, &recv, args, span);
        }

        let Some(con) = recv.head() else {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "`{}` could not be resolved to a concrete type",
                        self.tables().trait_(trait_id).name
                    ),
                )
                .with_fix("annotate the call with a turbofish, as in `f::<Str>(x)`"),
            );
            return ExprKind::Error;
        };

        let Some(imp) = self.tables().impls.get(&(trait_id, con)).cloned() else {
            let t = self.tables().trait_(trait_id).name.clone();
            let c = self.tables().tycon(con).name.clone();
            self.diags.push(
                Diagnostic::error(span, format!("`{c}` does not implement `{t}`"))
                    .with_note("conformance is nominal: a type satisfies a trait only where a declaration says so")
                    .with_fix(crate::compiler::semantics::types::conformance_fix(&t, &c)),
            );
            return ExprKind::Error;
        };

        // `derive` generates the trait's methods structurally: struct fields
        // in declaration order, enum variants in declaration order, recursing
        // into field types. It is a fold over one type definition.
        if imp.derived {
            return self.structural_call(trait_id, method, &recv, args, span);
        }

        let Some(&fid) = imp.methods.get(method) else { return ExprKind::Error };
        if fid.0 == u32::MAX {
            return ExprKind::Error;
        }
        // The impl's own generics are bound by the receiver's type arguments.
        let mut instance_targs: Vec<Ty> = match &recv {
            Ty::Con(_, a) => a.clone(),
            _ => Vec::new(),
        };
        let declared = self.tables().fun(fid).generics.len();
        instance_targs.extend(method_targs);
        instance_targs.truncate(declared.max(instance_targs.len()));
        while instance_targs.len() < declared {
            instance_targs.push(Ty::Unit);
        }
        instance_targs.truncate(declared);

        let slot = self.request(Key::Fn(fid, instance_targs));
        ExprKind::CallFn { func: FnId(slot as u32), targs: Vec::new(), args }
    }

    /// The structural implementations `derive` stands for. They are emitted as
    /// runtime operations over a type descriptor rather than as generated
    /// source, which keeps the output small: one `$eq` in the runtime rather
    /// than one per type.
    fn structural_call(
        &mut self,
        trait_id: TraitId,
        method: usize,
        recv: &Ty,
        args: Vec<typed::Expr>,
        span: Span,
    ) -> ExprKind {
        let name = self.tables().trait_(trait_id).name.clone();
        let desc = self.descriptor(recv);
        let desc_arg =
            typed::Expr::new(ExprKind::Int(desc as u128, false), Ty::Error, span);
        let mut all = args;
        all.push(desc_arg);
        match (name.as_str(), method) {
            ("Eq", _) => ExprKind::Intrinsic { name: "structuralEq".into(), targs: Vec::new(), args: all },
            ("Ord", _) => {
                ExprKind::Intrinsic { name: "structuralCompare".into(), targs: Vec::new(), args: all }
            }
            ("Show", _) => {
                // `show` takes a context it does not use here — rendering is
                // the runtime's. Drop it so the intrinsic sees value and
                // descriptor only.
                let mut trimmed: Vec<typed::Expr> = Vec::new();
                trimmed.push(all[0].clone());
                trimmed.push(all[all.len() - 1].clone());
                ExprKind::Intrinsic { name: "structuralShow".into(), targs: Vec::new(), args: trimmed }
            }
            ("ToJson", _) => {
                // Like `show`, `toJson` takes a context it does not use here:
                // building the tree is the runtime's. Drop it so the intrinsic
                // sees value and descriptor only.
                let trimmed = vec![all[0].clone(), all[all.len() - 1].clone()];
                ExprKind::Intrinsic {
                    name: "structuralToJson".into(),
                    targs: Vec::new(),
                    args: trimmed,
                }
            }
            ("Hash", _) => {
                ExprKind::Intrinsic { name: "structuralHash".into(), targs: Vec::new(), args: all }
            }
            // The operator traits derived on a newtype: apply the operation to
            // the wrapped value and rewrap.
            (op @ ("Add" | "Sub" | "Mul" | "Div" | "Rem" | "Neg"), _) => {
                self.derived_operator(op, recv, all, span)
            }
            _ => {
                self.diags.push(
                    Diagnostic::error(span, format!("`{name}` cannot be derived structurally"))
                        .with_fix("write the `impl` by hand"),
                );
                ExprKind::Error
            }
        }
    }

    /// `derive Add for Meters` provides `Meters + Meters` and nothing else, so
    /// the unit safety the newtype exists for survives contact with
    /// arithmetic.
    fn derived_operator(
        &mut self,
        op: &str,
        recv: &Ty,
        mut args: Vec<typed::Expr>,
        span: Span,
    ) -> ExprKind {
        args.pop(); // the descriptor, which an operator does not need
        let Some(con) = recv.head() else { return ExprKind::Error };
        let tycon = self.tables().tycon(con).clone();
        let fields = tycon.fields().to_vec();
        if fields.len() != 1 {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`{}` derives `{op}` only for a single-field struct", tycon.name),
                )
                .with_fix("write the `impl` by hand, or wrap exactly one value")
                .with_note("an arithmetic newtype wraps exactly one value"),
            );
            return ExprKind::Error;
        }
        let targs = match recv {
            Ty::Con(_, a) => a.clone(),
            _ => Vec::new(),
        };
        let inner = substitute(&fields[0].ty, &targs, None);
        let prim = self.tables().as_prim(&inner);
        let prim_op = match op {
            "Add" => typed::PrimOp::Add,
            "Sub" => typed::PrimOp::Sub,
            "Mul" => typed::PrimOp::Mul,
            "Div" => typed::PrimOp::Div,
            "Rem" => typed::PrimOp::Rem,
            _ => typed::PrimOp::Neg,
        };
        let unwrapped: Vec<typed::Expr> = args
            .into_iter()
            .map(|a| {
                typed::Expr::new(
                    ExprKind::Field { base: Box::new(a), index: 0 },
                    inner.clone(),
                    span,
                )
            })
            .collect();
        let computed =
            typed::Expr::new(ExprKind::Prim { op: prim_op, prim, args: unwrapped }, inner, span);
        ExprKind::StructLit { con, targs, fields: vec![computed] }
    }

    // -----------------------------------------------------------------------
    // Descriptors
    // -----------------------------------------------------------------------

    /// Interns a runtime type descriptor. Field names and variant names are
    /// what `show` needs; `eq` and `compare` need the shape.
    fn descriptor(&mut self, ty: &Ty) -> usize {
        if let Some(i) = self.desc_index.get(ty) {
            return *i;
        }
        // Reserve the slot first, so a recursive type terminates.
        let slot = self.descriptors.len();
        self.descriptors.push(Desc::Opaque(String::new()));
        self.desc_index.insert(ty.clone(), slot);

        let desc = match ty {
            Ty::Unit => Desc::Unit,
            Ty::Array(e) => Desc::Array(self.descriptor(e)),
            Ty::Tuple(es) => {
                Desc::Tuple(es.iter().map(|e| self.descriptor(e)).collect::<Vec<_>>())
            }
            Ty::Con(con, args) => {
                let tycon = self.tables().tycon(*con).clone();
                match &tycon.def {
                    TyDef::Prim(p) => Desc::Prim(*p),
                    TyDef::Struct { fields, record } => {
                        let types: Vec<usize> = fields
                            .iter()
                            .map(|f| {
                                let t = substitute(&f.ty, args, None);
                                self.descriptor(&t)
                            })
                            .collect();
                        Desc::Struct {
                            name: tycon.name.clone(),
                            record: *record,
                            fields: fields.iter().map(|f| f.name.clone()).collect(),
                            types,
                        }
                    }
                    // `Option` has no tag to read: `None` is `undefined` and
                    // `Some(x)` is `x`, so its descriptor says only what the
                    // payload is.
                    TyDef::Enum { .. } if self.tables().is_option(*con) => {
                        let payload = substitute(
                            args.first().unwrap_or(&Ty::Error),
                            args,
                            None,
                        );
                        let inner = self.descriptor(&payload);
                        Desc::Option(inner)
                    }
                    TyDef::Enum { variants } => {
                        let payloadless = variants.iter().all(|v| v.fields.is_empty());
                        let vs: Vec<DescVariant> = variants
                            .iter()
                            .map(|v| {
                                let types: Vec<usize> = v
                                    .fields
                                    .iter()
                                    .map(|f| {
                                        let t = substitute(&f.ty, args, None);
                                        self.descriptor(&t)
                                    })
                                    .collect();
                                DescVariant {
                                    name: v.name.clone(),
                                    record: v.record,
                                    fields: v.fields.iter().map(|f| f.name.clone()).collect(),
                                    types,
                                }
                            })
                            .collect();
                        Desc::Enum { name: tycon.name.clone(), variants: vs, payloadless }
                    }
                }
            }
            _ => Desc::Opaque("a value".into()),
        };
        self.descriptors[slot] = desc;
        slot
    }
}

fn is_prim(tables: &Tables, con: TyConId) -> bool {
    matches!(tables.tycon(con).def, TyDef::Prim(_))
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '$' { c } else { '_' }).collect()
}

/// A short, deterministic tag distinguishing two instantiations of one
/// function.
///
/// It is taken over the `Debug` form of the type arguments, which contains
/// `TyConId`s — indices into this compilation's type table. Two builds of the
/// same sources agree, because the table is built the same way both times; two
/// builds under *different toolchains* need not, and `golden_javascript` re-records
/// when that happens.
///
/// Hashing a rendering instead would be tidier, and does not work: a context
/// type is generated and has no name (SPEC 11.3), so `types::show` prints every
/// one of them as `a context`. Two generics instantiated over different
/// contexts would land on the same symbol and one body would silently replace
/// the other — a miscompile, and a worse thing than a symbol that moves. Nor
/// would rendering a context by the effects it binds help: two contexts binding
/// the same effects to different implementations are still different types,
/// which is what `Ty::Ctx(x) == Ty::Ctx(y)` means. The index is the identity.
///
/// `golden_javascript::generics_over_different_contexts_do_not_share_a_symbol` is the
/// test that says so.
fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let mut out = String::new();
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789";
    for _ in 0..6 {
        out.push(alphabet[(h % 36) as usize] as char);
        h /= 36;
    }
    out
}
