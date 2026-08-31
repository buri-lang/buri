## User interfaces

The `ui/*` modules are the reactivity vocabulary: a much larger surface than
`core/*`, and a different kind of thing. They are part of the standard library
([the standard library](./standard-library.md)), ship with the toolchain, and
are never listed in a `dependencies`.

`ui/effect` declares `Watch` and `Ui`, the `Scope` a reactive closure
is handed, and the `Event` a handler is handed. Requests are not among them: a
page asks for `core/effect`'s `Net` like every other platform, and writes the
answer on the line after the question. `ui/signal` is `Signal<T>` —
`get`, `set`, `update` — plus `signal` and `watch`; `ui/prop` is `Prop<T>` and
`memo`. `ui/testing` is a headless platform and a renderer to look at what a
tree became, importable only from a test source.

The whole of it rests on one idea: **a signal handle is inert data, and the
authority to read or write it travels through `ctx`** — the same split `Alloc`
and `Region` use. So a `Signal<T>` may be captured by an event handler, and the
handler takes its context as a parameter rather than closing over one.

| | Cost |
|---|---|
| `signal(ctx, v)` | O(1) |
| `get` | O(1) outside a computation. Inside one, O(k) in that computation's dependencies so far, because the edge is recorded once and recording it looks first |
| `set`, `update` | O(1) when the value is unchanged, and otherwise O(d) over what read the cell, transitively through memos |
| `memo(ctx, f)` | O(1) to declare — `f` does not run until something reads it, and then only after a cell it actually read has changed |
| `watch(ctx, f)` | runs once now, and once per batch in which something it read changed |

Tracking is automatic and exact: dependencies are collected afresh on every
run, so a read behind an `if` subscribes to the branch taken and not to the
other one. Writing a value identical to the one already there is not a change
and re-runs nothing.

### The tree

`ui/node` is what an interface *is*: `Node<C>`, eighteen `Role`s, and the
seventeen functions that build one. `ui/style` is how a container arranges and
paints what is inside it, and `mount` — the eighteenth — puts a tree on the
screen and leaves it there.

Two rules run through the vocabulary and are worth knowing before reading it.
**Meaning is the role and arrangement is the style**: `region(.List, ...)` says
what a group of children *is*, so a screen reader announces a list of five
items, while `.Layout(.Row)` says only how it is arranged and means nothing to
anybody but a display. No constructor is named after an HTML element and there
is no tag-string escape hatch. And **a parameter an assistive technology cannot
do without is a parameter**: `image` takes its `alt`, `link` its `dest`, and
`field` and `toggle` their `label`. A field with no label is not something this
vocabulary can express, which is what makes the commonest accessibility failure
on the web a compile error.

A component is an ordinary function and it runs **once**. Three constructors
put reactivity in the tree, and each re-runs the smallest thing it can:

| | What re-runs |
|---|---|
| a `Prop` on a leaf | one run of text, or one attribute. Nothing else in the tree is touched, and a `Prop.Const` registers nothing at all |
| `choose(cond, then, otherwise)` | one of two subtrees, when the condition changes. The subtree that goes is disposed, and the computations inside it go with it |
| `computed(build)` | the subtree `build` answers, when anything `build` read changes. The coarse instrument: reach for a `Prop` on a leaf when only a string is changing |
| `each(items, key, row)` | O(n) in the list, and **no row that is still there**: a row is keyed, so a reorder moves it and never rebuilds it. That is what keeps the focus, the scroll position and the computations inside a row alive |

`choose` was first written `when`, which it cannot be: `when` is a reserved
word, held for a language feature nobody has taken yet, so no function may be
called one.

Handlers — `button`'s `onPress` and `form`'s `onSubmit` — take their context as
a parameter, because a lambda may not capture one, and the runtime hands each
the very context the tree was mounted with. Everything one press writes is one
update: the handler runs inside a transaction, so three writes cause one pass
over the watchers rather than three. A field and a toggle have no change event
at all, because they are bound to a `Signal` and what the reader typed is in it.

### Styling, and the two tiers a style can be in

`ui/style` is 45 properties and five ways of composing them. Every property is
one value applied to one element, none is named after a CSS declaration, and
there is no `margin`: `Gap`, stacks and `AlignCross` replace it, and edges are
logical (`.Start`, `.End`) rather than left and right, so a right-to-left page
is right by construction.

The part worth understanding is where a style *goes*.

**Static — everything except `Computed`.** The compiler evaluates it, turns each
distinct property value into one atomic class, and writes the classes into a
stylesheet that ships with the artifact. `.Padding(.Px(8))` is `.p-8` wherever it
was written, in whichever module, so two packages that ask for the same padding
get one class and one rule without having seen each other. Nothing is generated
at run time, ever.

Two constructors exist only in this tier, because neither has an inline form:

- `On(State, [Style])` is a pseudo-class — hover, focus, pressed, disabled,
  checked. **This is why hover is not an event.** A pseudo-class costs nothing,
  needs no signal write on a mouse move, survives into an email's `<style>`
  block, and maps to a native pressed or focused trait.
- `At(Screen, [Style])` is a breakpoint, from one of four widths upwards.
  Mobile-first: the media queries are written in ascending order, so a larger
  tier overrides a smaller one by position, and there is never a maximum-width
  query. What is outside every `At` is the smallest screen's.

`When(cond, then, otherwise)` is static on both sides: both branches go in the
stylesheet, and what the runtime does when `cond` changes is pick one of two
precomputed class strings.

**Computed — `Computed(fn(Scope) => [Style])`.** For a value a signal drives: a
drag, a cursor-follow, an animation. Applied inline to the element and
re-serialised on every change, and deliberately absent from the stylesheet.
Reach for it last.

A style the compiler cannot evaluate — one built out of a function's parameters,
or out of a value it had to read — is **not an error**. It degrades to the same
inline application `Computed` gets. The exception is `On` and `At`, which have
nowhere to degrade to, so a style under one of them is statically known or the
program is rejected ([`style-not-static`](../errors/style-not-static.md)).

**Conflicts resolve per property, last wins.** When both sides are literals the
compiler resolves them and the element carries one class rather than two that
fight. When a style *arrives as a parameter* — the overridable-component case —
the runtime resolves it by a scan over `(slot, class)` pairs the compiler
assigned, which can only ever choose between classes that are already in the
sheet. Between two *different* properties that touch the same declaration —
`Padding` and `PaddingX` — the order the variants are declared in decides,
because that is the order the sheet is written in, and the narrower property is
always declared later.

Constant folding is what makes design tokens work. `.Background(Token.Surface.color())`
is a *call*, not a literal, and it still reaches the stylesheet: the extractor
inlines any function that is pure by its signature — no `ctx`, no effect-carrying
`self`, no allocator — which is a question about a signature and not about a
body.

### Design tokens, and why exhaustiveness is the whole contract

A design token is a name whose value the app decides. Every package that uses
tokens — a library or an app, the rules are the same — declares its own closed
vocabulary as an ordinary enum, with a constructor answering a colour:

```buri
from "core/effect" import { Alloc };
from "core/host" import * as host;
from "ui/effect" import { Scope, Ui, Watch };
from "ui/node" import * as ui;
from "ui/style" import * as style;
from "ui/style" import { Color };
from "ui/theme" import * as theme;
from "ui/theme" import { Theme };

// `cardlib`'s vocabulary, and the constructor that names each of its tokens.
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

// `cardlib`'s half of the loop: the one function only it can write, because
// only it knows what its tokens are.
export fn themed(f: fn(Token) => Color): Theme {
    theme.themed([
        (Token.Surface.color(), f(.Surface)),
        (Token.OnSurface.color(), f(.OnSurface)),
        (Token.Danger.color(), f(.Danger)),
    ])
}

// The consumer's half. This `match` is the compatibility check: a colour
// written out, or another package's token, which is a chain.
fn cardTheme(t: Token): Color {
    match (t) {
        .Surface => .Rgb(240, 240, 245),
        .OnSurface => .Rgb(24, 24, 27),
        .Danger => .Rgb(220, 38, 38),
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Ui: host.ui,
        Watch: host.watch,
    };
    let card = ui.stack([.Background(Token.Surface.color())], []);
    ui.mount(ctx, card, [themed(cardTheme)])
}
```

`style.token` answers a `Color.Token`, which holds an opaque reference and
nothing else. So a library's styles name only the library's own vocabulary,
`Style` never learns about any package's token type, and a definition site is
type-safe: `.Background(Token.Surface.color())` cannot name a token that does
not exist.

The app closes the loop at mount, with **one theme per package it uses** — the
package's `themed` applied to the app's mapping, all of them in the list
`mount` takes.

**Exhaustiveness is the compatibility contract.** The day `cardlib` adds a
token, that `match` stops covering its type and every consumer fails to compile
until it says what the new token is worth
([`match-not-exhaustive`](../errors/match-not-exhaustive.md)). No
registry, no schema language, no default — a token nobody mapped would be a
variable the page never defines, and a silently unpainted element is what this
refuses.

Chains resolve at mount, in one step: a library's token to the app's token to a
colour is followed until it reaches a value, and what the page reads is the
value.

**On the web, a token is a namespaced custom property.** A class in the
stylesheet reads `var(--cardlib-surface)`, where the namespace is the package,
so a library's tokens and an app's can never collide. The class is therefore
decided at compile time and does not depend on what the token turns out to be
worth; a theme is a `:root` block of values, written once at mount.

That is what makes dark mode free. `theme.switching(condition, whenTrue,
whenFalse)` takes a `Prop<Bool>` — a signal the app writes, a stored preference,
a media query bridged into one — and when it changes, the block of values is
written again. No class changes, no element is touched, and the stylesheet is
not involved at all.
