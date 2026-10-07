//! The element document `platform/effect/testing`'s `render` builds, the readers a
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
#![expect(
    clippy::indexing_slicing,
    reason = "every record index here was answered by `add` or read out of a record's \
              `children` or `parent`, and the record arena only ever grows"
)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here is tree depths, child positions, and counts of the records, \
              regions and identities a document already holds, plus element offsets into a list \
              the caller holds"
)]

use crate::list::{Release, Retain};
use crate::ui::ComputeEntry;
use crate::value::{list_of_bytes, str_of, BuriList, BuriStr, BURI_RT_STR_LEN_MASK};
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
    /// The signal a field's caret/selection is bound to, or `-1`. `select` writes
    /// it an `(anchor, focus)` pair — never read while the scene is built, since
    /// a caret is not in the paint.
    selection_signal: i64,
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
    /// A file picker's handler graph node, or `-1`. `pickFile` fires it and
    /// `press` never does.
    pick: i64,
    /// The pointer handler graph nodes, down/move/up, each `-1` when unset.
    pointer: [i64; 3],
    /// Whether this element is announced as the current page — the
    /// JavaScript `aria-current`. `isCurrent` reads it.
    current: bool,
    /// Whether this element and everything in it is hidden from assistive
    /// technology — the JavaScript `aria-hidden`. `spoken` skips it.
    decorative: bool,
    /// A tooltip wrapper's shown cell, which the bubble's region reads, or `-1`
    /// for every other element.
    tip: i64,
    /// What a reader hears with a tooltip wrapper's trigger — the JavaScript
    /// `aria-describedby`'s text.
    tip_text: String,
    /// Whether Escape hid this tooltip while it showed. It stays hidden until
    /// the pointer and the focus have both left it.
    tip_dismissed: bool,
    /// An `onKey`'s handler graph node, or `-1`. `key` fires it.
    key: i64,
    /// The focus this element stands for, an index into the document's
    /// `stops`, or `-1` for an element the focus never lands on.
    focus_key: i64,
}

/// One element the focus can land on, minted once by `focusKey` and handed
/// to whichever record draws the element now, so a widget that rebuilds its
/// record keeps the focus. The JavaScript twin is a scene document's `stops`.
struct Stop {
    /// The record that draws the element now, or `None` before the first.
    record: Option<usize>,
    /// A control or a link: focusable, and in the order unless it says not.
    natural: bool,
    /// What `isInFocusOrder` last said, or `None` for the element's default.
    order: Option<bool>,
    /// The `hasFocus` signal, or `-1`.
    signal: i64,
    /// Whether the signal is a group's, true while the focus is inside it.
    within: bool,
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
        selection_signal: -1,
        submit: false,
        label: String::new(),
        follow: -1,
        pick: -1,
        pointer: [-1; 3],
        current: false,
        decorative: false,
        tip: -1,
        tip_text: String::new(),
        tip_dismissed: false,
        key: -1,
        focus_key: -1,
    }
}

/// The key `key` is dispatching, read back by each handler it fires, and
/// whether one of them claimed it.
#[derive(Default)]
struct KeyPress {
    name: String,
    claimed: bool,
}

/// The file `pickFile` last offered a document: its name, the type a browser
/// would report, and its bytes.
#[derive(Default)]
struct Offer {
    name: String,
    mime_type: String,
    content: Vec<u8>,
}

/// Where the pointer is for the handler being fired, and the key of the row
/// under it, read back into its `PointerAt`.
#[derive(Default)]
struct Pointer {
    x: f64,
    y: f64,
    row: Option<String>,
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
    /// The file `pickFile` last offered, which a picker's handler reads back.
    offer: Offer,
    /// The element holding the pointer since a `pointerDown`.
    capture: Option<usize>,
    /// What the pointer handler being fired reads back.
    pointer: Pointer,
    /// The element the pointer is over: the last one a pointer method named.
    hovered: Option<usize>,
    /// The stop that has the focus, an index into `stops`.
    focused: Option<usize>,
    /// Every element the focus can land on, by the key `focusKey` minted.
    stops: Vec<Stop>,
    /// What the key handler being fired reads back.
    key: KeyPress,
    /// The context `render` was called with, handed to every walk and handler
    /// this document drives. `None` for one rendered with no context bytes.
    ctx: Option<Supplied>,
}

/// The context a document supplies: the bytes of the one it was mounted with,
/// holding a count of their own until exit, and where in a walk's or a
/// handler's record they go. JavaScript keeps the same thing as a scene
/// document's `ctx`.
struct Supplied {
    /// The context's bytes, in words so the glue reads aligned pointers.
    words: Vec<u64>,
    /// How many of those bytes are the context.
    bytes: usize,
    /// The offset of the context in every record this document drives. The
    /// mount's record says where, and every other record is the same backend's.
    at: usize,
    retain: Retain,
    release: Release,
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
            offer: Offer::default(),
            capture: None,
            pointer: Pointer::default(),
            hovered: None,
            focused: None,
            stops: Vec::new(),
            key: KeyPress::default(),
            ctx: None,
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

/// `render(ctx, root)` on the native backend — the mount `render`'s thin body
/// reaches: open a document, walk `root` into it with the `renderInto` closure
/// the caller passed, and answer the handle a `Rendered` carries.
///
/// The context crosses inside the walk's record, at `ctx_at`, `ctx_bytes` long.
/// The document keeps a copy of it, retained through `ctx_retain`, and hands it
/// to every walk and handler it drives ([`supply`]): a region a watcher
/// rebuilds, a row a reconcile builds and a press each get the context `render`
/// was called with, as they do in JavaScript. `ctx_release` gives the count back
/// at exit. `root` crosses by reference, a pointer to the one `Node` the walk
/// destructures and this side never reads; and the walk is the closure `render`
/// handed over, invoked once through the same trampoline
/// [`crate::ui::buri_rt_ui_render_walk`] is. The initial render runs each leaf
/// watcher and each region watcher once through that walk, so what comes back
/// is already the resting tree.
///
/// # Safety
/// `root` points at one whole `Node`; `entry` is the thunk the backend
/// generated for the walk and `state` the record it was generated against;
/// `frame_at` is an offset inside the record or negative; and `ctx_bytes` is
/// zero, or the `ctx_bytes` bytes at `ctx_at` in the record are one whole
/// context of the type `ctx_retain` and `ctx_release` were generated for.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_host_testing_mount(
    root: *const u8,
    entry: crate::ui::ComputeEntry,
    state: *mut u8,
    frame_at: i64,
    ctx_at: u64,
    ctx_bytes: u64,
    ctx_retain: Retain,
    ctx_release: Release,
) -> i64 {
    let handle = open();
    let at = usize::try_from(ctx_at).unwrap_or(0);
    let bytes = usize::try_from(ctx_bytes).unwrap_or(0);
    if bytes > 0 {
        let mut words = vec![0u64; bytes.div_ceil(8)];
        // SAFETY: the caller promises `bytes` readable bytes at `at` in the
        // record, and `words` has at least that many.
        unsafe {
            std::ptr::copy_nonoverlapping(state.add(at), words.as_mut_ptr().cast::<u8>(), bytes);
        }
        if let Some(f) = ctx_retain {
            // SAFETY: `words` holds one whole context of the glue's type.
            unsafe { f(words.as_mut_ptr().cast()) };
        }
        give_back_at_exit();
        with_doc(handle, |doc| {
            doc.ctx = Some(Supplied { words, bytes, at, retain: ctx_retain, release: ctx_release });
        });
    }
    // SAFETY: `state` is the record the document's context was read out of.
    unsafe { supply(handle, state) };
    // SAFETY: forwarded to the caller's promise; `handle` is a live document.
    unsafe { crate::ui::buri_rt_ui_render_walk(entry, state, handle, root, frame_at) };
    handle
}

/// Writes document `handle`'s context into `state`, the record of a walk or a
/// handler it is about to drive, with a count of its own: the thunk hands the
/// context to a Buri function that owns it. Nothing for a document with none.
///
/// # Safety
/// `state` is a record the same backend built as the mount's, with room for the
/// context at the document's offset.
pub(crate) unsafe fn supply(handle: i64, state: *mut u8) {
    let Some(Some((words, bytes, at, retain))) = with_doc(handle, |doc| {
        doc.ctx.as_ref().map(|c| (c.words.clone(), c.bytes, c.at, c.retain))
    }) else {
        return;
    };
    // SAFETY: the caller promises room for `bytes` bytes at `at`.
    let to = unsafe { state.add(at) };
    // SAFETY: `words` has at least `bytes` bytes, and `to` room for them.
    unsafe { std::ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), to, bytes) };
    if let Some(f) = retain {
        // SAFETY: `to` now holds one whole context of the glue's type.
        unsafe { f(to) };
    }
}

/// Registers [`give_back`] after the heap audit, once, so it runs first:
/// `atexit` is last-in-first-out, as `cli/runtime/ui.rs` says.
fn give_back_at_exit() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::memory::arm_heap_audit();
        // SAFETY: `give_back` is an `extern "C" fn()` taking no arguments and
        // returning normally, which is the whole of `atexit`'s contract.
        unsafe { atexit(give_back) };
    });
}

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

/// Every document's context, given back. `try_lock` for `ui.rs`'s reason: an
/// abort can exit while this lock is held, and then the audit is quiet anyway.
extern "C" fn give_back() {
    let Ok(mut all) = DOCUMENTS.try_lock() else { return };
    for doc in all.iter_mut() {
        let Some(mut c) = doc.ctx.take() else { continue };
        if let Some(f) = c.release {
            // SAFETY: `words` holds one whole context of the glue's type, and
            // nothing names it after this.
            unsafe { f(c.words.as_mut_ptr().cast()) };
        }
    }
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
    // SAFETY: `state` is the walk's record, built as the mount's was.
    unsafe { supply(handle, state) };
    // SAFETY: forwarded to the caller's promise; `handle` is a live document,
    // its builder now pointed at the region gap.
    unsafe { crate::ui::buri_rt_ui_render_walk(entry, state, handle, node, frame_at) };
    with_doc(handle, |doc| doc.end_region());
    check_focus(handle);
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
/// the frame `begin_region` pushed. A widget rebuilt in place has handed its
/// focus to the record it drew, so the focus is lost only when what had it
/// went for good.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_end_region(handle: i64) {
    with_doc(handle, |doc| doc.end_region());
    check_focus(handle);
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
        doc.each_regions.push(EachRegion { end, parent, list_owner, rows: Vec::new() });
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

    for (i, key) in keys.iter().enumerate().rev() {
        if let Some(row) = by_key.remove(key) {
            let (start, end) = (row.start, row.end);
            with_doc(handle, |doc| doc.move_block(parent, start, end, anchor));
            anchor = start;
            if let Some(slot) = next.get_mut(i) {
                *slot = row;
            }
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
            // SAFETY: `build_state` is the row body's record, built as the
            // mount's was.
            unsafe { supply(handle, build_state) };
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
            if let Some(slot) = next.get_mut(i) {
                *slot = RowRec { key: key.clone(), start: row_start, end: row_end, owner: row_owner };
            }
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
    // A row that left with the focus took it with it.
    check_focus(handle);
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
// native `$host_testing_Rendered_*`.

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

/// `registerSelection(builder, signal)` — stores the signal a field's
/// caret/selection is bound to on the open element, so `select` writes it an
/// `(anchor, focus)` pair.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_register_selection(handle: i64, signal: i64) {
    with_doc(handle, |doc| {
        let open = open_element(doc);
        if let Some(r) = doc.records.get_mut(open) {
            r.selection_signal = signal;
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

/// `markCurrent(builder, at, on)` — says whether the element `openElement`
/// answered `at` is announced as the current page.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_mark_current(handle: i64, at: i64, on: u8) {
    with_doc(handle, |doc| {
        if let Some(r) = usize::try_from(at).ok().and_then(|i| doc.records.get_mut(i)) {
            r.current = on != 0;
        }
    });
}

/// `markDecorative(builder)` — hides the open element and everything in it from
/// assistive technology.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_mark_decorative(handle: i64) {
    with_doc(handle, |doc| {
        let open = open_element(doc);
        if let Some(r) = doc.records.get_mut(open) {
            r.decorative = true;
        }
    });
}

/// `openTip(builder)` — makes the open element a tooltip's wrapper and answers
/// the `Bool` cell [`sync_tips`] writes. The cell belongs to whatever is
/// running, so it goes with the subtree it was rendered in.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_open_tip(handle: i64) -> i64 {
    let cell = crate::ui::new_bool_signal(false);
    with_doc(handle, |doc| {
        let open = open_element(doc);
        if let Some(r) = doc.records.get_mut(open) {
            r.tip = cell;
        }
    });
    cell
}

/// `describeTip(builder, at, text)` — keeps the description a reader hears with
/// the tooltip wrapper `at`.
///
/// # Safety
/// `text` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_describe_tip(
    handle: i64,
    at: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let text = unsafe { text_of(ptr, len) };
    with_doc(handle, |doc| {
        if let Some(r) = usize::try_from(at).ok().and_then(|i| doc.records.get_mut(i)) {
            r.tip_text = text;
        }
    });
}

/// `registerKey(builder, handler)` — keeps an `onKey`'s handler on a graph node
/// and stores it on the open element, for `key` to fire.
///
/// # Safety
/// `entry`/`state`/`bytes`/`frame_at`/`body` are the kept handler's trampoline
/// arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_register_key(
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
            r.key = node;
        }
    });
}

/// `keyPressed(builder)` — the key the dispatch in flight carries.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_key_pressed(handle: i64, out: *mut BuriStr) {
    let name = with_doc(handle, |doc| doc.key.name.clone()).unwrap_or_default();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&name)) };
}

/// `claimKey(builder)` — the handler being fired claimed its key.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_claim_key(handle: i64) {
    with_doc(handle, |doc| doc.key.claimed = true);
}

/// `registerPick(builder, onPick)` — keeps a file picker's handler on a graph
/// node and stores it on the open element, in a slot `press` never reads, so
/// only `pickFile` fires it.
///
/// # Safety
/// `entry`/`state`/`bytes`/`frame_at`/`body` are the kept handler's trampoline
/// arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_register_pick(
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
            r.pick = node;
        }
    });
}

/// `offeredName(builder)` — the name of the file `pickFile` last offered.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_offered_name(handle: i64, out: *mut BuriStr) {
    let name = with_doc(handle, |doc| doc.offer.name.clone()).unwrap_or_default();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&name)) };
}

/// `offeredType(builder)` — the type of the file `pickFile` last offered.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_offered_type(handle: i64, out: *mut BuriStr) {
    let mime_type = with_doc(handle, |doc| doc.offer.mime_type.clone()).unwrap_or_default();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&mime_type)) };
}

/// `offeredBytes(builder)` — the bytes of the file `pickFile` last offered.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_offered_bytes(handle: i64, out: *mut BuriList) {
    let content = with_doc(handle, |doc| doc.offer.content.clone()).unwrap_or_default();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(list_of_bytes(&content)) };
}

/// `registerPointer(builder, phase, handler)` — keeps a pointer handler on a
/// graph node and stores it on the open element under `phase` (0 down, 1 move,
/// 2 up), for the pointer dispatch to fire.
///
/// # Safety
/// `entry`/`state`/`bytes`/`frame_at`/`body` are the kept handler's trampoline
/// arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_register_pointer(
    handle: i64,
    phase: i64,
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
        let slot = usize::try_from(phase).ok();
        if let Some(handler) = slot.and_then(|s| doc.records.get_mut(open)?.pointer.get_mut(s)) {
            *handler = node;
        }
    });
}

/// `pointerX(builder)` — the x the pointer dispatch in flight carries.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_pointer_x(handle: i64) -> f64 {
    with_doc(handle, |doc| doc.pointer.x).unwrap_or(0.0)
}

/// `pointerY(builder)` — the y the pointer dispatch in flight carries.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_pointer_y(handle: i64) -> f64 {
    with_doc(handle, |doc| doc.pointer.y).unwrap_or(0.0)
}

/// `pointerOverRow(builder)` — whether the dispatch in flight found a row under
/// the pointer for the element it is firing at.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_pointer_over_row(handle: i64) -> u8 {
    u8::from(with_doc(handle, |doc| doc.pointer.row.is_some()).unwrap_or(false))
}

/// `pointerRow(builder)` — that row's key, or `""`.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_pointer_row(handle: i64, out: *mut BuriStr) {
    let row = with_doc(handle, |doc| doc.pointer.row.clone()).flatten().unwrap_or_default();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&row)) };
}

impl Document {
    /// The accessible name a reader hears for `node`: every run of text in its
    /// subtree, in document order, joined by a space — the same joining
    /// [`text()`](buri_rt_host_testing_rendered_text) does for a whole tree, and
    /// what an anchor's name is, so a link wrapping many runs is addressed by
    /// the words it shows and not by the runs run together. It does not descend
    /// into a nested control, so a field label's name is its own text and not
    /// the value run inside its input.
    fn accessible_name(&self, node: usize) -> String {
        let mut runs: Vec<&str> = Vec::new();
        self.name_runs(node, true, &mut runs);
        runs.join(" ")
    }

    /// Gathers the text runs of `node`'s subtree into `out`, skipping the subtree
    /// of any nested control. `root` is the node the name is computed for, whose
    /// own control-ness never stops the walk.
    fn name_runs<'a>(&'a self, node: usize, root: bool, out: &mut Vec<&'a str>) {
        let r = &self.records[node];
        if !root && r.kind == Kind::Element && is_control(&r.name) {
            return;
        }
        if r.kind == Kind::Text {
            out.push(r.text.as_str());
        }
        for &child in &r.children {
            self.name_runs(child, false, out);
        }
    }

    /// The first element of `name`, in document order, whose own label is
    /// `label` — the native `$tree_labelled`.
    fn labelled(&self, name: &str, label: &str) -> Option<usize> {
        self.ordered().into_iter().find_map(|(i, _)| {
            let r = &self.records[i];
            // A control's accessible name is its stored label where it has one —
            // a button's glyphs are not its name — and the text of its
            // descendants otherwise.
            let named = if r.label.is_empty() { self.accessible_name(i) } else { r.label.clone() };
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

    /// The innermost element whose name — its label, or else its text — is
    /// `label`: the first in document order, then down through any child
    /// element with the same name. The native `$scene_named`.
    fn named(&self, label: &str) -> Option<usize> {
        let matches = |i: usize| {
            let r = &self.records[i];
            r.kind == Kind::Element
                && (if r.label.is_empty() { self.accessible_name(i) } else { r.label.clone() })
                    == label
        };
        let mut found = self.ordered().into_iter().map(|(i, _)| i).find(|&i| matches(i))?;
        while let Some(&inner) = self.records[found].children.iter().find(|&&c| matches(c)) {
            found = inner;
        }
        Some(found)
    }

    /// The innermost keyed row holding `node`, as `(list, key, start marker)`:
    /// at each level up, the row of an `each` under that parent whose markers
    /// stand either side of the node. The native `$scene_rowOf`.
    fn row_of(&self, node: usize) -> Option<(usize, String, usize)> {
        let mut at = node;
        loop {
            let parent = self.records[at].parent?;
            let children = &self.records[parent].children;
            let index = |n: usize| children.iter().position(|&c| c == n);
            let pos = index(at)?;
            let mut best: Option<(usize, (usize, String, usize))> = None;
            for (list, region) in self.each_regions.iter().enumerate() {
                if region.parent != parent {
                    continue;
                }
                for row in &region.rows {
                    let (Some(s), Some(e)) = (index(row.start), index(row.end)) else { continue };
                    if s < pos && pos < e && best.as_ref().is_none_or(|(b, _)| s > *b) {
                        best = Some((s, (list, row.key.clone(), row.start)));
                    }
                }
            }
            if let Some((_, row)) = best {
                return Some(row);
            }
            at = parent;
        }
    }

    /// The key of the row under the pointer, `over`, in the list the element
    /// `at` is a row of. The native `$scene_overRow`.
    fn over_row(&self, at: usize, over: usize) -> Option<String> {
        let (own, _, _) = self.row_of(at)?;
        let mut row = self.row_of(over);
        while let Some((list, key, start)) = row {
            if list == own {
                return Some(key);
            }
            row = self.row_of(start);
        }
        None
    }

    /// The pointer handlers `phase` reaches from `target` out to the root, each
    /// with the row under the pointer as that element sees it — the bubbling
    /// path, taken before any handler runs.
    fn pointer_path(&self, phase: usize, target: usize, over: usize) -> Vec<(i64, Option<String>)> {
        let mut out = Vec::new();
        let mut at = Some(target);
        while let Some(i) = at {
            let handler = self.records[i].pointer.get(phase).copied().unwrap_or(-1);
            if handler >= 0 {
                out.push((handler, self.over_row(i, over)));
            }
            at = self.records[i].parent;
        }
        out
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
        if let Some((p, v)) = token.split_once(':')
            && p == prop
        {
            return Some(v.to_owned());
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

/// Whether `name` is a form control whose own text an accessible-name walk does
/// not fold into an ancestor's name — so a field label's name stays its own text
/// and not the value run inside its input.
fn is_control(name: &str) -> bool {
    matches!(name, "input" | "textarea" | "select" | "button")
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
pub unsafe extern "C" fn buri_rt_host_testing_rendered_press(
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
    // A press is the pointer's, so the button is what the pointer is over, and
    // it takes the focus before its handler runs, the way a browser's does.
    hover(handle, button);
    if let Some(key) = with_doc(handle, |doc| doc.stop_of(button).filter(|&k| doc.focusable(k))).flatten() {
        set_focus(handle, Some(key));
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
        crate::ui::fire(node, handle);
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
        crate::ui::fire(press, handle);
    }
    // A submit button has no handler of its own: reaching the form is the
    // browser's default action for the press.
    if submit {
        let form = with_doc(handle, |doc| doc.enclosing(button, "form")).flatten();
        if let Some(form) = form {
            let node = with_doc(handle, |doc| doc.records[form].press).unwrap_or(-1);
            if node >= 0 {
                crate::ui::fire(node, handle);
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
pub unsafe extern "C" fn buri_rt_host_testing_rendered_fill(
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
    let (disabled, inert, signal, range) = with_doc(handle, |doc| {
        let body = &doc.records[field].body;
        (
            is_disabled(body),
            doc.inert(field),
            doc.records[field].value_signal,
            decl_value(body, "field").as_deref() == Some("range"),
        )
    })
    .unwrap_or((true, true, -1, false));
    if disabled || inert {
        return;
    }
    if signal >= 0 {
        // One update transaction, as the JavaScript `input` listener's
        // `$ui_flush` is: a write and the cascade it wakes are one pass.
        crate::ui::buri_rt_ui_flush_begin();
        if range {
            // A slider binds a `Signal<Float>`, so what a reader types crosses
            // back as a `Float` — the same coercion the browser's `input`
            // listener makes — rather than as the `Str` a text field's signal
            // holds. A value that is not a number reads as `0.0`, the way an
            // empty range control does.
            crate::ui::set_f64_signal(signal, value.trim().parse::<f64>().unwrap_or(0.0));
        } else {
            crate::ui::set_str_signal(signal, &value);
        }
        crate::ui::buri_rt_ui_flush_end();
    }
}

/// `Rendered.select(label, start, end)` — move the caret and selection of the
/// field the label names to the `(anchor, focus)` offsets, unless it is disabled,
/// inert, or keeps no `selection` signal.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_select(
    handle: i64,
    _lbase: *mut u8,
    lptr: *const u8,
    llen: u64,
    start: i64,
    end: i64,
) {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(lptr, llen) };
    let field = with_doc(handle, |doc| {
        doc.labelled("label", &label).and_then(|l| doc.first_named(l, &["input", "textarea"]))
    })
    .flatten();
    let Some(field) = field else {
        crate::abort::die(&[b"the label \"", label.as_bytes(), b"\" is not a field"])
    };
    let (disabled, inert, signal) = with_doc(handle, |doc| {
        (
            is_disabled(&doc.records[field].body),
            doc.inert(field),
            doc.records[field].selection_signal,
        )
    })
    .unwrap_or((true, true, -1));
    if disabled || inert {
        return;
    }
    if signal >= 0 {
        // One update transaction, the way `fill` is.
        crate::ui::buri_rt_ui_flush_begin();
        crate::ui::set_i64_pair_signal(signal, start, end);
        crate::ui::buri_rt_ui_flush_end();
    }
}

/// `Rendered.flip(label)` — flip the bound signal of the toggle the label names,
/// unless it is disabled or inert.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_flip(
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

/// `offerFile(page, name, mimeType, content)` — `pickFile`'s first half: keep
/// the file on the document for the picker's handler to read back.
///
/// # Safety
/// `name` and `mime_type` are readable UTF-8 ranges, and `content` a readable
/// byte range — each may be null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_offer_file(
    handle: i64,
    _nbase: *mut u8,
    nptr: *const u8,
    nlen: u64,
    _tbase: *mut u8,
    tptr: *const u8,
    tlen: u64,
    cptr: *const u8,
    clen: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let name = unsafe { text_of(nptr, nlen) };
    // SAFETY: forwarded to the caller's promise.
    let mime_type = unsafe { text_of(tptr, tlen) };
    let content = if cptr.is_null() || clen == 0 {
        Vec::new()
    } else {
        // SAFETY: the caller promises `clen` readable bytes at `cptr`.
        unsafe { std::slice::from_raw_parts(cptr, clen as usize) }.to_vec()
    };
    with_doc(handle, |doc| doc.offer = Offer { name, mime_type, content });
}

/// `deliverFile(page, label)` — `pickFile`'s second half: fire the handler of
/// the picker the label names, unless it is disabled or out of reach.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_deliver_file(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(ptr, len) };
    let picker = with_doc(handle, |doc| {
        doc.labelled("button", &label).filter(|&b| doc.records[b].pick >= 0)
    })
    .flatten();
    let Some(picker) = picker else {
        crate::abort::die(&[b"this tree has no file picker labelled \"", label.as_bytes(), b"\""])
    };
    let (open, pick) = with_doc(handle, |doc| {
        let r = &doc.records[picker];
        (doc.reachable(picker) && !doc.inert(picker) && !is_disabled(&r.body), r.pick)
    })
    .unwrap_or((false, -1));
    if open {
        crate::ui::fire(pick, handle);
    }
}

/// Fires `phase`'s handlers from `target` out to the root, each reading back
/// where the pointer is and the row under it as its own element sees it.
fn fire_pointer(handle: i64, phase: usize, target: usize, over: usize, x: f64, y: f64) {
    let path = with_doc(handle, |doc| doc.pointer_path(phase, target, over)).unwrap_or_default();
    for (handler, row) in path {
        with_doc(handle, |doc| doc.pointer = Pointer { x, y, row });
        crate::ui::fire(handler, handle);
    }
}

/// The element `label` names, or an abort — what every pointer method starts by.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
unsafe fn pointer_target(handle: i64, ptr: *const u8, len: u64) -> usize {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(ptr, len) };
    match with_doc(handle, |doc| doc.named(&label)).flatten() {
        Some(node) => node,
        None => crate::abort::die(&[b"this tree has no element named \"", label.as_bytes(), b"\""]),
    }
}

/// `Rendered.pointerDown(label, x, y)` — press the element `label` names: an
/// overlay watching for a press outside sees it, the nearest element with a
/// pointer handler captures the pointer, and the press bubbles from the target.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_pointer_down(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    x: f64,
    y: f64,
) {
    // SAFETY: forwarded to the caller's promise.
    let target = unsafe { pointer_target(handle, ptr, len) };
    let reached = with_doc(handle, |doc| doc.reachable(target) && !doc.inert(target)).unwrap_or(false);
    if !reached {
        return;
    }
    hover(handle, target);
    let outside: Vec<i64> = with_doc(handle, |doc| {
        doc.outside.iter().filter(|(_, elem)| !doc.within(*elem, target)).map(|(n, _)| *n).collect()
    })
    .unwrap_or_default();
    for node in outside {
        crate::ui::fire(node, handle);
    }
    with_doc(handle, |doc| {
        let mut at = Some(target);
        doc.capture = None;
        while let Some(i) = at {
            if doc.records[i].pointer.iter().any(|&h| h >= 0) {
                doc.capture = Some(i);
                break;
            }
            at = doc.records[i].parent;
        }
    });
    fire_pointer(handle, 0, target, target, x, y);
}

/// Where a move or a release goes: the element holding the capture while it is
/// still in the tree, and otherwise what the pointer is over, if it is reached.
fn pointer_route(doc: &Document, over: usize) -> Option<usize> {
    match doc.capture {
        Some(held) if doc.within(0, held) => Some(held),
        _ => (doc.reachable(over) && !doc.inert(over)).then_some(over),
    }
}

/// `Rendered.pointerMove(over, x, y)` — move the pointer over the element `over`
/// names; the move reaches the capturing element while a press is held.
///
/// # Safety
/// `over` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_pointer_move(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    x: f64,
    y: f64,
) {
    // SAFETY: forwarded to the caller's promise.
    let over = unsafe { pointer_target(handle, ptr, len) };
    hover(handle, over);
    if let Some(target) = with_doc(handle, |doc| pointer_route(doc, over)).flatten() {
        fire_pointer(handle, 1, target, over, x, y);
    }
}

/// `Rendered.pointerUp(over, x, y)` — release the pointer over the element
/// `over` names, routed as a move is, and end the capture.
///
/// # Safety
/// `over` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_pointer_up(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    x: f64,
    y: f64,
) {
    // SAFETY: forwarded to the caller's promise.
    let over = unsafe { pointer_target(handle, ptr, len) };
    hover(handle, over);
    let target = with_doc(handle, |doc| {
        let target = pointer_route(doc, over);
        doc.capture = None;
        target
    })
    .flatten();
    if let Some(target) = target {
        fire_pointer(handle, 2, target, over, x, y);
    }
}

/// Shows and hides every tooltip after the pointer, the focus or an Escape
/// moved. A tooltip shows while the hovered or the focused element is inside
/// it and Escape hasn't hidden it, and Escape's hiding ends once neither is.
/// A cell that already holds what it should is written nothing, because an
/// equal write is no write.
fn sync_tips(handle: i64) {
    let wanted = with_doc(handle, |doc| {
        let focused = doc.focused_record();
        let tips: Vec<usize> = doc
            .ordered()
            .into_iter()
            .map(|(i, _)| i)
            .filter(|&i| doc.records[i].tip >= 0)
            .collect();
        let mut out = Vec::with_capacity(tips.len());
        for i in tips {
            let over = doc.hovered.is_some_and(|h| doc.within(i, h));
            let inside = focused.is_some_and(|f| doc.within(i, f));
            let r = &mut doc.records[i];
            if !over && !inside {
                r.tip_dismissed = false;
            }
            out.push((r.tip, (over || inside) && !r.tip_dismissed));
        }
        out
    })
    .unwrap_or_default();
    if wanted.is_empty() {
        return;
    }
    crate::ui::buri_rt_ui_flush_begin();
    for (cell, on) in wanted {
        crate::ui::set_bool_signal(cell, on);
    }
    crate::ui::buri_rt_ui_flush_end();
}

/// The pointer is over `node` now, so a tooltip around it shows — unless the
/// pointer passes through `node` or a dialog has taken it out of reach, and
/// then it is over nothing this document can name.
fn hover(handle: i64, node: usize) {
    with_doc(handle, |doc| {
        doc.hovered = (doc.reachable(node) && !doc.inert(node)).then_some(node);
    });
    sync_tips(handle);
}

impl Document {
    /// The record that draws stop `key` now, while it is in the tree.
    fn stop_record(&self, key: usize) -> Option<usize> {
        self.stops.get(key).and_then(|s| s.record).filter(|&r| self.within(0, r))
    }

    /// The record the focus is on, while it is in the tree.
    fn focused_record(&self) -> Option<usize> {
        self.focused.and_then(|k| self.stop_record(k))
    }

    /// The stop that has the focus, while what draws it is in the tree.
    fn focus_now(&self) -> Option<usize> {
        self.focused.filter(|&k| self.stop_record(k).is_some())
    }

    /// Whether a reader can focus stop `key`: an element in the tree, enabled,
    /// not behind an open dialog, and either a control or a link or something
    /// the program gave a focus. A group's signal is the group's, and the
    /// group itself is never where the focus lands.
    fn focusable(&self, key: usize) -> bool {
        let Some(stop) = self.stops.get(key) else { return false };
        let Some(record) = self.stop_record(key) else { return false };
        !stop.within
            && (stop.natural || stop.signal >= 0 || stop.order.is_some())
            && self.records[record].kind == Kind::Element
            && !is_disabled(&self.records[record].body)
            && !self.inert(record)
    }

    /// Whether moving the focus forward or back reaches stop `key`.
    fn in_order(&self, key: usize) -> bool {
        self.focusable(key) && self.stops[key].order.unwrap_or(self.stops[key].natural)
    }

    /// The stop drawn as `record`, if one is.
    fn stop_of(&self, record: usize) -> Option<usize> {
        usize::try_from(self.records[record].focus_key).ok()
    }

    /// The stops in the focus order, in document order.
    fn order(&self) -> Vec<usize> {
        self.ordered()
            .into_iter()
            .filter_map(|(i, _)| self.stop_of(i))
            .filter(|&k| self.in_order(k))
            .collect()
    }

    /// Where `focus(name)` lands: the element `name` names when it takes the
    /// focus, else the first thing inside it that does — a label's field —
    /// else the nearest around it that does — a button's text.
    fn focus_target(&self, node: usize) -> Option<usize> {
        let mut inside = Vec::new();
        self.visit(node, 0, &mut inside);
        std::iter::once(node)
            .chain(inside.into_iter().map(|(i, _)| i))
            .find_map(|i| self.stop_of(i).filter(|&k| self.focusable(k)))
            .or_else(|| {
                let mut at = self.records[node].parent;
                while let Some(i) = at {
                    if let Some(k) = self.stop_of(i).filter(|&k| self.focusable(k)) {
                        return Some(k);
                    }
                    at = self.records[i].parent;
                }
                None
            })
    }

    /// The writes the focus moving from stop `old` to stop `new` makes: the
    /// signal of each, and of every group either is inside. Nothing else's — a
    /// signal the program set and the platform has not got to yet is left for
    /// its own request, which is how the last of several writes wins.
    fn moved(&self, old: Option<usize>, new: Option<usize>) -> Vec<(i64, bool)> {
        let old_at = old.and_then(|k| self.stops.get(k)).and_then(|s| s.record);
        let new_at = new.and_then(|k| self.stops.get(k)).and_then(|s| s.record);
        let mut out = Vec::new();
        for (key, stop) in self.stops.iter().enumerate() {
            if stop.signal < 0 {
                continue;
            }
            if stop.within {
                let Some(group) = stop.record else { continue };
                let had = old_at.is_some_and(|r| self.within(group, r));
                let has = new_at.is_some_and(|r| self.within(group, r));
                if had || has {
                    out.push((stop.signal, has));
                }
            } else if Some(key) == old || Some(key) == new {
                out.push((stop.signal, Some(key) == new));
            }
        }
        out
    }
}

/// Moves the focus to stop `new`, or takes it off the page for `None`, and
/// writes what that changes: the two elements' signals and their groups', and
/// every tooltip. One update transaction.
fn set_focus(handle: i64, new: Option<usize>) {
    let writes = with_doc(handle, |doc| {
        let old = doc.focus_now();
        if old == new {
            return Vec::new();
        }
        doc.focused = new;
        doc.moved(old, new)
    })
    .unwrap_or_default();
    write_focus(&writes);
    sync_tips(handle);
}

fn write_focus(writes: &[(i64, bool)]) {
    if writes.is_empty() {
        return;
    }
    crate::ui::buri_rt_ui_flush_begin();
    for &(signal, on) in writes {
        crate::ui::set_bool_signal(signal, on);
    }
    crate::ui::buri_rt_ui_flush_end();
}

/// After a region or a row went: when what had the focus is gone, the focus
/// is nowhere, and the signal of the element that had it is written `false`,
/// with every group's — the browser's focus fixup, which writes nothing.
pub(crate) fn check_focus(handle: i64) {
    let writes = with_doc(handle, |doc| {
        let lost = doc.focused.filter(|&k| doc.stop_record(k).is_none())?;
        doc.focused = None;
        let mut out = Vec::new();
        if let Some(stop) = doc.stops.get(lost)
            && stop.signal >= 0
            && !stop.within
        {
            out.push((stop.signal, false));
        }
        for stop in &doc.stops {
            if stop.within && stop.signal >= 0 {
                out.push((stop.signal, false));
            }
        }
        Some(out)
    })
    .flatten();
    if let Some(writes) = writes {
        write_focus(&writes);
        sync_tips(handle);
    }
}

/// `focusKey(builder)` — mints a stop, drawn by no record yet.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_focus_key(handle: i64) -> i64 {
    with_doc(handle, |doc| {
        doc.stops.push(Stop { record: None, natural: false, order: None, signal: -1, within: false });
        (doc.stops.len() as i64) - 1
    })
    .unwrap_or(-1)
}

/// `attachFocus(builder, key, natural)` — the open element draws stop `key`.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_attach_focus(handle: i64, key: i64, natural: u8) {
    with_doc(handle, |doc| {
        let open = open_element(doc);
        let Some(stop) = usize::try_from(key).ok().and_then(|k| doc.stops.get_mut(k)) else {
            return;
        };
        stop.record = Some(open);
        stop.natural = natural != 0;
        if let Some(r) = doc.records.get_mut(open) {
            r.focus_key = key;
        }
    });
}

/// `setFocusOrder(builder, key, on)` — puts stop `key` in the order, or takes it
/// out.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_set_focus_order(handle: i64, key: i64, on: u8) {
    with_doc(handle, |doc| {
        if let Some(stop) = usize::try_from(key).ok().and_then(|k| doc.stops.get_mut(k)) {
            stop.order = Some(on != 0);
        }
    });
}

/// `bindFocusSignal(builder, key, signal, within)` — keeps stop `key`'s
/// `hasFocus` signal.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_bind_focus_signal(handle: i64, key: i64, signal: i64, within: u8) {
    with_doc(handle, |doc| {
        if let Some(stop) = usize::try_from(key).ok().and_then(|k| doc.stops.get_mut(k)) {
            stop.signal = signal;
            stop.within = within != 0;
        }
    });
}

/// `requestFocus(builder, key, on)` — the program wrote stop `key`'s signal.
///
/// `true` moves the focus there: to the element, or for a group to its first
/// option in the order, else its first that takes the focus. An element that
/// can't take it — disabled, or behind an open dialog — has `false` written
/// back, so the signal never says a thing has the focus that doesn't. `false`
/// takes the focus away from there, and off the page.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_ui_node_request_focus(handle: i64, key: i64, on: u8) {
    let Ok(key) = usize::try_from(key) else { return };
    enum Asked {
        Nothing,
        Move(Option<usize>),
        Refuse(i64),
    }
    let asked = with_doc(handle, |doc| {
        let Some(stop) = doc.stops.get(key) else { return Asked::Nothing };
        let (within, signal) = (stop.within, stop.signal);
        let focused = doc.focused_record();
        let here = match (within, stop.record) {
            (true, Some(group)) => focused.is_some_and(|f| doc.within(group, f)),
            _ => doc.focus_now() == Some(key),
        };
        if on == 0 {
            return if here { Asked::Move(None) } else { Asked::Nothing };
        }
        if here {
            return Asked::Nothing;
        }
        let target = match (within, doc.stop_record(key)) {
            (true, Some(group)) => {
                let mut inside = Vec::new();
                doc.visit(group, 0, &mut inside);
                let stops: Vec<usize> = inside.iter().filter_map(|&(i, _)| doc.stop_of(i)).collect();
                stops
                    .iter()
                    .copied()
                    .find(|&k| doc.in_order(k))
                    .or_else(|| stops.iter().copied().find(|&k| doc.focusable(k)))
            }
            (false, _) => Some(key).filter(|&k| doc.focusable(k)),
            (true, None) => None,
        };
        match target {
            Some(k) => Asked::Move(Some(k)),
            None => Asked::Refuse(signal),
        }
    })
    .unwrap_or(Asked::Nothing);
    match asked {
        Asked::Nothing => {}
        Asked::Move(to) => set_focus(handle, to),
        Asked::Refuse(signal) => write_focus(&[(signal, false)]),
    }
}

/// `relabel(builder, at, label)` — writes over the accessible name of the
/// element `openElement` answered `at`.
///
/// # Safety
/// `label` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_node_relabel(
    handle: i64,
    at: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(ptr, len) };
    with_doc(handle, |doc| {
        if let Some(r) = usize::try_from(at).ok().and_then(|i| doc.records.get_mut(i)) {
            r.label = label;
        }
    });
}

/// `Rendered.description(name)` — the text of the tooltip around the element
/// `name` names, or `""` for one in none.
///
/// # Safety
/// `name` is a readable UTF-8 range, or null with a zero length; `out` is
/// writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_description(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) {
    // SAFETY: forwarded to the caller's promise.
    let node = unsafe { pointer_target(handle, ptr, len) };
    let text = with_doc(handle, |doc| {
        let mut at = Some(node);
        while let Some(i) = at {
            if doc.records[i].tip >= 0 {
                return doc.records[i].tip_text.clone();
            }
            at = doc.records[i].parent;
        }
        String::new()
    })
    .unwrap_or_default();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&text)) };
}

/// `Rendered.focus(name)` — move the focus to the element `name` names, to the
/// field a label names, or to the control a piece of text is part of.
///
/// # Safety
/// `name` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_focus(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let label = unsafe { text_of(ptr, len) };
    let target = with_doc(handle, |doc| doc.named(&label).and_then(|n| doc.focus_target(n))).flatten();
    let Some(target) = target else {
        crate::abort::die(&[b"this tree has nothing to focus named \"", label.as_bytes(), b"\""])
    };
    set_focus(handle, Some(target));
}

/// `Rendered.focused()` — the name of the element with the focus, or `""`.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_focused(handle: i64, out: *mut BuriStr) {
    let name = with_doc(handle, |doc| {
        let Some(record) = doc.focused_record() else { return String::new() };
        let r = &doc.records[record];
        if !r.label.is_empty() {
            return r.label.clone();
        }
        // A field's control is named by the label around it, as a reader hears it.
        if matches!(r.name.as_str(), "input" | "textarea")
            && let Some(label) = r.parent.and_then(|p| doc.enclosing(p, "label"))
        {
            return doc.accessible_name(label);
        }
        doc.accessible_name(record)
    })
    .unwrap_or_default();
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&name)) };
}

/// Fires every `onKey` from the focused element out to the root with `name`,
/// innermost first, and answers whether one claimed it. The path is taken
/// before any handler runs, the way a browser's is.
fn dispatch_key(handle: i64, name: &str) -> bool {
    let path: Vec<i64> = with_doc(handle, |doc| {
        doc.key = KeyPress { name: name.to_owned(), claimed: false };
        let mut out = Vec::new();
        let mut at = doc.focused_record();
        while let Some(i) = at {
            if doc.records[i].key >= 0 {
                out.push(doc.records[i].key);
            }
            at = doc.records[i].parent;
        }
        out
    })
    .unwrap_or_default();
    for handler in path {
        crate::ui::fire(handler, handle);
    }
    with_doc(handle, |doc| doc.key.claimed).unwrap_or(false)
}

/// Tab and Shift+Tab: `"Tab"` to every `onKey`, and unless one claims it the
/// focus moves to the next stop in the order — or the previous one — wrapping
/// round at the end.
fn tab(handle: i64, forward: bool) {
    if dispatch_key(handle, "Tab") {
        return;
    }
    let next = with_doc(handle, |doc| {
        let order = doc.order();
        if order.is_empty() {
            return None;
        }
        let at = doc.focus_now().and_then(|k| order.iter().position(|&o| o == k));
        let n = order.len();
        let index = match (at, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        order.get(index).copied()
    })
    .flatten();
    if next.is_some() {
        set_focus(handle, next);
    }
}

/// `Rendered.tab()` — Tab.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_rendered_tab(handle: i64) {
    tab(handle, true);
}

/// `Rendered.shiftTab()` — Shift+Tab.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_rendered_shift_tab(handle: i64) {
    tab(handle, false);
}

/// `Rendered.key(key)` — every `onKey` from the focused element out to the
/// root hears `key`, innermost first. An Escape none of them claimed hides the
/// tooltip that is showing, and a Tab is `tab`.
///
/// # Safety
/// `key` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_key(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) {
    // SAFETY: forwarded to the caller's promise.
    let name = unsafe { text_of(ptr, len) };
    if name == "Tab" {
        tab(handle, true);
        return;
    }
    if dispatch_key(handle, &name) || name != "Escape" {
        return;
    }
    with_doc(handle, |doc| {
        let focused = doc.focused_record();
        let tips: Vec<usize> = doc
            .ordered()
            .into_iter()
            .map(|(i, _)| i)
            .filter(|&i| doc.records[i].tip >= 0)
            .collect();
        for i in tips {
            let over = doc.hovered.is_some_and(|h| doc.within(i, h));
            let inside = focused.is_some_and(|f| doc.within(i, f));
            if over || inside {
                doc.records[i].tip_dismissed = true;
            }
        }
    });
    sync_tips(handle);
}

/// `Rendered.submit(index)` — submit the `index`th form under the implicit-
/// submission rule: an enabled submit button submits it, and a form with none is
/// submitted only while exactly one of its fields blocks implicit submission.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_host_testing_rendered_submit(handle: i64, index: i64) {
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
        crate::ui::fire(node, handle);
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
pub unsafe extern "C" fn buri_rt_host_testing_rendered_markup(handle: i64, out: *mut BuriStr) {
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
pub unsafe extern "C" fn buri_rt_host_testing_rendered_text(handle: i64, out: *mut BuriStr) {
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

/// `Rendered.spoken()` — every run of text a screen reader reaches, in order,
/// joined by a space: `text()` without what a decorative element hides.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_spoken(handle: i64, out: *mut BuriStr) {
    let all = documents();
    let text = usize::try_from(handle)
        .ok()
        .and_then(|i| all.get(i))
        .map(|doc| {
            let mut runs: Vec<&str> = Vec::new();
            doc.spoken_runs(0, &mut runs);
            runs.join(" ")
        })
        .unwrap_or_default();
    let answer = str_of(&text);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(answer) };
}

impl Document {
    /// The runs under `node` a screen reader reaches, in document order: every
    /// one, except under an element that is decorative.
    fn spoken_runs<'a>(&'a self, node: usize, out: &mut Vec<&'a str>) {
        for &child in &self.records[node].children {
            let r = &self.records[child];
            match r.kind {
                Kind::Text => out.push(r.text.as_str()),
                Kind::Element if r.decorative => {}
                _ => self.spoken_runs(child, out),
            }
        }
    }
}

/// `Rendered.isCurrent(name)` — whether the element `name` names is announced
/// as the current page.
///
/// # Safety
/// `name` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_is_current(
    handle: i64,
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
) -> u8 {
    // SAFETY: forwarded to the caller's promise.
    let node = unsafe { pointer_target(handle, ptr, len) };
    u8::from(with_doc(handle, |doc| doc.records[node].current).unwrap_or(false))
}

/// `Rendered.count(name)` — how many elements of this name the tree holds.
///
/// The JavaScript `$dom_elements(self, name).length`, and the name is the scene
/// element name the record keeps.
///
/// # Safety
/// `name` is a readable UTF-8 range, or null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_host_testing_rendered_count(
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
pub unsafe extern "C" fn buri_rt_host_testing_rendered_identity(
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
