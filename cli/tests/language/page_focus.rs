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

/// A button that opens a panel, writing first the panel's `hasFocus` and then
/// its row's, as a menu that takes the focus as it opens and hands it to its
/// ticked row does (#370). `position` is the panel's, and `named` makes it a
/// group named "Panel" or leaves it a plain stack. Answers what has the focus
/// once it opens, and what the two signals hold.
fn menu(position: &str, named: bool) -> String {
    let group = if named {
        r#"role: .Some(.Group), label: .Some(.Const("Panel")),"#
    } else {
        ""
    };
    let helpers = format!(
        r#"fn menu<C: Ui>(ctx: C): Node<C> {{
    let open = signal(ctx, false);
    let outer = signal(ctx, false);
    let inner = signal(ctx, false);
    ui.stack({{
        styles: [.Layout(.Column), .Position(.Flow)],
        children: [
            ui.button({{
                label: .Const("Open"),
                styles: [],
                onPress: .Some(fn(c) => {{
                    let _ = outer.set(c, true);
                    let _ = inner.set(c, true);
                    open.set(c, true)
                }}),
            }}),
            ui.choose(
                .Cell(open),
                ui.stack({{
                    styles: [.Layout(.Column), .Position({position})],
                    {group}
                    hasFocus: .Some(outer),
                    isInFocusOrder: .Some(.Const(false)),
                    children: [
                        ui.button({{
                            label: .Const("Row"),
                            styles: [],
                            onPress: .None,
                            hasFocus: .Some(inner),
                        }}),
                    ],
                }}),
                ui.empty(),
            ),
            ui.text({{
                content: .Computed(fn(s) => str.format(s, "outer:${{outer.get(s)}} inner:${{inner.get(s)}}")),
            }}),
        ],
    }})
}}"#
    );
    drive(
        "focus-menu",
        &page(&helpers, "menu(ctx)"),
        &format!(
            r#"{FOCUSED}
const said = () => body.firstChild.lastChild.textContent;
const open = labelled("Open");
open.focus();
open.click();
console.log(`${{focused()}} ${{said()}}`);
"#
        ),
    )
}

#[test]
fn the_row_written_last_takes_the_focus_from_a_group_in_the_flow() {
    assert_eq!(menu(".Flow", true), "Row outer:false inner:true\n");
}

#[test]
fn the_row_written_last_takes_the_focus_from_a_pinned_group() {
    assert_eq!(menu(".PinAnchor", true), "Row outer:false inner:true\n");
}

#[test]
fn the_row_written_last_takes_the_focus_from_a_stack_in_the_flow() {
    assert_eq!(menu(".Flow", false), "Row outer:false inner:true\n");
}

#[test]
fn the_row_written_last_takes_the_focus_from_a_pinned_stack() {
    assert_eq!(menu(".PinAnchor", false), "Row outer:false inner:true\n");
}

/// A button bound to `hasFocus`, a picker bound to one too, a plain button,
/// and a line saying what the two signals hold (#371).
const BOUND: &str = r#"fn bound<C: Ui>(ctx: C): Node<C> {
    let focused = signal(ctx, false);
    let picking = signal(ctx, false);
    ui.stack({
        styles: [.Layout(.Column)],
        children: [
            ui.button({
                label: .Const("Bound"),
                styles: [],
                onPress: .None,
                hasFocus: .Some(focused),
            }),
            ui.picker({
                label: .Const("Size"),
                options: [("s", ui.text({ content: .Const("Small") })), ("l", ui.text({ content: .Const("Large") }))],
                value: signal(ctx, "s"),
                styles: [],
                hasFocus: .Some(picking),
            }),
            ui.button({ label: .Const("Plain"), styles: [], onPress: .None }),
            ui.text({
                content: .Computed(fn(s) => str.format(s, "bound:${focused.get(s)} picker:${picking.get(s)}")),
            }),
        ],
    })
}"#;

/// What a browser does as the window loses the focus and gets it back while
/// `element` has it: `blur` and `focusout` on the element, which stays the
/// document's active element, then `focus` and `focusin` on it again. Prints
/// what has the focus before, while the window is away, and once it is back.
const AWAY_AND_BACK: &str = r#"
const said = () => body.firstChild.lastChild.textContent;
const awayAndBack = (element) => {
  element.focus();
  console.log(`${focused()} ${said()}`);
  element.dispatchEvent(new FocusEvent("blur"));
  element.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
  console.log(`away ${focused()} ${said()}`);
  element.dispatchEvent(new FocusEvent("focus"));
  element.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
  console.log(`back ${focused()} ${said()}`);
};
"#;

#[test]
fn a_button_bound_to_has_focus_keeps_the_focus_while_the_window_is_away() {
    let printed = drive(
        "focus-window-button",
        &page(BOUND, "bound(ctx)"),
        &format!("{FOCUSED}{AWAY_AND_BACK}awayAndBack(labelled(\"Bound\"));\n"),
    );
    assert_eq!(
        printed,
        "Bound bound:true picker:false\n\
         away Bound bound:false picker:false\n\
         back Bound bound:true picker:false\n"
    );
}

#[test]
fn a_picker_bound_to_has_focus_keeps_the_focus_while_the_window_is_away() {
    let printed = drive(
        "focus-window-picker",
        &page(BOUND, "bound(ctx)"),
        &format!(
            "{FOCUSED}{AWAY_AND_BACK}awayAndBack(document.querySelector(\"input[value=s]\"));\n"
        ),
    );
    assert_eq!(
        printed,
        "Small bound:false picker:true\n\
         away Small bound:false picker:false\n\
         back Small bound:false picker:true\n"
    );
}

#[test]
fn a_plain_button_keeps_the_focus_while_the_window_is_away() {
    let printed = drive(
        "focus-window-plain",
        &page(BOUND, "bound(ctx)"),
        &format!("{FOCUSED}{AWAY_AND_BACK}awayAndBack(labelled(\"Plain\"));\n"),
    );
    assert_eq!(
        printed,
        "Plain bound:false picker:false\n\
         away Plain bound:false picker:false\n\
         back Plain bound:false picker:false\n"
    );
}

/// Buttons A and B, bound to `a` and `b`, and a button "Open", bound to
/// `trigger`, whose press runs `writes` and then opens a group named "Panel",
/// bound to `panel`, holding a button "Row", bound to `row`. "Arm" writes `row`
/// alone. Runs `script`, then answers what has the focus and what the signals
/// hold.
fn opening(writes: &str, script: &str) -> String {
    let helpers = format!(
        r#"fn bound<C: Ui>(label: Str, focus: Signal<Bool>): Node<C> {{
    ui.button({{ label: .Const(label), styles: [], onPress: .None, hasFocus: .Some(focus) }})
}}

fn opening<C: Ui>(ctx: C): Node<C> {{
    let open = signal(ctx, false);
    let a = signal(ctx, false);
    let b = signal(ctx, false);
    let trigger = signal(ctx, false);
    let panel = signal(ctx, false);
    let row = signal(ctx, false);
    ui.stack({{
        styles: [],
        children: [
            bound("A", a),
            bound("B", b),
            ui.button({{
                label: .Const("Arm"),
                styles: [],
                onPress: .Some(fn(c) => row.set(c, true)),
            }}),
            ui.button({{
                label: .Const("Open"),
                styles: [],
                onPress: .Some(fn(c) => {{
                    {writes}
                    open.set(c, true)
                }}),
                hasFocus: .Some(trigger),
            }}),
            ui.choose(
                .Cell(open),
                ui.stack({{
                    styles: [],
                    role: .Some(.Group),
                    label: .Some(.Const("Panel")),
                    hasFocus: .Some(panel),
                    isInFocusOrder: .Some(.Const(false)),
                    children: [bound("Row", row)],
                }}),
                ui.empty(),
            ),
            ui.text({{
                content: .Computed(fn(s) => str.format(
                    s,
                    "a:${{a.get(s)}} b:${{b.get(s)}} trigger:${{trigger.get(s)}} panel:${{panel.get(s)}} row:${{row.get(s)}}",
                )),
            }}),
        ],
    }})
}}"#
    );
    drive(
        "focus-opening",
        &page(&helpers, "opening(ctx)"),
        &format!(
            r#"{FOCUSED}
const said = () => body.firstChild.lastChild.textContent;
{script}
console.log(`${{focused()}} ${{said()}}`);
"#
        ),
    )
}

/// Presses "Open", whose handler runs `writes` before it opens the panel.
fn press_open(writes: &str) -> String {
    opening(
        writes,
        r#"const open = labelled("Open");
open.focus();
open.click();"#,
    )
}

#[test]
fn a_signal_written_false_and_then_true_again_is_the_last_one_written() {
    assert_eq!(
        press_open(
            "let _ = a.set(c, true); let _ = b.set(c, true); let _ = a.set(c, false); let _ = a.set(c, true);"
        ),
        "A a:true b:false trigger:false panel:false row:false\n"
    );
}

#[test]
fn in_a_panel_that_mounts_the_signal_written_last_gets_the_focus() {
    assert_eq!(
        press_open("let _ = row.set(c, true); let _ = panel.set(c, true);"),
        "Panel a:false b:false trigger:false panel:true row:false\n"
    );
}

#[test]
fn an_element_shown_written_after_one_in_a_panel_that_mounts_gets_the_focus() {
    assert_eq!(
        press_open("let _ = row.set(c, true); let _ = trigger.set(c, true);"),
        "Open a:false b:false trigger:true panel:false row:false\n"
    );
}

#[test]
fn a_row_written_before_its_panel_mounts_gets_the_focus_though_the_focus_moved() {
    assert_eq!(
        opening(
            "",
            r#"labelled("Arm").click();
const open = labelled("Open");
open.focus();
open.click();"#,
        ),
        "Row a:false b:false trigger:false panel:false row:true\n"
    );
}
