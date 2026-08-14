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
//! 5. Check each function body, independently and in any order (13.3).
//!
//! Only step 5 needs inference, and it never crosses a function boundary,
//! because top-level signatures are mandatory.

use crate::ast;
use crate::compile::{Loaded, Role};
use crate::diag::{Diagnostic, Diagnostics, Span};
use crate::hir;
use crate::stdlib;
use crate::types::*;
use crate::workspace::{PkgId, Workspace};
use std::collections::{BTreeSet, HashMap, HashSet};

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
    pub bodies: HashMap<FnId, hir::Body>,
    pub consts: HashMap<ConstId, hir::Expr>,
    /// `main`, when this compilation has one.
    pub entry: Option<FnId>,
    pub tests: Vec<TestCase>,
}

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
    pub bodies: HashMap<FnId, hir::Body>,
    pub const_values: HashMap<ConstId, hir::Expr>,
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
            bodies: HashMap::new(),
            const_values: HashMap::new(),
            entry: None,
            tests: Vec::new(),
            prim_module: ModuleId(u32::MAX),
            surfaces: HashMap::new(),
            known_traits: HashMap::new(),
            known_types: HashMap::new(),
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
        }
    }

    pub fn err(&mut self, span: Span, msg: impl Into<String>) -> &mut Diagnostic {
        self.diags.items.push(Diagnostic::error(span, msg));
        self.diags.items.last_mut().unwrap()
    }

    pub fn module(&self, id: ModuleId) -> &crate::compile::ModuleData {
        &self.loaded.modules[id.index()]
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
            let items = self.module(id).ast.items.clone();
            for (index, item) in items.iter().enumerate() {
                self.collect_item(id, index as u32, item);
            }
        }
    }

    fn collect_item(&mut self, module: ModuleId, index: u32, item: &ast::Item) {
        let ast_ref = AstRef { module, item: index, sub: u32::MAX };
        match item {
            ast::Item::Struct(d) => {
                let generics = self.generic_shells(&d.generics);
                let id = self.tables.add_tycon(TyCon {
                    name: d.name.name.clone(),
                    module,
                    generics,
                    def: TyDef::Struct { fields: Vec::new(), record: matches!(d.body, ast::StructBody::Record(_)) },
                    exported: d.exported,
                    span: d.name.span,
                });
                self.declare(module, &d.name, Sym::Ty(id), d.exported);
            }
            ast::Item::Enum(d) => {
                let generics = self.generic_shells(&d.generics);
                let id = self.tables.add_tycon(TyCon {
                    name: d.name.name.clone(),
                    module,
                    generics,
                    def: TyDef::Enum { variants: Vec::new() },
                    exported: d.exported,
                    span: d.name.span,
                });
                self.declare(module, &d.name, Sym::Ty(id), d.exported);
            }
            ast::Item::Trait(d) => {
                // `effect` may be declared only by platform modules.
                if d.is_effect && self.module(module).role != Role::Platform {
                    self.err(d.span, "only a platform module may declare an effect").note(
                        "the set of things a Buri program can do to the world is fixed by its \
                         platform rather than open-ended",
                    );
                }
                let generics = self.generic_shells(&d.generics);
                let id = self.tables.add_trait(TraitInfo {
                    name: d.name.name.clone(),
                    module,
                    generics,
                    methods: Vec::new(),
                    is_effect: d.is_effect,
                    exported: d.exported,
                    span: d.name.span,
                });
                self.declare(module, &d.name, Sym::Trait(id), d.exported);
            }
            ast::Item::Fn(d) => {
                let generics = self.generic_shells(&d.generics);
                let id = self.tables.add_fn(FnInfo {
                    name: d.name.name.clone(),
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
                self.declare(module, &d.name, Sym::Fn(id), d.exported);
            }
            ast::Item::Const(d) => {
                let id = self.tables.add_const(ConstInfo {
                    name: d.name.name.clone(),
                    module,
                    ty: Ty::Error,
                    exported: d.exported,
                    span: d.name.span,
                    ast: ast_ref,
                });
                self.declare(module, &d.name, Sym::Const(id), d.exported);
            }
            ast::Item::Context(d) => {
                // A `context` declaration may appear only in the module
                // exporting `main`, a test source, or a test-only module.
                let role = self.module(module).role;
                if !role.may_build_context() {
                    self.err(d.span, "a `context` declaration is not legal here").note(
                        "a context may be declared only in the module exporting `main`, in a \
                         test source, or in a test-only module",
                    );
                }
                if d.exported && !matches!(role, Role::TestOnly | Role::Platform) {
                    self.err(d.span, "a `context` may be exported only from a test-only module")
                        .note(
                            "a path containing a `testing` segment is importable only from a \
                             test source, which is what keeps the fixture out of a program",
                        );
                }
                let id = self.tables.add_ctx_decl(ContextDeclInfo {
                    name: d.name.name.clone(),
                    module,
                    exported: d.exported,
                    ty: None,
                    ctor: None,
                    span: d.name.span,
                    ast: ast_ref,
                });
                self.declare(module, &d.name, Sym::Context(id), d.exported);
            }
            // An inherent `impl` puts each of its exported methods into the
            // module's scope under its own name. Nothing resolves through that
            // entry — a method is found through its receiver — but a library's
            // `lib.buri` needs a name to re-export, so that its surface stays
            // one file you can read top to bottom.
            ast::Item::Impl(d) if d.trait_ty.is_none() => {
                let owner = d.self_ty.head_name().unwrap_or("?").to_string();
                let scope = &mut self.scopes[module.index()];
                for method in &d.methods {
                    let sym = Sym::Method(owner.clone());
                    scope.own.entry(method.name.name.clone()).or_insert(sym.clone());
                    if method.exported {
                        scope.exports.entry(method.name.name.clone()).or_insert(sym);
                    }
                }
            }
            ast::Item::TypeAlias(_)
            | ast::Item::Import(_)
            | ast::Item::ReExport(_)
            | ast::Item::Impl(_)
            | ast::Item::Derive(_)
            | ast::Item::Test(_) => {}
        }
    }

    fn generic_shells(&mut self, params: &[ast::GenericParam]) -> Vec<GenericInfo> {
        params
            .iter()
            .map(|p| GenericInfo { name: p.name.name.clone(), bounds: Vec::new(), span: p.span })
            .collect()
    }

    fn declare(&mut self, module: ModuleId, name: &ast::Ident, sym: Sym, exported: bool) {
        let scope = &mut self.scopes[module.index()];
        if let Some(existing) = scope.own.get(&name.name) {
            // Two methods of the same name on different types are the shape
            // `core/num`'s conversions have; anything else is a redeclaration.
            if let (Sym::Fn(a), Sym::Fn(b)) = (existing.clone(), &sym) {
                scope.own.insert(name.name.clone(), Sym::Overloaded(vec![a, *b]));
                if exported {
                    scope.exports.insert(name.name.clone(), Sym::Overloaded(vec![a, *b]));
                }
                return;
            }
            if let (Sym::Overloaded(mut fs), Sym::Fn(b)) = (existing.clone(), &sym) {
                fs.push(*b);
                scope.own.insert(name.name.clone(), Sym::Overloaded(fs.clone()));
                if exported {
                    scope.exports.insert(name.name.clone(), Sym::Overloaded(fs));
                }
                return;
            }
            let msg = format!("`{}` is declared twice in this module", name.name);
            self.diags.push(Diagnostic::error(name.span, msg));
            return;
        }
        scope.own.insert(name.name.clone(), sym.clone());
        if exported {
            scope.exports.insert(name.name.clone(), sym);
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: scopes
    // -----------------------------------------------------------------------

    fn resolve_scopes(&mut self) {
        // Type aliases are transparent, so they are not symbols: an alias
        // resolves to whatever it names at elaboration time. They are recorded
        // per module here so `Type::Named` can find them.
        for m in 0..self.loaded.modules.len() {
            let _id = ModuleId(m as u32);
            let own = self.scopes[m].own.clone();
            self.scopes[m].names = own;
        }

        // Prelude names sit under everything, so a module may shadow any of
        // them and importing one explicitly is harmless.
        let prelude: Vec<(String, ModuleId, String)> = stdlib::PRELUDE
            .iter()
            .filter_map(|(path, name)| {
                self.loaded.find(path).map(|m| (name.to_string(), m, name.to_string()))
            })
            .collect();
        for m in 0..self.loaded.modules.len() {
            for (local, from, name) in &prelude {
                if self.scopes[m].names.contains_key(local) {
                    continue;
                }
                if let Some(sym) = self.scopes[from.index()].exports.get(name).cloned() {
                    self.scopes[m].names.insert(local.clone(), sym);
                }
            }
        }

        for m in 0..self.loaded.modules.len() {
            let id = ModuleId(m as u32);
            let items = self.module(id).ast.items.clone();
            for item in &items {
                match item {
                    ast::Item::Import(imp) => self.apply_import(id, imp),
                    ast::Item::ReExport(re) => self.apply_reexport(id, re),
                    _ => {}
                }
            }
        }
    }

    fn apply_import(&mut self, module: ModuleId, imp: &ast::Import) {
        let Some(from) = self.loaded.find(&imp.path) else { return };
        match &imp.clause {
            ast::ImportClause::Namespace(alias) => {
                self.scopes[module.index()].namespaces.insert(alias.name.clone(), from);
            }
            ast::ImportClause::Named(specs) => {
                for spec in specs {
                    let Some(sym) = self.lookup_export(from, &spec.name.name) else {
                        let module_path = imp.path.clone();
                        let name = spec.name.name.clone();
                        let mut note = None;
                        // A name that exists but is not exported is a
                        // different mistake from a name that does not exist.
                        if self.scopes[from.index()].own.contains_key(&name) {
                            note = Some(format!(
                                "`{name}` is declared in \"{module_path}\" but not exported"
                            ));
                        } else if let Some(near) = self.nearest_export(from, &name) {
                            note = Some(format!("did you mean `{near}`?"));
                        }
                        let d = self.err(
                            spec.name.span,
                            format!("\"{module_path}\" does not export `{name}`"),
                        );
                        if let Some(n) = note {
                            d.notes.push(n);
                        }
                        continue;
                    };
                    let local = spec.local().clone();
                    // An explicit import wins over a prelude name.
                    self.scopes[module.index()].names.insert(local.name.clone(), sym);
                }
            }
        }
    }

    fn apply_reexport(&mut self, module: ModuleId, re: &ast::ReExport) {
        let Some(from) = self.loaded.find(&re.path) else { return };
        for spec in &re.specs {
            let Some(sym) = self.lookup_export(from, &spec.name.name) else {
                let path = re.path.clone();
                let name = spec.name.name.clone();
                self.err(spec.name.span, format!("\"{path}\" does not export `{name}`"))
                    .notes
                    .push("a re-export may name only what its module path exports".into());
                continue;
            };
            // Re-exporting a name does not import it — write both declarations
            // if the module also uses it.
            let local = spec.local().name.clone();
            self.scopes[module.index()].exports.insert(local, sym);
        }
    }

    /// Follows re-export chains, guarding against a cycle.
    fn lookup_export(&mut self, module: ModuleId, name: &str) -> Option<Sym> {
        if let Some(sym) = self.scopes[module.index()].exports.get(name) {
            return Some(sym.clone());
        }
        let key = (module, name.to_string());
        if self.resolving.contains(&key) {
            return None;
        }
        self.resolving.push(key);
        // The re-export may not have been applied yet, if the modules were
        // visited in an unhelpful order. Resolve it on demand.
        let items = self.module(module).ast.items.clone();
        let mut found = None;
        for item in &items {
            if let ast::Item::ReExport(re) = item {
                if re.specs.iter().any(|s| s.local().name == name) {
                    if let Some(from) = self.loaded.find(&re.path) {
                        let spec = re.specs.iter().find(|s| s.local().name == name).unwrap();
                        found = self.lookup_export(from, &spec.name.name);
                    }
                    break;
                }
            }
        }
        self.resolving.pop();
        if let Some(sym) = &found {
            self.scopes[module.index()].exports.insert(name.to_string(), sym.clone());
        }
        found
    }

    /// `lookup_export`, for the body checker.
    pub fn lookup_export_pub(&mut self, module: ModuleId, name: &str) -> Option<Sym> {
        self.lookup_export(module, name)
    }

    fn nearest_export(&self, module: ModuleId, name: &str) -> Option<String> {
        let names: Vec<&str> =
            self.scopes[module.index()].exports.keys().map(|s| s.as_str()).collect();
        crate::buildfile::nearest(name, &names).map(|s| s.to_string())
    }

    // -----------------------------------------------------------------------
    // Phase 3: signatures
    // -----------------------------------------------------------------------

    fn elaborate_signatures(&mut self) {
        // Bounds first: elaborating a signature may need to know whether a
        // parameter is effect-carrying.
        for m in 0..self.loaded.modules.len() {
            let id = ModuleId(m as u32);
            let items = self.module(id).ast.items.clone();
            for item in &items {
                match item {
                    ast::Item::Struct(d) => {
                        let Some(Sym::Ty(con)) = self.scopes[m].own.get(&d.name.name).cloned()
                        else {
                            continue;
                        };
                        let generics = self.elaborate_generics(id, &d.generics);
                        self.tables.tycons[con.index()].generics = generics.clone();
                        let def = match &d.body {
                            ast::StructBody::Record(fields) => TyDef::Struct {
                                fields: fields
                                    .iter()
                                    .map(|f| FieldInfo {
                                        name: f.name.name.clone(),
                                        ty: self.elaborate(id, &generics, &f.ty, Some(con)),
                                        exported: f.exported,
                                        span: f.span,
                                    })
                                    .collect(),
                                record: true,
                            },
                            ast::StructBody::Tuple(fields) => TyDef::Struct {
                                fields: fields
                                    .iter()
                                    .enumerate()
                                    .map(|(i, f)| FieldInfo {
                                        name: i.to_string(),
                                        ty: self.elaborate(id, &generics, &f.ty, Some(con)),
                                        exported: f.exported,
                                        span: f.span,
                                    })
                                    .collect(),
                                record: false,
                            },
                        };
                        self.tables.tycons[con.index()].def = def;
                        self.check_unique_field_names(con);
                    }
                    ast::Item::Enum(d) => {
                        let Some(Sym::Ty(con)) = self.scopes[m].own.get(&d.name.name).cloned()
                        else {
                            continue;
                        };
                        let generics = self.elaborate_generics(id, &d.generics);
                        self.tables.tycons[con.index()].generics = generics.clone();
                        let variants = d
                            .variants
                            .iter()
                            .map(|v| {
                                let (fields, record) = match &v.payload {
                                    ast::VariantPayload::None => (Vec::new(), false),
                                    ast::VariantPayload::Tuple(tys) => (
                                        tys.iter()
                                            .enumerate()
                                            .map(|(i, t)| FieldInfo {
                                                name: i.to_string(),
                                                ty: self.elaborate(id, &generics, t, Some(con)),
                                                exported: v.exported,
                                                span: t.span(),
                                            })
                                            .collect(),
                                        false,
                                    ),
                                    ast::VariantPayload::Record(fs) => (
                                        fs.iter()
                                            .map(|f| FieldInfo {
                                                name: f.name.name.clone(),
                                                ty: self.elaborate(id, &generics, &f.ty, Some(con)),
                                                exported: f.exported,
                                                span: f.span,
                                            })
                                            .collect(),
                                        true,
                                    ),
                                };
                                VariantInfo {
                                    name: v.name.name.clone(),
                                    fields,
                                    record,
                                    exported: v.exported,
                                    span: v.span,
                                }
                            })
                            .collect();
                        self.tables.tycons[con.index()].def = TyDef::Enum { variants };
                        self.check_unique_variant_names(con);
                        self.check_productive(con, d.span);
                    }
                    ast::Item::Trait(d) => {
                        let Some(Sym::Trait(tid)) = self.scopes[m].own.get(&d.name.name).cloned()
                        else {
                            continue;
                        };
                        let generics = self.elaborate_generics(id, &d.generics);
                        self.tables.traits[tid.index()].generics = generics.clone();
                        self.in_self_scope = true;
                        let methods = d
                            .methods
                            .iter()
                            .map(|sig| {
                                let mut g = generics.clone();
                                g.extend(self.elaborate_generics(id, &sig.generics));
                                TraitMethod {
                                    name: sig.name.name.clone(),
                                    generics: g.clone(),
                                    params: self.elaborate_params(id, &g, &sig.params),
                                    ret: self.elaborate(id, &g, &sig.ret, None),
                                    span: sig.span,
                                }
                            })
                            .collect();
                        self.tables.traits[tid.index()].methods = methods;
                        self.in_self_scope = false;
                    }
                    ast::Item::Fn(d) => {
                        let Some(sym) = self.scopes[m].own.get(&d.name.name).cloned() else {
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
                        self.elaborate_fn_signature(id, fid, d);
                    }
                    ast::Item::Const(d) => {
                        let Some(Sym::Const(cid)) = self.scopes[m].own.get(&d.name.name).cloned()
                        else {
                            continue;
                        };
                        let ty = self.elaborate(id, &[], &d.ty, None);
                        self.tables.consts[cid.index()].ty = ty;
                    }
                    _ => {}
                }
            }
        }
    }

    fn elaborate_fn_signature(&mut self, module: ModuleId, fid: FnId, d: &ast::FnDecl) {
        let generics = self.elaborate_generics(module, &d.generics);
        self.tables.fns[fid.index()].generics = generics.clone();
        let params = self.elaborate_params(module, &generics, &d.params);
        let ret = self.elaborate(module, &generics, &d.ret, None);
        self.tables.fns[fid.index()].params = params.clone();
        self.tables.fns[fid.index()].ret = ret;

        // A method is declared inside an `impl` block for its type, so a
        // `self` parameter at the top level has no receiver type to attach to.
        if let Some(first) = params.first() {
            if first.role == ParamRole::SelfParam {
                let n = d.name.name.clone();
                self.err(first.span, format!("`{n}` takes `self`, so it is a method"))
                    .note("a method is declared inside an `impl` block for its type, as in `impl Square { fn area(self: Square): Int { ... } }`");
            }
        }

        self.check_ctx_rule(fid, d);
        self.record_intrinsic(fid, module, d);
    }

    /// A effect-carrying parameter must be `self` or `ctx`, at most one of
    /// each. Both are fixed positions with fixed names, so you read the first
    /// two parameters and stop (SPEC 10.2).
    fn check_ctx_rule(&mut self, fid: FnId, d: &ast::FnDecl) {
        let info = self.tables.fun(fid).clone();
        let mut ctx_count = 0;
        let mut self_count = 0;
        for (i, p) in info.params.iter().enumerate() {
            match p.role {
                ParamRole::SelfParam => {
                    self_count += 1;
                    if i != 0 {
                        self.err(p.span, "`self` may appear only as the first parameter");
                    }
                }
                ParamRole::Ctx => {
                    ctx_count += 1;
                    let expected = if self_count > 0 { 1 } else { 0 };
                    if i != expected {
                        self.err(
                            p.span,
                            "`ctx` must come first, or immediately after `self`",
                        )
                        .notes
                        .push(
                            "the calling convention is receiver first, context second, \
                             everything else after"
                                .into(),
                        );
                    }
                    if ctx_count > 1 {
                        self.err(p.span, "a function has at most one `ctx` parameter").notes.push(
                            "a function cannot take two independent contexts; bundle them into \
                             one type instead"
                                .into(),
                        );
                    }
                }
                ParamRole::Normal => {
                    if self.tables.is_effect_carrying(&p.ty, &info.generics) {
                        let name = p.name.clone();
                        self.err(
                            p.span,
                            format!("`{name}` carries an effect, so it must be named `ctx`"),
                        )
                        .notes
                        .push(
                            "a function is effectful if and only if it has a `ctx` parameter or \
                             an effect-carrying `self`, which is what lets a reader stop after \
                             the first two parameters"
                                .into(),
                        );
                    }
                }
            }
        }
        // `main` takes no parameters, declares no generics, and returns
        // `Result<(), Str>`.
        if info.name == "main" && info.exported && self.module(info.module).role == Role::Entry {
            self.check_main_signature(fid, d);
        }
    }

    fn check_main_signature(&mut self, fid: FnId, d: &ast::FnDecl) {
        let info = self.tables.fun(fid).clone();
        if !info.params.is_empty() {
            self.err(d.span, "`main` takes no parameters").notes.push(
                "it builds the one context the program has, which is why there is no fake to \
                 pass it and why logic worth testing goes in a function it calls"
                    .into(),
            );
        }
        if !info.generics.is_empty() {
            self.err(d.span, "`main` declares no generic parameters");
        }
        let unit = Ty::Unit;
        let str_ty = self.tables.prim(Prim::Str);
        let ok = match &info.ret {
            Ty::Con(id, args) => {
                self.known_types.get("Result") == Some(id)
                    && args.len() == 2
                    && args[0] == unit
                    && args[1] == str_ty
            }
            _ => false,
        };
        if !ok && !info.ret.is_error() {
            self.err(d.ret.span(), "`main` must return `Result<(), Str>`")
                .notes
                .push("`.Ok(())` exits 0; `.Err(msg)` prints `msg` to stderr and exits 1".into());
        }
        self.entry = Some(fid);
    }

    fn record_intrinsic(&mut self, fid: FnId, module: ModuleId, d: &ast::FnDecl) {
        if d.body.is_some() {
            return;
        }
        let path = self.module(module).path.clone();
        if !path.starts_with("core/") {
            // Already reported by the parser.
            return;
        }
        self.tables.fns[fid.index()].intrinsic = true;
    }

    fn elaborate_generics(
        &mut self,
        module: ModuleId,
        params: &[ast::GenericParam],
    ) -> Vec<GenericInfo> {
        let mut out: Vec<GenericInfo> = Vec::new();
        for p in params {
            out.push(GenericInfo {
                name: p.name.name.clone(),
                bounds: Vec::new(),
                span: p.span,
            });
        }
        // Bounds may mention earlier parameters, so resolve them after the
        // names exist.
        for (i, p) in params.iter().enumerate() {
            let mut bounds = Vec::new();
            for b in &p.bounds {
                match self.resolve_trait(module, b) {
                    Some(t) => bounds.push(t),
                    None => {
                        let shown = b.head_name().unwrap_or("?").to_string();
                        self.err(b.span(), format!("`{shown}` is not a trait or effect"))
                            .notes
                            .push("a bound names a declared trait; there are no where clauses".into());
                    }
                }
            }
            out[i].bounds = bounds;
        }
        out
    }

    fn resolve_trait(&mut self, module: ModuleId, t: &ast::TypeExpr) -> Option<TraitId> {
        let ast::TypeExpr::Named { path, .. } = t else { return None };
        match self.resolve_path(module, path)? {
            Sym::Trait(id) => Some(id),
            _ => None,
        }
    }

    /// Resolves a possibly-qualified path (`Order`, `cap.Alloc`) in a module's
    /// scope.
    pub fn resolve_path(&mut self, module: ModuleId, path: &[ast::Ident]) -> Option<Sym> {
        if path.len() == 1 {
            return self.scopes[module.index()].names.get(&path[0].name).cloned();
        }
        // `ns.Name` where `ns` is a namespace import.
        let ns = self.scopes[module.index()].namespaces.get(&path[0].name).copied()?;
        let mut current = ns;
        for seg in &path[1..path.len() - 1] {
            let _ = seg;
            return None;
        }
        let last = &path[path.len() - 1].name;
        let sym = self.lookup_export(current, last);
        current = ns;
        let _ = current;
        sym
    }

    fn elaborate_params(
        &mut self,
        module: ModuleId,
        generics: &[GenericInfo],
        params: &[ast::Param],
    ) -> Vec<ParamInfo> {
        params
            .iter()
            .map(|p| ParamInfo {
                name: p.name.name.clone(),
                ty: self.elaborate(module, generics, &p.ty, None),
                role: match p.kind {
                    ast::ParamKind::SelfParam => ParamRole::SelfParam,
                    ast::ParamKind::CtxParam => ParamRole::Ctx,
                    ast::ParamKind::Normal => ParamRole::Normal,
                },
                span: p.span,
            })
            .collect()
    }

    /// Turns a syntactic type into a `Ty`. Aliases are transparent, so they
    /// are expanded here and never appear in a `Ty`.
    pub fn elaborate(
        &mut self,
        module: ModuleId,
        generics: &[GenericInfo],
        t: &ast::TypeExpr,
        inside: Option<TyConId>,
    ) -> Ty {
        match t {
            ast::TypeExpr::Unit { .. } => Ty::Unit,
            ast::TypeExpr::SelfType { span } => {
                // `Self` stands for the implementing type and is legal only
                // inside a trait or an `impl` body.
                if !self.in_self_scope {
                    self.err(*span, "`Self` is legal only inside a `trait` or `impl`")
                        .note("it stands for the implementing type, and there is none here");
                    return Ty::Error;
                }
                Ty::SelfTy
            }
            ast::TypeExpr::Array { elem, .. } => {
                Ty::Array(Box::new(self.elaborate(module, generics, elem, inside)))
            }
            ast::TypeExpr::Tuple { elems, .. } => Ty::Tuple(
                elems.iter().map(|e| self.elaborate(module, generics, e, inside)).collect(),
            ),
            ast::TypeExpr::Fn { params, ret, .. } => Ty::Fn(
                params.iter().map(|p| self.elaborate(module, generics, p, inside)).collect(),
                Box::new(self.elaborate(module, generics, ret, inside)),
            ),
            ast::TypeExpr::Named { path, args, span } => {
                let name = &path[path.len() - 1].name;
                // A generic parameter shadows everything.
                if path.len() == 1 {
                    if let Some(i) = generics.iter().position(|g| &g.name == name) {
                        if !args.is_empty() {
                            self.err(*span, format!("`{name}` is a type parameter and takes no type arguments"));
                        }
                        return Ty::Param(i as u32);
                    }
                }
                let elaborated_args: Vec<Ty> =
                    args.iter().map(|a| self.elaborate(module, generics, a, inside)).collect();

                // A type alias is transparent: `type UserId = Str` makes
                // `UserId` and `Str` the same type.
                if path.len() == 1 {
                    if let Some(ty) = self.expand_alias(module, name, &elaborated_args, *span) {
                        return ty;
                    }
                }
                if path.len() == 1 {
                    if let Some(id) = self.builtin_type(name) {
                        if !elaborated_args.is_empty() {
                            self.err(*span, format!("`{name}` takes no type arguments"));
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
                                *span,
                                format!("`{n}` takes {arity} type arguments, but {got} were given"),
                            );
                            return Ty::Error;
                        }
                        Ty::Con(id, elaborated_args)
                    }
                    Some(Sym::Trait(_)) => {
                        let shown = name.clone();
                        self.err(*span, format!("`{shown}` is a trait, not a type"))
                            .notes
                            .push("there are no trait objects; use a bound on a type parameter".into());
                        Ty::Error
                    }
                    _ => {
                        let shown = path
                            .iter()
                            .map(|p| p.name.clone())
                            .collect::<Vec<_>>()
                            .join(".");
                        let mut note = None;
                        if let Some(near) = self.nearest_type_name(module, name) {
                            note = Some(format!("did you mean `{near}`?"));
                        }
                        let d = self.err(*span, format!("there is no type `{shown}`"));
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
        let items = self.module(module).ast.items.clone();
        let alias = items.iter().find_map(|i| match i {
            ast::Item::TypeAlias(a) if a.name.name == name => Some(a.clone()),
            _ => None,
        })?;
        let generics: Vec<GenericInfo> = alias
            .generics
            .iter()
            .map(|g| GenericInfo { name: g.name.name.clone(), bounds: Vec::new(), span: g.span })
            .collect();
        if args.len() != generics.len() {
            self.err(span, format!("`{name}` takes {} type arguments", generics.len()));
            return Some(Ty::Error);
        }
        let body = self.elaborate(module, &generics, &alias.ty, None);
        Some(substitute(&body, args, None))
    }

    fn nearest_type_name(&self, module: ModuleId, name: &str) -> Option<String> {
        let mut candidates: Vec<String> = self.scopes[module.index()]
            .names
            .iter()
            .filter(|(_, s)| matches!(s, Sym::Ty(_)))
            .map(|(k, _)| k.clone())
            .collect();
        candidates.extend(Prim::all().iter().map(|p| p.name().to_string()));
        candidates.extend(["Int", "Float", "Uint", "Byte"].map(String::from));
        let refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
        crate::buildfile::nearest(name, &refs).map(|s| s.to_string())
    }

    /// Struct field names and enum variant names must be unique within their
    /// scope (SPEC 14.6).
    fn check_unique_field_names(&mut self, con: TyConId) {
        let fields = self.tables.tycon(con).fields().to_vec();
        let mut seen = HashSet::new();
        for f in &fields {
            if !seen.insert(f.name.clone()) {
                let n = f.name.clone();
                self.err(f.span, format!("field `{n}` is declared twice"));
            }
        }
    }

    /// Enum variant names must be unique within their scope (SPEC 14.6).
    fn check_unique_variant_names(&mut self, con: TyConId) {
        let variants = self.tables.tycon(con).variants().to_vec();
        let mut seen: Vec<String> = Vec::new();
        for v in &variants {
            if seen.contains(&v.name) {
                let n = v.name.clone();
                self.err(v.span, format!("variant `{n}` is declared twice"));
            }
            seen.push(v.name.clone());
            // A variant's own fields have to be unique too.
            let mut fields: Vec<String> = Vec::new();
            for f in &v.fields {
                if fields.contains(&f.name) {
                    let fname = f.name.clone();
                    let vname = v.name.clone();
                    self.err(f.span, format!("field `{fname}` of `{vname}` is declared twice"));
                }
                fields.push(f.name.clone());
            }
        }
    }

    /// A recursive enum must have at least one variant that does not recurse,
    /// or no value of it could ever be built (SPEC 14.14).
    fn check_productive(&mut self, con: TyConId, span: Span) {
        let variants = self.tables.tycon(con).variants().to_vec();
        if variants.is_empty() {
            return;
        }
        let productive = variants.iter().any(|v| {
            !v.fields.iter().any(|f| self.mentions_directly(&f.ty, con))
        });
        if !productive {
            let name = self.tables.tycon(con).name.clone();
            self.err(span, format!("`{name}` can never be constructed"))
                .notes
                .push("a recursive enum needs at least one variant that does not recurse".into());
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
        for (path, name) in stdlib::PRELUDE {
            if let Some(m) = self.loaded.find(path) {
                match self.scopes[m.index()].exports.get(*name) {
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
                match self.scopes[m.index()].exports.get(name) {
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
            let items = self.module(id).ast.items.clone();
            for (index, item) in items.iter().enumerate() {
                match item {
                    ast::Item::Impl(d) => self.register_impl(id, index as u32, d),
                    ast::Item::Derive(d) => self.register_derive(id, d),
                    _ => {}
                }
            }
        }
    }

    fn register_method(&mut self, con: TyConId, name: &str, fid: FnId, span: Span) {
        // A method may not share a name with a field of its `self` type.
        if self.tables.tycon(con).field_index(name).is_some() {
            let ty = self.tables.tycon(con).name.clone();
            self.err(span, format!("`{name}` is already a field of `{ty}`")).notes.push(
                "a `.` resolves to a field before a method, so the two may not share a name"
                    .into(),
            );
            return;
        }
        if let Some(prev) = self.tables.methods.get(&(con, name.to_string())) {
            let prev_span = self.tables.fun(*prev).span;
            let ty = self.tables.tycon(con).name.clone();
            self.err(span, format!("`{ty}` already has a method `{name}`"))
                .subs
                .push(crate::diag::SubSpan { span: prev_span, label: "first declared here".into() });
            return;
        }
        self.tables.methods.insert((con, name.to_string()), fid);
    }

    fn register_impl(&mut self, module: ModuleId, index: u32, d: &ast::ImplDecl) {
        self.in_self_scope = true;
        let generics = self.elaborate_generics(module, &d.generics);
        // No `for` clause: this declares the type's own methods rather than
        // conformance to anything.
        let Some(trait_ref) = &d.trait_ty else {
            self.register_inherent_impl(module, index, d, &generics);
            self.in_self_scope = false;
            return;
        };
        let Some(trait_id) = self.resolve_trait(module, trait_ref) else {
            let shown = trait_ref.head_name().unwrap_or("?").to_string();
            self.err(trait_ref.span(), format!("`{shown}` is not a trait or effect"));
            self.in_self_scope = false;
            return;
        };
        let self_ty = self.elaborate(module, &generics, &d.self_ty, None);
        let Some(self_con) = self_ty.head() else {
            if !self_ty.is_error() {
                self.err(d.self_ty.span(), "an `impl` names a declared type");
            }
            return;
        };

        // An `impl` may appear only in the defining module of its type.
        let owner = self.tables.tycon(self_con).module;
        let is_prim = matches!(self.tables.tycon(self_con).def, TyDef::Prim(_))
            && stdlib::defining_module(&self.tables.tycon(self_con).name)
                == self.module(module).path;
        if owner != module && !is_prim {
            let name = self.tables.tycon(self_con).name.clone();
            self.err(d.self_ty.span(), format!("`{name}` is not declared in this module")).notes.push(
                "there is no way to implement a trait for someone else's type, which is the same \
                 restriction that already applies to methods"
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
            )
            .subs
            .push(crate::diag::SubSpan { span: other_span, label: "the other one".into() });
        }

        if self.tables.impls.contains_key(&(trait_id, self_con)) {
            let t = self.tables.trait_(trait_id).name.clone();
            let c = self.tables.tycon(self_con).name.clone();
            self.err(d.span, format!("`{c}` already implements `{t}`"))
                .notes
                .push("there is exactly one candidate per (trait, type)".into());
            return;
        }

        // Register the methods, checked against the trait's signatures.
        let trait_methods = self.tables.trait_(trait_id).methods.clone();
        let mut supplied = vec![None; trait_methods.len()];
        for (sub, method) in d.methods.iter().enumerate() {
            let Some(slot) = trait_methods.iter().position(|m| m.name == method.name.name) else {
                let t = self.tables.trait_(trait_id).name.clone();
                let n = method.name.name.clone();
                self.err(method.name.span, format!("`{t}` declares no method `{n}`"));
                continue;
            };
            let mut g = generics.clone();
            g.extend(self.elaborate_generics(module, &method.generics));
            let params = self.elaborate_params(module, &g, &method.params);
            let ret = self.elaborate(module, &g, &method.ret, None);
            let fid = self.tables.add_fn(FnInfo {
                name: method.name.name.clone(),
                module,
                generics: g,
                params,
                ret,
                exported: true,
                span: method.name.span,
                self_ty: Some(self_con),
                impl_of: Some((trait_id, slot)),
                ast: AstRef { module, item: index, sub: sub as u32 },
                intrinsic: method.body.is_none(),
            });
            if supplied[slot].is_some() {
                let n = method.name.name.clone();
                self.err(method.name.span, format!("`{n}` is supplied twice"));
            }
            supplied[slot] = Some(fid);
            self.register_method(self_con, &method.name.name, fid, method.name.span);
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
            self.err(d.span, format!("`{c}`'s `impl {t}` is missing {}", missing.join(", ")));
        }

        let methods: Vec<FnId> = supplied.into_iter().map(|s| s.unwrap_or(FnId(u32::MAX))).collect();
        self.tables.impls.insert(
            (trait_id, self_con),
            ImplInfo { trait_id, self_con, methods, span: d.span, derived: false },
        );
        self.in_self_scope = false;
    }

    /// `impl Type { ... }` — the type's own methods. This is the only place a
    /// method may be declared, so a method always sits with the type it is a
    /// method of.
    fn register_inherent_impl(
        &mut self,
        module: ModuleId,
        index: u32,
        d: &ast::ImplDecl,
        generics: &[GenericInfo],
    ) {
        let self_ty = self.elaborate(module, generics, &d.self_ty, None);
        let target = match &self_ty {
            Ty::Con(con, _) => Some(*con),
            Ty::Array(_) => None,
            Ty::Error => return,
            other => {
                let shown = show(&self.tables, None, generics, other);
                self.err(d.self_ty.span(), format!("`{shown}` has no methods"))
                    .note("tuples, function types, and `Template` have no defining module");
                return;
            }
        };

        // An `impl` may appear only in the defining module of its type.
        if let Some(con) = target {
            let owner = self.tables.tycon(con).module;
            // A primitive has no declaring module of its own, so its methods
            // belong to the `core` module named for it and nowhere else.
            let is_prim = matches!(self.tables.tycon(con).def, TyDef::Prim(_))
                && stdlib::defining_module(&self.tables.tycon(con).name)
                    == self.module(module).path;
            if owner != module && !is_prim {
                let name = self.tables.tycon(con).name.clone();
                self.err(d.self_ty.span(), format!("`{name}` is not declared in this module"))
                    .note(
                        "there is no way to add a method to someone else's type, which is what                          keeps `x.f()` a single lookup",
                    );
                return;
            }
        }

        for (sub, method) in d.methods.iter().enumerate() {
            let mut g = generics.to_vec();
            g.extend(self.elaborate_generics(module, &method.generics));
            let params = self.elaborate_params(module, &g, &method.params);
            let ret = self.elaborate(module, &g, &method.ret, None);

            // A method is a function whose first parameter is `self`, and an
            // `impl` block is where one is declared — so anything else in here
            // is a mistake worth naming.
            match params.first() {
                Some(p) if p.role == ParamRole::SelfParam => {}
                _ => {
                    let n = method.name.name.clone();
                    self.err(
                        method.name.span,
                        format!("`{n}` is in an `impl` block but takes no `self`"),
                    )
                    .note("an `impl` block declares methods; a function with no receiver is declared at the top level");
                    continue;
                }
            }

            let fid = self.tables.add_fn(FnInfo {
                name: method.name.name.clone(),
                module,
                generics: g,
                params,
                ret,
                exported: method.exported,
                span: method.name.span,
                self_ty: target,
                impl_of: None,
                ast: AstRef { module, item: index, sub: sub as u32 },
                intrinsic: method.body.is_none(),
            });
            match target {
                Some(con) => self.register_method(con, &method.name.name, fid, method.name.span),
                // `[T]` has no type constructor; its methods live in a table
                // of their own, and only `core/list` may add to it.
                None => {
                    if self.module(module).path == "core/list" {
                        self.tables.array_methods.insert(method.name.name.clone(), fid);
                    } else {
                        self.err(d.self_ty.span(), "the defining module of `[T]` is `core/list`");
                    }
                }
            }
        }
    }

    fn register_derive(&mut self, module: ModuleId, d: &ast::DeriveDecl) {
        // A `derive` names a type *constructor*, not an instantiation of one:
        // `derive Eq for Option;` says every `Option<T>` compares whenever `T`
        // does. So the path is resolved directly rather than elaborated, which
        // would demand type arguments there is nothing to bind.
        let Some(self_con) = self.derive_target(module, &d.self_ty) else {
            return;
        };
        if self.tables.tycon(self_con).module != module {
            let name = self.tables.tycon(self_con).name.clone();
            self.err(d.self_ty.span(), format!("`{name}` is not declared in this module"));
            return;
        }
        for t in &d.traits {
            let Some(trait_id) = self.resolve_trait(module, t) else {
                let shown = t.head_name().unwrap_or("?").to_string();
                self.err(t.span(), format!("`{shown}` is not a trait"));
                continue;
            };
            let name = self.tables.trait_(trait_id).name.clone();
            // Derivation is available for Eq, Ord, Show, Hash, and the
            // operator traits. It is a fold over one type definition.
            const DERIVABLE: &[&str] = &[
                "Eq", "Ord", "Show", "Hash", "Add", "Sub", "Mul", "Div", "Rem", "Neg",
            ];
            if !DERIVABLE.contains(&name.as_str()) {
                self.err(t.span(), format!("`{name}` cannot be derived"))
                    .notes
                    .push(format!("derivable: {}", DERIVABLE.join(", ")));
                continue;
            }
            if self.tables.impls.contains_key(&(trait_id, self_con)) {
                let c = self.tables.tycon(self_con).name.clone();
                self.err(t.span(), format!("`{c}` already implements `{name}`"));
                continue;
            }
            self.tables.impls.insert(
                (trait_id, self_con),
                ImplInfo {
                    trait_id,
                    self_con,
                    methods: Vec::new(),
                    span: d.span,
                    derived: true,
                },
            );
        }
    }

    /// The type constructor a `derive` names.
    fn derive_target(&mut self, module: ModuleId, t: &ast::TypeExpr) -> Option<TyConId> {
        let ast::TypeExpr::Named { path, args, span } = t else {
            self.err(t.span(), "a `derive` names a declared type");
            return None;
        };
        match self.resolve_path(module, path) {
            Some(Sym::Ty(con)) => {
                // Naming the arguments is allowed, and has to be consistent.
                if !args.is_empty() && args.len() != self.tables.tycon(con).arity() {
                    let n = self.tables.tycon(con).name.clone();
                    let arity = self.tables.tycon(con).arity();
                    self.err(*span, format!("`{n}` takes {arity} type arguments"));
                }
                Some(con)
            }
            _ => {
                let shown = path.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(".");
                self.err(*span, format!("there is no type `{shown}`"));
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
            .filter(|(_, i)| i.derived)
            .map(|((t, c), i)| (*t, *c, i.span))
            .collect();
        let mut sorted = derived;
        sorted.sort_by_key(|(t, c, _)| (t.0, c.0));

        for (tr, con, span) in sorted {
            let tycon = self.tables.tycon(con).clone();
            // A generic type's components are checked at each use site, where
            // the arguments are known; here only the ones that cannot depend
            // on an argument are decidable.
            let components: Vec<(String, Ty)> = match &tycon.def {
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
                    let c = tycon.name.clone();
                    let shown = show(&self.tables, None, &tycon.generics, &ty);
                    self.err(
                        span,
                        format!("`{c}` cannot derive `{t}`: `{name}` has type `{shown}`"),
                    )
                    .note(format!("a derived implementation is a fold over the type's components, and `{shown}` does not satisfy `{t}`"));
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
        for m in 0..self.loaded.modules.len() {
            let module = &self.loaded.modules[m];
            let Some(pkg) = module.pkg else { continue };
            let is_surface = self
                .ws
                .map(|ws| ws.pkg(pkg).label() == module.path)
                .unwrap_or(false);
            if !is_surface {
                continue;
            }
            let names: HashSet<String> =
                self.scopes[m].exports.keys().cloned().collect();
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
            let items = self.module(id).ast.items.clone();
            for item in &items {
                // `test` declarations are legal only in a test source.
                if let ast::Item::Test(t) = item {
                    if role != Role::TestSource {
                        self.err(t.span, "a `test` declaration is legal only in a test source")
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
        crate::infer::check_all(self);
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
