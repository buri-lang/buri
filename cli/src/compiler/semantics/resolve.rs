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

use crate::build::workspace::{PkgId, Workspace};
use crate::compiler::modules::{Loaded, Role};
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::*;
use crate::compiler::standard_library;
use crate::diagnostics::{Diagnostic, Diagnostics, Invariant as _, Span, SubSpan};
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

pub struct Checked {
    pub tables: Tables,
    pub scopes: Vec<ModuleScope>,
    pub bodies: HashMap<FnId, typed::Body>,
    pub consts: HashMap<ConstId, typed::Expr>,
    /// `main`, when this compilation has one.
    pub entry: Option<FnId>,
    pub tests: Vec<TestCase>,
    /// Per package, the set of names its `lib.buri` puts on the surface. The
    /// checker needs it to filter method resolution; `unreachable-export` needs
    /// it to ask the opposite question — what is exported and reaches nobody.
    pub surfaces: HashMap<PkgId, HashSet<String>>,
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
    pub surfaces: HashMap<PkgId, HashSet<String>>,
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
    /// The free functions whose signatures are elaborated and whose `ctx` rule
    /// is still to be checked, in declaration order — which is the order the
    /// check used to run in, so the diagnostics are the same ones in the same
    /// sequence. Held rather than checked in place because rule 26 asks two
    /// questions no earlier pass can answer (see `Checker::run`).
    pending_ctx_rules: Vec<(FnId, ModuleId, u32)>,
    /// Guards re-export cycles.
    resolving: Vec<(ModuleId, String)>,
    /// Whether the declaration being elaborated is a trait, an effect, or an
    /// `impl`, which are the only places `Self` means anything.
    in_self_scope: bool,
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
            known_traits: HashMap::default(),
            known_types: HashMap::default(),
            option_con: None,
            result_con: None,
            order_con: None,
            pending_ctx_rules: Vec::new(),
            resolving: Vec::new(),
            in_self_scope: false,
        }
    }

    pub fn run(mut self) -> Checked {
        self.register_primitives();
        self.collect_declarations();
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
        Checked {
            tables: self.tables,
            scopes: self.scopes,
            bodies: self.bodies,
            consts: self.const_values,
            entry: self.entry,
            tests: self.tests,
            surfaces: self.surfaces,
        }
    }

    pub fn err(&mut self, span: Span, msg: impl Into<String>) -> &mut Diagnostic {
        self.diags.items.push(Diagnostic::error(span, msg));
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
                    self.err(d.span, "only a platform module may declare an effect").code("effect-outside-platform")
                        .note(
                            "the set of things a Buri program can do to the world is fixed by \
                             its platform rather than open-ended",
                        )
                        .fix("declare it as a plain `trait`, or move it into a platform module");
                }
                let generics = self.generic_shells(module, &d.generics);
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
            tree::Item::Const(d) => {
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
                    self.err(d.span, "a `context` declaration is not legal here").code("context-not-allowed")
                        .fix("move it into the module that exports `main`, or into a test-only module")
                        .note(
                        "a context may be declared only in the module exporting `main`, in a \
                         test source, or in a test-only module",
                    );
                }
                if d.exported && !matches!(role, Role::TestOnly | Role::Platform) {
                    self.err(d.span, "a `context` may be exported only from a test-only module")
                        .fix("drop the `export`, or move it into a test-only module")
                        .note(
                            "a path containing a `testing` segment is importable only from a \
                             test source, which is what keeps the fixture out of a program",
                        );
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
            tree::Item::TypeAlias(_)
            | tree::Item::Import(_)
            | tree::Item::ReExport(_)
            | tree::Item::Impl(_)
            | tree::Item::Derive(_)
            | tree::Item::Test(_) => {}
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
            let msg = format!("`{text}` is declared twice in this module");
            self.diags.push(
                Diagnostic::error(name.span, msg)
                    .with_fix("rename one of them; a name has one meaning in a module"),
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

    fn resolve_scopes(&mut self) {
        // Type aliases are transparent, so they are not symbols: an alias
        // resolves to whatever it names at elaboration time. They are recorded
        // per module here so `Type::Named` can find them.
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
                        let d = self.err(
                            spec.name.span,
                            format!("\"{module_path}\" does not export `{name}`"),
                        ).code("no-such-export");
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
                self.err(spec.name.span, format!("\"{path}\" does not export `{name}`")).code("no-such-export")
                    .fix(format!("add `export` to `{name}`'s declaration in \"{path}\", or drop it from this list"))
                    .notes
                    .push("a re-export may name only what its module path exports".into());
                continue;
            };
            // Re-exporting a name does not import it — write both declarations
            // if the module also uses it.
            let local = t.name(spec.local()).to_string();
            self.scope_mut(module).exports.insert(local, sym);
        }
    }

    /// Follows re-export chains, guarding against a cycle.
    fn lookup_export(&mut self, module: ModuleId, name: &str) -> Option<Sym> {
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

    /// `lookup_export`, for the body checker.
    pub fn lookup_export_pub(&mut self, module: ModuleId, name: &str) -> Option<Sym> {
        self.lookup_export(module, name)
    }

    fn nearest_export(&self, module: ModuleId, name: &str) -> Option<String> {
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
                                                exported: v.exported,
                                                span: t.type_span(*ty),
                                            })
                                            .collect(),
                                        false,
                                    ),
                                    tree::VariantPayload::Record(fs) => (
                                        fs.iter()
                                            .map(|f| FieldInfo {
                                                name: t.name(f.name).to_string(),
                                                ty: self.elaborate(id, &generics, f.ty),
                                                exported: f.exported,
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
                                    exported: v.exported,
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
                        let methods = self.enter_self_scope(|s| {
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
                    }
                    tree::Item::Fn(d) => {
                        let Some(sym) = self.scope(id).own.get(t.name(d.name)).cloned() else {
                            continue;
                        };
                        let fid = match sym {
                            Sym::Fn(f) => f,
                            Sym::Overloaded(fs) => {
                                match fs.iter().find(|f| self.tables.fun(**f).span == d.name.span) {
                                    Some(f) => *f,
                                    None => continue,
                                }
                            }
                            _ => continue,
                        };
                        self.elaborate_fn_signature(id, index as u32, fid, d);
                    }
                    tree::Item::Const(d) => {
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
        self.tables.fun_mut(fid).generics = generics.clone();
        let params = self.elaborate_params(module, &generics, &d.params);
        let ret = self.elaborate(module, &generics, d.ret);
        self.tables.fun_mut(fid).params = params.clone();
        self.tables.fun_mut(fid).ret = ret;

        // A method is declared inside an `impl` block for its type, so a
        // `self` parameter at the top level has no receiver type to attach to.
        if let Some(first) = params.first() {
            if first.role == ParamRole::SelfParam {
                let n = self.name_text(module, d.name).to_string();
                self.err(first.span, format!("`{n}` takes `self`, so it is a method")).code("method-declared-free")
                    .fix("move it into an `impl` block for its type, as in `impl Square { fn area(self: Square): Int { ... } }`")
                    .note("a method is found through its receiver's type, so it is declared with that type");
            }
        }

        self.pending_ctx_rules.push((fid, module, item));
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
    fn check_ctx_rules(&mut self) {
        for (fid, module, item) in std::mem::take(&mut self.pending_ctx_rules) {
            let Some(tree::Item::Fn(d)) = self.module(module).ast.items.get(item as usize) else {
                continue;
            };
            self.check_ctx_rule(fid, d);
        }
    }

    /// A effect-carrying parameter must be `self` or `ctx`, at most one of
    /// each. Both are fixed positions with fixed names, so you read the first
    /// two parameters and stop (SPEC 10.2).
    fn check_ctx_rule(&mut self, fid: FnId, d: &tree::FnDecl) {
        // Whether there is anything to say is decided from a borrow; only
        // saying it needs an owned `FnInfo`. This runs once per function in
        // the program, the standard library included, and the answer is almost
        // always "nothing" — so the copy belongs on the reporting path.
        if self.violates_ctx_rule(fid) {
            self.report_ctx_rule(fid);
        }
        // `main` takes no parameters, declares no generics, and returns
        // `Result<(), Str>`.
        let info = self.tables.fun(fid);
        let (name_is_main, exported, module) =
            (info.name == "main", info.exported, info.module);
        if name_is_main && exported && self.module(module).role == Role::Entry {
            self.check_main_signature(fid, d);
        }
    }

    /// The predicate half of the rule above: does any parameter break it?
    /// Written as a mirror of the loop that reports, so the two cannot drift —
    /// a `true` here is exactly one diagnostic or more there.
    fn violates_ctx_rule(&self, fid: FnId) -> bool {
        let info = self.tables.fun(fid);
        let mut ctx_count: usize = 0;
        let mut self_count: usize = 0;
        for (i, p) in info.params.iter().enumerate() {
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
                    if self.tables.is_effect_carrying(&p.ty, &info.generics) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn report_ctx_rule(&mut self, fid: FnId) {
        let info = self.tables.fun(fid).clone();
        let mut ctx_count: usize = 0;
        let mut self_count: usize = 0;
        for (i, p) in info.params.iter().enumerate() {
            match p.role {
                ParamRole::SelfParam => {
                    self_count = self_count.saturating_add(1);
                    if i != 0 {
                        self.err(p.span, "`self` may appear only as the first parameter").code("self-not-first")
                            .fix("move it to the front, or rename it if this parameter is not the receiver");
                    }
                }
                ParamRole::Ctx => {
                    ctx_count = ctx_count.saturating_add(1);
                    let expected = if self_count > 0 { 1 } else { 0 };
                    if i != expected {
                        self.err(
                            p.span,
                            "`ctx` must come first, or immediately after `self`",
                        ).code("ctx-not-first")
                        .fix("move `ctx` to that position")
                        .notes
                        .push(
                            "the calling convention is receiver first, context second, \
                             everything else after"
                                .into(),
                        );
                    }
                    if ctx_count > 1 {
                        self.err(p.span, "a function has at most one `ctx` parameter")
                            .fix("combine the two into one context, which is what `context { ..base, ... }` is for")
                            .notes
                            .push(
                            "a function cannot take two independent contexts; bundle them into \
                             one type instead"
                                .into(),
                        );
                    }
                }
                ParamRole::Normal => {
                    if self.tables.is_effect_carrying(&p.ty, &info.generics) {
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
                        let d = self.err(
                            p.span,
                            format!("`{name}` carries an effect, so it must be named `ctx`"),
                        );
                        d.code("effect-param-not-ctx");
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
        let info = self.tables.fun(fid).clone();
        if !info.params.is_empty() {
            self.err(d.span, "`main` takes no parameters").code("main-signature")
                .fix("drop them, and build the context `main` needs in its own body")
                .notes
                .push(
                "it builds the one context the program has, which is why there is no fake to \
                 pass it and why logic worth testing goes in a function it calls"
                    .into(),
            );
        }
        if !info.generics.is_empty() {
            self.err(d.span, "`main` declares no generic parameters").code("main-signature")
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
            self.err(at, "`main` must return `Result<(), Str>`").code("main-signature")
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
        self.tables.fun_mut(fid).intrinsic = true;
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
                        self.err(t.type_span(*b), format!("`{shown}` is not a trait or effect")).code("not-a-trait")
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

    /// Resolves a possibly-qualified path (`Order`, `cap.Alloc`) in a module's
    /// scope.
    pub fn resolve_path(&mut self, module: ModuleId, path: &[flat::Loc]) -> Option<Sym> {
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

    fn elaborate_params(
        &mut self,
        module: ModuleId,
        generics: &[GenericInfo],
        params: &[tree::Param],
    ) -> Vec<ParamInfo> {
        let t = self.tree(module);
        params
            .iter()
            .map(|p| ParamInfo {
                name: t.name(p.name).to_string(),
                ty: self.elaborate(module, generics, p.ty),
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
                // inside a trait or an `impl` body.
                if !self.in_self_scope {
                    self.err(span, "`Self` is legal only inside a `trait` or `impl`").code("self-type-outside-impl")
                        .fix("name the type itself here")
                        .note("`Self` stands for the implementing type, and there is none here");
                    return Ty::Error;
                }
                Ty::SelfTy
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
                            self.err(span, format!("`{name}` is a type parameter and takes no type arguments"))
                    .fix("drop the arguments; a type parameter stands for one type already");
                        }
                        return Ty::Param(i as u32);
                    }
                }
                let elaborated_args: Vec<Ty> =
                    args.iter().map(|a| self.elaborate(module, generics, *a)).collect();

                // A type alias is transparent: `type UserId = Str` makes
                // `UserId` and `Str` the same type.
                if path.len() == 1 {
                    if let Some(ty) = self.expand_alias(module, name, &elaborated_args, span) {
                        return ty;
                    }
                }
                if path.len() == 1 {
                    if let Some(id) = self.builtin_type(name) {
                        if !elaborated_args.is_empty() {
                            self.err(span, format!("`{name}` takes no type arguments"))
                        .fix("drop them");
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
                            self.err(
                                span,
                                format!("`{n}` takes {arity} type arguments, but {got} were given"),
                            )
                            .mismatch(arity.to_string(), got.to_string())
                            .fix(format!("supply exactly {arity}"));
                            return Ty::Error;
                        }
                        Ty::Con(id, elaborated_args)
                    }
                    Some(Sym::Trait(_)) => {
                        let shown = name.to_string();
                        self.err(span, format!("`{shown}` is a trait, not a type"))
                    .fix(format!("use it as a bound — `<T: {shown}>` — and name `T` here"))
                            .notes
                            .push("there are no trait objects; use a bound on a type parameter".into());
                        Ty::Error
                    }
                    _ => {
                        let shown = t.path_text(path);
                        let mut note = None;
                        if let Some(near) = self.nearest_type_name(module, name) {
                            note = Some(format!("did you mean `{near}`?"));
                        }
                        let d = self.err(span, format!("there is no type `{shown}`")).code("unresolved-type");
                d.fix("declare it, import it, or correct the spelling");
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
        // Every named type in the module reaches this, so the miss — no alias
        // by that name, which is the overwhelmingly common case — has to cost
        // a scan of item discriminants and nothing else. It used to clone the
        // module's whole syntax tree first.
        let items = &self.module(module).ast.items;
        let t = self.tree(module);
        let alias = items.iter().find_map(|i| match i {
            tree::Item::TypeAlias(a) if t.name(a.name) == name => Some(a),
            _ => None,
        })?;
        let generics: Vec<GenericInfo> = alias
            .generics
            .iter()
            .map(|g| GenericInfo { name: t.name(g.name).to_string(), bounds: Vec::new(), span: g.span })
            .collect();
        if args.len() != generics.len() {
            self.err(span, format!("`{name}` takes {} type arguments", generics.len()))
                .fix(format!("supply exactly {}", generics.len()));
            return Some(Ty::Error);
        }
        let body = self.elaborate(module, &generics, alias.ty);
        Some(substitute(&body, args, None))
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
            self.err(span, format!("field `{n}` is declared twice")).code("duplicate-declaration")
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
                    self.err(span, format!("variant `{n}` is declared twice")).code("duplicate-declaration")
                        .fix("rename one of them; `match` tells variants apart by name");
                }
                Dup::Field(span, fname, vname) => {
                    self.err(span, format!("field `{fname}` of `{vname}` is declared twice")).code("duplicate-declaration")
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
            self.err(span, format!("`{name}` can never be constructed")).code("uninhabited")
                .fix(format!(
                    "give `{name}` a variant that does not mention itself, the way `.None` \
                     terminates an `Option`"
                ))
                .notes
                .push("every variant recurses, so building one would need one already".into());
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
        if let Some(m) = self.loaded.find("core/json") {
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
        if let Some(m) = self.loaded.find("core/cap") {
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

    fn register_conformance(&mut self) {
        // Methods declared as ordinary functions with a `self` parameter.
        for f in 0..self.tables.fns.len() {
            let fid = FnId(f as u32);
            let info = self.tables.fun(fid).clone();
            if info.impl_of.is_some() {
                continue;
            }
            let Some(first) = info.params.first() else { continue };
            if first.role != ParamRole::SelfParam {
                continue;
            }
            match (&first.ty, info.self_ty) {
                (_, Some(con)) => {
                    self.register_method(con, &info.name, fid, info.span);
                }
                (Ty::Array(_), None) => {
                    if self.module(info.module).path == "core/list" {
                        self.tables.array_methods.insert(info.name.clone(), fid);
                    }
                }
                _ => {}
            }
        }

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
            self.err(span, format!("`{name}` is already a field of `{ty}`")).code("duplicate-field")
                .fix("rename the method, or rename the field")
                .notes
                .push(
                    "a `.` resolves to a field before a method, so the two may not share a name"
                        .into(),
                );
            return;
        }
        if let Some(prev) = self.tables.method(con, name) {
            let prev_span = self.tables.fun(prev).span;
            let ty = self.tables.tycon(con).name.clone();
            self.err(span, format!("`{ty}` already has a method `{name}`"))
                .fix("rename one of them")
                .subs
                .push(SubSpan { span: prev_span, label: "first declared here".into() });
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
        self.enter_self_scope(|s| s.register_impl_body(module, index, d));
    }

    /// Runs `f` with `Self` in scope, restoring the previous scope afterwards
    /// however `f` returns.
    fn enter_self_scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = std::mem::replace(&mut self.in_self_scope, true);
        let out = f(self);
        self.in_self_scope = outer;
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
            self.err(t.type_span(trait_ref), format!("`{shown}` is not a trait or effect")).code("not-a-trait")
                .fix(format!(
                    "name a declared trait or effect after `impl`, or drop the `for` clause if \
                     `{shown}` was meant to be the type whose own methods these are"
                ));
            return;
        };
        let self_ty = self.elaborate(module, &generics, d.self_ty);
        let Some(self_con) = self_ty.head() else {
            if !self_ty.is_error() {
                self.err(self.tree(module).type_span(d.self_ty), "an `impl` names a declared type")
                    .fix("name a struct or enum this module declares");
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
            self.err(self.tree(module).type_span(d.self_ty), format!("`{name}` is not declared in this module")).code("unresolved-name")
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
            self.err(d.span, format!("`{tname}` is derived, not implemented"))
                .code("derive-only-trait")
                .fix(format!("write `derive {tname} for {c};` instead"))
                .notes
                .push(
                    "a derived encoder is a fold over the type's shape, and would encode a \
                     hand-written one structurally rather than calling it — so an `impl` \
                     would be obeyed at the top of a document and ignored inside it"
                        .into(),
                );
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
            self.err(
                d.span,
                format!("`{name}` cannot implement both the effect `{eff}` and the trait `{tr}`"),
            ).code("effect-and-trait")
            .fix("split it in two: a type is either part of the world or part of your data")
            .subs
            .push(SubSpan { span: other_span, label: "the other one".into() });
        }

        if self.tables.impls.contains_key(&(trait_id, self_con)) {
            let t = self.tables.trait_(trait_id).name.clone();
            let c = self.tables.tycon(self_con).name.clone();
            self.err(d.span, format!("`{c}` already implements `{t}`"))
                .fix("delete one of the two, or merge them")
                .notes
                .push("there is exactly one candidate per (trait, type)".into());
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
                self.err(method.name.span, format!("`{t}` declares no method `{n}`"))
                    .fix(format!("remove it, or move it into an inherent `impl` block for the type — `{t}` supplies only what it declares"));
                continue;
            };
            let mut g = generics.clone();
            g.extend(self.elaborate_generics(module, &method.generics));
            let params = self.elaborate_params(module, &g, &method.params);
            let ret = self.elaborate(module, &g, method.ret);
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
            // `slot` is a position in `trait_methods`, which is what `supplied`
            // was sized to, so it is always in range.
            let already = supplied.get(slot).is_some_and(Option::is_some);
            if already {
                let n = mname.to_string();
                self.err(method.name.span, format!("`{n}` is supplied twice"))
                    .fix("delete one of the two");
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
            self.err(d.span, format!("`{c}`'s `impl {t}` is missing {missing}")).code("incomplete-impl")
                .fix(format!("add {missing} to the block, with the signature `{t}` declares"))
                .note("an `impl` supplies every method its trait declares");
        }

        self.tables.add_impl(ImplInfo {
            trait_id,
            self_con,
            body: ImplBody::Written(supplied),
            span: d.span,
        });
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
        let target = match &self_ty {
            Ty::Con(con, _) => Some(*con),
            Ty::Array(_) => None,
            Ty::Error => return,
            other => {
                let shown = show(&self.tables, None, generics, other);
                self.err(self.tree(module).type_span(d.self_ty), format!("`{shown}` has no methods")).code("no-such-method")
                    .fix("write a free function instead, and call it as one")
                    .note("tuples, function types, and `Template` have no defining module");
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
                self.err(self.tree(module).type_span(d.self_ty), format!("`{name}` is not declared in this module")).code("unresolved-name")
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
                    self.err(
                        method.name.span,
                        format!("`{n}` is in an `impl` block but takes no `self`"),
                    ).code("impl-fn-without-self")
                    .fix("give it a `self` parameter, or move it out of the `impl` block")
                    .note("an `impl` block declares methods; a function with no receiver is declared at the top level");
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
            match target {
                Some(con) => self.register_method(con, mname, fid, method.name.span),
                // `[T]` has no type constructor; its methods live in a table
                // of their own, and only `core/list` may add to it.
                None => {
                    if self.module(module).path == "core/list" {
                        self.tables.array_methods.insert(mname.to_string(), fid);
                    } else {
                        self.err(self.tree(module).type_span(d.self_ty), "the defining module of `[T]` is `core/list`")
                            .fix("write a free function over the array instead");
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
            self.err(self.tree(module).type_span(d.self_ty), format!("`{name}` is not declared in this module")).code("unresolved-name")
                .fix(format!("move the `derive` into `{name}`'s own module"));
            return;
        }
        for ty in self.tree(module).type_list(d.traits) {
            let at = self.tree(module).type_span(*ty);
            let Some(trait_id) = self.resolve_trait(module, *ty) else {
                let shown = self.tree(module).type_head(*ty).unwrap_or("?").to_string();
                self.err(at, format!("`{shown}` is not a trait"))
                    .fix("name a declared trait; `derive` generates a trait's methods");
                continue;
            };
            let name = self.tables.trait_(trait_id).name.clone();
            if !DERIVABLE.contains(&name.as_str()) {
                self.err(at, format!("`{name}` cannot be derived"))
                    .fix(format!("write `impl {name} for ... {{ ... }}` by hand"))
                    .notes
                    .push(format!(
                        "derivable: {}",
                        crate::diagnostics::names(
                            &DERIVABLE.iter().map(|s| s.to_string()).collect::<Vec<_>>()
                        )
                    ));
                continue;
            }
            if self.tables.impls.contains_key(&(trait_id, self_con)) {
                let c = self.tables.tycon(self_con).name.clone();
                self.err(at, format!("`{c}` already implements `{name}`"))
                    .fix("drop it from this `derive`, or delete the hand-written `impl`");
                continue;
            }
            self.tables.add_impl(ImplInfo {
                trait_id,
                self_con,
                body: ImplBody::Derived,
                span: d.span,
            });
        }
    }

    /// The type constructor a `derive` names.
    fn derive_target(&mut self, module: ModuleId, id: TypeId) -> Option<TyConId> {
        let flat::TypeView::Named { path, args, span } = self.tree(module).ty(id) else {
            self.err(self.tree(module).type_span(id), "a `derive` names a declared type")
                .fix("name a struct or enum this module declares");
            return None;
        };
        match self.resolve_path(module, path) {
            Some(Sym::Ty(con)) => {
                // Naming the arguments is allowed, and has to be consistent.
                if !args.is_empty() && args.len() != self.tables.tycon(con).arity() {
                    let n = self.tables.tycon(con).name.clone();
                    let arity = self.tables.tycon(con).arity();
                    self.err(span, format!("`{n}` takes {arity} type arguments"))
                        .fix(format!("a `derive` names the constructor alone: `derive ... for {n};`"));
                }
                Some(con)
            }
            _ => {
                let shown = self.tree(module).path_text(path);
                self.err(span, format!("there is no type `{shown}`")).code("unresolved-type")
                    .fix("declare it, import it, or correct the spelling");
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
                    self.err(
                        span,
                        format!("`{c}` cannot derive `{t}`: `{name}` has type `{shown}`"),
                    ).code("underivable")
                    .note(format!("a derived implementation is a fold over the type's components, and `{shown}` does not satisfy `{t}`"))
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
                .map(|ws| ws.pkg(pkg).label() == module.path)
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
                        self.err(t.span, format!("this file already has a test called {name:?}"))
                        .code("duplicate-test-name")
                        .label("declared again here")
                        .sub(first, "first declared here")
                        .fix("rename one of them, so each test in this file has its own title")
                        .notes
                        .push(
                            "a title is how a failing test is named in the report and how \
                             `--filter` selects one, so two tests sharing a title in one file \
                             cannot be told apart"
                                .into(),
                        );
                    }
                    if role != Role::TestSource {
                        self.err(t.span, "a `test` declaration is legal only in a test source").code("test-outside-test-source")
                            .fix("move it into a file listed in the target's `test.sources`")
                            .notes
                            .push(
                                "a module is a test source because a rule lists it in \
                                 `test.sources`; that is the only thing that makes one"
                                    .into(),
                            );
                    }
                }
                // A test source may not `export`, and may not be imported.
                if role == Role::TestSource && item.is_exported() {
                    self.err(item.span(), "a test source may not export")
                        .fix("drop the `export`; move the declaration into a library if something else needs it")
                        .notes
                        .push(
                            "test sources are compiled independently and are not modules anybody \
                             can name; shared helpers belong in a library"
                                .into(),
                        );
                }
            }
        }
    }

    fn check_bodies(&mut self) {
        crate::compiler::semantics::inference::check_all(self);
    }

    /// Every trait a type is known to satisfy, for diagnostics.
    pub fn traits_of(&self, con: TyConId) -> BTreeSet<String> {
        self.tables
            .impls
            .keys()
            .filter(|(_, c)| *c == con)
            .map(|(t, _)| self.tables.trait_(*t).name.clone())
            .collect()
    }
}
