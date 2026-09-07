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
    /// The module's canonical path, which for a repository module is the file:
    /// `//lib/money/cents.buri`, `//lib/money/lib.buri`. For the standard
    /// library it is the module: `core/list`. Canonical rather than as
    /// written, because a repository module has two legal spellings — see
    /// [`crate::build::workspace::PackageModule::path`].
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
    /// The exported function the output being built enters through.
    ///
    /// `Some("fetch")` says that this artifact starts at `fetch` and holds only
    /// what `fetch` reaches, so `main`'s own `core/host` bindings are none of
    /// this build's business. `None` is every analysis that is not building one
    /// artifact, and every build of the default entry.
    pub entry: Option<String>,
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
    /// The entry the output being built enters through, carried over from
    /// [`Unit::entry`]. See it for what it decides.
    pub entry: Option<String>,
    /// The platforms the suites in this compilation declared, by package.
    ///
    /// A suite's `test.platforms` is not one of its binary's `outputs`, and it
    /// is still a platform that binary's entry point has to compile for: a
    /// batched test binary links `main` in, so a suite that runs on WEB
    /// compiles `main.buri` for WEB whatever the `outputs` say. Recorded per
    /// package because `analyze_all` batches many targets into one
    /// compilation, and each one's suite speaks only for its own entry point.
    pub test_platforms: HashMap<crate::build::workspace::PackageId, Vec<Platform>>,
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
    /// The rules whose generators this compilation has already reported on.
    generated_rules: std::collections::BTreeSet<TargetId>,
    /// See [`Loaded::platform`].
    platform: Option<Platform>,
    /// See [`Loaded::entry`].
    entry: Option<String>,
    /// See [`Loaded::test_platforms`].
    test_platforms: HashMap<crate::build::workspace::PackageId, Vec<Platform>>,
    test_sources: Vec<ModuleId>,
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
            generated_rules: std::collections::BTreeSet::new(),
            test_sources: Vec::new(),
            platform: None,
            entry: None,
            test_platforms: HashMap::default(),
        }
    }

    pub fn finish(self) -> Loaded {
        Loaded {
            modules: self.modules,
            by_path: self.by_path,
            test_sources: self.test_sources,
            platform: self.platform,
            entry: self.entry,
            test_platforms: self.test_platforms,
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
            self.entry = unit.entry.clone();
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
        // Recorded before the sources load, so that the entry point this unit
        // pulls in already knows which platforms its suite runs on.
        //
        // A binary's suite only. A library's suite links no `main` — and a
        // package may hold both rules, so recording a library suite's
        // platforms under the package would put them on a binary entry point
        // that its run never touches.
        if unit.with_tests && target.kind == RuleKind::Binary {
            if let Some(suite) = pkg.test_suite(target.kind) {
                if !suite.platforms.is_empty() {
                    self.test_platforms
                        .entry(target.package)
                        .or_default()
                        .extend(suite.platforms.iter().map(|p| p.value));
                }
            }
        }

        match target.kind {
            RuleKind::Library => {
                if pkg.build.library.is_none() {
                    return;
                }
                self.check_testing_surface_declared(target);
                self.load_path(&pkg.module_path("lib.buri"), Role::Source, Span::NONE);
                self.load_closure_generators(target);
                let Some(lib) = &pkg.build.library else { return };
                for src in &lib.sources {
                    self.load_package_source(target, &src.value, Role::Source, src.span);
                }
                if let Some(testing) = &lib.testing {
                    self.load_path(&pkg.module_path("testing/lib.buri"), Role::TestOnly, Span::NONE);
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
                if pkg.build.binary.is_none() {
                    return;
                }
                // A `testing/` surface belongs to the library rule. A package
                // that has no library rule still has the directory, and
                // nothing else would look at it, so the binary asks on its
                // behalf — and only then, so a package with both rules is not
                // told twice.
                if !pkg.has_library() {
                    self.check_testing_surface_declared(target);
                }
                self.load_closure_generators(target);
                let Some(bin) = &pkg.build.binary else { return };
                self.load_path(&pkg.module_path("main.buri"), Role::Entry, Span::NONE);
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
        // Generated text has no file on disk, so the module path is its whole
        // identity — and a map kept between compilations (`build::sources`)
        // already holds the last one. Adding unconditionally minted a fresh id
        // for the same module on every analysis, which re-parsed it and left
        // the old entry in the map for good.
        let file = match self.map.find(path) {
            Some(id) if self.map.text(id) == text => id,
            Some(id) => {
                self.map.replace(id, text);
                self.cache.forget(id);
                id
            }
            None => self.map.add(path.to_string(), PathBuf::new(), text),
        };
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
    /// Nothing else can ask this. `unused-library` walks the files and
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
        // The path *is* the file, so nothing is stripped: what a rule listed
        // in `sources` and what an import writes are one string.
        let path = pkg.module_path(rel);
        self.load_file(&path, disk, role, span)
    }

    /// Every rule in this target's closure that declares a generator.
    ///
    /// Over the closure rather than over the target, because a dependency's
    /// generated modules are what the dependent is built from: a library whose
    /// generator failed has to say so wherever it is compiled into something,
    /// not only when that library is the target named on the command line.
    fn load_closure_generators(&mut self, target: TargetId) {
        let Some(ws) = self.ws else { return };
        for member in ws.closure(target) {
            self.load_generators(member);
        }
    }

    /// Everything one rule's `generators` produced: the diagnostics the run
    /// collected, then a module per entry the generator named.
    ///
    /// The work happened in the build layer — `generators::prepare`, at the one
    /// door every command opens a repository through — because the front end
    /// can neither build nor spawn a tool. What arrives here is data.
    fn load_generators(&mut self, target: TargetId) {
        let Some(ws) = self.ws else { return };
        if crate::build::generators::declared(ws, target).is_empty() {
            return;
        }
        // Once per rule per compilation. `analyze_all` batches units and every
        // unit asks about its whole closure, so without this a library two
        // targets both depend on reports its generator's diagnostics twice.
        if !self.generated_rules.insert(target) {
            return;
        }
        // Before anything the run produced: a tool built from the target that
        // declares it is a cycle, and saying so is the whole answer.
        if let Some((generator, path)) = crate::build::generators::cycle_of(ws, target) {
            self.diags.push(
                Diagnostic::templated("generator-cycle", generator.tool.span)
                    .with_bind("tool", generator.tool.value.as_str())
                    .with_bind("target", ws.label(target))
                    .with_bind("path", crate::build::generators::cycle_sentence(ws, &path)),
            );
            return;
        }
        let Some(outcome) = ws.generated.outcome(target) else { return };
        for (d, span) in &outcome.diagnostics {
            let reported = self.generator_diagnostic(d, *span);
            self.diags.push(reported);
        }
        let pkg = ws.package(target.package);
        for module in &outcome.modules {
            let path = pkg.module_path(&module.name);
            self.load_generated(&path, Role::Source);
        }
    }

    /// One diagnostic a generator reported, as one this toolchain prints.
    ///
    /// A code the catalogue knows prints under that code with the generator's
    /// own sentence, which is what keeps every `proto-*` page working when the
    /// `.proto` reader is a generator. A code it does not know prints under
    /// `generator-diagnostic`, naming the code the generator asked for — a
    /// generator cannot invent a page, and a diagnostic with no page has no
    /// wording anybody can hold it to.
    fn generator_diagnostic(
        &mut self,
        d: &crate::build::generators::Diagnostic,
        entry: Span,
    ) -> Diagnostic {
        let span = self.generator_origin(d.origin.as_ref()).unwrap_or(entry);
        // The two the loader raises itself, whose wording is their page's.
        if d.code == "generator-failed" {
            let mut reported = Diagnostic::templated("generator-failed", span);
            if let Some(note) = &d.note {
                reported = reported.with_note(note.clone());
            }
            return reported;
        }
        if d.code == "no-such-source" {
            return Diagnostic::templated("no-such-source", span)
                .with_bind("source", d.message.clone())
                .with_bind("field", "generators");
        }
        let known = crate::documentation::page_of_code(&d.code).is_some();
        let mut reported = match known {
            true => Diagnostic::error(span, d.message.clone()).with_code(d.code.clone()),
            false => Diagnostic::templated("generator-diagnostic", span)
                .with_bind("code", d.code.clone())
                .with_bind("message", d.message.clone()),
        };
        if let Some(note) = &d.note {
            reported = reported.with_note(note.clone());
        }
        if let Some(fix) = &d.fix {
            reported = reported.with_fix(fix.clone());
        }
        reported
    }

    /// The span a generator's origin names, once the file it names is in the
    /// source map.
    ///
    /// `None` when the generator named a file this repository does not have,
    /// which leaves the diagnostic on the `generators` entry rather than on a
    /// position nobody can open.
    fn generator_origin(
        &mut self,
        origin: Option<&crate::build::generators::Origin>,
    ) -> Option<Span> {
        let origin = origin?;
        let ws = self.ws?;
        let disk = ws.root.join(&origin.file);
        let file = self.map.load(&origin.file, &disk).ok()?;
        let len = self.map.text(file).len();
        let start = origin.span.0.min(len);
        let end = origin.span.1.min(len).max(start);
        Some(Span::new(file, start, end))
    }

    /// A module a generator produced. Its text comes from the store, and it
    /// goes through `load_source_in` — the same seam a `.proto` module and a
    /// documented fence take, so the real parser and the real checker see it.
    fn load_generated(&mut self, path: &str, role: Role) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        let ws = self.ws?;
        let module = ws.generated.module(path)?;
        let pkg = ws.generated.owner(path).map(|t| t.package);
        self.load_source_in(path, role, module.text.clone(), pkg)
    }

    fn load_std(&mut self, path: &str, span: Span) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        // `core/effect` is what the table holds and `core/effect/lib.buri`
        // names the same module the long way round. The module is keyed by the
        // canonical spelling, so the two cannot become two.
        let Some(module) = standard_library::find(path) else {
            // A path this library used to have is a different mistake from a
            // path it never had, and the reader can be told the answer rather
            // than the rule. It is still a refusal: the old name does not
            // load, so nothing compiles against two spellings of one module.
            let diagnostic = match standard_library::retired(path) {
                Some(now) => Diagnostic::templated("retired-module", span)
                    .with_bind("path", path)
                    .with_bind("now", now),
                None => Diagnostic::templated("no-such-module", span)
                    .with_bind("path", path)
                    .with_bind("roots", standard_library::roots_phrase()),
            };
            self.diags.push(diagnostic);
            return None;
        };
        let (written, text) = (path, module.source);
        let path = module.path;
        if let Some(id) = self.by_path.get(path) {
            let id = *id;
            self.alias(written, id);
            return Some(id);
        }
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
        self.alias(written, id);
        self.load_imports(id);
        Some(id)
    }

    /// Records that `written` reaches the module already loaded as `id`.
    ///
    /// A module has one identity — one [`ModuleData`], one canonical path —
    /// and two legal spellings, because which one an import writes depends on
    /// whether it crosses a package boundary. `by_path` is therefore every
    /// spelling that reaches a module rather than one key per module: the
    /// resolver looks a module up by the string in the import line, and both
    /// strings have to land on the same module or the types they carry are
    /// not the same types.
    fn alias(&mut self, written: &str, id: ModuleId) {
        if !self.by_path.contains_key(written) {
            self.by_path.insert(written.to_string(), id);
        }
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
        // `m.path` rather than `path`: two spellings reach one file — a module
        // is `//lib/money` from outside and `//lib/money/lib.buri` from inside
        // — and the module is keyed by the file, so the two cannot become two
        // copies of everything the file declares.
        match ws.resolve_module(path) {
            Ok(ModuleLocation::InPackage(m)) => {
                let (canonical, kind, file) = (m.path, m.kind, m.file);
                let id = match kind {
                    ModuleKind::Generated => self.load_generated(&canonical, role),
                    _ => self.load_file(&canonical, file, role, span),
                };
                if let Some(id) = id {
                    self.alias(path, id);
                }
                id
            }
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
        // Asked of the canonical spelling, so that naming the surface file the
        // long way round is not a way past the gate.
        if standard_library::canonical(path) == Some(standard_library::HOST_MODULE)
            && role != Role::Entry
        {
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
        if is_declared_test_source(ws, Some(loc.package), &loc.path) {
            self.diags
                .push(Diagnostic::templated("test-source-import", span).with_bind("path", path));
            return false;
        }

        // A surface is named as a module — `//lib/money`, `//lib/money/testing`
        // — wherever it is written, including from inside the package that
        // owns it: a suite reaches its own library the way its dependents do.
        // Every other module is a file, and its path is that file's name.
        //
        // So the check is on the *kind*, and the same-package guard is only
        // about which diagnostic gets to speak: from outside, a path naming a
        // module inside another package is `internal-import`, and telling its
        // writer to add `.buri` would be telling them to write a different
        // error.
        //
        // The fix is read off the resolver rather than guessed. One textual
        // rule cannot produce both answers: in a repository holding
        // `lib/money/cents.buri` and `cmd/app/main.buri`, `//lib/money/cents`
        // means `cents.buri` while `//cmd/app/main` means `main.buri`, and
        // only the layout says which.
        //
        // A generated module is the third thing that names itself: it has no
        // file, and the name is whatever the generator called it. Telling its
        // importer to add `.buri` would name a file that will never exist.
        let names_itself = matches!(
            loc.kind,
            ModuleKind::LibrarySurface | ModuleKind::TestingSurface | ModuleKind::Generated
        );
        if Some(loc.package) == importer_pkg
            && !names_itself
            && !crate::build::workspace::names_a_file(path)
        {
            let d = Diagnostic::templated("import-path-without-a-file", span)
                .with_bind("path", path)
                .with_fix(format!("write \"{}\"", loc.path))
                .with_edit(span.inside_quotes(self.map.text(span.file)), &loc.path);
            self.diags.push(d);
            return false;
        }

        match loc.kind {
            // A `//pkg/inner` import resolves only inside `//pkg`. A
            // generated module is one of these: it belongs to the rule that
            // declared the generator, and reaches the outside world the way
            // every other internal module does — through `lib.buri`.
            ModuleKind::Internal | ModuleKind::Generated => {
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
                // `loc.path` and not `path`: this asks which *file* a rule
                // owns, so it needs the module's canonical path rather than
                // the string somebody typed. An extensionless spelling would
                // otherwise find no file and skip the check in silence.
                let importer_rule = rule_of(importer_pkg, importer_path);
                let target_rule = rule_of(Some(loc.package), &loc.path);
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
/// `//lib/money/test/cents.buri` -> `lib/money/test/cents.buri`.
///
/// Since an import names a file this is only the `//` coming off. It is kept
/// as a function because the *reason* is worth a name: prose wants the file,
/// and a path that is not a repository path comes back unchanged rather than
/// being an error.
fn source_file_of(path: &str) -> String {
    path.strip_prefix("//").unwrap_or(path).to_string()
}

/// The file a module path names inside its own package:
/// `//lib/money/test/cents.buri` -> `test/cents.buri`.
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
    Some(rel.to_string())
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
