//! What the server knows between requests.
//!
//! Three things: where it is in the protocol's lifecycle, the buffers the
//! editor has open and may have edited since they were last saved, and the
//! analyses those two produced.
//!
//! There is no incremental engine here, and the honest reason is that there is
//! not one in the compiler either — `driver::analyze` is whole-closure. What
//! makes that liveable is the scheduling in `mod.rs`, and now also the
//! [`Cache`]: an answer is kept under a hash of every byte it was computed
//! from, so a second question about an unchanged repository is a lookup.

use crate::build::session::{self, Session};
use crate::build::workspace::TargetId;
use crate::commands::arguments::Flags;
use crate::compiler::driver::Analysis;
use crate::compiler::modules::Unit;
use crate::json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Where the server is in the protocol's lifecycle.
///
/// The protocol has four states and this used to be a `bool`. `handle` set it
/// for `shutdown` *and* for `exit`, so the loop had to re-read the method
/// string to tell which of the two had happened; `exit` with no `shutdown`
/// before it, and `initialize` after a `shutdown`, were both representable and
/// both handled by accident. Worst of the three, a request arriving before
/// `initialize` was answered — against whatever directory the process happened
/// to be started in, because no client had said yet where the repository is.
///
/// Naming the four states makes every one of those an ordinary rejection.
pub enum Lifecycle {
    /// Nothing has been agreed yet. Only `initialize` is accepted.
    New,
    /// `initialize` has been answered, and the process working directory is
    /// the root the client named. The root is not kept here as well: the rest
    /// of the toolchain finds the repository from the working directory, and a
    /// second copy of it would be a second thing that could be wrong.
    Running,
    /// `shutdown` has been answered. Only `exit` is accepted now.
    ShuttingDown,
    /// `exit` has arrived and the loop is over. `orderly` is false when it
    /// came without a `shutdown` before it, which the protocol says is a
    /// non-zero exit.
    Exited { orderly: bool },
}

impl Lifecycle {
    /// `Err` with the reason when `initialize` arrives out of order.
    pub fn initialize(&mut self) -> Result<(), &'static str> {
        match self {
            Lifecycle::New => {
                *self = Lifecycle::Running;
                Ok(())
            }
            _ => Err("the server is already initialized"),
        }
    }

    /// `Err` with the reason when `shutdown` arrives out of order.
    pub fn shutdown(&mut self) -> Result<(), &'static str> {
        match self {
            Lifecycle::New => Err("the server has not been initialized"),
            Lifecycle::Running => {
                *self = Lifecycle::ShuttingDown;
                Ok(())
            }
            Lifecycle::ShuttingDown | Lifecycle::Exited { .. } => {
                Err("the server is already shutting down")
            }
        }
    }

    /// `exit` is never refused — the client is going away either way. What it
    /// records is whether a `shutdown` came first.
    pub fn exit(&mut self) {
        let orderly = matches!(self, Lifecycle::ShuttingDown);
        *self = Lifecycle::Exited { orderly };
    }

    /// Whether the server may answer ordinary requests: after `initialize`,
    /// and before `shutdown`.
    pub fn is_running(&self) -> bool {
        matches!(self, Lifecycle::Running)
    }

    /// The process exit code, once `exit` has arrived.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Lifecycle::Exited { orderly } => Some(if *orderly { 0 } else { 1 }),
            _ => None,
        }
    }
}

pub struct State {
    pub lifecycle: Lifecycle,
    /// Open buffers, by absolute path. The editor's copy wins over the disk's
    /// for as long as the file is open — including after a save, where they
    /// agree anyway.
    pub open: BTreeMap<PathBuf, String>,
    /// What the last whole analysis said about each file, by URI.
    ///
    /// Kept because a keystroke publishes parse errors and a publish replaces
    /// everything the editor is showing for that file: without somewhere to
    /// put the type errors back, the first character typed erased them and
    /// only a save brought them back.
    pub published: BTreeMap<String, Vec<Value>>,
    /// The files whose last publish came from the parse rather than from the
    /// analysis. Only those need clearing when the buffer parses again.
    pub showing_parse_errors: BTreeSet<String>,
    /// Analyses, under a hash of everything they were computed from.
    cache: Cache,
}

pub struct Analyzed {
    pub session: Session,
    pub analysis: Analysis,
}

/// What `buri lint` found, and the session whose source map its spans point
/// into. The two travel together because neither means anything alone.
pub struct Linted {
    pub session: Session,
    pub diagnostics: crate::diagnostics::Diagnostics,
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// Everything computed from one state of the repository.
///
/// The key is a hash of every input an analysis reads, so a generation is
/// valid entirely or not at all. That coarseness is the design and not a
/// shortcut: the front end is whole-closure, so there is no smaller unit whose
/// answer an edit elsewhere provably does not change, and a per-target scheme
/// would have to prove exactly that.
#[derive(Default)]
struct Generation {
    fingerprint: u64,
    /// Keyed by the target owning the file rather than by the file, because
    /// that is what the answer depends on — every source in one target shares
    /// an entry.
    targets: Vec<(Option<TargetId>, Rc<Analyzed>)>,
    /// The whole-repository analysis, which has no target to key on.
    whole: Option<Rc<Analyzed>>,
    lints: Vec<(TargetId, Rc<Linted>)>,
}

/// How many fingerprints are kept at once. Two: the one being asked about, and
/// the one before it, so that a keystroke and its undo both land on a hit.
const GENERATIONS: usize = 2;

/// How many targets one generation holds an analysis for. A person edits in a
/// handful of files at a time, and an `Analysis` carries the whole closure —
/// keeping one per target in a large repository is how a server ends up
/// holding the repository several times over.
const PER_GENERATION: usize = 4;

#[derive(Default)]
pub struct Cache {
    /// Oldest first, so eviction is from the front.
    generations: Vec<Generation>,
}

impl Cache {
    fn find(&self, fingerprint: u64) -> Option<&Generation> {
        self.generations.iter().find(|g| g.fingerprint == fingerprint)
    }

    /// The generation for this fingerprint, made if it is new — evicting the
    /// oldest to keep at most [`GENERATIONS`] of them.
    fn generation(&mut self, fingerprint: u64) -> Option<&mut Generation> {
        if !self.generations.iter().any(|g| g.fingerprint == fingerprint) {
            if self.generations.len() >= GENERATIONS {
                self.generations.remove(0);
            }
            self.generations.push(Generation { fingerprint, ..Generation::default() });
        }
        self.generations.iter_mut().find(|g| g.fingerprint == fingerprint)
    }
}

impl Generation {
    /// Any session this generation holds, for the questions that need the
    /// build graph and nothing else — which target owns a file, above all.
    ///
    /// Every session in a generation was opened from the same bytes, so which
    /// one it is does not matter.
    fn any_session(&self) -> Option<&Session> {
        if let Some((_, analyzed)) = self.targets.first() {
            return Some(&analyzed.session);
        }
        if let Some(analyzed) = &self.whole {
            return Some(&analyzed.session);
        }
        self.lints.first().map(|(_, linted)| &linted.session)
    }
}

/// Pushes onto a list that keeps only its last [`PER_GENERATION`] entries.
fn remember<K: PartialEq, V>(list: &mut Vec<(K, V)>, key: K, value: V) {
    if let Some(slot) = list.iter_mut().find(|(k, _)| *k == key) {
        slot.1 = value;
        return;
    }
    if list.len() >= PER_GENERATION {
        list.remove(0);
    }
    list.push((key, value));
}

impl State {
    pub fn new() -> State {
        State {
            lifecycle: Lifecycle::New,
            open: BTreeMap::new(),
            published: BTreeMap::new(),
            showing_parse_errors: BTreeSet::new(),
            cache: Cache::default(),
        }
    }

    /// A hash of every byte that feeds an analysis.
    ///
    /// That is every `.buri` file in the repository — sources, every
    /// `BUILD.buri`, and `REPO.buri`, which all wear the one extension — plus
    /// the open buffers layered over them, because the editor's copy is what
    /// the analysis actually reads. The standard library is not in it: it is
    /// compiled into this binary and cannot change while the server runs.
    ///
    /// Reading and hashing bytes is not parsing them, which is the whole
    /// point — this is paid on every request and the analysis it replaces is
    /// orders more expensive.
    ///
    /// `None` when there is no repository to fingerprint, and then nothing is
    /// cached: an answer with no key is not one worth keeping.
    fn fingerprint(&self) -> Option<u64> {
        let cwd = std::env::current_dir().ok()?;
        let root = crate::build::workspace::find_root(&cwd)?;
        let mut files = Vec::new();
        crate::commands::format::collect(&root, &mut files);
        // `read_dir` order is the filesystem's, and the hash must not be.
        files.sort();

        let mut hasher = crate::hash::FxHasher::default();
        hasher.write(root.as_os_str().as_encoded_bytes());
        for path in &files {
            hasher.write(path.as_os_str().as_encoded_bytes());
            match std::fs::read(path) {
                Ok(bytes) => hasher.write(&bytes),
                // A file that cannot be read still has to move the hash when
                // it appears or goes away.
                Err(_) => hasher.write_u8(0),
            }
        }
        // The overlay, in the map's order so that which file was opened first
        // is not part of the answer.
        for (path, text) in &self.open {
            hasher.write(path.as_os_str().as_encoded_bytes());
            hasher.write(text.as_bytes());
        }
        Some(hasher.finish())
    }

    /// The buffer's text if it is open, otherwise the file on disk.
    pub fn text_of(&self, path: &Path) -> Option<String> {
        if let Some(t) = self.open.get(path) {
            return Some(t.clone());
        }
        std::fs::read_to_string(path).ok()
    }

    /// The build graph on its own, with no analysis behind it.
    ///
    /// A question about a `BUILD.buri` is a question about the graph: what a
    /// label names and where a package's file is are both answered by loading
    /// the repository, and the front end has nothing to add.
    pub fn session(&self) -> Option<Session> {
        session::open(&Flags::default()).ok()
    }

    /// Runs the whole front end for the target owning `path`.
    ///
    /// The overlay trick is `SourceMap::load` reusing an entry whose name
    /// already exists (`diagnostics.rs`): seeding the map with each open buffer under
    /// the name the loader will ask for means the loader never reaches the
    /// disk, and `compiler::modules` needs no notion of an editor at all.
    ///
    /// Shared rather than returned by value, and keyed rather than stashed.
    /// The field this used to be written to was documented as a cache and was
    /// not one: it carried no key saying which file or which revision produced
    /// it, and the method overwrote it unconditionally on every call. The key
    /// is [`State::fingerprint`], and it is what makes reuse a claim rather
    /// than a hope.
    pub fn analyze(&mut self, path: &Path) -> Option<Rc<Analyzed>> {
        let fingerprint = self.fingerprint();
        if let Some(fingerprint) = fingerprint {
            if let Some(hit) = self.cached_analysis(fingerprint, path) {
                return Some(hit);
            }
        }

        let mut session = self.overlaid_session()?;
        let target = target_for(&session, path);
        let unit = Unit {
            target,
            // The editor is not building an output, and a file open in it
            // belongs to every output its target declares. See `Unit::platform`.
            platform: None,
            // A test source is a source an editor opens like any other, so the
            // server would be blind in exactly the files most often edited.
            with_tests: true,
        };
        let analysis = crate::compiler::driver::analyze(
            Some(&session.workspace),
            &mut session.map,
            &mut session.parsed,
            &unit,
        );
        let analyzed = Rc::new(Analyzed { session, analysis });
        if let Some(generation) = fingerprint.and_then(|f| self.cache.generation(f)) {
            remember(&mut generation.targets, target, Rc::clone(&analyzed));
        }
        Some(analyzed)
    }

    /// Runs the whole front end for *every* target in the repository, as one
    /// compilation.
    ///
    /// [`State::analyze`] answers a question about one file, and the target
    /// owning it is enough for that. A question about a *name* is not: a
    /// function is referred to from wherever it is imported, which is a set of
    /// targets nothing about the file it was declared in narrows down. So the
    /// answer has to be looked for everywhere, and `driver::analyze_all` is one
    /// loader and one checker over the lot — a module two targets both reach is
    /// parsed once and gets one id, which is what makes a single scan of the
    /// result well defined.
    ///
    /// It was paid per request. It is now paid per *edit*: the key is a hash
    /// of every file the analysis reads, so `references`, `rename` and
    /// `workspace/symbol` asked one after another about an untouched
    /// repository run this once between them.
    pub fn analyze_workspace(&mut self) -> Option<Rc<Analyzed>> {
        let fingerprint = self.fingerprint();
        if let Some(hit) =
            fingerprint.and_then(|f| self.cache.find(f)).and_then(|g| g.whole.as_ref())
        {
            return Some(Rc::clone(hit));
        }

        let mut session = self.overlaid_session()?;
        let units: Vec<Unit> = session
            .workspace
            .targets()
            .into_iter()
            .map(|target| Unit { target: Some(target), platform: None, with_tests: true })
            .collect();
        let analysis = crate::compiler::driver::analyze_all(
            Some(&session.workspace),
            &mut session.map,
            &mut session.parsed,
            &units,
        );
        let analyzed = Rc::new(Analyzed { session, analysis });
        if let Some(generation) = fingerprint.and_then(|f| self.cache.generation(f)) {
            generation.whole = Some(Rc::clone(&analyzed));
        }
        Some(analyzed)
    }

    /// The findings `buri lint` reports for the target owning `path`.
    ///
    /// A separate session and a second whole-closure analysis, because the
    /// checks build their own. That is a real cost, and it is why this ran on
    /// save and on open rather than on a keystroke — the same reason
    /// `driver::analyze` did. It is cached under the same key as the analysis
    /// now, so the second question about one repository state does not pay it
    /// at all.
    ///
    /// Shared, and therefore read-only. A caller that must write to the
    /// session wants [`State::overlaid_session`].
    pub fn lint(&mut self, path: &Path) -> Option<Rc<Linted>> {
        let fingerprint = self.fingerprint();
        if let Some(fingerprint) = fingerprint {
            if let Some(hit) = self.cached_lint(fingerprint, path) {
                return Some(hit);
            }
        }

        let mut session = session::open(&Flags::default()).ok()?;
        if session.diagnostics.has_errors() {
            return None;
        }
        for (p, text) in &self.open {
            let rel = session.workspace.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }
        let target = target_for(&session, path)?;
        let diagnostics = crate::commands::lint::findings_for(&mut session, &[target]);
        let linted = Rc::new(Linted { session, diagnostics });
        if let Some(generation) = fingerprint.and_then(|f| self.cache.generation(f)) {
            remember(&mut generation.lints, target, Rc::clone(&linted));
        }
        Some(linted)
    }

    /// A session with the open buffers layered over the disk, and nothing
    /// analysed in it yet.
    ///
    /// The overlay trick is `SourceMap::load` reusing an entry whose name
    /// already exists (`diagnostics.rs`): seeding the map with each open buffer
    /// under the name the loader will ask for means the loader never reaches
    /// the disk, and `compiler::modules` needs no notion of an editor at all.
    ///
    /// Public because `buri gen` writes through the session it is handed, and
    /// a cached one is shared by everything holding an `Rc` to it.
    pub fn overlaid_session(&self) -> Option<Session> {
        let mut session = session::open(&Flags::default()).ok()?;
        for (p, text) in &self.open {
            let rel = session.workspace.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }
        Some(session)
    }

    /// The analysis for the target owning `path`, if this fingerprint has one.
    ///
    /// Which target that is takes a loaded build graph, and a generation that
    /// holds anything already has one — so a hit costs no `session::open`.
    fn cached_analysis(&self, fingerprint: u64, path: &Path) -> Option<Rc<Analyzed>> {
        let generation = self.cache.find(fingerprint)?;
        let target = target_for(generation.any_session()?, path);
        generation.targets.iter().find(|(t, _)| *t == target).map(|(_, a)| Rc::clone(a))
    }

    /// The same lookup for the lint pass.
    fn cached_lint(&self, fingerprint: u64, path: &Path) -> Option<Rc<Linted>> {
        let generation = self.cache.find(fingerprint)?;
        let target = target_for(generation.any_session()?, path)?;
        generation.lints.iter().find(|(t, _)| *t == target).map(|(_, l)| Rc::clone(l))
    }
}

/// The target whose closure contains `path`.
///
/// A package holds a library and a binary at once, and their sources are
/// disjoint: `main.buri` and the binary's test suite are in the binary's
/// closure and in nothing else. Taking the package's first target analysed
/// the library for every file in the package, so a binary entry point and
/// its tests were never loaded at all — and a file no analysis reached has
/// no hover, no definition and no diagnostics, which is how a whole file
/// can look like the server is not running.
///
/// So the rule that decides which target owns a file is the build file's,
/// [`Workspace::rule_of_file`] — the same one `buri build` uses. The first
/// target is still the answer for a file the build file does not list,
/// which is a file being written before it is declared.
///
/// A free function rather than a method: it reads the build graph and the
/// path, and nothing else about the server — which is what lets a cache hit
/// resolve a target from a session it is holding rather than opening one.
fn target_for(session: &Session, path: &Path) -> Option<TargetId> {
    let package = session.workspace.owning_package(path)?;
    let targets = session.workspace.targets();
    let declared = path
        .strip_prefix(&session.workspace.package(package).dir)
        .ok()
        .map(|rel| rel.display().to_string().replace('\\', "/"))
        .and_then(|rel| session.workspace.rule_of_file(package, &rel))
        .map(|kind| TargetId { package, kind })
        .filter(|t| targets.contains(t));
    declared.or_else(|| targets.into_iter().find(|t| t.package == package))
}
