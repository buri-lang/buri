//! **Each `ui.text` is a box of its own in a browser (buri-lang/buri#357).**
//!
//! A flex container wraps a run of bare text in one anonymous item, however
//! many text nodes it holds and whatever comments sit between them. The
//! headless painter lays out each `ui.text` on its own, so a page that wrote
//! sibling texts bare showed `$550k$600k$650k` on one line where the snapshot
//! drew three.
//!
//! Each test builds a page, imports it into happy-dom's document and prints
//! what a browser makes of a column's children: one box per element and one
//! per run of text, with the column's `textContent`. Every page has a "Change"
//! button that writes its signals, so each test reads the first render and the
//! update after it.

use super::pages::{drive, run_served, served};
use crate::harness::browser::browser;

/// Prints the boxes a browser lays out in the column beside "Change".
const SHOW: &str = r#"
const tick = () => new Promise((done) => setTimeout(done, 0));
// One box per element, and one per run of text between elements: a browser
// skips comments and empty text when it gathers a run.
function boxes(column) {
  const out = [];
  let run = null;
  for (const node of column.childNodes) {
    if (node.nodeType === 1) {
      run = null;
      out.push(node.textContent);
    } else if (node.nodeType === 3 && node.data !== "") {
      if (run === null) out.push("");
      run = (run ?? "") + node.data;
      out[out.length - 1] = run;
    }
  }
  return out;
}
const show = () => {
  const column = labelled("Change").nextElementSibling;
  const shown = boxes(column);
  console.log(`${shown.join(" | ")} (${shown.length} box${shown.length === 1 ? "" : "es"}, "${column.textContent}")`);
};
const change = async () => {
  labelled("Change").click();
  await tick();
};
"#;

/// A mounted page whose column holds `children`. `signals` makes what they
/// read, and `writes` are the statements "Change" runs before it ends.
fn page(signals: &str, writes: &str, children: &str) -> String {
    format!(
        r#"from "platform/effect" import {{ Allocator, Ui }};
from "ui/node" import * as ui;
from "ui/signal" import {{ signal }};
from "web" import {{ WebHost }};

export fn main(host: WebHost): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
    }};
    let changed = signal(ctx, false);
    {signals}
    ui.mount(
        ctx,
        ui.stack({{
            styles: [],
            children: [
                ui.button({{
                    label: .Const("Change"),
                    styles: [],
                    onPress: .Some(fn(c) => {{
                        {writes}
                        changed.set(c, true)
                    }}),
                }}),
                ui.stack({{ styles: [.Layout(.Column)], children: [{children}] }}),
            ],
        }}),
        [],
    )
}}
"#
    )
}

/// The column's boxes when the page mounts, then after "Change".
fn mounted(name: &str, signals: &str, writes: &str, children: &str) -> String {
    let driver = format!("{SHOW}\nshow();\nawait change();\nshow();\n");
    drive(name, &page(signals, writes, children), &driver)
}

#[test]
fn adjacent_texts_are_one_box_each() {
    let shown = mounted(
        "texts-adjacent",
        "",
        "",
        r#"ui.text({ content: .Const("$550k") }),
           ui.text({ content: .Const("$600k") }),
           ui.text({ content: .Const("$650k") })"#,
    );
    assert_eq!(
        shown,
        "$550k | $600k | $650k (3 boxes, \"$550k$600k$650k\")\n\
         $550k | $600k | $650k (3 boxes, \"$550k$600k$650k\")\n"
    );
}

#[test]
fn a_text_beside_an_element_is_a_box_of_its_own() {
    let shown = mounted(
        "texts-beside-an-element",
        "",
        "",
        r#"ui.text({ content: .Const("Total") }),
           ui.button({ label: .Const("Edit"), styles: [], onPress: .Some(fn(_c) => ()) }),
           ui.text({ content: .Const("$5") })"#,
    );
    assert_eq!(
        shown,
        "Total | Edit | $5 (3 boxes, \"TotalEdit$5\")\n\
         Total | Edit | $5 (3 boxes, \"TotalEdit$5\")\n"
    );
}

#[test]
fn texts_that_update_stay_one_box_each() {
    let shown = mounted(
        "texts-that-update",
        r#"let first = signal(ctx, "one");
    let second = signal(ctx, "two");"#,
        r#"let _ = first.set(c, "uno");
                        let _ = second.set(c, "dos");"#,
        r#"ui.text({ content: .Cell(first) }),
           ui.text({ content: .Cell(second) })"#,
    );
    assert_eq!(
        shown,
        "one | two (2 boxes, \"onetwo\")\n\
         uno | dos (2 boxes, \"unodos\")\n"
    );
}

#[test]
fn a_text_that_starts_empty_is_a_box_of_its_own_once_it_has_words() {
    let shown = mounted(
        "texts-start-empty",
        r#"let first = signal(ctx, "");"#,
        r#"let _ = first.set(c, "a");"#,
        r#"ui.text({ content: .Cell(first) }),
           ui.text({ content: .Const("b") })"#,
    );
    // The empty text is an empty box, as it is an empty run in the snapshot.
    assert_eq!(
        shown,
        " | b (2 boxes, \"b\")\n\
         a | b (2 boxes, \"ab\")\n"
    );
}

#[test]
fn texts_in_each_row_are_one_box_each() {
    let shown = mounted(
        "texts-in-rows",
        r#"let items = signal(ctx, ["a", "b"]);"#,
        r#"let _ = items.set(c, ["a", "b", "c"]);"#,
        r#"ui.text({ content: .Const("List") }),
           ui.each(.Cell(items), fn(s) => s, fn(_c, s, _i) => ui.text({ content: .Const(s) })),
           ui.text({ content: .Const("End") })"#,
    );
    assert_eq!(
        shown,
        "List | a | b | End (4 boxes, \"ListabEnd\")\n\
         List | a | b | c | End (5 boxes, \"ListabcEnd\")\n"
    );
}

#[test]
fn a_text_a_condition_shows_is_a_box_of_its_own() {
    let shown = mounted(
        "texts-in-a-condition",
        r#"let flag = signal(ctx, false);"#,
        r#"let _ = flag.set(c, true);"#,
        r#"ui.text({ content: .Const("x") }),
           ui.choose(.Cell(flag), ui.text({ content: .Const("y") }), ui.text({ content: .Const("z") })),
           ui.text({ content: .Const("w") })"#,
    );
    assert_eq!(
        shown,
        "x | z | w (3 boxes, \"xzw\")\n\
         x | y | w (3 boxes, \"xyw\")\n"
    );
}

/// A page a server renders and the browser resumes, its column holding
/// `children`. `page` makes the signals they read with `ctx`, so the server
/// and the page build the same tree.
fn served_page(signals: &str, writes: &str, children: &str) -> String {
    format!(
        r#"from "core/net/http" import * as http;
from "platform/effect" import {{ Allocator, Request, Response, Ui }};
from "ui/node" import * as ui;
from "ui/node" import {{ Node }};
from "ui/signal" import {{ signal }};
from "ui/web" import * as web;
from "web" import {{ WebHost }};
from "//platform/server" import {{ ServerHost }};

fn page<C: Ui>(ctx: C): Node<C> {{
    let changed = signal(ctx, false);
    {signals}
    ui.stack({{
        styles: [],
        children: [
            ui.button({{
                label: .Const("Change"),
                styles: [],
                onPress: .Some(fn(c) => {{
                    {writes}
                    changed.set(c, true)
                }}),
            }}),
            ui.stack({{ styles: [.Layout(.Column)], children: [{children}] }}),
        ],
    }})
}}

export fn main(host: WebHost): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
    }};
    web.resume(ctx, page(ctx))
}}

export fn fetch(host: ServerHost, request: Request): Response {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
    }};
    http.html(ctx, web.shell(ctx, request.path(), web.defaultDocument(), web.render(page(ctx)), .Null))
}}
"#
    )
}

/// The column's boxes as the server's document parses, once the page has
/// taken it over, and after "Change".
fn resumed(name: &str, signals: &str, writes: &str, children: &str) -> String {
    let scratch = served(name, &served_page(signals, writes, children));
    let script = format!(
        r#"{}
{SHOW}
const {{ default: server }} = await import("./.buri/out/platform/server/cmd/page/fetch.mjs");
parse(await (await server.fetch(new Request("http://localhost/"))).text());
show();
await import("./.buri/out/web/cmd/page/main.mjs");
await tick();
show();
await change();
show();
"#,
        browser()
    );
    run_served(&scratch.write("drive.mjs", &script), "served")
}

#[test]
fn adjacent_texts_a_server_sends_are_one_box_each() {
    let shown = resumed(
        "served-texts-adjacent",
        "",
        "",
        r#"ui.text({ content: .Const("$550k") }),
           ui.text({ content: .Const("$600k") }),
           ui.text({ content: .Const("$650k") })"#,
    );
    let line = "$550k | $600k | $650k (3 boxes, \"$550k$600k$650k\")\n";
    assert_eq!(shown, line.repeat(3));
}

#[test]
fn texts_a_server_sends_update_after_the_page_takes_over() {
    let shown = resumed(
        "served-texts-update",
        r#"let first = signal(ctx, "one");
    let second = signal(ctx, "two");"#,
        r#"let _ = first.set(c, "uno");
                    let _ = second.set(c, "dos");"#,
        r#"ui.text({ content: .Cell(first) }),
           ui.text({ content: .Cell(second) }),
           ui.text({ content: .Const("three") })"#,
    );
    assert_eq!(
        shown,
        "one | two | three (3 boxes, \"onetwothree\")\n\
         one | two | three (3 boxes, \"onetwothree\")\n\
         uno | dos | three (3 boxes, \"unodosthree\")\n"
    );
}

#[test]
fn an_empty_text_a_server_sends_gets_its_words_after_the_page_takes_over() {
    let shown = resumed(
        "served-texts-start-empty",
        r#"let first = signal(ctx, "");"#,
        r#"let _ = first.set(c, "a");"#,
        r#"ui.text({ content: .Cell(first) }),
           ui.text({ content: .Const("b") })"#,
    );
    assert_eq!(
        shown,
        " | b (2 boxes, \"b\")\n\
         \x20| b (2 boxes, \"b\")\n\
         a | b (2 boxes, \"ab\")\n"
    );
}

#[test]
fn texts_in_rows_a_server_sends_stay_one_box_each_as_rows_arrive() {
    let shown = resumed(
        "served-texts-in-rows",
        r#"let items = signal(ctx, ["a", "b"]);"#,
        r#"let _ = items.set(c, ["a", "b", "c"]);"#,
        r#"ui.each(.Cell(items), fn(s) => s, fn(_c, s, _i) => ui.text({ content: .Const(s) })),
           ui.text({ content: .Const("End") })"#,
    );
    assert_eq!(
        shown,
        "a | b | End (3 boxes, \"abEnd\")\n\
         a | b | End (3 boxes, \"abEnd\")\n\
         a | b | c | End (4 boxes, \"abcEnd\")\n"
    );
}

#[test]
fn a_text_a_condition_a_server_sends_shows_is_a_box_of_its_own() {
    let shown = resumed(
        "served-texts-in-a-condition",
        r#"let flag = signal(ctx, false);"#,
        r#"let _ = flag.set(c, true);"#,
        r#"ui.text({ content: .Const("x") }),
           ui.choose(.Cell(flag), ui.text({ content: .Const("y") }), ui.text({ content: .Const("z") })),
           ui.text({ content: .Const("w") })"#,
    );
    assert_eq!(
        shown,
        "x | z | w (3 boxes, \"xzw\")\n\
         x | z | w (3 boxes, \"xzw\")\n\
         x | y | w (3 boxes, \"xyw\")\n"
    );
}
