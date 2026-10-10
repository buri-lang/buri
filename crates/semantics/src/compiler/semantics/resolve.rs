//! Name resolution and signature elaboration.
//!
//! Runs in phases, and the phase split is the compile-speed argument of
//! guides/compile-speed.md made concrete:
//!
//! 0. Register the primitives, their methods, and their trait impls.
//! 1. Register every module's own declarations, names and arities only.
//! 2. Resolve imports and re-exports into per-module scopes.
//! 3. Elaborate the types in every signature. This is a module's entire
//!    inter-module surface (13.4).
//! 4. Register `impl` and `derive`, and build the method table.
//!    4½. Check the `ctx` rule, which needs both of the two above finished: which
//!    positions of a type constructor hand a value back (step 3's type bodies)
//!    and which types implement an effect (step 4's conformances).
//! 5. Check each function body, independently and in any order (13.3).
//!
//! Only step 5 needs inference, and it never crosses a function boundary,
//! because top-level signatures are mandatory.

use crate::build::buildfile::Platform;
use crate::build::workspace::{PackageId, Packages, RuleKind, TargetId};
use crate::compiler::modules::{Loaded, Role};
use crate::compiler::semantics::layered::{IdMap, Layered};
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::*;
use crate::compiler::standard_library;
use crate::diagnostics::{counted, were_given, Diagnostic, Diagnostics, FileId, Invariant as _, Span, SecondarySpan};
use crate::parsing::flat::{self, TypeId};
use crate::parsing::tree;
use crate::hash::{Map as HashMap, Set as HashSet};
use std::collections::BTreeSet;

mod platforms;
pub use platforms::{declared_host, is_effect_package_module, is_effect_package_path, js_structs};

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
    /// method, ambiguous as a free function — which is the shape `core/number`'s
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

/// One module's own type, by name, when this compilation loaded the module.
///
/// `None` for every compilation that did not, which is every program that is
/// not a user interface.
pub fn own_type(loaded: &Loaded, scopes: &Layered<ModuleScope>, module: &str, name: &str) -> Option<TyConId> {
    match own_sym(loaded, scopes, module, name)? {
        Sym::Ty(id) => Some(*id),
        _ => None,
    }
}

/// One module's own function, by name, when this compilation loaded the
/// module.
pub fn own_fn(loaded: &Loaded, scopes: &Layered<ModuleScope>, module: &str, name: &str) -> Option<FnId> {
    match own_sym(loaded, scopes, module, name)? {
        Sym::Fn(id) => Some(*id),
        _ => None,
    }
}

fn own_sym<'s>(loaded: &Loaded, scopes: &'s Layered<ModuleScope>, module: &str, name: &str) -> Option<&'s Sym> {
    scopes.get(loaded.find(module)?.index())?.own.get(name)
}

#[derive(Default, Clone)]
pub struct ModuleScope {
    /// What this module imports by name. With `own` and `prelude` below, in
    /// that order, it is everything visible unqualified inside the module:
    /// see [`ModuleScope::name`].
    imported: HashMap<String, Sym>,
    /// What every prelude name refers to. One table for every module of a
    /// compilation, rather than a copy of it in each.
    prelude: std::sync::Arc<Prelude>,
    /// What this module publishes.
    pub exports: HashMap<String, Sym>,
    /// Names declared in this module's own source, before imports.
    pub own: HashMap<String, Sym>,
    /// Namespace imports, by local name.
    pub namespaces: HashMap<String, ModuleId>,
}

/// What every prelude name refers to, by name.
pub type Prelude = HashMap<String, Sym>;

impl ModuleScope {
    /// What `name` means unqualified inside this module: an explicit import
    /// first, then the module's own declaration, then the prelude.
    pub fn name(&self, name: &str) -> Option<&Sym> {
        self.imported.get(name).or_else(|| self.own.get(name)).or_else(|| self.prelude.get(name))
    }

    /// Every name visible unqualified inside this module, once each, with what
    /// [`ModuleScope::name`] says it means. In no particular order.
    pub fn visible(&self) -> impl Iterator<Item = (&str, &Sym)> {
        let own = self.own.iter().filter(|(k, _)| !self.imported.contains_key(*k));
        let prelude = self
            .prelude
            .iter()
            .filter(|(k, _)| !self.imported.contains_key(*k) && !self.own.contains_key(*k));
        self.imported.iter().chain(own).chain(prelude).map(|(k, s)| (k.as_str(), s))
    }
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

/// Every checked function body, by the function's id.
pub type BodyMap = IdMap<FnId, std::sync::Arc<typed::Body>>;
/// Every checked module-level `let`, by the constant's id.
pub type ConstMap = IdMap<ConstId, typed::Expr>;

pub struct Checked {
    pub tables: Tables,
    pub scopes: Layered<ModuleScope>,
    pub bodies: BodyMap,
    pub consts: ConstMap,
    /// `main`, when this compilation has one.
    pub entry: Option<FnId>,
    /// Every exported free function of the entry module, by name.
    ///
    /// A build looks its output's `entry` up here and monomorphizes from what
    /// it finds, so one `main.buri` holding a page's `main` and a worker's
    /// `fetch` produces two artifacts, each holding only what its own entry
    /// reaches.
    pub entries: HashMap<String, FnId>,
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
    /// `ui/node`'s `NodeKind`, when this compilation loaded it. The link step
    /// needs it to tell which interactive elements a program builds, which is
    /// what decides the reset the stylesheet opens with.
    pub node_con: Option<TyConId>,
    /// `ui/node`'s `Role`, when this compilation loaded it. `region` takes its
    /// role as a parameter, so a list is named by a literal at the call site
    /// rather than by a `NodeKind` one inside the constructor.
    pub role_con: Option<TyConId>,
    /// Types by well-known name, `platform/effect`'s `Request` and `Response`
    /// among them.
    pub known_types: HashMap<String, TyConId>,
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
    "Equal", "Ordered", "Show", "Hash", "ToJson", "FromJson", "Add", "Subtract", "Multiply", "Divide", "Remainder", "Negate",
    "Flags",
];

/// The module `Flags` is declared in. A `derive Flags` changes how the type is
/// stored, so only that trait, and not one a program happens to call `Flags`,
/// may be derived under the name.
pub const FLAGS_MODULE: &str = "core/flags";

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
    pub ws: Option<&'a dyn Packages>,
    pub diags: &'a mut Diagnostics,
    pub tables: Tables,
    pub scopes: Layered<ModuleScope>,
    pub bodies: BodyMap,
    pub const_values: ConstMap,
    pub entry: Option<FnId>,
    /// See [`Checked::entries`].
    pub entries: HashMap<String, FnId>,
    /// Which of those an output actually enters through — plus `main`, which
    /// every analysis treats as one whether or not a build file was read.
    ///
    /// [`Checker::entries`] is the wider table: it holds every exported
    /// function of `main.buri`, so a build can look one up and say what it
    /// found. This one is the set that may build a context. Both this and
    /// [`Checker::module_entries`] are per entry module, so several binaries
    /// can share one compilation and each `main` builds its own context.
    pub entry_points: HashSet<(ModuleId, String)>,
    /// Every exported free function of each entry module, by name.
    pub module_entries: HashMap<ModuleId, HashMap<String, FnId>>,
    pub tests: Vec<TestCase>,
    /// The synthetic module the primitives are declared in.
    pub prim_module: ModuleId,
    /// Per package, the set of names its `lib.buri` puts on the surface. A
    /// method call from outside a library resolves only to these.
    pub surfaces: HashMap<PackageId, HashSet<String>>,
    /// Per package, the set of names its `testing/lib.buri` puts on the
    /// surface of `//pkg/testing`. A method declared under `testing/` is
    /// filtered by this one rather than by [`Checker::surfaces`].
    pub testing_surfaces: HashMap<PackageId, HashSet<String>>,
    /// See [`Checked::ctx_rebindings`].
    pub ctx_rebindings: Vec<Span>,
    /// Traits by well-known name, for operators and `derive`.
    pub known_traits: HashMap<String, TraitId>,
    /// Enums by well-known name.
    pub known_types: HashMap<String, TyConId>,
    /// The three of those the body checker asks about by name on hot paths —
    /// `?` and every comparison. They are settled by
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
    /// What checking the compilation's leading standard library modules
    /// already settled, when this analysis starts from it. See [`Base`].
    base: Option<&'a Base>,
    /// Inference's per-body buffers, between bodies.
    pub(crate) scratch: crate::compiler::semantics::inference::Scratch<'a>,
}

/// What checking the standard library modules a compilation opens with left
/// behind, for an analysis to start from rather than redo.
///
/// Every compilation loads the same modules first — `Loader::load_unit` loads
/// the prelude and the built-in types' modules before anything of the
/// repository's, and a snippet loads the whole library before its own text —
/// and their text is compiled into this binary. So what the passes below make
/// of them is the same every time, and `compiler::snapshot` keeps it once per
/// process. [`Checker::resume`] then runs each pass over the remaining modules
/// only: their ids continue where these stop, so every type constructor,
/// trait and module keeps the id a whole run would have given it.
///
/// It is sound because nothing a later module declares can change what these
/// modules mean. They import nothing but each other; an `impl` lives in its
/// type's own module, so none of their types gains a conformance later; and
/// `analyze_std_module` already holds that each module checks on top of the
/// prelude alone.
pub struct Base {
    /// How many of the compilation's leading modules this covers.
    modules: usize,
    tables: Tables,
    scopes: Layered<ModuleScope>,
    bodies: BodyMap,
    const_values: ConstMap,
    known_traits: HashMap<String, TraitId>,
    known_types: HashMap<String, TyConId>,
    /// What every prelude name refers to, which is the same in every module
    /// and is found in these modules' scopes.
    prelude: std::sync::Arc<Prelude>,
    prim_module: ModuleId,
    ctx_rebindings: Vec<Span>,
    ctx_decls_reached: HashSet<ContextDeclId>,
    /// Whether every function body of these modules was checked, or only the
    /// ones a scoped analysis must have whatever it asked for.
    ///
    /// Where they all were, the passes after checking walked them too, and
    /// the two fields below are what those passes left.
    bodies_checked: bool,
    /// The functions among these that wait on `core/lazy`'s `load`.
    waiting: HashSet<FnId>,
    /// What style extraction made of these modules' constants and bodies.
    styled: crate::compiler::semantics::styles::Styled,
}

/// Which constants and bodies the passes after checking — icons, reactive
/// builders, style extraction — walk.
///
/// Those of the files a scoped analysis asked about, or every one; and of
/// those, none a [`Base`] walked already. A base's bodies call nothing
/// declared after them, so what a pass reports about one of them, or rewrites
/// in it, is the same whatever comes after.
pub struct Walked {
    only: Option<Vec<FileId>>,
    /// The functions and constants whose ids are below these were walked by
    /// the base.
    fns_from: usize,
    consts_from: usize,
}

impl Walked {
    /// The bodies the base didn't walk, in id order.
    pub fn unsettled<'m>(
        &self,
        bodies: &'m BodyMap,
    ) -> impl Iterator<Item = (FnId, &'m std::sync::Arc<typed::Body>)> {
        bodies.iter_from(self.fns_from)
    }

    /// The bodies a pass walks, in id order.
    pub fn functions<'m>(
        &'m self,
        tables: &'m Tables,
        bodies: &'m BodyMap,
    ) -> impl Iterator<Item = (FnId, &'m std::sync::Arc<typed::Body>)> {
        self.unsettled(bodies).filter(|(id, _)| self.wants(tables.fn_info(*id).span.file))
    }

    /// The constants a pass walks, in id order.
    pub fn constants<'m>(
        &'m self,
        tables: &'m Tables,
        consts: &'m ConstMap,
    ) -> impl Iterator<Item = (ConstId, &'m typed::Expr)> {
        consts.iter_from(self.consts_from).filter(|(id, _)| self.wants(tables.const_(*id).span.file))
    }

    fn wants(&self, file: FileId) -> bool {
        self.only.as_ref().is_none_or(|files| files.contains(&file))
    }
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
        ws: Option<&'a dyn Packages>,
        diags: &'a mut Diagnostics,
    ) -> Checker<'a> {
        let mut scopes = Layered::default();
        scopes.resize_with(loaded.modules.len(), ModuleScope::default);
        Checker {
            loaded,
            ws,
            diags,
            tables: Tables::default(),
            scopes,
            bodies: IdMap::default(),
            const_values: IdMap::default(),
            entry: None,
            entries: HashMap::default(),
            entry_points: HashSet::default(),
            module_entries: HashMap::default(),
            tests: Vec::new(),
            prim_module: ModuleId(u32::MAX),
            surfaces: HashMap::default(),
            testing_surfaces: HashMap::default(),
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
            base: None,
            scratch: Default::default(),
        }
    }

    /// A checker for a compilation whose leading modules are the ones `base`
    /// was made from, which runs every pass over the rest only.
    ///
    /// The caller loaded `base`'s modules first, in its order, which is what
    /// makes the ids it holds the ids of this compilation's modules too.
    pub fn resume(
        loaded: &'a Loaded,
        ws: Option<&'a dyn Packages>,
        diags: &'a mut Diagnostics,
        base: &'a Base,
    ) -> Checker<'a> {
        let mut c = Checker::new(loaded, ws, diags);
        // The base's entries are shared rather than copied: these start
        // empty, and read through to the base for every id below theirs.
        let mut scopes = base.scopes.layer();
        scopes.resize_with(loaded.modules.len(), ModuleScope::default);
        c.scopes = scopes;
        c.tables = base.tables.layer();
        c.bodies = base.bodies.layer();
        c.const_values = base.const_values.layer();
        c.known_traits = base.known_traits.clone();
        c.known_types = base.known_types.clone();
        c.prim_module = base.prim_module;
        c.ctx_rebindings = base.ctx_rebindings.clone();
        c.ctx_decls_reached = base.ctx_decls_reached.clone();
        c.base = Some(base);
        c
    }

    /// Runs every pass up to and including the bodies, and keeps what they
    /// made for analyses to [`resume`](Checker::resume) from.
    ///
    /// Where every body was checked, the passes after them — icons, reactive
    /// builders, styles — walk them here too, and what they found is kept
    /// beside the rest; an analysis then walks only its own (see
    /// [`Walked`]).
    pub fn base(mut self) -> Base {
        self.check_through_bodies();
        let bodies_checked = matches!(self.wanted, Bodies::All);
        // The passes after checking, over these modules alone, when every body
        // is here to walk. Style extraction rewrites what it walks, and an
        // analysis folds its own styles through the bodies as they were
        // before that — so the pass runs on copies, and keeps only what it
        // changed.
        self.tables.freeze();
        self.scopes.freeze();
        self.bodies.freeze();
        self.const_values.freeze();
        let (mut waiting, mut styled) = Default::default();
        if bodies_checked {
            let walked = self.walked();
            waiting = self.check_icons_and_builders(&walked, &HashSet::default());
            let (mut bodies, mut consts) = (self.bodies.layer(), self.const_values.layer());
            let (found, _) = crate::compiler::semantics::styles::run(
                self.loaded,
                &self.tables,
                &self.scopes,
                &mut bodies,
                &mut consts,
                self.diags,
                &walked,
            );
            // What the pass rewrote is what it wrote into its own layer.
            styled = crate::compiler::semantics::styles::Styled {
                bodies: bodies.written().map(|(id, body)| (id, std::sync::Arc::clone(body))).collect(),
                consts: consts.written().map(|(id, init)| (id, init.clone())).collect(),
                ..found
            };
        }
        let prelude = self.prelude();
        Base {
            modules: self.loaded.modules.len(),
            bodies_checked,
            waiting,
            styled,
            tables: self.tables,
            scopes: self.scopes,
            bodies: self.bodies,
            const_values: self.const_values,
            prelude,
            known_traits: self.known_traits,
            known_types: self.known_types,
            prim_module: self.prim_module,
            ctx_rebindings: self.ctx_rebindings,
            ctx_decls_reached: self.ctx_decls_reached,
        }
    }

    /// The first module this checker elaborates: the one after its base's, or
    /// the first.
    fn first_module(&self) -> usize {
        self.base.map_or(0, |b| b.modules)
    }

    /// The modules this checker elaborates, as ids.
    fn own_modules(&self) -> impl Iterator<Item = ModuleId> {
        (self.first_module()..self.loaded.modules.len()).map(|m| ModuleId(m as u32))
    }

    /// For a function the base declared, whether the base checked its body;
    /// `None` for one this checker declared.
    pub(crate) fn settled_by_base(&self, fid: FnId) -> Option<bool> {
        let base = self.base?;
        (fid.index() < base.tables.fns.len()).then_some(base.bodies_checked)
    }

    /// The well-known names the base registered, when there is one.
    pub(crate) fn base_known_traits(&self) -> Option<&'a HashMap<String, TraitId>> {
        self.base.map(|b| &b.known_traits)
    }

    /// How many constants and `context` declarations the base checked: the
    /// ones whose ids come first.
    pub(crate) fn base_counts(&self) -> (usize, usize) {
        self.base.map_or((0, 0), |b| (b.tables.consts.len(), b.tables.ctx_decls.len()))
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
        self.check_through_bodies();
        // Last, because it reads every checked body and rewrites the ones that
        // hold a static style. It must also run before `monomorphize` inlines a
        // constant, or a module-level `let` of `Style` would be extracted once
        // per use site instead of once.
        let walked = self.walked();
        let settled = self.base.filter(|b| b.bodies_checked);
        let waiting = settled.map(|b| b.waiting.clone()).unwrap_or_default();
        self.check_icons_and_builders(&walked, &waiting);
        let (found, style_con) = crate::compiler::semantics::styles::run(
            self.loaded,
            &self.tables,
            &self.scopes,
            &mut self.bodies,
            &mut self.const_values,
            self.diags,
            &walked,
        );
        let styles = match settled {
            Some(base) => found.after(&base.styled, &mut self.bodies, &mut self.const_values),
            None => found.const_rules.into_iter().chain(found.body_rules).collect(),
        };
        // `ui/theme`'s one opaque type, looked up the way `styles::run` looks
        // up `Style`: by module path in the loaded set, then by name in that
        // module's own scope. `None` for every compilation that did not load
        // the module, which is every program that is not a user interface.
        let theme_con = own_type(self.loaded, &self.scopes, "ui/theme", "Theme");
        // `ui/node`'s private tree enum, looked up the same way. Private is no
        // obstacle: this is the module's own scope, which is what a name is
        // declared into before anything is exported.
        let node_con = own_type(self.loaded, &self.scopes, "ui/node", "NodeKind");
        let role_con = own_type(self.loaded, &self.scopes, "ui/node", "Role");
        Checked {
            tables: self.tables,
            scopes: self.scopes,
            bodies: self.bodies,
            consts: self.const_values,
            entry: self.entry,
            entries: self.entries,
            tests: self.tests,
            styles,
            style_con,
            theme_con,
            node_con,
            role_con,
            known_types: self.known_types,
            surfaces: self.surfaces,
            ctx_rebindings: self.ctx_rebindings,
        }
    }

    /// What the passes after checking walk: the bodies this analysis asked
    /// for, less the ones its base walked already.
    fn walked(&self) -> Walked {
        let settled = self.base.filter(|b| b.bodies_checked);
        Walked {
            only: match &self.wanted {
                Bodies::All => None,
                Bodies::In(files) => Some(files.clone()),
            },
            fns_from: settled.map_or(0, |b| b.tables.fns.len()),
            consts_from: settled.map_or(0, |b| b.tables.consts.len()),
        }
    }

    /// The two passes after checking that only report, and answer which
    /// functions wait on a `load` — starting from `waiting`, the base's answer.
    fn check_icons_and_builders(&mut self, walked: &Walked, waiting: &HashSet<FnId>) -> HashSet<FnId> {
        // Before extraction, so the folder reads bodies nothing has rewritten.
        crate::compiler::semantics::icons::run(
            self.loaded,
            &self.tables,
            &self.scopes,
            &self.bodies,
            &self.const_values,
            self.diags,
            walked,
        );
        crate::compiler::semantics::decorative::run(
            self.loaded,
            &self.tables,
            &self.scopes,
            &self.bodies,
            &self.const_values,
            self.diags,
            walked,
        );
        // A `load` reached synchronously from a reactive builder answers a
        // promise the renderer cannot render (#152), so it is refused here where
        // the builder's body is still a lambda in the typed tree.
        crate::compiler::semantics::reactive::run(
            self.loaded,
            &self.tables,
            &self.scopes,
            &self.bodies,
            self.diags,
            walked,
            waiting,
        )
    }

    /// Every pass up to and including the bodies, over the modules this
    /// checker owns (see [`Base`]).
    fn check_through_bodies(&mut self) {
        if self.base.is_none() {
            self.register_primitives();
        }
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
        // After the entry table is complete and before any body is checked, so
        // an output naming a function that is not there is printed above its
        // consequences.
        self.check_declared_entries();
        self.check_effect_test_implementations();
        self.register_primitive_methods();
        self.check_derives();
        self.compute_surfaces();
        self.check_module_rules();
        self.check_bodies();
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
        for id in self.own_modules() {
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
                let generics = self.generic_shells(module, t.list(d.generics));
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
                let generics = self.generic_shells(module, t.list(d.generics));
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
                // `effect` may be declared only by the bundled platform modules
                // and, in a repository, by an effect package under
                // `//platform/effect/`.
                if d.is_effect
                    && self.module(module).role != Role::Platform
                    && !is_effect_package_module(self.ws, self.module(module))
                {
                    self.templated("effect-outside-effect-directory", d.span);
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
                // `Show.show<C: Allocator>`, `Ui.memo<T>` — and are what a trait
                // parameter would have been used for.
                let generics: Vec<GenericInfo> = self.generic_shells(module, t.list(d.generics));
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
                // Built as the `Arc` the declaration keeps, and nothing at all
                // for a function with no generics, which is most of them.
                let written = t.list(d.generics);
                let generics = if written.is_empty() {
                    std::sync::Arc::default()
                } else {
                    self.generic_shells(module, written)
                };
                let id = self.tables.add_fn(FnInfo {
                    name: t.name(d.name).to_string(),
                    module,
                    generics,
                    params: Vec::new(),
                    ret: Ty::ERROR,
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
                    ty: Ty::ERROR,
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
                    self.templated("misplaced-context-declaration", d.span);
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
                for method in t.list(d.methods) {
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

    fn generic_shells<C: FromIterator<GenericInfo>>(
        &mut self,
        module: ModuleId,
        params: &[tree::GenericParam],
    ) -> C {
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
        // A type annotation reads a built-in name before any declaration
        // (`elaborate`), so a type declared under one could be built but never
        // named.
        if matches!(sym, Sym::Ty(_) | Sym::Alias(..)) && self.builtin_type(text).is_some() {
            self.diags.push(
                Diagnostic::templated("built-in-type-name", name.span)
                    .with_bind("name", text.to_string()),
            );
        }
        let scope = self.scope_mut(module);
        if let Some(existing) = scope.own.get(text) {
            // An inherent method's entry is only a name to re-export, and
            // nothing resolves through it, so a declaration of the same name
            // takes its place, as it does when it comes first.
            if matches!(existing, Sym::Method(_)) {
                scope.own.insert(text.to_string(), sym.clone());
                if exported {
                    scope.exports.insert(text.to_string(), sym);
                }
                return;
            }
            // Two methods of the same name on different types are the shape
            // `core/number`'s conversions have; anything else is a redeclaration.
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
                Diagnostic::templated("duplicate-declaration", name.span)
                    .with_bind("declaration", format!("`{text}`"))
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
        // Everything a module declares is visible unqualified inside it,
        // before its imports add to that.
        let first = self.first_module();
        debug_assert_eq!(self.scopes.base_len(), first);
        // Prelude names sit under everything, so a module may shadow any of
        // them and importing one explicitly is harmless. What each one refers
        // to is the same in every module, so it is looked up once rather than
        // once per module — and once per process where a base looked it up.
        let prelude = match self.base {
            Some(base) => std::sync::Arc::clone(&base.prelude),
            None => self.prelude(),
        };
        for scope in self.scopes.own_mut() {
            scope.prelude = std::sync::Arc::clone(&prelude);
        }

        for id in self.own_modules() {
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
                let platform = self.loaded.module(from).path.clone();
                for spec in t.list(*specs) {
                    // A platform's entry is a declaration for the program to
                    // fill, with no body of its own: the program exports one of
                    // the same name, and nothing calls the declaration.
                    let name = t.name(spec.name);
                    if standard_library::is_entry_declaration(&platform, name) {
                        self.templated("entry-declaration-imported", spec.name.span)
                            .bind("entry", name.to_string())
                            .bind("platform", platform.clone());
                        continue;
                    }
                    let Some(sym) = self.lookup_export(from, t.name(spec.name)) else {
                        let module_path = imp.path.clone();
                        let name = t.name(spec.name).to_string();
                        let mut note = None;
                        let mut near = None;
                        // A name that exists but is not exported is a
                        // different mistake from a name that does not exist.
                        if self.scope(from).own.contains_key(&name) {
                            note = Some(format!(
                                "`{name}` is declared in \"{module_path}\" but not exported"
                            ));
                        } else if let Some(n) = self.nearest_export(from, &name) {
                            note = Some(format!("did you mean `{n}`?"));
                            near = Some(n);
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
                            .templated("unknown-export", spec.name.span)
                            .bind("path", module_path.clone())
                            .bind("name", name.clone());
                        if let Some(n) = &near {
                            d.fix(crate::diagnostics::candidate_fix(
                                n,
                                &Self::where_the_surface_is(&module_path),
                            ));
                        } else if is_surface {
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
                    self.scope_mut(module).imported.insert(local, sym);
                }
            }
        }
    }

    fn apply_reexport(&mut self, module: ModuleId, re: &tree::ReExport) {
        let Some(from) = self.loaded.find(&re.path) else { return };
        let t = self.tree(module);
        for spec in t.list(re.specs) {
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
                } else if let Some(n) = self.nearest_export(from, &name) {
                    note = Some(format!("did you mean `{n}`?"));
                    crate::diagnostics::candidate_fix(&n, &Self::where_the_surface_is(&path))
                } else {
                    note = None;
                    format!("check the spelling, or drop `{name}` from this list")
                };
                let d = self
                    .templated("unknown-export", spec.name.span)
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
                let Some(spec) = t.list(re.specs).iter().find(|s| t.name(s.local()) == name) else {
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

    /// Where a reader finds a module's whole surface, for the second half of a
    /// fix that has a candidate to offer. The toolchain's own modules have a
    /// page; a repository's module has its `export` declarations and nothing
    /// else, and `buri docs` does not read them.
    fn where_the_surface_is(path: &str) -> String {
        match standard_library::is_std_path(path) {
            true => format!("`buri docs {path}` lists what the module exports"),
            false => format!("the `export` declarations in \"{path}\" are its whole surface"),
        }
    }

    pub(crate) fn nearest_export(&self, module: ModuleId, name: &str) -> Option<String> {
        let names: Vec<&str> =
            self.scope(module).exports.keys().map(|s| s.as_str()).collect();
        crate::build::buildfile::nearest(name, &names).map(|s| s.to_string())
    }

    /// Reports naming a member `ns` does not export, in whichever position the
    /// name was written: `ns.member(...)`, `ns.member` as a value, or
    /// `ns.Member` as a type.
    ///
    /// One reporter for all three, because the answer is the same in all
    /// three and it is the module's surface: the module the import named, what
    /// it does export, the nearest of those to what was written, and the page
    /// that lists the rest. Reporting the *base* instead — "there is nothing
    /// named `fs` in scope" — sends a reader after a missing import when the
    /// import is the one part that was right.
    pub(crate) fn report_no_such_member(&mut self, ns: ModuleId, name: &str, span: Span) {
        let path = self.loaded.module(ns).path.clone();
        let mut exports: Vec<String> = self.scope(ns).exports.keys().cloned().collect();
        exports.sort();
        // A surface of a dozen names is worth reading here; `core/list`'s is
        // not, and the fix already points at the page that lists it.
        let listed = match exports.len() {
            0 => "nothing".to_string(),
            1..=12 => crate::diagnostics::names(&exports),
            n => format!("{n} names"),
        };
        let near = self.nearest_export(ns, name);
        let d = self
            .templated("unknown-export", span)
            .bind("path", path.clone())
            .bind("name", name)
            .note(format!("the module exports {listed}"))
            .fix(format!("correct the member name, or check `buri docs {path}`"));
        if let Some(n) = near {
            d.notes.push(format!("did you mean `{n}`?"));
            d.fix(crate::diagnostics::candidate_fix(&n, &Self::where_the_surface_is(&path)));
        }
    }

    // -----------------------------------------------------------------------
    // Phase 3: signatures
    // -----------------------------------------------------------------------

    fn elaborate_signatures(&mut self) {
        // Bounds first: elaborating a signature may need to know whether a
        // parameter is effect-carrying.
        for id in self.own_modules() {
            let items = &self.module(id).ast.items;
            let t = self.tree(id);
            for (index, item) in items.iter().enumerate() {
                match item {
                    tree::Item::Struct(d) => {
                        let Some(Sym::Ty(con)) = self.scope(id).own.get(t.name(d.name)).cloned()
                        else {
                            continue;
                        };
                        let generics = self.elaborate_generics(id, t.list(d.generics));
                        self.tables.tycon_mut(con).generics = generics.clone();
                        let def = match &d.body {
                            tree::StructBody::Record(fields) => TyDef::Struct {
                                fields: t.list(*fields)
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
                                fields: t.list(*fields)
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
                        let generics = self.elaborate_generics(id, t.list(d.generics));
                        self.tables.tycon_mut(con).generics = generics.clone();
                        let variants = t
                            .list(d.variants)
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
                                        t.list(*fs).iter()
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
                        let generics = self.elaborate_generics(id, t.list(d.generics));
                        self.tables.trait_mut(tid).generics = generics.clone();
                        // A trait's `Self` is whatever type implements it,
                        // which is not known here and so stays abstract.
                        let methods = self.enter_self_scope(Ty::SELF, |s| {
                            t.list(d.methods)
                                .iter()
                                .map(|sig| {
                                    let mut g = generics.clone();
                                    g.extend(s.elaborate_generics(id, t.list(sig.generics)));
                                    TraitMethod {
                                        name: t.name(sig.name).to_string(),
                                        generics: g.clone(),
                                        params: s.elaborate_params(id, &g, t.list(sig.params)),
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
        // The shells `collect_item` named are completed in place: only their
        // bounds were missing.
        let mut generics = std::mem::take(&mut self.tables.fn_info_mut(fid).generics);
        let written = self.tree(module).list(d.generics);
        if !written.is_empty() {
            for (g, p) in std::sync::Arc::make_mut(&mut generics).iter_mut().zip(written) {
                g.bounds = self.bounds_of(module, p);
            }
        }
        let params = self.elaborate_params(module, &generics, self.tree(module).list(d.params));
        let ret = self.elaborate(module, &generics, d.ret);
        // A method is declared inside an `impl` block for its type, so a
        // `self` parameter at the top level has no receiver type to attach to.
        let stray_self =
            params.first().filter(|p| p.role == ParamRole::SelfParam).map(|p| p.span);
        let info = self.tables.fn_info_mut(fid);
        info.generics = generics;
        info.params = params;
        info.ret = ret;

        if let Some(span) = stray_self {
            let n = self.name_text(module, d.name).to_string();
            self.templated("method-outside-impl", span).bind("name", n);
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
    /// s.nowMilliseconds() }` was admitted, defeating the invariant the diagnostic
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

    /// Records an exported free function of the entry module, and checks it
    /// against the shape every output that enters through it fixes.
    ///
    /// Only [`PendingCtxRule::Free`] reaches here, which is the whole rule
    /// about what may be an entry: a method is never one, whatever it is
    /// called.
    fn check_entry_point(&mut self, fid: FnId, module: ModuleId, item: u32) {
        if !self.tables.fn_info(fid).exported || self.module(module).role != Role::Entry {
            return;
        }
        let name = self.tables.fn_info(fid).name.clone();
        self.entries.insert(name.clone(), fid);
        self.module_entries.entry(module).or_default().insert(name.clone(), fid);
        if name == "main" {
            self.entry = Some(fid);
        }
        let Some(tree::Item::Fn(d)) = self.module(module).ast.items.get(item as usize) else {
            return;
        };
        let d = d.clone();
        // Asked of every output the binary declares, not of the one being
        // built: `fetch` is an entry and builds its own context while the page
        // is the artifact being compiled, and refusing it there would refuse a
        // program that is correct.
        if self.declares_entry(module, &name) {
            self.entry_points.insert((module, name.clone()));
        }
        // An entry of a repository platform is held to that platform's own
        // declaration of it.
        for custom in self.custom_entries(module, &name).unwrap_or_default() {
            self.check_custom_entry(fid, &d, &name, &custom);
        }
        for platform in self.entry_platforms(module, &name) {
            self.check_entry_signature(fid, &d, &name, platform);
        }
    }

    /// The bundled platforms whose program signature this function has to
    /// have, because an output enters through it. `None` is any bundled
    /// platform's host.
    ///
    /// Empty for an exported function no output names, which is an ordinary
    /// function that happens to live in `main.buri`. `main` is always checked
    /// even where no build file was read, because every analysis that has a
    /// `main` at all expects the one signature — a documentation snippet
    /// included.
    ///
    /// Two platforms means two outputs hand one function two different hosts.
    /// Both are checked, and at least one of them fails, which is the refusal:
    /// a function cannot take two hosts.
    fn entry_platforms(&self, module: ModuleId, name: &str) -> Vec<Option<Platform>> {
        // A build produces one artifact, and that artifact has one entry. Its
        // output is the whole answer: reporting the other output's requirement
        // here would report it twice, once per output built.
        if let (Some(platform), Some(built)) = (self.loaded.platform, self.loaded.entry.as_deref())
        {
            return match built == name {
                // A repository platform's entry is its declaration's business.
                true if self.loaded.custom.is_some() => Vec::new(),
                true => vec![Some(platform)],
                // Not this artifact's entry. Another entry of a repository
                // platform answers to its declaration when that artifact is
                // built; `main` otherwise still answers below, because every
                // analysis that has one expects the one shape.
                false if self.fills_custom_entry(module, name) => Vec::new(),
                false => self.default_entry_platform(name, None),
            };
        }
        let mut declared: Vec<Option<Platform>> = Vec::new();
        let mut outputs = false;
        // Whether a repository platform's output enters here, which holds the
        // function to that platform's declaration instead.
        let mut custom = false;
        if let (Some(ws), Some(pkg)) = (self.ws, self.module(module).pkg) {
            let target = TargetId { package: pkg, kind: RuleKind::Binary };
            for entry in ws.declared_entries(target) {
                outputs = true;
                if entry.custom.is_some() {
                    if entry.name == name {
                        custom = true;
                    }
                    continue;
                }
                // `native` is two platforms here, Linux and macOS, and one host.
                let same = |p: &Option<Platform>| p.map(Platform::slug) == Some(entry.platform.slug());
                if entry.name == name && !declared.iter().any(same) {
                    declared.push(Some(entry.platform));
                }
            }
        }
        if !declared.is_empty() || custom {
            return declared;
        }
        // A binary that names no output builds for `node`. Where the build
        // file names outputs and none of them enters here, or where there is
        // no build file at all, the platform is whichever one this analysis
        // was asked about — and any bundled host is accepted where it was
        // asked about none.
        let in_a_package = self.ws.is_some() && self.module(module).pkg.is_some();
        let platform = match (outputs, in_a_package) {
            (false, true) => Some(Platform::Js),
            (true, _) => None,
            (false, false) => self.loaded.platform,
        };
        self.default_entry_platform(name, platform)
    }

    /// Whether an `outputs` entry names this function.
    ///
    /// True for `main` whatever the build file says, because every analysis
    /// that has a `main` treats it as the entry — a documentation snippet
    /// included.
    fn declares_entry(&self, module: ModuleId, name: &str) -> bool {
        if name == "main" {
            return true;
        }
        let (Some(ws), Some(pkg)) = (self.ws, self.module(module).pkg) else { return false };
        let target = TargetId { package: pkg, kind: RuleKind::Binary };
        // An output whose `function` names nothing the binary exports is
        // `unknown-entry-function`. The function named after the platform's
        // entry stands in for it here, so its context isn't reported as well.
        ws.declared_entries(target).iter().any(|e| {
            e.name == name
                || (e.named
                    && e.custom.as_ref().is_some_and(|c| c.point == name)
                    && !self.exports_fn(module, &e.name))
        })
    }

    /// Whether an output of a repository platform enters through `name`.
    fn fills_custom_entry(&self, module: ModuleId, name: &str) -> bool {
        let (Some(ws), Some(pkg)) = (self.ws, self.module(module).pkg) else { return false };
        let target = TargetId { package: pkg, kind: RuleKind::Binary };
        ws.declared_entries(target).iter().any(|e| e.name == name && e.custom.is_some())
    }

    /// Whether `module` exports a free function called `name`.
    fn exports_fn(&self, module: ModuleId, name: &str) -> bool {
        match self.scope(module).own.get(name) {
            Some(Sym::Fn(f)) => self.tables.fn_info(*f).exported,
            _ => false,
        }
    }

    /// What `main` is held to where no output names it: the one shape every
    /// analysis expects, a documentation snippet included.
    fn default_entry_platform(&self, name: &str, platform: Option<Platform>) -> Vec<Option<Platform>> {
        match name {
            "main" => vec![platform],
            _ => Vec::new(),
        }
    }

    /// Reports every `outputs` entry naming a function `main.buri` does not
    /// export.
    ///
    /// Runs where the entry table is complete and before any body is checked,
    /// so the cause is printed above its consequences: an output that names no
    /// function leaves the function it *did* name an ordinary one, which may
    /// then not build a context.
    fn check_declared_entries(&mut self) {
        let entry_modules: Vec<ModuleId> = (0..self.loaded.modules.len() as u32)
            .map(ModuleId)
            .filter(|m| self.module(*m).role == Role::Entry && self.module(*m).pkg.is_some())
            .collect();
        for module in entry_modules {
            self.check_declared_entries_of(module);
        }
    }

    /// [`Checker::check_declared_entries`] for one binary's entry module.
    fn check_declared_entries_of(&mut self, module: ModuleId) {
        let Some(ws) = self.ws else { return };
        let Some(pkg) = self.module(module).pkg else { return };
        let target = TargetId { package: pkg, kind: RuleKind::Binary };
        let own = self.module_entries.get(&module);
        let mut exported: Vec<String> = own.map(|m| m.keys().cloned().collect()).unwrap_or_default();
        exported.sort();
        let label = ws.package(pkg).label();
        let mut said: Vec<String> = Vec::new();
        for entry in ws.declared_entries(target) {
            // An output that named nothing is `missing-main`'s business, which is a
            // different mistake and is reported by the build.
            if !entry.named || exported.contains(&entry.name) {
                continue;
            }
            // The build file may name one missing entry from several outputs;
            // the mistake is the name, so it is reported once.
            if said.contains(&entry.name) {
                continue;
            }
            said.push(entry.name.clone());
            let refs: Vec<&str> = exported.iter().map(String::as_str).collect();
            let near = crate::build::buildfile::nearest(&entry.name, &refs).map(str::to_string);
            let d = self
                .templated("unknown-entry-function", entry.span)
                .bind("entry", entry.name.clone())
                .bind("package", label.clone());
            if let Some(near) = near {
                d.notes.push(format!("did you mean `{near}`?"));
                let entry = &entry.name;
                d.fix(crate::diagnostics::candidate_fix(
                    &near,
                    &format!("export `{entry}` from its `main.buri`"),
                ));
            }
            match exported.is_empty() {
                true => d.notes.push("`main.buri` exports no function at all".into()),
                false => d.notes.push(format!("`main.buri` exports: {}", exported.join(", "))),
            }
        }
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
                        self.templated("duplicate-ctx", p.span);
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
                        let d = self.templated("effect-parameter-not-ctx", p.span);
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

    /// One entry against `fn <entry>(host: H): Result<(), Str>`, the signature
    /// its platform fixes — the program runs itself, handed the host its
    /// platform declares.
    ///
    /// `platform` is `None` where the analysis builds no particular output — a
    /// snippet, or an entry another output's build is passing by — and then
    /// any bundled platform's host is the host.
    fn check_entry_signature(
        &mut self,
        fid: FnId,
        d: &tree::FnDecl,
        name: &str,
        platform: Option<Platform>,
    ) {
        let info = self.tables.fn_info(fid).clone();
        self.refuse_entry_generics(&info, d, name);
        let expected = platform.and_then(standard_library::host_type);
        // The host type the fix spells: the platform's, or `node`'s — what a
        // binary that names no output builds for.
        let (owner, host) = expected.unwrap_or(("node", "NodeHost"));
        match info.params.as_slice() {
            [] => {
                let fix = format!(
                    "take the platform's host and bind its fields:\n     \
                     export fn {name}(host: {host}): Result<(), Str> {{\n         \
                     run(context {{ Allocator: host.alloc, Stdout: host.stdout }})\n     \
                     }}"
                );
                self.templated("entry-missing-host", d.span)
                    .bind("entry", name.to_string())
                    .fix(fix)
                    .notes
                    .push(format!(
                        "the host type is `{owner}`'s: `from \"{owner}\" import {{ {host} }};`"
                    ));
            }
            [param, rest @ ..] => {
                let taken = self.bundled_host(&param.ty);
                match (taken, expected) {
                    // Unresolved already, and reported where it was written.
                    _ if param.ty.is_error() => {}
                    (Some(taken), Some((_, wanted))) if taken.1 != wanted => {
                        let platform = platform.map_or("this", Platform::slug);
                        self.templated("entry-host-mismatch", param.span)
                            .bind("entry", name.to_string())
                            .bind("taken", taken.1)
                            .bind("platform", platform)
                            .bind("wanted", wanted)
                            .fix(format!(
                                "take `{wanted}`, imported with `from \"{owner}\" import \
                                 {{ {wanted} }};` — or build this function for {}, and give \
                                 this output an entry of its own",
                                taken.0
                            ));
                    }
                    (Some(_), _) => {}
                    (None, _) => {
                        self.templated("entry-signature-mismatch", param.span)
                            .bind("entry", name.to_string())
                            .bind("requirement", format!("take its platform's host, `{host}`"))
                            .fix(format!(
                                "write it `fn {name}(host: {host}): Result<(), Str>`, and take \
                                 anything else from the host's fields"
                            ));
                    }
                }
                if let Some(extra) = rest.first() {
                    self.templated("entry-signature-mismatch", extra.span)
                        .bind("entry", name.to_string())
                        .bind("requirement", "take one parameter, its platform's host")
                        .fix(format!(
                            "drop the rest: the platform hands `{name}` its host and nothing \
                             else, and a function `{name}` calls can take whatever it needs"
                        ));
                }
            }
        }
        if !self.is_program_answer(&info.ret) && !info.ret.is_error() {
            let at = self.tree(info.module).type_span(d.ret);
            self.templated("entry-signature-mismatch", at)
                .bind("entry", name.to_string())
                .bind("requirement", "return `Result<(), Str>`")
                .fix("change the return type to `Result<(), Str>`")
                .notes
                .push("`.Ok(())` exits 0; `.Err(msg)` prints `msg` to stderr and exits 1".into());
        }
    }

    /// The bundled platform and host type `ty` is, when it is one of the three
    /// hosts the bundled platforms declare.
    fn bundled_host(&self, ty: &Ty) -> Option<(&'static str, &'static str)> {
        let TyKind::Con(con, args) = ty.kind() else { return None };
        if !args.is_empty() {
            return None;
        }
        let tycon = self.tables.tycon(*con);
        // A built-in type has no module, and is no host.
        let module = self.loaded.modules.get(tycon.module.index())?.path.as_str();
        standard_library::host_type_of(module).filter(|(_, host)| *host == tycon.name)
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
        if crate::compiler::standard_library::find(&path).is_none() && !self.is_platform_surface(module) {
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
        params
            .iter()
            .map(|p| GenericInfo {
                name: t.name(p.name).to_string(),
                bounds: self.bounds_of(module, p),
                span: p.span,
            })
            .collect()
    }

    /// The traits a generic parameter is bounded by. A bound names a trait,
    /// never another parameter, so this needs no parameter in scope.
    fn bounds_of(&mut self, module: ModuleId, p: &tree::GenericParam) -> Vec<TraitId> {
        let t = self.tree(module);
        let mut bounds = Vec::new();
        for b in t.type_list(p.bounds) {
            match self.resolve_trait(module, *b) {
                Some(id) => bounds.push(id),
                None => {
                    // `T: ns.Name` with no `Name` in `ns` is a missing
                    // member, not a name that is "not a trait": the module
                    // has no such thing to be one.
                    if self.namespace_member_missing_in(module, *b) {
                        continue;
                    }
                    let shown = t.type_head(*b).unwrap_or("?").to_string();
                    let at = t.type_span(*b);
                    let d = self.templated("bound-not-trait", at).bind("name", shown.clone());
                    d.fix(format!(
                        "name a declared trait or effect, or declare `{shown}` as one"
                    ));
                    d.notes
                        .push("a bound names a declared trait; there are no where clauses".into());
                }
            }
        }
        bounds
    }

    fn resolve_trait(&mut self, module: ModuleId, id: TypeId) -> Option<TraitId> {
        let flat::TypeView::Named { path, .. } = self.tree(module).ty(id) else { return None };
        match self.resolve_path(module, path)? {
            Sym::Trait(t) => Some(t),
            _ => None,
        }
    }

    /// Resolves a possibly-qualified path (`Order`, `effects.Allocator`) in a module's
    /// scope.
    pub fn resolve_path(&mut self, module: ModuleId, path: &[flat::Location]) -> Option<Sym> {
        let t = self.tree(module);
        match path {
            [name] => self.scope(module).name(t.text(*name)).cloned(),
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
    /// `method-outside-impl` reports and there is no type to give it.
    fn elaborate_params(
        &mut self,
        module: ModuleId,
        generics: &[GenericInfo],
        params: &[tree::Param],
    ) -> Vec<ParamInfo> {
        let t = self.tree(module);
        let receiver = self.self_scope;
        params
            .iter()
            .map(|p| ParamInfo {
                name: t.name(p.name).to_string(),
                ty: match p.written_type() {
                    Some(ty) => self.elaborate(module, generics, ty),
                    None => receiver.unwrap_or(Ty::ERROR),
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
            flat::TypeView::Unit { .. } => Ty::UNIT,
            flat::TypeView::SelfType { span } => {
                // `Self` stands for the implementing type and is legal only
                // inside a trait or an `impl` body. Inside an `impl` that type
                // is known, and `Self` *is* it from here on: nothing between
                // this point and `middle::layout` substitutes a `Ty::SelfTy`
                // that reached an `impl` method's signature.
                let Some(ty) = self.self_scope else {
                    self.templated("self-type-outside-impl", span);
                    return Ty::ERROR;
                };
                ty
            }
            flat::TypeView::Array { elem, .. } => {
                Ty::array(self.elaborate(module, generics, elem))
            }
            flat::TypeView::Tuple { elems, .. } => Ty::tuple(elems.iter().map(|e| self.elaborate(module, generics, *e))),
            flat::TypeView::Fn { params, ret, .. } => {
                let ps: Vec<Ty> = params.iter().map(|p| self.elaborate(module, generics, *p)).collect();
                Ty::func(ps, self.elaborate(module, generics, ret))
            }
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
                            self.templated("type-argument-count", span)
                                .bind("subject", format!("the type parameter `{name}`"))
                                .bind("expected", counted(0, "type argument"))
                                .bind("given", were_given(args.len()))
                                .fix("drop the arguments; a type parameter stands for one type already");
                        }
                        return Ty::param(i as u32);
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
                        .unwrap_or(Ty::ERROR);
                }
                if path.len() == 1 {
                    if let Some(id) = self.builtin_type(name) {
                        if !elaborated_args.is_empty() {
                            self.templated("type-argument-count", span)
                                .bind("subject", format!("`{name}`"))
                                .bind("expected", counted(0, "type argument"))
                                .bind("given", were_given(elaborated_args.len()))
                                .fix("drop them");
                        }
                        return Ty::con(id, []);
                    }
                }
                match self.resolve_path(module, path) {
                    Some(Sym::Ty(id)) => {
                        let arity = self.tables.tycon(id).arity();
                        if elaborated_args.len() != arity {
                            let n = self.tables.tycon(id).name.clone();
                            let got = elaborated_args.len();
                            self.templated("type-argument-count", span)
                                .bind("subject", format!("`{n}`"))
                                .bind("expected", counted(arity, "type argument"))
                                .bind("given", were_given(got))
                                .mismatch(arity.to_string(), got.to_string());
                            return Ty::ERROR;
                        }
                        Ty::con(id, elaborated_args)
                    }
                    Some(Sym::Trait(_)) => {
                        let shown = name.to_string();
                        self.templated("trait-not-type", span).bind("name", shown);
                        Ty::ERROR
                    }
                    _ => {
                        // `ns.Name`, where the import is right and the member
                        // is not: the answer is about the member, and
                        // `nearest_type_name` below draws from *this* module's
                        // types, which are the wrong set to offer.
                        if self.namespace_member_missing(module, path, span) {
                            return Ty::ERROR;
                        }
                        let shown = t.path_text(path);
                        let near = self.nearest_type_name(module, name);
                        let d = self.templated("unknown-type", span).bind("name", shown);
                        if let Some(n) = near {
                            let scope = crate::diagnostics::NAMES_IN_SCOPE;
                            d.fix(crate::diagnostics::candidate_fix(&n, scope));
                            d.notes.push(format!("did you mean `{n}`?"));
                        }
                        Ty::ERROR
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
            return Some(Ty::ERROR);
        }
        // Every alias of a cycle already reported is answered with the error
        // type in silence. One cycle is one mistake, however many signatures
        // and fields name it.
        if self.cyclic_aliases.contains(&key) {
            return Some(Ty::ERROR);
        }
        let generics: Vec<GenericInfo> = t
            .list(alias.generics)
            .iter()
            .map(|g| GenericInfo { name: t.name(g.name).to_string(), bounds: Vec::new(), span: g.span })
            .collect();
        if args.len() != generics.len() {
            self.templated("type-argument-count", span)
                .bind("subject", format!("`{name}`"))
                .bind("expected", counted(generics.len(), "type argument"))
                .bind("given", were_given(args.len()));
            return Some(Ty::ERROR);
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

    /// Whether a written type path is `ns.Name` with `ns` a namespace import
    /// whose module exports no `Name`, reporting it as the missing member it
    /// is. `false` for every other path, including one whose head names no
    /// namespace at all — a module that was never imported is a different
    /// mistake, and keeps the answer about the path as written.
    fn namespace_member_missing(
        &mut self,
        module: ModuleId,
        path: &[flat::Location],
        span: Span,
    ) -> bool {
        let t = self.tree(module);
        let [ns, member] = path else { return false };
        let Some(from) = self.scope(module).namespaces.get(t.text(*ns)).copied() else {
            return false;
        };
        let member = t.text(*member);
        if self.lookup_export(from, member).is_some() {
            return false;
        }
        self.report_no_such_member(from, member, span);
        true
    }

    /// The same question asked of a written type rather than of a path, for
    /// the positions that resolve one whole: a bound, and an `impl` head.
    fn namespace_member_missing_in(&mut self, module: ModuleId, id: TypeId) -> bool {
        let flat::TypeView::Named { path, span, .. } = self.tree(module).ty(id) else {
            return false;
        };
        self.namespace_member_missing(module, path, span)
    }

    fn nearest_type_name(&self, module: ModuleId, name: &str) -> Option<String> {
        let mut candidates: Vec<String> = self
            .scope(module)
            .visible()
            .filter(|(_, s)| matches!(s, Sym::Ty(_)))
            .map(|(k, _)| k.to_string())
            .collect();
        candidates.extend(Prim::all().iter().map(|p| p.name().to_string()));
        candidates.extend(["Int", "Float", "Uint", "Byte"].map(String::from));
        let refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
        crate::build::buildfile::nearest(name, &refs).map(|s| s.to_string())
    }

    /// Struct field names and enum variant names must be unique within their
    /// scope (design/static-rules.md rule 6).
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

    /// Enum variant names must be unique within their scope
    /// (design/static-rules.md rule 6).
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
    /// or no value of it could ever be built (design/static-rules.md rule 14).
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
        match ty.kind() {
            TyKind::Con(id, args) => {
                *id == con || args.iter().any(|a| self.mentions_directly(a, con))
            }
            TyKind::Tuple(es) => es.iter().any(|e| self.mentions_directly(e, con)),
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
    /// What every prelude name refers to, from the scopes of the modules
    /// that export them.
    fn prelude(&self) -> std::sync::Arc<Prelude> {
        let mut out = Prelude::default();
        for (path, name) in standard_library::prelude() {
            let Some(from) = self.loaded.find(path) else { continue };
            let Some(sym) = self.scope(from).exports.get(name) else { continue };
            // The first of two prelude entries of one name is the one seen.
            out.entry(name.to_string()).or_insert_with(|| sym.clone());
        }
        std::sync::Arc::new(out)
    }

    fn register_known_names(&mut self) {
        for (path, name) in standard_library::prelude() {
            if let Some(m) = self.loaded.find(path) {
                self.know(m, name);
            }
        }
        // `core/json` is loaded on import rather than eagerly, so these are
        // known exactly when a program could name them — which is the only
        // time a primitive needs an implementation of either to be found.
        if let Some(m) = self.loaded.find("core/json") {
            for name in ["ToJson", "FromJson", "DecodeError", "Json"] {
                self.know(m, name);
            }
        }
        if let Some(m) = self.loaded.find("platform/effect") {
            // `Request` and `Response` are here for the entry check: a
            // platform that calls its entry fixes those two types, and the
            // check is a comparison against the ids rather than against a
            // spelling a program could shadow.
            // `Scope` is the allocator that charges nothing, which a test
            // report renders a hand-written `Show` with.
            for name in ["Allocator", "IoError", "Region", "Request", "Response", "Scope"] {
                self.know(m, name);
            }
        }
        self.option_con = self.known_types.get("Option").copied();
        self.result_con = self.known_types.get("Result").copied();
        self.order_con = self.known_types.get("Order").copied();
    }

    /// Records `module`'s export `name` as a well-known trait or type, when it
    /// is one. A name a base already recorded is left as it is.
    fn know(&mut self, module: ModuleId, name: &str) {
        match self.scope(module).exports.get(name) {
            Some(&Sym::Trait(t)) if self.known_traits.get(name) != Some(&t) => {
                self.known_traits.insert(name.to_string(), t);
            }
            Some(&Sym::Ty(c)) if self.known_types.get(name) != Some(&c) => {
                self.known_types.insert(name.to_string(), c);
            }
            _ => {}
        }
    }

    /// Registers the methods of every `impl` block, and every `derive`.
    ///
    /// A top-level `fn` taking `self` used to be registered here as well, off
    /// the type its annotation named. There is no such annotation now, so a
    /// method declared free has no receiver type at all — only the
    /// `method-outside-impl` diagnostic `elaborate_fn_signature` already
    /// reports.
    fn register_conformance(&mut self) {
        for id in self.own_modules() {
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
        self.enter_self_scope(Ty::SELF, |s| s.register_impl_body(module, index, d));
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
        let generics = self.elaborate_generics(module, self.tree(module).list(d.generics));
        // No `for` clause: this declares the type's own methods rather than
        // conformance to anything.
        let Some(trait_ref) = d.trait_ty else {
            self.register_inherent_impl(module, index, d, &generics);
            return;
        };
        let Some(trait_id) = self.resolve_trait(module, trait_ref) else {
            if self.namespace_member_missing_in(module, trait_ref) {
                return;
            }
            let t = self.tree(module);
            let shown = t.type_head(trait_ref).unwrap_or("?").to_string();
            let at = t.type_span(trait_ref);
            self.templated("bound-not-trait", at)
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
        self.self_scope = Some(self_ty);
        let Some(self_con) = self_ty.head() else {
            if !self_ty.is_error() {
                let at = self.tree(module).type_span(d.self_ty);
                self.templated("impl-target-not-type", at);
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
            self.templated("impl-outside-type-module", at)
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
        let own_flags = tname == "Flags" && !self.declared_in(trait_id, FLAGS_MODULE);
        if crate::compiler::semantics::types::is_derive_only(&tname) && !own_flags {
            let c = self.tables.tycon(self_con).name.clone();
            let reason = if tname == "Flags" {
                "a `Flags` type is stored as a word with one bit per field, and its operations \
                 are that word's, so only the compiler can write them"
            } else {
                "a derived encoder is a fold over the type's shape, and would encode a \
                 hand-written one structurally rather than calling it — so an `impl` would be \
                 obeyed at the top of a document and ignored inside it"
            };
            self.templated("derive-only-trait", d.span)
                .bind("trait", tname)
                .bind("type", c)
                .bind("reason", reason);
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
            self.templated("duplicate-impl", d.span)
                .bind("type", c)
                .bind("trait", t)
                .fix("delete one of the two, or merge them")
                .note("there is exactly one candidate per (trait, type)");
            return;
        }

        // Register the methods, checked against the trait's signatures.
        let trait_methods = self.tables.trait_(trait_id).methods.clone();
        let mut supplied = vec![None; trait_methods.len()];
        for (sub, method) in self.tree(module).list(d.methods).iter().enumerate() {
            let mname = self.name_text(module, method.name);
            let Some(slot) = trait_methods.iter().position(|m| m.name == mname) else {
                let t = self.tables.trait_(trait_id).name.clone();
                let n = mname.to_string();
                self.templated("impl-unknown-method", method.name.span)
                    .bind("trait", t.clone())
                    .bind("method", n)
                    .fix(format!("remove it, or move it into an inherent `impl` block for the type — `{t}` supplies only what it declares"));
                continue;
            };
            let mut g = generics.clone();
            g.extend(self.elaborate_generics(module, self.tree(module).list(method.generics)));
            let params = self.elaborate_params(module, &g, self.tree(module).list(method.params));
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
                generics: shared(g),
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
            // A method supplied twice is reported by `register_method` below,
            // as `duplicate-method`, the way one declared twice anywhere is.
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
            self.templated("impl-missing-method", d.span)
                .bind("type", c)
                .bind("trait", t)
                .bind("methods", missing);
        }

        self.tables.add_impl(ImplInfo {
            trait_id,
            self_con,
            head: self_ty,
            generics,
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
            self.templated("impl-signature-mismatch", at)
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
        self.self_scope = Some(self_ty);
        let target = match self_ty.kind() {
            TyKind::Con(con, _) => Some(*con),
            TyKind::Array(_) => None,
            TyKind::Error => return,
            _ => {
                let shown = show(&self.tables, None, generics, &self_ty);
                let at = self.tree(module).type_span(d.self_ty);
                self.templated("impl-target-not-declared-type", at).bind("type", shown);
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
                self.templated("impl-outside-type-module", at)
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

        for (sub, method) in self.tree(module).list(d.methods).iter().enumerate() {
            let mname = self.name_text(module, method.name);
            let mut g = generics.to_vec();
            g.extend(self.elaborate_generics(module, self.tree(module).list(method.generics)));
            let params = self.elaborate_params(module, &g, self.tree(module).list(method.params));
            let ret = self.elaborate(module, &g, method.ret);

            // A method is a function whose first parameter is `self`, and an
            // `impl` block is where one is declared — so anything else in here
            // is a mistake worth naming.
            match params.first() {
                Some(p) if p.role == ParamRole::SelfParam => {}
                _ => {
                    let n = mname.to_string();
                    self.templated("impl-missing-self", method.name.span).bind("name", n);
                    continue;
                }
            }

            let fid = self.tables.add_fn(FnInfo {
                name: mname.to_string(),
                module,
                generics: shared(g),
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
                    if self.module(module).path == "core/list" {
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
        // `derive Equal for Option;` says every `Option<T>` compares whenever `T`
        // does. So the path is resolved directly rather than elaborated, which
        // would demand type arguments there is nothing to bind.
        let Some(self_con) = self.derive_target(module, d.self_ty) else {
            return;
        };
        if self.tables.tycon(self_con).module != module {
            let name = self.tables.tycon(self_con).name.clone();
            let at = self.tree(module).type_span(d.self_ty);
            self.templated("impl-outside-type-module", at)
                .bind("name", name.clone())
                .fix(format!("move the `derive` into `{name}`'s own module"));
            return;
        }
        for ty in self.tree(module).type_list(d.traits) {
            let at = self.tree(module).type_span(*ty);
            let Some(trait_id) = self.resolve_trait(module, *ty) else {
                let shown = self.tree(module).type_head(*ty).unwrap_or("?").to_string();
                self.templated("derive-not-trait", at).bind("name", shown);
                continue;
            };
            let name = self.tables.trait_(trait_id).name.clone();
            let foreign_flags = name == "Flags" && !self.declared_in(trait_id, FLAGS_MODULE);
            if !DERIVABLE.contains(&name.as_str()) || foreign_flags {
                let note = if foreign_flags {
                    format!("only the `Flags` that `{FLAGS_MODULE}` declares is derivable")
                } else {
                    format!(
                        "derivable: {}",
                        crate::diagnostics::names(
                            &DERIVABLE.iter().map(|s| s.to_string()).collect::<Vec<_>>()
                        )
                    )
                };
                self.templated("trait-not-derivable", at).bind("trait", name).note(note);
                continue;
            }
            if self.tables.impls.contains_key(&(trait_id, self_con)) {
                let c = self.tables.tycon(self_con).name.clone();
                self.templated("duplicate-impl", at)
                    .bind("type", c)
                    .bind("trait", name)
                    .fix("drop it from this `derive`, or delete the hand-written `impl`");
                continue;
            }
            self.tables.add_impl(ImplInfo {
                trait_id,
                self_con,
                head: self.tables.generic_head(self_con),
                generics: Vec::new(),
                body: ImplBody::Derived,
                span: d.span,
            });
        }
    }

    /// The type constructor a `derive` names.
    fn derive_target(&mut self, module: ModuleId, id: TypeId) -> Option<TyConId> {
        let flat::TypeView::Named { path, args, span } = self.tree(module).ty(id) else {
            let at = self.tree(module).type_span(id);
            self.templated("derive-target-not-type", at);
            return None;
        };
        match self.resolve_path(module, path) {
            Some(Sym::Ty(con)) => {
                // Naming the arguments is allowed, and has to be consistent.
                if !args.is_empty() && args.len() != self.tables.tycon(con).arity() {
                    let n = self.tables.tycon(con).name.clone();
                    let arity = self.tables.tycon(con).arity();
                    self.templated("type-argument-count", span)
                        .bind("subject", format!("`{n}`"))
                        .bind("expected", counted(arity, "type argument"))
                        .bind("given", were_given(args.len()))
                        .fix(format!("a `derive` names the constructor alone: `derive ... for {n};`"));
                }
                Some(con)
            }
            _ => {
                let shown = self.tree(module).path_text(path);
                self.templated("unknown-type", span).bind("name", shown);
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
            // The base checked its own, and they are the same ones.
            .filter(|(key, _)| !self.base.is_some_and(|b| b.tables.impls.contains_key(*key)))
            .map(|((t, c), i)| (*t, *c, i.span))
            .collect();
        let mut sorted = derived;
        sorted.sort_by_key(|(t, c, _)| (t.0, c.0));

        for (tr, con, span) in sorted {
            if self.tables.trait_(tr).name == "Flags" {
                self.check_flags(con, span);
                continue;
            }
            // A generic type's components are checked at each use site, where
            // the arguments are known; here only the ones that cannot depend
            // on an argument are decidable.
            //
            // The components are read out of the declaration rather than out
            // of a copy of it: a type deriving four traits is walked four
            // times, and each walk copied every variant and every field. Only
            // the first that cannot satisfy the trait is named, and its name
            // is the one string this spells.
            let fails = |ty: &Ty| !self.component_can_satisfy(ty, tr, con);
            let failing = match &self.tables.tycon(con).def {
                TyDef::Struct { fields, .. } => {
                    fields.iter().find(|f| fails(&f.ty)).map(|f| (f.name.clone(), f.ty))
                }
                TyDef::Enum { variants } => variants.iter().find_map(|v| {
                    let f = v.fields.iter().find(|f| fails(&f.ty))?;
                    Some((format!("{}.{}", v.name, f.name), f.ty))
                }),
                TyDef::Prim(_) => None,
            };
            if let Some((name, ty)) = failing {
                let t = self.tables.trait_(tr).name.clone();
                let c = self.tables.tycon(con).name.clone();
                let generics = &self.tables.tycon(con).generics;
                let shown = show(&self.tables, None, generics, &ty);
                // The fix names the type that lacks the trait, which is `Item`
                // in `OrderedMap<Int, Item>` rather than the map (#269).
                let lacking_ty = self.lacking(&ty, tr, con);
                let lacking = show(&self.tables, None, generics, &lacking_ty);
                let home = self.toolchain_home(&lacking_ty, tr);
                self.templated("underivable-field", span)
                    .bind("type", c)
                    .bind("trait", t.clone())
                    .bind("field", name)
                    .bind("field_type", shown.clone())
                .fix(if let Some(home) = home {
                    format!(
                        "drop `{t}` from this `derive`: `{lacking}` comes from `{home}`, which \
                         leaves it out, and `buri docs {home}` says what to use instead"
                    )
                } else if crate::compiler::semantics::types::is_derive_only(&t) {
                    format!(
                        "make `{lacking}` satisfy `{t}` first — `derive {t} for {lacking};` in \
                         its own module — or drop `{t}` from this `derive`"
                    )
                } else {
                    format!(
                        "make `{lacking}` satisfy `{t}` first — `derive {t} for {lacking};` in \
                         its own module, or an `impl` — or drop `{t}` from this `derive`"
                    )
                });
            }
        }
    }

    /// A `Flags` type is a word with one bit per field, so it is a struct
    /// whose fields are all `Bool`, at least one and at most
    /// [`types::FLAGS_MAX`].
    fn check_flags(&mut self, con: TyConId, span: Span) {
        let tycon = self.tables.tycon(con);
        let name = tycon.name.clone();
        let TyDef::Struct { fields, .. } = &tycon.def else {
            self.templated("flags-not-struct", span).bind("type", name);
            return;
        };
        let bool_ty = self.tables.prim(Prim::Bool);
        if let Some(f) = fields.iter().find(|f| f.ty != bool_ty) {
            let field = f.name.clone();
            let shown = show(&self.tables, None, &tycon.generics, &f.ty);
            self.templated("flags-field-not-bool", span)
                .bind("type", name)
                .bind("field", field)
                .bind("field_type", shown);
        } else if fields.is_empty() {
            self.templated("flags-empty", span).bind("type", name);
        } else if fields.len() > FLAGS_MAX {
            let count = fields.len().to_string();
            self.templated("flags-too-wide", span)
                .bind("type", name)
                .bind("count", count)
                .bind("max", FLAGS_MAX.to_string());
        }
    }

    /// Whether a trait is the one declared in the standard-library module at
    /// `path`.
    fn declared_in(&self, trait_id: TraitId, path: &str) -> bool {
        let module = self.tables.trait_(trait_id).module;
        self.loaded.modules.get(module.index()).is_some_and(|m| m.path == path)
    }

    /// The innermost part of a component that cannot satisfy the trait: an
    /// argument of a type that has the trait, or the component itself.
    fn lacking(&self, ty: &Ty, tr: TraitId, owner: TyConId) -> Ty {
        let inner = match ty.kind() {
            TyKind::Array(e) => Some(*e),
            TyKind::Tuple(es) => es.iter().find(|e| !self.component_can_satisfy(e, tr, owner)).copied(),
            TyKind::Con(id, args) if self.tables.impls.contains_key(&(tr, *id)) => {
                args.iter().find(|a| !self.component_can_satisfy(a, tr, owner)).copied()
            }
            _ => None,
        };
        inner.map_or(*ty, |t| self.lacking(&t, tr, owner))
    }

    /// Whether a component could satisfy the trait for some instantiation. A
    /// type parameter is decided at the use site; a function type never can.
    fn component_can_satisfy(&self, ty: &Ty, tr: TraitId, owner: TyConId) -> bool {
        match ty.kind() {
            // Undecidable here, and checked where the arguments are known.
            TyKind::Param(_) | TyKind::Var(_) | TyKind::SelfTy | TyKind::Error => true,
            TyKind::Fn(..) => false,
            TyKind::Ctx(_) => false,
            TyKind::Unit => true,
            TyKind::Array(e) => self.component_can_satisfy(e, tr, owner),
            TyKind::Tuple(es) => es.iter().all(|e| self.component_can_satisfy(e, tr, owner)),
            TyKind::Con(id, args) => {
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
    /// from outside the library resolves only to names on it. Its
    /// `testing/lib.buri` is the same for `//pkg/testing`, and kept apart,
    /// because each one answers for the methods declared behind it.
    fn compute_surfaces(&mut self) {
        let Some(ws) = self.ws else { return };
        for (module, scope) in self.loaded.modules.iter().zip(&self.scopes) {
            let Some(pkg) = module.pkg else { continue };
            let package = ws.package(pkg);
            let surfaces = if package.module_path("lib.buri") == module.path {
                &mut self.surfaces
            } else if package.module_path("testing/lib.buri") == module.path {
                &mut self.testing_surfaces
            } else {
                continue;
            };
            let names: HashSet<String> = scope.exports.keys().cloned().collect();
            surfaces.insert(pkg, names);
        }
    }

    // -----------------------------------------------------------------------
    // Module-level rules
    // -----------------------------------------------------------------------

    fn check_module_rules(&mut self) {
        for id in self.own_modules() {
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
                        self.templated("duplicate-test", t.span)
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

    /// The toolchain module that declares `ty`'s constructor, when one does and
    /// it has no `impl` of `tr` at all. Nobody else can add one, so a fix that
    /// says "derive it in its own module" points at that module's page instead.
    pub fn toolchain_home(&self, ty: &Ty, tr: TraitId) -> Option<String> {
        let TyKind::Con(con, _) = ty.kind() else { return None };
        if self.tables.impls.contains_key(&(tr, *con)) {
            return None;
        }
        let info = self.tables.tycon(*con);
        if let TyDef::Prim(p) = &info.def {
            return Some(standard_library::defining_module(*p).to_string());
        }
        let module = self.module(info.module);
        matches!(module.role, Role::Std | Role::Platform).then(|| module.path.clone())
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
    /// Compared as a set, so `C: Allocator + Fs` and `C: Fs + Allocator` are the same
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
///   for in the `impl` — `Ty::SelfTy` on one side, `[T]` or `HostFileSystem` on the
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
        (0..declared_own).map(|i| Ty::param(impl_generics.saturating_add(i) as u32)).collect();
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
                found: s.ty,
            });
        }
    }
    let expected = substitute(&declared.ret, &args, Some(self_ty));
    if !agrees(&expected, supplied.ret) {
        out.push(SignatureMismatch::Return { expected, found: *supplied.ret });
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
/// bound list, and a repeat is `duplicate-context-binding`'s business rather than this
/// rule's.
fn as_a_set(bounds: &[TraitId]) -> BTreeSet<TraitId> {
    bounds.iter().copied().collect()
}

/// A type in backticks, the way every other diagnostic quotes one.
fn quoted_ty(tables: &Tables, generics: &[GenericInfo], ty: &Ty) -> String {
    format!("`{}`", show(tables, None, generics, ty))
}

/// A type parameter with its bounds, as a message names it: `` `C: Allocator + Fs` ``,
/// or `` `C` with no bounds `` where there are none to name.
fn bound_phrase(tables: &Tables, name: &str, bounds: &[TraitId]) -> String {
    if bounds.is_empty() {
        return format!("`{name}` with no bounds");
    }
    let named: Vec<&str> = bounds.iter().map(|t| tables.trait_(*t).name.as_str()).collect();
    format!("`{name}: {}`", named.join(" + "))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ty: Ty::SELF,
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
        Ty::con(BAG, [])
    }

    fn crate_ty() -> Ty {
        Ty::con(CRATE, [])
    }

    /// The everyday shape: no generics anywhere, `self` on both sides. The
    /// trait's `Self` is abstract and the `impl`'s is the head it was written
    /// for, and the substitution is what makes them the same type.
    #[test]
    fn a_signature_that_agrees_reports_nothing() {
        let declared = trait_method(vec![], vec![receiver(), param(Ty::UNIT)], bag());
        let params = [param(bag()), param(Ty::UNIT)];
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
        let head = Ty::array(Ty::param(0));
        let declared =
            trait_method(vec![generic("C", &[])], vec![receiver(), param(Ty::param(0))], Ty::UNIT);
        let generics = [generic("T", &[]), generic("C", &[])];
        let params = [param(head), param(Ty::param(1))];
        let ret = Ty::UNIT;
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
            trait_method(vec![generic("T", &[])], vec![receiver(), param(Ty::param(0))], Ty::UNIT);
        let params = [param(bag()), param(crate_ty())];
        let ret = Ty::UNIT;
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
        let declared = trait_method(vec![], vec![receiver()], Ty::UNIT);
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
        let declared = trait_method(vec![], vec![receiver(), param(Ty::UNIT)], bag());
        let params = [param(bag()), param(crate_ty())];
        let ret = crate_ty();
        let found = supplied(&[], &params, &ret);
        assert_eq!(
            signature_mismatches(&declared, 0, &found, 0, &bag()),
            vec![
                SignatureMismatch::Parameter { index: 1, expected: Ty::UNIT, found: crate_ty() },
                SignatureMismatch::Return { expected: bag(), found: crate_ty() },
            ],
        );
    }

    /// A receiver the `impl` wrote as the wrong type is caught by the same
    /// arm, because `self` is a parameter like any other once `Self` has been
    /// substituted.
    #[test]
    fn a_receiver_is_compared_after_self_is_substituted() {
        let declared = trait_method(vec![], vec![receiver()], Ty::UNIT);
        let params = [param(crate_ty())];
        let ret = Ty::UNIT;
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
        let declared = trait_method(vec![], vec![receiver(), param(Ty::ERROR)], Ty::UNIT);
        let params = [param(bag()), param(crate_ty())];
        let ret = Ty::ERROR;
        let found = supplied(&[], &params, &ret);
        assert_eq!(signature_mismatches(&declared, 0, &found, 0, &bag()), vec![]);
    }

    /// Bounds are a set: the same ones in another order are the same
    /// declaration.
    #[test]
    fn bounds_in_another_order_agree() {
        let declared = trait_method(
            vec![generic("C", &[SHOWN, ALLOCS])],
            vec![receiver(), param(Ty::param(0))],
            Ty::UNIT,
        );
        let generics = [generic("C", &[ALLOCS, SHOWN])];
        let params = [param(bag()), param(Ty::param(0))];
        let ret = Ty::UNIT;
        let found = supplied(&generics, &params, &ret);
        assert_eq!(signature_mismatches(&declared, 0, &found, 0, &bag()), vec![]);
    }

    /// A bound the trait does not declare is a requirement the `impl`'s
    /// callers were never told to meet, and it is carried in the order each
    /// side wrote it so the message can echo the source.
    #[test]
    fn a_bound_the_trait_does_not_declare_is_refused() {
        let declared = trait_method(vec![generic("C", &[SHOWN])], vec![receiver()], Ty::UNIT);
        let generics = [generic("C", &[SHOWN, ALLOCS])];
        let params = [param(bag())];
        let ret = Ty::UNIT;
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
            vec![receiver(), param(Ty::param(0))],
            Ty::param(1),
        );
        let params = [param(crate_ty())];
        let ret = Ty::UNIT;
        let found = supplied(&[], &params, &ret);
        assert_eq!(signature_mismatches(&declared, 1, &found, 0, &bag()), vec![]);
    }
}
