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
///    `containingBlock`, by the inset, size and self-alignment it computes to,
///    and under a `position-area` that spans past its anchor, by the area,
///    the room the viewport leaves and the first `position-try-fallbacks`
///    flip that fits, slid back on the page where its self-alignment lets it;
///  * `offsetOf(element, dimension)`, where a browser lays out a box in the
///    flow from the viewport's top left, which happy-dom leaves out;
///  * `clippedBy(element)`, the ancestors whose `overflow` cuts `element`,
///    nearest first, which happy-dom paints none of;
///  * `movedBy(element)`, how far the translates a browser draws `element`
///    with move it from where the layout put it, which happy-dom draws none
///    of;
///  * nothing a browser draws none of taking the focus: what is inside a shut
///    dialog, a shut disclosure's body or a `hidden` element, and the focus
///    leaving a shut dialog that gives it back to such an element;
///  * nothing a style gives `display: none` taking the focus either;
///  * `resize(width, height)`, which makes the window that wide, and that tall
///    where a height is given, and restyles the page
///    for it, which happy-dom leaves out, before any of the page's `resize`
///    listeners run;
///  * `display(element)`, the `display` a browser gives `element`, from the
///    page's sheets, where happy-dom drops the values it can't parse;
///  * `boxes(container)`, the boxes a browser lays out for `container`'s
///    children, each the nodes it holds, in the order a flex or grid
///    container places them by `order`, which happy-dom lays out none of;
///  * `parse(html)`, which lays out a document a server sent as a browser
///    parses it, running its inline scripts;
///  * `moves`, every move the address bar made, in order;
///  * Back and Forward scrolling the window to where it was when the reader
///    left that entry, which happy-dom leaves out;
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
// The window `width` CSS pixels across, and `height` down, as a reader
// resizing it leaves it.
const resize = (width, height = innerHeight) => happyDOM.setViewport({ width, height });

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
// holds, in the order it places them. A child is a box of its own where the
// container's layout makes it one: always in a flex or a grid container,
// where it is a table entry in a table row, where it is a row in a table, and
// where it isn't inline in a block. A run of the other children shares one box, which is a
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
  // A flex or grid container places its items by `order`, lowest first, and
  // items of one order in the tree's. A run of text is an item of order 0.
  if (flexible) {
    const order = ([n]) => (n.nodeType === 1 ? Number(getComputedStyle(n).order) || 0 : 0);
    out.sort((a, b) => order(a) - order(b));
  }
  return out;
}

// The box a browser measures the out-of-flow box `element` from, its
// containing block, by CSS Positioned Layout 3 and Anchor Positioning 1:
// `placedAgainst(element)` under a `position-area` that finds an anchor, and
// otherwise the nearest ancestor with a box that is positioned, transformed or
// backdrop-filtered. A fixed box skips the positioned ones. `null` is the
// viewport, which a box with none of these is measured from, and so is a box
// in the top layer.
function containingBlock(element) {
  const against = placedAgainst(element);
  if (against !== null) return against;
  if (topLayers.includes(element)) return null;
  const fixed = getComputedStyle(element).position === "fixed";
  for (let at = element.parentElement; at !== null && at !== document.documentElement; at = at.parentElement) {
    if (display(at) === "contents") continue;
    const s = getComputedStyle(at);
    if (!unsetStyle(s.transform) || !unsetStyle(s.translate) || !unsetStyle(s.backdropFilter)) return at;
    if (!fixed && !unsetStyle(s.position) && s.position !== "static") return at;
  }
  return null;
}

// The ancestors whose `overflow` cuts `element` in a browser, nearest first,
// by CSS Overflow 3: those on its chain of containing blocks, which is its
// parent for a box in the flow and `containingBlock` for one out of it. A box
// between an out-of-flow box and its containing block cuts nothing, and
// nothing cuts a box in the top layer or one measured from the viewport.
function clippedBy(element) {
  const out = [];
  for (let at = element; at !== null && !topLayers.includes(at); ) {
    const position = getComputedStyle(at).position;
    at = ["absolute", "fixed"].includes(position) ? containingBlock(at) : at.parentElement;
    if (at === null || at === document.body || at === document.documentElement) break;
    const s = getComputedStyle(at);
    // happy-dom leaves the longhands empty under the `overflow` shorthand.
    if ([s.overflowX, s.overflowY].some((o) => (o || s.overflow || "visible") !== "visible")) out.push(at);
  }
  return out;
}

// How far a browser draws `element` from where the layout put it, `{ x, y }`,
// by CSS Transforms 1: its own `transform` and each ancestor's added up, each
// a `translate`, the one transform a page writes. A box in the top layer is
// moved by none of its ancestors' translates, and its `placedAt` is measured
// from its anchor's box as laid out, before any translate, so no translate on
// the anchor or around it moves it either. Safari 26 draws both that way. A
// percentage is of the moved box's own size.
function movedBy(element) {
  const moved = { x: 0, y: 0 };
  for (let at = element; at !== null && at !== document.documentElement; at = at.parentElement) {
    const value = getComputedStyle(at).transform;
    if (!unsetStyle(value)) {
      const parts = /^translate\(([^,]+),([^)]+)\)$/.exec(value.replace(/\s+/g, ""));
      if (parts === null) throw new Error(`movedBy: the transform ${value}`);
      moved.x += lengthIn(parts[1], () => sizeOf(at, "width"));
      moved.y += lengthIn(parts[2], () => sizeOf(at, "height"));
    }
    if (topLayers.includes(at)) break;
  }
  return moved;
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
// percentages of the box placed against: its border box under an area, else
// its padding box, which is where the offsets start too. A box with no width
// of its own shrinks to fit its content inside the inset room, or fills that
// room when it stretches; one with no height is as tall as its children in
// the flow, or the inset room when it stretches. A box with both insets set
// and a self-alignment that is neither `normal` nor marked `safe` or `unsafe`
// slides back into its inset room, or onto the viewport under an area, when it
// runs past it ([`shifted`]). Left to right and top to bottom only.
function placedAt(element) {
  const s = getComputedStyle(element);
  const anchor = placedAgainst(element);
  if (anchor !== null && !/^center( center)?$/.test(s.positionArea)) return placedInArea(element, anchor);
  const area = anchor !== null;
  const against = containingBlock(element);
  const room = (dimension) =>
    against === null ? viewport(dimension) : sizeOf(against, dimension) - (area ? 0 : borders(against, dimension));
  // What a box may slide inside besides its inset room: the viewport, from
  // the anchor's top left, or its containing block.
  const page = (dimension) =>
    area ? [-offsetOf(anchor, dimension), viewport(dimension) - offsetOf(anchor, dimension)] : [0, room(dimension)];
  const [x, width] = placedAlong(element, "width", s.insetInlineStart, s.insetInlineEnd, aligned(s.justifySelf), room("width"), area, page("width"));
  const [y, height] = placedAlong(element, "height", s.insetBlockStart, s.insetBlockEnd, aligned(s.alignSelf), room("height"), area, page("height"));
  return { x, y, width, height };
}
const viewport = (dimension) => (dimension === "width" ? window.innerWidth : window.innerHeight);
// A self-alignment as Chromium reads it. happy-dom resolves `var(--v,) start`
// to `initial start` when `--v` is `initial`, where the empty fallback
// should apply.
const aligned = (value) => (value ?? "").replace(/^initial /, "");
// The start and the size of `element` along one axis, inside `room` pixels,
// slid inside `page` too where `shifts` says.
function placedAlong(element, dimension, start, end, align, room, area, page) {
  const from = lengthIn(start, () => room);
  const to = lengthIn(end, () => room);
  let size = lengthIn(getComputedStyle(element)[dimension], () => room);
  let self = (align ?? "").replace(/^(safe|unsafe) /, "");
  if (self === "" || self === "auto") self = "normal";
  if (["self-start", "flex-start"].includes(self)) self = "start";
  if (["self-end", "flex-end"].includes(self)) self = "end";
  const shift = shifts(align, self) && (area || (from !== null && to !== null));
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
  if (size === null && stretches) size = inside;
  if (size === null && dimension === "width") {
    const [least, most] = intrinsicWidths(element);
    size = Math.min(most, Math.max(least, inside));
  }
  if (size === null) size = flowSize(element, dimension);
  if (self === "stretch") self = "start";
  if (self === "normal") self = area ? "center" : "start";
  if (!area && from === null) return [room - e - size, size];
  if (!area && to === null) return [s, size];
  let at;
  if (self === "start") at = s;
  else if (self === "end") at = s + inside - size;
  else if (self === "center") at = s + (inside - size) / 2;
  else throw new Error(`placedAt: self-alignment ${align}`);
  return [shift ? shifted(at, size, [s, s + inside], page) : at, size];
}
// Whether a box with both insets set slides back inside its containing block
// when it runs out of its inset room, by CSS Box Alignment 3: under any
// self-alignment `self` but `normal`, unless `align` says `safe` or `unsafe`.
const shifts = (align, self) => !/^(safe|unsafe) /.test(align ?? "") && self !== "normal";
// Where a box `size` long that starts at `at` slides to, by CSS Box Alignment
// 3 §4.4.1.2 as Chromium does it: inside `room`, its inset room, where it fits
// there, and otherwise inside `room` and `page` together, start edge first.
function shifted(at, size, room, page) {
  const [low, high] = size <= room[1] - room[0] ? room : [Math.min(room[0], page[0]), Math.max(room[1], page[1])];
  return Math.max(Math.min(at, high - size), low);
}
// Where a browser lays out `element` under a `position-area` other than
// `center`, as a `Fallback` or a `FitRoom` places a panel, by Anchor
// Positioning 1, from the top left of its anchor's box. Each axis is placed
// in an area of the viewport: from the anchor's start edge to the viewport's
// end edge under `span-*-end`, the other way under `span-*-start`, the whole
// axis under `span-all`, and the anchor's box under `center`. Its insets,
// auto ones as 0, leave the room the box must fit in, and an `anchor-center`
// box is centred on the anchor, then slid back as [`shifts`] says, before the
// fit is checked. The box takes the first of its own place and
// each `position-try-fallbacks` flip that fits — ordered by that room where
// `position-try-order` says so — or its own place where none does. A flip
// mirrors the axis's insets, area and self-alignment. The anchor is laid out
// in the flow ([`offsetOf`]).
function placedInArea(element, anchor) {
  const s = getComputedStyle(element);
  const [down, across = down] = s.positionArea.split(" ");
  const sizes = { width: sizeOf(anchor, "width"), height: sizeOf(anchor, "height") };
  const tries = unsetStyle(s.positionTryFallbacks) ? [] : s.positionTryFallbacks.split(",");
  const place = (tactics) => {
    const flips = tactics.trim().split(/\s+/);
    const along = (dimension, axis, area, start, end, align, max) => {
      const mirror = { [`span-${axis}-start`]: `span-${axis}-end`, [`span-${axis}-end`]: `span-${axis}-start` };
      let self = (align ?? "").replace(/^(safe|unsafe) /, "");
      const shift = shifts(align, self);
      if (flips.includes(`flip-${axis}`)) {
        [start, end] = [end, start];
        area = mirror[area] ?? area;
        self = { start: "end", end: "start" }[self] ?? self;
      }
      const at = offsetOf(anchor, dimension);
      const length = sizes[dimension];
      const far = viewport(dimension);
      const [low, high] = {
        center: [at, at + length],
        "span-all": [0, far],
        [`span-${axis}-start`]: [0, at + length],
        [`span-${axis}-end`]: [at, far],
      }[area] ?? [null, null];
      if (low === null) throw new Error(`placedAt: the area ${area}`);
      const room = high - low;
      const from = lengthIn(start, () => room, sizes) ?? 0;
      const to = lengthIn(end, () => room, sizes) ?? 0;
      const inside = room - from - to;
      let size = lengthIn(getComputedStyle(element)[dimension], () => room, sizes);
      if (size === null && self === "stretch") size = inside;
      if (size === null && dimension === "width") {
        const [least, most] = intrinsicWidths(element);
        size = Math.min(most, Math.max(least, inside));
      }
      if (size === null) size = flowSize(element, dimension);
      size = Math.min(size, lengthIn(max, () => room, sizes) ?? Infinity);
      let offset;
      if (self === "start" || self === "stretch") offset = from;
      else if (self === "end") offset = room - to - size;
      else if (self === "anchor-center") offset = at + (length - size) / 2 - low;
      else throw new Error(`placedAt: self-alignment ${align} in an area`);
      if (shift) offset = shifted(offset, size, [from, room - to], [-low, far - low]);
      const fits = offset >= from - 0.5 && offset + size <= room - to + 0.5;
      return { at: low + offset - at, size, fits, inside };
    };
    return {
      x: along("width", "inline", across, s.insetInlineStart, s.insetInlineEnd, aligned(s.justifySelf), s.maxWidth),
      y: along("height", "block", down, s.insetBlockStart, s.insetBlockEnd, aligned(s.alignSelf), s.maxHeight),
    };
  };
  const options = ["", ...tries].map(place);
  const order = { "most-inline-size": "x", "most-block-size": "y" }[s.positionTryOrder];
  if (order !== undefined) options.sort((a, b) => b[order].inside - a[order].inside);
  const { x, y } = options.find((o) => o.x.fits && o.y.fits) ?? place("");
  return { x: x.at, y: y.at, width: x.size, height: y.size };
}
// Where `element`'s border box starts along `dimension` from the viewport's
// top left, for a box in the flow: its parent's start, padding and border,
// then where `flexLines` places it in a flex container, or below every box in
// the flow before it otherwise, then its own margin.
function offsetOf(element, dimension) {
  const parent = element.parentElement;
  if (parent === null) return 0;
  const s = getComputedStyle(element);
  if (["absolute", "fixed"].includes(s.position)) throw new Error("offsetOf: a box out of the flow");
  const side = dimension === "width" ? "Left" : "Top";
  const p = getComputedStyle(parent);
  const basis = () => contentOf(parent, "width");
  let at = offsetOf(parent, dimension) + (lengthIn(paddingSide(p, side), basis) ?? 0);
  if (!unsetStyle(p[`border${side}Style`]) && p[`border${side}Style`] !== "hidden") {
    at += lengthIn(p[`border${side}Width`], () => 0) ?? 0;
  }
  const flex = /flex/.test(display(parent));
  if (flex) {
    at += flexLines(parent).places.get(element)[dimension === "width" ? "x" : "y"];
  } else if (dimension === "height") {
    for (const sibling of inFlow(parent)) {
      if (sibling === element) break;
      at += sizeOf(sibling, dimension);
    }
  }
  return at + (lengthIn(s[`margin${side}`], basis) ?? 0);
}
// A computed length in pixels: a percentage of `room()`, or `null` for auto.
// With `anchor`, the anchor's `{ width, height }`, it reads a `calc()` of
// lengths added and taken away too, each a length, a `calc()` or a fraction
// of the anchor's size, as `FitRoom` and a `Fallback`'s pins write them. A
// custom property left `initial` is a guaranteed-invalid value, which a
// browser reads as `auto` for an inset and as its fallback inside a `var()`;
// happy-dom reads it as the word, so this reads it as auto, or 0 in a sum.
function lengthIn(value, room, anchor = null) {
  if (unsetStyle(value) || value === "initial") return null;
  if (value.endsWith("%")) return (room() * Number(value.slice(0, -1))) / 100;
  if (value === "0" || value.endsWith("px")) return Number.parseFloat(value);
  const sized = /^calc\(anchor-size\((width|height), 100%\) \* ([\d.]+) \/ 100\)$/.exec(value);
  if (anchor !== null && sized !== null) return (anchor[sized[1]] * Number(sized[2])) / 100;
  if (anchor !== null && value.startsWith("calc(") && value.endsWith(")")) {
    let [sum, depth, term, sign] = [0, 0, "", 1];
    const add = () => {
      sum += sign * (lengthIn(term.trim(), room, anchor) ?? 0);
      term = "";
    };
    for (const token of value.slice(5, -1).split(/( [+-] |[()])/)) {
      if (depth === 0 && /^ [+-] $/.test(token)) {
        add();
        sign = token.trim() === "-" ? -1 : 1;
        continue;
      }
      if (token === "(") depth++;
      if (token === ")") depth--;
      term += token;
    }
    add();
    return sum;
  }
  throw new Error(`placedAt: the length ${value}`);
}
// The `dimension` of `element`'s border box: where `placedAt` lays it out
// when it is out of the flow, else its own, never less than its padding and
// border, else what it takes from the flow.
function sizeOf(element, dimension) {
  if (element.parentElement === null) return viewport(dimension);
  const s = getComputedStyle(element);
  if (["absolute", "fixed"].includes(s.position)) return placedAt(element)[dimension];
  const own = lengthIn(s[dimension], () => contentOf(element.parentElement, dimension));
  if (own !== null) return Math.max(own, padding(element, dimension) + borders(element, dimension));
  return flowSize(element, dimension);
}
// The `dimension` of `element`'s content box, which its children in the flow
// are laid out in.
function contentOf(element, dimension) {
  if (element.parentElement === null) return viewport(dimension);
  return sizeOf(element, dimension) - padding(element, dimension) - borders(element, dimension);
}
// The padding on both sides of `element` along `dimension`.
function padding(element, dimension) {
  const s = getComputedStyle(element);
  const basis = () => contentOf(element.parentElement, "width");
  return sides(dimension).reduce((sum, side) => sum + (lengthIn(paddingSide(s, side), basis) ?? 0), 0);
}
// The border on both sides of `element` along `dimension`: none on a side
// whose style is `none` or `hidden`, whatever its width says.
function borders(element, dimension) {
  const s = getComputedStyle(element);
  return sides(dimension).reduce((sum, side) => {
    const style = s[`border${side}Style`];
    if (unsetStyle(style) || style === "hidden") return sum;
    return sum + (lengthIn(s[`border${side}Width`], () => 0) ?? 0);
  }, 0);
}
const sides = (dimension) => (dimension === "width" ? ["Left", "Right"] : ["Top", "Bottom"]);
// The padding on one side of a computed style, `s`. happy-dom keeps the
// logical `padding-inline` and `padding-block` shorthands unexpanded, so the
// physical side reads what the reset set under them; a shorthand that is set
// is the class's, and wins: its first value starts the axis, its last ends it.
function paddingSide(s, side) {
  const axis = ["Left", "Right"].includes(side) ? "inline" : "block";
  const values = s.getPropertyValue(`padding-${axis}`).trim().split(/\s+/);
  if (values[0] === "") return s[`padding${side}`];
  return ["Left", "Top"].includes(side) ? values[0] : values[values.length - 1];
}
// The `dimension` a box with none of its own takes from the flow: its
// max-content width in a flex row, else (or when it holds text, which has no
// width here) its parent's content width, or the heights of its children in the flow added up, or the
// tallest of them in a table row, whose entries sit side by side, plus its own
// padding and border. A flex container is as tall as `flexLines` lays its
// lines out.
function flowSize(element, dimension) {
  const parent = element.parentElement;
  if (dimension === "width" && /flex/.test(display(parent)) && getComputedStyle(parent).flexDirection.startsWith("row")) {
    try {
      return intrinsicWidths(element)[1];
    } catch (error) {
      if (error.message !== "placedAt: the width of text") throw error;
    }
  }
  if (dimension === "width") return wrappedLine(parent) ?? contentOf(parent, "width");
  const s = getComputedStyle(element);
  if (/flex/.test(display(element))) {
    const lines = flexLines(element);
    const height = s.flexDirection.startsWith("column") ? lines.main : lines.cross;
    return height + padding(element, "height") + borders(element, "height");
  }
  const row = display(element) === "table-row";
  let total = 0;
  for (const child of inFlow(element)) {
    const height = sizeOf(child, "height");
    total = row ? Math.max(total, height) : total + height;
  }
  return total + padding(element, "height") + borders(element, "height");
}
// How wide the line is that `container`'s children stretch across, when it is
// a flex column that wraps, else `null`. Its height is never definite here, so
// it has one line, but a wrapping container sizes that line by its items: the
// widest fit-content width among them, never narrower than the container. A
// box holding text wraps the text to fit, so it doesn't widen the line.
function wrappedLine(container) {
  const s = getComputedStyle(container);
  if (!/flex/.test(display(container)) || !s.flexDirection.startsWith("column") || s.flexWrap !== "wrap") return null;
  const room = contentOf(container, "width");
  let line = room;
  for (const child of inFlow(container)) {
    if (child.textContent.trim() !== "") continue;
    const [least, most] = intrinsicWidths(child);
    line = Math.max(line, Math.min(most, Math.max(least, room)));
  }
  return line;
}
// How a flex container lays out its boxes in the flow, in lines:
// `{ places, main, cross }`, each box's `{ x, y }` from the top left of the
// container's content box, and how long and how deep its content box is.
//
// A container that wraps breaks a line before the box that would run past its
// main size, when that is definite (`definiteHeight` for a column), else past
// its maximum, else never. Its main size is that definite size, else its
// longest line held to its maximum. Lines stack across it in order, each as
// deep as its deepest box, or the one line of a container that doesn't wrap as
// deep as the container when that is definite, and a box sits at its line's start, centre or end
// as it aligns. A container that runs in reverse lays each line out from its
// main end. Each box keeps the size `sizeOf` gives it: nothing grows or
// shrinks.
function flexLines(container) {
  const s = getComputedStyle(container);
  const column = s.flexDirection.startsWith("column");
  const [main, cross] = column ? ["height", "width"] : ["width", "height"];
  const edges = (dimension) => padding(container, dimension) + borders(container, dimension);
  const [mainGap, crossGap] = (column ? ["row", "column"] : ["column", "row"]).map((axis) => gapOf(container, axis));
  const definite = column ? definiteHeight(container) : sizeOf(container, "width");
  const room = () => contentOf(container.parentElement, "height");
  const held = definiteHeight(container.parentElement) !== null;
  const maximum = column && (s.maxHeight.endsWith("%") ? held : true) ? lengthIn(s.maxHeight, room) : null;
  const inner = (outer, dimension = main) => (outer === null ? null : outer - edges(dimension));
  const limit = s.flexWrap !== "wrap" ? Infinity : (inner(definite) ?? inner(maximum) ?? Infinity);
  const lines = [];
  for (const item of inFlow(container)) {
    const box = { item, main: sizeOf(item, main), cross: sizeOf(item, cross) };
    const line = lines.at(-1);
    if (line === undefined || line.length + mainGap + box.main > limit) {
      lines.push({ boxes: [box], length: box.main, depth: box.cross });
    } else {
      line.boxes.push(box);
      line.length += mainGap + box.main;
      line.depth = Math.max(line.depth, box.cross);
    }
  }
  const depth = s.flexWrap === "wrap" ? null : inner(column ? sizeOf(container, "width") : definiteHeight(container), cross);
  if (lines.length === 1 && depth !== null) lines[0].depth = depth;
  const longest = Math.max(0, ...lines.map((line) => line.length));
  const length = inner(definite) ?? Math.min(longest, inner(maximum) ?? Infinity);
  const places = new Map();
  let across = 0;
  for (const line of lines) {
    let along = 0;
    for (const box of line.boxes) {
      const align = getComputedStyle(box.item).alignSelf;
      const own = unsetStyle(align) ? s.alignItems : align;
      const free = line.depth - box.cross;
      const shift = /end/.test(own) ? free : own === "center" ? free / 2 : 0;
      const start = s.flexDirection.endsWith("reverse") ? length - along - box.main : along;
      places.set(box.item, column ? { x: across + shift, y: start } : { x: start, y: across + shift });
      along += box.main + mainGap;
    }
    across += line.depth + crossGap;
  }
  return { places, main: length, cross: Math.max(0, across - crossGap) };
}
// The height of `element`'s border box when a browser knows it before laying
// out its content, else `null`: a length of its own; a percentage of a parent
// whose height is definite; stretched across a flex row of definite height
// that doesn't wrap; or the one box that grows in a flex column of definite
// height, with a minimum height of its own or its overflow cut, which takes
// the room the rest leave. The viewport's is definite; the page's isn't.
function definiteHeight(element) {
  const parent = element.parentElement;
  if (parent === null) return viewport("height");
  const s = getComputedStyle(element);
  if (["absolute", "fixed"].includes(s.position)) return null;
  if (s.height.endsWith("px")) return sizeOf(element, "height");
  const held = definiteHeight(parent);
  if (held === null) return null;
  if (s.height.endsWith("%")) return sizeOf(element, "height");
  if (!unsetStyle(s.height) || !/flex/.test(display(parent))) return null;
  const p = getComputedStyle(parent);
  const room = held - padding(parent, "height") - borders(parent, "height");
  const align = unsetStyle(s.alignSelf) ? p.alignItems : s.alignSelf;
  if (p.flexDirection.startsWith("row")) {
    return p.flexWrap !== "wrap" && (unsetStyle(align) || ["normal", "stretch"].includes(align)) ? room : null;
  }
  const grows = (box) => Number(getComputedStyle(box).flexGrow) > 0;
  const others = inFlow(parent).filter((box) => box !== element);
  const kept = !unsetStyle(s.minHeight) || ["hidden", "clip", "scroll", "auto"].includes(s.overflowY);
  if (!grows(element) || !kept || others.some(grows)) return null;
  const gap = gapOf(parent, "row");
  return room - others.reduce((sum, box) => sum + sizeOf(box, "height") + gap, 0);
}
// The gap between `element`'s rows (`axis` "row") or its columns ("column"),
// a percentage of `room()`. happy-dom keeps the `gap` shorthand whole, its
// row gap then its column gap, so this reads it there too.
function gapOf(element, axis, room = () => contentOf(element, "width")) {
  const s = getComputedStyle(element);
  const [rows, columns = rows] = (s.gap ?? "").split(/\s+/);
  const value = s[`${axis}Gap`] || (axis === "row" ? rows : columns);
  return unsetStyle(value) || value === "normal" ? 0 : (lengthIn(value, room) ?? 0);
}
// `element`'s children in the flow: neither out of it nor shown as nothing.
const inFlow = (element) =>
  [...element.children].filter((child) => {
    const c = getComputedStyle(child);
    return !["absolute", "fixed"].includes(c.position) && c.display !== "none";
  });
// The narrowest and the widest `element`'s border box gets without its
// content overflowing, `[min-content, max-content]`: its own width, or what
// its children in the flow need, side by side in a flex row and one above
// another otherwise. A row that wraps needs only its widest child at the
// narrowest. A field is as wide as `editorWidth` says. Text has no width
// here, so it throws, except a textarea's, which is its value.
//
// Each box inside the one being fitted (`inside`) is held between its minimum
// and maximum width. A percentage there would be of a width that depends on
// it, so CSS Sizing 3 counts it as none while the box is fitted: a width or a
// maximum as `auto`, and a minimum or a padding as zero. Safari 26 fits a box
// that way, and only then resolves each percentage against the width it got.
function intrinsicWidths(element, inside = false) {
  const s = getComputedStyle(element);
  const length = (value, none) =>
    inside && value.endsWith("%") ? none : lengthIn(value, () => contentOf(element.parentElement, "width"));
  const clamp = (width) =>
    inside ? Math.min(length(s.maxWidth, null) ?? Infinity, Math.max(length(s.minWidth, 0) ?? 0, width)) : width;
  const own = length(s.width, null);
  if (own !== null) return [clamp(own), clamp(own)];
  const area = element.localName === "textarea";
  if (!area && [...element.childNodes].some((n) => n.nodeType === 3 && n.data.trim() !== "")) {
    throw new Error("placedAt: the width of text");
  }
  const row = /flex/.test(display(element)) && s.flexDirection === "row";
  const wraps = s.flexWrap === "wrap";
  const children = inFlow(element);
  const gaps = row ? gapOf(element, "column", () => 0) * Math.max(0, children.length - 1) : 0;
  const typed = ["input", "textarea"].includes(element.localName) ? editorWidth(element) : 0;
  let [least, most] = [typed, typed];
  for (const child of children) {
    const [l, m] = intrinsicWidths(child, true);
    least = row && !wraps ? least + l : Math.max(least, l);
    most = row ? most + m : Math.max(most, m);
  }
  const pad = sides("width").reduce((sum, side) => sum + (length(paddingSide(s, side), 0) ?? 0), 0);
  const edges = pad + borders(element, "width");
  return [clamp(least + (row && !wraps ? gaps : 0) + edges), clamp(most + gaps + edges)];
}
// How wide a browser makes the editor in a field, `element`, an `<input>` or a
// `<textarea>`: Chromium's `TextFieldIntrinsicInlineSize`, `size` average
// characters (20 unless it says), or `TextAreaIntrinsicInlineSize`, `cols` (20
// unless it says). The average is the face's, rounded only where that rounds
// it up, and an `<input>` adds what the face's glyph box runs past one,
// rounded. The face is the one the painter sets the field in, with its OS/2
// average and its head glyph box in font units, of 2048 per em.
function editorWidth(element) {
  const s = getComputedStyle(element);
  const bold = Number(s.fontWeight) > 500 || s.fontWeight === "bold";
  const [average, widest] = /monospace/.test(s.fontFamily) ? [1233, 2612]
    : s.fontStyle === "italic" ? [1042, 2845] : bold ? [1095, 2934] : [1073, 2893];
  const size = (lengthIn(s.fontSize, () => 0) ?? 16) / 2048;
  const each = Math.max(average * size, Math.round(average * size));
  if (element.localName === "textarea") return Math.ceil(each * (Number(element.getAttribute("cols")) || 20));
  const count = Number(element.getAttribute("size")) || 20;
  return Math.ceil(each * (count - 1) + Math.round(widest * size));
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

// Scroll restoration, which happy-dom leaves out: each history entry keeps
// where the window was scrolled when the reader left it, and going back or
// forward to it scrolls there once `popstate` has run.
const scrolled = [null];
let entry = 0;
const pushState = history.pushState.bind(history);
history.pushState = (state, title, url) => {
  scrolled[entry] = [scrollX, scrollY];
  pushState(state, title, url);
  scrolled.splice(++entry, Infinity, null);
};
for (const [method, steps] of [["back", () => -1], ["forward", () => 1], ["go", (delta) => delta]]) {
  const traverse = history[method].bind(history);
  history[method] = (delta) => {
    const to = entry + steps(delta);
    if (to === entry || scrolled[to] === undefined) return traverse(delta);
    scrolled[entry] = [scrollX, scrollY];
    entry = to;
    traverse(delta);
    if (history.scrollRestoration === "auto" && scrolled[to] !== null) scrollTo(...scrolled[to]);
  };
}
"##;
