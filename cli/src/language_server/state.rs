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
    /// `initialize` has been answered and the roots the client named are in
    /// [`State::roots`].
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

/// How much of what the server is doing the client asked to be told.
///
/// `$/setTrace` sets it and `$/logTrace` is what it turns on. The protocol
/// spells the three levels `off`, `messages` and `verbose`, and the difference
/// between the last two is detail on the same lines rather than more lines.
#[derive(Clone, Copy, PartialEq)]
pub enum Trace {
    Off,
    Messages,
    Verbose,
}

impl Trace {
    /// The level a `$/setTrace` names, or `None` for a word that is not one of
    /// the three.
    pub fn named(value: &str) -> Option<Trace> {
        match value {
            "off" => Some(Trace::Off),
            "messages" => Some(Trace::Messages),
            "verbose" => Some(Trace::Verbose),
            _ => None,
        }
    }
}

/// How many ids each of the two lists below remembers.
///
/// A cancel whose request never arrives is a leak of one id, and a client that
/// sent nothing but cancels must not grow that without bound. Sixteen is far
/// more than the number of requests a client has in flight at once against a
/// server that answers them one at a time.
const IDS_REMEMBERED: usize = 16;

pub struct State {
    pub lifecycle: Lifecycle,
    /// The repository roots the client has open, in the order it named them.
    ///
    /// A Buri repository is rooted at a `REPO.buri`, so two open folders are
    /// two repositories with two build graphs and two closures. This used to be
    /// a single `set_current_dir` at `initialize`, which made every request
    /// about the second folder answer out of the first one's graph — a wrong
    /// answer rather than a missing feature.
    pub roots: Vec<PathBuf>,
    /// Whether the client accepts a watcher registered after startup.
    ///
    /// Read from its `initialize` capabilities and not assumed: registering
    /// something a client never said it supports is a message it is entitled
    /// to reject, and the server would then be waiting for notifications that
    /// are never coming without knowing it.
    pub can_register_watchers: bool,
    /// Whether the client knows about folders and named none, which is the one
    /// case where asking it for them is worth a round trip.
    pub must_ask_for_folders: bool,
    /// Whether a pull-diagnostics answer may carry the findings this analysis
    /// made in *other* files.
    ///
    /// Read from its `initialize` capabilities and not assumed. A client that
    /// did not claim `relatedDocumentSupport` is entitled to ignore the field,
    /// and the findings it would have carried are what `workspace/diagnostic`
    /// is for.
    pub related_documents_supported: bool,
    /// Per pulled document: the related documents its last report carried
    /// findings for.
    ///
    /// A related file that is clean now is absent from the new report, and an
    /// absent entry says nothing — so the client went on showing an error that
    /// was fixed. Remembering what was named is what lets the next report name
    /// it again with empty `items`, which is how a finding is retracted.
    pub related_reported: BTreeMap<String, Vec<String>>,
    /// Whether the outline goes out as the nested `DocumentSymbol` tree.
    ///
    /// Read from its `initialize` capabilities and not assumed. A client that
    /// did not claim `hierarchicalDocumentSymbolSupport` is entitled to read
    /// the reply as `SymbolInformation[]`, whose required `location` the tree
    /// does not carry — so it gets that flat shape instead.
    pub hierarchical_symbols: bool,
    /// Which `MarkupKind` a hover is rendered in, from the client's
    /// `contentFormat`. A plaintext-only client draws a markdown fence as three
    /// literal backticks.
    pub hover_markup: super::features::Markup,
    /// Whether a code action may be offered without its edit.
    ///
    /// Read from its `initialize` capabilities and not assumed. A client that
    /// did not claim it will never send `codeAction/resolve`, so deferring the
    /// edit would leave it holding a fix that does nothing.
    pub code_action_resolve: bool,
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
    /// What the client is holding for `textDocument/semanticTokens`.
    pub semantic_tokens: SemanticTokens,
    /// What each `workspace/*/refresh` family has said and when.
    pub refreshes: Refreshes,
    /// The `workspace/applyEdit` requests waiting for an answer.
    pub applying: Applying,
    /// The result ids pull diagnostics have been answered with.
    diagnostic_results: DiagnosticResults,
    /// How much the client asked to be told, from its `$/setTrace`.
    pub trace: Trace,
    /// The ids most recently answered, oldest first.
    ///
    /// A cancel that names one of these lost its race with the answer, which is
    /// the ordinary case against a server that answers one message at a time.
    /// Remembering them is what makes such a cancel a no-op on arrival rather
    /// than a trap for a later request that happens to carry the same id.
    answered: Vec<Value>,
    /// Ids a `$/cancelRequest` named and no request has claimed yet.
    ///
    /// The request itself is what claims one: when its turn comes it is refused
    /// rather than run. See `mod.rs` on why that is the whole of cancellation
    /// in a server that reads and answers one message at a time.
    cancelled: Vec<Value>,
    /// Messages the server decided to send while it was serving something else.
    ///
    /// A repository that will not load is found where it is opened, which is
    /// several calls below the request being answered — so the message goes
    /// here and `handle` sends it with that request's replies.
    outgoing: Vec<Value>,
    /// Per repository: the analysis fingerprint its failure to load was last
    /// announced at, so a broken repository says so once rather than once per
    /// request.
    announced: BTreeMap<PathBuf, u64>,
    /// Analyses, under a hash of everything they were computed from.
    cache: Cache,
}

/// The edits this server has asked the client to write, and the command each
/// one is the answer to.
///
/// A `workspace/executeCommand` that edits is not finished when its handler
/// returns: the server has no way to write a file the editor may be holding
/// unsaved, so the edit goes to the client and the command's result is what the
/// client says it did with it. This is where the command's id waits in between.
#[derive(Default)]
pub struct Applying {
    /// The id of the `workspace/executeCommand` waiting on each edit, by the id
    /// that edit went out with.
    pub waiting: BTreeMap<String, Value>,
    /// How many have gone out, so each carries an id of its own — one still in
    /// flight must not be reused.
    pub sent: u64,
}

/// The `resultId`s pull diagnostics have gone out with, one per repository.
///
/// The protocol asks a result id to be opaque and asks the server never to call
/// a report unchanged against an id it did not itself hand out. A counter is
/// both, where the fingerprint alone would be only the first: an id issued is
/// an id this server produced, so "unchanged" is a fact rather than a client's
/// claim about a number it could have made up.
///
/// One id per repository rather than per file, because that is the granularity
/// the analysis has — the front end is whole-closure, so there is no file whose
/// answer an edit elsewhere provably does not change.
#[derive(Default)]
struct DiagnosticResults {
    /// Per root: the analysis fingerprint the last id stood for, and that id.
    issued: BTreeMap<PathBuf, (u64, String)>,
    /// The counter behind them, shared across roots so no two ids are ever the
    /// same string.
    count: u64,
}

/// What the server remembers about the colours it has handed out.
///
/// This is the server's second keyed store and it is keyed honestly: a result
/// belongs to one open document and to one `resultId`, and both die when the
/// document is closed. A delta computed against anything else would be a delta
/// against a buffer the client is no longer showing.
#[derive(Default)]
pub struct SemanticTokens {
    /// Per open document: the id the client will quote back, and the encoded
    /// tokens it stands for.
    pub results: BTreeMap<PathBuf, (String, Vec<u32>)>,
    /// The counter behind those ids. Monotonic, so an id is never reused and a
    /// client quoting a stale one is told so by the mismatch rather than by a
    /// wrong delta.
    pub issued: u64,
}

/// One family of server-sent refresh, and everything deciding whether the next
/// one goes out.
///
/// Four of these exist and they behave identically, which is the reason they
/// are one type: semantic tokens, inlay hints, code lenses and pull diagnostics
/// are all answers computed from an analysis, so all go stale exactly when the
/// analysis fingerprint moves, and a second copy of that rule would be a second
/// place for it to drift.
#[derive(Default)]
pub struct Refresh {
    /// Whether the client said it accepts this request. Read from its
    /// `initialize` capabilities and not assumed: a request a client never
    /// claimed is one it is entitled to reject.
    pub supported: bool,
    /// The analysis fingerprint the client's answers were computed from, so a
    /// save that changed nothing can be told from one that did.
    pub fingerprint: Option<u64>,
    /// How many have gone out, so each carries an id of its own — one still in
    /// flight must not be reused.
    pub sent: u64,
}

/// Every refresh family, in the order they are sent.
#[derive(Default)]
pub struct Refreshes {
    pub semantic_tokens: Refresh,
    pub inlay_hints: Refresh,
    pub code_lenses: Refresh,
    pub diagnostics: Refresh,
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
    /// The lint pass over every target at once, which has no target to key on
    /// either. `workspace/diagnostic` is what asks for it.
    whole_lint: Option<Rc<Linted>>,
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
        if let Some((_, linted)) = self.lints.first() {
            return Some(&linted.session);
        }
        self.whole_lint.as_ref().map(|linted| &linted.session)
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
            roots: Vec::new(),
            can_register_watchers: false,
            must_ask_for_folders: false,
            related_documents_supported: false,
            related_reported: BTreeMap::new(),
            hierarchical_symbols: false,
            hover_markup: super::features::Markup::Markdown,
            code_action_resolve: false,
            open: BTreeMap::new(),
            published: BTreeMap::new(),
            showing_parse_errors: BTreeSet::new(),
            semantic_tokens: SemanticTokens::default(),
            refreshes: Refreshes::default(),
            applying: Applying::default(),
            diagnostic_results: DiagnosticResults::default(),
            trace: Trace::Off,
            answered: Vec::new(),
            cancelled: Vec::new(),
            outgoing: Vec::new(),
            announced: BTreeMap::new(),
            cache: Cache::default(),
        }
    }

    /// Remembers the id a `$/cancelRequest` named.
    ///
    /// A cancel for a request that is already answered is dropped here, which
    /// is the no-op the protocol asks for.
    pub fn cancel(&mut self, id: &Value) {
        if self.answered.contains(id) || self.cancelled.contains(id) {
            return;
        }
        if self.cancelled.len() >= IDS_REMEMBERED {
            self.cancelled.remove(0);
        }
        self.cancelled.push(id.clone());
    }

    /// Records that a request has been answered, cancellably or not.
    pub fn record_answered(&mut self, id: &Value) {
        if self.answered.contains(id) {
            return;
        }
        if self.answered.len() >= IDS_REMEMBERED {
            self.answered.remove(0);
        }
        self.answered.push(id.clone());
    }

    /// Whether this request was cancelled before its turn came, and takes the
    /// id — one cancel refuses one request.
    pub fn was_cancelled(&mut self, id: &Value) -> bool {
        let Some(at) = self.cancelled.iter().position(|c| c == id) else { return false };
        self.cancelled.remove(at);
        true
    }

    /// The messages produced while serving, handed over to be sent.
    pub fn take_outgoing(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outgoing)
    }

    /// Opens a repository, and says on screen when it does not load.
    ///
    /// This is the one failure in the server a reader cannot otherwise see. An
    /// error in a `REPO.buri` or a `BUILD.buri` is an error in the repository's
    /// *own* files: no analysis produced it, so no publish carries it, and what
    /// a reader gets instead is a lint pass that quietly stops running and — if
    /// the file cannot be read at all — every request answering `null`. Either
    /// presents as the server doing nothing rather than as a file to fix.
    ///
    /// Said once per state of the repository: again when the file changes and
    /// is still wrong, and not at all while it sits there unedited.
    fn opened(&mut self, root: &Path) -> Option<Session> {
        match session::open_at(root, &Flags::default()) {
            Ok(session) => {
                if let Some(why) = first_error(&session) {
                    self.announce(root, &why);
                }
                Some(session)
            }
            Err(why) => {
                self.announce(
                    root,
                    &format!(
                        "this repository will not open: {why}. Nothing in it can be analysed until that is fixed."
                    ),
                );
                None
            }
        }
    }

    /// Queues the message, unless this state of the repository has already been
    /// announced.
    fn announce(&mut self, root: &Path, said: &str) {
        let now = self.fingerprint(root);
        if self.announced.get(root) == Some(&now) {
            return;
        }
        self.announced.insert(root.to_path_buf(), now);
        self.outgoing.push(super::message("window/showMessage", super::ERROR, said));
    }

    /// The repository root that owns `path`, if the client has it open.
    ///
    /// The nearest `REPO.buri` above the file decides which repository it is
    /// in — the same walk `find_root` does for every other command — and the
    /// answer counts only when the client named that repository as a folder. A
    /// file in no open folder gets no root, and the requests about it answer
    /// nothing rather than answering out of whichever repository happened to
    /// be first.
    pub fn root_of(&self, path: &Path) -> Option<PathBuf> {
        let root = crate::build::workspace::find_root(path)?;
        self.roots.iter().find(|r| **r == root).cloned()
    }

    /// Adds a folder, as the repository root it sits in.
    ///
    /// A client may name a subdirectory, and the repository is still the one
    /// above it — so what is kept is always a root, and a folder in no
    /// repository is not kept at all.
    pub fn add_root(&mut self, folder: &Path) {
        let Some(root) = crate::build::workspace::find_root(folder) else { return };
        if !self.roots.contains(&root) {
            self.roots.push(root);
        }
    }

    /// Drops a folder, by the same normalisation [`State::add_root`] applied.
    pub fn remove_root(&mut self, folder: &Path) {
        let Some(root) = crate::build::workspace::find_root(folder) else { return };
        self.roots.retain(|r| *r != root);
    }

    /// A hash of every byte that feeds an analysis of one repository.
    ///
    /// That is every `.buri` file under the root — sources, every `BUILD.buri`,
    /// and `REPO.buri`, which all wear the one extension — plus the open
    /// buffers layered over them, because the editor's copy is what the
    /// analysis actually reads. The standard library is not in it: it is
    /// compiled into this binary and cannot change while the server runs.
    ///
    /// The root itself is hashed, and only the buffers inside it are, so two
    /// open repositories key their answers separately and a keystroke in one
    /// does not invalidate the other's.
    ///
    /// Reading and hashing bytes is not parsing them, which is the whole
    /// point — this is paid on every request and the analysis it replaces is
    /// orders more expensive.
    fn fingerprint(&self, root: &Path) -> u64 {
        let mut files = Vec::new();
        crate::commands::format::collect(root, &mut files);
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
        for (path, text) in self.buffers_under(root) {
            hasher.write(path.as_os_str().as_encoded_bytes());
            hasher.write(text.as_bytes());
        }
        hasher.finish()
    }

    /// One number for the whole of what the client has open.
    ///
    /// [`State::fingerprint`] answers for one repository, which is what an
    /// analysis is keyed by. "Has anything the client is looking at moved" is
    /// the other question, and it is every root's answer together.
    pub fn analysis_fingerprint(&self) -> u64 {
        let mut hasher = crate::hash::FxHasher::default();
        for root in &self.roots {
            hasher.write_u64(self.fingerprint(root));
        }
        hasher.finish()
    }

    /// Records the state every refresh family's answers were computed from,
    /// and announces nothing.
    ///
    /// This is what opening a document does: nothing has gone stale yet, and a
    /// comparison a later save makes needs something to compare against.
    pub fn record_refresh_fingerprint(&mut self) {
        let now = self.analysis_fingerprint();
        self.refreshes.semantic_tokens.fingerprint = Some(now);
        self.refreshes.inlay_hints.fingerprint = Some(now);
        self.refreshes.code_lenses.fingerprint = Some(now);
        self.refreshes.diagnostics.fingerprint = Some(now);
    }

    /// The `resultId` a pull-diagnostics answer for this repository carries.
    ///
    /// The same id for as long as the analysis fingerprint does not move, and a
    /// fresh one the moment it does — so a client quoting the id it was last
    /// given is asking exactly "is anything my report was computed from
    /// different now", and the answer costs a hash rather than a compilation.
    pub fn diagnostic_result_id(&mut self, root: &Path) -> String {
        let now = self.fingerprint(root);
        if let Some((seen, id)) = self.diagnostic_results.issued.get(root) {
            if *seen == now {
                return id.clone();
            }
        }
        self.diagnostic_results.count = self.diagnostic_results.count.saturating_add(1);
        let id = self.diagnostic_results.count.to_string();
        self.diagnostic_results.issued.insert(root.to_path_buf(), (now, id.clone()));
        id
    }

    /// Files an encoded semantic-token result under a fresh id, and hands the
    /// id back to be sent with it.
    pub fn record_semantic_tokens(&mut self, path: &Path, data: Vec<u32>) -> String {
        self.semantic_tokens.issued = self.semantic_tokens.issued.saturating_add(1);
        let id = self.semantic_tokens.issued.to_string();
        self.semantic_tokens.results.insert(path.to_path_buf(), (id.clone(), data));
        id
    }

    /// The open buffers belonging to one repository.
    ///
    /// A session is loaded from one root and names its files relative to it, so
    /// a buffer from the other open repository has no name in it and must not
    /// be layered over it.
    fn buffers_under<'a>(
        &'a self,
        root: &'a Path,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a String)> {
        self.open.iter().filter(move |(path, _)| path.starts_with(root))
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
    pub fn session_for(&mut self, path: &Path) -> Option<Session> {
        let root = self.root_of(path)?;
        self.opened(&root)
    }

    /// The label of the target whose closure `path` is in, as a command line
    /// would name it.
    ///
    /// The same [`target_for`] every analysis is keyed by, so a command run
    /// from a lens runs over the target the file's diagnostics came from — a
    /// test source belongs to the rule that declared it, which is the library
    /// or the binary rather than a suite of its own.
    pub fn target_label(&mut self, path: &Path) -> Option<String> {
        let session = self.session_for(path)?;
        let target = target_for(&session, path)?;
        Some(session.workspace.label(target))
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
        let root = self.root_of(path)?;
        let fingerprint = self.fingerprint(&root);
        if let Some(hit) = self.cached_analysis(fingerprint, path) {
            return Some(hit);
        }

        let mut session = self.overlaid_session(&root)?;
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
        if let Some(generation) = self.cache.generation(fingerprint) {
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
    pub fn analyze_workspace(&mut self, root: &Path) -> Option<Rc<Analyzed>> {
        let fingerprint = self.fingerprint(root);
        if let Some(hit) = self.cache.find(fingerprint).and_then(|g| g.whole.as_ref()) {
            return Some(Rc::clone(hit));
        }

        let mut session = self.overlaid_session(root)?;
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
        if let Some(generation) = self.cache.generation(fingerprint) {
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
        let root = self.root_of(path)?;
        let fingerprint = self.fingerprint(&root);
        if let Some(hit) = self.cached_lint(fingerprint, path) {
            return Some(hit);
        }

        let mut session = self.opened(&root)?;
        if session.diagnostics.has_errors() {
            return None;
        }
        for (p, text) in self.buffers_under(&root) {
            let rel = session.workspace.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }
        let target = target_for(&session, path)?;
        let diagnostics = crate::commands::lint::findings_for(&mut session, &[target]);
        let linted = Rc::new(Linted { session, diagnostics });
        if let Some(generation) = self.cache.generation(fingerprint) {
            remember(&mut generation.lints, target, Rc::clone(&linted));
        }
        Some(linted)
    }

    /// The findings `buri lint //...` reports, over every target at once.
    ///
    /// [`State::lint`] answers for the target owning one file, which is what a
    /// publish about that file needs. `workspace/diagnostic` asks about the
    /// repository, and a per-target loop would run the checks that are about a
    /// *package* once per target in it — and would report each of their
    /// findings as many times.
    ///
    /// The promotion `REPO.buri`'s `fail_on_finding` asks for happens inside
    /// `lint::findings_for`, so a pulled finding wears the severity the
    /// terminal prints for exactly the same reason a pushed one does.
    pub fn lint_workspace(&mut self, root: &Path) -> Option<Rc<Linted>> {
        let fingerprint = self.fingerprint(root);
        if let Some(hit) = self.cache.find(fingerprint).and_then(|g| g.whole_lint.as_ref()) {
            return Some(Rc::clone(hit));
        }

        let mut session = self.opened(root)?;
        if session.diagnostics.has_errors() {
            return None;
        }
        for (p, text) in self.buffers_under(root) {
            let rel = session.workspace.rel_of(p);
            session.map.add(rel, p.clone(), text.clone());
        }
        let targets = session.workspace.targets();
        let diagnostics = crate::commands::lint::findings_for(&mut session, &targets);
        let linted = Rc::new(Linted { session, diagnostics });
        if let Some(generation) = self.cache.generation(fingerprint) {
            generation.whole_lint = Some(Rc::clone(&linted));
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
    pub fn overlaid_session(&mut self, root: &Path) -> Option<Session> {
        let mut session = self.opened(root)?;
        for (p, text) in self.buffers_under(root) {
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

/// The first error a session collected while loading the repository, named by
/// the file it is in.
///
/// A repository reads its `REPO.buri` and every `BUILD.buri` before anything is
/// compiled, so a failure there is the one that makes every later answer empty.
fn first_error(session: &Session) -> Option<String> {
    let d = session.diagnostics.items.iter().find(|d| d.is_error() && !d.span.is_none())?;
    let name = &session.map.get(d.span.file).name;
    Some(format!(
        "{name}: {}. That file is the repository's own, so no analysis publishes the finding — and the lint pass does not run until it is fixed.",
        d.message
    ))
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
