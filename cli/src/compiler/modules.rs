//! Loading a compilation: which modules a target is made of, and where each
//! one comes from.
//!
//! A source file is a module, named by its path from the repository root.
//! Loading follows imports, and the restrictions on which module may import
//! which — the library boundary, `core/host`, and the `testing` segment — are
//! checked here, where the import line is.

use crate::build::buildfile::Platform;
use crate::build::workspace::{
    is_test_only_path, ModuleKind, ModuleLocation, RuleKind, TargetId, Workspace,
};
use crate::compiler::semantics::types::ModuleId;
use crate::compiler::standard_library;
use crate::diagnostics::{Diagnostic, Diagnostics, FileId, Invariant as _, SourceMap, Span};
use crate::parsing::tree;
use crate::hash::Map as HashMap;
use std::path::PathBuf;

/// What a module is being compiled as. This is what decides whether `test`
/// declarations, expression statements, and test-only imports are legal in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// A `core/...` module, shipping with the toolchain.
    Std,
    /// A platform module: `core/effect`, `core/host`, `core/testing/*`. Only
    /// these may declare effects.
    Platform,
    /// Ordinary library or binary source.
    Source,
    /// The module exporting `main`. The only one that may import `core/host`,
    /// and the only place in a program where a context may be built.
    Entry,
    /// A module listed in a rule's `test.sources`. `test` declarations and
    /// imports of test-only modules are legal here and nowhere else.
    TestSource,
    /// A module under `testing/`, reachable only from a test source.
    TestOnly,
}

impl Role {
    fn is_test_context(self) -> bool {
        matches!(self, Role::TestSource | Role::TestOnly)
    }

    /// Where a context may be built (SPEC 11.3).
    pub fn may_build_context(self) -> bool {
        matches!(self, Role::Entry | Role::TestSource | Role::TestOnly | Role::Platform)
    }
}

pub struct ModuleData {
    pub id: ModuleId,
    /// The module path as written in an import: `//lib/money/cents`,
    /// `core/list`.
    pub path: String,
    pub file: FileId,
    pub role: Role,
    /// Shared, not owned: one file is parsed once per process and every
    /// target that imports it reads the same tree.
    pub ast: std::rc::Rc<tree::Module>,
    /// The package this module belongs to, and the target that compiles it.
    pub pkg: Option<crate::build::workspace::PackageId>,
    /// The file this module was read from, for a module that came from disk.
    /// `None` for the embedded standard library and for generated modules,
    /// which have no file — that used to be an empty `PathBuf`, whose
    /// `file_name()` is `None`, so every reader took the "not a surface file"
    /// branch by accident rather than by decision.
    pub disk: Option<PathBuf>,
}

/// One thing to build: a target, a platform, and whether tests are included.
#[derive(Clone, Debug)]
pub struct Unit {
    pub target: Option<TargetId>,
    /// The output this unit is being built for, when there is one.
    ///
    /// `Some(p)` subsets `core/host` to the effects `p` grants, which is what
    /// makes a platform *be* the set of effects its host exports rather than a
    /// claim a comment makes: binding `Ui: host.ui` under `platform: LINUX` is
    /// then an unresolved name at the line that asked for it, and so is
    /// `Net: host.net` under `platform: WEB`.
    ///
    /// `None` is an analysis that is not building an artifact — `buri lint`,
    /// the language server, the documentation harness, `buri test` — and it
    /// grants the whole host. Those commands ask the same questions of the
    /// same modules for every output a target declares at once, so refusing a
    /// program on behalf of one of them would report a build error in a place
    /// that is not building. **The check belongs to the build, per output**,
    /// which is where `design/ui-reactivity.md` §Targets puts it.
    pub platform: Option<Platform>,
    /// Compile the target's `test.sources` too, and run them.
    pub with_tests: bool,
}

pub struct Loaded {
    pub modules: Vec<ModuleData>,
    pub by_path: HashMap<String, ModuleId>,
    /// Modules that are test sources, in declaration order.
    pub test_sources: Vec<ModuleId>,
    /// The output this compilation is for, carried over from [`Unit::platform`]
    /// so that the checker can subset `core/host` to what that platform
    /// grants. `None` for every analysis that is not building one.
    pub platform: Option<Platform>,
}

impl Loaded {
    pub fn module(&self, id: ModuleId) -> &ModuleData {
        self.modules
            .get(id.index())
            .or_ice("a ModuleId is minted by the loader as it pushes the module onto this vector")
    }

    pub fn find(&self, path: &str) -> Option<ModuleId> {
        self.by_path.get(path).copied()
    }
}

pub struct Loader<'a> {
    ws: Option<&'a Workspace>,
    map: &'a mut SourceMap,
    diags: &'a mut Diagnostics,
    /// Parses, shared with every other analysis in this process.
    cache: &'a mut crate::parsing::parser::Cache,
    modules: Vec<ModuleData>,
    by_path: HashMap<String, ModuleId>,
    /// Modules currently being loaded, for the circular-import diagnostic.
    stack: Vec<String>,
    /// See [`Loaded::platform`].
    platform: Option<Platform>,
    test_sources: Vec<ModuleId>,
    /// The schema each `.proto` module was generated from, kept because a
    /// schema importing another needs that one's declarations to resolve its
    /// field types — and because a diamond should be read once.
    schemas: HashMap<String, crate::build::protoschema::Schema>,
}

impl<'a> Loader<'a> {
    pub fn new(
        ws: Option<&'a Workspace>,
        map: &'a mut SourceMap,
        diags: &'a mut Diagnostics,
        cache: &'a mut crate::parsing::parser::Cache,
    ) -> Loader<'a> {
        Loader {
            ws,
            map,
            diags,
            cache,
            modules: Vec::new(),
            by_path: HashMap::default(),
            stack: Vec::new(),
            test_sources: Vec::new(),
            schemas: HashMap::default(),
            platform: None,
        }
    }

    pub fn finish(self) -> Loaded {
        Loaded {
            modules: self.modules,
            by_path: self.by_path,
            test_sources: self.test_sources,
            platform: self.platform,
        }
    }

    /// Loads every module of a unit: the target's entry point, its declared
    /// sources, and everything they import.
    pub fn load_unit(&mut self, unit: &Unit) {
        // The first unit's, and every unit batched into one compilation shares
        // it: `analyze_all` batches test suites, which build no output and
        // carry `None` for exactly that reason.
        if self.platform.is_none() {
            self.platform = unit.platform;
        }
        // The modules that define the built-in types, and no others. A method
        // needs no import (SPEC 6.7.3), so `[T]`'s and `Str`'s defining modules
        // have to be present for `xs.map(...)` and `s.trim()` to resolve in a
        // program that never names them. The rest of the standard library
        // declares methods only on its own types, which a program cannot have
        // without importing the module that declares them — so it loads on
        // import, and nothing pays to parse `core/crypto` to compile a program
        // that has never heard of it.
        self.load_builtin_modules();
        let (Some(ws), Some(target)) = (self.ws, unit.target) else { return };
        let pkg = ws.package(target.package);

        match target.kind {
            RuleKind::Library => {
                let Some(lib) = &pkg.build.library else { return };
                self.check_testing_surface_declared(target);
                self.load_path(&pkg.label(), Role::Source, Span::NONE);
                for src in &lib.proto_sources {
                    self.load_declared_proto(target, &src.value, src.span);
                }
                for src in &lib.sources {
                    self.load_package_source(target, &src.value, Role::Source, src.span);
                }
                if let Some(testing) = &lib.testing {
                    self.load_path(&format!("{}/testing", pkg.label()), Role::TestOnly, Span::NONE);
                    for src in &testing.sources {
                        self.load_package_source(target, &src.value, Role::TestOnly, src.span);
                    }
                }
                if unit.with_tests {
                    for src in lib.test.iter().flat_map(|t| t.sources.iter()) {
                        if let Some(id) =
                            self.load_package_source(target, &src.value, Role::TestSource, src.span)
                        {
                            self.test_sources.push(id);
                        }
                    }
                }
            }
            RuleKind::Binary => {
                let Some(bin) = &pkg.build.binary else { return };
                // A `testing/` surface belongs to the library rule. A package
                // that has no library rule still has the directory, and
                // nothing else would look at it, so the binary asks on its
                // behalf — and only then, so a package with both rules is not
                // told twice.
                if !pkg.has_library() {
                    self.check_testing_surface_declared(target);
                }
                for src in &bin.proto_sources {
                    self.load_declared_proto(target, &src.value, src.span);
                }
                self.load_path(&format!("{}/main", pkg.label()), Role::Entry, Span::NONE);
                for src in &bin.sources {
                    self.load_package_source(target, &src.value, Role::Source, src.span);
                }
                if unit.with_tests {
                    for src in bin.test.iter().flat_map(|t| t.sources.iter()) {
                        if let Some(id) =
                            self.load_package_source(target, &src.value, Role::TestSource, src.span)
                        {
                            self.test_sources.push(id);
                        }
                    }
                }
            }
        }
    }

    /// The prelude's modules are in scope in every module, so they are always
    /// part of a compilation whether or not anything imports them.
    fn load_prelude(&mut self) {
        for path in standard_library::prelude_modules() {
            self.load_std(path, Span::NONE);
        }
    }

    /// The modules a compilation always needs: the prelude, and the defining
    /// module of every built-in type. See `standard_library::EAGER_MODULES`.
    pub fn load_builtin_modules(&mut self) {
        self.load_prelude();
        for path in standard_library::eager_modules() {
            self.load_std(path, Span::NONE);
        }
    }

    /// One standard library module by path, with its imports.
    pub fn load_std_module(&mut self, path: &str) {
        self.load_std(path, Span::NONE);
    }

    /// Every standard library module, for the toolchain's own self-check.
    pub fn load_all_std(&mut self) {
        self.load_prelude();
        for m in standard_library::MODULES {
            self.load_std(m.path, Span::NONE);
        }
    }

    /// Loads a module from text rather than from disk.
    ///
    /// This is what lets a fenced block in a document be compiled in-process:
    /// the documentation examples are real modules, checked by the real
    /// checker against the real standard library, with no temporary directory
    /// and no second process. `name` is what diagnostics will call the module,
    /// so callers pass the document's path and remap the line afterwards.
    ///
    /// `Role::Std` parses with bodyless declarations allowed, which is how a
    /// document can show a signature without inventing an implementation for
    /// it.
    pub fn load_source(
        &mut self,
        path: &str,
        role: Role,
        text: String,
    ) -> Option<ModuleId> {
        self.load_source_in(path, role, text, None)
    }

    /// The same, but the module belongs to `pkg`.
    ///
    /// Documentation about a library shows the library's *own* files —
    /// `cents.buri` importing its neighbour — and those imports are legal only
    /// from inside the package. Saying which package the example belongs to is
    /// what lets such a block be compiled rather than merely displayed.
    pub fn load_source_in(
        &mut self,
        path: &str,
        role: Role,
        text: String,
        pkg: Option<crate::build::workspace::PackageId>,
    ) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        let file = self.map.add(path.to_string(), PathBuf::new(), text);
        let bodyless = matches!(role, Role::Std | Role::Platform);
        let (ast, errors) = self.cache.parse(self.map.text(file), file, bodyless);
        self.diags.extend(errors.iter().cloned());
        let id = ModuleId(self.modules.len() as u32);
        self.by_path.insert(path.to_string(), id);
        self.modules.push(ModuleData {
            id,
            path: path.to_string(),
            file,
            role,
            ast,
            pkg,
            disk: None,
        });
        self.stack.push(path.to_string());
        self.load_imports(id);
        self.stack.pop();
        Some(id)
    }

    /// The `testing` block is required when the directory is there
    /// (BUILD-FILES.md:194-196).
    ///
    /// Nothing else can ask this. `undeclared-source` walks the files and
    /// finds the ones inside `testing/` — but `testing/lib.buri` is an entry
    /// point, so it is in the known set unconditionally, and a `testing/`
    /// directory holding nothing but its own entry point passed with no block
    /// at all. The surface was then invisible: no target compiled it, and
    /// `//pkg/testing` resolved to a file the build had never heard of.
    fn check_testing_surface_declared(&mut self, target: TargetId) {
        let Some(ws) = self.ws else { return };
        let pkg = ws.package(target.package);
        let dir = pkg.dir.join("testing");
        if !dir.is_dir() {
            return;
        }
        // A `testing/` that carries its own BUILD.buri is a package of its
        // own, and its files are that package's business.
        if ws.owning_package(&dir.join("x")) != Some(target.package) {
            return;
        }
        if pkg.build.library.as_ref().is_some_and(|l| l.testing.is_some()) {
            return;
        }
        self.diags.push(
            Diagnostic::templated("undeclared-testing-surface", Span::point(pkg.build_file_id, 0))
                .with_bind("package", pkg.label())
                .with_bind("package_path", pkg.path.as_str()),
        );
    }

    /// A source listed in a rule, named by its package-relative path.
    fn load_package_source(
        &mut self,
        target: TargetId,
        rel: &str,
        role: Role,
        span: Span,
    ) -> Option<ModuleId> {
        let ws = self.ws?;
        let pkg = ws.package(target.package);
        // An entry point is named by the rule kind rather than listed
        // (BUILD-FILES.md:140-144, 194-196). Listing one says nothing the rule
        // did not already say, and it reads as though the rule could be
        // written without it — which is not a state the build system wants to
        // have a diagnostic for.
        if is_entry_point(rel) {
            self.diags.push(
                Diagnostic::templated("entry-point-listed", span).with_bind("source", rel),
            );
            return None;
        }
        let disk = pkg.dir.join(rel);
        if !disk.is_file() {
            self.diags.push(
                Diagnostic::templated("no-such-source", span)
                    .with_bind("source", rel)
                    .with_bind("field", "sources"),
            );
            return None;
        }
        let stem = rel.strip_suffix(".buri").unwrap_or(rel);
        let path = if pkg.path.is_empty() {
            format!("//{stem}")
        } else {
            format!("//{}/{stem}", pkg.path)
        };
        self.load_file(&path, disk, role, span)
    }

    /// A `.proto` a rule lists in `proto_sources`.
    fn load_declared_proto(&mut self, target: TargetId, rel: &str, span: Span) -> Option<ModuleId> {
        let ws = self.ws?;
        let pkg = ws.package(target.package);
        if !rel.ends_with(".proto") {
            self.diags.push(
                Diagnostic::templated("proto-source-not-a-schema", span).with_bind("source", rel),
            );
            return None;
        }
        let disk = pkg.dir.join(rel);
        if !disk.is_file() {
            self.diags.push(
                Diagnostic::templated("no-such-source", span)
                    .with_bind("source", rel)
                    .with_bind("field", "proto_sources"),
            );
            return None;
        }
        let path = if pkg.path.is_empty() {
            format!("//{rel}")
        } else {
            format!("//{}/{rel}", pkg.path)
        };
        self.load_proto(&path, disk, Role::Source, span)
    }

    /// Reads a `.proto` schema and loads the module it *becomes*.
    ///
    /// This is the one module in a compilation that is generated rather than
    /// read. Everything downstream is unchanged: the generated text goes
    /// through `load_source_in`, the same seam the documentation harness
    /// compiles a fenced block through, so the real parser and the real checker
    /// see it. A type error in generated code is therefore a toolchain bug that
    /// fails loudly rather than a wrong program that compiles.
    fn load_proto(
        &mut self,
        path: &str,
        disk: PathBuf,
        role: Role,
        span: Span,
    ) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        if let Some(at) = self.stack.iter().position(|p| p == path) {
            let cycle = self.stack.get(at..).unwrap_or_default().join(" -> ");
            self.diags.push(
                Diagnostic::templated("proto-circular-import", span)
                    .with_bind("cycle", cycle)
                    .with_bind("path", path),
            );
            return None;
        }

        let rel = match self.ws {
            Some(ws) => ws.rel_of(&disk),
            None => disk.display().to_string(),
        };
        let file = match self.map.load(&rel, &disk) {
            Ok(f) => f,
            Err(e) => {
                self.diags.push(
                    Diagnostic::error(span, format!("cannot read {rel}: {e}"))
                        .with_fix("check the file exists and is readable"),
                );
                return None;
            }
        };
        let parsed = crate::build::protoschema::parse(self.map.text(file), file);
        for d in parsed.errors {
            self.diags.push(d);
        }
        let schema = parsed.schema;
        self.schemas.insert(path.to_string(), schema.clone());

        // A schema's imports are written from the repository root, so they are
        // module paths once `//` is put in front of them. They have to be
        // loaded first: a field's type may live in any of them.
        self.stack.push(path.to_string());
        let mut deps: Vec<(String, crate::build::protoschema::Schema)> = Vec::new();
        for import in &schema.imports {
            let dep_path = crate::build::protogen::import_module_path(&import.path);
            match self.ws.map(|ws| ws.resolve_module(&dep_path)) {
                Some(Ok(ModuleLocation::InPackage(m))) if m.kind == ModuleKind::Proto => {
                    self.load_proto(&dep_path, m.file, role, import.span);
                }
                _ => {
                    self.diags.push(crate::build::protogen::unresolved_import(
                        import.span,
                        &import.path,
                    ));
                    continue;
                }
            }
            if let Some(dep) = self.schemas.get(&dep_path) {
                deps.push((dep_path, dep.clone()));
            }
        }
        let mut gen_diags = Vec::new();
        let generated =
            crate::build::protogen::generate(&rel, &schema, &deps, &mut gen_diags);
        for d in gen_diags {
            self.diags.push(d);
        }
        self.stack.pop();

        let pkg = self.ws.and_then(|ws| ws.owning_package(&disk));
        self.load_source_in(path, role, generated.source, pkg)
    }

    fn load_std(&mut self, path: &str, span: Span) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        let Some(text) = standard_library::source(path) else {
            self.diags.push(
                Diagnostic::templated("no-such-module", span)
                    .with_bind("path", path)
                    .with_bind("roots", standard_library::roots_phrase()),
            );
            return None;
        };
        let file = self.map.embedded(path, text);
        let (ast, errors) = self.cache.parse(self.map.text(file), file, true);
        self.diags.extend(errors.iter().cloned());
        let role = if standard_library::is_platform_module(path) { Role::Platform } else { Role::Std };
        let id = ModuleId(self.modules.len() as u32);
        self.by_path.insert(path.to_string(), id);
        self.modules.push(ModuleData {
            id,
            path: path.to_string(),
            file,
            role,
            ast,
            pkg: None,
            disk: None,
        });
        self.load_imports(id);
        Some(id)
    }

    /// Loads by module path, resolving through the workspace.
    fn load_path(&mut self, path: &str, role: Role, span: Span) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        if standard_library::is_std_path(path) {
            return self.load_std(path, span);
        }
        let Some(ws) = self.ws else {
            self.diags.push(
                Diagnostic::templated("module-outside-repository", span).with_bind("path", path),
            );
            return None;
        };
        match ws.resolve_module(path) {
            Ok(ModuleLocation::InPackage(m)) if m.kind == ModuleKind::Proto => {
                self.load_proto(path, m.file, role, span)
            }
            Ok(ModuleLocation::InPackage(m)) => self.load_file(path, m.file, role, span),
            Ok(ModuleLocation::Std { .. }) => self.load_std(path, span),
            Err(msg) => {
                // The resolver says which of the several ways a path can fail
                // to name a file this one took, so the whole sentence is bound.
                self.diags.push(
                    Diagnostic::templated("module-not-found", span).with_bind("problem", msg),
                );
                None
            }
        }
    }

    fn load_file(
        &mut self,
        path: &str,
        disk: PathBuf,
        role: Role,
        span: Span,
    ) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        // Circular imports are an error, at the module level exactly as at the
        // package level.
        if let Some(at) = self.stack.iter().position(|p| p == path) {
            let cycle = self.stack.get(at..).unwrap_or_default().join(" -> ");
            self.diags.push(
                Diagnostic::templated("circular-import", span)
                    .with_bind("cycle", cycle)
                    .with_bind("path", path),
            );
            return None;
        }

        let rel = match self.ws {
            Some(ws) => ws.rel_of(&disk),
            None => disk.display().to_string(),
        };
        let file = match self.map.load(&rel, &disk) {
            Ok(f) => f,
            Err(e) => {
                self.diags.push(
                    Diagnostic::error(span, format!("cannot read {rel}: {e}"))
                        .with_fix("check the file exists and is readable"),
                );
                return None;
            }
        };
        let (ast, errors) = self.cache.parse(self.map.text(file), file, false);
        self.diags.extend(errors.iter().cloned());

        let pkg = self.ws.and_then(|ws| ws.owning_package(&disk));
        let id = ModuleId(self.modules.len() as u32);
        self.by_path.insert(path.to_string(), id);
        self.modules.push(ModuleData {
            id,
            path: path.to_string(),
            file,
            role,
            ast,
            pkg,
            disk: Some(disk),
        });

        self.stack.push(path.to_string());
        self.load_imports(id);
        self.stack.pop();
        Some(id)
    }

    /// Loads everything a module imports, checking each import line against
    /// the rules that govern where a path may be named from.
    fn load_imports(&mut self, id: ModuleId) {
        // The names an import binds travel with it, because one of the
        // restrictions below has to say what to re-export, and a fix that names
        // the symbol is the difference between a rule and an instruction.
        let importer = self
            .modules
            .get(id.index())
            .or_ice("a ModuleId is minted by the loader as it pushes the module onto `modules`");
        let t = &importer.ast.tree;
        let imports: Vec<(String, Span, Vec<String>)> = importer
            .ast
            .items
            .iter()
            .filter_map(|item| match item {
                tree::Item::Import(i) => {
                    let names = match &i.clause {
                        tree::ImportClause::Named(specs) => {
                            specs.iter().map(|s| t.name(s.name).to_string()).collect()
                        }
                        tree::ImportClause::Namespace(_) => Vec::new(),
                    };
                    Some((i.path.clone(), i.path_span, names))
                }
                tree::Item::ReExport(r) => Some((
                    r.path.clone(),
                    r.path_span,
                    r.specs.iter().map(|s| t.name(s.name).to_string()).collect(),
                )),
                _ => None,
            })
            .collect();

        let role = importer.role;
        let importer_path = importer.path.clone();
        let importer_pkg = importer.pkg;

        for (path, span, names) in imports {
            if !self.check_import_legality(&importer_path, importer_pkg, role, &path, span, &names)
            {
                continue;
            }
            // What a module imports is loaded in the role its own path
            // implies, not in the importer's role.
            let target_role = self.role_for(&path);
            self.load_path(&path, target_role, span);
        }
    }

    /// The role a module is loaded in when something imports it.
    ///
    /// A module's role is a property of the module, not of whoever named it,
    /// so this asks the path and the workspace rather than the importer. The
    /// case that matters is a binary's entry point: `main.buri` is an `Entry`
    /// wherever it is reached from, and the only thing that may reach it is
    /// that binary's own test sources (TESTING.md, "Testing a binary"). Loaded
    /// as ordinary `Source` it would have its `core/host` import and its
    /// `context` rejected — the two things an entry point exists to do.
    ///
    /// In a real build this was latent, because `load_unit` pre-loads the
    /// entry point as `Role::Entry` before anything can import it. A test
    /// binary compiled on its own, or a documentation example standing in the
    /// package, reaches it here first.
    fn role_for(&self, path: &str) -> Role {
        if standard_library::is_std_path(path) {
            return if standard_library::is_platform_module(path) { Role::Platform } else { Role::Std };
        }
        if is_test_only_path(path) {
            return Role::TestOnly;
        }
        if let Some(ws) = self.ws {
            let resolved = ws.resolve_module(path);
            let entry = matches!(
                resolved,
                Ok(ModuleLocation::InPackage(m)) if m.kind == ModuleKind::BinaryEntry
            );
            if entry {
                return Role::Entry;
            }
        }
        Role::Source
    }

    /// The import restrictions. Each one is visible in the import line, which
    /// is where the person writing it is looking.
    fn check_import_legality(
        &mut self,
        importer_path: &str,
        importer_pkg: Option<crate::build::workspace::PackageId>,
        role: Role,
        path: &str,
        span: Span,
        names: &[String],
    ) -> bool {
        if path.starts_with('.') {
            self.diags
                .push(Diagnostic::templated("relative-import", span).with_bind("path", path));
            return false;
        }

        // `core/host` is importable only from the module that exports `main`.
        if path == "core/host" && role != Role::Entry {
            self.diags.push(Diagnostic::templated("host-import", span));
            return false;
        }

        // A path containing a `testing` segment is importable only from a test
        // source — or from another test-only module.
        if is_test_only_path(path) && !role.is_test_context() {
            // The second note names the importer, which the page cannot.
            self.diags.push(
                Diagnostic::templated("test-only-import", span)
                    .with_note(format!("{importer_path} is not one")),
            );
            return false;
        }

        let Some(ws) = self.ws else { return true };
        if !path.starts_with("//") {
            return true;
        }

        // A `//...` path always resolves inside this repository, so there is no
        // `core/` case to skip past here.
        let Ok(ModuleLocation::InPackage(loc)) = ws.resolve_module(path) else {
            // `load_path` reports the resolution failure itself.
            return true;
        };

        // A test source is not a module anybody can name. Test sources are
        // compiled independently — one test binary each — so there is nothing
        // for an import to resolve to, whoever writes it (TESTING.md, "What a
        // test can reach").
        if is_declared_test_source(ws, Some(loc.package), path) {
            self.diags
                .push(Diagnostic::templated("test-source-import", span).with_bind("path", path));
            return false;
        }

        match loc.kind {
            // A `//pkg/inner` import resolves only inside `//pkg`. A
            // generated `.proto` module is one of these: it belongs to the
            // rule that declared the schema, and reaches the outside world the
            // way every other internal module does — through `lib.buri`.
            ModuleKind::Internal | ModuleKind::Proto => {
                if Some(loc.package) != importer_pkg {
                    let owner = ws.package(loc.package).label();
                    self.diags.push(
                        Diagnostic::templated("internal-import", span)
                            .with_bind("path", path)
                            .with_bind("owner_path", owner.trim_start_matches("//"))
                            .with_bind("owner", owner.as_str()),
                    );
                    return false;
                }
                // Inside the package, every other module may reach it — but a
                // test source may not. A test reaches its library the way a
                // dependent does, and that is the rule that confines a suite to
                // the public surface (TESTING.md:105-130).
                if is_declared_test_source(ws, importer_pkg, importer_path) {
                    let owner = ws.package(loc.package).label();
                    let dir = owner.trim_start_matches("//");
                    // The names the import asked for, already spelled as a
                    // phrase — the page has no way to list them.
                    let what = match names {
                        [] => "what the test needs".to_string(),
                        [one] => format!("`{one}`"),
                        many => format!("`{}`", many.join("`, `")),
                    };
                    self.diags.push(
                        Diagnostic::templated("test-internal-import", span)
                            .with_bind("test_source", source_file_of(importer_path))
                            .with_bind("owner", owner.as_str())
                            .with_bind("exports", what)
                            .with_bind("owner_path", dir),
                    );
                    return false;
                }
                // Inside one package, the boundary is still there: it belongs
                // to the *rule*, not to the directory (BUILD-FILES.md:301-308).
                // A binary's sources reach the library beside them only
                // through `//pkg`, and a library may not reach the binary at
                // all. Asking which package the importer is in answered the
                // first question with "yes, it is right there", which is
                // exactly the case the two rules are about.
                let rule_of = |pkg: Option<crate::build::workspace::PackageId>, p: &str| {
                    let id = pkg?;
                    let rel = package_relative_source(ws, id, p)?;
                    ws.rule_of_file(id, &rel)
                };
                let importer_rule = rule_of(importer_pkg, importer_path);
                let target_rule = rule_of(Some(loc.package), path);
                if let (Some(from), Some(to)) = (importer_rule, target_rule) {
                    if from != to {
                        let owner = ws.package(loc.package).label();
                        let dir = owner.trim_start_matches("//");
                        let importer_file = source_file_of(importer_path);
                        let d = match to {
                            RuleKind::Library => {
                                Diagnostic::templated("binary-internal-import", span)
                                    .with_bind("path", path)
                                    .with_bind("owner", owner.as_str())
                                    .with_bind("owner_path", dir)
                                    .with_bind("importer_file", importer_file)
                            }
                            RuleKind::Binary => {
                                Diagnostic::templated("binary-source-import", span)
                                    .with_bind("path", path)
                                    .with_bind("owner", owner.as_str())
                                    .with_bind("importer_file", importer_file)
                            }
                        };
                        self.diags.push(d);
                        return false;
                    }
                }
            }
            // A binary's entry point is importable only from that binary's own
            // test sources.
            ModuleKind::BinaryEntry => {
                let same_package = Some(loc.package) == importer_pkg;
                if !same_package || !role.is_test_context() {
                    self.diags.push(
                        Diagnostic::templated("binary-entry-import", span).with_bind("path", path),
                    );
                    return false;
                }
            }
            _ => {}
        }
        true
    }
}

/// The three package-relative names a rule names by its kind rather than by
/// listing: a library's surface, a binary's entry point, and the `testing`
/// block's surface.
fn is_entry_point(rel: &str) -> bool {
    matches!(rel, "lib.buri" | "main.buri" | "testing/lib.buri")
}

/// The file a `//...` module path names, relative to the repository root:
/// `//lib/money/test/cents` -> `lib/money/test/cents.buri`.
///
/// Used only in prose, so a path that is not a repository path comes back
/// unchanged rather than being an error.
fn source_file_of(path: &str) -> String {
    match path.strip_prefix("//") {
        Some(rest) if rest.ends_with(".proto") => rest.to_string(),
        Some(rest) => format!("{rest}.buri"),
        None => path.to_string(),
    }
}

/// The file a module path names inside its own package:
/// `//lib/money/test/cents` -> `test/cents.buri`.
fn package_relative_source(
    ws: &Workspace,
    pkg: crate::build::workspace::PackageId,
    path: &str,
) -> Option<String> {
    let rest = path.strip_prefix("//")?;
    let pkg_path = ws.package(pkg).path.clone();
    let rel = if pkg_path.is_empty() {
        rest
    } else {
        rest.strip_prefix(&format!("{pkg_path}/"))?
    };
    if rel.ends_with(".proto") {
        return Some(rel.to_string());
    }
    Some(format!("{rel}.buri"))
}

/// True when a rule lists this module in its `test.sources`.
///
/// That is the only thing that makes a module a test source (TESTING.md:37-40)
/// — not the directory it sits in and not a flag — so it is also the only thing
/// worth asking. A snippet compiled from a document is named by its origin
/// rather than by a `//...` path, so it is never one, which is what keeps a
/// documented example of a library's own internals compilable.
fn is_declared_test_source(
    ws: &Workspace,
    pkg: Option<crate::build::workspace::PackageId>,
    path: &str,
) -> bool {
    let Some(pkg_id) = pkg else { return false };
    let Some(rel) = package_relative_source(ws, pkg_id, path) else { return false };
    let build = &ws.package(pkg_id).build;
    let listed = |suite: &crate::build::buildfile::TestSuite| {
        suite.sources.iter().any(|s| s.value == rel)
    };
    build.library.as_ref().and_then(|l| l.test.as_ref()).is_some_and(listed)
        || build.binary.as_ref().and_then(|b| b.test.as_ref()).is_some_and(listed)
}
