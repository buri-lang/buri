# User interfaces

The `ui/*` modules are the reactivity vocabulary. They are part of
[the standard library](../reference/standard-library.md), ship with the
toolchain, and are never listed in a `dependencies`.

`ui/effect` declares `Watch` and `Ui`, the `Scope` a reactive closure is handed
— which reads the graph and allocates — and the `Event` a handler is handed. Requests are not among them: a page asks
for `core/effect`'s `Network` like every other platform. `ui/signal` is `Signal<T>` —
`get`, `set`, `update` — plus `signal` and `watch`. `ui/prop` is `Prop<T>` and
`memo`. `ui/testing` is a headless platform, a renderer for looking at what a
tree became, and `snapshot`, which paints one and holds it to a golden PNG. Only
a test source may import it.

The whole of it rests on one idea: **a signal handle is inert data, and the
authority to read or write it travels through `ctx`**, the same split `Allocator`
and `Region` use. So an event handler may capture a `Signal<T>`, and takes its
context as a parameter rather than closing over one.

```buri
from "ui/effect" import { Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal };

/// The lambda captures the handle. The authority arrives as `c`.
export fn addOne<C: Ui>(clicks: Signal<Int>): Node<C> {
    ui.button(
        .Const("add one"),
        [],
        [],
        fn(c, _event) => { clicks.update(c, fn(n) => n + 1) },
        .Const(false),
    )
}
```

| | Cost |
|---|---|
| `signal(ctx, v)` | O(1) |
| `get` | O(1) outside a computation. Inside one, O(k) in that computation's dependencies so far, because the edge is recorded once and recording it looks first |
| `set`, `update` | O(n) in the value's size to compare it with the one already there, and then O(d) over what read the cell — transitively through memos — where the two differ |
| `memo(ctx, f)` | O(1) to declare — `f` does not run until something reads it, and then only after a cell it actually read has changed |
| `watch(ctx, f)` | runs once now, and once per batch in which something it read changed |

Tracking is automatic and exact: every run collects the dependencies afresh, so
a read behind an `if` subscribes to the branch taken and not the other. Writing
a value equal to the one already there re-runs nothing — equal being `==`, which
is structural, so two lists of the same elements are one value however each was
built.

## Derived values

A `Scope` grants `Watch` and `Allocator`, so a derivation may map, filter, sort
or format what it read. The `Scope` arrives as a parameter, so the function
building the derivation needs no context of its own.

```buri
from "core/str" import * as str;
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal };

fn isEven(value: Int): Bool {
    value % 2 == 0
}

/// A filtered view of a signal, re-filtered on every write and reconciled by
/// key — the rows that survive are moved rather than rebuilt.
export fn evens<C>(xs: Signal<[Int]>): Node<C> {
    ui.each(
        .Computed(fn(scope) => xs.get(scope).filter(scope, isEven)),
        fn(x) => "${x}",
        fn(c, x, index) => ui.text(.Computed(fn(s) => str.format(s, "${x}"))),
    )
}
```

It cannot write. `Ui` is the effect that writes, and a closure that wrote a
signal it read would be a loop the runtime schedules rather than a value it
caches — so `set` and `update` inside a derivation are a compile error.

Nothing is counted. `allocate` on a `Scope` answers the bytes it was asked for,
as `core/host`'s own allocator does: a derived value is reclaimed when the last
reference to it goes, and there is no budget on a computation.

## The tree

`ui/node` is what an interface *is*: `Node<C>`, eighteen `Role`s, and the
nineteen functions that build one. `ui/style` is how a container arranges and
paints what is inside it. `mount`, the twentieth function, puts a tree on the
screen. Two rules run through the vocabulary.

**Meaning is the role and arrangement is the style.** `region(.List, ...)` says
what a group of children *is*, so a screen reader announces a list of five
items. `.Layout(.Row)` says only how it is arranged. No constructor is named
after an HTML element, and there is no tag-string escape hatch.

**A parameter an assistive technology cannot do without is a parameter.**
`image` takes its `alt`, `link` its `dest`, and `field` and `toggle` their
`label`. A field with no label is not something this vocabulary can express,
which is what makes the commonest accessibility failure on the web a compile
error. `icon` is the other side of that rule: it is decoration and carries no
name at all, because what it means is said by the button or the link it is
inside.

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };

/// A check, as an icon set ships one: `currentColor` where the stroke goes.
let check: Str =
    "<svg viewBox='0 0 24 24' stroke='currentColor'><path d='M20 6 9 17l-5-5'/></svg>";

/// It paints in whatever colour the thing around it is painting in.
export fn saved<C>(): Node<C> {
    ui.icon([.Width(.Px(16)), .Height(.Px(16))], check)
}
```

An `icon` puts the artwork **in** the tree, as an `<svg>`, which is what lets
`currentColor` in it be the element's own `Foreground`: the glyph follows the
text beside it and turns over with a theme, for nothing. An `image` cannot —
its source is a document of its own, so a data URI paints whatever colour was
baked into it. The artwork is written out at the call site because the compiler
reads it: an `<svg>` and the shapes inside it, and a script or a reference to
somewhere else is `icon-not-drawable` rather than something the renderer
quietly drops.

A component is an ordinary function and it runs **once**. Three constructors
put reactivity in the tree, and each re-runs the smallest thing it can:

| | What re-runs |
|---|---|
| a `Prop` on a leaf | one run of text, or one attribute. Nothing else in the tree is touched, and a `Prop.Const` registers nothing at all |
| `choose(cond, then, otherwise)` | one of two subtrees, when the condition changes. The subtree that goes is disposed, and the computations inside it go with it |
| `computed(build)` | the subtree `build` answers, when anything `build` read changes. The coarse instrument: reach for a `Prop` on a leaf when only a string is changing |
| `each(items, key, row)` | O(n) in the list, and **no row that is still there**: a row is keyed, so a reorder moves it and never rebuilds it. That is what keeps the focus, the scroll position and the computations inside a row alive |

The conditional is `choose` rather than `when`, because `when` is a reserved
word and no function may be called one.

Handlers — `button`'s `onPress` and `form`'s `onSubmit` — take their context as
a parameter, because a lambda may not capture one, and the runtime hands each
the very context the tree was mounted with. Everything one press writes is one
update: the handler runs inside a transaction, so three writes cause one pass
over the watchers rather than three. A field and a toggle have no change event
at all — they are bound to a `Signal`, and what the reader typed is in it.

**A form ends in a `submit`.** Enter in a field is the browser's own dispatch,
and the browser's own rule comes with it: a form is submitted implicitly
through its submit button, and a form with none is submitted only while it
holds exactly one field. So `submit` is the button that carries no handler —
the form's `onSubmit` is its handler — and an ordinary `button` beside it stays
the Cancel it was written as.

```buri
from "ui/effect" import { Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal };

export fn contact<C: Ui>(
    name: Signal<Str>,
    email: Signal<Str>,
    sent: Signal<Str>,
): Node<C> {
    ui.form(fn(c, _e) => sent.set(c, "sent"), [], [
        ui.field(.Const("Name"), .Text, [], [], name, .Const(false), .Const(false)),
        ui.field(.Const("Email"), .Email, [], [], email, .Const(false), .Const(false)),
        ui.submit(.Const("Send"), []),
    ])
}
```

## Styling, and the two tiers a style can be in

`ui/style` is 53 properties and five ways of composing them. Every property is
one value applied to one element, none is named after a CSS declaration, and
there is no `margin`: `Gap`, stacks and `AlignCross` replace it. Edges are
logical (`.Start`, `.End`) rather than left and right, so a right-to-left page is
right by construction, and a corner is the two edges that meet at it
(`.TopStart`). What matters is where a style *goes*.

A distance is a `Length`: `.Px`, `.Rem`, `.Em`, `.Percent`, `.Auto`, `.Full`. A
rem follows the root's text size and an em follows the element's own, which is
what tracking wants — `.LetterSpacing(.Em(-0.025))` is right at every size the
type is ever set at, and the rem that matches it at sixteen pixels is wrong at
every other.

`Layout` is `.Column`, `.Row`, `.ColumnReverse`, `.RowReverse`, `.Grid` and
`.Layers`. A reversed stack paints its children backwards and leaves the
document's order alone — the order the caller wrote, and the order a screen
reader and the tab ring read — so a toaster grows from the bottom and a dialog
footer puts the confirming action on top without the caller reordering anything.
A grid does not reverse: its children go in the tracks the container named.

`Bleed(Edge, Length)` is the one way *out* of the box a container put a child
in, and it is a distance outwards rather than a margin: `.Auto` and a negative
length bleed nothing, so the space between things still belongs to the
container. Two bleeds naming different edges compose.

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };

/// A rule from one edge of a menu padded by four to the other. Without the
/// bleed it stops four short at each end.
export fn separator<C>(): Node<C> {
    ui.stack([.Height(.Px(1)), .Bleed(.Start, .Px(4)), .Bleed(.End, .Px(4))], [])
}

/// An avatar that laps the one before it. The later one paints over the
/// earlier, the way a document stacks them.
export fn lapped<C>(letter: Str): Node<C> {
    ui.stack(
        [
            .Width(.Px(32)),
            .Height(.Px(32)),
            .Radius(.Percent(50.0)),
            .Bleed(.Start, .Px(8)),
        ],
        [ui.text(.Const(letter))],
    )
}
```

`Clip(Bool)` cuts what runs past the box, corners included, and makes no scroll
container while it does — that is the whole of what separates it from
`Scroll(Axis)`. `Passthrough(Bool)` says the pointer goes to whatever is behind
this element instead. Both inherit the way the platform does, so a subtree opts
out and a child opts back in.

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };

/// A card whose banner runs edge to edge. Without the clip the picture squares
/// the corner the radius rounded.
export fn card<C>(banner: Node<C>, body: Node<C>): Node<C> {
    ui.column([.Radius(.Px(12)), .Clip(true)], [banner, body])
}

/// A toaster dock pinned across the viewport. It must not intercept a press
/// meant for the page under it; each toast in it takes the pointer back.
export fn dock<C>(toasts: [Node<C>]): Node<C> {
    ui.column(
        [
            .Position(.PinViewport),
            .Pin(.Bottom, .Px(0)),
            .Width(.Full),
            .Passthrough(true),
        ],
        toasts,
    )
}

export fn toast<C>(text: Node<C>): Node<C> {
    ui.stack([.Passthrough(false), .Padding(.Px(12))], [text])
}
```

**Static — everything except `Computed`.** The compiler evaluates it, turns each
distinct property value into one atomic class, and writes the classes into a
stylesheet that ships with the artifact. `.Padding(.Px(8))` is `.p-8` wherever it
was written, in whichever module, so two packages that ask for the same padding
get one class and one rule. Nothing is generated at run time.

Two constructors exist only in this tier, because neither has an inline form:

- `On(State, [Style])` is a state — hover, focus, focus-within, pressed,
  disabled, checked, invalid. **This is why hover is not an event.** It costs
  nothing, needs no signal write on a mouse move, and maps to a native pressed
  or focused trait. `Focus` is the element's own keyboard attention and
  `FocusWithin` is a container's: the wrapper of an input group owns the
  hairline and the ring, and the control inside it stays bare. Four of the
  seven are the platform's own. `Invalid` and `Disabled` are the two a program
  enters, by passing a control an `invalid` or a `disabled` — which write the
  `aria-invalid` and the `disabled` a reader is told about and the rules hang
  off, so the ring and the announcement are one fact. `Checked` is neither: a
  toggle's own signal is the answer, so a page holds one that is on and one
  that is off and each paints its own.
- `At(Screen, [Style])` is a breakpoint, from one of four widths upwards.
  Mobile-first: the media queries are written in ascending order, so a larger
  tier overrides a smaller one by position, and there is never a maximum-width
  query. What is outside every `At` is the smallest screen's.

`When(cond, then, otherwise)` is static on both sides. Both branches go in the
stylesheet, and when `cond` changes the runtime picks one of two precomputed
class strings.

**Computed — `Computed(fn(Scope) => [Style])`.** For a value a signal drives: a
drag, a cursor-follow, an animation. It is applied inline and re-serialised on
every change, and never reaches the stylesheet. Reach for it last.

A style the compiler cannot evaluate — one built out of a function's parameters,
or out of a value it had to read — is **not an error**. It degrades to the same
inline application `Computed` gets. `On` and `At` are the exception, because
they have nowhere to degrade to: a style under one of them is statically known,
or the compiler rejects the program
([`style-not-static`](../reference/errors/style-not-static.md)).

**Conflicts resolve per property, last wins.** When both sides are literals the
compiler resolves them, and the element carries one class rather than two that
fight. When a style *arrives as a parameter* — the overridable-component case —
the runtime resolves it with a scan over `(slot, class)` pairs the compiler
assigned, which can only choose between classes already in the sheet. Between
two *different* properties that touch the same declaration, say `Padding` and
`PaddingX`, the order the variants are declared in decides, and the narrower
property is always declared later.

**Four properties name one edge or one corner**, and two of them naming
different ones compose rather than fighting: `Pin`, `PaddingEdge`,
`BorderEdge(Edge, Length)` and `RadiusCorner(Corner, Length)`. A border's colour
and style stay whole-box, so `BorderEdge` is the row rule under a table row and
the guide line down a submenu, and nothing has to invent a one-pixel element to
draw one. `RadiusCorner` is how a caller that already knows where a child sits
squares the side that meets its neighbour — a joined button group maps over its
own children, so the parent decides and the child carries plain styles.

```buri
from "ui/effect" import { Event };
from "ui/node" import * as ui;
from "ui/node" import { Node };

/// One button of a group welded to the one beside it.
export fn joined<C>(label: Str, first: Bool, onPress: fn(C, Event) => ()): Node<C> {
    ui.button(
        .Const(label),
        [
            .PaddingX(.Px(12)),
            .PaddingY(.Px(6)),
            .Radius(.Px(6)),
            .BorderWidth(.Px(1)),
            .Group(if (first) {
                []
            } else {
                [
                    .RadiusCorner(.TopStart, .Px(0)),
                    .RadiusCorner(.BottomStart, .Px(0)),
                    .BorderEdge(.Start, .Px(0)),
                ]
            }),
        ],
        [],
        onPress,
        .Const(false),
    )
}
```

**An element casts one `box-shadow`, and it may have layers.** `Shadow(Shadow)`
is one; `Shadows([Shadow])` is a list, painted first over last. Every elevation
worth having is two — a wide soft layer, and a tight one that keeps the near
edge crisp — and a focus ring is a spread shadow beside them, so a card that is
raised *and* focused wants three. The two spellings are one conflict slot, so
whichever is written last is the element's shadow; reach for `Shadow` when there
is one layer.

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/style" import { Shadow, Style };

let lift: Shadow = Shadow {
    x: .Px(0),
    y: .Px(4),
    blur: .Px(6),
    spread: .Px(-1),
    color: .Rgba(0, 0, 0, 0.1),
};

let near: Shadow = Shadow {
    x: .Px(0),
    y: .Px(2),
    blur: .Px(4),
    spread: .Px(-2),
    color: .Rgba(0, 0, 0, 0.1),
};

let ring: Shadow = Shadow {
    x: .Px(0),
    y: .Px(0),
    blur: .Px(0),
    spread: .Px(3),
    color: .Rgba(59, 130, 246, 0.5),
};

export fn card<C>(label: Str): Node<C> {
    ui.stack(
        [
            .Radius(.Px(8)),
            .Shadows([lift, near]),
            .On(.Focus, [.Shadows([ring, lift, near])]),
        ],
        [ui.text(.Const(label))],
    )
}
```

**A control carries its own styles.** `button`, `submit`, `link`, `image`,
`field` and `toggle` take a `[Style]`, and it lands on the element itself — so
`On(.Hover, ...)`, `On(.Focus, ...)` and `On(.Disabled, ...)` fire. A wrapper
around a button is none of those things. A field's and a toggle's styles go on
the input rather than on the label around it, for the same reason, and a
picture's go on the picture: only the picture can be told to fill its box, to
crop square, or to take a corner.

```buri
from "ui/effect" import { Event };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/prop" import { Prop };

export fn primary<C>(
    label: Str,
    onPress: fn(C, Event) => (),
    busy: Prop<Bool>,
): Node<C> {
    ui.button(
        .Const(label),
        [
            .PaddingX(.Px(12)),
            .PaddingY(.Px(6)),
            .Radius(.Px(6)),
            .Background(.Rgb(24, 24, 27)),
            .Foreground(.Rgb(255, 255, 255)),
            .On(.Hover, [.Opacity(0.9)]),
            .On(.Disabled, [.Opacity(0.5)]),
        ],
        [],
        onPress,
        busy,
    )
}

export fn avatar<C>(source: Str): Node<C> {
    ui.stack([.Width(.Px(32)), .Height(.Px(32)), .Radius(.Full)], [
        ui.image(.Const(source), .Const(""), [
            .Width(.Full),
            .AspectRatio(1.0),
            .Radius(.Full),
        ]),
    ])
}
```

`Translate(Length, Length)` moves an element after it has been laid out, so
nothing around it shifts. That is what a press is:
`On(.Active, [.Translate(.Px(0), .Px(1))])` sinks a button by a pixel and leaves
the row it is in alone, where a padding would reflow the row.

**A state a widget holds is the widget's answer, not the snapshot's.**
`button`, `field` and `toggle` take a `disabled: Prop<Bool>`, and it is an
attribute rather than a style: it takes the control out of the tab order,
refuses the press before the handler runs, and tells a reader the control is
unavailable rather than absent. Dimming it is `On(.Disabled, ...)`, which the
flag is what fires. `On(.Checked, ...)` needs no flag at all — a toggle's own
signal says whether it is checked, the same signal its mark is drawn from, so a
page holds one toggle that is on and one that is off and each paints its own
answer.

**A button holds children, and one with none shows its label.** So a mark and a
word are one element: the wash that says hovered, or says current page, covers
both, because there is one thing to wash. The label stays a parameter and the
markup carries it as `aria-label`, so what a reader hears is never the glyphs.

```buri
from "ui/effect" import { Event };
from "ui/node" import * as ui;
from "ui/node" import { Node };

export fn entry<C>(mark: Node<C>, onPress: fn(C, Event) => ()): Node<C> {
    ui.button(
        .Const("Overview"),
        [.Gap(.Px(8)), .AlignCross(.Center), .On(.Hover, [.Opacity(0.9)])],
        [mark, ui.text(.Const("Overview"))],
        onPress,
        .Const(false),
    )
}
```

**A labelled control takes two style lists.** `field` and `toggle` render an
input inside a `<label>`, and the label is the box a surrounding `row` lays out.
`styles` lands on the input, `around` lands on the label — so `Grow`, `Shrink`,
`AlignSelf`, `Span` and `Width` belong in `around`, and everything the input is
belongs in `styles`.

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal };

/// The addons keep their width and the field takes the rest.
export fn site<C>(value: Signal<Str>): Node<C> {
    ui.row([.Width(.Full)], [
        ui.stack([.Shrink(0)], [ui.text(.Const("https://"))]),
        ui.field(
            .Const("Site"),
            .Text,
            [.Width(.Full)],
            [.Grow(1)],
            value,
            .Const(false),
            .Const(false),
        ),
        ui.stack([.Shrink(0)], [ui.text(.Const(".com"))]),
    ])
}
```

**A toggle draws its own mark**, and `ToggleKind` picks which: a `Checkbox` has
a tick when it is on, a `Switch` has a thumb that sits at the near end of its
track when off and at the far end when on. Both are painted in the box's
`Foreground`, so that is the colour that marks it.

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal };

export fn notify<C>(value: Signal<Bool>): Node<C> {
    ui.toggle(
        .Const("Email me every week"),
        .Switch,
        [
            .Width(.Px(32)),
            .Height(.Px(18)),
            .Radius(.Full),
            .Padding(.Px(2)),
            .Background(.Rgb(200, 205, 215)),
            .Foreground(.Rgb(255, 255, 255)),
            .On(.Checked, [.Background(.Rgb(40, 120, 220))]),
        ],
        [],
        value,
        .Const(false),
        .Const(false),
    )
}
```

**A `.Range` field is a slider**, and it carries what it runs between:
`Range(min, max, step)`. The three are the kind's payload because a reader is
told the range they are dragging inside, so by the rule above they are
parameters rather than attributes somebody may remember to set — and the one
`type="range"` they lower to buys the thumb, the drag, the arrow and Home/End
keys and the `role="slider"` announcement without a line of your own. The bar
and the thumb are the control's `Foreground`; a `Background` sits behind them.

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal };

export fn volume<C>(value: Signal<Str>): Node<C> {
    ui.field(
        .Const("Volume"),
        .Range(0.0, 100.0, 1.0),
        [.Width(.Px(180)), .Foreground(.Rgb(40, 50, 90))],
        [],
        value,
        .Const(false),
        .Const(false),
    )
}
```

The value is a `Signal<Str>` like every other kind's, because it is what the
control holds rather than what it means — a browser answers a range's `value` as
the text of a number. Read it with `str.toFloat`.

`heading` takes one too. Its level is the document's outline, so the size and
the weight are the styles' — an unstyled heading reads at the size of the text
around it.

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };

export fn title<C>(text: Str): Node<C> {
    ui.heading(2, [.FontSize(.Px(28)), .FontWeight(.Bold)], .Const(text))
}
```

The sheet opens by dropping what a browser paints on one of these by itself —
the bevel on a button, the blue underline on a link, the border and the inner
shadow on a field, the size, the weight and the margins on a heading, the inset
border on a separator, the baseline a picture or an icon sits on — so your
styles are all there is. Those rules are `:where(...)`, which weighs nothing in
the cascade,
and only the elements the program actually builds get one. The same rules lay a
labelled control's `<label>` out as a wrapping row, give a checkbox its box and
its mark, and give a range its track, its bar and its thumb, and a class on any
of them beats them.

Three of the opening rules are about the page rather than about one element:

- **A size is the whole box.** `box-sizing: border-box`, so a `Width` beside a
  `Padding` or a `BorderWidth` counts them inside it and a full-width padded
  child stays inside its parent. It is what the headless painter measures too.
- **A container with no `Layout` is a column**, which is what `Layout`'s default
  says it is. `AlignMain` and `AlignCross` therefore mean something on a bare
  `stack`. The four table elements keep a browser's own table layout, because
  that is what the scene document mirrors for them.
- **The document carries no margin.** A browser puts eight pixels around
  `<body>`, which is where `mount` renders, so without this every page sits in a
  band of the browser's white. Nothing a program writes could reach it: this
  vocabulary has no margin at all.

**A designed focus ring replaces the platform's.** A browser paints its own
`outline` over anything you put on the focused element, so the sheet takes it
away from exactly the elements that draw a ring of their own — an
`On(.Focus, ...)` carrying a `Shadow`, `Shadows`, `BorderWidth` or `BorderEdge`.
A control that styles nothing keeps the platform's ring, and so does one whose
ring only starts at a breakpoint, because there is a width at which it paints
nothing.

**A list region is reset the same way.** `region(.List, ...)` is a `ul`, and a
browser marks and indents one by itself, so the sheet drops the disc, the
indent and the margin — a rail, a menu and a tab strip are all lists, and none
of them wants a bullet. `ListMarker(.Disc)` or `ListMarker(.Decimal)` asks for
marks back. They hang outside the item, as a browser's do, so give the list a
`PaddingEdge(.Start, ...)` for them to sit in.

Constant folding is what makes design tokens work. `.Background(Token.Surface.color())`
is a *call*, not a literal, and it still reaches the stylesheet: the extractor
inlines any function that is pure by its signature — no `ctx`, no
effect-carrying `self`, no allocator.

## Design tokens, and why exhaustiveness is the whole contract

A design token is a name whose value the app decides. Every package that uses
tokens declares its own closed vocabulary as an ordinary enum, with a
constructor answering a colour:

```buri
from "core/effect" import { Allocator };
from "core/host" import * as host;
from "ui/effect" import { Scope, Ui, Watch };
from "ui/node" import * as ui;
from "ui/style" import * as style;
from "ui/style" import { Color };
from "ui/theme" import * as theme;
from "ui/theme" import { Theme };

/// `cardlib`'s vocabulary, and the constructor that names each of its tokens.
export enum Token {
    Surface,
    OnSurface,
    Danger,
}

impl Token {
    export fn color(self): Color {
        match (self) {
            .Surface => style.token("cardlib", "surface"),
            .OnSurface => style.token("cardlib", "onSurface"),
            .Danger => style.token("cardlib", "danger"),
        }
    }
}

/// `cardlib`'s half of the loop: the one function only it can write, because
/// only it knows what its tokens are.
export fn themed(f: fn(Token) => Color): Theme {
    theme.themed([
        (Token.Surface.color(), f(.Surface)),
        (Token.OnSurface.color(), f(.OnSurface)),
        (Token.Danger.color(), f(.Danger)),
    ])
}

/// The consumer's half. This `match` is the compatibility check: a colour
/// written out, or another package's token, which is a chain.
fn cardTheme(t: Token): Color {
    match (t) {
        .Surface => .Rgb(240, 240, 245),
        .OnSurface => .Rgb(24, 24, 27),
        .Danger => .Rgb(220, 38, 38),
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Ui: host.ui,
        Watch: host.watch,
    };
    let card = ui.stack([.Background(Token.Surface.color())], []);
    ui.mount(ctx, card, [themed(cardTheme)])
}
```

`style.token` answers a `Color.Token`, which holds an opaque reference and
nothing else, so a library's styles name only its own vocabulary and
`.Background(Token.Surface.color())` cannot name a token that does not exist. The
app closes the loop at mount, with **one theme per package it uses**: the
package's `themed` applied to the app's mapping, all of them in the list `mount`
takes.

**Exhaustiveness is the compatibility contract.** The day `cardlib` adds a
token, that `match` stops covering its type, and every consumer fails to compile
until it says what the new token is worth
([`match-not-exhaustive`](../reference/errors/match-not-exhaustive.md)). No
registry, no schema language, no default.

Chains resolve at mount, in one step: a library's token to the app's token to a
colour is followed until it reaches a value.

**A faded token is still that token.** `color.alpha(f)` answers the same colour
at `f` of its opacity, and on a token it stays a token:
`.BorderColor(Token.Ring.color().alpha(0.5))` is
`color-mix(in srgb, var(--cardlib-ring) 50%, transparent)`, so the theme still
decides the hue. That is what saves a design system from minting a
`var(--cardlib-ringSoft)` beside `var(--cardlib-ring)` and hand-blending the
pair in every theme, where the two drift apart the day one of them moves. A
colour written out
has nothing to defer and fades to a plain `Rgba`; `.Transparent` and `.Inherit`
answer themselves. The fraction is `0.0` to `1.0` and anything else is refused
([`style-alpha-out-of-range`](../reference/errors/style-alpha-out-of-range.md)),
because a `color-mix` percentage outside 0 to 100 makes the whole declaration
invalid and the element loses the colour rather than gaining a louder one.

`Opacity` is the other instrument and a different one: it fades the whole
element, content included, and `alpha` fades one of its colours.

**On the web, a token is a namespaced custom property.** A class in the
stylesheet reads `var(--cardlib-surface)`, where the namespace is the package,
so a library's tokens and an app's can never collide. The class therefore does
not depend on what the token turns out to be worth, and a theme is a `:root`
block of values written once at mount.

That is what makes dark mode free. `theme.switching(condition, whenTrue,
whenFalse)` takes a `Prop<Bool>`: a signal the app writes, a stored preference,
a media query bridged into one. When it changes the runtime writes the block of
values again. No class changes, no element is touched.

`theme.scheme(.Dark)` is a theme that binds no token and says only which scheme
the page is in — `color-scheme`, in the same `:root` block. It is what the
platform paints its own things in: a native control, the document scrollbar, a
date picker, the form autofill. Without it a page painted near-black still
reports itself light, and every one of those glares white. It belongs to the
app rather than to a package, and `switching` takes one on either side.

```buri
from "ui/prop" import { Prop };
from "ui/theme" import * as theme;
from "ui/theme" import { Theme };

export fn schemeFor(dark: Prop<Bool>): Theme {
    theme.switching(dark, theme.scheme(.Dark), theme.scheme(.Light))
}
```

`theme.page(background, foreground)` is the other one, and it says what the page
itself is painted in:

```buri
from "ui/theme" import * as theme;
from "ui/theme" import { Theme };

export fn ground(): Theme {
    theme.page(.Rgb(10, 10, 10), .Rgb(250, 250, 250))
}
```

That lands on the document — `background-color` and `color` on `body`, in the
same block — so the window is the ground colour to its edges and every box
inherits the text colour. A box inside the page cannot do this: it paints only
as far as it reaches, leaving the browser's white behind an overscroll and
anywhere the root box does not cover. Either colour may be a token, read the way
a class reads one, so switching the values switches the page.

## Snapshots

`ui/testing`'s `snapshot` paints a tree and compares the PNG, byte for byte,
against a golden checked in beside the suite:

```buri role=test
from "core/effect" import { Allocator };
from "core/host/testing" import { alloc };
from "ui/effect" import { Ui };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/prop" import { Prop };
from "ui/testing" import { headless, snapshot };

fn card<C>(name: Prop<Str>): Node<C> {
    ui.stack([.Padding(.Px(8))], [ui.text(name)])
}

test "the card" {
    let ctx = context {
        Allocator: alloc(),
        Ui: headless(),
    };
    snapshot(ctx, "card", card(.Const("Ada")), .Hover, []);
}
```

The last argument is `mount`'s, and it means the same thing: one theme per
package whose tokens the tree uses. The painter resolves them before it puts a
colour anywhere, so two calls with two theme lists give a component a light
golden and a dark one.

```sh
buri test //lib/cardlib --update    record test/__snapshots__/card.png
buri test //lib/cardlib             compare against it
```

A mismatch fails the test:

```text
the snapshot "card" changed: see test/__snapshots__/card.diff.png
```

That file is the golden washed out, with every differing pixel in magenta. The
next `--update` clears it.

**The toolchain paints it — no browser, no window, and no markup anywhere near
a snapshot file.** `taffy` lays out, `cosmic-text` shapes, `tiny-skia` paints,
and three Roboto faces ship with the toolchain. No system font is ever loaded,
hinting is off, layout rounds in exactly one place, and the PNG encoder is
written for this. So the same tree paints the same bytes on Linux and on macOS,
the comparison needs no tolerance, and a golden is worth checking in.

**Snapshots run natively.** A suite that says `test { platforms: [JS] }` fails
the call, because the JavaScript runtime has no painter:

```text
the snapshot "card" was not painted: snapshots run natively, and this suite is JS
```

The graph runs there too: `signal`, `memo`, `watch` and the `Recorder` are all
native, so a suite that reads and writes signals needs no `platforms: [JS]`
either. `ui/testing`'s `render` is the part that still does — it wants a
document, and only a browser has one.

The rest is short:

- The context binds `Allocator` as well as `Ui`, because building the scene builds a
  string. The signature is
  `snapshot<C: Allocator + Ui>(ctx: C, name: Str, root: Node<C>, state: State): ()`.
- `state` is `ui/style`'s `State`, and it applies to **every** element in the
  tree. A hovered card and a resting one are two snapshots of one tree.
- **The picture is the size of what is in it.** The page is 800 CSS pixels
  across and as tall as the paint came to, never less than one line. So a
  snapshot of a button is the size of a button, and a page taller than a screen
  is painted whole rather than stopping half way down. Where paint runs past
  the page sideways — a dock pinned wider than it, a bleed, a translate off the
  edge — the picture is wider too, and the layout still ran against the 800.
- **800 across is stated because things resolve against it.** `At(.Small)` and
  every percentage width are measured from the page, so its width may not
  depend on what the tree turned out to hold. `snapshotWide` is the same call
  with that width stated:

  ```text
  snapshotWide(ctx, "card-narrow", card(.Const("Ada")), .Hover, [], 390);
  ```

  which is how one tree is painted either side of a breakpoint. Only the width
  is stated: the height is still the paint's, because a height a test picks is
  a height that cuts something off.
- A snapshot name is a file name: never empty, never holding a path separator.
  It names one file, so two `snapshot` calls with one name share one golden and
  the last `--update` wins.
- `.FontSize` and `.LineHeight` bottom out at one pixel. Zero is a size a
  program may ask for and not a picture anyone can compare.
- **An icon's `currentColor` is the colour its element paints in**, so a golden
  shows a glyph following the `Foreground` around it and turning over between
  two themes. An image's is black whatever the page says, because its source is
  a document of its own.
- **A snapshot fetches nothing**, so an image paints from its source or not at
  all. A `data:` URI holding a PNG paints at its own pixel size, and one holding
  an SVG is drawn at whatever size the box is — shapes, paths and transforms,
  but no text, gradients or CSS. Every other source — an address, a path, an
  interlaced PNG — paints a framed grey placeholder at the size the box around
  it declared.
- `.Position(.PinViewport)` is measured against the page, so a dock pinned to
  the bottom right lands in the bottom right however deep it was written, and it
  paints over everything else. The page it is measured against is the flow's:
  the tree settles how tall the picture is, and the pin is placed on it — so a
  bottom-pinned dock sits on the last row of the tree rather than a screenful
  below it.
- A `.Clip` or a `.Scroll` is measured at its own box. It cuts what is under
  it, so what a reader would have to scroll to see is not what the picture is
  tall enough to show.
- `.Table` stacks its rows and a `.TableRow` divides into one equal column per
  cell, so a column lines up down the table. `.ColumnHeader` and `.RowHeader`
  are bold and centred, which is what a browser does to a `<th>`.
- **A `.Password` field's value is never painted.** It is one • per
  character, the way a browser draws `<input type="password">`, so a recorded
  golden holds the width of the secret and none of it. `.Multiline` is the one
  kind that wraps; the other four paint alike, because the reset takes away the
  chrome a browser would tell them apart by.
- **A `.Range` field is painted as the slider it is**: a bar across the middle
  of the track and a round thumb on it at the value, both in the control's
  `Foreground`. Its value never appears as text — a browser draws no number for
  one either. A value outside the bounds is clamped into them and one that is
  not a number sits in the middle, which is what HTML says a `value` attribute
  is worth. A track shorter than one line is the one place the picture and a
  browser differ: the thumb shrinks to fit the box here and overflows it there.
- `ui/node`'s `describe(ctx, root, state)` answers the scene document `snapshot`
  paints — every prop read, every style expanded, every child in order. Print it
  when a snapshot surprises you.

A `box-shadow`'s blur is three integer box passes over a coverage mask — the
approximation the SVG filter specification writes down for a Gaussian, and what
a browser does for a shadow. Integers, so the bytes are the same bytes on every
machine.
