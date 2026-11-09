//! A document a page built for `web` can mount into, for the suites that
//! import one under `js_runtime()`.
//!
//! The double is small on purpose. It bubbles what a browser bubbles, keeps
//! every listener a type has, and moves `activeElement` with the four events
//! that come with it. Everything else a page could ask of a browser, it
//! doesn't have.

/// A document a page can mount into, with the events and the focus a reader
/// drives. `showing()` is the body's markup; `fire(target, type, fields)`
/// dispatches an event; `press(key)` is a keydown at the focused element;
/// `moves` lists every move the address bar made.
pub const BROWSER: &str = r##"
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
    // A press with nothing behind it, as a file input's `click()` is: there is
    // no picker to open.
    click() {
      fire(this, "click");
    },
    // A `<dialog>`'s own pair: `open` is the attribute the element reflects.
    open: false,
    showModal() {
      this.open = true;
      this.attributes.open = "";
    },
    close() {
      this.open = false;
      delete this.attributes.open;
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
