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
//!
//! The reading behind those hashes is not here. `build::sources` owns the
//! directory walk, the content hashes and the repository itself, because
//! `buri lint` and `buri test --watch` want the same reuse and a second copy
//! of it would be a second place for it to drift.

use crate::build::session::Session;
use crate::build::sources::{Overlay, Sources};
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
    /// Per repository: the graph, every file read so far, and the parses of
    /// them. The server owns one of these and adds nothing to it — the
    /// keeping, the hashing and the re-reading are `build::sources`', so that
    /// `buri lint` and `buri test --watch` get the same reuse from the same
    /// code.
    sources: BTreeMap<PathBuf, Sources>,
    /// Per repository and target: what the two passes said about that target,
    /// already converted, and the closure state they said it about.
    ///
    /// Kept as findings rather than as analyses because a `workspace/diagnostic`
    /// asks about every target at once: holding one `Analysis` per target in a
    /// large repository is holding the repository several times over, and what
    /// the report needs off each of them is a few hundred bytes of JSON.
    target_findings: BTreeMap<(PathBuf, TargetId), TargetFindings>,
    /// What has been done since the server started. `handle` reads it either
    /// side of a request and reports the difference.
    work: Work,
    /// The whole-repository sweep, and the worker that runs it.
    ///
    /// Everything above this line is the request thread's alone. What the
    /// worker sends back is bytes — findings as JSON, the closures they were
    /// read at — which is why the two can be warm at once with no lock between
    /// them. See `super::sweep`.
    pub sweeps: super::sweep::Sweeps,
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

/// The `resultId`s pull diagnostics have gone out with.
///
/// The protocol asks a result id to be opaque and asks the server never to call
/// a report unchanged against an id it did not itself hand out. A counter is
/// both, where a hash alone would be only the first: an id issued is an id this
/// server produced, so "unchanged" is a fact rather than a client's claim about
/// a number it could have made up.
///
/// One id per *closure*, and the same id whichever of the two pulls asked. It
/// used to be one id per repository, which meant a keystroke in one library
/// made every report in it stale — so the answer to "has this file's report
/// changed" was "yes" for a hundred files it could not have changed for, and
/// each of them got its whole report sent again.
///
/// One id space and not two, because a client keeps one id per file and does
/// not remember which request gave it: Zed quotes the id from a
/// `workspace/diagnostic` back in the next `textDocument/diagnostic`. Two
/// spaces would make every such quote a miss, which is correct and useless.
#[derive(Default)]
struct DiagnosticResults {
    /// Per root and target: the closure key the last id stood for, and that
    /// id. `None` is the target for a file no rule in the graph claims.
    documents: BTreeMap<(PathBuf, Option<TargetId>), (u64, String)>,
    /// The counter behind them all, so no two ids are ever the same string.
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

/// What `buri lint` found, and the analysis its rules were read off.
///
/// The analysis and not a session of its own: every rule here asks about the
/// closure the diagnostics have already compiled, so a second one would be the
/// same front end run twice for one answer.
pub struct Linted {
    pub analyzed: Rc<Analyzed>,
    pub diagnostics: crate::diagnostics::Diagnostics,
}

// ---------------------------------------------------------------------------
// What a request cost
// ---------------------------------------------------------------------------

/// What the server has done, counted in work rather than in milliseconds.
///
/// A wall clock cannot be pinned by a test — a debug build on a loaded runner
/// is an order slower than a release build on an idle one — but the work is the
/// same number every time, and it is where the milliseconds come from. "A
/// keystroke in one library re-analyses one target and not three" is the claim
/// the speed rests on, and these counters are how a recorded session states it.
///
/// Reported by `handle` as the difference either side of one request, at
/// `verbose` and nowhere else.
#[derive(Clone, Copy, Default)]
pub struct Work {
    /// Front-end runs: one per target analysed, and one per whole-repository
    /// analysis.
    pub analyses: u64,
    /// Lint passes, counted the same way.
    pub lints: u64,
    /// `session::open_at` calls. Each reads `REPO.buri` and every `BUILD.buri`
    /// and parses them.
    pub sessions_opened: u64,
    /// Files whose bytes were read to answer "has anything moved".
    pub files_hashed: u64,
    /// Bytes of JSON the answers came to.
    pub bytes_written: u64,
    /// Sweeps asked for that never ran, because a newer edit replaced them
    /// while they waited. The debounce, said out loud.
    pub sweeps_superseded: u64,
}

impl Work {
    /// What happened between two readings of the counters.
    pub fn since(self, before: Work) -> Work {
        Work {
            analyses: self.analyses.saturating_sub(before.analyses),
            lints: self.lints.saturating_sub(before.lints),
            sessions_opened: self.sessions_opened.saturating_sub(before.sessions_opened),
            files_hashed: self.files_hashed.saturating_sub(before.files_hashed),
            bytes_written: self.bytes_written.saturating_sub(before.bytes_written),
            sweeps_superseded: self.sweeps_superseded.saturating_sub(before.sweeps_superseded),
        }
    }

    /// Two readings added, which is how what a worker did lands in what the
    /// server has done.
    pub fn plus(self, other: Work) -> Work {
        Work {
            analyses: self.analyses.saturating_add(other.analyses),
            lints: self.lints.saturating_add(other.lints),
            sessions_opened: self.sessions_opened.saturating_add(other.sessions_opened),
            files_hashed: self.files_hashed.saturating_add(other.files_hashed),
            bytes_written: self.bytes_written.saturating_add(other.bytes_written),
            sweeps_superseded: self
                .sweeps_superseded
                .saturating_add(other.sweeps_superseded),
        }
    }

    /// The `$/logTrace` line. Every number here is deterministic, which is what
    /// lets a golden session hold it.
    ///
    /// The superseded sweeps are named only when there are some: a counter
    /// that is zero in every session but the ones written to provoke it says
    /// nothing, and every recorded line would carry it. It goes *before* the
    /// bytes, because the bytes are the one number a golden normalises away
    /// and it does that by reading to the end of the line.
    pub fn spelled(&self) -> String {
        let mut said = format!(
            "analyses {}, lints {}, sessions opened {}, files hashed {}",
            self.analyses, self.lints, self.sessions_opened, self.files_hashed
        );
        if self.sweeps_superseded > 0 {
            said.push_str(&format!(", sweeps superseded {}", self.sweeps_superseded));
        }
        said.push_str(&format!(", bytes written {}", self.bytes_written));
        said
    }
}

// ---------------------------------------------------------------------------
// What one message read
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// One answer, and the state of the files it was computed from.
///
/// The key used to be a hash of the whole repository, which made every answer
/// in it valid entirely or not at all: a keystroke in one library threw away
/// the analysis of every target, including the ones that cannot see that file.
/// A closure is the honest unit — `driver::analyze` reads exactly the modules
/// it names, and `Analyzed` says which those were — so the key is a hash of
/// *those* files, and the graph the closure was derived from.
struct Cached<T> {
    root: PathBuf,
    /// `None` for a file no rule in the build graph claims.
    target: Option<TargetId>,
    /// Which bodies were type-checked. `None` is every body in the closure;
    /// `Some(file)` is that file's alone — see [`State::analyze_for_query`].
    ///
    /// Part of the key and not a note about it. The two answers are built from
    /// the same bytes and are not the same answer: the scoped one has no entry
    /// in `Checked::bodies` for any other file, so handing it to a caller that
    /// asked for the whole closure would report a repository with one file's
    /// problems in it. A full analysis is a superset and may stand in for a
    /// scoped one; never the other way round.
    scope: Option<PathBuf>,
    key: u64,
    /// The files the answer read, so a later request asks whether *those*
    /// moved rather than whether anything in the repository did.
    closure: Rc<Vec<PathBuf>>,
    value: Rc<T>,
}

/// How many per-target answers of each kind are kept.
///
/// A person edits in a handful of files at a time, and an `Analysis` carries
/// its whole closure — keeping one per target in a large repository is how a
/// server ends up holding the repository several times over. Eight is enough
/// for every buffer a person has open at once to keep its own.
const REMEMBERED: usize = 8;

#[derive(Default)]
pub struct Cache {
    /// Oldest first, so eviction is from the front.
    analyses: Vec<Cached<Analyzed>>,
    /// Analyses that checked one file's bodies and nothing else, filed by that
    /// file rather than by its target: two open files of one target are two of
    /// these, which is the whole point of them.
    ///
    /// A list of their own rather than a `scope` alongside the full ones, so
    /// that a person with eight files open cannot evict the full analysis a
    /// pull needs — and so that `known_closure`, which is what a result id
    /// stands for, can only ever find an analysis that checked everything.
    queries: Vec<Cached<Analyzed>>,
    lints: Vec<Cached<Linted>>,
    /// The whole-repository analysis, which has no closure smaller than the
    /// repository and is keyed on all of it.
    ///
    /// The questions that need it are the ones about a *name* — references,
    /// rename, the workspace outline — where one compilation is what makes a
    /// single scan of the result well defined. Diagnostics are not among them
    /// any more: see [`State::workspace_findings`].
    whole: Option<(PathBuf, u64, Rc<Analyzed>)>,
}

/// Files an answer, evicting the oldest to keep at most [`REMEMBERED`].
fn remember<T>(list: &mut Vec<Cached<T>>, entry: Cached<T>) {
    if let Some(slot) = list
        .iter_mut()
        .find(|c| c.root == entry.root && c.target == entry.target && c.scope == entry.scope)
    {
        *slot = entry;
        return;
    }
    if list.len() >= REMEMBERED {
        list.remove(0);
    }
    list.push(entry);
}

/// The files on disk an analysis read, which is what its answer depends on.
///
/// The standard library is not among them: it is compiled into this binary and
/// its modules carry no path.
fn closure_of(analysis: &Analysis) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> =
        analysis.loaded.modules.iter().filter_map(|m| m.disk.clone()).collect();
    files.sort();
    files.dedup();
    files
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
            sources: BTreeMap::new(),
            target_findings: BTreeMap::new(),
            work: Work::default(),
            sweeps: super::sweep::Sweeps::new(),
        }
    }

    /// Points a freshly made `State` at one repository and one set of buffers.
    ///
    /// This is how the sweep worker is told what to sweep: it holds a `State`
    /// of its own, and a job is a root and the editor's unsaved text. Nothing
    /// else reaches it — the disk it reads itself.
    pub fn stand_at(&mut self, root: &Path, buffers: Overlay) {
        if !self.roots.iter().any(|known| known == root) {
            self.roots.push(root.to_path_buf());
        }
        self.open = buffers;
        self.begin_message();
    }

    /// A new message has arrived, so nothing read for the last one may be
    /// reused.
    ///
    /// The server answers one message at a time and nothing under it writes to
    /// the repository, which is what makes reading a file twice while serving
    /// one message waste rather than caution. Between two messages the client
    /// may have written anything, so this is where that reasoning stops.
    pub fn begin_message(&mut self) {
        for sources in self.sources.values_mut() {
            sources.begin_round();
        }
    }

    /// What the server has done since it started, its own counters and the
    /// reading `build::sources` did for it together.
    pub fn work(&self) -> Work {
        let mut work = self.work;
        for sources in self.sources.values() {
            work.sessions_opened = work.sessions_opened.saturating_add(sources.reads.opened);
            work.files_hashed = work.files_hashed.saturating_add(sources.reads.hashed);
        }
        work.sweeps_superseded =
            work.sweeps_superseded.saturating_add(self.sweeps.counters.superseded);
        work
    }

    /// Records the bytes an answer came to, for the trace line that reports it.
    pub fn wrote(&mut self, bytes: u64) {
        self.work.bytes_written = self.work.bytes_written.saturating_add(bytes);
    }

    /// Files a buffer the editor opened or edited.
    ///
    /// Not a bare `open.insert`: the overlay is what a fingerprint hashes, so
    /// a buffer that moved invalidates everything this message has read.
    pub fn set_buffer(&mut self, path: PathBuf, text: String) {
        self.open.insert(path, text);
        self.begin_message();
    }

    /// Drops one, by the same rule.
    pub fn drop_buffer(&mut self, path: &Path) -> Option<String> {
        let was = self.open.remove(path);
        self.begin_message();
        was
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

    /// This repository's loaded state, which is where every file it has read
    /// and every parse of one is kept.
    ///
    /// Returned alongside the overlay because the two are read together and
    /// are different fields: what an analysis reads for a file is the editor's
    /// copy where there is one.
    fn sources_of(&mut self, root: &Path) -> (&mut Sources, &Overlay) {
        let sources = self
            .sources
            .entry(root.to_path_buf())
            .or_insert_with(|| Sources::at(root, Flags::default()));
        (sources, &self.open)
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
    ///
    /// Shared, and therefore read-only. A caller that must write to the
    /// session wants [`State::overlaid_session`].
    fn opened(&mut self, root: &Path) -> Option<Rc<Session>> {
        let (sources, open) = self.sources_of(root);
        match sources.shared(open) {
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

    /// The files an analysis went on to read, kept for the next one.
    fn keep(&mut self, root: &Path, session: &Session) {
        let (sources, _) = self.sources_of(root);
        sources.keep(session);
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

    /// A hash of the files named in `closure`, under the graph they were
    /// resolved through. See `build::sources`, which is where the reading and
    /// the hashing live.
    fn closure_key(&mut self, root: &Path, closure: &[PathBuf]) -> u64 {
        let (sources, open) = self.sources_of(root);
        sources.closure_key(closure, open)
    }

    /// A hash of everything that decides the build graph.
    fn graph_key(&mut self, root: &Path) -> u64 {
        let (sources, open) = self.sources_of(root);
        sources.graph_key(open)
    }

    /// A hash of every byte that feeds an analysis of one repository.
    fn fingerprint(&mut self, root: &Path) -> u64 {
        let (sources, open) = self.sources_of(root);
        sources.fingerprint(open)
    }

    /// One number for the whole of what the client has open.
    ///
    /// [`State::fingerprint`] answers for one repository, which is what an
    /// analysis is keyed by. "Has anything the client is looking at moved" is
    /// the other question, and it is every root's answer together.
    pub fn analysis_fingerprint(&mut self) -> u64 {
        let mut hasher = crate::hash::FxHasher::default();
        for root in self.roots.clone() {
            let one = self.fingerprint(&root);
            hasher.write_u64(one);
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

    /// The id the last `textDocument/diagnostic` report about this document
    /// went out with, if what that report was computed from has not moved.
    ///
    /// This is the whole of the unchanged path, and it costs a hash of one
    /// closure. A client quoting the id it was last given is asking "is
    /// anything my report was computed from different now", and for a file in
    /// a library nobody has touched the answer is no however much of the rest
    /// of the repository has changed.
    pub fn current_result_id(&mut self, path: &Path) -> Option<String> {
        let root = self.root_of(path)?;
        let target = self.target_of(&root, path);
        let closure = self.known_closure(&root, target)?;
        let now = self.closure_key(&root, &closure);
        let (seen, id) = self.diagnostic_results.documents.get(&(root, target))?;
        (*seen == now).then(|| id.clone())
    }

    /// The id a report just computed goes out with: the one already issued if
    /// nothing moved, and a fresh one if it did.
    pub fn issue_result_id(&mut self, path: &Path) -> Option<String> {
        let root = self.root_of(path)?;
        let target = self.target_of(&root, path);
        // A file no rule in the graph claims has no closure to key on — and no
        // findings either, because nothing analyses it. What could give it
        // some is a build file that starts claiming it, so the graph is the
        // key: `REPO.buri` then keeps its id through a keystroke in a source
        // rather than being restated on every one.
        let now = match self.known_closure(&root, target) {
            Some(closure) => self.closure_key(&root, &closure),
            None => self.graph_key(&root),
        };
        Some(self.issued((root, target), now))
    }

    /// The id for a file's entry in a `workspace/diagnostic` report.
    ///
    /// The same id [`State::issue_result_id`] would hand out when the sweep has
    /// caught up, and the id the *last* sweep's report went out with when it
    /// has not. A result id stands for the state the report describes, and a
    /// report a keystroke old describes the state before that keystroke — so
    /// issuing the current state's id for it would tell a client its stale
    /// findings are current, and the refresh that follows would be answered
    /// `unchanged` for ever.
    pub fn reported_result_id(&mut self, path: &Path) -> Option<String> {
        let root = self.root_of(path)?;
        if let Some(target) = self.target_of(&root, path) {
            if let Some(known) = self.target_findings.get(&(root.clone(), target)) {
                let filed = known.key;
                return Some(self.issued((root, Some(target)), filed));
            }
        }
        self.issue_result_id(path)
    }

    /// The id filed under this key, minting one when the state it stands for
    /// has moved.
    fn issued(&mut self, key: (PathBuf, Option<TargetId>), now: u64) -> String {
        if let Some((seen, id)) = self.diagnostic_results.documents.get(&key) {
            if *seen == now {
                return id.clone();
            }
        }
        self.diagnostic_results.count = self.diagnostic_results.count.saturating_add(1);
        let id = self.diagnostic_results.count.to_string();
        self.diagnostic_results.documents.insert(key, (now, id.clone()));
        id
    }

    /// The files the last analysis of this target read, if there was one.
    fn known_closure(&self, root: &Path, target: Option<TargetId>) -> Option<Rc<Vec<PathBuf>>> {
        self.cache
            .analyses
            .iter()
            .find(|c| c.root == root && c.target == target)
            .map(|c| Rc::clone(&c.closure))
            .or_else(|| {
                self.target_findings
                    .get(&(root.to_path_buf(), target?))
                    .map(|known| Rc::clone(&known.closure))
            })
    }

    /// Files an encoded semantic-token result under a fresh id, and hands the
    /// id back to be sent with it.
    pub fn record_semantic_tokens(&mut self, path: &Path, data: Vec<u32>) -> String {
        self.semantic_tokens.issued = self.semantic_tokens.issued.saturating_add(1);
        let id = self.semantic_tokens.issued.to_string();
        self.semantic_tokens.results.insert(path.to_path_buf(), (id.clone(), data));
        id
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
    ///
    /// Shared, and kept for as long as the files that decide the graph are
    /// unchanged. It used to be opened afresh on every call, which meant
    /// re-reading and re-parsing `REPO.buri` and every `BUILD.buri` to answer
    /// which target owns a file — a question asked several times per request.
    pub fn session_for(&mut self, path: &Path) -> Option<Rc<Session>> {
        let root = self.root_of(path)?;
        self.graph(&root)
    }

    /// The loaded graph of one repository.
    fn graph(&mut self, root: &Path) -> Option<Rc<Session>> {
        self.opened(root)
    }

    /// The target whose closure `path` is in, read off the graph alone.
    ///
    /// The overlay does not reach this: `Workspace::load` has already run by
    /// the time a buffer is layered into a session's map, so which rule owns a
    /// file is the disk's answer either way.
    fn target_of(&mut self, root: &Path, path: &Path) -> Option<TargetId> {
        let session = self.graph(root)?;
        target_for(&session, path)
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
    /// The key is the *closure* — the files this analysis read, and the graph
    /// that decided which those are — so a keystroke in one library leaves
    /// every target that cannot see that file with its answer intact. A
    /// repository-wide key threw all of them away on every character typed.
    pub fn analyze(&mut self, path: &Path) -> Option<Rc<Analyzed>> {
        let root = self.root_of(path)?;
        let target = self.target_of(&root, path);
        self.analyze_target(&root, target)
    }

    /// The same, for a target named rather than found from a file.
    ///
    /// `workspace/diagnostic` asks about every target and has no file to ask
    /// through, and asking through one of the target's sources would mean
    /// finding one first.
    pub fn analyze_target(
        &mut self,
        root: &Path,
        target: Option<TargetId>,
    ) -> Option<Rc<Analyzed>> {
        if let Some(hit) = self.cached_analysis(root, target) {
            return Some(hit);
        }

        let mut session = self.overlaid_session(root)?;
        let unit = Unit {
            target,
            // The editor is not building an output, and a file open in it
            // belongs to every output its target declares. See `Unit::platform`.
            platform: None,
            // A test source is a source an editor opens like any other, so the
            // server would be blind in exactly the files most often edited.
            with_tests: true,
        };
        self.work.analyses = self.work.analyses.saturating_add(1);
        let analysis = crate::compiler::driver::analyze(
            Some(&session.workspace),
            &mut session.map,
            &mut session.parsed,
            &unit,
        );
        let closure = closure_of(&analysis);
        self.keep(root, &session);
        let analyzed = Rc::new(Analyzed { session, analysis });
        let key = self.closure_key(root, &closure);
        remember(
            &mut self.cache.analyses,
            Cached {
                root: root.to_path_buf(),
                target,
                // Every body in the closure, which is what makes this one
                // usable for diagnostics and for a scoped question alike.
                scope: None,
                key,
                closure: Rc::new(closure),
                value: Rc::clone(&analyzed),
            },
        );
        Some(analyzed)
    }

    /// The analysis a question *about one position in one file* needs.
    ///
    /// The same closure as [`State::analyze`] — loaded, resolved, every
    /// signature, type, trait, impl and module-level `let` elaborated — but the
    /// bodies are type-checked for this file only. Hover, definition,
    /// completion, the tokens, the hints, the highlights and signature help all
    /// walk the bodies of the file under the cursor and filter every other one
    /// out by file id, so checking the rest of the repository was work whose
    /// entire result they discarded. `tests/language/scoped_bodies.rs` holds
    /// the two to being the same answer, file by file, over every fixture
    /// repository here.
    ///
    /// A full analysis whose closure has not moved is a superset and is used
    /// instead — a hover right after a save costs nothing, and pays for nothing
    /// either. **The reverse is not allowed**, which is why these are filed
    /// apart from the full ones: see [`Cached::scope`].
    ///
    /// **Not for diagnostics.** A file this did not check reports nothing, so a
    /// problem list built from it would go quiet in every file but one. Those
    /// paths ask [`State::analyze`], and are untouched.
    pub fn analyze_for_query(&mut self, path: &Path) -> Option<Rc<Analyzed>> {
        let root = self.root_of(path)?;
        let target = self.target_of(&root, path);
        if let Some(hit) = self.cached_analysis(&root, target) {
            return Some(hit);
        }
        if let Some(hit) = self.cached_query(&root, path) {
            return Some(hit);
        }

        let mut session = self.overlaid_session(&root)?;
        let unit = Unit { target, platform: None, with_tests: true };
        // The file as the loader will name it. A file the editor has open is
        // already in the map, layered over the disk by `overlaid_session`; one
        // it does not is seeded here with the same bytes the loader would have
        // read, which is the same trick and for the same reason — `SourceMap::
        // load` reuses an entry whose name already exists.
        let name = session.workspace.rel_of(path);
        let file = match session.map.find(&name) {
            Some(file) => file,
            None => session.map.add(name, path.to_path_buf(), self.text_of(path)?),
        };
        self.work.analyses = self.work.analyses.saturating_add(1);
        let analysis = crate::compiler::driver::analyze_bodies_in(
            Some(&session.workspace),
            &mut session.map,
            &mut session.parsed,
            &unit,
            &[file],
        );
        // The loading phase is the whole closure either way, so this is the
        // same set of files [`State::analyze`] would have keyed on — plus the
        // queried file itself, which a scoped analysis reads the bodies of and
        // which no rule need have claimed.
        let mut closure = closure_of(&analysis);
        if !closure.contains(&path.to_path_buf()) {
            closure.push(path.to_path_buf());
            closure.sort();
        }
        self.keep(&root, &session);
        let analyzed = Rc::new(Analyzed { session, analysis });
        let key = self.closure_key(&root, &closure);
        remember(
            &mut self.cache.queries,
            Cached {
                root: root.to_path_buf(),
                target,
                scope: Some(path.to_path_buf()),
                key,
                closure: Rc::new(closure),
                value: Rc::clone(&analyzed),
            },
        );
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
    /// Its closure is the repository, so this one is keyed on all of it.
    pub fn analyze_workspace(&mut self, root: &Path) -> Option<Rc<Analyzed>> {
        let fingerprint = self.fingerprint(root);
        if let Some((at, seen, hit)) = self.cache.whole.as_ref() {
            if at == root && *seen == fingerprint {
                return Some(Rc::clone(hit));
            }
        }

        let mut session = self.overlaid_session(root)?;
        let units: Vec<Unit> = session
            .workspace
            .targets()
            .into_iter()
            .map(|target| Unit { target: Some(target), platform: None, with_tests: true })
            .collect();
        self.work.analyses = self.work.analyses.saturating_add(1);
        let analysis = crate::compiler::driver::analyze_all(
            Some(&session.workspace),
            &mut session.map,
            &mut session.parsed,
            &units,
        );
        self.keep(root, &session);
        let analyzed = Rc::new(Analyzed { session, analysis });
        self.cache.whole = Some((root.to_path_buf(), fingerprint, Rc::clone(&analyzed)));
        Some(analyzed)
    }

    /// The findings `buri lint` reports for the target owning `path`.
    ///
    /// Keyed on the same closure as the analysis, so a second question about
    /// one state of that closure is a lookup.
    ///
    /// Shared, and therefore read-only. A caller that must write to the
    /// session wants [`State::overlaid_session`].
    pub fn lint(&mut self, path: &Path) -> Option<Rc<Linted>> {
        let root = self.root_of(path)?;
        let target = self.target_of(&root, path)?;
        self.lint_target(&root, target)
    }

    /// The same, for a target named rather than found from a file.
    pub fn lint_target(&mut self, root: &Path, target: TargetId) -> Option<Rc<Linted>> {
        if let Some(hit) = self.cached_lint(root, target) {
            return Some(hit);
        }
        // Every rule asks about the closure this analysis already read, so it
        // rides that one. The pass used to open a session and compile the
        // closure again, which was a whole second front end per pull.
        let analyzed = self.analyze_target(root, Some(target))?;
        // A repository whose own files will not load has no lint pass: the
        // checks read the graph, and there is no graph.
        if analyzed.session.diagnostics.has_errors() {
            return None;
        }
        self.work.lints = self.work.lints.saturating_add(1);
        let diagnostics = crate::commands::lint::findings_for_target(
            &analyzed.session,
            target,
            &analyzed.analysis,
        );
        let closure = Rc::new(closure_of(&analyzed.analysis));
        let linted = Rc::new(Linted { analyzed, diagnostics });
        let key = self.closure_key(root, &closure);
        remember(
            &mut self.cache.lints,
            Cached {
                root: root.to_path_buf(),
                target: Some(target),
                // The analysis behind it checked every body in the closure.
                scope: None,
                key,
                closure,
                value: Rc::clone(&linted),
            },
        );
        Some(linted)
    }

    /// Everything both passes have to say about every target in a repository.
    ///
    /// This is `workspace/diagnostic`, and what makes it liveable is that it is
    /// asked again after every keystroke. It used to be one whole-repository
    /// analysis and one lint pass over every target — a fixed cost of a few
    /// hundred milliseconds, paid in full for a character typed in a library
    /// most of those targets cannot see. Now each target's findings are kept
    /// under the closure they were read from, so a keystroke recomputes the
    /// targets that can see the edited file and quotes the rest back.
    ///
    /// The recomputed ones share one session: opening the repository, reading
    /// its build files and parsing a module are each paid once however many
    /// targets went stale together.
    ///
    /// **This never runs on the thread that answers questions.** It is what the
    /// sweep worker does with a job; the request thread reads the reports it
    /// files and asks for a new sweep — see [`State::workspace_findings`] and
    /// `super::sweep`. `wanted` is how a sweep hears that the bytes it is
    /// reading have moved: the targets it has finished stay filed under the
    /// closures they were read at, and the rest are left for the sweep that
    /// replaces this one.
    pub fn sweep_now(
        &mut self,
        root: &Path,
        wanted: &super::sweep::Wanted,
    ) -> Option<super::Published> {
        let targets = self.graph(root)?.workspace.targets();
        let mut merged = super::Published::new();
        let mut stale = Vec::new();
        for target in targets {
            match self.cached_findings(root, target) {
                Some(found) => merge_findings(&mut merged, &found),
                None => stale.push(target),
            }
        }
        if stale.is_empty() {
            return Some(merged);
        }

        let mut session = self.overlaid_session(root)?;
        // A repository whose own files will not load has no lint pass: the
        // checks read the graph, and there is no graph. The analysis still
        // runs, and what it finds is still worth saying.
        let graph_loads = !session.diagnostics.has_errors();
        // Shared across the targets: what a shared library says is said again
        // by every target that reaches it, and rendering it once is the
        // difference between a sweep and a stall. See `add_finding_rendering`.
        let mut rendered = super::Rendered::new();
        for target in stale {
            // Between targets and not inside one: a target's findings are
            // filed whole or not at all, so this is the only place where
            // stopping leaves the cache saying something true.
            if wanted.superseded() {
                break;
            }
            self.work.analyses = self.work.analyses.saturating_add(1);
            let analysis = crate::commands::lint::analysis_of(&mut session, target);
            let mut found = super::Published::new();
            for d in &analysis.diagnostics.items {
                super::add_finding_rendering(&mut found, &mut rendered, &session, d);
            }
            if graph_loads {
                self.work.lints = self.work.lints.saturating_add(1);
                let linted =
                    crate::commands::lint::findings_for_target(&session, target, &analysis);
                for d in &linted.items {
                    super::add_finding_rendering(&mut found, &mut rendered, &session, d);
                }
            }
            let closure = Rc::new(closure_of(&analysis));
            let key = self.closure_key(root, &closure);
            self.target_findings.insert(
                (root.to_path_buf(), target),
                TargetFindings { key, closure, found: found.clone() },
            );
            merge_findings(&mut merged, &found);
        }
        self.keep(root, &session);
        Some(merged)
    }

    /// The answer to a `workspace/diagnostic`, from what the server knows now.
    ///
    /// Per target, and each target's entry is one of three things: the report
    /// the last sweep filed, if the closure it was read at has not moved; the
    /// report the last sweep filed anyway, marked stale, with a fresh sweep
    /// asked for; or nothing at all, on the first question of a session, which
    /// is the one case worth waiting for.
    ///
    /// **Nothing here compiles.** A keystroke in a library twelve targets can
    /// see used to cost a fifth of a second on this thread and every request
    /// behind it the same again; now it costs a hash of each target's closure
    /// and a job handed to the worker. What the client gets in exchange is a
    /// report a keystroke old, followed by a `workspace/diagnostic/refresh` and
    /// a publish when the sweep lands — which is the flow the protocol has that
    /// request *for*.
    pub fn workspace_findings(&mut self, root: &Path) -> Option<super::Published> {
        // Three rounds is one for the answer, one for a sweep that was
        // overtaken, and one to spare. Nothing writes to the repository while
        // a message is being answered, so the first is normally the last.
        for _ in 0..3 {
            let targets = self.graph(root)?.workspace.targets();
            let mut merged = super::Published::new();
            let mut stale = false;
            let mut unknown = false;
            for target in targets {
                match self.cached_findings(root, target) {
                    Some(found) => merge_findings(&mut merged, &found),
                    None => {
                        stale = true;
                        match self.target_findings.get(&(root.to_path_buf(), target)) {
                            Some(known) => {
                                let found = known.found.clone();
                                merge_findings(&mut merged, &found);
                            }
                            None => unknown = true,
                        }
                    }
                }
            }
            if !stale {
                return Some(merged);
            }
            self.sweeps.schedule(root, &self.open);
            // A target nobody has ever swept has no report to quote, and an
            // empty one would tell the client its errors are fixed. That is
            // the cold start, and it is paid once behind the `$/progress` this
            // request already sends.
            if !unknown && self.sweeps.mode != super::sweep::Mode::Synchronous {
                return Some(merged);
            }
            self.settle_sweeps();
        }
        None
    }

    /// Everything the last complete sweep of this repository said, merged.
    pub fn reported_findings(&mut self, root: &Path) -> super::Published {
        let mut merged = super::Published::new();
        for ((at, _), known) in &self.target_findings {
            if at == root {
                let found = known.found.clone();
                merge_findings(&mut merged, &found);
            }
        }
        merged
    }

    /// Waits for every sweep asked for, and believes what they found.
    pub fn settle_sweeps(&mut self) -> Vec<PathBuf> {
        self.sweeps.settle(&self.open);
        self.take_swept()
    }

    /// Believes whatever the worker has finished, and says which repositories
    /// that was.
    pub fn take_swept(&mut self) -> Vec<PathBuf> {
        self.sweeps.collect(&self.open);
        let mut roots = Vec::new();
        for swept in self.sweeps.take_landed() {
            if !roots.contains(&swept.root) {
                roots.push(swept.root.clone());
            }
            self.absorb(swept);
        }
        roots
    }

    /// One sweep's report, filed where the request thread reads it.
    ///
    /// The closures come back as paths rather than as the `Rc`s the worker
    /// holds, which is the whole of what makes this two threads: what is
    /// shared is bytes, and each side hashes them with its own `Sources`
    /// against the same files.
    fn absorb(&mut self, swept: super::sweep::Swept) {
        for (target, key, closure, found) in swept.targets {
            self.target_findings.insert(
                (swept.root.clone(), target),
                TargetFindings { key, closure: Rc::new(closure), found },
            );
        }
        self.work = self.work.plus(swept.work);
        self.outgoing.extend(swept.said);
    }

    /// What this repository's targets are filed as saying, in the shape a
    /// report travels between threads in.
    pub fn swept_findings(
        &self,
        root: &Path,
    ) -> Vec<(TargetId, u64, Vec<PathBuf>, super::Published)> {
        self.target_findings
            .iter()
            .filter(|((at, _), _)| at == root)
            .map(|((_, target), known)| {
                (*target, known.key, (*known.closure).clone(), known.found.clone())
            })
            .collect()
    }

    /// What was last said about this target, if the closure it was said about
    /// is where it was.
    fn cached_findings(&mut self, root: &Path, target: TargetId) -> Option<super::Published> {
        let closure = self.known_closure(root, Some(target))?;
        let now = self.closure_key(root, &closure);
        let known = self.target_findings.get(&(root.to_path_buf(), target))?;
        (known.key == now).then(|| known.found.clone())
    }

    /// A session to analyse in: the open buffers layered over the disk, every
    /// file read so far already in it, and nothing analysed yet.
    ///
    /// A copy rather than the kept one, and a cheap copy — the text and the
    /// parses are shared. `build::sources` is where the layering and the
    /// keeping happen; what an analysis goes on to read is offered back to it
    /// through [`State::keep`].
    ///
    /// Public because `buri gen` writes through the session it is handed, and
    /// a kept one is shared by everything holding an `Rc` to it.
    pub fn overlaid_session(&mut self, root: &Path) -> Option<Session> {
        Some((*self.opened(root)?).clone())
    }

    /// The analysis for the target owning `path`, if the closure it read is
    /// where it was.
    ///
    /// The cost of asking is a hash of that closure and nothing else — which
    /// is the point: the files an edit did not touch are not read at all.
    fn cached_analysis(&mut self, root: &Path, target: Option<TargetId>) -> Option<Rc<Analyzed>> {
        let filed = self.cache.analyses.iter().find(|c| c.root == root && c.target == target)?;
        let (was, closure) = (filed.key, Rc::clone(&filed.closure));
        let now = self.closure_key(root, &closure);
        let filed = self.cache.analyses.iter().find(|c| c.root == root && c.target == target)?;
        (was == now).then(|| Rc::clone(&filed.value))
    }

    /// The same lookup for a scoped analysis, which is filed by the file whose
    /// bodies it checked rather than by that file's target.
    ///
    /// Only ever consulted by [`State::analyze_for_query`]. The closure it is
    /// keyed on is the one the whole-closure loader read, so a keystroke in a
    /// library this file cannot see leaves it standing exactly as it leaves a
    /// full analysis standing.
    fn cached_query(&mut self, root: &Path, path: &Path) -> Option<Rc<Analyzed>> {
        let scope = Some(path.to_path_buf());
        let filed = self.cache.queries.iter().find(|c| c.root == root && c.scope == scope)?;
        let (was, closure) = (filed.key, Rc::clone(&filed.closure));
        let now = self.closure_key(root, &closure);
        let filed = self.cache.queries.iter().find(|c| c.root == root && c.scope == scope)?;
        (was == now).then(|| Rc::clone(&filed.value))
    }

    /// The same lookup for the lint pass.
    fn cached_lint(&mut self, root: &Path, target: TargetId) -> Option<Rc<Linted>> {
        let wanted = Some(target);
        let filed = self.cache.lints.iter().find(|c| c.root == root && c.target == wanted)?;
        let (was, closure) = (filed.key, Rc::clone(&filed.closure));
        let now = self.closure_key(root, &closure);
        let filed = self.cache.lints.iter().find(|c| c.root == root && c.target == wanted)?;
        (was == now).then(|| Rc::clone(&filed.value))
    }
}

/// What a `workspace/diagnostic` last said about one target, and the state of
/// the closure it said it about.
struct TargetFindings {
    key: u64,
    closure: Rc<Vec<PathBuf>>,
    found: super::Published,
}

/// Adds one target's findings to the report being built, without saying the
/// same thing twice.
///
/// Two targets in one closure both report what the shared library said, and
/// the package rules are asked once per target rather than once per package —
/// so the same words at the same place arrive several times, and they are one
/// squiggle. The rule is [`super::same_finding`]'s, the same one a publish
/// merges by.
fn merge_findings(into: &mut super::Published, from: &super::Published) {
    for (uri, items) in from {
        let bucket = into.entry(uri.clone()).or_default();
        for item in items {
            if !bucket.iter().any(|existing| super::same_finding(existing, item)) {
                bucket.push(item.clone());
            }
        }
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
