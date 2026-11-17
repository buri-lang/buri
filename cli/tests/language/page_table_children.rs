//! **Each child of a table row is a table entry, and each child of a table a
//! row, in a browser as in the painter.**
//!
//! The painter gives each child of a `.TableRow` a column of its own and each
//! child of a `.Table` a row of its own. A browser groups children that aren't
//! table entries into one anonymous entry, and an HTML parser moves them out of
//! a table a server sends. So the runtime holds each such child in a `<td>`,
//! and in a table in a `<tr>` too.
//!
//! Each test builds a page whose table row and table each hold three children
//! of one kind, the middle one a row of a list. It prints what a browser makes
//! of them: the boxes in the row, then the boxes in the table, each by its text,
//! else by what a reader hears it called, else by its element. "Change" adds a
//! row to the list, so each test reads the first render and the update after
//! it. The served tests read the document a server sends as a browser parses
//! it, then the page that takes it over, then the update.
//!
//! A table entry outside any table row is a plain box in the painter. The
//! tests of it read a stack and a table entry that each hold three of them.

use super::pages::{drive, run_served, served};
use crate::harness::browser::browser;

/// Prints the boxes a browser lays out in each container on the page labelled
/// "Row", "Table", "Stack" or "Entry".
const SHOW: &str = r#"
const tick = () => new Promise((done) => setTimeout(done, 0));
const named = (n) => {
  if (n.nodeType !== 1 || n.textContent !== "") return n.textContent;
  const at = [n, ...n.querySelectorAll("*")].find((e) => e.getAttribute("aria-label") || e.getAttribute("alt"));
  if (at !== undefined) return at.getAttribute("aria-label") || at.getAttribute("alt");
  return `<${[n, ...n.querySelectorAll("*")].find((e) => !["td", "th", "tr"].includes(e.localName)).localName}>`;
};
const line = (label) => {
  const shown = boxes(labelled(label)).map((box) => box.map(named).join(""));
  return `${label}: ${shown.join(" | ")} (${shown.length} box${shown.length === 1 ? "" : "es"})`;
};
const LABELS = ["Row", "Table", "Stack", "Entry"];
// What is in the top layer, over the page, from the bottom up.
const over = () => {
  const shown = topLayers.filter((e) => e.isConnected).map(named);
  return shown.length === 0 ? [] : [`over the page: ${shown.join(" | ")}`];
};
const show = () => console.log([...LABELS.filter((l) => labelled(l) !== null).map(line), ...over()].join("; "));
const change = async () => {
  labelled("Change").click();
  await tick();
};
"#;

/// The children the row and the table each hold: `kid` called "one", a row per
/// item, then `kid` called "End".
const KIDS: &str = r#"kid(ctx, "one", shared),
    ui.each(.Cell(items), fn(s) => s, fn(c, s, _i) => kid(c, s, shared)),
    kid(ctx, "End", shared)"#;

/// The imports, and `kid`, whose body is `kid`: a node called `name`. The
/// controls share the signals in `shared`.
fn head(kid: &str) -> String {
    format!(
        r#"from "platform/effect" import {{ Allocator, Ui }};
from "ui/node" import * as ui;
from "ui/node" import {{ Node }};
from "ui/signal" import {{ Signal, signal }};

struct Shared {{
    text: Signal<Str>,
    on: Signal<Bool>,
    open: Signal<Bool>,
    level: Signal<Float>,
    picked: Signal<Str>,
}}

fn shared<C: Ui>(ctx: C): Shared {{
    Shared {{
        text: signal(ctx, ""),
        on: signal(ctx, false),
        open: signal(ctx, true),
        level: signal(ctx, 0.5),
        picked: signal(ctx, "x"),
    }}
}}

fn kid<C: Ui>(ctx: C, name: Str, shared: Shared): Node<C> {{
    {kid}
}}
"#
    )
}

/// The button, then a table holding a row labelled "Row", then a table
/// labelled "Table", each holding [`KIDS`].
const BODY: &str = r#"ui.stack({
        styles: [],
        children: [
            ui.button({
                label: .Const("Change"),
                styles: [],
                onPress: .Some(fn(c) => items.set(c, ["a", "b"])),
            }),
            ui.stack({ role: .Some(.Table), styles: [], children: [
                ui.stack({ role: .Some(.TableRow), label: .Some(.Const("Row")), styles: [], children: [KIDS] }),
            ] }),
            ui.stack({ role: .Some(.Table), label: .Some(.Const("Table")), styles: [], children: [KIDS] }),
        ],
    })"#;

/// A mounted page whose table row and table hold `kid`s.
fn page(body: &str, kid: &str) -> String {
    format!(
        r#"{}from "web" import {{ WebHost }};

export fn main(host: WebHost): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
    }};
    let items = signal(ctx, ["a"]);
    let shared = shared(ctx);
    ui.mount(ctx, {}, [])
}}
"#,
        head(kid),
        body.replace("KIDS", KIDS)
    )
}

/// What a browser makes of the row and the table when the page mounts, then
/// after "Change".
fn mounted(name: &str, body: &str, kid: &str) -> String {
    let driver = format!("{SHOW}\nshow();\nawait change();\nshow();\n");
    drive(name, &page(body, kid), &driver)
}

/// A page a server renders and the browser takes over, its table row and
/// table holding `kid`s.
fn served_page(body: &str, kid: &str) -> String {
    format!(
        r#"{}from "core/net/http" import * as http;
from "platform/effect" import {{ Request, Response }};
from "ui/web" import * as web;
from "web" import {{ WebHost }};
from "//platform/server" import {{ ServerHost }};

fn page<C: Ui>(ctx: C): Node<C> {{
    let items = signal(ctx, ["a"]);
    let shared = shared(ctx);
    {}
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
    http.html(ctx, web.shell(ctx, request.path(), web.defaultDocument(), web.render(ctx, page(ctx)), .Null))
}}
"#,
        head(kid),
        body.replace("KIDS", KIDS)
    )
}

/// What a browser makes of the row and the table as the server's document
/// parses, once the page has taken it over, and after "Change".
fn resumed(name: &str, body: &str, kid: &str) -> String {
    let scratch = served(name, &served_page(body, kid));
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

/// What a browser shows of the two containers labelled `labels` when the page
/// mounts, then after "Change", each line the boxes it holds: `before`, then
/// `after`. Each line then says what is over the page, from `over`: as a
/// server's document parses, once the page shows, and after "Change".
fn twice(labels: [&str; 2], before: &str, after: &str, over: [&str; 3]) -> String {
    format!(
        "{}{}",
        shown(labels, before, over[1]),
        shown(labels, after, over[2])
    )
}

/// One line of what a browser shows: the containers labelled `labels`, each
/// holding `boxes`, and `over` over the page.
fn shown(labels: [&str; 2], boxes: &str, over: &str) -> String {
    let n = boxes.split(" | ").count();
    let held = format!("{boxes} ({n} box{})", if n == 1 { "" } else { "es" });
    let over = if over.is_empty() {
        String::new()
    } else {
        format!("; over the page: {over}")
    };
    let [first, second] = labels;
    format!("{first}: {held}; {second}: {held}{over}\n")
}

/// [`twice`] as a server sends it: first the server's document as it parses.
fn served_twice(labels: [&str; 2], before: &str, after: &str, over: [&str; 3]) -> String {
    format!(
        "{}{}",
        shown(labels, before, over[0]),
        twice(labels, before, after, over)
    )
}

/// A test that `kid`s in a row and a table are one box each, on a page that
/// mounts, and one a server sends: each box shows `before` and then, once
/// "Change" adds a row, `after`. `over` is what is over the page, in the top
/// layer, at each of those times.
macro_rules! one_by_one {
    ($mounted:ident, $served:ident, $kid:expr) => {
        one_by_one!(
            $mounted,
            $served,
            $kid,
            "one | a | End",
            "one | a | b | End"
        );
    };
    ($mounted:ident, $served:ident, $kid:expr, $before:expr, $after:expr) => {
        one_by_one!($mounted, $served, $kid, $before, $after, ["", "", ""]);
    };
    ($mounted:ident, $served:ident, $kid:expr, $before:expr, $after:expr, $over:expr) => {
        one_by_one!(@in BODY, ["Row", "Table"], $mounted, $served, $kid, $before, $after, $over);
    };
    (@in $body:expr, $labels:expr, $mounted:ident, $served:ident, $kid:expr, $before:expr, $after:expr, $over:expr) => {
        #[test]
        fn $mounted() {
            assert_eq!(
                mounted(stringify!($mounted), $body, $kid),
                twice($labels, $before, $after, $over)
            );
        }

        #[test]
        fn $served() {
            assert_eq!(
                resumed(stringify!($served), $body, $kid),
                served_twice($labels, $before, $after, $over)
            );
        }
    };
}

/// A test that `kid`s, each a table entry outside any table row, are one box
/// each, as the painter draws them: in a stack, and in a table entry.
macro_rules! alone {
    ($mounted:ident, $served:ident, $kid:expr) => {
        one_by_one!(
            @in LONE,
            ["Stack", "Entry"],
            $mounted,
            $served,
            $kid,
            "one! | a! | End!",
            "one! | a! | b! | End!",
            ["", "", ""]
        );
    };
}

/// The button, then a group labelled "Stack" and a table entry labelled
/// "Entry", each holding [`KIDS`].
const LONE: &str = r#"ui.stack({
        styles: [],
        children: [
            ui.button({
                label: .Const("Change"),
                styles: [],
                onPress: .Some(fn(c) => items.set(c, ["a", "b"])),
            }),
            ui.stack({ role: .Some(.Group), label: .Some(.Const("Stack")), styles: [], children: [KIDS] }),
            ui.stack({ role: .Some(.Table), styles: [], children: [
                ui.stack({ role: .Some(.TableRow), styles: [], children: [
                    ui.stack({ role: .Some(.Cell), label: .Some(.Const("Entry")), styles: [], children: [KIDS] }),
                ] }),
            ] }),
        ],
    })"#;

one_by_one!(
    stacks_in_a_table_row_and_a_table_are_one_box_each,
    stacks_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ styles: [], children: [ui.text({ content: .Const(name) })] })"#
);

one_by_one!(
    headings_in_a_table_row_and_a_table_are_one_box_each,
    headings_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.text({ content: .Const(name), headingLevel: .Some(2) })"#
);

one_by_one!(
    groups_in_a_table_row_and_a_table_are_one_box_each,
    groups_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ role: .Some(.Group), styles: [], children: [ui.text({ content: .Const(name) })] })"#
);

one_by_one!(
    list_items_in_a_table_row_and_a_table_are_one_box_each,
    list_items_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ role: .Some(.ListItem), styles: [], children: [ui.text({ content: .Const(name) })] })"#
);

// A table entry is one already in a row, and in a table it is a row of its
// own.
one_by_one!(
    table_entries_in_a_table_row_and_a_table_are_one_box_each,
    table_entries_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ role: .Some(.Cell), styles: [], children: [ui.text({ content: .Const(name) })] })"#
);

one_by_one!(
    column_headers_in_a_table_row_and_a_table_are_one_box_each,
    column_headers_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ role: .Some(.ColumnHeader), styles: [], children: [ui.text({ content: .Const(name) })] })"#
);

one_by_one!(
    row_headers_in_a_table_row_and_a_table_are_one_box_each,
    row_headers_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ role: .Some(.RowHeader), styles: [], children: [ui.text({ content: .Const(name) })] })"#
);

// A table row is a row already in a table, and in a row it is a table of one
// row in a table entry of its own.
one_by_one!(
    table_rows_in_a_table_row_and_a_table_are_one_box_each,
    table_rows_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ role: .Some(.TableRow), styles: [], children: [ui.text({ content: .Const(name) })] })"#
);

one_by_one!(
    tables_in_a_table_row_and_a_table_are_one_box_each,
    tables_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ role: .Some(.Table), styles: [], children: [ui.text({ content: .Const(name) })] })"#
);

one_by_one!(
    buttons_in_a_table_row_and_a_table_are_one_box_each,
    buttons_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.button({ label: .Const(name), styles: [], onPress: .None })"#
);

one_by_one!(
    submits_in_a_table_row_and_a_table_are_one_box_each,
    submits_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.button({ label: .Const(name), kind: .Some(.Submit), styles: [], onPress: .None })"#
);

one_by_one!(
    links_in_a_table_row_and_a_table_are_one_box_each,
    links_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.link({ dest: .Const("/"), children: [ui.text({ content: .Const(name) })], styles: [] })"#
);

one_by_one!(
    images_in_a_table_row_and_a_table_are_one_box_each,
    images_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.image({ source: .Const("/a.png"), alt: .AccessibilityText(name), styles: [] })"#
);

one_by_one!(
    icons_in_a_table_row_and_a_table_are_one_box_each,
    icons_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.image({
        source: .Const("<svg viewBox=\"0 0 10 10\"><path d=\"M0 0L10 10\"/></svg>"),
        alt: .Decorative,
        styles: [],
    })"#,
    "<svg> | <svg> | <svg>",
    "<svg> | <svg> | <svg> | <svg>"
);

one_by_one!(
    drawings_in_a_table_row_and_a_table_are_one_box_each,
    drawings_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.drawing({ alt: .AccessibilityText(name), size: .Const((10.0, 10.0)), styles: [], shapes: [] })"#
);

one_by_one!(
    fields_in_a_table_row_and_a_table_are_one_box_each,
    fields_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.field({ label: .Const(name), kind: .Text, value: shared.text, styles: [] })"#
);

one_by_one!(
    sliders_in_a_table_row_and_a_table_are_one_box_each,
    sliders_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.slider({ label: .Const(name), value: shared.level, min: 0.0, max: 1.0, styles: [] })"#
);

one_by_one!(
    toggles_in_a_table_row_and_a_table_are_one_box_each,
    toggles_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.toggle({ label: .Const(name), kind: .Checkbox, isOn: shared.on, styles: [] })"#
);

one_by_one!(
    pickers_in_a_table_row_and_a_table_are_one_box_each,
    pickers_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.picker({
        label: .Const(name),
        options: [("x", ui.text({ content: .Const(name) }))],
        value: shared.picked,
        styles: [],
    })"#
);

one_by_one!(
    file_pickers_in_a_table_row_and_a_table_are_one_box_each,
    file_pickers_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.filePicker({ label: .Const(name), maxBytes: 100, onPick: fn(_c, _file) => (), styles: [] })"#
);

one_by_one!(
    forms_in_a_table_row_and_a_table_are_one_box_each,
    forms_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.form({ onSubmit: fn(_c) => (), children: [ui.text({ content: .Const(name) })], styles: [] })"#
);

one_by_one!(
    disclosures_in_a_table_row_and_a_table_are_one_box_each,
    disclosures_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.disclosure({ summary: .Const(name), isOpen: shared.on, children: [], styles: [] })"#
);

one_by_one!(
    progress_bars_in_a_table_row_and_a_table_are_one_box_each,
    progress_bars_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.progress({ label: .Const(name), value: .Some(.Const(0.5)), styles: [] })"#
);

one_by_one!(
    tooltips_in_a_table_row_and_a_table_are_one_box_each,
    tooltips_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.tooltip({
        text: .Const(""),
        styles: [],
        children: [ui.button({ label: .Const(name), styles: [], onPress: .None })],
    })"#
);

one_by_one!(
    conditions_in_a_table_row_and_a_table_are_one_box_each,
    conditions_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.choose(.Const(true), ui.stack({ styles: [], children: [ui.text({ content: .Const(name) })] }), ui.empty())"#
);

one_by_one!(
    computed_nodes_in_a_table_row_and_a_table_are_one_box_each,
    computed_nodes_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.computed(fn(_s) => ui.stack({ styles: [], children: [ui.text({ content: .Const(name) })] }))"#
);

// An empty node is no box, in the painter as in a browser.
one_by_one!(
    empty_nodes_in_a_table_row_and_a_table_are_no_box,
    empty_nodes_a_server_sends_in_a_table_row_and_a_table_are_no_box,
    r#"if (name == "a" || name == "b") {
        ui.empty()
    } else {
        ui.stack({ styles: [], children: [ui.text({ content: .Const(name) })] })
    }"#,
    "one | End",
    "one | End"
);

// A shut dialog draws nothing, and an open one is drawn over the page, so
// neither takes a table entry or a row.
one_by_one!(
    dialogs_in_a_table_row_and_a_table_are_no_box,
    dialogs_a_server_sends_in_a_table_row_and_a_table_are_no_box,
    r#"if (name == "a" || name == "b") {
        ui.dialog({ isOpen: shared.on, label: .Const(name), children: [], styles: [] })
    } else {
        ui.stack({ styles: [], children: [ui.text({ content: .Const(name) })] })
    }"#,
    "one | End",
    "one | End"
);

// An open dialog is drawn over the page, so it takes no table entry or row
// either.
one_by_one!(
    open_dialogs_in_a_table_row_and_a_table_are_no_box,
    open_dialogs_a_server_sends_in_a_table_row_and_a_table_are_no_box,
    r#"if (name == "a" || name == "b") {
        ui.dialog({ isOpen: shared.open, label: .Const(name), children: [], styles: [] })
    } else {
        ui.stack({ styles: [], children: [ui.text({ content: .Const(name) })] })
    }"#,
    "one | End",
    "one | End",
    ["a | a", "a | a", "a | a | b | b"]
);

// A box out of the flow keeps its table entry and its row, as an anonymous
// entry holds it in a browser: held to the viewport, pinned, or pinned to an
// anchor.
one_by_one!(
    boxes_held_to_the_viewport_in_a_table_row_and_a_table_are_one_box_each,
    boxes_held_to_the_viewport_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ styles: [.Position(.PinViewport)], children: [ui.text({ content: .Const(name) })] })"#
);

one_by_one!(
    pinned_boxes_in_a_table_row_and_a_table_are_one_box_each,
    pinned_boxes_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ styles: [.Pin(.Top, .Px(0))], children: [ui.text({ content: .Const(name) })] })"#
);

one_by_one!(
    anchored_panels_in_a_table_row_and_a_table_are_one_box_each,
    anchored_panels_a_server_sends_in_a_table_row_and_a_table_are_one_box_each,
    r#"ui.stack({ styles: [.Position(.PinAnchor)], children: [ui.text({ content: .Const(name) })] })"#,
    "one | a | End",
    "one | a | b | End",
    [
        "",
        "one | a | End | one | a | End",
        "one | a | b | End | one | a | b | End"
    ]
);

// A table entry, a column header or a row header outside any table row is a
// plain box, as the painter draws it: a browser would put a run of them in an
// anonymous table, and a parser drops each from a document a server sends.
alone!(
    table_entries_outside_a_table_row_are_one_box_each,
    table_entries_a_server_sends_outside_a_table_row_are_one_box_each,
    r#"ui.stack({ role: .Some(.Cell), styles: [], children: [ui.text({ content: .Const(name) }), ui.text({ content: .Const("!") })] })"#
);

alone!(
    column_headers_outside_a_table_row_are_one_box_each,
    column_headers_a_server_sends_outside_a_table_row_are_one_box_each,
    r#"ui.stack({ role: .Some(.ColumnHeader), styles: [], children: [ui.text({ content: .Const(name) }), ui.text({ content: .Const("!") })] })"#
);

alone!(
    row_headers_outside_a_table_row_are_one_box_each,
    row_headers_a_server_sends_outside_a_table_row_are_one_box_each,
    r#"ui.stack({ role: .Some(.RowHeader), styles: [], children: [ui.text({ content: .Const(name) }), ui.text({ content: .Const("!") })] })"#
);

/// Prints how a browser draws each text in the group labelled "Stack": its
/// weight and its alignment.
const WEIGHTS: &str = r#"
const drawn = () => {
  const texts = [...labelled("Stack").querySelectorAll("span")];
  console.log(texts.map((t) => `${t.textContent} ${getComputedStyle(t).fontWeight} ${getComputedStyle(t).textAlign}`).join(", "));
};
"#;

/// Column headers and a row header in the group labelled "Stack", outside any
/// table row: how a browser draws their texts once the page mounts, or as a
/// server's document parses and then once the page takes it over.
fn headers_drawn(name: &str, served_too: bool) -> String {
    let kid = r#"ui.stack({
        role: .Some(if (name == "End") { .RowHeader } else { .ColumnHeader }),
        styles: [],
        children: [ui.text({ content: .Const(name) })],
    })"#;
    if !served_too {
        return drive(name, &page(LONE, kid), &format!("{WEIGHTS}\ndrawn();\n"));
    }
    let scratch = served(name, &served_page(LONE, kid));
    let script = format!(
        r#"{}
{WEIGHTS}
const {{ default: server }} = await import("./.buri/out/platform/server/cmd/page/fetch.mjs");
parse(await (await server.fetch(new Request("http://localhost/"))).text());
drawn();
await import("./.buri/out/web/cmd/page/main.mjs");
await new Promise((done) => setTimeout(done, 0));
drawn();
"#,
        browser()
    );
    run_served(&scratch.write("drive.mjs", &script), "served")
}

/// Each header's text bold and centred, as the painter draws a header and a
/// browser draws one in a table row.
const BOLD: &str = "one bold center, a bold center, End bold center\n";

#[test]
fn headers_outside_a_table_row_are_bold_and_centred() {
    let name = "headers_outside_a_table_row_are_bold_and_centred";
    assert_eq!(headers_drawn(name, false), BOLD);
}

#[test]
fn headers_a_server_sends_outside_a_table_row_are_bold_and_centred() {
    let name = "headers_a_server_sends_outside_a_table_row_are_bold_and_centred";
    assert_eq!(headers_drawn(name, true), BOLD.repeat(2));
}
