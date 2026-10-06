//! The build graph: repository root, packages, targets, labels, module
//! resolution, visibility, tags, and platforms.
//!
//! Five rules produce the shape of a repository, and everything here follows
//! from them (crates/docs/src/docs/reference/build/overview.md):
//!
//! 1. A directory with a `BUILD.buri` is a package.
//! 2. `lib.buri` is a library's whole public surface.
//! 3. `main.buri` is a compilation entry point.
//! 4. Tests live in `test/` and see only the target's surface.
//! 5. Everything is declared — a file on disk that no rule lists is an error.

use crate::build::buildfile::{
    self, Backend, OutputPlatform, Platform, PlatformRef, RepoConfig, Spanned,
};
use crate::diagnostics::{Diagnostic, Diagnostics, Invariant as _, Span};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

// The graph's vocabulary — packages, targets, labels, visibility, module
// locations — is `buri-project`'s, because the checker reads it too.
pub use buri_project::build::workspace::*;

pub struct Workspace {
    pub root: PathBuf,
    pub repo: RepoConfig,
    pub packages: Vec<Package>,
    /// What this repository's generators produced.
    ///
    /// Empty when the graph is loaded and filled by
    /// [`crate::build::generators::prepare`], because generating needs to build
    /// and spawn a tool and loading the graph cannot. It hangs here because the
    /// workspace is the one thing already threaded to the compiler's loader, so
    /// every command and the language server read one answer.
    pub generated: crate::build::generators::Store,
    by_path: HashMap<String, PackageId>,
    /// `actions::graph_key` per mode, worked out once: it is the build files'
    /// bytes, and a workspace is loaded anew whenever one of them moves.
    pub graph_keys: std::sync::Mutex<Vec<(crate::commands::arguments::BuildMode, crate::build::cache::ActionKey)>>,
}

impl Packages for Workspace {
    fn package(&self, id: PackageId) -> &Package {
        Workspace::package(self, id)
    }

    fn resolve_module(&self, path: &str) -> Result<ModuleLocation, String> {
        Workspace::resolve_module(self, path)
    }

    fn declared_entries(&self, target: TargetId) -> Vec<DeclaredEntry> {
        Workspace::declared_entries(self, target)
    }
}

/// A `tool` rule may only be declared in `tools/` or a package below it.
fn is_tool_directory(package_path: &str) -> bool {
    package_path.split('/').next() == Some("tools")
}

/// Where a misplaced tool package belongs. A package under the old `tool/`
/// keeps its path below it, so `//tool/db/seed` moves to `//tools/db/seed`.
fn tool_destination(package_path: &str, name: &str) -> String {
    match package_path.strip_prefix("tool/") {
        Some(rest) => format!("//tools/{rest}"),
        None => format!("//tools/{name}"),
    }
}

/// A `platform` rule may only be declared below `platform/`, and never in
/// `platform/effect/`, where effects live.
fn is_platform_directory(package_path: &str) -> bool {
    let mut segments = package_path.split('/');
    segments.next() == Some("platform") && segments.next().is_some_and(|s| s != "effect")
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Walks up from `start` looking for the `REPO.buri` whose presence makes a
/// directory a repository root.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() { start.to_path_buf() } else { start.parent()?.to_path_buf() };
    loop {
        if dir.join("REPO.buri").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

impl Workspace {
    pub fn load(
        root: &Path,
        map: &mut crate::diagnostics::SourceMap,
        diagnostics: &mut Diagnostics,
    ) -> std::io::Result<Workspace> {
        let repo_path = root.join("REPO.buri");
        let repo_id = map.load("REPO.buri", &repo_path)?;
        let read = buildfile::read_repo_config(map.text(repo_id), repo_id);
        diagnostics.extend(read.errors);
        let repo = read.value;

        let mut dirs = Vec::new();
        collect_packages(root, root, &mut dirs);
        dirs.sort();

        let mut packages = Vec::new();
        // Platform rules the reader refused something in. Their outputs aren't
        // checked against them, so the rule's error isn't followed by one per
        // output that names an entry the rule lost.
        let mut refused: Vec<String> = Vec::new();
        for path in dirs {
            let dir = if path.is_empty() { root.to_path_buf() } else { root.join(&path) };
            let build_path = dir.join("BUILD.buri");
            let rel = if path.is_empty() {
                "BUILD.buri".to_string()
            } else {
                format!("{path}/BUILD.buri")
            };
            let id = map.load(&rel, &build_path)?;
            let read = buildfile::read_build_file(map.text(id), id);
            if read.value.platform.is_some() && !read.errors.is_empty() {
                refused.push(path.clone());
            }
            diagnostics.extend(read.errors);
            let name = path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("<name>");
            let misplaced = [
                ("tool", read.value.tool.is_some() && !is_tool_directory(&path), "outside //tools/", tool_destination(&path, name)),
                (
                    "platform",
                    read.value.platform.is_some() && !is_platform_directory(&path),
                    if path == "platform/effect" || path.starts_with("platform/effect/") {
                        "inside //platform/effect/, which holds effect packages"
                    } else {
                        "outside //platform/"
                    },
                    format!("//platform/{}", if name == "effect" { "<name>" } else { name }),
                ),
            ];
            for (rule, wrong, directory, destination) in misplaced {
                if !wrong {
                    continue;
                }
                let span = read.document.as_message().get(rule).map_or(Span::point(id, 0), |f| f.name_span);
                diagnostics.push(
                    Diagnostic::templated("misplaced-rule", span)
                        .with_bind("package", format!("//{path}"))
                        .with_bind("rule", rule)
                        .with_bind("place", directory)
                        .with_bind("destination", destination),
                );
            }
            if read.value.library.is_none()
                && read.value.binary.is_none()
                && read.value.tool.is_none()
                && read.value.platform.is_none()
            {
                diagnostics.push(
                    Diagnostic::templated("package-missing-rule", Span::point(id, 0))
                        .with_bind("package_path", path.clone()),
                );
            }
            packages.push(Package {
                path,
                dir,
                build_path,
                build_file_id: id,
                build: read.value,
                document: read.document,
            });
        }

        let by_path: HashMap<String, PackageId> = packages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.path.clone(), PackageId(i as u32)))
            .collect();
        resolve_custom_outputs(&mut packages, &by_path, &refused, diagnostics);
        check_artifact_paths(root, &packages, diagnostics);

        let workspace =
            Workspace {
                root: root.to_path_buf(),
                repo,
                packages,
                by_path,
                generated: crate::build::generators::Store::default(),
                graph_keys: std::sync::Mutex::new(Vec::new()),
            };
        // Only once the build file it names has been read can a tool name be
        // resolved, so the references are checked here rather than by the
        // reader.
        diagnostics.extend(crate::build::tools::validate(&workspace));
        diagnostics.extend(check_platform_labels(&workspace));
        Ok(workspace)
    }

    pub fn package(&self, id: PackageId) -> &Package {
        self.packages
            .get(id.0 as usize)
            .or_ice("every PackageId is an index this table minted while loading the repository")
    }

    pub fn package_by_path(&self, path: &str) -> Option<PackageId> {
        self.by_path.get(path).copied()
    }

    pub fn ids(&self) -> impl Iterator<Item = PackageId> {
        (0..self.packages.len() as u32).map(PackageId)
    }

    // -- targets ------------------------------------------------------------

    pub fn targets(&self) -> Vec<TargetId> {
        let mut out = Vec::new();
        for id in self.ids() {
            let p = self.package(id);
            if p.has_library() {
                out.push(TargetId { package: id, kind: RuleKind::Library });
            }
            if p.has_binary() {
                out.push(TargetId { package: id, kind: RuleKind::Binary });
            }
            if p.has_tool() {
                out.push(TargetId { package: id, kind: RuleKind::Tool });
            }
        }
        out
    }

    pub fn label(&self, target: TargetId) -> String {
        self.package(target.package).label()
    }

    /// The declared dependencies of a target, as labels with spans.
    pub fn declared_deps(&self, target: TargetId) -> &[Spanned<String>] {
        let p = self.package(target.package);
        match target.kind {
            RuleKind::Library => p.build.library.as_ref().map(|l| &l.dependencies[..]).unwrap_or(&[]),
            RuleKind::Binary => p.build.binary.as_ref().map(|b| &b.dependencies[..]).unwrap_or(&[]),
            RuleKind::Tool => p.build.tool.as_ref().map(|t| &t.dependencies[..]).unwrap_or(&[]),
        }
    }

    /// A tool carries no tags: nothing is built from it, so there is nothing
    /// a policy could forbid it from reaching.
    pub fn tags(&self, target: TargetId) -> &[Spanned<String>] {
        let p = self.package(target.package);
        match target.kind {
            RuleKind::Library => p.build.library.as_ref().map(|l| &l.tags[..]).unwrap_or(&[]),
            RuleKind::Binary => p.build.binary.as_ref().map(|b| &b.tags[..]).unwrap_or(&[]),
            RuleKind::Tool => &[],
        }
    }

    /// Resolved dependency edges: (dependency library target, the label span).
    /// A binary and a tool additionally depend on the library in their own
    /// package, which is implicit and carries no span.
    pub fn dep_edges(&self, target: TargetId) -> Vec<(TargetId, Option<Span>)> {
        let mut out = Vec::new();
        if target.kind != RuleKind::Library && self.package(target.package).has_library() {
            out.push((TargetId { package: target.package, kind: RuleKind::Library }, None));
        }
        for dep in self.declared_deps(target) {
            if let Some(id) = self.dep_target(&dep.value) {
                out.push((id, Some(dep.span)));
            }
        }
        out
    }

    /// The edges a target's *test* code adds: `test.dependencies` on either
    /// rule, and `testing.dependencies` on a library.
    ///
    /// These are deliberately not part of [`Self::dep_edges`]. A test
    /// dependency is not a dependency of the thing being shipped, so it must
    /// not enter [`Self::closure`] — it would otherwise drag its tags into the
    /// production tag closure and make a cycle out of a suite that merely
    /// borrows a helper. What it *is* subject to is
    /// visibility: BUILD-FILES.md:359-360 exempts only a suite reaching the
    /// target under test, and says everything else "including a test suite
    /// reaching a library named in `test.dependencies`, is checked normally".
    pub fn test_dep_edges(&self, target: TargetId) -> Vec<(TargetId, Option<Span>)> {
        let p = self.package(target.package);
        let mut declared: Vec<&Spanned<String>> = Vec::new();
        match target.kind {
            RuleKind::Library => {
                if let Some(l) = &p.build.library {
                    declared.extend(l.test.iter().flat_map(|t| t.dependencies.iter()));
                    declared.extend(l.testing.iter().flat_map(|t| t.dependencies.iter()));
                }
            }
            RuleKind::Binary => {
                if let Some(b) = &p.build.binary {
                    declared.extend(b.test.iter().flat_map(|t| t.dependencies.iter()));
                }
            }
            RuleKind::Tool => {
                if let Some(t) = &p.build.tool {
                    declared.extend(t.test.iter().flat_map(|t| t.dependencies.iter()));
                }
            }
        }
        let mut out = Vec::new();
        for dep in declared {
            if let Some(id) = self.dep_target(&dep.value) {
                out.push((id, Some(dep.span)));
            }
        }
        out
    }

    /// A label in a `dependencies` list always means the library of that
    /// package, because a library is the only thing that can be depended on.
    /// `//lib/ledger/testing` names the testing surface, which lives in the
    /// same package's library rule.
    pub fn dep_target(&self, label: &str) -> Option<TargetId> {
        let path = label.strip_prefix("//")?;
        if let Some(id) = self.package_by_path(path) {
            if self.package(id).has_library() {
                return Some(TargetId { package: id, kind: RuleKind::Library });
            }
            return None;
        }
        // `//lib/ledger/testing` -> the library rule of //lib/ledger.
        let owner = path.strip_suffix("/testing")?;
        let id = self.package_by_path(owner)?;
        self.package(id)
            .build
            .library
            .as_ref()
            .filter(|l| l.testing.is_some())
            .map(|_| TargetId { package: id, kind: RuleKind::Library })
    }

    /// Every source file this package's rules declare, package-relative.
    ///
    /// The two entry points lead, and neither is written in a `sources` list:
    /// `lib.buri` and `main.buri` are named by the rule kind, exactly as
    /// [`Self::rule_of_file`] answers for them before it reads any list. A name
    /// here is what the build file says, so one whose file is missing is on
    /// this list too — the caller decides what a declared source that is not
    /// there means.
    ///
    /// This is the file set behind a label, which is what lets `buri format`
    /// take one: a label names packages, and a package's files are these.
    pub fn declared_sources(&self, package: PackageId) -> Vec<String> {
        let p = self.package(package);
        let mut out: Vec<String> = Vec::new();
        if let Some(l) = &p.build.library {
            out.push("lib.buri".into());
            out.extend(l.sources.iter().map(|s| s.value.clone()));
            out.extend(l.test.iter().flat_map(|t| t.sources.iter()).map(|s| s.value.clone()));
            if let Some(testing) = &l.testing {
                out.push("testing/lib.buri".into());
                out.extend(testing.sources.iter().map(|s| s.value.clone()));
            }
        }
        if let Some(b) = &p.build.binary {
            out.push("main.buri".into());
            out.extend(b.sources.iter().map(|s| s.value.clone()));
            out.extend(b.test.iter().flat_map(|t| t.sources.iter()).map(|s| s.value.clone()));
        }
        if let Some(t) = &p.build.tool {
            out.push("tool.buri".into());
            out.extend(t.sources.iter().map(|s| s.value.clone()));
            out.extend(t.test.iter().flat_map(|t| t.sources.iter()).map(|s| s.value.clone()));
        }
        out
    }

    /// Which rule a file in a package belongs to, by its package-relative
    /// path.
    ///
    /// Every file belongs to exactly one rule — the `sources` sets are
    /// disjoint (BUILD-FILES.md:299) — and in a package holding both rules
    /// that is what the library boundary is drawn around. The boundary is a
    /// property of the *rule*, not of the directory, so "is the importer
    /// inside the package" is the wrong question and this is the right one.
    ///
    /// The entry points answer first, because they are named by the rule kind
    /// rather than listed. A file no rule reaches is `None`, which is
    /// `unused-source`'s business rather than this function's.
    pub fn rule_of_file(&self, package: PackageId, rel: &str) -> Option<RuleKind> {
        let p = self.package(package);
        match rel {
            "lib.buri" | "testing/lib.buri" if p.has_library() => return Some(RuleKind::Library),
            "main.buri" if p.has_binary() => return Some(RuleKind::Binary),
            "tool.buri" if p.has_tool() => return Some(RuleKind::Tool),
            _ => {}
        }
        if let Some(t) = &p.build.tool {
            if t.sources.iter().chain(t.test.iter().flat_map(|t| t.sources.iter())).any(|s| s.value == rel)
            {
                return Some(RuleKind::Tool);
            }
        }
        if let Some(l) = &p.build.library {
            let listed = l
                .sources
                .iter()
                .chain(l.test.iter().flat_map(|t| t.sources.iter()))
                .chain(l.testing.iter().flat_map(|t| t.sources.iter()));
            if listed.into_iter().any(|s| s.value == rel) {
                return Some(RuleKind::Library);
            }
        }
        if let Some(b) = &p.build.binary {
            if b.sources
                .iter()
                .chain(b.test.iter().flat_map(|t| t.sources.iter()))
                .any(|s| s.value == rel)
            {
                return Some(RuleKind::Binary);
            }
        }
        None
    }

    /// Everything reachable from `target` through `dependencies`, including it.
    pub fn closure(&self, target: TargetId) -> Vec<TargetId> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![target];
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur) {
                continue;
            }
            for (dep, _) in self.dep_edges(cur) {
                stack.push(dep);
            }
        }
        seen.into_iter().collect()
    }

    /// The shortest dependency path from `from` to `to`, if there is one, with
    /// the span of the edge that introduced each step.
    pub fn dep_path(&self, from: TargetId, to: TargetId) -> Option<Vec<(TargetId, Option<Span>)>> {
        let mut prev: HashMap<TargetId, (TargetId, Option<Span>)> = HashMap::new();
        let mut queue = std::collections::VecDeque::from([from]);
        let mut seen = BTreeSet::from([from]);
        while let Some(cur) = queue.pop_front() {
            if cur == to {
                let mut path = vec![(cur, None)];
                let mut node = cur;
                while let Some((p, span)) = prev.get(&node).copied() {
                    if let Some(last) = path.last_mut() {
                        last.1 = span;
                    }
                    path.push((p, None));
                    node = p;
                }
                path.reverse();
                return Some(path);
            }
            for (dep, span) in self.dep_edges(cur) {
                if seen.insert(dep) {
                    prev.insert(dep, (cur, span));
                    queue.push_back(dep);
                }
            }
        }
        None
    }

    // -- module resolution --------------------------------------------------

    /// The dependency label an import path names, seen from `own`.
    ///
    /// `None` four ways, all of them meaning "not a cross-package dependency":
    /// a relative path, a path that resolves to nothing, one that lands
    /// outside a package, and one that lands back in `own`. `lint`, `gen` and
    /// `gen`'s on-disk import scan each walked these same steps, and a
    /// `/testing` suffix decided in three places is one place for the rule to
    /// be forgotten.
    pub fn dependency_label(&self, own: PackageId, path: &str) -> Option<String> {
        if !path.starts_with("//") {
            return None;
        }
        let Ok(ModuleLocation::InPackage(loc)) = self.resolve_module(path) else {
            return None;
        };
        if loc.package == own {
            return None;
        }
        // A platform is no library: an output that names it depends on it,
        // and nothing writes it in `dependencies`.
        if loc.kind == ModuleKind::PlatformSurface {
            return None;
        }
        let label = self.package(loc.package).label();
        Some(if is_test_only_path(path) { format!("{label}/testing") } else { label })
    }

    /// Resolves a module path written in an import.
    ///
    /// Two forms arrive here and both are legal; which one a writer may use is
    /// decided by where they are writing *from*, and that is
    /// `check_import_legality`'s question rather than this one's:
    ///
    /// | written | means |
    /// |---|---|
    /// | `//lib/money` | the module `lib/money` — its `lib.buri` |
    /// | `//lib/money/testing` | that library's testing surface |
    /// | `//lib/money/lib.buri` | the same file, named the long way round |
    /// | `//lib/money/cents.buri` | one file inside that module |
    /// | `//proto/address.proto` | a schema, unchanged |
    ///
    /// Whichever was written, [`PackageModule::path`] comes back canonical —
    /// `//` plus the repository-relative file name — so a module has one
    /// identity however it was reached.
    pub fn resolve_module(&self, path: &str) -> Result<ModuleLocation, String> {
        if path.starts_with('.') {
            return Err(format!(
                "\"{path}\" is a relative path; every module path is absolute, so a file can \
                 move between directories without its imports changing"
            ));
        }
        if crate::compiler::standard_library::is_std_path(path) {
            // Canonical here too, and for the same reason: `platform/effect` and
            // `platform/effect/lib.buri` are one module or they are two copies of
            // `Allocator`. A path the library does not have keeps its spelling, so
            // that `unknown-module` quotes back what was written.
            let canonical = crate::compiler::standard_library::canonical(path).unwrap_or(path);
            return Ok(ModuleLocation::Std { path: canonical.to_string() });
        }
        if path == "core" {
            return Err("\"core\" is not a module; name one, as in \"core/list\"".into());
        }
        if path == "ui" {
            return Err("\"ui\" is not a module; name one, as in \"ui/signal\"".into());
        }
        let Some(rest) = path.strip_prefix("//") else {
            return Err(format!(
                "\"{path}\" is not a module path; the forms are {} and \"//...\"",
                crate::compiler::standard_library::ROOTS
                    .iter()
                    .map(|r| format!("\"{r}...\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };

        // Longest package prefix wins.
        if let Some((package_path, remainder, id)) = self.owning_prefix(rest) {
            let package = self.package(id);
            // A module a generator produced has no file, so it is answered
            // before anything asks the disk about one. Its name is whatever the
            // generator called it, which is why this is a lookup rather than a
            // rule about the spelling.
            let generated = package.module_path(remainder);
            if !remainder.is_empty() && self.generated.holds(&generated) {
                return Ok(ModuleLocation::InPackage(PackageModule {
                    path: generated,
                    kind: ModuleKind::Generated,
                    package: id,
                    // The path the module *would* have, so a reader that wants
                    // somewhere to point has somewhere. Nothing is there, and
                    // every reader that opens a file checks first.
                    file: package.dir.join(remainder),
                    rel: self.rel_of(&package.dir.join(remainder)),
                }));
            }
            // What is left of the path after the package's own name is either
            // a file inside that package, letter for letter, or nothing at all
            // — and nothing at all is the module form, which names the
            // package's surface. The three names a rule knows by kind rather
            // than by listing — `lib.buri`, `testing/lib.buri`, `main.buri` —
            // are what decide which kind of module a file is, and `testing`
            // and `main` are the extensionless spellings of two of them.
            // A repository platform's surface is its `platform.buri`, named by
            // the platform's label as a library is by its own.
            let platform = package.build.platform.is_some() && is_platform_directory(package_path);
            let (kind, file) = match remainder {
                "" | "platform.buri" if platform => {
                    (ModuleKind::PlatformSurface, package.dir.join("platform.buri"))
                }
                "" => (ModuleKind::LibrarySurface, package.dir.join("lib.buri")),
                "lib.buri" => (ModuleKind::LibrarySurface, package.dir.join("lib.buri")),
                "testing" | "testing/lib.buri" => {
                    (ModuleKind::TestingSurface, package.dir.join("testing/lib.buri"))
                }
                "main" | "main.buri" => (ModuleKind::BinaryEntry, package.dir.join("main.buri")),
                // A `.proto` names a schema, and a schema is a generator's
                // input rather than a module of its own. The only module one
                // produces is the one the `proto` tool handed back, which
                // the lookup above already answered — so reaching here means its
                // check failed, or no `generators` entry declares it, and the
                // sentence says which.
                r if r.ends_with(".proto") => {
                    let file = package.dir.join(r);
                    let listed = self
                        .targets()
                        .into_iter()
                        .filter(|t| t.package == id)
                        .any(|t| crate::build::generators::inputs(self, t).iter().any(|i| i == r));
                    if listed && file.is_file() {
                        return Err(SCHEMA_HAS_ERRORS.to_string());
                    }
                    return Err(if file.is_file() {
                        format!(
                            "\"{path}\" names a schema, and no `generators` entry in {} hands it to a tool",
                            package.label()
                        )
                    } else {
                        format!("\"{path}\" names no file ({})", self.rel_of(&file))
                    });
                }
                r if r.ends_with(".buri") => (ModuleKind::Internal, package.dir.join(r)),
                // An extensionless inner path. Legal to *resolve* — it is how
                // a dependent used to name someone else's internals, and it is
                // an `internal-import` from there and an
                // `import-missing-extension` from inside — so both
                // diagnostics can name the file it meant.
                r => (ModuleKind::Internal, package.dir.join(format!("{r}.buri"))),
            };
            if !file.is_file() {
                return Err(format!("\"{path}\" names no file ({})", self.rel_of(&file)));
            }
            let rel = self.rel_of(&file);
            return Ok(ModuleLocation::InPackage(PackageModule {
                path: format!("//{rel}"),
                kind,
                package: id,
                file,
                rel,
            }));
        }
        Err(format!("\"{path}\" is in no package of this repository"))
    }

    /// The longest package path that is `rest`, a prefix of it ending before a
    /// `/`, or the root's, and what of `rest` follows it. Only those can
    /// contain `rest`, so each is one lookup rather than a scan of packages.
    fn owning_prefix<'r>(&self, rest: &'r str) -> Option<(&'r str, &'r str, PackageId)> {
        if let Some(&id) = self.by_path.get(rest) {
            return Some((rest, "", id));
        }
        let mut end = rest.len();
        while let Some(slash) = rest.get(..end).and_then(|r| r.rfind('/')) {
            let (package_path, tail) = rest.split_at_checked(slash)?;
            let remainder = tail.strip_prefix('/')?;
            if !package_path.is_empty() {
                if let Some(&id) = self.by_path.get(package_path) {
                    return Some((package_path, remainder, id));
                }
            }
            end = slash;
        }
        self.by_path.get("").map(|&id| ("", rest, id))
    }

    pub fn rel_of(&self, p: &Path) -> String {
        p.strip_prefix(&self.root).unwrap_or(p).display().to_string().replace('\\', "/")
    }

    /// The package a path on disk belongs to: the nearest ancestor with a
    /// `BUILD.buri`.
    pub fn owning_package(&self, p: &Path) -> Option<PackageId> {
        let rel = self.rel_of(p);
        let mut dir = rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        loop {
            if let Some(id) = self.package_by_path(&dir) {
                return Some(id);
            }
            match dir.rsplit_once('/') {
                Some((d, _)) => dir = d.to_string(),
                None => {
                    if dir.is_empty() {
                        return None;
                    }
                    dir = String::new();
                }
            }
        }
    }

    // -- visibility ---------------------------------------------------------

    /// Whether `from`'s package may depend on the library `to`. Two edges skip
    /// the check because neither is a dependency anyone chose: a target's own
    /// test suite reaching the target under test, and a binary reaching the
    /// library in its own package.
    pub fn visible(&self, from: PackageId, to: TargetId) -> bool {
        if from == to.package {
            return true;
        }
        let Some(lib) = &self.package(to.package).build.library else { return false };
        let from_path = &self.package(from).path;
        lib.visibility.iter().any(|v| v.value.allows(from_path))
    }

    pub fn visibility_list(&self, to: TargetId) -> String {
        match &self.package(to.package).build.library {
            Some(l) if !l.visibility.is_empty() => {
                l.visibility.iter().map(|v| v.value.spelling()).collect::<Vec<_>>().join(", ")
            }
            // A rule that omits `visibility` is `//visibility:private`. There
            // is no package default and no repository default.
            _ => "//visibility:private (nothing, outside its own package)".into(),
        }
    }

    // -- platforms ----------------------------------------------------------

    /// Every entry a binary's `outputs` name, in declaration order.
    ///
    /// This is what makes the entry checks per entry rather than per target. A
    /// binary with a page and a worker in it declares two entries, and the
    /// page's `host.ui` is checked against `WebHost` alone.
    ///
    /// Empty for a library, and for a binary that declares no `outputs`.
    pub fn declared_entries(&self, target: TargetId) -> Vec<DeclaredEntry> {
        if target.kind != RuleKind::Binary {
            return Vec::new();
        }
        let Some(bin) = self.package(target.package).build.binary.as_ref() else {
            return Vec::new();
        };
        bin.outputs
            .iter()
            .map(|o| DeclaredEntry {
                name: o.entry_name().to_string(),
                named: o.entry.is_some(),
                // The `entry` field where there is one, the output itself
                // otherwise: a caret on a field nobody wrote points at nothing.
                span: o.entry.as_ref().map_or(o.span, |e| e.span),
                platform: o.platform(),
                custom: o.custom.clone(),
            })
            .collect()
    }

    /// The repository platform a label names: its package and its rule.
    /// `None` for a label naming no `platform` rule under `//platform/`.
    pub fn platform_rule(&self, label: &str) -> Option<(PackageId, &buildfile::PlatformRule)> {
        let path = label.strip_prefix("//")?;
        if !is_platform_directory(path) {
            return None;
        }
        let id = self.package_by_path(path)?;
        Some((id, self.package(id).build.platform.as_ref()?))
    }

    /// Everything an output of `target` for `platform` is built from: the
    /// target's closure, and for a repository platform the closure of every
    /// library the platform depends on. What policy is checked over.
    pub fn policy_members(&self, target: TargetId, platform: &OutputPlatform) -> Vec<TargetId> {
        let mut members = self.closure(target);
        if let OutputPlatform::Repository { label, .. } = platform {
            if let Some((_, rule)) = self.platform_rule(label) {
                for dep in &rule.dependencies {
                    if let Some(t) = self.dep_target(&dep.value) {
                        members.extend(self.closure(t));
                    }
                }
            }
        }
        members.sort();
        members.dedup();
        members
    }

    /// The repository platforms a binary's outputs name, each once, in the
    /// order they are first named.
    pub fn custom_platforms(&self, target: TargetId) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in self.declared_entries(target) {
            if let Some(c) = e.custom {
                if !out.contains(&c.label.value) {
                    out.push(c.label.value);
                }
            }
        }
        out
    }

    /// Every platform an output here can be built for: each bundled one, and
    /// each repository platform with every backend its entries are built by.
    pub fn every_platform(&self) -> BTreeSet<OutputPlatform> {
        let mut out: BTreeSet<OutputPlatform> = Platform::ALL.into_iter().map(OutputPlatform::Bundled).collect();
        for p in &self.packages {
            let Some(rule) = p.build.platform.as_ref().filter(|_| is_platform_directory(&p.path)) else {
                continue;
            };
            for entry in &rule.entries {
                out.insert(OutputPlatform::Repository { label: format!("//{}", p.path), backend: entry.backend.value });
            }
        }
        out
    }

    /// The platforms a target can be built for: the intersection, over every
    /// target in its closure, of that target's `backends` and `platforms` and
    /// what every tag it carries admits — its `requires` (unset is "all")
    /// minus its `forbids`.
    pub fn platforms(&self, target: TargetId) -> BTreeSet<OutputPlatform> {
        self.platforms_of(&self.closure(target))
    }

    /// The same, over any set of targets.
    pub fn platforms_of(&self, members: &[TargetId]) -> BTreeSet<OutputPlatform> {
        let mut allowed = self.every_platform();
        for &member in members {
            if let Some(lib) = &self.package(member.package).build.library {
                if member.kind == RuleKind::Library {
                    allowed.retain(|p| lib.admits.admits(p));
                }
            }
            for tag in self.tags(member) {
                if let Some(decl) = self.repo.tag(&tag.value) {
                    allowed.retain(|p| decl.admits(p));
                }
            }
        }
        allowed
    }

    /// The platforms a target's suite runs on, one per backend its `test`
    /// block names: the host for `NATIVE`, and for `JS` the first bundled
    /// JavaScript platform the target's outputs name, or else admits. Empty
    /// when the block names none.
    pub fn suite_platforms(&self, target: TargetId) -> Vec<Platform> {
        let Some(suite) = self.package(target.package).test_suite(target.kind) else {
            return Vec::new();
        };
        let allowed = self.platforms(target);
        let admits = |p: &Platform| allowed.contains(&OutputPlatform::Bundled(*p));
        let declared: Vec<Platform> = match (target.kind, &self.package(target.package).build.binary) {
            (RuleKind::Binary, Some(bin)) => bin.outputs.iter().filter(|o| o.custom.is_none()).map(|o| o.platform()).collect(),
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        for backend in &suite.backends {
            let candidates = match backend.value {
                Backend::Native => vec![crate::compiler::driver::host_native_platform()],
                Backend::Js => Backend::Js.platforms().to_vec(),
            };
            let chosen = candidates
                .iter()
                .find(|p| declared.contains(p) && admits(p))
                .or_else(|| candidates.iter().find(|p| admits(p)))
                .or(candidates.first())
                .copied();
            if let Some(p) = chosen.filter(|p| !out.contains(p)) {
                out.push(p);
            }
        }
        out
    }

    /// What a suite run on `platform` is held to: that platform where the
    /// target admits it, or else a repository platform the target admits on
    /// the same backend. A suite is plain Buri, so a library written for one
    /// repository platform tests on that platform's backend.
    pub fn suite_output_platform(&self, target: TargetId, platform: Platform) -> OutputPlatform {
        let bundled = OutputPlatform::Bundled(platform);
        let allowed = self.platforms(target);
        if allowed.contains(&bundled) {
            return bundled;
        }
        allowed
            .into_iter()
            .find(|p| matches!(p, OutputPlatform::Repository { .. }) && p.backend() == platform.backend())
            .unwrap_or(bundled)
    }

    /// Explains why `platform` is not available to `target`: the member of the
    /// closure that rules it out, and how it was reached.
    pub fn platform_blocker(&self, target: TargetId, platform: &OutputPlatform) -> Option<PlatformBlocker> {
        self.platform_blocker_of(&self.closure(target), platform)
    }

    /// The same, over any set of targets.
    pub fn platform_blocker_of(&self, members: &[TargetId], platform: &OutputPlatform) -> Option<PlatformBlocker> {
        for &member in members {
            if let Some(lib) = &self.package(member.package).build.library {
                if member.kind == RuleKind::Library && !lib.admits.admits(platform) {
                    return Some(PlatformBlocker {
                        member,
                        why: format!("{} declares {}", self.label(member), lib.admits.phrase()),
                        forbidden: None,
                    });
                }
            }
            for tag in self.tags(member) {
                if let Some(decl) = self.repo.tag(&tag.value) {
                    if !decl.requires.admits(platform) {
                        return Some(PlatformBlocker {
                            member,
                            why: format!(
                                "{} is tagged \"{}\", which requires {}",
                                self.label(member),
                                tag.value,
                                decl.requires.phrase()
                            ),
                            forbidden: None,
                        });
                    }
                    if let Some(word) = decl.forbids.naming(platform) {
                        return Some(PlatformBlocker {
                            member,
                            why: format!(
                                "{} is tagged \"{}\", which forbids {word}",
                                self.label(member),
                                tag.value,
                            ),
                            forbidden: Some(word.to_string()),
                        });
                    }
                }
            }
        }
        None
    }

    // -- tags ---------------------------------------------------------------

    /// Every tag carried anywhere in a target's closure, with the target that
    /// carries it.
    pub fn closure_tags(&self, target: TargetId) -> BTreeMap<String, TargetId> {
        self.tags_of(&self.closure(target))
    }

    /// Every tag carried by a set of targets, with the target that carries it.
    pub fn tags_of(&self, members: &[TargetId]) -> BTreeMap<String, TargetId> {
        let mut out = BTreeMap::new();
        for &member in members {
            for tag in self.tags(member) {
                out.entry(tag.value.clone()).or_insert(member);
            }
        }
        out
    }

    /// Two tags that forbid each other may not appear anywhere in the same
    /// dependency closure. `forbids` is symmetric, and the check is a union
    /// over the closure rather than a path — a binary that pulls client-only
    /// code down one dependency and server-only code down another is an error
    /// even though neither reaches the other. `members` is the closure.
    pub fn forbidden_pair_of(&self, members: &[TargetId]) -> Option<(String, TargetId, String, TargetId)> {
        let carried = self.tags_of(members);
        for (a, a_by) in &carried {
            for (b, b_by) in &carried {
                if a >= b {
                    continue;
                }
                let forbids = self
                    .repo
                    .tag(a)
                    .is_some_and(|d| d.forbids_tags.iter().any(|f| &f.value == b))
                    || self
                        .repo
                        .tag(b)
                        .is_some_and(|d| d.forbids_tags.iter().any(|f| &f.value == a));
                if forbids {
                    return Some((a.clone(), *a_by, b.clone(), *b_by));
                }
            }
        }
        None
    }

    pub fn tag_doc(&self, name: &str) -> String {
        self.repo.tag(name).map(|t| t.doc.clone()).unwrap_or_default()
    }
}

/// Checks every output that names a repository platform against that
/// platform's rule, and turns it into one output per entry the platform has.
///
/// The reader keeps a `//platform/<name>` output as written, because the rule
/// it names is in another build file. Here every build file has been read, so
/// the label, the `variant` and the `entries` are held to the rule, through
/// the checks a bundled platform's output gets from the reader. An
/// output that fails is dropped, so nothing downstream builds it.
fn resolve_custom_outputs(
    packages: &mut [Package],
    by_path: &HashMap<String, PackageId>,
    refused: &[String],
    diagnostics: &mut Diagnostics,
) {
    use buildfile::{NativePlatform, OutputTarget};
    // The rules, by package path, read before any output is rewritten.
    let rules: HashMap<String, buildfile::PlatformRule> = packages
        .iter()
        .filter_map(|p| Some((p.path.clone(), p.build.platform.clone()?)))
        .collect();
    for package in packages.iter_mut() {
        let Some(binary) = package.build.binary.as_mut() else { continue };
        let mut resolved = Vec::new();
        for output in std::mem::take(&mut binary.outputs) {
            let Some(custom) = output.custom.clone() else {
                resolved.push(output);
                continue;
            };
            let path = custom.package_path().to_string();
            if refused.contains(&path) {
                continue;
            }
            let rule = by_path.get(&path).and_then(|_| rules.get(&path));
            let Some(rule) = rule.filter(|_| is_platform_directory(&path)) else {
                let platforms: Vec<&str> = rules.keys().map(String::as_str).collect();
                diagnostics.push(no_such_platform(&custom.label, &platforms));
                continue;
            };
            let label = custom.label.value.as_str();
            let variant = custom.variant.as_ref();
            if let Some(d) =
                buildfile::check_variant(label, &rule.variants(), rule.variant_required(), variant, output.span)
            {
                diagnostics.push(d);
                continue;
            }
            if let Some(d) = buildfile::check_entry_variants(label, rule, variant) {
                diagnostics.push(d);
                continue;
            }
            if let Some(d) = output.artifact_name.as_ref().and_then(|n| buildfile::check_artifact_name(label, rule, n)) {
                diagnostics.push(d);
                continue;
            }
            let (filled, errors) = buildfile::check_entries(label, &rule.entry_names(), &custom.entries);
            if !errors.is_empty() {
                for d in errors {
                    diagnostics.push(d);
                }
                continue;
            }
            // One output per entry: each entry is its own artifact, named after
            // the entry, in the output's one directory.
            for entry in &rule.entries {
                let mut one = output.clone();
                let function = filled.iter().find(|(k, _)| k.value == entry.name.value);
                one.entry = function.map(|(_, f)| f.clone());
                one.target = match entry.backend.value {
                    Backend::Js => OutputTarget::Js,
                    // The reader held a native entry's variants to
                    // `<os>-<arch>`, and the checks above held the output's
                    // variant to the entry's, so a written one parses.
                    Backend::Native => match custom.variant.as_ref().and_then(|v| {
                        buildfile::native_variant(&v.value).map(|(os, arch)| (os, Spanned::new(arch, v.span)))
                    }) {
                        Some((os, arch)) => OutputTarget::Native { platform: os, arch: Some(arch) },
                        None => {
                            let os = match crate::compiler::driver::host_native_platform() {
                                Platform::Macos => NativePlatform::Macos,
                                _ => NativePlatform::Linux,
                            };
                            OutputTarget::Native { platform: os, arch: None }
                        }
                    },
                };
                if let Some(c) = one.custom.as_mut() {
                    c.point = entry.name.value.clone();
                    c.backend = entry.backend.value;
                    c.js = entry.js.as_ref().map(|j| j.value.clone());
                }
                resolved.push(one);
            }
        }
        binary.outputs = resolved;
    }
}

/// `duplicate-artifact-path` for two outputs of one binary that land at one
/// path, where the second would overwrite the first.
fn check_artifact_paths(root: &Path, packages: &[Package], diagnostics: &mut Diagnostics) {
    for package in packages {
        let Some(binary) = package.build.binary.as_ref() else { continue };
        let mut seen: Vec<(PathBuf, &buildfile::Output)> = Vec::new();
        // One written output repeats per entry; it is told about once.
        let mut said: Vec<Span> = Vec::new();
        for output in &binary.outputs {
            let path = crate::build::actions::artifact_relative(root, &package.path, output);
            let Some((_, first)) = seen.iter().find(|(p, _)| *p == path) else {
                seen.push((path, output));
                continue;
            };
            if said.contains(&output.span) {
                continue;
            }
            said.push(output.span);
            let platform = output.platform_label();
            let (span, note, fix) = match (&output.artifact_name, first.span == output.span) {
                (Some(name), true) => (
                    name.span,
                    format!("`{platform}` has several entries, and `artifact_name` names each of them `{}`", name.value),
                    String::from("remove `artifact_name`: each entry's artifact is named after the entry"),
                ),
                _ if output.artifact_name.is_none() && first.artifact_name.is_none() && ships_assets(packages, output) => (
                    output.span,
                    format!("both outputs are `{platform}` pages, and a page's file is named after its entry"),
                    format!("drop one of the two `{platform}` outputs"),
                ),
                _ => (
                    output.span,
                    format!("both outputs are `{platform}`, in one directory, under one name"),
                    String::from("give one of them a different `artifact_name`, or drop it"),
                ),
            };
            let shown = path.display().to_string().replace('\\', "/");
            diagnostics.push(
                Diagnostic::templated("duplicate-artifact-path", span)
                    .with_bind("target", package.label())
                    .with_bind("path", shown)
                    .with_bind("note", note)
                    .with_bind("fix", fix),
            );
        }
    }
}

/// Whether an output's platform ships assets, which name its artifacts.
fn ships_assets(packages: &[Package], output: &buildfile::Output) -> bool {
    let rule = match &output.custom {
        Some(custom) => packages.iter().find(|p| p.path == custom.package_path()).and_then(|p| p.build.platform.as_ref()),
        None => crate::build::platforms::bundled(output.platform().slug()),
    };
    rule.is_some_and(|r| !r.assets.is_empty())
}

/// `unknown-platform` for a `//` label that names no `platform` rule under
/// `//platform/`. `rules` are the package paths holding a `platform` rule.
fn no_such_platform(label: &Spanned<String>, rules: &[&str]) -> Diagnostic {
    let path = label.value.strip_prefix("//").unwrap_or(&label.value);
    let mut names: Vec<String> =
        buildfile::PlatformName::BUNDLED.iter().map(|p| format!("\"{}\"", p.name())).collect();
    names.extend(rules.iter().filter(|k| is_platform_directory(k)).map(|k| format!("\"//{k}\"")));
    names.sort();
    Diagnostic::templated("unknown-platform", label.span)
        .with_bind("platform", label.value.as_str())
        .with_note(format!(
            "a repository's platform is a `platform` rule in a package under `//platform/`, and \
             `//{path}` holds none"
        ))
        .with_fix(format!("name one of {}", names.join(", ")))
}

/// Every `//` label a library's `platforms` list or a tag's `requires` and
/// `forbids` name, held to the repository's `platform` rules. The reader
/// can't see another build file, so this runs once every one is read.
fn check_platform_labels(workspace: &Workspace) -> Vec<Diagnostic> {
    let rules: Vec<&str> = workspace
        .packages
        .iter()
        .filter(|p| p.build.platform.is_some() && is_platform_directory(&p.path))
        .map(|p| p.path.as_str())
        .collect();
    let libraries = workspace.packages.iter().filter_map(|p| p.build.library.as_ref()).map(|l| &l.admits);
    let tags = workspace.repo.tags.iter().flat_map(|t| [&t.requires, &t.forbids]);
    let mut out = Vec::new();
    for admitted in libraries.chain(tags) {
        for written in &admitted.platforms {
            let PlatformRef::Repository(label) = &written.value else { continue };
            let path = label.strip_prefix("//").unwrap_or(label);
            if !rules.contains(&path) {
                out.push(no_such_platform(&Spanned::new(label.clone(), written.span), &rules));
            }
        }
    }
    out
}

/// Walks the tree collecting every directory that holds a `BUILD.buri`.
fn collect_packages(root: &Path, dir: &Path, out: &mut Vec<String>) {
    if dir.join("BUILD.buri").is_file() {
        let rel = dir
            .strip_prefix(root)
            .or_ice("this walk started at `root` and only ever descends, so every path is under it")
            .display()
            .to_string()
            .replace('\\', "/");
        out.push(rel);
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut subdirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            // `.buri` holds the cache and the outputs; nothing there is source.
            !name.starts_with('.') && name != "target" && name != "node_modules"
        })
        .collect();
    subdirs.sort();
    for sub in subdirs {
        collect_packages(root, &sub, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_patterns() {
        assert_eq!(Pattern::parse("//...").unwrap(), Pattern::All);
        assert_eq!(Pattern::parse("//lib/...").unwrap(), Pattern::Recursive("lib".into()));
        assert_eq!(Pattern::parse("//lib/money").unwrap(), Pattern::Package("lib/money".into()));
        assert!(Pattern::parse("lib/money").is_err());
        assert!(Pattern::parse("@other//lib").unwrap_err().contains("external repository"));
    }

    #[test]
    fn recursive_patterns_include_the_package_itself() {
        let p = Pattern::parse("//lib/...").unwrap();
        assert!(p.matches("lib"));
        assert!(p.matches("lib/money"));
        assert!(!p.matches("libx"));
        assert!(!p.matches("cmd/server"));
    }

    #[test]
    fn visibility_forms() {
        assert!(Visibility::parse("//visibility:public").unwrap().allows("anything"));
        assert!(!Visibility::parse("//visibility:private").unwrap().allows("other"));
        let v = Visibility::parse("//cmd/...").unwrap();
        assert!(v.allows("cmd"));
        assert!(v.allows("cmd/server"));
        assert!(!v.allows("lib/money"));
        let v = Visibility::parse("//lib/money").unwrap();
        assert!(v.allows("lib/money"));
        assert!(!v.allows("lib/money/sub"));
    }

    #[test]
    fn testing_segment_anywhere_makes_a_path_test_only() {
        assert!(is_test_only_path("core/testing/assert"));
        assert!(is_test_only_path("//lib/ledger/testing"));
        // Both spellings of one module, and the rule reads the same segment in
        // each.
        assert!(is_test_only_path("//lib/ledger/testing/lib.buri"));
        assert!(is_test_only_path("//lib/testing/fakes.buri"));
        assert!(!is_test_only_path("//lib/money/lib.buri"));
        // Not a segment, so not test-only.
        assert!(!is_test_only_path("//lib/testingtools/lib.buri"));
        // The file name is a segment like any other, and `testing.buri` is not
        // the word `testing`.
        assert!(!is_test_only_path("//lib/money/testing.buri"));
    }

    /// A repository with a library, a module inside it and a testing surface,
    /// for the two spellings to be resolved against.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("buri-workspace-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(dir.join("lib/money/testing"));
        let _ = std::fs::write(dir.join("REPO.buri"), "name: \"scratch\"\n");
        let _ = std::fs::write(
            dir.join("lib/money/BUILD.buri"),
            "library {\n  sources: [\"cents.buri\"]\n  testing { sources: [] }\n}\n",
        );
        let _ = std::fs::write(dir.join("lib/money/lib.buri"), "");
        let _ = std::fs::write(dir.join("lib/money/cents.buri"), "");
        let _ = std::fs::write(dir.join("lib/money/testing/lib.buri"), "");
        dir
    }

    /// **Two spellings, one module.** A dependent writes `//lib/money` and a
    /// test source inside the package writes `//lib/money/lib.buri`, and both
    /// can stand in one compilation — so the resolver has to answer with one
    /// identity or the loader keys one file twice and the types it exports
    /// stop being the same types.
    #[test]
    fn both_spellings_of_a_module_resolve_to_one_canonical_path() {
        let dir = scratch("two-spellings");
        let mut map = crate::diagnostics::SourceMap::default();
        let mut diags = Diagnostics::default();
        let ws = Workspace::load(&dir, &mut map, &mut diags).expect("the scratch repository loads");
        let pairs = [
            ("//lib/money", "//lib/money/lib.buri", ModuleKind::LibrarySurface),
            ("//lib/money/testing", "//lib/money/testing/lib.buri", ModuleKind::TestingSurface),
            // And the extensionless inner path, which is legal to resolve so
            // that both diagnostics about it can name the file it meant.
            ("//lib/money/cents", "//lib/money/cents.buri", ModuleKind::Internal),
        ];
        for (module_form, file_form, kind) in pairs {
            let a = ws.resolve_module(module_form).expect(module_form);
            let b = ws.resolve_module(file_form).expect(file_form);
            let (a, b) = (a.in_package().expect(module_form).clone(), b.in_package().expect(file_form).clone());
            assert_eq!(a.path, file_form, "{module_form} is not canonicalised");
            assert_eq!(b.path, file_form);
            assert_eq!(a.file, b.file, "{module_form} and {file_form} are different files");
            assert_eq!(a.kind, kind);
            assert_eq!(b.kind, kind);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The longest package that contains a path owns it, down to the root's.
    #[test]
    fn the_longest_package_owns_a_path() {
        let dir = scratch("longest");
        let _ = std::fs::write(dir.join("BUILD.buri"), "library {\n  sources: [\"top.buri\"]\n}\n");
        let _ = std::fs::write(dir.join("top.buri"), "");
        let _ = std::fs::write(dir.join("lib/BUILD.buri"), "library {\n  sources: [\"moneyx/a.buri\"]\n}\n");
        let _ = std::fs::create_dir_all(dir.join("lib/moneyx"));
        let _ = std::fs::write(dir.join("lib/moneyx/a.buri"), "");
        let mut map = crate::diagnostics::SourceMap::default();
        let mut diags = Diagnostics::default();
        let ws = Workspace::load(&dir, &mut map, &mut diags).expect("the scratch repository loads");
        for (path, package, file) in [
            ("//lib/money/cents.buri", "lib/money", "lib/money/cents.buri"),
            ("//lib/money", "lib/money", "lib/money/lib.buri"),
            ("//lib/moneyx/a.buri", "lib", "lib/moneyx/a.buri"),
            ("//top.buri", "", "top.buri"),
        ] {
            let found = ws.resolve_module(path).expect(path);
            let found = found.in_package().expect(path);
            assert_eq!(ws.package(found.package).path, package, "{path}");
            assert_eq!(found.rel, file, "{path}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The half of the import rule that can be read off the string. The other
    /// half — which package the writer is in — is the loader's, and the two
    /// together are `import-missing-extension`.
    #[test]
    fn a_path_that_names_a_file_is_the_form_for_a_file_inside_your_own_package() {
        assert!(names_a_file("//lib/money/lib.buri"));
        assert!(names_a_file("//lib/money/cents.buri"));
        assert!(names_a_file("//proto/address.proto"));
        // The module form, which is what an import that leaves the package
        // writes. Legal, and not a file.
        assert!(!names_a_file("//lib/money"));
        assert!(!names_a_file("//lib/money/testing"));
        assert!(!names_a_file("core/list"));
    }
}
