//! Type inference and the checking of function bodies.
//!
//! Inference is local to one body, because top-level signatures are mandatory
//! (SPEC 9). Name resolution and inference interleave in a single traversal —
//! the one place Buri gives something up — because resolving `x.f()` needs the
//! receiver's type. What keeps it a traversal rather than a fixpoint is that
//! method resolution needs only the receiver's *head type constructor*, that
//! type information flows outside-in and left-to-right, and that there is no
//! overloading (guides/compile-speed.md).

use crate::compiler::modules::Role;
use crate::compiler::semantics::resolve::{Bodies, Checker};
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::*;
use crate::diagnostics::{Diagnostic, Diagnostics, Invariant as _, Span};
use crate::parsing::flat;
use crate::parsing::tree;
use crate::hash::Map as HashMap;

pub fn check_all(c: &mut Checker) {
    // What a base checked is already in the tables this checker started from.
    let (consts, ctx_decls) = c.base_counts();
    // Constants first: a module-level `let` may be referenced from any body.
    for i in consts..c.tables.consts.len() {
        check_const(c, ConstId(i as u32));
    }
    for i in ctx_decls..c.tables.ctx_decls.len() {
        check_context_decl(c, ContextDeclId(i as u32));
    }
    // Function bodies check independently and in any order
    // (guides/compile-speed.md), which is what lets a scoped analysis check
    // some of them and not others.
    for i in 0..c.tables.fns.len() {
        let fid = FnId(i as u32);
        if c.settled_by_base(fid) == Some(true) {
            continue;
        }
        if wanted(c, fid) {
            check_fn(c, fid);
        }
    }
    check_tests(c);
    check_bodies_the_extractor_folds(c);
}

/// Whether this analysis was asked for this function's body.
fn wanted(c: &Checker, fid: FnId) -> bool {
    match &c.wanted {
        Bodies::All => true,
        Bodies::In(_) => c.file_of(c.tables.fn_info(fid).ast).is_some_and(|f| c.wants_file(f)),
    }
}

/// The bodies a scoped analysis did not ask for but cannot do without: the
/// pure functions static style extraction inlines.
///
/// `styles::run` folds a `Style` by inlining the pure calls under it, so a
/// `let cardStyle: Style = .Group([..., .BorderColor(Token.Edge.color())])`
/// needs `Token::color`'s body — which is in another file. Without it the fold
/// gives up, the literal degrades to the runtime tier, and an `On` or an `At`
/// beneath it is *rejected* — so the open file would be checked differently
/// from the way a whole-closure run checks it. That is the one cross-body
/// dependency in the front end, and this is where it is paid: the transitive
/// pure callees of what the extractor will actually walk, and nothing else.
///
/// Nothing here is reported. These bodies belong to files this analysis was
/// not asked about, and a diagnostic in one of them is the other file's to
/// publish.
fn check_bodies_the_extractor_folds(c: &mut Checker) {
    let Bodies::In(files) = c.wanted.clone() else { return };
    // No `ui/style` in the closure is no extraction, and then no fold.
    if c.loaded.find("ui/style").is_none() {
        return;
    }
    let mut queue = Vec::new();
    for (id, body) in &c.bodies {
        if files.contains(&c.tables.fn_info(id).span.file) {
            callees_of(&body.expr, &mut queue);
        }
    }
    for (id, expr) in &c.const_values {
        if files.contains(&c.tables.const_(id).span.file) {
            callees_of(expr, &mut queue);
        }
    }

    let mut aside = Diagnostics::new();
    std::mem::swap(c.diags, &mut aside);
    let mut seen: std::collections::HashSet<FnId> = std::collections::HashSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id) || c.bodies.contains_key(&id) {
            continue;
        }
        if !crate::compiler::semantics::consteval::is_inlinable(&c.tables, id) {
            continue;
        }
        check_fn(c, id);
        if let Some(body) = c.bodies.get(&id) {
            callees_of(&body.expr, &mut queue);
        }
    }
    std::mem::swap(c.diags, &mut aside);
}

/// Every function named by a direct call or used as a value, anywhere under
/// this expression.
fn callees_of(e: &typed::Expr, out: &mut Vec<FnId>) {
    typed::walk(e, &mut |x| {
        let callee = match &x.kind {
            typed::ExprKind::CallFn { func, .. } | typed::ExprKind::FnRef(func) => func,
            _ => return,
        };
        if let Some(id) = callee.decl() {
            out.push(id);
        }
    });
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
                tree::Item::Impl(d) => c.module(module).ast.tree.list(d.methods).get(sub as usize),
                _ => None,
            }
        }
    }
}

fn check_fn(c: &mut Checker, fid: FnId) {
    let info = c.tables.fn_info(fid);
    let Some(decl) = body_ast(c, info.ast) else { return };
    let Some(body) = decl.body else { return };

    // A method supplied to an `effect` impl is where that effect's operation is
    // implemented, so it is the one body outside the standard library that may
    // still call an effect method on a value (`report_effect_method`).
    let in_effect_impl =
        info.impl_of.is_some_and(|(tid, _)| c.tables.trait_(tid).is_effect);
    // An entry builds its own context, and there may be several: a page's
    // `main` and a worker's `fetch` out of one `main.buri`. The table the
    // resolver filled says which exported functions those are.
    let in_main = c.module(info.module).role == Role::Entry
        && info.exported
        && c.entry_points.contains(&info.name)
        && c.entries.get(&info.name) == Some(&fid);
    // The parts the body is checked against, copied once: the checker is
    // borrowed mutably from here on, and the declaration lives in its tables.
    // The parameters are read one at a time below instead, so that each name
    // is copied once, into its local.
    let (module, self_ty, generics, expected, arity) =
        (info.module, info.self_ty, info.generics.clone(), info.ret, info.params.len());

    let mut inf = Infer::new(c, module, generics, expected);
    inf.self_con = self_ty;
    inf.in_effect_impl = in_effect_impl;
    inf.in_main = in_main;
    inf.push_scope();
    inf.params.reserve_exact(arity);
    for i in 0..arity {
        let Some(p) = inf.c.tables.fn_info(fid).params.get(i) else { break };
        let (name, ty, span, role) = (p.name.clone(), p.ty, p.span, p.role);
        let local = inf.bind_new(name, ty, span);
        inf.params.push(local);
        // The capture rule is scoped to *effect-carrying* values (SPEC 10.6,
        // design/static-rules.md rule 8). `ctx` is one by construction — the
        // `ctx` rule admits nothing else there — but `self` is whatever the
        // receiver type is, and an ordinary struct's methods must still be
        // able to write `fn(x) => x > self.n`. So `self` is gated on its type,
        // exactly as a normal parameter is in `check_ctx_rule`.
        if role == ParamRole::Ctx {
            inf.effect_locals.insert(local);
        } else {
            inf.note_capture_risk(local, &ty);
        }
    }
    let body_span = inf.t.block_span(body);
    let expr = inf.check_block(body, Some(&expected));
    inf.unify_at(body_span, &expr.ty, &expected, "the declared return type");
    let hir_body = inf.finish(expr);
    c.bodies.insert(fid, std::sync::Arc::new(hir_body));
}

fn check_const(c: &mut Checker, cid: ConstId) {
    let info = c.tables.const_(cid).clone();
    let Some((module, index)) = info.ast.item() else { return };
    let Some(tree::Item::Let(decl)) = c.module(module).ast.items.get(index as usize) else {
        return;
    };
    let mut inf = Infer::new(c, info.module, Vec::new(), info.ty);
    inf.push_scope();
    let ty = info.ty;
    let value_span = inf.t.span(decl.value);
    let value = inf.check_expr(decl.value, Some(&ty));
    inf.unify_at(value_span, &value.ty, &ty, "the declared type");
    let body = inf.finish(value);
    c.const_values.insert(cid, body.expr);
}

/// Checks one `context` declaration, once.
///
/// Called both from the loop above, in the order the ids were minted, and from
/// a *use* of the declaration that found it unchecked
/// (`expressions.rs`'s `Static::Context`). The second caller is what makes the
/// order the ids were minted in stop mattering: a declaration built from
/// another reads the base's recorded type, and the minting order is the order
/// the modules were discovered in — so `context Fixture { ..Base() }` in a file
/// that is the first in its package to import the module `Base` comes from used
/// to be checked *before* `Base` was, and quietly kept only the bindings it
/// wrote itself. Every use of it then failed `missing-impl` for an effect
/// the spread was supposed to supply.
///
/// [`Checker::ctx_decls_reached`] is what keeps this to once each: a second
/// call returns, and so does a call that arrives round a cycle
/// (`context A { ..A() }`), which leaves `checked` at `None` and the use at
/// `Ty::Error` exactly as before.
pub(super) fn check_context_decl(c: &mut Checker, id: ContextDeclId) {
    if !c.ctx_decls_reached.insert(id) {
        return;
    }
    let info = c.tables.ctx_decl(id).clone();
    let Some((decl_module, decl_index)) = info.ast.item() else { return };
    let Some(tree::Item::Context(decl)) = c.module(decl_module).ast.items.get(decl_index as usize)
    else {
        return;
    };
    let mut inf = Infer::new(c, info.module, Vec::new(), Ty::UNIT);
    inf.in_main = true;
    inf.push_scope();
    let expr = inf.check_context_body(decl.body, decl.span);
    // A body that did not evaluate to a context type has no generated type,
    // and then its constructor is not usable either — so neither is recorded.
    let ctx_ty = match expr.ty.kind() {
        TyKind::Ctx(ct) => Some(*ct),
        _ => None,
    };
    let body = inf.finish(expr);
    // A named context is constructed by calling it, and each call builds a
    // fresh one, so the declaration becomes a nullary function.
    let ret = body.expr.ty;
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
    c.bodies.insert(ctor, std::sync::Arc::new(body));
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
                ret: Ty::UNIT,
                exported: false,
                span: t.span,
                self_ty: None,
                impl_of: None,
                ast: AstRef::Item { module, item: index as u32 },
                intrinsic: false,
            });
            // The case is registered whatever this analysis was asked for, so
            // that `Checked::tests` — and the ids everything after it counts
            // from — do not move with the selection. Only the body is scoped.
            if c.wants_file(c.module(module).file) {
                let mut inf = Infer::new(c, module, Vec::new(), Ty::UNIT);
                inf.in_main = true;
                inf.push_scope();
                let expr = inf.check_block(t.body, None);
                let body = inf.finish(expr);
                c.bodies.insert(fid, std::sync::Arc::new(body));
            }
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
pub(crate) struct LitCheck<'b> {
    pub(crate) value: u128,
    pub(crate) negative: bool,
    pub(crate) raw: &'b str,
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
    /// Every local in scope, innermost last: its name's hash and its id. A
    /// local's name is the one it was bound under, so a lookup compares the
    /// hash and then the name in [`Infer::locals`].
    ///
    /// One flat list rather than a map per scope: a body binds a handful of
    /// names, and walking them back from the innermost is quicker than hashing
    /// into a table that had to be allocated, with a copy of every name as its
    /// key.
    pub(crate) scopes: Vec<(u64, LocalId)>,
    /// Where each open scope starts in `scopes`.
    pub(crate) scope_starts: Vec<usize>,
    /// Emptied type lists, for a call being checked to borrow and give back.
    /// A call's arguments are calls too, so this holds one per level of
    /// nesting.
    pub(crate) ty_lists: Vec<Vec<Ty>>,
    pub(crate) locals: Vec<typed::Local>,
    pub(crate) params: Vec<LocalId>,
    pub(crate) self_con: Option<TyConId>,
    /// Locals holding an effect-carrying value, which a lambda may not
    /// capture (SPEC 10.6).
    pub(crate) effect_locals: std::collections::HashSet<LocalId>,
    /// Locals whose type mentions a type parameter in a position that would
    /// hold an effect if the parameter were instantiated at a context type. A
    /// lambda may not capture one of these either: the body is checked once,
    /// polymorphically (guides/compile-speed.md), so this is the last point at
    /// which the question can be asked at all. See `Tables::may_carry_effect`.
    pub(crate) poly_locals: std::collections::HashSet<LocalId>,
    pub(crate) lambda_depth: u32,
    pub(crate) obligations: Vec<(Ty, TraitId, Span)>,
    pub(crate) lit_checks: Vec<LitCheck<'b>>,
    /// Template holes, checked after defaulting so `"${1 + 1}"` is fine.
    pub(crate) hole_checks: Vec<(Ty, Span)>,
    /// Calls to a bodyless declaration — an intrinsic the runtime supplies —
    /// with the type arguments the call site instantiated it at.
    pub(crate) erased_calls: Vec<(FnId, Vec<Ty>, Span)>,
    pub(crate) role: Role,
    /// Whether the body being checked supplies a method of an *effect*.
    ///
    /// That body is where the operation is implemented, so it is one of the
    /// two layers that may still call an effect method on a value — the other
    /// is the standard library, which [`Infer::role`] names. It is what keeps
    /// SPEC 10.8's attenuation wrapper writable: `ReadOnly<C>`'s `readFile`
    /// delegates with `self.0.readFile(path)`, and cannot delegate to
    /// `core/fs`'s wrapper, which is bounded `Allocator + FileSystemRead` where
    /// the `impl` carries only `C: FileSystemRead`. See `expressions.rs`'s `report_effect_method`.
    pub(crate) in_effect_impl: bool,
    /// Whether the body being checked is an entry's. A context may be built in
    /// an entry's body, not merely anywhere in the module that exports it.
    ///
    /// There may be more than one entry: a binary declaring a page and a worker
    /// enters through `main` and through `fetch`, and both build their own.
    pub(crate) in_main: bool,
    /// The or-pattern being checked, if any.
    pub(crate) or_scope: Option<OrScope>,
    /// Names bound by the pattern currently being checked, so a duplicate
    /// within one pattern is caught (design/static-rules.md rule 6).
    pub(crate) pattern_names: Vec<&'b str>,
    /// The regions covered by blocks in this body whose closing `}` was never
    /// written.
    ///
    /// Where such a block ends is the parser's guess, so everything it
    /// swallowed past the mistake is text the missing brace moved rather than
    /// a program somebody wrote. [`Infer::finish`] takes back what was
    /// reported from inside one, leaving the syntax error standing alone.
    pub(crate) broken: Vec<Span>,
    /// How many diagnostics had been reported before this body was looked at.
    /// The parser's own sit below it, so retracting counts from here.
    mark: usize,
}

/// The buffers inference fills and empties within one body, kept on the
/// checker between bodies so each body reuses the allocations of the last.
#[derive(Default)]
pub(crate) struct Scratch<'b> {
    subst: Subst,
    scopes: Vec<(u64, LocalId)>,
    scope_starts: Vec<usize>,
    ty_lists: Vec<Vec<Ty>>,
    obligations: Vec<(Ty, TraitId, Span)>,
    lit_checks: Vec<LitCheck<'b>>,
    pattern_names: Vec<&'b str>,
}

impl Drop for Infer<'_, '_> {
    fn drop(&mut self) {
        let mut subst = std::mem::take(&mut self.subst);
        subst.clear();
        let mut scopes = std::mem::take(&mut self.scopes);
        scopes.clear();
        let mut scope_starts = std::mem::take(&mut self.scope_starts);
        scope_starts.clear();
        let ty_lists = std::mem::take(&mut self.ty_lists);
        let mut obligations = std::mem::take(&mut self.obligations);
        obligations.clear();
        let mut lit_checks = std::mem::take(&mut self.lit_checks);
        lit_checks.clear();
        let mut pattern_names = std::mem::take(&mut self.pattern_names);
        pattern_names.clear();
        self.c.scratch =
            Scratch { subst, scopes, scope_starts, ty_lists, obligations, lit_checks, pattern_names };
    }
}

/// The hash a local's name is found by in [`Infer::scopes`].
fn name_hash(name: &str) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault};
    BuildHasherDefault::<crate::hash::FxHasher>::default().hash_one(name)
}

impl<'a, 'b> Infer<'a, 'b> {
    fn new(c: &'a mut Checker<'b>, module: ModuleId, generics: Vec<GenericInfo>, ret: Ty) -> Self {
        let role = c.module(module).role;
        let mark = c.diags.items.len();
        let t = &c.module(module).ast.tree;
        let Scratch { subst, scopes, scope_starts, ty_lists, obligations, lit_checks, pattern_names } =
            std::mem::take(&mut c.scratch);
        Infer {
            c,
            t,
            module,
            generics,
            ret,
            subst,
            scopes,
            scope_starts,
            ty_lists,
            locals: Vec::new(),
            params: Vec::new(),
            self_con: None,
            effect_locals: std::collections::HashSet::default(),
            poly_locals: std::collections::HashSet::default(),
            lambda_depth: 0,
            obligations,
            lit_checks,
            hole_checks: Vec::new(),
            erased_calls: Vec::new(),
            role,
            in_effect_impl: false,
            in_main: false,
            or_scope: None,
            pattern_names,
            broken: Vec::new(),
            mark,
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
        // `subst`, `generics` and `c` are different fields, so neither the
        // type nor the list is copied — and this runs for every parameter,
        // every `let` and every pattern binding.
        let resolved = &self.subst.shallow(ty);
        let carries = self.c.tables.is_effect_carrying(resolved, &self.generics);
        let may = !carries && self.c.tables.may_carry_effect(resolved, &self.generics);
        if carries {
            self.effect_locals.insert(local);
        } else if may {
            self.poly_locals.insert(local);
        }
    }

    fn finish(mut self, expr: typed::Expr) -> typed::Body {
        // Only if nothing constrains a literal does the default apply.
        self.subst.default_numerics(&self.c.tables);
        self.discharge_obligations();
        self.check_literal_ranges();
        self.check_template_holes();
        self.check_erased_calls();
        self.retract_from_broken_blocks();
        // After the checks above, never before: they read an unbound variable
        // as "not yet known" and would report a type the body never wrote.
        self.subst.default_unconstrained();
        let mut expr = expr;
        self.resolve_expr(&mut expr);
        let mut locals = std::mem::take(&mut self.locals);
        locals.iter_mut().for_each(|l| self.subst.resolve_in_place(&mut l.ty));
        typed::Body { locals, params: std::mem::take(&mut self.params), expr }
    }

    /// Takes back everything this body reported from inside a block whose `}`
    /// was never written, so the reader is told the one mistake they made.
    ///
    /// The bindings stay on the body — that is what an editor reads. Runs
    /// after the deferred checks, because a literal's range and a template
    /// hole are reported long after the block they sit in returned.
    fn retract_from_broken_blocks(&mut self) {
        if self.broken.is_empty() {
            return;
        }
        let broken = std::mem::take(&mut self.broken);
        self.c.diags.retract_from(self.mark, |d| {
            broken
                .iter()
                .any(|r| d.span.file == r.file && d.span.start >= r.start && d.span.end <= r.end)
        });
    }

    /// A callee is pre-monomorphization here, so only its type arguments need
    /// resolving.
    fn resolve_callee(&self, c: &mut typed::Callee) {
        match c {
            typed::Callee::Decl { targs, .. } => self.resolve_all(targs),
            typed::Callee::Func(_) => {}
        }
    }

    fn resolve_all(&self, tys: &mut [Ty]) {
        tys.iter_mut().for_each(|t| self.subst.resolve_in_place(t));
    }

    /// Applies the substitution to every type in a checked body, in place.
    ///
    /// In place because the tree is this body's own: rebuilding it moved every
    /// boxed child into a fresh box and copied every type, including the ones
    /// with no variable in them, which are most.
    fn resolve_expr(&self, e: &mut typed::Expr) {
        self.subst.resolve_in_place(&mut e.ty);
        match &mut e.kind {
            typed::ExprKind::CallValue { callee, args } => {
                self.resolve_expr(callee);
                args.iter_mut().for_each(|a| self.resolve_expr(a));
            }
            typed::ExprKind::CallFn { func, args } => {
                self.resolve_callee(func);
                args.iter_mut().for_each(|a| self.resolve_expr(a));
            }
            typed::ExprKind::CallTrait { recv, targs, args, .. } => {
                self.subst.resolve_in_place(recv);
                self.resolve_all(targs);
                args.iter_mut().for_each(|a| self.resolve_expr(a));
            }
            typed::ExprKind::StructLit { targs, fields: args, .. }
            | typed::ExprKind::EnumLit { targs, args, .. }
            | typed::ExprKind::Intrinsic { targs, args, .. } => {
                self.resolve_all(targs);
                args.iter_mut().for_each(|a| self.resolve_expr(a));
            }
            typed::ExprKind::StructUpdate { base, updates, .. } => {
                self.resolve_expr(base);
                updates.iter_mut().for_each(|(_, e)| self.resolve_expr(e));
            }
            typed::ExprKind::Tuple(args)
            | typed::ExprKind::Array(args)
            | typed::ExprKind::Prim { args, .. }
            | typed::ExprKind::StructuralEq { args, .. }
            | typed::ExprKind::StructuralCmp { args, .. } => {
                args.iter_mut().for_each(|a| self.resolve_expr(a));
            }
            typed::ExprKind::Field { base, .. }
            | typed::ExprKind::TupleIndex { base, .. }
            | typed::ExprKind::Try { base, .. }
            | typed::ExprKind::CtxGet { base, .. } => self.resolve_expr(base),
            typed::ExprKind::Index { base, index, elem } => {
                self.resolve_expr(base);
                self.resolve_expr(index);
                self.subst.resolve_in_place(elem);
            }
            typed::ExprKind::Block { stmts, tail } => {
                for s in stmts {
                    match s {
                        typed::Stmt::Let { pattern, value, .. } => {
                            self.resolve_pattern(pattern);
                            self.resolve_expr(value);
                        }
                        typed::Stmt::Expr(e) => self.resolve_expr(e),
                    }
                }
                if let Some(t) = tail {
                    self.resolve_expr(t);
                }
            }
            typed::ExprKind::If { cond, then, else_ } => {
                self.resolve_expr(cond);
                self.resolve_expr(then);
                self.resolve_expr(else_);
            }
            typed::ExprKind::Match { scrutinee, arms } => {
                self.resolve_expr(scrutinee);
                for a in arms {
                    self.resolve_pattern(&mut a.pattern);
                    if let Some(g) = &mut a.guard {
                        self.resolve_expr(g);
                    }
                    self.resolve_expr(&mut a.body);
                }
            }
            typed::ExprKind::Lambda { body, .. } => self.resolve_expr(body),
            typed::ExprKind::And { lhs, rhs } | typed::ExprKind::Or { lhs, rhs } => {
                self.resolve_expr(lhs);
                self.resolve_expr(rhs);
            }
            typed::ExprKind::Template { parts } => {
                for p in parts {
                    if let typed::TemplatePart::Hole(h) = p {
                        self.resolve_expr(h);
                    }
                }
            }
            typed::ExprKind::CtxLit { bindings } => bindings.iter_mut().for_each(|(_, e)| self.resolve_expr(e)),
            typed::ExprKind::FnRef(c) => self.resolve_callee(c),
            _ => {}
        }
    }

    fn resolve_pattern(&self, p: &mut typed::Pattern) {
        self.subst.resolve_in_place(&mut p.ty);
        match &mut p.kind {
            typed::PatKind::Bind { sub: Some(s), .. } => self.resolve_pattern(s),
            typed::PatKind::Tuple(ps) | typed::PatKind::Or(ps) | typed::PatKind::Array { elems: ps, .. } => {
                ps.iter_mut().for_each(|x| self.resolve_pattern(x));
            }
            typed::PatKind::Struct { fields, .. } | typed::PatKind::Variant { fields, .. } => {
                fields.iter_mut().for_each(|f| self.resolve_pattern(&mut f.pattern));
            }
            _ => {}
        }
    }

    // -- scopes -------------------------------------------------------------

    pub(crate) fn push_scope(&mut self) {
        self.scope_starts.push(self.scopes.len());
    }

    pub(crate) fn pop_scope(&mut self) {
        if let Some(start) = self.scope_starts.pop() {
            self.scopes.truncate(start);
        }
    }

    pub(crate) fn new_local(&mut self, name: &str, ty: Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(typed::Local { name: name.to_string(), ty, span });
        id
    }

    /// A new local, bound in the innermost scope under the name it takes.
    pub(crate) fn bind_new(&mut self, name: String, ty: Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.scopes.push((name_hash(&name), id));
        self.locals.push(typed::Local { name, ty, span });
        id
    }

    pub(crate) fn bind(&mut self, name: &str, local: LocalId) {
        // Shadowing is permitted, both in nested scopes and within a block:
        // the innermost binding is the last one, and a lookup finds it first.
        debug_assert!(!self.scope_starts.is_empty(), "a body is checked inside a scope pushed by `push_scope`");
        debug_assert_eq!(self.local(local).name, name, "a local is bound under its own name");
        self.scopes.push((name_hash(name), local));
    }

    pub(crate) fn lookup_local(&self, name: &str) -> Option<LocalId> {
        let hash = name_hash(name);
        self.scopes
            .iter()
            .rev()
            .find(|(h, id)| *h == hash && self.local(*id).name == name)
            .map(|(_, id)| *id)
    }

    pub(crate) fn local(&self, id: LocalId) -> &typed::Local {
        self.locals.get(id.index()).or_ice("every LocalId was minted by new_local on this body")
    }

    pub(crate) fn local_ty(&self, id: LocalId) -> Ty {
        self.local(id).ty
    }

    // -- unification --------------------------------------------------------

    pub(crate) fn fresh(&mut self, span: Span) -> Ty {
        self.subst.fresh(span)
    }

    pub(crate) fn unify_at(&mut self, span: Span, actual: &Ty, expected: &Ty, what: &str) {
        if let Err((a, b)) = self.subst.unify(&self.c.tables, actual, expected) {
            let a = show_in_diagnostic(&self.c.tables, &self.subst, &self.generics, &a);
            let b = show_in_diagnostic(&self.c.tables, &self.subst, &self.generics, &b);
            let (found, wanted) = (a.quoted(), b.quoted());
            let mut d = Diagnostic::templated("type-mismatch", span)
                .with_bind("expected", wanted.clone())
                .with_bind("found", found.clone())
                .with_mismatch(wanted.clone(), found);
            if !what.is_empty() {
                d = d.with_label(format!("{what} is {wanted}"));
            }
            // There is no implicit promotion of any kind, and the most common
            // way to hit this is expecting one. The conversion is named
            // explicitly, because which one it is depends on whether the value
            // can fail to fit. Every other mismatch gets no `fix`: the only
            // sentence there is to write is "produce the expected type", which
            // the `expected` and `actual` lines have already said.
            if is_numeric_mismatch(&a, &b) {
                d = d
                    .with_note("there is no implicit promotion of any kind")
                    .with_fix(numeric_fix(&a, &b));
            } else if let Some(note) = unpinned_literal_note(&a, &b) {
                d = d.with_note(note);
            }
            self.c.diags.push(d);
        }
    }

    /// A `.Variant` — as an expression or as a pattern — where the position
    /// expects something that is not an enum.
    ///
    /// It is an ordinary type mismatch, so it reads as one: the position wants
    /// `expected`, and what is written is a variant. The note teaches the
    /// syntax, because a reader who wrote the dot form usually believes the
    /// position is an enum, and nothing here can guess the value they meant.
    pub(crate) fn report_dot_form_against(&mut self, expected: &str, variant: &str, span: Span) {
        let wanted = format!("`{expected}`");
        let written = format!("a variant `.{variant}`");
        self.templated("type-mismatch", span)
            .bind("expected", wanted.clone())
            .bind("found", written.clone())
            .mismatch(wanted, written)
            .note(format!(
                "a value written with a leading dot names a variant of the enum the \
                 position expects, and this position expects `{expected}`"
            ));
    }

    pub(crate) fn resolve(&self, t: &Ty) -> Ty {
        self.subst.shallow(t)
    }

    /// Whether `t` holds no inference variable anywhere, once resolved.
    ///
    /// `resolve` answers for the head only, which is all almost every caller
    /// needs. This is for the one that needs the whole type: an anonymous
    /// struct literal reads its type from above rather than solving for it, so
    /// `Holder<?>` is not a type it may be given even though its head is known.
    pub(crate) fn is_settled(&self, t: &Ty) -> bool {
        match self.resolve(t).kind() {
            TyKind::Var(_) => false,
            TyKind::Con(_, args) | TyKind::Tuple(args) => args.iter().all(|a| self.is_settled(a)),
            TyKind::Array(e) => self.is_settled(e),
            TyKind::Fn(params, ret) => {
                params.iter().all(|p| self.is_settled(p)) && self.is_settled(ret)
            }
            _ => true,
        }
    }

    pub(crate) fn prim(&self, p: Prim) -> Ty {
        self.c.tables.prim(p)
    }

    /// `as_prim` reads the head constructor and nothing else, and it runs
    /// twice per expression node through `coerce`, so it must not copy the
    /// type to ask.
    pub(crate) fn as_prim(&self, t: &Ty) -> Option<Prim> {
        self.c.tables.as_prim(&self.resolve(t))
    }

    pub(crate) fn show_ty(&self, t: &Ty) -> String {
        show(&self.c.tables, Some(&self.subst), &self.generics, t)
    }

    /// A diagnostic whose wording lives on its page. What follows is
    /// `.bind(…)` for each `{placeholder}` the page names.
    pub(crate) fn templated(&mut self, code: &str, span: Span) -> &mut Diagnostic {
        self.c.diags.items.push(Diagnostic::templated(code, span));
        self.c.diags.items.last_mut().or_ice("the diagnostic just pushed is the last one")
    }

    pub(crate) fn error_expr(&self, span: Span) -> typed::Expr {
        typed::Expr::new(typed::ExprKind::Error, Ty::ERROR, span)
    }

    // -- obligations --------------------------------------------------------

    fn discharge_obligations(&mut self) {
        let mut obligations = std::mem::take(&mut self.obligations);
        for (ty, tr, span) in obligations.drain(..) {
            let ty = self.subst.resolve(&ty);
            if self.satisfies(&ty, tr) {
                continue;
            }
            let trait_name = self.c.tables.trait_(tr).name.clone();
            let shown = self.show_ty(&ty);
            let mut note = None;
            let mut fix = None;
            // The one failure that is about the *kind* of type rather than a
            // missing implementation. Saying "add `derive Equal`" here would be
            // advice that cannot be taken.
            if !self.c.tables.trait_(tr).is_effect
                && self.c.tables.is_effect_carrying(&ty, &self.generics)
            {
                self.templated("effect-carrying-bound", span)
                    .bind("type", shown)
                    .bind("trait", trait_name);
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
                } else if let Some((imp, (param, arg, missing))) = self
                    .c
                    .tables
                    .impls
                    .get(&(tr, con))
                    .and_then(|imp| Some((imp, self.unmet_impl_bound(imp, &ty, &mut Vec::new())?)))
                {
                    // The `impl` is there, and asks something of its type
                    // arguments that this one falls short of.
                    let head = show(&self.c.tables, None, &imp.generics, &imp.head);
                    let name = imp.generics.get(param).map_or("_", |g| g.name.as_str());
                    let lacking = self.c.tables.trait_(missing).name.clone();
                    let condition =
                        format!("`{head}` implements `{trait_name}` only when `{name}` satisfies `{lacking}`");
                    if let TyKind::Ctx(id) = arg.kind() {
                        // A context is the one argument whose fix is a
                        // binding rather than an `impl`.
                        let bound: Vec<String> = self
                            .c
                            .tables
                            .ctx_type(*id)
                            .bindings
                            .iter()
                            .map(|(t, _)| self.c.tables.trait_(*t).name.clone())
                            .collect();
                        let binds = if bound.is_empty() {
                            "no effects".to_string()
                        } else {
                            crate::diagnostics::names(&bound)
                        };
                        note = Some(format!(
                            "{condition}, and here `{name}` is a context that binds {binds}"
                        ));
                        fix = Some(format!(
                            "bind `{lacking}` in the `context {{ ... }}` this call is handed"
                        ));
                    } else {
                        let arg = self.show_ty(&arg);
                        note = Some(format!("{condition}, and here `{name}` is `{arg}`"));
                        fix = Some(format!("make `{arg}` satisfy `{lacking}`"));
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
            let d = self.templated("missing-impl", span);
            d.bind("type", shown).bind("trait", trait_name);
            d.fix(fix);
            if let Some(n) = note {
                d.notes.push(n);
            }
        }
        // Kept for the next body, unless discharging one added another.
        if self.obligations.is_empty() {
            self.obligations = obligations;
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
        // `impl<C> Equal for Holder<C>` would let `Holder<Ctx>` through a
        // `T: Equal` bound, and a lambda in that function could capture the
        // capability inside it (SPEC 10.6).
        if !matches!(ty.kind(), TyKind::Error | TyKind::Var(_))
            && !self.c.tables.trait_(tr).is_effect
            && self.c.tables.is_effect_carrying(ty, &self.generics)
        {
            return false;
        }
        match ty.kind() {
            TyKind::Param(i) => self
                .generics
                .get(*i as usize)
                .is_some_and(|g| g.bounds.contains(&tr)),
            TyKind::Error | TyKind::Var(_) => true,
            TyKind::Ctx(id) => self.c.tables.ctx_type(*id).has(tr),
            TyKind::SelfTy => true,
            // `[T]`, tuples and function types satisfy the structural traits
            // when their components do (SPEC 5.11).
            TyKind::Array(e) => self.structural_trait(tr) && self.satisfies_seen(e, tr, seen),
            TyKind::Tuple(es) => {
                self.structural_trait(tr)
                    && es.iter().all(|e| self.satisfies_seen(e, tr, seen))
            }
            TyKind::Unit => self.structural_trait(tr),
            TyKind::Con(id, args) => {
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
                    return self.unmet_impl_bound(imp, ty, seen).is_none();
                }
                false
            }
            TyKind::Fn(..) => false,
        }
    }

    /// The first of a hand-written `impl`'s own bounds that `ty` fails: the
    /// parameter's index, the type standing for it, and the trait it lacks.
    ///
    /// `impl<C: Stdout> Stdout for Scoped<C>` makes `Scoped<C>` a `Stdout`
    /// only where `C` is one. Matching the `impl`'s head against `ty` says
    /// which argument stands for `C`, and that argument is asked the same
    /// question in turn. Each one is a component of `ty`, so the walk only
    /// ever descends and needs no guard of its own; `seen` is passed along for
    /// the derived impls it reaches.
    ///
    /// A parameter the match leaves unbound, or binds to a type inference has
    /// not settled yet, is not counted against `ty`: the first was already
    /// reported at the `impl`, and the second has nothing yet to fail.
    fn unmet_impl_bound(
        &self,
        imp: &ImplInfo,
        ty: &Ty,
        seen: &mut Vec<TyConId>,
    ) -> Option<(usize, Ty, TraitId)> {
        if imp.generics.iter().all(|g| g.bounds.is_empty()) {
            return None;
        }
        let mut bound = vec![None; imp.generics.len()];
        bind_impl_params(&imp.head, ty, &mut bound);
        imp.generics.iter().zip(bound).enumerate().find_map(|(i, (g, arg))| {
            let arg = arg?;
            let missing = g.bounds.iter().find(|b| !self.satisfies_seen(&arg, **b, seen))?;
            Some((i, arg, *missing))
        })
    }

    /// The first field or payload type of a derived type that does not itself
    /// satisfy the trait.
    fn failing_component(&self, con: TyConId, ty: &Ty, tr: TraitId) -> Option<String> {
        let args = match ty.kind() {
            TyKind::Con(_, a) => a.to_vec(),
            _ => Vec::new(),
        };
        let tycon = self.c.tables.tycon(con);
        let components: Vec<Ty> = match &tycon.def {
            TyDef::Struct { fields, .. } => fields.iter().map(|f| f.ty).collect(),
            TyDef::Enum { variants } => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty))
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
            "Equal" | "Ordered" | "Show" | "Hash" | "ToJson" | "FromJson"
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
            TyDef::Struct { fields, .. } => fields.iter().map(|f| f.ty).collect(),
            TyDef::Enum { variants } => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty))
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
        let mut checks = std::mem::take(&mut self.lit_checks);
        for lit in checks.drain(..) {
            let Some(p) = self.c.tables.as_prim(&self.subst.shallow(&lit.ty)) else { continue };
            let Some((lo, hi)) = p.int_range() else { continue };
            let fits = if lit.negative {
                p.is_signed() && (lit.value <= (lo.unsigned_abs()))
            } else {
                lit.value <= hi
            };
            if !fits {
                let name = p.name();
                let raw = if lit.negative { format!("-{}", lit.raw) } else { lit.raw.to_string() };
                let mut d = Diagnostic::templated("literal-out-of-range", lit.span)
                    .with_bind("literal", raw.clone())
                    .with_bind("type", name);
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
        if self.lit_checks.is_empty() {
            self.lit_checks = checks;
        }
    }
}

impl<'a, 'b> Infer<'a, 'b> {
    /// A hole holds a primitive — `Int` (any width), `Float` (any width),
    /// `Bool`, `Char`, `Str` — or a value whose `Show` is **derived**
    /// (SPEC 3.6).
    ///
    /// The line is drawn at the derive rather than at the trait because of what
    /// interpolation may not do. A hole is rendered where the `Template` is
    /// built, and a `Template` names no context: `io.println(ctx, "hi ${name}")`
    /// needs `Stdout` and nothing else. A derived `Show` is a fold over the
    /// type's shape that the *runtime* performs — `middle::monomorphize` drops
    /// the context from `x.show(ctx)` at a derived impl already — so admitting
    /// those holes adds no bound to any signature. A hand-written
    /// `impl Show`'s `show<C: Allocator>(self, ctx: C)` has to be *called*, and
    /// there is no context here to call it with, so that conversion stays the
    /// author's: `${p.show(ctx)}`.
    /// Every call to a **bodyless declaration** was made at a type the body
    /// determines.
    ///
    /// An intrinsic's body is `cli/runtime`, compiled once against no Buri type
    /// at all, and the key the backend reaches it by carries no type arguments
    /// (`middle/monomorphize.rs`'s `GENERIC_INTRINSICS`). So the type argument
    /// at the call site is not merely the caller's opinion of what comes back —
    /// it is the *only* record of it, and everything the compiler generates
    /// around the value it answers is generated from it: its layout, and the
    /// release walk that lets go of whatever the block holds.
    ///
    /// A type argument nothing constrains is resolved to `()` a few lines below
    /// this, which is right for a value the body never inspects and built
    /// itself, and wrong for one the runtime hands back: `()` holds nothing, so
    /// the release frees the block and lets go of nothing inside it. That is a
    /// silent leak of every counted value the block was carrying, and
    /// `core/actor`'s `stop` is where it was found — a discard loop never looks
    /// inside what it drops, so nothing there determined the message type.
    ///
    /// **Before the defaulting**, because after it the two cases are one type.
    fn check_erased_calls(&mut self) {
        let calls = std::mem::take(&mut self.erased_calls);
        for (f, targs, span) in calls {
            // The declaration is read in place and the two strings are the only
            // thing taken out of it, so that reporting nothing costs nothing:
            // `answers_its_own_type` has already thrown away every call whose
            // parameters are all determined by an argument, which is most of
            // them.
            let (name, parameters) = {
                let info = self.c.tables.fn_info(f);
                let mut undetermined: Vec<&str> = Vec::new();
                for (i, (g, t)) in info.generics.iter().zip(&targs).enumerate() {
                    if answered_only(&info.params, &info.ret, i as u32)
                        && mentions_var(&self.subst.resolve(t))
                    {
                        undetermined.push(&g.name);
                    }
                }
                if undetermined.is_empty() {
                    continue;
                }
                (info.name.clone(), undetermined.join(", "))
            };
            self.c.diags.push(
                Diagnostic::templated("undetermined-type", span)
                    .with_bind("function", format!("`{name}`"))
                    .with_label("nothing here says what this answers")
                    .with_note(format!(
                        "nothing determines {parameters}, and a runtime operation is compiled \
                         once against no Buri type, so the type argument written at the call is \
                         the only record of what the value it answers holds"
                    ))
                    .with_fix(
                        "use the value the call answers at the type it really has, or write the \
                         type argument out",
                    ),
            );
        }
    }

    fn check_template_holes(&mut self) {
        let checks = std::mem::take(&mut self.hole_checks);
        for (ty, span) in checks {
            let resolved = self.subst.resolve(&ty);
            if matches!(resolved.kind(), TyKind::Error | TyKind::Var(_)) {
                continue;
            }
            // `()` renders as `()` under a derived `Show` and is admitted
            // *inside* a shape for that reason, but a hole holding one on its
            // own is a mistake with no rendering to ask for.
            if !matches!(resolved.kind(), TyKind::Unit)
                && self.renders_structurally(&resolved, &mut Vec::new())
            {
                continue;
            }
            let shown = show(&self.c.tables, Some(&self.subst), &self.generics, &resolved);
            let mut d =
                Diagnostic::templated("not-interpolatable", span).with_bind("type", shown.clone());
            let show_tid = self.c.known_traits.get("Show").copied();
            // Whether writing `derive Show` on *this* type would be the edit.
            // Not "does not satisfy `Show`", which a type whose components fall
            // short also fails — the derive belongs on the component then — and
            // never a primitive, since `Template` is the one that lands here
            // and there is no deriving `Show` for it.
            let undeclared = match (resolved.head(), show_tid) {
                (Some(con), Some(tr)) => {
                    self.c.tables.as_prim(&resolved).is_none()
                        && !self.c.tables.impls.contains_key(&(tr, con))
                }
                _ => false,
            };
            // Three different mistakes, and the edit is different for each: a
            // rendering that exists and has to be *called*, a type parameter
            // whose instantiation decides which of the two it is, and a type
            // with no rendering at all. Where the hand-written `Show` is on a
            // component, the component is what the author has to look at.
            if let Some(hand) = self.hand_written_show(&resolved, &mut Vec::new()) {
                d = d.with_note(format!(
                    "`{hand}`'s `Show` is written by hand, and `show<C: Allocator>(self, ctx: C)` \
                     names a context a hole has no way to reach"
                ));
                if hand == shown {
                    d = d.with_fix("call it in the hole, as in `${x.show(ctx)}`");
                }
            } else if matches!(resolved.kind(), TyKind::Param(_))
                && show_tid.is_some_and(|tr| self.satisfies(&resolved, tr))
            {
                d = d
                    .with_note(format!(
                        "a `{shown}: Show` may be instantiated at a type whose `Show` is written \
                         by hand, so a hole cannot render it structurally"
                    ))
                    .with_fix("call it in the hole, as in `${x.show(ctx)}`");
            } else if undeclared {
                d = d.with_fix(format!(
                    "add `derive Show for {shown};` in that type's own module, or render it \
                     first with `.show(ctx)`"
                ));
            }
            self.c.diags.push(d);
        }
    }

    /// The first type in this shape whose `Show` is an `impl` rather than a
    /// `derive`, which is the one that keeps the whole hole out.
    fn hand_written_show(&self, ty: &Ty, seen: &mut Vec<TyConId>) -> Option<String> {
        let tr = self.c.known_traits.get("Show").copied()?;
        match ty.kind() {
            TyKind::Array(e) => self.hand_written_show(e, seen),
            TyKind::Tuple(es) => es.iter().find_map(|e| self.hand_written_show(e, seen)),
            TyKind::Con(id, args) => {
                if self.c.tables.as_prim(ty).is_some() {
                    return None;
                }
                let imp = self.c.tables.impls.get(&(tr, *id))?;
                if !imp.is_derived() {
                    return Some(show(&self.c.tables, Some(&self.subst), &self.generics, ty));
                }
                if seen.contains(id) {
                    return None;
                }
                seen.push(*id);
                let found = self
                    .component_types(*id, args)
                    .into_iter()
                    .find_map(|t| self.hand_written_show(&t, seen));
                seen.pop();
                found
            }
            _ => None,
        }
    }

    /// Whether a value of this type renders the way `derive Show` renders it,
    /// with no context: a primitive, an array or tuple of such, or a type
    /// whose `Show` impl is a `derive` — all the way down.
    ///
    /// The recursion is the same one [`Infer::satisfies`] performs for a
    /// derived impl, and for the same reason: a derived rendering is a fold
    /// over the components, so a component with a hand-written `Show` would be
    /// rendered structurally and disagree with its own `show`. `seen` stops a
    /// recursive type, whose answer depends on itself and is therefore yes.
    fn renders_structurally(&self, ty: &Ty, seen: &mut Vec<TyConId>) -> bool {
        // A type that merely mentions an effect is part of the world rather
        // than part of your data, and nothing renders it (SPEC 10.1).
        if self.c.tables.is_effect_carrying(ty, &self.generics) {
            return false;
        }
        match ty.kind() {
            TyKind::Array(e) => self.renders_structurally(e, seen),
            TyKind::Tuple(es) => es.iter().all(|e| self.renders_structurally(e, seen)),
            TyKind::Con(id, args) => {
                if let Some(p) = self.c.tables.as_prim(ty) {
                    return p.is_integer()
                        || p.is_float()
                        || matches!(p, Prim::Bool | Prim::Char | Prim::Str);
                }
                let Some(tr) = self.c.known_traits.get("Show").copied() else { return false };
                let Some(imp) = self.c.tables.impls.get(&(tr, *id)) else { return false };
                if !imp.is_derived() {
                    return false;
                }
                if seen.contains(id) {
                    return true;
                }
                seen.push(*id);
                let ok = self.components_render_structurally(*id, args, seen);
                seen.pop();
                ok
            }
            _ => false,
        }
    }

    fn components_render_structurally(
        &self,
        con: TyConId,
        args: &[Ty],
        seen: &mut Vec<TyConId>,
    ) -> bool {
        self.component_types(con, args)
            .iter()
            .all(|t| matches!(t.kind(), TyKind::Unit) || self.renders_structurally(t, seen))
    }

    /// Every field and payload type of one type constructor, with the
    /// constructor's own arguments substituted in.
    fn component_types(&self, con: TyConId, args: &[Ty]) -> Vec<Ty> {
        let tycon = self.c.tables.tycon(con);
        let declared: Vec<Ty> = match &tycon.def {
            TyDef::Struct { fields, .. } => fields.iter().map(|f| f.ty).collect(),
            TyDef::Enum { variants } => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty))
                .collect(),
            TyDef::Prim(_) => Vec::new(),
        };
        declared.iter().map(|t| substitute(t, args, None)).collect()
    }
}

/// The conversion to reach for, named exactly. Which one it is depends on
/// whether the value can fail to fit, so a generic "convert it" would leave the
/// reader to work out the return type for themselves.
fn numeric_fix(actual: &Spelling, expected: &Spelling) -> String {
    match (actual, expected) {
        // Neither side is pinned, so there is no conversion to name: one of the
        // two literals has to be written in the other's kind.
        (Spelling::Literal(_), Spelling::Literal(_)) => {
            return "write both literals in the same kind, either both integers or both floats"
                .to_string();
        }
        // A literal has not been pinned to a type yet, so an annotation is the
        // edit, not a conversion.
        (Spelling::Literal(_), _) => {
            return format!("annotate the literal, as in `let x: {} = ...`", expected.name());
        }
        (_, Spelling::Literal(_)) => {
            return format!(
                "write a literal of the right kind, or convert with `.to{}()`",
                actual.name()
            );
        }
        _ => {}
    }
    let (actual, expected) = (actual.name(), expected.name());
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

fn is_numeric_mismatch(a: &Spelling, b: &Spelling) -> bool {
    let numericish = |s: &Spelling| match s {
        Spelling::Code(name) => {
            name.starts_with('I') || name.starts_with('U') || name.starts_with('F')
        }
        Spelling::Literal(_) => true,
        Spelling::Unconstrained => false,
    };
    numericish(a) && numericish(b) && a != b
}

/// The message names the literal by its default, so the note says the literal
/// is not held to it: any type of the class would have done, and this is not one.
fn unpinned_literal_note(a: &Spelling, b: &Spelling) -> Option<String> {
    let (class, other) = match (a, b) {
        (Spelling::Literal(class), Spelling::Code(other))
        | (Spelling::Code(other), Spelling::Literal(class)) => (class, other),
        _ => return None,
    };
    let kind = match class {
        NumClass::Int => "integer",
        NumClass::Float => "float",
    };
    Some(format!(
        "{} defaults to `{}` but takes any {kind} type, and `{other}` is not one",
        class.literal_phrase(),
        class.default_name()
    ))
}

/// Whether a **resolved** type still holds an unbound inference variable.
///
/// `Subst::resolve` has already followed every binding, so a `Ty::Var` left in
/// the tree is one nothing in the body ever constrained.
fn mentions_var(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Var(_) => true,
        TyKind::Con(_, args) => args.iter().any(mentions_var),
        TyKind::Array(e) => mentions_var(e),
        TyKind::Tuple(es) => es.iter().any(mentions_var),
        TyKind::Fn(ps, r) => ps.iter().any(mentions_var) || mentions_var(r),
        _ => false,
    }
}

/// Whether a declaration's `i`th type parameter is one **only its answer**
/// mentions.
///
/// That is the shape where the runtime is the sole source of a value of the
/// type: nothing the caller passes in has it, so nothing the caller passes in
/// can say what it is. A parameter that also appears in an argument — a step's
/// error type, a context — is determined by the argument or is a type no value
/// is ever built at, and neither is this check's business.
fn answered_only(params: &[ParamInfo], ret: &Ty, i: u32) -> bool {
    !params.iter().any(|p| mentions_param(&p.ty, i)) && mentions_param(ret, i)
}

/// Whether a declaration has any such parameter at all.
///
/// The gate on the recording side, and it is what keeps this whole check off
/// the hot path: nearly every generic intrinsic — the whole of `core/list`, the
/// conversions, the renderers — takes each of its type parameters in an
/// argument, so nearly every call site answers `false` here and is never
/// recorded. `core/actor`'s entries are the family it is here for.
pub(crate) fn answers_its_own_type(info: &FnInfo) -> bool {
    (0..info.generics.len() as u32).any(|i| answered_only(&info.params, &info.ret, i))
}

/// Whether a type mentions the `i`th rigid generic parameter of its item.
fn mentions_param(ty: &Ty, i: u32) -> bool {
    match ty.kind() {
        TyKind::Param(p) => *p == i,
        TyKind::Con(_, args) => args.iter().any(|a| mentions_param(a, i)),
        TyKind::Array(e) => mentions_param(e, i),
        TyKind::Tuple(es) => es.iter().any(|e| mentions_param(e, i)),
        TyKind::Fn(ps, r) => ps.iter().any(|p| mentions_param(p, i)) || mentions_param(r, i),
        _ => false,
    }
}

/// Binds an `impl`'s type parameters by matching its head against a type the
/// head names: `Scoped<C>` against `Scoped<Ctx>` binds `C` to `Ctx`.
///
/// A part of `ty` that inference has not settled, or that is already an
/// error, matches anything and binds nothing, so a parameter it would have
/// decided stays `None` and is not held against the type. A parameter the
/// head mentions twice keeps its first binding.
fn bind_impl_params(head: &Ty, ty: &Ty, bound: &mut [Option<Ty>]) {
    match (head.kind(), ty.kind()) {
        (_, TyKind::Var(_) | TyKind::Error) => {}
        (TyKind::Param(i), _) => {
            if let Some(slot @ None) = bound.get_mut(*i as usize) {
                *slot = Some(*ty);
            }
        }
        (TyKind::Con(_, xs), TyKind::Con(_, ys)) | (TyKind::Tuple(xs), TyKind::Tuple(ys)) => {
            xs.iter().zip(ys.iter()).for_each(|(h, t)| bind_impl_params(h, t, bound));
        }
        (TyKind::Array(h), TyKind::Array(t)) => bind_impl_params(h, t, bound),
        (TyKind::Fn(xs, a), TyKind::Fn(ys, b)) => {
            xs.iter().zip(ys.iter()).for_each(|(h, t)| bind_impl_params(h, t, bound));
            bind_impl_params(a, b, bound);
        }
        _ => {}
    }
}
