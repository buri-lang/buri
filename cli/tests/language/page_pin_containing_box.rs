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
//! the pinned box in a table entry, a table row, a table and a dialog. More
//! setups give the box measured from a border, padding, or a size taken from
//! its parent's content.
//!
//! Further setups hold "Pinned" to the viewport inside a translated or
//! backdrop-blurred box, which a browser measures it from, and place it
//! against an anchor, the nearest `Flow` box or else the viewport. The last
//! ones give "Pinned" no width and three boxes in a wrapping row, so a browser
//! shrinks it to fit the room beside its inset.
//!
//! More of those give a child of "Pinned" a percentage width, minimum or
//! maximum, which a browser counts as none while it fits "Pinned" to its
//! content and only then resolves against the width it fitted.
//!
//! The clip tests print the boxes whose overflow cuts "Pinned" instead: only
//! those on its chain of containing blocks do.
//!
//! The move tests print how far a `Translate` moves "Pinned" from where it is
//! placed, too. A box in the top layer, a `PinAnchor` box or a dialog, is
//! moved by none of its ancestors' translates, and it is placed against its
//! anchor where the anchor is laid out, before the anchor's own translate.

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
/// A positioned parent with padding and a border, which a pin measures from
/// inside the border.
const BORDERED: &str = r#"box("Grand", [], [box("Parent", [
                .Position(.Flow),
                .Width(.Px(200)),
                .Height(.Px(100)),
                .Padding(.Px(8)),
                .BorderWidth(.Px(5)),
            ], [PINNED])])"#;
/// A positioned parent as tall as its padding and a box 20 tall in the flow.
const PADDED_HEIGHT: &str = r#"box("Grand", [], [box("Parent", [.Position(.Flow), .Width(.Px(200)), .Padding(.Px(8))], [
                box("Filler", [.Height(.Px(20))], []),
                PINNED,
            ])])"#;
/// A positioned parent as wide as its padded grandparent's content.
const PADDED_WIDTH: &str = r#"box("Grand", [.Width(.Px(200)), .Padding(.Px(8))], [
                box("Parent", [.Position(.Flow), .Height(.Px(100))], [PINNED]),
            ])"#;
/// A positioned parent half as wide as its padded grandparent's content.
const PADDED_PERCENT: &str = r#"box("Grand", [.Width(.Px(200)), .Padding(.Px(8))], [
                box("Parent", [.Position(.Flow), .Width(.Percent(50.0)), .Height(.Px(100))], [PINNED]),
            ])"#;
/// A translated box past a positioned parent.
const TRANSLATED: &str = r#"box("Grand", [.Translate(.Px(0), .Px(0)), .Width(.Px(200)), .Height(.Px(100))], [
                box("Parent", [.Position(.Flow), .Width(.Px(100)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// A backdrop-blurred box past a positioned parent.
const BLURRED: &str = r#"box("Grand", [.BackdropBlur(.Px(4)), .Width(.Px(200)), .Height(.Px(100))], [
                box("Parent", [.Position(.Flow), .Width(.Px(100)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// A `Flow` grandparent past a parent with no position.
const FLOW_GRANDPARENT: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                box("Parent", [.Width(.Px(100)), .Height(.Px(50))], [PINNED]),
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
    page_holding(setup, pins, "")
}

/// A page like [`page`]'s, with "Pinned" holding `children`.
fn page_holding(setup: &str, pins: &str, children: &str) -> String {
    let pinned = format!(
        r#"ui.stack({{
                role: .Some(.Group),
                label: .Some(.Const("Pinned")),
                styles: [{}, .Background(.Rgb(255, 0, 0))],
                children: [{}],
            }})"#,
        pins, children
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

/// The same, with "Pinned" holding `kids`.
fn fitted(name: &str, setup: &str, kids: &str, pins: &str) -> String {
    drive(name, &page_holding(setup, pins, kids), SHOW)
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

/// Held to the viewport, pinned by both edges in each axis.
const VIEWPORT_BOTH: &str =
    ".Position(.PinViewport), .Pin(.Top, .Px(10)), .Pin(.Bottom, .Px(20)), \
                             .Pin(.Start, .Px(30)), .Pin(.End, .Px(40))";
/// Held to the viewport, sized by percentages.
const VIEWPORT_SIZES: &str = ".Position(.PinViewport), .Pin(.Top, .Px(0)), .Pin(.Start, .Px(0)), \
                              .Width(.Percent(50.0)), .Height(.Percent(25.0))";
/// Placed against an anchor, pinned by an edge in each axis.
const ANCHOR_EDGES: &str =
    ".Position(.PinAnchor), .Pin(.Top, .Px(10)), .Pin(.Start, .Px(20)), .Width(.Px(30)), .Height(.Px(40))";
/// Placed against an anchor, sized by percentages.
const ANCHOR_SIZES: &str = ".Position(.PinAnchor), .Pin(.Top, .Px(0)), .Pin(.Start, .Px(0)), \
                            .Width(.Percent(50.0)), .Height(.Percent(25.0))";
/// Placed against an anchor with no pin, which centres it.
const ANCHOR_CENTRED: &str = ".Position(.PinAnchor), .Width(.Px(30)), .Height(.Px(40))";

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

    both_edges_measure_inside_the_border: BORDERED, BOTH => "Parent at 30,10 120x60";
    sizes_measure_inside_the_border: BORDERED, SIZES => "Parent at 0,0 95x22.5";
    both_edges_measure_from_a_box_as_tall_as_its_padding_and_flow: PADDED_HEIGHT, BOTH => "Parent at 30,10 130x6";
    both_edges_measure_from_a_box_as_wide_as_its_parents_content: PADDED_WIDTH, BOTH => "Parent at 30,10 114x70";
    sizes_measure_from_a_box_a_percentage_of_its_parents_content: PADDED_PERCENT, SIZES => "Parent at 0,0 46x25";

    both_edges_on_the_viewport_measure_from_a_translated_box: TRANSLATED, VIEWPORT_BOTH => "Grand at 30,10 130x70";
    sizes_on_the_viewport_measure_from_a_translated_box: TRANSLATED, VIEWPORT_SIZES => "Grand at 0,0 100x25";
    both_edges_on_the_viewport_measure_from_a_blurred_box: BLURRED, VIEWPORT_BOTH => "Grand at 30,10 130x70";
    sizes_on_the_viewport_measure_from_a_blurred_box: BLURRED, VIEWPORT_SIZES => "Grand at 0,0 100x25";

    edges_on_an_anchor_measure_from_a_flow_grandparent: FLOW_GRANDPARENT, ANCHOR_EDGES => "Grand at 20,10 30x40";
    sizes_on_an_anchor_measure_from_a_flow_grandparent: FLOW_GRANDPARENT, ANCHOR_SIZES => "Grand at 0,0 100x25";
    no_pin_on_an_anchor_centres_on_a_flow_grandparent: FLOW_GRANDPARENT, ANCHOR_CENTRED => "Grand at 85,30 30x40";
    edges_on_an_anchor_with_no_flow_measure_from_the_viewport: NONE, ANCHOR_EDGES => "the viewport at 20,10 30x40";
    sizes_on_an_anchor_with_no_flow_measure_from_the_viewport: NONE, ANCHOR_SIZES => "the viewport at 0,0 512x192";
}

/// Three boxes 50 wide and 10 tall.
const KIDS: &str = r#"box("A", [.Width(.Px(50)), .Height(.Px(10))], []),
                box("B", [.Width(.Px(50)), .Height(.Px(10))], []),
                box("C", [.Width(.Px(50)), .Height(.Px(10))], [])"#;

/// A box half as wide as "Pinned", beside two boxes 50 wide.
const HALF: &str = r#"box("A", [.Width(.Percent(50.0)), .Height(.Px(10))], []),
                box("B", [.Width(.Px(50)), .Height(.Px(10))], []),
                box("C", [.Width(.Px(50)), .Height(.Px(10))], [])"#;
/// A box half as wide as "Pinned" holding one 80 wide, then one 50 wide.
const HALF_HOLDING: &str = r#"box("A", [.Width(.Percent(50.0)), .Height(.Px(10))], [
                    box("D", [.Width(.Px(80)), .Height(.Px(5))], []),
                ]),
                box("B", [.Width(.Px(50)), .Height(.Px(10))], [])"#;
/// A box half as wide as "Pinned", then one 50 wide.
const HALF_FIRST: &str = r#"box("A", [.Width(.Percent(50.0)), .Height(.Px(10))], []),
                box("B", [.Width(.Px(50)), .Height(.Px(10))], [])"#;
/// A box 30 wide and at least half as wide as "Pinned", then one 50 wide.
const AT_LEAST_HALF: &str = r#"box("A", [.Width(.Px(30)), .MinWidth(.Percent(50.0)), .Height(.Px(10))], []),
                box("B", [.Width(.Px(50)), .Height(.Px(10))], [])"#;
/// A box 30 wide and at least 70 wide, then one 50 wide.
const AT_LEAST_70: &str = r#"box("A", [.Width(.Px(30)), .MinWidth(.Px(70)), .Height(.Px(10))], []),
                box("B", [.Width(.Px(50)), .Height(.Px(10))], [])"#;
/// A box 120 wide and at most half as wide as "Pinned", then one 50 wide.
const AT_MOST_HALF: &str = r#"box("A", [.Width(.Px(120)), .MaxWidth(.Percent(50.0)), .Height(.Px(10))], []),
                box("B", [.Width(.Px(50)), .Height(.Px(10))], [])"#;

/// One test per setup, children and pin kind, with "Pinned" 40 tall and no
/// width, each expecting `against <box> at <x>,<y> <width>x<height>`. A
/// layout of `wrap` makes "Pinned" a wrapping row, and `column` leaves it
/// the column a stack is.
macro_rules! fitted {
    ($($test:ident: $setup:ident, $kids:ident, $layout:ident, $pins:literal => $expected:literal;)*) => {
        $(
            #[test]
            fn $test() {
                let layout = match stringify!($layout) {
                    "wrap" => ".Layout(.Row), .Wrap(true), ",
                    _ => "",
                };
                let pins = format!("{layout}.Height(.Px(40)), {}", $pins);
                let placed = fitted(stringify!($test), $setup, $kids, &pins);
                assert_eq!(placed, concat!("against ", $expected, "\n"));
            }
        )*
    };
}

fitted! {
    no_width_fits_beside_a_start_pin_in_a_grandparent: FLOW_GRANDPARENT, KIDS, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(20))" => "Grand at 20,0 150x40";
    no_width_fits_in_the_room_beside_a_start_pin: PARENT, KIDS, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(120))" => "Parent at 120,0 80x40";
    no_width_fits_beside_an_end_pin: PARENT, KIDS, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.End, .Px(20))" => "Parent at 30,0 150x40";
    no_width_is_never_narrower_than_its_widest_child: PARENT, KIDS, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(180))" => "Parent at 180,0 50x40";
    no_width_on_an_anchor_fits_the_anchor: FLOW_GRANDPARENT, KIDS, wrap,
        ".Position(.PinAnchor)" => "Grand at 25,30 150x40";

    no_width_counts_a_percentage_width_child_as_empty: PARENT, HALF, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(20))" => "Parent at 20,0 100x40";
    no_width_counts_a_percentage_width_child_by_its_content: PARENT, HALF_HOLDING, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(20))" => "Parent at 20,0 130x40";
    no_width_in_a_column_counts_a_percentage_width_child_as_empty: PARENT, HALF_FIRST, column,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(20))" => "Parent at 20,0 50x40";
    no_width_counts_a_percentage_minimum_as_none: PARENT, AT_LEAST_HALF, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(20))" => "Parent at 20,0 80x40";
    no_width_holds_a_child_to_its_minimum: PARENT, AT_LEAST_70, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(20))" => "Parent at 20,0 120x40";
    no_width_counts_a_percentage_maximum_as_none: PARENT, AT_MOST_HALF, wrap,
        ".Pin(.Top, .Px(0)), .Pin(.Start, .Px(20))" => "Parent at 20,0 170x40";
    no_width_on_an_anchor_counts_a_percentage_width_child_as_empty: FLOW_GRANDPARENT, HALF, wrap,
        ".Position(.PinAnchor)" => "Grand at 50,30 100x40";
}

/// Prints the boxes whose overflow cuts "Pinned", nearest first.
const CLIPS: &str = r#"
const names = clippedBy(labelled("Pinned")).map((at) => at.getAttribute("aria-label"));
console.log(`clipped by ${names.length === 0 ? "nothing" : names.join(", ")}`);
"#;

/// A `Clip` box between "Pinned" and its positioned grandparent.
const CLIP_BETWEEN: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                box("Clipper", [.Clip(true), .Width(.Px(50)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// A `Scroll` box between "Pinned" and its positioned grandparent.
const SCROLL_BETWEEN: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                box("Scroller", [.Scroll(.Both), .Width(.Px(50)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// A `Clip` box around "Pinned", with nothing positioned.
const CLIP_UNPOSITIONED: &str = r#"box("Grand", [.Width(.Px(200)), .Height(.Px(100))], [
                box("Clipper", [.Clip(true), .Width(.Px(50)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// A `Clip` box outside the positioned box "Pinned" measures from, and
/// another inside it.
const CLIP_OUTSIDE: &str = r#"box("Outer", [.Clip(true), .Width(.Px(200)), .Height(.Px(100))], [
                box("Grand", [.Position(.Flow), .Width(.Px(300)), .Height(.Px(100))], [
                    box("Clipper", [.Clip(true), .Width(.Px(50)), .Height(.Px(50))], [PINNED]),
                ]),
            ])"#;

/// One test per setup, each expecting `clipped by <boxes>`.
macro_rules! clipped {
    ($($test:ident: $setup:ident => $expected:literal;)*) => {
        $(
            #[test]
            fn $test() {
                let clipped = drive(stringify!($test), &page($setup, EDGES), CLIPS);
                assert_eq!(clipped, concat!("clipped by ", $expected, "\n"));
            }
        )*
    };
}

clipped! {
    a_clip_between_a_pin_and_its_positioned_box_cuts_nothing: CLIP_BETWEEN => "nothing";
    a_scroll_between_a_pin_and_its_positioned_box_cuts_nothing: SCROLL_BETWEEN => "nothing";
    a_clip_around_a_pin_measured_from_the_viewport_cuts_nothing: CLIP_UNPOSITIONED => "nothing";
    a_clip_outside_a_pins_positioned_box_cuts_it: CLIP_OUTSIDE => "Outer";
}

/// Prints what "Pinned" is measured from, where it lands, and how far the
/// translates a browser draws it with move it from there.
const MOVED: &str = r#"
const pinned = labelled("Pinned");
const against = containingBlock(pinned);
const name = against === null ? "the viewport" : (against.getAttribute("aria-label") ?? `<${against.localName}>`);
const { x, y, width, height } = placedAt(pinned);
const moved = movedBy(pinned);
console.log(`against ${name} at ${x},${y} ${width}x${height} moved ${moved.x},${moved.y}`);
"#;

/// A translated box around a `Flow` parent.
const AROUND_FLOW: &str = r#"box("Grand", [.Translate(.Px(40), .Px(20)), .Width(.Px(200)), .Height(.Px(100))], [
                box("Parent", [.Position(.Flow), .Width(.Px(100)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// A translated `Flow` parent.
const TRANSLATED_FLOW: &str = r#"box("Grand", [], [
                box("Parent", [.Position(.Flow), .Translate(.Px(40), .Px(20)), .Width(.Px(100)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// A translated parent inside a `Flow` grandparent.
const TRANSLATED_IN_FLOW: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                box("Parent", [.Translate(.Px(40), .Px(20)), .Width(.Px(100)), .Height(.Px(50))], [PINNED]),
            ])"#;
/// A translated box with nothing positioned.
const TRANSLATED_NONE: &str = r#"box("Grand", [.Translate(.Px(40), .Px(20)), .Width(.Px(200)), .Height(.Px(100))], [
                box("Parent", [.Padding(.Px(8))], [PINNED]),
            ])"#;
/// A translated `PinAnchor` box inside a `Flow` grandparent.
const IN_A_TRANSLATED_PANEL: &str = r#"box("Grand", [.Position(.Flow), .Width(.Px(200)), .Height(.Px(100))], [
                box("Outer", [
                    .Position(.PinAnchor),
                    .Pin(.Top, .Px(0)),
                    .Pin(.Start, .Px(0)),
                    .Translate(.Px(40), .Px(20)),
                    .Width(.Px(100)),
                    .Height(.Px(50)),
                ], [PINNED]),
            ])"#;
/// A dialog's panel inside a translated box.
const TRANSLATED_DIALOG: &str = r#"box("Grand", [.Translate(.Px(40), .Px(20))], [
                ui.dialog({
                    isOpen: signal(ctx, true),
                    label: .Const("Dialog"),
                    styles: [.Width(.Px(200)), .Height(.Px(100))],
                    children: [box("Panel", [.Padding(.Px(8))], [PINNED])],
                }),
            ])"#;

/// Placed against an anchor, pinned by an edge in each axis, and translated.
const ANCHOR_TRANSLATED: &str =
    ".Position(.PinAnchor), .Pin(.Top, .Px(10)), .Pin(.Start, .Px(20)), \
                                 .Width(.Px(30)), .Height(.Px(40)), .Translate(.Px(5), .Px(7))";

/// One test per setup and pin kind, each expecting `against <box> at <x>,<y>
/// <width>x<height> moved <x>,<y>`.
macro_rules! moved {
    ($($test:ident: $setup:ident, $pins:ident => $expected:literal;)*) => {
        $(
            #[test]
            fn $test() {
                let moved = drive(stringify!($test), &page($setup, $pins), MOVED);
                assert_eq!(moved, concat!("against ", $expected, "\n"));
            }
        )*
    };
}

moved! {
    a_pin_moves_with_a_translate_around_its_positioned_box: AROUND_FLOW, EDGES =>
        "Parent at 20,10 30x40 moved 40,20";
    a_pin_on_an_anchor_ignores_a_translate_around_the_anchor: AROUND_FLOW, ANCHOR_EDGES =>
        "Parent at 20,10 30x40 moved 0,0";
    a_pin_on_an_anchor_ignores_the_anchors_translate: TRANSLATED_FLOW, ANCHOR_EDGES =>
        "Parent at 20,10 30x40 moved 0,0";
    no_pin_on_an_anchor_centres_on_it_and_ignores_its_translate: TRANSLATED_FLOW, ANCHOR_CENTRED =>
        "Parent at 35,5 30x40 moved 0,0";
    a_pin_on_an_anchor_ignores_a_translate_inside_the_anchor: TRANSLATED_IN_FLOW, ANCHOR_EDGES =>
        "Grand at 20,10 30x40 moved 0,0";
    a_pin_on_an_anchor_with_no_flow_ignores_a_translate: TRANSLATED_NONE, ANCHOR_EDGES =>
        "the viewport at 20,10 30x40 moved 0,0";
    a_pin_on_an_anchor_ignores_a_translated_panel_around_it: IN_A_TRANSLATED_PANEL, ANCHOR_EDGES =>
        "Grand at 20,10 30x40 moved 0,0";
    a_pin_on_an_anchor_moves_by_its_own_translate: FLOW_GRANDPARENT, ANCHOR_TRANSLATED =>
        "Grand at 20,10 30x40 moved 5,7";
    a_pin_in_a_dialog_ignores_a_translate_around_the_dialog: TRANSLATED_DIALOG, EDGES =>
        "Dialog at 20,10 30x40 moved 0,0";
}
