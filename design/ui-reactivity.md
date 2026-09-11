# UI reactivity and styling

**This has shipped.** What a user needs lives where the suites can check it: the
`ui/*` modules' own documentation (`buri docs ui/node` and its neighbours), the
guide's "User interfaces" section, and — for the `WEB` output — `cli/src/docs/build/`.
What stays here is the **argument**: why the shape is this shape, what was
refused, and, in "As built" below, every place compiling it overruled the
argument. The fragments illustrate the reasoning; they are not the signatures of
record.

Signal-based, fine-grained reactivity for Buri. No virtual DOM, no top-level
model, no JSX. A component is an ordinary function that runs **once**; only
reactive leaves re-run. The tree vocabulary is platform-neutral: web is one
backend, native and HTML email are others.

The design rests on one idea: **a signal handle is inert data, and the
authority to read or write it travels through `ctx`.** That is the same split
`Allocator` and `Region` already use — the handle is the reference, the context is
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
/// because the authority is: `Network.fetch` blocks until the response arrives,
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

An implementor of `Ui` grants strictly more than an implementor of `Watch`: it
reads too, because `Signal.update<C: Ui>` reads the old value on its way to
writing a new one.

**A `Scope` also implements `Allocator`**, and that is what makes a derivation
more than a projection: `filter`, `sort`, `map` and `str.format` all name
`Allocator`, so a `Scope` that implements `Watch` alone can pick a value out of
the graph and cannot build one. It grants nothing by doing so — `Allocator` is
the one effect whose implementation carries no authority, because a `Region` is
a number, which is the same reason `core/alloc` is importable anywhere and
`core/host` is not. Writing stays out: `Ui` is the effect that writes, and a
closure that wrote a signal it read would be a loop the runtime schedules rather
than a value it caches. The two alternatives that lost, and the memory
argument, are in `design/native/DECISIONS.md`.

`Fetch`'s callback takes `Self` rather than a bare context type, and that is
what makes a test double possible. A free `fetch<C: Fetch>` intrinsic would have
one implementation for every `C`, so a headless `NoFetch` would still reach the
network. Handing the receiver to the runtime keeps the call site
`ui.fetch(ctx, request, done)` and the double ordinary Buri.

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
each<C, T: Equal>(items: Prop<[T]>, key: fn(T) => Str,      // ui/node
               row: fn(C, T, Int) => Node<C>): Node<C>
mount<C: Ui>(ctx: C, root: Node<C>, themes: [Theme]): Result<(), Str>
```

`each` takes no context: a list is a description, and describing one is pure.
The `key` is a parameter for the same reason `alt` is — keying a list by
position silently corrupts it the moment the list is reordered, and nothing can
notice.

## The tree

HTML mixes layout and meaning; this vocabulary splits them. Meaning comes from
the accessibility taxonomy (ARIA landmarks), which is already cross-platform:
web lowers roles to semantic elements, native backends lower them to
accessibility traits. **No constructor is named after an HTML element**, and
there is no tag-string escape hatch. Anything reachable only by tag name becomes
a role or a widget.

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
// the level is the outline; the size and the weight are the styles
ui.heading(level: Int, styles, content: Prop<Str>): Node<C>

// widgets — interactive behaviour, not roles. Accessibility-critical
// parameters (alt, dest, label) are required, not attributes.
// `disabled` is an attribute and not a style: it is what refuses the press,
// takes the control out of the tab order, and fires On(.Disabled, ...).
// The styles land on the control itself, so a state rule fires on it;
// `around` is the second box a labelled control has, and the one a row lays
// out. A button with no children shows its label.
ui.button(label, styles, children, onPress: fn(C, Event) => (), disabled: Prop<Bool>): Node<C>
ui.link(dest: Prop<Str>, styles, children: [Node<C>]): Node<C>
ui.image(source: Prop<Str>, alt: Prop<Str>, styles): Node<C>
// artwork in the tree, so `currentColor` in it is the element's own Foreground.
// The source is written out and the compiler reads it: `<svg>` and shapes only.
ui.icon(styles, source: Str): Node<C>            // decorative, always
ui.field(label, kind: FieldKind, styles, around,
         value: Signal<Str>, invalid: Prop<Bool>, disabled: Prop<Bool>): Node<C>
// value is also the answer to On(.Checked, ...): the box is checked, not the page
ui.toggle(label, kind: ToggleKind, styles, around,
          value: Signal<Bool>, invalid: Prop<Bool>, disabled: Prop<Bool>): Node<C>
ui.form(onSubmit: fn(C, Event) => (), styles, children): Node<C>
// the form's action: no handler of its own, and `type="submit"` is what makes
// Enter in a field reach onSubmit. A form of two fields needs one.
ui.submit(label: Prop<Str>, styles): Node<C>

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
relationships, which is accessibility, so it gets roles; visual arrangement is
`.Layout(.Grid)`. Using either for the other is a named antipattern. There is
deliberately no `Grid` *role*, so the confusion has nothing to grab.

**`field` takes its label.** An unlabelled input is the commonest accessibility
failure there is, and unlike a missing `alt` it has no visual fallback, so the
vocabulary will not express it. A field and a toggle have no change event
either: both bind to a `Signal`, so what the reader typed is already there and
two-way binding replaces the event.

**`form` is a widget and not a role**, because submission is behaviour: pressing
Enter in a field inside one runs `onSubmit`, the browser's own dispatch rather
than a key handler every app would otherwise write.

`Node<C>` keeps its one type parameter because handlers are open-ended. A press
may legitimately need `Network`, and `main` chose the effect budget. Everything else
(`Prop`, `Style`, `Signal`) names no context type and is plain, capturable data.

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
  // 53 properties. The arithmetic, because the cut line is the design:
  //   11  arrangement, and a child's part in it: Layout, AlignMain, AlignCross,
  //       AlignSelf, Wrap, Scroll, Grow, Shrink, Span, Pin, Position
  //    8  space:      Gap{,X,Y}, Padding{,X,Y}, PaddingEdge, Bleed
  //    7  extent:     {Min,Max,}Width, {Min,Max,}Height, AspectRatio
  //   11  paint:      Background, Foreground, Border{Width,Edge,Color,Style},
  //       Radius, RadiusCorner, Opacity, Shadow, Shadows
  //   11  type:       FontFamily, FontSize, FontWeight, Italic, LineHeight,
  //       LetterSpacing, TextAlign, TextCase, TextLine, TextWrap, Truncate
  //    2  interaction: Cursor, Passthrough
  //    1  ListMarker
  //    1  Translate — the one transform, applied after the layout
  //    1  Clip
  Layout(Layout),                       // on the container
  AlignMain(Align), AlignCross(Align),  // main/cross axis: survives direction flips
  Grow(Int), Shrink(Int), Span(Int),    // on a child
  Pin(Edge, Length), Position(Position),
  PaddingX(Length), Gap(Length), Width(Length), Radius(Length),
  BorderEdge(Edge, Length),             // one edge; the colour stays whole-box
  RadiusCorner(Corner, Length),         // one corner; a joined group squares a side
  Bleed(Edge, Length),                  // the one way out of the container's box
  Background(Color), Foreground(Color), Truncate(Int), ...,
  Shadow(Shadow), Shadows([Shadow]),    // one slot; the last written wins
  Translate(Length, Length),            // after the layout; no sibling moves
  Clip(Bool),                           // cut to the box, without a scroll container
  Passthrough(Bool),                    // the pointer goes to whatever is behind

  // and six combinators
  Group([Style]),                       // composition; array literal, no Allocator
  On(State, [Style]),                   // pseudo-class; static, in the stylesheet
  At(Screen, [Style]),                  // breakpoint;   static, in the stylesheet
  When(Prop<Bool>, [Style], [Style]),   // both branches statically extracted
  Computed(fn(Scope) => [Style]),       // never in the stylesheet
  Extracted([(Int, Str)]),              // compiler-produced; a program cannot write one
}

export enum Layout {
  Column, Row,              // stacks; Column is the default
  ColumnReverse, RowReverse,// the same, painted backwards; the document keeps
                            // the order the caller wrote, which is what a
                            // screen reader and the tab ring read. A grid does
                            // not reverse: its children go in named tracks
  Grid([Track]),            // explicit tracks; Track = Fraction(Int) | Fixed(Length) | Auto
  Layers,                   // children share one space (ZStack), in written order
}

export enum Edge   { Top, Bottom, Start, End }
export enum Corner { TopStart, TopEnd, BottomStart, BottomEnd }
export enum State { Hover, Focus, FocusWithin, Active, Disabled, Checked,
                    Invalid }
                          // Focus is :focus-visible on the element; FocusWithin
                          // is the container's — an input group rings as one.
                          // Six the platform tracks, and one a program enters:
                          // `field` and `toggle` take an `invalid`, which writes
                          // the `aria-invalid` the rule hangs off
export enum Screen { Small, Medium, Large, ExtraLarge }
                          // closed names, so libraries compose; the widths are
                          // 40 / 48 / 64 / 80 rem, which follow the reader's
                          // text size rather than the device's pixels

export enum Length { Px(Int), Rem(Float), Em(Float), Percent(Float), Auto, Full }
                          // Rem follows the root's text size, Em the element's
                          // own — which is the only way tracking is right at
                          // more than one size
export enum Color  { Rgb(Int, Int, Int), Rgba(Int, Int, Int, Float),
                     Token(TokenReference), Transparent, Inherit,
                     Faded(TokenReference, Float) }
                          // `Faded` is what `color.alpha(0.5)` answers for a
                          // token, and only for a token: a colour written out
                          // fades to an `Rgba` the compiler works out. It is
                          // last because the order is the tag the runtime
                          // reads
```

`Shadow(Shadow)` and `Shadows([Shadow])` are two spellings of one `box-shadow`,
so they share one conflict slot and the last written wins. Every elevation worth
having is two layers and a focus ring is a third beside them, which is why the
list exists; one layer stays the shorter spelling.

Deliberately absent: floats, margin collapsing, inline-block — stacks, `Gap`,
and `Layers` replace them, and none survive cross-platform. Space between
things belongs to the container that arranged them, so there is no inward
margin. `Bleed` is the one margin there is and it only goes outwards: a child
reaching past its parent's padding — a full-width rule in a padded menu, an
avatar lapping the one before it — has nothing else to ask with.

`Clip` and `Scroll` are the pair over one declaration: `Scroll` says content
outside the box can be reached by scrolling to it, `Clip` says it is not
painted at all. A card cutting a full-bleed picture to its corners wants the
second and would pay for the first in scrollbars and a region a keyboard lands
in.

**Hover is a style, not an event**, and `On` is why. A pseudo-class costs
nothing at run time, needs no listener, survives into an email's `<style>`
block, and maps to a native pressed or focused trait. A signal written on every
mouse move does none of the four and costs a render each time.

Two tiers, on purpose:

- **Static** (everything except `Computed`): the compiler extracts each distinct
  property value into one atomic utility class and dedupes across the whole
  build. `When` emits both branches as ordinary classes and the runtime picks a
  precomputed class string — nothing is generated at runtime, ever. `At` emits
  media-query-scoped variant classes, mobile-first, with larger tiers overriding
  smaller, so breakpoints work in email `<style>` blocks and cost nothing at
  runtime. Native backends re-resolve `At` when the window size class changes.
- **`Computed`**: for values driven by signals (drag, cursor-follow,
  animation). The runtime applies these per element (inline styles on web) and
  keeps them out of the stylesheet. Each one re-serializes on change, so the
  documentation says static tier first, `Computed` for genuinely dynamic values.

A style the compiler cannot evaluate — one built out of a function's parameters,
say — is **not an error**. It degrades to the same inline application `Computed`
gets, which lets folding improve later without opening a correctness window.
`On` and `At` are the two exceptions, because neither has an inline form to
degrade *to*: you cannot write `:hover` or a media query into an element's
`style` attribute. Anything under one of them is statically known, or the
compiler rejects the program.

Conflicts resolve per property, last one wins, whether the compiler settled it
or a runtime scan did — the scan only ever *chooses between* classes the
compiler already emitted. A style that arrives as a *parameter* (the
overridable-component case) resolves at runtime by a linear scan over
compiler-assigned `(slot, class)` pairs. A slot is the property, **its
condition**, and **the edge or corner** where the property names one: `Padding`
and `On(.Hover, [Padding])` are different slots, so are `Pin(.Top, ...)` and
`Pin(.Bottom, ...)`, and so are `RadiusCorner(.TopStart, ...)` and
`RadiusCorner(.TopEnd, ...)`, and "per property" stopped being enough the moment
`On` existed. Two *different* properties that touch the same underlying declaration
— `Padding` and `PaddingX`, `BorderWidth` and `BorderStyle` — are settled by the
declaration order of the variants, because the sheet is written in that order and
equal-specificity rules resolve by position. **The variant order is part of the
vocabulary's contract**, not an implementation detail, and the narrower property
always comes after the broader one.

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

`token(namespace, name)` is the constructor. `TokenReference` is opaque with
private fields, so `Style` never learns any package's token type: a library's
styles name only that library's vocabulary, and a reference is all that crosses
the boundary. A consumer closes the loop at mount with one theme function per
library it uses, mapping that library's tokens to its own tokens or to raw
values:

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

On web, each token lowers to a namespaced custom property (`--cardlib-surface`),
and each theme installs one `:root` block of values in the order the app passed
them. `theme.switching(condition, whenTrue, whenFalse)` switches whole themes,
and the condition is a `Prop<Bool>`, so dark mode is a signal, a stored
preference, or a media query bridged into one. Switching rewrites the block and
**every class on every element stays exactly as it was**: nothing is
re-extracted, no element is touched, and the browser repaints from variables it
already had. That is the whole reason dark mode is not a second stylesheet.

`Color.alpha(f)` is what keeps a translucent shade from needing a token of its
own. On a colour written out it is arithmetic the compiler does; on a token it
lowers to `color-mix(in srgb, var(--cardlib-ring) 50%, transparent)`, so the
token still decides the hue and a theme that changes it changes the fade with
it. Without it a focus ring alone costs three extra tokens — `--ring-soft`,
`--destructive-soft` and its dark twin — hand-blended in every theme and
drifting apart the day one of them moves. The fraction is 0 to 1 and the
extractor refuses anything else, because a `color-mix` percentage outside 0 to
100 makes the declaration invalid and the colour vanishes.

## Rules that make it typecheck

1. **Pure constructors leave `C` unbounded.** `fn region<C>(role: Role, styles:
   [Style], children: [Node<C>]): Node<C>`. With an effect bound on `C`,
   `[Node<C>]` is effect-carrying and rule 26 rejects the parameter. Unbounded,
   it is legal. Sound because `C` occurs only in argument position: extracting
   a `C` from a `Node<C>` would require already holding one.

   **The soundness argument was right; the predicate was not.** Reading it off
   the enclosing signature's bounds made `mount(ctx: C, root: Node<C>)` with
   `C: Ui` a rule-26 error. What landed is a variance-aware predicate: a least
   fixpoint `provides(con, i)` over every declared constructor, computed from
   its fields, dropping function *parameters* and keeping results. It
   generalises to every user-declared type the rule the built-in `Ty::Fn`
   already had — only the result counts — so "occurs only in argument position"
   is something the compiler knows rather than something this document asserts.
2. **Nothing captures a context or a `Scope`.** Handlers and computed closures
   receive theirs as a parameter — the `mapCtx` shape §10.6 already mandates.
3. **Derivation is pure.** `.Computed(fn(c) => user.read(c).name)` needs no
   enclosing `ctx`, so a component that only reads props and builds a tree has
   no context parameter and is pure by §10.4.
4. **Tree and style construction need no `Allocator`.** Struct, enum, array and
   closure literals are fixed-size construction (§10.5).
5. **A captured generic must be bound where the type is actually stored.**
   `Prop<T>` holds its `T`, so capturing one needs `T: Equal` or another ordinary
   trait — an unbounded `T` answers `true` to `may_carry_effect`. `Signal<T>` is
   phantom in `T` and needs nothing, which is what makes an event handler that
   closes over a signal expressible without a bound the caller has to invent.
   `each` bounds its element type for the first reason, not the second.

## Runtime

Auto-tracking. The runtime holds a "currently executing computation" pointer.
`read` records a source → computation edge; `write` marks dependents dirty and
schedules them. Every run re-collects dependencies, so conditional reads are
tracked exactly. `Prop.Const` is a visible constructor, so a static prop
registers nothing. Disposal keys on which computation was executing when a
signal was created.

## Compilation

- **Reactivity needs no compiler work** beyond the platform modules and their
  intrinsics (bodyless methods lowered to intrinsic keys, resolved by each
  backend's runtime — the existing `core/host` mechanism).
- **Styling has to be toolchain work.** With no macros, no reflection and no
  runtime generation, no library can see another module's style literals.
  Compiling a module collects its static `Style` literals into a `Vec` on
  `Checked`, and the link step merges and dedupes them into one stylesheet plus
  the `(slot, class)` table. Local compilation survives; only the link step is
  global, and it already was.
- Token constructors are calls, not literals, so extraction needs constant
  folding of pure calls in `const` initializers. **Const-folding is what
  landed** — an interpreter over the typed tree, reading purity off the
  function's own signature — rather than generated token modules: it improves an
  ordinary style helper as much as it improves a token.
- **A `WEB` build writes three files**, not one: the `.mjs`, the `.css` the
  extractor produced, and an `.html` shell that links the sheet and loads the
  module. The shell's `<link id="buri-styles">` carries the id the runtime's own
  installer looks for, so the rules are in the page before the first paint.
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
    ui.button(.Const(label), [], [], fn(c, e) => count.update(c, fn(n) => n + 1), .Const(false)),
    badge(.Const(label), .Cell(count)),
    badge(.Const("doubled"), .Computed(fn(c) => count.get(c) * 2)),
  ])
}

export fn main(): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Ui: host.ui, Watch: host.watch };
  ui.mount(ctx, counter(ctx, "clicks"), [app.themed(appTheme)])
}
```

`main` returns `.Ok(())` and the page stays live: the JS entry wrapper only
exits on an `.Err`, and registered listeners keep running.

That program is in the corpus at `cli/tests/golden_javascript/ui_counter/`. A
whole application — a keyed list, a form, both style tiers, one library's tokens
themed by an app, and a request that answers through a callback — is
`cli/tests/example/cmd/basket/`, built as a `WEB` artifact and tested with no
browser.

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

  `ANDROID` and `EMAIL` are the shapes it was widened for, and neither is in the
  enum yet. Each would cost a row in the grant table and a backend, and nothing
  above would change.

  A `WEB` output takes no `arch`, because JavaScript has none, and no
  `js { module }`, because a browser loads an ES module and there is no second
  kind. Naming either is a build-file error rather than a field the toolchain
  then quietly ignores.
- **Enforcement is a compile error, over every output at once.** `main` is the
  only module that can import `core/host`, and the compiler checks `main.buri`
  against the platforms its rule's `outputs` name — every one of them, plus
  every platform its suite names in `test.platforms`, because a test binary
  links `main` in. So it refuses `Ui: host.ui` under `platform: LINUX`, refuses
  `FileSystemRead: host.fs` under `platform: WEB`, and refuses a binary declaring both
  `MACOS` and `WEB` for the second whichever one is being built. The diagnostic
  is `effect-not-on-platform`, and it names the effect, the platforms that do
  not allow it, and the platforms that *do* grant it. A platform *is* the set of
  effects its host exports; there is no second declaration.

  The error lands where the program asked. A named import is refused on the name
  inside the braces; a namespace import names no effect, so `host.fs` is
  refused on the member reference. Both are semantics-layer diagnostics on a
  span, so `buri lint`, `buri test` and the language server report them before
  anything is built.

  Three consequences worth writing down. **A grant is a pair** — the value and
  the implementation struct — and both are refused together: a host struct has
  no private field, so allowing `HostNetwork` while refusing `net` would leave the
  authority one `Network: host.HostNetwork {}` away. **A build still subsets
  `core/host` per output**, the backstop this check sits in front of. And **a
  rule that declares no platforms commits to none**: a library with no
  `platforms` field is platform-generic and is never refused, which keeps a
  bound — `FileSystemRead` taken as a bound rather than bound to a host — legal
  everywhere, a page included.

  WEB grants `Allocator`, `Stdout`, `Stderr`, `Clock`, `Random`, `Network`, `Tasks`, `Ui`
  and `Watch`, and withholds `FileSystemRead`, `FileSystemWrite`, `Stdin`, `Environment`, `Process`,
  `Listen` and `Sockets`. `LINUX` and `MACOS` grant all fourteen non-UI effects
  and neither UI one; `JS` grants twelve of the fourteen — everything but
  `Listen` and `Sockets`.

  **A row may name no platform at all**, and that is the route `Tasks`, `Listen`
  and `Sockets` each came down: the names, the signature and the refusal exist
  before the runtime does, with no second "not implemented yet" flag anywhere,
  and the grant is later that one row gaining platforms. No program written
  against the reviewed signature had to change when the runtime arrived.
  `Listen` and `Sockets` were granted **together**, because being a server is
  one authority in two halves: accepting a connection, and writing to one
  somebody already accepted. `JS` and `WEB` will never have them — a page is
  served rather than serving — which bounds what an empty row ever claimed: not
  that everybody eventually grants this. `Tasks` came down it too and then
  widened again: granted by nobody, then on the three platforms that are
  not a page, and now on all four, once `core/tasks`'s `spawn` gave a page a
  task worth running. `design/native/DECISIONS.md` carries that reversal.
- **Email is a different effect grant, not a lesser web.** Its host exports
  rendering and nothing interactive: no `Ui`, no `Fetch`. A `render` evaluates
  the tree once, so `Const` and `Computed` props resolve and `Cell` has nothing
  to back it. Component libraries written against `Prop` and `Style` work
  untouched, and an app that binds interactive effects fails at `main`. An
  `EMAIL` row in the grant table granting neither is the whole mechanism.

## Modules

UI gets its own reserved root, `ui/...`, so `core/` keeps meaning "the
deliberately small essentials." **This extends SPEC rule 35** (module paths were
`core/...` or `//...`): the rule's wording, the path check in `modules.rs`, and
entries in the static `MODULES` table. The platform implementations fold into
the existing `core/host`, which is already per-platform and main-only;
UI-capable platforms export three more values from it. When external
repositories land, `ui/...` can migrate out wholesale.

| Module | Kind | Exports |
|---|---|---|
| `ui/effect` | platform | `effect Watch`, `effect Ui`, `effect Fetch`, `Scope`, `Event`, `Request`, `FetchError`, `fetch` |
| `core/host` (WEB, …) | platform | adds `ui`, `watch`, `fetch` — the implementations `main` binds |
| `ui/signal` | library | `Signal<T>` (`get`/`set`/`update`), `signal`, `watch` |
| `ui/prop` | library | `Prop<T>` (`read`), `memo` |
| `ui/node` | library | `Node<C>`, `Role`, `FieldKind`, `nothing`, `stack`, `region`, `row`, `column`, `spacer`, `text`, `heading`, `button`, `link`, `image`, `field`, `toggle`, `form`, `submit`, `choose`, `computed`, `each`, `icon`, `mount` |
| `ui/style` | library | `Style`, `Layout`, `Track`, `Screen`, `State`, `Position`, `Length`, `Color`, `Align`, `Axis`, `Edge`, `Weight`, `FontFamily`, `BorderStyle`, `TextCase`, `TextLine`, `TextWrap`, `Cursor`, `Shadow`, `TokenReference`, `token` |
| `ui/theme` | library | `Theme`, `Scheme`, `themed`, `switching`, `scheme` |
| `ui/testing` | test platform | headless `Ui`/`Watch`/`Fetch`, render-to-document, event firing, the extracted stylesheet, installed theme values, and a recorder — test-only automatically via the `testing` path segment |

There is **no `ui` umbrella module**: re-exporting from seven modules buys one
import and costs a reader the answer to "which module is this name from".
Adding it later is a re-export list and nothing else. Token modules are not
standard library either: each package declares its own.

Typical imports — a component module:

```buri
from "ui/node" import * as ui;
from "ui/node" import { Node };              // signature types
from "ui/prop" import { Prop };
from "ui/style" import { Style };
from "//lib/cardlib" import { Token };
```

`main`:

```buri
from "core/host" import * as host;
from "ui/effect" import { Fetch, Ui, Watch };
from "ui/node" import * as ui;
// context { Allocator: host.alloc, Ui: host.ui, Watch: host.watch, Fetch: host.fetch }
```

A component test:

```buri
from "core/testing/assert" import * as assert;
from "ui/testing" import { headless, observer, render };
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

Where compiling the argument above overruled it. Each row is a deviation from
this document's first draft, with the reason.

| This document said | What shipped | Why |
|---|---|---|
| `ui/effects`, `ui/cap` | `ui/effect` | "cap" is an abbreviation and not the language's word; the module names one thing |
| `Ui.effect`, `ui.effect` | `watch`, on both | `effect` is a reserved word, so no function may be called one |
| `Ui` without `read` | `Ui` reads too | `Signal.update<C: Ui>` reads the old value; without it the design's own signature is unimplementable |
| `ui.when` | `ui.choose` | `when` is a reserved word, held for a language feature not yet taken |
| `memo` in `ui/signal` | `memo` in `ui/prop` | it returns a `Prop`, and a module may not import the module that imports it |
| `ui.each(ctx, items, row)` | `each(items, key, row)` | a list is a description, so it needs no context; and keying by position corrupts a reordered list silently |
| `ui.field(value, .Const(false))` | `field(label, kind, styles, around, value, disabled)` | this document's own rule — an accessibility-critical parameter is a parameter — and an unlabelled input has no visual fallback |
| `Role::Form` | `ui.form`, a widget | submission is behaviour, not meaning |
| A control with no styles of its own | `styles` on all five, plus a reset | a wrapper is not what a browser hovers, focuses or disables, so `On(...)` on one never fired — and the browser's own chrome sat under whatever the wrapper painted |
| `button(label, styles, onPress)`, no children | `children` between the styles and the handler | a mark beside a word had to be a row, so the wash that marks an entry hovered or current went on the wrapper and only the focus ring stayed on the button |
| One style list on `field` and `toggle` | `styles` on the control, `around` on the `<label>` | the label is what a row lays out and nothing on the input reaches it, so a field inside a row was stuck at the width of its own label text |
| A `Style` naming a toggle's mark | `ToggleKind`, and the widget draws it | a mark is a shape rather than a box — there is no radius that makes a tick — and a control that can be asked for no mark at all is a control a program can leave unreadable |
| Rule 1 as an assertion | a variance-aware predicate | three of the APIs above did not compile without it; "occurs only in argument position" is now something the compiler computes |
| Rule 5 over `Signal<T>` | over `Prop<T>` | `Signal` is phantom in `T` and carries nothing; `Prop` stores its `T` |
| Style literals "cached with the module" | a `Vec` on `Checked` | the machinery it named does not exist: test cases are not cached, verdicts are |
| Const-folding *or* generated token modules | const-folding | it improves an ordinary style helper too, where a generator would have helped tokens alone |
| Blocking `Network.fetch` for pages | a separate `Fetch` effect | a callback shape, so a request does not freeze a page; a platform grants one or the other |
| "Enforcement already exists" | it does now | the main-only import rule existed; the per-output host subset did not, and was built — and is a compile error over every declared output at once now, `effect-not-on-platform` |
| Screen widths "app config at mount" | fixed at 40/48/64/80 rem | a breakpoint that varies per app is one a library cannot compose against |
| No way to express hover | `On(State, [Style])` | a pseudo-class costs nothing and survives to targets that have no pointer |
| A `ui` umbrella module | seven modules, no umbrella | an umbrella hides which module a name belongs to, and buys one import |
| `ui.viewport` | not built | structural responsiveness is still open, and the signal is the smallest part of it |
| `Scope` implements `Watch` alone | `Watch` **and** `Allocator` | a derivation that cannot allocate cannot filter, sort or format, so `each` could only ever walk a signal's raw contents and every formatted number in an app had to be a `.Const` built once and never updated (buri-lang/buri#50) |

## Open

- **`Tasks` on `WEB`.** Granted on the other three, withheld from the page:
  `parallel` waits for its last task, and a page has an interface where that
  wait shows. A page would want the callback shape `Fetch` already has, and that
  belongs to the concurrency work, not to this document.
- **A socket to hand out, and a handler per task.** The acceptor exists:
  `Listen` is four operations, `core/net/server` runs the accept loop over them
  in Buri, and `cli/runtime/net.rs` answers them with a hand-framed HTTP/1.1
  server. Two things are left, and both are runtime work rather than a table
  edit. `serve` takes one connection at a time and runs the handler on the
  calling task, so one slow handler becomes the whole server's latency. And
  `Sockets` is granted but unreachable: nothing performs a WebSocket upgrade, so
  no program can get hold of a socket to write to.
- **A per-target vocabulary check.** A style or widget with no meaning on some
  target — hover in email, a form in a static render. Backend degradation with a
  warning is the answer until real components hit it.
- **Structural responsiveness** (different children per size, not different
  styles) needs a `viewport` signal and `choose` — JS-ful targets only. Not a
  style concern, and not built.
- **Grid auto-flow vs. explicit `.Area` placement** — decide when a real
  photo-grid component needs it; `Track` + `Span` cover the common case.
- **The browser's own half of the tests.** `ui/testing` renders with the
  shipping renderer against a document the runtime supplies, so what goes
  unasserted is exactly what only a browser does: layout and painting, its
  dispatch of a press, focus and selection, and what assistive technology
  announces. Covering that needs a real browser driven from outside, and this
  repository has no such suite.
