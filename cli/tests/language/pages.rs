//! **What a mounted page does when a browser drives it.**
//!
//! `conformance/lib/ui` asserts the tree through the headless double, and
//! `conformance/lib/web` asserts the HTML a worker sends. Neither runs the
//! half of `runtime.js` a browser runs after `mount`: the listeners that show a
//! tooltip and the calls that move the focus. So each test here builds a page
//! for `web`, imports it on top of [`BROWSER`], a document that dispatches
//! events and keeps a focused element, and drives it the way a reader would.
//!
//! The double is small on purpose. It bubbles what a browser bubbles, keeps
//! every listener a type has, and moves `activeElement` with the four events
//! that come with it. Everything else a page could ask of a browser, it
//! doesn't have.

use crate::harness::hang::Capped;
use crate::harness::{js_runtime, tests_dir, Scratch};
use std::process::Command;

/// A document a page can mount into, with the events and the focus a reader
/// drives. `showing()` is the body's markup; `fire(target, type, fields)`
/// dispatches an event; `press(key)` is a keydown at the focused element.
const BROWSER: &str = r##"
const SVG = "http://www.w3.org/2000/svg";
// The events that stay on their target, as a browser's do.
const STAYS = new Set(["focus", "blur", "pointerenter", "pointerleave", "mouseenter", "mouseleave"]);

function listening(self) {
  self.listeners = {};
  self.addEventListener = (type, handler) => {
    (self.listeners[type] ??= []).push(handler);
  };
  self.removeEventListener = (type, handler) => {
    const held = self.listeners[type] || [];
    const at = held.indexOf(handler);
    if (at >= 0) held.splice(at, 1);
  };
}

function node(nodeType, nodeName, namespaceURI) {
  const self = {
    nodeType,
    nodeName,
    namespaceURI,
    childNodes: [],
    parentNode: null,
    attributes: {},
    data: "",
    disabled: false,
    get ownerDocument() {
      return globalThis.document;
    },
    get className() {
      return this.attributes.class ?? "";
    },
    set className(value) {
      this.attributes.class = value;
    },
    get nextSibling() {
      const parent = this.parentNode;
      if (parent === null) return null;
      const at = parent.childNodes.indexOf(this);
      return at + 1 < parent.childNodes.length ? parent.childNodes[at + 1] : null;
    },
    get firstChild() {
      return this.childNodes.length > 0 ? this.childNodes[0] : null;
    },
    insertBefore(child, before) {
      if (child.parentNode !== null) child.parentNode.removeChild(child);
      child.parentNode = this;
      const at = before === null ? this.childNodes.length : this.childNodes.indexOf(before);
      this.childNodes.splice(at, 0, child);
      return child;
    },
    appendChild(child) {
      return this.insertBefore(child, null);
    },
    removeChild(child) {
      const at = this.childNodes.indexOf(child);
      if (at >= 0) this.childNodes.splice(at, 1);
      child.parentNode = null;
      return child;
    },
    contains(other) {
      for (let at = other; at !== null && at !== undefined; at = at.parentNode) {
        if (at === this) return true;
      }
      return false;
    },
    setAttribute(name, value) {
      this.attributes[name] = String(value);
    },
    getAttribute(name) {
      return this.attributes[name] ?? null;
    },
    removeAttribute(name) {
      delete this.attributes[name];
    },
    focus() {
      focusOn(this);
    },
    blur() {
      if (document.activeElement === this) focusOn(null);
    },
  };
  listening(self);
  self.style = {
    get cssText() {
      return self.attributes.style ?? "";
    },
    set cssText(text) {
      if (text === "") delete self.attributes.style;
      else self.attributes.style = text;
    },
    setProperty(name, value) {
      const held = self.style.cssText;
      self.attributes.style = held === "" ? `${name}: ${value}` : `${held}; ${name}: ${value}`;
    },
  };
  return self;
}

// Dispatches `type` at `target`: to it, then out through its ancestors to the
// document, unless the type stays where it was fired.
function fire(target, type, fields) {
  const event = {
    type,
    target,
    defaultPrevented: false,
    preventDefault() {
      this.defaultPrevented = true;
    },
    ...fields,
  };
  const path = [target];
  if (!STAYS.has(type)) {
    for (let at = target.parentNode; at !== null; at = at.parentNode) path.push(at);
    path.push(document);
  }
  for (const at of path) {
    for (const handler of [...(at.listeners[type] || [])]) handler(event);
  }
  return event;
}

// What a browser lets the focus land on: a control or a link that is enabled,
// and anything with a `tabindex`.
function focusable(element) {
  if (element === null || element.nodeType !== 1) return false;
  if (element.attributes.tabindex !== undefined) return true;
  const name = element.nodeName.toLowerCase();
  return ["a", "button", "input", "select", "textarea"].includes(name) && !element.disabled;
}

function focusOn(element) {
  const next = element === null || focusable(element) ? element : document.activeElement;
  const was = document.activeElement;
  if (next === was) return;
  document.activeElement = next;
  if (was !== null) {
    fire(was, "blur", { relatedTarget: next });
    fire(was, "focusout", { relatedTarget: next });
  }
  if (next !== null) {
    fire(next, "focus", { relatedTarget: was });
    fire(next, "focusin", { relatedTarget: was });
  }
}

// A keydown where the focus is, or at the body when nothing has it.
function press(key) {
  return fire(document.activeElement ?? body, "keydown", { key });
}

const VOID = new Set(["img", "input", "hr"]);

function markup(n) {
  if (n.nodeType === 8) return "";
  if (n.nodeType === 3) return n.data;
  const name = n.nodeName.toLowerCase();
  let out = "<" + name;
  for (const key of Object.keys(n.attributes)) out += ` ${key}="${n.attributes[key]}"`;
  if (VOID.has(name)) return out + " />";
  return `${out}>${n.childNodes.map(markup).join("")}</${name}>`;
}

const body = node(1, "BODY");
const head = node(1, "HEAD");
const showing = () => body.childNodes.map(markup).join("");

// The element whose `aria-label` is `label`, wherever it is.
function labelled(label, at = body) {
  if (at.nodeType === 1 && at.attributes["aria-label"] === label) return at;
  for (const child of at.childNodes) {
    const found = labelled(label, child);
    if (found !== null) return found;
  }
  return null;
}

globalThis.document = {
  body,
  head,
  activeElement: null,
  getElementById() {
    return null;
  },
  createElement(name) {
    return node(1, name.toUpperCase());
  },
  createElementNS(namespace, name) {
    return node(1, name, namespace);
  },
  createTextNode(data) {
    const run = node(3, "#text");
    run.data = data;
    return run;
  },
  createComment() {
    return node(8, "#comment");
  },
};
listening(globalThis.document);

// The address bar: where the page is, and every move it made, in order.
const moves = [];
globalThis.location = { pathname: "/" };
globalThis.history = {
  pushState(_state, _title, path) {
    location.pathname = path;
    moves.push(`pushed ${path}`);
  },
  replaceState(_state, _title, path) {
    location.pathname = path;
    moves.push(`replaced ${path}`);
  },
};
"##;

/// Builds `source` as a page and runs `driver` against it on top of
/// [`BROWSER`], answering what the driver printed.
fn drive(name: &str, source: &str, driver: &str) -> String {
    let scratch = Scratch::repo(name);
    scratch.write(
        "cmd/page/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: \"web\" },\n    ]\n}\n",
    );
    scratch.write("cmd/page/main.buri", source);
    scratch.run(&["build", "//cmd/page"]).ok();
    let out = run_page(&scratch, ".buri/out/web/cmd/page/main.mjs", driver);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "the page did not run:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// Imports the module `artifact` names on top of [`BROWSER`], then runs
/// `driver`.
fn run_page(scratch: &Scratch, artifact: &str, driver: &str) -> std::process::Output {
    let script = format!("{BROWSER}\nawait import(\"./{artifact}\");\n{driver}");
    let path = scratch.write("drive.mjs", &script);
    Command::new(js_runtime())
        .arg(&path)
        .capped_output()
        .expect("the javascript runtime runs")
}

/// A copy button with a description, and a paste button beside it.
const TOOLTIP_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.tooltip({
                    text: .Const("Copy to clipboard"),
                    styles: [],
                    children: [
                        ui.button({ label: .Const("Copy"), styles: [], onPress: .Some(fn(_c) => ()) }),
                    ],
                }),
                ui.button({ label: .Const("Paste"), styles: [], onPress: .Some(fn(_c) => ()) }),
            ],
        }),
        [],
    )
}
"#;

/// A menu button and the menu it opens. Opening it puts the focus on its first
/// item, and Escape shuts it and hands the focus back to the button. A line
/// under them says what the three signals hold.
const MENU_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/signal" import { signal };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let open = signal(ctx, false);
    let trigger = signal(ctx, false);
    let first = signal(ctx, false);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.button({
                    label: .Const("Actions"),
                    styles: [],
                    onPress: .Some(fn(c) => {
                        let _ = open.set(c, true);
                        first.set(c, true)
                    }),
                    hasFocus: .Some(trigger),
                }),
                ui.choose(
                    .Cell(open),
                    ui.stack({
                        styles: [],
                        children: [
                            ui.button({
                                label: .Const("Rename"),
                                styles: [],
                                onPress: .Some(fn(c) => open.set(c, false)),
                                hasFocus: .Some(first),
                            }),
                            ui.button({
                                label: .Const("Delete"),
                                styles: [],
                                onPress: .Some(fn(c) => open.set(c, false)),
                                isInFocusOrder: .Some(.Const(false)),
                            }),
                        ],
                        role: .Some(.Group),
                        onKey: .Some(fn(c, key) => {
                            if (key == "Escape") {
                                let _ = open.set(c, false);
                                let _ = trigger.set(c, true);
                                true
                            } else {
                                false
                            }
                        }),
                    }),
                    ui.empty(),
                ),
                ui.text({
                    content: .Computed(fn(s) => {
                        str.format(s, "open:${open.get(s)} trigger:${trigger.get(s)} first:${first.get(s)}")
                    }),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_menu_takes_the_focus_and_escape_hands_it_back_to_its_trigger() {
    let printed = drive(
        "focus-page",
        MENU_PAGE,
        r#"
const said = () => body.childNodes[0].childNodes.at(-1).data;
const name = () => document.activeElement?.attributes["aria-label"] ?? "nothing";
const actions = labelled("Actions");
console.log(said());
// A browser focuses what the pointer presses, then clicks it.
actions.focus();
console.log(`pressed ${name()} ${said()}`);
fire(actions, "click");
console.log(`opened ${name()} ${said()}`);
console.log(`order ${labelled("Delete").attributes.tabindex}`);
const escaped = press("Escape");
console.log(`escaped ${name()} ${said()} claimed:${escaped.defaultPrevented}`);
console.log(`menu ${labelled("Rename") === null ? "gone" : "there"}`);
labelled("Actions").blur();
console.log(`blurred ${name()} ${said()}`);
"#,
    );
    assert_eq!(
        printed,
        "open:false trigger:false first:false\n\
         pressed Actions open:false trigger:true first:false\n\
         opened Rename open:true trigger:false first:true\n\
         order -1\n\
         escaped Actions open:false trigger:true first:false claimed:true\n\
         menu gone\n\
         blurred nothing open:false trigger:false first:false\n"
    );
}

#[test]
fn a_tooltip_shows_on_hover_and_on_focus_and_escape_hides_it() {
    let printed = drive(
        "tooltip-page",
        TOOLTIP_PAGE,
        r#"
const copy = labelled("Copy");
const paste = labelled("Paste");
const tip = copy.parentNode;
const shown = () => (tip.childNodes[1].attributes.hidden === undefined ? "shown" : "hidden");
console.log(showing());
fire(tip, "pointerenter");
console.log(`hovered ${shown()}`);
fire(tip, "pointerleave");
console.log(`left ${shown()}`);
copy.focus();
console.log(`focused ${shown()}`);
press("Escape");
console.log(`escaped ${shown()}`);
fire(tip, "pointerenter");
console.log(`hovered while dismissed ${shown()}`);
fire(tip, "pointerleave");
paste.focus();
console.log(`both left ${shown()}`);
fire(tip, "pointerenter");
console.log(`hovered again ${shown()}`);
"#,
    );
    assert_eq!(
        printed,
        "<div><div><button type=\"button\" aria-label=\"Copy\" aria-describedby=\"buri-tip-0\">Copy</button>\
         <div role=\"tooltip\" id=\"buri-tip-0\" hidden=\"\">Copy to clipboard</div></div>\
         <button type=\"button\" aria-label=\"Paste\">Paste</button></div>\n\
         hovered shown\n\
         left hidden\n\
         focused shown\n\
         escaped hidden\n\
         hovered while dismissed hidden\n\
         both left hidden\n\
         hovered again shown\n"
    );
}

/// Two lists whose rows are built by functions that only *hold* a handler
/// that navigates: one written inline, one bound to a `let` first. Building a
/// row waits on nothing, so every row renders, and pressing one still
/// navigates (buri-lang/buri#325).
const ROWS_PAGE: &str = r#"from "platform/effect" import { Allocator, Location, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/web" import * as web;
from "web" import { WebHost };

fn row<C: Location + Ui>(s: Str): Node<C> {
    ui.button({ label: .Const(s), styles: [], onPress: .Some(fn(c) => web.navigate(c, s)) })
}

fn held<C: Location + Ui>(s: Str): Node<C> {
    let go = fn(c: C) => web.navigate(c, s);
    ui.button({ label: .Const(s), styles: [], onPress: .Some(go) })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
        Location: host.location,
    };
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.each(.Const(["a", "b"]), fn(s) => s, fn(_c, s, _i) => row(s)),
                ui.each(.Const(["c"]), fn(s) => s, fn(_c, s, _i) => held(s)),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn rows_built_by_a_function_holding_a_navigating_handler_render_and_navigate() {
    let printed = drive(
        "rows-page",
        ROWS_PAGE,
        r#"
console.log(["a", "b", "c"].map((l) => `${l}:${labelled(l) === null ? "missing" : "shown"}`).join(" "));
fire(labelled("b"), "click");
await new Promise((done) => setTimeout(done, 0));
fire(labelled("c"), "click");
await new Promise((done) => setTimeout(done, 0));
console.log(moves.join("\n"));
"#,
    );
    assert_eq!(printed, "a:shown b:shown c:shown\npushed b\npushed c\n");
}

/// A page whose `ui.each` rows each run `act` with their context while they
/// are built: `act` is the row function's body, and ends in the row's node.
fn acting_rows(act: &str) -> String {
    format!(
        r#"from "platform/effect" import {{ Allocator, Location, Ui }};
from "ui/node" import * as ui;
from "ui/node" import {{ Node }};
from "ui/web" import * as web;
from "web" import {{ WebHost }};

fn row<C: Location + Ui>(ctx: C, s: Str): Node<C> {{
    {act}
}}

export fn main(host: WebHost): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
        Location: host.location,
    }};
    ui.mount(ctx, ui.each(.Const(["a", "b"]), fn(s) => s, fn(c, s, _i) => row(c, s)), [])
}}
"#
    )
}

/// What a row driver prints: whether each row's button is there, then every
/// move the address bar made.
const ROWS_AND_MOVES: &str = r#"
console.log(["a", "b"].map((l) => `${l}:${labelled(l) === null ? "missing" : "shown"}`).join(" "));
console.log(moves.join("\n"));
"#;

/// A row that redirects while it is built calls the address bar's `replace`,
/// which answers at once, so the row is a node rather than a promise and both
/// rows render (buri-lang/buri#330).
#[test]
fn rows_that_replace_the_address_while_they_are_built_render() {
    let page = acting_rows(
        r#"let _ = web.replace(ctx, "/x");
    ui.button({ label: .Const(s), styles: [], onPress: .None })"#,
    );
    let printed = drive("rows-replace", &page, ROWS_AND_MOVES);
    assert_eq!(printed, "a:shown b:shown\nreplaced /x\nreplaced /x\n");
}

/// The same for `push`, which `navigate` calls.
#[test]
fn rows_that_navigate_while_they_are_built_render() {
    let page = acting_rows(
        r#"let _ = web.navigate(ctx, "/y");
    ui.button({ label: .Const(s), styles: [], onPress: .None })"#,
    );
    let printed = drive("rows-navigate", &page, ROWS_AND_MOVES);
    assert_eq!(printed, "a:shown b:shown\npushed /y\npushed /y\n");
}

/// The same for `path`, which `route` reads: each row shows the address, `/`.
#[test]
fn rows_that_read_the_route_while_they_are_built_render() {
    let page = acting_rows(r#"ui.text({ content: web.route(ctx) })"#);
    let printed = drive(
        "rows-route",
        &page,
        "console.log(body.childNodes.filter((n) => n.nodeType === 3).map((n) => n.data).join(\" \"));\n",
    );
    assert_eq!(printed, "/ /\n");
}

/// `//platform/two` with `HostUi` on its page's host and a second method on
/// `Greet`, `check`, which answers a `Result`. The page mounts rows that each
/// ask for a name and label themselves with what `check` says of their item.
/// `name` and `check` are the `js` file's two methods of `HostGreet`.
fn greeting_rows(name: &str, check: &str) -> Scratch {
    let scratch = Scratch::copy_of(
        "greeting-rows",
        &tests_dir().join("repositories/platform/two_entries_two_hosts/repo"),
    );
    let edit = |path: &str, from: &str, to: &str| {
        let held = scratch.read(path);
        assert!(held.contains(from), "{path} holds {from}");
        scratch.write(path, &held.replace(from, to));
    };
    let platform = "platform/two/platform.buri";
    edit(platform, "{ HostAllocator }", "{ HostAllocator, HostUi }");
    edit(
        platform,
        "export greet: HostGreet,\n}",
        "export greet: HostGreet,\n    export ui: HostUi,\n}",
    );
    let checks = "    fn check(self, s: Str): Result<Str, Str>;\n}";
    edit(
        platform,
        "    fn name(self): Str;\n}",
        &format!("    fn name(self): Str;\n\n{checks}"),
    );
    edit(
        "platform/effect/greet/lib.buri",
        "    fn name(self): Str;\n}",
        &format!(
            "    fn name(self): Str;\n\n{checks}\n\n\
             export fn check<C: Greet>(ctx: C, s: Str): Result<Str, Str> {{\n    ctx.check(s)\n}}"
        ),
    );
    edit(
        "platform/effect/greet/testing/lib.buri",
        "        \"test\"\n    }\n",
        "        \"test\"\n    }\n\n    fn check(self, s: Str): Result<Str, Str> {\n        .Ok(s)\n    }\n",
    );
    scratch.write(
        "platform/two/page.mjs",
        &format!(
            "import {{ page }} from \"buri:program\";\n\n\
             export const HostGreet = {{\n  name: {name},\n  check: {check},\n}};\nawait page();\n"
        ),
    );
    scratch.write(
        "cmd/app/main.buri",
        r#"from "core/net/http" import * as http;
from "platform/effect" import { Allocator, Request, Response, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "//platform/effect/count" import * as count;
from "//platform/effect/greet" import * as greet;
from "//platform/effect/greet" import { Greet };
from "//platform/two" import { PageHost, WorkerHost };

fn row<C: Greet + Ui>(ctx: C, s: Str): Node<C> {
    let _ = greet.name(ctx);
    let label = match (greet.check(ctx, s)) {
        .Ok(said) => said,
        .Err(refused) => refused,
    };
    ui.button({ label: .Const(label), styles: [], onPress: .None })
}

export fn page(host: PageHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
        Greet: host.greet,
    };
    ui.mount(ctx, ui.each(.Const(["a", "b"]), fn(s) => s, fn(c, s, _i) => row(c, s)), [])
}

export fn worker(host: WorkerHost, request: Request): Response {
    let _ = count.next(host.count);
    http.status(200)
}
"#,
    );
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch
}

const PAGE_ARTIFACT: &str = ".buri/out/platform/two/cmd/app/page.mjs";

/// A `check` that answers `a` in capitals and refuses `b`.
const CHECK: &str =
    r#"(self, s) => { if (s === "b") throw new Error("no b"); return s.toUpperCase(); }"#;

/// A repository platform's methods that its `js` file writes without `async`
/// answer at once, a thrown error included, so rows that call them while they
/// are built render, as `web`'s address bar does (buri-lang/buri#330).
#[test]
fn rows_that_call_a_platform_s_synchronous_methods_while_they_are_built_render() {
    let scratch = greeting_rows(
        r#"(self) => { console.log("named"); return "page"; }"#,
        CHECK,
    );
    let out = run_page(
        &scratch,
        PAGE_ARTIFACT,
        r#"console.log(["A", "no b"].map((l) => `${l}:${labelled(l) === null ? "missing" : "shown"}`).join(" "));"#,
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "named\nnamed\nA:shown no b:shown\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success());
}

/// A method written without `async` that answers a promise anyway is a
/// mistake in the `js` file that the program can't wait out. It fails at the
/// call, naming the method and the fix, rather than handing the program a
/// promise for a name.
#[test]
fn a_synchronous_method_that_answers_a_promise_fails_naming_itself() {
    let scratch = greeting_rows(r#"(self) => Promise.resolve("page")"#, CHECK);
    let out = run_page(&scratch, PAGE_ARTIFACT, "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "the page ran:\n{stderr}");
    assert!(
        stderr.contains("`HostGreet.name` answered a promise; write it `async` in the js file"),
        "{stderr}"
    );
}

/// The same for a method that answers a `Result`: the promise is the file's
/// mistake, not the method's `.Err`.
#[test]
fn a_synchronous_result_method_that_answers_a_promise_fails_naming_itself() {
    let scratch = greeting_rows(r#"(self) => "page""#, r#"(self, s) => Promise.resolve(s)"#);
    let out = run_page(&scratch, PAGE_ARTIFACT, "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "the page ran:\n{stderr}");
    assert!(
        stderr.contains("`HostGreet.check` answered a promise; write it `async` in the js file"),
        "{stderr}"
    );
}

/// An `async` method is still awaited where nothing calls it without waiting:
/// the page asks for its name before it mounts.
#[test]
fn a_platform_s_async_method_is_awaited() {
    let scratch = greeting_rows(r#"async (self) => "page""#, CHECK);
    assert_eq!(named_before_mount(&scratch), "page\n");
}

/// A method the file writes as a name rather than a function may be `async`,
/// so it's awaited too.
#[test]
fn a_platform_s_method_written_as_a_name_is_awaited() {
    let scratch = greeting_rows("greetName", CHECK);
    let file = "platform/two/page.mjs";
    let held = scratch.read(file);
    scratch.write(
        file,
        &held.replace(
            "export const HostGreet",
            "const greetName = async (self) => \"page\";\nexport const HostGreet",
        ),
    );
    assert_eq!(named_before_mount(&scratch), "page\n");
}

/// Rebuilds a [`greeting_rows`] page to ask for its name before it mounts,
/// and runs it, answering the text the name went into.
fn named_before_mount(scratch: &Scratch) -> String {
    let main = "cmd/app/main.buri";
    let held = scratch.read(main);
    scratch.write(
        main,
        &held
            .replace("    let _ = greet.name(ctx);\n", "")
            .replace(
                "    ui.mount(ctx, ui.each(",
                "    let named = greet.name(ctx);\n    ui.mount(ctx, ui.stack({ styles: [], children: [ui.text({ content: .Const(named) }), ui.each(",
            )
            .replace("row(c, s)), [])", "row(c, s))] }), [])"),
    );
    scratch.run(&["build", "//cmd/app"]).ok();
    let out = run_page(
        scratch,
        PAGE_ARTIFACT,
        "console.log(body.childNodes[0].childNodes[0].data);\n",
    );
    assert!(
        out.status.success(),
        "the page did not run:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}
