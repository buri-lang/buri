//! Loading a compilation: which modules a target is made of, and where each
//! one comes from.
//!
//! A source file is a module, named by its path from the repository root.
//! Loading follows imports, and the restrictions on which module may import
//! which — the library boundary, `core/host`, and the `testing` segment — are
//! checked here, where the import line is.

use crate::build::buildfile::Platform;
use crate::build::workspace::{is_test_only_path, ModuleKind, RuleKind, TargetId, Workspace};
use crate::compiler::semantics::types::ModuleId;
use crate::compiler::standard_library;
use crate::diagnostics::{Diagnostic, Diagnostics, FileId, SourceMap, Span};
use crate::parsing::tree;
use std::collections::HashMap;
use std::path::PathBuf;

/// What a module is being compiled as. This is what decides whether `test`
/// declarations, expression statements, and test-only imports are legal in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// A `core/...` module, shipping with the toolchain.
    Std,
    /// A platform module: `core/cap`, `core/host`, `core/testing/*`. Only
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
    pub fn is_test_context(self) -> bool {
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
    pub ast: tree::Module,
    /// The package this module belongs to, and the target that compiles it.
    pub pkg: Option<crate::build::workspace::PkgId>,
    pub disk: PathBuf,
}

/// One thing to build: a target, a platform, and whether tests are included.
#[derive(Clone, Debug)]
pub struct Unit {
    pub target: Option<TargetId>,
    pub platform: Platform,
    /// Compile the target's `test.sources` too, and run them.
    pub with_tests: bool,
}

pub struct Loaded {
    pub modules: Vec<ModuleData>,
    pub by_path: HashMap<String, ModuleId>,
    /// The module exporting `main`, when this unit has one.
    pub entry: Option<ModuleId>,
    /// Modules that are test sources, in declaration order.
    pub test_sources: Vec<ModuleId>,
}

impl Loaded {
    pub fn module(&self, id: ModuleId) -> &ModuleData {
        &self.modules[id.index()]
    }

    pub fn find(&self, path: &str) -> Option<ModuleId> {
        self.by_path.get(path).copied()
    }
}

pub struct Loader<'a> {
    ws: Option<&'a Workspace>,
    map: &'a mut SourceMap,
    diags: &'a mut Diagnostics,
    modules: Vec<ModuleData>,
    by_path: HashMap<String, ModuleId>,
    /// Modules currently being loaded, for the circular-import diagnostic.
    stack: Vec<String>,
    entry: Option<ModuleId>,
    test_sources: Vec<ModuleId>,
}

impl<'a> Loader<'a> {
    pub fn new(
        ws: Option<&'a Workspace>,
        map: &'a mut SourceMap,
        diags: &'a mut Diagnostics,
    ) -> Loader<'a> {
        Loader {
            ws,
            map,
            diags,
            modules: Vec::new(),
            by_path: HashMap::new(),
            stack: Vec::new(),
            entry: None,
            test_sources: Vec::new(),
        }
    }

    pub fn finish(self) -> Loaded {
        Loaded {
            modules: self.modules,
            by_path: self.by_path,
            entry: self.entry,
            test_sources: self.test_sources,
        }
    }

    /// Loads every module of a unit: the target's entry point, its declared
    /// sources, and everything they import.
    pub fn load_unit(&mut self, unit: &Unit) {
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
        let pkg = ws.pkg(target.pkg);

        match target.kind {
            RuleKind::Library => {
                let Some(lib) = &pkg.build.library else { return };
                self.load_path(&pkg.label(), Role::Source, Span::NONE);
                for src in &lib.sources {
                    self.load_package_source(target, &src.value, Role::Source, src.span);
                }
                if lib.testing.present {
                    self.load_path(&format!("{}/testing", pkg.label()), Role::TestOnly, Span::NONE);
                    for src in &lib.testing.sources {
                        self.load_package_source(target, &src.value, Role::TestOnly, src.span);
                    }
                }
                if unit.with_tests {
                    for src in &lib.test.sources {
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
                let entry = self.load_path(&format!("{}/main", pkg.label()), Role::Entry, Span::NONE);
                self.entry = entry;
                for src in &bin.sources {
                    self.load_package_source(target, &src.value, Role::Source, src.span);
                }
                if unit.with_tests {
                    for src in &bin.test.sources {
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
    pub fn load_prelude(&mut self) {
        for path in standard_library::PRELUDE_MODULES {
            self.load_std(path, Span::NONE);
        }
    }

    /// The modules a compilation always needs: the prelude, and the defining
    /// module of every built-in type. See `standard_library::EAGER_MODULES`.
    pub fn load_builtin_modules(&mut self) {
        self.load_prelude();
        for path in standard_library::EAGER_MODULES {
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
        for path in standard_library::MODULES {
            self.load_std(path, Span::NONE);
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
        pkg: Option<crate::build::workspace::PkgId>,
    ) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        let file = self.map.add(path.to_string(), PathBuf::new(), text);
        let parsed = match role {
            Role::Std | Role::Platform => {
                crate::parsing::parser::parse_stdlib(self.map.text(file), file)
            }
            _ => crate::parsing::parser::parse(self.map.text(file), file),
        };
        self.diags.extend(parsed.errors);
        let id = ModuleId(self.modules.len() as u32);
        self.by_path.insert(path.to_string(), id);
        self.modules.push(ModuleData {
            id,
            path: path.to_string(),
            file,
            role,
            ast: parsed.module,
            pkg,
            disk: PathBuf::new(),
        });
        if role == Role::Entry {
            self.entry = Some(id);
        }
        self.stack.push(path.to_string());
        self.load_imports(id);
        self.stack.pop();
        Some(id)
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
        let pkg = ws.pkg(target.pkg);
        let disk = pkg.dir.join(rel);
        if !disk.is_file() {
            self.diags.push(
                Diagnostic::error(span, format!("{rel} does not exist"))
                    .with_fix("create the file, or remove it from `sources`")
                    .with_note("every source is declared one path at a time; there are no globs"),
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

    fn load_std(&mut self, path: &str, span: Span) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        let Some(text) = standard_library::source(path) else {
            self.diags.push(
                Diagnostic::error(span, format!("there is no module \"{path}\"")).with_code("no-such-module")
                    .with_fix("check the path; the standard library's modules are all `core/...`"),
            );
            return None;
        };
        let file = self.map.add(path.to_string(), PathBuf::new(), text.to_string());
        let parsed = crate::parsing::parser::parse_stdlib(self.map.text(file), file);
        self.diags.extend(parsed.errors);
        let role = if standard_library::is_platform_module(path) { Role::Platform } else { Role::Std };
        let id = ModuleId(self.modules.len() as u32);
        self.by_path.insert(path.to_string(), id);
        self.modules.push(ModuleData {
            id,
            path: path.to_string(),
            file,
            role,
            ast: parsed.module,
            pkg: None,
            disk: PathBuf::new(),
        });
        self.load_imports(id);
        Some(id)
    }

    /// Loads by module path, resolving through the workspace.
    fn load_path(&mut self, path: &str, role: Role, span: Span) -> Option<ModuleId> {
        if let Some(id) = self.by_path.get(path) {
            return Some(*id);
        }
        if path.starts_with("core/") {
            return self.load_std(path, span);
        }
        let Some(ws) = self.ws else {
            self.diags.push(
                Diagnostic::error(span, format!("\"{path}\" is outside any repository")).with_code("module-outside-repository")
                    .with_fix("import from `\"core/...\"` or from a `//...` path in this repository"),
            );
            return None;
        };
        match ws.resolve_module(path) {
            Ok(loc) => self.load_file(path, loc.file, role, span),
            Err(msg) => {
                self.diags.push(Diagnostic::error(span, msg).with_code("module-not-found").with_fix(
                    "create the file the path names, or correct the path — a module path maps \
                     to exactly one file, with no search",
                ));
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
            let cycle = self.stack[at..].join(" -> ");
            self.diags.push(
                Diagnostic::error(span, format!("circular import: {cycle} -> {path}")).with_code("circular-import")
                    .with_fix("break the cycle: move what both modules need into a third one")
                    .with_note("modules form a graph with no cycles, at the module level and at the package level alike"),
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
        let parsed = crate::parsing::parser::parse(self.map.text(file), file);
        self.diags.extend(parsed.errors);

        let pkg = self.ws.and_then(|ws| ws.owning_package(&disk));
        let id = ModuleId(self.modules.len() as u32);
        self.by_path.insert(path.to_string(), id);
        self.modules.push(ModuleData {
            id,
            path: path.to_string(),
            file,
            role,
            ast: parsed.module,
            pkg,
            disk,
        });

        self.stack.push(path.to_string());
        self.load_imports(id);
        self.stack.pop();
        Some(id)
    }

    /// Loads everything a module imports, checking each import line against
    /// the rules that govern where a path may be named from.
    fn load_imports(&mut self, id: ModuleId) {
        let imports: Vec<(String, Span)> = self.modules[id.index()]
            .ast
            .items
            .iter()
            .filter_map(|item| match item {
                tree::Item::Import(i) => Some((i.path.clone(), i.path_span)),
                tree::Item::ReExport(r) => Some((r.path.clone(), r.path_span)),
                _ => None,
            })
            .collect();

        let role = self.modules[id.index()].role;
        let importer_path = self.modules[id.index()].path.clone();
        let importer_pkg = self.modules[id.index()].pkg;

        for (path, span) in imports {
            if !self.check_import_legality(&importer_path, importer_pkg, role, &path, span) {
                continue;
            }
            // What a module imports is loaded in the role its own path
            // implies, not in the importer's role.
            let target_role = self.role_for(&path);
            self.load_path(&path, target_role, span);
        }
    }

    fn role_for(&self, path: &str) -> Role {
        if path.starts_with("core/") {
            return if standard_library::is_platform_module(path) { Role::Platform } else { Role::Std };
        }
        if is_test_only_path(path) {
            return Role::TestOnly;
        }
        Role::Source
    }

    /// The import restrictions. Each one is visible in the import line, which
    /// is where the person writing it is looking.
    fn check_import_legality(
        &mut self,
        importer_path: &str,
        importer_pkg: Option<crate::build::workspace::PkgId>,
        role: Role,
        path: &str,
        span: Span,
    ) -> bool {
        if path.starts_with('.') {
            self.diags.push(
                Diagnostic::error(span, format!("\"{path}\" is a relative module path")).with_code("relative-import")
                    .with_note(
                        "every module path is absolute, so a path means the same module wherever \
                         it is written and a file can move without its imports changing",
                    )
                    .with_fix(
                        "write the absolute path: `\"core/...\"` for the standard library, \
                         `\"//...\"` for this repository",
                    ),
            );
            return false;
        }

        // `core/host` is importable only from the module that exports `main`.
        if path == "core/host" && role != Role::Entry {
            self.diags.push(
                Diagnostic::error(span, "\"core/host\" is importable only from the module that exports `main`").with_code("host-import")
                    .with_fix(
                        "take what you need as a `ctx` bound instead, and let `main` supply the \
                         implementation",
                    )
                    .with_note(
                        "the context `main` builds is the program's complete effect budget; a \
                         module that could import `core/host` would be a second place authority \
                         enters",
                    ),
            );
            return false;
        }

        // A path containing a `testing` segment is importable only from a test
        // source — or from another test-only module.
        if is_test_only_path(path) && !role.is_test_context() {
            self.diags.push(
                Diagnostic::error(span, "this is a test-only module").with_code("test-only-import")
                    .with_label("importable only from a test source")
                    .with_note(
                        "a path containing a `testing` segment may be imported only from a test \
                         source",
                    )
                    .with_note(format!("{importer_path} is not one"))
                    .with_fix(
                        "import it from a file listed in a target's `test.sources`, or drop the \
                         import",
                    ),
            );
            return false;
        }

        let Some(ws) = self.ws else { return true };
        if !path.starts_with("//") {
            return true;
        }

        let Ok(loc) = ws.resolve_module(path) else {
            // `load_path` reports the resolution failure itself.
            return true;
        };

        match loc.kind {
            // A `//pkg/inner` import resolves only inside `//pkg`.
            ModuleKind::Internal => {
                if loc.pkg != importer_pkg {
                    let owner = ws.pkg(loc.pkg.unwrap()).label();
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("{path} is internal to {owner}"),
                        ).with_code("internal-import")
                        .with_fix(format!("import the library instead: from \"{owner}\" import {{ ... }}"))
                        .with_note(format!(
                            "only names re-exported by {}/lib.buri are available",
                            owner.trim_start_matches("//")
                        )),
                    );
                    return false;
                }
            }
            // A binary's entry point is importable only from that binary's own
            // test sources.
            ModuleKind::BinaryEntry => {
                let same_package = loc.pkg == importer_pkg;
                if !same_package || !role.is_test_context() {
                    self.diags.push(
                        Diagnostic::error(span, format!("{path} is a binary's entry point")).with_code("binary-entry-import")
                            .with_fix(
                                "move what you need into a library both can depend on",
                            )
                            .with_note(
                                "only that binary's own test sources may import it; a library may \
                                 not reach the binary in its package at all",
                            ),
                    );
                    return false;
                }
            }
            _ => {}
        }
        true
    }
}
