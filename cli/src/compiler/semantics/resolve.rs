//! Name resolution and signature elaboration.
//!
//! Runs in phases, and the phase split is the compile-speed argument of
//! SPEC 13 made concrete:
//!
//! 0. Register the primitives, their methods, and their trait impls.
//! 1. Register every module's own declarations, names and arities only.
//! 2. Resolve imports and re-exports into per-module scopes.
//! 3. Elaborate the types in every signature. This is a module's entire
//!    inter-module surface (13.4).
//! 4. Register `impl` and `derive`, and build the method table.
//! 4½. Check the `ctx` rule, which needs both of the two above finished: which
//!    positions of a type constructor hand a value back (step 3's type bodies)
//!    and which types implement an effect (step 4's conformances).
//! 5. Check each function body, independently and in any order (13.3).
//!
//! Only step 5 needs inference, and it never crosses a function boundary,
//! because top-level signatures are mandatory.

use crate::build::workspace::{PackageId, Workspace};
use crate::compiler::modules::{Loaded, Role};
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::*;
use crate::compiler::standard_library;
use crate::diagnostics::{Diagnostic, Diagnostics, FileId, Invariant as _, Span, SecondarySpan};
use crate::parsing::flat::{self, TypeId};
use crate::parsing::tree;
use crate::hash::{Map as HashMap, Set as HashSet};
use std::collections::BTreeSet;

/// What a name in scope refers to.
#[derive(Clone, Debug)]
pub enum Sym {
    Ty(TyConId),
    Fn(FnId),
    Trait(TraitId),
    Const(ConstId),
    Context(ContextDeclId),
    /// A `import * as list` namespace.
    Namespace(ModuleId),
    /// Several methods of that name exist on different types. Usable as a
    /// method, ambiguous as a free function — which is the shape `core/num`'s
    /// per-type conversions would have if they were written out.
    Overloaded(Vec<FnId>),
    /// A method declared in an `impl` block, carrying its receiver type as
    /// written. The name exists in the module's scope only so that a library's
    /// `lib.buri` can put it on the surface; a method is never callable as a
    /// free function, because it is reached through a receiver.
    Method(String),
    /// A transparent type alias, carrying the module that declares it and the
    /// name it is declared under. Both are needed because the alias expands in
    /// its declaring module, wherever an import or a rename carried it to.
    Alias(ModuleId, String),
}

#[derive(Default)]
pub struct ModuleScope {
    /// Everything visible unqualified inside this module.
    pub names: HashMap<String, Sym>,
    /// What this module publishes.
    pub exports: HashMap<String, Sym>,
    /// Names declared in this module's own source, before imports.
    pub own: HashMap<String, Sym>,
    /// Namespace imports, by local name.
    pub namespaces: HashMap<String, ModuleId>,
}

/// Which function bodies an analysis is asked to type-check.
///
/// Everything another body can *see* — signatures, type definitions, traits,
/// impls, module-level `let`s, `context` declarations — is elaborated for the
/// whole closure either way. This chooses only how much of step 5 runs, and it
/// is what lets an editor query cost the file under the cursor rather than the
/// repository.
#[derive(Clone, Debug)]
pub enum Bodies {
    /// Every body in the closure. What a build, a lint pass and a published
    /// diagnostic all need.
    All,
    /// Only the bodies written in these files. [`Checked::bodies`] simply has
    /// no entry for the rest.
    In(Vec<FileId>),
}

pub struct Checked {
    pub tables: Tables,
    pub scopes: Vec<ModuleScope>,
    pub bodies: HashMap<FnId, typed::Body>,
    pub consts: HashMap<ConstId, typed::Expr>,
    /// `main`, when this compilation has one.
    pub entry: Option<FnId>,
    pub tests: Vec<TestCase>,
    /// The stylesheet rules this compilation's static `ui/style` literals
    /// extracted to, in walk order.
    ///
    /// A `Vec` beside `tests` rather than a cache sidecar, and the two are the
    /// same shape for the same reason: both are things a module's own compile
    /// discovers, and both are merged by whoever links. A class here names
    /// itself (`semantics::styles`), so merging is a dedupe and never a
    /// renumbering — which is what keeps compilation local.
    pub styles: Vec<crate::compiler::semantics::styles::StyleRule>,
    /// `ui/style`'s `Style`, when this compilation loaded it. The link step
    /// needs it to tell which of the rules above a program actually reaches.
    pub style_con: Option<TyConId>,
    /// `ui/theme`'s `Theme`, when this compilation loaded it. Beside
    /// `style_con` and for the same kind of reason: the link step needs it to
    /// tell whether a program can build one at all, which is what lets the
    /// backend leave the theme half of the runtime out of one that cannot.
    pub theme_con: Option<TyConId>,
    /// Per package, the set of names its `lib.buri` puts on the surface. The
    /// checker needs it to filter method resolution; `dead-code` needs it to
    /// ask the opposite question — what is exported and reaches nobody.
    pub surfaces: HashMap<PackageId, HashSet<String>>,
    /// Every `let ctx = ...` written where a context may not be built, in walk
    /// order. Binding the name is legal, so this is not a diagnostic — it is
    /// what `ctx-rebinding` reports, and the checker records it because the
    /// checker is the only pass that knows where the line falls (SPEC 11.3).
    pub ctx_rebindings: Vec<Span>,
}

/// The traits `derive` can generate. Derivation is a fold over one type
/// definition, which is what these have in common and what nothing else does.
///
/// Read by `register_derive` to check a `derive`, and by `expressions.rs` to
/// tell a method that is missing because nobody wrote it from one that is
/// missing because the type did not derive the trait it comes from.
pub const DERIVABLE: &[&str] = &[
    "Eq", "Ord", "Show", "Hash", "ToJson", "FromJson", "Add", "Sub", "Mul", "Div", "Rem", "Neg",
];

#[derive(Clone, Debug)]
pub struct TestCase {
    pub name: String,
    pub module: ModuleId,
    /// The synthetic function holding the test's body.
    pub func: FnId,
    pub span: Span,
}

pub struct Checker<'a> {
    pub loaded: &'a Loaded,
    pub ws: Option<&'a Workspace>,
    pub diags: &'a mut Diagnostics,
    pub tables: Tables,
    pub scopes: Vec<ModuleScope>,
    pub bodies: HashMap<FnId, typed::Body>,
    pub const_values: HashMap<ConstId, typed::Expr>,
    pub entry: Option<FnId>,
    pub tests: Vec<TestCase>,
    /// The synthetic module the primitives are declared in.
    pub prim_module: ModuleId,
    /// Per package, the set of names its `lib.buri` puts on the surface. A
    /// method call from outside a library resolves only to these.
    pub surfaces: HashMap<PackageId, HashSet<String>>,
    /// See [`Checked::ctx_rebindings`].
    pub ctx_rebindings: Vec<Span>,
    /// Traits by well-known name, for operators and `derive`.
    pub known_traits: HashMap<String, TraitId>,
    /// Enums by well-known name.
    pub known_types: HashMap<String, TyConId>,
    /// The three of those the body checker asks about by name on hot paths —
    /// `?`, `??`, and every comparison. They are settled by
    /// `register_known_names` before any body is looked at, so asking the map
    /// again was a string hash and a probe per `let` and per operator.
    pub option_con: Option<TyConId>,
    pub result_con: Option<TyConId>,
    pub order_con: Option<TyConId>,
    /// The signatures whose `ctx` rule is still to be checked, in declaration
    /// order — which is the order the check used to run in, so the diagnostics
    /// are the same ones in the same sequence. Held rather than checked in
    /// place because rule 26 asks two questions no earlier pass can answer
    /// (see `Checker::run`).
    pending_ctx_rules: Vec<PendingCtxRule>,
    /// Guards re-export cycles.
    resolving: Vec<(ModuleId, String)>,
    /// The aliases being expanded, outermost first, each as the module that
    /// declares it and the name it is declared under. Guards alias cycles, the
    /// way `resolving` guards re-export ones.
    expanding: Vec<(ModuleId, String)>,
    /// Every alias on a cycle already reported. A cycle is one mistake, so the
    /// second signature that names one of these gets the error type without a
    /// second diagnostic.
    cyclic_aliases: HashSet<(ModuleId, String)>,
    /// What `Self` stands for in the declaration being elaborated — a trait,
    /// an effect, or an `impl`, which are the only places it means anything.
    ///
    /// `Some(Ty::SelfTy)` inside a `trait` or `effect`, where the implementing
    /// type is not known yet and `Self` stays abstract until an `impl` supplies
    /// one. `Some(that type)` inside an `impl`'s methods, so a *written* `Self`
    /// in a signature resolves the same way the implicit type of a `self`
    /// parameter does — a `Ty::SelfTy` left in an `impl` method's `FnInfo` is
    /// substituted by nothing downstream and reaches `middle::layout` as a
    /// type it has no size for. `None` outside both, where `Self` is the
    /// mistake `self-type-outside-impl` reports.
    self_scope: Option<Ty>,
    /// Which bodies step 5 is asked for. [`Bodies::All`] unless a caller said
    /// otherwise.
    pub wanted: Bodies,
    /// The `context` declarations checking has already reached, whether it
    /// finished them or not.
    ///
    /// A declaration built from another (`context Deep { ..Base() }`) reads
    /// the base's *recorded* type, so the base has to have been checked first
    /// — and the order the ids were minted in is the order the modules were
    /// discovered in, which is not that order. So a use checks its declaration
    /// on demand (`expressions.rs`, `Static::Context`), and this is what keeps
    /// that from doing the work twice or from following a cycle round for
    /// ever: a declaration already in here answers with what it has, which for
    /// one still in progress is nothing.
    pub ctx_decls_reached: HashSet<ContextDeclId>,
}

/// A signature waiting for rule 26.
///
/// The rule reads a parameter list and the generics it was written against,
/// and three kinds of declaration carry one. A free `fn` and a method supplied
/// by an `impl` both have an [`FnInfo`]; a method *signature* in a `trait` or
/// `effect` body never becomes an `FnId` at all and is found by its position
/// in the trait's own table. Only a free `fn` can be `main`, so only that
/// variant carries the item index the `main` check needs to find its
/// `tree::FnDecl` again.
#[derive(Clone, Copy)]
enum PendingCtxRule {
    /// A `fn` at module scope, with the module and item index it was declared
    /// at.
    Free(FnId, ModuleId, u32),
    /// A method supplied by an `impl` block, whether it implements a trait or
    /// is one of the type's own.
    Method(FnId),
    /// A method signature in a `trait` or `effect` body: the trait, and the
    /// method's position in its `methods`.
    Signature(TraitId, usize),
}

impl<'a> Checker<'a> {
    pub fn new(
        loaded: &'a Loaded,
        ws: Option<&'a Workspace>,
        diags: &'a mut Diagnostics,
    ) -> Checker<'a> {
        let mut scopes = Vec::new();
        scopes.resize_with(loaded.modules.len(), ModuleScope::default);
        Checker {
            loaded,
            ws,
            diags,
            tables: Tables::default(),
            scopes,
            bodies: HashMap::default(),
            const_values: HashMap::default(),
            entry: None,
            tests: Vec::new(),
            prim_module: ModuleId(u32::MAX),
            surfaces: HashMap::default(),
            ctx_rebindings: Vec::new(),
            known_traits: HashMap::default(),
            known_types: HashMap::default(),
            option_con: None,
            result_con: None,
            order_con: None,
            pending_ctx_rules: Vec::new(),
            resolving: Vec::new(),
            expanding: Vec::new(),
            cyclic_aliases: HashSet::default(),
            self_scope: None,
            wanted: Bodies::All,
            ctx_decls_reached: HashSet::default(),
        }
    }

    /// Narrows step 5 to the bodies written in one set of files.
    pub fn checking(mut self, bodies: Bodies) -> Checker<'a> {
        self.wanted = bodies;
        self
    }

    /// The file a declaration's body is written in, for anything that has one.
    pub fn file_of(&self, ast: AstRef) -> Option<FileId> {
        ast.item().map(|(module, _)| self.module(module).file)
    }

    /// Whether step 5 is being asked for the bodies in this file.
    pub fn wants_file(&self, file: FileId) -> bool {
        match &self.wanted {
            Bodies::All => true,
            Bodies::In(files) => files.contains(&file),
        }
    }

    pub fn run(mut self) -> Checked {
        self.register_primitives();
        self.collect_declarations();
        self.withhold_ungranted_host_effects();
        self.resolve_scopes();
        self.register_known_names();
        self.elaborate_signatures();
        self.register_conformance();
        // Both of these read what the two passes above finished: the fixpoint
        // needs elaborated type bodies, and rule 26 needs to know which types
        // implement an effect. Asking either question from inside
        // `elaborate_signatures` — where the `ctx` rule used to be checked —
        // means asking it of a half-built table.
        self.tables.compute_variance();
        self.check_ctx_rules();
        self.register_primitive_methods();
        self.check_derives();
        self.compute_surfaces();
        self.check_module_rules();
        self.check_bodies();
        // Last, because it reads every checked body and rewrites the ones that
        // hold a static style. It must also run before `monomorphize` inlines a
        // constant, or a module-level `let` of `Style` would be extracted once
        // per use site instead of once.
        let only: Option<Vec<FileId>> = match &self.wanted {
            Bodies::All => None,
            Bodies::In(files) => Some(files.clone()),
        };
        let (styles, style_con) = crate::compiler::semantics::styles::run(
            self.loaded,
            &self.tables,
            &self.scopes,
            &mut self.bodies,
            &mut self.const_values,
            self.diags,
            only.as_deref(),
        );
        // `ui/theme`'s one opaque type, looked up the way `styles::run` looks
        // up `Style`: by module path in the loaded set, then by name in that
        // module's own scope. `None` for every compilation that did not load
        // the module, which is every program that is not a user interface.
        let theme_con = self
            .loaded
            .modules
            .iter()
            .position(|m| m.path == "ui/theme/lib.buri")
            .and_then(|i| self.scopes.get(i))
            .and_then(|s| s.own.get("Theme"))
            .and_then(|s| match s {
                Sym::Ty(id) => Some(*id),
                _ => None,
            });
        Checked {
            tables: self.tables,
            scopes: self.scopes,
            bodies: self.bodies,
            consts: self.const_values,
            entry: self.entry,
            tests: self.tests,
            styles,
            style_con,
            theme_con,
            surfaces: self.surfaces,
            ctx_rebindings: self.ctx_rebindings,
        }
    }

    /// A diagnostic whose wording lives on its page. What follows is
    /// `.bind(…)` for each `{placeholder}` the page names.
    pub fn templated(&mut self, code: &str, span: Span) -> &mut Diagnostic {
        self.diags.items.push(Diagnostic::templated(code, span));
        self.diags.items.last_mut().or_ice("the diagnostic just pushed is the last one")
    }

    /// One module's scope. `new` sizes this table to `loaded.modules`, which is
    /// the same table every `ModuleId` indexes, so the id is always in range.
    pub fn scope(&self, module: ModuleId) -> &ModuleScope {
        self.scopes.get(module.index()).or_ice("every ModuleId indexes the loaded module list")
    }

    fn scope_mut(&mut self, module: ModuleId) -> &mut ModuleScope {
        self.scopes.get_mut(module.index()).or_ice("every ModuleId indexes the loaded module list")
    }

    /// The borrow is `'a`, not `&self` — the modules live in `loaded`, which
    /// the checker only reads, so a caller can hold the syntax tree while it
    /// mutates the tables it is filling in. That is the difference between
    /// iterating a module's items and cloning them: every pass below walks
    /// `self.module(id).ast.items` while calling `&mut self` methods, and with
    /// a `&self` borrow the only way to do that is to deep-copy the whole
    /// tree — every body, every expression — once per pass and once per type
    /// alias lookup. It was more than half the wall time of a build.
    pub fn module(&self, id: ModuleId) -> &'a crate::compiler::modules::ModuleData {
        self.loaded
            .modules
            .get(id.index())
            .or_ice("every ModuleId was minted as an index into this list")
    }

    /// The flat tree of a module, borrowed for `'a` rather than for `&self`,
    /// for the reason [`Resolver::module`] gives: a name is the source under
    /// its span, so every read of one holds a borrow of the tree while the
    /// tables it is filling in are mutated.
    pub fn tree(&self, module: ModuleId) -> &'a crate::parsing::flat::Tree {
        &self.module(module).ast.tree
    }

    /// The text a declared name was written with.
    fn name_text(&self, module: ModuleId, name: tree::Name) -> &'a str {
        self.tree(module).name(name)
    }

    // -----------------------------------------------------------------------
    // Phase 0: primitives
    // -----------------------------------------------------------------------

    fn register_primitives(&mut self) {
        // The primitives live in a synthetic module so that every table entry
        // has an owner, but their *defining* modules — where their methods are
        // declared — are the `core/*` ones of SPEC 6.7.3.
        self.prim_module = ModuleId(u32::MAX);
        for p in Prim::all() {
            let id = self.tables.add_tycon(TyCon {
                name: p.name().to_string(),
                module: self.prim_module,
                generics: Vec::new(),
                def: TyDef::Prim(*p),
                exported: true,
                span: Span::NONE,
            });
            self.tables.register_prim(*p, id);
        }
    }

    /// `Int`, `Float`, `Uint` and `Byte` are aliases, not distinct types, so a
    /// function declared with `Int` and one declared with `I64` interoperate
    /// with no conversion. Diagnostics print whichever spelling was used.
    fn builtin_type(&self, name: &str) -> Option<TyConId> {
        let prim = match name {
            "Int" => Prim::I64,
            "Float" => Prim::F64,
            "Uint" => Prim::U64,
            "Byte" => Prim::U8,
            other => Prim::all().iter().copied().find(|p| p.name() == other)?,
        };
        Some(self.tables.prim_id(prim))
    }

    // -----------------------------------------------------------------------
    // Phase 1: declarations
    // -----------------------------------------------------------------------

    fn collect_declarations(&mut self) {
        for m in 0..self.loaded.modules.len() {
            let id = ModuleId(m as u32);
            let items = &self.module(id).ast.items;
            for (index, item) in items.iter().enumerate() {
                self.collect_item(id, index as u32, item);
            }
        }
    }

    fn collect_item(&mut self, module: ModuleId, index: u32, item: &tree::Item) {
        let ast_ref = AstRef::Item { module, item: index };
        let t = self.tree(module);
        match item {
            tree::Item::Struct(d) => {
                let generics = self.generic_shells(module, &d.generics);
                let id = self.tables.add_tycon(TyCon {
                    name: t.name(d.name).to_string(),
                    module,
                    generics,
                    def: TyDef::Struct { fields: Vec::new(), record: matches!(d.body, tree::StructBody::Record(_)) },
                    exported: d.exported,
                    span: d.name.span,
                });
                self.declare(module, d.name, Sym::Ty(id), d.exported);
            }
            tree::Item::Enum(d) => {
                let generics = self.generic_shells(module, &d.generics);
                let id = self.tables.add_tycon(TyCon {
                    name: t.name(d.name).to_string(),
                    module,
                    generics,
                    def: TyDef::Enum { variants: Vec::new() },
                    exported: d.exported,
                    span: d.name.span,
                });
                self.declare(module, d.name, Sym::Ty(id), d.exported);
            }
            tree::Item::Trait(d) => {
                // `effect` may be declared only by platform modules.
                if d.is_effect && self.module(module).role != Role::Platform {
                    self.templated("effect-outside-platform", d.span);
                }
                // A trait's *own* parameters have nowhere to be bound. An
                // `impl` is written `impl Trait for Type`, with no arguments
                // after the trait's name, so `Trait<Str>` and `Trait<Int>`
                // would be one conformance; and monomorphization rebuilds an
                // implementation's type arguments by matching the `impl` head
                // against the receiver, which mentions the trait's parameters
                // nowhere (`middle/monomorphize.rs`, `instance_targs`). The
                // refusal is here rather than there because a declaration is
                // something to fix and a miscompiled call is not.
                //
                // A *method's* own generics are supported and shipping —
                // `Show.show<C: Alloc>`, `Ui.memo<T>` — and are what a trait
                // parameter would have been used for.
                let generics = self.generic_shells(module, &d.generics);
                if let Some(first) = generics.first() {
                    let at = generics.iter().fold(first.span, |acc, g| acc.to(g.span));
                    let name = t.name(d.name).to_string();
                    self.templated("generic-effect-unsupported", at).bind("name", name);
                }
                let id = self.tables.add_trait(TraitInfo {
                    name: t.name(d.name).to_string(),
                    module,
                    generics,
                    methods: Vec::new(),
                    is_effect: d.is_effect,
                    exported: d.exported,
                    span: d.name.span,
                });
                self.declare(module, d.name, Sym::Trait(id), d.exported);
            }
            tree::Item::Fn(d) => {
                let generics = self.generic_shells(module, &d.generics);
                let id = self.tables.add_fn(FnInfo {
                    name: t.name(d.name).to_string(),
                    module,
                    generics,
                    params: Vec::new(),
                    ret: Ty::Error,
                    exported: d.exported,
                    span: d.name.span,
                    self_ty: None,
                    impl_of: None,
                    ast: ast_ref,
                    intrinsic: d.body.is_none(),
                });
                self.declare(module, d.name, Sym::Fn(id), d.exported);
            }
            tree::Item::Let(d) => {
                let id = self.tables.add_const(ConstInfo {
                    name: t.name(d.name).to_string(),
                    module,
                    ty: Ty::Error,
                    exported: d.exported,
                    span: d.name.span,
                    ast: ast_ref,
                });
                self.declare(module, d.name, Sym::Const(id), d.exported);
            }
            tree::Item::Context(d) => {
                // A `context` declaration may appear only in the module
                // exporting `main`, a test source, or a test-only module.
                let role = self.module(module).role;
                if !role.may_build_context() {
                    self.templated("context-declaration-not-allowed", d.span);
                }
                if d.exported && !matches!(role, Role::TestOnly | Role::Platform) {
                    self.templated("context-export", d.span);
                }
                let id = self.tables.add_ctx_decl(ContextDeclInfo {
                    name: t.name(d.name).to_string(),
                    module,
                    exported: d.exported,
                    checked: None,
                    span: d.name.span,
                    ast: ast_ref,
                });
                self.declare(module, d.name, Sym::Context(id), d.exported);
            }
            // An inherent `impl` puts each of its exported methods into the
            // module's scope under its own name. Nothing resolves through that
            // entry — a method is found through its receiver — but a library's
            // `lib.buri` needs a name to re-export, so that its surface stays
            // one file you can read top to bottom.
            tree::Item::Impl(d) if d.trait_ty.is_none() => {
                let owner = t.type_head(d.self_ty).unwrap_or("?").to_string();
                let scope = self.scope_mut(module);
                for method in &d.methods {
                    let sym = Sym::Method(owner.clone());
                    scope.own.entry(t.name(method.name).to_string()).or_insert(sym.clone());
                    if method.exported {
                        scope.exports.entry(t.name(method.name).to_string()).or_insert(sym);
                    }
                }
            }
            // An alias is transparent, but it is still a name a module
            // declares and may publish, so it is a symbol like any other.
            tree::Item::TypeAlias(d) => {
                let name = t.name(d.name).to_string();
                self.declare(module, d.name, Sym::Alias(module, name), d.exported);
            }
            tree::Item::Import(_)
            | tree::Item::ReExport(_)
            | tree::Item::Impl(_)
            | tree::Item::Derive(_)
            | tree::Item::Test(_)
            // The parser already said what is wrong with it.
            | tree::Item::Error(_) => {}
        }
    }

    fn generic_shells(&mut self, module: ModuleId, params: &[tree::GenericParam]) -> Vec<GenericInfo> {
        let t = self.tree(module);
        params
            .iter()
            .map(|p| GenericInfo {
                name: t.name(p.name).to_string(),
                bounds: Vec::new(),
                span: p.span,
            })
            .collect()
    }

    fn declare(&mut self, module: ModuleId, name: tree::Name, sym: Sym, exported: bool) {
        let text = self.name_text(module, name);
        let scope = self.scope_mut(module);
        if let Some(existing) = scope.own.get(text) {
            // Two methods of the same name on different types are the shape
            // `core/num`'s conversions have; anything else is a redeclaration.
            if let (Sym::Fn(a), Sym::Fn(b)) = (existing.clone(), &sym) {
                scope.own.insert(text.to_string(), Sym::Overloaded(vec![a, *b]));
                if exported {
                    scope.exports.insert(text.to_string(), Sym::Overloaded(vec![a, *b]));
                }
                return;
            }
            if let (Sym::Overloaded(mut fs), Sym::Fn(b)) = (existing.clone(), &sym) {
                fs.push(*b);
                scope.own.insert(text.to_string(), Sym::Overloaded(fs.clone()));
                if exported {
                    scope.exports.insert(text.to_string(), Sym::Overloaded(fs));
                }
                return;
            }
            self.diags.push(
                Diagnostic::templated("duplicate-module-declaration", name.span)
                    .with_bind("name", text),
            );
            return;
        }
        scope.own.insert(text.to_string(), sym.clone());
        if exported {
            scope.exports.insert(text.to_string(), sym);
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: scopes
    // -----------------------------------------------------------------------

    /// Removes from `core/host`'s exports every name the output's platform
    /// does not grant.
    ///
    /// **This is the whole of per-output host subsetting**, and it is one
    /// removal rather than a check bolted onto every use, because the property
    /// wanted is the one `design/ui-reactivity.md` §Targets states: *a platform
    /// is the set of effects its host exports; there is no second declaration.*
    /// A name that is not exported cannot be imported, cannot be reached
    /// through the namespace, and cannot be re-exported — so a program that
    /// never names `host.net` cannot open a socket, for the same reason and by
    /// the same mechanism that a program that never names it could not before.
    ///
    /// Both halves of a grant go — the implementation struct as well as the
    /// value — because `HostNet {}` has no private field and would otherwise
    /// be constructible by name from the one module that can see it.
    ///
    /// Runs between `collect_declarations` (which fills the exports) and
    /// `resolve_scopes` (which is the first pass to read them), so every later
    /// reader sees the subset and none of them has to know this happened.
    fn withhold_ungranted_host_effects(&mut self) {
        let Some(platform) = self.loaded.platform else { return };
        let Some(host) = self.loaded.find(standard_library::HOST_MODULE) else { return };
        let withheld: Vec<String> = self
            .scope(host)
            .exports
            .keys()
            .filter(|name| standard_library::host_withholds(platform, name))
            .cloned()
            .collect();
        for name in withheld {
            self.scope_mut(host).exports.remove(&name);
        }
    }

    /// Reports naming a `core/host` export that this output's platform does not
    /// grant, answering whether it did.
    ///
    /// `true` means the caller has nothing left to say: the name is missing on
    /// purpose, and "does not export" would send a reader to look for a
    /// spelling mistake in a file that spells it correctly.
    pub fn report_host_not_granted(&mut self, span: Span, name: &str) -> bool {
        let Some(platform) = self.loaded.platform else { return false };
        let Some(grant) = standard_library::host_grant_of(name) else { return false };
        if grant.platforms.contains(&platform) {
            return false;
        }
        let (effect, because) = (grant.effect, grant.because);
        // The whole "or build it elsewhere" clause, not just the list: a row
        // that names no platform has no elsewhere, and `HostGrant` is where
        // that sentence is decided.
        let elsewhere = grant.elsewhere_clause();
        let (proto, name) = (platform.proto().to_string(), name.to_string());
        self.templated("host-not-granted", span)
            .bind("platform", proto)
            .bind("name", name)
            .bind("effect", effect)
            .bind("because", because)
            .bind("elsewhere", elsewhere);
        true
    }

    fn resolve_scopes(&mut self) {
        // Everything a module declares is visible unqualified inside it,
        // before its imports add to that.
        for scope in &mut self.scopes {
            scope.names = scope.own.clone();
        }

        // Prelude names sit under everything, so a module may shadow any of
        // them and importing one explicitly is harmless. What each one refers
        // to is the same in every module, so it is looked up once here rather
        // than once per module.
        let prelude: Vec<(String, Sym)> = standard_library::prelude()
            .filter_map(|(path, name)| {
                let from = self.loaded.find(path)?;
                let sym = self.scope(from).exports.get(name)?.clone();
                Some((name.to_string(), sym))
            })
            .collect();
        for scope in &mut self.scopes {
            for (local, sym) in &prelude {
                scope.names.entry(local.clone()).or_insert_with(|| sym.clone());
            }
        }

        for m in 0..self.loaded.modules.len() {
            let id = ModuleId(m as u32);
            let items = &self.module(id).ast.items;
            for item in items {
                match item {
                    tree::Item::Import(imp) => self.apply_import(id, imp),
                    tree::Item::ReExport(re) => self.apply_reexport(id, re),
                    _ => {}
                }
            }
        }
    }

    fn apply_import(&mut self, module: ModuleId, imp: &tree::Import) {
        let Some(from) = self.loaded.find(&imp.path) else { return };
        let t = self.tree(module);
        match &imp.clause {
            tree::ImportClause::Namespace(alias) => {
                self.scope_mut(module).namespaces.insert(t.name(*alias).to_string(), from);
            }
            tree::ImportClause::Named(specs) => {
                for spec in specs {
                    let Some(sym) = self.lookup_export(from, t.name(spec.name)) else {
                        let module_path = imp.path.clone();
                        let name = t.name(spec.name).to_string();
                        // A name `core/host` withholds is missing on purpose,
                        // and saying "does not export it" would send a reader
                        // looking for a typo in a file that spells it right.
                        if module_path == standard_library::HOST_MODULE
                            && self.report_host_not_granted(spec.name.span, &name)
                        {
                            continue;
                        }
                        let mut note = None;
                        // A name that exists but is not exported is a
                        // different mistake from a name that does not exist.
                        if self.scope(from).own.contains_key(&name) {
                            note = Some(format!(
                                "`{name}` is declared in \"{module_path}\" but not exported"
                            ));
                        } else if let Some(near) = self.nearest_export(from, &name) {
                            note = Some(format!("did you mean `{near}`?"));
                        }
                        // A package path resolves to its `lib.buri`, whose
                        // surface is what it re-exports — the declaration
                        // itself may already be exported, so pointing at it
                        // would send the reader to the wrong file.
                        let is_surface = self
                            .loaded
                            .module(from)
                            .disk
                            .as_ref()
                            .and_then(|d| d.file_name())
                            .is_some_and(|f| f == "lib.buri");
                        let d = self
                            .templated("no-such-export", spec.name.span)
                            .bind("path", module_path.clone())
                            .bind("name", name.clone());
                        if is_surface {
                            d.fix(format!(
                                "check the spelling, or re-export `{name}` from \
                                 \"{module_path}\"'s `lib.buri`"
                            ));
                        } else {
                            d.fix(format!(
                                "check the spelling, or add `export` to `{name}`'s declaration in \
                                 \"{module_path}\""
                            ));
                        }
                        if let Some(n) = note {
                            d.notes.push(n);
                        }
                        continue;
                    };
                    let local = t.name(spec.local()).to_string();
                    // An explicit import wins over a prelude name.
                    self.scope_mut(module).names.insert(local, sym);
                }
            }
        }
    }

    fn apply_reexport(&mut self, module: ModuleId, re: &tree::ReExport) {
        let Some(from) = self.loaded.find(&re.path) else { return };
        let t = self.tree(module);
        for spec in &re.specs {
            let Some(sym) = self.lookup_export(from, t.name(spec.name)) else {
                let path = re.path.clone();
                let name = t.name(spec.name).to_string();
                // A name held back is a different mistake from a name that is
                // not there, and only the first is answered by `export`.
                let note;
                let fix = if self.scope(from).own.contains_key(&name) {
                    note = Some(format!("`{name}` is declared in \"{path}\" but not exported"));
                    format!(
                        "add `export` to `{name}`'s declaration in \"{path}\", or drop it from \
                         this list"
                    )
                } else {
                    note = self.nearest_export(from, &name).map(|n| format!("did you mean `{n}`?"));
                    format!("check the spelling, or drop `{name}` from this list")
                };
                let d = self
                    .templated("no-such-export", spec.name.span)
                    .bind("path", path)
                    .bind("name", name)
                    .fix(fix);
                d.notes.push("a re-export may name only what its module path exports".into());
                if let Some(n) = note {
                    d.notes.push(n);
                }
                continue;
            };
            // Re-exporting a name does not import it — write both declarations
            // if the module also uses it.
            let local = t.name(spec.local()).to_string();
            self.scope_mut(module).exports.insert(local, sym);
        }
    }

    /// Follows re-export chains, guarding against a cycle.
    pub(crate) fn lookup_export(&mut self, module: ModuleId, name: &str) -> Option<Sym> {
        if let Some(sym) = self.scope(module).exports.get(name) {
            return Some(sym.clone());
        }
        let key = (module, name.to_string());
        if self.resolving.contains(&key) {
            return None;
        }
        self.resolving.push(key);
        // The re-export may not have been applied yet, if the modules were
        // visited in an unhelpful order. Resolve it on demand.
        let items = &self.module(module).ast.items;
        let t = self.tree(module);
        let mut found = None;
        for item in items {
            if let tree::Item::ReExport(re) = item {
                let Some(spec) = re.specs.iter().find(|s| t.name(s.local()) == name) else {
                    continue;
                };
                if let Some(from) = self.loaded.find(&re.path) {
                    found = self.lookup_export(from, t.name(spec.name));
                }
                break;
            }
        }
        self.resolving.pop();
        if let Some(sym) = &found {
            self.scope_mut(module).exports.insert(name.to_string(), sym.clone());
        }
        found
    }

    pub(crate) fn nearest_export(&self, module: ModuleId, name: &str) -> Option<String> {
        let names: Vec<&str> =
            self.scope(module).exports.keys().map(|s| s.as_str()).collect();
        crate::build::buildfile::nearest(name, &names).map(|s| s.to_string())
    }

    // -----------------------------------------------------------------------
    // Phase 3: signatures
    // -----------------------------------------------------------------------

    fn elaborate_signatures(&mut self) {
        // Bounds first: elaborating a signature may need to know whether a
        // parameter is effect-carrying.
        for m in 0..self.loaded.modules.len() {
            let id = ModuleId(m as u32);
            let items = &self.module(id).ast.items;
            let t = self.tree(id);
            for (index, item) in items.iter().enumerate() {
                match item {
                    tree::Item::Struct(d) => {
                        let Some(Sym::Ty(con)) = self.scope(id).own.get(t.name(d.name)).cloned()
                        else {
                            continue;
                        };
                        let generics = self.elaborate_generics(id, &d.generics);
                        self.tables.tycon_mut(con).generics = generics.clone();
                        let def = match &d.body {
                            tree::StructBody::Record(fields) => TyDef::Struct {
                                fields: fields
                                    .iter()
                                    .map(|f| FieldInfo {
                                        name: t.name(f.name).to_string(),
                                        ty: self.elaborate(id, &generics, f.ty),
                                        exported: f.exported,
                                        span: f.span,
                                    })
                                    .collect(),
                                record: true,
                            },
                            tree::StructBody::Tuple(fields) => TyDef::Struct {
                                fields: fields
                                    .iter()
                                    .enumerate()
                                    .map(|(i, f)| FieldInfo {
                                        name: i.to_string(),
                                        ty: self.elaborate(id, &generics, f.ty),
                                        exported: f.exported,
                                        span: f.span,
                                    })
                                    .collect(),
                                record: false,
                            },
                        };
                        self.tables.tycon_mut(con).def = def;
                        self.tables.index_members(con);
                        self.check_unique_field_names(con);
                    }
                    tree::Item::Enum(d) => {
                        let Some(Sym::Ty(con)) = self.scope(id).own.get(t.name(d.name)).cloned()
                        else {
                            continue;
                        };
                        let generics = self.elaborate_generics(id, &d.generics);
                        self.tables.tycon_mut(con).generics = generics.clone();
                        let variants = d
                            .variants
                            .iter()
                            .map(|v| {
                                let (fields, record) = match &v.payload {
                                    tree::VariantPayload::None => (Vec::new(), false),
                                    tree::VariantPayload::Tuple(tys) => (
                                        t.type_list(*tys)
                                            .iter()
                                            .enumerate()
                                            .map(|(i, ty)| FieldInfo {
                                                name: i.to_string(),
                                                ty: self.elaborate(id, &generics, *ty),
                                                exported: d.exported,
                                                span: t.type_span(*ty),
                                            })
                                            .collect(),
                                        false,
                                    ),
                                    // A payload field has no `export` of its
                                    // own; the enum's is the whole answer.
                                    tree::VariantPayload::Record(fs) => (
                                        fs.iter()
                                            .map(|f| FieldInfo {
                                                name: t.name(f.name).to_string(),
                                                ty: self.elaborate(id, &generics, f.ty),
                                                exported: d.exported,
                                                span: f.span,
                                            })
                                            .collect(),
                                        true,
                                    ),
                                };
                                VariantInfo {
                                    name: t.name(v.name).to_string(),
                                    fields,
                                    record,
                                    exported: d.exported,
                                    span: v.span,
                                }
                            })
                            .collect();
                        self.tables.tycon_mut(con).def = TyDef::Enum { variants };
                        self.tables.index_members(con);
                        self.check_unique_variant_names(con);
                        self.check_productive(con, d.span);
                    }
                    tree::Item::Trait(d) => {
                        let Some(Sym::Trait(tid)) = self.scope(id).own.get(t.name(d.name)).cloned()
                        else {
                            continue;
                        };
                        let generics = self.elaborate_generics(id, &d.generics);
                        self.tables.trait_mut(tid).generics = generics.clone();
                        // A trait's `Self` is whatever type implements it,
                        // which is not known here and so stays abstract.
                        let methods = self.enter_self_scope(Ty::SelfTy, |s| {
                            d.methods
                                .iter()
                                .map(|sig| {
                                    let mut g = generics.clone();
                                    g.extend(s.elaborate_generics(id, &sig.generics));
                                    TraitMethod {
                                        name: t.name(sig.name).to_string(),
                                        generics: g.clone(),
                                        params: s.elaborate_params(id, &g, &sig.params),
                                        ret: s.elaborate(id, &g, sig.ret),
                                        span: sig.span,
                                    }
                                })
                                .collect()
                        });
                        self.tables.trait_mut(tid).methods = methods;
                        // Rule 26 holds of a declaration as much as of a
                        // definition: an `effect` whose method takes a second
                        // context, or names one `env`, is the same mistake
                        // wherever the body ends up being written.
                        let count = self.tables.trait_(tid).methods.len();
                        self.pending_ctx_rules.extend(
                            (0..count).map(|slot| PendingCtxRule::Signature(tid, slot)),
                        );
                    }
                    tree::Item::Fn(d) => {
                        let Some(sym) = self.scope(id).own.get(t.name(d.name)).cloned() else {
                            continue;
                        };
                        let fid = match sym {
                            Sym::Fn(f) => f,
                            Sym::Overloaded(fs) => {
                                match fs
                                    .iter()
                                    .find(|f| self.tables.fn_info(**f).span == d.name.span)
                                {
                                    Some(f) => *f,
                                    None => continue,
                                }
                            }
                            _ => continue,
                        };
                        self.elaborate_fn_signature(id, index as u32, fid, d);
                    }
                    tree::Item::Let(d) => {
                        let Some(Sym::Const(cid)) = self.scope(id).own.get(t.name(d.name)).cloned()
                        else {
                            continue;
                        };
                        let ty = self.elaborate(id, &[], d.ty);
                        self.tables.const_mut(cid).ty = ty;
                    }
                    _ => {}
                }
            }
        }
    }

    fn elaborate_fn_signature(
        &mut self,
        module: ModuleId,
        item: u32,
        fid: FnId,
        d: &tree::FnDecl,
    ) {
        let generics = self.elaborate_generics(module, &d.generics);
        self.tables.fn_info_mut(fid).generics = generics.clone();
        let params = self.elaborate_params(module, &generics, &d.params);
        let ret = self.elaborate(module, &generics, d.ret);
        self.tables.fn_info_mut(fid).params = params.clone();
        self.tables.fn_info_mut(fid).ret = ret;

        // A method is declared inside an `impl` block for its type, so a
        // `self` parameter at the top level has no receiver type to attach to.
        if let Some(first) = params.first() {
            if first.role == ParamRole::SelfParam {
                let n = self.name_text(module, d.name).to_string();
                self.templated("method-declared-free", first.span).bind("name", n);
            }
        }

        self.pending_ctx_rules.push(PendingCtxRule::Free(fid, module, item));
        self.record_intrinsic(fid, module, d);
    }

    /// Phase 4½: rule 26, once the tables it reads are finished.
    ///
    /// It asks whether a parameter is effect-carrying, and that question has
    /// two dependencies neither of which `elaborate_signatures` can satisfy
    /// while it runs. `con_carries_effect` reads the conformance table, which
    /// `register_conformance` fills in afterwards — so a concrete implementor
    /// of an effect used to be invisible here and `fn sneaky(s: Scope): I64 {
    /// s.nowMillis() }` was admitted, defeating the invariant the diagnostic
    /// itself states. And `provides` reads elaborated type bodies, which the
    /// same interleaved loop is still filling in, item by item.
    ///
    /// Both are settled by the time this runs, and nothing between the two
    /// points reads what it reports.
    ///
    /// Running last is also what lets the rule reach a *method*. An `impl`'s
    /// methods do not exist as functions until `register_conformance` has run,
    /// and that pass comes after the one that elaborates free signatures — so
    /// a check that ran in place could only ever have seen the free half,
    /// which is exactly the half it used to see.
    fn check_ctx_rules(&mut self) {
        for pending in std::mem::take(&mut self.pending_ctx_rules) {
            let Some((params, generics)) = self.ctx_rule_signature(pending) else {
                continue;
            };
            // Whether there is anything to say is decided from a borrow; only
            // saying it needs owned copies. This runs once per signature in
            // the program, the standard library included, and the answer is
            // almost always "nothing" — so the copy belongs on the reporting
            // path.
            if self.violates_ctx_rule(params, generics) {
                let (params, generics) = (params.to_vec(), generics.to_vec());
                self.report_ctx_rule(&params, &generics);
            }
            if let PendingCtxRule::Free(fid, module, item) = pending {
                self.check_entry_point(fid, module, item);
            }
        }
    }

    /// The two things rule 26 reads, wherever the signature was written.
    ///
    /// The slot of a `Signature` is minted from the very list it indexes, so
    /// today it cannot miss; the lookup is total anyway rather than an `ice`,
    /// because a later pass that rewrote a trait's methods would otherwise
    /// turn a stale slot into a crash instead of a skipped check.
    fn ctx_rule_signature(
        &self,
        pending: PendingCtxRule,
    ) -> Option<(&[ParamInfo], &[GenericInfo])> {
        match pending {
            PendingCtxRule::Free(fid, _, _) | PendingCtxRule::Method(fid) => {
                let info = self.tables.fn_info(fid);
                Some((&info.params, &info.generics))
            }
            PendingCtxRule::Signature(trait_id, slot) => self
                .tables
                .trait_(trait_id)
                .methods
                .get(slot)
                .map(|m| (m.params.as_slice(), m.generics.as_slice())),
        }
    }

    /// `main` takes no parameters, declares no generics, and returns
    /// `Result<(), Str>`. It is a free `fn` in the entry module, which is why
    /// only [`PendingCtxRule::Free`] reaches here: a method is never `main`,
    /// whatever it is called.
    fn check_entry_point(&mut self, fid: FnId, module: ModuleId, item: u32) {
        let info = self.tables.fn_info(fid);
        let (name_is_main, exported) = (info.name == "main", info.exported);
        if !name_is_main || !exported || self.module(module).role != Role::Entry {
            return;
        }
        let Some(tree::Item::Fn(d)) = self.module(module).ast.items.get(item as usize) else {
            return;
        };
        self.check_main_signature(fid, d);
    }

    /// An effect-carrying parameter must be `self` or `ctx`, at most one of
    /// each. Both are fixed positions with fixed names, so you read the first
    /// two parameters and stop (SPEC 10.2).
    ///
    /// The predicate half: does any parameter break it? Written as a mirror of
    /// the loop that reports, so the two cannot drift — a `true` here is
    /// exactly one diagnostic or more there.
    fn violates_ctx_rule(&self, params: &[ParamInfo], generics: &[GenericInfo]) -> bool {
        let mut ctx_count: usize = 0;
        let mut self_count: usize = 0;
        for (i, p) in params.iter().enumerate() {
            match p.role {
                ParamRole::SelfParam => {
                    self_count = self_count.saturating_add(1);
                    if i != 0 {
                        return true;
                    }
                }
                ParamRole::Ctx => {
                    ctx_count = ctx_count.saturating_add(1);
                    let expected = if self_count > 0 { 1 } else { 0 };
                    if i != expected || ctx_count > 1 {
                        return true;
                    }
                }
                ParamRole::Normal => {
                    if self.tables.is_effect_carrying(&p.ty, generics) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn report_ctx_rule(&mut self, params: &[ParamInfo], generics: &[GenericInfo]) {
        let mut ctx_count: usize = 0;
        let mut self_count: usize = 0;
        for (i, p) in params.iter().enumerate() {
            match p.role {
                ParamRole::SelfParam => {
                    self_count = self_count.saturating_add(1);
                    if i != 0 {
                        self.templated("self-not-first", p.span)
                            .bind("position", "the first parameter");
                    }
                }
                ParamRole::Ctx => {
                    ctx_count = ctx_count.saturating_add(1);
                    let expected = if self_count > 0 { 1 } else { 0 };
                    if i != expected {
                        self.templated("ctx-not-first", p.span);
                    }
                    if ctx_count > 1 {
                        self.templated("duplicate-ctx-parameter", p.span);
                    }
                }
                ParamRole::Normal => {
                    if self.tables.is_effect_carrying(&p.ty, generics) {
                        let name = p.name.clone();
                        // A type that implements an effect *is* the
                        // capability, so "drop the effect bound" would be
                        // advice that cannot be taken — there is no bound
                        // anywhere in the signature to drop. Name the `impl`
                        // instead, which is the only thing a reader can act
                        // on.
                        let nominal = self.tables.effect_implementor(&p.ty).map(|(con, tr)| {
                            (
                                self.tables.tycon(con).name.clone(),
                                self.tables.trait_(tr).name.clone(),
                            )
                        });
                        let fix = match &nominal {
                            Some(_) => format!(
                                "rename `{name}` to `ctx` and make it the first parameter, or \
                                 take a type that implements no effect if this parameter is \
                                 ordinary data"
                            ),
                            None => format!(
                                "rename `{name}` to `ctx` and make it the first parameter, or \
                                 drop the effect bound if this parameter is ordinary data"
                            ),
                        };
                        let d = self.templated("effect-param-not-ctx", p.span);
                        d.bind("name", name.clone());
                        d.fix(fix);
                        if let Some((con, tr)) = nominal {
                            d.notes.push(format!(
                                "`{con}` implements the effect `{tr}`, so holding one is holding \
                                 the capability"
                            ));
                        }
                        d.notes.push(
                            "a function is effectful if and only if it has a `ctx` parameter or \
                             an effect-carrying `self`, which is what lets a reader stop after \
                             the first two parameters"
                                .into(),
                        );
                    }
                }
            }
        }
    }

    fn check_main_signature(&mut self, fid: FnId, d: &tree::FnDecl) {
        let info = self.tables.fn_info(fid).clone();
        if !info.params.is_empty() {
            self.templated("main-signature", d.span)
                .bind("requirement", "takes no parameters")
                .fix("drop them, and build the context `main` needs in its own body")
                .notes
                .push(
                "it builds the one context the program has, which is why there is no fake to \
                 pass it and why logic worth testing goes in a function it calls"
                    .into(),
            );
        }
        if !info.generics.is_empty() {
            self.templated("main-signature", d.span)
                .bind("requirement", "declares no generic parameters")
                .fix("drop them: `main` is called by the runtime, so there is nothing to infer them from");
        }
        let unit = Ty::Unit;
        let str_ty = self.tables.prim(Prim::Str);
        let ok = match &info.ret {
            Ty::Con(id, args) => {
                self.result_con.as_ref() == Some(id)
                    && matches!(args.as_slice(), [ok, err] if *ok == unit && *err == str_ty)
            }
            _ => false,
        };
        if !ok && !info.ret.is_error() {
            let at = self.tree(info.module).type_span(d.ret);
            self.templated("main-signature", at)
                .bind("requirement", "must return `Result<(), Str>`")
                .fix("change the return type to `Result<(), Str>`")
                .notes
                .push("`.Ok(())` exits 0; `.Err(msg)` prints `msg` to stderr and exits 1".into());
        }
        self.entry = Some(fid);
    }

    fn record_intrinsic(&mut self, fid: FnId, module: ModuleId, d: &tree::FnDecl) {
        if d.body.is_some() {
            return;
        }
        let path = self.module(module).path.clone();
        // A bundled standard-library module, asked of the table rather than of
        // the path's spelling: the roots are `core/` and `ui/`, and a
        // documentation example loaded with `Role::Std` is under neither, so
        // this cannot mark a fenced signature as something the runtime is
        // expected to supply. Anything else bodyless was already reported by
        // the parser.
        if crate::compiler::standard_library::find(&path).is_none() {
            return;
        }
        self.tables.fn_info_mut(fid).intrinsic = true;
    }

    fn elaborate_generics(
        &mut self,
        module: ModuleId,
        params: &[tree::GenericParam],
    ) -> Vec<GenericInfo> {
        let t = self.tree(module);
        let mut out: Vec<GenericInfo> = Vec::new();
        for p in params {
            out.push(GenericInfo {
                name: t.name(p.name).to_string(),
                bounds: Vec::new(),
                span: p.span,
            });
        }
        // Bounds may mention earlier parameters, so resolve them after the
        // names exist.
        let mut resolved: Vec<Vec<TraitId>> = Vec::new();
        for p in params {
            let mut bounds = Vec::new();
            for b in t.type_list(p.bounds) {
                match self.resolve_trait(module, *b) {
                    Some(id) => bounds.push(id),
                    None => {
                        let shown = t.type_head(*b).unwrap_or("?").to_string();
                        let at = t.type_span(*b);
                        self.templated("not-a-trait", at)
                            .bind("name", shown.clone())
                            .fix(format!(
                                "name a declared trait or effect, or declare `{shown}` as one"
                            ))
                            .notes
                            .push(
                                "a bound names a declared trait; there are no where clauses"
                                    .into(),
                            );
                    }
                }
            }
            resolved.push(bounds);
        }
        for (g, bounds) in out.iter_mut().zip(resolved) {
            g.bounds = bounds;
        }
        out
    }

    fn resolve_trait(&mut self, module: ModuleId, id: TypeId) -> Option<TraitId> {
        let flat::TypeView::Named { path, .. } = self.tree(module).ty(id) else { return None };
        match self.resolve_path(module, path)? {
            Sym::Trait(t) => Some(t),
            _ => None,
        }
    }

    /// Resolves a possibly-qualified path (`Order`, `effects.Alloc`) in a module's
    /// scope.
    pub fn resolve_path(&mut self, module: ModuleId, path: &[flat::Location]) -> Option<Sym> {
        let t = self.tree(module);
        match path {
            [name] => self.scope(module).names.get(t.text(*name)).cloned(),
            // `ns.Name`, where `ns` is a namespace import. That is the only
            // qualification there is, so a longer path names nothing.
            [ns, name] => {
                let from = self.scope(module).namespaces.get(t.text(*ns)).copied()?;
                self.lookup_export(from, t.text(*name))
            }
            _ => None,
        }
    }

    /// A `self` parameter has no written type, and takes the one `Self` stands
    /// for here: the `impl` head's type, or `Ty::SelfTy` inside a `trait`. It
    /// is the same scope [`Checker::elaborate`] resolves a *written* `Self`
    /// against, so the two spellings of the receiver's type cannot part
    /// company. Outside both, a `self` parameter is the mistake
    /// `method-declared-free` reports and there is no type to give it.
    fn elaborate_params(
        &mut self,
        module: ModuleId,
        generics: &[GenericInfo],
        params: &[tree::Param],
    ) -> Vec<ParamInfo> {
        let t = self.tree(module);
        let receiver = self.self_scope.clone();
        params
            .iter()
            .map(|p| ParamInfo {
                name: t.name(p.name).to_string(),
                ty: match p.written_type() {
                    Some(ty) => self.elaborate(module, generics, ty),
                    None => receiver.clone().unwrap_or(Ty::Error),
                },
                role: match p.kind {
                    tree::ParamKind::SelfParam => ParamRole::SelfParam,
                    tree::ParamKind::CtxParam => ParamRole::Ctx,
                    tree::ParamKind::Normal => ParamRole::Normal,
                },
                span: p.span,
            })
            .collect()
    }

    /// Turns a syntactic type into a `Ty`. Aliases are transparent, so they
    /// are expanded here and never appear in a `Ty`.
    pub fn elaborate(&mut self, module: ModuleId, generics: &[GenericInfo], id: TypeId) -> Ty {
        let t = self.tree(module);
        match t.ty(id) {
            flat::TypeView::Unit { .. } => Ty::Unit,
            flat::TypeView::SelfType { span } => {
                // `Self` stands for the implementing type and is legal only
                // inside a trait or an `impl` body. Inside an `impl` that type
                // is known, and `Self` *is* it from here on: nothing between
                // this point and `middle::layout` substitutes a `Ty::SelfTy`
                // that reached an `impl` method's signature.
                let Some(ty) = self.self_scope.clone() else {
                    self.templated("self-type-outside-impl", span);
                    return Ty::Error;
                };
                ty
            }
            flat::TypeView::Array { elem, .. } => {
                Ty::Array(Box::new(self.elaborate(module, generics, elem)))
            }
            flat::TypeView::Tuple { elems, .. } => Ty::Tuple(
                elems.iter().map(|e| self.elaborate(module, generics, *e)).collect(),
            ),
            flat::TypeView::Fn { params, ret, .. } => Ty::Fn(
                params.iter().map(|p| self.elaborate(module, generics, *p)).collect(),
                Box::new(self.elaborate(module, generics, ret)),
            ),
            flat::TypeView::Named { path, args, span } => {
                let name = t.text(
                    *path
                        .last()
                        .or_ice("the parser builds every named type from at least one identifier"),
                );
                // A generic parameter shadows everything.
                if path.len() == 1 {
                    if let Some(i) = generics.iter().position(|g| g.name == name) {
                        if !args.is_empty() {
                            self.templated("type-parameter-with-arguments", span)
                                .bind("name", name);
                        }
                        return Ty::Param(i as u32);
                    }
                }
                let elaborated_args: Vec<Ty> =
                    args.iter().map(|a| self.elaborate(module, generics, *a)).collect();

                // A type alias is transparent: `type UserId = Str` makes
                // `UserId` and `Str` the same type. It expands in the module
                // that declared it, wherever an import carried the name to.
                if let Some(Sym::Alias(owner, declared)) = self.resolve_path(module, path) {
                    return self
                        .expand_alias(owner, &declared, &elaborated_args, span)
                        .unwrap_or(Ty::Error);
                }
                if path.len() == 1 {
                    if let Some(id) = self.builtin_type(name) {
                        if !elaborated_args.is_empty() {
                            self.templated("no-type-arguments", span).bind("name", name);
                        }
                        return Ty::Con(id, Vec::new());
                    }
                }
                match self.resolve_path(module, path) {
                    Some(Sym::Ty(id)) => {
                        let arity = self.tables.tycon(id).arity();
                        if elaborated_args.len() != arity {
                            let n = self.tables.tycon(id).name.clone();
                            let got = elaborated_args.len();
                            self.templated("type-argument-count", span)
                                .bind("type", n)
                                .bind("expected", arity.to_string())
                                .bind("given", got.to_string())
                                .mismatch(arity.to_string(), got.to_string());
                            return Ty::Error;
                        }
                        Ty::Con(id, elaborated_args)
                    }
                    Some(Sym::Trait(_)) => {
                        let shown = name.to_string();
                        self.templated("trait-used-as-a-type", span).bind("name", shown);
                        Ty::Error
                    }
                    _ => {
                        let shown = t.path_text(path);
                        let mut note = None;
                        if let Some(near) = self.nearest_type_name(module, name) {
                            note = Some(format!("did you mean `{near}`?"));
                        }
                        let d = self.templated("unresolved-type", span).bind("name", shown);
                        if let Some(n) = note {
                            d.notes.push(n);
                        }
                        Ty::Error
                    }
                }
            }
        }
    }

    fn expand_alias(
        &mut self,
        module: ModuleId,
        name: &str,
        args: &[Ty],
        span: Span,
    ) -> Option<Ty> {
        // A scan of item discriminants, borrowing the tree rather than cloning
        // it: this used to copy the module's whole syntax tree per lookup.
        let items = &self.module(module).ast.items;
        let t = self.tree(module);
        let alias = items.iter().find_map(|i| match i {
            tree::Item::TypeAlias(a) if t.name(a.name) == name => Some(a),
            _ => None,
        })?;
        // An alias is transparent, so expanding one is a walk that has to end
        // at a type that is not an alias. `type A = A;` — or `A` through `B`
        // back to `A`, which an exported alias lets span modules — never ends,
        // and used to be a stack overflow rather than a diagnostic.
        let key = (module, name.to_string());
        if let Some(at) = self.expanding.iter().position(|k| *k == key) {
            self.report_alias_cycle(at, alias.name.span);
            return Some(Ty::Error);
        }
        // Every alias of a cycle already reported is answered with the error
        // type in silence. One cycle is one mistake, however many signatures
        // and fields name it.
        if self.cyclic_aliases.contains(&key) {
            return Some(Ty::Error);
        }
        let generics: Vec<GenericInfo> = alias
            .generics
            .iter()
            .map(|g| GenericInfo { name: t.name(g.name).to_string(), bounds: Vec::new(), span: g.span })
            .collect();
        if args.len() != generics.len() {
            self.templated("type-argument-arity", span)
                .bind("type", name)
                .bind("expected", generics.len().to_string())
                .fix(format!("supply exactly {}", generics.len()));
            return Some(Ty::Error);
        }
        self.expanding.push(key);
        let body = self.elaborate(module, &generics, alias.ty);
        self.expanding.pop();
        Some(substitute(&body, args, None))
    }

    /// Reports the cycle that starts at `at` in [`Checker::expanding`] and has
    /// just come back round to it, and records every alias on it so that the
    /// next use of any of them is silent.
    ///
    /// The primary span is the declaration the cycle closes on, because that is
    /// where the edit goes; the uses that led here are correct in themselves.
    /// Every other alias on the chain gets a secondary span, which is the only
    /// way a reader sees the other file when the cycle crosses a module.
    fn report_alias_cycle(&mut self, at: usize, decl: Span) {
        let chain: Vec<(ModuleId, String)> = self.expanding.iter().skip(at).cloned().collect();
        // `at` came from a `position` in the same vector, so the chain has a
        // head; the compiler cannot know that, and a cycle of no aliases is
        // nothing to say anyway.
        let Some(head) = chain.first().map(|(m, _)| *m) else { return };
        if chain.iter().any(|k| self.cyclic_aliases.contains(k)) {
            return;
        }
        // A cycle inside one module names its aliases plainly; one that crosses
        // a boundary has to say which module each name was declared in, or
        // `A -> B -> A` is a chain a reader cannot follow to a file.
        let crosses = chain.iter().any(|(m, _)| *m != head);
        let written: Vec<String> = chain
            .iter()
            .map(|(m, n)| match crosses {
                true => format!("`{n}` in \"{}\"", self.module(*m).path),
                false => format!("`{n}`"),
            })
            .collect();
        let closes = written.first().cloned().unwrap_or_default();
        let cycle = format!("{} -> {closes}", written.join(" -> "));
        let others: Vec<(Span, String)> = chain
            .iter()
            .skip(1)
            .filter_map(|(m, n)| {
                let span = self.alias_decl_span(*m, n)?;
                Some((span, format!("`{n}`, next on the cycle, is declared here")))
            })
            .collect();
        let d = self.templated("circular-type-alias", decl).bind("cycle", cycle);
        for (span, label) in others {
            d.secondary_span(span, label);
        }
        for key in chain {
            self.cyclic_aliases.insert(key);
        }
    }

    /// Where a module declares the alias it calls `name`.
    fn alias_decl_span(&self, module: ModuleId, name: &str) -> Option<Span> {
        let t = self.tree(module);
        self.module(module).ast.items.iter().find_map(|i| match i {
            tree::Item::TypeAlias(a) if t.name(a.name) == name => Some(a.name.span),
            _ => None,
        })
    }

    fn nearest_type_name(&self, module: ModuleId, name: &str) -> Option<String> {
        let mut candidates: Vec<String> = self
            .scope(module)
            .names
            .iter()
            .filter(|(_, s)| matches!(s, Sym::Ty(_)))
            .map(|(k, _)| k.clone())
            .collect();
        candidates.extend(Prim::all().iter().map(|p| p.name().to_string()));
        candidates.extend(["Int", "Float", "Uint", "Byte"].map(String::from));
        let refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
        crate::build::buildfile::nearest(name, &refs).map(|s| s.to_string())
    }

    /// Struct field names and enum variant names must be unique within their
    /// scope (SPEC 14.6).
    fn check_unique_field_names(&mut self, con: TyConId) {
        // Found under the borrow and reported after it, so that the check
        // reads the declaration in place rather than copying every field of
        // it to get a `&mut self` for the diagnostic.
        let mut dups: Vec<(Span, String)> = Vec::new();
        {
            let mut seen: HashSet<&str> = HashSet::default();
            for f in self.tables.tycon(con).fields() {
                if !seen.insert(f.name.as_str()) {
                    dups.push((f.span, f.name.clone()));
                }
            }
        }
        for (span, n) in dups {
            self.templated("duplicate-declaration", span)
                .bind("declaration", format!("field `{n}`"))
                .fix("rename one of them, or delete the duplicate");
        }
    }

    /// Enum variant names must be unique within their scope (SPEC 14.6).
    fn check_unique_variant_names(&mut self, con: TyConId) {
        // A set rather than a `Vec` searched with `contains`: one arm per
        // variant is already the shape a wide enum has, and this was N²/2
        // string comparisons on top of a copy of every variant.
        /// A duplicate, in the order the walk below meets it: a variant's own
        /// diagnostic comes before its fields'.
        enum Dup {
            Variant(Span, String),
            Field(Span, String, String),
        }
        let mut dups: Vec<Dup> = Vec::new();
        {
            let mut seen: HashSet<&str> = HashSet::default();
            for v in self.tables.tycon(con).variants() {
                if !seen.insert(v.name.as_str()) {
                    dups.push(Dup::Variant(v.span, v.name.clone()));
                }
                // A variant's own fields have to be unique too.
                let mut fields: HashSet<&str> = HashSet::default();
                for f in &v.fields {
                    if !fields.insert(f.name.as_str()) {
                        dups.push(Dup::Field(f.span, f.name.clone(), v.name.clone()));
                    }
                }
            }
        }
        for dup in dups {
            match dup {
                Dup::Variant(span, n) => {
                    self.templated("duplicate-declaration", span)
                        .bind("declaration", format!("variant `{n}`"))
                        .fix("rename one of them; `match` tells variants apart by name");
                }
                Dup::Field(span, fname, vname) => {
                    self.templated("duplicate-declaration", span)
                        .bind("declaration", format!("field `{fname}` of `{vname}`"))
                        .fix("rename one of them, or delete the duplicate");
                }
            }
        }
    }

    /// A recursive enum must have at least one variant that does not recurse,
    /// or no value of it could ever be built (SPEC 14.14).
    fn check_productive(&mut self, con: TyConId, span: Span) {
        let variants = self.tables.tycon(con).variants();
        if variants.is_empty() {
            return;
        }
        let productive = variants
            .iter()
            .any(|v| !v.fields.iter().any(|f| self.mentions_directly(&f.ty, con)));
        if !productive {
            let name = self.tables.tycon(con).name.clone();
            self.templated("uninhabited", span).bind("name", name);
        }
    }

    fn mentions_directly(&self, ty: &Ty, con: TyConId) -> bool {
        match ty {
            Ty::Con(id, args) => {
                *id == con || args.iter().any(|a| self.mentions_directly(a, con))
            }
            Ty::Tuple(es) => es.iter().any(|e| self.mentions_directly(e, con)),
            // An array can be empty, so it is a base case.
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Phase 4: conformance and the method table
    // -----------------------------------------------------------------------

    /// Well-known names, so operators, `derive`, and the `main` signature
    /// check can find their traits and types. Runs before signatures are
    /// elaborated, because that is where `main` is checked.
    fn register_known_names(&mut self) {
        for (path, name) in standard_library::prelude() {
            if let Some(m) = self.loaded.find(path) {
                match self.scope(m).exports.get(name) {
                    Some(Sym::Trait(t)) => {
                        self.known_traits.insert(name.to_string(), *t);
                    }
                    Some(Sym::Ty(c)) => {
                        self.known_types.insert(name.to_string(), *c);
                    }
                    _ => {}
                }
            }
        }
        // `core/json` is loaded on import rather than eagerly, so these are
        // known exactly when a program could name them — which is the only
        // time a primitive needs an implementation of either to be found.
        if let Some(m) = self.loaded.find("core/json/lib.buri") {
            for name in ["ToJson", "FromJson", "DecodeError", "Json"] {
                match self.scope(m).exports.get(name) {
                    Some(Sym::Trait(t)) => {
                        self.known_traits.insert(name.to_string(), *t);
                    }
                    Some(Sym::Ty(c)) => {
                        self.known_types.insert(name.to_string(), *c);
                    }
                    _ => {}
                }
            }
        }
        if let Some(m) = self.loaded.find("core/effect/lib.buri") {
            for name in ["Alloc", "IoError", "Region"] {
                match self.scope(m).exports.get(name) {
                    Some(Sym::Trait(t)) => {
                        self.known_traits.insert(name.to_string(), *t);
                    }
                    Some(Sym::Ty(c)) => {
                        self.known_types.insert(name.to_string(), *c);
                    }
                    _ => {}
                }
            }
        }
        self.option_con = self.known_types.get("Option").copied();
        self.result_con = self.known_types.get("Result").copied();
        self.order_con = self.known_types.get("Order").copied();
    }

    /// Registers the methods of every `impl` block, and every `derive`.
    ///
    /// A top-level `fn` taking `self` used to be registered here as well, off
    /// the type its annotation named. There is no such annotation now, so a
    /// method declared free has no receiver type at all — only the
    /// `method-declared-free` diagnostic `elaborate_fn_signature` already
    /// reports.
    fn register_conformance(&mut self) {
        for m in 0..self.loaded.modules.len() {
            let id = ModuleId(m as u32);
            let items = &self.module(id).ast.items;
            for (index, item) in items.iter().enumerate() {
                match item {
                    tree::Item::Impl(d) => self.register_impl(id, index as u32, d),
                    tree::Item::Derive(d) => self.register_derive(id, d),
                    _ => {}
                }
            }
        }
    }

    fn register_method(&mut self, con: TyConId, name: &str, fid: FnId, span: Span) {
        // A method may not share a name with a field of its `self` type.
        if self.tables.field_index(con, name).is_some() {
            let ty = self.tables.tycon(con).name.clone();
            self.templated("duplicate-field", span).bind("name", name).bind("type", ty);
            return;
        }
        if let Some(prev) = self.tables.method(con, name) {
            let prev_span = self.tables.fn_info(prev).span;
            let ty = self.tables.tycon(con).name.clone();
            self.templated("duplicate-method", span)
                .bind("type", ty)
                .bind("name", name)
                .secondary_spans
                .push(SecondarySpan { span: prev_span, label: "first declared here".into() });
            return;
        }
        self.tables.add_method(con, name, fid);
    }

    fn register_impl(&mut self, module: ModuleId, index: u32, d: &tree::ImplDecl) {
        // `Self` means something only inside this declaration. Entering and
        // leaving the scope around the *whole* body is what makes that true:
        // four of the early returns below used to leave the flag set, so a
        // later declaration in the same module could write `Self` outside any
        // `trait` or `impl` and have it admitted.
        //
        // The scope opens abstract because the head is elaborated before its
        // own type is known; `register_impl_body` narrows it to that type the
        // moment it has one, and this call is what puts the outer scope back
        // however the whole declaration returns.
        self.enter_self_scope(Ty::SelfTy, |s| s.register_impl_body(module, index, d));
    }

    /// Runs `f` with `Self` in scope standing for `ty`, restoring the previous
    /// scope afterwards however `f` returns.
    ///
    /// `Ty::SelfTy` is what a `trait` or `effect` declaration passes, and an
    /// `impl` head's own type is what its methods are elaborated under.
    fn enter_self_scope<R>(&mut self, ty: Ty, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.self_scope.replace(ty);
        let out = f(self);
        self.self_scope = outer;
        out
    }

    fn register_impl_body(&mut self, module: ModuleId, index: u32, d: &tree::ImplDecl) {
        let generics = self.elaborate_generics(module, &d.generics);
        // No `for` clause: this declares the type's own methods rather than
        // conformance to anything.
        let Some(trait_ref) = d.trait_ty else {
            self.register_inherent_impl(module, index, d, &generics);
            return;
        };
        let Some(trait_id) = self.resolve_trait(module, trait_ref) else {
            let t = self.tree(module);
            let shown = t.type_head(trait_ref).unwrap_or("?").to_string();
            let at = t.type_span(trait_ref);
            self.templated("not-a-trait", at)
                .bind("name", shown.clone())
                .fix(format!(
                    "name a declared trait or effect after `impl`, or drop the `for` clause if \
                     `{shown}` was meant to be the type whose own methods these are"
                ));
            return;
        };
        let self_ty = self.elaborate(module, &generics, d.self_ty);
        // From here down `Self` is the type the head named. `register_impl`
        // restores the outer scope however this returns, so the early exits
        // below need no unwinding of their own.
        self.self_scope = Some(self_ty.clone());
        let Some(self_con) = self_ty.head() else {
            if !self_ty.is_error() {
                let at = self.tree(module).type_span(d.self_ty);
                self.templated("impl-head-not-a-type", at);
            }
            return;
        };

        // An `impl` may appear only in the defining module of its type.
        let owner = self.tables.tycon(self_con).module;
        let is_prim = match self.tables.tycon(self_con).def {
            TyDef::Prim(p) => standard_library::defining_module(p) == self.module(module).path,
            _ => false,
        };
        if owner != module && !is_prim {
            let name = self.tables.tycon(self_con).name.clone();
            let at = self.tree(module).type_span(d.self_ty);
            self.templated("impl-outside-its-module", at)
                .bind("name", name.clone())
                .fix(format!(
                    "move the `impl` into `{name}`'s own module, or wrap it in a type of yours \
                     — `struct MyRegion(Region);` — and implement the trait for that"
                ))
                .notes
                .push(
                    "there is no way to implement a trait for someone else's type, which is the \
                     same restriction that already applies to methods"
                        .into(),
                );
            return;
        }

        // `ToJson` and `FromJson` say what a type's *shape* is on the wire,
        // and the shape is what the type descriptor carries — so a derived
        // implementation that holds a hand-written one encodes it structurally
        // and never calls it. Rather than obey an `impl` in some positions and
        // ignore it in others, there is no hand-written one.
        let tname = self.tables.trait_(trait_id).name.clone();
        if tname == "ToJson" || tname == "FromJson" {
            let c = self.tables.tycon(self_con).name.clone();
            self.templated("derive-only-trait", d.span)
                .bind("trait", tname)
                .bind("type", c);
            return;
        }

        // No type may implement both an effect and a trait. A type is either
        // part of the world or part of your data, and the boundary is checked
        // rather than assumed.
        let is_effect = self.tables.trait_(trait_id).is_effect;
        let conflict = self
            .tables
            .impls
            .iter()
            .find(|((t, c), _)| *c == self_con && self.tables.trait_(*t).is_effect != is_effect)
            .map(|((t, _), i)| (self.tables.trait_(*t).name.clone(), i.span));
        if let Some((other, other_span)) = conflict {
            let name = self.tables.tycon(self_con).name.clone();
            let this = self.tables.trait_(trait_id).name.clone();
            let (eff, tr) = if is_effect { (this, other) } else { (other, this) };
            self.templated("effect-and-trait", d.span)
                .bind("type", name)
                .bind("effect", eff)
                .bind("trait", tr)
                .secondary_spans
            .push(SecondarySpan { span: other_span, label: "the other one".into() });
        }

        if self.tables.impls.contains_key(&(trait_id, self_con)) {
            let t = self.tables.trait_(trait_id).name.clone();
            let c = self.tables.tycon(self_con).name.clone();
            self.templated("duplicate-implementation", d.span)
                .bind("type", c)
                .bind("trait", t)
                .fix("delete one of the two, or merge them")
                .note("there is exactly one candidate per (trait, type)");
            return;
        }

        // Register the methods, checked against the trait's signatures.
        let trait_methods = self.tables.trait_(trait_id).methods.clone();
        let mut supplied = vec![None; trait_methods.len()];
        for (sub, method) in d.methods.iter().enumerate() {
            let mname = self.name_text(module, method.name);
            let Some(slot) = trait_methods.iter().position(|m| m.name == mname) else {
                let t = self.tables.trait_(trait_id).name.clone();
                let n = mname.to_string();
                self.templated("not-a-trait-method", method.name.span)
                    .bind("trait", t.clone())
                    .bind("method", n)
                    .fix(format!("remove it, or move it into an inherent `impl` block for the type — `{t}` supplies only what it declares"));
                continue;
            };
            let mut g = generics.clone();
            g.extend(self.elaborate_generics(module, &method.generics));
            let params = self.elaborate_params(module, &g, &method.params);
            let ret = self.elaborate(module, &g, method.ret);
            // The name is what found the slot; whether the signature is the
            // one the slot declares is a second question, and one nothing
            // asked before. A caller reaching this method through a bound is
            // typechecked against the *trait's* declaration, so a disagreement
            // here is a promise the body does not keep.
            let declared =
                trait_methods.get(slot).or_ice("`slot` is a position in `trait_methods`");
            self.check_impl_signature(
                trait_id,
                declared,
                &SuppliedSignature {
                    name: mname,
                    generics: &g,
                    params: &params,
                    ret: &ret,
                    name_span: method.name.span,
                    ret_span: self.tree(module).type_span(method.ret),
                },
                generics.len(),
                &self_ty,
            );
            let fid = self.tables.add_fn(FnInfo {
                name: mname.to_string(),
                module,
                generics: g,
                params,
                ret,
                exported: true,
                span: method.name.span,
                self_ty: Some(self_con),
                impl_of: Some((trait_id, slot)),
                ast: AstRef::Method { module, item: index, sub: sub as u32 },
                intrinsic: method.body.is_none(),
            });
            self.pending_ctx_rules.push(PendingCtxRule::Method(fid));
            // `slot` is a position in `trait_methods`, which is what `supplied`
            // was sized to, so it is always in range.
            let already = supplied.get(slot).is_some_and(Option::is_some);
            if already {
                let n = mname.to_string();
                self.templated("method-supplied-twice", method.name.span).bind("method", n);
            }
            if let Some(cell) = supplied.get_mut(slot) {
                *cell = Some(fid);
            }
            self.register_method(self_con, mname, fid, method.name.span);
        }

        // An `impl` must supply every method the trait declares.
        let missing: Vec<String> = trait_methods
            .iter()
            .zip(&supplied)
            .filter(|(_, s)| s.is_none())
            .map(|(m, _)| m.name.clone())
            .collect();
        if !missing.is_empty() {
            let t = self.tables.trait_(trait_id).name.clone();
            let c = self.tables.tycon(self_con).name.clone();
            let missing = crate::diagnostics::names(&missing);
            self.templated("incomplete-impl", d.span)
                .bind("type", c)
                .bind("trait", t)
                .bind("methods", missing);
        }

        self.tables.add_impl(ImplInfo {
            trait_id,
            self_con,
            head: self_ty,
            body: ImplBody::Written(supplied),
            span: d.span,
        });
    }

    /// Rule: an `impl`'s method has the signature its trait declares.
    ///
    /// `register_impl_body` matches a method to its slot by name and then
    /// elaborates it from scratch, in the `impl`'s own generic scope — so
    /// until this ran, the only thing the two signatures had to share was the
    /// name. Everything downstream assumes more than that: a call through a
    /// bound is checked against the trait's declaration and dispatched to the
    /// `impl`'s function, and `monomorphize::instance_targs` reconstructs that
    /// function's type arguments from the trait's. A disagreement was found
    /// there, at the call, or nowhere.
    ///
    /// It is found here now, at the declaration that made it.
    fn check_impl_signature(
        &mut self,
        trait_id: TraitId,
        declared: &TraitMethod,
        supplied: &SuppliedSignature<'_>,
        impl_generics: usize,
        self_ty: &Ty,
    ) {
        let trait_generics = self.tables.trait_(trait_id).generics.len();
        let mismatches =
            signature_mismatches(declared, trait_generics, supplied, impl_generics, self_ty);
        if mismatches.is_empty() {
            return;
        }
        let trait_name = self.tables.trait_(trait_id).name.clone();
        for mismatch in mismatches {
            // Both sides are rendered in the `impl`'s vocabulary: the expected
            // type has already been rewritten into it, so `T` names the same
            // parameter in the two halves of one message.
            let shown = |ty: &Ty| quoted_ty(&self.tables, supplied.generics, ty);
            let (at, expected, found) = match &mismatch {
                SignatureMismatch::GenericCount { expected, found } => (
                    supplied.name_span,
                    counted(*expected, "type parameter"),
                    counted(*found, "type parameter"),
                ),
                SignatureMismatch::Arity { expected, found } => (
                    supplied.name_span,
                    counted(*expected, "parameter"),
                    counted(*found, "parameter"),
                ),
                SignatureMismatch::Bounds { index, expected, found } => {
                    // Named by the `impl`'s spelling of the parameter, on both
                    // sides: what the trait called it is its own business, and
                    // a message that renamed it mid-sentence would read as two
                    // different parameters.
                    let generic = supplied.generics.get(impl_generics.saturating_add(*index));
                    let name = generic.map_or("_", |g| g.name.as_str());
                    (
                        generic.map_or(supplied.name_span, |g| g.span),
                        bound_phrase(&self.tables, name, expected),
                        bound_phrase(&self.tables, name, found),
                    )
                }
                SignatureMismatch::Parameter { index, expected, found } => (
                    supplied.params.get(*index).map_or(supplied.name_span, |p| p.span),
                    shown(expected),
                    shown(found),
                ),
                SignatureMismatch::Return { expected, found } => {
                    (supplied.ret_span, shown(expected), shown(found))
                }
            };
            let name = supplied.name.to_string();
            self.templated("signature-mismatch", at)
                .bind("method", name)
                .bind("trait", trait_name.clone())
                .bind("expected", expected)
                .bind("found", found)
                .secondary_spans
                .push(SecondarySpan { span: declared.span, label: "declared here".into() });
        }
    }

    /// `impl Type { ... }` — the type's own methods. This is the only place a
    /// method may be declared, so a method always sits with the type it is a
    /// method of.
    fn register_inherent_impl(
        &mut self,
        module: ModuleId,
        index: u32,
        d: &tree::ImplDecl,
        generics: &[GenericInfo],
    ) {
        let self_ty = self.elaborate(module, generics, d.self_ty);
        // As in `register_trait_impl`: `Self` is the head's type for the rest
        // of this declaration, and `register_impl` puts the outer scope back.
        self.self_scope = Some(self_ty.clone());
        let target = match &self_ty {
            Ty::Con(con, _) => Some(*con),
            Ty::Array(_) => None,
            Ty::Error => return,
            other => {
                let shown = show(&self.tables, None, generics, other);
                let at = self.tree(module).type_span(d.self_ty);
                self.templated("type-has-no-methods", at).bind("type", shown);
                return;
            }
        };

        // An `impl` may appear only in the defining module of its type.
        if let Some(con) = target {
            let owner = self.tables.tycon(con).module;
            // A primitive has no declaring module of its own, so its methods
            // belong to the `core` module named for it and nowhere else.
            let is_prim = match self.tables.tycon(con).def {
                TyDef::Prim(p) => {
                    standard_library::defining_module(p) == self.module(module).path
                }
                _ => false,
            };
            if owner != module && !is_prim {
                let name = self.tables.tycon(con).name.clone();
                let at = self.tree(module).type_span(d.self_ty);
                self.templated("impl-outside-its-module", at)
                    .bind("name", name.clone())
                    .fix(format!(
                        "move the `impl` into `{name}`'s own module, or write a free function \
                         here and call it as one"
                    ))
                    .note(
                        "there is no way to add a method to someone else's type, which is what \
                         keeps `x.f()` a single lookup",
                    );
                return;
            }
        }

        for (sub, method) in d.methods.iter().enumerate() {
            let mname = self.name_text(module, method.name);
            let mut g = generics.to_vec();
            g.extend(self.elaborate_generics(module, &method.generics));
            let params = self.elaborate_params(module, &g, &method.params);
            let ret = self.elaborate(module, &g, method.ret);

            // A method is a function whose first parameter is `self`, and an
            // `impl` block is where one is declared — so anything else in here
            // is a mistake worth naming.
            match params.first() {
                Some(p) if p.role == ParamRole::SelfParam => {}
                _ => {
                    let n = mname.to_string();
                    self.templated("impl-fn-without-self", method.name.span).bind("name", n);
                    continue;
                }
            }

            let fid = self.tables.add_fn(FnInfo {
                name: mname.to_string(),
                module,
                generics: g,
                params,
                ret,
                exported: method.exported,
                span: method.name.span,
                self_ty: target,
                impl_of: None,
                ast: AstRef::Method { module, item: index, sub: sub as u32 },
                intrinsic: method.body.is_none(),
            });
            self.pending_ctx_rules.push(PendingCtxRule::Method(fid));
            match target {
                Some(con) => self.register_method(con, mname, fid, method.name.span),
                // `[T]` has no type constructor; its methods live in a table
                // of their own, and only `core/list` may add to it.
                None => {
                    if self.module(module).path == "core/list/lib.buri" {
                        self.tables.array_methods.insert(mname.to_string(), fid);
                    } else {
                        let at = self.tree(module).type_span(d.self_ty);
                        self.templated("array-impl-outside-core-list", at);
                    }
                }
            }
        }
    }

    fn register_derive(&mut self, module: ModuleId, d: &tree::DeriveDecl) {
        // A `derive` names a type *constructor*, not an instantiation of one:
        // `derive Eq for Option;` says every `Option<T>` compares whenever `T`
        // does. So the path is resolved directly rather than elaborated, which
        // would demand type arguments there is nothing to bind.
        let Some(self_con) = self.derive_target(module, d.self_ty) else {
            return;
        };
        if self.tables.tycon(self_con).module != module {
            let name = self.tables.tycon(self_con).name.clone();
            let at = self.tree(module).type_span(d.self_ty);
            self.templated("impl-outside-its-module", at)
                .bind("name", name.clone())
                .fix(format!("move the `derive` into `{name}`'s own module"));
            return;
        }
        for ty in self.tree(module).type_list(d.traits) {
            let at = self.tree(module).type_span(*ty);
            let Some(trait_id) = self.resolve_trait(module, *ty) else {
                let shown = self.tree(module).type_head(*ty).unwrap_or("?").to_string();
                self.templated("derive-not-a-trait", at).bind("name", shown);
                continue;
            };
            let name = self.tables.trait_(trait_id).name.clone();
            if !DERIVABLE.contains(&name.as_str()) {
                self.templated("trait-not-derivable", at).bind("trait", name).note(format!(
                    "derivable: {}",
                    crate::diagnostics::names(
                        &DERIVABLE.iter().map(|s| s.to_string()).collect::<Vec<_>>()
                    )
                ));
                continue;
            }
            if self.tables.impls.contains_key(&(trait_id, self_con)) {
                let c = self.tables.tycon(self_con).name.clone();
                self.templated("duplicate-implementation", at)
                    .bind("type", c)
                    .bind("trait", name)
                    .fix("drop it from this `derive`, or delete the hand-written `impl`");
                continue;
            }
            self.tables.add_impl(ImplInfo {
                trait_id,
                self_con,
                head: self.tables.generic_head(self_con),
                body: ImplBody::Derived,
                span: d.span,
            });
        }
    }

    /// The type constructor a `derive` names.
    fn derive_target(&mut self, module: ModuleId, id: TypeId) -> Option<TyConId> {
        let flat::TypeView::Named { path, args, span } = self.tree(module).ty(id) else {
            let at = self.tree(module).type_span(id);
            self.templated("derive-target-not-a-type", at);
            return None;
        };
        match self.resolve_path(module, path) {
            Some(Sym::Ty(con)) => {
                // Naming the arguments is allowed, and has to be consistent.
                if !args.is_empty() && args.len() != self.tables.tycon(con).arity() {
                    let n = self.tables.tycon(con).name.clone();
                    let arity = self.tables.tycon(con).arity();
                    self.templated("type-argument-arity", span)
                        .bind("type", n.clone())
                        .bind("expected", arity.to_string())
                        .fix(format!("a `derive` names the constructor alone: `derive ... for {n};`"));
                }
                Some(con)
            }
            _ => {
                let shown = self.tree(module).path_text(path);
                self.templated("unresolved-type", span).bind("name", shown);
                None
            }
        }
    }

    /// "A `derive` fails to compile if any field's type does not itself
    /// satisfy the trait" (SPEC 5.12.3). The error belongs on the `derive`
    /// line, naming the component — not at every use site.
    fn check_derives(&mut self) {
        let derived: Vec<(TraitId, TyConId, Span)> = self
            .tables
            .impls
            .iter()
            .filter(|(_, i)| i.is_derived())
            .map(|((t, c), i)| (*t, *c, i.span))
            .collect();
        let mut sorted = derived;
        sorted.sort_by_key(|(t, c, _)| (t.0, c.0));

        for (tr, con, span) in sorted {
            // A generic type's components are checked at each use site, where
            // the arguments are known; here only the ones that cannot depend
            // on an argument are decidable.
            //
            // The components are read out of the declaration rather than out
            // of a copy of it: a type deriving four traits is walked four
            // times, and each walk copied every variant and every field.
            let components: Vec<(String, Ty)> = match &self.tables.tycon(con).def {
                TyDef::Struct { fields, .. } => {
                    fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect()
                }
                TyDef::Enum { variants } => variants
                    .iter()
                    .flat_map(|v| {
                        v.fields.iter().map(move |f| (format!("{}.{}", v.name, f.name), f.ty.clone()))
                    })
                    .collect(),
                TyDef::Prim(_) => Vec::new(),
            };
            for (name, ty) in components {
                if !self.component_can_satisfy(&ty, tr, con) {
                    let t = self.tables.trait_(tr).name.clone();
                    let c = self.tables.tycon(con).name.clone();
                    let shown = show(&self.tables, None, &self.tables.tycon(con).generics, &ty);
                    self.templated("underivable", span)
                        .bind("type", c)
                        .bind("trait", t.clone())
                        .bind("field", name)
                        .bind("field_type", shown.clone())
                    .fix(if crate::compiler::semantics::types::is_derive_only(&t) {
                        format!(
                            "make `{shown}` satisfy `{t}` first — `derive {t} for {shown};` in \
                             its own module — or drop `{t}` from this `derive`"
                        )
                    } else {
                        format!(
                            "make `{shown}` satisfy `{t}` first — `derive {t} for {shown};` in \
                             its own module, or an `impl` — or drop `{t}` from this `derive`"
                        )
                    });
                    break;
                }
            }
        }
    }

    /// Whether a component could satisfy the trait for some instantiation. A
    /// type parameter is decided at the use site; a function type never can.
    fn component_can_satisfy(&self, ty: &Ty, tr: TraitId, owner: TyConId) -> bool {
        match ty {
            // Undecidable here, and checked where the arguments are known.
            Ty::Param(_) | Ty::Var(_) | Ty::SelfTy | Ty::Error => true,
            Ty::Fn(..) => false,
            Ty::Ctx(_) => false,
            Ty::Unit => true,
            Ty::Array(e) => self.component_can_satisfy(e, tr, owner),
            Ty::Tuple(es) => es.iter().all(|e| self.component_can_satisfy(e, tr, owner)),
            Ty::Con(id, args) => {
                if *id == owner {
                    return true;
                }
                if matches!(self.tables.tycon(*id).def, TyDef::Prim(Prim::Template)) {
                    return false;
                }
                if !self.tables.impls.contains_key(&(tr, *id)) {
                    return false;
                }
                args.iter().all(|a| self.component_can_satisfy(a, tr, owner))
            }
        }
    }

    /// A library's `lib.buri` is its whole public surface, and a method call
    /// from outside the library resolves only to names on it.
    fn compute_surfaces(&mut self) {
        for (module, scope) in self.loaded.modules.iter().zip(&self.scopes) {
            let Some(pkg) = module.pkg else { continue };
            let is_surface = self
                .ws
                .map(|ws| ws.package(pkg).module_path("lib.buri") == module.path)
                .unwrap_or(false);
            if !is_surface {
                continue;
            }
            let names: HashSet<String> = scope.exports.keys().cloned().collect();
            self.surfaces.insert(pkg, names);
        }
    }

    // -----------------------------------------------------------------------
    // Module-level rules
    // -----------------------------------------------------------------------

    fn check_module_rules(&mut self) {
        for m in 0..self.loaded.modules.len() {
            let id = ModuleId(m as u32);
            let role = self.module(id).role;
            let items = &self.module(id).ast.items;
            // Where each title was first declared *in this file*. Two files of
            // one suite may use one title — they are separate modules, and a
            // report names the file — so the map is per module and not per
            // suite (TESTING.md, "Naming a test").
            let mut titles: std::collections::HashMap<&str, Span> = std::collections::HashMap::new();
            for item in items {
                // `test` declarations are legal only in a test source.
                if let tree::Item::Test(t) = item {
                    if let Some(first) = titles.insert(t.name.as_str(), t.span) {
                        let name = t.name.clone();
                        self.templated("duplicate-test-name", t.span)
                            .bind("quoted_title", format!("{name:?}"))
                            .secondary_span(first, "first declared here");
                    }
                    if role != Role::TestSource {
                        self.templated("test-outside-test-source", t.span);
                    }
                }
                // A test source may not `export`, and may not be imported.
                if role == Role::TestSource && item.is_exported() {
                    self.templated("test-source-export", item.span());
                }
            }
        }
    }

    fn check_bodies(&mut self) {
        crate::compiler::semantics::inference::check_all(self);
    }

    /// Every trait a type is known to satisfy, for diagnostics.
    pub fn traits_of(&self, con: TyConId) -> BTreeSet<String> {
        crate::compiler::semantics::types::traits_of(&self.tables, con)
    }
}

// ---------------------------------------------------------------------------
// An `impl` method against the signature its trait declares
// ---------------------------------------------------------------------------

/// One `impl` method's elaborated signature, as the conformance check reads it.
///
/// It is the half of an [`FnInfo`] this comparison needs, borrowed before the
/// `FnInfo` is built, plus the two spans a disagreement is reported at: the
/// method's name for a whole-signature one, its written return type for a
/// return one. A parameter carries its own span already.
struct SuppliedSignature<'s> {
    name: &'s str,
    generics: &'s [GenericInfo],
    params: &'s [ParamInfo],
    ret: &'s Ty,
    name_span: Span,
    ret_span: Span,
}

/// One way an `impl`'s method can disagree with the signature its trait
/// declares.
///
/// The first two are exclusive of everything after them, and deliberately: a
/// method that takes the wrong number of type parameters has no shared
/// numbering left to compare types under, and one that takes the wrong number
/// of parameters would report every parameter after the first extra one. Both
/// are the whole answer on their own.
#[derive(Clone, PartialEq, Debug)]
enum SignatureMismatch {
    /// A different number of the method's *own* type parameters — the impl
    /// head's are not the method's and are not counted.
    GenericCount { expected: usize, found: usize },
    /// A different number of parameters, `self` included.
    Arity { expected: usize, found: usize },
    /// The method's `index`th own type parameter carries different bounds.
    /// Compared as a set, so `C: Alloc + Fs` and `C: Fs + Alloc` are the same
    /// declaration and neither is reported against the other; carried in the
    /// order each side wrote them, so the message echoes the source rather
    /// than the comparison's own ordering.
    Bounds { index: usize, expected: Vec<TraitId>, found: Vec<TraitId> },
    /// Parameter `index` has a different type. `expected` is the trait's,
    /// already rewritten into the `impl`'s vocabulary.
    Parameter { index: usize, expected: Ty, found: Ty },
    /// A different return type, likewise rewritten.
    Return { expected: Ty, found: Ty },
}

/// The trait's declaration of a method against the `impl`'s.
///
/// The two are elaborated in different scopes, which is the whole reason this
/// is not `==` on two lists of types:
///
/// * `Self` is abstract in the trait and is the head the `impl` was written
///   for in the `impl` — `Ty::SelfTy` on one side, `[T]` or `HostFs` on the
///   other.
/// * A method's own type parameters are numbered from the end of the *trait's*
///   generics on one side and from the end of the *impl head's* on the other,
///   so `Show.show<C>`'s `C` is `Param(0)` in the trait and `Param(1)` in
///   `impl<T> Show for [T]`.
///
/// Both are undone by substituting the trait's side into the `impl`'s
/// numbering, after which the comparison is structural equality — the same
/// thing `unify` does with two rigid types, minus the inference variables that
/// cannot appear in an elaborated signature.
///
/// A trait with generics of its own has no such renumbering: its parameters
/// would have to be bound by the `impl`'s head, and that shape is refused at
/// the declaration (`generic-effect-unsupported`). Rather than invent a
/// mapping for a program that is already rejected, the comparison stands
/// aside.
fn signature_mismatches(
    declared: &TraitMethod,
    trait_generics: usize,
    supplied: &SuppliedSignature<'_>,
    impl_generics: usize,
    self_ty: &Ty,
) -> Vec<SignatureMismatch> {
    if trait_generics > 0 {
        return Vec::new();
    }
    let declared_own = declared.generics.len();
    let supplied_own = supplied.generics.len().saturating_sub(impl_generics);
    if declared_own != supplied_own {
        return vec![SignatureMismatch::GenericCount {
            expected: declared_own,
            found: supplied_own,
        }];
    }
    if declared.params.len() != supplied.params.len() {
        return vec![SignatureMismatch::Arity {
            expected: declared.params.len(),
            found: supplied.params.len(),
        }];
    }
    // The trait's `Param(i)` is the method's own `i`th, which the `impl`
    // numbers after its head's.
    let args: Vec<Ty> =
        (0..declared_own).map(|i| Ty::Param(impl_generics.saturating_add(i) as u32)).collect();
    let mut out = Vec::new();
    // A bound is half of what a type parameter is. An `impl` that asks for
    // one the trait does not declare is asking for something its callers were
    // never told to supply — the body may then call a method the caller's type
    // has no `impl` for, and the disagreement surfaces at monomorphization,
    // in a program the reader did not write.
    for index in 0..declared_own {
        let expected = bounds_of(declared.generics.get(index));
        let found = bounds_of(supplied.generics.get(impl_generics.saturating_add(index)));
        if as_a_set(&expected) != as_a_set(&found) {
            out.push(SignatureMismatch::Bounds { index, expected, found });
        }
    }
    for (index, (d, s)) in declared.params.iter().zip(supplied.params).enumerate() {
        let expected = substitute(&d.ty, &args, Some(self_ty));
        if !agrees(&expected, &s.ty) {
            out.push(SignatureMismatch::Parameter {
                index,
                expected,
                found: s.ty.clone(),
            });
        }
    }
    let expected = substitute(&declared.ret, &args, Some(self_ty));
    if !agrees(&expected, supplied.ret) {
        out.push(SignatureMismatch::Return { expected, found: supplied.ret.clone() });
    }
    out
}

/// Whether two elaborated types are the same type, with poison agreeing to
/// everything.
///
/// A `Ty::Error` is a type that was already reported — an unresolved name, an
/// alias that would not expand — and a second diagnostic saying it does not
/// match is the cascade the poison exists to prevent.
fn agrees(a: &Ty, b: &Ty) -> bool {
    a.is_error() || b.is_error() || a == b
}

/// A type parameter's bounds as it wrote them, and empty for a parameter that
/// is not there (which `GenericCount` has already reported).
fn bounds_of(generic: Option<&GenericInfo>) -> Vec<TraitId> {
    generic.map(|g| g.bounds.clone()).unwrap_or_default()
}

/// The same, as the set the comparison is about: order carries no meaning in a
/// bound list, and a repeat is `duplicate-bound`'s business rather than this
/// rule's.
fn as_a_set(bounds: &[TraitId]) -> BTreeSet<TraitId> {
    bounds.iter().copied().collect()
}

/// A type in backticks, the way every other diagnostic quotes one.
fn quoted_ty(tables: &Tables, generics: &[GenericInfo], ty: &Ty) -> String {
    format!("`{}`", show(tables, None, generics, ty))
}

/// A type parameter with its bounds, as a message names it: `` `C: Alloc + Fs` ``,
/// or `` `C` with no bounds `` where there are none to name.
fn bound_phrase(tables: &Tables, name: &str, bounds: &[TraitId]) -> String {
    if bounds.is_empty() {
        return format!("`{name}` with no bounds");
    }
    let named: Vec<&str> = bounds.iter().map(|t| tables.trait_(*t).name.as_str()).collect();
    format!("`{name}: {}`", named.join(" + "))
}

/// "no parameters", "1 parameter", "3 parameters" — a count as the noun phrase
/// a message wants, where zero reads better as a word than as a digit.
fn counted(n: usize, noun: &str) -> String {
    match n {
        0 => format!("no {noun}s"),
        1 => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::SourceMap;

    /// One snippet through the real front end, with its `Tables`.
    fn tables_of(src: &str) -> Tables {
        let mut map = SourceMap::new();
        let analysis = crate::compiler::driver::analyze_snippet(
            &mut map,
            "resolve_test.buri",
            src,
            crate::compiler::modules::Role::Entry,
        );
        let errors: Vec<String> = analysis
            .diagnostics
            .items
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect();
        assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
        analysis.checked.tables
    }

    /// The method named `name` on the type named `ty`.
    fn method<'t>(tables: &'t Tables, ty: &str, name: &str) -> &'t FnInfo {
        let con =
            tables.tycons.iter().position(|c| c.name == ty).expect("the snippet declares the type");
        tables
            .fns
            .iter()
            .find(|f| f.name == name && f.self_ty.map(TyConId::index) == Some(con))
            .expect("the snippet declares the method")
    }

    const RELAY: &str = r#"
struct Wrap { value: Int }

trait Relay {
  fn relay(self, done: fn(Self, Int) => Int): Int;
}

impl Relay for Wrap {
  fn relay(self, done: fn(Self, Int) => Int): Int {
    done(self, self.value)
  }
}

fn main(): () {}
"#;

    /// A written `Self` in an `impl` method's parameter is the `impl` head's
    /// type by the time it reaches `FnInfo.params`. Left as `Ty::SelfTy` it is
    /// substituted by nothing downstream — `middle::monomorphize` passes `None`
    /// for the self type at every `substitute` — and reaches `middle::layout`
    /// as a type it has no size for.
    #[test]
    fn a_written_self_in_an_impl_method_parameter_is_the_impl_heads_type() {
        let tables = tables_of(RELAY);
        let f = method(&tables, "Wrap", "relay");
        let Ty::Fn(params, _) = &f.params[1].ty else {
            panic!("the parameter is a function type: {:?}", f.params[1].ty)
        };
        let wrap = tables.tycons.iter().position(|c| c.name == "Wrap").unwrap_or_default();
        assert!(
            matches!(&params[0], Ty::Con(id, args) if id.index() == wrap && args.is_empty()),
            "`Self` stayed unresolved in an `impl` method's parameter: {:?}",
            params[0],
        );
    }

    /// The implicit type of a `self` parameter and a written `Self` come from
    /// the same scope, so the two spellings of the receiver agree.
    #[test]
    fn the_self_parameter_and_a_written_self_agree() {
        let tables = tables_of(RELAY);
        let f = method(&tables, "Wrap", "relay");
        let Ty::Fn(params, _) = &f.params[1].ty else { panic!("a function type") };
        assert_eq!(f.params[0].ty, params[0]);
        assert_eq!(f.params[0].role, ParamRole::SelfParam);
    }

    /// A `trait`'s own signature is the other half: there is no implementing
    /// type yet, so `Self` stays abstract and an `impl` is what supplies one.
    #[test]
    fn a_written_self_in_a_trait_signature_stays_abstract() {
        let tables = tables_of(RELAY);
        let tr = tables.traits.iter().find(|t| t.name == "Relay").expect("the trait");
        let m = tr.methods.first().expect("the method");
        assert_eq!(m.params[0].ty, Ty::SelfTy);
        let Ty::Fn(params, _) = &m.params[1].ty else { panic!("a function type") };
        assert_eq!(params[0], Ty::SelfTy);
    }

    /// The head's own type parameters are in scope in what `Self` expands to,
    /// so a generic `impl` gets `Crate<T>` and not a bare constructor — in the
    /// return type as well as in a parameter.
    #[test]
    fn a_written_self_carries_the_impl_heads_type_arguments() {
        let tables = tables_of(
            r#"
struct Crate<T> { value: T }

trait Copyable {
  fn copy(self, other: Self): Self;
}

impl<T> Copyable for Crate<T> {
  fn copy(self, other: Self): Self {
    other
  }
}

fn main(): () {}
"#,
        );
        let f = method(&tables, "Crate", "copy");
        let head = |ty: &Ty| matches!(ty, Ty::Con(_, args) if args.as_slice() == [Ty::Param(0)]);
        assert!(head(&f.params[1].ty), "a written `Self` parameter: {:?}", f.params[1].ty);
        assert!(head(&f.ret), "a written `Self` return: {:?}", f.ret);
    }

    /// An inherent `impl` resolves `Self` the same way a trait `impl` does:
    /// the two share `register_impl`'s scope and differ only in what they
    /// register.
    #[test]
    fn an_inherent_impl_resolves_a_written_self_too() {
        let tables = tables_of(
            r#"
struct Knob { value: Int }

impl Knob {
  fn pick(self, other: Self): Self {
    if (self.value > other.value) { self } else { other }
  }
}

fn main(): () {}
"#,
        );
        let f = method(&tables, "Knob", "pick");
        assert_eq!(f.params[1].ty, f.params[0].ty);
        assert_eq!(f.ret, f.params[0].ty);
        assert!(!matches!(f.params[1].ty, Ty::SelfTy));
    }

    // -----------------------------------------------------------------------
    // Rule 26 inside a `trait` body and an `impl` block
    // -----------------------------------------------------------------------

    /// How many times a snippet reports one diagnostic code.
    ///
    /// The count rather than the presence: what this covers is a set of
    /// signatures nobody read, so a test that merely saw *a* diagnostic would
    /// pass on the one signature that was already being checked.
    fn reported(src: &str, code: &str) -> usize {
        let mut map = SourceMap::new();
        let analysis = crate::compiler::driver::analyze_snippet(
            &mut map,
            "resolve_test.buri",
            src,
            crate::compiler::modules::Role::Entry,
        );
        analysis
            .diagnostics
            .items
            .iter()
            .filter(|d| d.code.as_deref() == Some(code))
            .count()
    }

    /// A `trait` declaration, the `impl` that supplies it, and one of the
    /// type's own methods — the three shapes a method signature comes in, each
    /// taking a context under a name that is not `ctx`.
    const SINKS: &str = r#"
from "core/effect/lib.buri" import { Stdout };

struct Ledger { lines: Int }

trait Report {
  fn write<C: Stdout>(self, sink: C): ();
}

impl Report for Ledger {
  fn write<C: Stdout>(self, sink: C): () {
    ()
  }
}

impl Ledger {
  fn dump<C: Stdout>(self, sink: C): () {
    ()
  }
}

fn main(): () {}
"#;

    /// All three, not none. `pending_ctx_rules` used to be pushed from the
    /// `Item::Fn` path alone, so a method — which is where a context is most
    /// often taken — was exempt from the rule its own callers read.
    #[test]
    fn an_effect_carrying_parameter_is_refused_in_every_kind_of_method() {
        assert_eq!(reported(SINKS, "effect-param-not-ctx"), 3);
    }

    /// The position half of the rule reaches a method too: receiver first,
    /// context second, everything else after.
    #[test]
    fn a_ctx_out_of_position_is_refused_in_every_kind_of_method() {
        let src = r#"
from "core/effect/lib.buri" import { Stdout };

struct Bell { tone: Str }

trait Ring {
  fn ring<C: Stdout>(self, times: Int, ctx: C): ();
}

impl Ring for Bell {
  fn ring<C: Stdout>(self, times: Int, ctx: C): () {
    ()
  }
}

impl Bell {
  fn peal<C: Stdout>(self, times: Int, ctx: C): () {
    ()
  }
}

fn main(): () {}
"#;
        assert_eq!(reported(src, "ctx-not-first"), 3);
    }

    /// A misplaced receiver, in the two places one survives to rule 26: an
    /// inherent `impl` refuses a method that does not open with `self` before
    /// it is ever registered — that is `impl-fn-without-self` — so what is
    /// left is a `trait` body and the `impl` that supplies it.
    #[test]
    fn a_misplaced_self_is_refused_in_a_trait_body_and_in_its_impl() {
        let src = r#"
struct Square { side: Int }

trait Scale {
  fn scaled(factor: Int, self): Int;
}

impl Scale for Square {
  fn scaled(factor: Int, self): Int {
    self.side * factor
  }
}

fn main(): () {}
"#;
        // The parser reports the same code at each site on its own, so the
        // semantic half is the second of each pair.
        assert_eq!(reported(src, "self-not-first"), 4);
    }

    /// A legitimate `ctx` does not license a second capability beside it: the
    /// loop reads on past the first two parameters, in a method as anywhere.
    #[test]
    fn an_effect_carrying_parameter_after_a_ctx_is_still_refused() {
        let src = r#"
from "core/effect/lib.buri" import { Stdout };

struct Twice { n: Int }

impl Twice {
  fn both<C: Stdout, D: Stdout>(self, ctx: C, also: D): () {
    ()
  }
}

fn main(): () {}
"#;
        assert_eq!(reported(src, "effect-param-not-ctx"), 1);
    }

    /// The other direction, which is what keeps this from being a rule against
    /// methods: one that follows the convention says nothing at all.
    #[test]
    fn a_method_that_follows_the_convention_is_admitted() {
        let src = r#"
from "core/effect/lib.buri" import { Stdout };

struct Quiet { n: Int }

trait Speak {
  fn speak<C: Stdout>(self, ctx: C, times: Int): ();
}

impl Speak for Quiet {
  fn speak<C: Stdout>(self, ctx: C, times: Int): () {
    ()
  }
}

impl Quiet {
  fn hush<C: Stdout>(self, ctx: C): () {
    ()
  }
}

fn main(): () {}
"#;
        for code in ["effect-param-not-ctx", "ctx-not-first", "self-not-first"] {
            assert_eq!(reported(src, code), 0, "a well-formed method reported `{code}`");
        }
    }

    // -----------------------------------------------------------------------
    // An `impl` method against the signature its trait declares
    // -----------------------------------------------------------------------

    /// The two type constructors these tests name. Nothing here reads the
    /// tables — the comparison is over elaborated types — so any two distinct
    /// ids stand for any two distinct types.
    const BAG: TyConId = TyConId(7);
    const CRATE: TyConId = TyConId(8);
    const SHOWN: TraitId = TraitId(1);
    const ALLOCS: TraitId = TraitId(2);

    fn generic(name: &str, bounds: &[TraitId]) -> GenericInfo {
        GenericInfo { name: name.to_string(), bounds: bounds.to_vec(), span: Span::NONE }
    }

    fn param(ty: Ty) -> ParamInfo {
        ParamInfo { name: "x".to_string(), ty, role: ParamRole::Normal, span: Span::NONE }
    }

    fn receiver() -> ParamInfo {
        ParamInfo {
            name: "self".to_string(),
            ty: Ty::SelfTy,
            role: ParamRole::SelfParam,
            span: Span::NONE,
        }
    }

    fn trait_method(generics: Vec<GenericInfo>, params: Vec<ParamInfo>, ret: Ty) -> TraitMethod {
        TraitMethod { name: "m".to_string(), generics, params, ret, span: Span::NONE }
    }

    fn supplied<'s>(
        generics: &'s [GenericInfo],
        params: &'s [ParamInfo],
        ret: &'s Ty,
    ) -> SuppliedSignature<'s> {
        SuppliedSignature {
            name: "m",
            generics,
            params,
            ret,
            name_span: Span::NONE,
            ret_span: Span::NONE,
        }
    }

    fn bag() -> Ty {
        Ty::Con(BAG, Vec::new())
    }

    fn crate_ty() -> Ty {
        Ty::Con(CRATE, Vec::new())
    }

    /// The everyday shape: no generics anywhere, `self` on both sides. The
    /// trait's `Self` is abstract and the `impl`'s is the head it was written
    /// for, and the substitution is what makes them the same type.
    #[test]
    fn a_signature_that_agrees_reports_nothing() {
        let declared = trait_method(vec![], vec![receiver(), param(Ty::Unit)], bag());
        let params = [param(bag()), param(Ty::Unit)];
        let ret = bag();
        let found = supplied(&[], &params, &ret);
        assert_eq!(signature_mismatches(&declared, 0, &found, 0, &bag()), vec![]);
    }

    /// `impl<T> Show for [T]` supplying `show<C>`: the impl head's parameter is
    /// not the method's, so the counts agree at one apiece, and the method's
    /// own `C` is `Param(0)` in the trait and `Param(1)` in the `impl`.
    ///
    /// This is the renumbering the whole comparison exists for. Comparing the
    /// two lists as written would report a disagreement on every generic
    /// method of every generic `impl` in the tree.
    #[test]
    fn an_impl_heads_generics_are_not_the_methods_own() {
        let head = Ty::Array(Box::new(Ty::Param(0)));
        let declared =
            trait_method(vec![generic("C", &[])], vec![receiver(), param(Ty::Param(0))], Ty::Unit);
        let generics = [generic("T", &[]), generic("C", &[])];
        let params = [param(head.clone()), param(Ty::Param(1))];
        let ret = Ty::Unit;
        let found = supplied(&generics, &params, &ret);
        assert_eq!(signature_mismatches(&declared, 0, &found, 1, &head), vec![]);
    }

    /// A method that declares the wrong number of its own type parameters is
    /// reported once, and the parameter types it also disagrees about are not
    /// reported at all: with no shared numbering there is nothing to compare
    /// them under.
    #[test]
    fn a_generic_count_is_the_whole_answer() {
        let declared =
            trait_method(vec![generic("T", &[])], vec![receiver(), param(Ty::Param(0))], Ty::Unit);
        let params = [param(bag()), param(crate_ty())];
        let ret = Ty::Unit;
        let found = supplied(&[], &params, &ret);
        assert_eq!(
            signature_mismatches(&declared, 0, &found, 0, &bag()),
            vec![SignatureMismatch::GenericCount { expected: 1, found: 0 }],
        );
    }

    /// The same for an arity that disagrees, and for the same reason: every
    /// parameter after the extra one would be compared against its neighbour.
    #[test]
    fn an_arity_is_the_whole_answer() {
        let declared = trait_method(vec![], vec![receiver()], Ty::Unit);
        let params = [param(bag()), param(crate_ty())];
        let ret = crate_ty();
        let found = supplied(&[], &params, &ret);
        assert_eq!(
            signature_mismatches(&declared, 0, &found, 0, &bag()),
            vec![SignatureMismatch::Arity { expected: 1, found: 2 }],
        );
    }

    /// A parameter and a return type disagreeing are two findings, not one:
    /// they are written in two places and each is reported at its own.
    #[test]
    fn a_parameter_and_a_return_type_are_reported_separately() {
        let declared = trait_method(vec![], vec![receiver(), param(Ty::Unit)], bag());
        let params = [param(bag()), param(crate_ty())];
        let ret = crate_ty();
        let found = supplied(&[], &params, &ret);
        assert_eq!(
            signature_mismatches(&declared, 0, &found, 0, &bag()),
            vec![
                SignatureMismatch::Parameter { index: 1, expected: Ty::Unit, found: crate_ty() },
                SignatureMismatch::Return { expected: bag(), found: crate_ty() },
            ],
        );
    }

    /// A receiver the `impl` wrote as the wrong type is caught by the same
    /// arm, because `self` is a parameter like any other once `Self` has been
    /// substituted.
    #[test]
    fn a_receiver_is_compared_after_self_is_substituted() {
        let declared = trait_method(vec![], vec![receiver()], Ty::Unit);
        let params = [param(crate_ty())];
        let ret = Ty::Unit;
        let found = supplied(&[], &params, &ret);
        assert_eq!(
            signature_mismatches(&declared, 0, &found, 0, &bag()),
            vec![SignatureMismatch::Parameter {
                index: 0,
                expected: bag(),
                found: crate_ty(),
            }],
        );
    }

    /// Poison agrees with everything. A type that would not elaborate has
    /// already been reported, and a second diagnostic saying it does not match
    /// is the cascade `Ty::Error` exists to prevent.
    #[test]
    fn an_error_type_reports_nothing_on_either_side() {
        let declared = trait_method(vec![], vec![receiver(), param(Ty::Error)], Ty::Unit);
        let params = [param(bag()), param(crate_ty())];
        let ret = Ty::Error;
        let found = supplied(&[], &params, &ret);
        assert_eq!(signature_mismatches(&declared, 0, &found, 0, &bag()), vec![]);
    }

    /// Bounds are a set: the same ones in another order are the same
    /// declaration.
    #[test]
    fn bounds_in_another_order_agree() {
        let declared = trait_method(
            vec![generic("C", &[SHOWN, ALLOCS])],
            vec![receiver(), param(Ty::Param(0))],
            Ty::Unit,
        );
        let generics = [generic("C", &[ALLOCS, SHOWN])];
        let params = [param(bag()), param(Ty::Param(0))];
        let ret = Ty::Unit;
        let found = supplied(&generics, &params, &ret);
        assert_eq!(signature_mismatches(&declared, 0, &found, 0, &bag()), vec![]);
    }

    /// A bound the trait does not declare is a requirement the `impl`'s
    /// callers were never told to meet, and it is carried in the order each
    /// side wrote it so the message can echo the source.
    #[test]
    fn a_bound_the_trait_does_not_declare_is_refused() {
        let declared = trait_method(vec![generic("C", &[SHOWN])], vec![receiver()], Ty::Unit);
        let generics = [generic("C", &[SHOWN, ALLOCS])];
        let params = [param(bag())];
        let ret = Ty::Unit;
        let found = supplied(&generics, &params, &ret);
        assert_eq!(
            signature_mismatches(&declared, 0, &found, 0, &bag()),
            vec![SignatureMismatch::Bounds {
                index: 0,
                expected: vec![SHOWN],
                found: vec![SHOWN, ALLOCS],
            }],
        );
    }

    /// A trait with generics of its own is left to `generic-effect-unsupported`
    /// rather than compared under a renumbering that does not exist. The
    /// signature below disagrees in every way it can and is still silent here.
    #[test]
    fn a_generic_trait_is_left_to_its_own_refusal() {
        let declared = trait_method(
            vec![generic("T", &[]), generic("C", &[])],
            vec![receiver(), param(Ty::Param(0))],
            Ty::Param(1),
        );
        let params = [param(crate_ty())];
        let ret = Ty::Unit;
        let found = supplied(&[], &params, &ret);
        assert_eq!(signature_mismatches(&declared, 1, &found, 0, &bag()), vec![]);
    }

    /// End to end, and the direction that matters most: an `impl` that agrees
    /// with its trait says nothing. This is the shape the whole standard
    /// library is written in.
    #[test]
    fn an_impl_that_agrees_with_its_trait_is_admitted() {
        let src = r#"
struct Knob { value: Int }

trait Pick {
  fn pick(self, other: Self): Self;
  fn label<T>(self, tag: T): Int;
}

impl Pick for Knob {
  fn pick(self, other: Knob): Knob {
    if (self.value > other.value) { self } else { other }
  }
  fn label<T>(self, tag: T): Int { self.value }
}

fn main(): () {}
"#;
        assert_eq!(reported(src, "signature-mismatch"), 0);
    }

    /// And the same `impl` written with `Self` throughout, which A1 made
    /// legal: `Self` and the head type are the same type inside the block, so
    /// the comparison must accept either spelling.
    #[test]
    fn an_impl_that_writes_self_for_the_head_type_is_admitted() {
        let src = r#"
struct Knob { value: Int }

trait Pick {
  fn pick(self, other: Self): Self;
}

impl Pick for Knob {
  fn pick(self, other: Self): Self {
    if (self.value > other.value) { self } else { other }
  }
}

fn main(): () {}
"#;
        assert_eq!(reported(src, "signature-mismatch"), 0);
    }

    /// End to end in the other direction: three methods, one disagreement
    /// each, one report each.
    #[test]
    fn every_half_of_a_signature_is_compared() {
        let src = r#"
struct Knob { value: Int }

trait Pick {
  fn pick(self, other: Self): Self;
  fn count(self): Int;
  fn label<T>(self, tag: T): Int;
}

impl Pick for Knob {
  fn pick(self, other: Int): Self { self }
  fn count(self): Str { "one" }
  fn label(self, tag: Int): Int { self.value }
}

fn main(): () {}
"#;
        assert_eq!(reported(src, "signature-mismatch"), 3);
    }

    /// And `Self` outside both is still the mistake it was: the scope is
    /// entered and left around one declaration, so the next one in the same
    /// module does not inherit it.
    #[test]
    fn self_outside_an_impl_is_still_refused() {
        let mut map = SourceMap::new();
        let analysis = crate::compiler::driver::analyze_snippet(
            &mut map,
            "resolve_test.buri",
            r#"
struct Knob { value: Int }

impl Knob {
  fn pick(self): Self {
    self
  }
}

fn free(x: Int): Self { x }

fn main(): () {}
"#,
            crate::compiler::modules::Role::Entry,
        );
        assert!(
            analysis
                .diagnostics
                .items
                .iter()
                .any(|d| d.code.as_deref() == Some("self-type-outside-impl")),
            "a free function's `Self` was admitted",
        );
    }

    /// A `context` declaration built from one written *after* it.
    ///
    /// `..Base()` reads the type checking recorded for `Base`, and the
    /// declarations used to be checked strictly in the order their ids were
    /// minted: item order inside a module, and module-*discovery* order across
    /// them. So a declaration whose base came later kept only the bindings it
    /// wrote itself, silently — no diagnostic where the mistake was, and an
    /// `unsatisfied-bound` at every use for an effect that is right there in
    /// the source.
    ///
    /// It is not a contrived order. `cli/tests/conformance`'s
    /// `lib/semantics/test/effects.buri` spreads `core/testing/context`'s
    /// `Hermetic`, and the day the migration to `core/host/testing` made that
    /// file the first in its package to import the module, the module was
    /// discovered *through* it — so `Hermetic` was minted second and eleven
    /// tests started failing on a file nothing had edited.
    ///
    /// [`Checker::ctx_decls_reached`] is the fix: a use checks its declaration
    /// if checking has not reached it yet.
    const SPREAD_BEFORE_ITS_BASE: &str = r#"
from "core/effect/lib.buri" import { Clock };

struct Frozen { at: I64 }

impl Clock for Frozen {
  fn nowMillis(self): I64 { self.at }
  fn sleepMillis(self, millis: Int): () { () }
}

context Deep {
  ..Base(),
}

context Base {
  Clock: Frozen { at: 7 },
}

fn reading<C: Clock>(ctx: C): I64 { ctx.nowMillis() }

test "the spread carries the base's binding" {
  let ctx = Deep();
  let _ = reading(ctx);
}
"#;

    #[test]
    fn a_context_may_spread_one_declared_after_it() {
        // A test source, because that is where a context may be built — and
        // where every one this found is written.
        let mut map = SourceMap::new();
        let analysis = crate::compiler::driver::analyze_snippet(
            &mut map,
            "resolve_test.buri",
            SPREAD_BEFORE_ITS_BASE,
            crate::compiler::modules::Role::TestSource,
        );
        let errors: Vec<String> = analysis
            .diagnostics
            .items
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect();
        // The whole claim: the use of `Deep` type-checks. The binding is
        // asserted below as well, so a `Deep` that compiled by binding nothing
        // would still fail here.
        assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
        let tables = analysis.checked.tables;
        let deep = tables
            .ctx_decls
            .iter()
            .find(|d| d.name == "Deep")
            .expect("the snippet declares `Deep`");
        let checked = deep.checked.expect("`Deep` was checked");
        assert_eq!(
            tables.ctx_type(checked.ty).bindings.len(),
            1,
            "`Deep` binds what the spread gave it",
        );
    }
}
