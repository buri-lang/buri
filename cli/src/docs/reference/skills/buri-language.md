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
`language/programs`. `buri docs search <words>` searches every page at once, and
each hit prints as the command that reads it.

**Explore the library before writing a helper.** Bare `buri docs` lists every
`core/*` and `ui/*` module. `buri docs core/str` renders one module, and
`buri docs core/str.padStart` a single item. Comparators, hex, base64, varints,
grouping, checksums, dates, argument parsing and a CLI are already there.
Hand-roll one and you get a wrong answer that compiles.

## The twelve things that will trip you up

- **No mutation.** Every binding is final. No assignment operator, no `mut`,
  no interior mutability, no references, no borrow checker, no lifetimes.
- **No loops.** Iterate with recursion or a fold. Implementations must
  eliminate tail calls, mutual ones included.
- **No `return`.** Postfix `?` is the only early exit in the language.
- **No `null` and no `undefined`.** Absence is `Option<T>`, and indexing an
  array yields `Option<T>` rather than `T`.
- **`else` is mandatory**, conditions are parenthesised, and there is no
  truthiness: `if (n < 0) { ... } else { ... }`.
- **No implicit numeric conversion of any kind.** `1.0 + 1` is an error, and
  so is `I32 + I64`. Convert with a method: `a.toI64()`.
- **Effects arrive as a parameter named `ctx`. You perform one by handing
  `ctx` to a function.** A function with no `ctx` and no effect-carrying `self`
  cannot touch the world. And `ctx.println("hi")` is not how you print:
  `io.println(ctx, "hi")` is, because you cannot call an effect method on the
  value carrying it (`effect-method-call`). The doors are `core/io`, `core/fs`,
  `core/env`, `core/time`, `core/random`, `core/alloc`, `core/net/http`,
  `core/net/server`, `core/process`, `core/tasks` and `ui/signal`. The
  filesystem is **two** effects, `FsRead` and `FsWrite`. `core/fs` declares
  both, not `core/effect`, and every function there takes a `Path` from
  `core/path` rather than a `Str`. See the `buri-types` skill.
- **A bare identifier in a pattern is always a binding.** `None` binds a
  variable; write `.None` or `Option.None` to match the variant.
- **You may not discard a `Result`.** `let _ = someResult()` is a compile
  error (`result-discarded`). So is a `_` further down the pattern, as in
  `let (n, _) = (1, someResult())`, and so is leaving the call standing as a
  statement. Consume it with `?`, `match` or `.withDefault(...)`, or drop it
  on purpose with `.ignore()`, which `buri lint` reports as
  `discarded-result`. **A print returns one too.** A line the program does not
  care about reads `let _ = io.println(ctx, "hi").ignore();`.
- **No relative imports.** A module path is `core/...`, `ui/...`, `std/...`, or
  `//...` from the repository root, and means the same module everywhere.
- **Methods live in an `impl` block in their type's own module.** You reach
  them through the receiver's type rather than through scope, so they need no
  import, and you cannot add one to somebody else's type.
- **There is no `panic`, no `unreachable`, no bottom type.** Every case is
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

`main` takes no parameters and returns `Result<(), Str>`. It is an *entry*, and
only an entry may build a context; only `main.buri` may import `core/host`.
`.Ok(())` exits 0. `.Err(msg)` prints `msg` on stderr and exits 1. A build file's
`outputs` may name a second entry — `{ platform: CLOUDFLARE_WORKER, entry:
"fetch" }` enters at `fn fetch(request: Request): Response`.

**Import the effect names.** `context { Alloc: host.alloc }` without
`from "core/effect" import { Alloc };` above it fails with `not-an-effect`.

## Modules

The path comes first, before the specifier list. Import declarations end
in `;`.

```buri
from "core/list" import { map, filter };
from "core/list" import { map as listMap };
from "core/list" import * as list;
from "//lib/money" import { Cents };
```

- The grammar has no `from "core/list" import *;`. The only wildcard form is
  `* as <name>`.
- A declaration stays module-private unless you prefix it with `export`.
  Struct fields carry their own `export`, so you export a struct's name and its
  representation separately. An enum's variants take the enum's visibility and
  write no `export` of their own.
- Re-export mirrors import: `from "//lib/money/cents.buri" export { Cents, add };`.
  There is no `export *`.
- `impl` and `derive` are never exported.
- Declaration order does not matter, and mutual recursion needs no forward
  declarations. Circular imports are an error.
- Anyone allowed to name a surface names it as a module: `"core/list"`,
  `"//lib/money"`, `"//lib/money/testing"`, its own suite included. Everything
  else is a file, and only its own package may name it, as in
  `"//lib/money/cents.buri"` or `"//cmd/app/main.buri"`. Leave the file name
  off and you get `import-path-without-a-file`. Leave the package and name a
  file inside it and you get `internal-import`.
- A `testing` directory segment makes a module test-only. Only the module
  exporting `main` may import `core/host`.

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

- Every top-level `fn` **must** write its return type, and its parameter
  types. The compiler infers lambdas and `let` bindings (Hindley–Milner).
- No overloading, no default arguments, no variadics.
- Every function inside an `impl` takes `self` first, and no function outside
  one may. An `impl` may appear only in the module declaring its type.
- Struct update: `User { ..u, secret: "new" }`. Field shorthand: `User { id }`.

## Expressions

Everything produces a value: `if`, `match`, blocks. `let` is the only
statement, and expression statements exist only in a test source. There, any
expression of type `()` is a statement — a call, a `match`, an `if`, a block —
and each ends with `;`.

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

- Parenthesise a `match` scrutinee. Arms are comma-separated, and the comma is
  required even after a brace-terminated body. The first matching arm wins,
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

`?` on a `Result<T, E>` needs the enclosing function to return `Result<_, E>`.
Nothing converts the error for you, so reach for `result.mapErr`. There is no
coalescing operator either. `withDefault` is a method on both `Option<T>` and
`Result<T, E>`, and it takes the fallback as an argument, so it runs either
way. Write the `match` out where the fallback must not run.

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

Values are immutable, so lambdas capture by value. The one exception is the
effect capture rule in the `buri-types` skill.

## Strings and numbers

- `"a ${b} c"` has type `Template`, not `Str`, and building one allocates
  nothing, which is why `io.println(ctx, "hi ${name}")` needs only `Stdout`.
  `str.format(ctx, "...")` turns one into a `Str`, and that allocates.
- `Str` widens implicitly to `Template` in argument position, the only implicit
  conversion in the language.
- Hole types are `Int`/`Float` (any width), `Bool`, `Char`, `Str`.
- Integer literals default to `Int` (= `I64`) and floats to `Float` (= `F64`)
  only when nothing else pins them. There are no literal suffixes, and a
  literal that does not fit its type is a compile error.
- Integer `/` truncates toward zero and `%` takes the sign of the dividend.
  Division by zero aborts, and overflow is **undefined behaviour**, so use
  `checkedAdd`/`wrappingAdd`/`saturatingAdd` or `core/bits` when it matters.
- `==` on floats is an equivalence relation, so `NaN == NaN` is true. `<` and
  friends stay IEEE-754, so they disagree with `==` at `NaN`.

## Conventions

`UpperCamelCase` types and variants, `lowerCamelCase` functions and bindings,
`SCREAMING_SNAKE_CASE` constants, `lowercase` modules. The grammar enforces
none of it. `buri format` gives you the one canonical layout: four-space
indent, sorted leading imports, a struct's fields and an enum's variants one to
a line however short they are, no options.

**The third slash is what publishes.** `//` runs to the end of the line and
`/* */` nests, and neither is ever rendered. `///` above a declaration is a
**documentation comment**, and `//!` at the top of a file documents the module.
`buri docs <module>`, editor hover and `buri docs search` read those two. So put
a note a *caller* needs in a `///` above the `export fn`, `struct`, `enum`,
field or variant it describes, and keep `//` for the reader of the body. A `//!`
lower down the file is `module-doc-not-first`: it is legal only above the first
item.

## When something does not compile

Every diagnostic ends with a code in brackets, such as `[unsatisfied-bound]`.
`buri docs error <code>` explains one and shows a program that provokes it, and
`buri docs error` lists them all. A `buri lint` finding carries one too:
`buri docs lint <code>`.
