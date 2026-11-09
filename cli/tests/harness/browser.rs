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
///  * `press(key)`, a keydown at the focused element;
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

const body = document.body;
const showing = () => body.innerHTML.replace(/<!--.*?-->/gs, "");
const labelled = (label) => document.querySelector(`[aria-label=${JSON.stringify(label)}]`);

// The event a browser fires for each type a test causes: its interface, whether
// it bubbles, and whether a listener can cancel it.
const FIRED = {
  pointerdown: [PointerEvent, true, true],
  pointerup: [PointerEvent, true, true],
  pointerenter: [PointerEvent, false, false],
  pointerleave: [PointerEvent, false, false],
  mousedown: [MouseEvent, true, true],
  mouseup: [MouseEvent, true, true],
  mouseenter: [MouseEvent, false, false],
  mouseleave: [MouseEvent, false, false],
  keydown: [KeyboardEvent, true, true],
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

// A keydown where the focus is: the body when nothing has it.
const press = (key) => fire(document.activeElement, "keydown", { key });

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
