//! Putting the front end together: load a unit, check it, report.

use crate::build::actions;
use crate::build::buildfile::Platform;
use crate::build::workspace::{Packages, Rule, TargetId, Workspace};
use crate::compiler::modules::{Loaded, Loader, ModuleData, PerModule, Unit};
use crate::compiler::semantics::resolve::{Bodies, Checked, Checker};
use crate::compiler::semantics::types::ModuleId;
use crate::compiler::snapshot::{self, Opening, Snapshot};
use crate::diagnostics::{Diagnostic, Diagnostics, FileId, SourceMap, Span};

pub struct Analysis {
    pub loaded: Loaded,
    pub checked: Checked,
    pub diagnostics: Diagnostics,
}

impl Analysis {
    /// The exported function `name` of the entry module at `path`.
    pub fn entry(&self, path: &str, name: &str) -> Option<crate::compiler::semantics::types::FnId> {
        let module = self.loaded.find(path)?;
        self.checked.entries.get(&module)?.get(name).copied()
    }

    /// The same, where it starts by itself: see
    /// [`Started`](crate::compiler::semantics::resolve::Started).
    pub fn started(
        &self,
        path: &str,
        name: &str,
    ) -> Option<&crate::compiler::semantics::resolve::Started> {
        self.checked.started.get(&self.entry(path, name)?)
    }

    /// All of this analysis as one program's share of it.
    pub fn whole(&self) -> Share {
        Share {
            holds: self.loaded.modules.per_module(true),
            reported: self.diagnostics.items.clone(),
            generated_rules: self.loaded.generated_rules.clone(),
            restyled: None,
        }
    }
}

/// Loads and checks one unit. The two halves are separate so that `lint` and
/// `query` can stop after loading.
pub fn analyze(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    unit: &Unit,
) -> Analysis {
    analyze_all(ws, map, cache, std::slice::from_ref(unit))
}

/// Loads and checks **several** units as one compilation.
///
/// One `Loader`, one `Checker`, one set of modules: a module two units both
/// reach is loaded once, and a `test` declaration in either one is in
/// `Checked::tests`, so `monomorphize::Roots::Tests` roots the program at every
/// suite in the list. That is the whole of what a batched test binary needs from
/// the front end — nothing below here knows how many targets it came from.
///
/// Three properties make one call the same thing as several, and all three are
/// `Loader`'s already rather than something this adds:
///
/// - **Loading is idempotent.** Every entry point consults `by_path` first, so a
///   library that two units both depend on is parsed once and gets one
///   `ModuleId`; a second `load_unit` naming it again is a lookup.
/// - **Nothing is granted by presence.** Whether an import is legal is decided
///   at the import line, against the workspace — being in the same compilation
///   as a module is not a way to reach it. Visibility is the build system's
///   (`actions::check_visibility`) and is asked per target either way.
/// - **A unit has one root at a time.** Two binaries batched together load two
///   `Role::Entry` modules and the checker keeps the last one's `main` in
///   `Checked::entry`; nothing reads it here, because a test program's roots are
///   its `test` blocks and `main` is never instantiated.
///
/// The order of the list is the order the test sources load in, which is the
/// order `Checked::tests` — and therefore the block numbering of the linked
/// binary — comes out in. Callers that attribute a block to a suite depend on
/// that, so it is stated here rather than assumed there.
///
/// A one-element list is byte-for-byte [`analyze`]: same loader, same calls,
/// same order.
pub fn analyze_all(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    units: &[Unit],
) -> Analysis {
    let loading = load_all(ws, map, cache, units);
    check(loading, ws, map)
}

/// The loaded half of an [`analyze_all`], before any checking.
pub struct Loading {
    loaded: Loaded,
    diagnostics: Diagnostics,
    /// How many of `diagnostics` the opening's snapshot carried in.
    opening: usize,
}

/// The first half of [`analyze_all`]: loads the units.
///
/// The halves are apart because they need different things. Loading reads
/// files and mints their ids in `map`, and parses into `cache`, so it runs
/// where those live; checking needs neither, and runs on any thread. That is
/// how `buri test` checks its suites side by side while one thread loads them.
pub fn load_all(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    units: &[Unit],
) -> Loading {
    load_on(
        ws,
        map,
        cache,
        snapshot::of(Opening::Builtin, true),
        |loader| load_units(loader, units),
    )
}

impl Loading {
    /// What was loaded, for a caller recording which files it read.
    pub fn loaded(&self) -> &Loaded {
        &self.loaded
    }

    /// What loading reported, after the opening's own diagnostics.
    pub fn reported(&self) -> &[Diagnostic] {
        self.diagnostics
            .items
            .get(self.opening..)
            .unwrap_or_default()
    }

    /// The bytes of repository source this load holds, which is what checking
    /// it grows with. The standard library is left out: every load has it.
    pub fn source_bytes(&self, map: &SourceMap) -> u64 {
        source_bytes(self.loaded.modules.iter(), map)
    }
}

/// The bytes of repository source in `modules`.
fn source_bytes<'m>(modules: impl Iterator<Item = &'m ModuleData>, map: &SourceMap) -> u64 {
    modules
        .filter(|m| m.pkg.is_some())
        .map(|m| u64::try_from(map.get(m.file).text.len()).unwrap_or(u64::MAX))
        .fold(0, u64::saturating_add)
}

/// The second half of [`analyze_all`]: checks what [`load_all`] loaded.
///
/// `map` only names files, to put the diagnostics in order. A copy taken
/// after the load does.
pub fn check(loading: Loading, ws: Option<&Workspace>, map: &SourceMap) -> Analysis {
    check_all(loading, ws, map, false)
}

/// [`check`], [`restylable`](Checker::restylable) where asked.
fn check_all(
    loading: Loading,
    ws: Option<&Workspace>,
    map: &SourceMap,
    restylable: bool,
) -> Analysis {
    check_on(
        loading,
        ws,
        map,
        snapshot::of(Opening::Builtin, true),
        Bodies::All,
        restylable,
    )
}

fn load_units(loader: &mut Loader, units: &[Unit]) {
    for unit in units {
        loader.load_unit(unit);
    }
}

/// Whose bodies [`analyze_on`] checks. Named before loading, and resolved to
/// files after it, because which files are the repository's is known only then.
enum Scope<'a> {
    All,
    /// The files [`repository_files`] names.
    Repository,
    Files(&'a [FileId]),
}

/// Loads with `load` on top of the standard library modules `opening` names,
/// checked once per process rather than once per call (`compiler::snapshot`),
/// and checks the bodies `scope` names.
///
/// `load` has to load the opening's modules first, which every caller does
/// by starting the way a loader from nothing would: `Loader::load_unit` loads
/// [`Opening::Builtin`]'s before anything else, and a snippet loads the whole
/// library after it, which is [`Opening::Library`].
///
/// What comes back is what a loader and a checker starting from nothing
/// return, diagnostics included: the modules keep their ids, and so does every
/// type constructor, trait and constant they declare.
fn analyze_on(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    opening: Opening,
    scope: Scope,
    load: impl FnOnce(&mut Loader),
) -> Analysis {
    let snapshot = snapshot::of(opening, matches!(scope, Scope::All));
    let loading = load_on(ws, map, cache, snapshot, load);
    let bodies = match scope {
        Scope::All => Bodies::All,
        Scope::Repository => Bodies::In(repository_files(&loading.loaded)),
        Scope::Files(files) => Bodies::In(files.to_vec()),
    };
    check_on(loading, ws, map, snapshot, bodies, false)
}

fn load_on(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    snapshot: &'static Snapshot,
    load: impl FnOnce(&mut Loader),
) -> Loading {
    let _phase = crate::profile::enter(crate::profile::Phase::Parse);
    let mut diagnostics = Diagnostics::new();
    diagnostics.extend(snapshot.diagnostics.items.iter().cloned());
    let opening = diagnostics.items.len();
    let loaded = {
        let mut loader = Loader::seeded(ws, map, &mut diagnostics, cache, snapshot);
        load(&mut loader);
        loader.finish()
    };
    Loading {
        loaded,
        diagnostics,
        opening,
    }
}

fn check_on(
    loading: Loading,
    ws: Option<&Workspace>,
    map: &SourceMap,
    snapshot: &Snapshot,
    bodies: Bodies,
    restylable: bool,
) -> Analysis {
    let _phase = crate::profile::enter(crate::profile::Phase::Check);
    let Loading {
        loaded,
        mut diagnostics,
        ..
    } = loading;
    let checked = Checker::resume(
        &loaded,
        ws.map(|w| w as &dyn Packages),
        &mut diagnostics,
        &snapshot.base,
    )
    .checking(bodies)
    .restylable(restylable)
    .run();
    diagnostics.sort(map);
    Analysis {
        loaded,
        checked,
        diagnostics,
    }
}

/// Loads one unit and checks the bodies the *repository* wrote, leaving the
/// standard library's own bodies unchecked.
///
/// Everything else is the whole closure exactly as [`analyze`] has it: every
/// module is loaded and parsed, and every signature, type, trait, impl,
/// module-level `let` and `context` — the standard library's included — is
/// elaborated, because that is what a repository body is checked *against*.
/// What is skipped is step 5 for the modules that ship inside this binary.
///
/// Nothing is lost by skipping them. A standard library body can only report a
/// diagnostic if the standard library itself is broken, and a broken one is a
/// broken *toolchain*: its text is compiled into this executable and cannot
/// move while the process runs, `buri version --self-check` reads all of it
/// ([`analyze_stdlib`]), and `analyze_std_module` checks each module the way a
/// program reaches it. So a repository asking what is wrong with *its* files
/// was type-checking a thousand function bodies per compilation to be told
/// what the toolchain's own tests already say.
///
/// What comes back is byte-identical for every repository file —
/// `tests/language/scoped_bodies.rs` holds that over every fixture repository
/// — because this is [`analyze_bodies_in`] with the repository's files named,
/// and that equality is the property that pass already has.
///
/// **Not for a build.** `middle::monomorphize` walks the body of every
/// function an entry point reaches, and most of those are the standard
/// library's. A build wants [`analyze`].
pub fn analyze_program(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    unit: &Unit,
) -> Analysis {
    analyze_on(
        ws,
        map,
        cache,
        Opening::Builtin,
        Scope::Repository,
        |loader| loader.load_unit(unit),
    )
}

/// [`analyze_program`] over several units as one compilation, in its two
/// halves: this one loads, and [`check_programs`] checks.
///
/// `buri lint` asks this of every target at once, so that a library twenty
/// targets depend on is loaded and checked once rather than twenty times.
pub fn load_programs(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    units: &[Unit],
) -> Loading {
    load_on(
        ws,
        map,
        cache,
        snapshot::of(Opening::Builtin, false),
        |loader| load_units(loader, units),
    )
}

/// The second half of [`load_programs`]. One unit through both halves is
/// [`analyze_program`].
pub fn check_programs(loading: Loading, ws: Option<&Workspace>, map: &SourceMap) -> Analysis {
    let bodies = Bodies::In(repository_files(&loading.loaded));
    check_on(
        loading,
        ws,
        map,
        snapshot::of(Opening::Builtin, false),
        bodies,
        false,
    )
}

/// Which bodies a [`shared`] compilation checks: all of them, as a build's
/// [`analyze`] does, or the repository's, as [`analyze_program`] does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Checking {
    All,
    Repository,
}

/// Several targets checked as one compilation, and each one's share of it.
pub struct Shared {
    /// Shared with the workers that build its programs.
    pub analysis: std::sync::Arc<Analysis>,
    /// By target, and by whether the share holds the target's tests.
    pub shares: std::collections::BTreeMap<(TargetId, bool), Share>,
}

/// What one target's own compilation would have held and reported.
#[derive(Clone)]
pub struct Share {
    pub holds: PerModule<bool>,
    /// What its own compilation reports, in the order it sorts in.
    pub reported: Vec<Diagnostic>,
    /// The rules whose generated code it loaded.
    pub generated_rules: Vec<crate::build::workspace::TargetId>,
    /// A program's own styles, once [`Shared::restyle`] has made them.
    pub restyled: Option<std::sync::Arc<crate::compiler::semantics::resolve::Restyled>>,
}

impl Shared {
    /// `target`'s share, where it reports nothing, with its program's own
    /// styles: see [`Checked::restyle`](crate::compiler::semantics::resolve::Checked::restyle).
    /// Made once, for every output of the program. A share whose styles
    /// report anything is not clean, and is built on its own.
    pub fn restyle(&mut self, target: TargetId) -> Option<&Share> {
        let share = self.shares.get_mut(&(target, false))?;
        if share.restyled.is_none() && share.reported.is_empty() {
            let mut reported = Diagnostics::new();
            let restyled = self.analysis.checked.restyle(
                &self.analysis.loaded,
                share.holds.clone(),
                &mut reported,
            )?;
            share.reported = reported.items;
            share.restyled = Some(std::sync::Arc::new(restyled));
        }
        Some(&*share).filter(|s| s.reported.is_empty())
    }

    /// The modules `holds` as one program: style extraction over those
    /// modules alone, as that program's own compilation does it. `None` where
    /// it reports an error. Billed to check, where that compilation does it.
    pub fn restyled(
        &self,
        holds: PerModule<bool>,
    ) -> Option<crate::compiler::semantics::resolve::Restyled> {
        let _phase = crate::profile::enter(crate::profile::Phase::Check);
        let mut reported = Diagnostics::new();
        let restyled =
            self.analysis
                .checked
                .restyle(&self.analysis.loaded, holds, &mut reported)?;
        (!reported.has_errors()).then_some(restyled)
    }

    /// [`Loading::source_bytes`] for the modules `holds`.
    pub fn source_bytes(&self, holds: &PerModule<bool>, map: &SourceMap) -> u64 {
        let modules = self.analysis.loaded.modules.iter();
        source_bytes(modules.filter(|m| *holds.get(m.id)), map)
    }
}

impl Share {
    /// The modules this target's own compilation holds, in load order.
    pub fn modules<'a>(
        &'a self,
        analysis: &'a Analysis,
    ) -> impl Iterator<Item = &'a crate::compiler::modules::ModuleData> + 'a {
        analysis
            .loaded
            .modules
            .iter()
            .filter(|m| *self.holds.get(m.id))
    }

    /// [`Loading::source_bytes`] for this share.
    pub fn source_bytes(&self, analysis: &Analysis, map: &SourceMap) -> u64 {
        source_bytes(self.modules(analysis), map)
    }
}

/// What [`Diagnostics`] deduplicates on.
type Said = (u32, u32, u32, String);

fn said(d: &Diagnostic) -> Said {
    (d.span.file.0, d.span.start, d.span.end, d.message.clone())
}

/// Several units, one per target, loaded and checked as one compilation, so a
/// library twenty targets depend on is checked once rather than twenty times.
///
/// **Each target's share has to be what its own compilation gives**, so this
/// declines, and each target is analysed alone, anywhere that might not hold:
///
/// - **Checking a module does not depend on what else is loaded.** A name
///   resolves through the module's imports, and a method through its
///   receiver's type, whose `impl`s can only be in the type's own module
///   (`impl-outside-type-module`), so in the compilation of anything that can
///   name the type. Each binary's entries are its own entry module's.
/// - **Loading reported nothing but parse errors.** A parse error is a fact
///   about one file. Everything else the loader says can depend on the order
///   modules were reached in, is said once per compilation
///   (a generator's diagnostics), or sits in a build file several targets
///   share.
/// - **Every module was loaded in the role each target would load it in.** The
///   role is decided where a module is first reached, and the import rules
///   read it, so a test source that something also imports could come out
///   differently.
/// - **Every diagnostic lands in a module's file**, which is how it is handed
///   to the targets whose closure holds that module. Every target holds the
///   standard library modules each load opens with, so it gets theirs.
pub fn shared(
    ws: &Workspace,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    units: &[Unit],
    bodies: Checking,
) -> Option<Shared> {
    use crate::compiler::modules::Role;
    // One target's compilation is its own analysis already.
    if units.len() < 2 {
        return None;
    }
    // A target asked about both with and without its tests loads once, with.
    let mut loads: Vec<Unit> = Vec::new();
    for unit in units {
        match loads.iter_mut().find(|u| u.target == unit.target) {
            Some(load) => load.with_tests |= unit.with_tests,
            None => loads.push(unit.clone()),
        }
    }
    let loading = match bodies {
        Checking::All => load_all(Some(ws), map, cache, &loads),
        Checking::Repository => load_programs(Some(ws), map, cache, &loads),
    };
    let loaded = loading.loaded();

    let mut parse_errors: crate::hash::Set<Said> = crate::hash::Set::default();
    for m in &loaded.modules {
        if crate::compiler::standard_library::find(&m.path).is_some() {
            continue;
        }
        let (_, errors) = cache.parse(map.text(m.file), m.file, false);
        parse_errors.extend(errors.iter().map(said));
    }
    if !loading
        .reported()
        .iter()
        .all(|d| parse_errors.contains(&said(d)))
    {
        return None;
    }
    // A module's role, as an import of it would decide it.
    for m in &loaded.modules {
        for path in imports(m) {
            let Some(id) = loaded.find(path) else {
                continue;
            };
            let reached = loaded.module(id);
            if reached.pkg.is_none() {
                continue;
            }
            let test_only = crate::build::workspace::is_test_only_path(path);
            let agrees = match reached.role {
                Role::TestSource => false,
                Role::Source => !test_only,
                Role::TestOnly => test_only,
                Role::Entry | Role::Std | Role::Platform => true,
            };
            if !agrees {
                return None;
            }
        }
    }
    let mut shares = std::collections::BTreeMap::new();
    for unit in units {
        let target = unit.target?;
        let (holds, generated_rules) = reach(ws, loaded, target, unit.with_tests)?;
        shares.insert(
            (target, unit.with_tests),
            Share {
                holds,
                reported: Vec::new(),
                generated_rules,
                restyled: None,
            },
        );
    }

    let analysis = match bodies {
        Checking::All => check_all(loading, Some(ws), map, true),
        Checking::Repository => check_programs(loading, Some(ws), map),
    };
    let mut module_of: crate::hash::Map<FileId, ModuleId> = crate::hash::Map::default();
    for m in &analysis.loaded.modules {
        module_of.insert(m.file, m.id);
    }
    for d in &analysis.diagnostics.items {
        // A spanless diagnostic's file is `FileId::NONE`, which is no module's.
        if !module_of.contains_key(&d.span.file) {
            return None;
        }
    }
    for share in shares.values_mut() {
        share.reported = analysis
            .diagnostics
            .items
            .iter()
            .filter(|d| {
                module_of
                    .get(&d.span.file)
                    .is_some_and(|i| *share.holds.get(*i))
            })
            .cloned()
            .collect();
    }
    Some(Shared {
        analysis: std::sync::Arc::new(analysis),
        shares,
    })
}

/// The module paths a module imports and re-exports, as written.
pub(crate) fn imports(m: &ModuleData) -> impl Iterator<Item = &str> {
    m.ast.items.iter().filter_map(|item| match item {
        crate::parsing::tree::Item::Import(i) => Some(i.path.as_str()),
        crate::parsing::tree::Item::ReExport(r) => Some(r.path.as_str()),
        _ => None,
    })
}

/// The modules one target's own compilation holds, found in a compilation of
/// several, and the rules whose generated code it loaded.
///
/// What `Loader::load_unit` starts from, its test sources only `with_tests`,
/// followed through every import. `None` where one of those starting points
/// wasn't loaded, or was loaded in a role other than the one this target gives
/// it (see [`shared`]).
pub(crate) fn reach(
    ws: &crate::build::workspace::Workspace,
    loaded: &crate::compiler::modules::Loaded,
    target: TargetId,
    with_tests: bool,
) -> Option<(PerModule<bool>, Vec<TargetId>)> {
    use crate::compiler::modules::Role;
    let pkg = ws.package(target.package);
    // Each start, and the role the target loads it in; `None` for the standard
    // library's, whose role is the library's own.
    let mut starts: Vec<(String, Option<Role>)> =
        crate::compiler::standard_library::prelude_modules()
            .chain(crate::compiler::standard_library::eager_modules())
            .map(|path| (path.to_string(), None))
            .collect();
    let rules: Vec<TargetId> = ws
        .closure(target)
        .into_iter()
        .filter(|r| {
            !crate::build::generators::declared(ws, *r).is_empty()
                || crate::build::generators::has_contracts(ws, *r)
        })
        .collect();
    let generated = |starts: &mut Vec<(String, Option<Role>)>| {
        if rules.is_empty() {
            return;
        }
        for m in &loaded.modules {
            if m.disk.is_none()
                && ws
                    .generated
                    .owner(&m.path)
                    .is_some_and(|r| rules.contains(&r))
            {
                starts.push((m.path.clone(), Some(Role::Source)));
            }
        }
    };
    let listed = |starts: &mut Vec<(String, Option<Role>)>,
                  sources: &[crate::build::buildfile::Spanned<String>],
                  role: Role| {
        starts.extend(
            sources
                .iter()
                .map(|s| (pkg.module_path(&s.value), Some(role))),
        );
    };
    match ws.rule(target) {
        Rule::Library(lib) => {
            starts.push((pkg.module_path("lib.buri"), Some(Role::Source)));
            generated(&mut starts);
            listed(&mut starts, &lib.sources, Role::Source);
            if let Some(testing) = &lib.testing {
                starts.push((pkg.module_path("testing/lib.buri"), Some(Role::TestOnly)));
                listed(&mut starts, &testing.sources, Role::TestOnly);
            }
            if let Some(suite) = lib.test.as_ref().filter(|_| with_tests) {
                listed(&mut starts, &suite.sources, Role::TestSource);
            }
        }
        Rule::Binary(bin) => {
            generated(&mut starts);
            for platform in ws.custom_platforms(target) {
                starts.push((platform.label, Some(Role::Source)));
            }
            starts.push((pkg.module_path("main.buri"), Some(Role::Entry)));
            listed(&mut starts, &bin.sources, Role::Source);
            if let Some(suite) = bin.test.as_ref().filter(|_| with_tests) {
                listed(&mut starts, &suite.sources, Role::TestSource);
            }
        }
        Rule::Tool(tool) => {
            generated(&mut starts);
            starts.push((pkg.module_path("tool.buri"), Some(Role::Source)));
            listed(&mut starts, &tool.sources, Role::Source);
            if let Some(suite) = tool.test.as_ref().filter(|_| with_tests) {
                listed(&mut starts, &suite.sources, Role::TestSource);
            }
        }
    }

    let mut holds = loaded.modules.per_module(false);
    let mut queue: Vec<ModuleId> = Vec::new();
    for (path, role) in &starts {
        let id = loaded.find(path)?;
        if role.is_some_and(|role| loaded.module(id).role != role) {
            return None;
        }
        queue.push(id);
    }
    while let Some(id) = queue.pop() {
        let seen = holds.get_mut(id);
        if std::mem::replace(seen, true) {
            continue;
        }
        let module = loaded.module(id);
        // As the loader does: a module of the program's own that imports from
        // `ui/` loads `classify` too.
        let own = !matches!(module.role, Role::Std | Role::Platform);
        let classify = (own && imports(module).any(|path| path.starts_with("ui/")))
            .then(|| loaded.find(crate::compiler::standard_library::CLASSIFY_MODULE))
            .flatten();
        queue.extend(
            imports(module)
                .filter_map(|path| loaded.find(path))
                .chain(classify),
        );
    }
    Some((holds, rules))
}

/// Every file in the closure that the standard library did not supply.
///
/// By module path against the library's own table rather than by
/// [`Role`](crate::compiler::modules::Role): a documentation snippet is loaded
/// as `Role::Std` so that it may show a signature with no body, and a snippet
/// is the one thing a doc harness is asking about.
fn repository_files(loaded: &Loaded) -> Vec<FileId> {
    loaded
        .modules
        .iter()
        .filter(|m| crate::compiler::standard_library::find(&m.path).is_none())
        .map(|m| m.file)
        .collect()
}

/// Loads and checks one unit, but type-checks only the bodies written in
/// `files`.
///
/// Everything one body can see of another is still elaborated for the whole
/// closure: every signature, type definition, trait, impl, module-level `let`
/// and `context` declaration. What is left out is step 5 for the files nobody
/// asked about, and `Checked::bodies` simply has no entry for those.
///
/// This is what an editor query wants. Hover, definition, completion, the
/// tokens, the hints and the highlights all read the bodies of the file under
/// the cursor and filter every other one out by file id, so checking the rest
/// of the repository was work whose whole result was discarded. What comes
/// back here is byte-identical for those files — `tests/language/
/// scoped_bodies.rs` asserts exactly that over every fixture repository — and
/// the diagnostics are the signature phase's, for every module, plus the body
/// phase's for these files and no others.
///
/// **Not** for publishing diagnostics, and not for any question about a name:
/// a file that was not asked about reports nothing here, which is right for a
/// hover and wrong for a problem list.
pub fn analyze_bodies_in(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    unit: &Unit,
    files: &[FileId],
) -> Analysis {
    analyze_on(
        ws,
        map,
        cache,
        Opening::Builtin,
        Scope::Files(files),
        |loader| loader.load_unit(unit),
    )
}

/// Loads and checks every module of the standard library, with no repository.
/// This is what `buri version --self-check` runs, and what the toolchain's own
/// tests use.
pub fn analyze_stdlib(map: &mut SourceMap) -> Analysis {
    let mut diags = Diagnostics::new();
    let mut cache = crate::parsing::parser::Cache::new();
    let loaded = {
        let mut loader = Loader::new(None, map, &mut diags, &mut cache);
        loader.load_all_std();
        loader.finish()
    };
    let checked = Checker::new(&loaded, None, &mut diags).run();
    diags.sort(map);
    Analysis {
        loaded,
        checked,
        diagnostics: diags,
    }
}

/// Loads every module of the standard library, as [`analyze_stdlib`] does,
/// and checks none of it.
///
/// For a reader of declarations rather than of types: `buri docs` renders a
/// module's page from its syntax tree, and checking forty thousand lines to
/// print one of them was most of what the command cost.
pub fn load_stdlib(map: &mut SourceMap) -> Loaded {
    let mut diags = Diagnostics::new();
    let mut cache = crate::parsing::parser::Cache::new();
    let mut loader = Loader::new(None, map, &mut diags, &mut cache);
    loader.load_all_std();
    loader.finish()
}

/// Loads one unit, as [`analyze`] does, and checks none of it. What
/// [`load_stdlib`] is for the standard library, for a repository's target.
pub fn load(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    unit: &Unit,
) -> Loaded {
    let mut diags = Diagnostics::new();
    let mut loader = Loader::new(ws, map, &mut diags, cache);
    loader.load_unit(unit);
    loader.finish()
}

/// Loads and checks one standard library module the way a *program* would
/// reach it: on top of the built-in types and whatever it imports for itself,
/// and nothing else.
///
/// `analyze_stdlib` loads every module together, so it cannot notice one that
/// only checks because something else happened to be present. Since the
/// standard library loads lazily, that is exactly the mistake worth catching:
/// a module is first seen in a compilation holding the eager set and its own
/// imports.
pub fn analyze_std_module(map: &mut SourceMap, path: &str) -> Analysis {
    let mut cache = crate::parsing::parser::Cache::new();
    analyze_on(
        None,
        map,
        &mut cache,
        Opening::Builtin,
        Scope::All,
        |loader| {
            loader.load_builtin_modules();
            loader.load_std_module(path);
        },
    )
}

/// Loads and checks one module given as text, on top of the whole standard
/// library and with no repository.
///
/// This is the documentation harness's entry point. It is deliberately the
/// same `Loader` and the same `Checker` the compiler runs, because a doctest
/// that passed against a simplified pipeline would prove nothing about the
/// example a reader is about to copy.
pub fn analyze_snippet(
    map: &mut SourceMap,
    name: &str,
    text: &str,
    role: crate::compiler::modules::Role,
) -> Analysis {
    let mut cache = crate::parsing::parser::Cache::new();
    analyze_snippet_on(None, None, map, &mut cache, name, text, role, None)
}

/// The same, against a repository, standing in for a file of `pkg`, and built
/// for one platform. Each is optional.
///
/// - The repository is what lets a snippet that imports `//lib/money` resolve
///   it. The build system's documentation is mostly *about* a monorepo, and
///   compiling its examples against the worked example repository is what makes
///   them testable instead of illustrative.
/// - Standing in for a file of `pkg` is what makes a document about a library's
///   internals compilable.
/// - A snippet has no output, so by default its `main` may take any bundled
///   platform's host. A document *about* the host says which with `platform=`
///   on its fence: that is what lets the error page for `entry-host-mismatch`
///   carry a program that actually provokes it.
#[allow(
    clippy::too_many_arguments,
    reason = "bundling them into a struct would give every caller a builder to fill in for \
              the one field it varies, and they are what a snippet *is*: where it stands, \
              what it is called, what it says, and what it is compiled as."
)]
pub fn analyze_snippet_on(
    ws: Option<&Workspace>,
    pkg: Option<crate::build::workspace::PackageId>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    name: &str,
    text: &str,
    role: crate::compiler::modules::Role,
    platform: Option<Platform>,
) -> Analysis {
    analyze_on(ws, map, cache, Opening::Library, Scope::All, |loader| {
        loader.load_unit(&crate::compiler::modules::Unit {
            target: None,
            platform,
            entry: None,
            with_tests: false,
        });
        loader.load_all_std();
        loader.load_source_in(name, role, text.to_string(), pkg);
    })
}

/// Compiles a snippet that exports `main`, runs it, and returns its standard
/// output. A repository lets a documented program import the packages the
/// document is about.
///
/// The tail of `actions::build_target` minus the cache and the artifact
/// directory — so a documented program is executed exactly the way `buri run`
/// would execute it.
pub fn run_snippet(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    name: &str,
    text: &str,
) -> Result<String, Diagnostics> {
    let (source, chunks) = compile_snippet_js_as(ws, None, map, name, text)?;
    execute(name, &source, &chunks)
}

/// The same up to running it: the JavaScript a snippet that exports `main`
/// compiles to, with the snippet standing in for a file of `pkg`.
///
/// [`run_snippet`] is this and then a subprocess.
pub fn compile_snippet_js_as(
    ws: Option<&Workspace>,
    pkg: Option<crate::build::workspace::PackageId>,
    map: &mut SourceMap,
    name: &str,
    text: &str,
) -> Result<(String, Vec<String>), Diagnostics> {
    let (mut program, analysis) = monomorphize_snippet(ws, pkg, map, name, text, None)?;
    let mut diags = Diagnostics::new();
    let flags = crate::commands::arguments::Flags::default();
    actions::emit_all(
        &mut program,
        &analysis.checked.tables,
        crate::compiler::backend::Target {
            platform: crate::build::buildfile::Platform::Js,
            arch: None,
        },
        &flags,
        &mut diags,
    )
}

/// The program a snippet that exports `main` monomorphizes to, checked for
/// `platform` with the snippet standing in for a file of `pkg`, and the
/// analysis whose tables it reads.
///
/// `build::tools` calls it too: the `main` the toolchain writes for a `tool`
/// rule is a snippet, and it imports that tool's `tool.buri`, which only a file
/// of its own package may. This is the program the build links into the
/// executable it hands a request on standard input.
pub fn monomorphize_snippet(
    ws: Option<&Workspace>,
    pkg: Option<crate::build::workspace::PackageId>,
    map: &mut SourceMap,
    name: &str,
    text: &str,
    platform: Option<Platform>,
) -> Result<(crate::compiler::middle::monomorphize::Program, Analysis), Diagnostics> {
    let mut cache = crate::parsing::parser::Cache::new();
    // `main` is the entry this program is built through, so it answers to
    // `platform`'s host whatever outputs `pkg`'s build file names.
    let analysis = analyze_on(
        ws,
        map,
        &mut cache,
        Opening::Library,
        Scope::All,
        |loader| {
            loader.load_unit(&crate::compiler::modules::Unit {
                target: None,
                platform,
                entry: platform.map(|_| "main".to_string()),
                with_tests: false,
            });
            loader.load_all_std();
            loader.load_source_in(
                name,
                crate::compiler::modules::Role::Entry,
                text.to_string(),
                pkg,
            );
        },
    );
    if analysis.diagnostics.has_errors() {
        return Err(analysis.diagnostics);
    }
    let mut diags = Diagnostics::new();
    let Some(entry) = analysis.started(name, "main") else {
        diags.push(Diagnostic::templated("example-missing-main", Span::NONE));
        return Err(diags);
    };
    let module_paths: Vec<String> = analysis
        .loaded
        .modules
        .iter()
        .map(|m| m.path.clone())
        .collect();
    let program = crate::compiler::middle::monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diags,
        crate::compiler::middle::monomorphize::Roots::Main(entry),
    );
    if diags.has_errors() {
        return Err(diags);
    }
    Ok((program, analysis))
}

/// Writes the emitted module to a scratch file and runs it under the JS
/// runtime, because an ES module has to come from a file to be imported.
///
/// A `core/lazy` chunk goes beside it under the name the module will ask for,
/// for the same reason: an example that splits itself has to be able to find
/// its own halves.
fn execute(name: &str, source: &str, chunks: &[String]) -> Result<String, Diagnostics> {
    use std::process::Command;
    let fail = |msg: String, fix: &str| {
        let mut d = Diagnostics::new();
        d.push(Diagnostic::error(Span::NONE, msg).with_fix(fix.to_string()));
        d
    };
    // The file name has to be unique across concurrently running tests, and
    // derived from the snippet so a rerun overwrites rather than accumulates.
    // A digest of the name rather than the name: a page deep in a repository
    // has an origin longer than a file name may be. The process id too: the
    // stem is unique within one run, but two toolchain processes testing the
    // same snippet would otherwise overwrite each other's file mid-execution.
    let stem = crate::build::cache::ActionKey::of(name.as_bytes());
    let path = std::env::temp_dir().join(format!(
        "buri-doctest-{}-{}.mjs",
        std::process::id(),
        stem.as_str()
    ));
    let written = actions::chunk_paths(&path, chunks);
    let chunks = written.iter().map(|(at, text)| (at, text.as_str()));
    for (at, text) in std::iter::once((&path, source)).chain(chunks) {
        if let Err(e) = std::fs::write(at, text) {
            return Err(fail(
                format!("cannot create {}: {e}", at.display()),
                "check TMPDIR",
            ));
        }
    }
    let out = match crate::build::spawn::output(
        Command::new(crate::commands::test::js_runtime()).arg(&path),
    ) {
        Ok(o) => o,
        Err(e) => {
            return Err(fail(
                format!("cannot run {}: {e}", crate::commands::test::js_runtime()),
                "install bun, or set BURI_JS to a JavaScript runtime",
            ))
        }
    };
    let _ = std::fs::remove_file(&path);
    for (at, _) in &written {
        let _ = std::fs::remove_file(at);
    }
    if !out.status.success() {
        return Err(fail(
            format!(
                "the example exited {}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            "fix the example, or change the expected output beneath it",
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub use crate::build::buildfile::host_native_platform;
