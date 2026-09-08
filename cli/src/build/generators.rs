//! `generators`: a program the build runs, whose output becomes a module.
//!
//! A generator is an ordinary Buri binary. The build hands it one line of JSON
//! on standard input and reads one line of JSON back, and every module it names
//! is loaded the way a `.proto` module is: through the real parser, into the
//! rule that declared the generator, with no file on disk.
//!
//! ```text
//! -> {"inputs":[["lib/wire/point.proto","edition = \"2026\";\n"]],"dependencies":[]}
//! <- {"modules":[{"name":"point.proto","text":"export struct Point {}\n","anchors":[]}],
//!     "diagnostics":[]}
//! ```
//!
//! **Text plus anchors, never a tree.** An anchor says which region of the
//! generated text came from which span of which input, which is the whole of
//! what go-to-definition and a diagnostic inside generated code need. The
//! compiler then parses the text with its one ordinary parser.
//!
//! Two kinds of tool, **one path**. A `tool` beginning `//` names a binary
//! target in this repository: it is built for `JS` through the ordinary action
//! path and run under the JavaScript runtime. Anything else names a generator
//! the toolchain ships — `std/codegen/proto` is the only one — which is a Buri
//! program too, compiled from [`PROTO_MAIN`] the first time a build needs it
//! and run through the same [`run_artifact`]. That is what makes the boundary
//! provable rather than asserted: the `.proto` generator is not privileged,
//! and nothing here would notice if it moved into a repository.
//!
//! The JSON is written and read by hand. This workspace may not grow a
//! dependency (`language::corpus::dependencies_stay_behind_the_bar`), and the
//! request and response documents belong to this protocol rather than to
//! whatever a derive would print.

use crate::build::buildfile::{Generator, Output, Platform};
use crate::build::cache::{Action, ActionKey, Cache, KeyBuilder};
use crate::build::session::Session;
use crate::build::sources::Overlay;
use crate::build::workspace::{RuleKind, TargetId, Workspace};
use crate::commands::arguments::Flags;
use crate::diagnostics::Span;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

/// The generator the toolchain ships. Every other non-`//` tool is refused.
pub const PROTO_TOOL: &str = "std/codegen/proto";

/// The `code` of a [`Diagnostic`] whose `message` is already the whole
/// sentence, so the loader prints it rather than a page's wording.
///
/// Not a catalogue code: a file the operating system would not hand over has
/// no rule behind it to explain, and the sentence is the error the read
/// returned. `sources` reports the same file the same way
/// (`compiler::modules`), which is what makes the two agree.
pub const UNREADABLE: &str = "an-input-that-could-not-be-read";

// ---------------------------------------------------------------------------
// The protocol
// ---------------------------------------------------------------------------

/// What the build hands a generator: the files it declared, and the files of
/// the rules it depends on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    /// `(repository-relative path, contents)`, in the order the rule declared
    /// them.
    pub inputs: Vec<(String, String)>,
    /// The same, for what the declaring rule's dependencies own. Empty today:
    /// nothing yet declares a generator that reads across a rule boundary, and
    /// the field is in the wire so that one can without the protocol moving.
    pub dependencies: Vec<(String, String)>,
}

/// A position in one of the generator's inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// Repository-relative, as the request spelled it.
    pub file: String,
    /// Byte offsets into that file.
    pub span: (usize, usize),
}

/// Which region of generated text came from which span of which input.
///
/// `start` and `end` are byte offsets into [`GeneratedModule::text`]. Sorted by
/// `start`, outermost first where two share one — so the *last* anchor
/// containing an offset is the innermost node covering it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub start: usize,
    pub end: usize,
    pub file: String,
    pub span: (usize, usize),
}

impl Anchor {
    pub fn origin(&self) -> Origin {
        Origin { file: self.file.clone(), span: self.span }
    }
}

/// One module a generator produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedModule {
    /// The module's name inside the declaring package. A generator naming
    /// `point.proto` in `//lib/wire` produces `//lib/wire/point.proto`.
    pub name: String,
    /// Buri source, as `buri format` would leave it.
    pub text: String,
    pub anchors: Vec<Anchor>,
}

impl GeneratedModule {
    /// The innermost anchor covering a byte offset in [`Self::text`], if there
    /// is one. This is what turns a position in generated text into a position
    /// in the input the generator read.
    pub fn anchor_at(&self, offset: usize) -> Option<&Anchor> {
        self.anchors.iter().rfind(|a| a.start <= offset && offset < a.end)
    }
}

/// Something the generator has to say about its input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub note: Option<String>,
    pub fix: Option<String>,
    /// Where in the input this is about. `None` anchors it on the `generators`
    /// entry that ran the tool.
    pub origin: Option<Origin>,
}

/// What a generator writes back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    pub modules: Vec<GeneratedModule>,
    pub diagnostics: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// The JSON this protocol speaks, as a value.
///
/// Small on purpose: objects, arrays, strings, non-negative integers and
/// `null` are the whole of the grammar the request and the response use, and a
/// parser that accepts more would accept documents this protocol cannot mean.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Int(usize),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, name: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_usize(&self) -> Option<usize> {
        match self {
            Json::Int(n) => Some(*n),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// `null` and an absent field are the same claim, which is what lets
    /// `note`, `fix` and `origin` be written either way.
    fn present(&self) -> Option<&Json> {
        match self {
            Json::Null => None,
            other => Some(other),
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below a space has no literal spelling in JSON.
            // Everything above it does, including every non-ASCII scalar: this
            // is a UTF-8 stream, so escaping them would only make the line
            // longer and the diff worse.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_json(out: &mut String, value: &Json) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Int(n) => out.push_str(&n.to_string()),
        Json::Str(s) => write_string(out, s),
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(out, item);
            }
            out.push(']');
        }
        Json::Object(fields) => {
            out.push('{');
            for (i, (name, value)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, name);
                out.push(':');
                write_json(out, value);
            }
            out.push('}');
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.at = self.at.saturating_add(1);
        Some(b)
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at = self.at.saturating_add(1);
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        self.skip_space();
        if self.bump() == Some(b) {
            return Ok(());
        }
        Err(format!("expected `{}` at byte {}", b as char, self.at))
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_space();
        match self.peek() {
            Some(b'n') => {
                for b in b"null" {
                    if self.bump() != Some(*b) {
                        return Err(format!("expected `null` at byte {}", self.at));
                    }
                }
                Ok(Json::Null)
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => {
                self.at = self.at.saturating_add(1);
                let mut items = Vec::new();
                self.skip_space();
                if self.peek() == Some(b']') {
                    self.at = self.at.saturating_add(1);
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.skip_space();
                    match self.bump() {
                        Some(b',') => continue,
                        Some(b']') => return Ok(Json::Array(items)),
                        _ => return Err(format!("expected `,` or `]` at byte {}", self.at)),
                    }
                }
            }
            Some(b'{') => {
                self.at = self.at.saturating_add(1);
                let mut fields = Vec::new();
                self.skip_space();
                if self.peek() == Some(b'}') {
                    self.at = self.at.saturating_add(1);
                    return Ok(Json::Object(fields));
                }
                loop {
                    self.skip_space();
                    let name = self.string()?;
                    self.expect(b':')?;
                    fields.push((name, self.value()?));
                    self.skip_space();
                    match self.bump() {
                        Some(b',') => continue,
                        Some(b'}') => return Ok(Json::Object(fields)),
                        _ => return Err(format!("expected `,` or `}}` at byte {}", self.at)),
                    }
                }
            }
            Some(b) if b.is_ascii_digit() => {
                let start = self.at;
                while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    self.at = self.at.saturating_add(1);
                }
                let text = self.bytes.get(start..self.at).unwrap_or_default();
                let text = std::str::from_utf8(text).unwrap_or_default();
                text.parse::<usize>().map(Json::Int).map_err(|e| e.to_string())
            }
            _ => Err(format!("not a value at byte {}", self.at)),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.skip_space();
        if self.bump() != Some(b'"') {
            return Err(format!("expected a string at byte {}", self.at));
        }
        // The escapes are decoded over bytes and the rest is copied over
        // bytes, so a multi-byte scalar passes through whole rather than
        // arriving one continuation byte at a time.
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.bump() {
                None => return Err("a string is not closed".to_string()),
                Some(b'"') => break,
                Some(b'\\') => match self.bump() {
                    Some(b'"') => out.push(b'"'),
                    Some(b'\\') => out.push(b'\\'),
                    Some(b'/') => out.push(b'/'),
                    Some(b'n') => out.push(b'\n'),
                    Some(b'r') => out.push(b'\r'),
                    Some(b't') => out.push(b'\t'),
                    Some(b'b') => out.push(0x08),
                    Some(b'f') => out.push(0x0c),
                    Some(b'u') => {
                        let code = self.hex4()?;
                        // A surrogate pair is two escapes for one scalar. The
                        // high half alone is not a character, so the low half
                        // has to be read here rather than left for the next
                        // turn of the loop to reject.
                        let scalar = match code {
                            0xd800..=0xdbff => {
                                if self.bump() != Some(b'\\') || self.bump() != Some(b'u') {
                                    return Err("a high surrogate with no low half".to_string());
                                }
                                let low = self.hex4()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    return Err("a high surrogate with no low half".to_string());
                                }
                                0x10000u32
                                    .saturating_add((code.saturating_sub(0xd800)) << 10)
                                    .saturating_add(low.saturating_sub(0xdc00))
                            }
                            other => other,
                        };
                        let c = char::from_u32(scalar)
                            .ok_or_else(|| format!("\\u{scalar:04x} is not a character"))?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => return Err(format!("unknown escape at byte {}", self.at)),
                },
                Some(b) => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| "a string is not UTF-8".to_string())
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..4 {
            let b = self.bump().ok_or_else(|| "a short \\u escape".to_string())?;
            let digit = (b as char)
                .to_digit(16)
                .ok_or_else(|| format!("`{}` is not a hex digit", b as char))?;
            value = value.saturating_mul(16).saturating_add(digit);
        }
        Ok(value)
    }
}

fn parse_json(text: &str) -> Result<Json, String> {
    let mut p = Parser { bytes: text.as_bytes(), at: 0 };
    let value = p.value()?;
    p.skip_space();
    match p.peek() {
        None => Ok(value),
        Some(_) => Err(format!("trailing bytes after the document, at byte {}", p.at)),
    }
}

fn span_json(span: (usize, usize)) -> Json {
    Json::Object(vec![
        ("start".to_string(), Json::Int(span.0)),
        ("end".to_string(), Json::Int(span.1)),
    ])
}

fn read_span(value: &Json) -> Option<(usize, usize)> {
    Some((value.get("start")?.as_usize()?, value.get("end")?.as_usize()?))
}

impl Request {
    /// The one line the build writes to a generator's standard input.
    pub fn encode(&self) -> String {
        let pairs = |list: &[(String, String)]| {
            Json::Array(
                list.iter()
                    .map(|(path, text)| {
                        Json::Array(vec![
                            Json::Str(path.clone()),
                            Json::Str(text.clone()),
                        ])
                    })
                    .collect(),
            )
        };
        let mut out = String::new();
        write_json(
            &mut out,
            &Json::Object(vec![
                ("inputs".to_string(), pairs(&self.inputs)),
                ("dependencies".to_string(), pairs(&self.dependencies)),
            ]),
        );
        out
    }

    pub fn decode(text: &str) -> Result<Request, String> {
        let json = parse_json(text)?;
        let pairs = |name: &str| -> Result<Vec<(String, String)>, String> {
            let Some(list) = json.get(name).and_then(Json::present) else { return Ok(Vec::new()) };
            let items =
                list.as_array().ok_or_else(|| format!("`{name}` is not a list"))?;
            let mut out = Vec::new();
            for item in items {
                let pair = item
                    .as_array()
                    .ok_or_else(|| format!("`{name}` holds something that is not a pair"))?;
                let [path, text] = pair else {
                    return Err(format!("`{name}` holds a pair of the wrong length"));
                };
                let path = path.as_str().ok_or_else(|| format!("`{name}`: a path is not a string"))?;
                let text = text.as_str().ok_or_else(|| format!("`{name}`: a text is not a string"))?;
                out.push((path.to_string(), text.to_string()));
            }
            Ok(out)
        };
        Ok(Request { inputs: pairs("inputs")?, dependencies: pairs("dependencies")? })
    }
}

impl Response {
    /// The one line a generator writes to its standard output.
    pub fn encode(&self) -> String {
        let modules = Json::Array(
            self.modules
                .iter()
                .map(|m| {
                    Json::Object(vec![
                        ("name".to_string(), Json::Str(m.name.clone())),
                        ("text".to_string(), Json::Str(m.text.clone())),
                        (
                            "anchors".to_string(),
                            Json::Array(
                                m.anchors
                                    .iter()
                                    .map(|a| {
                                        Json::Object(vec![
                                            ("start".to_string(), Json::Int(a.start)),
                                            ("end".to_string(), Json::Int(a.end)),
                                            ("file".to_string(), Json::Str(a.file.clone())),
                                            ("span".to_string(), span_json(a.span)),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ),
                    ])
                })
                .collect(),
        );
        let optional = |v: &Option<String>| match v {
            Some(s) => Json::Str(s.clone()),
            None => Json::Null,
        };
        let diagnostics = Json::Array(
            self.diagnostics
                .iter()
                .map(|d| {
                    Json::Object(vec![
                        ("code".to_string(), Json::Str(d.code.clone())),
                        ("message".to_string(), Json::Str(d.message.clone())),
                        ("note".to_string(), optional(&d.note)),
                        ("fix".to_string(), optional(&d.fix)),
                        (
                            "origin".to_string(),
                            match &d.origin {
                                None => Json::Null,
                                Some(o) => Json::Object(vec![
                                    ("file".to_string(), Json::Str(o.file.clone())),
                                    ("span".to_string(), span_json(o.span)),
                                ]),
                            },
                        ),
                    ])
                })
                .collect(),
        );
        let mut out = String::new();
        write_json(
            &mut out,
            &Json::Object(vec![
                ("modules".to_string(), modules),
                ("diagnostics".to_string(), diagnostics),
            ]),
        );
        out
    }

    pub fn decode(text: &str) -> Result<Response, String> {
        let json = parse_json(text)?;
        let mut modules = Vec::new();
        if let Some(list) = json.get("modules").and_then(Json::present) {
            for item in list.as_array().ok_or_else(|| "`modules` is not a list".to_string())? {
                let name = item
                    .get("name")
                    .and_then(Json::as_str)
                    .ok_or_else(|| "a module has no `name`".to_string())?;
                let text = item
                    .get("text")
                    .and_then(Json::as_str)
                    .ok_or_else(|| "a module has no `text`".to_string())?;
                let mut anchors = Vec::new();
                if let Some(list) = item.get("anchors").and_then(Json::present) {
                    for a in list.as_array().ok_or_else(|| "`anchors` is not a list".to_string())? {
                        let start =
                            a.get("start").and_then(Json::as_usize).ok_or("an anchor has no `start`")?;
                        let end =
                            a.get("end").and_then(Json::as_usize).ok_or("an anchor has no `end`")?;
                        let file =
                            a.get("file").and_then(Json::as_str).ok_or("an anchor has no `file`")?;
                        let span = a.get("span").and_then(read_span).ok_or("an anchor has no `span`")?;
                        anchors.push(Anchor { start, end, file: file.to_string(), span });
                    }
                }
                modules.push(GeneratedModule {
                    name: name.to_string(),
                    text: text.to_string(),
                    anchors,
                });
            }
        }
        let mut diagnostics = Vec::new();
        if let Some(list) = json.get("diagnostics").and_then(Json::present) {
            for d in list.as_array().ok_or_else(|| "`diagnostics` is not a list".to_string())? {
                let code =
                    d.get("code").and_then(Json::as_str).ok_or("a diagnostic has no `code`")?;
                let message = d
                    .get("message")
                    .and_then(Json::as_str)
                    .ok_or("a diagnostic has no `message`")?;
                let text_of = |name: &str| {
                    d.get(name).and_then(Json::present).and_then(Json::as_str).map(str::to_string)
                };
                let origin = match d.get("origin").and_then(Json::present) {
                    None => None,
                    Some(o) => {
                        let file =
                            o.get("file").and_then(Json::as_str).ok_or("an origin has no `file`")?;
                        let span = o.get("span").and_then(read_span).ok_or("an origin has no `span`")?;
                        Some(Origin { file: file.to_string(), span })
                    }
                };
                diagnostics.push(Diagnostic {
                    code: code.to_string(),
                    message: message.to_string(),
                    note: text_of("note"),
                    fix: text_of("fix"),
                    origin,
                });
            }
        }
        Ok(Response { modules, diagnostics })
    }
}

// ---------------------------------------------------------------------------
// What a build knows about a generated module
// ---------------------------------------------------------------------------

/// What running one rule's generators produced.
///
/// A failure is a diagnostic and no modules rather than an absence, so a rule
/// whose generator did not run reports the reason once, where the entry is
/// written, instead of failing later as an import that resolves to nothing.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub modules: Vec<Arc<GeneratedModule>>,
    /// Diagnostics, each with the `generators` entry it belongs to.
    pub diagnostics: Vec<(Diagnostic, Span)>,
}

/// Every module the generators in this repository produced.
///
/// **This is the seam the compiler front end reads generated code through.**
/// Generation needs to build and spawn a tool, which the front end cannot do,
/// so it happens in the build layer ([`prepare`]) and arrives here as data. The
/// store hangs off the [`Workspace`] because the workspace is the one thing
/// already threaded to `compiler::modules::Loader`, and it is shared by every
/// clone of a `Session`, so `buri build`, `buri test`, `buri lint` and the
/// language server all read one answer.
///
/// Interior mutability, because the workspace is behind an `Rc` by the time
/// there is a session to build a tool with. Nothing else about the graph is
/// writable and nothing here rewrites the graph.
///
/// A `Mutex` rather than a `RefCell` because a `Workspace` is `Send + Sync` —
/// the native suites keep one in a `OnceLock` — and one field that is not would
/// take that away from the whole graph.
#[derive(Default)]
pub struct Store {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Per target: the key the outcome was produced under, and the outcome.
    /// The key is what makes a second session a lookup rather than a re-run.
    by_target: BTreeMap<TargetId, (String, Outcome)>,
    /// Canonical module path -> the module, for [`Workspace::resolve_module`]
    /// and for the loader.
    by_path: BTreeMap<String, (TargetId, Arc<GeneratedModule>)>,
}

impl Store {
    fn read(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while the store was being written leaves what was written,
        // and what was written is a build's answer rather than an invariant.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The outcome recorded for a target, if one has been.
    pub fn outcome(&self, target: TargetId) -> Option<Outcome> {
        self.read().by_target.get(&target).map(|(_, o)| o.clone())
    }

    /// The key an outcome was recorded under, for deciding whether to run.
    pub fn key_of(&self, target: TargetId) -> Option<String> {
        self.read().by_target.get(&target).map(|(k, _)| k.clone())
    }

    /// A generated module by its canonical path, `//lib/wire/point.proto`.
    ///
    /// The accessor the language server reads: the text a diagnostic or a hover
    /// is about, and [`GeneratedModule::anchor_at`] to turn an offset in it
    /// back into a span in the input the generator read.
    pub fn module(&self, path: &str) -> Option<Arc<GeneratedModule>> {
        self.read().by_path.get(path).map(|(_, m)| Arc::clone(m))
    }

    /// The rule that produced the module at this path.
    pub fn owner(&self, path: &str) -> Option<TargetId> {
        self.read().by_path.get(path).map(|(t, _)| *t)
    }

    /// Whether any generator has produced a module at this path.
    pub fn holds(&self, path: &str) -> bool {
        self.read().by_path.contains_key(path)
    }

    fn record(
        &self,
        workspace: &Workspace,
        target: TargetId,
        key: String,
        outcome: Outcome,
    ) {
        let package = workspace.package(target.package);
        let mut inner = self.read();
        // A rule's previous answer goes with it: a module the generator has
        // stopped producing must stop resolving.
        inner.by_path.retain(|_, (owner, _)| *owner != target);
        for module in &outcome.modules {
            inner
                .by_path
                .insert(package.module_path(&module.name), (target, Arc::clone(module)));
        }
        inner.by_target.insert(target, (key, outcome));
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("modules", &self.read().by_path.len()).finish()
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// The generators one rule declares.
pub fn declared(workspace: &Workspace, target: TargetId) -> &[Generator] {
    let p = workspace.package(target.package);
    match target.kind {
        RuleKind::Library => p.build.library.as_ref().map(|l| &l.generators[..]).unwrap_or(&[]),
        RuleKind::Binary => p.build.binary.as_ref().map(|b| &b.generators[..]).unwrap_or(&[]),
    }
}

/// Every input every generator on this rule declares, package-relative and
/// sorted.
///
/// One enumeration, because four things read it: the action key, the watch
/// set, `buri query 'sources(...)'`, and the lint that says every file on disk
/// belongs to a rule.
pub fn inputs(workspace: &Workspace, target: TargetId) -> Vec<String> {
    let mut out: Vec<String> = declared(workspace, target)
        .iter()
        .flat_map(|g| g.inputs.iter().map(|i| i.value.clone()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every module this rule's generators produced, in the order they were named.
///
/// Empty until [`prepare`] has run, and empty for a rule that declares no
/// generator — so a caller folding these into a key gets nothing for a rule
/// that has none.
pub fn modules_of(workspace: &Workspace, target: TargetId) -> Vec<Arc<GeneratedModule>> {
    workspace.generated.outcome(target).map(|o| o.modules).unwrap_or_default()
}

/// The key `--explain` reports for one rule's generators.
///
/// One line per rule rather than one per entry: `generators` is a list of ways
/// to produce the rule's modules, and what a reader is asking is whether *this
/// rule's* generated code moved. Each entry's own answer is stored under
/// [`generate_key`], which is what a cache needs; this is what a person
/// compares between two runs.
///
/// The tool is in it twice over: by name, and — for a repository tool — as the
/// `link` key of the binary that is the tool. Editing the tool must move this
/// line, or `--explain` would say nothing changed about generation while the
/// generated code changed underneath it.
pub fn rule_key(
    session: &Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Generate, flags.mode);
    k.platform(output.platform(), output.arch());
    let package = session.workspace.package(target.package);
    let paths = inputs(&session.workspace, target);
    k.rule_identity(&package.label(), "generate", &paths);
    for g in declared(&session.workspace, target) {
        k.input("tool", g.tool.value.as_bytes());
        if let Some(tool) = tool_target(&session.workspace, &g.tool.value) {
            k.dependency(&crate::build::actions::action_key(
                session,
                tool,
                &Output::js(Span::NONE),
                flags,
                Action::Link,
            ));
        }
    }
    for rel in &paths {
        let full = package.dir.join(rel);
        k.input(&session.workspace.rel_of(&full), &std::fs::read(&full).unwrap_or_default());
    }
    k.finish()
}

/// The binary target a `//label` tool names.
pub fn tool_target(workspace: &Workspace, tool: &str) -> Option<TargetId> {
    let path = tool.strip_prefix("//")?;
    let package = workspace.package_by_path(path)?;
    workspace
        .package(package)
        .has_binary()
        .then_some(TargetId { package, kind: RuleKind::Binary })
}

// ---------------------------------------------------------------------------
// Running one
// ---------------------------------------------------------------------------

/// The program a toolchain generator *is*.
///
/// `std/codegen/proto` is the whole of the list, and there is nothing in here
/// a user could not have written: `core/codegen`'s `run`, over the `emit` the
/// standard library exports. What the build runs is this, compiled to
/// JavaScript and handed a request on standard input — the same protocol and
/// the same subprocess a `//label` tool gets.
const GENERATOR_MAIN_NAME: &str = "toolchain-generator-main";

const PROTO_MAIN: &str = r#"from "core/codegen" import * as codegen;
from "core/effect" import { Allocator, Stdin, Stdout };
from "core/host" import * as host;
from "std/codegen/proto" import * as proto;

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdin: host.stdin,
        Stdout: host.stdout,
    };
    codegen.run(ctx, fn(c, request) => proto.emit(c, request))
}
"#;

/// Runs the toolchain generator named `tool`.
///
/// One path, not two. A generator this toolchain ships goes through
/// [`run_artifact`] exactly as a repository tool does, so what proves the
/// protocol is the `.proto` generator itself rather than a wrapper written to
/// look like one.
pub fn run_toolchain(
    session: &Session,
    tool: &str,
    request: &Request,
    flags: &Flags,
) -> Result<Response, String> {
    let artifact = toolchain_artifact(session, tool, flags)?;
    run_artifact(&artifact, request)
}

/// The `.mjs` a toolchain generator is compiled to, built once and kept.
///
/// The file's name is its action key, which already carries the toolchain
/// version, so a new toolchain writes a new file rather than reading a stale
/// one — and `buri clean`, which drops `.buri`, drops this with everything
/// else. Written through a temporary and renamed, because two builds in one
/// repository may reach this at the same moment and a half-written module is
/// worse than a second compile.
fn toolchain_artifact(
    session: &Session,
    tool: &str,
    flags: &Flags,
) -> Result<std::path::PathBuf, String> {
    if tool != PROTO_TOOL {
        return Err(format!(
            "`{tool}` is not a generator this toolchain ships; `{PROTO_TOOL}` is the only one"
        ));
    }
    let source = PROTO_MAIN;
    let mut k = KeyBuilder::new(Action::Generate, flags.mode);
    k.platform(Platform::Js, None);
    k.rule_identity(tool, "toolchain-generator", &[]);
    k.input("main.buri", source.as_bytes());
    let key = k.finish();
    let dir = session.root.join(".buri/out/toolchain");
    let path = dir.join(format!("{}.mjs", key.as_str()));
    if path.is_file() {
        return Ok(path);
    }
    // Not the tool's own name: the snippet is loaded as a module beside the
    // standard library, and a module path already taken is one it would shadow.
    let mut map = crate::diagnostics::SourceMap::new();
    // The generator this toolchain ships never calls `lazy.load`, so it has no
    // chunks to write beside itself.
    let (js, _chunks) =
        crate::compiler::driver::compile_snippet_js(None, &mut map, GENERATOR_MAIN_NAME, source)
        .map_err(
        |d| match d.items.first() {
            Some(first) => format!("`{tool}` does not compile: {}", map.render(first, false)),
            None => format!("`{tool}` does not compile"),
        },
    )?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let staged = dir.join(format!("{}.mjs.{}", key.as_str(), std::process::id()));
    std::fs::write(&staged, js.as_bytes()).map_err(|e| format!("{}: {e}", staged.display()))?;
    std::fs::rename(&staged, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Runs a built `.mjs` generator under the JavaScript runtime, one line in and
/// one line out.
///
/// The command comes from [`crate::build::spawn::command`] rather than
/// `Command::new`, so a generator's process gets the same explicit environment
/// every other action's does: cleared, then `TZ` and `SOURCE_DATE_EPOCH`.
///
/// The clock is the effect system's job rather than this one's.
/// [`crate::build::spawn::FIXED_CLOCK_JS`] is spliced into a *suite's* script,
/// which the runner writes; a generator's artifact is the ordinary linked one,
/// and nothing here rewrites it. What keeps a generator off the clock is that
/// `core/codegen`'s `run` hands `generate` a context bounded by `Allocator`,
/// `Stdin` and `Stdout` — reach past those three and the program does not
/// compile (`cli/tests/reject/generator_reaches_beyond_its_context`). A `main`
/// that binds more than `run` needs is out of that bound, and
/// `--check-reproducible` is what answers for it.
pub fn run_artifact(artifact: &std::path::Path, request: &Request) -> Result<Response, String> {
    use std::io::{Read as _, Write as _};
    use std::process::Stdio;

    let program = crate::commands::test::js_runtime();
    let Some(mut cmd) = crate::build::spawn::command(&program) else {
        return Err(format!(
            "`{program}` is not on PATH; install bun, or point BURI_JS at a JavaScript runtime"
        ));
    };
    let mut child = cmd
        .arg(artifact)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    // Each pipe on a thread of its own, and none of them read after the wait.
    // A pipe holds a page or two: a generator writing more than that — a schema
    // of any size produces far more — blocks on the write, and this process
    // waiting for an exit that the block prevents is two processes waiting on
    // each other, with nothing to end it. Draining while the tool runs is what
    // makes the size of the answer not matter, and it is what makes the wait
    // below safe to be a plain one.
    let mut stdin = child.stdin.take().ok_or("the generator has no standard input")?;
    let line = format!("{}\n", request.encode());
    let feeding = std::thread::spawn(move || {
        // A write that fails because the tool exited before reading is not
        // itself the failure worth reporting: the exit status below says more.
        let _ = stdin.write_all(line.as_bytes());
        let _ = stdin.flush();
    });
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let reading_out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let reading_err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    // **Waited for, not timed.** This used to poll under a sixty-second
    // deadline and kill the tool at it, and that number could only ever measure
    // the machine: a two-thousand-field schema is five seconds on an idle
    // laptop and past sixty on a four-core runner with sixteen tests on it,
    // which is how CI came to fail a build every other host completed. Nothing
    // else the build spawns — `cc`, a linker, the JavaScript runtime — carries
    // a clock either. The one bound in this toolchain that stops a subprocess
    // is `timeout_seconds` on a `test` rule, which a person wrote in a build
    // file about their own tests. A generator that never answers is a program
    // its author can run and interrupt, and the suite that drives this has a
    // cap of its own that can tell a stuck process from a busy one
    // (`cli/tests/harness/hang.rs`).
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = feeding.join();
    let stdout = reading_out.join().unwrap_or_default();
    let stderr = reading_err.join().unwrap_or_default();
    if !status.success() {
        return Err(said(&format!("the generator {}", how_it_ended(&status)), &stderr));
    }
    // One line out. Anything before it is the tool talking to a person, which
    // is not this protocol — the response is the last non-empty line.
    let Some(line) = stdout.lines().rev().find(|l| !l.trim().is_empty()) else {
        return Err(said("the generator wrote nothing", &stderr));
    };
    Response::decode(line)
        .map_err(|e| said(&format!("the generator's answer is not a response: {e}"), &stderr))
}

/// How much of what a tool put on standard error a note carries.
///
/// A generator that fills its pipe must not fill the page a person is reading,
/// and a JavaScript runtime's stack trace is long: four kilobytes is a screen
/// or two, which is enough to say what went wrong and short enough to read.
const STDERR_TAIL: usize = 4096;

/// How a generator's process ended, in the words the note carries.
///
/// **A tool with no exit status was killed, and which signal killed it is the
/// diagnosis.** A crash and a stack overflow arrive as `SIGSEGV`; a runner that
/// ran out of memory sends `SIGKILL`. "The generator produced no response" is
/// the same sentence for all three, and a CI job that says only that leaves
/// nothing to go on.
fn how_it_ended(status: &std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exited with {code}");
    }
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(status) {
        return match signal_name(signal) {
            Some(name) => format!("was killed by {name} (signal {signal})"),
            None => format!("was killed by signal {signal}"),
        };
    }
    "ended without a status".to_string()
}

/// The name of a signal, for the numbers macOS and Linux agree on.
///
/// The two disagree about several — `SIGBUS` is 10 on one and 7 on the other —
/// so the ones they disagree about are reported by number. A wrong name is
/// worse than no name.
fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _other => return None,
    })
}

/// A sentence, with what the tool put on standard error under it.
///
/// The **tail** of it, at most [`STDERR_TAIL`] bytes: what a program says last
/// is what says why it stopped, and a runtime's stack trace buries its first
/// line under a hundred frames.
fn said(sentence: &str, stderr: &str) -> String {
    let text = stderr.trim();
    if text.is_empty() {
        return sentence.to_string();
    }
    if text.len() <= STDERR_TAIL {
        return format!("{sentence}\n{text}");
    }
    let skip = text.len().saturating_sub(STDERR_TAIL);
    // Forward to the next character boundary, so a note is never cut through
    // the middle of a character. `len()` is one, so the search always ends.
    let cut = (skip..=text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(text.len());
    let tail = text.get(cut..).unwrap_or_default();
    format!("{sentence}\n(the first {cut} bytes of standard error are not shown)\n{tail}")
}

// ---------------------------------------------------------------------------
// Running every generator in a repository
// ---------------------------------------------------------------------------

/// Runs every generator this repository declares, and records what each one
/// produced on the workspace's [`Store`].
///
/// Called from [`crate::build::sources::Sources::session`] — the one door every
/// command opens a repository through — so `buri build`, `buri test`,
/// `buri lint` and the language server all read one answer, produced once.
///
/// A rule whose recorded answer is already under the keys its inputs and its
/// tool produce now is left alone, so a second session costs the keys rather
/// than a second run of the tool.
pub fn prepare(session: &mut Session, flags: &Flags, overlay: &Overlay) {
    let targets: Vec<TargetId> = session
        .workspace
        .targets()
        .into_iter()
        .filter(|t| !declared(&session.workspace, *t).is_empty())
        .collect();
    let mut done: BTreeSet<TargetId> = BTreeSet::new();
    for target in targets {
        ensure(session, target, flags, overlay, &mut done);
    }
}

/// One rule's generators, and — first — the generators of whatever its tools
/// are built from.
///
/// `done` is entered *before* the recursion, so a graph that turns back on
/// itself terminates here and is reported by [`cycle`] rather than looping.
fn ensure(
    session: &mut Session,
    target: TargetId,
    flags: &Flags,
    overlay: &Overlay,
    done: &mut BTreeSet<TargetId>,
) {
    if !done.insert(target) {
        return;
    }
    let workspace = Rc::clone(&session.workspace);
    for generator in declared(&workspace, target) {
        let Some(tool) = tool_target(&workspace, &generator.tool.value) else { continue };
        if cycle(&workspace, target, tool).is_some() {
            continue;
        }
        for member in workspace.closure(tool) {
            if !declared(&workspace, member).is_empty() {
                ensure(session, member, flags, overlay, done);
            }
        }
    }
    run_rule(session, target, flags, overlay);
}

/// Whether a generator's tool is built from the target that declares it.
///
/// Answered off the graph, so it is answered before anything is built: a
/// generator that needs its own output is an error, never a hang. The path
/// comes back with the span of the edge that introduced each step, the way
/// `circular-import` reports one.
fn cycle(workspace: &Workspace, target: TargetId, tool: TargetId) -> Option<CyclePath> {
    workspace.closure(tool).contains(&target).then(|| {
        workspace.dep_path(tool, target).unwrap_or_else(|| vec![(tool, None), (target, None)])
    })
}

/// One entry, ready to run: what it was handed and what that keys as.
struct Entry {
    generator: Generator,
    request: Request,
    key: ActionKey,
}

fn run_rule(session: &mut Session, target: TargetId, flags: &Flags, overlay: &Overlay) {
    let workspace = Rc::clone(&session.workspace);
    let mut entries: Vec<Entry> = Vec::new();
    let mut missing: Vec<(Diagnostic, Span)> = Vec::new();
    // The keys, plus a line per input nothing could read. Together they are the
    // whole of what decides this rule's answer, so a session whose fingerprint
    // has not moved has nothing to re-run — and a missing input that appears
    // moves it, which is what makes writing the file enough.
    let mut fingerprint = String::new();

    for generator in declared(&workspace, target) {
        let package = workspace.package(target.package);
        let mut request = Request::default();
        let mut unreadable = false;
        for input in &generator.inputs {
            let full = package.dir.join(&input.value);
            let rel = workspace.rel_of(&full);
            let text = match overlay.get(&full) {
                Some(text) => Ok(text.clone()),
                None => std::fs::read_to_string(&full),
            };
            match text {
                Ok(text) => request.inputs.push((rel, text)),
                Err(e) => {
                    unreadable = true;
                    fingerprint.push_str(&format!("unreadable {rel}: {}\n", e.kind()));
                    // **A file that is there is never reported as absent.** A
                    // schema saved in UTF-16 answers `InvalidData` here, and
                    // "create the file" is no advice about a file a person can
                    // see in the directory the diagnostic names. A `sources`
                    // entry over the same bytes says `cannot read <path>: …`,
                    // and this says the same sentence.
                    missing.push((
                        match e.kind() {
                            std::io::ErrorKind::NotFound => Diagnostic {
                                code: "no-such-source".to_string(),
                                // The entry, so the loader can name it: this
                                // diagnostic's wording is its page's, and the
                                // page asks which source and which field.
                                message: input.value.clone(),
                                note: None,
                                fix: None,
                                origin: None,
                            },
                            _other => Diagnostic {
                                code: UNREADABLE.to_string(),
                                message: format!("cannot read {rel}: {e}"),
                                note: None,
                                fix: Some(
                                    "check the file exists and is readable".to_string(),
                                ),
                                origin: None,
                            },
                        },
                        input.span,
                    ));
                }
            }
        }
        if unreadable {
            continue;
        }
        let key = generate_key(session, target, &generator.tool.value, &request, flags);
        fingerprint.push_str(key.as_str());
        fingerprint.push('\n');
        entries.push(Entry { generator: generator.clone(), request, key });
    }

    if workspace.generated.key_of(target).as_deref() == Some(fingerprint.as_str()) && !flags.force {
        return;
    }

    let mut outcome = Outcome { diagnostics: missing, ..Outcome::default() };
    let mut produced: Vec<(GeneratedModule, Span)> = Vec::new();
    for entry in entries {
        match answer(session, &workspace, target, &entry, flags) {
            Ok(response) => {
                for module in response.modules {
                    produced.push((module, entry.generator.span));
                }
                for d in response.diagnostics {
                    outcome.diagnostics.push((d, entry.generator.span));
                }
            }
            Err(why) => outcome.diagnostics.push((
                Diagnostic {
                    code: "generator-failed".to_string(),
                    message: String::new(),
                    note: Some(why),
                    fix: None,
                    origin: None,
                },
                entry.generator.span,
            )),
        }
    }
    keep_the_names_that_are_free(&workspace, target, produced, &mut outcome);
    workspace.generated.record(&workspace, target, fingerprint, outcome);
}

/// Moves the modules whose names are the generator's own into the outcome, and
/// reports the ones that are not.
///
/// **A name is either a generator's or a person's, never both.** Two entries
/// naming one module used to be one of them silently replacing the other, and a
/// generated `lib.buri` used to replace a library's whole public surface: the
/// program ran, printed the generator's answer, and `lint` had nothing to say
/// about the file nobody was compiling any more.
///
/// The one that is already there wins, so the file on disk keeps meaning what
/// it says while the build reports the collision.
fn keep_the_names_that_are_free(
    workspace: &Workspace,
    target: TargetId,
    produced: Vec<(GeneratedModule, Span)>,
    outcome: &mut Outcome,
) {
    let package = workspace.package(target.package);
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for (module, span) in produced {
        let clash = if taken.contains(&module.name) {
            Some("a generator on this rule has already named it".to_string())
        } else {
            shadowed_source(&package.dir, &module.name)
                .map(|file| format!("`{}` is a source of this package", file))
        };
        match clash {
            None => {
                taken.insert(module.name.clone());
                outcome.modules.push(Arc::new(module));
            }
            Some(note) => outcome.diagnostics.push((
                Diagnostic {
                    code: "generator-module-taken".to_string(),
                    // The path a person would write, which is what the page
                    // asks for and what an import would have named.
                    message: package.module_path(&module.name),
                    note: Some(note),
                    fix: None,
                    origin: None,
                },
                span,
            )),
        }
    }
}

/// The source file a generated module's name would take over, if it takes one.
///
/// Only the names that resolve to a *module* of the package count, which is why
/// this is a short list rather than "a file with this name exists":
/// `std/codegen/proto` names its module `point.proto` and `lib/wire/point.proto`
/// is a file on disk, and those two are not a collision — a schema is the
/// generator's input, not a module anybody imports.
fn shadowed_source(dir: &std::path::Path, name: &str) -> Option<String> {
    let file = match name {
        "" | "lib.buri" => "lib.buri",
        "main" | "main.buri" => "main.buri",
        "testing" | "testing/lib.buri" => "testing/lib.buri",
        other if other.ends_with(".buri") => other,
        _other => return None,
    };
    dir.join(file).is_file().then(|| file.to_string())
}

/// One entry's answer: the cache's, or the tool's.
fn answer(
    session: &mut Session,
    workspace: &Workspace,
    target: TargetId,
    entry: &Entry,
    flags: &Flags,
) -> Result<Response, String> {
    let tool = &entry.generator.tool.value;
    let cache = Cache::open(&session.root);
    if !flags.force {
        if let Some(response) = cache
            .get(&entry.key)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| Response::decode(&text).ok())
        {
            return Ok(response);
        }
    }
    let response = match tool.strip_prefix("//") {
        None => run_toolchain(session, tool, &entry.request, flags)?,
        Some(_) => {
            let Some(tool_target) = tool_target(workspace, tool) else {
                return Err(format!("`{tool}` names no binary target in this repository"));
            };
            if let Some(path) = cycle(workspace, target, tool_target) {
                return Err(cycle_sentence(workspace, &path));
            }
            let artifact = build_tool(session, tool_target, flags)?;
            run_artifact(&artifact, &entry.request)?
        }
    };
    cache.put(&entry.key, response.encode().as_bytes());
    Ok(response)
}

/// The key one `generators` entry's answer is stored under.
///
/// The platform, the rule's identity, the tool, and the contents of every
/// declared input.
///
/// For a repository tool the tool's identity is its `link` action key. That key
/// is the SHA-256 of everything that decides the artifact's bytes — a strictly
/// finer identity than the artifact's own digest — and it can be computed
/// without linking the tool, so a cache hit costs no build. For a toolchain
/// generator the identity is the tool's *name* on top of the toolchain version
/// every key already carries, because the generator is the toolchain and there
/// are no other bytes to hash.
fn generate_key(
    session: &Session,
    target: TargetId,
    tool: &str,
    request: &Request,
    flags: &Flags,
) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Generate, flags.mode);
    // In the key for the same reason it is in every other one, even though
    // generation does not vary along it today: a key that leaves out something
    // a future action varies on is the shape of a stale-cache bug.
    k.platform(Platform::Js, None);
    let paths: Vec<String> = request.inputs.iter().map(|(p, _)| p.clone()).collect();
    k.rule_identity(&session.workspace.label(target), "generate", &paths);
    k.input("tool", tool.as_bytes());
    if let Some(tool_target) = tool_target(&session.workspace, tool) {
        k.dependency(&crate::build::actions::action_key(
            session,
            tool_target,
            &Output::js(Span::NONE),
            flags,
            Action::Link,
        ));
    }
    for (path, text) in &request.inputs {
        k.input(path, text.as_bytes());
    }
    k.finish()
}

/// Builds a tool for JavaScript, and answers with the artifact it wrote.
///
/// JavaScript, always: a generator runs on the machine doing the build, and an
/// `.mjs` is the one artifact every host can produce and run without a linker.
/// The tool's own `outputs` do not decide this — a generator is built because
/// something else needs it, not because the tool declared an artifact.
fn build_tool(
    session: &mut Session,
    tool: TargetId,
    flags: &Flags,
) -> Result<std::path::PathBuf, String> {
    let output = Output::js(Span::NONE);
    match crate::build::actions::build_target(session, tool, &output, flags) {
        Ok(artifact) => Ok(artifact.path),
        Err(diagnostics) => Err(match diagnostics.items.first() {
            Some(first) => format!("the tool does not build: {}", first.message),
            None => "the tool does not build".to_string(),
        }),
    }
}

/// One step of a cycle: a target, and the span of the edge that reached the
/// next one.
pub type CyclePath = Vec<(TargetId, Option<Span>)>;

/// The path a cycle took, one label to the next.
pub fn cycle_sentence(workspace: &Workspace, path: &[(TargetId, Option<Span>)]) -> String {
    path.iter().map(|(t, _)| workspace.label(*t)).collect::<Vec<_>>().join(" -> ")
}

/// The cycle a rule's generator closes, if it closes one.
///
/// Public because the loader reports it: the diagnostic belongs beside the
/// `generators` entry that wrote the tool down, which is a source location the
/// build layer has no `SourceMap` to render.
pub fn cycle_of(workspace: &Workspace, target: TargetId) -> Option<(&Generator, CyclePath)> {
    for generator in declared(workspace, target) {
        let Some(tool) = tool_target(workspace, &generator.tool.value) else { continue };
        if let Some(path) = cycle(workspace, target, tool) {
            return Some((generator, path));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hard_strings() -> Vec<String> {
        vec![
            String::new(),
            "plain".to_string(),
            "a \"quoted\" word".to_string(),
            "a back\\slash".to_string(),
            "two\nlines\r\nand\ta tab".to_string(),
            "\u{7}bell and \u{1f} unit separator".to_string(),
            "ünïcödé — 日本語 — 🧪".to_string(),
            "}{[],:\"".to_string(),
        ]
    }

    #[test]
    fn a_request_survives_the_wire() {
        let request = Request {
            inputs: hard_strings()
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("lib/x/{i}.schema"), s.clone()))
                .collect(),
            dependencies: vec![("lib/y/dep.schema".to_string(), "a\\b\"c\n".to_string())],
        };
        let line = request.encode();
        assert!(!line.contains('\n'), "the request is one line: {line}");
        assert_eq!(Request::decode(&line).expect("the request decodes"), request);
    }

    #[test]
    fn a_response_survives_the_wire() {
        let response = Response {
            modules: hard_strings()
                .iter()
                .enumerate()
                .map(|(i, s)| GeneratedModule {
                    name: format!("m{i}"),
                    text: s.clone(),
                    anchors: vec![Anchor {
                        start: 0,
                        end: s.len(),
                        file: format!("lib/x/{i}.schema"),
                        span: (3, 9),
                    }],
                })
                .collect(),
            diagnostics: vec![
                Diagnostic {
                    code: "proto-unsupported".to_string(),
                    message: "a \"message\" with\na newline".to_string(),
                    note: Some("a note".to_string()),
                    fix: Some("a fix".to_string()),
                    origin: Some(Origin { file: "lib/x/0.schema".to_string(), span: (18, 29) }),
                },
                Diagnostic {
                    code: "generator-diagnostic".to_string(),
                    message: "nothing to point at".to_string(),
                    note: None,
                    fix: None,
                    origin: None,
                },
            ],
        };
        let line = response.encode();
        assert!(!line.contains('\n'), "the response is one line: {line}");
        assert_eq!(Response::decode(&line).expect("the response decodes"), response);
    }

    /// The shape the protocol is specified in, written by hand rather than by
    /// this encoder — so the two agree about the document rather than about
    /// each other.
    #[test]
    fn the_documented_wire_shape_decodes() {
        let line = concat!(
            r#"{"modules":[{"name":"point.proto","text":"export struct Point {}\n","#,
            r#""anchors":[{"start":0,"end":24,"file":"lib/wire/point.proto","#,
            r#""span":{"start":18,"end":29}}]}],"#,
            r#" "diagnostics":[{"code":"proto-unsupported","message":"no","note":null,"#,
            r#""fix":null,"origin":{"file":"lib/wire/point.proto","span":{"start":18,"end":29}}}]}"#
        );
        let response = Response::decode(line).expect("the documented shape decodes");
        assert_eq!(response.modules.len(), 1);
        let module = response.modules.first().expect("one module");
        assert_eq!(module.name, "point.proto");
        assert_eq!(module.text, "export struct Point {}\n");
        assert_eq!(module.anchor_at(0).map(Anchor::origin).map(|o| o.span), Some((18, 29)));
        assert_eq!(module.anchor_at(24), None);
        let d = response.diagnostics.first().expect("one diagnostic");
        assert_eq!(d.note, None);
        assert_eq!(d.origin.as_ref().map(|o| o.file.as_str()), Some("lib/wire/point.proto"));

        let request = Request::decode(
            r#"{"inputs":[["lib/wire/point.proto","edition = \"2026\";\n"]],"dependencies":[]}"#,
        )
        .expect("the documented request decodes");
        assert_eq!(
            request.inputs,
            vec![("lib/wire/point.proto".to_string(), "edition = \"2026\";\n".to_string())]
        );
    }

    /// A `\u` escape and a surrogate pair, which a generator written against a
    /// JSON library may well produce even where this encoder would not.
    #[test]
    fn escaped_scalars_decode() {
        let response =
            Response::decode(r#"{"modules":[{"name":"m","text":"é🧪","anchors":[]}]}"#)
                .expect("escapes decode");
        assert_eq!(response.modules.first().map(|m| m.text.as_str()), Some("é🧪"));
    }

    #[test]
    fn what_is_not_a_response_says_so() {
        for bad in [
            "",
            "not json",
            "{",
            "[]}",
            r#"{"modules":[{"text":"x"}]}"#,
            r#"{"modules":[{"name":"m","text":"x","anchors":[{"start":0}]}]}"#,
            r#"{"modules":"a string"}"#,
        ] {
            assert!(Response::decode(bad).is_err(), "`{bad}` decoded as a response");
        }
    }

    /// The failure that sent this suite looking: a generator killed by a
    /// signal, whose note said "a signal" and nothing else. A crash, a stack
    /// overflow and an out-of-memory kill are three different bugs and the
    /// note has to tell them apart.
    #[cfg(unix)]
    #[test]
    fn a_generator_killed_by_a_signal_is_named_with_its_signal() {
        use std::os::unix::process::ExitStatusExt as _;
        let crashed = std::process::ExitStatus::from_raw(11);
        let note = said(&format!("the generator {}", how_it_ended(&crashed)), "");
        assert_eq!(note, "the generator was killed by SIGSEGV (signal 11)");

        let out_of_memory = std::process::ExitStatus::from_raw(9);
        let note = said(&format!("the generator {}", how_it_ended(&out_of_memory)), "");
        assert_eq!(note, "the generator was killed by SIGKILL (signal 9)");

        // A number the two platforms disagree about goes unnamed rather than
        // named wrongly.
        let other = std::process::ExitStatus::from_raw(10);
        assert_eq!(how_it_ended(&other), "was killed by signal 10");
    }

    /// What the tool said on the way down comes with the note, whichever way
    /// it went down.
    #[cfg(unix)]
    #[test]
    fn a_crashing_generator_is_reported_with_its_standard_error() {
        use std::os::unix::process::ExitStatusExt as _;
        let crashed = std::process::ExitStatus::from_raw(11);
        let note = said(
            &format!("the generator {}", how_it_ended(&crashed)),
            "\nRangeError: Maximum call stack size exceeded.\n  at print\n",
        );
        assert_eq!(
            note,
            "the generator was killed by SIGSEGV (signal 11)\n\
             RangeError: Maximum call stack size exceeded.\n  at print"
        );

        let refused = std::process::ExitStatus::from_raw(1 << 8);
        assert_eq!(how_it_ended(&refused), "exited with 1");
    }

    /// A tool that fills its pipe may not fill the page. The **tail** is kept,
    /// because what a program says last is what says why it stopped.
    #[test]
    fn a_flood_on_standard_error_is_cut_to_its_tail() {
        let flood = format!("{}the last line", "x".repeat(20_000));
        let note = said("the generator exited with 1", &flood);
        assert!(note.len() < STDERR_TAIL + 200, "the note is bounded: {} bytes", note.len());
        assert!(note.starts_with("the generator exited with 1\n"), "{note}");
        assert!(note.ends_with("the last line"), "the tail is what is kept");
        assert!(
            note.contains("bytes of standard error are not shown"),
            "the note says it was cut: {note}"
        );

        // Cut through a character rather than between two: the note is still
        // text, and the character is not half-written.
        let wide = "é".repeat(20_000);
        let note = said("the generator exited with 1", &wide);
        assert!(note.ends_with('é'), "the tail ends on a character");
        assert!(note.chars().all(|c| c != '\u{fffd}'), "no character was cut in half");
    }

    #[test]
    fn the_innermost_anchor_wins() {
        let module = GeneratedModule {
            name: "m".to_string(),
            text: "0123456789".to_string(),
            anchors: vec![
                Anchor { start: 0, end: 10, file: "a".into(), span: (0, 1) },
                Anchor { start: 0, end: 4, file: "a".into(), span: (2, 3) },
                Anchor { start: 6, end: 8, file: "a".into(), span: (4, 5) },
            ],
        };
        assert_eq!(module.anchor_at(1).map(|a| a.span), Some((2, 3)));
        assert_eq!(module.anchor_at(5).map(|a| a.span), Some((0, 1)));
        assert_eq!(module.anchor_at(7).map(|a| a.span), Some((4, 5)));
        assert_eq!(module.anchor_at(10), None);
    }
}
