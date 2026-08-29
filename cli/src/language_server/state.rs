//! What the server knows between requests.
//!
//! Two things: where it is in the protocol's lifecycle, and the buffers the
//! editor has open and may have edited since they were last saved. Not the
//! analysis — that is recomputed and returned by value, because it was never
//! keyed and so was never a cache.
//!
//! There is no incremental engine here, and the honest reason is that there is
//! not one in the compiler either — `driver::analyze` is whole-closure. What
//! makes that liveable is the scheduling in `mod.rs`: a keystroke re-parses one
//! buffer, and only a save pays for the analysis.

use crate::build::session::{self, Session};
use crate::build::workspace::TargetId;
use crate::commands::arguments::Flags;
use crate::compiler::driver::Analysis;
use crate::compiler::modules::Unit;
use crate::json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
}

pub struct Analyzed {
    pub session: Session,
    pub analysis: Analysis,
}

impl State {
    pub fn new() -> State {
        State {
            lifecycle: Lifecycle::New,
            open: BTreeMap::new(),
            published: BTreeMap::new(),
            showing_parse_errors: BTreeSet::new(),
        }
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
    /// Returned by value rather than stashed in a field. The field it used to
    /// be written to was documented as a cache and was not one: it carried no
    /// key saying which file or which revision produced it, and this method
    /// overwrote it unconditionally on every call. What it actually did was
    /// give the result somewhere to live long enough to be borrowed from, which
    /// the caller can do for itself.
    pub fn analyze(&self, path: &Path) -> Option<Analyzed> {
        let mut session = session::open(&Flags::default()).ok()?;
        for (p, text) in &self.open {
            let rel = session.workspace.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }

        let target = self.target_for(&session, path);
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
        Some(Analyzed { session, analysis })
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
    /// It is paid per request, with no cache. A cache here would need a key
    /// saying which files and which revisions produced it, and the last field
    /// that claimed to be one had neither.
    pub fn analyze_workspace(&self) -> Option<Analyzed> {
        let mut session = session::open(&Flags::default()).ok()?;
        for (p, text) in &self.open {
            let rel = session.workspace.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }
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
        Some(Analyzed { session, analysis })
    }

    /// The findings `buri lint` reports for the target owning `path`.
    ///
    /// A separate session and a second whole-closure analysis, because the
    /// checks build their own. That is a real cost and it is why this runs on
    /// save and on open rather than on a keystroke — the same reason
    /// `driver::analyze` does.
    pub fn lint(&self, path: &Path) -> Option<(Session, crate::diagnostics::Diagnostics)> {
        let mut session = session::open(&Flags::default()).ok()?;
        if session.diagnostics.has_errors() {
            return None;
        }
        for (p, text) in &self.open {
            let rel = session.workspace.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }
        let target = self.target_for(&session, path)?;
        let diagnostics = crate::commands::lint::findings_for(&mut session, &[target]);
        Some((session, diagnostics))
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
    fn target_for(&self, session: &Session, path: &Path) -> Option<TargetId> {
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
}
