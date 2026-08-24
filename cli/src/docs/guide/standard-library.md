## The standard library

The standard library ships with the toolchain. It is never listed in a
`dependencies`, it is available to every target, and it cannot be replaced —
there is one, and this is it. It owns two reserved module roots: `core/*`, the
deliberately small set of essentials, and `ui/*`, the reactivity vocabulary,
which is a different kind of thing and a much larger surface.

**The reference for a module is the module.** `buri docs core/list` renders it
from the source the compiler checked, so a signature on the page is the
signature that exists, and `buri docs core/list.map` renders one item of it.
`buri docs` lists every module. This page is the map over the top of that:
which modules there are, what each one costs, and what is deliberately absent.

### The purity tiers

Every function sits in one of three tiers, and the tier is visible in the
signature rather than in a comment. This is
[`SPEC.md` §10.5](./cli/src/docs/SPEC.md), applied:

| Tier | Shape | Example |
|---|---|---|
| **Pure** | no `ctx` parameter | `xs.len()`, `date.weekday(d)`, `v.dot(o)` |
| **Deterministic** | `ctx` bounded by `Alloc` only | `xs.map(ctx, f)`, `json.stringify(ctx, v)` |
| **Effectful** | `ctx` bounded by anything else | `fs.readText(ctx, p)`, `time.now(ctx)` |

The rule that decides it: an operation whose result size is fixed is pure, and
one whose result size depends on runtime data names `Alloc`. `len` and `fold`
are pure; `map` and `filter` are not. A `F32x4` is four numbers in a struct, so
every operation in `core/simd` is pure — the vector types are exactly the shape
the rule was drawn around.

### Values and control

`core/option`, `core/result`, `core/order`, `core/num`, `core/bool`,
`core/math`, `core/bits`.

`Option`, `Result`, `Order` and the comparison and operator traits are in the
prelude, so `derive Eq for Point;` works in a module that imports nothing.

### Text

`core/str`, `core/char`, `core/bytes`, `core/json`, `core/proto`.

- **`core/bytes`** — UTF-8, hex, base64, varints. Free functions rather than
  methods on `[U8]`: a method may only be declared in its type's defining
  module, and `[T]`'s is `core/list`. Decoding is strict, and validates before
  it allocates: an overlong UTF-8 encoding, a truncated sequence, or a
  surrogate is an error at a named index, not a replacement character.

  The varints live here rather than in `core/proto`, beside hex and base64,
  because a varint is an encoding of a number as bytes, it has exactly one
  definition, and anything speaking a length-prefixed format needs the same
  one. They do 64-bit arithmetic on two 32-bit halves, so a negative `int64`
  writes the ten bytes protoc writes; an `Int` is still a double, so a value
  past 2^53 survives only to that precision.

- **`core/json`** — a `Json` tree, `parse`, and `stringify`. **An object is an
  ordered association list, not a map**, so key order round-trips, no `Hash`
  bound is needed, and `get` is O(n). Every number is a `Float`, which is what
  JSON says a number is. Nesting is capped at `MAX_DEPTH`, because parsing
  recurses and the recursion is not in tail position — without the cap a deep
  enough document is a crash rather than an error.

  **`derive ToJson` and `derive FromJson` map it to your own types**, and
  `encode` and `decode` are the two functions that use them. Both are on
  `derive`'s fixed list — no reflection and no macro is involved. The mapping
  from Buri shapes onto JSON ones is stated in the module's own source; the
  decisions with something at stake in them are that an enum is externally
  tagged, that a positional struct is an array whatever its arity, and that
  `Option<T>` is `T` or `null` — so `Option<Option<T>>` does not round-trip.

  Both traits are **derived and never written by hand**, which the compiler
  enforces. A derived encoder stands for the type's shape, so a hand-written
  one would be called where the type is encoded on its own and silently skipped
  where a type holding it is.

- **`core/proto`** — the protobuf wire format: tags, wire types, the packed
  readers, and `ProtoError`. Nothing here is written by hand either, but for a
  different reason: a `.proto` schema in a package *becomes* a module, and this
  is the part of that generated code which is the same for every schema. See
  [the proto reference](./cli/src/docs/build/proto.md) for the mapping and for
  why those codecs are generated Buri rather than a descriptor walk.

  Reading a request and writing a reply over a pipe is what `Stdin.readBytes`
  and `Stdout.writeBytes` are for: `readLine` reads the stream to its end, so a
  program using it cannot answer before the other side has finished speaking.
  Text and octets are two questions about one stream, so they are two
  operations, and a program should ask only one of them.

### Collections

`core/list`, `core/queue`, `core/map`, `core/set`, `core/bitset`.

Every one of these is a value, so every "modification" returns a new one. That
is not free, and the cost is stated per module rather than implied:

| | Lookup | Insert | Note |
|---|---|---|---|
| `core/queue` | O(1) | O(1) amortized | Banker's deque: two lists, the front reversed. The reversal is what makes both ends an append. |
| `core/map`, `core/set` | O(1) expected | O(b) in buckets | Buckets of association lists. Grows and rehashes past a load factor of 4. **Iteration order is unspecified and will change.** |
| `core/bitset` | O(1) | O(n/32) | 32 bits to an `Int` word — 32 and not 64 because `Int` is signed, and a bit in position 63 would make every shift a question about sign extension. |

`Queue`, `Map`, `Set` and `BitSet` provide `equals` rather than deriving `Eq`,
because a derived `Eq` would compare the *representation*: two queues holding
the same elements need not have the same front/back split, and two maps built
in different orders need not have the same bucket layout.

### Numbers and vectors

`core/simd` — `F32x4` and `I32x4`.

**On the JavaScript backend these are scalar and buy no speed.** There is no
SIMD reachable from a plain `.mjs` artifact. What they buy is the shape: a
kernel written lane-wise, with no loop-carried dependency, is the form a
backend with vector registers can lower directly, and the same kernel written
as a fold over a list is not, because a fold says "in this order". Do not
benchmark against a scalar loop expecting a win; there is not one today.

### Time

`core/time` is the clock, and reading it is an effect. `core/date` is the
calendar, and none of it is: what day of the week a date falls on does not
depend on anything.

`core/date` uses Hinnant's `days_from_civil`, which is exact over the whole
range of `Int` using integer arithmetic only. `Duration` is a length and
`Instant` is a point, and they are different types on purpose.

**There is no timezone database, and there will not be one.** tzdata is
megabytes that change several times a year, and this toolchain has no
dependencies and ships no data files. `Zoned` carries a fixed offset in
minutes, which covers UTC, a stored offset, and arithmetic within one offset.
It does not cover `America/New_York`, and it does not pretend to.

### Cryptography

`core/crypto` — SHA-256, HMAC-SHA-256, and a constant-time comparison. Written
in Buri rather than handed to the platform, because a dependency tree is a
second thing to audit. It is checked against the NIST vectors, as is the
independent SHA-256 the build cache uses, in two languages neither of which can
compile the other.

Deliberately absent, and not by oversight:

- **No ciphers.** Shipping a block function without a key schedule, a mode, a
  nonce discipline, and an authentication tag is how people end up with ECB.
- **No public-key anything.**
- **No cryptographically secure randomness.** `core/random` is seeded and
  reproducible on purpose — a hermetic test needs the same numbers every run.
  A CSPRNG is a different thing, and it would need a new `Entropy` effect in
  `core/effect` with a host implementation behind it. That is a decision to take
  deliberately, not a function to add quietly beside these.

`sha256` is **not a password hash**. It is fast, which is the wrong property.

### The platform

`core/effect` declares the effects; `core/host` implements them and may be
imported only by the module that exports `main`. `core/io`, `core/fs`,
`core/env`, `core/random`, `core/net/http` are the interfaces those effects
are used through. `core/testing/assert` and `core/testing/context` are
importable only from a test source.

### User interfaces

`ui/effect` declares `Watch`, `Ui` and `Fetch`, the `Scope` a reactive closure
is handed, and the `Event` a handler is handed. `ui/signal` is `Signal<T>` —
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

#### The tree

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

#### Styling, and the two tiers a style can be in

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
program is rejected ([`style-not-static`](./cli/src/docs/errors/style-not-static.md)).

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

#### Design tokens, and why exhaustiveness is the whole contract

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
  export Surface,
  export OnSurface,
  export Danger,
}

impl Token {
  export fn color(self: Token): Color {
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
  let ctx = context { Alloc: host.alloc, Ui: host.ui, Watch: host.watch };
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
([`match-not-exhaustive`](./cli/src/docs/errors/match-not-exhaustive.md)). No
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

### Allocators

`core/alloc` — `GeneralPurpose`, `Arena`, `FixedBuffer`. Three implementations
of `Alloc`, importable anywhere, because `Alloc` is the one effect whose
implementation carries no authority: a `Region` is a number, so a library that
builds its own allocator has been granted nothing.

- **`GeneralPurpose`** — unbounded, counts. `gp.stats()` answers
  `Stats { allocations, bytes }`.
- **`FixedBuffer(n)`** — a byte budget, and charging past it **aborts**. That
  is forced rather than chosen: `allocate` answers `Region` and not
  `Result<Region, _>`, so there is no value to report a failure with, and
  [`SPEC.md` §6.10](./cli/src/docs/SPEC.md) says that is what an abort is for.
  The message carries both numbers.
- **`Arena`** — a separate counter, and nothing more than a counter. It does
  not free in bulk, and it says so.

What an allocator is told about is narrower than what the cost model defines,
identically on both backends: **every `allocate(ctx, n)`, and nothing else.**
The charge for an operation is *defined* rather than measured — a `Str` of *n*
UTF-8 bytes charges `16 + n`, a `[T]` of *n* charges `16 + n * stride(T)`, a
view charges nothing — and the list and string rows are charged by definition
and reported to no allocator. The model is written down beside `Alloc` in
`core/effect`, where a reader of the effect meets it.

### What is deliberately not here

- **Struct-of-arrays / `MultiArrayList`.** Not typeable today: exposing "column
  *i* of `T`, at `T`'s *i*-th field type" needs dependent or row types, and
  [`SPEC.md` §5.5](./cli/src/docs/SPEC.md) has no records. Write the two-field
  struct yourself; on the JavaScript backend that is all a library would do.
- **Bulk reclamation — a real `Arena`.** The type is here and its counter is
  real; the bulk free is not. It needs a language feature that bounds a
  context's lifetime, which does not exist yet.
- **Automatic accounting of the list and string rows.** Stated above: the cost
  model defines them and no allocator is told about them.

Why each of those is where it is, and what would have to change, is in
[`design/STANDARD-LIBRARY.md`](./design/STANDARD-LIBRARY.md) — that is a
contributor's document, not a user's.
