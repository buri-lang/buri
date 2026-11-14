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
///  * nothing a browser draws none of taking the focus: what is inside a shut
///    dialog, a shut disclosure's body or a `hidden` element, and the focus
///    leaving a shut dialog that gives it back to such an element;
///  * nothing a style gives `display: none` taking the focus either;
///  * `resize(width)`, which makes the window that wide and restyles the page
///    for it, which happy-dom leaves out, before any of the page's `resize`
///    listeners run;
///  * `parse(html)`, which lays out a document a server sent as a browser
///    parses it, running its inline scripts;
///  * `moves`, every move the address bar made, in order.
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
// the track or a step when that is more, Home and End to either end. The value
// stays on the track and on a step from its start. With `step="any"` an arrow
// moves a hundredth of the track and nothing snaps, as Chromium's range input
// does; a step that is missing or not above nought is the default of one.
function slide(at, key) {
  const min = Number(at.min || 0);
  const max = Number(at.max || 100);
  const any = at.step.toLowerCase() === "any";
  const step = any ? (max - min) / 100 : Number(at.step) > 0 ? Number(at.step) : 1;
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
  const top = min + Math.floor((max - min) / step) * step;
  at.value = String(
    any
      ? Math.max(Math.min(to, max), min)
      : Math.min(Math.max(min + Math.round((to - min) / step) * step, min), top),
  );
  at.dispatchEvent(new InputEvent("input", { bubbles: true }));
  at.dispatchEvent(new Event("change", { bubbles: true }));
}

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
