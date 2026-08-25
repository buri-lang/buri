//! The build graph: repository root, packages, targets, labels, module
//! resolution, visibility, tags, and platforms.
//!
//! Five rules produce the shape of a repository, and everything here follows
//! from them (cli/src/docs/build/overview.md):
//!
//! 1. A directory with a `BUILD.buri` is a package.
//! 2. `lib.buri` is a library's whole public surface.
//! 3. `main.buri` is a compilation entry point.
//! 4. Tests live in `test/` and see only the target's surface.
//! 5. Everything is declared — a file on disk that no rule lists is an error.

use crate::build::buildfile::{self, BuildFile, Platform, RepoConfig, Spanned};
use crate::build::textproto::Document;
use crate::diagnostics::{Diagnostic, Diagnostics, FileId, Invariant as _, Span};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PackageId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RuleKind {
    Library,
    Binary,
}

/// A target is a package plus a rule kind. There is no `:name` syntax to learn
/// because a package holds at most one of each.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TargetId {
    pub package: PackageId,
    pub kind: RuleKind,
}

pub struct Package {
    /// `lib/money`, or the empty string for a package at the root.
    pub path: String,
    pub dir: PathBuf,
    pub build_path: PathBuf,
    pub build_file_id: FileId,
    pub build: BuildFile,
    /// The textproto tree, kept so `gen` and `format` can rewrite the file.
    pub document: Document,
}

impl Package {
    /// `//lib/money`
    pub fn label(&self) -> String {
        format!("//{}", self.path)
    }

    pub fn has_library(&self) -> bool {
        self.build.library.is_some()
    }

    pub fn has_binary(&self) -> bool {
        self.build.binary.is_some()
    }

    /// The suite one rule declares, if it declares one.
    ///
    /// "This target's test suite" is a question `test`, `watch` and `lint` all
    /// ask, and each used to answer it by matching on the rule kind itself —
    /// three copies of one two-line lookup, which is three places to forget
    /// when a rule gains a way to carry a suite.
    pub fn test_suite(&self, kind: RuleKind) -> Option<&buildfile::TestSuite> {
        match kind {
            RuleKind::Library => self.build.library.as_ref().and_then(|l| l.test.as_ref()),
            RuleKind::Binary => self.build.binary.as_ref().and_then(|b| b.test.as_ref()),
        }
    }
}

pub struct Workspace {
    pub root: PathBuf,
    pub repo: RepoConfig,
    pub packages: Vec<Package>,
    by_path: HashMap<String, PackageId>,
    /// Package paths longest-first, for resolving a module path to the package
    /// that contains it.
    sorted_paths: Vec<(String, PackageId)>,
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

/// A CLI target argument. Labels are always repository-absolute: a label means
/// the same thing wherever it is written, including from a subdirectory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// `//lib/money` — every target in that package.
    Package(String),
    /// `//lib/...` — that package and every package under it.
    Recursive(String),
    /// `//...`
    All,
}

impl Pattern {
    pub fn parse(s: &str) -> Result<Pattern, String> {
        if s == "//..." {
            return Ok(Pattern::All);
        }
        let Some(rest) = s.strip_prefix("//") else {
            if s.starts_with('@') {
                return Err(format!(
                    "`{s}` names an external repository, which is reserved and unimplemented"
                ));
            }
            return Err(format!(
                "`{s}` is not a label; labels are repository-absolute and start with `//`"
            ));
        };
        if let Some(package) = rest.strip_suffix("/...") {
            return Ok(Pattern::Recursive(package.to_string()));
        }
        if rest.ends_with('/') {
            return Err(format!("`{s}` has a trailing slash"));
        }
        if rest.contains("...") {
            return Err(format!("`{s}` is not a label; the only pattern forms are `//pkg/...` and `//...`"));
        }
        Ok(Pattern::Package(rest.to_string()))
    }

    pub fn matches(&self, package_path: &str) -> bool {
        match self {
            Pattern::All => true,
            Pattern::Package(p) => p == package_path,
            // `p` empty means every package, which is how `///...` — the one
            // spelling that parses to `Recursive("")` — selects the whole
            // repository. `Visibility::allows` deliberately does *not* read an
            // empty prefix that way, so the same string selects everything as a
            // target pattern and nothing but the root package as a visibility.
            // The asymmetry is load-bearing until somebody decides which of the
            // two is wrong; merging the arms is not a behaviour-preserving edit.
            Pattern::Recursive(p) => {
                package_path == p
                    || (p.is_empty() || package_path.starts_with(&format!("{p}/")))
            }
        }
    }
}

/// A `visibility` entry. The pattern language is the same shape as a label's,
/// plus the two `//visibility:` forms.
///
/// Parsed where the build file is read, so a rule's `visibility` is a list of
/// these rather than a list of strings each consumer re-parses and each
/// consumer is free to give up on. `//...` has its own variant rather than
/// being `Recursive("")`, so "everything" is a thing the enum says instead of
/// an empty string two `allows` arms have to remember to special-case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
    /// `//...` — every package in this repository.
    Everything,
    Package(String),
    Recursive(String),
}

impl Visibility {
    pub fn parse(s: &str) -> Result<Visibility, String> {
        match s {
            "//visibility:public" => return Ok(Visibility::Public),
            "//visibility:private" => return Ok(Visibility::Private),
            _ => {}
        }
        if s.starts_with("//visibility:") {
            return Err(format!(
                "`{s}` is not a visibility; the two forms are `//visibility:public` and \
                 `//visibility:private`"
            ));
        }
        match Pattern::parse(s)? {
            Pattern::All => Ok(Visibility::Everything),
            Pattern::Package(p) => Ok(Visibility::Package(p)),
            Pattern::Recursive(p) => Ok(Visibility::Recursive(p)),
        }
    }

    /// How the entry is written. Diagnostics print this rather than the text
    /// the build file held, so a rejected entry cannot be echoed back inside a
    /// list of entries that are in force.
    pub fn spelling(&self) -> String {
        match self {
            Visibility::Public => "//visibility:public".to_string(),
            Visibility::Private => "//visibility:private".to_string(),
            Visibility::Everything => "//...".to_string(),
            Visibility::Package(p) => format!("//{p}"),
            Visibility::Recursive(p) => format!("//{p}/..."),
        }
    }

    pub fn allows(&self, package_path: &str) -> bool {
        match self {
            Visibility::Public => true,
            Visibility::Private => false,
            Visibility::Everything => true,
            Visibility::Package(p) => p == package_path,
            // No `p.is_empty()` arm, unlike `Pattern::matches` above: a
            // `Recursive("")` visibility grants the root package and nothing
            // else, where the same shape as a target pattern selects every
            // package. See the note there — the two are not merged on purpose.
            Visibility::Recursive(p) => {
                package_path == p || package_path.starts_with(&format!("{p}/"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Module paths
// ---------------------------------------------------------------------------

/// What a module inside a package is. There is deliberately no `Std` here: a
/// `core/...` module has no package, no file on disk and no repository-relative
/// name, so it is a variant of [`ModuleLocation`] rather than a kind with three
/// fields nulled out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModuleKind {
    /// `//pkg` — the library's `lib.buri`.
    LibrarySurface,
    /// `//pkg/testing` — `testing/lib.buri`.
    TestingSurface,
    /// `//pkg/main` — a binary's entry point.
    BinaryEntry,
    /// `//pkg/inner` — one module inside a library.
    Internal,
    /// `//pkg/schema.proto` — a module generated from a `.proto` schema. It is
    /// `Internal` in every way that matters, and the separate kind exists so
    /// the loader knows to *generate* it rather than read it.
    Proto,
}

/// Where a module path resolves to.
///
/// The two cases are genuinely different shapes rather than one shape with
/// optional halves: a `core/...` module is embedded in the toolchain, so it has
/// no package and no file, and a module in this repository always has both.
/// Splitting them is what lets a consumer that needs the package get it without
/// an `Option` to skip past — and there is no longer an empty `PathBuf` standing
/// in for "there is no file".
#[derive(Clone, Debug)]
pub enum ModuleLocation {
    /// A `core/...` module, shipping with the toolchain.
    Std { path: String },
    InPackage(PackageModule),
}

#[derive(Clone, Debug)]
pub struct PackageModule {
    pub path: String,
    pub kind: ModuleKind,
    pub package: PackageId,
    /// Absolute path on disk.
    pub file: PathBuf,
    /// Repository-relative name, used in diagnostics and in cache keys.
    pub rel: String,
}

impl ModuleLocation {
    pub fn path(&self) -> &str {
        match self {
            ModuleLocation::Std { path } => path,
            ModuleLocation::InPackage(m) => &m.path,
        }
    }

    pub fn in_package(&self) -> Option<&PackageModule> {
        match self {
            ModuleLocation::Std { .. } => None,
            ModuleLocation::InPackage(m) => Some(m),
        }
    }
}

/// Any module path with a `testing` segment is test-only. The rule is in the
/// import line rather than in a build file three directories away.
pub fn is_test_only_path(path: &str) -> bool {
    path.trim_start_matches("//").split('/').any(|seg| seg == "testing")
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
            diagnostics.extend(read.errors);
            if read.value.library.is_none() && read.value.binary.is_none() {
                diagnostics.push(
                    Diagnostic::error(
                        Span::point(id, 0),
                        format!("//{path} declares neither a library nor a binary"),
                    )
                    .with_fix("add a `library { }` or `binary { }` rule, or delete the build file"),
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
        let mut sorted_paths: Vec<(String, PackageId)> =
            by_path.iter().map(|(k, v)| (k.clone(), *v)).collect();
        // Longest first, so `//lib/money/cents` finds `lib/money` before `lib`.
        sorted_paths.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));

        let workspace =
            Workspace { root: root.to_path_buf(), repo, packages, by_path, sorted_paths };
        workspace.check_package_module_collisions(diagnostics);
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

    /// Rule 3 of LIBRARIES.md: a module path may not also be a package path.
    /// If `lib/money/cents/` is a package, then `lib/money/cents.buri` in the
    /// parent has two meanings, so the pair is rejected by name.
    fn check_package_module_collisions(&self, diagnostics: &mut Diagnostics) {
        for p in &self.packages {
            if p.path.is_empty() {
                continue;
            }
            let Some((parent, last)) = p.path.rsplit_once('/') else { continue };
            let Some(parent_id) = self.package_by_path(parent) else { continue };
            let sibling = self.package(parent_id).dir.join(format!("{last}.buri"));
            if sibling.is_file() {
                diagnostics.push(
                    Diagnostic::error(
                        Span::point(p.build_file_id, 0),
                        format!(
                            "//{} is a package, and {}/{last}.buri is a module in //{parent}",
                            p.path, parent
                        ),
                    )
                    .with_fix(format!(
                        "the module path \"//{}\" would name both; rename one of them",
                        p.path
                    )),
                );
            }
        }
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
        }
    }

    pub fn tags(&self, target: TargetId) -> &[Spanned<String>] {
        let p = self.package(target.package);
        match target.kind {
            RuleKind::Library => p.build.library.as_ref().map(|l| &l.tags[..]).unwrap_or(&[]),
            RuleKind::Binary => p.build.binary.as_ref().map(|b| &b.tags[..]).unwrap_or(&[]),
        }
    }

    /// Resolved dependency edges: (dependency library target, the label span).
    /// A binary additionally depends on the library in its own package, which
    /// is implicit and carries no span.
    pub fn dep_edges(&self, target: TargetId) -> Vec<(TargetId, Option<Span>)> {
        let mut out = Vec::new();
        if target.kind == RuleKind::Binary && self.package(target.package).has_library() {
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
    /// production tag closure, count against `unused-dep`, and make a cycle
    /// out of a suite that merely borrows a helper. What it *is* subject to is
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
    fn dep_target(&self, label: &str) -> Option<TargetId> {
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
    /// `undeclared-source`'s business rather than this function's.
    pub fn rule_of_file(&self, package: PackageId, rel: &str) -> Option<RuleKind> {
        let p = self.package(package);
        match rel {
            "lib.buri" | "testing/lib.buri" if p.has_library() => return Some(RuleKind::Library),
            "main.buri" if p.has_binary() => return Some(RuleKind::Binary),
            _ => {}
        }
        if let Some(l) = &p.build.library {
            let listed = l
                .sources
                .iter()
                .chain(l.proto_sources.iter())
                .chain(l.test.iter().flat_map(|t| t.sources.iter()))
                .chain(l.testing.iter().flat_map(|t| t.sources.iter()));
            if listed.into_iter().any(|s| s.value == rel) {
                return Some(RuleKind::Library);
            }
        }
        if let Some(b) = &p.build.binary {
            if b.sources
                .iter()
                .chain(b.proto_sources.iter())
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
        let label = self.package(loc.package).label();
        Some(if is_test_only_path(path) { format!("{label}/testing") } else { label })
    }

    /// Resolves a module path written in an import.
    pub fn resolve_module(&self, path: &str) -> Result<ModuleLocation, String> {
        if path.starts_with('.') {
            return Err(format!(
                "\"{path}\" is a relative path; every module path is absolute, so a file can \
                 move between directories without its imports changing"
            ));
        }
        if crate::compiler::standard_library::is_std_path(path) {
            return Ok(ModuleLocation::Std { path: path.to_string() });
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

        // `//pkg` is `lib.buri` and nothing else — one spelling per module.
        if rest.ends_with("/lib") {
            let package = rest.trim_end_matches("/lib");
            return Err(format!(
                "\"{path}\" is not a legal module path; write \"//{package}\", which is that \
                 library's surface"
            ));
        }

        // Longest package prefix wins.
        for (package_path, id) in &self.sorted_paths {
            let remainder = if rest == package_path {
                ""
            } else if package_path.is_empty() {
                rest
            } else if let Some(r) = rest.strip_prefix(&format!("{package_path}/")) {
                r
            } else {
                continue;
            };

            let package = self.package(*id);
            let (kind, file) = match remainder {
                "" => (ModuleKind::LibrarySurface, package.dir.join("lib.buri")),
                "testing" => (ModuleKind::TestingSurface, package.dir.join("testing/lib.buri")),
                "main" => (ModuleKind::BinaryEntry, package.dir.join("main.buri")),
                // A `.proto` path names the schema itself, extension and all.
                // `build.proto`'s own header writes the import that way —
                // `from "//proto/foo.proto" import ...` — so the module path
                // and the file on disk are the same string, and there is no
                // second spelling to learn.
                r if r.ends_with(".proto") => (ModuleKind::Proto, package.dir.join(r)),
                r => (ModuleKind::Internal, package.dir.join(format!("{r}.buri"))),
            };
            if !file.is_file() {
                return Err(format!("\"{path}\" names no file ({})", self.rel_of(&file)));
            }
            let rel = self.rel_of(&file);
            return Ok(ModuleLocation::InPackage(PackageModule {
                path: path.to_string(),
                kind,
                package: *id,
                file,
                rel,
            }));
        }
        Err(format!("\"{path}\" is in no package of this repository"))
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

    /// The platforms a target can be built for: the intersection, over every
    /// target in its closure, of that target's `platforms` and the
    /// `requires.platforms` of every tag it carries — treating unset as "all".
    pub fn platforms(&self, target: TargetId) -> BTreeSet<Platform> {
        let mut allowed: BTreeSet<Platform> = Platform::ALL.into_iter().collect();
        for member in self.closure(target) {
            if let Some(lib) = &self.package(member.package).build.library {
                if member.kind == RuleKind::Library && !lib.platforms.is_empty() {
                    let declared: BTreeSet<Platform> =
                        lib.platforms.iter().map(|p| p.value).collect();
                    allowed = allowed.intersection(&declared).copied().collect();
                }
            }
            for tag in self.tags(member) {
                if let Some(decl) = self.repo.tag(&tag.value) {
                    if !decl.requires_platforms.is_empty() {
                        let required: BTreeSet<Platform> =
                            decl.requires_platforms.iter().map(|p| p.value).collect();
                        allowed = allowed.intersection(&required).copied().collect();
                    }
                }
            }
        }
        allowed
    }

    /// Explains why `platform` is not available to `target`: the member of the
    /// closure that rules it out, and how it was reached.
    pub fn platform_blocker(
        &self,
        target: TargetId,
        platform: Platform,
    ) -> Option<(TargetId, String)> {
        for member in self.closure(target) {
            if let Some(lib) = &self.package(member.package).build.library {
                if member.kind == RuleKind::Library
                    && !lib.platforms.is_empty()
                    && !lib.platforms.iter().any(|p| p.value == platform)
                {
                    let list: Vec<&str> =
                        lib.platforms.iter().map(|p| p.value.slug()).collect();
                    return Some((
                        member,
                        format!("{} declares platforms {}", self.label(member), list.join(", ")),
                    ));
                }
            }
            for tag in self.tags(member) {
                if let Some(decl) = self.repo.tag(&tag.value) {
                    if !decl.requires_platforms.is_empty()
                        && !decl.requires_platforms.iter().any(|p| p.value == platform)
                    {
                        let list: Vec<&str> =
                            decl.requires_platforms.iter().map(|p| p.value.slug()).collect();
                        return Some((
                            member,
                            format!(
                                "{} is tagged \"{}\", which requires {}",
                                self.label(member),
                                tag.value,
                                list.join(", ")
                            ),
                        ));
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
        let mut out = BTreeMap::new();
        for member in self.closure(target) {
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
    /// even though neither reaches the other.
    pub fn forbidden_pair(&self, target: TargetId) -> Option<(String, TargetId, String, TargetId)> {
        let carried = self.closure_tags(target);
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
        assert!(is_test_only_path("//lib/testing/fakes"));
        assert!(!is_test_only_path("//lib/money"));
        // Not a segment, so not test-only.
        assert!(!is_test_only_path("//lib/testingtools"));
    }
}
