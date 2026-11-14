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

/// A toolbar of two buttons, a button beside it, a row the pointer can enter
/// and a drawing with one labelled bar. Each counts the times `onFocus` or
/// `onHover` told it the focus or the pointer arrived and left, and a line
/// under them shows the counts. `conformance/lib/ui/test/handlers.buri`
/// drives the same handlers headlessly.
const HANDLERS_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/shape" import * as shape;
from "ui/signal" import { Signal, signal };
from "web" import { WebHost };

fn counted<C: Ui>(arrived: Signal<Int>, left: Signal<Int>): Option<fn(C, Bool) => ()> {
    .Some(fn(c, now) => {
        if (now) {
            arrived.update(c, fn(n) => n + 1)
        } else {
            left.update(c, fn(n) => n + 1)
        }
    })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let boldIn = signal(ctx, 0);
    let boldOut = signal(ctx, 0);
    let italicIn = signal(ctx, 0);
    let italicOut = signal(ctx, 0);
    let barIn = signal(ctx, 0);
    let barOut = signal(ctx, 0);
    let sendIn = signal(ctx, 0);
    let sendOut = signal(ctx, 0);
    let rowIn = signal(ctx, 0);
    let rowOut = signal(ctx, 0);
    let janIn = signal(ctx, 0);
    let janOut = signal(ctx, 0);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.stack({
                    styles: [],
                    children: [
                        ui.button({
                            label: .Const("Bold"),
                            styles: [],
                            onPress: .Some(fn(_c) => ()),
                            onFocus: counted(boldIn, boldOut),
                        }),
                        ui.button({
                            label: .Const("Italic"),
                            styles: [],
                            onPress: .Some(fn(_c) => ()),
                            onFocus: counted(italicIn, italicOut),
                        }),
                    ],
                    onFocus: counted(barIn, barOut),
                }),
                ui.button({
                    label: .Const("Send"),
                    styles: [],
                    onPress: .Some(fn(_c) => ()),
                    onFocus: counted(sendIn, sendOut),
                }),
                ui.stack({
                    styles: [],
                    children: [ui.text({ content: .Const("Apples") })],
                    onHover: counted(rowIn, rowOut),
                }),
                ui.drawing({
                    alt: .AccessibilityText("Sales by month"),
                    size: .Const((10.0, 10.0)),
                    styles: [],
                    shapes: [
                        shape.rect({
                            at: .Const((0.0, 0.0)),
                            size: .Const((8.0, 10.0)),
                            label: .Some(.Const("Jan")),
                            onFocus: counted(janIn, janOut),
                        }),
                    ],
                }),
                ui.text({
                    content: .Computed(fn(s) => {
                        str.format(
                            s,
                            "bold ${boldIn.get(s)}/${boldOut.get(s)} italic ${italicIn.get(s)}/${italicOut.get(s)} toolbar ${barIn.get(s)}/${barOut.get(s)} send ${sendIn.get(s)}/${sendOut.get(s)} row ${rowIn.get(s)}/${rowOut.get(s)} jan ${janIn.get(s)}/${janOut.get(s)}",
                        )
                    }),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn moving_the_focus_and_the_pointer_runs_on_focus_and_on_hover() {
    let printed = drive(
        "handlers-page",
        HANDLERS_PAGE,
        r#"
const counts = () => body.firstChild.lastChild.textContent;
const row = [...document.querySelectorAll("div")].find((d) => d.textContent === "Apples");
labelled("Bold").focus();
console.log(counts());
labelled("Italic").focus();
console.log(counts());
labelled("Send").focus();
console.log(counts());
fire(row, "mouseenter");
fire(row, "mouseleave");
console.log(counts());
labelled("Jan").focus();
console.log(counts());
"#,
    );
    assert_eq!(
        printed,
        "bold 1/0 italic 0/0 toolbar 1/0 send 0/0 row 0/0 jan 0/0\n\
         bold 1/1 italic 1/0 toolbar 2/1 send 0/0 row 0/0 jan 0/0\n\
         bold 1/1 italic 1/1 toolbar 2/2 send 1/0 row 0/0 jan 0/0\n\
         bold 1/1 italic 1/1 toolbar 2/2 send 1/0 row 1/1 jan 0/0\n\
         bold 1/1 italic 1/1 toolbar 2/2 send 1/1 row 1/1 jan 1/0\n"
    );
}

/// One of each element a key acts on by itself: a dialog, a button, a
/// checkbox, a radio group, a slider, a disclosure, a form of one field and
/// a form whose submit button is disabled.
/// A line under them says what each holds. `conformance/lib/ui/test/keys.buri`
/// presses the same keys headlessly.
const KEYS_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/signal" import { signal };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let open = signal(ctx, true);
    let saves = signal(ctx, 0);
    let wifi = signal(ctx, false);
    let size = signal(ctx, "s");
    let level = signal(ctx, 4.0);
    let more = signal(ctx, false);
    let name = signal(ctx, "");
    let email = signal(ctx, "");
    let sent = signal(ctx, 0);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.dialog({
                    isOpen: open,
                    label: .Const("Delete this?"),
                    children: [
                        ui.button({ label: .Const("Done"), styles: [], onPress: .Some(fn(_c) => ()) }),
                    ],
                    styles: [],
                }),
                ui.button({
                    label: .Const("Save"),
                    styles: [],
                    onPress: .Some(fn(c) => saves.update(c, fn(n) => n + 1)),
                }),
                ui.toggle({ label: .Const("Wifi"), kind: .Checkbox, isOn: wifi, styles: [] }),
                ui.picker({
                    label: .Const("Size"),
                    options: [
                        ("s", ui.text({ content: .Const("Small") })),
                        ("m", ui.text({ content: .Const("Medium") })),
                        ("l", ui.text({ content: .Const("Large") })),
                    ],
                    value: size,
                    styles: [],
                }),
                ui.slider({
                    label: .Const("Level"),
                    value: level,
                    min: 0.0,
                    max: 20.0,
                    styles: [],
                    step: .Some(0.5),
                }),
                ui.disclosure({
                    summary: .Const("More"),
                    isOpen: more,
                    children: [ui.text({ content: .Const("details") })],
                    styles: [],
                }),
                ui.form({
                    onSubmit: fn(c) => { sent.update(c, fn(n) => n + 1) },
                    children: [ui.field({ label: .Const("Name"), kind: .Text, value: name, styles: [] })],
                    styles: [],
                }),
                ui.form({
                    onSubmit: fn(c) => { sent.update(c, fn(n) => n + 1) },
                    children: [
                        ui.field({ label: .Const("Email"), kind: .Text, value: email, styles: [] }),
                        ui.button({
                            label: .Const("Send"),
                            styles: [],
                            kind: .Some(.Submit),
                            isDisabled: .Some(.Const(true)),
                        }),
                    ],
                    styles: [],
                }),
                ui.text({
                    content: .Computed(fn(s) => {
                        str.format(
                            s,
                            "open:${open.get(s)} saves:${saves.get(s)} wifi:${wifi.get(s)} size:${size.get(s)} level:${level.get(s)} more:${more.get(s)} sent:${sent.get(s)}",
                        )
                    }),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_key_does_in_a_browser_what_it_does_headlessly() {
    let printed = drive(
        "keys-page",
        KEYS_PAGE,
        r#"
const said = () => body.firstChild.lastChild.textContent;
// The input a `<label>` holds beside its words.
const control = (name) => [...document.querySelectorAll("label")].find((l) => l.textContent === name).querySelector("input");
labelled("Done").focus();
press("Escape");
console.log(`${said()} dialog:${document.querySelector("dialog[open]") === null ? "shut" : "open"}`);
labelled("Save").focus();
press("Enter");
press(" ");
control("Wifi").focus();
press(" ");
press("Enter");
console.log(said());
control("Small").focus();
press("ArrowDown");
press("ArrowDown");
press("ArrowDown");
press("ArrowUp");
console.log(said());
control("Level").focus();
press("ArrowRight");
press("PageUp");
console.log(said());
press("End");
press("ArrowRight");
console.log(said());
press("Home");
press("ArrowLeft");
document.querySelector("summary").focus();
press("Enter");
control("Name").focus();
press("Enter");
// A disabled submit button keeps its form from being submitted.
control("Email").focus();
press("Enter");
console.log(said());
"#,
    );
    assert_eq!(
        printed,
        "open:false saves:0 wifi:false size:s level:4.0 more:false sent:0 dialog:shut\n\
         open:false saves:2 wifi:true size:s level:4.0 more:false sent:0\n\
         open:false saves:2 wifi:true size:l level:4.0 more:false sent:0\n\
         open:false saves:2 wifi:true size:l level:6.5 more:false sent:0\n\
         open:false saves:2 wifi:true size:l level:20.0 more:false sent:0\n\
         open:false saves:2 wifi:true size:l level:0.0 more:true sent:1\n"
    );
}

/// A button that opens a dialog, which its own button shuts and another
/// button disables the opener from; a disabled picker; and a line field and a
/// multiline field that keep their selection.
/// `conformance/lib/ui/test/{dialogs,disabled,caret}.buri` do the same
/// headlessly.
const HEADLESS_GAPS_PAGE: &str = r#"from "core/str" import * as str;
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
    let off = signal(ctx, false);
    let size = signal(ctx, "s");
    let caret = signal(ctx, (0, 0));
    let lines = signal(ctx, (0, 0));
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.button({
                    label: .Const("Delete"),
                    styles: [],
                    onPress: .Some(fn(c) => open.set(c, true)),
                    isDisabled: .Some(.Cell(off)),
                }),
                ui.button({ label: .Const("Other"), styles: [], onPress: .Some(fn(_c) => ()) }),
                ui.dialog({
                    isOpen: open,
                    label: .Const("Delete this?"),
                    children: [
                        ui.button({
                            label: .Const("Done"),
                            styles: [],
                            onPress: .Some(fn(c) => open.set(c, false)),
                        }),
                        ui.button({
                            label: .Const("Lock"),
                            styles: [],
                            onPress: .Some(fn(c) => off.set(c, true)),
                        }),
                    ],
                    styles: [],
                }),
                ui.picker({
                    label: .Const("Size"),
                    options: [
                        ("s", ui.text({ content: .Const("Small") })),
                        ("m", ui.text({ content: .Const("Medium") })),
                    ],
                    value: size,
                    styles: [],
                    isDisabled: .Some(.Const(true)),
                }),
                ui.field({
                    label: .Const("Note"),
                    kind: .Text,
                    value: signal(ctx, "a👍🏽b"),
                    styles: [],
                    selection: .Some(caret),
                }),
                ui.field({
                    label: .Const("Lines"),
                    kind: .Multiline,
                    value: signal(ctx, "ab\ncd"),
                    styles: [],
                    selection: .Some(lines),
                }),
                ui.text({
                    content: .Computed(fn(s) => {
                        let (a, b) = caret.get(s);
                        let (c, d) = lines.get(s);
                        str.format(s, "open:${open.get(s)} size:${size.get(s)} caret:${a},${b} lines:${c},${d}")
                    }),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn the_headless_gaps_close_the_way_a_browser_does() {
    let printed = drive(
        "headless-gaps-page",
        HEADLESS_GAPS_PAGE,
        r#"
const said = () => body.firstChild.lastChild.textContent;
// What the focused element is called: its label, or "nothing" for the body.
const focused = () => {
  const at = document.activeElement;
  if (at === body) return "nothing";
  return at.getAttribute("aria-label") ?? at.closest("label").firstChild.textContent;
};
const shown = () => console.log(`${said()} focus:${focused()}`);
// Escape gives the focus back to the button that opened the dialog.
labelled("Delete").focus();
press("Enter");
labelled("Done").focus();
press("Escape");
shown();
// So does the dialog's own button shutting it.
press("Enter");
labelled("Done").focus();
press("Enter");
shown();
// An opener that can no longer take the focus does not get it back.
press("Enter");
labelled("Lock").focus();
press("Enter");
press("Escape");
shown();
// A disabled picker refuses a click on its options.
document.querySelectorAll("input[type=radio]")[1].click();
shown();
// The caret keys, over an emoji and in a field of two lines.
const note = document.querySelector("input[type=text]");
note.focus();
note.setSelectionRange(1, 1);
press("ArrowRight");
shown();
press("ArrowLeft");
shown();
note.setSelectionRange(1, 5);
press("ArrowLeft");
shown();
press("End");
shown();
press("Home");
shown();
const lines = document.querySelector("textarea");
lines.focus();
lines.setSelectionRange(4, 4);
press("Home");
shown();
press("End");
press("ArrowUp");
press("ArrowDown");
shown();
"#,
    );
    assert_eq!(
        printed,
        "open:false size:s caret:0,0 lines:0,0 focus:Delete\n\
         open:false size:s caret:0,0 lines:0,0 focus:Delete\n\
         open:false size:s caret:0,0 lines:0,0 focus:nothing\n\
         open:false size:s caret:0,0 lines:0,0 focus:nothing\n\
         open:false size:s caret:5,5 lines:0,0 focus:Note\n\
         open:false size:s caret:1,1 lines:0,0 focus:Note\n\
         open:false size:s caret:1,1 lines:0,0 focus:Note\n\
         open:false size:s caret:6,6 lines:0,0 focus:Note\n\
         open:false size:s caret:0,0 lines:0,0 focus:Note\n\
         open:false size:s caret:0,0 lines:3,3 focus:Lines\n\
         open:false size:s caret:0,0 lines:5,5 focus:Lines\n"
    );
}

/// A line of the page behind an open dialog.
const SCRIM_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/signal" import { signal };
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
                ui.text({ content: .Const("Behind") }),
                ui.dialog({
                    isOpen: signal(ctx, true),
                    label: .Const("Settings"),
                    children: [ui.text({ content: .Const("Inside") })],
                    styles: [],
                }),
            ],
        }),
        [],
    )
}
"#;

/// The page behind an open dialog is under one scrim of black at half
/// strength, the one the painter draws. A browser paints the dialog's
/// `::backdrop` itself, so a box of the page's own that also painted black
/// would make the page about three quarters dark.
#[test]
fn a_browser_draws_one_scrim_behind_an_open_dialog() {
    let printed = drive(
        "scrim-page",
        SCRIM_PAGE,
        r#"
const dialog = document.querySelector("dialog[open]");
const layers = [];
// The `::backdrop` rules that reach the open dialog.
for (const sheet of document.styleSheets) {
  for (const rule of sheet.cssRules) {
    const [on, pseudo] = (rule.selectorText ?? "").split("::");
    if (pseudo === "backdrop" && dialog.matches(on)) layers.push(`backdrop ${rule.style.backgroundColor}`);
  }
}
// Every box outside the dialog that paints a background.
for (const element of body.querySelectorAll("*")) {
  if (dialog.contains(element)) continue;
  const colour = getComputedStyle(element).backgroundColor;
  if (colour !== "" && colour !== "transparent" && colour !== "rgba(0, 0, 0, 0)") {
    layers.push(`${element.localName} ${colour}`);
  }
}
console.log(layers.join("\n"));
"#,
    );
    assert_eq!(printed, "backdrop rgba(0, 0, 0, 0.5)\n");
}

/// Dialogs a browser moves the focus into as they open: one holding a field
/// after some text, one inside it, one with nothing to focus, one whose first
/// control is disabled and whose next takes the focus out of the tab order, and
/// one open from the start. Beside them, a disclosure, which leaves the focus
/// where it was. `conformance/lib/ui/test/dialogs.buri` checks the same
/// headlessly.
const DIALOG_FOCUS_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal, signal };
from "web" import { WebHost };

fn opener<C: Ui>(label: Str, open: Signal<Bool>): Node<C> {
    ui.button({ label: .Const(label), styles: [], onPress: .Some(fn(c) => open.set(c, true)) })
}

fn idle<C: Ui>(label: Str, off: Bool): Node<C> {
    ui.button({
        label: .Const(label),
        styles: [],
        onPress: .Some(fn(_c) => ()),
        isDisabled: .Some(.Const(off)),
    })
}

fn panel<C: Ui>(label: Str, open: Signal<Bool>, children: [Node<C>]): Node<C> {
    ui.dialog({ isOpen: open, label: .Const(label), children: children, styles: [] })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let start = signal(ctx, true);
    let open = signal(ctx, false);
    let deeper = signal(ctx, false);
    let note = signal(ctx, false);
    let ask = signal(ctx, false);
    let held = signal(ctx, false);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                panel("Welcome", start, [idle("Hello", false)]),
                opener("Open", open),
                panel("Details", open, [
                    ui.text({ content: .Const("Why?") }),
                    ui.field({ label: .Const("Reason"), kind: .Text, value: signal(ctx, ""), styles: [] }),
                    opener("Deeper", deeper),
                    panel("More details", deeper, [idle("Back", false)]),
                ]),
                opener("Note", note),
                panel("Saved", note, [ui.text({ content: .Const("Saved.") })]),
                opener("Ask", ask),
                panel("Question", ask, [
                    idle("Wait", true),
                    ui.stack({
                        styles: [],
                        children: [ui.text({ content: .Const("Notes") })],
                        hasFocus: .Some(held),
                        isInFocusOrder: .Some(.Const(false)),
                    }),
                    idle("Close", false),
                ]),
                ui.disclosure({
                    summary: .Const("More"),
                    isOpen: signal(ctx, false),
                    children: [idle("Inside", false)],
                    styles: [],
                }),
                ui.text({ content: .Computed(fn(s) => str.format(s, "held:${held.get(s)}")) }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_browser_moves_the_focus_into_a_dialog_that_opens() {
    let printed = drive(
        "dialog-focus-page",
        DIALOG_FOCUS_PAGE,
        r#"
const said = () => body.firstChild.lastChild.textContent;
// What the focused element is called: its name, or "nothing" for the body.
const focused = () => {
  const at = document.activeElement;
  if (at === body) return "nothing";
  return at.getAttribute("aria-label") ?? at.closest("label")?.firstChild.textContent ?? at.textContent;
};
const shown = () => console.log(`${said()} focus:${focused()}`);
// A dialog open from the start has the focus.
shown();
press("Escape");
shown();
// Opening moves the focus past the text to the field.
labelled("Open").focus();
press("Enter");
shown();
// A dialog opened inside it takes the focus, and gives it back.
labelled("Deeper").focus();
press("Enter");
shown();
press("Escape");
shown();
press("Escape");
shown();
// Nothing to focus takes the focus off the opener; shutting gives it back.
labelled("Note").focus();
press("Enter");
shown();
press("Escape");
shown();
// A disabled control is passed over; a box out of the tab order is not.
labelled("Ask").focus();
press("Enter");
shown();
press("Escape");
shown();
// A disclosure leaves the focus on its summary.
document.querySelector("summary").focus();
press("Enter");
console.log(`open:${document.querySelector("details").open} focus:${focused()}`);
"#,
    );
    assert_eq!(
        printed,
        "held:false focus:Hello\n\
         held:false focus:nothing\n\
         held:false focus:Reason\n\
         held:false focus:Back\n\
         held:false focus:Deeper\n\
         held:false focus:Open\n\
         held:false focus:nothing\n\
         held:false focus:Note\n\
         held:true focus:Notes\n\
         held:false focus:Ask\n\
         open:true focus:More\n"
    );
}

/// One of each element that has an accessible name, beside a decorative
/// picture and a decorative drawing. `conformance/lib/ui/test/names.buri`
/// reads the same names through `spoken()`.
const NAMES_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/shape" import * as shape;
from "ui/signal" import { signal };
from "web" import { WebHost };

fn cross<C>(): ui.Node<C> {
    ui.image({
        source: .Const("<svg viewBox=\"0 0 10 10\"><path d=\"M0 0L10 10\"/></svg>"),
        alt: .Decorative,
        styles: [],
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
            children: [
                ui.button({
                    label: .Const("Close"),
                    styles: [],
                    onPress: .Some(fn(_c) => ()),
                    children: .Some([cross()]),
                }),
                ui.button({
                    label: .Const("Add to basket"),
                    styles: [],
                    onPress: .Some(fn(_c) => ()),
                    children: .Some([ui.text({ content: .Const("Add") })]),
                }),
                ui.image({ source: .Const("logo.png"), alt: .AccessibilityText("Acme"), styles: [] }),
                ui.drawing({
                    alt: .AccessibilityText("Sales by month"),
                    size: .Const((20.0, 10.0)),
                    styles: [],
                    shapes: [
                        shape.rect({
                            at: .Const((0.0, 0.0)),
                            size: .Const((8.0, 10.0)),
                            label: .Some(.Const("Jan: 10")),
                        }),
                        shape.rect({ at: .Const((10.0, 0.0)), size: .Const((8.0, 10.0)) }),
                    ],
                }),
                ui.drawing({
                    alt: .Decorative,
                    size: .Const((10.0, 10.0)),
                    styles: [],
                    shapes: [
                        shape.rect({
                            at: .Const((0.0, 0.0)),
                            size: .Const((8.0, 10.0)),
                            label: .Some(.Const("Feb")),
                        }),
                    ],
                }),
                ui.stack({
                    styles: [],
                    children: [ui.text({ content: .Const("Home") })],
                    role: .Some(.Navigation),
                    label: .Some(.Const("Primary")),
                }),
                ui.picker({
                    label: .Const("Size"),
                    options: [
                        ("s", ui.text({ content: .Const("Small") })),
                        ("l", ui.text({ content: .Const("Large") })),
                    ],
                    value: signal(ctx, "s"),
                    styles: [],
                }),
                ui.progress({ label: .Const("Uploading"), value: .Some(.Const(0.5)), styles: [] }),
                ui.form({
                    onSubmit: fn(_c) => (),
                    children: [ui.button({ label: .Const("Send"), styles: [], kind: .Some(.Submit) })],
                    styles: [],
                }),
                ui.dialog({
                    isOpen: signal(ctx, true),
                    label: .Const("Delete this?"),
                    children: [ui.text({ content: .Const("Are you sure?") })],
                    styles: [],
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_browser_hears_each_element_by_the_name_spoken_gives_it() {
    // A reader's walk of the accessibility tree: nothing hidden, each element's
    // name where it sits, and a button's or a picture's children presentational
    // but for a stop in the focus order inside it.
    let printed = drive(
        "names-page",
        NAMES_PAGE,
        r#"
const heard = (node, out, presentational) => {
  if (node.nodeType === 3) {
    const words = node.textContent.trim();
    if (words !== "" && !presentational) out.push(words);
    return;
  }
  if (node.nodeType !== 1 || node.getAttribute("aria-hidden") === "true") return;
  if (node.tagName === "DIALOG" && !node.open) return;
  const stop = node.getAttribute("tabindex") === "0";
  const name = node.getAttribute("aria-label") ?? (node.tagName === "IMG" ? node.getAttribute("alt") : null);
  if (name && (stop || !presentational)) out.push(name);
  const role = node.getAttribute("role");
  const inner = presentational || node.tagName === "BUTTON" || role === "button" || role === "img";
  for (const child of node.childNodes) heard(child, out, inner);
};
const out = [];
heard(body, out, false);
console.log(out.join(" "));
"#,
    );
    assert_eq!(
        printed,
        "Close Add to basket Acme Sales by month Jan: 10 Primary Home Size Small Large Uploading Send Delete this? Are you sure?\n"
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
                    ui.text({ content: .Computed(fn(t) => str.format(t, "${s}:${i.read(t) % 2}")) })
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
from "ui/prop" import { Prop };
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

fn named<C: Clock + Location + Ui>(ctx: C, s: Str, _i: Prop<Int>): Node<C> {
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

fn maker<C: Clock + Location + Ui>(): fn(C, Str, Prop<Int>) => Node<C> {
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
from "ui/prop" import { Prop };
from "web" import { WebHost };

fn row<C: Clock + Ui>(ctx: C, s: Str): Node<C> {
    let _ = time.sleep(ctx, time.milliseconds(1));
    ui.text({ content: .Const(s) })
}

fn list<C: Clock + Ui>(rows: fn(C, Str, Prop<Int>) => Node<C>): Node<C> {
    ui.each(.Const(["b"]), fn(s) => s, fn(c, s, i) => rows(c, s, i))
}

fn deep<C: Clock + Ui>(n: Int, rows: fn(C, Str, Prop<Int>) => Node<C>): Node<C> {
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
    assert!(printed.contains("main.buri:36:"), "{printed}");
    assert!(printed.contains("main.buri:37:"), "{printed}");
    assert!(printed.contains("main.buri:38:"), "{printed}");
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
         0 0 20 8 d=M0 8 L20 0 Z width=1.5 fill=color-mix(in srgb,var(--brand-accent,currentColor) 50%,transparent) stroke=var(--brand-accent, currentColor)\n\
         same true paths 1\n"
    );
}

/// One bar per month, keyed by name: each bar a group moved along by its
/// total, holding a rectangle and a dot, all in a group with no offset. A
/// button reorders the months and drops one.
const BARS_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/shape" import * as shape;
from "ui/signal" import { signal };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let months = signal(ctx, [("Jan", 10.0), ("Feb", 20.0), ("Mar", 30.0)]);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.drawing({
                    alt: .AccessibilityText("Sales by month"),
                    size: .Const((30.0, 40.0)),
                    styles: [],
                    shapes: [
                        // Left out, the offset adds no element: the rows are the
                        // drawing's own children.
                        shape.group({
                            offset: .None,
                            shapes: [
                                shape.each(.Cell(months), fn(m) => m.0, fn(_c, m, _i) => {
                                    shape.group({
                                        offset: .Some(.Const((m.1 - 10.0, 0.0))),
                                        shapes: [
                                            shape.rect({
                                                at: .Const((1.0, 40.0 - m.1)),
                                                size: .Const((8.0, m.1)),
                                                fill: .Some(.Const(.Rgb(30, 120, 60))),
                                            }),
                                            shape.circle({ center: .Const((5.0, 40.0 - m.1)), radius: .Const(1.5) }),
                                        ],
                                    })
                                }),
                            ],
                        }),
                    ],
                }),
                ui.button({
                    label: .Const("Shuffle"),
                    styles: [],
                    onPress: .Some(fn(c) => months.set(c, [("Mar", 30.0), ("Jan", 10.0)])),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_drawing_s_keyed_shapes_move_with_their_items_and_keep_their_elements() {
    let printed = drive(
        "bars-page",
        BARS_PAGE,
        r#"
const svg = labelled("Sales by month");
const show = () => {
  for (const group of svg.children) {
    const rect = group.querySelector("rect");
    const dot = group.querySelector("circle");
    console.log(`${group.namespaceURI} ${group.tagName} ${group.getAttribute("transform")} rect ${rect.getAttribute("x")} ${rect.getAttribute("y")} ${rect.getAttribute("width")} ${rect.getAttribute("height")} ${rect.style.getPropertyValue("fill")} dot ${dot.getAttribute("cx")} ${dot.getAttribute("cy")} ${dot.getAttribute("r")} ${dot.style.getPropertyValue("fill")}`);
  }
};
show();
const [jan, , mar] = svg.children;
labelled("Shuffle").click();
show();
const [first, second] = svg.children;
console.log(`kept ${first === mar} ${second === jan} count ${svg.children.length}`);
"#,
    );
    assert_eq!(
        printed,
        "http://www.w3.org/2000/svg g translate(0 0) rect 1 30 8 10 rgb(30,120,60) dot 5 30 1.5 none\n\
         http://www.w3.org/2000/svg g translate(10 0) rect 1 20 8 20 rgb(30,120,60) dot 5 20 1.5 none\n\
         http://www.w3.org/2000/svg g translate(20 0) rect 1 10 8 30 rgb(30,120,60) dot 5 10 1.5 none\n\
         http://www.w3.org/2000/svg g translate(20 0) rect 1 10 8 30 rgb(30,120,60) dot 5 10 1.5 none\n\
         http://www.w3.org/2000/svg g translate(0 0) rect 1 30 8 10 rgb(30,120,60) dot 5 30 1.5 none\n\
         kept true true count 2\n"
    );
}

/// One bar per month placed by its index, the months listed beside it with
/// their indices, and a button that reorders the months.
const PLACED_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/shape" import * as shape;
from "ui/signal" import { signal };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let months = signal(ctx, ["Jan", "Feb", "Mar"]);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.drawing({
                    alt: .AccessibilityText("Sales by month"),
                    size: .Const((30.0, 40.0)),
                    styles: [],
                    shapes: [
                        shape.each(.Cell(months), fn(m) => m, fn(_c, _m, i) => {
                            shape.group({
                                offset: .Some(.Computed(fn(s) => (i.read(s).toF64() * 10.0, 0.0))),
                                shapes: [shape.rect({ at: .Const((1.0, 0.0)), size: .Const((8.0, 10.0)) })],
                            })
                        }),
                    ],
                }),
                ui.each(.Cell(months), fn(m) => m, fn(_c, m, i) => {
                    ui.text({ content: .Computed(fn(s) => str.format(s, "${m}:${i.read(s)}")) })
                }),
                ui.button({
                    label: .Const("Shuffle"),
                    styles: [],
                    onPress: .Some(fn(c) => months.set(c, ["Mar", "Jan", "Apr"])),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_row_s_index_follows_its_place_after_a_reorder() {
    let printed = drive(
        "placed-page",
        PLACED_PAGE,
        r#"
const svg = labelled("Sales by month");
const show = () => {
  console.log([...svg.children].map((g) => g.getAttribute("transform")).join(" | "));
  console.log(body.textContent.replace("Shuffle", ""));
};
show();
const [jan, , mar] = svg.children;
labelled("Shuffle").click();
show();
console.log(`kept ${svg.children[0] === mar} ${svg.children[1] === jan}`);
"#,
    );
    assert_eq!(
        printed,
        "translate(0 0) | translate(10 0) | translate(20 0)\n\
         Jan:0Feb:1Mar:2\n\
         translate(0 0) | translate(10 0) | translate(20 0)\n\
         Mar:0Jan:1Apr:2\n\
         kept true true\n"
    );
}

/// A label in a drawing, whose words are a signal, and a button that changes
/// them to words that look like markup.
const LABEL_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/shape" import * as shape;
from "ui/signal" import { signal };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let says = signal(ctx, "Jan");
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.drawing({
                    alt: .AccessibilityText("Sales by month"),
                    size: .Const((10.0, 50.0)),
                    styles: [],
                    shapes: [shape.text({ at: .Const((5.0, 48.5)), content: .Cell(says) })],
                }),
                ui.button({
                    label: .Const("Rename"),
                    styles: [],
                    onPress: .Some(fn(c) => says.set(c, "<b>Q1 & Q2</b>")),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_drawing_s_text_is_centred_on_its_point_and_follows_its_words_in_place() {
    let printed = drive(
        "label-page",
        LABEL_PAGE,
        r#"
const svg = labelled("Sales by month");
const show = () => {
  const text = svg.querySelector("text");
  console.log(`${text.namespaceURI} x=${text.getAttribute("x")} y=${text.getAttribute("y")} anchor=${text.getAttribute("text-anchor")} size=${text.getAttribute("font-size")} fill=${text.style.getPropertyValue("fill")} says=${text.textContent} children=${text.children.length}`);
};
show();
const before = svg.querySelector("text");
labelled("Rename").click();
show();
console.log(`same ${svg.querySelector("text") === before} texts ${svg.querySelectorAll("text").length}`);
"#,
    );
    assert_eq!(
        printed,
        "http://www.w3.org/2000/svg x=5 y=48.5 anchor=middle size=5 fill=currentColor says=Jan children=0\n\
         http://www.w3.org/2000/svg x=5 y=48.5 anchor=middle size=5 fill=currentColor says=<b>Q1 & Q2</b> children=0\n\
         same true texts 1\n"
    );
}

/// One labelled bar per month, a dot with no label, a line saying which bar has
/// the focus, and a button that adds a word to every label.
const FOCUS_BARS_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/shape" import * as shape;
from "ui/signal" import { signal };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let months = signal(ctx, [("Jan", 10.0), ("Feb", 20.0)]);
    let suffix = signal(ctx, "");
    let picked = signal(ctx, "nothing");
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.drawing({
                    alt: .AccessibilityText("Sales by month"),
                    size: .Const((20.0, 40.0)),
                    styles: [],
                    shapes: [
                        shape.each(.Cell(months), fn(m) => m.0, fn(_c, m, i) => {
                            let (name, total) = m;
                            shape.rect({
                                at: .Computed(fn(s) => (i.read(s).toF64() * 10.0, 40.0 - total)),
                                size: .Const((8.0, total)),
                                label: .Some(.Computed(fn(s) => str.format(s, "${name}: ${total}${suffix.get(s)}"))),
                                onFocus: .Some(fn(c, now) => picked.set(c, if (now) { name } else { "nothing" })),
                            })
                        }),
                        shape.circle({ center: .Const((5.0, 5.0)), radius: .Const(1.0) }),
                    ],
                }),
                ui.text({ content: .Computed(fn(s) => str.format(s, "picked ${picked.get(s)}")) }),
                ui.button({
                    label: .Const("Units"),
                    styles: [],
                    onPress: .Some(fn(c) => suffix.set(c, " sold")),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_drawing_s_labelled_shapes_take_the_focus_and_are_read_by_their_label() {
    let printed = drive(
        "focus-bars-page",
        FOCUS_BARS_PAGE,
        r#"
const svg = labelled("Sales by month");
const show = () => {
  for (const shape of svg.querySelectorAll("rect, circle")) {
    console.log(`${shape.tagName} tabindex=${shape.getAttribute("tabindex")} role=${shape.getAttribute("role")} label=${shape.getAttribute("aria-label")}`);
  }
};
const picked = () => console.log(body.textContent.match(/picked (Jan|Feb|nothing)/)[0]);
show();
const [jan, feb] = svg.querySelectorAll("rect");
jan.focus();
picked();
feb.focus();
picked();
feb.blur();
picked();
labelled("Units").click();
show();
"#,
    );
    assert_eq!(
        printed,
        "rect tabindex=0 role=img label=Jan: 10.0\n\
         rect tabindex=0 role=img label=Feb: 20.0\n\
         circle tabindex=null role=null label=null\n\
         picked Jan\n\
         picked Feb\n\
         picked nothing\n\
         rect tabindex=0 role=img label=Jan: 10.0 sold\n\
         rect tabindex=0 role=img label=Feb: 20.0 sold\n\
         circle tabindex=null role=null label=null\n"
    );
}

/// A drawing 100 by 40 units that writes where the pointer is, in its units.
const POINTER_DRAWING_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/signal" import { signal };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let at = signal(ctx, "nowhere");
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.drawing({
                    alt: .AccessibilityText("Visitors"),
                    size: .Const((100.0, 40.0)),
                    styles: [],
                    shapes: [],
                    onPointerMove: .Some(fn(c, p) => at.set(c, str.format(c, "at ${p.x} ${p.y} viewport ${p.viewportX} ${p.viewportY}"))),
                }),
                ui.text({ content: .Cell(at) }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_drawing_hears_the_pointer_in_its_own_units() {
    let printed = drive(
        "pointer-drawing-page",
        POINTER_DRAWING_PAGE,
        r#"
const svg = labelled("Visitors");
// Laid out 200 by 80 pixels, 10 from the left and 20 from the top.
svg.getBoundingClientRect = () => ({ left: 10, top: 20, right: 210, bottom: 100, width: 200, height: 80, x: 10, y: 20 });
fire(svg, "pointermove", { clientX: 110, clientY: 40 });
console.log(body.textContent);
"#,
    );
    assert_eq!(printed, "at 50.0 10.0 viewport 110.0 40.0\n");
}

/// The four drawings #332's approved interface was shown through, as the
/// proposal wrote them: a sparkline, a themed area chart, one bar per month and
/// a marker that follows the pointer. A bar reads its index through a
/// `.Computed`, because a row's index is a `Prop<Int>`. Below them, which bar
/// has the focus, and buttons that change the values and reorder the months.
/// `conformance/lib/ui/test/drawings.buri` renders the same four with no
/// browser.
const PROPOSAL_PAGE: &str = r#"from "core/math" import * as math;
from "core/str" import * as str;
from "platform/effect" import { Allocator, Scope, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/prop" import { Prop };
from "ui/shape" import * as shape;
from "ui/shape" import { Segment };
from "ui/signal" import { Signal, signal };
from "ui/style" import * as style;
from "web" import { WebHost };

struct Month {
    name: Str,
    total: Float,
}

derive Equal for Month;

/// A line through `values`, across a drawing 100 units wide and 40 tall.
fn trace(ctx: Scope, values: [Float]): [Segment] {
    let step = 100.0 / (values.length() - 1).toF64();
    values.mapIndexed(ctx, fn(i, v) => {
        if (i == 0) {
            .MoveTo(0.0, 40.0 - v)
        } else {
            .LineTo(i.toF64() * step, 40.0 - v)
        }
    })
}

fn sparkline<C>(values: Signal<[Float]>): Node<C> {
    ui.drawing({
        alt: .Decorative,
        size: .Const((100.0, 40.0)),
        styles: [.Width(.Px(80)), .Height(.Px(24))],
        shapes: [
            shape.path({
                segments: .Computed(fn(s) => trace(s, values.get(s))),
                stroke: .Some(.Const(.Inherit)),
            }),
        ],
    })
}

fn visitors<C>(values: Signal<[Float]>): Node<C> {
    let accent = style.token("app", "accent");
    ui.drawing({
        alt: .AccessibilityText("Visitors over the last 30 days"),
        size: .Const((100.0, 40.0)),
        styles: [.Width(.Full), .Height(.Px(160))],
        shapes: [
            shape.path({
                segments: .Computed(fn(s) => {
                    trace(s, values.get(s)).concat(s, [
                        .LineTo(100.0, 40.0),
                        .LineTo(0.0, 40.0),
                        .Close,
                    ])
                }),
                fill: .Some(.Const(accent.alpha(0.2))),
            }),
            shape.path({
                segments: .Computed(fn(s) => trace(s, values.get(s))),
                stroke: .Some(.Const(accent)),
                strokeWidth: .Some(.Const(1.5)),
            }),
        ],
    })
}

fn monthly<C: Ui>(months: Prop<[Month]>, picked: Signal<Str>): Node<C> {
    ui.drawing({
        alt: .AccessibilityText("Sales by month"),
        size: .Const((120.0, 50.0)),
        styles: [.Width(.Full), .Height(.Px(200))],
        shapes: [
            shape.each(months, fn(m) => m.name, fn(c, m, i) => {
                shape.group({
                    offset: .Some(.Computed(fn(s) => (i.read(s).toF64() * 10.0, 0.0))),
                    shapes: [
                        shape.rect({
                            at: .Const((1.0, 40.0 - m.total)),
                            size: .Const((8.0, m.total)),
                            fill: .Some(.Const(style.token("app", "accent"))),
                            label: .Some(
                                .Computed(fn(s) => str.format(s, "${m.name}: ${m.total}")),
                            ),
                            onFocus: .Some(fn(c, now) => {
                                picked.set(c, if (now) { m.name } else { "" })
                            }),
                        }),
                        shape.text({ at: .Const((5.0, 48.0)), content: .Const(m.name) }),
                    ],
                })
            }),
        ],
    })
}

fn readout<C: Ui>(values: Signal<[Float]>, pointerX: Signal<Float>): Node<C> {
    ui.drawing({
        alt: .AccessibilityText("Visitors over the last 30 days"),
        size: .Const((100.0, 40.0)),
        styles: [.Width(.Full), .Height(.Px(160))],
        onPointerMove: .Some(fn(c, at) => pointerX.set(c, at.x)),
        shapes: [
            shape.path({
                segments: .Computed(fn(s) => trace(s, values.get(s))),
                stroke: .Some(.Const(.Inherit)),
            }),
            shape.circle({
                center: .Computed(fn(s) => {
                    let all = values.get(s);
                    let step = 100.0 / (all.length() - 1).toF64();
                    let i = math.round(pointerX.get(s) / step).toI64().withDefault(0);
                    (i.toF64() * step, 40.0 - all.getOr(i, 0.0))
                }),
                radius: .Const(1.5),
                fill: .Some(.Const(.Inherit)),
            }),
        ],
    })
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let values = signal(ctx, [10.0, 30.0, 20.0]);
    let jan = Month { name: "Jan", total: 10.0 };
    let feb = Month { name: "Feb", total: 30.0 };
    let months = signal(ctx, [jan, feb]);
    let picked = signal(ctx, "");
    let pointerX = signal(ctx, 0.0);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                sparkline(values),
                visitors(values),
                monthly(.Cell(months), picked),
                readout(values, pointerX),
                ui.text({ content: .Computed(fn(s) => str.format(s, "picked ${picked.get(s)};")) }),
                ui.button({
                    label: .Const("Redraw"),
                    styles: [],
                    onPress: .Some(fn(c) => values.set(c, [0.0, 40.0])),
                }),
                ui.button({
                    label: .Const("Swap"),
                    styles: [],
                    onPress: .Some(fn(c) => months.set(c, [feb, jan])),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn the_proposal_s_drawings_work_in_a_browser() {
    let printed = drive(
        "proposal-page",
        PROPOSAL_PAGE,
        r#"
const [spark, area, bars, marker] = document.querySelectorAll("svg");
const paths = (svg) => [...svg.querySelectorAll("path")]
  .map((p) => `${p.getAttribute("d")} fill=${p.style.getPropertyValue("fill")} stroke=${p.style.getPropertyValue("stroke")} width=${p.getAttribute("stroke-width")}`)
  .join(" | ");
const groups = () => [...bars.children]
  .map((g) => `${g.getAttribute("transform")} ${g.querySelector("rect").getAttribute("aria-label")} ${g.querySelector("text").textContent}`)
  .join(" | ");
const dot = () => { const c = marker.querySelector("circle"); return `${c.getAttribute("cx")} ${c.getAttribute("cy")}`; };
const picked = () => body.textContent.match(/picked [A-Za-z]*;/)[0];
console.log(`spark hidden=${spark.getAttribute("aria-hidden")} ${paths(spark)}`);
console.log(`area ${area.getAttribute("role")} ${paths(area)}`);
console.log(`bars ${groups()}`);
const [janBar, febBar] = bars.querySelectorAll("rect");
febBar.focus();
console.log(picked());
labelled("Swap").click();
console.log(`bars ${groups()} kept ${bars.children[0].querySelector("rect") === febBar} focus ${document.activeElement === febBar}`);
febBar.blur();
console.log(picked());
console.log(`dot ${dot()}`);
marker.getBoundingClientRect = () => ({ left: 0, top: 0, right: 200, bottom: 80, width: 200, height: 80, x: 0, y: 0 });
fire(marker, "pointermove", { clientX: 120, clientY: 10 });
console.log(`dot ${dot()}`);
labelled("Redraw").click();
console.log(`spark ${paths(spark)}`);
console.log(`dot ${dot()}`);
"#,
    );
    assert_eq!(
        printed,
        "spark hidden=true M0 30 L50 10 L100 20 fill=none stroke=currentColor width=1\n\
         area img M0 30 L50 10 L100 20 L100 40 L0 40 Z fill=color-mix(in srgb,var(--app-accent,currentColor) 20%,transparent) stroke=none width=1 | M0 30 L50 10 L100 20 fill=none stroke=var(--app-accent, currentColor) width=1.5\n\
         bars translate(0 0) Jan: 10.0 Jan | translate(10 0) Feb: 30.0 Feb\n\
         picked Feb;\n\
         bars translate(0 0) Feb: 30.0 Feb | translate(10 0) Jan: 10.0 Jan kept true focus true\n\
         picked ;\n\
         dot 0 30\n\
         dot 50 10\n\
         spark M0 40 L100 0 fill=none stroke=currentColor width=1\n\
         dot 100 0\n"
    );
}

/// Two navigation landmarks a reader tells apart by name, a group of drawn
/// bars heard as one summary, and a button that changes that summary.
const LABELLED_STACKS_PAGE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/signal" import { signal };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
    };
    let high = signal(ctx, 40);
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.stack({ styles: [], children: [], role: .Some(.Navigation), label: .Some(.Const("Primary")) }),
                ui.stack({ styles: [], children: [], role: .Some(.Navigation), label: .Some(.Const("Breadcrumb")) }),
                ui.stack({
                    styles: [.Layout(.Layers)],
                    children: [ui.stack({ styles: [], children: [], isDecorative: .Some(true) })],
                    role: .Some(.Group),
                    label: .Some(.Computed(fn(s) => str.format(s, "Value over the past year, from 12 to ${high.get(s)}"))),
                }),
                ui.stack({ styles: [], children: [], role: .Some(.List) }),
                ui.button({ label: .Const("Raise"), styles: [], onPress: .Some(fn(c) => high.set(c, 50)) }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_stack_with_a_role_is_named_by_its_label() {
    let printed = drive(
        "labelled-stacks-page",
        LABELLED_STACKS_PAGE,
        r#"
const named = () => [...body.firstChild.children]
  .map((e) => `${e.tagName.toLowerCase()} ${e.getAttribute("role")} ${e.getAttribute("aria-label")}`)
  .join(" | ");
console.log(named());
labelled("Raise").click();
console.log(named());
"#,
    );
    assert_eq!(
        printed,
        "nav null Primary | nav null Breadcrumb | div group Value over the past year, from 12 to 40 | ul null null | button null Raise\n\
         nav null Primary | nav null Breadcrumb | div group Value over the past year, from 12 to 50 | ul null null | button null Raise\n"
    );
}

/// A phone-only menu button, links shown only from `.Medium` up, and a heading
/// shown only between `.Small` and `.Large`, which has no `Layout` to restore.
const REMOVED_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
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
            styles: [.Layout(.Row)],
            children: [
                ui.button({
                    label: .Const("Menu"),
                    styles: [.At(.Medium, [.Removed(true)])],
                    onPress: .Some(fn(_c) => ()),
                }),
                ui.stack({
                    styles: [.Layout(.Row), .Removed(true), .At(.Medium, [.Removed(false)])],
                    children: [
                        ui.link({ dest: .Const("/a"), children: [ui.text({ content: .Const("A") })], styles: [] }),
                    ],
                }),
                ui.text({
                    content: .Const("Title"),
                    headingLevel: .Some(2),
                    styles: .Some([.Removed(true), .At(.Small, [.Removed(false)]), .At(.Large, [.Removed(true)])]),
                }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn an_element_removed_at_a_breakpoint_is_gone_there_and_back_elsewhere() {
    let printed = drive(
        "removed-page",
        REMOVED_PAGE,
        r#"
import { readFileSync } from "node:fs";
const sheet = document.createElement("style");
sheet.textContent = readFileSync(new URL("./.buri/out/web/cmd/page/main.css", import.meta.url), "utf8");
document.head.appendChild(sheet);
// What a reader reaches: an element whose every box up to the body is displayed.
const shown = (el) => {
  for (let at = el; at !== null && at !== body; at = at.parentElement) {
    if (getComputedStyle(at).display === "none") return "gone";
  }
  return getComputedStyle(el).display;
};
const menu = labelled("Menu");
const link = document.querySelector("a");
const heading = document.querySelector("h2");
for (const width of [390, 700, 800, 1100]) {
  window.happyDOM.setViewport({ width, height: 600 });
  // happy-dom keeps a computed style until the document changes, and a resize
  // isn't a change to it, so the sheet is put in again.
  sheet.textContent += " ";
  console.log(`${width} menu:${shown(menu)} links:${shown(link.parentElement)} link:${shown(link)} heading:${shown(heading)}`);
}
"#,
    );
    assert_eq!(
        printed,
        "390 menu:flex links:gone link:gone heading:gone\n\
         700 menu:flex links:gone link:gone heading:block\n\
         800 menu:gone links:flex link:flex heading:block\n\
         1100 menu:gone links:flex link:flex heading:gone\n"
    );
}

/// Tabular figures on a column of totals, proportional again from `.Medium` up,
/// read off a total rather than the stack that names them.
const FIGURES_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
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
            styles: [.TextAlign(.End), .Figures(.Tabular), .At(.Medium, [.Figures(.Proportional)])],
            children: [
                ui.text({ content: .Const("1,111.11"), headingLevel: .Some(2), styles: .None }),
                ui.text({ content: .Const("10,000.00") }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn figures_on_a_column_reach_the_numbers_in_it_at_each_width() {
    let printed = drive(
        "figures-page",
        FIGURES_PAGE,
        r#"
import { readFileSync } from "node:fs";
const css = readFileSync(new URL("./.buri/out/web/cmd/page/main.css", import.meta.url), "utf8");
// happy-dom drops `font-variant-numeric` from every style it parses, so the
// value is found the way a browser finds it: the last rule whose media query
// and selector match, on the element or else on the nearest ancestor, since
// the property inherits.
const rules = [];
const media = [];
for (const [, query, close, selector, body] of css.matchAll(/\s*(?:@media([^{]+)\{|(\})|([^{}@]+)\{([^}]*)\})/g)) {
  if (query !== undefined) media.push(query.trim());
  else if (close !== undefined) media.pop();
  const value = body?.match(/font-variant-numeric:([a-z-]+)/)?.[1];
  if (value) rules.push({ media: [...media], selector: selector.trim(), value });
}
const figures = (el) => {
  for (let at = el; at !== null; at = at.parentElement) {
    const found = rules.filter((r) => r.media.every((q) => window.matchMedia(q).matches) && at.matches(r.selector));
    if (found.length > 0) return found[found.length - 1].value;
  }
  return "normal";
};
const total = document.querySelector("h2");
for (const width of [390, 800]) {
  window.happyDOM.setViewport({ width, height: 600 });
  console.log(`${width} ${figures(total)}`);
}
"#,
    );
    assert_eq!(printed, "390 tabular-nums\n800 proportional-nums\n");
}

/// One faded group of each kind a snapshot fades as one picture (#343), each
/// in a stack named for it: every stack layout, a box with a border, a
/// drawing, an icon's SVG group, an SVG shape with a stroke, and a stack with
/// a pinned child and a fixed one.
const FADED_GROUPS_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/shape" import * as shape;
from "ui/style" import { Layout, Style };
from "web" import { WebHost };

fn named<C>(name: Str, subject: Node<C>): Node<C> {
    ui.stack({ styles: [], children: [subject], role: .Some(.Group), label: .Some(.Const(name)) })
}

fn square<C>(extra: [Style]): Node<C> {
    ui.stack({
        styles: [.Width(.Px(40)), .Height(.Px(40)), .Background(.Rgb(0, 0, 0)), .Group(extra)],
        children: [],
    })
}

fn half<C>(layout: Layout, extra: [Style], children: [Node<C>]): Node<C> {
    ui.stack({ styles: [.Layout(layout), .Opacity(0.5), .Group(extra)], children: children })
}

fn pair<C>(name: Str, layout: Layout): Node<C> {
    named(name, half(layout, [], [square([]), square([.Translate(.Px(-20), .Px(0))])]))
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
            children: [
                pair("layers", .Layers),
                pair("column", .Column),
                pair("row", .Row),
                pair("column reverse", .ColumnReverse),
                pair("row reverse", .RowReverse),
                pair("grid", .Grid([.Fixed(.Px(40)), .Fixed(.Px(40))])),
                named("border", square([.BorderWidth(.Px(4)), .BorderColor(.Rgb(0, 0, 220)), .Opacity(0.5)])),
                named(
                    "drawing",
                    ui.drawing({
                        alt: .AccessibilityText("Two squares"),
                        size: .Const((60.0, 40.0)),
                        styles: [.Width(.Px(60)), .Height(.Px(40)), .Opacity(0.5)],
                        shapes: [
                            shape.rect({ at: .Const((0.0, 0.0)), size: .Const((40.0, 40.0)), fill: .Some(.Const(.Rgb(0, 0, 0))) }),
                            shape.rect({ at: .Const((20.0, 0.0)), size: .Const((40.0, 40.0)), fill: .Some(.Const(.Rgb(0, 0, 0))) }),
                        ],
                    }),
                ),
                named(
                    "icon",
                    ui.image({
                        source: .Const("<svg xmlns='http://www.w3.org/2000/svg' width='60' height='40' viewBox='0 0 60 40'><g opacity='0.5'><rect width='40' height='40'/><rect x='20' width='40' height='40'/></g></svg>"),
                        alt: .Decorative,
                        styles: [.Width(.Px(60)), .Height(.Px(40))],
                    }),
                ),
                named(
                    "stroke",
                    ui.image({
                        source: .Const("<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24'><rect x='4' y='4' width='16' height='16' fill='red' stroke='blue' stroke-width='4' opacity='0.5'/></svg>"),
                        alt: .Decorative,
                        styles: [.Width(.Px(60)), .Height(.Px(40))],
                    }),
                ),
                named(
                    "pinned",
                    half(.Column, [.Position(.Flow)], [square([]), square([.Pin(.Start, .Px(20)), .Pin(.Top, .Px(0))])]),
                ),
                named(
                    "fixed",
                    half(.Column, [], [square([]), square([.Position(.PinViewport), .Pin(.Start, .Px(30)), .Pin(.Top, .Px(10))])]),
                ),
            ],
        }),
        [],
    )
}
"#;

/// The page fades each group the way a snapshot does: the opacity sits on the
/// group's own element and on nothing inside it, so the browser composites the
/// group as one picture. A pinned or fixed child is inside the faded element,
/// so it is part of that picture too.
#[test]
fn a_faded_group_carries_its_opacity_on_its_own_element_and_nothing_inside_it() {
    let printed = drive(
        "faded-groups-page",
        FADED_GROUPS_PAGE,
        r#"
import { readFileSync } from "node:fs";
const sheet = document.createElement("style");
sheet.textContent = readFileSync(new URL("./.buri/out/web/cmd/page/main.css", import.meta.url), "utf8");
document.head.appendChild(sheet);
// An SVG element's opacity is its attribute; a box's is its computed style.
const fade = (el) => el.getAttribute("opacity") ?? (getComputedStyle(el).opacity || "1");
for (const group of document.querySelectorAll("[role=group]")) {
  const faded = [...group.querySelectorAll("*")].filter((el) => fade(el) !== "1");
  const read = faded.map((el) => `${el.tagName.toLowerCase()} ${fade(el)} holding ${el.querySelectorAll("*").length}`);
  console.log(`${group.getAttribute("aria-label")}: ${read.join(", ")}`);
}
"#,
    );
    assert_eq!(
        printed,
        "layers: div 0.5 holding 2\n\
         column: div 0.5 holding 2\n\
         row: div 0.5 holding 2\n\
         column reverse: div 0.5 holding 2\n\
         row reverse: div 0.5 holding 2\n\
         grid: div 0.5 holding 2\n\
         border: div 0.5 holding 0\n\
         drawing: svg 0.5 holding 2\n\
         icon: g 0.5 holding 2\n\
         stroke: rect 0.5 holding 0\n\
         pinned: div 0.5 holding 2\n\
         fixed: div 0.5 holding 2\n"
    );
}

/// A page that shows its path, with a button onward and a Back button
/// (buri-lang/buri#341).
const BACK_PAGE: &str = r#"from "platform/effect" import { Allocator, Location, Ui };
from "ui/node" import * as ui;
from "ui/web" import * as web;
from "web" import { WebHost };

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
                ui.text({ content: web.route(ctx) }),
                ui.button({
                    label: .Const("Next"),
                    styles: [],
                    onPress: .Some(fn(c) => web.navigate(c, "/next")),
                }),
                ui.button({ label: .Const("Back"), styles: [], onPress: .Some(fn(c) => web.back(c)) }),
            ],
        }),
        [],
    )
}
"#;

#[test]
fn a_back_button_returns_to_the_page_the_reader_came_from_without_pushing() {
    let printed = drive(
        "back-page",
        BACK_PAGE,
        r#"
const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};
const shown = () => `${location.pathname} ${showing().match(/>(\/[^<]*)</)[1]}`;
labelled("Next").click();
await settle();
console.log(shown());
labelled("Back").click();
await settle();
console.log(shown());
console.log(history.length);
console.log(moves.join("\n"));
"#,
    );
    assert_eq!(printed, "/next /next\n/ /\n2\npushed /next\n");
}

/// Each style value twice, each in a stack named for it: once written where
/// the node is given it, and once arriving through a parameter, a local, a
/// function's result or a conditional.
const ARRIVING_STYLES_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/style" import { Align, Layout, ListMarker };
from "web" import { WebHost };

fn named<C>(name: Str, subject: Node<C>): Node<C> {
    ui.stack({ styles: [], children: [subject], role: .Some(.Group), label: .Some(.Const(name)) })
}

fn box<C>(): Node<C> {
    ui.stack({ styles: [.Width(.Px(20)), .Height(.Px(20))], children: [] })
}

fn item<C>(words: Str): Node<C> {
    ui.stack({ styles: [], children: [ui.text({ content: .Const(words) })] })
}

fn laidOut<C>(layout: Layout): Node<C> {
    ui.stack({ styles: [.Layout(layout)], children: [box(), box()] })
}

fn heldInALocal<C>(layout: Layout): Node<C> {
    let chosen = layout;
    ui.stack({ styles: [.Layout(chosen)], children: [box(), box()] })
}

fn pick(stacked: Bool): Layout {
    if (stacked) { .Layers } else { .Column }
}

fn picked<C>(stacked: Bool): Node<C> {
    ui.stack({ styles: [.Layout(pick(stacked))], children: [box(), box()] })
}

fn chosenHere<C>(stacked: Bool): Node<C> {
    ui.stack({ styles: [.Layout(if (stacked) { .Layers } else { .Column })], children: [box(), box()] })
}

fn marked<C>(marker: ListMarker): Node<C> {
    ui.stack({ styles: [.ListMarker(marker)], children: [item("One"), item("Two")] })
}

fn aligned<C>(align: Align): Node<C> {
    ui.stack({ styles: [.Layout(.Row), .AlignMain(align), .At(.Medium, [.AlignMain(.End)])], children: [box()] })
}

fn crossed<C>(align: Align): Node<C> {
    ui.stack({ styles: [.Layout(.Row), .AlignCross(.Center), .AlignCross(align)], children: [box()] })
}

fn removable<C>(gone: Bool, layout: Layout): Node<C> {
    ui.stack({ styles: [.Removed(gone), .Layout(layout)], children: [box()] })
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
            children: [
                named("layers written", ui.stack({ styles: [.Layout(.Layers)], children: [box(), box()] })),
                named("layers by parameter", laidOut(.Layers)),
                named("layers by local", heldInALocal(.Layers)),
                named("layers by result", picked(true)),
                named("layers by conditional", chosenHere(true)),
                named("marker written", ui.stack({ styles: [.ListMarker(.Decimal)], children: [item("One"), item("Two")] })),
                named("marker by parameter", marked(.Decimal)),
                named("align written", ui.stack({ styles: [.Layout(.Row), .AlignMain(.Start), .At(.Medium, [.AlignMain(.End)])], children: [box()] })),
                named("align by parameter", aligned(.Start)),
                named("cross written", ui.stack({ styles: [.Layout(.Row), .AlignCross(.Center), .AlignCross(.SpaceBetween)], children: [box()] })),
                named("cross by parameter", crossed(.SpaceBetween)),
                named("removed written", ui.stack({ styles: [.Removed(true), .Layout(.Row)], children: [box()] })),
                named("removed by parameter", removable(true, .Row)),
            ],
        }),
        [],
    )
}
"#;

/// A style value does on the page what the same value written in place does,
/// however it reached its node: a layer stack puts its children in one space,
/// a list's numbers mark each item, a breakpoint overrides an alignment, a
/// distribution across the axis stretches, and a removal stays removed under
/// a layout written after it.
#[test]
fn a_style_value_does_the_same_however_it_reaches_its_node() {
    let printed = drive(
        "arriving-styles-page",
        ARRIVING_STYLES_PAGE,
        r#"
import { readFileSync } from "node:fs";
const css = readFileSync(new URL("./.buri/out/web/cmd/page/main.css", import.meta.url), "utf8");
const sheet = document.createElement("style");
sheet.textContent = css;
document.head.appendChild(sheet);
window.happyDOM.setViewport({ width: 800, height: 600 });
sheet.textContent += " ";
// happy-dom computes no `::after`, so a mark is found the way a browser finds
// it: a rule for the item's `::after` whose `content` draws something.
const marks = [];
for (const [, selector, body] of css.matchAll(/([^{}@]+)\{([^}]*)\}/g)) {
  const at = selector.indexOf("::after");
  if (at >= 0 && /content:(?!none)/.test(body)) marks.push(selector.slice(0, at).trim());
}
const marked = (el) => (marks.some((s) => el.matches(s)) ? "marked" : "unmarked");
for (const group of document.querySelectorAll("[role=group]")) {
  const name = group.getAttribute("aria-label");
  const el = group.firstElementChild;
  let read;
  if (name.startsWith("layers")) {
    read = [...el.children].map((c) => getComputedStyle(c).gridArea || "auto").join(" ");
  } else if (name.startsWith("marker")) {
    read = [...el.children].map(marked).join(" ");
  } else if (name.startsWith("align")) {
    read = getComputedStyle(el).justifyContent;
  } else if (name.startsWith("cross")) {
    read = getComputedStyle(el).alignItems;
  } else {
    read = getComputedStyle(el).display;
  }
  console.log(`${name}: ${read}`);
}
"#,
    );
    assert_eq!(
        printed,
        "layers written: 1/1 1/1\n\
         layers by parameter: 1/1 1/1\n\
         layers by local: 1/1 1/1\n\
         layers by result: 1/1 1/1\n\
         layers by conditional: 1/1 1/1\n\
         marker written: marked marked\n\
         marker by parameter: marked marked\n\
         align written: flex-end\n\
         align by parameter: flex-end\n\
         cross written: stretch\n\
         cross by parameter: stretch\n\
         removed written: none\n\
         removed by parameter: none\n"
    );
}

/// Each open style value twice, written in place and handed through a
/// parameter, under an `At(.Medium, ...)` rule for the same property. Each
/// group's label opens with the CSS property the driver reads.
const OPEN_STYLES_PAGE: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/style" import { Corner, Edge, FontFamily, Length, Style, Track };
from "web" import { WebHost };

fn named<C>(name: Str, subject: Node<C>): Node<C> {
    ui.stack({ styles: [], children: [subject], role: .Some(.Group), label: .Some(.Const(name)) })
}

fn pinned<C>(edge: Edge, length: Length): Node<C> {
    ui.stack({ styles: [.Pin(edge, length), .At(.Medium, [.Pin(.Top, .Px(40))])], children: [] })
}

fn padded<C>(edge: Edge, length: Length): Node<C> {
    ui.stack({ styles: [.PaddingEdge(edge, length), .At(.Medium, [.PaddingEdge(.Start, .Px(40))])], children: [] })
}

fn bordered<C>(edge: Edge, length: Length): Node<C> {
    ui.stack({ styles: [.BorderEdge(edge, length), .At(.Medium, [.BorderEdge(.Top, .Px(12))])], children: [] })
}

fn rounded<C>(corner: Corner, length: Length): Node<C> {
    ui.stack({ styles: [.RadiusCorner(corner, length), .At(.Medium, [.RadiusCorner(.TopStart, .Px(30))])], children: [] })
}

fn bled<C>(edge: Edge, length: Length): Node<C> {
    ui.stack({ styles: [.Bleed(edge, length), .At(.Medium, [.Bleed(.Start, .Px(30))])], children: [] })
}

fn moved<C>(x: Length, y: Length): Node<C> {
    ui.stack({ styles: [.Translate(x, y), .At(.Medium, [.Translate(.Px(40), .Px(30))])], children: [] })
}

fn framed<C>(width: Length): Node<C> {
    ui.stack({ styles: [.BorderWidth(width), .At(.Medium, [.BorderWidth(.Px(12))])], children: [] })
}

fn dashed<C>(width: Length): Node<C> {
    ui.stack({ styles: [.BorderWidth(width), .BorderStyle(.Dashed)], children: [] })
}

fn typeset<C>(family: FontFamily): Node<C> {
    ui.stack({ styles: [.FontFamily(family), .At(.Medium, [.FontFamily(.Mono)])], children: [] })
}

fn clamped<C>(lines: Int): Node<C> {
    ui.stack({ styles: [.Truncate(lines), .At(.Medium, [.Truncate(0)])], children: [] })
}

fn unclamped<C>(lines: Int): Node<C> {
    ui.stack({ styles: [.Truncate(lines), .At(.Medium, [.Truncate(1)])], children: [] })
}

fn gridded<C>(tracks: [Track]): Node<C> {
    ui.stack({ styles: [.Layout(.Grid(tracks)), .At(.Medium, [.Layout(.Grid([.Fixed(.Px(50)), .Fixed(.Px(20))]))])], children: [] })
}

fn styledBy<C>(style: Style): Node<C> {
    ui.stack({ styles: [style, .At(.Medium, [.PaddingEdge(.Start, .Px(40))])], children: [] })
}

fn paddedBy<C>(length: Length): Node<C> {
    styledBy(.PaddingEdge(.Start, length))
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
            children: [
                named("inset-block-start written", ui.stack({ styles: [.Pin(.Top, .Px(0)), .At(.Medium, [.Pin(.Top, .Px(40))])], children: [] })),
                named("inset-block-start by parameter", pinned(.Top, .Px(0))),
                named("padding-inline-start written", ui.stack({ styles: [.PaddingEdge(.Start, .Px(0)), .At(.Medium, [.PaddingEdge(.Start, .Px(40))])], children: [] })),
                named("padding-inline-start by parameter", padded(.Start, .Px(0))),
                named("padding-inline-start by style parameter", paddedBy(.Px(0))),
                named("border-block-start-width written", ui.stack({ styles: [.BorderEdge(.Top, .Px(2)), .At(.Medium, [.BorderEdge(.Top, .Px(12))])], children: [] })),
                named("border-block-start-width by parameter", bordered(.Top, .Px(2))),
                named("border-start-start-radius written", ui.stack({ styles: [.RadiusCorner(.TopStart, .Px(0)), .At(.Medium, [.RadiusCorner(.TopStart, .Px(30))])], children: [] })),
                named("border-start-start-radius by parameter", rounded(.TopStart, .Px(0))),
                named("margin-inline-start written", ui.stack({ styles: [.Bleed(.Start, .Px(0)), .At(.Medium, [.Bleed(.Start, .Px(30))])], children: [] })),
                named("margin-inline-start by parameter", bled(.Start, .Px(0))),
                named("transform written", ui.stack({ styles: [.Translate(.Px(0), .Px(0)), .At(.Medium, [.Translate(.Px(40), .Px(30))])], children: [] })),
                named("transform by parameter", moved(.Px(0), .Px(0))),
                named("border-width written", ui.stack({ styles: [.BorderWidth(.Px(2)), .At(.Medium, [.BorderWidth(.Px(12))])], children: [] })),
                named("border-width by parameter", framed(.Px(2))),
                named("border-top-style written", ui.stack({ styles: [.BorderWidth(.Px(2)), .BorderStyle(.Dashed)], children: [] })),
                named("border-top-style by parameter", dashed(.Px(2))),
                named("font-family written", ui.stack({ styles: [.FontFamily(.Sans), .At(.Medium, [.FontFamily(.Mono)])], children: [] })),
                named("font-family by parameter", typeset(.Sans)),
                named("overflow of lines written", ui.stack({ styles: [.Truncate(3), .At(.Medium, [.Truncate(0)])], children: [] })),
                named("overflow of lines by parameter", clamped(3)),
                named("overflow of no lines written", ui.stack({ styles: [.Truncate(0), .At(.Medium, [.Truncate(1)])], children: [] })),
                named("overflow of no lines by parameter", unclamped(0)),
                named("grid-template-columns written", ui.stack({ styles: [.Layout(.Grid([.Fixed(.Px(20)), .Fixed(.Px(20))])), .At(.Medium, [.Layout(.Grid([.Fixed(.Px(50)), .Fixed(.Px(20))]))])], children: [] })),
                named("grid-template-columns by parameter", gridded([.Fixed(.Px(20)), .Fixed(.Px(20))])),
            ],
        }),
        [],
    )
}
"#;

/// An `At` rule overrides an open style value on the page whether the value is
/// written in place or handed through a parameter: an edge's pin, padding,
/// border and bleed, a corner's rounding, a translation, a border width, a
/// font, a line count and a grid's tracks. A border style written after a
/// border width applies however the width arrives.
#[test]
fn a_breakpoint_overrides_an_open_style_value_however_it_arrives() {
    let printed = drive(
        "open-styles-page",
        OPEN_STYLES_PAGE,
        r#"
import { readFileSync } from "node:fs";
const css = readFileSync(new URL("./.buri/out/web/cmd/page/main.css", import.meta.url), "utf8");
const sheet = document.createElement("style");
sheet.textContent = css;
document.head.appendChild(sheet);
window.happyDOM.setViewport({ width: 800, height: 600 });
sheet.textContent += " ";
for (const group of document.querySelectorAll("[role=group]")) {
  const name = group.getAttribute("aria-label");
  const property = name.split(" ")[0];
  const value = getComputedStyle(group.firstElementChild).getPropertyValue(property);
  console.log(`${name}: ${value.replace(/, /g, ",")}`);
}
"#,
    );
    assert_eq!(
        printed,
        "inset-block-start written: 40px\n\
         inset-block-start by parameter: 40px\n\
         padding-inline-start written: 40px\n\
         padding-inline-start by parameter: 40px\n\
         padding-inline-start by style parameter: 40px\n\
         border-block-start-width written: 12px\n\
         border-block-start-width by parameter: 12px\n\
         border-start-start-radius written: 30px\n\
         border-start-start-radius by parameter: 30px\n\
         margin-inline-start written: -30px\n\
         margin-inline-start by parameter: -30px\n\
         transform written: translate(40px,30px)\n\
         transform by parameter: translate(40px,30px)\n\
         border-width written: 12px\n\
         border-width by parameter: 12px\n\
         border-top-style written: dashed\n\
         border-top-style by parameter: dashed\n\
         font-family written: ui-monospace,SFMono-Regular,monospace\n\
         font-family by parameter: ui-monospace,SFMono-Regular,monospace\n\
         overflow of lines written: visible\n\
         overflow of lines by parameter: visible\n\
         overflow of no lines written: hidden\n\
         overflow of no lines by parameter: hidden\n\
         grid-template-columns written: 50px 20px\n\
         grid-template-columns by parameter: 50px 20px\n"
    );
}

/// A page of boxes, each given one style value, written in place or, when
/// `late`, read back from a signal, which the compiler can't fold. The two
/// forms are separate pages, so neither names a class the other could reuse.
/// Each box is a `(label, style, value, expected)`: `style` holds `{}` where
/// the value goes, and the label opens with the CSS property the driver reads.
fn values_page(late: bool, boxes: &[(&str, &str, &str, &str)]) -> String {
    let children: String = boxes
        .iter()
        .map(|(label, style, value, _)| {
            let value = if late {
                format!("late(ctx, {value})")
            } else {
                (*value).to_string()
            };
            let style = style.replace("{}", &value);
            format!("                named(\"{label}\", boxed([{style}])),\n")
        })
        .collect();
    format!(
        r#"from "platform/effect" import {{ Allocator, Ui, Watch }};
from "ui/node" import * as ui;
from "ui/node" import {{ Node }};
from "ui/signal" import {{ signal }};
from "ui/style" import * as style;
from "ui/style" import {{ Shadow, Style }};
from "web" import {{ WebHost }};

fn named<C>(name: Str, subject: Node<C>): Node<C> {{
    ui.stack({{ styles: [], children: [subject], role: .Some(.Group), label: .Some(.Const(name)) }})
}}

fn late<T, C: Ui + Watch>(ctx: C, x: T): T {{
    signal(ctx, x).get(ctx)
}}

fn boxed<C>(styles: [Style]): Node<C> {{
    ui.stack({{ styles: styles, children: [] }})
}}

export fn main(host: WebHost): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
        Watch: host.watch,
    }};
    ui.mount(
        ctx,
        ui.stack({{
            styles: [],
            children: [
{children}            ],
        }}),
        [],
    )
}}
"#
    )
}

/// Each box's label and the value its property reads on `page`.
fn values_read(name: &str, page: &str) -> String {
    let driver = r#"
import { readFileSync } from "node:fs";
const css = readFileSync(new URL("./.buri/out/web/cmd/page/main.css", import.meta.url), "utf8");
const sheet = document.createElement("style");
sheet.textContent = css;
document.head.appendChild(sheet);
for (const group of document.querySelectorAll("[role=group]")) {
  const name = group.getAttribute("aria-label");
  const property = name.split(" ")[0];
  const value = getComputedStyle(group.firstElementChild).getPropertyValue(property);
  console.log(`${name}: ${value}`);
}
"#;
    drive(name, page, driver)
}

/// Each box's label and the value it expects.
fn values_expected(boxes: &[(&str, &str, &str, &str)]) -> String {
    boxes
        .iter()
        .map(|(label, _, _, value)| format!("{label}: {value}\n"))
        .collect()
}

/// Builds `boxes` written in place and read back from a signal, and asserts
/// that every box's property reads its expected value on both pages.
fn arrives_the_same(name: &str, boxes: &[(&str, &str, &str, &str)]) {
    let expected = values_expected(boxes);
    let written = values_read(&format!("{name}-written-page"), &values_page(false, boxes));
    let late = values_read(&format!("{name}-late-page"), &values_page(true, boxes));
    assert_eq!(written, expected, "written in place");
    assert_eq!(late, expected, "read back from a signal");
}

/// A `Full` radius rounds a box into a pill however it reaches its node, and a
/// border width that isn't a width (a percentage, `Auto`, `Full` or a negative
/// length) draws no border however it arrives.
#[test]
fn a_radius_and_a_border_width_do_the_same_however_they_arrive() {
    let b = ".Width(.Px(80)), .Height(.Px(40)), .BorderStyle(.Solid), ";
    let boxes = [
        (
            "border-top-left-radius",
            format!("{b}.Radius({{}})"),
            ".Full",
            "9999px",
        ),
        (
            "border-start-end-radius",
            format!("{b}.RadiusCorner(.TopEnd, {{}})"),
            ".Full",
            "9999px",
        ),
        (
            "border-top-width of a percentage",
            format!("{b}.BorderWidth({{}})"),
            ".Percent(50.0)",
            "0px",
        ),
        (
            "border-top-width of auto",
            format!("{b}.BorderWidth({{}})"),
            ".Auto",
            "0px",
        ),
        (
            "border-top-width of full",
            format!("{b}.BorderWidth({{}})"),
            ".Full",
            "0px",
        ),
        (
            "border-top-width of a negative",
            format!("{b}.BorderWidth({{}})"),
            ".Px(-2)",
            "0px",
        ),
        (
            "border-block-start-width of a percentage",
            format!("{b}.BorderEdge(.Top, {{}})"),
            ".Percent(50.0)",
            "0px",
        ),
        (
            "border-block-start-width of a negative",
            format!("{b}.BorderEdge(.Top, {{}})"),
            ".Rem(-0.5)",
            "0px",
        ),
    ];
    let boxes: Vec<(&str, &str, &str, &str)> = boxes
        .iter()
        .map(|(label, style, value, css)| (*label, style.as_str(), *value, *css))
        .collect();
    arrives_the_same("edge-values", &boxes);
}

/// A negative or `Auto` padding is no padding, however it arrives.
#[test]
fn a_padding_that_isnt_a_distance_is_none_however_it_arrives() {
    arrives_the_same(
        "padding-values",
        &[
            (
                "padding-top of a negative",
                ".Padding({})",
                ".Px(-4)",
                "0px",
            ),
            ("padding-top of auto", ".Padding({})", ".Auto", "0px"),
            (
                "padding-inline of a negative",
                ".PaddingX({})",
                ".Rem(-1.0)",
                "0px",
            ),
            ("padding-block of auto", ".PaddingY({})", ".Auto", "0px"),
            (
                "padding-block-start of a negative",
                ".PaddingEdge(.Top, {})",
                ".Em(-0.5)",
                "0px",
            ),
            (
                "padding-inline-end of auto",
                ".PaddingEdge(.End, {})",
                ".Auto",
                "0px",
            ),
            (
                "padding-top of a percentage",
                ".Padding({})",
                ".Percent(5.0)",
                "5%",
            ),
        ],
    );
}

/// A negative or `Auto` gap is no gap, however it arrives.
#[test]
fn a_gap_that_isnt_a_distance_is_none_however_it_arrives() {
    arrives_the_same(
        "gap-values",
        &[
            ("gap of a negative", ".Gap({})", ".Px(-4)", "0px"),
            ("gap of auto", ".Gap({})", ".Auto", "0px"),
            ("column-gap of a negative", ".GapX({})", ".Rem(-1.0)", "0px"),
            ("row-gap of auto", ".GapY({})", ".Auto", "0px"),
        ],
    );
}

/// A negative size is zero, however it arrives.
#[test]
fn a_negative_size_is_zero_however_it_arrives() {
    arrives_the_same(
        "size-values",
        &[
            ("width of a negative", ".Width({})", ".Px(-10)", "0px"),
            ("height of a negative", ".Height({})", ".Rem(-1.0)", "0px"),
            (
                "min-width of a negative",
                ".MinWidth({})",
                ".Em(-2.0)",
                "0px",
            ),
            (
                "min-height of a negative",
                ".MinHeight({})",
                ".Percent(-5.0)",
                "0px",
            ),
            ("width of auto", ".Width({})", ".Auto", "auto"),
        ],
    );
}

/// An `Auto` maximum is no limit, and a negative one is zero, however it
/// arrives.
#[test]
fn a_maximum_of_auto_is_no_limit_however_it_arrives() {
    arrives_the_same(
        "maximum-values",
        &[
            ("max-width of auto", ".MaxWidth({})", ".Auto", "none"),
            ("max-height of auto", ".MaxHeight({})", ".Auto", "none"),
            ("max-width of a negative", ".MaxWidth({})", ".Px(-3)", "0px"),
            (
                "max-height of a negative",
                ".MaxHeight({})",
                ".Percent(-1.0)",
                "0px",
            ),
        ],
    );
}

/// A negative or `Auto` radius squares the corners, however it arrives.
#[test]
fn a_radius_that_isnt_a_distance_squares_the_corners_however_it_arrives() {
    arrives_the_same(
        "radius-values",
        &[
            (
                "border-top-left-radius of a negative",
                ".Radius({})",
                ".Px(-4)",
                "0px",
            ),
            (
                "border-top-left-radius of auto",
                ".Radius({})",
                ".Auto",
                "0px",
            ),
            (
                "border-start-end-radius of a negative",
                ".RadiusCorner(.TopEnd, {})",
                ".Em(-1.0)",
                "0px",
            ),
            (
                "border-end-start-radius of auto",
                ".RadiusCorner(.BottomStart, {})",
                ".Auto",
                "0px",
            ),
        ],
    );
}

/// A shadow length that isn't a distance (a percentage, `Auto` or `Full`, or a
/// negative blur) is zero, however it arrives.
#[test]
fn a_shadow_length_that_isnt_a_distance_is_zero_however_it_arrives() {
    arrives_the_same(
        "shadow-values",
        &[
            (
                "box-shadow of auto and a percentage",
                ".Shadow({})",
                "Shadow { x: .Auto, y: .Percent(50.0), blur: .Px(4), spread: .Full, \
                 color: .Rgb(0, 0, 0) }",
                "0px 0px 4px 0px rgb(0,0,0)",
            ),
            (
                "box-shadow of a negative blur",
                ".Shadow({})",
                "Shadow { x: .Px(-2), y: .Px(2), blur: .Px(-4), spread: .Px(-1), \
                 color: .Rgb(0, 0, 0) }",
                "-2px 2px 0px -1px rgb(0,0,0)",
            ),
            (
                "box-shadow of layers",
                ".Shadows({})",
                "[Shadow { x: .Px(1), y: .Auto, blur: .Rem(-1.0), spread: .Px(0), \
                 color: .Rgb(0, 0, 0) }, Shadow { x: .Full, y: .Px(1), blur: .Px(2), \
                 spread: .Em(1.0), color: .Rgb(0, 0, 0) }]",
                "1px 0px 0px 0px rgb(0,0,0),0px 1px 2px 1em rgb(0,0,0)",
            ),
        ],
    );
}

/// An `Auto` translate doesn't move, however it arrives.
#[test]
fn an_auto_translate_doesnt_move_however_it_arrives() {
    arrives_the_same(
        "translate-values",
        &[
            (
                "transform of auto across",
                ".Translate({}, .Px(4))",
                ".Auto",
                "translate(0px,4px)",
            ),
            (
                "transform of auto down",
                ".Translate(.Px(4), {})",
                ".Auto",
                "translate(4px,0px)",
            ),
        ],
    );
}

/// A span of zero or less covers one track, however it arrives.
#[test]
fn a_span_of_no_tracks_covers_one_however_it_arrives() {
    arrives_the_same(
        "span-values",
        &[
            ("grid-column of zero", ".Span({})", "0", "span 1"),
            ("grid-column of a negative", ".Span({})", "-2", "span 1"),
        ],
    );
}

/// A negative grow or shrink is zero, however it arrives.
#[test]
fn a_negative_grow_or_shrink_is_zero_however_it_arrives() {
    arrives_the_same(
        "flex-values",
        &[
            ("flex-grow of a negative", ".Grow({})", "-1", "0"),
            ("flex-shrink of a negative", ".Shrink({})", "-2", "0"),
        ],
    );
}

/// A negative text size is zero, and an `Auto` one is the size the text
/// inherits, however it arrives.
#[test]
fn a_text_size_that_isnt_a_distance_is_fitted_however_it_arrives() {
    arrives_the_same(
        "font-size-values",
        &[
            ("font-size of a negative", ".FontSize({})", ".Px(-4)", "0px"),
            ("font-size of auto", ".FontSize({})", ".Auto", "16px"),
        ],
    );
}

/// An `Auto` letter spacing adds no space, however it arrives.
#[test]
fn an_auto_letter_spacing_adds_nothing_however_it_arrives() {
    arrives_the_same(
        "letter-spacing-values",
        &[(
            "letter-spacing of auto",
            ".LetterSpacing({})",
            ".Auto",
            "0px",
        )],
    );
}

/// A negative line height is zero, however it arrives.
#[test]
fn a_negative_line_height_is_zero_however_it_arrives() {
    arrives_the_same(
        "line-height-values",
        &[("line-height of a negative", ".LineHeight({})", "-1.5", "0")],
    );
}

/// An aspect ratio of zero or less keeps no proportion, however it arrives.
#[test]
fn an_aspect_ratio_of_no_proportion_is_auto_however_it_arrives() {
    arrives_the_same(
        "aspect-ratio-values",
        &[
            ("aspect-ratio of zero", ".AspectRatio({})", "0.0", "auto"),
            (
                "aspect-ratio of a negative",
                ".AspectRatio({})",
                "-1.5",
                "auto",
            ),
        ],
    );
}

/// A backdrop blur whose radius isn't a distance blurs nothing, however it
/// arrives.
#[test]
fn a_backdrop_blur_that_isnt_a_distance_blurs_nothing_however_it_arrives() {
    arrives_the_same(
        "backdrop-values",
        &[
            (
                "backdrop-filter of a negative",
                ".BackdropBlur({})",
                ".Px(-4)",
                "blur(0px)",
            ),
            (
                "backdrop-filter of a percentage",
                ".BackdropBlur({})",
                ".Percent(10.0)",
                "blur(0px)",
            ),
            (
                "backdrop-filter of auto",
                ".BackdropBlur({})",
                ".Auto",
                "blur(0px)",
            ),
            (
                "backdrop-filter of full",
                ".BackdropBlur({})",
                ".Full",
                "blur(0px)",
            ),
        ],
    );
}

/// A negative grid track is zero wide, however it arrives.
#[test]
fn a_negative_grid_track_is_zero_however_it_arrives() {
    arrives_the_same(
        "track-values",
        &[(
            "grid-template-columns of negatives",
            ".Layout({})",
            ".Grid([.Fraction(-1), .Fixed(.Px(-20)), .Fixed(.Rem(1.0))])",
            "0fr 0px 1rem",
        )],
    );
}

/// A token faded past whole, by an alpha only known when the page runs, is the
/// whole token, and one faded below nothing is none of it. An alpha known when
/// the program is built is refused instead, so there is no page that writes one.
/// happy-dom reads no `color-mix`, so this reads the value the page writes.
#[test]
fn a_fade_out_of_range_known_only_at_run_time_is_clamped() {
    let boxes = [("more than whole", "1.5"), ("less than none", "-0.5")];
    let boxes: Vec<(&str, &str, &str, &str)> = boxes
        .iter()
        .map(|(label, alpha)| {
            (
                *label,
                ".Background(style.token(\"kit\", \"ink\").alpha({}))",
                *alpha,
                "",
            )
        })
        .collect();
    let driver = r#"
for (const group of document.querySelectorAll("[role=group]")) {
  const written = group.firstElementChild.getAttribute("style");
  console.log(`${group.getAttribute("aria-label")}: ${written.slice(written.indexOf(":") + 1).trim().replace(/;$/, "")}`);
}
"#;
    let late = drive("fade-values-late-page", &values_page(true, &boxes), driver);
    assert_eq!(
        late,
        "more than whole: color-mix(in srgb,var(--kit-ink) 100%,transparent)\n\
         less than none: color-mix(in srgb,var(--kit-ink) 0%,transparent)\n"
    );
}
