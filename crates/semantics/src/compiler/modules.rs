//! What a compilation is made of: its modules, the role each is compiled in,
//! and the unit being built. The checker reads these; `buri`'s module loader,
//! which reads build files, generators and tools, produces them.

use crate::build::buildfile::Platform;
use crate::build::workspace::TargetId;
use crate::compiler::semantics::types::ModuleId;
use crate::diagnostics::{FileId, Invariant as _};
use crate::hash::Map as HashMap;
use crate::parsing::tree;
use std::path::PathBuf;
/// What a module is being compiled as. This is what decides whether `test`
/// declarations, expression statements, and test-only imports are legal in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// A `core/...` module, shipping with the toolchain.
    Std,
    /// A platform module: `platform/effect`, `platform/host`, `core/testing/*`.
    /// Only these may declare effects.
    Platform,
    /// Ordinary library or binary source.
    Source,
    /// The module exporting `main`, and the only place in a program where a
    /// context may be built.
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

#[derive(Clone)]
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
    pub ast: std::sync::Arc<tree::Module>,
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
    /// `Some(p)` holds the entry to `p`'s host. `None` is an analysis that is
    /// not building an artifact — `buri lint`, the language server, the
    /// documentation harness, `buri test` — which accepts any bundled host.
    pub platform: Option<Platform>,
    /// The exported function the output being built enters through.
    ///
    /// `Some("fetch")` says that this artifact starts at `fetch` and holds only
    /// what `fetch` reaches, so `main`'s own signature is none of this
    /// build's business. `None` is every analysis that is not building one
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
    /// The output this compilation is for, carried over from [`Unit::platform`],
    /// which holds an entry to that platform's host. `None` for every analysis
    /// that is not building one.
    pub platform: Option<Platform>,
    /// The entry the output being built enters through, carried over from
    /// [`Unit::entry`]. See it for what it decides.
    pub entry: Option<String>,
    /// The rules whose generators this compilation reported on. Their inputs,
    /// and the schemas those were checked against, are files it read.
    pub generated_rules: Vec<TargetId>,
    /// The repository platform the output being built names, with the entry
    /// of it this build fills. `None` for a bundled platform and for every
    /// analysis that is not building one output.
    pub custom: Option<crate::build::buildfile::CustomPlatform>,
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
