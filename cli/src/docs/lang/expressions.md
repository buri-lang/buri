## 6. Expressions

Everything that produces a value is an expression, including `if`, `match`, and
blocks. There is no statement/expression split beyond `let`.

### 6.1 Precedence

Lowest to highest:

| Level | Operators | Associativity |
|---|---|---|
| 0 | `fn(...) => e` | top-level only (never a sub-operand) |
| 1 | `\|\|` | left |
| 2 | `??` | right |
| 3 | `&&` | left |
| 4 | `==` `!=` `<` `<=` `>` `>=` | **non-associative** |
| 5 | `\|` | left |
| 6 | `^` | left |
| 7 | `&` | left |
| 8 | `+` `-` | left |
| 9 | `*` `/` `%` | left |
| 10 | `-` `!` `~` (prefix) | right |
| 11 | `.f` `.0` `(args)` `[i]` `?` `<T>` `{ ... }` | left |

Comparison is non-associative: `a < b < c` is a parse error, not a bug waiting to
happen.

Bitwise operators bind tighter than comparison (as in Rust), so `a & MASK == 0`
means `(a & MASK) == 0`.

There is no `<<` or `>>`. Use `bits.shl(x, n)` and `bits.shr(x, n)`. See
Section 12.6.

### 6.2 Arithmetic

`+ - * / %` desugar to the operator traits of Section 5.12.4. On the built-in
numeric types they are defined on two operands of the *same* type and produce
that type. **There is no implicit promotion of any kind** — not integer
promotion, not int-to-float, not narrow-to-wide. `a: I32 + b: I64` is an error,
and so is `1.0 + 1`.

Integer `/` truncates toward zero; `%` takes the sign of the dividend, so
`a == (a / b) * b + (a % b)` holds for every non-zero `b`.

Division by zero **aborts**: there is no answer to give and no `Result` in the
signature to say so.

Overflow and underflow of an integer operation are **undefined behaviour**. The
program is wrong; the language does not say what it produces, and the backend
does not pay to find out. Overflow is not wrapping by default either: silent
wrapping is a correctness bug in almost all code and a deliberate technique in a
little of it, so the little of it says so out loud (below).

Undefined does not mean unbounded in practice, and what it means in practice
depends on the backend.

On a **native** backend every integer type is its own width and integer
arithmetic is two's complement, so the observable consequence of overflow is a
wrapped value. On the **JavaScript** backend a width up to 32 bits compiles to a
`number` and `I64` and `U64` compile to a `BigInt`, so those types hold their own
range exactly — and a `BigInt` has no width to overflow at, so the observable
consequence of overflow there is an answer larger than the type. Neither is promised and neither is a definition — a program that overflows
is wrong, and these are descriptions of two implementations rather than a
specification of one.

That the two differ is the reason overflow is undefined rather than
implementation-defined: a language that pinned one of them would be pinning a
backend. Code that needs a defined answer at the boundary says which one it
wants: `Checked` answers `.None`, `Wrapping` answers the low bits, and
`Saturating` answers the bound. Each of those means the same thing on every
backend.

Floating point follows IEEE-754, with one deliberate exception: **`==` is an
equivalence relation**. It compares numerically, so `-0.0 == 0.0` is true and
`0.1 + 0.2 != 0.3`, and it is reflexive, so **`NaN == NaN` is true** — every
`NaN` equals every other `NaN` regardless of sign or payload. IEEE-754 says the
opposite, and the trade is deliberate: an `==` that is not reflexive is not an
equivalence relation, and everything built on `==` quietly requires one. A value
put into a `Map` or a `Set` must be findable again; `list.contains(x)` must
answer `true` for an `x` taken out of the list; `derive Eq` on a struct must
make it equal to itself. Each of those is a bug at exactly one value if
`NaN != NaN`, and none of them can be fixed locally.

The **ordering** operators are unchanged and remain IEEE-754's: `NaN < x`,
`NaN <= x`, `NaN > x` and `NaN >= x` are all false, in both operand orders, and
so is `NaN < NaN`. So `a <= b && b <= a` does not imply `a == b`, and `!(a < b)
&& !(a > b)` does not imply it either. `math.isNan(x)` is how a program asks the
question `x != x` used to answer.

Rendering a float is the shortest decimal that
round-trips, and that is a promise about digits rather than only about values:
`1.0 / 3.0` prints the same characters on every backend.

#### 6.2.1 Conversions

Numeric conversions are explicit, and they are **ordinary methods** rather than
operators:

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

The return type is decided per source-and-target pair, so `i32.toI64()` yields
`I64` while `i64.toI32()` yields `Result<I32, RangeError>`. Whether a conversion
can fail is visible in the type rather than in the choice of operator.

`I64 → F64` is lossy above 2^53, so strictly it belongs in the second family —
but converting a count to a float is too common to route through a `Result`, so
`toF64` is defined on every integer type as an exact-to-53-bits conversion that
rounds beyond that, documented as such. This is the one place the language
prefers ergonomics to ceremony, and it is called out rather than hidden. That
bound is the float's rather than the backend's, so `toF64` rounds identically
everywhere.

Earlier drafts used three cast operators (`as`, `as?`, `as%`). They are gone,
because a method resolved by its receiver's type is the same lookup for none of
the cost (Section 12.5), and `as` now appears only in import specifiers.

There are a lot of these functions in `core/num` — one per source-and-target
pair. They are mechanical, greppable, and each says in its return type what it
can do.

`Char` and `U32` convert the same way: `c.toU32()` is exact, `n.toChar()` yields
`Result<Char, RangeError>`.

#### 6.2.2 Checked and wrapping arithmetic

The default `+` leaves overflow undefined. The alternatives are trait methods,
so they are spelled out where they are used and are available on any type that
derives them:

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
let safe = a.checkedAdd(b) ?? 0;
let hash = seed.wrappingMul(31).wrappingAdd(byte);
let ceiling = num.maxValue<U8>();
```

Every built-in integer type satisfies all four; the float types satisfy
`Bounded` only.

A `Checked` method answers `.None` whenever it cannot hand back the true result:
outside the type's range, or above what the backend represents exactly. Every
backend now represents every integer type's whole range exactly, so the two
bounds coincide and `.None` means two's-complement overflow and nothing else.
`.Some(v)` means `v` is the exact true result.

`Bounded` and `Saturating` report the type's own bounds on every backend.

### 6.3 Blocks

A block is zero or more `let` bindings followed by a result expression — the
`Block` production of [`grammar.ebnf`](./cli/src/docs/grammar.ebnf). The result
expression is optional syntactically, but a block without one has no value, which
the checker reports as an error everywhere a block may stand.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let hypotenuse = {
  let a2 = a * a;
  let b2 = b * b;
  math.sqrt(a2 + b2)
};
```

`let` bindings are evaluated **strictly, in source order** (Section 8.2). Each
binding is in scope for the remainder of the block. Shadowing is permitted, both
in nested scopes and within a single block:

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
- Both branches are blocks, and `else` is **mandatory** (Section 12.10): there is
  nothing sensible for a missing branch to produce in a language where `if` is an
  expression.
- Both branches must have the same type.

### 6.5 `match`

The pattern forms an arm may use are Section 7.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let describe = match (shape) {
  .Circle(r) if r > 100.0 => "huge circle",
  .Circle(_) => "circle",
  .Rect { width: w, height: h } => if (w == h) { "square" } else { "rect" },
  .Empty => "nothing",
};
```

- The scrutinee must be parenthesized.
- Arms are **comma-separated**, with an optional trailing comma (Section 12.12).
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
Parameter types and the return type may be omitted when inferable.

A lambda body extends as far right as possible, so a lambda cannot appear as a
bare operand of a binary operator (Section 12.11). `2 * fn(x) => x` is a parse
error; write `2 * (fn(x) => x)`.

Arguments are evaluated left to right before the call (Section 8.2). Partial
application is not built in; write a lambda.

### 6.7 Method calls

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
user.name          // struct field
pair.0             // tuple element
xs[i]              // Option<T>
list.map           // module member
sq.area()          // method call
```

All five are the same production — `PostfixExpr "." IDENT` — and they are told
apart during name resolution, never during parsing.

#### 6.7.1 Declaring a method

A method is declared **inside an `impl` block for its type**, and takes `self`
as its first parameter. Both halves are required, and each without the other is
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
with `for` declares trait conformance (Section 5.12.2). One keyword covers both,
because both answer the same question: what can you do with this type.

`self` is a keyword and may appear only as the first parameter of a function
inside an `impl` block. A top-level `fn` that takes `self` is an error — there
is no receiver type for it to attach to — and a function inside an `impl` block
that does not take `self` is an error too.

`self` is also the one parameter that writes no type. The `impl` head has
already written it, and a trait's signature means the implementing type, so an
annotation could only repeat what is above it or contradict it. Writing one is
the `self-with-a-type` error, which carries the edit that deletes it.

An `impl` block may appear only in the module that declares its type, which is
what keeps method resolution a single lookup (Section 6.7.3), and the block
itself — like a `derive` — is never `export`ed. A method inside one is `export`ed
on its own terms; a method supplied to a trait is not, because conformance
belongs to the type and travels wherever the type does.

The generic parameters split between the two: those the self type mentions
belong to the `impl`, the rest to the method.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
impl<T> Option<T> {
  export fn map<U>(self, f: fn(T) => U): Option<U> { ... }
}
```

An earlier draft made a function a method purely by taking `self`, with no
`impl` block. It read well in isolation and badly in a file: a type's operations
were scattered wherever someone happened to write them, and `area(sq)` and
`sq.area()` were two spellings of one call, so every method was also a free
function competing for a name in module scope. Requiring the block puts a type's
operations in one place and makes the method form the only one.

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

**Methods need no import.** That is the point of the feature:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
// main.buri
from "lib/square" import { Square };     // the type — not `area`, not `scaled`

fn describe(sq: Square): Int {
  sq.scaled(2).area()                    // both resolve with no further imports
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

Each step is a single table lookup keyed by name and by one type. There is no
candidate set, no autoref and no autoderef — Buri has no references — and no
coherence check, because conformance is nominal and a type has exactly one
defining module. Resolution does need the receiver's type, so name resolution
consults inference; a lookup rather than a search is the version of that cost
worth paying.

Where two bounds declare the same method name, the call is ambiguous and must be
disambiguated by calling the trait method as a function (`Ord.compare(x, y)`).

Defining modules:

| Type | Defining module |
|---|---|
| a `struct` or `enum` you declared | the module declaring it |
| `[T]` | `core/list` |
| `Str` | `core/str` |
| `Char` | `core/char` |
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
# from "core/effect" import { Alloc, Fs };
fn loadPort<C: Alloc + Fs>(ctx: C, path: Str): Result<Int, ConfigError> {
  let text = fs.readText(ctx, path)?;        // Err(e) => return Err(e)
  let cfg = parseConfig(text)?;
  .Ok(cfg.port)
}
```

- On `Result<T, E>`, the enclosing function must return `Result<_, E>`. There is
  no automatic error conversion in v0.3; map the error explicitly with
  `result.mapErr`.
- On `Option<T>`, the enclosing function must return `Option<_>`.

`?` is the only early exit in the language. There is no `return`.

Note that `??` is a single token, so `x??y` is coalescing, not try-then-coalesce.
Write `(x?) ?? y`.

### 6.9 `??` — default

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let port = cfg.port ?? 8080;             // Option<Int> ?? Int  -> Int
let name = lookup(id) ?? "anonymous";
```

Defined for `Option<T> ?? T` and `Result<T, E> ?? T`. It short-circuits (Section
8.2): the right operand is evaluated only when the left is `None` / `Err`. `??`
is right-associative, so `a ?? b ?? c` works.

### 6.10 Aborting

There is no way to write that a branch cannot happen. `panic` and `unreachable`
are reserved (Section 3.4), so reaching for either is named rather than silently
allowed as an identifier; `crash` is an ordinary identifier, because the concept
is gone rather than deferred. There is no bottom type either, so nothing unifies
with everything.

The reason is that such a claim is almost always wrong: a match arm the
programmer asserts is impossible is an arm the compiler was about to make them
handle, and "validated upstream" is a claim about code somewhere else that
nothing checks. Without an escape hatch, every case is handled — an `Option` is
unwrapped with `??` or matched, and an impossible state is a type that cannot
represent it.

A program can still stop. Division by zero, a shift at or beyond the width of its
type, and stack exhaustion **abort**: the program ends with a message on stderr
and a non-zero exit status. Each is a case where the language has no answer to
give and no `Result` in the signature to give it through.

An abort is not an effect in the `ctx` sense — it can occur in a function with no
context parameter — but a context-free function cannot *observe* or *recover
from* one. There is no catch.

---
