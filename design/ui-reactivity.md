# UI reactivity and styling

**This has shipped.** What a user needs is written where it can be checked: the
`ui/*` modules' own documentation (`buri docs ui/node` and its neighbours), the
guide's "User interfaces" section, and — for the `WEB` output, its three files
and the `host-not-granted` diagnostic — `cli/src/docs/build/`. Per
[`design/README.md`](./README.md), a design document that has shipped must not
become a second copy of the reference, so what stays here is the **argument**:
why the shape is this shape, what was considered and refused, and, in "As
built" below, every place the argument was overruled by what compiling it
taught. The code fragments below are illustrative of the reasoning, not the
signatures of record.

Signal-based, fine-grained reactivity for Buri. No virtual DOM, no top-level
model, no JSX. A component is an ordinary function that runs **once**; only
reactive leaves re-run. The tree vocabulary is platform-neutral: web is one
backend, native and HTML email are others.

The design rests on one idea: **a signal handle is inert data, and the
authority to read or write it travels through `ctx`.** That is the same split
`Alloc` and `Region` already use — the handle is the reference, the context is
the capability.

## Effects

```buri
// ui/effect — a platform module (only platform modules may declare effects)

/// Reading signals, alone. Separate from `Ui` so a computation can be handed
/// read authority and provably nothing else.
export effect Watch {
  fn read<T>(self: Self, id: Int): T;
}

export effect Ui {
  fn signal<T>(self: Self, initial: T): Int;
  fn read<T>(self: Self, id: Int): T;
  fn write<T>(self: Self, id: Int, value: T): ();
  fn memo<T>(self: Self, compute: fn(Scope) => T): Int;
  fn watch(self: Self, run: fn(Scope) => ()): ();
}

/// Starting requests. A separate effect because the *shape* is different, not
/// because the authority is: `Net.fetch` blocks until the response arrives,
/// which on a platform with an interface means freezing it. A platform grants
/// one or the other, never both.
export effect Fetch {
  fn fetch(
    self: Self,
    request: Request,
    done: fn(Self, Result<NetResponse, FetchError>) => (),
  ): ();
}

/// The one concrete implementor of `Watch`, minted only by the runtime when it
/// evaluates a reactive closure. Concrete so that closure types can name it —
/// which is what keeps `Prop` and `Style` free of type parameters. It
/// implements an effect, so it is effect-carrying and may only arrive as
/// `ctx`: a signature taking a `Scope` is visibly effectful.
export struct Scope(Int);               // private field: unforgeable
```

Two things about `Ui` that reading the design alone would not predict, both
forced by trying to compile it. It declares `read` as well as `Watch` does,
because `Signal.update<C: Ui>` — this document's own signature — reads the old
value on its way to writing a new one, and a `Ui` with no read would make it
unimplementable; an implementor of `Ui` therefore grants strictly more than one
of `Watch`. And the registration method is `watch`, not `effect`, because
`effect` is a reserved word and no function may be called one.

`Fetch`'s callback takes `Self` rather than a bare context type, and that is
what makes a test double possible: a free `fetch<C: Fetch>` intrinsic would
have one implementation for every `C`, so a headless `NoFetch` would still
reach the network. Declaring the callback as `fn(Self, …) => ()` puts the
receiver in the runtime's hands, so the call site is still
`ui.fetch(ctx, request, done)` and the double is ordinary Buri.

## Reactivity types

One convention runs through everything: **wherever a value can vary, there is a
`Computed` variant taking `fn(Scope) => X`.** The runtime supplies the `Scope`
at evaluation time; the closure can never capture one.

```buri
/// An index into the runtime graph. Holds no authority, so a lambda may
/// capture it. That is what makes event handlers expressible.
export struct Signal<T>(Int);           // ui/signal

/// A time-varying value. A component cannot tell which variant it was given —
/// props are uniform over reactivity, distinct over writability (a component
/// that can write takes the Signal itself, or a callback).
export enum Prop<T> {                   // ui/prop
  Const(T),                 // never changes; the runtime registers nothing
  Cell(Signal<T>),          // backed by a writable cell
  Computed(fn(Scope) => T), // a derivation; buildable in a pure component
}
```

```buri
Signal.get<C: Watch>(self, ctx: C): T
Signal.set<C: Ui>(self, ctx: C, value: T): ()
Signal.update<C: Ui>(self, ctx: C, f: fn(T) => T): ()
Prop.read(self, ctx: Scope): T

signal<C: Ui, T>(ctx: C, initial: T): Signal<T>          // ui/signal
watch<C: Ui>(ctx: C, run: fn(Scope) => ()): ()           // ui/signal
memo<C: Ui, T>(ctx: C, compute: fn(Scope) => T): Prop<T> // ui/prop
each<C, T: Eq>(items: Prop<[T]>, key: fn(T) => Str,      // ui/node
               row: fn(C, T, Int) => Node<C>): Node<C>
mount<C: Ui>(ctx: C, root: Node<C>, themes: [Theme]): Result<(), Str>
```

`memo` is in `ui/prop` and not beside `signal`, because it answers a `Prop<T>`
and `Prop.Cell` holds a `Signal<T>`: one of the two modules has to import the
other, and a cycle is an error. It goes with the type it returns.

`each` takes no context. A list is a description, and describing one is as pure
as describing anything else here — the version in this document's first draft
threaded a `ctx` it had no use for. The `key` is a parameter for the reason
`alt` is one: keying a list by position silently corrupts it the moment the
list is reordered, and nothing can notice.

## The tree

HTML conflates layout and meaning; this vocabulary splits them. Meaning comes
from the accessibility taxonomy (ARIA landmarks), which is already the
cross-platform one — web lowers roles to semantic elements, native backends
lower them to accessibility traits. **No constructor is named after an HTML
element**, and there is no tag-string escape hatch: anything reachable only by
tag name becomes a role or a widget.

```buri
export enum Role {
  Navigation, Main, Banner, ContentInfo, Complementary, Article, Search,
  List, ListItem, Group, Separator,          // structure a reader navigates by
  Status, Alert,                             // live regions: polite, assertive
  Table, TableRow, RowHeader, ColumnHeader, Cell,   // data tables ARE semantics
}

// containers — arrangement is style, meaning is the role
ui.stack(styles: [Style], children: [Node<C>]): Node<C>   // no semantics
ui.region(role: Role, styles: [Style], children: [Node<C>]): Node<C>
ui.row(styles, children) / ui.column(styles, children)    // stack sugar
ui.spacer(): Node<C>                                      // sugar: grown empty stack
ui.nothing(): Node<C>                                     // occupies nothing, emits nothing

// text
ui.text(content: Prop<Str>): Node<C>
ui.heading(level: Int, content: Prop<Str>): Node<C>

// widgets — interactive behaviour, not roles. Accessibility-critical
// parameters (alt, dest, label) are required, not attributes.
ui.button(label: Prop<Str>, onPress: fn(C, Event) => ()): Node<C>
ui.link(dest: Prop<Str>, children: [Node<C>]): Node<C>
ui.image(source: Prop<Str>, alt: Prop<Str>): Node<C>
ui.field(label: Prop<Str>, kind: FieldKind, value: Signal<Str>): Node<C>
ui.toggle(label: Prop<Str>, value: Signal<Bool>): Node<C>
ui.form(onSubmit: fn(C, Event) => (), styles, children): Node<C>

// reactivity in the tree
ui.computed(build: fn(Scope) => Node<C>): Node<C>
ui.choose(cond: Prop<Bool>, then: Node<C>, otherwise: Node<C>): Node<C>
ui.each(items, key, row): Node<C>
```

Role→element on web: `Navigation → nav`, `Main → main`, `Banner → header`,
`ContentInfo → footer`, `Complementary → aside`, `List → ul`, `ListItem → li`,
`Table → table/tr/th/td`, plain `stack → div`, with `role=` attributes as the
fallback.

**Grid is layout; table is semantics.** A data table expresses cell↔header
relationships (accessibility), so it is roles; visual arrangement is
`.Layout(.Grid)`. Each used for the other is a named antipattern. There is
deliberately no `Grid` *role*, so the confusion has nothing to grab.

**`field` takes its label**, which is a correction this document owes its own
rule. An unlabelled input is the commonest accessibility failure there is and,
unlike a missing `alt`, it has no visual fallback — so it is not expressible. A
field and a toggle have no change event either: both are bound to a `Signal`,
so what the reader typed is in the signal already and two-way binding replaces
the event entirely.

**`form` is a widget and not a role**, because submission is behaviour: pressing
Enter in a field inside one runs `onSubmit`, which is the browser's own
dispatch rather than a key handler every app would otherwise write.

`Node<C>` keeps its one type parameter because handlers are open-ended: a
press may legitimately need `Net`, and which effects a program permits is the
budget `main` chose. Everything else (`Prop`, `Style`, `Signal`) names no
context type and is plain, capturable data.

Three constructors put reactivity *in* the tree, and each re-runs the smallest
thing it can: `choose` rebuilds one of two subtrees when its condition changes,
`computed` rebuilds its subtree when anything it read changes, and `each`
reconciles by key, so a row that is still there is moved rather than rebuilt.
Everything else runs once. A `Prop` on a leaf is the fine instrument and
`computed` is the coarse one; reach for the `Prop` first.

## Styling

A `Style` is a property, a group, a condition, or a computation:

```buri
export enum Style {
  // 45 properties. The arithmetic, because the cut line is the design:
  //   11  arrangement, and a child's part in it: Layout, AlignMain, AlignCross,
  //       AlignSelf, Wrap, Scroll, Grow, Shrink, Span, Pin, Position
  //    7  space:      Gap{,X,Y}, Padding{,X,Y}, PaddingEdge
  //    7  extent:     {Min,Max,}Width, {Min,Max,}Height, AspectRatio
  //    8  paint:      Background, Foreground, Border{Width,Color,Style},
  //       Radius, Opacity, Shadow
  //   11  type:       FontFamily, FontSize, FontWeight, Italic, LineHeight,
  //       LetterSpacing, TextAlign, TextCase, TextLine, TextWrap, Truncate
  //    1  Cursor
  Layout(Layout),                       // on the container
  AlignMain(Align), AlignCross(Align),  // main/cross axis: survives direction flips
  Grow(Int), Shrink(Int), Span(Int),    // on a child
  Pin(Edge, Length), Position(Position),
  PaddingX(Length), Gap(Length), Width(Length), Radius(Length),
  Background(Color), Foreground(Color), Truncate(Int), ...,

  // and six combinators
  Group([Style]),                       // composition; array literal, no Alloc
  On(State, [Style]),                   // pseudo-class; static, in the stylesheet
  At(Screen, [Style]),                  // breakpoint;   static, in the stylesheet
  When(Prop<Bool>, [Style], [Style]),   // both branches statically extracted
  Computed(fn(Scope) => [Style]),       // never in the stylesheet
  Extracted([(Int, Str)]),              // compiler-produced; a program cannot write one
}

export enum Layout {
  Column, Row,              // stacks; Column is the default
  Grid([Track]),            // explicit tracks; Track = Fraction(Int) | Fixed(Length) | Auto
  Layers,                   // children share one space (ZStack), in written order
}

export enum State { Hover, Focus, Active, Disabled, Checked }
export enum Screen { Small, Medium, Large, ExtraLarge }
                          // closed names, so libraries compose; the widths are
                          // 40 / 48 / 64 / 80 rem, which follow the reader's
                          // text size rather than the device's pixels

export enum Length { Px(Int), Rem(Float), Percent(Float), Auto, Full }
export enum Color  { Rgb(Int, Int, Int), Rgba(Int, Int, Int, Float),
                     Token(TokenReference), Transparent, Inherit }
```

Deliberately absent: floats, margin collapsing, inline-block — stacks, `Gap`,
and `Layers` replace them, and none survive cross-platform. There are no
margins at all: space between things belongs to the container that arranged
them.

**Hover is a style, not an event**, and `On` is why. A pseudo-class costs
nothing at run time, needs no listener, survives into an email's `<style>`
block, and maps to a native pressed or focused trait — where a signal written
on every mouse move does none of the four and costs a render each time.

Two tiers, on purpose:

- **Static** (everything except `Computed`): extracted at compile time into one
  atomic utility class per distinct property value, deduped across the whole
  build. `When` emits both branches as ordinary classes and the runtime picks a
  precomputed class string — nothing is generated at runtime, ever. `At` emits
  media-query-scoped variant classes, mobile-first, larger tiers overriding
  smaller — so breakpoints work in email `<style>` blocks and cost zero runtime;
  native backends re-resolve `At` on window size-class change.
- **`Computed`**: for values driven by signals (drag, cursor-follow,
  animation). Applied per-element by the runtime (inline styles on web);
  deliberately absent from the stylesheet. Each one re-serializes on change, so
  the doc default is the static tier first, `Computed` for genuinely dynamic
  values.

A style the compiler cannot evaluate — one built out of a function's parameters,
say — is **not an error**: it degrades to the same inline application `Computed`
gets, which is what lets folding improve later without a correctness window.
`On` and `At` are the two exceptions, because neither has an inline form to
degrade *to*: there is no `:hover` and no media query you can write into an
element's `style` attribute. Anything under one of them is statically known or
the program is rejected.

Conflict resolution is per property, last wins, whether the compiler resolved it
or a runtime scan did — the scan only ever *chooses between* classes the
compiler already emitted. A style that arrives as a *parameter* (the
overridable-component case) resolves at runtime by a linear scan over
compiler-assigned `(slot, class)` pairs, where a slot is the property **and its
condition**: `Padding` and `On(.Hover, [Padding])` are different slots, and
"per property" stopped being enough the moment `On` existed. Between two
*different* properties that touch the same underlying declaration — `Padding`
and `PaddingX`, `BorderWidth` and `BorderStyle` — the declaration order of the
variants decides, because the sheet is written in that order and
equal-specificity rules resolve by position. **The variant order is part of the
vocabulary's contract**, not an implementation detail, and the narrower property
is always declared after the broader one.

## Design tokens

Every package that uses tokens — libraries and apps alike — declares its own
closed vocabulary as an ordinary enum, with a constructor producing an opaque
reference:

```buri
// cardlib/tokens
export enum Token { Surface, OnSurface, Primary, Danger }
impl Token {
  export fn color(self: Token): Color {
    match (self) { .Surface => style.token("cardlib", "surface"), ... }
  }
}

/// The one function only cardlib can write, because only cardlib knows what
/// its tokens are. `ui/theme`'s `themed` takes the bindings; this one fills
/// them in, one per variant.
export fn themed(f: fn(Token) => Color): Theme {
  theme.themed([(Token.Surface.color(), f(.Surface)), ...])
}
```

`token(namespace, name)` is the constructor, and `TokenReference` is opaque
with private fields, so `Style` never learns about any package's token type: a
library's styles name only the library's own vocabulary and a reference is all
that crosses the boundary. A consumer closes the loop at mount with one theme
function per library it uses, mapping that library's tokens to its own tokens
or to raw values:

```buri
fn cardTheme(t: cardlib.Token): Color {
  match (t) {
    .Surface   => app.Shade.Bg.color(),
    .OnSurface => app.Shade.Fg.color(),
    .Primary   => .Rgb(29, 78, 216),
    .Danger    => .Rgb(220, 38, 38),
  }
}

ui.mount(ctx, root, [cardlib.themed(cardTheme), app.themed(appTheme)])
```

**Exhaustiveness is the contract**: if a library adds a token, every consumer
fails to compile until its theme maps it. No registry, no schema language.
Chains (`library token → app token → value`) resolve at mount, once. A chain
that ends nowhere is not an error: the custom property is simply not written,
and the browser ignores a `var()` with nothing behind it.

On web, each token lowers to a namespaced custom property (`--cardlib-surface`)
and each theme installs one `:root` block of values, in the order the app passed
them. Theme switching is `theme.switching(condition, whenTrue, whenFalse)` over
whole themes, with the condition a `Prop<Bool>` — so dark mode is a signal, a
stored preference, or a media query bridged into one. The block is rewritten and
**every class on every element stays exactly as it was**: nothing is
re-extracted, no element is touched, and the browser repaints from variables it
already had. That is the whole reason dark mode is not a second stylesheet.

## Rules that make it typecheck

1. **Pure constructors leave `C` unbounded.** `fn region<C>(role: Role, styles:
   [Style], children: [Node<C>]): Node<C>`. With an effect bound on `C`,
   `[Node<C>]` is effect-carrying and rule 26 rejects the parameter. Unbounded,
   it is legal. Sound because `C` occurs only in argument position: extracting
   a `C` from a `Node<C>` would require already holding one.

   **The soundness argument was right and the predicate was not.** Reading it
   off the enclosing signature's bounds made `mount(ctx: C, root: Node<C>)`
   with `C: Ui` a rule-26 error — three of this document's own APIs were
   unbuildable. What landed is a variance-aware predicate: a least fixpoint
   `provides(con, i)` over every declared constructor, computed from its fields,
   dropping function *parameters* and keeping results. That generalises the
   rule the built-in `Ty::Fn` already had — only the result counts — to every
   user-declared type, and it is what makes "occurs only in argument position"
   a thing the compiler knows rather than a thing this document asserts.
2. **Nothing captures a context or a `Scope`.** Handlers and computed closures
   receive theirs as a parameter — the `mapCtx` shape §10.6 already mandates.
3. **Derivation is pure.** `.Computed(fn(c) => user.read(c).name)` needs no
   enclosing `ctx`, so a component that only reads props and builds a tree has
   no context parameter and is pure by §10.4.
4. **Tree and style construction need no `Alloc`.** Struct, enum, array and
   closure literals are fixed-size construction (§10.5).
5. **A captured generic must be bound where the type is actually stored.**
   `Prop<T>` holds its `T`, so capturing one needs `T: Eq` or another ordinary
   trait — an unbounded `T` answers `true` to `may_carry_effect`. `Signal<T>` is
   phantom in `T` and needs nothing, which is what makes an event handler that
   closes over a signal expressible without a bound the caller has to invent.
   `each` bounds its element type for the first reason, not the second.

## Runtime

Auto-tracking. The runtime holds a "currently executing computation" pointer;
`read` records a source → computation edge; `write` marks dependents dirty and
schedules them. Dependencies are re-collected per run, so conditional reads are
tracked exactly. `Prop.Const` is a visible constructor, so a static prop
registers nothing. Disposal is keyed on which computation was executing when a
signal was created.

## Compilation

- **Reactivity needs no compiler work** beyond the platform modules and their
  intrinsics (bodyless methods lowered to intrinsic keys, resolved by each
  backend's runtime — the existing `core/host` mechanism).
- **Styling is toolchain work by necessity**: no macros, no reflection, and no
  runtime generation means no library can see other modules' style literals.
  Each module's compile collects its static `Style` literals into a `Vec` on
  `Checked`; link merges and dedupes them into one stylesheet plus the
  `(slot, class)` table. Local compilation is preserved; only the link step is
  global, and it already was. (The first draft said "cached with the module, the
  same shape as test collection", which named machinery that does not exist —
  test *cases* are not cached, verdicts are, per suite.)
- Token constructors are calls, not literals, so extraction needs constant
  folding of pure calls in `const` initializers. **Const-folding is what
  landed**, rather than generating token modules the way the proto path
  generates types: an interpreter over the typed tree, with purity read off the
  function's own signature. It costs nothing at run time, and it improves an
  ordinary style helper as much as it improves a token — a generator would have
  helped tokens and nothing else.
- **A `WEB` build writes three files**, not one: the `.mjs`, the `.css` the
  extractor produced, and an `.html` shell that links the sheet and loads the
  module. The shell's `<link id="buri-styles">` is the id the runtime's own
  installer looks for, so the rules are in the page before the first paint and
  `mount` finds them there and installs nothing — no duplication, and no flash
  of unstyled content. A program with no static styles writes no `.css` and
  links none.

## Example

```buri
const badgeStyle: Style = .Group([
  .PaddingX(.Rem(0.5)), .Radius(.Px(6)), .Background(Token.Surface.color()),
  .On(.Hover, [.Background(Token.OnSurface.color())]),
]);

// Pure: no ctx. Cannot tell whether either prop varies.
fn badge<C>(title: Prop<Str>, count: Prop<Int>): Node<C> {
  ui.row([badgeStyle], [
    ui.text(title),
    ui.text(.Computed(fn(c) => "${count.read(c)}")),
  ])
}

fn counter<C: Ui>(ctx: C, label: Str): Node<C> {
  let count = signal(ctx, 0);

  ui.column([], [
    ui.button(.Const(label), fn(c, e) => count.update(c, fn(n) => n + 1)),
    badge(.Const(label), .Cell(count)),
    badge(.Const("doubled"), .Computed(fn(c) => count.get(c) * 2)),
  ])
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Ui: host.ui, Watch: host.watch };
  ui.mount(ctx, counter(ctx, "clicks"), [app.themed(appTheme)])
}
```

`main` returns `.Ok(())` and the page stays live: the JS entry wrapper only
exits on an `.Err`, and registered listeners keep running.

That program is in the corpus, compiled and its output recorded, at
`cli/tests/golden_javascript/ui_counter/`. A whole application — a keyed list, a
form, both style tiers, one library's tokens themed by an app, and a request
that answers through a callback — is `cli/tests/example/cmd/basket/`, which
builds as a `WEB` artifact and is tested with no browser.

## Targets

Which platform an app or library renders to is a build-system fact, not a
language one, and the existing machinery covers it:

- **Libraries declare nothing, by default.** A UI library is ordinary Buri over
  neutral types (`Node`, `Style`, `Role`), cannot construct a context, and
  cannot import `core/host` — platform-agnostic by construction, the way a pure
  library is effect-free by construction. A genuinely target-specific library
  uses the existing `platforms` field on its build rule.
- **Apps declare targets in `outputs`.** The closed `Platform` enum is
  `LINUX | MACOS | JS | WEB` today:

  ```textproto
  outputs: [
    { platform: WEB },
  ]
  ```

  `ANDROID` and `EMAIL` are the shapes it was widened for and neither is in the
  enum yet; what each would cost is a row in the grant table and a backend, not
  a change to anything above.

  A `WEB` output takes neither an `arch` — JavaScript has none — nor a
  `js { module }`, because a browser loads an ES module and there is no second
  kind; naming either is a build-file error rather than a field the toolchain
  then quietly ignores.
- **Enforcement is per output, and it is real.** `main` is the only module that
  can import `core/host`, and the output's platform subsets what `core/host`
  exports before the first pass that reads it — so the same rule that makes
  `Ui: host.ui` under `platform: LINUX` an unresolved name makes
  `Net: host.net` under `platform: WEB` one. The diagnostic is
  `host-not-granted`, and it names the platforms that *do* grant the effect. A
  platform *is* the set of effects its host exports; there is no second
  declaration.

  Two consequences worth writing down. **A grant is a pair** — the value and the
  implementation struct — and both are withheld together, because a host struct
  has no private field, so exporting `HostNet` while withholding `net` would
  leave the authority one `Net: host.HostNet {}` away. And **the same `main` may
  compile for one of a binary's outputs and not for another**, which is what
  "per output" costs and buys.

  WEB grants `Alloc`, `Stdout`, `Stderr`, `Clock`, `Rand`, `Ui`, `Watch` and
  `Fetch`, and withholds `Fs`, `Net`, `Stdin`, `Env` and `Proc`. `LINUX`,
  `MACOS` and `JS` grant the ten non-UI effects and none of the three UI ones.
  Telling the first three apart from each other is a table edit, not new
  machinery, and is deferred (see Open).
- **Email is a different effect grant, not a lesser web.** Its host exports
  rendering but nothing interactive — no `Ui`, no `Fetch`; a `render` evaluates
  the tree once (`Const` and `Computed` props resolve; `Cell` has nothing to
  back it). Component libraries written against `Prop` and `Style` work
  untouched; an app that binds interactive effects fails at `main`, and that is
  now a mechanism rather than an aspiration: an `EMAIL` row in the grant table
  granting neither is the whole of it.
- **Open**: a style or widget with no meaning on some target (hover in email).
  Start with backend degradation plus warnings; per-output vocabulary checking
  is more machinery than the problem deserves until real components hit it.

## Modules

UI gets its own reserved root, `ui/...`, so `core/` keeps meaning "the
deliberately small essentials." **This extends SPEC rule 35** (module paths were
`core/...` or `//...`) — a small change: the rule's wording, the path check in
`modules.rs`, and entries in the static `MODULES` table. The platform's
implementations still fold into the existing `core/host` (it is already
per-platform and main-only); UI-capable platforms export three more values from
it. When external repositories land, `ui/...` can migrate out wholesale.

| Module | Kind | Exports |
|---|---|---|
| `ui/effect` | platform | `effect Watch`, `effect Ui`, `effect Fetch`, `Scope`, `Event`, `Request`, `FetchError`, `fetch` |
| `core/host` (WEB, …) | platform | adds `ui`, `watch`, `fetch` — the implementations `main` binds |
| `ui/signal` | library | `Signal<T>` (`get`/`set`/`update`), `signal`, `watch` |
| `ui/prop` | library | `Prop<T>` (`read`), `memo` |
| `ui/node` | library | `Node<C>`, `Role`, `FieldKind`, `nothing`, `stack`, `region`, `row`, `column`, `spacer`, `text`, `heading`, `button`, `link`, `image`, `field`, `toggle`, `form`, `choose`, `computed`, `each`, `mount` |
| `ui/style` | library | `Style`, `Layout`, `Track`, `Screen`, `State`, `Position`, `Length`, `Color`, `Align`, `Axis`, `Edge`, `Weight`, `FontFamily`, `BorderStyle`, `TextCase`, `TextLine`, `TextWrap`, `Cursor`, `Shadow`, `TokenReference`, `token` |
| `ui/theme` | library | `Theme`, `themed`, `switching` |
| `ui/testing` | test platform | headless `Ui`/`Watch`/`Fetch`, render-to-document, event firing, the extracted stylesheet, installed theme values, and a recorder — test-only automatically via the `testing` path segment |

There is **no `ui` umbrella module**. It was in the first draft and was not
built: re-exporting from seven modules buys one import and costs a reader the
answer to "which module is this name from", and every file written against the
vocabulary since has wanted three or four specific imports rather than one broad
one. Adding it later is a re-export list and nothing else. Token modules are not
standard library either: each package declares its own.

Typical imports — a component module:

```buri
from "ui/node/lib.buri" import * as ui;
from "ui/node/lib.buri" import { Node };              // signature types
from "ui/prop/lib.buri" import { Prop };
from "ui/style/lib.buri" import { Style };
from "//lib/cardlib/lib.buri" import { Token };
```

`main`:

```buri
from "core/host/lib.buri" import * as host;
from "ui/effect/lib.buri" import { Fetch, Ui, Watch };
from "ui/node/lib.buri" import * as ui;
// context { Alloc: host.alloc, Ui: host.ui, Watch: host.watch, Fetch: host.fetch }
```

A component test:

```buri
from "core/testing/assert/lib.buri" import * as assert;
from "ui/testing/lib.buri" import { headless, observer, render };
```

Method calls (`count.get(c)`, `prop.read(c)`, `token.color()`) need no import —
resolution goes through the receiver's defining module.

## What ships where

| Piece | Where | Why |
|---|---|---|
| `ui/effect`, the `core/host` additions, `Scope` | compiler stdlib, platform modules | `effect` is legal only in a platform module, and platform-ness is a flag on the static `MODULES` table |
| The `Ui`/`Watch`/`Fetch` intrinsics | backend runtime | bodyless methods lower to intrinsic keys; the JS backend resolves them to `$host_*` functions |
| Style extraction + stylesheet link step | compiler | needs cross-module visibility no library has |
| `Signal`, `Prop`, `Node`, `Style`, `Role`, all constructors | `ui/*`, ordinary Buri | no compiler support needed; movable to a real library once external repos land |
| `ui/theme`'s `themed`, taking bindings | compiler stdlib | it is the type's constructor |
| Per-package `themed`, token enums, theme functions | each package / each app | ordinary Buri; exhaustive `match` is the compatibility check |

## As built

Where the argument above was overruled by compiling it. Each row is a deviation
from this document's first draft, with the reason; none of them changed what the
design is *for*.

| This document said | What shipped | Why |
|---|---|---|
| `ui/effects`, `ui/cap` | `ui/effect` | "cap" is an abbreviation and not the language's word; the module names one thing |
| `Ui.effect`, `ui.effect` | `watch`, on both | `effect` is a reserved word, so no function may be called one |
| `Ui` without `read` | `Ui` reads too | `Signal.update<C: Ui>` reads the old value; without it the design's own signature is unimplementable |
| `ui.when` | `ui.choose` | `when` is a reserved word, held for a language feature not yet taken |
| `memo` in `ui/signal` | `memo` in `ui/prop` | it returns a `Prop`, and a module may not import the module that imports it |
| `ui.each(ctx, items, row)` | `each(items, key, row)` | a list is a description, so it needs no context; and keying by position corrupts a reordered list silently |
| `ui.field(value)` | `field(label, kind, value)` | this document's own rule — an accessibility-critical parameter is a parameter — and an unlabelled input has no visual fallback |
| `Role::Form` | `ui.form`, a widget | submission is behaviour, not meaning |
| Rule 1 as an assertion | a variance-aware predicate | three of the APIs above did not compile without it; "occurs only in argument position" is now something the compiler computes |
| Rule 5 over `Signal<T>` | over `Prop<T>` | `Signal` is phantom in `T` and carries nothing; `Prop` stores its `T` |
| Style literals "cached with the module" | a `Vec` on `Checked` | the machinery it named does not exist: test cases are not cached, verdicts are |
| Const-folding *or* generated token modules | const-folding | it improves an ordinary style helper too, where a generator would have helped tokens alone |
| Blocking `Net.fetch` for pages | a separate `Fetch` effect | a callback shape, so a request does not freeze a page; a platform grants one or the other |
| "Enforcement already exists" | it does now | the main-only import rule existed; the per-output host subset did not, and was built — `host-not-granted` |
| Screen widths "app config at mount" | fixed at 40/48/64/80 rem | a breakpoint that varies per app is one a library cannot compose against |
| No way to express hover | `On(State, [Style])` | a pseudo-class costs nothing and survives to targets that have no pointer |
| A `ui` umbrella module | seven modules, no umbrella | an umbrella hides which module a name belongs to, and buys one import |
| `ui.viewport` | not built | structural responsiveness is still open, and the signal is the smallest part of it |

## Open

- **Host subsetting among the non-UI platforms.** The mechanism is a table, and
  `LINUX`, `MACOS` and `JS` all still grant the same ten effects — so
  `Fs: host.fs` under `platform: JS` compiles, which it should not. Telling them
  apart is a table edit and a reject case.
- **A per-target vocabulary check.** A style or widget with no meaning on some
  target — hover in email, a form in a static render. Backend degradation with a
  warning is the answer until real components hit it.
- **Structural responsiveness** (different children per size, not different
  styles) needs a `viewport` signal and `choose` — JS-ful targets only. Not a
  style concern, and not built.
- **Grid auto-flow vs. explicit `.Area` placement** — decide when a real
  photo-grid component needs it; `Track` + `Span` cover the common case.
- **The browser's own half of the tests.** `ui/testing` renders with the
  shipping renderer against a document the runtime supplies, so what is left
  unasserted is exactly what only a browser does: layout and painting, its
  dispatch of a press, focus and selection, and what assistive technology
  announces. That needs a real browser driven from the outside, which is a suite
  this repository does not have.
