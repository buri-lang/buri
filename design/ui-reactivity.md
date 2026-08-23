# UI reactivity

Signal-based, fine-grained reactivity for Buri. No virtual DOM, no top-level
model, no JSX. A component is an ordinary function that runs **once**; only
reactive leaves re-run.

The design rests on one idea: **a signal handle is inert data, and the authority
to read or write it travels through `ctx`.** That is the same split `Alloc` and
`Region` already use — the handle is the reference, the context is the
capability.

## Types

```buri
// ui/cap — a platform module (only platform modules may declare effects)
export effect Ui {
  fn signal<T>(self: Self, initial: T): Int;
  fn read<T>(self: Self, id: Int): T;
  fn write<T>(self: Self, id: Int, value: T): ();
  fn memo<T>(self: Self, compute: fn(UiRead) => T): Int;
  fn effect(self: Self, run: fn(UiRead) => ()): ();
  // list/keyed reconciliation, node registration, mount
}

/// The read capability, alone. Minted only by attenuating a real context, so
/// a `Computed` provably cannot fetch, log, write, or navigate — only read.
export struct UiRead(Int);              // private field: unforgeable outside ui/host
export fn reader<C: Ui>(ctx: C): UiRead
```

```buri
// ui — ordinary library code

/// An index into the runtime graph. Holds no authority, so a lambda may
/// capture it. That is what makes event handlers expressible.
export struct Signal<T>(Int);

/// A time-varying value. A component cannot tell which variant it was given.
export enum Prop<T> {
  Const(T),
  Cell(Signal<T>),
  Computed(fn(UiRead) => T),
}

export struct Attr<C> { name: Str, value: AttrValue<C> }

enum AttrValue<C> {
  Static(Str),
  Dynamic(fn(C) => Str),
  Handler(fn(C, Event) => ()),
}

export enum Node<C> {
  Text(Str),
  Dyn(fn(C) => Node<C>),
  El { tag: Str, attrs: [Attr<C>], children: [Node<C>] },
  Keyed(ListView),                      // registered by ui.each
}
```

`Prop<T>` names no context type parameter, so it is not effect-carrying and is
freely capturable inside handlers. `Node<C>` keeps one because handlers are
open-ended: a click may legitimately need `Net`, and which effects a program
permits is the budget `main` chose.

## Rules that make it typecheck

1. **Pure constructors leave `C` unbounded.** `fn el<C>(tag: Str, attrs:
   [Attr<C>], children: [Node<C>]): Node<C>`. With an effect bound on `C`,
   `[Node<C>]` is effect-carrying and rule 26 rejects the parameter. Unbounded,
   it is legal. Sound because `C` occurs only in argument position: extracting a
   `C` from a `Node<C>` would require already holding one.
2. **Nothing captures a context.** Handlers and thunks receive theirs as a
   parameter — the `mapCtx` shape §10.6 already mandates.
3. **Derivation is pure.** `.Computed(fn(r) => user.read(r).name)` needs no
   `ctx`, so a component that only reads props and builds a tree has no context
   parameter and is pure by §10.4.
4. **Tree construction needs no `Alloc`.** Struct, enum, array and closure
   literals are fixed-size construction (§10.5).
5. **Generic components bound `T` with an ordinary trait** (e.g. `T: Eq`) if
   they capture a `Signal<T>` or `Prop<T>` in a handler; an unbounded `T`
   answers `true` to `may_carry_effect`.

## API

```buri
// state — needs a context
ui.signal<C: Ui, T>(ctx: C, initial: T): Signal<T>
ui.memo<C: Ui, T>(ctx: C, f: fn(UiRead) => T): Prop<T>
ui.effect<C: Ui>(ctx: C, f: fn(UiRead) => ()): ()
ui.each<C: Ui, T>(ctx: C, items: Prop<[T]>, row: fn(C, T, Int) => Node<C>): Node<C>
ui.mount<C: Ui>(ctx: C, root: Node<C>): Result<(), Str>

// reading — Signal needs a context, Prop takes a reader
Signal.get<C: Ui>(self, ctx: C): T
Signal.set<C: Ui>(self, ctx: C, value: T): ()
Signal.update<C: Ui>(self, ctx: C, f: fn(T) => T): ()
Prop.read(self, r: UiRead): T

// tree — all pure, all leave C unbounded
ui.el, ui.text, ui.dyn, ui.attr, ui.bindAttr, ui.on, ui.when
```

## Runtime

Auto-tracking. The runtime holds a "currently executing computation" pointer;
`read` records a source → computation edge; `write` marks dependents dirty and
schedules them. Dependencies are re-collected per run, so conditional reads are
tracked exactly.

`Prop.Const` is visible to the runtime as a constructor, so a static prop
registers no node at all. Disposal is keyed on which computation was executing
when a signal was created.

## Example

```buri
// Pure: no ctx. Cannot tell whether either prop varies.
fn badge<C>(title: Prop<Str>, count: Prop<Int>): Node<C> {
  ui.el("span", [ui.attr("class", "badge")], [
    ui.dyn(fn(c) => ui.text("${title.read(ui.reader(c))}: ${count.read(ui.reader(c))}")),
  ])
}

fn counter<C: Ui>(ctx: C, label: Str): Node<C> {
  let count = ui.signal(ctx, 0);

  ui.el("div", [], [
    ui.el("button",
      [ui.on("click", fn(c, e) => count.update(c, fn(n) => n + 1))],
      [ui.text(label)]),
    badge(.Const(label), .Cell(count)),
    badge(.Const("doubled"), .Computed(fn(r) => count.get(r) * 2)),
  ])
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Ui: host.ui };
  ui.mount(ctx, counter(ctx, "clicks"))
}
```

`main` returns `.Ok(())` and the page stays live: the JS entry wrapper only
exits on an `.Err`, and registered listeners keep running.

## What ships where

| Piece | Where | Why |
|---|---|---|
| `ui/cap`, `ui/host`, `UiRead` | compiler stdlib, platform modules | `effect` is legal only in a platform module, and platform-ness is a flag on the static `MODULES` table |
| The `Ui` intrinsics | backend runtime | bodyless methods lower to intrinsic keys; the JS backend resolves them to `$host_*` functions |
| `Signal`, `Prop`, `Node`, all constructors | `ui`, ordinary Buri | no compiler support needed; movable to a real library once external repos land |

## Open

- **Rule 1 is unblessed.** It works because `is_effect_carrying` consults the
  enclosing signature's bounds and nothing re-checks at instantiation. Either
  document the contravariance rule or make the predicate variance-aware.
- **`Net.fetch` is synchronous** and the JS backend implements it with blocking
  `XMLHttpRequest`, which freezes a browser UI. `async` is deferred, so a UI
  platform needs a callback-shaped fetch: `ui.fetch(ctx, url, fn(c, resp) => ...)`.
