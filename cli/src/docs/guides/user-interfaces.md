# User interfaces

The `ui/*` modules are the reactivity vocabulary. They are part of
[the standard library](../reference/standard-library.md), ship with the
toolchain, and are never listed in a `dependencies`.

`ui/effect` declares `Watch` and `Ui`, the `Scope` a reactive closure is handed,
and the `Event` a handler is handed. Requests are not among them: a page asks
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
    ui.button(.Const("add one"), [], fn(c, _event) => clicks.update(c, fn(n) => n + 1))
}
```

| | Cost |
|---|---|
| `signal(ctx, v)` | O(1) |
| `get` | O(1) outside a computation. Inside one, O(k) in that computation's dependencies so far, because the edge is recorded once and recording it looks first |
| `set`, `update` | O(1) when the value is unchanged, and otherwise O(d) over what read the cell, transitively through memos |
| `memo(ctx, f)` | O(1) to declare — `f` does not run until something reads it, and then only after a cell it actually read has changed |
| `watch(ctx, f)` | runs once now, and once per batch in which something it read changed |

Tracking is automatic and exact: every run collects the dependencies afresh, so
a read behind an `if` subscribes to the branch taken and not the other. Writing
a value identical to the one already there re-runs nothing.

## The tree

`ui/node` is what an interface *is*: `Node<C>`, eighteen `Role`s, and the
seventeen functions that build one. `ui/style` is how a container arranges and
paints what is inside it. `mount`, the eighteenth function, puts a tree on the
screen. Two rules run through the vocabulary.

**Meaning is the role and arrangement is the style.** `region(.List, ...)` says
what a group of children *is*, so a screen reader announces a list of five
items. `.Layout(.Row)` says only how it is arranged. No constructor is named
after an HTML element, and there is no tag-string escape hatch.

**A parameter an assistive technology cannot do without is a parameter.**
`image` takes its `alt`, `link` its `dest`, and `field` and `toggle` their
`label`. A field with no label is not something this vocabulary can express,
which is what makes the commonest accessibility failure on the web a compile
error.

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

## Styling, and the two tiers a style can be in

`ui/style` is 46 properties and five ways of composing them. Every property is
one value applied to one element, none is named after a CSS declaration, and
there is no `margin`: `Gap`, stacks and `AlignCross` replace it. Edges are
logical (`.Start`, `.End`) rather than left and right, so a right-to-left page is
right by construction. What matters is where a style *goes*.

**Static — everything except `Computed`.** The compiler evaluates it, turns each
distinct property value into one atomic class, and writes the classes into a
stylesheet that ships with the artifact. `.Padding(.Px(8))` is `.p-8` wherever it
was written, in whichever module, so two packages that ask for the same padding
get one class and one rule. Nothing is generated at run time.

Two constructors exist only in this tier, because neither has an inline form:

- `On(State, [Style])` is a pseudo-class — hover, focus, pressed, disabled,
  checked. **This is why hover is not an event.** It costs nothing, needs no
  signal write on a mouse move, and maps to a native pressed or focused trait.
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

**A control carries its own styles.** `button`, `link`, `field` and `toggle`
take a `[Style]`, and it lands on the element itself — so `On(.Hover, ...)`,
`On(.Focus, ...)` and `On(.Disabled, ...)` fire. A wrapper around a button is
none of those things. A field's and a toggle's styles go on the input rather
than on the label around it, for the same reason.

```buri
from "ui/effect" import { Event };
from "ui/node" import * as ui;
from "ui/node" import { Node };

export fn primary<C>(label: Str, onPress: fn(C, Event) => ()): Node<C> {
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
        onPress,
    )
}
```

The sheet opens by dropping what a browser paints on one of these by itself —
the bevel on a button, the blue underline on a link, the border and the inner
shadow on a field — so your styles are all there is. Those rules are
`:where(...)`, which weighs nothing in the cascade, and only the elements the
program actually builds get one. A checkbox is left alone: `appearance: none`
erases the tick, and this vocabulary has nothing to draw a new one with.

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

**On the web, a token is a namespaced custom property.** A class in the
stylesheet reads `var(--cardlib-surface)`, where the namespace is the package,
so a library's tokens and an app's can never collide. The class therefore does
not depend on what the token turns out to be worth, and a theme is a `:root`
block of values written once at mount.

That is what makes dark mode free. `theme.switching(condition, whenTrue,
whenFalse)` takes a `Prop<Bool>`: a signal the app writes, a stored preference,
a media query bridged into one. When it changes the runtime writes the block of
values again. No class changes, no element is touched.

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
    snapshot(ctx, "card", card(.Const("Ada")), .Hover);
}
```

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
- The viewport is 800x600 CSS pixels, always.
- A snapshot name is a file name: never empty, never holding a path separator.
  It names one file, so two `snapshot` calls with one name share one golden and
  the last `--update` wins.
- `.FontSize` and `.LineHeight` bottom out at one pixel. Zero is a size a
  program may ask for and not a picture anyone can compare.
- **A snapshot fetches nothing**, so an image paints from its source or not at
  all. A `data:` URI holding a PNG paints at its own pixel size; every other
  source — an SVG data URI, an address, a path — paints a framed grey
  placeholder at the size the box around it declared.
- `.Position(.PinViewport)` is measured against the page, so a dock pinned to
  the bottom right lands in the bottom right however deep it was written, and it
  paints over everything else.
- `.Table` stacks its rows and a `.TableRow` divides into one equal column per
  cell, so a column lines up down the table. `.ColumnHeader` and `.RowHeader`
  are bold and centred, which is what a browser does to a `<th>`.
- `ui/node`'s `describe(ctx, root, state)` answers the scene document `snapshot`
  paints — every prop read, every style expanded, every child in order. Print it
  when a snapshot surprises you.

A `box-shadow`'s blur is three integer box passes over a coverage mask — the
approximation the SVG filter specification writes down for a Gaussian, and what
a browser does for a shadow. Integers, so the bytes are the same bytes on every
machine.
