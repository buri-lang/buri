//! Monomorphization.
//!
//! A generic body is checked once, polymorphically; instantiating it is a
//! codegen concern (SPEC 13.5). This pass walks out from the entry point and
//! produces one concrete function per `(function, type arguments)` pair it
//! reaches, resolving every trait and effect call to a direct one on the way.
//!
//! Because Buri has no dynamic dispatch — no trait objects, no virtual calls —
//! the call graph of direct calls is fully known once this runs, which is what
//! makes the tail-call elimination in `tail_calls.rs` exact.
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
/// (`stencil::emit`'s `rc_counted`), and they build their classifier from a
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
                let info = self.tables().fn_info(*f);
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
                // The whole path, file name and all. Taking `lib.buri` off the
                // way `intrinsic_key` does would be shorter and would collide:
                // `//lib/a.buri` and `//lib/a/lib.buri` are two modules that
                // may both exist — see the `a_module_beside_a_package_of_its_name`
                // case — and two functions on one symbol is a miscompile.
                // `intrinsic_key` may strip it because it only ever names a
                // standard library module, and none of those has a sibling.
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
                let rewritten = self.rewrite(b.expr, &[]);
                b.expr = self.leaving(i, rewritten);
                let f = self.func_mut(slot);
                f.params = b.params;
                f.locals = b.locals;
                f.set_body(b.expr);
            }
        }
    }

    /// A `test` body, with the runner's end-of-block hook after it.
    ///
    /// `buri_rt_test_leave(index)` is the other half of `core/host/testing`'s
    /// fault plan: *a fault whose call never happens fails the test*, which is a
    /// claim only something that outlives the block can check. Its twin
    /// `buri_rt_test_enter` is emitted by the two native test entry points
    /// instead, and the asymmetry is the difference between the two questions —
    /// `enter` answers *whether to run this block*, which is the runner's
    /// protocol and belongs where the blocks are called from, and this is the
    /// *program's* rule about what the block promised, which belongs to the
    /// block. Putting it here is also what gives all three backends one
    /// implementation: the JavaScript generator reads the same tree.
    ///
    /// A block that aborts never reaches the hook, which is the order a reader
    /// wants: the failed assertion is the failure, and an unused plan is a
    /// consequence of stopping early rather than a second complaint.
    fn leaving(&self, index: usize, body: typed::Expr) -> typed::Expr {
        let span = body.span;
        let leave = typed::Expr::new(
            ExprKind::Intrinsic {
                name: String::from("test.leave"),
                targs: Vec::new(),
                args: vec![typed::Expr::new(
                    ExprKind::Int(index as u128, false),
                    self.tables().prim(Prim::I64),
                    span,
                )],
            },
            Ty::Unit,
            span,
        );
        typed::Expr::new(
            ExprKind::Block {
                stmts: vec![typed::Stmt::Expr(body), typed::Stmt::Expr(leave)],
                tail: None,
            },
            Ty::Unit,
            span,
        )
    }

    fn build_fn(&mut self, f: FnId, targs: Vec<Ty>, slot: usize) {
        let info = self.tables().fn_info(f).clone();
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
    ///
    /// **The key does not carry `targs`, and that is deliberate**: one key
    /// names one runtime body, so every instantiation of a generic intrinsic
    /// shares a key and the type it was instantiated at does not survive the
    /// call. [`GENERIC_INTRINSICS`] is the rule for which keys that is allowed
    /// for, and this is where it is enforced.
    fn intrinsic_key(&self, info: &FnInfo, targs: &[Ty]) -> String {
        let module = self
            .module_paths
            .get(info.module.index())
            .cloned()
            .unwrap_or_else(|| "core/num/lib.buri".into());
        // The key names the *module*, not the file: `core/str/lib.buri` is
        // `str` and `ui/effect/lib.buri` is `ui_effect`, which is what every
        // backend's runtime table is written against. A module path names a
        // file, so the surface's name comes off here rather than at each of the
        // three tables.
        let stem = module.strip_suffix("/lib.buri").unwrap_or(&module);
        let short = stem.strip_prefix("core/").unwrap_or(stem).replace('/', "_");
        let key = match info.self_ty {
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
            _ => format!("{short}.{}", info.name),
        };
        // Only a bundled standard-library module can declare an operation the
        // runtime supplies — `resolve::record_intrinsic` guards on exactly this
        // — and a bodyless `fn` anywhere else is `declaration-without-a-body`
        // from the parser. So this cannot be reached by any input, which is
        // what makes an `ice` the right shape for it rather than a diagnostic:
        // there is nothing the author of the *program* could do about it.
        let generic = !targs.is_empty() || !info.generics.is_empty();
        if generic
            && crate::compiler::standard_library::find(&module).is_some()
            && !generic_intrinsic_allowed(&key)
        {
            crate::ice!(
                "the intrinsic `{key}` is generic, and the key the runtime is reached by \
                 carries no type arguments — so nothing tells the runtime what it was \
                 instantiated at; if that is intended, add `{key}` to `GENERIC_INTRINSICS` \
                 in `middle/monomorphize.rs` alongside the stride, glue or descriptor \
                 that makes it sound"
            );
        }
        key
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
                self.diags.push(Diagnostic::templated("unbound-effect", span));
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
                Diagnostic::templated("type-arguments-required", span)
                    .with_bind("trait", self.tables().trait_(trait_id).name.clone()),
            );
            return ExprKind::Error;
        };

        let Some(imp) = self.tables().impls.get(&(trait_id, con)).cloned() else {
            let t = self.tables().trait_(trait_id).name.clone();
            let c = self.tables().tycon(con).name.clone();
            self.diags.push(
                Diagnostic::templated("missing-conformance", span)
                    .with_bind("type", c.clone())
                    .with_bind("trait", t.clone())
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
        let counts = Counts {
            declared: self.tables().fn_info(fid).generics.len(),
            trait_generics: self.tables().trait_(trait_id).generics.len(),
        };
        let instance_targs = match instance_targs(&imp.head, &recv, counts, &method_targs) {
            Ok(targs) => targs,
            Err(err) => {
                let name = self.tables().trait_(trait_id).name.clone();
                self.diags.push(
                    Diagnostic::templated("type-arguments-required", span)
                        .with_bind("trait", name)
                        .with_note(err.note())
                        .with_fix(err.fix()),
                );
                return ExprKind::Error;
            }
        };

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
                    Diagnostic::templated("no-structural-derive", span)
                        .with_bind("trait", name.clone()),
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
                Diagnostic::templated("derive-operator-not-a-newtype", span)
                    .with_bind("type", tycon.name.clone())
                    .with_bind("operator", op),
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
                Diagnostic::templated("derive-operator-not-numeric", span)
                    .with_bind("type", tycon.name.clone())
                    .with_bind("operator", op)
                    .with_bind("wrapped", shown),
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

// ---------------------------------------------------------------------------
// Rebuilding an implementation's type arguments
// ---------------------------------------------------------------------------
//
// A trait method call carries the type arguments of the *trait's* declaration
// of the method; the function that has to be instantiated is the *impl's*, and
// the two have different generic lists:
//
//   trait   Fs                          declares          fn readFile<>(…)
//   TraitMethod.generics                is                trait generics ++ method generics
//   impl<C: Fs> Fs for ReadOnly<C>      supplies          FnInfo.generics = impl generics ++ method generics
//
// So the method's own arguments carry over unchanged, and the impl's have to
// be read back off the receiver — `ReadOnly<HostFs>` says `C = HostFs`. That is
// a match of the impl's head against the receiver, and it is written as one
// here. It used to be arithmetic: take the receiver's arguments, append the
// method's, pad with `Ty::Unit` and truncate to the declared count. That is
// right exactly when the trait has no generics of its own and the impl head is
// the type constructor applied to its parameters in order — true of everything
// in the tree today — and every other shape came out as a silently wrong
// instantiation rather than as a diagnostic.

/// The two counts the split needs, from the callee and from the trait.
#[derive(Clone, Copy, Debug)]
struct Counts {
    /// `FnInfo.generics.len()` of the function the `impl` supplies: the impl's
    /// own generics followed by the method's.
    declared: usize,
    /// `TraitInfo.generics.len()`: the trait's own parameters, which prefix
    /// every one of its methods' generic lists.
    trait_generics: usize,
}

/// Why an implementation's type arguments could not be rebuilt. Every variant
/// is a disagreement between an `impl` and the trait it implements, which is
/// what `signature-mismatch` will report at the declaration once it exists;
/// until then it is reported here, at the call that would have been
/// miscompiled.
#[derive(Clone, PartialEq, Eq, Debug)]
enum TargError {
    /// The trait declares parameters of its own. `impl T for X` has nowhere to
    /// write what they are bound to, so nothing here can recover them —
    /// `generic-effect-unsupported` refuses the declaration outright, and this
    /// is the same refusal seen from the far end.
    GenericTrait,
    /// The `impl` declares fewer generics in total than the method it supplies
    /// needs — its method's generic list disagrees with the trait's.
    MethodArity { declared: usize, method_own: usize },
    /// The receiver is not an instance of the `impl`'s head.
    HeadMismatch,
    /// The `impl` declares a generic the head never mentions, so no receiver
    /// can say what it is.
    Unbound(u32),
}

impl TargError {
    fn note(&self) -> String {
        match self {
            TargError::GenericTrait => {
                "a trait or effect with type parameters of its own has no way to say what an \
                 `impl` binds them to"
                    .into()
            }
            TargError::MethodArity { declared, method_own } => format!(
                "the implementation declares {declared} type parameters in all, and the method \
                 it supplies needs {method_own} of its own"
            ),
            TargError::HeadMismatch => {
                "the receiver is not an instance of the type the `impl` names".into()
            }
            TargError::Unbound(i) => format!(
                "the implementation's type parameter {i} is never mentioned in the type it is \
                 written for, so the receiver does not say what it is"
            ),
        }
    }

    /// What to do about it. The page's own fix — annotate the call — is the
    /// answer to the *other* thing it reports, a receiver whose type is not
    /// yet known; none of these is fixed at the call site at all.
    fn fix(&self) -> &'static str {
        match self {
            TargError::GenericTrait => {
                "declare the type parameters on the methods that need them rather than on the \
                 trait or effect itself"
            }
            TargError::MethodArity { .. } => {
                "give the method in the `impl` the type parameters its trait declares for it, \
                 and no others"
            }
            TargError::HeadMismatch => {
                "implement the trait for the type the receiver has, or pass a receiver of the \
                 type the `impl` is written for"
            }
            TargError::Unbound(_) => {
                "mention every one of the `impl`'s type parameters in the type it is written \
                 for, or drop the ones it does not use"
            }
        }
    }
}

/// The type arguments to instantiate an `impl`'s method at.
///
/// `head` is the `impl`'s head in its own generic scope (`ReadOnly<C>` as
/// `Con(ReadOnly, [Param(0)])`), `recv` the concrete receiver, and
/// `method_targs` what the call site instantiated the *trait's* declaration of
/// the method at — the trait's generics followed by the method's.
fn instance_targs(
    head: &Ty,
    recv: &Ty,
    counts: Counts,
    method_targs: &[Ty],
) -> Result<Vec<Ty>, TargError> {
    if counts.trait_generics != 0 {
        return Err(TargError::GenericTrait);
    }
    // With the trait's own list empty, every argument the call site supplied
    // is the method's, and the split point is known rather than guessed.
    let method_own = method_targs.len();
    let Some(impl_generics) = counts.declared.checked_sub(method_own) else {
        return Err(TargError::MethodArity { declared: counts.declared, method_own });
    };

    let mut bound: Vec<Option<Ty>> = vec![None; impl_generics];
    if !match_head(head, recv, &mut bound) {
        return Err(TargError::HeadMismatch);
    }
    let mut targs = Vec::with_capacity(counts.declared);
    for (i, slot) in bound.into_iter().enumerate() {
        match slot {
            Some(ty) => targs.push(ty),
            None => return Err(TargError::Unbound(i as u32)),
        }
    }
    targs.extend_from_slice(method_targs);
    Ok(targs)
}

/// Matches an `impl` head against a concrete receiver, binding the head's
/// parameters. Returns whether the two have the same shape; `bound` is
/// meaningful only when they do.
///
/// A parameter the head mentions twice — `impl<T> Pair<T, T>` — has to be
/// bound to the same type both times, which is why a bound slot is compared
/// rather than overwritten.
fn match_head(head: &Ty, recv: &Ty, bound: &mut [Option<Ty>]) -> bool {
    match (head, recv) {
        (Ty::Param(i), actual) => match bound.get_mut(*i as usize) {
            Some(slot @ None) => {
                *slot = Some(actual.clone());
                true
            }
            Some(Some(already)) => already == actual,
            // A parameter index past the impl's own generics: the head was
            // elaborated in a scope this call does not know about.
            None => false,
        },
        (Ty::Con(a, xs), Ty::Con(b, ys)) => {
            a == b && xs.len() == ys.len() && zip_match(xs, ys, bound)
        }
        (Ty::Array(a), Ty::Array(b)) => match_head(a, b, bound),
        (Ty::Tuple(xs), Ty::Tuple(ys)) => xs.len() == ys.len() && zip_match(xs, ys, bound),
        (Ty::Fn(xs, a), Ty::Fn(ys, b)) => {
            xs.len() == ys.len() && zip_match(xs, ys, bound) && match_head(a, b, bound)
        }
        (Ty::Unit, Ty::Unit) => true,
        // `Ty::Error` matches nothing on purpose: a poisoned receiver has
        // already been reported, and `imp.method` returning `None` is the arm
        // that catches it before this is reached.
        _ => false,
    }
}

fn zip_match(heads: &[Ty], recvs: &[Ty], bound: &mut [Option<Ty>]) -> bool {
    heads.iter().zip(recvs).all(|(h, r)| match_head(h, r, bound))
}

// ---------------------------------------------------------------------------
// Which intrinsics may be generic
// ---------------------------------------------------------------------------

/// The standard-library intrinsic keys whose declaration may be generic.
///
/// [`Monomorphizer::intrinsic_key`] builds a key out of the module, the type
/// and the name, and out of **nothing else** — the type arguments an instance
/// was requested at are not in it. One key names one runtime body, so every
/// instantiation of a generic intrinsic reaches the same body, and the type
/// stops at the boundary: `cli/runtime` is compiled once, against no Buri type
/// at all, and `runtime.js` is one function per key.
///
/// A generic intrinsic is therefore **type-erased at the call**, and anything
/// the runtime needs to know about the erased type has to arrive as a value.
/// There are exactly three ways it does, and each has a column of its own:
///
/// * the element **stride and retain glue** of
///   [`Extra::Element`](crate::compiler::backend::runtime_table::Extra::Element)
///   — every `core/list` entry;
/// * an **address**, for an argument whose type is a bare `T` and so has no
///   leaf list a C signature could name
///   ([`Entry::by_ref`](crate::compiler::backend::runtime_table::Entry));
/// * a **runtime descriptor** ([`Func::desc`]), which is the whole shape of a
///   type — `json.decode` and the two `core/testing/assert` entries.
///
/// A key whose erased parameter is only ever a **context** needs none of the
/// three: a `C: Alloc` appears in argument position, and rule 1 of the emission
/// order flattens an argument into its leaves whatever its type is. That is
/// most of this list, and it is still listed, because "the parameter happens to
/// be a context" is a fact about today's signature rather than a rule the next
/// author will re-derive.
///
/// **Anything not here is an `ice`**, from `intrinsic_key`. Adding a generic
/// intrinsic is therefore a deliberate two-line edit — the declaration, and a
/// row here — and the second line is where a reviewer asks which of the three
/// carries the type.
///
/// Sorted, and asserted sorted, so a reader can find a key and a duplicate is
/// visible. `num.<Prim>.show` and `num.<Prim>.toJson` are **not** here: they
/// are one fact about every primitive rather than twenty-six, and
/// [`prim_show_or_to_json`] states it once.
const GENERIC_INTRINSICS: &[&str] = &[
    // `core/bool` and `core/char`: `show` and `toJson` are minted by
    // `semantics::builtins` at every primitive and both name `C: Alloc`,
    // because rendering allocates. The type is in the key already.
    "bool.show",
    "bool.toJson",
    // `core/bytes`: four `C: Alloc` conversions. Every one of them answers a
    // `[U8]` or takes one, and the element type is fixed at `U8` — which is
    // why `runtime_table` gives them `Extra::None` and the stride is known.
    "bytes.f32ToBytes",
    "bytes.f64ToBytes",
    "bytes.fromUtf8",
    "bytes.toUtf8",
    "char.show",
    "char.toJson",
    // `core/host`'s two reactive impls, on WEB. `Ui.signal<T>`/`read<T>` name
    // a slot in the host's own table by `Int`, and the value never crosses in
    // a shape the runtime reads: `runtime.js` stores whatever it was handed.
    "host.HostUi.memo",
    "host.HostUi.read",
    "host.HostUi.signal",
    "host.HostUi.write",
    "host.HostWatch.read",
    // The one operation whose subject is in neither a parameter nor the
    // receiver: `decode` is asked for a `T` and handed a `Json`. `T` reaches
    // the runtime as a descriptor, built in `build_fn`.
    "json.decode",
    // `core/list`. Every entry here is generic in the element type, because
    // the whole module is `impl<T> [T]`, and every one of them gets its stride
    // and glue from `Extra::Element` (`runtime_table`) — `push` and `repeat`
    // additionally pass their bare `T` by address.
    "list.all",
    "list.any",
    "list.concat",
    "list.count",
    "list.drop",
    "list.empty",
    "list.filter",
    "list.filterCtx",
    "list.find",
    "list.findIndex",
    "list.flatten",
    "list.fold",
    "list.foldCtx",
    "list.foldResult",
    "list.foldResultCtx",
    "list.get",
    "list.join",
    "list.len",
    "list.map",
    "list.mapCtx",
    "list.push",
    "list.range",
    "list.repeat",
    "list.reverse",
    "list.slice",
    "list.sortBy",
    "list.take",
    "list.zip",
    // `Bounded`'s two, which take no argument at all and are generic in their
    // *return* type. Neither backend emits a call: both open-code the constant
    // from the destination's own width (`stencil/emit.rs`, `js/intrinsics.rs`),
    // so the erasure is repaired by there being no runtime call to erase into.
    "num.maxValue",
    "num.minValue",
    // `core/str`. Every one names `C: Alloc` for the block it builds and
    // nothing else; `Str` is three leaves at every instantiation, so there is
    // no element pair to supply and `runtime_table` gives them `Extra::None`.
    "str.chars",
    "str.concat",
    "str.format",
    "str.fromChars",
    "str.fromFloat",
    "str.fromInt",
    "str.lines",
    "str.padEnd",
    "str.padStart",
    "str.repeat",
    "str.replace",
    "str.show",
    "str.split",
    "str.splitAny",
    "str.toJson",
    "str.toLower",
    "str.toUpper",
    // The test runner's two. Both render values the program never rendered
    // itself, so both are given a descriptor in `build_fn` — the `desc` field
    // on `Func` exists for exactly these.
    "testing_assert.failExpected",
    "testing_assert.report",
    // `ui/effect` and `ui/testing`: the same reactive slots as `core/host`'s,
    // at the scope and at the headless test host. `ui_node.mount` and
    // `ui_testing.render` are generic in the *context* alone.
    "ui_effect.Scope.read",
    "ui_node.mount",
    "ui_testing.Headless.memo",
    "ui_testing.Headless.read",
    "ui_testing.Headless.signal",
    "ui_testing.Headless.write",
    "ui_testing.Observer.read",
    "ui_testing.render",
];

/// Whether a generic intrinsic named `key` is one the erasure has been thought
/// about for. See [`GENERIC_INTRINSICS`].
fn generic_intrinsic_allowed(key: &str) -> bool {
    GENERIC_INTRINSICS.contains(&key) || prim_show_or_to_json(key)
}

/// `num.I64.show`, `num.F64.toJson` and their siblings: the two generic methods
/// `semantics::builtins` mints at every primitive, for every primitive whose
/// defining module is `core/num` and whose key therefore carries the type.
///
/// Read off `Prim::all()` rather than written out, for the reason
/// `backend::intrinsic_keys::derive_key` is: the family is *every* primitive,
/// and a hand-written list of thirteen is a list that can be short by one.
/// Both methods name `C: Alloc` and nothing else — the type they are at is in
/// the key, which is what `short != "num"` in `intrinsic_key` arranges.
fn prim_show_or_to_json(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("num.") else { return false };
    let Some((ty, op)) = rest.split_once('.') else { return false };
    matches!(op, "show" | "toJson") && Prim::all().iter().any(|p| p.name() == ty)
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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Rebuilding an implementation's type arguments
    // -----------------------------------------------------------------------
    //
    // The inputs are two types, two counts and a list, so the cases are
    // written as those rather than as Buri source: the shapes that matter
    // include ones no program in the tree spells, which is the reason the old
    // arithmetic survived as long as it did.

    const P0: Ty = Ty::Param(0);
    const P1: Ty = Ty::Param(1);

    /// A type constructor id, named for what a test means by it.
    fn con(id: u32, args: Vec<Ty>) -> Ty {
        Ty::Con(TyConId(id), args)
    }

    fn int() -> Ty {
        con(1, vec![])
    }

    fn string() -> Ty {
        con(2, vec![])
    }

    fn counts(declared: usize, trait_generics: usize) -> Counts {
        Counts { declared, trait_generics }
    }

    /// Shape one: neither the `impl` nor the method is generic. Every `impl`
    /// in `core/host` is this — `impl Fs for HostFs`, called as
    /// `ctx.readFile(p)`.
    #[test]
    fn a_plain_impl_of_a_plain_method_instantiates_at_nothing() {
        let host_fs = con(10, vec![]);
        let got = instance_targs(&host_fs, &host_fs, counts(0, 0), &[]);
        assert_eq!(got, Ok(Vec::new()));
    }

    /// Shape two: the method has generics of its own and the `impl` has none.
    /// `impl Show for Order { fn show<C: Alloc>(self, ctx: C): Str }` — the
    /// call site's `C` carries over untouched.
    #[test]
    fn a_method_generic_carries_over_from_the_call_site() {
        let order = con(11, vec![]);
        let got = instance_targs(&order, &order, counts(1, 0), &[int()]);
        assert_eq!(got, Ok(vec![int()]));
    }

    /// Shape three: the `impl` head is generic and the method is not.
    /// `impl<C: Fs> Fs for ReadOnly<C>` in `core/testing/context`, reached as
    /// `ReadOnly<HostFs>`. The old arithmetic got this one right by
    /// coincidence — the receiver's arguments happened to be the impl's, in
    /// order.
    #[test]
    fn an_impl_generic_is_read_off_the_receiver() {
        let host_fs = con(10, vec![]);
        let head = con(12, vec![P0]);
        let recv = con(12, vec![host_fs.clone()]);
        let got = instance_targs(&head, &recv, counts(1, 0), &[]);
        assert_eq!(got, Ok(vec![host_fs]));
    }

    /// Shape four: both. The impl's generics come first and the method's
    /// second, which is the order `register_impl_body` builds `FnInfo.generics`
    /// in.
    #[test]
    fn an_impl_generic_and_a_method_generic_split_at_the_declared_count() {
        let head = con(12, vec![P0]);
        let recv = con(12, vec![int()]);
        let got = instance_targs(&head, &recv, counts(2, 0), &[string()]);
        assert_eq!(got, Ok(vec![int(), string()]));
    }

    /// The head need not be the constructor applied to its parameters in
    /// order, which is the assumption the arithmetic was built on. Nothing in
    /// the tree writes one of these yet, and the language admits them.
    #[test]
    fn a_head_that_reorders_or_nests_its_parameters_still_matches() {
        let head = con(13, vec![Ty::Array(Box::new(P1)), P0]);
        let recv = con(13, vec![Ty::Array(Box::new(string())), int()]);
        let got = instance_targs(&head, &recv, counts(2, 0), &[]);
        assert_eq!(got, Ok(vec![int(), string()]));
    }

    /// A head that pins an argument rather than abstracting over it —
    /// `impl Show for Pair<Int, T>` — binds one parameter, not two.
    #[test]
    fn a_partly_concrete_head_binds_only_what_it_abstracts_over() {
        let head = con(13, vec![int(), P0]);
        let recv = con(13, vec![int(), string()]);
        assert_eq!(instance_targs(&head, &recv, counts(1, 0), &[]), Ok(vec![string()]));

        let other = con(13, vec![string(), string()]);
        assert_eq!(instance_targs(&head, &other, counts(1, 0), &[]), Err(TargError::HeadMismatch));
    }

    /// One parameter used twice has to be bound to one type.
    #[test]
    fn a_parameter_the_head_repeats_is_bound_once() {
        let head = con(13, vec![P0, P0]);
        let same = con(13, vec![int(), int()]);
        assert_eq!(instance_targs(&head, &same, counts(1, 0), &[]), Ok(vec![int()]));

        let different = con(13, vec![int(), string()]);
        assert_eq!(
            instance_targs(&head, &different, counts(1, 0), &[]),
            Err(TargError::HeadMismatch)
        );
    }

    /// The case the `Ty::Unit` padding used to swallow: an `impl` generic the
    /// head never mentions. The old code produced `[C, Unit]` — the method's
    /// argument bound to the impl's parameter and the method's own left as
    /// unit — and compiled it.
    #[test]
    fn an_impl_generic_the_head_never_mentions_is_reported() {
        let point = con(11, vec![]);
        assert_eq!(
            instance_targs(&point, &point, counts(2, 0), &[int()]),
            Err(TargError::Unbound(0))
        );
    }

    /// A receiver of another type constructor entirely. `impls` is keyed by
    /// head constructor so this is unreachable from a call today; it is the
    /// property the match is asserting, and asserting it is what makes the
    /// key's guarantee checkable rather than assumed.
    #[test]
    fn a_receiver_of_another_type_does_not_match() {
        let head = con(12, vec![P0]);
        let recv = con(13, vec![int()]);
        assert_eq!(instance_targs(&head, &recv, counts(1, 0), &[]), Err(TargError::HeadMismatch));
    }

    /// The truncation the old code did: an `impl` whose method declares more
    /// generics than the trait's does. `signature-mismatch` will catch it at
    /// the declaration; until then it is caught here rather than papered over.
    #[test]
    fn an_impl_that_declares_too_few_generics_is_reported() {
        let order = con(11, vec![]);
        assert_eq!(
            instance_targs(&order, &order, counts(0, 0), &[int()]),
            Err(TargError::MethodArity { declared: 0, method_own: 1 })
        );
    }

    /// A trait with parameters of its own is refused rather than guessed at,
    /// from both ends: `generic-effect-unsupported` at the declaration, and
    /// this at the call a compiled-anyway program would have reached.
    #[test]
    fn a_generic_trait_is_refused() {
        let order = con(11, vec![]);
        assert_eq!(
            instance_targs(&order, &order, counts(1, 1), &[int()]),
            Err(TargError::GenericTrait)
        );
    }

    // -----------------------------------------------------------------------
    // Which intrinsics may be generic
    // -----------------------------------------------------------------------
    //
    // `intrinsic_key` reports a key outside the list with `ice`, which exits
    // the process — so these test the predicate it asks rather than the call,
    // which is also the only half a new key's author has to get right.

    /// The list, pinned. A key added to `GENERIC_INTRINSICS` and not here is a
    /// failing test, which is the point: erasing a *new* type at the runtime
    /// boundary should be a line in a review rather than a silent consequence
    /// of writing `fn f<T>(…);` in a standard-library module.
    ///
    /// Written as one string rather than as a second `&[&str]` so that the
    /// duplication is legible at a glance instead of being seventy lines that
    /// look like the constant and might not be it.
    const PINNED: &str = "\
        bool.show bool.toJson \
        bytes.f32ToBytes bytes.f64ToBytes bytes.fromUtf8 bytes.toUtf8 \
        char.show char.toJson \
        host.HostUi.memo host.HostUi.read host.HostUi.signal host.HostUi.write \
        host.HostWatch.read \
        json.decode \
        list.all list.any list.concat list.count list.drop list.empty \
        list.filter list.filterCtx list.find list.findIndex list.flatten \
        list.fold list.foldCtx list.foldResult list.foldResultCtx list.get \
        list.join list.len list.map list.mapCtx list.push list.range \
        list.repeat list.reverse list.slice list.sortBy list.take list.zip \
        num.maxValue num.minValue \
        str.chars str.concat str.format str.fromChars str.fromFloat \
        str.fromInt str.lines str.padEnd str.padStart str.repeat str.replace \
        str.show str.split str.splitAny str.toJson str.toLower str.toUpper \
        testing_assert.failExpected testing_assert.report \
        ui_effect.Scope.read ui_node.mount \
        ui_testing.Headless.memo ui_testing.Headless.read \
        ui_testing.Headless.signal ui_testing.Headless.write \
        ui_testing.Observer.read ui_testing.render";

    #[test]
    fn the_generic_intrinsics_are_exactly_these() {
        let pinned: Vec<&str> = PINNED.split_whitespace().collect();
        assert_eq!(GENERIC_INTRINSICS, pinned.as_slice());
    }

    /// Sorted and without repeats, so that a reader can find a key and a
    /// second copy of one is visible in the diff rather than harmless.
    #[test]
    fn the_list_is_sorted_and_has_no_duplicates() {
        let mut sorted: Vec<&str> = GENERIC_INTRINSICS.to_vec();
        sorted.sort_unstable();
        assert_eq!(GENERIC_INTRINSICS, sorted.as_slice(), "the list is not in order");
        sorted.dedup();
        assert_eq!(sorted.len(), GENERIC_INTRINSICS.len(), "the list repeats a key");
    }

    /// The whole of the slice: a generic intrinsic nobody wrote a rule for is
    /// refused. Each of these is a plausible next entry in the module it names
    /// — `core/list` gaining a `chunk`, `core/tasks` gaining the `parallel`
    /// the concurrency work wants — and each would be type-erased at the
    /// boundary with nothing carrying the element type across.
    ///
    /// **`host.HostTasks.parallel` is refused on purpose, and it exists.**
    /// `core/host` declares `impl Tasks for HostTasks` today, and the method is
    /// generic in `A` and `B`. It stays off the list because none of the three
    /// carriers has been chosen for it: there is no runtime body, no stride and
    /// no descriptor, so an entry here would be this file asserting an erasure
    /// is sound when nobody has yet made it sound. Nothing reaches the key —
    /// no platform grants `Tasks`, so no program can construct a `HostTasks` —
    /// and the day one does, the ice names the exact question to answer. The
    /// entry lands in the same commit as the carrier, which is the discipline
    /// the whole list exists for.
    #[test]
    fn a_generic_intrinsic_outside_the_list_is_refused() {
        for key in [
            "list.chunk",
            "tasks.parallel",
            "host.HostTasks.parallel",
            "str.splitInto",
            "host.HostUi.observe",
            "json.encodeAs",
            // Near-misses of entries that *are* on the list: the rule matches
            // a key, not a module or a name.
            "list.mapCtx2",
            "listx.map",
            "ui_testing.Headless.observe",
        ] {
            assert!(!generic_intrinsic_allowed(key), "`{key}` was let through");
        }
    }

    #[test]
    fn every_listed_key_is_allowed() {
        for key in GENERIC_INTRINSICS {
            assert!(generic_intrinsic_allowed(key), "`{key}` is on the list and was refused");
        }
    }

    /// The primitive family is stated once, over `Prim::all()`, so it cannot
    /// be short by a type the way a written-out list of thirteen can.
    #[test]
    fn show_and_to_json_are_allowed_at_every_primitive() {
        for p in Prim::all() {
            let name = p.name();
            assert!(generic_intrinsic_allowed(&format!("num.{name}.show")), "{name} show");
            assert!(generic_intrinsic_allowed(&format!("num.{name}.toJson")), "{name} toJson");
        }
    }

    /// And says nothing about anything else in `core/num`. `hash`, `eq` and
    /// `compare` are minted with no generics at all, so they never reach the
    /// check — this pins that widening the family would take an edit.
    #[test]
    fn the_primitive_family_is_those_two_methods_and_no_others() {
        for key in ["num.I64.hash", "num.I64.eq", "num.F64.compare", "num.I64.showOff"] {
            assert!(!generic_intrinsic_allowed(key), "`{key}` was let through");
        }
        // A type that is not a primitive, spelled into the same shape.
        assert!(!generic_intrinsic_allowed("num.Decimal.show"));
        // And the prefix alone is not enough.
        assert!(!generic_intrinsic_allowed("num.show"));
        assert!(!generic_intrinsic_allowed("numeric.I64.show"));
    }

    /// Every rejection says something specific enough to act on.
    #[test]
    fn every_rejection_has_a_note() {
        for err in [
            TargError::GenericTrait,
            TargError::MethodArity { declared: 0, method_own: 1 },
            TargError::HeadMismatch,
            TargError::Unbound(0),
        ] {
            assert!(!err.note().is_empty(), "{err:?} has no note");
            assert!(!err.fix().is_empty(), "{err:?} has no fix");
        }
    }
}
