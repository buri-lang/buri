//! The element document `ui/testing`'s `render` builds, the readers a
//! `Rendered` answers from it, and the reconciler that patches it on a write
//! (issue #53, phases 2–3).
//!
//! A native twin of the JavaScript `$dom` document double
//! (`backend/js/runtime.js`'s `$dom_make`) and of `$tree_bind`/`$tree_dynamic`.
//! Phase 2 built the static half — a `Prop` read once, a region built once,
//! every identity stamped and never touched. Phase 3 adds the reactive half:
//!
//!   - **A leaf prop patches in place.** `openText` mints one run of text, once,
//!     and `patchText` writes over it whenever the watcher `ui/node`'s
//!     `emitReactiveText` registered re-reads the prop — the record, and its
//!     identity, are never remade. This is the native `$tree_bind`.
//!   - **`computed`/`choose` rebuild.** `enterDynamic` puts two markers around a
//!     region; `beginRegion` removes everything between them and points the
//!     builder at the gap so the re-walk lands there, minting fresh identities;
//!     `endRegion` restores the builder. The watchers a rebuilt subtree left are
//!     disposed by the graph's `run` disposing the previous run's children
//!     (`cli/runtime/ui.rs`), so a departed subtree leaks nothing. This is the
//!     native `$tree_dynamic`.
//!
//! ## What a document holds
//!
//! One arena of records and a tree over it: each record names its parent and
//! its children in order, exactly as the JavaScript double does, because that
//! is what lets a region remove "everything between these two markers" and a
//! reader visit the tree in document order. `records[0]` is the host the tree
//! hangs off — the JavaScript `$dom_make(0, "root")` — and it is never written
//! to markup or counted. A record is an element, a run of text, or a marker; a
//! marker holds the place of a changeable region and emits nothing, exactly as
//! the JavaScript markers do (`runtime.js`'s `$dom_marker`).
//!
//! ## The scene document is the markup
//!
//! `markup()` writes the scene document `ui/node`'s `describe` writes, without
//! the `buri-scene 1` / `viewport` header — an `e <depth> <declarations>` line
//! per element and a `t <depth> <text>` line per run, a line at depth `d` a
//! child of the nearest line above it at `d - 1`. The depth is the tree depth a
//! reader walks to reach the record, and the declarations of an element are the
//! ones the Buri walk `renderInto` computed and handed here as one string, so a
//! native `markup()` is byte-for-byte a headerless `describe`.

use crate::list::Release;
use crate::ui::ComputeEntry;
use crate::value::{str_of, BuriStr, BURI_RT_STR_LEN_MASK};
use std::sync::Mutex;

/// What a record is. A `Marker` emits nothing in markup and is never counted;
/// it is a placeholder around a region a watcher rebuilds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Element,
    Text,
    Marker,
}

/// One record in a document.
///
/// `body` is the declaration half of an element's scene line — the classes and
/// declarations `renderInto` computed — and `text` is a run's raw content, kept
/// unescaped because `text()` joins the runs as they are and only `markup()`
/// escapes. `parent` and `children` are the tree the readers walk and the
/// reconciler patches; a record whose `parent` is `None` has been unlinked by a
/// region rebuild and no reader reaches it.
struct Record {
    identity: i64,
    kind: Kind,
    name: String,
    body: String,
    text: String,
    parent: Option<usize>,
    children: Vec<usize>,
}

/// Where the next record lands: under `parent`, before `anchor` — or at the end
/// of `parent`'s children when `anchor` is `None`. A stack of these is the open
/// path the walk is currently inside, so entering an element pushes the element
/// with a fresh `None` anchor (its children append) and leaving it pops back.
#[derive(Clone, Copy)]
struct Frame {
    parent: usize,
    anchor: Option<usize>,
}

/// A changeable region: the two markers `enterDynamic` put around it, by which
/// `beginRegion` finds the gap to clear and refill.
#[derive(Clone, Copy)]
struct Region {
    start: usize,
    end: usize,
}

/// One rendered tree.
struct Document {
    records: Vec<Record>,
    /// The open path; its last frame is where the next record lands. It is never
    /// empty: the host frame is the floor a balanced walk returns to.
    cursor: Vec<Frame>,
    /// Every region opened in this document, addressed by the handle
    /// `enterDynamic` answered.
    regions: Vec<Region>,
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
                parent: None,
                children: Vec::new(),
            }],
            cursor: vec![Frame { parent: 0, anchor: None }],
            regions: Vec::new(),
        }
    }

    /// The frame the next record lands in.
    fn top(&self) -> Frame {
        // The cursor is never empty; the host frame is always at its floor.
        self.cursor.last().copied().unwrap_or(Frame { parent: 0, anchor: None })
    }

    /// Adds a record in the current frame — under its parent, before its anchor
    /// — and answers its index.
    fn add(&mut self, kind: Kind, name: String, body: String, text: String) -> usize {
        let frame = self.top();
        let index = self.records.len();
        self.records.push(Record {
            identity: mint(),
            kind,
            name,
            body,
            text,
            parent: Some(frame.parent),
            children: Vec::new(),
        });
        let pos = match frame.anchor {
            Some(a) => {
                self.records[frame.parent].children.iter().position(|&c| c == a).unwrap_or_else(
                    || self.records[frame.parent].children.len(),
                )
            }
            None => self.records[frame.parent].children.len(),
        };
        self.records[frame.parent].children.insert(pos, index);
        index
    }

    /// The records reachable from the host, in document order, each with the
    /// tree depth its scene line carries — the host's own children at depth 0.
    fn ordered(&self) -> Vec<(usize, i64)> {
        let mut out = Vec::new();
        self.visit(0, 0, &mut out);
        out
    }

    fn visit(&self, node: usize, depth: i64, out: &mut Vec<(usize, i64)>) {
        for &child in &self.records[node].children {
            out.push((child, depth));
            self.visit(child, depth + 1, out);
        }
    }
}

/// Every document a program has rendered, one table rather than one allocation
/// per handle — `cli/runtime/ui.rs`'s recorder table's reason: a `Rendered`
/// carries an index nothing else can produce, so a test reaches only the tree
/// it rendered, and a watcher that patches one long after the render returned
/// reaches it by the same index.
static DOCUMENTS: Mutex<Vec<Document>> = Mutex::new(Vec::new());

fn documents() -> std::sync::MutexGuard<'static, Vec<Document>> {
    match DOCUMENTS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The monotonic identity counter — the JavaScript `$dom.identities`, which
/// stamps a record once at creation and reassigns it to no other. It runs
/// across every document a program builds rather than per document, exactly as
/// the JavaScript one does: a test asserts that a record kept its identity, and
/// two documents' records never share one.
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
// nothing here creates a graph node, so a reactive re-walk may call them from
// inside a watcher — the rule that keeps a Buri reconciler out of the graph does
// not reach a builder.

/// A fresh, empty document, and the handle a builder carries.
fn open() -> i64 {
    let mut all = documents();
    all.push(Document::new());
    (all.len() as i64) - 1
}

fn with_doc<R>(handle: i64, f: impl FnOnce(&mut Document) -> R) -> Option<R> {
    let mut all = documents();
    let doc = usize::try_from(handle).ok().and_then(|i| all.get_mut(i))?;
    Some(f(doc))
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
/// same trampoline [`crate::ui::buri_rt_ui_render_walk`] is. The initial render
/// runs each leaf watcher and each region watcher once through that walk, so
/// what comes back is already the resting tree.
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

/// `emitElement(builder, name, body)` — an element record, entered so that
/// everything `renderInto` emits until its matching `exitElement` is inside it.
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
    with_doc(handle, |doc| {
        let index = doc.add(Kind::Element, name, body, String::new());
        doc.cursor.push(Frame { parent: index, anchor: None });
    });
}

/// `exitElement(builder)` — closes the element `emitElement` opened, so the next
/// record is its sibling rather than its child.
///
/// The host is never closed: a walk that is balanced leaves it open, and one
/// that is not stops emptying the stack here rather than removing it.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_exit_element(handle: i64) {
    with_doc(handle, |doc| {
        if doc.cursor.len() > 1 {
            doc.cursor.pop();
        }
    });
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
    with_doc(handle, |doc| {
        doc.add(Kind::Text, String::new(), String::new(), content);
    });
}

/// `openText(builder)` — an empty run of text under the open element, and the
/// index a reactive leaf patches it by. Its identity is minted here, once.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_open_text(handle: i64) -> i64 {
    with_doc(handle, |doc| doc.add(Kind::Text, String::new(), String::new(), String::new()))
        .map(|i| i as i64)
        .unwrap_or(-1)
}

/// `patchText(builder, at, content)` — writes `content` over the run `openText`
/// answered `at`, leaving its identity as it was. The native `$dom_data`, at the
/// one field a run has.
///
/// # Safety
/// `content` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_patch_text(
    handle: i64,
    at: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let content = unsafe { text_of(ptr, len) };
    with_doc(handle, |doc| {
        if let Some(record) = usize::try_from(at).ok().and_then(|i| doc.records.get_mut(i)) {
            record.text = content;
        }
    });
}

/// `enterDynamic(builder)` — two markers around a changeable region, both in the
/// current frame and adjacent, and the handle they are addressed by. The native
/// `$tree_dynamic`'s two `$tree_mark`s.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_enter_dynamic(handle: i64) -> i64 {
    with_doc(handle, |doc| {
        let start = doc.add(Kind::Marker, String::new(), String::new(), String::new());
        let end = doc.add(Kind::Marker, String::new(), String::new(), String::new());
        doc.regions.push(Region { start, end });
        (doc.regions.len() as i64) - 1
    })
    .unwrap_or(-1)
}

impl Document {
    /// Clears a region and points the builder at the gap. Removes every record
    /// between the two markers (the whole of what the last build put there, its
    /// nested regions included) by unlinking them from their parent, then pushes
    /// a frame that inserts before the end marker — so the re-walk lands exactly
    /// where the last one did. The native `$tree_dynamic`'s
    /// `for (…of $dom_between) $dom_remove`.
    fn begin_region(&mut self, region: usize) {
        let Some(&Region { start, end }) = self.regions.get(region) else {
            return;
        };
        let parent = self.records.get(start).and_then(|r| r.parent).unwrap_or(0);
        let (si, ei) = {
            let children = &self.records[parent].children;
            (
                children.iter().position(|&c| c == start),
                children.iter().position(|&c| c == end),
            )
        };
        if let (Some(si), Some(ei)) = (si, ei)
            && ei > si + 1
        {
            let removed: Vec<usize> = self.records[parent].children.drain(si + 1..ei).collect();
            for child in removed {
                if let Some(record) = self.records.get_mut(child) {
                    record.parent = None;
                }
            }
        }
        self.cursor.push(Frame { parent, anchor: Some(end) });
    }

    /// Leaves the region `begin_region` entered, restoring where the builder
    /// emits. The frame it pops is the one `begin_region` pushed, because the
    /// re-walk between them is balanced.
    fn end_region(&mut self) {
        if self.cursor.len() > 1 {
            self.cursor.pop();
        }
    }
}

/// `rebuildRegion(builder, region, node, walk)` — the native `$tree_dynamic`'s
/// re-render: clear the region, point the builder at the gap, walk `node` there
/// with `walk`, and restore the builder. `walk` is `renderInto`, driven in place
/// through the same trampoline [`crate::ui::buri_rt_ui_render_walk`] the initial
/// mount uses — with its context supplied and dropped by the runtime, so the
/// watcher that calls this captured no context. The clearing runs inside the
/// document lock; the walk runs outside it, because the walk is Buri code that
/// reads and patches the document (and reads the graph) on its way through, so
/// holding the lock across it would deadlock the very emits it makes.
///
/// # Safety
/// `node` points at one whole `Node`; `entry`/`state`/`frame_at` are the walk
/// [`crate::ui::buri_rt_ui_render_walk`]'s arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_rebuild_region(
    handle: i64,
    region: i64,
    node: *const u8,
    entry: ComputeEntry,
    state: *mut u8,
    frame_at: i64,
) {
    let Ok(region) = usize::try_from(region) else {
        return;
    };
    with_doc(handle, |doc| doc.begin_region(region));
    // SAFETY: forwarded to the caller's promise; `handle` is a live document,
    // its builder now pointed at the region gap.
    unsafe { crate::ui::buri_rt_ui_render_walk(entry, state, handle, node, frame_at) };
    with_doc(handle, |doc| doc.end_region());
}

/// `reactive(body)` — the renderer's own `watch`: registers `body` in the graph
/// and runs it once, so a leaf follows its prop and a region rebuilds on every
/// change to what its build read.
///
/// It is [`crate::ui::buri_rt_ui_watch`] with the stride and release a watcher
/// never uses dropped, exactly as `Headless.watch` forwards to it. The graph
/// owns the watcher and disposes it with whatever created it — a region's
/// rebuild disposes the leaf watchers its last build registered — so this needs
/// nothing of the document and reaches only `cli/runtime/ui.rs`.
///
/// # Safety
/// `entry` is the thunk the backend generated for `body` and `state` the record
/// it was generated against; `bytes` and `frame_at` are that record's size and
/// the offset a working frame is written at, or negative.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_reactive(
    entry: ComputeEntry,
    state: *const u8,
    bytes: usize,
    frame_at: i64,
    _stride: usize,
    _release: Release,
    body: Release,
) {
    // SAFETY: forwarded to the caller's promise.
    unsafe { crate::ui::buri_rt_ui_watch(entry, state, bytes, frame_at, body) };
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
    for (index, depth) in doc.ordered() {
        let record = &doc.records[index];
        match record.kind {
            Kind::Element => lines.push(format!("e {} {}", depth, record.body)),
            Kind::Text => lines.push(format!("t {} {}", depth, escape_run(&record.text))),
            Kind::Marker => {}
        }
    }
    lines.join("\n")
}

/// `Rendered.text()` — every run of text, in order, joined by a space.
///
/// The JavaScript `$dom_runs(...).join(" ")`, and the runs are the records' own
/// content, unescaped.
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
            doc.ordered()
                .into_iter()
                .filter(|(i, _)| doc.records[*i].kind == Kind::Text)
                .map(|(i, _)| doc.records[i].text.as_str())
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
/// The JavaScript `$dom_elements(self, name)[index].identity`, and it aborts the
/// same way — `this tree has no <name> <index>` — when the index is out of
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
        .and_then(|doc| usize::try_from(index).ok().and_then(|at| named(doc, &name).nth(at)));
    match found {
        Some(identity) => identity,
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

/// The identities of the reachable elements of one name, in document order.
fn named<'a>(doc: &'a Document, name: &'a str) -> impl Iterator<Item = i64> + 'a {
    doc.ordered().into_iter().filter_map(move |(i, _)| {
        let record = &doc.records[i];
        (record.kind == Kind::Element && record.name == name).then_some(record.identity)
    })
}
