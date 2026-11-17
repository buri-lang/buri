//! A browser a page built for `web` can mount into, for the suites that import
//! one under `js_runtime()`.
//!
//! The document is happy-dom's, registered as the global window the way a page
//! finds it in a browser. `happy-dom/` holds it as one bundled file, pinned by
//! its `package.json` and `bun.lock` and rebuilt by its `bundle.sh`, so a test
//! run needs no install and no network, under `bun` and `node` alike.
//!
//! What is left here is what a test needs that a browser doesn't offer: a way
//! to fire the events a reader causes, a log of the address bar's moves, and an
//! exit status that fails when a listener or a timer threw.

/// The prelude a driver script starts with. It registers happy-dom's window as
/// the global one and defines:
///
///  * `body`, the document's body;
///  * `showing()`, the body's markup without the comments a page marks its
///    regions with;
///  * `labelled(label)`, the element whose `aria-label` is `label`, or `null`;
///  * `fire(target, type, fields)`, which dispatches `type` at `target` as the
///    event a browser fires for it and answers the event;
///  * `press(key)`, a keydown at the focused element and, when no listener
///    cancelled it, the key's default action — Escape cancels a dialog, Enter
///    and Space click a button, Space a checkbox or a radio, the arrows move a
///    radio group or a slider, the left and right arrows, Home and End move a
///    text field's caret, and Enter in a form's field submits the form —
///    which happy-dom doesn't take itself, then the keyup;
///  * a modal dialog that takes the focus as it opens and gives it back when
///    it shuts, which happy-dom leaves out too, and whose `close` event comes
///    a task later, as a browser queues it; `showModal` on a dialog already
///    open throws unless it is modal, as a browser's does;
///  * `modals`, the top layer happy-dom leaves out: the open modal dialogs,
///    from the bottom to the top, the one opened last on top, which `:modal`
///    matches and outside the topmost of which nothing takes the focus;
///  * `showPopover` and `hidePopover`, which happy-dom leaves out, and
///    `topLayers`, the whole top layer from the bottom to the top: the modal
///    dialogs and the popovers shown, each put on top as it opens and taken
///    out as it shuts or leaves the page, which `:popover-open` matches;
///  * `placedAgainst(element)`, the box a browser places `element` against:
///    the nearest ancestor whose anchor name its `position-anchor` names, when
///    its `position-area` makes that the containing block, or `null`;
///  * `containingBlock(element)`, the box a browser measures the out-of-flow
///    box `element` from: its anchor under a `position-area`, else the
///    nearest ancestor that is positioned, transformed or backdrop-filtered
///    (only the last two for a fixed box), or `null` for the viewport;
///  * `placedAt(element)`, where a browser lays out an out-of-flow box,
///    which happy-dom leaves out: its offset and size within its
///    `containingBlock`, by the inset, size and self-alignment it computes to;
///  * nothing a browser draws none of taking the focus: what is inside a shut
///    dialog, a shut disclosure's body or a `hidden` element, and the focus
///    leaving a shut dialog that gives it back to such an element;
///  * nothing a style gives `display: none` taking the focus either;
///  * `resize(width)`, which makes the window that wide and restyles the page
///    for it, which happy-dom leaves out, before any of the page's `resize`
///    listeners run;
///  * `display(element)`, the `display` a browser gives `element`, from the
///    page's sheets, where happy-dom drops the values it can't parse;
///  * `boxes(container)`, the boxes a browser lays out for `container`'s
///    children, each the nodes it holds, which happy-dom lays out none of;
///  * `parse(html)`, which lays out a document a server sent as a browser
///    parses it, running its inline scripts;
///  * `moves`, every move the address bar made, in order;
///  * an input's value sanitized as Chromium does it, where happy-dom
///    sanitizes in part: a slider on its track and its step, a number field
///    empty unless it holds a number, a line field without line breaks and an
///    email field without surrounding spaces, whether the value came from the
///    attribute or a script;
///  * a form that `requestSubmit` refuses because a field is invalid moving
///    the focus to that field, which happy-dom leaves out.
///
/// An error a listener or a timer throws is written to `console.error`, as a
/// browser does, and also makes the script exit with 1.
pub fn browser() -> String {
    let bundle = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/harness/happy-dom/happy-dom.mjs"
    );
    format!("import {{ GlobalRegistrator }} from {bundle:?};\n{PRELUDE}")
}

const PRELUDE: &str = r##"
GlobalRegistrator.register({ url: "http://localhost/" });
window.addEventListener("error", () => {
  process.exitCode = 1;
});
// happy-dom matches a media query against the viewport, but keeps each
// element's computed style from before the viewport changed; a browser
// restyles the page first. Setting the adopted sheets drops what it kept. This
// listener is the page's first, so every later one sees the new styles.
window.addEventListener("resize", () => {
  document.adoptedStyleSheets = document.adoptedStyleSheets;
});
// The window `width` CSS pixels across, as a reader resizing it leaves it.
const resize = (width) => happyDOM.setViewport({ width });

const body = document.body;
const showing = () => body.innerHTML.replace(/<!--.*?-->/gs, "");
const labelled = (label) => document.querySelector(`[aria-label=${JSON.stringify(label)}]`);

// The event a browser fires for each type a test causes: its interface, whether
// it bubbles, and whether a listener can cancel it.
const FIRED = {
  pointerdown: [PointerEvent, true, true],
  pointerup: [PointerEvent, true, true],
  pointermove: [PointerEvent, true, true],
  pointerenter: [PointerEvent, false, false],
  pointerleave: [PointerEvent, false, false],
  mousedown: [MouseEvent, true, true],
  mouseup: [MouseEvent, true, true],
  mouseenter: [MouseEvent, false, false],
  mouseleave: [MouseEvent, false, false],
  keydown: [KeyboardEvent, true, true],
  keyup: [KeyboardEvent, true, true],
  input: [InputEvent, true, false],
};

function fire(target, type, fields = {}) {
  const known = FIRED[type];
  if (known === undefined) throw new Error(`fire does not know how a browser fires "${type}"`);
  const [Interface, bubbles, cancelable] = known;
  const event = new Interface(type, { bubbles, cancelable, composed: true, ...fields });
  target.dispatchEvent(event);
  return event;
}

// A keydown where the focus is (the body when nothing has it), then, unless a
// listener cancelled it, what a browser does with the key by itself, then the
// keyup where the focus is by then.
function press(key) {
  const at = document.activeElement;
  const event = fire(at, "keydown", { key });
  if (!event.defaultPrevented) byItself(at, key);
  fire(document.activeElement, "keyup", { key });
  return event;
}

// The fields whose caret a page can read through `selectionStart`.
const CARET_FIELDS = ["text", "search", "url", "tel", "password"];

// The fields a browser submits a form from when it has no submit button.
const LINE_FIELDS = ["text", "search", "url", "tel", "email", "password", "number"];

// A key's default action on `at`, which happy-dom leaves out. It does
// implement what each one comes down to: `click()`, a dialog's `close()`, and
// `requestSubmit()`.
function byItself(at, key) {
  const tag = at.tagName;
  const type = tag === "INPUT" ? at.type : "";
  const pressing = key === "Enter" || key === " ";
  if (key === "Escape") {
    // Escape cancels the topmost modal dialog: `cancel`, which a listener can
    // cancel, then `close`.
    const dialog = modals.findLast((d) => d.isConnected && d.open);
    if (dialog !== undefined && dialog.dispatchEvent(new Event("cancel", { cancelable: true }))) {
      dialog.close();
    }
  } else if (pressing && (tag === "BUTTON" || tag === "SUMMARY")) at.click();
  else if (key === "Enter" && tag === "A") at.click();
  else if (key === " " && (type === "checkbox" || type === "radio")) at.click();
  else if (type === "radio" && key.startsWith("Arrow")) {
    // The arrows move to the next radio of the group, wrapping, and check it.
    const group = [...document.querySelectorAll(`input[type=radio][name=${JSON.stringify(at.name)}]`)];
    const forward = key === "ArrowDown" || key === "ArrowRight";
    const next = group[(group.indexOf(at) + (forward ? 1 : group.length - 1)) % group.length];
    next.focus();
    next.click();
  } else if (type === "range") slide(at, key);
  else if (tag === "TEXTAREA" || (tag === "INPUT" && CARET_FIELDS.includes(type))) moveCaret(at, key);
  if (key === "Enter" && tag === "INPUT" && at.form !== null) submitImplicitly(at);
}

// A slider's keys: an arrow moves one step, Page Up and Page Down a tenth of
// the track or a step when that is more, Home and End to either end. Setting
// the value keeps it on the track and its step, below. With `step="any"` an
// arrow moves a hundredth of the track, as Chromium's range input does.
function slide(at, key) {
  const { min, max, step: snap } = track(at);
  const step = snap === 0 ? (max - min) / 100 : snap;
  const now = Number(at.value);
  const page = Math.max(step, (max - min) / 10);
  const moves = {
    ArrowRight: now + step,
    ArrowUp: now + step,
    ArrowLeft: now - step,
    ArrowDown: now - step,
    PageUp: now + page,
    PageDown: now - page,
    Home: min,
    End: max,
  };
  const to = moves[key];
  if (to === undefined) return;
  at.value = String(to);
  at.dispatchEvent(new InputEvent("input", { bubbles: true }));
  at.dispatchEvent(new Event("change", { bubbles: true }));
}

// A range input's track as Chromium reads its attributes: a bound that isn't a
// number is the default, a largest below the smallest is the smallest, and a
// step that is missing, not a number or not above nought is one. `any` is a
// step of nought, a continuous track.
function track(at) {
  const number = (text, otherwise) => (FLOAT.test(text) && Number.isFinite(Number(text)) ? Number(text) : otherwise);
  const min = number(at.getAttribute("min") ?? "", 0);
  const max = Math.max(min, number(at.getAttribute("max") ?? "", 100));
  const given = at.getAttribute("step") ?? "";
  const step = given.toLowerCase() === "any" ? 0 : number(given, 1) > 0 ? number(given, 1) : 1;
  return { min, max, step };
}

// HTML's valid floating-point number.
const FLOAT = /^-?(\d+(\.\d+)?|\.\d+)([eE][+-]?\d+)?$/;

// The places after the point `x` is written with, and `x` rounded to them:
// Chromium steps in decimal, so three steps of 0.1 are 0.3.
const places = (x) => (String(x).split(/e/i)[0].split(".")[1] ?? "").length - Number(String(x).split(/e/i)[1] ?? 0);
const exact = (x, min, step) => Number(x.toFixed(Math.min(100, Math.max(0, places(min), places(step)))));

// Chromium's value sanitization, which happy-dom has only in part: a range's
// value is clamped and snapped to its step (`StepRange::ClampValue`), the
// middle when it isn't a number, a number's is empty unless it is a valid
// floating-point number, a line field's loses its line breaks and an email
// field's its surrounding spaces too. The value an attribute gives is
// sanitized as well as the one a script sets.
const inputValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
function sanitized(at, text) {
  if (at.type === "number") return FLOAT.test(text) && Number.isFinite(Number(text)) ? text : "";
  if (["text", "search", "password"].includes(at.type)) return text.replace(/[\r\n]/g, "");
  if (at.type === "email") return text.replace(/[\r\n]/g, "").replace(/^[\t\n\f\r ]+|[\t\n\f\r ]+$/g, "");
  if (at.type !== "range") return text;
  const { min, max, step } = track(at);
  const given = FLOAT.test(text) && Number.isFinite(Number(text)) ? Number(text) : min + (max - min) / 2;
  const on = Math.min(Math.max(given, min), max);
  if (step === 0) return String(on);
  const snapped = exact(min + Math.round((on - min) / step) * step, min, step);
  return String(snapped > max ? exact(snapped - step, min, step) : snapped);
}
Object.defineProperty(HTMLInputElement.prototype, "value", {
  configurable: true,
  get() {
    return sanitized(this, inputValue.get.call(this));
  },
  set(text) {
    inputValue.set.call(this, text);
  },
});
// With every value sanitized, a range or a number input is never bad input or
// off its step, which happy-dom's checks get wrong: `0.3 % 0.1` isn't nought,
// and `1e3` isn't a number to them.
const validity = Object.getPrototypeOf(document.createElement("input").validity);
for (const check of ["badInput", "stepMismatch"]) {
  const was = Object.getOwnPropertyDescriptor(validity, check).get;
  Object.defineProperty(validity, check, {
    configurable: true,
    get() {
      return ["number", "range"].includes(this.element.type) ? false : was.call(this);
    },
  });
}

// A refused submission moves the focus to the first invalid control, as
// Chromium's interactive validation does and happy-dom's doesn't.
const requestSubmit = HTMLFormElement.prototype.requestSubmit;
HTMLFormElement.prototype.requestSubmit = function (submitter) {
  if (!(submitter?.formNoValidate || this.noValidate)) {
    const invalid = [...this.elements].find((e) => !e.disabled && e.validity !== undefined && !e.validity.valid);
    if (invalid !== undefined) {
      invalid.focus();
      return;
    }
  }
  requestSubmit.call(this, submitter);
};

// A text field's caret keys: the left and right arrows move the caret one
// grapheme, or collapse a selection to the edge they point at; Home and End
// move it to the start and end of its line.
function moveCaret(at, key) {
  const text = at.value;
  const low = at.selectionStart;
  const high = at.selectionEnd;
  const bounds = [...new Intl.Segmenter().segment(text)].map((s) => s.index).concat([text.length]);
  const lineStart = (i) => text.lastIndexOf("\n", i - 1) + 1;
  const lineEnd = (i) => (text.indexOf("\n", i) < 0 ? text.length : text.indexOf("\n", i));
  const moves = {
    ArrowLeft: () => (low < high ? low : (bounds.findLast((b) => b < low) ?? 0)),
    ArrowRight: () => (low < high ? high : (bounds.find((b) => b > high) ?? high)),
    Home: () => lineStart(at.selectionDirection === "backward" ? low : high),
    End: () => lineEnd(at.selectionDirection === "backward" ? low : high),
  };
  const move = moves[key];
  if (move === undefined) return;
  const to = move();
  at.setSelectionRange(to, to);
}

// The element that had the focus when each open modal dialog opened.
const openers = new WeakMap();
// The top layer: each open modal dialog, the one opened last on top.
const modals = [];
const showModal = HTMLDialogElement.prototype.showModal;
// The elements a browser lets take the focus, before the checks below.
const FOCUSABLE = "a[href], button, input, select, textarea, summary, [tabindex]";
// Whether a browser draws `e`: it is inside no shut dialog, no `hidden`
// element, no element a style gives `display: none`, and no shut disclosure's
// body. A disclosure's summary is drawn while the disclosure is.
const rendered = (e) => {
  if (e.closest("[hidden], dialog:not([open])") !== null || !e.checkVisibility()) return false;
  const shut = e.closest("details:not([open])");
  if (shut === null) return true;
  return e.tagName === "SUMMARY" && e.parentElement === shut && (shut.parentElement === null || rendered(shut.parentElement));
};
// `showModal`, with the step happy-dom leaves out: the focus moves to the first
// element inside that can take it, in document order and whether or not Tab
// reaches it, and otherwise off the element that had it.
HTMLDialogElement.prototype.showModal = function () {
  if (modals.includes(this)) return;
  if (this.open) throw new DOMException("the dialog is already open and not modal", "InvalidStateError");
  const had = document.activeElement;
  showModal.call(this);
  modals.push(this);
  topLayers.push(this);
  openers.set(this, had === body ? null : had);
  const first = [...this.querySelectorAll(FOCUSABLE)].find((e) => !e.disabled && rendered(e));
  if (first !== undefined) first.focus();
  else if (had !== body) had.blur();
};
// `close`, with the steps happy-dom leaves out: the focus goes back to the
// element that had it when the dialog opened, while that element can still
// take it, and otherwise leaves the shut dialog; and `close` fires as a task.
HTMLDialogElement.prototype.close = function (returnValue) {
  if (!this.open) return;
  this.removeAttribute("open");
  if (modals.includes(this)) modals.splice(modals.indexOf(this), 1);
  if (topLayers.includes(this)) topLayers.splice(topLayers.indexOf(this), 1);
  this.returnValue = returnValue === undefined ? "" : String(returnValue);
  const back = openers.get(this) ?? null;
  openers.delete(this);
  if (back !== null && back.isConnected && !back.disabled) back.focus();
  const at = document.activeElement;
  if (at !== null && at !== body && !rendered(at)) at.blur();
  setTimeout(() => this.dispatchEvent(new Event("close")), 0);
};

// The top layer, from the bottom to the top: the modal dialogs and the shown
// popovers. One that left the page left it too, as a browser takes it out.
const topLayers = [];
const drop = () => {
  for (let at = topLayers.length - 1; at >= 0; at--) if (!topLayers[at].isConnected) topLayers.splice(at, 1);
};
// `showPopover` and `hidePopover`, which happy-dom leaves out: a popover goes
// on top of the top layer as it opens and comes out as it shuts. Each throws
// where a browser's does, on an element that is not a popover or not on the
// page; opening an open one or shutting a shut one does nothing.
HTMLElement.prototype.showPopover = function () {
  drop();
  if (!this.hasAttribute("popover")) throw new DOMException("the element is not a popover", "NotSupportedError");
  if (topLayers.includes(this)) return;
  if (!this.isConnected) throw new DOMException("the popover is not on the page", "InvalidStateError");
  topLayers.push(this);
};
HTMLElement.prototype.hidePopover = function () {
  drop();
  if (!this.hasAttribute("popover")) throw new DOMException("the element is not a popover", "NotSupportedError");
  if (topLayers.includes(this)) topLayers.splice(topLayers.indexOf(this), 1);
};

// `:modal` and `:popover-open`, which happy-dom's selectors leave out: an open
// dialog in the top layer, and a popover shown there.
const matches = Element.prototype.matches;
Element.prototype.matches = function (selector) {
  if (selector === ":modal") return modals.includes(this);
  if (selector === ":popover-open") {
    drop();
    return topLayers.includes(this) && this.hasAttribute("popover");
  }
  return matches.call(this, selector);
};

// The box a browser places `element` against, which happy-dom lays out none
// of: with `position-area` set, the nearest ancestor whose `anchor-name` is
// the one its `position-anchor` names; otherwise `null`.
function placedAgainst(element) {
  const s = getComputedStyle(element);
  if (unsetStyle(s.positionArea) || unsetStyle(s.positionAnchor)) return null;
  for (let at = element.parentElement; at !== null; at = at.parentElement) {
    if (getComputedStyle(at).anchorName === s.positionAnchor) return at;
  }
  return null;
}
const unsetStyle = (value) => value === undefined || value === "" || value === "none" || value === "auto";

// The `display` a browser gives `element`. happy-dom drops every value it
// can't parse, `table-cell` and `-webkit-box` among them, so this reads the
// page's `<style>` text itself: the weightiest matching rule that says
// `display`, the later of two that weigh the same, else a browser's own.
function display(element) {
  const inline = /(?:^|;)\s*display\s*:\s*([^;]+)/.exec(element.getAttribute("style") ?? "");
  if (inline !== null) return inline[1].trim();
  let best = null;
  for (const rule of displayRules()) {
    if (rule.media !== null && !matchMedia(rule.media).matches) continue;
    for (const selector of rule.selectors) {
      if (!element.matches(selector)) continue;
      const weight = [rule.important ? 1 : 0, ...specificity(selector)];
      if (best === null || weight.reduce((d, w, i) => d || w - best.weight[i], 0) >= 0) {
        best = { weight, value: rule.value };
      }
    }
  }
  if (best !== null) return best.value;
  if (element.hidden) return "none";
  const name = element.localName;
  if (name === "dialog") return element.open ? "block" : "none";
  return UA_DISPLAY[name] ?? (BLOCKS.includes(name) ? "block" : "inline");
}
const UA_DISPLAY = {
  table: "table",
  tbody: "table-row-group",
  thead: "table-header-group",
  tfoot: "table-footer-group",
  tr: "table-row",
  td: "table-cell",
  th: "table-cell",
  li: "list-item",
  input: "inline-block",
  button: "inline-block",
  select: "inline-block",
  textarea: "inline-block",
  head: "none",
  script: "none",
  style: "none",
  template: "none",
};
const BLOCKS = ["html", "body", "div", "p", "nav", "main", "header", "footer", "aside", "article", "section",
  "search", "ul", "ol", "hr", "form", "h1", "h2", "h3", "h4", "h5", "h6", "details", "summary", "fieldset"];
// Each rule in the page's sheets that says `display`, in the order they apply.
function displayRules() {
  const rules = [];
  for (const style of document.querySelectorAll("style")) {
    gather(style.textContent.replace(/\/\*[\s\S]*?\*\//g, ""), null, rules);
  }
  return rules;
}
function gather(text, media, rules) {
  let at = 0;
  while (true) {
    const open = text.indexOf("{", at);
    if (open < 0) return;
    let end = open + 1;
    for (let depth = 1; depth > 0 && end < text.length; end++) {
      if (text[end] === "{") depth++;
      else if (text[end] === "}") depth--;
    }
    const head = text.slice(at, open).trim();
    const inner = text.slice(open + 1, end - 1);
    if (head.startsWith("@media")) gather(inner, head.slice(6).trim(), rules);
    else if (!head.startsWith("@")) {
      const said = [...inner.matchAll(/(?:^|;)\s*display\s*:\s*([^;!]+)(!important)?/g)].at(-1);
      if (said !== undefined) {
        // A pseudo-element is no element `matches` can answer for.
        const selectors = topLevel(head).filter((selector) => !selector.includes("::"));
        rules.push({ selectors, media, value: said[1].trim(), important: said[2] !== undefined });
      }
    }
    at = end;
  }
}
// A selector list split at the commas outside any parentheses.
function topLevel(list) {
  const out = [];
  let depth = 0;
  let from = 0;
  for (let i = 0; i < list.length; i++) {
    if (list[i] === "(") depth++;
    else if (list[i] === ")") depth--;
    else if (list[i] === "," && depth === 0) {
      out.push(list.slice(from, i).trim());
      from = i + 1;
    }
  }
  out.push(list.slice(from).trim());
  return out;
}
// What a selector weighs: identifiers, then classes, attributes and
// pseudo-classes, then elements and pseudo-elements. `:where()` weighs nothing,
// and `:not()`, `:is()` and `:has()` weigh what they hold.
function specificity(selector) {
  let s = selector;
  for (let at = s.indexOf(":where("); at >= 0; at = s.indexOf(":where(")) {
    let end = at + 7;
    for (let depth = 1; depth > 0; end++) {
      if (s[end] === "(") depth++;
      else if (s[end] === ")") depth--;
    }
    s = s.slice(0, at) + " " + s.slice(end);
  }
  s = s.replace(/:(?:not|is|has)\(/g, " ").replace(/\)/g, " ");
  const count = (pattern) => (s.match(pattern) ?? []).length;
  const ids = count(/#[\w-]+/g);
  const classes = count(/\.[\w-]+|\[[^\]]*\]|(?<!:):[\w-]+/g);
  const elements = count(/(?:^|[\s>+~])[a-z][\w-]*|::[\w-]+/gi);
  return [ids, classes, elements];
}

// The boxes a browser lays out for `container`'s children, each the nodes it
// holds, in order. A child is a box of its own where the container's layout
// makes it one: always in a flex or a grid container, where it is a table
// entry in a table row, where it is a row in a table, and where it isn't
// inline in a block. A run of the other children shares one box, which is a
// run of text in a flex container, a line in a block, an anonymous entry in a
// table row and an anonymous row in a table. In a block, a run of table
// entries and rows shares one box too: the anonymous table a browser wraps
// them in. Comments, empty text, what shows nothing and what is in the top
// layer, which is laid out over the page, are no box, and the children of a
// `display: contents` element are laid out as its parent's.
function boxes(container) {
  drop();
  const outer = display(container);
  const children = (n) =>
    [...n.childNodes].flatMap((c) => (c.nodeType === 1 && display(c) === "contents" ? children(c) : [c]));
  const flexible = /flex|grid|box/.test(outer);
  const tabular = outer === "table-row" || /^(inline-)?table$|^table-.*group$/.test(outer);
  const own = flexible
    ? () => true
    : outer === "table-row"
      ? (d) => d === "table-cell"
      : tabular
        ? (d) => /^table-(row|.*group|caption)$/.test(d)
        : (d) => !d.startsWith("inline") && !d.startsWith("table-");
  // Whether a child of that display goes in an anonymous table.
  const part = (d) => !flexible && !tabular && d.startsWith("table-");
  const out = [];
  let run = null;
  let parts = false;
  for (const node of children(container)) {
    let d = "";
    if (node.nodeType === 1) {
      if (topLayers.includes(node)) continue;
      d = display(node);
      if (d === "none") continue;
      if (own(d)) {
        run = null;
        out.push([node]);
        continue;
      }
    } else if (node.nodeType !== 3 || node.data === "") continue;
    if (run === null || parts !== part(d)) out.push((run = []));
    parts = part(d);
    run.push(node);
  }
  return out;
}

// The box a browser measures the out-of-flow box `element` from, its
// containing block, by CSS Positioned Layout 3 and Anchor Positioning 1:
// `placedAgainst(element)` under a `position-area` that finds an anchor, and
// otherwise the nearest ancestor with a box that is positioned, transformed or
// backdrop-filtered. A fixed box skips the positioned ones. `null` is the
// viewport, which a box with none of these is measured from.
function containingBlock(element) {
  const against = placedAgainst(element);
  if (against !== null) return against;
  const fixed = getComputedStyle(element).position === "fixed";
  for (let at = element.parentElement; at !== null && at !== document.documentElement; at = at.parentElement) {
    if (display(at) === "contents") continue;
    const s = getComputedStyle(at);
    if (!unsetStyle(s.transform) || !unsetStyle(s.translate) || !unsetStyle(s.backdropFilter)) return at;
    if (!fixed && !unsetStyle(s.position) && s.position !== "static") return at;
  }
  return null;
}

// Where a browser lays out the out-of-flow box `element`, which happy-dom lays
// out none of: `{ x, y, width, height }` from the top left of its
// `containingBlock`, by CSS Positioned Layout 3 and Anchor Positioning 1.
//
// A `position-area` that finds an anchor turns every auto inset into 0, and its
// `center` area makes `normal` self-alignment centre the box: Chromium 143
// did so even with one inset auto (#360). The current draft aligns towards
// that inset instead, which no browser this was checked against does yet.
// Without one, `normal` is `start`, an auto inset leaves the box against
// its other one, and a box with both insets auto in an axis has a static
// position this doesn't lay out, so it throws. Lengths are pixels or
// percentages of the box placed against. A box with no width of its own
// fills a column's width, or the inset room when it stretches; one with no
// height is as tall as its children in the flow, or the inset room when it
// stretches. Left to right and top to bottom only.
function placedAt(element) {
  const s = getComputedStyle(element);
  const area = placedAgainst(element) !== null;
  const against = containingBlock(element);
  const room = (dimension) => (against === null ? viewport(dimension) : sizeOf(against, dimension));
  const [x, width] = placedAlong(element, "width", s.insetInlineStart, s.insetInlineEnd, s.justifySelf, room("width"), area);
  const [y, height] = placedAlong(element, "height", s.insetBlockStart, s.insetBlockEnd, s.alignSelf, room("height"), area);
  return { x, y, width, height };
}
const viewport = (dimension) => (dimension === "width" ? window.innerWidth : window.innerHeight);
// The start and the size of `element` along one axis, inside `room` pixels.
function placedAlong(element, dimension, start, end, align, room, area) {
  const from = lengthIn(start, () => room);
  const to = lengthIn(end, () => room);
  let size = lengthIn(getComputedStyle(element)[dimension], () => room);
  let self = (align ?? "").replace(/^(safe|unsafe) /, "");
  if (self === "" || self === "auto") self = "normal";
  if (["self-start", "flex-start"].includes(self)) self = "start";
  if (["self-end", "flex-end"].includes(self)) self = "end";
  if (!area && from === null && to === null) throw new Error(`placedAt: a static ${dimension} position`);
  let s = from ?? 0;
  let e = to ?? 0;
  // Never below zero: the weaker inset gives way, the auto one if only one
  // is auto and the end one otherwise.
  const over = s + e - room;
  if (over > 0 && from === null) s -= over;
  else if (over > 0) e -= over;
  const inside = room - s - e;
  const stretches = self === "stretch" || (self === "normal" && !area && from !== null && to !== null);
  if (size === null) size = stretches ? inside : flowSize(element, dimension);
  if (self === "stretch") self = "start";
  if (self === "normal") self = area ? "center" : "start";
  if (!area && from === null) return [room - e - size, size];
  if (!area && to === null) return [s, size];
  if (self === "start") return [s, size];
  if (self === "end") return [s + inside - size, size];
  if (self === "center") return [s + (inside - size) / 2, size];
  throw new Error(`placedAt: self-alignment ${align}`);
}
// A computed length in pixels: a percentage of `room()`, or `null` for auto.
function lengthIn(value, room) {
  if (unsetStyle(value)) return null;
  if (value.endsWith("%")) return (room() * Number(value.slice(0, -1))) / 100;
  if (value === "0" || value.endsWith("px")) return Number.parseFloat(value);
  throw new Error(`placedAt: the length ${value}`);
}
// The `dimension` of `element`'s box: where `placedAt` lays it out when it
// is out of the flow, else its own, else what it takes from the flow.
function sizeOf(element, dimension) {
  if (element.parentElement === null) return viewport(dimension);
  const s = getComputedStyle(element);
  if (["absolute", "fixed"].includes(s.position)) return placedAt(element)[dimension];
  const own = lengthIn(s[dimension], () => sizeOf(element.parentElement, dimension));
  if (own !== null) return own;
  return flowSize(element, dimension);
}
// The `dimension` a box with none of its own takes from the flow: a column's
// width, or the heights of its children in the flow added up, or the tallest
// of them in a table row, whose entries sit side by side.
function flowSize(element, dimension) {
  if (dimension === "width") return sizeOf(element.parentElement, "width");
  const row = display(element) === "table-row";
  let total = 0;
  for (const child of element.children) {
    const c = getComputedStyle(child);
    if (["absolute", "fixed"].includes(c.position) || c.display === "none") continue;
    const height = sizeOf(child, "height");
    total = row ? Math.max(total, height) : total + height;
  }
  return total;
}

// The inertness a modal dialog gives the rest of the page, which happy-dom
// leaves out: nothing outside the topmost one takes the focus. Nor does an
// element a browser draws none of.
for (const prototype of [HTMLElement.prototype, SVGElement.prototype]) {
  const focus = prototype.focus;
  prototype.focus = function (options) {
    const top = modals.findLast((d) => d.isConnected && d.open);
    if ((top === undefined || top.contains(this)) && rendered(this)) focus.call(this, options);
  };
}

// A document a server sent, `html`, as a browser parses it: the head and the
// body, and each inline classic script in the body run in order, as the parser
// reaches it. happy-dom runs no script markup brings.
function parse(html) {
  document.head.innerHTML = html.split("<head>")[1].split("</head>")[0];
  body.innerHTML = html.split("<body>")[1].split("</body>")[0];
  for (const script of body.querySelectorAll("script:not([type]):not([src])")) {
    (0, eval)(script.textContent);
  }
}

// Implicit submission: the form's first submit button is clicked unless it is
// disabled; a form with none is submitted from its one line field.
function submitImplicitly(at) {
  const form = at.form;
  const fallback = [...form.elements].find((e) => e.type === "submit");
  if (fallback !== undefined) {
    if (!fallback.disabled) fallback.click();
    return;
  }
  const lines = [...form.elements].filter((e) => e.tagName === "INPUT" && LINE_FIELDS.includes(e.type));
  if (lines.length === 1 && lines[0] === at) form.requestSubmit();
}

// The address bar's moves, as the program asked for them.
const moves = [];
for (const [method, word] of [["pushState", "pushed"], ["replaceState", "replaced"]]) {
  const move = history[method].bind(history);
  history[method] = (state, title, url) => {
    move(state, title, url);
    moves.push(`${word} ${url}`);
  };
}
"##;
