//! What the last lint pass found for a target, kept between invocations.
//!
//! `buri lint //...` re-reads every file and re-analyses every target, and on
//! a repository of any size that is most of what the command costs. Nothing
//! about a target's findings depends on when it was asked: they are a function
//! of the build graph and of the bytes of the files the analysis read. So the
//! answer is written down beside the build cache under a key over exactly
//! those two things, and a second run re-analyses only the targets whose
//! closure moved.
//!
//! Two things and no more are stored: the closure — each file the analysis
//! read, named repository-relatively, with a hash of its bytes — and the
//! findings. The closure is [`crate::build::sources::closure_of`]'s, so it
//! holds what a generated module was worked out from as well as the files a
//! module was read from: the inputs, present or not, the files a check or a
//! tool asked for, and the tool's own sources. Deliberately **not** the parse trees or the analysis tables: they
//! are large, and every release changes their shape, so a stale one would be a
//! wrong answer rather than a slow one. The parse reuse inside one run comes
//! from [`crate::build::sources`] instead.
//!
//! A record holds what the **catalogue** found, and not what this repository
//! chose to hear about: `REPO.buri`'s `rules` block is applied on the way out
//! of [`crate::commands::lint::findings_for`], after a record has been read
//! back. That is one decision and it settles two questions. A record written
//! while a rule was off cannot serve a verdict for that rule once it is back
//! on, because the record never held the *filtered* answer; and a rule turned
//! off after a record was written is dropped from what that record replays.
//! The rule set is therefore not in the key — it does not need to be, and
//! putting it there would claim the record depends on something it does not.
//! Editing `REPO.buri` re-analyses every target regardless: its bytes are in
//! the graph hash below, because a build file decides what a closure is.
//!
//! The key is an ordinary [`ActionKey`], so the toolchain version is in it by
//! construction and a record another `buri` wrote is unreachable rather than
//! trusted; entries land in `.buri/cache`, which `buri clean` drops and which
//! [`Cache::put`] writes through a temporary and a rename, so two `buri lint`
//! runs on one repository cannot leave half a record between them.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here walks a cursor forward through a byte buffer, and every step \
              is bounded by a length the step before it read out of that buffer"
)]

use crate::build::cache::{Action, ActionKey, Cache, KeyBuilder, Status};
use crate::build::session::Session;
use crate::build::sources::{Overlay, Sources};
use crate::build::workspace::TargetId;
use crate::commands::arguments::{BuildMode, Flags};
use crate::diagnostics::{Diagnostic, Edit, FileId, SecondarySpan, Severity, SourceMap, Span};
use std::path::{Path, PathBuf};

/// The shape of a record, so that a change to the encoding below is a miss
/// rather than a misreading. The toolchain version is already in every key;
/// this is what makes the decoder safe to point at a truncated file.
const FORMAT: &[u8] = b"buri-lint-findings-2\n";

/// The shape of a closure node ([`Store::node_of`]).
const NODE_FORMAT: &[u8] = b"buri-lint-closure-1\n";

/// The three lists the per-target pass adds to the report, in the order it
/// adds them.
///
/// They are kept apart because the package rules are asked once per *package*:
/// a package holding a library and a binary reports its own findings under
/// whichever of the two a run reaches first, and which one that is depends on
/// the targets the command was given rather than on anything a record knows.
pub struct Parts {
    pub analysis: Vec<Diagnostic>,
    /// Empty either because the package had nothing to report or because
    /// another target of it had already been asked, and the two have to be
    /// told apart — see [`Parts::asked_the_package`].
    pub package: Vec<Diagnostic>,
    pub target: Vec<Diagnostic>,
    /// Whether this pass was the one that asked the package rules.
    ///
    /// Which target of a package that is depends only on the order the loop
    /// reaches them, and every target pattern selects whole packages, so it is
    /// the same target every run. Recorded anyway: a record whose package part
    /// was somebody else's is not one a first-in-package target may replay,
    /// and saying so costs a byte.
    pub asked_the_package: bool,
}

/// One repository's lint records, for the length of one command.
pub struct Store {
    cache: Cache,
    root: PathBuf,
    /// The file hashing every key is built from — the same machinery the
    /// language server keys its per-closure answers on.
    sources: Sources,
    /// Empty: at the terminal the disk holds the only copy there is.
    overlay: Overlay,
    /// Which `.buri` files exist and what the build files say, hashed once.
    graph: u64,
    mode: BuildMode,
    explain: bool,
    /// Every file named this run, by repository-relative name, as a number.
    ids: std::collections::HashMap<String, u32>,
    names: Vec<String>,
    paths: std::collections::HashMap<PathBuf, u32>,
    /// Each named file's hash now, by number.
    hashes: Vec<Option<u64>>,
    /// Whether a closure node still holds, by key, once asked.
    holds: std::collections::HashMap<String, bool>,
    /// The files a node covers, sorted, once a new node needs them.
    sets: std::collections::HashMap<String, std::rc::Rc<Vec<u32>>>,
    /// The node each target's record named or was written with, this run.
    nodes: std::collections::HashMap<TargetId, ActionKey>,
    /// Records [`Store::remember`] took, written by [`Store::flush`].
    pending: Vec<Pending>,
}

/// A record waiting for [`Store::flush`]: its closure, as file numbers, sorted,
/// and the rest of it, encoded.
struct Pending {
    target: TargetId,
    closure: Vec<u32>,
    asked_the_package: bool,
    findings: Vec<u8>,
}

impl Store {
    pub fn open(root: &Path, flags: &Flags) -> Store {
        let mut sources = Sources::at(root, flags.clone());
        let overlay = Overlay::new();
        let graph = sources.graph_key(&overlay);
        Store {
            cache: Cache::open(root),
            root: root.to_path_buf(),
            sources,
            overlay,
            graph,
            mode: flags.mode,
            explain: flags.explain,
            ids: std::collections::HashMap::new(),
            names: Vec::new(),
            paths: std::collections::HashMap::new(),
            hashes: Vec::new(),
            holds: std::collections::HashMap::new(),
            sets: std::collections::HashMap::new(),
            nodes: std::collections::HashMap::new(),
            pending: Vec::new(),
        }
    }

    /// The number a file's repository-relative name goes by this run.
    fn id(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.ids.insert(name.to_string(), id);
        self.names.push(name.to_string());
        self.hashes.push(None);
        id
    }

    /// [`Store::id`] for a path, named repository-relatively once a run.
    fn id_of_path(&mut self, session: &Session, path: &Path) -> u32 {
        if let Some(&id) = self.paths.get(path) {
            return id;
        }
        let id = self.id(&session.workspace.rel_of(path));
        self.paths.insert(path.to_path_buf(), id);
        id
    }

    /// The hash of a file's bytes now, read once a run.
    fn hash(&mut self, id: u32) -> u64 {
        let at = id as usize;
        if let Some(Some(known)) = self.hashes.get(at) {
            return *known;
        }
        let path = self.root.join(self.names.get(at).map_or("", String::as_str));
        let hash = self.sources.content_hash(&path, &self.overlay);
        if let Some(slot) = self.hashes.get_mut(at) {
            *slot = Some(hash);
        }
        hash
    }

    /// Whether every file a closure node covers still holds the bytes it did.
    ///
    /// A node lists some files with their hashes and names the nodes it
    /// shares the rest with, so a chain of targets lists each file once. Each
    /// node is asked once a run.
    fn holds(&mut self, key: &ActionKey) -> bool {
        if let Some(&known) = self.holds.get(key.as_str()) {
            return known;
        }
        let held = self.check_node(key).unwrap_or(false);
        self.holds.insert(key.as_str().to_string(), held);
        held
    }

    fn check_node(&mut self, key: &ActionKey) -> Option<bool> {
        let bytes = self.cache.get(key)?;
        let mut reader = Reader::after(NODE_FORMAT, &bytes)?;
        for _ in 0..reader.u32()? {
            let name = reader.text()?;
            let was = reader.u64()?;
            let id = self.id(&name);
            if self.hash(id) != was {
                return Some(false);
            }
        }
        for _ in 0..reader.u32()? {
            let child = ActionKey::parse(&reader.text()?)?;
            if !self.holds(&child) {
                return Some(false);
            }
        }
        Some(true)
    }

    /// Every file a node covers, as sorted numbers.
    fn set_of(&mut self, key: &ActionKey) -> Option<std::rc::Rc<Vec<u32>>> {
        if let Some(known) = self.sets.get(key.as_str()) {
            return Some(std::rc::Rc::clone(known));
        }
        let bytes = self.cache.get(key)?;
        let mut reader = Reader::after(NODE_FORMAT, &bytes)?;
        let mut set = Vec::new();
        for _ in 0..reader.u32()? {
            let name = reader.text()?;
            let _ = reader.u64()?;
            set.push(self.id(&name));
        }
        for _ in 0..reader.u32()? {
            let child = ActionKey::parse(&reader.text()?)?;
            set.extend(self.set_of(&child)?.iter().copied());
        }
        set.sort_unstable();
        set.dedup();
        let set = std::rc::Rc::new(set);
        self.sets.insert(key.as_str().to_string(), std::rc::Rc::clone(&set));
        Some(set)
    }

    /// Writes the node covering exactly `closure`, and answers its key.
    ///
    /// The node names every node of `target`'s dependencies whose files are
    /// all in `closure`, and lists the rest. Content-addressed, so two
    /// targets with one closure share a node, and one already written is not
    /// written again.
    fn node_of(&mut self, session: &Session, target: TargetId, closure: &[u32]) -> ActionKey {
        let mut covered = vec![false; closure.len()];
        let mut children: Vec<ActionKey> = Vec::new();
        for (dep, _) in session.workspace.dep_edges(target) {
            let Some(key) = self.nodes.get(&dep).cloned() else { continue };
            let Some(set) = self.set_of(&key) else { continue };
            // Both sorted: one pass says whether `set` is inside `closure`,
            // and where.
            let mut at = Vec::with_capacity(set.len());
            let mut i = 0;
            let inside = set.iter().all(|f| {
                while closure.get(i).is_some_and(|c| c < f) {
                    i += 1;
                }
                let found = closure.get(i) == Some(f);
                if found {
                    at.push(i);
                }
                found
            });
            if inside && !children.iter().any(|c| c.as_str() == key.as_str()) {
                for i in at {
                    if let Some(c) = covered.get_mut(i) {
                        *c = true;
                    }
                }
                children.push(key);
            }
        }
        let mut out = NODE_FORMAT.to_vec();
        let own: Vec<u32> =
            closure.iter().zip(&covered).filter(|(_, c)| !**c).map(|(f, _)| *f).collect();
        put_u32(&mut out, own.len() as u32);
        for f in own {
            let hash = self.hash(f);
            put_text(&mut out, self.names.get(f as usize).map_or("", String::as_str));
            put_u64(&mut out, hash);
        }
        children.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        put_u32(&mut out, children.len() as u32);
        for child in &children {
            put_text(&mut out, child.as_str());
        }
        let key = ActionKey::of(&out);
        if self.cache.entry(&key).is_none() {
            self.cache.put(&key, &out);
        }
        self.holds.insert(key.as_str().to_string(), true);
        self.sets.insert(key.as_str().to_string(), std::rc::Rc::new(closure.to_vec()));
        key
    }

    /// The key one target's findings are filed under: the action, the build
    /// mode, this toolchain's version, the target, and the build graph.
    ///
    /// The graph is in the key rather than in the record because it decides
    /// what the closure *is*: a `BUILD.buri` edit can add a dependency, and a
    /// file appearing can turn a finding on, neither of which shows in the
    /// bytes of any file already in the closure.
    fn key(&self, session: &Session, target: TargetId) -> ActionKey {
        let kind = target.kind.name();
        let mut builder = KeyBuilder::new(Action::Lint, self.mode);
        // The declared sources are in the graph hash below, which is over the
        // bytes of every build file, so they are not listed again here.
        builder.rule_identity(&session.workspace.label(target), kind, &[]);
        builder.input("graph", &self.graph.to_le_bytes());
        builder.finish()
    }

    /// What the last run said about this target, if every file it read still
    /// holds the bytes it read.
    ///
    /// The spans come back pointing into this run's source map: a file the
    /// record names is found by name, or read into the map under it. A name
    /// with no file behind it makes the whole record unusable, and the target
    /// is analysed instead.
    ///
    /// Says nothing: [`Store::reused`] says so when the caller replays it, so
    /// `--explain` lists the targets in the order the report reads them.
    pub fn recall(
        &mut self,
        session: &mut Session,
        target: TargetId,
        first_in_package: bool,
    ) -> Option<Parts> {
        let key = self.key(session, target);
        let bytes = self.cache.get(&key)?;
        let mut reader = Reader::new(&bytes)?;
        let asked_the_package = reader.byte()? == 1;
        if first_in_package && !asked_the_package {
            return None;
        }
        let node = ActionKey::parse(&reader.text()?)?;
        if !self.holds(&node) {
            return None;
        }
        let parts = Parts {
            analysis: read_findings(&mut reader, &mut session.map, &self.root)?,
            package: read_findings(&mut reader, &mut session.map, &self.root)?,
            target: read_findings(&mut reader, &mut session.map, &self.root)?,
            asked_the_package,
        };
        self.nodes.insert(target, node);
        Some(parts)
    }

    /// `--explain`'s line for a target [`Store::recall`] answered.
    pub fn reused(&self, session: &Session, target: TargetId) {
        let key = self.key(session, target);
        self.say(Status::Cached, session, target, &key);
    }

    /// Takes down what this run found, under the closure it read
    /// ([`crate::build::sources::closure_of`]). Written by [`Store::flush`].
    pub fn remember(
        &mut self,
        session: &Session,
        target: TargetId,
        closure: &[PathBuf],
        parts: &Parts,
    ) {
        let key = self.key(session, target);
        self.say(Status::Run, session, target, &key);
        let mut ids: Vec<u32> = closure.iter().map(|path| self.id_of_path(session, path)).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut findings = Vec::new();
        for list in [&parts.analysis, &parts.package, &parts.target] {
            put_u32(&mut findings, list.len() as u32);
            for d in list {
                put_diagnostic(&mut findings, &session.map, d);
            }
        }
        self.pending.push(Pending { target, closure: ids, asked_the_package: parts.asked_the_package, findings });
    }

    /// Writes what [`Store::remember`] took. A target's node can name its
    /// dependencies' only once theirs exist, so the smallest closures go
    /// first: a dependency's closure is inside its dependent's.
    pub fn flush(&mut self, session: &Session) {
        let mut pending = std::mem::take(&mut self.pending);
        pending.sort_by_key(|p| p.closure.len());
        for p in pending {
            let node = self.node_of(session, p.target, &p.closure);
            self.nodes.insert(p.target, node.clone());
            let mut out = FORMAT.to_vec();
            out.push(u8::from(p.asked_the_package));
            put_text(&mut out, node.as_str());
            out.extend_from_slice(&p.findings);
            let key = self.key(session, p.target);
            self.cache.put(&key, &out);
        }
    }

    /// `--explain`'s line for one target: what became of it, and the key.
    ///
    /// The same fields the build's transcript uses, with `-` where a build
    /// writes the platform: a lint asks one question of a target's whole
    /// closure whatever that target is built for.
    fn say(&self, status: Status, session: &Session, target: TargetId, key: &ActionKey) {
        crate::build::cache::explain_without_platform(
            self.explain,
            status,
            Action::Lint,
            &session.workspace.label(target),
            key,
        );
    }
}

// ---------------------------------------------------------------------------
// The encoding
// ---------------------------------------------------------------------------

pub(crate) fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Length-prefixed rather than delimited: a message, a note and a fix are all
/// prose a diagnostic wrote, and any delimiter one of them may hold is one an
/// escape rule would then have to be right about.
pub(crate) fn put_text(out: &mut Vec<u8>, text: &str) {
    put_u32(out, text.len() as u32);
    out.extend_from_slice(text.as_bytes());
}

fn put_optional(out: &mut Vec<u8>, text: Option<&String>) {
    match text {
        Some(text) => {
            out.push(1);
            put_text(out, text);
        }
        None => out.push(0),
    }
}

/// A span, with its file named rather than numbered: a [`FileId`] is an index
/// into one process's source map and means nothing in the next one.
pub(crate) fn put_span(out: &mut Vec<u8>, map: &SourceMap, span: Span) {
    put_text(out, if span.is_none() { "" } else { map.name(span.file) });
    put_u32(out, span.start);
    put_u32(out, span.end);
}

pub(crate) fn put_diagnostic(out: &mut Vec<u8>, map: &SourceMap, d: &Diagnostic) {
    put_diagnostic_with(out, d, &mut |out, span| put_span(out, map, span));
}

/// [`put_diagnostic`], with each span written by `put_span`. `buri test`'s
/// build records write theirs relative to the tokens around them.
pub(crate) fn put_diagnostic_with(
    out: &mut Vec<u8>,
    d: &Diagnostic,
    put_span: &mut dyn FnMut(&mut Vec<u8>, Span),
) {
    out.push(match d.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Note => 2,
    });
    put_text(out, &d.message);
    put_span(out, d.span);
    put_optional(out, d.label.as_ref());
    put_optional(out, d.expected.as_ref());
    put_optional(out, d.actual.as_ref());
    put_u32(out, d.notes.len() as u32);
    for note in &d.notes {
        put_text(out, note);
    }
    put_optional(out, d.fix.as_ref());
    put_u32(out, d.secondary_spans.len() as u32);
    for secondary in &d.secondary_spans {
        put_span(out, secondary.span);
        put_text(out, &secondary.label);
    }
    put_optional(out, d.code.as_ref());
    put_u32(out, d.edits.len() as u32);
    for edit in &d.edits {
        put_span(out, edit.at);
        put_text(out, &edit.replacement);
    }
}

/// A cursor over a record. Every read is fallible and none of them panics: the
/// bytes came off a disk, and a truncated file has to read as a miss.
///
/// `buri test` reads its build records with it too (`commands/test.rs`).
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Option<Reader<'a>> {
        Reader::after(FORMAT, bytes)
    }

    /// A cursor past `format`, for a record that starts with it.
    pub(crate) fn after(format: &[u8], bytes: &'a [u8]) -> Option<Reader<'a>> {
        (bytes.get(..format.len()) == Some(format)).then_some(Reader { bytes, at: format.len() })
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub(crate) fn byte(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    pub(crate) fn text(&mut self) -> Option<String> {
        let length = self.u32()? as usize;
        String::from_utf8(self.take(length)?.to_vec()).ok()
    }

    fn optional(&mut self) -> Option<Option<String>> {
        match self.byte()? {
            0 => Some(None),
            1 => Some(Some(self.text()?)),
            _ => None,
        }
    }
}

/// The file a recorded name stands for in this run's source map, reading it in
/// if nothing has yet.
///
/// `None` for a name with no file behind it — an embedded standard library
/// module, or one generated from a schema — which is what makes such a record
/// unusable rather than silently misplaced.
pub(crate) fn place(map: &mut SourceMap, root: &Path, name: &str) -> Option<FileId> {
    if name.is_empty() {
        return Some(FileId::NONE);
    }
    if let Some(id) = map.find(name) {
        return Some(id);
    }
    map.load(name, &root.join(name)).ok()
}

pub(crate) fn read_span(reader: &mut Reader, map: &mut SourceMap, root: &Path) -> Option<Span> {
    let name = reader.text()?;
    let file = place(map, root, &name)?;
    let start = reader.u32()?;
    let end = reader.u32()?;
    Some(Span { file, start, end })
}

fn read_findings(
    reader: &mut Reader,
    map: &mut SourceMap,
    root: &Path,
) -> Option<Vec<Diagnostic>> {
    let count = reader.u32()?;
    let mut out = Vec::new();
    for _ in 0..count {
        out.push(read_diagnostic(reader, map, root)?);
    }
    Some(out)
}

pub(crate) fn read_diagnostic(
    reader: &mut Reader,
    map: &mut SourceMap,
    root: &Path,
) -> Option<Diagnostic> {
    read_diagnostic_with(reader, &mut |reader| read_span(reader, map, root))
}

/// The inverse of [`put_diagnostic_with`], with each span read by `read_span`.
pub(crate) fn read_diagnostic_with(
    reader: &mut Reader,
    read_span: &mut dyn FnMut(&mut Reader) -> Option<Span>,
) -> Option<Diagnostic> {
    let severity = match reader.byte()? {
        0 => Severity::Error,
        1 => Severity::Warning,
        2 => Severity::Note,
        _ => return None,
    };
    let message = reader.text()?;
    let span = read_span(reader)?;
    let label = reader.optional()?;
    let expected = reader.optional()?;
    let actual = reader.optional()?;
    let mut notes = Vec::new();
    for _ in 0..reader.u32()? {
        notes.push(reader.text()?);
    }
    let fix = reader.optional()?;
    let mut secondary_spans = Vec::new();
    for _ in 0..reader.u32()? {
        let span = read_span(reader)?;
        secondary_spans.push(SecondarySpan { span, label: reader.text()? });
    }
    let code = reader.optional()?;
    let mut edits = Vec::new();
    for _ in 0..reader.u32()? {
        let at = read_span(reader)?;
        edits.push(Edit { at, replacement: reader.text()? });
    }
    Some(Diagnostic {
        severity,
        message,
        span,
        label,
        expected,
        actual,
        notes,
        fix,
        secondary_spans,
        code,
        edits,
        // Not stored: the rendered fields are ordinary strings by now, and the
        // template is read only by a debug invariant about the emission site.
        template: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map with one file, and a scratch directory holding that file, so that
    /// `place` can find it by name the way a second run would.
    fn one_file() -> (SourceMap, PathBuf, FileId) {
        let root = std::env::temp_dir().join(format!("buri-lint-record-{}", std::process::id()));
        let _ = std::fs::create_dir_all(root.join("lib/money"));
        let text = "export fn cents(): I64 { 1 }\n";
        let _ = std::fs::write(root.join("lib/money/unit.buri"), text);
        let mut map = SourceMap::new();
        let id = map.add(
            "lib/money/unit.buri",
            root.join("lib/money/unit.buri"),
            text.to_string(),
        );
        (map, root, id)
    }

    /// Every field of a diagnostic survives the round trip, including the ones
    /// a rule fills in only sometimes.
    ///
    /// The claim the whole file rests on is that a cached finding prints what a
    /// fresh one prints, and what a finding prints is these fields — so a field
    /// added above and forgotten here fails on the next line.
    #[test]
    fn a_finding_survives_the_round_trip() {
        let (mut map, root, id) = one_file();
        let d = Diagnostic {
            severity: Severity::Warning,
            message: "cents is imported but not used".to_string(),
            span: Span::new(id, 7, 12),
            label: Some("this import".to_string()),
            expected: Some("`Str`".to_string()),
            actual: Some("`Cents`".to_string()),
            notes: vec!["one note".to_string(), "a note\nwith a newline in it".to_string()],
            fix: Some("remove the import".to_string()),
            secondary_spans: vec![SecondarySpan {
                span: Span::new(id, 0, 4),
                label: "declared here".to_string(),
            }],
            code: Some("unused-import".to_string()),
            edits: vec![Edit { at: Span::new(id, 0, 29), replacement: String::new() }],
            template: None,
        };

        let mut bytes = FORMAT.to_vec();
        put_u32(&mut bytes, 1);
        put_diagnostic(&mut bytes, &map, &d);
        let mut reader = Reader::new(&bytes).expect("the format line is there");
        let back = read_findings(&mut reader, &mut map, &root).expect("the record reads");

        let one = back.first().expect("one finding");
        assert_eq!(one.severity, d.severity);
        assert_eq!(one.message, d.message);
        assert_eq!(one.span, d.span);
        assert_eq!(one.label, d.label);
        assert_eq!(one.expected, d.expected);
        assert_eq!(one.actual, d.actual);
        assert_eq!(one.notes, d.notes);
        assert_eq!(one.fix, d.fix);
        assert_eq!(one.code, d.code);
        assert_eq!(one.edits.len(), 1);
        assert_eq!(one.edits.first().map(|e| e.at), Some(Span::new(id, 0, 29)));
        assert_eq!(one.secondary_spans.len(), 1);
        assert_eq!(
            one.secondary_spans.first().map(|s| s.label.clone()),
            Some("declared here".to_string())
        );
        // The renderings are the same string, which is the claim the fields
        // above are only evidence for.
        assert_eq!(map.render(one, false), map.render(&d, false));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A span with no location keeps having none: its file is written as the
    /// empty name rather than as an index nothing in the next run minted.
    #[test]
    fn a_span_with_no_location_survives() {
        let (mut map, root, _) = one_file();
        let d = Diagnostic::error(Span::NONE, "in no file at all");
        let mut bytes = FORMAT.to_vec();
        put_u32(&mut bytes, 1);
        put_diagnostic(&mut bytes, &map, &d);
        let mut reader = Reader::new(&bytes).expect("the format line is there");
        let back = read_findings(&mut reader, &mut map, &root).expect("the record reads");
        assert!(back.first().is_some_and(|d| d.span.is_none()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Bytes that are not a record of this shape read as a miss, at any length.
    /// The decoder is pointed at every prefix of a real record, because a
    /// truncated file is what a write interrupted mid-flight would leave.
    #[test]
    fn a_truncated_or_foreign_record_reads_as_a_miss() {
        let (mut map, root, id) = one_file();
        let d = Diagnostic::error(Span::new(id, 0, 4), "something");
        let mut bytes = FORMAT.to_vec();
        put_u32(&mut bytes, 1);
        put_diagnostic(&mut bytes, &map, &d);

        assert!(Reader::new(b"buri-lint-findings-0\n").is_none(), "another format was read");
        assert!(Reader::new(b"").is_none(), "an empty file was read");
        for length in 0..bytes.len() {
            let Some(prefix) = bytes.get(..length) else { continue };
            let read = Reader::new(prefix)
                .and_then(|mut r| read_findings(&mut r, &mut map, &root));
            assert!(read.is_none(), "a record truncated to {length} bytes was read");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
