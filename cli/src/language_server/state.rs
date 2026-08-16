//! What the server knows between requests.
//!
//! Two things: the buffers the editor has open and may have edited since they
//! were last saved, and the most recent whole-repository analysis.
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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct State {
    pub root: PathBuf,
    /// Open buffers, by absolute path. The editor's copy wins over the disk's
    /// for as long as the file is open — including after a save, where they
    /// agree anyway.
    pub open: BTreeMap<PathBuf, String>,
    /// The last full analysis, and the session it borrowed its spans from.
    pub analysis: Option<Analyzed>,
    pub shutdown: bool,
}

pub struct Analyzed {
    pub session: Session,
    pub analysis: Analysis,
}

impl State {
    pub fn new(root: PathBuf) -> State {
        State { root, open: BTreeMap::new(), analysis: None, shutdown: false }
    }

    /// The buffer's text if it is open, otherwise the file on disk.
    pub fn text_of(&self, path: &Path) -> Option<String> {
        if let Some(t) = self.open.get(path) {
            return Some(t.clone());
        }
        std::fs::read_to_string(path).ok()
    }

    /// Runs the whole front end for the target owning `path`.
    ///
    /// The overlay trick is `SourceMap::load` reusing an entry whose name
    /// already exists (`diagnostics.rs`): seeding the map with each open buffer under
    /// the name the loader will ask for means the loader never reaches the
    /// disk, and `compiler::modules` needs no notion of an editor at all.
    pub fn analyze(&mut self, path: &Path) -> Option<&Analyzed> {
        let mut session = session::open(&Flags::default()).ok()?;
        for (p, text) in &self.open {
            let rel = session.ws.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }

        let target = self.target_for(&session, path);
        let unit = Unit {
            target,
            platform: crate::compiler::driver::host_platform(),
            // A test source is a source an editor opens like any other, so the
            // server would be blind in exactly the files most often edited.
            with_tests: true,
        };
        let analysis = crate::compiler::driver::analyze(Some(&session.ws), &mut session.map, &unit);
        self.analysis = Some(Analyzed { session, analysis });
        self.analysis.as_ref()
    }

    /// The findings `buri lint` reports for the target owning `path`.
    ///
    /// A separate session and a second whole-closure analysis, because the
    /// checks build their own. That is a real cost and it is why this runs on
    /// save and on open rather than on a keystroke — the same reason
    /// `driver::analyze` does.
    pub fn lint(&self, path: &Path) -> Option<(Session, crate::diagnostics::Diagnostics)> {
        let mut session = session::open(&Flags::default()).ok()?;
        if session.diags.has_errors() {
            return None;
        }
        for (p, text) in &self.open {
            let rel = session.ws.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }
        let target = self.target_for(&session, path)?;
        let diags = crate::commands::lint::findings_for(&mut session, &[target]);
        Some((session, diags))
    }

    /// The target whose closure contains `path`. A file can belong to a library
    /// and a binary in the same package; either analysis sees the file, so the
    /// first is as good as the second.
    fn target_for(&self, session: &Session, path: &Path) -> Option<TargetId> {
        let pkg = session.ws.owning_package(path)?;
        session.ws.targets().into_iter().find(|t| t.pkg == pkg)
    }
}
