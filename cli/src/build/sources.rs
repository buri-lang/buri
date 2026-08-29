//! One repository, opened once and kept.
//!
//! [`session::open_at`] reads `REPO.buri` and every `BUILD.buri`, and the
//! [`Session`] it hands back reads and parses each source the first time an
//! analysis asks for it. A command that opens the repository once and then
//! analyses every target in it therefore pays for a file once. A command that
//! opens it again — the language server, once per request; `buri test --watch`,
//! once per pass — pays again for every file, whether or not anything moved.
//! That was 19 % of a language-server pull at every repository size.
//!
//! What stopped a session from outliving one question is that a
//! [`parser::Cache`](crate::parsing::parser::Cache) is keyed on
//! [`FileId`], and a `FileId` used to stand for one revision of a file: nothing
//! could put new text under an old id, so a kept session would serve the text a
//! file held before the edit. So the id stands for the *file* now
//! ([`SourceMap::replace`]), and what says a revision moved lives here: a hash
//! of the bytes behind each id, compared before the session is handed out, and
//! the parse dropped for the ids that moved.
//!
//! Everything in this file is keyed on bytes rather than on a clock, which is
//! also what makes it storable: writing `held` and the parses beside the build
//! cache would make the first analysis of a fresh process a lookup. Nothing
//! here reads a modification time or a request counter.

use crate::build::session::{self, Session};
use crate::commands::arguments::Flags;
use crate::diagnostics::FileId;
use std::collections::BTreeMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The editor's unsaved copies of files, layered over the disk.
///
/// Empty at the terminal, where the disk holds the only copy there is. The
/// language server's is the buffers the client has open.
pub type Overlay = BTreeMap<PathBuf, String>;

/// Every `.buri` file under one root, and a hash of their names alone.
///
/// The names are a key of their own: a file appearing or going away changes
/// what the build graph and the lint pass see without changing the bytes of
/// any file that already existed.
pub struct Listing {
    pub files: Vec<PathBuf>,
    pub names: u64,
}

/// What one question has already read off the disk.
///
/// A caller answers one question at a time and nothing under it writes to the
/// repository, so a file cannot change while one is being answered — which
/// makes reading it a second time waste rather than caution. Emptied by
/// [`Sources::begin_round`].
#[derive(Default)]
struct Round {
    listing: Option<Rc<Listing>>,
    contents: BTreeMap<PathBuf, u64>,
    /// Whether the warm session has been brought up to date this round.
    ///
    /// Once is right and twice is waste, for the same reason the two above are
    /// memos: nothing writes to the repository while one question is being
    /// answered. Without this, a `workspace/diagnostic` asked which target owns
    /// each of a hundred files paid a walk of every file read so far, a hundred
    /// times.
    refreshed: bool,
}

/// What answering cost in reading, as counters a test can pin.
#[derive(Default, Clone, Copy)]
pub struct Reads {
    /// `session::open_at` calls. Each reads `REPO.buri` and every `BUILD.buri`.
    pub opened: u64,
    /// Files whose bytes were read to answer "has anything moved".
    pub hashed: u64,
}

/// One repository's loaded state, kept between the questions asked of it.
pub struct Sources {
    root: PathBuf,
    flags: Flags,
    /// The graph and every file read so far, copied for each analysis.
    warm: Option<Rc<Session>>,
    /// What decided the graph the warm session holds.
    graph: u64,
    /// Per file in that session: the id its text is under, and a hash of that
    /// text. This is the whole of what says a revision moved.
    held: BTreeMap<PathBuf, (FileId, u64)>,
    round: Round,
    pub reads: Reads,
}

impl Sources {
    pub fn at(root: &Path, flags: Flags) -> Sources {
        Sources {
            root: root.to_path_buf(),
            flags,
            warm: None,
            graph: 0,
            held: BTreeMap::new(),
            round: Round::default(),
            reads: Reads::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A new question has arrived, so nothing read for the last one may be
    /// reused. Between two questions anything may have been written.
    pub fn begin_round(&mut self) {
        self.round = Round::default();
    }

    /// Every `.buri` file under the root, sorted, from one directory walk.
    ///
    /// Sources, every `BUILD.buri` and `REPO.buri` — they all wear the one
    /// extension. `read_dir` order is the filesystem's and no key may be, so
    /// the list is sorted before anything hashes it.
    pub fn listing(&mut self) -> Rc<Listing> {
        if let Some(known) = &self.round.listing {
            return Rc::clone(known);
        }
        let mut files = Vec::new();
        crate::commands::format::collect(&self.root, &mut files);
        files.sort();
        let mut hasher = crate::hash::FxHasher::default();
        hasher.write(self.root.as_os_str().as_encoded_bytes());
        for path in &files {
            hasher.write(path.as_os_str().as_encoded_bytes());
        }
        let listing = Rc::new(Listing { files, names: hasher.finish() });
        self.round.listing = Some(Rc::clone(&listing));
        listing
    }

    /// A hash of the bytes an analysis would read for one file.
    ///
    /// The overlay's if there is one, because that is what the analysis
    /// actually reads; the disk's otherwise. A file that cannot be read at all
    /// still has to move the hash when it appears or goes away, so it hashes as
    /// one byte rather than as nothing.
    pub fn content_hash(&mut self, path: &Path, overlay: &Overlay) -> u64 {
        if let Some(known) = self.round.contents.get(path) {
            return *known;
        }
        let mut hasher = crate::hash::FxHasher::default();
        match overlay.get(path) {
            Some(text) => hasher.write(text.as_bytes()),
            None => match std::fs::read(path) {
                Ok(bytes) => hasher.write(&bytes),
                Err(_) => hasher.write_u8(0),
            },
        }
        let hash = hasher.finish();
        self.reads.hashed = self.reads.hashed.saturating_add(1);
        self.round.contents.insert(path.to_path_buf(), hash);
        hash
    }

    /// A hash of everything that decides the build graph: which `.buri` files
    /// exist, and the bytes of `REPO.buri` and every `BUILD.buri`.
    pub fn graph_key(&mut self, overlay: &Overlay) -> u64 {
        let listing = self.listing();
        let mut hasher = crate::hash::FxHasher::default();
        hasher.write_u64(listing.names);
        for path in &listing.files {
            if !is_graph_file(path) {
                continue;
            }
            hasher.write(path.as_os_str().as_encoded_bytes());
            hasher.write_u64(self.content_hash(path, overlay));
        }
        hasher.finish()
    }

    /// A hash of every byte that feeds an analysis of this repository.
    ///
    /// The standard library is not in it: it is compiled into this binary and
    /// cannot change while the process runs. The root itself is hashed, so two
    /// open repositories key their answers separately.
    pub fn fingerprint(&mut self, overlay: &Overlay) -> u64 {
        let listing = self.listing();
        let mut hasher = crate::hash::FxHasher::default();
        hasher.write_u64(listing.names);
        for path in &listing.files {
            hasher.write_u64(self.content_hash(path, overlay));
        }
        hasher.finish()
    }

    /// A hash of the files named in `closure`, under the graph they were
    /// resolved through.
    ///
    /// Two halves, and both are needed. The closure's own bytes are what the
    /// analysis read. The graph is what decided the closure *is* that set: a
    /// `BUILD.buri` edit can add a dependency, and a file appearing can turn a
    /// lint finding on, neither of which shows up in the bytes of any file
    /// already in the closure.
    pub fn closure_key(&mut self, closure: &[PathBuf], overlay: &Overlay) -> u64 {
        let graph = self.graph_key(overlay);
        let mut hasher = crate::hash::FxHasher::default();
        hasher.write_u64(graph);
        for path in closure {
            hasher.write(path.as_os_str().as_encoded_bytes());
            hasher.write_u64(self.content_hash(path, overlay));
        }
        hasher.finish()
    }

    /// The repository, loaded once and kept for as long as its files stand.
    ///
    /// Shared, and therefore read-only: the questions answered off it are
    /// about the graph — which target owns a file, what a label names — and
    /// the front end has nothing to add to those.
    pub fn shared(&mut self, overlay: &Overlay) -> Result<Rc<Session>, String> {
        self.refresh(overlay)?;
        match &self.warm {
            Some(session) => Ok(Rc::clone(session)),
            // `refresh` either fills it or returns the reason it could not.
            None => Err("this repository will not open".to_string()),
        }
    }

    /// A copy of it to analyse in, with every file read so far already in it.
    ///
    /// Cheap: the text and the parses are shared, so what is copied is a
    /// vector of pointers. The copy is the caller's to write to, and what it
    /// goes on to read is offered back through [`Sources::keep`].
    pub fn session(&mut self, overlay: &Overlay) -> Result<Session, String> {
        Ok((*self.shared(overlay)?).clone())
    }

    /// The files an analysis went on to read, kept for the next one.
    ///
    /// The session handed in must be one [`Sources::session`] produced and only
    /// analyses have been run in: what it holds is a superset of the warm copy,
    /// so keeping it is how a file read for one target is already read for the
    /// next.
    pub fn keep(&mut self, session: &Session) {
        self.take_stock(session);
        let Some(warm) = self.warm.as_mut() else { return };
        // The two read caches and nothing else: the diagnostics loading the
        // repository collected belong to the warm copy, and a caller that has
        // reported and cleared its own must not clear them here.
        let kept = Rc::make_mut(warm);
        kept.map = session.map.clone();
        kept.parsed = session.parsed.clone();
    }

    /// Reopens the repository if the graph moved, and re-reads the files whose
    /// bytes did.
    ///
    /// `retried` is what makes this terminate: the only way a re-read fails is
    /// a file the warm copy read and nothing can read now, and one reopen
    /// leaves `held` holding only the build files — which `open_at` would have
    /// refused if they were the unreadable ones.
    fn refresh(&mut self, overlay: &Overlay) -> Result<(), String> {
        if self.round.refreshed {
            return Ok(());
        }
        let outcome = self.refresh_now(overlay);
        // A round that failed to open the repository has not refreshed
        // anything, and the next question in it deserves the same reason.
        self.round.refreshed = outcome.is_ok();
        outcome
    }

    fn refresh_now(&mut self, overlay: &Overlay) -> Result<(), String> {
        let mut retried = false;
        loop {
            let key = self.graph_key(overlay);
            if self.warm.is_none() || self.graph != key {
                self.reads.opened = self.reads.opened.saturating_add(1);
                let session = session::open_at(&self.root, &self.flags)?;
                self.graph = key;
                self.held = BTreeMap::new();
                self.take_stock(&session);
                self.warm = Some(Rc::new(session));
            }

            // What moved, and the text it moved to. Read before anything is
            // written, so that each borrow below is of one field.
            let known: Vec<(PathBuf, FileId, u64)> =
                self.held.iter().map(|(p, (f, h))| (p.clone(), *f, *h)).collect();
            let mut moved = Vec::new();
            let mut unreadable = false;
            for (path, file, was) in known {
                if self.content_hash(&path, overlay) == was {
                    continue;
                }
                let text = match overlay.get(&path) {
                    Some(text) => Some(text.clone()),
                    None => std::fs::read_to_string(&path).ok(),
                };
                match text {
                    Some(text) => moved.push((path, file, text)),
                    None => unreadable = true,
                }
            }
            if unreadable && !retried {
                // Rather than guess what the loader would have made of a file
                // nothing can read, throw the warm copy away and open again.
                retried = true;
                self.warm = None;
                self.held = BTreeMap::new();
                continue;
            }

            // Overlaid files nothing has read yet. Seeding one under the name
            // the loader will ask for is the whole of the overlay trick: the
            // loader never reaches the disk, and `compiler::modules` needs no
            // notion of an editor at all.
            let fresh: Vec<(PathBuf, String)> = overlay
                .iter()
                .filter(|(path, _)| {
                    path.starts_with(&self.root) && !self.held.contains_key(*path)
                })
                .map(|(path, text)| (path.clone(), text.clone()))
                .collect();

            if moved.is_empty() && fresh.is_empty() {
                return Ok(());
            }
            let Some(warm) = self.warm.as_mut() else { return Ok(()) };
            // The copy this makes when the session is shared is what leaves
            // every analysis already in hand reading the text it was built on.
            let session = Rc::make_mut(warm);
            for (path, file, text) in moved {
                let hash = hash_of(&text);
                session.map.replace(file, text);
                session.parsed.forget(file);
                self.held.insert(path, (file, hash));
            }
            for (path, text) in fresh {
                let hash = hash_of(&text);
                let name = session.workspace.rel_of(&path);
                let file = session.map.add(name, path.clone(), text);
                self.held.insert(path, (file, hash));
            }
            return Ok(());
        }
    }

    /// Records the id and the bytes behind every file a session has read.
    fn take_stock(&mut self, session: &Session) {
        for (id, file) in session.map.entries() {
            // The embedded standard library has no path: it is compiled into
            // this binary and cannot move while the process runs.
            if file.abs_path.as_os_str().is_empty() || self.held.contains_key(&file.abs_path) {
                continue;
            }
            self.held.insert(file.abs_path.clone(), (id, hash_of(&file.text)));
        }
    }
}

/// The hash [`Sources::content_hash`] would give the same bytes.
fn hash_of(text: &str) -> u64 {
    let mut hasher = crate::hash::FxHasher::default();
    hasher.write(text.as_bytes());
    hasher.finish()
}

/// Whether a path is one of the build graph's own files rather than a source.
fn is_graph_file(path: &Path) -> bool {
    matches!(path.file_name().and_then(|n| n.to_str()), Some("BUILD.buri" | "REPO.buri"))
}
