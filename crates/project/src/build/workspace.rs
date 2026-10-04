//! The build graph's vocabulary: packages, targets, labels, visibility and
//! where a module path resolves to.
//!
//! The graph itself, `Workspace`, is in `buri`, because loading one reads
//! generators and tools. The checker sees it through [`Packages`], which is
//! the three questions it asks.

use crate::build::buildfile::{self, BuildFile, Platform};
use crate::build::textproto::Document;
use crate::diagnostics::{FileId, Span};
use std::path::PathBuf;

/// What the checker asks of a workspace. `buri`'s `Workspace` answers it.
pub trait Packages {
    fn package(&self, id: PackageId) -> &Package;

    /// Where a module path resolves to, or why it resolves nowhere.
    fn resolve_module(&self, path: &str) -> Result<ModuleLocation, String>;

    /// Every entry a binary's `outputs` name, in declaration order.
    fn declared_entries(&self, target: TargetId) -> Vec<DeclaredEntry>;
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PackageId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RuleKind {
    Library,
    Binary,
    /// A program the build runs on a language's files, rooted at `tool.buri`.
    Tool,
}

impl RuleKind {
    /// The rule's name as a build file writes it.
    pub fn name(self) -> &'static str {
        match self {
            RuleKind::Library => "library",
            RuleKind::Binary => "binary",
            RuleKind::Tool => "tool",
        }
    }
}

/// A target is a package plus a rule kind. There is no `:name` syntax to learn
/// because a package holds at most one of each.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TargetId {
    pub package: PackageId,
    pub kind: RuleKind,
}

/// The member of a closure that rules a platform out, and why.
#[derive(Clone, Debug)]
pub struct PlatformBlocker {
    pub member: TargetId,
    pub why: String,
    /// The word a tag's `forbids` names it by, rather than a whitelist leaving
    /// it out.
    pub forbidden: Option<String>,
}

/// One entry a binary's `outputs` names, and the platform that fixes its shape.
///
/// This is what makes the entry-signature check per *entry* rather than per
/// target. A binary with a page and a worker in it declares two of these out
/// of one `main.buri`.
#[derive(Clone, Debug)]
pub struct DeclaredEntry {
    pub name: String,
    /// Whether the output wrote the name, rather than defaulting to `main`.
    pub named: bool,
    /// The `entry` field, or the output itself where it named none.
    pub span: Span,
    pub platform: Platform,
    /// The repository platform the output names, `//platform/<name>`, with
    /// the entry of it this function fills. `None` for a bundled platform.
    pub custom: Option<buildfile::CustomPlatform>,
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

    /// The module path of one of this package's files: `//lib/money/lib.buri`.
    ///
    /// Not `label()` with a suffix glued on, because a package at the
    /// repository root has the empty path and `label()` is then `//` — one
    /// slash too many. That package's surface is `//lib.buri`, and it is a
    /// module path like any other now, where under the old spelling it was the
    /// one module with no path at all.
    pub fn module_path(&self, rel: &str) -> String {
        match self.path.is_empty() {
            true => format!("//{rel}"),
            false => format!("//{}/{rel}", self.path),
        }
    }

    pub fn has_library(&self) -> bool {
        self.build.library.is_some()
    }

    pub fn has_binary(&self) -> bool {
        self.build.binary.is_some()
    }

    pub fn has_tool(&self) -> bool {
        self.build.tool.is_some()
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
            RuleKind::Tool => self.build.tool.as_ref().and_then(|t| t.test.as_ref()),
        }
    }
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
    /// `//pkg/lib.buri` — the library's surface.
    LibrarySurface,
    /// `//pkg/testing/lib.buri` — the testing surface.
    TestingSurface,
    /// `//pkg/main.buri` — a binary's entry point.
    BinaryEntry,
    /// `//platform/<name>/platform.buri` — a repository platform's host type,
    /// its entries and its production structs.
    PlatformSurface,
    /// `//pkg/inner.buri` — one module inside a library.
    Internal,
    /// `//pkg/whatever` — a module a `generators` entry produced. Also
    /// `Internal` in every way that matters, and a separate kind so the loader
    /// knows to take its text from [`crate::build::generators::Store`] rather
    /// than from disk. It has no file at all, and its name is whatever the
    /// generator called it, so every reader that resolves a module back to
    /// bytes on disk would be wrong about this one.
    Generated,
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
    /// The module's **canonical** path: `"//"` followed by
    /// [`PackageModule::rel`], letter for letter.
    ///
    /// Not the path as it was written, because two spellings reach one file
    /// and only one of them can be the module's identity. `//lib/money` is
    /// what a dependent writes and `//lib/money/lib.buri` is what a file
    /// inside `lib/money` writes, and both can appear in a single compilation
    /// — a test source naming its own surface while a dependency of that suite
    /// names it from outside. The loader keys a module by this path, so two
    /// keys would be two copies of every type `lib.buri` exports, and a value
    /// of one would not be a value of the other.
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

/// Whether a module path names a file rather than a module directory.
///
/// Why a schema a `generators` entry lists has no module: a check failed, and
/// said so on the file it failed.
pub const SCHEMA_HAS_ERRORS: &str = "a schema this entry lists has errors, so it generated nothing";

/// The two extensions a module can have are `.buri` and `.proto`. Asked of the
/// *path* rather than of the disk, because this is half of the question "may
/// this file be written here" — the other half is which package the writer is
/// in — and neither half is a fact about what is on disk.
pub fn names_a_file(path: &str) -> bool {
    path.ends_with(".buri") || path.ends_with(".proto")
}

/// Any module path with a `testing` segment is test-only. The rule is in the
/// import line rather than in a build file three directories away.
pub fn is_test_only_path(path: &str) -> bool {
    path.trim_start_matches("//").split('/').any(|seg| seg == "testing")
}
