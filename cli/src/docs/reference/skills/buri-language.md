---
name: buri-language
description: Use when reading or writing Buri (.buri) source — syntax, immutability, expressions, patterns, modules, and the rules that catch out anyone arriving from Rust, TypeScript, or Go.
---

# Buri: the language

Strict, purely functional, statically typed. TypeScript-shaped syntax,
Rust-shaped data declarations, Roc-shaped platforms and effects.

The full rules ship in the binary: `buri docs language/lexical`,
`language/modules`, `language/types`, `language/expressions`, `language/patterns`,
`language/evaluation`, `language/functions`, `language/effects` and
`language/programs`. `buri docs search <words>` searches every page.

**Check the library before writing a helper.** Bare `buri docs` lists every
`core/*` and `ui/*` module, `buri docs core/str` renders one, and
`buri docs core/str.padStart` one item. Comparators, hex, base64, varints,
grouping, checksums, dates, argument parsing and a CLI already exist. A
hand-rolled one compiles and gives the wrong answer.

## The twelve things that will trip you up

- **No mutation.** Every binding is final. No assignment, no `mut`, no
  interior mutability, no references, no borrow checker, no lifetimes.
- **No loops.** Use recursion or a fold. Tail calls are eliminated, mutual ones
  included.
- **No `return`.** Postfix `?` is the only early exit.
- **No `null` or `undefined`.** Absence is `Option<T>`, and indexing an array
  yields `Option<T>`.
- **`else` is mandatory**, conditions are parenthesised, and there's no
  truthiness: `if (n < 0) { ... } else { ... }`.
- **No implicit numeric conversion.** `1.0 + 1` is an error, and so is
  `I32 + I64`. Convert with a method: `a.toI64()`.
- **Effects arrive as a parameter named `ctx`, and you perform one by passing
  `ctx` to a function.** A function with no `ctx` and no effect-carrying `self`
  can't touch the world. Print with `io.println(ctx, "hi")`, not
  `ctx.println("hi")` (`effect-method-call`). The doors are `core/io`,
  `core/fs`, `core/env`, `core/time`, `core/random`, `core/alloc`,
  `core/net/http`, `core/net/server`, `core/process`, `core/tasks` and
  `ui/signal`. The filesystem is **two** effects, `FileSystemRead` and
  `FileSystemWrite`, declared in `core/fs` (not `platform/effect`), and every
  function there takes a `core/path` `Path`, not a `Str`. See the `buri-types`
  skill.
- **A bare identifier in a pattern is always a binding.** `None` binds a
  variable; write `.None` or `Option.None` to match the variant.
- **You may not discard a `Result`.** `let _ = someResult()` is a compile
  error (`result-discarded`), and so is a nested `_` like
  `let (n, _) = (1, someResult())`, or the bare call as a statement. Consume it
  with `?`, `match` or `.withDefault(...)`, or drop it on purpose with
  `.ignore()`, which `buri lint` reports as `discarded-result`. **Prints return
  one too:** `let _ = io.println(ctx, "hi").ignore();`.
- **No relative imports.** A module path is `core/...`, `ui/...`, `std/...`, or
  `//...` from the repository root.
- **Methods live in an `impl` block in their type's own module.** They're found
  through the receiver's type, so they need no import, and you can't add one to
  someone else's type.
- **No `panic`, no `unreachable`, no bottom type.** Every case is handled.
  Division by zero and stack exhaustion *abort*; nothing catches.

## A whole program

```buri
from "platform/effect" import { Allocator, Stdout };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Point {
    x: Float,
    y: Float,
}

enum Shape {
    Circle(Float),
    Rect { width: Float, height: Float },
    Empty,
}

// No context parameter, so this cannot allocate, read, write, or observe
// anything. It is a mathematical function of its argument.
impl Shape {
    fn area(self): Float {
        match (self) {
            .Circle(r) => 3.14159 * r * r,
            .Rect { width, height } => width * height,
            .Empty => 0.0,
        }
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };

    let shapes = [Shape.Circle(1.0), Shape.Rect { width: 2.0, height: 3.0 }];
    let total = shapes.map(ctx, fn(s) => s.area()).sumFloat();
    let _ = io.println(ctx, "total area: ${total}").ignore();
    .Ok(())
}
```

`main` is an *entry*: it takes its platform's host and returns
`Result<(), Str>`. The hosts are `NativeHost` from `"native"`, `NodeHost` from
`"node"` (the default when a binary has no `outputs`), and `WebHost` from
`"web"`. Only an entry may build a context, from the host's fields. A bare
`fn main()` is `entry-without-host`; a field the platform lacks is
`no-such-field`. `.Ok(())` exits 0; `.Err(msg)` prints `msg` on stderr and
exits 1.

A program for two platforms gives each output its own entry,
`{ platform: "node", entries: [{ name: "main", function: "mainForNode" }] }`,
and both call one function taking `ctx`. A host the toolchain doesn't ship,
like a Cloudflare Worker, is a repository platform:
`{ platform: "//platform/cloudflare_worker" }` (`buri docs guides/custom-platforms`).

**Import the effect names.** `context { Allocator: host.alloc }` without
`from "platform/effect" import { Allocator };` fails with `not-an-effect`.

## Modules

The path comes first, and imports end in `;`.

```buri
from "core/list" import { map, filter };
from "core/list" import { map as listMap };
from "core/list" import * as list;
from "//lib/money" import { Cents };
```

- There's no `from "core/list" import *;`. The only wildcard is `* as <name>`.
- Declarations are private unless prefixed with `export`. Struct fields carry
  their own `export`, so a struct's name and representation are exported
  separately. Enum variants take the enum's visibility.
- Re-export mirrors import: `from "//lib/money/cents.buri" export { Cents, add };`.
  There's no `export *`.
- `impl` and `derive` are never exported.
- Declaration order doesn't matter, and mutual recursion needs no forward
  declarations. Circular imports are an error.
- Outside a package, you import its surface as a module: `"core/list"`,
  `"//lib/money"`, `"//lib/money/testing"`; its own suite does the same. Inside
  the package, you name a file: `"//lib/money/cents.buri"`,
  `"//cmd/app/main.buri"`. Dropping the file name there is
  `import-path-without-a-file`; naming a file in another package is
  `internal-import`.
- A `testing` directory segment makes a module test-only. Only a platform's
  `platform.buri` may import `platform/host`.
- Effects live in `platform/effect`, their test implementations in
  `platform/effect/testing`. `core/effect`, `core/host` and `core/host/testing`
  are retired.

## Declarations

```buri
type UserId = Str;                          // transparent alias
struct Meters(export F64);                  // tuple struct; `;`-terminated
struct User { export id: UserId, secret: Str }   // record struct; no `;`

enum Tree<T> {
    Leaf,
    Node(Tree<T>, T, Tree<T>),
}

impl Meters {
    export fn doubled(self): Meters { Meters(self.0 * 2.0) }
}

derive Equal, Ordered, Show for Meters;
```

- Every top-level `fn` **must** declare its parameter and return types. Lambdas
  and `let` bindings are inferred (Hindley–Milner).
- No overloading, no default arguments, no variadics.
- Every `impl` function takes `self` first, and no other function may. An
  `impl` may appear only in its type's module.
- Struct update: `User { ..u, secret: "new" }`. Field shorthand: `User { id }`.

## Expressions

`if`, `match` and blocks are all expressions. `let` is the only statement,
except in a test source, where any `()`-typed expression — a call, `match`,
`if`, or block — can be a statement ending in `;`.

```buri
let hypotenuse = {
    let a2 = a * a;
    let b2 = b * b;
    math.squareRoot(a2 + b2)
};

let label = if (n < 0) { "negative" } else if (n == 0) { "zero" } else { "positive" };

let describe = match (shape) {
    .Circle(r) if r > 100.0 => "huge circle",
    .Circle(_) => "circle",
    .Rect { width: w, height: h } => if (w == h) { "square" } else { "rect" },
    .Empty => "nothing",
};

let inc = fn(x) => x + 1;
let sum = xs.fold(fn(acc, x) => acc + x, 0);
```

- Parenthesise a `match` scrutinee. Arms need a comma, even after a brace body.
  The first matching arm wins, guards don't count toward exhaustiveness, and a
  non-exhaustive or unreachable arm is a compile error.
- Comparison is **non-associative**: `a < b < c` is a parse error.
- Bitwise binds tighter than comparison: `a & MASK == 0` is `(a & MASK) == 0`.
- No `<<`/`>>`; use `bits.shiftLeft(x, n)` and `bits.shiftRight(x, n)`.
- A lambda body extends as far right as possible, so `2 * fn(x) => x` is a
  parse error — parenthesise it.
- Shadowing is allowed, even twice in one block.

### `?` and defaults

```buri
fn loadPort<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Int, ConfigError> {
    let text = fs.readText(ctx, at)?;         // Err(e) => return Err(e)
    let cfg = parseConfig(text)?;
    .Ok(cfg.port.withDefault(8080))
}
```

`?` on a `Result<T, E>` needs the enclosing function to return `Result<_, E>`.
Errors aren't converted for you; use `result.mapErr`. There's no coalescing
operator. `withDefault` works on `Option<T>` and `Result<T, E>`, and its
fallback is an argument, so it's always evaluated. Write a `match` when the
fallback must not run.

## Patterns

| Form | Example |
|---|---|
| Wildcard / binding | `_`, `n` |
| Named subpattern | `whole @ .Circle(r)` |
| Literal | `0`, `-1`, `"yes"`, `'x'`, `true` |
| Qualified / inferred variant | `Option.Some(x)`, `.Empty` |
| Struct | `User { id, name: n }`, `User { id, .. }` |
| Tuple struct / tuple / array | `Meters(m)`, `(a, b)`, `[first, ..rest]` |
| Or | `.Circle(_) \| .Empty` |

Without `..`, a struct pattern must name every field. Array rest binds only at
the end. Or-alternatives must bind the same names at the same types. `let`
patterns must be irrefutable.

## Evaluation

Strict, in a fixed order: `let` bindings top to bottom, call arguments left to
right, binary operands left to right except `&&` and `||`.

Lambdas capture by value. The one exception is the effect capture rule in the
`buri-types` skill.

## Strings and numbers

- `"a ${b} c"` is a `Template`, not a `Str`, and building one allocates
  nothing, so `io.println(ctx, "hi ${name}")` needs only `Stdout`.
  `str.format(ctx, "...")` turns one into a `Str`, which allocates.
- `Str` widens to `Template` in argument position — the only implicit
  conversion in the language.
- Hole types are `Int`/`Float` (any width), `Bool`, `Char`, `Str`.
- Integer literals default to `Int` (= `I64`) and floats to `Float` (= `F64`)
  only when nothing else pins them. No literal suffixes; a literal that doesn't
  fit its type is a compile error.
- Integer `/` truncates toward zero and `%` takes the dividend's sign. Division
  by zero aborts, and overflow is **undefined behaviour**, so use
  `checkedAdd`/`wrappingAdd`/`saturatingAdd` or `core/bits` when it matters.
- Float `==` is an equivalence relation, so `NaN == NaN` is true. `<` and
  friends stay IEEE-754, so they disagree with `==` at `NaN`.

## Conventions

`UpperCamelCase` types and variants, `lowerCamelCase` functions and bindings,
`SCREAMING_SNAKE_CASE` constants, `lowercase` modules — by convention, not
grammar. `buri format` has one layout and no options: four-space indent, sorted
leading imports, one struct field or enum variant per line.

**The third slash publishes.** `//` and nesting `/* */` are never rendered.
`///` above a declaration is a **documentation comment**, and `//!` at the top
of a file documents the module; `buri docs <module>`, editor hover and
`buri docs search` read them. Put what a *caller* needs in `///` above the
`export fn`, `struct`, `enum`, field or variant, and keep `//` for the body's
reader. A `//!` below the first item is `module-doc-not-first`.

## When something does not compile

Every diagnostic ends with a bracketed code such as `[unsatisfied-bound]`.
`buri docs error <code>` explains it with a program that provokes it, and
`buri docs error` lists them all. Lint findings work the same way:
`buri docs lint <code>`.
