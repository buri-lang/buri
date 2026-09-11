//! The element document `ui/testing`'s `render` builds, and the readers a
//! `Rendered` answers from it (issue #53, phase 2).
//!
//! A native twin of the JavaScript `$dom` document double
//! (`backend/js/runtime.js`'s `$dom_make`), and the static half of the renderer
//! over it. **Nothing here is reactive.** A `Prop` is read once for its current
//! value, a `computed`/`choose`/`each` is built once for its current content,
//! and every identity is stamped and never patched — the watchers, the keyed
//! reconciler and the disposal that make a write move a row rather than rebuild
//! it are a later phase (`design/native/`'s #53 plan, phases 3–4).
//!
//! ## What a document holds
//!
//! One arena of records in **document order** — the order a depth-first walk of
//! the tree visits them, which is the order they are emitted in. That order is
//! the whole of what the readers need: `markup` writes each record's line in
//! it, `text` joins the runs in it, and `count`/`identity` walk the records of
//! one name in it. A record is an element, a run of text, or a marker; a marker
//! emits nothing, exactly as the JavaScript markers around a changeable region
//! do (`runtime.js`'s `$dom_marker`), and none is made until the reconciler
//! that needs them lands.
//!
//! ## The scene document is the markup
//!
//! `markup()` writes the scene document `ui/node`'s `describe` writes, without
//! the `buri-scene 1` / `viewport` header — an `e <depth> <declarations>` line
//! per element and a `t <depth> <text>` line per run, a line at depth `d` a
//! child of the nearest line above it at `d - 1` (`cli/runtime/paint.rs`'s
//! header, `ui_node.buri`'s `elementLine`/`textLine`). The declarations of an
//! element are computed by the Buri walk `renderInto` — the same helpers
//! `describe` uses — and handed here as one string, so the two cannot drift and
//! a native `markup()` is byte-for-byte a headerless `describe`. What this side
//! adds is the depth (from the nesting the builder tracks) and, for a run, the
//! escape `describe`'s `textLine` makes: a backslash, a newline and a carriage
//! return, and no fourth.
//!
//! The element **name** — `h1`, `li`, `button`, `input`, `dialog` — is not in a
//! scene line at all; it is what `count(name)`/`identity(name, i)` walk the
//! records by, exactly as the JavaScript `$dom_elements(node, name)` reads
//! `node.name`. So a record keeps its name beside the line it writes.

use crate::value::{str_of, BuriStr, BURI_RT_STR_LEN_MASK};
use std::sync::Mutex;

/// What a record is. `Marker` is here for the reconciler that will make one and
/// is never built by the static walk; it emits nothing in markup either way.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Element,
    Text,
    Marker,
}

/// One record in a document.
///
/// `body` is the declaration half of an element's scene line — the classes and
/// the declarations `renderInto` computed — and `text` is a run's raw content,
/// kept unescaped because `text()` joins the runs as they are and only
/// `markup()` escapes. `depth` is the nesting the builder tracked when the
/// record was made, so a reader needs no tree walk to write the line. `parent`
/// and `children` record the tree the reconciler will patch; the static
/// readers do not need them, but the document is the reconciler's too.
struct Record {
    identity: i64,
    kind: Kind,
    name: String,
    body: String,
    text: String,
    depth: i64,
    #[allow(dead_code)]
    parent: Option<usize>,
    #[allow(dead_code)]
    children: Vec<usize>,
}

/// One rendered tree.
///
/// `records[0]` is the host the tree is rendered into — the JavaScript
/// `$dom_make(0, "root")` — and it is never written to markup or counted; the
/// tree proper is `records[1..]`. `open` is the stack of elements the walk is
/// currently inside, so a child knows its parent and the depth its line
/// carries.
struct Document {
    records: Vec<Record>,
    open: Vec<usize>,
    depth: i64,
}

impl Document {
    fn new() -> Document {
        Document {
            records: vec![Record {
                identity: mint(),
                kind: Kind::Element,
                name: "root".to_owned(),
                body: String::new(),
                text: String::new(),
                depth: -1,
                parent: None,
                children: Vec::new(),
            }],
            open: vec![0],
            depth: 0,
        }
    }

    /// Adds a record under whatever element is open, and answers its index.
    fn add(&mut self, kind: Kind, name: String, body: String, text: String) -> usize {
        let parent = self.open.last().copied();
        let index = self.records.len();
        self.records.push(Record {
            identity: mint(),
            kind,
            name,
            body,
            text,
            depth: self.depth,
            parent,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            self.records[p].children.push(index);
        }
        index
    }
}

/// Every document a program has rendered, one table rather than one allocation
/// per handle — `cli/runtime/ui.rs`'s recorder table's reason: a `Rendered`
/// carries an index nothing else can produce, so a test reaches only the tree
/// it rendered.
static DOCUMENTS: Mutex<Vec<Document>> = Mutex::new(Vec::new());

fn documents() -> std::sync::MutexGuard<'static, Vec<Document>> {
    match DOCUMENTS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The monotonic identity counter — the JavaScript `$dom.identities`, which
/// stamps a record once at creation and reassigns it to no other
/// (`runtime.js`'s `$dom_make`). It runs across every document a program builds
/// rather than per document, exactly as the JavaScript one does: a test asserts
/// that a record kept its identity, and two documents' records never share one.
static IDENTITIES: Mutex<i64> = Mutex::new(0);

fn mint() -> i64 {
    let mut g = match IDENTITIES.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let id = *g;
    *g += 1;
    id
}

/// The bytes a `Str` argument covers, its length masked of VALUE-MODEL.md
/// §3.1's ASCII flag as every entry that takes a `Str` masks it
/// (`cli/runtime/text.rs`'s header).
///
/// # Safety
/// `ptr` and `len` are a readable range, or `ptr` is null with a zero length.
unsafe fn text_of(ptr: *const u8, len: u64) -> String {
    let n = (len & BURI_RT_STR_LEN_MASK) as usize;
    if ptr.is_null() || n == 0 {
        return String::new();
    }
    // SAFETY: the caller promises `n` readable bytes at `ptr`.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, n) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// A run's content escaped for a `t` line — `ui_node.buri`'s `textLine`, word
/// for word: a backslash first (or the next two would double-escape), then a
/// newline and a carriage return, and no fourth character.
fn escape_run(content: &str) -> String {
    content.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r")
}

// ---------------------------------------------------------------------------
// The builder
// ---------------------------------------------------------------------------
//
// `renderInto` (Buri, `ui/node`) walks a `Node` and calls these to build the
// document, the way `describe` walks one and builds `[Str]`. The builder is
// inert data addressed by the handle these return and take, like the recorder:
// nothing here creates a graph node, so a later phase's reactive re-walk may
// call them from inside a computation — the rule that keeps a Buri reconciler
// out of the graph does not reach a builder.

/// A fresh, empty document, and the handle a builder carries.
fn open() -> i64 {
    let mut all = documents();
    all.push(Document::new());
    (all.len() as i64) - 1
}

/// `newDocument()` — a fresh, empty document, exposed for the C driver.
///
/// # Safety
/// `out` is writable and aligned for eight bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_doc_open(out: *mut i64) {
    let handle = open();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(handle) };
}

/// `render(ctx, root)` on the native backend — the mount `render`'s thin body
/// reaches: open a document, walk `root` into it with the `renderInto` closure
/// the caller passed, and answer the handle a `Rendered` carries.
///
/// The context is dropped as a step drops one; `root` crosses by reference, a
/// pointer to the one `Node` the walk destructures and this side never reads;
/// and the walk is the closure `render` handed over, invoked once through the
/// same trampoline [`crate::ui::buri_rt_ui_render_walk`] is. Static: the walk
/// reads each `Prop` once and builds each region once, so this is the initial
/// render and nothing re-runs.
///
/// # Safety
/// `root` points at one whole `Node`; `entry` is the thunk the backend
/// generated for the walk and `state` the record it was generated against;
/// `frame_at` is an offset inside the record or negative.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_mount(
    root: *const u8,
    entry: crate::ui::ComputeEntry,
    state: *mut u8,
    frame_at: i64,
) -> i64 {
    let handle = open();
    // SAFETY: forwarded to the caller's promise; `handle` is a live document.
    unsafe { crate::ui::buri_rt_ui_render_walk(entry, state, handle, root, frame_at) };
    handle
}

/// `emitElement(builder, name, body)` — an element record, and everything
/// `renderInto` emits until its matching `exitElement` is inside it.
///
/// `name` is the scene element name `count`/`identity` walk by; `body` is the
/// declaration half of its scene line, computed by the Buri walk.
///
/// # Safety
/// `name`/`body` are readable UTF-8 ranges, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_emit_element(
    handle: i64,
    _name_base: *mut u8,
    name_ptr: *const u8,
    name_len: u64,
    _body_base: *mut u8,
    body_ptr: *const u8,
    body_len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let name = unsafe { text_of(name_ptr, name_len) };
    // SAFETY: forwarded to the caller's promise.
    let body = unsafe { text_of(body_ptr, body_len) };
    let mut all = documents();
    let Some(doc) = usize::try_from(handle).ok().and_then(|i| all.get_mut(i)) else {
        return;
    };
    let index = doc.add(Kind::Element, name, body, String::new());
    doc.open.push(index);
    doc.depth += 1;
}

/// `exitElement(builder)` — closes the element `emitElement` opened, so the
/// next record is its sibling rather than its child.
///
/// The host is never closed: a walk that is balanced leaves it open, and one
/// that is not stops emptying the stack here rather than removing it.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_exit_element(handle: i64) {
    let mut all = documents();
    let Some(doc) = usize::try_from(handle).ok().and_then(|i| all.get_mut(i)) else {
        return;
    };
    if doc.open.len() > 1 {
        doc.open.pop();
        doc.depth -= 1;
    }
}

/// `emitText(builder, content)` — a run of text under whatever element is open.
///
/// # Safety
/// `content` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_emit_text(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let content = unsafe { text_of(ptr, len) };
    let mut all = documents();
    let Some(doc) = usize::try_from(handle).ok().and_then(|i| all.get_mut(i)) else {
        return;
    };
    doc.add(Kind::Text, String::new(), String::new(), content);
}

// ---------------------------------------------------------------------------
// The readers
// ---------------------------------------------------------------------------

/// `Rendered.markup()` — the scene document, headerless.
///
/// One `e <depth> <body>` per element and one `t <depth> <escaped>` per run, in
/// document order, joined by a newline. The host is skipped and a marker emits
/// nothing.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_rendered_markup(handle: i64, out: *mut BuriStr) {
    let all = documents();
    let text = usize::try_from(handle)
        .ok()
        .and_then(|i| all.get(i))
        .map(markup_of)
        .unwrap_or_default();
    let answer = str_of(&text);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(answer) };
}

fn markup_of(doc: &Document) -> String {
    let mut lines: Vec<String> = Vec::new();
    for record in doc.records.iter().skip(1) {
        match record.kind {
            Kind::Element => lines.push(format!("e {} {}", record.depth, record.body)),
            Kind::Text => {
                lines.push(format!("t {} {}", record.depth, escape_run(&record.text)))
            }
            Kind::Marker => {}
        }
    }
    lines.join("\n")
}

/// `Rendered.text()` — every run of text, in order, joined by a space.
///
/// The JavaScript `$dom_runs(...).join(" ")`, and the runs are the records'
/// own content, unescaped.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_rendered_text(handle: i64, out: *mut BuriStr) {
    let all = documents();
    let text = usize::try_from(handle)
        .ok()
        .and_then(|i| all.get(i))
        .map(|doc| {
            doc.records
                .iter()
                .skip(1)
                .filter(|r| r.kind == Kind::Text)
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let answer = str_of(&text);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(answer) };
}

/// `Rendered.count(name)` — how many elements of this name the tree holds.
///
/// The JavaScript `$dom_elements(self, name).length`, and the name is the scene
/// element name the record keeps.
///
/// # Safety
/// `name` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_rendered_count(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> i64 {
    // SAFETY: forwarded to the caller's promise.
    let name = unsafe { text_of(ptr, len) };
    let all = documents();
    usize::try_from(handle)
        .ok()
        .and_then(|i| all.get(i))
        .map(|doc| named(doc, &name).count() as i64)
        .unwrap_or(0)
}

/// `Rendered.identity(name, index)` — the number the runtime stamped the
/// `index`th element of this name with when it made it.
///
/// The JavaScript `$dom_elements(self, name)[index].identity`, and it aborts
/// the same way — `this tree has no <name> <index>` — when the index is out of
/// range, so a test that asks for a row that is not there fails rather than
/// reading a neighbour.
///
/// # Safety
/// `name` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_rendered_identity(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    index: i64,
) -> i64 {
    // SAFETY: forwarded to the caller's promise.
    let name = unsafe { text_of(ptr, len) };
    let all = documents();
    let found = usize::try_from(handle)
        .ok()
        .and_then(|i| all.get(i))
        .and_then(|doc| {
            usize::try_from(index).ok().and_then(|at| named(doc, &name).nth(at))
        });
    match found {
        Some(record) => record.identity,
        None => {
            drop(all);
            crate::abort::die(&[
                b"this tree has no ",
                name.as_bytes(),
                b" ",
                index.to_string().as_bytes(),
            ])
        }
    }
}

/// The element records of one name, in document order.
fn named<'a>(doc: &'a Document, name: &'a str) -> impl Iterator<Item = &'a Record> {
    doc.records
        .iter()
        .skip(1)
        .filter(move |r| r.kind == Kind::Element && r.name == name)
}
