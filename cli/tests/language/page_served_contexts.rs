//! **A row a server renders gets the server's context.**
//!
//! A row builder takes a context, and a row may make a signal with it. A
//! server render once handed every builder `null` in its place, so a page
//! whose rows make signals crashed on the server with `TypeError: null is not
//! an object`. Every builder a render reaches gets the context the server
//! renders with: a list's rows wherever the list sits, a rebuilt region, and a
//! drawing's rows.
//!
//! Each test builds a page whose rows each make a signal from their context.
//! It reads the document the server sends as a browser parses it, then the page
//! that takes it over, then a press on a row, which writes that row's signal,
//! then "Change", which adds a row. It runs the same driver on the page mounted
//! with no server, and the two must say the same, the markup included.

use super::pages::{run_served, served};
use crate::harness::browser::browser;

/// Prints what the rows in the column beside "Change" say, and presses "a", or
/// focuses it in a drawing.
const DRIVER: &str = r#"
const mode = process.argv[2];
const tick = () => new Promise((done) => setTimeout(done, 0));
const page = () => import("./.buri/out/web/cmd/page/main.mjs");
const column = () => labelled("Change").nextElementSibling;
const said = (b) => b.getAttribute("aria-label") ?? b.textContent;
const rows = () => [...column().querySelectorAll("button, rect")];
// The column's elements and text, each element's attributes in name order and
// its inline style as the browser reads it, since how they are written is
// nothing a reader sees.
const canon = (n) => {
  if (n.nodeType === 3) return n.textContent;
  if (n.nodeType !== 1) return "";
  const value = (a) => (a.name === "style" ? n.style.cssText : a.value);
  const attributes = [...n.attributes].map((a) => ` ${a.name}="${value(a)}"`).sort().join("");
  return `<${n.localName}${attributes}>${[...n.childNodes].map(canon).join("")}</${n.localName}>`;
};
const markup = () => [...column().childNodes].map(canon).join("");
let sent = null;
// The rows, and whether the markup is still what the server sent.
const show = () => {
  const changed = sent !== null && markup() !== sent;
  console.log(`${rows().map(said).join(", ")}${changed ? " (markup changed)" : ""}`);
};
if (mode === "served") {
  const { default: server } = await import("./.buri/out/platform/server/cmd/page/fetch.mjs");
  parse(await (await server.fetch(new Request("http://localhost/"))).text());
  sent = markup();
  show();
}
await page();
await tick();
show();
sent = null;
console.log(markup());
const a = rows().find((b) => said(b).startsWith("a "));
if (a.localName === "rect") a.focus();
else a.click();
await tick();
show();
labelled("Change").click();
await tick();
show();
"#;

/// A row: a button saying its name and whether it has been pressed, held in a
/// signal the row makes from its context.
const ROW: &str = r#"fn row<C: Ui>(ctx: C, name: Str): Node<C> {
    let pressed = signal(ctx, false);
    ui.button({
        label: .Computed(fn(s) => str.format(s, "${name} ${pressed.get(s)}")),
        styles: [],
        onPress: .Some(fn(c) => pressed.set(c, true)),
    })
}
"#;

/// A row per item of `items`.
const ROWS: &str = "ui.each(.Cell(items), fn(s) => s, fn(c, s, _i) => row(c, s))";

/// A page a server renders and the browser takes over: "Change", which adds
/// "c" to `items`, then a column holding `held`, where `ROWS` stands for a row
/// per item. `main` takes over the page a server sent and mounts it otherwise.
fn served_page(held: &str) -> String {
    let held = held.replace("ROWS", ROWS);
    format!(
        r#"from "core/net/http" import * as http;
from "core/str" import * as str;
from "platform/effect" import {{ Allocator, Request, Response, Ui }};
from "ui/node" import * as ui;
from "ui/node" import {{ Node }};
from "ui/shape" import * as shape;
from "ui/signal" import {{ signal }};
from "ui/web" import * as web;
from "web" import {{ WebHost }};
from "//platform/server" import {{ ServerHost }};

{ROW}
fn page<C: Ui>(ctx: C): Node<C> {{
    let items = signal(ctx, ["a", "b"]);
    let shown = signal(ctx, true);
    ui.stack({{
        styles: [],
        children: [
            ui.button({{
                label: .Const("Change"),
                styles: [],
                onPress: .Some(fn(c) => items.set(c, ["a", "b", "c"])),
            }}),
            ui.stack({{ styles: [.Layout(.Column)], children: [{held}] }}),
        ],
    }})
}}

export fn main(host: WebHost): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
    }};
    match (web.state(ctx)) {{
        .Some(_sent) => web.resume(ctx, page(ctx)),
        .None => ui.mount(ctx, page(ctx), []),
    }}
}}

export fn fetch(host: ServerHost, request: Request): Response {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
    }};
    http.html(ctx, web.shell(ctx, request.path(), web.defaultDocument(), web.render(ctx, page(ctx)), .Null))
}}
"#
    )
}

/// What the served page says, after checking that the mounted page says the
/// same once it is running.
fn rows_say(name: &str, held: &str) -> String {
    let scratch = served(name, &served_page(held));
    let path = scratch.write("drive.mjs", &format!("{}\n{DRIVER}", browser()));
    let served = run_served(&path, "served");
    let mounted = run_served(&path, "mounted");
    let taken_over = served.split_once('\n').map_or("", |(_, rest)| rest);
    assert_eq!(
        taken_over, mounted,
        "the page a server sent differs from the page mounted"
    );
    served
}

/// The two rows as the page holds them once it takes over.
const BUTTONS: &str = "<button aria-label=\"a false\" type=\"button\">a false</button><button aria-label=\"b false\" type=\"button\">b false</button>";

/// The rows say this on every page below: as the server sent them, once the
/// page takes over, once "a" is pressed, and once "Change" adds "c". `markup`
/// is the column as the page holds it once it takes over.
fn expected(markup: &str) -> String {
    format!(
        "a false, b false\n\
         a false, b false\n\
         {markup}\n\
         a true, b false\n\
         a true, b false, c false\n"
    )
}

#[test]
fn rows_that_make_signals_from_their_context_render_on_a_server() {
    assert_eq!(rows_say("served-context-rows", "ROWS"), expected(BUTTONS));
}

#[test]
fn rows_a_condition_shows_make_signals_on_a_server() {
    assert_eq!(
        rows_say(
            "served-context-condition",
            "ui.choose(.Cell(shown), ui.stack({ styles: [], children: [ROWS] }), ui.empty())",
        ),
        expected(&format!("<div>{BUTTONS}</div>"))
    );
}

#[test]
fn rows_a_computed_node_builds_make_signals_on_a_server() {
    assert_eq!(
        rows_say(
            "served-context-computed",
            "ui.computed(fn(_s) => ui.stack({ styles: [], children: [ROWS] }))",
        ),
        expected(&format!("<div>{BUTTONS}</div>"))
    );
}

#[test]
fn rows_in_an_open_dialog_make_signals_on_a_server() {
    assert_eq!(
        rows_say(
            "served-context-dialog",
            r#"ui.dialog({ isOpen: shown, label: .Const("Rows"), children: [ROWS], styles: [] })"#,
        ),
        expected(&format!(
            "<dialog aria-label=\"Rows\" class=\"pos-viewport pin-out pin-top-0 pin-bottom-0 pin-start-0 pin-end-0 lay-row am-center ac-center bg-000000a0_5\" open=\"\"><div>{BUTTONS}</div></dialog>"
        ))
    );
}

#[test]
fn rows_in_an_open_disclosure_make_signals_on_a_server() {
    assert_eq!(
        rows_say(
            "served-context-disclosure",
            r#"ui.disclosure({ summary: .Const("More"), isOpen: shown, children: [ROWS], styles: [] })"#,
        ),
        expected(&format!(
            "<details open=\"\"><summary>More</summary>{BUTTONS}</details>"
        ))
    );
}

#[test]
fn rows_a_tooltip_wraps_make_signals_on_a_server() {
    assert_eq!(
        rows_say(
            "served-context-tooltip",
            r#"ui.tooltip({ text: .Const("Tip"), styles: [], children: [ROWS] })"#,
        ),
        expected(
            "<div><button aria-describedby=\"buri-tip-0\" aria-label=\"a false\" type=\"button\">a false</button>\
             <button aria-label=\"b false\" type=\"button\">b false</button>\
             <div hidden=\"\" id=\"buri-tip-0\" role=\"tooltip\">Tip</div></div>"
        )
    );
}

#[test]
fn rows_inside_rows_make_signals_on_a_server() {
    assert_eq!(
        rows_say(
            "served-context-nested",
            r#"ui.each(.Const(["outer"]), fn(s) => s, fn(c, _s, _i) => {
                let _own = signal(c, 0);
                ui.stack({ styles: [], children: [ROWS] })
            })"#,
        ),
        expected(&format!("<div>{BUTTONS}</div>"))
    );
}

#[test]
fn a_rebuilt_region_makes_signals_on_a_server() {
    assert_eq!(
        rows_say(
            "served-context-rebuild",
            "ui.rebuild(.Cell(shown), fn(c, _on) => ui.stack({ styles: [], children: [ROWS, row(c, \"z\")] }))",
        ),
        format!(
            "a false, b false, z false\n\
             a false, b false, z false\n\
             <div>{BUTTONS}{}</div>\n\
             a true, b false, z false\n\
             a true, b false, c false, z false\n",
            "<button aria-label=\"z false\" type=\"button\">z false</button>"
        )
    );
}

#[test]
fn shapes_a_drawing_keys_make_signals_on_a_server() {
    let rect = |name: &str, x: u32| {
        format!(
            "<rect aria-label=\"{name} false\" height=\"8\" role=\"img\" style=\"fill: none;\" tabindex=\"0\" width=\"8\" x=\"{x}\" y=\"0\"></rect>"
        )
    };
    assert_eq!(
        rows_say(
            "served-context-drawing",
            r#"ui.drawing({
                alt: .AccessibilityText("Rows"),
                size: .Const((30.0, 10.0)),
                styles: [],
                shapes: [
                    shape.each(.Cell(items), fn(s) => s, fn(c, s, i) => {
                        let focused = signal(c, false);
                        shape.rect({
                            at: .Computed(fn(sc) => (i.read(sc).toF64() * 10.0, 0.0)),
                            size: .Const((8.0, 8.0)),
                            fill: .None,
                            label: .Some(.Computed(fn(sc) => str.format(sc, "${s} ${focused.get(sc)}"))),
                            onFocus: .Some(fn(c2, now) => focused.set(c2, now)),
                        })
                    }),
                ],
            })"#,
        ),
        expected(&format!(
            "<svg aria-label=\"Rows\" preserveAspectRatio=\"none\" role=\"img\" viewBox=\"0 0 30 10\">{}{}</svg>",
            rect("a", 0),
            rect("b", 10)
        ))
    );
}
