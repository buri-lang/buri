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
use crate::diagnostics::{Diagnostic, Diagnostics, Invariant as _, Span};
use crate::hash::Map as HashMap;

/// What a concrete function *is*.
///
/// These were two `Option`s side by side. Both set meant the backend picked
/// the body and silently dropped the intrinsic, while `inline_intrinsic` took
/// the other view; neither set was a function the caller could call, which the
/// backend compiled to `return 0` — a trait method with no impl returning zero
/// at whatever type the caller expected.
pub enum FuncKind {
    /// Requested but never built: a trait method with no impl, or a build that
    /// stopped after a diagnostic. Reaching one at run time is a compiler bug,
    /// so it compiles to an abort rather than to a value.
    Unbuilt,
    Body(typed::Expr),
    /// An operation the runtime supplies, by key.
    Intrinsic(String),
}

/// One concrete function in the output.
pub struct Func {
    /// A stable, deterministic symbol, derived from the module path and name
    /// rather than from instantiation order.
    pub symbol: String,
    pub debug_name: String,
    pub params: Vec<LocalId>,
    pub locals: Vec<typed::Local>,
    pub kind: FuncKind,
    pub ret: Ty,
    /// A descriptor the backend passes as an extra argument. Only the test
    /// runner's `report` needs one: it renders both values on a failure, and
    /// rendering is the runner's rather than the program's.
    pub desc: Option<usize>,
    pub span: Span,
}

impl Func {
    pub fn body(&self) -> Option<&typed::Expr> {
        match &self.kind {
            FuncKind::Body(e) => Some(e),
            _ => None,
        }
    }

    pub fn body_mut(&mut self) -> Option<&mut typed::Expr> {
        match &mut self.kind {
            FuncKind::Body(e) => Some(e),
            _ => None,
        }
    }

    /// Takes the body out, leaving the function unbuilt. Only the inliner does
    /// this, and it puts one back.
    pub fn take_body(&mut self) -> Option<typed::Expr> {
        match std::mem::replace(&mut self.kind, FuncKind::Unbuilt) {
            FuncKind::Body(e) => Some(e),
            other => {
                self.kind = other;
                None
            }
        }
    }

    pub fn set_body(&mut self, e: typed::Expr) {
        self.kind = FuncKind::Body(e);
    }

    pub fn intrinsic_key(&self) -> Option<&str> {
        match &self.kind {
            FuncKind::Intrinsic(k) => Some(k),
            _ => None,
        }
    }
}

pub struct TestEntry {
    pub name: String,
    pub func: FuncIdx,
    pub module: String,
    pub span: Span,
}

/// One field of a described aggregate: its name and the descriptor of its
/// type. These were two `Vec`s side by side — `fields: Vec<String>` and
/// `types: Vec<usize>` — that had to be the same length, and the equality
/// generator iterates `types` alone while `show` reads `fields`, so a skew
/// made `show` read past its own name table.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DescField {
    pub name: String,
    pub ty: usize,
}

/// A runtime type descriptor, for the structural operations `derive` generates.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Desc {
    /// Compared and rendered by value.
    Prim(Prim),
    Unit,
    /// Rendered as `Name { field: .., .. }` or `Name(..)`.
    Struct { name: String, record: bool, fields: Vec<DescField> },
    Enum { name: String, variants: Vec<DescVariant> },
    Array(usize),
    Tuple(Vec<usize>),
    /// `Option<T>`, whose value is the payload itself.
    Option(usize),
    /// A type with no structural rendering.
    Opaque(String),
    /// A slot claimed before its descriptor was built, so that a recursive
    /// type terminates. It was `Opaque(String::new())`, which made an
    /// unfinished descriptor indistinguishable from a finished opaque one.
    Reserved,
}

impl Desc {
    /// Whether an enum's variants all carry nothing, so two values of it are
    /// equal exactly when they are the same tag.
    ///
    /// Derived rather than cached: it was a `bool` beside `variants`, and
    /// `eq_kind` turns `true` into `a === b`, which is *always false* for two
    /// separately built payload-carrying arrays.
    pub fn payloadless(&self) -> bool {
        match self {
            Desc::Enum { variants, .. } => variants.iter().all(|v| v.fields.is_empty()),
            _ => false,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DescVariant {
    pub name: String,
    pub record: bool,
    pub fields: Vec<DescField>,
}

/// What a compiled program is run for.
///
/// `entry: Option<usize>` and `tests: Vec<TestEntry>` were two fields, so a
/// program with both was representable and the backend emitted the entry
/// epilogue *and* the test harness into one artifact; a program with neither
/// was an artifact with no roots at all, which the minifier then deleted
/// entirely. `Roots` already said there were exactly two cases; this is the
/// output side of the same statement.
pub enum ProgramRoots {
    Main(FuncIdx),
    Tests(Vec<TestEntry>),
}

impl ProgramRoots {
    pub fn tests(&self) -> &[TestEntry] {
        match self {
            ProgramRoots::Main(_) => &[],
            ProgramRoots::Tests(t) => t,
        }
    }
}

pub struct Program {
    pub funcs: Vec<Func>,
    pub roots: ProgramRoots,
    pub descriptors: Vec<Desc>,
    /// The path of the module that declares each descriptor's type, at the
    /// same index as [`Program::descriptors`]. `None` for a type with no
    /// declaring module of its own: `[T]`, a tuple, `Option`, an opaque.
    ///
    /// It is recorded here because `middle::derives` runs with a `Program` and
    /// no `Tables`, and a derived function has to be told which codegen unit it
    /// belongs in — the unit of the type it was derived for. Without it every
    /// derived function in the program lands in `root` (ARCHITECTURE.md §5.1),
    /// which is then invalidated by any new derive anywhere.
    pub desc_modules: Vec<Option<String>>,
    /// Which descriptor describes a type, so the backend can compile a
    /// structural operation *at* a type instead of calling a runtime walker
    /// that rediscovers the shape on every element.
    pub desc_index: HashMap<Ty, usize>,
    /// Number of effect slots each context type carries, in binding order.
    pub ctx_layouts: HashMap<CtxTypeId, Vec<TraitId>>,
    /// What every declared type is made of. See [`Shapes`].
    pub shapes: Shapes,
    /// The stylesheet the static `ui/style` literals in this program extracted
    /// to, merged and deduped across every module it links.
    ///
    /// It travels on the program because it is *part of the artifact*: the
    /// JavaScript backend hands it to `mount`, which puts it in the document.
    /// Empty for every program that never styles anything, which is every
    /// program that is not a user interface.
    pub stylesheet: String,
    /// Whether any style in this program can reach the **inline tier** — one
    /// property lowered to a `style=` declaration at run time, rather than to
    /// a class the compiler put in the sheet.
    ///
    /// It travels on the program for the reason [`Program::stylesheet`] does,
    /// and it exists because the machinery is 3.5 KB that dead-code
    /// elimination cannot see: the runtime's collector names the lowering
    /// unconditionally, so *every* user interface paid for the tier and a
    /// program whose styles are all static paid for it twice over. The backend
    /// binds the lowering through a hole in the runtime when this is true and
    /// leaves the hole empty when it is false, which is the same mechanism
    /// `$ui_sheet` already uses and is what makes the tier droppable.
    ///
    /// [`crate::compiler::semantics::styles::Reached::inline`] is what decides
    /// it, and says why an over-approximation is the safe side.
    pub inline_styles: bool,
    /// Whether this program can build a `ui/theme` `Theme`.
    ///
    /// The same shape as [`Program::inline_styles`] and for the same reason:
    /// `mount` installs themes unconditionally, so 1.7 KB of resolution,
    /// rendering and switching shipped in every user interface — including one
    /// with no design tokens at all, which can only ever hand `mount` an empty
    /// list.
    pub themes: bool,
}

/// What every declared type is made of, in a form a pass holding no `Tables`
/// can still substitute into.
///
/// Recorded here for the reason [`Program::desc_modules`] is: the question is a
/// `Tables` question and the pass that asks it is handed a `Program`.
/// `middle::rc` decides whether a value carries a reference count, and
/// `middle::native` — the pipeline that runs it — takes no `Tables`; a type it
/// could not classify got no reference operations at all, which is a leaked
/// block per value of it. Both native backends ask *rc's* question rather than
/// the layout table's wherever they generate one half of a pair rc completes
/// (`cranelift::emit::Cx::rc_counted`), and they build their oracle from a
/// `Program` too, so the answer has to travel with the program or the two
/// halves disagree.
///
/// Keyed by constructor rather than by instantiated type, and holding the
/// *declared* field types with `Ty::Param` still in them: a type first spelled
/// after this pass has run — `derives` and `closures` both build them — is then
/// answered by substituting its own arguments, rather than being absent.
#[derive(Clone, Debug, Default)]
pub struct Shapes {
    /// By `TyConId`, in `Tables::tycons` order.
    pub cons: Vec<ConShape>,
    /// By `CtxTypeId`: the type of each effect binding, in binding order.
    pub ctxs: Vec<Vec<Ty>>,
}

/// What one type constructor is made of.
#[derive(Clone, Debug)]
pub enum ConShape {
    Prim(Prim),
    /// A struct's declared fields, or every variant's fields concatenated. A
    /// type carries a count when *any* variant's field does, so the union over
    /// variants is the accumulation the question wants and the tags are not
    /// part of it.
    Fields(Vec<Ty>),
}

/// [`Shapes`] for the whole compilation, which is every declared type rather
/// than only the reached ones: a shape is two clones of a field list, the
/// question is asked at types no body names, and a table with a hole in it is
/// the defect this exists to close.
fn shapes_of(tables: &Tables) -> Shapes {
    let cons = tables
        .tycons
        .iter()
        .map(|c| match &c.def {
            TyDef::Prim(p) => ConShape::Prim(*p),
            TyDef::Struct { fields, .. } => {
                ConShape::Fields(fields.iter().map(|f| f.ty.clone()).collect())
            }
            TyDef::Enum { variants } => ConShape::Fields(
                variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty.clone())).collect(),
            ),
        })
        .collect();
    let ctxs = tables
        .ctx_types
        .iter()
        .map(|c| c.bindings.iter().map(|(_, t)| t.clone()).collect())
        .collect();
    Shapes { cons, ctxs }
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
    desc_modules: Vec<Option<String>>,
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
        index: HashMap::default(),
        queue: Vec::new(),
        descriptors: Vec::new(),
        desc_modules: Vec::new(),
        desc_index: HashMap::default(),
        ctx_layouts: HashMap::default(),
        module_paths,
    };

    let program_roots = match roots {
        Roots::Main(f) => ProgramRoots::Main(FuncIdx(m.request(Key::Fn(f, Vec::new())) as u32)),
        Roots::Tests => {
            let mut tests = Vec::new();
            for (i, case) in m.checked.tests.iter().enumerate() {
                let idx = m.request(Key::Test(i));
                tests.push(TestEntry {
                    name: case.name.clone(),
                    func: FuncIdx(idx as u32),
                    module: m.module_paths.get(case.module.index()).cloned().unwrap_or_default(),
                    span: case.span,
                });
            }
            ProgramRoots::Tests(tests)
        }
    };

    // Instantiation order follows the worklist, which follows source order, so
    // two builds of the same commit produce identical output.
    while let Some((key, slot)) = m.queue.pop() {
        m.build(key, slot);
    }

    let shapes = shapes_of(&checked.tables);
    // What the program's styles are, and whether it has a theme at all. Asked
    // of the built functions rather than of the source, because a library's
    // unused styles are exactly what must not ship — and because the two flags
    // are about what the *artifact* can reach, which is what dead-code
    // elimination is about.
    //
    // One walk for both style questions; a second for themes, because the two
    // are different type constructors and a compilation that loaded one may
    // not have loaded the other. Both cost nothing for a program that is not a
    // user interface: neither constructor exists, so neither walk starts.
    let mut reached = crate::compiler::semantics::styles::Reached::default();
    let mut themes = false;
    for f in &mut m.funcs {
        let FuncKind::Body(body) = &mut f.kind else { continue };
        if let Some(style_con) = checked.style_con {
            crate::compiler::semantics::styles::collect(body, style_con, &mut reached);
        }
        if let Some(theme_con) = checked.theme_con {
            themes = themes
                || crate::compiler::semantics::styles::builds_a_theme(body, theme_con);
        }
    }
    Program {
        funcs: m.funcs,
        roots: program_roots,
        descriptors: m.descriptors,
        desc_modules: m.desc_modules,
        desc_index: m.desc_index,
        ctx_layouts: m.ctx_layouts,
        shapes,
        // Merged here rather than by each caller, so that `buri build` and
        // `buri test` cannot disagree about what a program's styles are.
        stylesheet: crate::compiler::semantics::styles::stylesheet(
            &checked.styles,
            &reached.classes,
        ),
        inline_styles: reached.inline,
        themes,
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

    /// The function a slot names.
    ///
    /// Every slot in this pass came out of `request`, which mints one by
    /// pushing onto `funcs`, and nothing ever removes a row, so the slot is an
    /// index this table has.
    fn func_mut(&mut self, slot: usize) -> &mut Func {
        self.funcs.get_mut(slot).or_ice("every function slot was minted by `request`, which makes it by pushing onto `funcs`")
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
            kind: FuncKind::Unbuilt,
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
                let case = self
                    .checked
                    .tests
                    .get(*i)
                    .or_ice("a test key holds the index `run` enumerated `checked.tests` with");
                // Qualified with the declaring module, because a debug name is
                // what `lower::unit_name` reads a codegen unit out of and a
                // test's title is arbitrary prose: `test "parses a: b"` put its
                // body in a unit called `a`. The module prefix is the first
                // colon, so a title's own punctuation is past the split.
                let module = self
                    .module_paths
                    .get(case.module.index())
                    .cloned()
                    .unwrap_or_else(|| "core".into());
                (format!("test${i}"), format!("{module}:{}", case.name), case.span)
            }
        }
    }

    fn build(&mut self, key: Key, slot: usize) {
        match key {
            Key::Fn(f, targs) => self.build_fn(f, targs, slot),
            Key::CtxCtor(c) => {
                let Some(checked) = self.tables().ctx_decl(c).checked else { return };
                let ctor = checked.ctor;
                let Some(body) = self.checked.bodies.get(&ctor) else { return };
                let mut b = body.clone();
                b.expr = self.rewrite(b.expr, &[]);
                let f = self.func_mut(slot);
                f.params = b.params;
                f.locals = b.locals;
                f.set_body(b.expr);
            }
            Key::Test(i) => {
                let fid = self
                    .checked
                    .tests
                    .get(i)
                    .or_ice("a test key holds the index `run` enumerated `checked.tests` with")
                    .func;
                let Some(body) = self.checked.bodies.get(&fid) else { return };
                let mut b = body.clone();
                b.expr = self.rewrite(b.expr, &[]);
                let f = self.func_mut(slot);
                f.params = b.params;
                f.locals = b.locals;
                f.set_body(b.expr);
            }
        }
    }

    fn build_fn(&mut self, f: FnId, targs: Vec<Ty>, slot: usize) {
        let info = self.tables().fun(f).clone();
        if info.intrinsic {
            let key = self.intrinsic_key(&info, &targs);
            // The instantiated parameter types, which only the descriptor
            // choice below reads. They used to live on `Func` as a second
            // vector parallel to `params`, filled on this path and left empty
            // on the body path, and nothing outside this function read them.
            let param_types: Vec<Ty> =
                info.params.iter().map(|p| substitute(&p.ty, &targs, None)).collect();
            let locals: Vec<typed::Local> = info
                .params
                .iter()
                .map(|p| typed::Local {
                    name: p.name.clone(),
                    ty: substitute(&p.ty, &targs, None),
                    span: p.span,
                })
                .collect();
            let ret = substitute(&info.ret, &targs, None);
            let f = self.func_mut(slot);
            f.kind = FuncKind::Intrinsic(key.clone());
            f.locals = locals;
            f.params = (0..info.params.len()).map(|i| LocalId(i as u32)).collect();
            f.ret = ret;
            if key == "testing_assert.report" || key == "testing_assert.failExpected" {
                if let Some(t) = param_types.get(2).or_else(|| param_types.get(1)).cloned() {
                    let desc = self.descriptor(&t);
                    self.func_mut(slot).desc = Some(desc);
                }
            }
            // `json.decode` is the one operation whose subject is in neither a
            // parameter nor the receiver: it is asked for a `T` and handed a
            // `Json`. `T` is the first type argument, and the descriptor of it
            // is the whole of what the runtime needs to read one.
            if key == "json.decode" {
                if let Some(t) = targs.first().cloned() {
                    let desc = self.descriptor(&t);
                    self.func_mut(slot).desc = Some(desc);
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
        let f = self.func_mut(slot);
        f.params = b.params;
        f.locals = b.locals;
        f.set_body(b.expr);
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
            ExprKind::CallFn { func, args } => {
                // The one place the two index spaces meet: a declaration and
                // its type arguments in, one concrete function out.
                let typed::Callee::Decl { id, targs: call_targs } = func else {
                    return typed::Expr::new(ExprKind::Error, e.ty, e.span);
                };
                let resolved: Vec<Ty> =
                    call_targs.iter().map(|t| substitute(t, targs, None)).collect();
                let args = self.rewrite_all(args, targs);
                let slot = self.request(Key::Fn(id, resolved));
                ExprKind::CallFn { func: typed::Callee::Func(FuncIdx(slot as u32)), args }
            }
            ExprKind::CallTrait { trait_id, method, recv, targs: mt, args } => {
                let recv = substitute(&recv, targs, None);
                let mt: Vec<Ty> = mt.iter().map(|t| substitute(t, targs, None)).collect();
                let args = self.rewrite_all(args, targs);
                self.resolve_trait_call(trait_id, method, recv, mt, args, e.span)
            }
            ExprKind::FnRef(callee) => {
                let typed::Callee::Decl { id, targs: ft } = callee else {
                    return typed::Expr::new(ExprKind::Error, e.ty, e.span);
                };
                let resolved: Vec<Ty> = ft.iter().map(|t| substitute(t, targs, None)).collect();
                let slot = self.request(Key::Fn(id, resolved));
                ExprKind::FnRef(typed::Callee::Func(FuncIdx(slot as u32)))
            }
            ExprKind::CtxCall { decl } => {
                let slot = self.request(Key::CtxCtor(decl));
                ExprKind::CallFn {
                    func: typed::Callee::Func(FuncIdx(slot as u32)),
                    args: Vec::new(),
                }
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
                    .map(|p| match p {
                        typed::TemplatePart::Text(t) => typed::TemplatePart::Text(t),
                        typed::TemplatePart::Hole(h) => {
                            typed::TemplatePart::Hole(self.rewrite(h, targs))
                        }
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
            return self.resolve_trait_call(trait_id, method, impl_ty, method_targs, args, span);
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
                .with_fix("annotate the call with type arguments, as in `f<Str>(x)`"),
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
        if imp.is_derived() {
            return self.structural_call(trait_id, method, &recv, args, span);
        }

        // `None` covers an out-of-range slot and a method the `impl` never
        // supplied; both are already diagnosed, and both are poison here.
        let Some(fid) = imp.method(method) else { return ExprKind::Error };
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
        ExprKind::CallFn { func: typed::Callee::Func(FuncIdx(slot as u32)), args }
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
            // `show` and `toJson` each take a context they do not use here:
            // rendering, and building the tree, are the runtime's. Drop it so
            // the intrinsic sees value and descriptor only.
            ("Show", _) => ExprKind::Intrinsic {
                name: "structuralShow".into(),
                targs: Vec::new(),
                args: value_and_descriptor(&all),
            },
            ("ToJson", _) => ExprKind::Intrinsic {
                name: "structuralToJson".into(),
                targs: Vec::new(),
                args: value_and_descriptor(&all),
            },
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
        let [field] = fields.as_slice() else {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`{}` derives `{op}` only for a single-field struct", tycon.name),
                )
                .with_fix("write the `impl` by hand, or wrap exactly one value")
                .with_note("an arithmetic newtype wraps exactly one value"),
            );
            return ExprKind::Error;
        };
        let targs = match recv {
            Ty::Con(_, a) => a.clone(),
            _ => Vec::new(),
        };
        let inner = substitute(&field.ty, &targs, None);
        // A derived arithmetic operator compiles to a primitive operation on
        // the wrapped value, so the wrapped value has to *be* a primitive.
        // Without this the backend used to default the missing type to `I64`
        // and emit integer arithmetic on whatever was inside.
        let Some(prim) = self.tables().as_prim(&inner) else {
            let shown =
                crate::compiler::semantics::types::show(self.tables(), None, &[], &inner);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "`{}` derives `{op}` only for a struct wrapping a number, not `{shown}`",
                        tycon.name
                    ),
                )
                .with_fix("write the `impl` by hand, or wrap a numeric type")
                .with_note("a derived arithmetic operator is the operation on the wrapped value"),
            );
            return ExprKind::Error;
        };
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

    /// The path of the module a type is declared in, where it has one.
    ///
    /// `[T]`, a tuple and an `Option` payload are structural rather than
    /// declared, so they answer `None` and the functions derived for them stay
    /// in `root`.
    fn declaring_module(&self, ty: &Ty) -> Option<String> {
        let con = ty.head()?;
        let module = self.tables().tycon(con).module;
        self.module_paths.get(module.index()).cloned()
    }

    /// Interns a runtime type descriptor. Field names and variant names are
    /// what `show` needs; `eq` and `compare` need the shape.
    fn descriptor(&mut self, ty: &Ty) -> usize {
        if let Some(i) = self.desc_index.get(ty) {
            return *i;
        }
        // Reserve the slot first, so a recursive type terminates.
        let slot = self.descriptors.len();
        self.descriptors.push(Desc::Reserved);
        self.desc_modules.push(self.declaring_module(ty));
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
                        let described: Vec<DescField> = fields
                            .iter()
                            .map(|f| {
                                let t = substitute(&f.ty, args, None);
                                DescField { name: f.name.clone(), ty: self.descriptor(&t) }
                            })
                            .collect();
                        Desc::Struct {
                            name: tycon.name.clone(),
                            record: *record,
                            fields: described,
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
                        let vs: Vec<DescVariant> = variants
                            .iter()
                            .map(|v| {
                                let fields: Vec<DescField> = v
                                    .fields
                                    .iter()
                                    .map(|f| {
                                        let t = substitute(&f.ty, args, None);
                                        DescField {
                                            name: f.name.clone(),
                                            ty: self.descriptor(&t),
                                        }
                                    })
                                    .collect();
                                DescVariant {
                                    name: v.name.clone(),
                                    record: v.record,
                                    fields,
                                }
                            })
                            .collect();
                        Desc::Enum { name: tycon.name.clone(), variants: vs }
                    }
                }
            }
            _ => Desc::Opaque("a value".into()),
        };
        *self
            .descriptors
            .get_mut(slot)
            .or_ice("the descriptor slot was reserved by pushing onto `descriptors`, which nothing shrinks") = desc;
        slot
    }
}

/// The subject and the descriptor of an intrinsic's argument list, dropping
/// everything between them. `structural_call` appends the descriptor last, so
/// this is never empty on the paths that call it.
fn value_and_descriptor(all: &[typed::Expr]) -> Vec<typed::Expr> {
    match (all.first(), all.last()) {
        (Some(value), Some(desc)) => vec![value.clone(), desc.clone()],
        _ => Vec::new(),
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
#[expect(
    clippy::indexing_slicing,
    reason = "`h % 36` is a digit of a base-36 numeral and the alphabet is the 36 characters it names"
)]
pub(super) fn short_hash(s: &str) -> String {
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
