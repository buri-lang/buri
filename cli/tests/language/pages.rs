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

use crate::harness::{js_runtime, Scratch};
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
    let script = format!(
        "{BROWSER}\nawait import(\"./.buri/out/web/cmd/page/main.mjs\");\n{driver}"
    );
    let path = scratch.write("drive.mjs", &script);
    let out = Command::new(js_runtime()).arg(&path).output().expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "the page did not run:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
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
