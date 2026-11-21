//! **Where a browser puts the focus a `hasFocus` signal hands out.**
//!
//! `conformance/lib/ui` asserts the focus through the headless double. Each
//! test here builds a page for `web`, drives it in happy-dom through
//! `harness::browser`, and prints the element `document.activeElement` names.

use super::pages::drive;

/// The page `body` is the tree of, mounted with `ctx` holding `host`'s
/// allocator and `Ui`, and with `helpers` before it.
fn page(helpers: &str, body: &str) -> String {
    format!(
        r#"from "core/str" import * as str;
from "platform/effect" import {{ Allocator, Ui }};
from "ui/node" import * as ui;
from "ui/node" import {{ Node }};
from "ui/signal" import {{ Signal, signal }};
from "web" import {{ WebHost }};

{helpers}

export fn main(host: WebHost): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
    }};
    ui.mount(ctx, {body}, [])
}}
"#
    )
}

/// Defines `focused()`, what has the focus: its accessible name, a summary's
/// text, a radio's label, or else its element.
const FOCUSED: &str = r#"
const focused = () => {
  const at = document.activeElement;
  if (at === null || at === body) return "body";
  if (at.tagName === "SUMMARY") return at.textContent;
  return at.getAttribute("aria-label") ?? at.closest("label")?.textContent ?? at.tagName.toLowerCase();
};
"#;

/// A button that swaps itself for a file picker and hands the picker the focus
/// as it goes, the way the issue (#369) builds it.
const CHOOSER: &str = r#"fn chooser<C: Ui>(ctx: C): Node<C> {
    let picking = signal(ctx, false);
    let focus = signal(ctx, false);
    ui.choose(
        .Cell(picking),
        ui.filePicker({
            label: .Const("Choose a file"),
            maxBytes: 1000,
            onPick: fn(_c, _picked) => (),
            styles: [],
            hasFocus: .Some(focus),
        }),
        ui.button({
            label: .Const("Choose a file instead"),
            styles: [],
            onPress: .Some(fn(c) => {
                let _ = focus.set(c, true);
                picking.set(c, true)
            }),
        }),
    )
}"#;

#[test]
fn a_file_picker_a_press_mounts_takes_the_focus_its_signal_hands_it() {
    let printed = drive(
        "focus-file-picker",
        &page(CHOOSER, "chooser(ctx)"),
        &format!(
            r#"{FOCUSED}
const swap = labelled("Choose a file instead");
swap.focus();
swap.click();
console.log(focused());
"#
        ),
    );
    assert_eq!(printed, "Choose a file\n");
}

/// A disclosure whose summary is bound to a signal, and a button that writes
/// it.
const SECTION: &str = r#"fn section<C: Ui>(ctx: C): Node<C> {
    let focus = signal(ctx, false);
    ui.stack({
        styles: [],
        children: [
            ui.button({
                label: .Const("Go to details"),
                styles: [],
                onPress: .Some(fn(c) => focus.set(c, true)),
            }),
            ui.disclosure({
                summary: .Const("Details"),
                isOpen: signal(ctx, false),
                children: [ui.text({ content: .Const("Body") })],
                styles: [],
                hasFocus: .Some(focus),
                isInFocusOrder: .Some(.Const(false)),
            }),
            ui.text({ content: .Computed(fn(s) => str.format(s, "focus:${focus.get(s)}")) }),
        ],
    })
}"#;

#[test]
fn a_disclosure_s_summary_takes_the_focus_its_signal_hands_it() {
    let printed = drive(
        "focus-disclosure",
        &page(SECTION, "section(ctx)"),
        &format!(
            r#"{FOCUSED}
const said = () => body.firstChild.lastChild.textContent;
const go = labelled("Go to details");
go.focus();
go.click();
console.log(`${{focused()}} ${{said()}}`);
console.log(`order ${{document.querySelector("summary").getAttribute("tabindex")}}`);
go.focus();
console.log(`${{focused()}} ${{said()}}`);
"#
        ),
    );
    assert_eq!(
        printed,
        "Details focus:true\norder -1\nGo to details focus:false\n"
    );
}
