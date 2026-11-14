//! **Every UI snapshot case, mounted again as a page built for `web`.**
//!
//! A snapshot paints through the native renderer with the `headless()` double,
//! so it never runs the JavaScript a browser runs. A function the JavaScript
//! backend gets wrong breaks only the page: buri-lang/buri#325 printed a row
//! builder `async`, and the page threw while it mounted, with every snapshot
//! still green. The JavaScript backend decides what parks by the effects a
//! program is handed, so a `backends: [JS]` suite holding `headless()` doesn't
//! compile the tree the way a page does. Only a page built for `web`, holding
//! `WebHost`'s effects, does.
//!
//! So [`render_corpus`] turns each test source that calls `snapshot` into a
//! page, builds every page of a case in one `buri build`, and imports them
//! under `js_runtime()` into happy-dom's document, through [`super::browser`]:
//!
//!  * every `test` block that calls `snapshot` or `snapshotWide` becomes a block
//!    in the page's `main`, with each expression statement bound to `let _`, as
//!    a source outside a test has to;
//!  * `alloc()`, `headless()`, `observer()` and `address(...)` become the
//!    matching `WebHost` field;
//!  * `snapshot` and `snapshotWide` mount the tree instead of painting it, after
//!    a text holding the state, the name and the tree's scene, so the driver
//!    knows what to look for;
//!  * `core/testing/assert` becomes a module whose assertions check nothing:
//!    `buri test` already ran them, and they are not what a page shows;
//!  * a block that calls neither snapshot function is left out.
//!
//! The driver then fails the case when importing a page throws, when a promise
//! is rejected with nobody listening, when the page writes `console.error`, when
//! a snapshot never mounted, or when a text the scene holds is not on the page.
//! It also drives every element the way the snapshot's state describes, and
//! clicks everything with a click listener, failing on anything that throws.

use super::browser::browser;
use super::hang::Capped;
use super::{case_dirs, copy_tree, js_runtime, pool, Scratch};
use std::path::Path;
use std::process::Command;

/// Builds and mounts every case under `dir` that calls `snapshot`, and fails
/// with every problem found. `floor` is the fewest pages the walk may find.
pub fn render_corpus(dir: &Path, floor: usize) {
    let cases = case_dirs(dir, "CASE.textproto", 1);
    let found = pool::map(&cases, |case| render_case(case));
    let pages: usize = found.iter().map(|f| f.pages).sum();
    let snapshots: usize = found.iter().map(|f| f.snapshots).sum();
    let rendered = found.iter().filter(|f| f.pages > 0).count();
    let problems: Vec<String> = found.into_iter().flat_map(|f| f.problems).collect();
    assert!(
        problems.is_empty(),
        "{} problem(s) rendering UI snapshot cases as pages built for `web`:\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
    assert!(
        pages >= floor,
        "found {pages} pages to render, fewer than {floor}; the walk is broken"
    );
    eprintln!("rendered {snapshots} snapshots from {rendered} cases on pages built for `web`");
}

/// What one case came to.
struct Rendered {
    pages: usize,
    snapshots: usize,
    problems: Vec<String>,
}

fn render_case(case: &Path) -> Rendered {
    let pages = match build_pages(case) {
        Ok(pages) => pages,
        Err(rendered) => return rendered,
    };
    let mut rendered = Rendered {
        pages: pages.imports.len(),
        snapshots: pages.imports.iter().map(|(_, n)| n).sum(),
        problems: Vec::new(),
    };
    let out = pages.drive(&pages.imports, DRIVER);
    let stdout = String::from_utf8_lossy(&out.stdout);
    if out.status.code() != Some(0) || !stdout.contains(ALL_RENDERED) {
        rendered.problems.push(format!(
            "{}: its snapshots did not render as pages for `web`:\n{}{}",
            pages.name,
            super::indent(&stdout),
            super::indent(&String::from_utf8_lossy(&out.stderr))
        ));
    }
    rendered
}

/// The order a browser paints the boxes of each of `case`'s snapshots in, as
/// `(name, colours)` pairs in the order the snapshots mounted.
///
/// Each page is mounted in a happy-dom document of its own, so its boxes are
/// styled by its own stylesheet, and the hover a snapshot names is driven;
/// the order is then read off the computed styles by the
/// rules of CSS 2.1 Appendix E: a stacking context's own background, its
/// lifted boxes below zero, its flow, then its lifted boxes at zero in tree
/// order and those above zero by level, and an open dialog in the top layer
/// last. A box is named by its colour — `red`, `blue` or `green` — and a box
/// painted any other colour, or none, is left out.
pub fn paint_orders(case: &Path) -> Vec<(String, String)> {
    read_snapshots(case, ORDERS, "order ")
}

/// What a browser draws for each open dialog's scrim in each of `case`'s
/// snapshots, as `(name, scrims)` pairs in the order the snapshots mounted.
///
/// Each page is mounted the way [`paint_orders`] mounts it. A snapshot's
/// scrims are its open dialogs', from the bottom of the top layer to the top,
/// joined by ` / `, or `none`. A scrim is what the dialog's box computes to,
/// then `panel` and what the box inside it computes to, then any `::backdrop`
/// that reaches the dialog and paints a colour, a layer between the page and
/// the scrim.
pub fn scrims(case: &Path) -> Vec<(String, String)> {
    read_snapshots(case, SCRIMS, "scrims ")
}

/// The arrangement a browser gives the boxes of each of `case`'s snapshots,
/// as `(name, boxes)` pairs in the order the snapshots mounted.
///
/// Each page is mounted the way [`paint_orders`] mounts it. A snapshot's
/// boxes are its root and every element under it, in tree order, each named
/// by its tag and the alignment and padding it computes to that differ from
/// the property's initial value; a box with none is left out. So two scenes
/// that should look alike read the same.
pub fn arrangements(case: &Path) -> Vec<(String, String)> {
    read_snapshots(case, ARRANGEMENTS, "boxes ")
}

/// Mounts each of `case`'s pages and runs `script` after, answering each line
/// it prints that starts with `prefix` as a `(name, rest)` pair split at `: `.
fn read_snapshots(case: &Path, script: &str, prefix: &str) -> Vec<(String, String)> {
    let pages = match build_pages(case) {
        Ok(pages) => pages,
        Err(rendered) => panic!("{}", rendered.problems.join("\n\n")),
    };
    let mut read = Vec::new();
    for page in &pages.imports {
        let out = pages.drive(std::slice::from_ref(page), script);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.code() == Some(0) && stdout.contains(ALL_RENDERED),
            "{}: its snapshots did not render as pages for `web`:\n{}{}",
            pages.name,
            super::indent(&stdout),
            super::indent(&String::from_utf8_lossy(&out.stderr))
        );
        read.extend(
            stdout
                .lines()
                .filter_map(|line| line.strip_prefix(prefix))
                .filter_map(|line| line.split_once(": "))
                .map(|(name, rest)| (name.to_string(), rest.to_string())),
        );
    }
    read
}

/// A case's test sources that call `snapshot`, built as pages for `web` in a
/// scratch copy of its repository.
struct Pages {
    name: String,
    scratch: Scratch,
    /// The module of each page, as the driver imports it, and how many
    /// snapshots it mounts.
    imports: Vec<(String, usize)>,
}

/// Builds `case`'s pages, or answers what the case came to without them:
/// nothing, when no source calls `snapshot`, and the problem when the pages
/// do not build.
fn build_pages(case: &Path) -> Result<Pages, Rendered> {
    let name = case.file_name().unwrap().to_string_lossy().to_string();
    let scratch = Scratch::empty(&format!("web-{name}"));
    copy_tree(&case.join("repo"), &scratch.root, &[".buri", "out"]);
    let mut sources = Vec::new();
    collect_sources(&scratch.root, &mut sources);
    widen_surfaces(&scratch.root, &scratch.root);
    let mut targets = Vec::new();
    let mut counts = Vec::new();
    for source in sources {
        let text = std::fs::read_to_string(&source).unwrap();
        let Some(page) = page_of(&text) else { continue };
        let package = format!("buri_web/page_{}", targets.len());
        scratch.write(&format!("{package}/BUILD.buri"), PAGE_BUILD);
        scratch.write(&format!("{package}/main.buri"), &page.source);
        counts.push(page.snapshots);
        targets.push(package);
    }
    let mut rendered = Rendered {
        pages: targets.len(),
        snapshots: counts.iter().sum(),
        problems: Vec::new(),
    };
    if targets.is_empty() {
        return Err(rendered);
    }
    scratch.write("buri_web/snapshot/BUILD.buri", LIBRARY_BUILD);
    scratch.write("buri_web/snapshot/lib.buri", SNAPSHOT_LIB);
    scratch.write("buri_web/assert/BUILD.buri", LIBRARY_BUILD);
    scratch.write("buri_web/assert/lib.buri", ASSERT_LIB);

    let mut args = vec!["build".to_string()];
    args.extend(targets.iter().map(|t| format!("//{t}")));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let built = scratch.run(&args);
    if built.code != 0 {
        rendered.problems.push(format!(
            "{name}: its snapshots cannot be built as pages for `web`:\n{}",
            super::indent(&built.normalised(&scratch.root))
        ));
        return Err(rendered);
    }
    let imports = targets
        .iter()
        .zip(counts)
        .map(|(t, n)| (format!("\"./.buri/out/web/{t}/main.mjs\""), n))
        .collect();
    Ok(Pages {
        name,
        scratch,
        imports,
    })
}

impl Pages {
    /// Mounts `pages` in one document under `js_runtime()`, then runs `then`
    /// over the snapshots they mounted.
    fn drive(&self, pages: &[(String, usize)], then: &str) -> std::process::Output {
        let imports: Vec<&str> = pages.iter().map(|(page, _)| page.as_str()).collect();
        let script = format!(
            "{}\nconst PAGES = [{}];\nconst EXPECTED = {};\n{MOUNT}{then}",
            browser(),
            imports.join(", "),
            pages.iter().map(|(_, n)| n).sum::<usize>()
        );
        let path = self.scratch.write("drive.mjs", &script);
        Command::new(js_runtime())
            .arg(&path)
            .current_dir(&self.scratch.root)
            .capped_output()
            .expect("the javascript runtime runs")
    }
}

/// Puts every method a library's sources export on its surface, under `dir`.
///
/// A test source is inside its package, so it can call a method the package's
/// `lib.buri` does not export, and the page it becomes is outside. So the
/// scratch copy re-exports each method by name, which is what makes the page
/// see what the test saw.
fn widen_surfaces(root: &Path, dir: &Path) {
    let build = std::fs::read_to_string(dir.join("BUILD.buri")).unwrap_or_default();
    if let Some(at) = build.find("library {") {
        let package = dir
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let listed = &build[at..];
        let open = listed
            .find("sources: [")
            .map_or(0, |n| n + "sources: [".len());
        let close = open + listed[open..].find(']').unwrap_or(0);
        let mut exports = String::new();
        for file in listed[open..close].split(',') {
            let file = file.trim().trim_matches('"');
            let Ok(text) = std::fs::read_to_string(dir.join(file)) else {
                continue;
            };
            let methods: Vec<&str> = text
                .lines()
                .filter(|l| l.starts_with(' '))
                .filter_map(|l| l.trim_start().strip_prefix("export fn "))
                .map(|rest| rest.split(['(', '<']).next().unwrap())
                .collect();
            if !methods.is_empty() {
                exports.push_str(&format!(
                    "from \"//{package}/{file}\" export {{ {} }};\n",
                    methods.join(", ")
                ));
            }
        }
        if !exports.is_empty() {
            let lib = dir.join("lib.buri");
            let held = std::fs::read_to_string(&lib).unwrap_or_default();
            std::fs::write(&lib, format!("{held}\n{exports}")).unwrap();
        }
    }
    for entry in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
        if entry.path().is_dir() {
            widen_surfaces(root, &entry.path());
        }
    }
}

/// Every `.buri` file under `dir` that holds a `test` block, sorted.
fn collect_sources(dir: &Path, into: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_sources(&path, into);
        } else if path.extension().is_some_and(|e| e == "buri") {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.starts_with("test ") || text.contains("\ntest ") {
                into.push(path);
            }
        }
    }
}

/// A test source, rewritten as a page's `main.buri`.
struct Page {
    source: String,
    /// How many `snapshot` and `snapshotWide` calls the page makes.
    snapshots: usize,
}

/// The names `platform/effect/testing` exports that a page spells as a field
/// of its `WebHost`.
const HOST_FIELDS: &[(&str, &str)] = &[
    ("alloc", "host.alloc"),
    ("headless", "host.ui"),
    ("observer", "host.watch"),
    ("address", "host.location"),
];

const SNAPSHOT_FUNCTIONS: &[&str] = &["snapshot", "snapshotWide"];

/// `source` as a page, or nothing when no `test` block in it calls a snapshot
/// function.
fn page_of(source: &str) -> Option<Page> {
    let blanked = blank(source);
    let mut items = String::new();
    let mut blocks = Vec::new();
    let mut testing: Vec<String> = Vec::new();
    let mut snapshots = 0;
    let mut at = 0;
    while at < source.len() {
        let line_end = source[at..].find('\n').map_or(source.len(), |n| at + n + 1);
        let line = &source[at..line_end];
        if line.starts_with("test ") {
            let open = at + blanked[at..].iter().position(|&c| c == b'{').unwrap();
            let close = matching(&blanked, open);
            let calls: usize = SNAPSHOT_FUNCTIONS
                .iter()
                .map(|f| calls_of(&blanked[open..close], f).len())
                .sum();
            if calls > 0 {
                snapshots += calls;
                blocks.push(block(&source[open + 1..close], &testing));
            }
            at = source[close..]
                .find('\n')
                .map_or(source.len(), |n| close + n + 1);
            continue;
        }
        if line.starts_with("from \"platform/effect/testing\"")
            || line.starts_with("from \"core/testing/assert\"")
        {
            let end = at + blanked[at..].iter().position(|&c| c == b';').unwrap() + 1;
            let import = &source[at..end];
            if import.contains("core/testing/assert") {
                items.push_str(&import.replacen("core/testing/assert", "//buri_web/assert", 1));
            } else {
                let names = &import[import.find('{').unwrap() + 1..import.rfind('}').unwrap()];
                testing = names.split(',').map(|n| n.trim().to_string()).collect();
                let kept: Vec<&str> = testing
                    .iter()
                    .map(String::as_str)
                    .filter(|n| SNAPSHOT_FUNCTIONS.contains(n))
                    .collect();
                if !kept.is_empty() {
                    items.push_str(&format!(
                        "from \"//buri_web/snapshot\" import {{ {} }};",
                        kept.join(", ")
                    ));
                }
            }
            at = end;
            continue;
        }
        // Anything else is copied whole: the item that starts on this line
        // runs until a line ends with every bracket closed.
        let mut depth = 0i32;
        let mut end = at;
        for (k, &c) in blanked.iter().enumerate().skip(at) {
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'\n' if depth == 0 => {
                    end = k + 1;
                    break;
                }
                _ => {}
            }
            end = k + 1;
        }
        items.push_str(&source[at..end]);
        at = end;
    }
    if blocks.is_empty() {
        return None;
    }
    let mut page = String::from("from \"web\" import { WebHost };\n");
    page.push_str(&items);
    page.push_str("\nexport fn main(host: WebHost): Result<(), Str> {\n");
    for body in blocks {
        page.push_str(&format!("    let _ = {{{body}    ()\n    }};\n"));
    }
    page.push_str("    .Ok(())\n}\n");
    Some(Page {
        source: page,
        snapshots,
    })
}

/// A `test` block's body as a block in `main`: each expression statement
/// bound to `let _`, and each double from `testing` spelled as its host field.
fn block(body: &str, testing: &[String]) -> String {
    let blanked = blank(body);
    let mut out = String::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (k, &c) in blanked.iter().enumerate() {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b';' if depth == 0 => {
                out.push_str(&statement(&body[start..k], &blanked[start..k]));
                out.push(';');
                start = k + 1;
            }
            _ => {}
        }
    }
    let tail = &blanked[start..];
    if tail.iter().all(u8::is_ascii_whitespace) {
        out.push_str(&body[start..]);
    } else {
        out.push_str(&statement(&body[start..], tail));
        out.push_str(";\n");
    }
    for (name, field) in HOST_FIELDS {
        if testing.iter().any(|t| t == name) {
            out = replace_calls(&out, name, field);
        }
    }
    out
}

/// One statement, bound to `let _` unless it is a `let` already.
fn statement(text: &str, blanked: &[u8]) -> String {
    match blanked.iter().position(|c| !c.is_ascii_whitespace()) {
        Some(first) if !blanked[first..].starts_with(b"let ") => {
            format!("{}let _ = {}", &text[..first], &text[first..])
        }
        _ => text.to_string(),
    }
}

/// `text` with every call of `name` replaced by `with`, arguments and all.
fn replace_calls(text: &str, name: &str, with: &str) -> String {
    let blanked = blank(text);
    let mut out = String::new();
    let mut from = 0;
    for start in calls_of(&blanked, name) {
        let open = start + name.len();
        let close = matching(&blanked, open);
        out.push_str(&text[from..start]);
        out.push_str(with);
        from = close + 1;
    }
    out.push_str(&text[from..]);
    out
}

/// Where each call of `name` starts in `blanked`: the name, then `(`, with no
/// identifier character or `.` before it.
fn calls_of(blanked: &[u8], name: &str) -> Vec<usize> {
    let pattern = format!("{name}(");
    let pattern = pattern.as_bytes();
    (0..blanked.len().saturating_sub(pattern.len() - 1))
        .filter(|&k| blanked[k..].starts_with(pattern))
        .filter(|&k| {
            k == 0 || {
                let before = blanked[k - 1];
                !(before.is_ascii_alphanumeric() || before == b'_' || before == b'.')
            }
        })
        .collect()
}

/// The index of the bracket that closes the one at `open`.
fn matching(blanked: &[u8], open: usize) -> usize {
    let mut depth = 0i32;
    for (k, &c) in blanked.iter().enumerate().skip(open) {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    panic!("a bracket at byte {open} is never closed")
}

/// `source` with every comment and everything inside a string blanked to a
/// space, byte for byte, so brackets and semicolons can be read off it. A
/// string's own quotes stay, and so do line breaks.
fn blank(source: &str) -> Vec<u8> {
    enum In {
        Text,
        Interpolation(usize),
    }
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut stack: Vec<In> = Vec::new();
    let mut k = 0;
    let wipe = |out: &mut Vec<u8>, k: usize| {
        if out[k] != b'\n' {
            out[k] = b' ';
        }
    };
    while k < bytes.len() {
        let comment = bytes[k] == b'/' && bytes.get(k + 1) == Some(&b'/');
        match stack.last_mut() {
            Some(In::Text) => match bytes[k] {
                b'\\' => {
                    wipe(&mut out, k);
                    if k + 1 < bytes.len() {
                        wipe(&mut out, k + 1);
                    }
                    k += 1;
                }
                b'"' => {
                    stack.pop();
                    if !stack.is_empty() {
                        wipe(&mut out, k);
                    }
                }
                b'$' if bytes.get(k + 1) == Some(&b'{') => {
                    stack.push(In::Interpolation(0));
                    wipe(&mut out, k);
                    wipe(&mut out, k + 1);
                    k += 1;
                }
                _ => wipe(&mut out, k),
            },
            Some(In::Interpolation(depth)) => {
                match bytes[k] {
                    b'{' => *depth += 1,
                    b'}' if *depth == 0 => {
                        stack.pop();
                    }
                    b'}' => *depth -= 1,
                    b'"' => stack.push(In::Text),
                    _ => {}
                }
                wipe(&mut out, k);
            }
            None if comment => {
                while k < bytes.len() && bytes[k] != b'\n' {
                    wipe(&mut out, k);
                    k += 1;
                }
                continue;
            }
            None => {
                if bytes[k] == b'"' {
                    stack.push(In::Text);
                }
            }
        }
        k += 1;
    }
    out
}

const PAGE_BUILD: &str = "binary {\n    outputs: [\n        { platform: \"web\" },\n    ]\n}\n";

const LIBRARY_BUILD: &str = "library {\n    sources: []\n}\n";

/// `snapshot` and `snapshotWide` as a page has them: the tree is mounted, after
/// a text the driver reads its state, its name and its scene from. A second
/// mount follows, of a text the driver watches for, so it reads the tree the
/// moment it was mounted and not after the test wrote its next signal.
const SNAPSHOT_LIB: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/style" import { State };
from "ui/theme" import { Theme };

export fn snapshot<C: Allocator + Ui>(
    ctx: C,
    name: Str,
    root: Node<C>,
    state: State,
    themes: [Theme],
): () {
    snapshotWide(ctx, name, root, state, themes, 800)
}

export fn snapshotWide<C: Allocator + Ui>(
    ctx: C,
    name: Str,
    root: Node<C>,
    state: State,
    themes: [Theme],
    width: Int,
): () {
    let scene = ui.describeWide(ctx, root, state, width);
    let mark = str.format(ctx, "buri-snapshot\n${stateName(state)}\n${name}\n${scene}");
    let _ = ui.mount(
        ctx,
        ui.stack({ styles: [], children: [ui.text({ content: .Const(mark) }), root] }),
        themes,
    ).ignore();
    let _ = ui.mount(ctx, ui.text({ content: .Const("buri-snapshot-end") }), []).ignore();
}

fn stateName(state: State): Str {
    match (state) {
        .Hover => "hover",
        .Focus => "focus",
        .FocusWithin => "focus-within",
        .Active => "active",
        .Disabled => "disabled",
        .Checked => "checked",
        .Invalid => "invalid",
    }
}
"#;

/// `core/testing/assert`'s names, checking nothing. `some` still answers what
/// it holds, and on `.None`, which `buri test` has already ruled out, it never
/// returns.
const ASSERT_LIB: &str = r#"from "core/order" import { Equal };

export fn containsText(_haystack: Str, _needle: Str): () {
    ()
}

export fn equal<T: Equal>(_actual: T, _expected: T): () {
    ()
}

export fn notEqual<T: Equal>(_actual: T, _expected: T): () {
    ()
}

export fn isTrue(_actual: Bool): () {
    ()
}

export fn isFalse(_actual: Bool): () {
    ()
}

export fn length<T>(_items: [T], _expected: Int): () {
    ()
}

export fn some<T>(option: Option<T>): T {
    match (option) {
        .Some(v) => v,
        .None => some(option),
    }
}
"#;

/// What the driver prints last when every snapshot rendered.
const ALL_RENDERED: &str = "every snapshot rendered";

/// Imports each page in `PAGES`, checking each snapshot as it is mounted into
/// `snapshots`. A driver script goes on from here.
const MOUNT: &str = r#"
const problems = [];
process.on("unhandledRejection", (e) => problems.push(`a promise was rejected: ${e?.stack ?? e}`));
// What the driver is doing, for a problem the page reports while it does it.
let doing = "";
console.error = (...args) => problems.push(`the page wrote to console.error${doing}: ${args.join(" ")}`);
const settle = () => new Promise((done) => setTimeout(done, 0));

// Every element with a click listener, to click each one later.
const clicked = new Map();
const listen = EventTarget.prototype.addEventListener;
const unlisten = EventTarget.prototype.removeEventListener;
EventTarget.prototype.addEventListener = function (type, handler, options) {
  if (type === "click") clicked.set(this, (clicked.get(this) ?? new Set()).add(handler));
  return listen.call(this, type, handler, options);
};
EventTarget.prototype.removeEventListener = function (type, handler, options) {
  if (type === "click") clicked.get(this)?.delete(handler);
  return unlisten.call(this, type, handler, options);
};

// Everything a reader sees of `n`: its text, and the attributes and values a
// control shows as text.
function seen(n) {
  if (n.nodeType === 3) return n.data;
  if (n.nodeType !== 1) return "";
  const own = [...[...n.attributes].map((a) => a.value), typeof n.value === "string" ? n.value : ""];
  return own.join("\n") + "\n" + [...n.childNodes].map(seen).join("");
}
const unescape = (t) => t.replace(/\\(.)/g, (_, c) => (c === "n" ? "\n" : c === "r" ? "\r" : c));

// A snapshot root: its mark, then the tree.
const marked = (root) => {
  const mark = root.firstChild;
  return mark !== null && mark.nodeType === 3 && mark.data.startsWith("buri-snapshot\n");
};
const snapshots = [];
// The tree the last mount put on the page, read as the snapshot saw it: every
// text the scene holds is on the page.
function check(end) {
  const mounted = [...body.childNodes];
  const at = mounted.indexOf(end);
  const root = mounted.at(at < 0 ? -1 : at - 1);
  if (root === undefined || !marked(root)) {
    problems.push(`something other than a snapshot was mounted: ${showing()}`);
    return;
  }
  if (snapshots.some((s) => s.root === root)) {
    problems.push(`a snapshot did not mount: ${showing()}`);
    return;
  }
  const [, state, name, ...scene] = root.firstChild.data.split("\n");
  const tree = [...root.childNodes].slice(1);
  const page = tree.map(seen).join("\n");
  for (const line of scene) {
    if (!line.startsWith("t ")) continue;
    const text = unescape(line.slice(line.indexOf(" ", 2) + 1));
    if (!page.includes(text)) problems.push(`"${name}": "${text}" is in the snapshot and not on the page`);
  }
  snapshots.push({ root, name, state, tree });
}
// A text is made empty and given its words after, so the end text is caught as
// its words are written.
const data = Object.getOwnPropertyDescriptor(CharacterData.prototype, "data");
const createTextNode = document.createTextNode.bind(document);
document.createTextNode = (words) => {
  const text = createTextNode(words);
  Object.defineProperty(text, "data", {
    get: () => data.get.call(text),
    set: (value) => {
      data.set.call(text, value);
      if (value === "buri-snapshot-end") check(text);
    },
  });
  return text;
};

for (const page of PAGES) {
  try {
    await import(page);
  } catch (e) {
    problems.push(`importing ${page} threw: ${e?.stack ?? e}`);
  }
}
await settle();
if (snapshots.length !== EXPECTED) problems.push(`${snapshots.length} of ${EXPECTED} snapshots mounted`);
"#;

/// Drives what the pages mounted.
const DRIVER: &str = r#"

// Then each tree is driven the way its state describes, and everything that
// listens for a click is clicked.
function* elements(n) {
  if (n.nodeType === 1) yield n;
  for (const child of n.childNodes) yield* elements(child);
}
const EVENTS = {
  hover: ["pointerenter", "mouseenter", "pointerleave", "mouseleave"],
  active: ["pointerdown", "mousedown", "pointerup", "mouseup"],
};
for (const { name, state, tree } of snapshots) {
  const all = tree.flatMap((n) => [...elements(n)]);
  const act = (what, element, run) => {
    if (!element.isConnected) return;
    doing = ` while ${what} <${element.localName}> in "${name}"`;
    try {
      run();
    } catch (e) {
      problems.push(`"${name}": ${what} <${element.localName}> threw: ${e?.stack ?? e}`);
    }
    doing = "";
  };
  for (const element of all) {
    for (const type of EVENTS[state] ?? []) act(type, element, () => fire(element, type));
    if (state === "focus" || state === "focus-within") {
      act("focusing", element, () => element.focus());
      act("blurring", element, () => element.blur());
    }
  }
  for (const element of all) {
    if ((clicked.get(element)?.size ?? 0) > 0) act("clicking", element, () => element.click());
  }
  await settle();
}
await settle();
console.log(problems.length === 0 ? "every snapshot rendered" : problems.join("\n"));
"#;

/// Prints the order a browser paints each mounted snapshot's boxes in, one
/// `order <name>: <colours>` line each, after driving the hover a hovered
/// snapshot names ([`paint_orders`]).
const ORDERS: &str = r#"
const NAMED = { "255,0,0": "red", "0,0,255": "blue", "0,160,0": "green" };
// The name of an opaque colour written as `#rrggbb`, `rgb()` or `rgba()`.
function named(text) {
  const hex = /^#([0-9a-f]{6})$/i.exec(text ?? "");
  const rgb = /^rgba?\(([^)]*)\)$/.exec(text ?? "");
  let parts;
  if (hex) parts = [0, 2, 4].map((k) => parseInt(hex[1].slice(k, k + 2), 16));
  else if (rgb) parts = rgb[1].split(/[\s,/]+/).filter(Boolean).map(Number);
  else return null;
  if (parts.length > 3 && parts[3] < 1) return null;
  return NAMED[parts.slice(0, 3).join(",")] ?? null;
}
// What a box paints of its own: its background, or the fill of a drawing's
// first shape.
function own(element, out) {
  const shape = element.localName === "svg" ? element.firstElementChild : null;
  const colour =
    element.localName === "svg"
      ? named(shape?.style.fill)
      : named(getComputedStyle(element).backgroundColor);
  if (colour !== null) out.push(colour);
}
const unset = (value) => value === "" || value === "none" || value === "auto";
const positioned = (s) => ["relative", "absolute", "fixed", "sticky"].includes(s.position);
const isolates = (s) =>
  s.position === "fixed" ||
  s.position === "sticky" ||
  (positioned(s) && !unset(s.zIndex)) ||
  (s.opacity !== "" && Number(s.opacity) < 1) ||
  !unset(s.transform) ||
  !unset(s.backdropFilter);
const level = (s) => (positioned(s) && !unset(s.zIndex) ? Number(s.zIndex) : 0);
const topLayer = (element) => element.localName === "dialog" && element.open;
// The flow under `element` into `out`, in tree order, and its stacking
// context's lifted boxes under it into `lifted`, in tree order.
function walk(element, out, lifted) {
  for (const child of element.children) {
    const s = getComputedStyle(child);
    if (topLayer(child) || s.display === "none") continue;
    if (positioned(s) || isolates(s)) {
      lifted.push(child);
      if (!isolates(s)) walk(child, [], lifted);
      continue;
    }
    own(child, out);
    if (child.localName !== "svg") walk(child, out, lifted);
  }
}
function lift(element, out) {
  if (isolates(getComputedStyle(element))) return context(element, out);
  own(element, out);
  walk(element, out, []);
}
// A stacking context, painted whole.
function context(element, out) {
  own(element, out);
  const flow = [];
  const lifted = [];
  walk(element, flow, lifted);
  const levels = lifted
    .map((box, at) => [level(getComputedStyle(box)), at, box])
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  for (const [at, , box] of levels) if (at < 0) lift(box, out);
  out.push(...flow);
  for (const [at, , box] of levels) if (at >= 0) lift(box, out);
}
for (const { root, name, state, tree } of snapshots) {
  if (state === "hover") {
    for (const node of tree) {
      for (const element of [node, ...(node.querySelectorAll?.("*") ?? [])]) {
        fire(element, "pointerenter");
        fire(element, "mouseenter");
      }
    }
    await settle();
  }
  const out = [];
  context(root, out);
  for (const dialog of modals) if (topLayer(dialog) && root.contains(dialog)) context(dialog, out);
  console.log(`order ${name}: ${out.join(" ")}`);
}
console.log(problems.length === 0 ? "every snapshot rendered" : problems.join("\n"));
"#;

/// Prints what each mounted snapshot's open dialogs draw for their scrims, one
/// `scrims <name>: <scrims>` line each ([`scrims`]).
const SCRIMS: &str = r#"
const DRAWN = [
  "display", "position", "inset-block-start", "inset-block-end", "inset-inline-start", "inset-inline-end",
  "margin", "width", "height",
  "background-color", "backdrop-filter", "opacity", "align-items", "justify-content", "padding",
];
const PANEL = ["background-color", "width", "height"];
const clear = (colour) =>
  colour === "" || colour === "none" || colour === "transparent" || colour === "rgba(0, 0, 0, 0)";
const drawn = (element, fields) => {
  const s = getComputedStyle(element);
  return fields
    .map((field) => `${field}=${s.getPropertyValue(field) || "-"}`)
    .join(" ");
};
function scrim(dialog) {
  const panel = dialog.firstElementChild;
  let text = drawn(dialog, DRAWN);
  if (panel !== null) text += ` panel ${drawn(panel, PANEL)}`;
  for (const sheet of document.styleSheets) {
    for (const rule of sheet.cssRules) {
      const [on, pseudo] = (rule.selectorText ?? "").split("::");
      const colour = rule.style?.backgroundColor ?? "";
      if (pseudo === "backdrop" && dialog.matches(on) && !clear(colour)) text += ` backdrop=${colour}`;
    }
  }
  return text;
}
for (const { root, name } of snapshots) {
  const open = modals.filter((dialog) => dialog.open && root.contains(dialog));
  // A shut dialog draws nothing, so all that is said of one is how it is laid out.
  const shut = [...root.querySelectorAll("dialog:not([open])")].map(
    (dialog) => ` | shut display=${getComputedStyle(dialog).display}`,
  );
  console.log(`scrims ${name}: ${open.length === 0 ? "none" : open.map(scrim).join(" / ")}${shut.join("")}`);
}
console.log(problems.length === 0 ? "every snapshot rendered" : problems.join("\n"));
"#;

/// Prints each snapshot's boxes for [`arrangements`].
const ARRANGEMENTS: &str = r#"
const INITIAL = {
  "align-items": "normal", "align-self": "auto", "text-align": "start", "padding": "0px",
};
for (const { root, name } of snapshots) {
  const boxes = [root, ...root.querySelectorAll("*")].flatMap((element) => {
    const s = getComputedStyle(element);
    const set = Object.entries(INITIAL)
      .map(([field, initial]) => [field, s.getPropertyValue(field)])
      .filter(([field, value]) => value !== "" && value !== INITIAL[field])
      .map(([field, value]) => `${field}=${value}`);
    return set.length === 0 ? [] : [`${element.tagName.toLowerCase()} ${set.join(" ")}`];
  });
  console.log(`boxes ${name}: ${boxes.join(" | ")}`);
}
console.log(problems.length === 0 ? "every snapshot rendered" : problems.join("\n"));
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_block_becomes_a_block_of_main() {
        let page = page_of(
            r#"// One test.
from "platform/effect" import { Allocator, Ui };
from "platform/effect/testing" import { alloc, headless, snapshot };
from "core/testing/assert" import * as assert;

fn helper(): Str { "a; b { c" }

test "a ; test { with braces" {
    let ctx = context { Allocator: alloc(), Ui: headless() };
    // a comment; with a semicolon
    assert.containsText(ui.describe(ctx, helper(), .Hover), "x;${"}"}");
    snapshot(ctx, "one", tree(ctx), .Hover, []);
}

test "no picture" {
    assert.isTrue(true);
}
"#,
        )
        .unwrap();
        assert_eq!(page.snapshots, 1);
        assert_eq!(
            page.source,
            r#"from "web" import { WebHost };
// One test.
from "platform/effect" import { Allocator, Ui };
from "//buri_web/snapshot" import { snapshot };
from "//buri_web/assert" import * as assert;

fn helper(): Str { "a; b { c" }



export fn main(host: WebHost): Result<(), Str> {
    let _ = {
    let ctx = context { Allocator: host.alloc, Ui: host.ui };
    // a comment; with a semicolon
    let _ = assert.containsText(ui.describe(ctx, helper(), .Hover), "x;${"}"}");
    let _ = snapshot(ctx, "one", tree(ctx), .Hover, []);
    ()
    };
    .Ok(())
}
"#
        );
    }

    #[test]
    fn a_source_without_a_snapshot_is_no_page() {
        assert!(page_of("test \"t\" {\n    assert.isTrue(true);\n}\n").is_none());
    }

    #[test]
    fn an_address_becomes_the_host_location() {
        let testing = ["address".to_string(), "headless".to_string()];
        assert_eq!(
            block("\n    let l = address(headless(), \"/\");\n", &testing),
            "\n    let l = host.location;\n"
        );
    }
}
