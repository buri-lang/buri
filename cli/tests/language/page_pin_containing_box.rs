//! **A pinned box is measured from the nearest ancestor with a `Position`, in
//! a browser as the docs say.**
//!
//! `Pin` writes `position: absolute`, and a browser measures such a box from
//! its containing block: the nearest positioned ancestor, else the viewport.
//! Each box carries the role `Group` only so it can carry a label.
//! Each test builds a page that holds one red box labelled "Pinned" inside one
//! ancestor setup, pinned one way, and prints the box `placedAt` measures it
//! from, by its label, and where it lands there.
//!
//! The pin kinds are an edge in each axis, both edges in each axis, and sizes
//! that are percentages of what the box is measured from. The setups put the
//! nearest `Position` on the parent, on the grandparent, or nowhere, and put
//! the pinned box in a table entry, a table row, a table and a dialog.

use super::pages::drive;

/// Prints what "Pinned" is measured from and where it lands.
const SHOW: &str = r#"
const pinned = labelled("Pinned");
const against = containingBlock(pinned);
const name = against === null ? "the viewport" : (against.getAttribute("aria-label") ?? `<${against.localName}>`);
const { x, y, width, height } = placedAt(pinned);
console.log(`against ${name} at ${x},${y} ${width}x${height}`);
"#;

/// Pinned by an edge in each axis.
const EDGES: &str = ".Pin(.Top, .Px(10)), .Pin(.Start, .Px(20)), .Width(.Px(30)), .Height(.Px(40))";
/// Pinned by both edges in each axis.
const BOTH: &str =
    ".Pin(.Top, .Px(10)), .Pin(.Bottom, .Px(20)), .Pin(.Start, .Px(30)), .Pin(.End, .Px(40))";
/// Pinned at a corner, sized by percentages.
const SIZES: &str =
    ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(0)), .Width(.Percent(50.0)), .Height(.Percent(25.0))";

// Each setup is the tree around "Pinned", where `PINNED` stands for the box.

/// A positioned parent.
const PARENT: &str = r#"box("Grand", [], [box("Parent", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [PINNED])])"#;
/// A positioned grandparent past a parent with no position.
const GRANDPARENT: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100)), .Padding(.Px(8))], [
                box("Parent", [.Width(.Px(100)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// No positioned ancestor.
const NONE: &str = r#"box("Grand", [.Width(.Px(200)), .Height(.Px(100))], [box("Parent", [.Padding(.Px(8))], [PINNED])])"#;
/// A table entry, in a positioned box.
const ENTRY: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                table([row([], [entry([PINNED])])]),
            ])"#;
/// A table row with no position, in a positioned box.
const ROW: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                table([row([], [entry([]), PINNED])]),
            ])"#;
/// A table row with a position.
const FLOW_ROW: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                table([row([.Position(.Flow), .Height(.Px(40))], [entry([]), PINNED])]),
            ])"#;
/// Straight in a table, in a positioned box.
const TABLE: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                table([row([], [entry([])]), PINNED]),
            ])"#;
/// A dialog's panel.
const DIALOG: &str = r#"ui.dialog({
                isOpen: signal(ctx, true),
                label: .Const("Dialog"),
                styles: [.Width(.Px(200)), .Height(.Px(100))],
                children: [box("Panel", [.Padding(.Px(8))], [PINNED])],
            })"#;

/// A page holding the `setup` tree with "Pinned" pinned by `pins`.
fn page(setup: &str, pins: &str) -> String {
    let pinned = format!(
        r#"ui.stack({{
                role: .Some(.Group),
                label: .Some(.Const("Pinned")),
                styles: [{}, .Background(.Rgb(255, 0, 0))],
                children: [],
            }})"#,
        pins
    );
    format!(
        r#"from "platform/effect" import {{ Allocator, Ui }};
from "ui/node" import * as ui;
from "ui/node" import {{ Node }};
from "ui/signal" import {{ signal }};
from "ui/style" import {{ Style }};
from "web" import {{ WebHost }};

fn box<C>(name: Str, styles: [Style], children: [Node<C>]): Node<C> {{
    ui.stack({{ role: .Some(.Group), label: .Some(.Const(name)), styles: styles, children: children }})
}}

fn table<C>(children: [Node<C>]): Node<C> {{
    ui.stack({{
        role: .Some(.Table),
        label: .Some(.Const("Table")),
        styles: [.Width(.Px(200))],
        children: children,
    }})
}}

fn row<C>(styles: [Style], children: [Node<C>]): Node<C> {{
    ui.stack({{ role: .Some(.TableRow), label: .Some(.Const("Row")), styles: styles, children: children }})
}}

fn entry<C>(children: [Node<C>]): Node<C> {{
    ui.stack({{
        role: .Some(.Cell),
        label: .Some(.Const("Entry")),
        styles: [.Width(.Px(60)), .Height(.Px(20))],
        children: children,
    }})
}}

export fn main(host: WebHost): Result<(), Str> {{
    let ctx = context {{
        Allocator: host.alloc,
        Ui: host.ui,
    }};
    ui.mount(ctx, {}, [])
}}
"#,
        setup.replace("PINNED", &pinned)
    )
}

/// What a browser measures "Pinned" from, and where it lands there.
fn placed(name: &str, setup: &str, pins: &str) -> String {
    drive(name, &page(setup, pins), SHOW)
}

/// One test per setup and pin kind, each expecting `against <box> at <x>,<y>
/// <width>x<height>`.
macro_rules! placed {
    ($($test:ident: $setup:ident, $pins:ident => $expected:literal;)*) => {
        $(
            #[test]
            fn $test() {
                let placed = placed(stringify!($test), $setup, $pins);
                assert_eq!(placed, concat!("against ", $expected, "\n"));
            }
        )*
    };
}

placed! {
    edges_measure_from_a_positioned_parent: PARENT, EDGES => "Parent at 20,10 30x40";
    both_edges_measure_from_a_positioned_parent: PARENT, BOTH => "Parent at 30,10 130x70";
    sizes_measure_from_a_positioned_parent: PARENT, SIZES => "Parent at 0,0 100x25";

    edges_measure_from_a_positioned_grandparent: GRANDPARENT, EDGES => "Grand at 20,10 30x40";
    both_edges_measure_from_a_positioned_grandparent: GRANDPARENT, BOTH => "Grand at 30,10 130x70";
    sizes_measure_from_a_positioned_grandparent: GRANDPARENT, SIZES => "Grand at 0,0 100x25";

    edges_with_nothing_positioned_measure_from_the_viewport: NONE, EDGES => "the viewport at 20,10 30x40";
    both_edges_with_nothing_positioned_measure_from_the_viewport: NONE, BOTH => "the viewport at 30,10 954x738";
    sizes_with_nothing_positioned_measure_from_the_viewport: NONE, SIZES => "the viewport at 0,0 512x192";

    edges_in_a_table_entry_measure_past_the_table: ENTRY, EDGES => "Grand at 20,10 30x40";
    both_edges_in_a_table_entry_measure_past_the_table: ENTRY, BOTH => "Grand at 30,10 130x70";
    sizes_in_a_table_entry_measure_past_the_table: ENTRY, SIZES => "Grand at 0,0 100x25";

    edges_in_a_table_row_measure_past_the_table: ROW, EDGES => "Grand at 20,10 30x40";
    both_edges_in_a_table_row_measure_past_the_table: ROW, BOTH => "Grand at 30,10 130x70";
    sizes_in_a_table_row_measure_past_the_table: ROW, SIZES => "Grand at 0,0 100x25";

    edges_in_a_flow_table_row_measure_from_the_row: FLOW_ROW, EDGES => "Row at 20,10 30x40";
    both_edges_in_a_flow_table_row_measure_from_the_row: FLOW_ROW, BOTH => "Row at 30,10 130x10";
    sizes_in_a_flow_table_row_measure_from_the_row: FLOW_ROW, SIZES => "Row at 0,0 100x10";

    edges_in_a_table_measure_past_it: TABLE, EDGES => "Grand at 20,10 30x40";
    both_edges_in_a_table_measure_past_it: TABLE, BOTH => "Grand at 30,10 130x70";
    sizes_in_a_table_measure_past_it: TABLE, SIZES => "Grand at 0,0 100x25";

    edges_in_a_dialog_measure_from_the_dialog: DIALOG, EDGES => "Dialog at 20,10 30x40";
    both_edges_in_a_dialog_measure_from_the_dialog: DIALOG, BOTH => "Dialog at 30,10 954x738";
    sizes_in_a_dialog_measure_from_the_dialog: DIALOG, SIZES => "Dialog at 0,0 512x192";
}
