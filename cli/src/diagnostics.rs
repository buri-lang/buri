//! Diagnostics: source map, spans, and the renderer.
//!
//! The output shape is fixed by the examples throughout `SPEC.md` and
//! `cli/src/docs/`:
//!
//! ```text
//! error: cmd/server/routes.buri imports //lib/money, which is not in deps
//!   --> cmd/server/routes.buri:3:6
//!    |
//!  3 | from "//lib/money" import { Cents, format };
//!    |      ^^^^^^^^^^^^^
//!    |
//!    = expected: a module path this target may see
//!    = actual: //lib/money, which is not among cmd/server's dependencies
//!    = fix: add "//lib/money" to deps in cmd/server/BUILD.buri
//! ```
//!
//! Four things every diagnostic answers, in a fixed order, so that neither a
//! person nor a program has to infer any of them:
//!
//! * **where** — the span, rendered as a caret under the source line
//! * **expected** — what the language required at that location
//! * **actual** — what the source says instead
//! * **fix** — the concrete edit that resolves it
//!
//! `expected` and `actual` are omitted where the error is not a mismatch (a
//! duplicate declaration has no "expected"), but `fix` is not: if a diagnostic
//! cannot say what to do about it, it is not finished. The reject corpus
//! asserts exactly that, case by case.
//!
//! [`Diagnostic::to_json`] renders the same content as one JSON object per
//! line, for `buri <cmd> --error-format=json`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The one thing the toolchain does when an invariant it is supposed to uphold
/// turns out not to hold.
///
/// The promise the lint set in `Cargo.toml` pins is that **no input panics the
/// compiler**: a malformed source file, build file, schema, flag, or
/// language-server message is a diagnostic and a clean exit. That leaves the
/// other kind of failure — the compiler contradicting itself, which no input
/// can cause and no user can act on. `unwrap` reports that as a backtrace
/// through frames nobody outside this repository can read; this reports it as
/// a sentence saying what broke, that it is a toolchain bug rather than a bug
/// in the caller's code, and where to send it.
///
/// It exits rather than unwinding because there is nothing to recover to: the
/// state that violated the invariant is the state a recovery would run in. 70
/// is `EX_SOFTWARE`, which is what it is for.
///
/// Every call site is a claim that input cannot reach it, and carries the
/// comment saying what makes that true. A site that a hostile file *can* reach
/// is a bug in the site, not a use for this.
#[cold]
#[inline(never)]
#[allow(
    clippy::print_stderr,
    reason = "an internal error has no Session to route through — it may be the Session that broke"
)]
pub fn ice(what: &str) -> ! {
    eprintln!("internal compiler error: {what}");
    eprintln!("  = this is a bug in the Buri toolchain, not in the code it was given");
    eprintln!("  = fix: please report it, with the input that produced it, at");
    eprintln!("         https://github.com/buri-lang/buri/issues");
    std::process::exit(70)
}

/// [`ice`] with a formatted message: `ice!("no slot for {name}")`.
#[macro_export]
macro_rules! ice {
    ($($arg:tt)*) => { $crate::diagnostics::ice(&format!($($arg)*)) };
}

/// `.expect(…)` for an invariant, without the panic.
///
/// Reads as the claim it is — `or_ice("every id in this table was minted by
/// it")` — and fails the way [`ice`] does rather than the way `unwrap` does.
pub trait Invariant<T> {
    /// The value, or [`ice`] with `what` if the invariant did not hold.
    fn or_ice(self, what: &str) -> T;
}

impl<T> Invariant<T> for Option<T> {
    fn or_ice(self, what: &str) -> T {
        match self {
            Some(v) => v,
            None => ice(what),
        }
    }
}

impl<T, E: std::fmt::Display> Invariant<T> for Result<T, E> {
    fn or_ice(self, what: &str) -> T {
        match self {
            Ok(v) => v,
            Err(e) => ice(&format!("{what}: {e}")),
        }
    }
}

/// A repository bug, reported where a test will see it and nowhere else.
///
/// The templates in the documentation pages are written by hand, and the three
/// ways they can be wrong — no page, an unbound placeholder, a binding nothing
/// uses — are all mistakes in *this* repository. In a debug build, which is
/// every test run, each is a panic naming the code and the placeholder. In a
/// release build none of them is worth taking a user's build down for: the
/// template prints as written, which is a legible bug report.
#[allow(
    clippy::panic,
    reason = "a documentation page that does not match its emission site is a bug in this \
              repository rather than in the code being compiled, and this is the debug-only \
              panic that says so; the release build prints the template instead"
)]
fn debug_panic(what: &str) {
    #[cfg(debug_assertions)]
    panic!("{what}");
    #[cfg(not(debug_assertions))]
    let _ = what;
}

/// Index into the [`SourceMap`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FileId(pub u32);

impl FileId {
    /// A span with no file, for diagnostics that are about the invocation
    /// rather than about a location in a file.
    pub const NONE: FileId = FileId(u32::MAX);
}

/// A byte range within one file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: FileId, start: usize, end: usize) -> Span {
        Span { file, start: start as u32, end: end as u32 }
    }

    /// A zero-width span, used where a construct is missing rather than wrong.
    pub fn point(file: FileId, at: usize) -> Span {
        Span::new(file, at, at)
    }

    pub const NONE: Span = Span { file: FileId::NONE, start: 0, end: 0 };

    pub fn is_none(&self) -> bool {
        self.file == FileId::NONE
    }

    /// The span covering both, assuming they are in the same file.
    pub fn to(self, other: Span) -> Span {
        if self.is_none() {
            return other;
        }
        if other.is_none() {
            return self;
        }
        Span { file: self.file, start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

impl Default for Span {
    fn default() -> Span {
        Span::NONE
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// A secondary span, rendered after the primary one with its own caret line.
#[derive(Clone, Debug)]
pub struct SecondarySpan {
    pub span: Span,
    pub label: String,
}

/// The page a templated diagnostic takes its wording from, and what has been
/// bound into it.
///
/// The rendered fields below are kept in step with this on every `bind`, so
/// `message`, `label`, `notes` and `fix` are ordinary strings by the time
/// anything reads them — deduplication, `--error-format=json`, and the language
/// server all predate templates and none of them has to know about one.
#[derive(Clone, Debug)]
pub struct Template {
    page: &'static crate::documentation::frontmatter::Page,
    bindings: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    /// Text printed beside the carets.
    pub label: Option<String>,
    /// What the language required at this location. `None` where the error is
    /// not a mismatch and inventing one would be noise.
    pub expected: Option<String>,
    /// What the source says instead.
    pub actual: Option<String>,
    /// The `= ...` lines beneath the snippet: the background a reader needs,
    /// as opposed to the edit, which is `fix`.
    pub notes: Vec<String>,
    /// The concrete edit that resolves this. Every diagnostic has one.
    pub fix: Option<String>,
    pub secondary_spans: Vec<SecondarySpan>,
    /// Lint name, for `buri lint` output.
    pub code: Option<String>,
    /// The `fix` as bytes, when it has exactly one mechanical form. Empty for
    /// almost every diagnostic: `fix` is prose a reader acts on, and a rule
    /// whose answer needs a judgement call must not pretend otherwise.
    pub edits: Vec<Edit>,
    /// Where the wording came from, for a diagnostic built by
    /// [`Diagnostic::templated`]. `None` for one whose site writes its own.
    pub template: Option<Template>,
}

/// A replacement of one byte range.
///
/// The range is a [`Span`] rather than a repeat of its three fields, because a
/// second copy of "file, start, end" is a second opportunity for the two to
/// disagree about the same edit.
#[derive(Clone, Debug)]
pub struct Edit {
    pub at: Span,
    pub replacement: String,
}

/// Why a set of edits could not be applied.
///
/// Applying an edit is `String::replace_range`, which *panics* on an inverted
/// range, on an offset past the end, and on an offset that is not a UTF-8
/// character boundary. A rewriting tool must not have a panic as its failure
/// mode, so every one of those is checked once, at the point where an
/// [`EditSet`] is built, and reported as a refusal instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadEdits {
    /// An edit for a different file than the set is for.
    WrongFile,
    /// `start > end`.
    Inverted { start: u32, end: u32 },
    /// An offset past the end of the file.
    PastEnd { offset: u32, len: usize },
    /// An offset in the middle of a multi-byte character.
    NotACharBoundary { offset: u32 },
    /// Two edits that both claim the same byte.
    Overlap { first_end: u32, second_start: u32 },
}

impl std::fmt::Display for BadEdits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BadEdits::WrongFile => write!(f, "an edit for another file"),
            BadEdits::Inverted { start, end } => {
                write!(f, "an edit whose range runs backwards ({start}..{end})")
            }
            BadEdits::PastEnd { offset, len } => {
                write!(f, "an edit at offset {offset}, past the end of the {len}-byte file")
            }
            BadEdits::NotACharBoundary { offset } => {
                write!(f, "an edit at offset {offset}, which is inside a character")
            }
            BadEdits::Overlap { first_end, second_start } => {
                write!(f, "overlapping fixes ({first_end} > {second_start})")
            }
        }
    }
}

/// Every edit for one file, sorted and known to be applicable.
///
/// The invariants — one file, ascending, non-overlapping, in bounds, on
/// character boundaries — hold by construction, so [`EditSet::apply`] is total:
/// there is no arrangement of an `EditSet` that makes it fail.
#[derive(Clone, Debug)]
pub struct EditSet {
    file: FileId,
    /// Ascending by start, non-overlapping, in bounds of the text checked at
    /// construction.
    edits: Vec<Edit>,
}

impl EditSet {
    /// Validates every edit against `text`, which must be the current contents
    /// of `file`. Returns the first reason the set cannot be applied.
    pub fn new(
        file: FileId,
        text: &str,
        edits: impl IntoIterator<Item = Edit>,
    ) -> Result<EditSet, BadEdits> {
        let mut edits: Vec<Edit> = edits.into_iter().collect();
        for e in &edits {
            if e.at.file != file {
                return Err(BadEdits::WrongFile);
            }
            if e.at.start > e.at.end {
                return Err(BadEdits::Inverted { start: e.at.start, end: e.at.end });
            }
            for offset in [e.at.start, e.at.end] {
                if offset as usize > text.len() {
                    return Err(BadEdits::PastEnd { offset, len: text.len() });
                }
                if !text.is_char_boundary(offset as usize) {
                    return Err(BadEdits::NotACharBoundary { offset });
                }
            }
        }
        edits.sort_by_key(|e| (e.at.start, e.at.end));
        for pair in edits.windows(2) {
            let [first, second] = pair else { continue };
            if first.at.end > second.at.start {
                return Err(BadEdits::Overlap {
                    first_end: first.at.end,
                    second_start: second.at.start,
                });
            }
        }
        Ok(EditSet { file, edits })
    }

    pub fn file(&self) -> FileId {
        self.file
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    /// The edited text. Back to front, so an earlier edit's offsets are still
    /// the offsets of the text it is applied to.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for e in self.edits.iter().rev() {
            out.replace_range(e.at.start as usize..e.at.end as usize, &e.replacement);
        }
        out
    }
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            span,
            label: None,
            expected: None,
            actual: None,
            notes: Vec::new(),
            fix: None,
            secondary_spans: Vec::new(),
            code: None,
            edits: Vec::new(),
            template: None,
        }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Warning, ..Diagnostic::error(span, message) }
    }

    /// A diagnostic whose wording is the page's rather than this call site's.
    ///
    /// The severity, the message, the label, the note and the fix all come from
    /// the frontmatter of `cli/src/docs/errors/<code>.md` (or `lints/`), and
    /// what the call site supplies is the data: `.bind("function", name)` fills
    /// the `{function}` the page's fix is written around. One code is one
    /// message, so a rule whose two sites need different sentences is two
    /// rules.
    ///
    /// A code with no page, an unbound placeholder, and a binding no template
    /// uses are each a bug in this repository rather than in the code being
    /// compiled, so each panics in a debug build — which is every test run —
    /// and degrades to printing the template as written in a release one. A
    /// user meeting a `{function}` has a bug report; a user meeting a crash has
    /// lost their build.
    pub fn templated(code: &str, span: Span) -> Diagnostic {
        let mut d = Diagnostic::error(span, code.to_string());
        d.code = Some(code.to_string());
        match crate::documentation::page_of_code(code) {
            Some(page) => {
                d.severity = page.front.severity;
                d.template = Some(Template { page, bindings: Vec::new() });
                d.render_template();
            }
            None => debug_panic(&format!(
                "`{code}` has no documentation page with frontmatter, so \
                 `Diagnostic::templated` has no message to print. Add \
                 cli/src/docs/errors/{code}.md with a `---` block, and register it in \
                 documentation/errors.rs"
            )),
        }
        d
    }

    /// Binds one placeholder. The value is the finished phrase — a template has
    /// no filters, so anything conditional is decided here.
    pub fn bind(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Diagnostic {
        let (name, value) = (name.into(), value.into());
        if let Some(t) = &mut self.template {
            match t.bindings.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = value,
                None => t.bindings.push((name, value)),
            }
        }
        self.render_template();
        self
    }

    pub fn with_bind(mut self, name: impl Into<String>, value: impl Into<String>) -> Diagnostic {
        self.bind(name, value);
        self
    }

    /// Re-renders the templated fields from the page, with whatever is bound so
    /// far. Cheap — four short strings and a handful of bindings — and done on
    /// every `bind` so that no reader of a `Diagnostic` has to know that the
    /// wording came from a template.
    fn render_template(&mut self) {
        let Some(t) = &self.template else { return };
        let front = &t.page.front;
        let bindings = &t.bindings;
        let fill = |template: &str| crate::documentation::frontmatter::render(template, bindings);
        self.message = fill(&front.message);
        self.label = front.label.as_deref().map(&fill);
        self.fix = front.fix.as_deref().map(&fill);
        // The page's note is the first of them, put there when the diagnostic
        // was built, so a re-render replaces it rather than the call site's own.
        if let Some(note) = front.note.as_deref() {
            let rendered = fill(note);
            match self.notes.first_mut() {
                Some(first) => *first = rendered,
                None => self.notes.push(rendered),
            }
        }
    }

    /// The check that makes a mistyped binding a failure rather than a sentence
    /// with a hole in it. Debug builds only, on the way to being rendered.
    fn check_template(&self) {
        #[cfg(debug_assertions)]
        if let Some(t) = &self.template {
            let front = &t.page.front;
            let templates: Vec<&str> = [
                Some(front.message.as_str()),
                front.label.as_deref(),
                front.note.as_deref(),
                front.fix.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect();
            let holes: Vec<&str> = templates
                .iter()
                .flat_map(|t| crate::documentation::frontmatter::placeholders(t))
                .collect();
            for hole in &holes {
                if !t.bindings.iter().any(|(n, _)| n == hole) {
                    debug_panic(&format!(
                        "`{}` prints `{{{hole}}}`, and nothing bound it: add \
                         `.bind(\"{hole}\", …)` at the emission site",
                        t.page.code
                    ));
                }
            }
            for (name, _) in &t.bindings {
                if !holes.iter().any(|h| h == name) {
                    debug_panic(&format!(
                        "`{}` binds `{name}`, and no template of its page uses it: either the \
                         page has a typo or the binding is left over",
                        t.page.code
                    ));
                }
            }
        }
    }

    /// The chaining forms, for the `Diagnostic` a `push` is about to take.
    ///
    /// Each is its in-place twin below plus `self`. They used to be written out
    /// twice — seven pairs of byte-identical bodies — so a field that gained a
    /// rule gained it in one of the two.
    pub fn with_label(mut self, label: impl Into<String>) -> Diagnostic {
        self.label(label);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.note(note);
        self
    }

    pub fn with_secondary_span(mut self, span: Span, label: impl Into<String>) -> Diagnostic {
        self.secondary_span(span, label);
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Diagnostic {
        self.code(code);
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Diagnostic {
        self.fix(fix);
        self
    }

    pub fn with_edit(mut self, at: Span, replacement: &str) -> Diagnostic {
        self.edit(at, replacement);
        self
    }

    pub fn with_mismatch(
        mut self,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Diagnostic {
        self.mismatch(expected, actual);
        self
    }

    /// The in-place forms, for use on the `&mut Diagnostic` a sink hands back.
    pub fn note(&mut self, note: impl Into<String>) -> &mut Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn secondary_span(&mut self, span: Span, label: impl Into<String>) -> &mut Diagnostic {
        self.secondary_spans.push(SecondarySpan { span, label: label.into() });
        self
    }

    pub fn label(&mut self, label: impl Into<String>) -> &mut Diagnostic {
        self.label = Some(label.into());
        self
    }

    /// A stable name for the rule this diagnostic enforces.
    ///
    /// The code is what `buri docs error <code>` explains, and what a reader
    /// greps for. It is kebab-case rather than a number because a number is
    /// unsearchable in a repository whose whole aesthetic is that the message
    /// says the thing.
    pub fn code(&mut self, code: impl Into<String>) -> &mut Diagnostic {
        self.code = Some(code.into());
        self
    }

    pub fn fix(&mut self, fix: impl Into<String>) -> &mut Diagnostic {
        self.fix = Some(fix.into());
        self
    }

    /// The byte form of the fix. Only for a rule whose answer is mechanical —
    /// `--fix` applies these without asking.
    pub fn edit(&mut self, at: Span, replacement: &str) -> &mut Diagnostic {
        self.edits.push(Edit { at, replacement: replacement.to_string() });
        self
    }

    /// The two halves of a mismatch, which are almost always set together.
    pub fn mismatch(
        &mut self,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> &mut Diagnostic {
        self.expected = Some(expected.into());
        self.actual = Some(actual.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

#[derive(Clone)]
pub struct SourceFile {
    /// Repository-relative where possible, so that two checkouts in different
    /// directories produce identical diagnostics and identical cache keys.
    pub name: String,
    pub abs_path: PathBuf,
    pub text: String,
    /// Byte offset of the start of each line.
    line_starts: Vec<u32>,
}

impl SourceFile {
    fn new(name: String, abs_path: PathBuf, text: String) -> SourceFile {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                // Saturating rather than `as`, so the one file that does not
                // fit in a `u32` of offsets degrades to a wrong line number
                // instead of wrapping to offset 0. `SourceMap::load` turns that
                // file away before it gets here; this is the belt to that
                // brace, for text handed to `add` directly.
                line_starts.push(u32::try_from(i.saturating_add(1)).unwrap_or(u32::MAX));
            }
        }
        SourceFile { name, abs_path, text, line_starts }
    }

    /// The largest offset at or below `at` that starts a character.
    ///
    /// A span does not have to land on a character boundary to reach here: a
    /// language-server position is arithmetic on numbers the client sent, and
    /// `--error-format=json` will happily be asked about byte 3 of a two-byte
    /// `é`. Slicing on such an offset panics, so every offset is walked back to
    /// a boundary first, which at worst widens the report by one character.
    fn floor_boundary(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while at > 0 && !self.text.is_char_boundary(at) {
            at = at.saturating_sub(1);
        }
        at
    }

    /// 1-based line and column (column counted in characters, not bytes).
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let offset = self.floor_boundary(offset as usize);
        // `line_starts` opens with 0 and `offset` is not negative, so the
        // insertion point is never 0 and the subtraction never wraps; it
        // saturates rather than say so twice.
        let line = match self.line_starts.binary_search(&(offset as u32)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let start = self.line_starts.get(line).map_or(0, |&s| s as usize);
        let col = self.text.get(start..offset).map_or(0, |s| s.chars().count());
        (line.saturating_add(1), col.saturating_add(1))
    }

    /// The text of a 1-based line, empty for a line this file does not have.
    fn line_text(&self, line: usize) -> &str {
        let Some(start) = line.checked_sub(1).and_then(|i| self.line_starts.get(i)) else {
            return "";
        };
        let end = self.line_starts.get(line).map_or(self.text.len(), |&e| e as usize);
        self.text
            .get(*start as usize..end)
            .unwrap_or("")
            .trim_end_matches(['\n', '\r'])
    }

    /// The byte offset a 1-based line begins at, 0 for a line this file has
    /// not got.
    pub fn line_start(&self, line: usize) -> usize {
        line.checked_sub(1)
            .and_then(|i| self.line_starts.get(i))
            .map_or(0, |&s| s as usize)
    }
}

/// The most source one file may hold.
///
/// A [`Span`] is two `u32`s, because a syntax tree is mostly spans and eight
/// bytes each is what makes that affordable. So four gigabytes is the size of
/// file this can describe, and one byte more has to be turned away at the door
/// rather than silently given offsets that wrapped.
const MAX_SOURCE_BYTES: u64 = u32::MAX as u64;

/// Cheap to clone, and deliberately so. A file's text is shared rather than
/// copied, so a command that hands one analysis its own copy of the map is
/// copying a vector of pointers — which is what lets the parses [`SourceMap`]
/// gives file ids to outlive the one analysis that read them
/// (`build::sources`).
#[derive(Clone)]
pub struct SourceMap {
    files: Vec<Rc<SourceFile>>,
    /// `find` by name, which `load` calls for every file every compilation
    /// asks for. Scanning `files` made loading a repository quadratic in the
    /// number of files it has — invisible at ten and not at a thousand — and
    /// the map is append-only, so an index costs one insert per file.
    by_name: HashMap<String, FileId>,
    /// What [`SourceMap::get`] answers for a [`FileId`] this map never minted —
    /// `FileId::NONE` above all, which is the file of every span that has no
    /// location. Rendering a diagnostic is the last thing that happens before
    /// a user sees an error, and it is not allowed to be the thing that
    /// crashes; an empty file renders as no snippet, which is what a span with
    /// no location should look like anyway.
    missing: Rc<SourceFile>,
}

impl Default for SourceMap {
    fn default() -> SourceMap {
        SourceMap {
            files: Vec::new(),
            by_name: HashMap::new(),
            missing: Rc::new(SourceFile::new("<none>".to_string(), PathBuf::new(), String::new())),
        }
    }
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    pub fn add(&mut self, name: impl Into<String>, abs_path: PathBuf, text: String) -> FileId {
        let id = FileId(self.files.len() as u32);
        let name = name.into();
        self.by_name.insert(name.clone(), id);
        self.files.push(Rc::new(SourceFile::new(name, abs_path, text)));
        id
    }

    /// A source with no file behind it — an embedded standard library module —
    /// reusing the entry if that module is already here.
    ///
    /// Deliberately not `add`. The standard library's text is compiled into
    /// the binary, so two analyses that both reach `core/list` are looking at
    /// the same bytes, and giving them one `FileId` is what lets everything
    /// keyed on `FileId` — the parse cache above all — see that. Using `add`
    /// meant a process that analysed a hundred targets accumulated a hundred
    /// copies of the whole standard library in the map, text and line index
    /// and all.
    ///
    /// This is *not* the right call for a snippet: two snippets legitimately
    /// share a name and differ in their text, and they must stay separate.
    pub fn embedded(&mut self, name: &str, text: &str) -> FileId {
        if let Some(id) = self.find(name) {
            return id;
        }
        self.add(name, PathBuf::new(), text.to_string())
    }

    /// Load a file, reusing the entry if it is already present.
    ///
    /// A file larger than [`MAX_SOURCE_BYTES`] is refused here, at the only
    /// door source comes through, rather than by every offset downstream.
    pub fn load(&mut self, name: &str, abs_path: &Path) -> std::io::Result<FileId> {
        if let Some(id) = self.find(name) {
            return Ok(id);
        }
        if std::fs::metadata(abs_path).is_ok_and(|m| m.len() > MAX_SOURCE_BYTES) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("the file is larger than {MAX_SOURCE_BYTES} bytes, which is the most one source file may hold"),
            ));
        }
        let text = std::fs::read_to_string(abs_path)?;
        Ok(self.add(name, abs_path.to_path_buf(), text))
    }

    pub fn find(&self, name: &str) -> Option<FileId> {
        self.by_name.get(name).copied()
    }

    /// Puts new text under an id this map already minted, keeping the id.
    ///
    /// The id is what everything downstream of a parse is keyed by, and it
    /// stands for a file rather than for a revision of one — so a map kept
    /// between analyses re-reads an edited file into the entry it already has.
    /// Maps cloned before this call keep the text they were cloned with, which
    /// is what lets an analysis already in hand go on rendering its own spans.
    pub fn replace(&mut self, id: FileId, text: String) {
        let Some(slot) = self.files.get_mut(id.0 as usize) else { return };
        let (name, abs_path) = (slot.name.clone(), slot.abs_path.clone());
        *slot = Rc::new(SourceFile::new(name, abs_path, text));
    }

    /// Every file in this map, with the id it was minted under.
    pub fn entries(&self) -> impl Iterator<Item = (FileId, &SourceFile)> {
        self.files.iter().enumerate().map(|(i, f)| (FileId(i as u32), &**f))
    }

    /// Every file this map read from a path, in the order it read them.
    ///
    /// The embedded standard library and the snippets are skipped: neither has
    /// a path, and the caller asking this is asking which files on disk an
    /// answer was computed from.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files
            .iter()
            .map(|f| f.abs_path.as_path())
            .filter(|p| !p.as_os_str().is_empty())
    }

    /// The file an id names, or the empty stand-in if this map never minted it.
    pub fn get(&self, id: FileId) -> &SourceFile {
        self.files.get(id.0 as usize).unwrap_or(&self.missing)
    }

    pub fn name(&self, id: FileId) -> &str {
        if id == FileId::NONE {
            "<none>"
        } else {
            &self.get(id).name
        }
    }

    pub fn text(&self, id: FileId) -> &str {
        &self.get(id).text
    }

    /// The source text a span covers, empty for a span this map cannot place.
    ///
    /// Every way a span can be wrong — past the end, backwards, or landing
    /// inside a character — is answered with the empty string. A span arrives
    /// from a client's arithmetic as often as from the lexer's, and a caller
    /// that asks what text is under one is asking a question, not asserting an
    /// invariant.
    pub fn snippet(&self, span: Span) -> &str {
        if span.is_none() {
            return "";
        }
        let f = self.get(span.file);
        let start = f.floor_boundary(span.start as usize);
        let end = f.floor_boundary(span.end as usize).max(start);
        f.text.get(start..end).unwrap_or("")
    }

    /// [`SourceMap::render`], followed the first time a code is seen by the
    /// explanation from its page. Every diagnostic a command prints for a human
    /// leaves through here.
    pub fn render_with_body(&self, d: &Diagnostic, color: bool) -> String {
        let mut out = self.render(d, color);
        if let Some(body) = self.explanation(d, color) {
            out.push_str(&body);
        }
        out
    }

    pub fn render(&self, d: &Diagnostic, color: bool) -> String {
        d.check_template();
        let mut out = String::new();
        let (c_bold, c_red, c_yellow, c_blue, c_reset) = if color {
            ("\x1b[1m", "\x1b[1;31m", "\x1b[1;33m", "\x1b[1;34m", "\x1b[0m")
        } else {
            ("", "", "", "", "")
        };
        let sev_color = match d.severity {
            Severity::Error => c_red,
            Severity::Warning => c_yellow,
            Severity::Note => c_blue,
        };

        let head = match &d.code {
            Some(code) => format!("{}: {} [{}]", d.severity.label(), d.message, code),
            None => format!("{}: {}", d.severity.label(), d.message),
        };
        let (sev, rest) = head.split_at(d.severity.label().len());
        let _ = writeln!(out, "{sev_color}{sev}{c_reset}{c_bold}{rest}{c_reset}");

        if !d.span.is_none() {
            self.render_snippet(&mut out, d.span, d.label.as_deref(), sev_color, c_blue, c_reset);
            for secondary in &d.secondary_spans {
                let label = Some(secondary.label.as_str());
                self.render_snippet(&mut out, secondary.span, label, c_blue, c_blue, c_reset);
            }
            // The `=` sits in the gutter column, directly under the `|`s above
            // it, so the left edge of the block is a straight line.
            let gutter = self.gutter_width(d);
            for line in Self::trailer(d) {
                let mut lines = line.lines();
                if let Some(first) = lines.next() {
                    let _ = writeln!(out, "{:w$}{c_blue}={c_reset} {first}", "", w = gutter);
                }
                for cont in lines {
                    let _ = writeln!(out, "{:w$}  {cont}", "", w = gutter);
                }
            }
        } else {
            for line in Self::trailer(d) {
                let _ = writeln!(out, "  = {line}");
            }
        }
        out
    }

    /// The `= ...` block, in the order a reader needs it: what was required,
    /// what is there, why, and then what to do — the last line being the one
    /// to act on.
    fn trailer(d: &Diagnostic) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(e) = &d.expected {
            out.push(format!("expected: {e}"));
        }
        if let Some(a) = &d.actual {
            out.push(format!("actual: {a}"));
        }
        out.extend(d.notes.iter().cloned());
        if let Some(f) = &d.fix {
            out.push(format!("fix: {f}"));
        }
        out
    }

    /// The page's explanation, indented under the trailer and dimmed, or
    /// `None` — which is the usual answer.
    ///
    /// It is printed once per code per run. A file with forty type mismatches
    /// in it wants the paragraph once; printing it forty times would bury the
    /// forty locations, which are the part that differs.
    fn explanation(&self, d: &Diagnostic, color: bool) -> Option<String> {
        if !bodies_are_printed() {
            return None;
        }
        let code = d.code.as_deref()?;
        let page = crate::documentation::page_of_code(code)?;
        let body = crate::documentation::explanation_of(page);
        if body.trim().is_empty() || !first_sighting(code) {
            return None;
        }
        let indent = " ".repeat(self.gutter_width(d).saturating_add(2));
        let width = crate::documentation::Width::new(
            crate::documentation::terminal_width().saturating_sub(indent.len()),
        );
        // Rendered without colour and dimmed a line at a time: the escape a
        // heading would carry ends with a reset, and a reset in the middle of a
        // dimmed line un-dims the rest of it.
        let rendered = crate::documentation::markdown::to_terminal(&body, width, false);
        let (dim, reset) = if color {
            (crate::documentation::markdown::DIM, crate::documentation::markdown::RESET)
        } else {
            ("", "")
        };
        let mut out = String::from("\n");
        for line in rendered.trim_end().lines() {
            if line.trim().is_empty() {
                out.push('\n');
            } else {
                let _ = writeln!(out, "{indent}{dim}{line}{reset}");
            }
        }
        out.push('\n');
        Some(out)
    }

    /// One JSON object per diagnostic, for `--error-format=json`.
    ///
    /// The shape is flat and the field names are the questions they answer, so
    /// a consumer needs no schema to use it: `severity`, `message`,
    /// `location`, `expected`, `actual`, `notes`, `fix`, and `related`. The
    /// content is the same as the human form; the rendering is not repeated
    /// here, because a consumer of this wants the fields, not a picture of
    /// them.
    pub fn to_json(&self, d: &Diagnostic) -> String {
        d.check_template();
        let mut out = String::from("{");
        let _ = write!(out, "\"severity\":{}", json_str(d.severity.label()));
        let _ = write!(out, ",\"message\":{}", json_str(&d.message));
        if let Some(code) = &d.code {
            let _ = write!(out, ",\"code\":{}", json_str(code));
        }
        out.push_str(",\"location\":");
        out.push_str(&self.json_location(d.span, d.label.as_deref()));
        for (key, value) in [("expected", &d.expected), ("actual", &d.actual), ("fix", &d.fix)] {
            if let Some(v) = value {
                let _ = write!(out, ",\"{key}\":{}", json_str(v));
            }
        }
        out.push_str(",\"notes\":[");
        for (i, n) in d.notes.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&json_str(n));
        }
        out.push_str("],\"related\":[");
        for (i, secondary) in d.secondary_spans.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&self.json_location(secondary.span, Some(&secondary.label)));
        }
        out.push(']');
        out.push('}');
        out
    }

    fn json_location(&self, span: Span, label: Option<&str>) -> String {
        if span.is_none() {
            return "null".into();
        }
        let f = self.get(span.file);
        let (line, column) = f.line_col(span.start);
        let (end_line, end_column) = f.line_col(span.end);
        let mut out = String::from("{");
        let _ = write!(out, "\"file\":{}", json_str(&f.name));
        let _ = write!(out, ",\"line\":{line},\"column\":{column}");
        let _ = write!(out, ",\"endLine\":{end_line},\"endColumn\":{end_column}");
        let _ = write!(out, ",\"text\":{}", json_str(f.line_text(line)));
        if let Some(l) = label {
            let _ = write!(out, ",\"label\":{}", json_str(l));
        }
        out.push('}');
        out
    }

    /// The column the trailing `= ...` block sits in.
    ///
    /// Each snippet gets a gutter as wide as *its own* line number, so a
    /// diagnostic whose spans are on lines 10 and 5 renders two gutters of
    /// different widths. The `=` block belongs to the snippet directly above
    /// it — the last secondary span, or the primary span when there is none —
    /// so it is that one's width that puts the `=` under the `|`s it continues.
    /// Taking the widest of all of them instead indented the block for a line
    /// number that is no longer the one on screen.
    fn gutter_width(&self, d: &Diagnostic) -> usize {
        let last = d
            .secondary_spans
            .iter()
            .rev()
            .map(|s| s.span)
            .find(|s| !s.is_none())
            .unwrap_or(d.span);
        if last.is_none() {
            return 2;
        }
        let (line, _) = self.get(last.file).line_col(last.start);
        line.to_string().len().max(1).saturating_add(1)
    }

    fn render_snippet(
        &self,
        out: &mut String,
        span: Span,
        label: Option<&str>,
        caret_color: &str,
        c_blue: &str,
        c_reset: &str,
    ) {
        if span.is_none() {
            return;
        }
        let f = self.get(span.file);
        let (line, col) = f.line_col(span.start);
        let (end_line, end_col) = f.line_col(span.end);
        let gw = line.to_string().len().max(1).saturating_add(1);
        let inner = gw.saturating_sub(1);

        let _ = writeln!(out, "{:w$}{c_blue}-->{c_reset} {}:{}:{}", "", f.name, line, col, w = inner);
        let _ = writeln!(out, "{:w$}{c_blue}|{c_reset}", "", w = gw);

        // Carets under the span. A span crossing a line break is underlined to
        // the end of its first line, which is what the reader needs to see.
        let full = f.line_text(line);
        let span_chars = if end_line == line {
            (end_col.saturating_sub(col)).max(1)
        } else {
            full.chars().count().saturating_sub(col.saturating_sub(1)).max(1)
        };
        let Window { text, col: shown_col, cut_left, cut_right } =
            window(full, f.line_start(line), span.start as usize);
        let span_chars = span_chars.min(
            text.chars().count().saturating_sub(shown_col.saturating_sub(1)).max(1),
        );

        let _ = writeln!(
            out,
            "{c_blue}{:>w$} |{c_reset} {}{}{}",
            line,
            if cut_left { "…" } else { "" },
            text,
            if cut_right { "…" } else { "" },
            w = inner
        );

        // The ellipsis stands in the column its text would have, so the carets
        // still line up under what is on screen.
        let lead = usize::from(cut_left);
        let pad = " ".repeat(shown_col.saturating_sub(1).saturating_add(lead));
        let carets = "^".repeat(span_chars);
        match label {
            Some(l) => {
                let _ = writeln!(
                    out,
                    "{:w$}{c_blue}|{c_reset} {pad}{caret_color}{carets} {l}{c_reset}",
                    "",
                    w = gw
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "{:w$}{c_blue}|{c_reset} {pad}{caret_color}{carets}{c_reset}",
                    "",
                    w = gw
                );
            }
        }
        let _ = writeln!(out, "{:w$}{c_blue}|{c_reset}", "", w = gw);
    }
}

// ---------------------------------------------------------------------------
// What has already been explained
// ---------------------------------------------------------------------------

/// Whether a diagnostic's page is printed under it at all. `--dense` says no.
///
/// Process-wide because "print this paragraph once" is a fact about the run,
/// not about a source map: a command that opens two sessions, or analyses forty
/// targets, still owes the reader one copy of each explanation.
static BODIES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Set from `--dense`, once, as a command opens.
pub fn print_bodies(yes: bool) {
    BODIES.store(yes, std::sync::atomic::Ordering::Relaxed);
}

fn bodies_are_printed() -> bool {
    BODIES.load(std::sync::atomic::Ordering::Relaxed)
}

/// The codes whose page has already been printed in this process.
static EXPLAINED: std::sync::OnceLock<std::sync::Mutex<crate::hash::Set<String>>> =
    std::sync::OnceLock::new();

/// True the first time it is asked about a code, false every time after.
fn first_sighting(code: &str) -> bool {
    let seen = EXPLAINED.get_or_init(Default::default);
    // A poisoned lock means another thread panicked mid-insert. The set is a
    // presentation detail, so the answer is to carry on with what is in it
    // rather than to take the build down for it.
    let mut seen = match seen.lock() {
        Ok(seen) => seen,
        Err(poisoned) => poisoned.into_inner(),
    };
    seen.insert(code.to_string())
}

/// Forgets what has been explained. For tests, which share a process.
#[cfg(test)]
fn forget_explanations() {
    if let Some(seen) = EXPLAINED.get() {
        match seen.lock() {
            Ok(mut seen) => seen.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }
    }
}

/// The widest source line a snippet prints, in bytes.
///
/// A generated, minified, or truncated file is one enormous line, and every
/// diagnostic in it used to print that line whole. Forty thousand errors in a
/// two-megabyte line wrote *seventy gigabytes* to the terminal — a file that
/// fills a disk is a worse failure than one that crashes, because it takes the
/// machine with it. So a long line is shown as a window around the caret, with
/// `…` where it was cut.
const MAX_SNIPPET_BYTES: usize = 240;

/// How much of a long line to keep to the left of the caret, so that the
/// reader sees what leads up to it rather than only what follows.
const SNIPPET_LEAD_BYTES: usize = 60;

/// The part of a line a snippet shows.
struct Window<'a> {
    text: &'a str,
    /// 1-based column of the caret *within `text`*, counted in characters.
    col: usize,
    cut_left: bool,
    cut_right: bool,
}

/// Narrows `line` — which begins at byte `line_start` in its file — to at most
/// [`MAX_SNIPPET_BYTES`] around the byte `at`.
fn window(line: &str, line_start: usize, at: usize) -> Window<'_> {
    let at = at.saturating_sub(line_start).min(line.len());
    if line.len() <= MAX_SNIPPET_BYTES {
        let col = line.get(..at).map_or(0, |s| s.chars().count()).saturating_add(1);
        return Window { text: line, col, cut_left: false, cut_right: false };
    }
    let mut lo = at.saturating_sub(SNIPPET_LEAD_BYTES);
    while lo > 0 && !line.is_char_boundary(lo) {
        lo = lo.saturating_sub(1);
    }
    let mut hi = lo.saturating_add(MAX_SNIPPET_BYTES).min(line.len());
    while hi > lo && !line.is_char_boundary(hi) {
        hi = hi.saturating_sub(1);
    }
    let text = line.get(lo..hi).unwrap_or("");
    let col = line.get(lo..at.max(lo)).map_or(0, |s| s.chars().count()).saturating_add(1);
    Window { text, col, cut_left: lo > 0, cut_right: hi < line.len() }
}

/// A JSON string literal. There is no serde here, on purpose: the toolchain
/// has no dependencies, and this is the whole of what it needs.
///
/// Public because `buri docs --format=json` emits the same shape of output and
/// must escape it the same way; one escaper means one set of edge cases.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Joins names for a message, each in backticks: ``a``, ``b`` and ``c``. Every
/// other identifier in a diagnostic is quoted, and a list of them should not be
/// the exception.
pub fn names(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|n| format!("`{n}`")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// A diagnostic sink that keeps errors in source order and deduplicates.
#[derive(Default, Clone)]
pub struct Diagnostics {
    pub items: Vec<Diagnostic>,
    /// What has been reported, so that the deduplication above is a lookup.
    ///
    /// It was a scan of everything reported so far, which is `n²` in the
    /// number of diagnostics — invisible at ten and not at forty thousand,
    /// which one malformed generated file produces.
    seen: crate::hash::Set<(u32, u32, u32, String)>,
}

impl Diagnostics {
    pub fn new() -> Diagnostics {
        Diagnostics::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        // The checker can reach the same bad construct through more than one
        // path; reporting it twice helps nobody.
        let key = (d.span.file.0, d.span.start, d.span.end, d.message.clone());
        if !self.seen.insert(key) {
            return;
        }
        self.items.push(d);
    }

    /// Empties the sink, including what it remembers having reported — a
    /// command that has printed one batch starts the next one afresh.
    pub fn clear(&mut self) {
        self.items.clear();
        self.seen.clear();
    }

    pub fn extend(&mut self, other: impl IntoIterator<Item = Diagnostic>) {
        for d in other {
            self.push(d);
        }
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.is_error())
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Source order, errors before warnings at the same location.
    pub fn sort(&mut self, map: &SourceMap) {
        self.items.sort_by_key(|d| {
            let name = if d.span.is_none() { String::new() } else { map.name(d.span.file).to_string() };
            (name, d.span.start, d.severity == Severity::Warning)
        });
    }
}

#[cfg(test)]
#[allow(
    clippy::string_slice,
    reason = "a test that slices a string on a known boundary is asserting where that boundary is; `clippy.toml` has no in-tests exemption for this one"
)]
mod tests {
    use super::*;

    fn one_file() -> (SourceMap, FileId) {
        let mut map = SourceMap::new();
        let id = map.add(
            "cmd/x/main.buri",
            PathBuf::from("/tmp/main.buri"),
            "fn f(): Int {\n  a + b\n}\n".to_string(),
        );
        (map, id)
    }

    #[test]
    fn the_trailer_reads_expected_actual_notes_then_fix() {
        let (map, id) = one_file();
        let d = Diagnostic::error(Span::new(id, 16, 21), "expected `I32`, found `I64`")
            .with_mismatch("`I32`", "`I64`")
            .with_note("there is no implicit promotion of any kind")
            .with_fix("convert explicitly with `.toI32()?`");
        let text = map.render(&d, false);
        let trailer: Vec<&str> =
            text.lines().filter_map(|l| l.trim_start().strip_prefix("= ")).collect();
        assert_eq!(
            trailer,
            vec![
                "expected: `I32`",
                "actual: `I64`",
                "there is no implicit promotion of any kind",
                "fix: convert explicitly with `.toI32()?`",
            ],
            "the fix is last, because it is the line to act on"
        );
    }

    #[test]
    fn the_gutter_is_a_straight_line() {
        let (map, id) = one_file();
        let d = Diagnostic::error(Span::new(id, 16, 21), "no")
            .with_fix("do something else");
        // Every `|` and every `=` sits in the same column.
        let columns: Vec<usize> = map
            .render(&d, false)
            .lines()
            .filter(|l| l.contains('|') || l.trim_start().starts_with('='))
            .map(|l| l.find(['|', '=']).unwrap())
            .collect();
        assert!(columns.windows(2).all(|w| w[0] == w[1]), "ragged gutter: {columns:?}");
    }

    /// Two spans whose line numbers are different widths. The `=` block sits
    /// under the *second* snippet, so it is the second snippet's gutter it has
    /// to line up with — `duplicate-source` (lines 10 and 5) is the case that
    /// showed this, and it rendered the block indented for line 10 while
    /// sitting under line 5.
    #[test]
    fn the_trailer_lines_up_with_the_snippet_above_it() {
        let mut map = SourceMap::new();
        let text: String = (1..=12).map(|n| format!("line {n}\n")).collect();
        let id = map.add("x/BUILD.buri", PathBuf::from("/tmp/BUILD.buri"), text);
        // Line 10 starts at byte 9 * 7 = 63; line 5 at byte 4 * 7 = 28.
        let d = Diagnostic::error(Span::new(id, 63, 67), "listed by two rules")
            .with_secondary_span(Span::new(id, 28, 32), "first listed here")
            .with_fix("list it under one rule only");
        let rendered = map.render(&d, false);
        let gutters: Vec<usize> = rendered
            .lines()
            .filter(|l| l.contains('|') || l.trim_start().starts_with('='))
            .map(|l| l.find(['|', '=']).unwrap())
            .collect();
        // Four lines for the primary snippet (|, source, carets, |) at width 3,
        // then four for the secondary span at width 2, then the `=` — which
        // belongs to that secondary span.
        let last = *gutters.last().unwrap();
        let under = gutters[gutters.len() - 2];
        assert_eq!(last, under, "the `=` is not under the `|` above it: {gutters:?}\n{rendered}");
        assert!(
            rendered.contains("\n  = fix: list it under one rule only\n"),
            "expected the fix in line 5's gutter:\n{rendered}"
        );
    }

    #[test]
    fn json_omits_what_does_not_apply() {
        let (map, id) = one_file();
        let d = Diagnostic::error(Span::new(id, 16, 17), "nope").with_fix("do it differently");
        let json = map.to_json(&d);
        assert!(json.contains(r#""severity":"error""#));
        assert!(json.contains(r#""fix":"do it differently""#));
        assert!(json.contains(r#""line":2,"column":3"#));
        assert!(json.contains(r#""text":"  a + b""#));
        // Absent means "not applicable", so the keys are not emitted at all.
        assert!(!json.contains("expected"), "{json}");
        assert!(!json.contains("actual"), "{json}");
        assert!(!json.contains("rendered"), "{json}");
        assert!(json.ends_with('}') && json.starts_with('{'));
        assert!(!json.contains('\n'), "one diagnostic is one line");
    }

    /// Every one of these reached `String::replace_range` and *panicked*. A
    /// tool that rewrites your files must not have a panic as its failure
    /// mode, so each is now a refusal that says which file and why.
    #[test]
    fn an_edit_that_cannot_be_applied_is_refused_rather_than_panicking() {
        let mut map = SourceMap::new();
        let id = map.add("a.buri", PathBuf::new(), "let s = \"héllo\";\n".to_string());
        let text = map.text(id);
        let edit = |start, end| Edit {
            at: Span::new(id, start, end),
            replacement: String::new(),
        };

        // Backwards. The collection-level overlap check never saw this: it
        // compares *pairs*, and one edit is not a pair.
        assert!(matches!(
            EditSet::new(id, text, [edit(9, 4)]),
            Err(BadEdits::Inverted { .. })
        ));
        assert!(matches!(
            EditSet::new(id, text, [edit(0, 500)]),
            Err(BadEdits::PastEnd { .. })
        ));
        // The `é` starts at byte 10 and is two bytes wide, so 11 is inside it.
        assert_eq!(&text[10..12], "é");
        assert!(matches!(
            EditSet::new(id, text, [edit(0, 11)]),
            Err(BadEdits::NotACharBoundary { .. })
        ));
        assert!(matches!(
            EditSet::new(id, text, [edit(0, 6), edit(4, 8)]),
            Err(BadEdits::Overlap { .. })
        ));
    }

    /// Order is the set's, not the caller's: it sorts, and it applies back to
    /// front so an earlier edit's offsets are still the offsets of the text it
    /// is applied to.
    #[test]
    fn a_valid_set_applies_whatever_order_it_arrives_in() {
        let mut map = SourceMap::new();
        let id = map.add("a.buri", PathBuf::new(), "one two three".to_string());
        let set = EditSet::new(
            id,
            map.text(id),
            [
                Edit { at: Span::new(id, 8, 13), replacement: "3".into() },
                Edit { at: Span::new(id, 0, 3), replacement: "1".into() },
            ],
        )
        .expect("in bounds and not overlapping");
        assert_eq!(set.apply(map.text(id)), "1 two 3");
    }

    #[test]
    fn json_escapes_what_would_break_the_line() {
        let mut map = SourceMap::new();
        let id = map.add("a.buri", PathBuf::new(), "let s = \"x\";\n".to_string());
        let d = Diagnostic::error(Span::new(id, 8, 11), "a \"quoted\" thing\twith a tab")
            .with_fix("mind the \\ backslash");
        let json = map.to_json(&d);
        assert!(json.contains(r#"a \"quoted\" thing\twith a tab"#), "{json}");
        assert!(json.contains(r#"mind the \\ backslash"#), "{json}");
        assert!(!json.contains('\t'));
    }

    /// A span does not have to land on a character boundary to be rendered: it
    /// can come from a language-server position, from `--error-format=json`,
    /// or from arithmetic in a caller. Every one of these used to be a
    /// `String` index panic.
    #[test]
    fn a_span_that_makes_no_sense_still_renders() {
        let mut map = SourceMap::new();
        let id = map.add("a.buri", PathBuf::new(), "let s = \"café 🙂\";\n".to_string());
        let cases = [
            ("inside a two-byte character", Span::new(id, 10, 11)),
            ("inside a four-byte character", Span::new(id, 14, 16)),
            ("past the end", Span::new(id, 9_000, 10_000)),
            ("backwards", Span::new(id, 12, 4)),
            ("as wide as the address space", Span { file: id, start: 0, end: u32::MAX }),
            ("in a file this map never had", Span::new(FileId(77), 0, 3)),
            ("in no file at all", Span::NONE),
        ];
        for (what, span) in cases {
            let d = Diagnostic::error(span, "no").with_fix("do something else");
            let rendered = map.render(&d, false);
            assert!(rendered.starts_with("error: no"), "{what}: {rendered}");
            let _ = map.to_json(&d);
            let _ = map.snippet(span);
        }
    }

    /// A generated or minified file is one enormous line, and a diagnostic used
    /// to print the whole of it — forty thousand of them wrote seventy
    /// gigabytes. What is printed is now a window around the caret.
    #[test]
    fn a_very_long_line_is_shown_as_a_window() {
        let mut map = SourceMap::new();
        let line = format!("let x = {};\n", "1 + ".repeat(50_000));
        let at = line.len() as u32 / 2;
        let id = map.add("a.buri", PathBuf::new(), line);
        let d = Diagnostic::error(Span { file: id, start: at, end: at + 1 }, "no")
            .with_fix("do something else");
        let rendered = map.render(&d, false);
        assert!(rendered.len() < 2_000, "the snippet was {} bytes", rendered.len());
        assert!(rendered.contains('…'), "nothing said the line was cut:\n{rendered}");
        // The carets still sit under the character the span names.
        let lines: Vec<&str> = rendered.lines().collect();
        let source = lines.iter().position(|l| l.contains('…')).expect("a windowed line");
        let carets = lines.get(source + 1).expect("a caret line");
        let column = carets.find('^').expect("a caret");
        assert!(column > 0, "the carets are in the gutter:\n{rendered}");
    }

    /// A short line is untouched, because the window is only for the case that
    /// needed one.
    #[test]
    fn a_short_line_is_printed_whole() {
        let mut map = SourceMap::new();
        let id = map.add("a.buri", PathBuf::new(), "let x = 1 + 2;\n".to_string());
        let d = Diagnostic::error(Span::new(id, 8, 9), "no").with_fix("f");
        let rendered = map.render(&d, false);
        assert!(rendered.contains("let x = 1 + 2;"), "{rendered}");
        assert!(!rendered.contains('…'), "{rendered}");
    }

    /// The one thing a migration has to preserve: the same inputs produce the
    /// same bytes. `type-args-on-a-value` is the worked case — its message and
    /// its fix moved from `semantics/expressions.rs` into the frontmatter of
    /// `docs/errors/type-args-on-a-value.md` and nothing about what a user
    /// reads changed.
    #[test]
    fn a_templated_diagnostic_renders_what_the_call_site_used_to_build() {
        let (map, id) = one_file();
        let span = Span::new(id, 16, 21);
        let by_hand = Diagnostic::error(span, "explicit type arguments qualify a function or a call")
            .with_code("type-args-on-a-value")
            .with_fix("attach the type arguments to the call, as in `a<Str>(x)`");
        let templated =
            Diagnostic::templated("type-args-on-a-value", span).with_bind("function", "a");
        assert_eq!(map.render(&templated, false), map.render(&by_hand, false));
        assert_eq!(map.to_json(&templated), map.to_json(&by_hand));
    }

    /// The explanation is context, and context repeated is noise: a file with
    /// forty of one mistake in it wants the paragraph once and the forty
    /// locations forty times.
    #[test]
    fn only_the_first_of_a_code_is_explained() {
        let (map, id) = one_file();
        let d = Diagnostic::templated("type-args-on-a-value", Span::new(id, 16, 21))
            .with_bind("function", "a");

        forget_explanations();
        let first = map.render_with_body(&d, false);
        assert!(first.contains("comparison operators are non-associative"), "{first}");
        assert_eq!(map.render_with_body(&d, false), map.render(&d, false));

        // And `--dense` says not even the first time.
        forget_explanations();
        print_bodies(false);
        let dense = map.render_with_body(&d, false);
        print_bodies(true);
        assert_eq!(dense, map.render(&d, false));
    }

    /// A page whose specimen and reproduction are all it has explains nothing,
    /// and printing an empty block under a diagnostic is worse than printing
    /// nothing.
    #[test]
    fn a_diagnostic_with_no_page_is_rendered_as_it_always_was() {
        let (map, id) = one_file();
        // Spelled through a binding: `docs::documents::every_emitted_code_is_documented`
        // scans this file for the codes it attaches, and this one has no page
        // on purpose.
        let unknown = "a-code-with-no-page";
        let d = Diagnostic::error(Span::new(id, 16, 21), "no").with_code(unknown);
        assert_eq!(map.render_with_body(&d, false), map.render(&d, false));
    }

    #[test]
    #[should_panic(expected = "nothing bound it")]
    fn an_unbound_placeholder_is_a_failure_rather_than_a_hole_in_a_sentence() {
        let (map, id) = one_file();
        let d = Diagnostic::templated("type-args-on-a-value", Span::new(id, 16, 21));
        let _ = map.render(&d, false);
    }

    #[test]
    #[should_panic(expected = "no template of its page uses it")]
    fn a_binding_nothing_uses_is_a_failure_too() {
        let (map, id) = one_file();
        let d = Diagnostic::templated("type-args-on-a-value", Span::new(id, 16, 21))
            .with_bind("function", "a")
            .with_bind("callee", "a");
        let _ = map.render(&d, false);
    }

    /// The map hands out every `FileId`, and every span carrying one it did not
    /// hand out — `Span::NONE` above all — has to render as no location rather
    /// than as a crash on the way to the terminal.
    #[test]
    fn a_file_this_map_never_had_is_empty_rather_than_a_panic() {
        let map = SourceMap::new();
        assert_eq!(map.text(FileId(3)), "");
        assert_eq!(map.text(FileId::NONE), "");
        assert_eq!(map.name(FileId::NONE), "<none>");
        assert_eq!(map.get(FileId(9)).line_text(1), "");
        assert_eq!(map.get(FileId(9)).line_col(500), (1, 1));
    }
}
