//! **What a mounted page does when a browser drives it.**
//!
//! `conformance/lib/ui` asserts the tree through the headless double, and
//! `conformance/lib/web` asserts the HTML a worker sends. Neither runs the
//! half of `runtime.js` a browser runs after `mount`: the listeners that show a
//! tooltip and the calls that move the focus. So each test here builds a page
//! for `web`, imports it into happy-dom's document through `harness::browser`,
//! and drives it the way a reader would.

use crate::harness::browser::browser;
use crate::harness::hang::Capped;
use crate::harness::{js_runtime, tests_dir, Scratch};
use std::process::Command;

/// Builds `source` as a page and runs `driver` against it in [`browser`],
/// answering what the driver printed.
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

/// Imports the module `artifact` names in [`browser`], then runs `driver`.
fn run_page(scratch: &Scratch, artifact: &str, driver: &str) -> std::process::Output {
    let script = format!("{}\nawait import(\"./{artifact}\");\n{driver}", browser());
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
const said = () => body.firstChild.lastChild.data;
const name = () => document.activeElement.getAttribute("aria-label") ?? "nothing";
const actions = labelled("Actions");
console.log(said());
// A browser focuses what the pointer presses, then clicks it.
actions.focus();
console.log(`pressed ${name()} ${said()}`);
actions.click();
console.log(`opened ${name()} ${said()}`);
console.log(`order ${labelled("Delete").getAttribute("tabindex")}`);
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
const shown = () => (tip.childNodes[1].hasAttribute("hidden") ? "hidden" : "shown");
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
labelled("b").click();
await new Promise((done) => setTimeout(done, 0));
labelled("c").click();
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
        "console.log([...body.childNodes].filter((n) => n.nodeType === 3).map((n) => n.data).join(\" \"));\n",
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
    scratch
}

/// [`greeting_rows`], built.
fn greeting_page(name: &str, check: &str) -> Scratch {
    let scratch = greeting_rows(name, check);
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
    let scratch = greeting_page(
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
    let scratch = greeting_page(r#"(self) => Promise.resolve("page")"#, CHECK);
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
    let scratch = greeting_page(r#"(self) => "page""#, r#"(self, s) => Promise.resolve(s)"#);
    let out = run_page(&scratch, PAGE_ARTIFACT, "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "the page ran:\n{stderr}");
    assert!(
        stderr.contains("`HostGreet.check` answered a promise; write it `async` in the js file"),
        "{stderr}"
    );
}

/// Rows that call an `async` method while they are built wait, and the page
/// builds rows without waiting, so the build refuses them.
#[test]
fn rows_that_call_a_platform_s_async_method_while_they_are_built_are_refused() {
    let scratch = greeting_rows(r#"async (self) => "page""#, CHECK);
    scratch
        .run(&["build", "//cmd/app"])
        .exits(1)
        .says("this function waits, and the page needs its answer at once")
        .says("main.buri:25:");
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
        "console.log(body.firstChild.firstChild.data);\n",
    );
    assert!(
        out.status.success(),
        "the page did not run:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A list whose rows are told apart by their position: each row's label is
/// worked out from its index. The index is an `Int` like any other, so
/// arithmetic on it renders every row, on the page as in a snapshot.
const NUMBERED_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
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
                ui.each(.Const(["a", "b", "c"]), fn(s) => s, fn(c, s, i) => {
                    ui.text({ content: .Const(str.format(c, "${s}:${i % 2}")) })
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn rows_worked_out_from_their_index_render() {
    let printed = drive("numbered-page", NUMBERED_PAGE, "console.log(showing());\n");
    assert_eq!(printed, "<div>a:0b:1c:0</div>\n");
}

/// Rows built by functions that call a helper which a press handler also
/// calls, once with a callback that does nothing and once with one that
/// waits: a generic `tap`, `mapCtx`, and a `tapped` the handler calls through
/// a local holding it. The row's call waits on nothing, so every row renders,
/// and the press still waits and then navigates.
const SHARED_HELPER_PAGE: &str = r#"from "platform/effect" import { Allocator, Clock, Location, Ui };
from "core/time" import * as time;
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/web" import * as web;
from "web" import { WebHost };

fn tap<C>(ctx: C, f: fn(C) => ()): () {
    f(ctx)
}

fn tapped<C>(ctx: C, f: fn(C) => ()): () {
    f(ctx)
}

fn later<C: Clock + Location + Ui>(ctx: C, path: Str): () {
    let _ = time.sleep(ctx, time.milliseconds(1));
    web.navigate(ctx, path)
}

fn row<C: Allocator + Clock + Location + Ui>(ctx: C, s: Str): Node<C> {
    let _ = tap(ctx, fn(_d) => ());
    ui.button({ label: .Const(s), styles: [], onPress: .Some(fn(d) => tap(d, fn(e) => later(e, s))) })
}

fn held<C: Allocator + Clock + Location + Ui>(ctx: C, s: Str): Node<C> {
    let _ = tapped(ctx, fn(_d) => ());
    let go = fn(d: C) => {
        let t = tapped;
        t(d, fn(e) => later(e, s))
    };
    ui.button({ label: .Const(s), styles: [], onPress: .Some(go) })
}

fn mapped<C: Allocator + Clock + Location + Ui>(ctx: C, s: Str): Node<C> {
    let labels = [s].mapCtx(ctx, fn(_d, l) => l);
    let go = fn(d: C) => {
        let paths = [s].mapCtx(d, fn(e, l) => {
            let _ = time.sleep(e, time.milliseconds(1));
            l
        });
        web.navigate(d, paths.join(d, ""))
    };
    ui.button({ label: .Const(labels.join(ctx, "")), styles: [], onPress: .Some(go) })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
        Clock: host.clock,
        Location: host.location,
    };
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.each(.Const(["a", "b"]), fn(s) => s, fn(c, s, _i) => row(c, s)),
                ui.each(.Const(["c"]), fn(s) => s, fn(c, s, _i) => mapped(c, s)),
                ui.each(.Const(["d"]), fn(s) => s, fn(c, s, _i) => held(c, s)),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn rows_calling_a_helper_a_waiting_handler_also_calls_render_and_navigate() {
    let printed = drive(
        "shared-helper-page",
        SHARED_HELPER_PAGE,
        r#"
console.log(["a", "b", "c", "d"].map((l) => `${l}:${labelled(l) === null ? "missing" : "shown"}`).join(" "));
labelled("b").click();
await new Promise((done) => setTimeout(done, 30));
labelled("c").click();
await new Promise((done) => setTimeout(done, 30));
labelled("d").click();
await new Promise((done) => setTimeout(done, 30));
console.log(moves.join("\n"));
"#,
    );
    assert_eq!(
        printed,
        "a:shown b:shown c:shown d:shown\npushed b\npushed c\npushed d\n"
    );
}

/// One button per way a row can be reached: a builder held in a record, in a
/// list, in an option, a builder passed by name, a method, a trait method, a
/// closure a function returned, a recursive builder, and the `ui.rebuild`,
/// `ui.computed` and `ui.choose` callbacks. Every button's handler sleeps and
/// then navigates, and no builder waits, so every button renders and every
/// press navigates.
const FORMS_PAGE: &str = r#"from "platform/effect" import { Allocator, Clock, Location, Ui };
from "core/time" import * as time;
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/web" import * as web;
from "web" import { WebHost };

fn later<C: Clock + Location + Ui>(ctx: C, path: Str): () {
    let _ = time.sleep(ctx, time.milliseconds(1));
    web.navigate(ctx, path)
}

fn button<C: Clock + Location + Ui>(s: Str): Node<C> {
    ui.button({ label: .Const(s), styles: [], onPress: .Some(fn(d) => later(d, s)) })
}

struct Kit<C> {
    make: fn(Str) => Node<C>,
}

fn named<C: Clock + Location + Ui>(ctx: C, s: Str, _i: Int): Node<C> {
    button(s)
}

struct Maker {
    prefix: Str,
}

impl Maker {
    fn build<C: Clock + Location + Ui>(self, s: Str): Node<C> {
        button(s)
    }
}

trait Shows {
    fn node<C: Clock + Location + Ui>(self): Node<C>;
}

struct Label {
    text: Str,
}

impl Shows for Label {
    fn node<C: Clock + Location + Ui>(self): Node<C> {
        button(self.text)
    }
}

fn viaTrait<T: Shows, C: Clock + Location + Ui>(x: T): Node<C> {
    x.node()
}

fn maker<C: Clock + Location + Ui>(): fn(C, Str, Int) => Node<C> {
    let wrap = fn(s: Str) => button(s);
    fn(_c, s, _i) => wrap(s)
}

fn nested<C: Clock + Location + Ui>(s: Str, depth: Int): Node<C> {
    if (depth <= 0) {
        button(s)
    } else {
        ui.stack({ styles: [], children: [nested(s, depth - 1)] })
    }
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
        Clock: host.clock,
        Location: host.location,
    };
    let kit = Kit { make: fn(s) => button(s) };
    let kits = [kit];
    let held = Option.Some(fn(s: Str) => button(s));
    let m = Maker { prefix: "m" };
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.each(.Const(["record"]), fn(s) => s, fn(_c, s, _i) => kit.make(s)),
                ui.each(.Const(["list"]), fn(s) => s, fn(_c, s, _i) => {
                    match (kits.get(0)) {
                        .Some(k) => k.make(s),
                        .None => ui.empty(),
                    }
                }),
                ui.each(.Const(["option"]), fn(s) => s, fn(_c, s, _i) => {
                    match (held) {
                        .Some(f) => f(s),
                        .None => ui.empty(),
                    }
                }),
                ui.each(.Const(["named"]), fn(s) => s, named),
                ui.each(.Const(["method"]), fn(s) => s, fn(_c, s, _i) => m.build(s)),
                ui.each(.Const(["trait"]), fn(s) => s, fn(_c, s, _i) => viaTrait(Label { text: s })),
                ui.each(.Const(["closure"]), fn(s) => s, maker()),
                ui.each(.Const(["nested"]), fn(s) => s, fn(_c, s, _i) => nested(s, 2)),
                ui.rebuild(.Const("rebuilt"), fn(_c, k) => button(k)),
                ui.computed(fn(_scope) => button("computed")),
                ui.choose(.Const(true), button("chosen"), button("other")),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn rows_reached_every_way_render_and_their_waiting_presses_navigate() {
    let printed = drive(
        "forms-page",
        FORMS_PAGE,
        r#"
const labels = ["record", "list", "option", "named", "method", "trait", "closure", "nested", "rebuilt", "computed", "chosen"];
console.log(labels.map((l) => `${l}:${labelled(l) === null ? "missing" : "shown"}`).join(" "));
for (const l of labels) {
  if (labelled(l) !== null) {
    labelled(l).click();
    await new Promise((done) => setTimeout(done, 30));
  }
}
console.log(moves.join("\n"));
"#,
    );
    assert_eq!(
        printed,
        "record:shown list:shown option:shown named:shown method:shown trait:shown \
         closure:shown nested:shown rebuilt:shown computed:shown chosen:shown\n\
         pushed record\npushed list\npushed option\npushed named\npushed method\n\
         pushed trait\npushed closure\npushed nested\npushed rebuilt\npushed computed\n\
         pushed chosen\n"
    );
}

/// Rows built by a function that runs a hook out of a struct field, while a
/// press handler of the same type sleeps and then navigates. The hook in the
/// field waits on nothing, so every row renders: one whose struct was built
/// by a literal, and one by an update.
const HOOKS_PAGE: &str = r#"from "platform/effect" import { Allocator, Clock, Location, Ui };
from "core/time" import * as time;
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/web" import * as web;
from "web" import { WebHost };

struct Hooks<C> {
    seen: fn(C) => (),
    name: Str,
}

fn later<C: Clock + Location + Ui>(ctx: C, path: Str): () {
    let _ = time.sleep(ctx, time.milliseconds(1));
    web.navigate(ctx, path)
}

fn row<C: Clock + Location + Ui>(ctx: C, hooks: Hooks<C>, s: Str): Node<C> {
    let _ = hooks.seen(ctx);
    ui.button({ label: .Const(s), styles: [], onPress: .Some(fn(d) => later(d, s)) })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
        Clock: host.clock,
        Location: host.location,
    };
    let hooks = Hooks { seen: fn(_c) => (), name: "plain" };
    let updated = Hooks { ..hooks, seen: fn(_c) => () };
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.each(.Const(["a"]), fn(s) => s, fn(c, s, _i) => row(c, hooks, s)),
                ui.each(.Const(["b"]), fn(s) => s, fn(c, s, _i) => row(c, updated, s)),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn rows_running_a_hook_from_a_struct_field_render_and_navigate() {
    let printed = drive(
        "hooks-page",
        HOOKS_PAGE,
        r#"
console.log(["a", "b"].map((l) => `${l}:${labelled(l) === null ? "missing" : "shown"}`).join(" "));
labelled("a").click();
await new Promise((done) => setTimeout(done, 30));
labelled("b").click();
await new Promise((done) => setTimeout(done, 30));
console.log(moves.join("\n"));
"#,
    );
    assert_eq!(printed, "a:shown b:shown\npushed a\npushed b\n");
}

/// Builds `source` as a page, expecting the build to refuse it, and answers
/// what it printed.
fn refused(name: &str, source: &str) -> String {
    let scratch = Scratch::repo(name);
    scratch.write(
        "cmd/page/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: \"web\" },\n    ]\n}\n",
    );
    scratch.write("cmd/page/main.buri", source);
    let run = scratch.run(&["build", "//cmd/page"]);
    run.exits(1);
    run.all()
}

/// A page whose `ui.each` rows wait while they are built: one row sleeps
/// itself, one goes through a helper that only passes it on to `ui.each`,
/// and one goes through a helper that passes it to itself first. A row
/// beside them that doesn't wait is not blamed.
const WAITING_ROWS_PAGE: &str = r#"from "core/time" import * as time;
from "platform/effect" import { Allocator, Clock, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "web" import { WebHost };

fn row<C: Clock + Ui>(ctx: C, s: Str): Node<C> {
    let _ = time.sleep(ctx, time.milliseconds(1));
    ui.text({ content: .Const(s) })
}

fn list<C: Clock + Ui>(rows: fn(C, Str, Int) => Node<C>): Node<C> {
    ui.each(.Const(["b"]), fn(s) => s, fn(c, s, i) => rows(c, s, i))
}

fn deep<C: Clock + Ui>(n: Int, rows: fn(C, Str, Int) => Node<C>): Node<C> {
    if (n <= 0) {
        ui.each(.Const(["c"]), fn(s) => s, rows)
    } else {
        deep(n - 1, rows)
    }
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        Ui: host.ui,
    };
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.each(.Const(["a"]), fn(s) => s, fn(c, s, _i) => row(c, s)),
                list(fn(c, s, _i) => row(c, s)),
                deep(2, fn(c, s, _i) => row(c, s)),
                ui.each(.Const(["d"]), fn(s) => s, fn(_c, s, _i) => ui.text({ content: .Const(s) })),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn rows_that_wait_while_they_are_built_are_refused_where_they_are_written() {
    let printed = refused("waiting-rows-page", WAITING_ROWS_PAGE);
    assert_eq!(
        printed
            .matches("this function waits, and the page needs its answer at once")
            .count(),
        3,
        "{printed}"
    );
    assert!(printed.contains("main.buri:35:"), "{printed}");
    assert!(printed.contains("main.buri:36:"), "{printed}");
    assert!(printed.contains("main.buri:37:"), "{printed}");
}

/// A page whose key handlers wait before they answer whether they claimed
/// the key: one written in place, and one a helper is handed.
const WAITING_KEYS_PAGE: &str = r#"from "core/time" import * as time;
from "platform/effect" import { Allocator, Clock, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "web" import { WebHost };

fn keyed<C: Clock + Ui>(handler: Option<fn(C, Str) => Bool>): Node<C> {
    ui.stack({ styles: [], children: [], onKey: handler })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        Ui: host.ui,
    };
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.stack({
                    styles: [],
                    children: [],
                    onKey: .Some(fn(c, key) => {
                        let _ = time.sleep(c, time.milliseconds(1));
                        key == "Enter"
                    }),
                }),
                keyed(.Some(fn(c, key) => {
                    let _ = time.sleep(c, time.milliseconds(1));
                    key == "Enter"
                })),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn key_handlers_that_wait_before_they_answer_are_refused() {
    let printed = refused("waiting-keys-page", WAITING_KEYS_PAGE);
    assert_eq!(
        printed
            .matches("this function waits, and the page needs its answer at once")
            .count(),
        2,
        "{printed}"
    );
    assert!(printed.contains("main.buri:25:"), "{printed}");
    assert!(printed.contains("main.buri:8:"), "{printed}");
}

/// A key handler a helper is handed that answers at once claims its key.
const KEYED_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "web" import { WebHost };

fn keyed<C: Ui>(handler: Option<fn(C, Str) => Bool>): Node<C> {
    ui.stack({
        styles: [],
        children: [ui.button({ label: .Const("Go"), styles: [], onPress: .None })],
        onKey: handler,
    })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    ui.mount(ctx, keyed(.Some(fn(_c, key) => key == "Enter")), [])
}
"#;

#[test]
fn a_key_handler_a_helper_is_handed_that_answers_at_once_claims_its_key() {
    let printed = drive(
        "keyed-page",
        KEYED_PAGE,
        r#"
labelled("Go").focus();
console.log(`Enter:${press("Enter").defaultPrevented} x:${press("x").defaultPrevented}`);
"#,
    );
    assert_eq!(printed, "Enter:true x:false\n");
}

/// A sparkline drawn twice: once as decoration, once as the content with a
/// name. Its line is in the colour of the text around it, and the area under
/// the line has a fill of its own.
const DRAWING_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Alt, Node };
from "ui/shape" import * as shape;
from "web" import { WebHost };

fn sparkline<C>(alt: Alt): Node<C> {
    ui.drawing({
        alt: alt,
        size: .Const((100.0, 40.0)),
        styles: [.Width(.Px(100)), .Height(.Px(40))],
        shapes: [
            shape.path({
                segments: .Const([.MoveTo(0.0, 40.0), .LineTo(50.5, 10.0), .LineTo(100.0, 40.0), .Close]),
                fill: .Some(.Const(.Rgba(30, 120, 60, 0.25))),
            }),
            shape.path({
                segments: .Const([.MoveTo(0.0, 40.0), .LineTo(50.5, 10.0), .LineTo(100.0, 40.0)]),
                stroke: .Some(.Const(.Inherit)),
                strokeWidth: .Some(.Const(2.0)),
            }),
        ],
    })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [sparkline(.Decorative), sparkline(.AccessibilityText("Sales this week"))],
        }),
        [],
    )
}
"#;

#[test]
fn a_drawing_is_svg_its_paths_are_painted_and_its_alt_names_it() {
    let printed = drive(
        "drawing-page",
        DRAWING_PAGE,
        r#"
const [decorative, named] = document.querySelectorAll("svg");
console.log(`namespace ${decorative.namespaceURI}`);
console.log(`decorative ${decorative.getAttribute("aria-hidden")} ${decorative.getAttribute("role")}`);
console.log(`named ${named.getAttribute("role")} ${named.getAttribute("aria-label")} ${named.getAttribute("aria-hidden")}`);
console.log(`box ${named.getAttribute("viewBox")} ${named.getAttribute("preserveAspectRatio")}`);
for (const path of named.querySelectorAll("path")) {
  console.log(`${path.namespaceURI} d=${path.getAttribute("d")} width=${path.getAttribute("stroke-width")} fill=${path.style.getPropertyValue("fill")} stroke=${path.style.getPropertyValue("stroke")}`);
}
"#,
    );
    assert_eq!(
        printed,
        "namespace http://www.w3.org/2000/svg\n\
         decorative true null\n\
         named img Sales this week null\n\
         box 0 0 100 40 none\n\
         http://www.w3.org/2000/svg d=M0 40 L50.5 10 L100 40 Z width=1 fill=rgba(30,120,60,0.25) stroke=none\n\
         http://www.w3.org/2000/svg d=M0 40 L50.5 10 L100 40 width=2 fill=none stroke=currentColor\n"
    );
}

/// A drawing whose size, outline, paint and width are signals, and a button
/// that writes all four.
const GROWING_DRAWING_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/shape" import * as shape;
from "ui/shape" import { Segment };
from "ui/signal" import { Signal, signal };
from "ui/style" import * as style;
from "ui/style" import { Color };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let size = signal(ctx, (10.0, 4.0));
    let segments: Signal<[Segment]> = signal(ctx, [.MoveTo(0.0, 4.0), .LineTo(10.0, 0.0)]);
    let ink = signal(ctx, Color.Rgb(1, 2, 3));
    let width = signal(ctx, 0.5);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.drawing({
                    alt: .AccessibilityText("Growth"),
                    size: .Cell(size),
                    styles: [],
                    shapes: [
                        shape.path({
                            segments: .Cell(segments),
                            fill: .Some(.Computed(fn(s) => ink.get(s).alpha(0.5))),
                            stroke: .Some(.Cell(ink)),
                            strokeWidth: .Some(.Cell(width)),
                        }),
                    ],
                }),
                ui.button({
                    label: .Const("Grow"),
                    styles: [],
                    onPress: .Some(fn(c) => {
                        let _ = size.set(c, (20.0, 8.0));
                        let _ = segments.set(c, [.MoveTo(0.0, 8.0), .LineTo(20.0, 0.0), .Close]);
                        let _ = ink.set(c, style.token("brand", "accent"));
                        width.set(c, 1.5)
                    }),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_drawing_follows_its_size_outline_and_paint_in_place() {
    let printed = drive(
        "growing-drawing-page",
        GROWING_DRAWING_PAGE,
        r#"
const svg = labelled("Growth");
const show = () => {
  const path = svg.querySelector("path");
  console.log(`${svg.getAttribute("viewBox")} d=${path.getAttribute("d")} width=${path.getAttribute("stroke-width")} fill=${path.style.getPropertyValue("fill")} stroke=${path.style.getPropertyValue("stroke")}`);
};
show();
labelled("Grow").click();
show();
console.log(`same ${labelled("Growth") === svg} paths ${svg.querySelectorAll("path").length}`);
"#,
    );
    assert_eq!(
        printed,
        "0 0 10 4 d=M0 4 L10 0 width=0.5 fill=rgba(1,2,3,0.5) stroke=rgb(1,2,3)\n\
         0 0 20 8 d=M0 8 L20 0 Z width=1.5 fill=color-mix(in srgb,var(--brand-accent) 50%,transparent) stroke=var(--brand-accent)\n\
         same true paths 1\n"
    );
}
