## 6. Expressions

Everything that produces a value is an expression, including `if`, `match`, and
blocks. There is no statement/expression split beyond `let`.

### 6.1 Precedence

Lowest to highest:

| Level | Operators | Associativity |
|---|---|---|
| 0 | `fn(...) => e` | top-level only (never a sub-operand) |
| 1 | `\|\|` | left |
| 2 | `&&` | left |
| 3 | `==` `!=` `<` `<=` `>` `>=` | **non-associative** |
| 4 | `\|` | left |
| 5 | `^` | left |
| 6 | `&` | left |
| 7 | `+` `-` | left |
| 8 | `*` `/` `%` | left |
| 9 | `-` `!` `~` (prefix) | right |
| 10 | `.f` `.0` `(args)` `[i]` `?` `<T>` `{ ... }` | left |

Comparison is non-associative: `a < b < c` is a parse error.

Bitwise operators bind tighter than comparison (as in Rust), so `a & MASK == 0`
means `(a & MASK) == 0`.

There is no `<<` or `>>`. Use `bits.shl(x, n)` and `bits.shr(x, n)`. See
`design/grammar-rationale.md` 12.6.

### 6.2 Arithmetic

`+ - * / %` desugar to the operator traits of Section 5.12.4. On the built-in
numeric types they take two operands of the *same* type and produce that type.
**There is no implicit promotion of any kind** — not integer promotion, not
int-to-float, not narrow-to-wide. `a: I32 + b: I64` is an error, and so is
`1.0 + 1`.

Integer `/` truncates toward zero; `%` takes the sign of the dividend, so
`a == (a / b) * b + (a % b)` holds for every non-zero `b`.

*Integer* division by zero **aborts**: there is no answer to give and no
`Result` in the signature to say so. Float division by zero does not — IEEE-754
has an answer for it, so `1.0 / 0.0` is `+inf`, `-1.0 / 0.0` is `-inf` and
`0.0 / 0.0` is `NaN`.

Overflow and underflow of an integer operation are **undefined behaviour**. The
program is wrong; the language does not say what it produces, and it is not
wrapping by default. What it does in practice depends on the backend: a
**native** one is two's complement at each type's own width, so overflow shows up
as a wrapped value, while on the **JavaScript** backend a width at 64 bits or
above is a `BigInt`, which has no width to overflow at, so overflow shows up as
an answer larger than the type. Neither is promised.

Code that needs a defined answer at the boundary says which one it wants:
`Checked` answers `.None`, `Wrapping` answers the low bits, and `Saturating`
answers the bound. Each of those means the same thing on every backend.

Floating point follows IEEE-754, with one deliberate exception: **`==` is an
equivalence relation**. It compares numerically, so `-0.0 == 0.0` is true and
`0.1 + 0.2 != 0.3`, and it is reflexive, so **`NaN == NaN` is true** — every
`NaN` equals every other `NaN` regardless of sign or payload. IEEE-754 says the
opposite, and the trade is deliberate: everything built on `==` — a `Map` key, a
`Set` member, `list.contains`, `derive Eq` — quietly requires an equivalence
relation.

The **ordering** operators are unchanged and remain IEEE-754's: `NaN < x`,
`NaN <= x`, `NaN > x` and `NaN >= x` are all false, in both operand orders, and
so is `NaN < NaN`. So `a <= b && b <= a` does not imply `a == b`, and `!(a < b)
&& !(a > b)` does not imply it either. `math.isNan(x)` is how a program asks the
question `x != x` used to answer.

A payload is not part of a `NaN`'s value, so nothing preserves one.
`bytes.f64FromBytes` answers the canonical quiet NaN for every NaN pattern, on
every backend, and round-trips back to the same eight bytes.

Rendering a float gives the shortest decimal that round-trips. That is a promise
about digits and not only about values: `1.0 / 3.0` prints the same characters on
every backend.

#### 6.2.1 Conversions

Numeric conversions are explicit **methods** rather than operators:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let a: I32 = 7;
let b = a.toI64();                       // I64 — always exact
let c: Result<I32, RangeError> = big.toI32();      // may not fit
let d = big.wrapToU8();                  // modular, for checksums and wire formats
let ratio = hits.toF64() / total.toF64();
```

Three families, distinguished by what happens when the value does not fit:

| Shape | Returns | When it does not fit |
|---|---|---|
| `x.toT()` where every `T` value fits | `T` | cannot happen |
| `x.toT()` where it might not | `Result<T, RangeError>` | `.Err` |
| `x.wrapToT()` | `T` | wraps (integers) or rounds (floats) |

Each source-and-target pair decides its own return type, so `i32.toI64()` yields
`I64` while `i64.toI32()` yields `Result<I32, RangeError>`. The type says whether
a conversion can fail.

`I64 → F64` is lossy above 2^53, so strictly it belongs in the second family. But
converting a count to a float is too common to route through a `Result`, so every
integer type defines `toF64` as an exact-to-53-bits conversion that rounds beyond
that. That bound is the float's rather than the backend's, so `toF64` rounds
identically everywhere.

`core/num` holds one of these functions per source-and-target pair. `as` appears
only in import specifiers (`design/grammar-rationale.md` 12.5).

`Char` and `U32` convert the same way: `c.toU32()` is exact, `n.toChar()` yields
`Result<Char, RangeError>`.

#### 6.2.2 Checked and wrapping arithmetic

The default `+` leaves overflow undefined. The alternatives are trait methods, so
you spell them out where you use them:

```buri
trait Checked {
    fn checkedAdd(self, rhs: Self): Option<Self>;
    fn checkedSub(self, rhs: Self): Option<Self>;
    fn checkedMul(self, rhs: Self): Option<Self>;
    fn checkedDiv(self, rhs: Self): Option<Self>;
}

trait Wrapping {
    fn wrappingAdd(self, rhs: Self): Self;
    fn wrappingSub(self, rhs: Self): Self;
    fn wrappingMul(self, rhs: Self): Self;
}

trait Saturating {
    fn saturatingAdd(self, rhs: Self): Self;
    fn saturatingSub(self, rhs: Self): Self;
    fn saturatingMul(self, rhs: Self): Self;
}

trait Bounded {
    fn minValue(): Self;
    fn maxValue(): Self;
}
```

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let safe = a.checkedAdd(b).withDefault(0);
let hash = seed.wrappingMul(31).wrappingAdd(byte);
let ceiling = num.maxValue<U8>();
```

Every built-in integer type satisfies all four; the float types satisfy
`Bounded` only.

A `Checked` method answers `.None` on two's-complement overflow and nothing else;
`.Some(v)` means `v` is the exact true result. `Bounded` and `Saturating` report
the type's own bounds on every backend.

### 6.3 Blocks

A block is zero or more `let` bindings followed by a result expression — the
`Block` production of [`grammar.ebnf`](./cli/src/docs/grammar.ebnf). The grammar
makes the result expression optional, but a block without one has no value, and
the checker reports that as an error wherever a block may stand.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let hypotenuse = {
  let a2 = a * a;
  let b2 = b * b;
  math.sqrt(a2 + b2)
};
```

Buri evaluates `let` bindings **strictly, in source order** (Section 8.2). Each
binding is in scope for the rest of the block. You may shadow, both in nested
scopes and within a single block:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let name = str.trim(raw);
let name = str.toLower(ctx, name);   // legal; the earlier `name` is inaccessible
```

The pattern in a `let` must be irrefutable. Use `match` for anything else.

### 6.4 `if`

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let label = if (n < 0) { "negative" } else if (n == 0) { "zero" } else { "positive" };
```

- The condition must be parenthesized and must have type `Bool`. There is no
  truthiness.
- Both branches are blocks, and `else` is **mandatory** (`design/grammar-rationale.md` 12.10): there is
  nothing sensible for a missing branch to produce in a language where `if` is an
  expression.
- Both branches must have the same type.

### 6.5 `match`

Section 7 has the pattern forms an arm may use.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let describe = match (shape) {
  .Circle(r) if r > 100.0 => "huge circle",
  .Circle(_) => "circle",
  .Rect { width: w, height: h } => if (w == h) { "square" } else { "rect" },
  .Empty => "nothing",
};
```

- The scrutinee must be parenthesized.
- Arms are **comma-separated**, with an optional trailing comma (`design/grammar-rationale.md` 12.12).
  The comma is required even after a brace-terminated arm body.
- Arms are tried in order; the first matching arm wins.
- A guard (`if expr`) may follow a pattern. Guards do not contribute to
  exhaustiveness.
- The match must be **exhaustive**. A non-exhaustive match is a compile error
  that names a missing case.

### 6.6 Calls and lambdas

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
fn add(a: Int, b: Int): Int { a + b }

let inc = fn(x) => x + 1;
let addTyped = fn(a: Int, b: Int): Int => a + b;
let sum = xs.fold(fn(acc, x) => acc + x, 0);
```

Lambdas begin with `fn` so that `(x)` is never ambiguous with a parameter list.
You may omit parameter types and the return type where they are inferable.

A lambda body extends as far right as possible, so a lambda cannot appear as a
bare operand of a binary operator (`design/grammar-rationale.md` 12.11). `2 * fn(x) => x` is a parse
error; write `2 * (fn(x) => x)`.

Buri evaluates arguments left to right before the call (Section 8.2). There is no
built-in partial application; write a lambda.

### 6.7 Method calls

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
user.name          // struct field
pair.0             // tuple element
xs[i]              // Option<T>
list.map           // module member
sq.area()          // method call
```

All five are the same production — `PostfixExpr "." IDENT` — and name resolution
tells them apart, never parsing.

#### 6.7.1 Declaring a method

You declare a method **inside an `impl` block for its type**, and it takes `self`
as its first parameter. Both halves are required, and either without the other is
an error:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
export struct Square { height: Int, width: Int }

impl Square {
  export fn area(self): Int { self.height * self.width }

  export fn scaled(self, factor: Int): Square {
    Square { height: self.height * factor, width: self.width * factor }
  }
}

export fn combine(a: Square, b: Square): Square { ... }   // NOT a method
```

An `impl` with no `for` clause declares the type's own methods; the same block
with `for` declares trait conformance (Section 5.12.2).

`self` is a keyword and may appear only as the first parameter of a function
inside an `impl` block. A top-level `fn` that takes `self` is an error, and so is
a function inside an `impl` block that does not take `self`.

`self` is also the one parameter that writes no type: the `impl` head has already
written it. Writing one is the `self-with-a-type` error, which carries the edit
that deletes it.

An `impl` block may appear only in the module that declares its type, which keeps
method resolution a single lookup (Section 6.7.3). The block itself is never
`export`ed, and neither is a `derive`. A method inside one carries its own
`export`; a method supplied to a trait does not, because conformance travels
wherever the type does.

The generic parameters split between the two: those the self type mentions
belong to the `impl`, the rest to the method.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
impl<T> Option<T> {
  export fn map<U>(self, f: fn(T) => U): Option<U> { ... }
}
```

#### 6.7.2 Calling a method

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
x.f(a, b)      //  self = x, then a and b
x.f()          //  self = x
```

**The receiver comes first**, and a context parameter — when there is one —
comes second:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc };
impl<A> [A] {
  export fn map<B, C: Alloc>(self, ctx: C, f: fn(A) => B): [B];
}

xs.map(ctx, double)          // reads as: this list, in this world, mapped
```

That is the calling convention of Section 10.7, which the standard library
follows throughout.

**Methods need no import.**

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// main.buri
from "lib/square" import { Square }; // the type — not `area`, not `scaled`

fn describe(sq: Square): Int {
    sq.scaled(2).area() // both resolve with no further imports
}
```

You import the type; its methods come along. If the value arrives from elsewhere
and you never name its type, you need no import at all.

#### 6.7.3 Resolution

`x.f(...)` resolves in three steps, each a lookup rather than a search:

1. If `x`'s type has a field named `f`, this is field access. A field of function
   type is called as `(x.f)(...)`.
2. If `x`'s type is a concrete type, `f` must be a method declared by an `impl`
   block in that type's **defining module**. Inherent methods and methods
   supplied to a trait live in the same namespace and are found together; only
   the first kind is subject to `export`.
3. If `x`'s type is a type parameter, `f` must be declared by one of its
   **bounds** (Section 5.10). A bare parameter with no bounds has no methods.

**Steps 2 and 3 exclude an effect's methods.** You perform an effect by handing
the context to a function: `ctx.println(t)` is `io.println(ctx, t)`. Two layers
sit below that line and keep the method form: the standard library, which holds
those wrapper functions, and the body of an `impl` that supplies an effect.
Section 10.2 has the rule in full, and `effect-method-call` names the function to
call instead.

Each step is a single table lookup keyed by name and by one type. There is no
candidate set, no autoref, no autoderef, and no coherence check. Resolution does
need the receiver's type, so name resolution consults inference.

Where two bounds declare the same method name, the call is ambiguous.
Disambiguate it by calling the trait method as a function: `Ord.compare(x, y)`.

Defining modules:

| Type | Defining module |
|---|---|
| a `struct` or `enum` you declared | the module declaring it |
| `[T]` | `core/list` |
| `Str` | `core/str` |
| `Char` | `core/character` |
| `Bool` | `core/bool` |
| every integer and float type | `core/num` |
| `Option<T>` | `core/option` |
| `Result<T, E>` | `core/result` |
| tuples, function types, `Template` | none — no methods |

Type aliases are transparent, so `Int` and `I64` have the same methods.

Three consequences:

- **Methods are not extensible.** `impl Str { ... }` in your own module is an
  error. Write a free function and call it as one.
- **Methods are not values.** Neither `sq.area` nor a bare `area` is one; write
  `sq.area()`, or wrap the call in a lambda to pass it on.
- **The receiver's type must be known.** Inside `fn f<T>(x: T)`, `x.anything()`
  is an error unless `T` carries a bound that declares the method (Section 5.12).

### 6.8 `?` — error propagation

Postfix `?` unwraps a `Result` or `Option`, returning early from the enclosing
function on the failure case.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc };
# from "core/fs" import { FsRead, Path };

fn loadPort<C: Alloc + FsRead>(ctx: C, at: Path): Result<Int, ConfigError> {
    let text = fs.readText(ctx, at)?; // Err(e) => return Err(e)
    let cfg = parseConfig(text)?;
    .Ok(cfg.port)
}
```

- On `Result<T, E>`, the enclosing function must return `Result<_, E>`. There is
  no automatic error conversion in v0.3; map the error explicitly with
  `result.mapErr`.
- On `Option<T>`, the enclosing function must return `Option<_>`.

`?` is the only early exit in the language. There is no `return`.

Give a value the function is not propagating a default with `withDefault`, which
`Option<T>` and `Result<T, E>` both have: `cfg.port.withDefault(8080)`. It
evaluates its argument like any other call, so write a `match` when the default
must not run unless it is needed.

### 6.9 Aborting

There is no way to write that a branch cannot happen. `panic` and `unreachable`
are reserved (Section 3.4), so reaching for either gets named rather than quietly
accepted as an identifier. `crash` is an ordinary identifier. There is no bottom
type either, so nothing unifies with everything.

With no escape hatch you handle every case: unwrap an `Option` with `withDefault`
or match it, and make an impossible state a type that cannot represent it.

A program can still stop. Division by zero, a shift at or beyond the width of its
type, and stack exhaustion **abort**: the program ends with a message on stderr
and a non-zero exit status. Each is a case where the language has no answer to
give and no `Result` in the signature to give it through.

An abort is not an effect in the `ctx` sense — it can occur in a function with no
context parameter — but a context-free function cannot *observe* or *recover
from* one. There is no catch.

---
