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
    /// The graph node a button's `onPress` or a form's `onSubmit` lives on, or
    /// `-1`. Firing it is `$dom_fire`, and disposing the row it belongs to
    /// disposes it — the native twin of the JavaScript listener on an element.
    press: i64,
    /// The signal a field's or a toggle's value is bound to, or `-1`. `fill`
    /// writes it a string and `flip` its negation, the way the JavaScript
    /// `input`/`change` listener writes the bound signal.
    value_signal: i64,
    /// Whether this button submits the form it is in — the JavaScript
    /// `type="submit"`, the whole of what makes a press or Enter submit.
    submit: bool,
    /// A control's accessible name, for `press` to address it by even when its
    /// glyphs are its children — the JavaScript `aria-label`. Empty when the
    /// element carries none, in which case its own text is its name.
    label: String,
    /// A route link's plain-click handler graph node, or `-1`. `follow` fires
    /// it and `openInNewTab` leaves it alone — the scene twin of the anchor's
    /// click listener.
    follow: i64,
}

/// A record with no widget state — an element, a run or a marker before any
/// event arm attaches to it.
fn plain_record(identity: i64, kind: Kind, name: String, body: String, text: String, parent: Option<usize>) -> Record {
    Record {
        identity,
        kind,
        name,
        body,
        text,
        parent,
        children: Vec::new(),
        press: -1,
        value_signal: -1,
        submit: false,
        label: String::new(),
        follow: -1,
    }
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

/// One row of a keyed list: its key, the two markers holding its place — so that
/// moving it moves whatever it rendered — and the graph owner it was built
/// under, so that disposing it disposes what it created. The native
/// `$tree_row`'s `{ key, start, end, owner }`.
#[derive(Clone)]
struct RowRec {
    key: String,
    start: usize,
    end: usize,
    owner: i64,
}

/// A keyed list: the two markers `enterEach` put around it, the parent they and
/// the rows are children of, the graph owner the rows hang off — deliberately
/// not the reconciling watcher, so a row survives the watcher re-running — and
/// the rows as they stand, keyed. The native `$tree_each`'s state.
struct EachRegion {
    start: usize,
    end: usize,
    parent: usize,
    list_owner: i64,
    rows: Vec<RowRec>,
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
    /// Every keyed list opened in this document, addressed by the handle
    /// `enterEach` answered.
    each_regions: Vec<EachRegion>,
    /// Every `onPressOutside` handler registered, paired with the element whose
    /// subtree a press must land outside of to fire it. The handler lives on a
    /// graph node (disposed with the subtree), so a stale pair fires nothing.
    outside: Vec<(i64, usize)>,
}

impl Document {
    fn new() -> Document {
        Document {
            records: vec![plain_record(
                mint(),
                Kind::Element,
                "root".to_owned(),
                String::new(),
                String::new(),
                None,
            )],
            cursor: vec![Frame { parent: 0, anchor: None }],
            regions: Vec::new(),
            each_regions: Vec::new(),
            outside: Vec::new(),
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
        self.records.push(plain_record(mint(), kind, name, body, text, Some(frame.parent)));
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

/// `openElement(builder, name, body)` — emits an element and enters it, as
/// [`buri_rt_ui_node_emit_element`] does, and answers the index a reactive
/// style patches its body by. Its identity is minted once, here.
///
/// # Safety
/// `name`/`body` are readable UTF-8 ranges, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_open_element(
    handle: i64,
    _name_base: *mut u8,
    name_ptr: *const u8,
    name_len: u64,
    _body_base: *mut u8,
    body_ptr: *const u8,
    body_len: u64,
) -> i64 {
    // SAFETY: forwarded to the caller's promise.
    let name = unsafe { text_of(name_ptr, name_len) };
    // SAFETY: forwarded to the caller's promise.
    let body = unsafe { text_of(body_ptr, body_len) };
    with_doc(handle, |doc| {
        let index = doc.add(Kind::Element, name, body, String::new());
        doc.cursor.push(Frame { parent: index, anchor: None });
        index as i64
    })
    .unwrap_or(-1)
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

/// `patchBody(builder, at, body)` — writes `body` over the element `openElement`
/// answered `at`, leaving its identity, children and place. The scene twin of a
/// `$tree_styles` bind re-running when a reactive style changes.
///
/// # Safety
/// `body` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_patch_body(
    handle: i64,
    at: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let body = unsafe { text_of(ptr, len) };
    with_doc(handle, |doc| {
        if let Some(record) = usize::try_from(at).ok().and_then(|i| doc.records.get_mut(i)) {
            record.body = body;
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

/// `beginRegion(builder, region)` — clears `region` and points the builder at
/// its gap, so the emits that follow land between its markers. `rebuildRegion`
/// split open, for a reactive widget that emits inline under its own watcher
/// rather than walking a node (it needs no context).
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_begin_region(handle: i64, region: i64) {
    let Ok(region) = usize::try_from(region) else {
        return;
    };
    with_doc(handle, |doc| doc.begin_region(region));
}

/// `endRegion(builder)` — restores where the builder emits after `beginRegion`,
/// the frame `begin_region` pushed.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_end_region(handle: i64) {
    with_doc(handle, |doc| doc.end_region());
}

impl Document {
    /// A marker record under `parent`, inserted before the child `anchor` — the
    /// positional twin of [`Document::add`], reached by the reconciler rather
    /// than the walk's cursor. A marker emits nothing and is never counted; it
    /// holds the place of a row so the row can be moved and disposed by it.
    fn marker_before(&mut self, parent: usize, anchor: usize) -> usize {
        let index = self.records.len();
        self.records.push(plain_record(
            mint(),
            Kind::Marker,
            String::new(),
            String::new(),
            String::new(),
            Some(parent),
        ));
        let children = &mut self.records[parent].children;
        let pos = children.iter().position(|&c| c == anchor).unwrap_or(children.len());
        children.insert(pos, index);
        index
    }

    /// Moves the contiguous run of children from `start` to `end` (a row's two
    /// markers and everything between them) to just before `anchor` — the native
    /// `$tree_move`. Walking the keys backwards makes `anchor` a record already
    /// in its final place, so one pass over the list positions everything.
    fn move_block(&mut self, parent: usize, start: usize, end: usize, anchor: usize) {
        let children = &mut self.records[parent].children;
        let (Some(si), Some(ei)) = (
            children.iter().position(|&c| c == start),
            children.iter().position(|&c| c == end),
        ) else {
            return;
        };
        if ei < si {
            return;
        }
        let block: Vec<usize> = children.drain(si..=ei).collect();
        let pos = children.iter().position(|&c| c == anchor).unwrap_or(children.len());
        for (k, record) in block.into_iter().enumerate() {
            children.insert(pos + k, record);
        }
    }

    /// Unlinks a row's run of children from `parent`, so no reader reaches it —
    /// the native `$tree_detach`. What the row's graph owner holds is disposed
    /// separately, by the caller.
    fn detach_block(&mut self, parent: usize, start: usize, end: usize) {
        let children = &mut self.records[parent].children;
        let (Some(si), Some(ei)) = (
            children.iter().position(|&c| c == start),
            children.iter().position(|&c| c == end),
        ) else {
            return;
        };
        if ei < si {
            return;
        }
        let block: Vec<usize> = children.drain(si..=ei).collect();
        for record in block {
            if let Some(r) = self.records.get_mut(record) {
                r.parent = None;
            }
        }
    }
}

/// `enterEach(builder)` — the native `$tree_each`'s setup: two markers around
/// the keyed list, and one graph **owner** the rows hang off. The owner is made
/// under whatever is running — a top-level list belongs to nothing and a list
/// inside a rebuilt region belongs to that region's watcher, so it is disposed
/// with the subtree that holds it — and deliberately not under the reconciling
/// watcher, so a row survives that watcher re-running. Answers the handle
/// `reconcile` names the list by.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_enter_each(handle: i64) -> i64 {
    let list_owner = crate::ui::rows::owner();
    with_doc(handle, |doc| {
        let start = doc.add(Kind::Marker, String::new(), String::new(), String::new());
        let end = doc.add(Kind::Marker, String::new(), String::new(), String::new());
        let parent = doc.records.get(start).and_then(|r| r.parent).unwrap_or(0);
        doc.each_regions.push(EachRegion { start, end, parent, list_owner, rows: Vec::new() });
        (doc.each_regions.len() as i64) - 1
    })
    .unwrap_or(-1)
}

/// `reconcile(builder, region, keys, build)` — the native `$tree_reconcile`,
/// driven from inside the list's watcher on every change to what the keys read.
///
/// `keys` is the `[Str]` the watcher computed by driving `count`/`keyAt` under
/// its scope (so those reads are the list's dependencies); this side keys the
/// prior rows, walks the new keys backwards, **moves** a surviving key's records
/// — the same records, the same identities — and builds only genuinely new keys.
/// A departed key's row owner is disposed, which stops every watcher the row
/// created, and forgotten from the list owner so the list holds no disposed
/// node.
///
/// `build` is the row's `fn(C, Scope, Int) => ()` — `renderInto(c, builder,
/// rowAt(c, scope, index))` — driven through [`crate::ui::buri_rt_ui_render_walk`]
/// with the row's owner as the `Scope` (the trampoline's index word) and the
/// row index as the element. It is set to build **under** that owner and with
/// its reads subscribing nothing, so every watcher the row's leaves register is
/// the owner's child and dies with the row. The document lock is dropped across
/// each build, because the build is Buri code that locks the document to emit.
///
/// # Safety
/// `keys` is a `[Str]`: `keys_ptr` points at `keys_len` [`BuriStr`] elements, or
/// is null with a zero length. `build_entry`/`build_state`/`build_frame_at` are
/// the walk trampoline's arguments for the row body.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_reconcile(
    handle: i64,
    region: i64,
    keys_ptr: *const u8,
    keys_len: u64,
    build_entry: ComputeEntry,
    build_state: *mut u8,
    build_frame_at: i64,
) {
    let Ok(region) = usize::try_from(region) else {
        return;
    };
    // SAFETY: the caller promises `keys_len` `BuriStr` elements at `keys_ptr`.
    let keys = unsafe { keys_of(keys_ptr, keys_len) };

    // The list's markers, its parent and owner, and the rows as they stand.
    let Some((end, parent, list_owner, prev)) = with_doc(handle, |doc| {
        doc.each_regions
            .get(region)
            .map(|r| (r.end, r.parent, r.list_owner, r.rows.clone()))
    })
    .flatten() else {
        return;
    };

    let mut by_key: std::collections::HashMap<String, RowRec> =
        prev.into_iter().map(|r| (r.key.clone(), r)).collect();
    let mut next: Vec<RowRec> = Vec::with_capacity(keys.len());
    for _ in 0..keys.len() {
        next.push(RowRec { key: String::new(), start: 0, end: 0, owner: -1 });
    }
    let mut anchor = end;

    for i in (0..keys.len()).rev() {
        let key = &keys[i];
        if let Some(row) = by_key.remove(key) {
            let (start, end) = (row.start, row.end);
            with_doc(handle, |doc| doc.move_block(parent, start, end, anchor));
            anchor = start;
            next[i] = row;
        } else {
            let row_owner = crate::ui::rows::child_owner(list_owner);
            let (row_start, row_end) = with_doc(handle, |doc| {
                let start = doc.marker_before(parent, anchor);
                let end = doc.marker_before(parent, anchor);
                doc.cursor.push(Frame { parent, anchor: Some(end) });
                (start, end)
            })
            .unwrap_or((0, 0));
            let saved = crate::ui::rows::enter_row(row_owner);
            let at = i as i64;
            let at_ptr = std::ptr::addr_of!(at).cast::<u8>();
            // SAFETY: `build_*` are the row body's trampoline arguments; the
            // owner is one live word crossing as the scope, and `at_ptr` one
            // readable word crossing as the element.
            unsafe {
                crate::ui::buri_rt_ui_render_walk(
                    build_entry,
                    build_state,
                    row_owner,
                    at_ptr,
                    build_frame_at,
                );
            }
            crate::ui::rows::leave_row(saved);
            with_doc(handle, |doc| {
                if doc.cursor.len() > 1 {
                    doc.cursor.pop();
                }
            });
            anchor = row_start;
            next[i] = RowRec { key: key.clone(), start: row_start, end: row_end, owner: row_owner };
        }
    }

    // Whatever key is left never appeared this time: its records leave the tree
    // and its owner is disposed and forgotten, so the next write runs the
    // computations of the rows that stayed and no others.
    for (_key, row) in by_key.drain() {
        with_doc(handle, |doc| doc.detach_block(parent, row.start, row.end));
        crate::ui::rows::dispose(row.owner);
        crate::ui::rows::forget(list_owner, row.owner);
    }

    with_doc(handle, |doc| {
        if let Some(r) = doc.each_regions.get_mut(region) {
            r.rows = next;
        }
    });
}

// ---------------------------------------------------------------------------
// The event arms
// ---------------------------------------------------------------------------
//
// `registerPress`/`registerValue`/`markSubmit` are builder intrinsics the walk
// calls while an element is open, to arm it: a button or a form stores the
// handler graph node its press or submit fires, a field or a toggle stores the
// signal its `fill`/`flip` writes, and a submit button flags that its press
// submits the form. `press`/`fill`/`flip`/`submit` are the readers a `Rendered`
// answers, each resolving a widget and applying the reachability, inertness and
// implicit-submission rules before it mutates a signal or fires a handler — the
// native `$ui_testing_Rendered_*`.

/// The element the walk currently has open — where an event arm attaches. The
/// cursor's top frame's parent is that element, because emitting one pushes a
/// frame under it.
fn open_element(doc: &Document) -> usize {
    doc.top().parent
}

/// `registerPress(builder, onPress)` — keeps a button's or a form's handler on a
/// graph node and stores that node on the open element, so a press or a submit
/// can fire it. The keeping is the graph's ([`crate::ui::buri_rt_ui_node_register_handler`]),
/// so disposing the row the element is in disposes the handler with it.
///
/// # Safety
/// `entry`/`state`/`bytes`/`frame_at`/`body` are the kept handler's trampoline
/// arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_register_press(
    handle: i64,
    entry: ComputeEntry,
    state: *const u8,
    bytes: usize,
    frame_at: i64,
    body: Release,
) {
    // SAFETY: forwarded to the caller's promise.
    let node =
        unsafe { crate::ui::buri_rt_ui_node_register_handler(entry, state, bytes, frame_at, body) };
    with_doc(handle, |doc| {
        let open = open_element(doc);
        if let Some(r) = doc.records.get_mut(open) {
            r.press = node;
        }
    });
}

/// `registerOutside(builder, handler)` — keeps an `onPressOutside`'s handler on
/// the document, paired with the open element, so a press landing outside that
/// element's subtree fires it. The handler lives on a graph node disposed with
/// the subtree, so a pair whose subtree has gone fires nothing (its node is
/// disposed and [`crate::ui::fire`] is a no-op on one).
///
/// # Safety
/// `entry`/`state`/`bytes`/`frame_at`/`body` are the kept handler's trampoline
/// arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_register_outside(
    handle: i64,
    entry: ComputeEntry,
    state: *const u8,
    bytes: usize,
    frame_at: i64,
    body: Release,
) {
    // SAFETY: forwarded to the caller's promise.
    let node =
        unsafe { crate::ui::buri_rt_ui_node_register_handler(entry, state, bytes, frame_at, body) };
    with_doc(handle, |doc| {
        let open = open_element(doc);
        doc.outside.push((node, open));
    });
}

/// `registerValue(builder, signal)` — stores the signal a field's or a toggle's
/// value is bound to on the open element, so `fill` writes it a string and
/// `flip` its negation.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_register_value(handle: i64, signal: i64) {
    with_doc(handle, |doc| {
        let open = open_element(doc);
        if let Some(r) = doc.records.get_mut(open) {
            r.value_signal = signal;
        }
    });
}

/// `registerLabel(builder, label)` — keeps a control's accessible name on the
/// open element, so `press` addresses a button by the label a reader hears
/// rather than its glyphs.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_register_label(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(ptr, len) };
    with_doc(handle, |doc| {
        let open = open_element(doc);
        if let Some(r) = doc.records.get_mut(open) {
            r.label = label;
        }
    });
}

/// `registerFollow(builder, onFollow)` — keeps a route link's plain-click
/// handler on the open anchor, its destination already captured, so `follow`
/// fires it. It is `registerPress`'s kept handler under a different name and a
/// different slot, so `follow` fires it and `press` never does.
///
/// # Safety
/// `entry`/`state`/`bytes`/`frame_at`/`body` are the kept handler's trampoline
/// arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_register_follow(
    handle: i64,
    _dest_base: *mut u8,
    _dest_ptr: *const u8,
    _dest_len: u64,
    entry: ComputeEntry,
    state: *const u8,
    bytes: usize,
    frame_at: i64,
    body: Release,
) {
    // The handler is kept on a graph node so the subtree's disposal takes it;
    // the destination it would be fired with is the JavaScript reader's to
    // carry — this backend never follows a link (`web/document.buri` is
    // JS-only), so it keeps the handler and nothing fires it.
    // SAFETY: forwarded to the caller's promise.
    let node =
        unsafe { crate::ui::buri_rt_ui_node_register_handler(entry, state, bytes, frame_at, body) };
    with_doc(handle, |doc| {
        let open = open_element(doc);
        if let Some(r) = doc.records.get_mut(open) {
            r.follow = node;
        }
    });
}

/// `markSubmit(builder)` — flags the open button as the one whose press submits
/// its form, the native `type="submit"`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_mark_submit(handle: i64) {
    with_doc(handle, |doc| {
        let open = open_element(doc);
        if let Some(r) = doc.records.get_mut(open) {
            r.submit = true;
        }
    });
}

impl Document {
    /// The text runs **directly** under `node`, concatenated — a control's own
    /// label. It does not descend into a nested control, so a field label's text
    /// is not joined with the value run inside its input.
    fn direct_text(&self, node: usize) -> String {
        let mut out = String::new();
        for &child in &self.records[node].children {
            if self.records[child].kind == Kind::Text {
                out.push_str(&self.records[child].text);
            }
        }
        out
    }

    /// The first element of `name`, in document order, whose own label is
    /// `label` — the native `$tree_labelled`.
    fn labelled(&self, name: &str, label: &str) -> Option<usize> {
        self.ordered().into_iter().find_map(|(i, _)| {
            let r = &self.records[i];
            // A control's accessible name is its stored label where it has one —
            // a button's glyphs are not its name — and its own text otherwise.
            let named = if r.label.is_empty() { self.direct_text(i) } else { r.label.clone() };
            (r.kind == Kind::Element && r.name == name && named == label).then_some(i)
        })
    }

    /// The first descendant of `node` (itself included), in document order,
    /// whose name is one of `names` — the native `$dom_first`.
    fn first_named(&self, node: usize, names: &[&str]) -> Option<usize> {
        let r = &self.records[node];
        if r.kind == Kind::Element && names.contains(&r.name.as_str()) {
            return Some(node);
        }
        for &child in &r.children {
            if let Some(found) = self.first_named(child, names) {
                return Some(found);
            }
        }
        None
    }

    /// Whether `node` is `ancestor` itself or a descendant of it — a press
    /// inside the subtree `onPressOutside` watches is not a press outside it.
    fn within(&self, ancestor: usize, node: usize) -> bool {
        let mut at = Some(node);
        while let Some(i) = at {
            if i == ancestor {
                return true;
            }
            at = self.records[i].parent;
        }
        false
    }

    /// The nearest element of `name` at or above `node` — the native
    /// `$dom_enclosing`.
    fn enclosing(&self, node: usize, name: &str) -> Option<usize> {
        let mut at = Some(node);
        while let Some(i) = at {
            let r = &self.records[i];
            if r.kind == Kind::Element && r.name == name {
                return Some(i);
            }
            at = r.parent;
        }
        None
    }

    /// Every element of `name` reachable from the host, in document order.
    fn elements(&self, name: &str) -> Vec<usize> {
        self.ordered()
            .into_iter()
            .filter(|(i, _)| self.records[*i].kind == Kind::Element && self.records[*i].name == name)
            .map(|(i, _)| i)
            .collect()
    }

    /// Every element of `name` at or under `node`, in document order.
    fn descendants(&self, node: usize, name: &str) -> Vec<usize> {
        let mut out = Vec::new();
        self.gather(node, name, &mut out);
        out
    }

    fn gather(&self, node: usize, name: &str, out: &mut Vec<usize>) {
        let r = &self.records[node];
        if r.kind == Kind::Element && r.name == name {
            out.push(node);
        }
        for &child in &r.children {
            self.gather(child, name, out);
        }
    }

    /// Whether the pointer reaches `node` — the native `$dom_reachable`.
    /// `pointer-events` inherits, so the nearest ancestor that declares one
    /// answers, read from the element's classes or its inline declarations.
    fn reachable(&self, node: usize) -> bool {
        let mut at = Some(node);
        while let Some(i) = at {
            if let Some(reaches) = self.pointer_events(i) {
                return reaches;
            }
            at = self.records[i].parent;
        }
        true
    }

    /// What this element declares for `pointer-events`, or `None` — an inline
    /// declaration first, then a `pass-<value>` class, which is the compiler's
    /// atomic name for the property (`semantics/styles.rs`), read as the resolved
    /// declaration the design calls for without the extracted sheet phase 5
    /// hands over.
    fn pointer_events(&self, node: usize) -> Option<bool> {
        let body = &self.records[node].body;
        if let Some(v) = decl_value(body, "pointer-events") {
            return Some(v != "none");
        }
        for class in classes_of(body) {
            if let Some(v) = class.strip_prefix("pass-") {
                return Some(v != "none");
            }
        }
        None
    }

    /// Whether a dialog has taken `node` out of the page — the native
    /// `$dom_inert`. A dialog is rendered only while open here, so a `dialog` in
    /// the tree is an open one: what is inside one is reached, and what is
    /// outside every one is inert while any is present.
    fn inert(&self, node: usize) -> bool {
        let mut at = Some(node);
        while let Some(i) = at {
            let r = &self.records[i];
            if r.kind == Kind::Element && r.name == "dialog" {
                return false;
            }
            at = r.parent;
        }
        !self.elements("dialog").is_empty()
    }
}

/// The value of a `prop:value` declaration in a scene `e`-line body, or `None`.
fn decl_value(body: &str, prop: &str) -> Option<String> {
    for token in body.split(';') {
        if let Some((p, v)) = token.split_once(':') {
            if p == prop {
                return Some(v.to_owned());
            }
        }
    }
    None
}

/// The class names a body's `class:<names>` token carries.
fn classes_of(body: &str) -> Vec<String> {
    for token in body.split(';') {
        if let Some(names) = token.strip_prefix("class:") {
            return names.split(' ').filter(|s| !s.is_empty()).map(|s| s.to_owned()).collect();
        }
    }
    Vec::new()
}

/// Whether a control's flag is set — the scene's `disabled:true`.
fn is_disabled(body: &str) -> bool {
    decl_value(body, "disabled").as_deref() == Some("true")
}

/// Whether a field blocks implicit submission — the HTML Standard's list of the
/// kinds a reader types a line into, read from the scene's `field:<kind>`.
fn blocks_submission(body: &str) -> bool {
    matches!(
        decl_value(body, "field").as_deref(),
        Some("text" | "password" | "email" | "number" | "search")
    )
}

/// `Rendered.press(label)` — resolve the button by label, and if it is reached
/// and not inert, fire its handler and, for a submit button, its form's.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_rendered_press(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(ptr, len) };
    let Some(button) = with_doc(handle, |doc| doc.labelled("button", &label)).flatten() else {
        crate::abort::die(&[b"this tree has no button labelled \"", label.as_bytes(), b"\""])
    };
    let reachable =
        with_doc(handle, |doc| doc.reachable(button) && !doc.inert(button)).unwrap_or(false);
    if !reachable {
        return;
    }
    // The press reaches the document before the button, the way a browser's
    // `pointerdown` does: an overlay watching for a press outside itself sees
    // this one and fires on it, unless the press landed inside its subtree. A
    // handler whose subtree has gone is a disposed node and fires nothing.
    let outside: Vec<i64> = with_doc(handle, |doc| {
        doc.outside
            .iter()
            .filter(|(_, elem)| !doc.within(*elem, button))
            .map(|(node, _)| *node)
            .collect()
    })
    .unwrap_or_default();
    for node in outside {
        crate::ui::fire(node);
    }
    let (press, submit, disabled) = with_doc(handle, |doc| {
        let r = &doc.records[button];
        (r.press, r.submit, is_disabled(&r.body))
    })
    .unwrap_or((-1, false, true));
    if disabled {
        return;
    }
    if press >= 0 {
        crate::ui::fire(press);
    }
    // A submit button has no handler of its own: reaching the form is the
    // browser's default action for the press.
    if submit {
        let form = with_doc(handle, |doc| doc.enclosing(button, "form")).flatten();
        if let Some(form) = form {
            let node = with_doc(handle, |doc| doc.records[form].press).unwrap_or(-1);
            if node >= 0 {
                crate::ui::fire(node);
            }
        }
    }
}

/// `Rendered.fill(label, value)` — write `value` to the bound signal of the
/// field the label names, unless it is disabled or inert.
///
/// # Safety
/// `label` and `value` are readable UTF-8 ranges, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_rendered_fill(
    handle: i64,
    _lbase: *mut u8,
    lptr: *const u8,
    llen: u64,
    _vbase: *mut u8,
    vptr: *const u8,
    vlen: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(lptr, llen) };
    // SAFETY: forwarded to the caller's promise.
    let value = unsafe { text_of(vptr, vlen) };
    let field = with_doc(handle, |doc| {
        doc.labelled("label", &label).and_then(|l| doc.first_named(l, &["input", "textarea"]))
    })
    .flatten();
    let Some(field) = field else {
        crate::abort::die(&[b"the label \"", label.as_bytes(), b"\" is not a field"])
    };
    let (disabled, inert, signal) = with_doc(handle, |doc| {
        (is_disabled(&doc.records[field].body), doc.inert(field), doc.records[field].value_signal)
    })
    .unwrap_or((true, true, -1));
    if disabled || inert {
        return;
    }
    if signal >= 0 {
        // One update transaction, as the JavaScript `input` listener's
        // `$ui_flush` is: a write and the cascade it wakes are one pass.
        crate::ui::buri_rt_ui_flush_begin();
        crate::ui::set_str_signal(signal, &value);
        crate::ui::buri_rt_ui_flush_end();
    }
}

/// `Rendered.flip(label)` — flip the bound signal of the toggle the label names,
/// unless it is disabled or inert.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_rendered_flip(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(ptr, len) };
    let toggle = with_doc(handle, |doc| {
        doc.labelled("label", &label).and_then(|l| doc.first_named(l, &["input"]))
    })
    .flatten();
    let Some(toggle) = toggle else {
        crate::abort::die(&[b"the label \"", label.as_bytes(), b"\" is not a toggle"])
    };
    let (disabled, inert, signal) = with_doc(handle, |doc| {
        (is_disabled(&doc.records[toggle].body), doc.inert(toggle), doc.records[toggle].value_signal)
    })
    .unwrap_or((true, true, -1));
    if disabled || inert {
        return;
    }
    if signal >= 0 {
        // One update transaction, as the JavaScript `change` listener's is.
        crate::ui::buri_rt_ui_flush_begin();
        crate::ui::flip_bool_signal(signal);
        crate::ui::buri_rt_ui_flush_end();
    }
}

/// `Rendered.submit(index)` — submit the `index`th form under the implicit-
/// submission rule: an enabled submit button submits it, and a form with none is
/// submitted only while exactly one of its fields blocks implicit submission.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_testing_rendered_submit(handle: i64, index: i64) {
    let forms = with_doc(handle, |doc| doc.elements("form")).unwrap_or_default();
    let form = usize::try_from(index).ok().and_then(|i| forms.get(i).copied());
    let Some(form) = form else {
        crate::abort::die(&[b"this tree has no form ", index.to_string().as_bytes()])
    };
    if with_doc(handle, |doc| doc.inert(form)).unwrap_or(true) {
        return;
    }
    let (has_submit, blocking, node) = with_doc(handle, |doc| {
        let has_submit = doc
            .descendants(form, "button")
            .into_iter()
            .any(|b| doc.records[b].submit && !is_disabled(&doc.records[b].body));
        let blocking = doc
            .descendants(form, "input")
            .into_iter()
            .filter(|i| blocks_submission(&doc.records[*i].body))
            .count();
        (has_submit, blocking, doc.records[form].press)
    })
    .unwrap_or((false, 0, -1));
    if (has_submit || blocking == 1) && node >= 0 {
        crate::ui::fire(node);
    }
}

/// A `[Str]` argument's strings — the native list is one block of [`BuriStr`]
/// elements at their own stride, `len` of them.
///
/// # Safety
/// `ptr` points at `len` `BuriStr` elements, or is null with a zero `len`.
unsafe fn keys_of(ptr: *const u8, len: u64) -> Vec<String> {
    let n = len as usize;
    if ptr.is_null() || n == 0 {
        return Vec::new();
    }
    let stride = std::mem::size_of::<BuriStr>();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // SAFETY: the caller promises `n` `BuriStr` elements at `ptr`.
        let elem = unsafe { &*(ptr.add(i * stride).cast::<BuriStr>()) };
        // SAFETY: a live `Str` view built by generated code is valid UTF-8.
        out.push(unsafe { elem.as_str() }.into_owned());
    }
    out
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
