//! The standard library modules every compilation opens with, loaded and
//! checked once per process.
//!
//! A compilation loads the same modules before anything of its own: the
//! prelude and the built-in types' modules (`Loader::load_unit`), and for a
//! snippet the whole library after them (`driver::analyze_snippet_on`). Their
//! text is compiled into this binary, and each of their files has the same id
//! in every source map (`FileId::standard`), so what loading and checking them
//! produces is the same every time it is done. A [`Snapshot`] is that result,
//! and an analysis starts from it: it loads its own modules after the
//! snapshot's (`Loader::seeded`) and runs every checker pass over those alone
//! (`Checker::resume`).
//!
//! One per process, shared by every thread: `buri test` checks its suites on a
//! pool of workers, and they all read the same snapshot. The first thread to
//! ask builds it, and a thread that asks meanwhile waits for that one rather
//! than building a second.

use crate::compiler::modules::{Loader, ModuleData};
use crate::compiler::semantics::resolve::{Base, Bodies, Checker};
use crate::compiler::semantics::types::ModuleId;
use crate::diagnostics::{Diagnostics, SourceMap};
use crate::hash::Map as HashMap;
use std::sync::OnceLock;

/// Which standard library modules a compilation opens with.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Opening {
    /// The prelude's modules and the built-in types', which every compilation
    /// loads first.
    Builtin,
    /// Those, then every module of the library, which is how a snippet is
    /// compiled.
    Library,
}

/// The modules of an [`Opening`], loaded and checked.
pub struct Snapshot {
    /// The modules, in the order a compilation loads them.
    pub modules: Vec<ModuleData>,
    /// Every spelling that reaches one of them, as `Loaded::by_path` has it.
    pub by_path: HashMap<String, ModuleId>,
    /// What loading and checking them reported, which is nothing unless the
    /// standard library itself is broken.
    pub diagnostics: Diagnostics,
    /// What the checker made of them.
    pub base: Base,
}

/// The snapshot of `opening`, built the first time any thread asks.
///
/// `bodies` says whether the modules' function bodies are checked too, which
/// an analysis that checks every body wants and a scoped one must not have:
/// its `Checked::bodies` holds only what it asked for.
pub fn of(opening: Opening, bodies: bool) -> &'static Snapshot {
    static BUILTIN: OnceLock<Snapshot> = OnceLock::new();
    static BUILTIN_BODIES: OnceLock<Snapshot> = OnceLock::new();
    static LIBRARY: OnceLock<Snapshot> = OnceLock::new();
    static LIBRARY_BODIES: OnceLock<Snapshot> = OnceLock::new();
    let once = match (opening, bodies) {
        (Opening::Builtin, false) => &BUILTIN,
        (Opening::Builtin, true) => &BUILTIN_BODIES,
        (Opening::Library, false) => &LIBRARY,
        (Opening::Library, true) => &LIBRARY_BODIES,
    };
    once.get_or_init(|| build(opening, bodies))
}

/// The snapshot of `opening`, built afresh rather than kept: what [`of`]
/// pays the first time it is asked.
pub fn build(opening: Opening, bodies: bool) -> Snapshot {
    let mut map = SourceMap::new();
    let mut cache = crate::parsing::parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = {
        let _phase = crate::profile::enter(crate::profile::Phase::Parse);
        let mut loader = Loader::new(None, &mut map, &mut diagnostics, &mut cache);
        loader.load_builtin_modules();
        if opening == Opening::Library {
            loader.load_all_std();
        }
        loader.finish()
    };
    let wanted = if bodies { Bodies::All } else { Bodies::In(Vec::new()) };
    let base = {
        let _phase = crate::profile::enter(crate::profile::Phase::Check);
        Checker::new(&loaded, None, &mut diagnostics).checking(wanted).base()
    };
    Snapshot { modules: loaded.modules, by_path: loaded.by_path, diagnostics, base }
}
