# UI reactivity and styling

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
// ui/effects — a platform module (only platform modules may declare effects)

/// Reading signals, alone. Separate from `Ui` so a computation can be handed
/// read authority and provably nothing else.
export effect Watch {
  fn read<T>(self: Self, id: Int): T;
}

export effect Ui {
  fn signal<T>(self: Self, initial: T): Int;
  fn write<T>(self: Self, id: Int, value: T): ();
  fn memo<T>(self: Self, compute: fn(Scope) => T): Int;
  fn effect(self: Self, run: fn(Scope) => ()): ();
  // keyed reconciliation, node registration, mount
}

/// The one concrete implementor of `Watch`, minted only by the runtime when it
/// evaluates a reactive closure. Concrete so that closure types can name it —
/// which is what keeps `Prop` and `Style` free of type parameters. It
/// implements an effect, so it is effect-carrying and may only arrive as
/// `ctx`: a signature taking a `Scope` is visibly effectful.
export struct Scope(Int);               // private field: unforgeable
```

## Reactivity types

One convention runs through everything: **wherever a value can vary, there is a
`Computed` variant taking `fn(Scope) => X`.** The runtime supplies the `Scope`
at evaluation time; the closure can never capture one.

```buri
// ui — ordinary library code

/// An index into the runtime graph. Holds no authority, so a lambda may
/// capture it. That is what makes event handlers expressible.
export struct Signal<T>(Int);

/// A time-varying value. A component cannot tell which variant it was given —
/// props are uniform over reactivity, distinct over writability (a component
/// that can write takes the Signal itself, or a callback).
export enum Prop<T> {
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

ui.signal<C: Ui, T>(ctx: C, initial: T): Signal<T>
ui.memo<C: Ui, T>(ctx: C, f: fn(Scope) => T): Prop<T>
ui.effect<C: Ui>(ctx: C, f: fn(Scope) => ()): ()
ui.each<C: Ui, T>(ctx: C, items: Prop<[T]>, row: fn(C, T, Int) => Node<C>): Node<C>
ui.mount<C: Ui>(ctx: C, root: Node<C>, themes: [Theme]): Result<(), Str>
```

## The tree

HTML conflates layout and meaning; this vocabulary splits them. Meaning comes
from the accessibility taxonomy (ARIA landmarks), which is already the
cross-platform one — web lowers roles to semantic elements, native backends
lower them to accessibility traits. **No constructor is named after an HTML
element**, and there is no tag-string escape hatch: anything reachable only by
tag name becomes a role or a widget.

```buri
export enum Role {
  Navigation, Main, Banner, ContentInfo, Complementary, Article, Search, Form,
  Table, TableRow, Cell, ColumnHeader,       // data tables ARE semantics
}

// containers — arrangement is style, meaning is the role
ui.stack(styles: [Style], children: [Node<C>]): Node<C>   // no semantics
ui.region(role: Role, styles: [Style], children: [Node<C>]): Node<C>
ui.row(styles, children) / ui.column(styles, children)    // stack sugar
ui.spacer(): Node<C>                                      // sugar: grown empty stack

// text
ui.text(content: Prop<Str>): Node<C>
ui.heading(level: Int, content: Prop<Str>): Node<C>

// widgets — interactive behaviour, not roles. Accessibility-critical
// parameters (alt, dest) are required, not attributes.
ui.button(label: Prop<Str>, onPress: fn(C, Event) => ()): Node<C>
ui.link(dest: Prop<Str>, children: [Node<C>]): Node<C>
ui.image(source: Prop<Str>, alt: Prop<Str>): Node<C>
ui.field(value: Signal<Str>): Node<C>

// reactivity in the tree
Node.Computed(fn(Scope) => Node<C>)     // constructor: ui.computed
ui.when(cond: Prop<Bool>, then: Node<C>, otherwise: Node<C>): Node<C>
```

Role→element on web: `Navigation → nav`, `Main → main`, `Banner → header`,
`ContentInfo → footer`, `Complementary → aside`, `Table → table/tr/td/th`,
plain `stack → div`, with `role=` attributes as the fallback.

**Grid is layout; table is semantics.** A data table expresses cell↔header
relationships (accessibility), so it is roles; visual arrangement is
`.Layout(.Grid)`. Each used for the other is a named antipattern.

`Node<C>` keeps its one type parameter because handlers are open-ended: a
press may legitimately need `Net`, and which effects a program permits is the
budget `main` chose. Everything else (`Prop`, `Style`, `Signal`) names no
context type and is plain, capturable data.

## Styling

A `Style` is a property, a group, a condition, or a computation:

```buri
export enum Style {
  // one variant per property, grouped here for brevity
  PaddingX(Length), PaddingY(Length), Width(Length), Height(Length),
  Gap(Length), Radius(Length),
  Background(Color), Foreground(Color), BorderColor(Color), ...,

  // arrangement — containers own it; children own their part in it
  Layout(Layout),                       // on the container
  AlignMain(Align), AlignCross(Align),  // main/cross axis: survives direction flips
  Wrap(Bool), Scroll(Axis),
  Grow(Int), Span(Int),                 // on a child (flex-grow / grid span)
  Pin(Edge, Length),                    // on a child of .Layers; also .Sticky mode
                                        // and .PinViewport (CSS fixed)

  Group([Style]),                       // composition; array literal, no Alloc
  At(Screen, [Style]),                  // breakpoint; static, in the stylesheet
  When(Prop<Bool>, [Style], [Style]),   // both branches statically extracted
  Computed(fn(Scope) => [Style]),       // never in the stylesheet
}

export enum Layout {
  Column, Row,              // stacks; Column is the default
  Grid([Track]),            // explicit tracks; Track = Fraction(Int) | Fixed(Length) | Auto
  Layers,                   // children share one space (ZStack); sized by the
                            // first non-pinned child
}

export enum Screen { Small, Medium, Large, ExtraLarge }
                          // closed names, so libraries compose;
                          // min-width values are app config at mount

export enum Length { Px(Int), Rem(Float), Percent(Float), Auto, Full }
export enum Color  { Rgb(Int, Int, Int), Token(TokenReference), Transparent }
```

Deliberately absent: floats, margin collapsing, inline-block — stacks, `Gap`,
and `Layers` replace them, and none survive cross-platform.

Two tiers, on purpose:

- **Static** (everything except `Computed`): extracted at compile time into one
  atomic utility class per distinct property value, deduped across the whole
  build. `When` emits both branches as ordinary classes and the runtime picks a
  precomputed class string — nothing is generated at runtime, ever. `At` emits
  media-query-scoped variant classes (`medium-px-8`), mobile-first, larger tiers
  overriding smaller — so breakpoints work in email `<style>` blocks and cost
  zero runtime; native backends re-resolve `At` on window size-class change.
  `Computed` may not appear inside `At` (a closure cannot be scoped to a media
  query); `When` inside `At` is fine.
- **`Computed`**: for values driven by signals (drag, cursor-follow,
  animation). Applied per-element by the runtime (inline styles on web);
  deliberately absent from the stylesheet. Each one re-serializes on change, so
  the doc default is `styled` first, `Computed` for genuinely dynamic values.

Conflict resolution is per property, last wins. When both sides of a merge are
literals it resolves at compile time. A style that arrives as a *parameter*
(the overridable-component case) resolves at runtime by a linear scan over
compiler-assigned `(propId, classId)` pairs — still only choosing among
already-emitted classes.

## Design tokens

Every package that uses tokens — libraries and apps alike — declares its own
closed vocabulary as an ordinary enum, with a constructor producing an opaque
reference:

```buri
// cardlib/tokens
export enum Token { Surface, OnSurface, Primary, Danger }
impl Token { export fn color(self: Token): Color { ... } }   // -> .Token(TokenReference)
```

Library styles reference only the library's own enum, so definition sites are
type-safe and `Style` never learns about token types. A consumer closes the
loop at mount with one **theme function per library it uses**, mapping that
library's tokens to its own tokens or to raw values:

```buri
fn cardTheme(t: cardlib.Token): Color {
  match (t) {
    .Surface   => app.Token.Bg.color(),
    .OnSurface => app.Token.Fg.color(),
    .Primary   => .Rgb(29, 78, 216),
    .Danger    => .Rgb(220, 38, 38),
  }
}

ui.mount(ctx, root, [cardlib.themed(cardTheme), app.themed(appTheme)])
```

**Exhaustiveness is the contract**: if a library adds a token, every consumer
fails to compile until its theme maps it. No registry, no schema language.
Chains (`library token → app token → value`) resolve at mount.

On web, each token lowers to a namespaced custom property (`--cardlib-surface`)
and a theme is a `:root` block of values. Theme switching (dark mode) swaps
var values — drivable by a `Prop` — with the stylesheet untouched.

## Rules that make it typecheck

1. **Pure constructors leave `C` unbounded.** `fn row<C>(role: Role, styles:
   [Style], children: [Node<C>]): Node<C>`. With an effect bound on `C`,
   `[Node<C>]` is effect-carrying and rule 26 rejects the parameter. Unbounded,
   it is legal. Sound because `C` occurs only in argument position: extracting
   a `C` from a `Node<C>` would require already holding one.
2. **Nothing captures a context or a `Scope`.** Handlers and computed closures
   receive theirs as a parameter — the `mapCtx` shape §10.6 already mandates.
3. **Derivation is pure.** `.Computed(fn(c) => user.read(c).name)` needs no
   enclosing `ctx`, so a component that only reads props and builds a tree has
   no context parameter and is pure by §10.4.
4. **Tree and style construction need no `Alloc`.** Struct, enum, array and
   closure literals are fixed-size construction (§10.5).
5. **Generic components bound `T` with an ordinary trait** (e.g. `T: Eq`) if
   they capture a `Signal<T>` or `Prop<T>` in a handler; an unbounded `T`
   answers `true` to `may_carry_effect`.

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
  Each module's compile collects its static `Style` literals (cached with the
  module, the same shape as test collection); link merges and dedupes them into
  one stylesheet plus the `(propId, classId)` table. Local compilation is
  preserved; only the link step is global, and it already was.
- Token constructors are calls, not literals, so extraction needs constant
  folding of pure calls in `const` initializers — sound, since purity is read
  off the signature. The alternative is generating token modules the way the
  proto path generates types.

## Example

```buri
const badgeStyle: Style = .Group([
  .PaddingX(.Rem(0.5)), .Radius(.Px(6)), .Background(Token.Surface.color()),
]);

// Pure: no ctx. Cannot tell whether either prop varies.
fn badge<C>(title: Prop<Str>, count: Prop<Int>): Node<C> {
  ui.row([badgeStyle], [
    ui.text(title),
    ui.text(.Computed(fn(c) => "${count.read(c)}")),
  ])
}

fn counter<C: Ui>(ctx: C, label: Str): Node<C> {
  let count = ui.signal(ctx, 0);

  ui.column([], [
    ui.button(.Const(label), fn(c, e) => count.update(c, fn(n) => n + 1)),
    badge(.Const(label), .Cell(count)),
    badge(.Const("doubled"), .Computed(fn(c) => count.get(c) * 2)),
  ])
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Ui: host.ui };
  ui.mount(ctx, counter(ctx, "clicks"), [app.themed(appTheme)])
}
```

`main` returns `.Ok(())` and the page stays live: the JS entry wrapper only
exits on an `.Err`, and registered listeners keep running.

## Targets

Which platform an app or library renders to is a build-system fact, not a
language one, and the existing machinery covers it:

- **Libraries declare nothing, by default.** A UI library is ordinary Buri over
  neutral types (`Node`, `Style`, `Role`), cannot construct a context, and
  cannot import `core/host` — platform-agnostic by construction, the way a pure
  library is effect-free by construction. A genuinely target-specific library
  uses the existing `platforms` field on its build rule.
- **Apps declare targets in `outputs`**, extending the closed `Platform` enum
  (`LINUX | MACOS | JS`) with UI targets:

  ```textproto
  outputs: [
    { platform: WEB },
    { platform: ANDROID, arch: ARM64 },
    { platform: EMAIL },
  ]
  ```

- **Enforcement already exists.** `main` is the only module that can import
  `core/host`, and its context is checked per output — the same rule that makes
  `Fs: host.fs` under `platform: JS` an unresolved name makes `Ui: host.ui`
  under `platform: LINUX` one. A platform *is* the set of effects its host
  exports; there is no second declaration.
- **Email is a different effect grant, not a lesser web.** Its host exports
  rendering but nothing interactive — no `Ui.signal`/`write`; a `render`
  evaluates the tree once (`Const` and `Computed` props resolve; `Cell` has
  nothing to back it). Component libraries written against `Prop` and `Style`
  work untouched; an app that binds interactive effects fails at `main`.
- **Open**: a style or widget with no meaning on some target (hover in email).
  Start with backend degradation plus warnings; per-output vocabulary checking
  is more machinery than the problem deserves until real components hit it.

## Modules

UI gets its own reserved root, `ui/...`, so `core/` keeps meaning "the
deliberately small essentials." **This extends SPEC rule 35** (module paths are
`core/...` or `//...` today) — a small change: the rule's wording, the path
check in `modules.rs`, and entries in the static `MODULES` table. The
platform's implementations still fold into the existing `core/host` (it is
already per-platform and main-only); UI-capable platforms export two more
values from it. When external repositories land, `ui/...` can migrate out
wholesale.

| Module | Kind | Exports |
|---|---|---|
| `ui/effects` | platform | `effect Watch`, `effect Ui`, `Scope`, `Event` |
| `core/host` (WEB, ANDROID, …) | platform | adds `ui`, `watch` — the implementations `main` binds |
| `ui/signal` | library | `Signal<T>` (`get`/`set`/`update` methods), `signal`, `memo`, `effect` |
| `ui/prop` | library | `Prop<T>` (`read` method) |
| `ui/node` | library | `Node<C>`, `Role`, `stack`, `region`, `row`, `column`, `spacer`, `text`, `heading`, `button`, `link`, `image`, `field`, `when`, `computed`, `each`, `mount`, `viewport` |
| `ui/style` | library | `Style`, `Layout`, `Track`, `Screen`, `Length`, `Color`, `Align`, `Axis`, `Edge`, `TokenReference` |
| `ui/theme` | library | `Theme`, `themed` |
| `ui` | library | umbrella: re-exports the common names above, so one import serves most files |
| `ui/testing` | test platform | headless `Ui`/`Watch` implementations, render-to-tree, event firing — test-only automatically via the `testing` path segment |

Token modules are not standard library: each package declares its own.

Typical imports — a component module:

```buri
from "ui" import * as ui;
from "ui/effects" import { Ui, Watch };      // bounds
from "ui/node" import { Node };              // signature types
from "ui/prop" import { Prop };
from "ui/style" import { Style };
from "//lib/cardlib/tokens" import { Token };
```

`main`:

```buri
from "core/host" import * as host;
from "ui" import * as ui;
from "ui/effects" import { Ui, Watch };
// context { Alloc: host.alloc, Ui: host.ui, Watch: host.watch }
```

A component test:

```buri
from "core/testing/assert" import * as assert;
from "ui/testing" import { Headless };
```

Method calls (`count.get(c)`, `prop.read(c)`, `token.color()`) need no import —
resolution goes through the receiver's defining module.

## What ships where

| Piece | Where | Why |
|---|---|---|
| `ui/effects`, the `core/host` additions, `Scope` | compiler stdlib, platform modules | `effect` is legal only in a platform module, and platform-ness is a flag on the static `MODULES` table |
| The `Ui`/`Watch` intrinsics | backend runtime | bodyless methods lower to intrinsic keys; the JS backend resolves them to `$host_*` functions |
| Style extraction + stylesheet link step | compiler | needs cross-module visibility no library has |
| `Signal`, `Prop`, `Node`, `Style`, `Role`, all constructors | `ui`, ordinary Buri | no compiler support needed; movable to a real library once external repos land |
| Token enums and theme functions | each package / each app | ordinary Buri; exhaustive `match` is the compatibility check |

## Open

- **Rule 1 is unblessed.** It works because `is_effect_carrying` consults the
  enclosing signature's bounds and nothing re-checks at instantiation. Either
  document the contravariance rule or make the predicate variance-aware.
- **`Net.fetch` is synchronous** and the JS backend implements it with blocking
  `XMLHttpRequest`, which freezes a browser UI. `async` is deferred, so a UI
  platform needs a callback-shaped fetch: `ui.fetch(ctx, url, fn(c, resp) => ...)`.
- **Const-folding vs. generated token modules** — one of the two must be
  picked for token extraction (see Compilation).
- **The full property, widget, and event vocabularies** are unenumerated; the
  variants above are representative.
- **Structural responsiveness** (different children per size, not different
  styles) goes through `ui.viewport(ctx): Signal<ScreenSize>` and `ui.when` —
  JS-ful targets only. Not a style concern.
- **Grid auto-flow vs. explicit `.Area` placement** — decide when a real
  photo-grid component needs it; `Track` + `Span` cover the common case.
