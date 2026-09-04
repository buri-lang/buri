---
name: buri-language
description: Use when reading or writing Buri (.buri) source — syntax, immutability, expressions, patterns, modules, and the rules that catch out anyone arriving from Rust, TypeScript, or Go.
---

# Buri: the language

Strict, purely functional, statically typed. TypeScript-shaped syntax,
Rust-shaped data declarations, Roc-shaped ideas about platforms and effects.

The normative text ships in the binary: `buri docs language/lexical`,
`language/modules`, `language/types`, `language/expressions`, `language/patterns`,
`language/evaluation`, `language/functions`, `language/effects` and
`language/programs`. `buri docs search <words>` looks in every page at once — in the prose, not only the names, so a question like `buri docs search compare ints` works — and prints each hit as the command that reads it.

**Explore the library before writing a helper.** Bare `buri docs` lists every
`core/*` and `ui/*` module in one screen; `buri docs core/str` renders one from
the source the compiler checked, and `buri docs core/str.padStart` renders a
single item. Comparators, hex, base64, varints, grouping, checksums, dates,
argument parsing and a CLI are already there, and a hand-rolled one is a wrong
answer that compiles.

## The twelve things that will trip you up

1. **No mutation.** Every binding is final. No assignment operator, no `mut`,
   no interior mutability, no references, no borrow checker, no lifetimes.
2. **No loops.** Iteration is recursion — implementations must eliminate tail
   calls, including mutual ones — or a fold.
3. **No `return`.** Postfix `?` is the only early exit in the language.
4. **No `null` and no `undefined`.** Absence is `Option<T>`, and indexing an
   array yields `Option<T>` rather than `T`.
5. **`else` is mandatory**, conditions are parenthesised, and there is no
   truthiness: `if (n < 0) { ... } else { ... }`.
6. **No implicit numeric conversion of any kind.** `1.0 + 1` is an error, and
   so is `I32 + I64`. Convert with a method: `a.toI64()`.
7. **Effects arrive as a parameter named `ctx`, and are *performed* by handing
   it to a function.** A function with no `ctx` and no effect-carrying `self`
   cannot touch the world; and `ctx.println("hi")` is not how you print —
   `io.println(ctx, "hi")` is, because an effect method is not callable on the
   value that carries it (`effect-method-call`). `core/io`, `core/fs`,
   `core/env`, `core/time`, `core/random`, `core/alloc`, `core/net/http`,
   `core/net/server`, `core/proc`, `core/tasks` and `ui/signal` are the doors.
   The filesystem is **two** effects, `FsRead` and `FsWrite`, both declared in
   `core/fs` rather than `core/effect`, and every function there takes a `Path`
   from `core/path` rather than a `Str`. See the `buri-types` skill.
8. **A bare identifier in a pattern is always a binding.** `None` binds a
   variable; write `.None` or `Option.None` to match the variant.
9. **`Result` may not be discarded.** `let _ = someResult()` is a compile
   error (`result-discarded`), and so is a `_` further down the pattern —
   `let (n, _) = (1, someResult())` — and so is leaving the call standing as a
   statement. Consume it with `?`, `match` or `.withDefault(...)`, or drop it
   on purpose with `.ignore()`, which `buri lint` reports as
   `discarded-result`. **A print answers one too**, so a line a program does
   not care about is `let _ = io.println(ctx, "hi").ignore();` — and a program
   that does care answers instead, with `mapErr` to carry the `IoError` into
   its own error type.
10. **No relative imports.** A module path is `core/...`, `ui/...`, or
    `//...` from the repository root, and means the same module everywhere.
11. **Methods live in an `impl` block in their type's own module**, and are
    reached through the receiver's type rather than through scope — so they
    need no import, and you cannot add one to somebody else's type.
12. **There is no `panic`, no `unreachable`, no bottom type.** Every case is
    handled. Division by zero and stack exhaustion *abort*; nothing catches.

## A whole program

```buri
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
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

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
    };

    let shapes = [Shape.Circle(1.0), Shape.Rect { width: 2.0, height: 3.0 }];
    let total = shapes.map(ctx, fn(s) => s.area()).sumFloat();
    let _ = io.println(ctx, "total area: ${total}").ignore();
    .Ok(())
}
```

`main` takes no parameters, returns `Result<(), Str>`, and is the only place in
a program where `core/host` may be imported and a context built. `.Ok(())`
exits 0; `.Err(msg)` prints `msg` on stderr and exits 1.

**The effect names have to be imported.** `context { Alloc: host.alloc }` with
no `from "core/effect" import { Alloc };` above it fails with
`not-an-effect` — a common first mistake.

## Modules

The path comes first, before the specifier list, so an editor can complete the
names. Import declarations end in `;`.

```buri
from "core/list" import { map, filter };
from "core/list" import { map as listMap };
from "core/list" import * as list;
from "//lib/money" import { Cents };
```

- `from "core/list" import *;` is not derivable — the only wildcard form is
  `* as <name>`. Every unqualified name in a module is written in that module.
- A declaration is module-private unless prefixed `export`. Struct fields carry
  their own `export`, so a struct's name and its representation are exported
  separately. An enum's variants take the enum's visibility and write no
  `export` of their own.
- Re-export mirrors import: `from "//lib/money/cents.buri" export { Cents, add };`.
  There is no `export *`.
- `impl` and `derive` are never exported.
- Declaration order does not matter; mutual recursion needs no forward
  declarations. Circular imports are an error.
- A surface is named as a module — `"core/list"`, `"//lib/money"`,
  `"//lib/money/testing"` — by anyone allowed to name it, its own suite
  included. Everything else is a file, and only its own package may name it:
  `"//lib/money/cents.buri"`, `"//cmd/app/main.buri"`. A path with the file
  name left off is `import-path-without-a-file`; one that leaves the package
  and names a file inside is `internal-import`.
- A `testing` directory segment makes a module test-only.
  `core/host` is importable only from the module exporting `main`.

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

derive Eq, Ord, Show for Meters;
```

- The return type is **required** on every top-level `fn`; parameter types are
  required. Lambdas and `let` bindings are inferred (Hindley–Milner).
- No overloading, no default arguments, no variadics.
- Every function inside an `impl` takes `self` first; no function outside one
  may. An `impl` may appear only in the module declaring its type.
- Struct update: `User { ..u, secret: "new" }`. Field shorthand: `User { id }`.

## Expressions

Everything produces a value — `if`, `match`, blocks. `let` is the only
statement, and there are no expression statements outside a test source. Inside
one, any expression of type `()` is a statement — a call, a `match`, an `if`, a
block — and each ends with `;`.

```buri
let hypotenuse = {
    let a2 = a * a;
    let b2 = b * b;
    math.sqrt(a2 + b2)
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

- A `match` scrutinee is parenthesised, arms are comma-separated (the comma is
  required even after a brace-terminated body), the first matching arm wins,
  guards do not count toward exhaustiveness, and a non-exhaustive or
  unreachable arm is a compile error.
- Comparison is **non-associative**: `a < b < c` is a parse error.
- Bitwise binds tighter than comparison, so `a & MASK == 0` is `(a & MASK) == 0`.
- There is no `<<`/`>>`; use `bits.shl(x, n)` and `bits.shr(x, n)`.
- A lambda body extends as far right as possible, so `2 * fn(x) => x` is a
  parse error — parenthesise it.
- Shadowing is allowed, including twice in one block.

### `?` and defaults

```buri
fn loadPort<C: Alloc + FsRead>(ctx: C, at: Path): Result<Int, ConfigError> {
    let text = fs.readText(ctx, at)?;         // Err(e) => return Err(e)
    let cfg = parseConfig(text)?;
    .Ok(cfg.port.withDefault(8080))
}
```

`?` on a `Result<T, E>` requires the enclosing function to return
`Result<_, E>`; there is no automatic error conversion — use `result.mapErr`.
There is no coalescing operator: `withDefault` is a method on both `Option<T>`
and `Result<T, E>`. It takes the fallback as an argument, so it is evaluated
either way; write the `match` out where the fallback must not run.

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

Without `..`, a struct pattern must mention every field. Array rest binds only
at the end. Or-alternatives must bind the same names at the same types. `let`
patterns must be irrefutable.

## Evaluation

Strict, with a fully specified order: `let` bindings top to bottom, call
arguments left to right, binary operands left to right except `&&` and `||`.
That is what makes effect sequencing meaningful, since effects are ordinary
calls rather than a monad.

Values are immutable, so lambdas capture by value and capture is unobservable
— except for the effect capture rule in the `buri-types` skill.

## Strings and numbers

- `"a ${b} c"` has type `Template`, not `Str`, and constructing one allocates
  nothing — which is why `io.println(ctx, "hi ${name}")` needs only `Stdout`.
  `str.format(ctx, "...")` turns one into a `Str`, and that allocates.
- `Str` widens implicitly to `Template` in argument position. It is the only
  implicit conversion in the language.
- Hole types are `Int`/`Float` (any width), `Bool`, `Char`, `Str`.
- Integer literals default to `Int` (= `I64`) and floats to `Float` (= `F64`)
  only when nothing else pins them. There are no literal suffixes; a literal
  that does not fit its type is a compile error.
- Integer `/` truncates toward zero, `%` takes the sign of the dividend,
  division by zero aborts, and overflow is **undefined behaviour** — use
  `checkedAdd`/`wrappingAdd`/`saturatingAdd` or `core/bits` when it matters.
- `==` on floats is an equivalence relation: `NaN == NaN` is true. `<` and
  friends stay IEEE-754, so they disagree with `==` at `NaN`.

## Conventions

`UpperCamelCase` types and variants, `lowerCamelCase` functions and bindings,
`SCREAMING_SNAKE_CASE` constants, `lowercase` modules. None of it is enforced
by the grammar. `buri format` is the one canonical layout — four-space indent,
sorted leading imports, a struct's fields and an enum's variants one to a line
however short they are, no options.

**The third slash is what publishes.** `//` runs to the end of the line and
`/* */` nests; neither is ever rendered. `///` above a declaration is a
**documentation comment**, and `//!` at the top of a file documents the module —
those two are what `buri docs <module>`, editor hover and `buri docs search`
read, and their fenced examples are compiled by the test suite like any other.
So a note a *caller* needs gets `///`, above the `export fn`, `struct`, `enum`,
field or variant it describes; `//` is for the reader of the body. A `//!` lower
down the file is `module-doc-not-first` — it attaches upward, to the module, and
is legal only above the first item.

## When something does not compile

Every diagnostic ends with a code in brackets, such as
`[unsatisfied-bound]`. `buri docs error <code>` explains that code and shows a
program that provokes it; `buri docs error` lists them all. A `buri lint`
finding carries a code the same way, looked up with `buri docs lint <code>`.
