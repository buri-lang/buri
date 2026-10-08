## 6. Expressions

`if`, `match`, and blocks are expressions. Only `let` is not.

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

`a < b < c` is a parse error. Bitwise operators bind tighter than comparison, as
in Rust: `a & MASK == 0` means `(a & MASK) == 0`.

There is no `<<` or `>>`; use `bits.shiftLeft(x, n)` and `bits.shiftRight(x, n)`
(`design/grammar-rationale.md` 12.6).

### 6.2 Arithmetic

`+ - * / %` desugar to the operator traits of Section 5.12.4. On built-in
numeric types both operands and the result share one type. **There is no
implicit promotion**: `a: I32 + b: I64` is an error, and so is `1.0 + 1`.

Integer `/` truncates toward zero and `%` takes the dividend's sign, so
`a == (a / b) * b + (a % b)` for every non-zero `b`.

*Integer* division by zero **aborts**. Float division by zero follows IEEE-754:
`1.0 / 0.0` is `+inf`, `-1.0 / 0.0` is `-inf`, `0.0 / 0.0` is `NaN`.

Integer overflow and underflow are **undefined behaviour**, not wrapping. In
practice a **native** backend wraps at the type's width, and on **JavaScript**
widths of 64 bits and up are `BigInt`s, so overflow yields a value too large for
the type. Neither is promised. For a defined answer, pick one: `Checked` answers
`.None`, `Wrapping` the low bits, `Saturating` the bound, the same on every
backend.

Floats follow IEEE-754 except that **`==` is an equivalence relation**. It
compares numerically (`-0.0 == 0.0`, `0.1 + 0.2 != 0.3`) and is reflexive:
**`NaN == NaN` is true**, whatever the sign or payload. `Map` keys, `Set`
members, `list.contains` and `derive Equal` all need that.

The **ordering** operators stay IEEE-754's: any `<`, `<=`, `>` or `>=` with a
`NaN` operand is false, including `NaN < NaN`. So neither `a <= b && b <= a` nor
`!(a < b) && !(a > b)` implies `a == b`. Use `math.isNan(x)` to test for `NaN`.
That holds behind an `Ordered` bound too: `a < b` at a `T` that is a float is
IEEE's, not `compare`'s.

`compare` is the total order (Section 5.11): `-0.0` before `0.0`, and every
`NaN` after `inf` and `.Equal` to every other `NaN`.

A `NaN`'s payload isn't part of its value, so nothing preserves one:
`bytes.f64FromBytes` answers the canonical quiet NaN for every NaN pattern, which
round-trips to the same eight bytes.

A float renders as the shortest decimal that round-trips, so `1.0 / 3.0` prints
the same characters on every backend.

#### 6.2.1 Conversions

Numeric conversions are explicit methods:

```buri
# from "core/number" import { RangeError };
#
# fn demo(big: I64, hits: Int, total: Int): F64 {
    let a: I32 = 7;
    let b = a.toI64(); // I64, always exact
    let c: Result<I32, RangeError> = big.toI32(); // may not fit
    let d = big.wrapToU8(); // modular, for checksums and wire formats
    let ratio = hits.toF64() / total.toF64();
#     ratio
# }
```

Three families:

| Shape | Returns | When it does not fit |
|---|---|---|
| `x.toT()` where every `T` value fits | `T` | cannot happen |
| `x.toT()` where it might not | `Result<T, RangeError>` | `.Err` |
| `x.wrapToT()` | `T` | wraps (integers) or rounds (floats) |

So `i32.toI64()` yields `I64` while `i64.toI32()` yields
`Result<I32, RangeError>`: the type says whether a conversion can fail.

`toF64` on an integer is the exception: it never fails, but is exact only up to
2^53 and rounds beyond, identically on every backend. Converting a count to a
float is too common to return a `Result`.

`Char` converts the same way: `c.toU32()` is exact, `n.toChar()` yields
`Result<Char, RangeError>`.

A float converts to an integer only if it's whole and in range, so
`2.5.toI64()`, `NaN` and the infinities are `.Err`. Use `wrapToT` to truncate.

These methods live in `core/number`. There is no `as` cast; `as` appears only in
imports (`design/grammar-rationale.md` 12.5).

#### 6.2.2 Checked and wrapping arithmetic

For defined overflow, call one of these trait methods:

```buri
trait Checked {
    fn checkedAdd(self, rhs: Self): Option<Self>;
    fn checkedSubtract(self, rhs: Self): Option<Self>;
    fn checkedMultiply(self, rhs: Self): Option<Self>;
    fn checkedDivide(self, rhs: Self): Option<Self>;
}

trait Wrapping {
    fn wrappingAdd(self, rhs: Self): Self;
    fn wrappingSubtract(self, rhs: Self): Self;
    fn wrappingMultiply(self, rhs: Self): Self;
}

trait Saturating {
    fn saturatingAdd(self, rhs: Self): Self;
    fn saturatingSubtract(self, rhs: Self): Self;
    fn saturatingMultiply(self, rhs: Self): Self;
}

trait Bounded {
    fn minValue(): Self;
    fn maxValue(): Self;
}
```

```buri
# from "core/number" import * as number;
#
# fn demo(a: Int, b: Int, seed: U32, byte: U32): U8 {
    let safe = a.checkedAdd(b).withDefault(0);
    let hash = seed.wrappingMultiply(31).wrappingAdd(byte);
    let ceiling = number.maxValue<U8>();
#     ceiling
# }
```

Every built-in integer type satisfies all four; float types satisfy only
`Bounded`. A `Checked` method answers `.None` exactly when the true result
doesn't fit. `Bounded` and `Saturating` use the type's own bounds on every
backend.

### 6.3 Blocks

A block is zero or more `let` bindings followed by a result expression. The
[grammar](./crates/docs/src/docs/grammar.ebnf) makes the result optional, but the checker
rejects a block without one.

```buri
# from "core/math" import * as math;
#
# fn demo(a: Float, b: Float): Float {
    let hypotenuse = {
        let a2 = a * a;
        let b2 = b * b;
        math.squareRoot(a2 + b2)
    };
#     hypotenuse
# }
```

Bindings evaluate in order (Section 8.2) and are in scope for the rest of the
block. Shadowing is allowed, even within one block:

```buri wrap=body
# let raw = "  Ada ";
let name = raw.trim();
let name = name.toLower(ctx); // legal; the earlier `name` is inaccessible
```

A `let` pattern must be irrefutable; use `match` otherwise.

### 6.4 `if`

```buri wrap=body
# let n = 3;
let label = if (n < 0) {
    "negative"
} else if (n == 0) {
    "zero"
} else {
    "positive"
};
```

- The condition is parenthesized and has type `Bool`. There is no truthiness.
- Both branches are blocks of the same type, and `else` is **mandatory**
  (`design/grammar-rationale.md` 12.10).

### 6.5 `match`

```buri
# enum Shape {
#     Empty,
#     Circle(Float),
#     Rect { width: Float, height: Float },
# }
#
# fn demo(shape: Shape): Str {
    let describe = match (shape) {
        .Circle(r) if r > 100.0 => "huge circle",
        .Circle(_) => "circle",
        .Rect { width: w, height: h } => if (w == h) { "square" } else { "rect" },
        .Empty => "nothing",
    };
#     describe
# }
```

- The scrutinee is parenthesized.
- Arms are **comma-separated**, even after a brace-terminated body, with an
  optional trailing comma (`design/grammar-rationale.md` 12.12).
- The first matching arm wins.
- A guard (`if expr`) may follow a pattern. Guards don't count toward
  exhaustiveness.
- The match must be **exhaustive** (Section 7.3); the error names a missing
  case.

### 6.6 Calls and lambdas

```buri
fn add(a: Int, b: Int): Int {
    a + b
}

# fn demo(xs: [Int]): Int {
    let inc = fn(x) => x + 1;
    let addTyped = fn(a: Int, b: Int): Int => a + b;
    let sum = xs.fold(fn(acc, x) => acc + x, 0);
#     sum
# }
```

Lambdas begin with `fn` so `(x)` is never mistaken for a parameter list. Their
parameter and return types may be omitted where inferable, but are still checked:
`let f: fn(Int) => Str = fn(_x) => 5` is a `type-mismatch` at the `5`.

A lambda body extends as far right as possible, so `2 * fn(x) => x` is a parse
error; write `2 * (fn(x) => x)` (`design/grammar-rationale.md` 12.11).

There is no partial application; write a lambda.

### 6.7 Method calls

```buri
# from "core/math" import * as math;
#
# struct Square(Int);
#
# impl Square {
#     fn area(self): Int {
#         self.0 * self.0
#     }
# }
#
# struct User {
#     name: Str,
# }
#
# fn demo(user: User, pair: (Int, Str), xs: [Int], i: Int, sq: Square): Int {
    let name = user.name; // struct field
    let first = pair.0; // tuple element
    let item = xs[i]; // Option<T>
    let root = math.squareRoot; // module member
    let area = sq.area(); // method call
#     area
# }
```

The dotted forms all parse the same way; name resolution tells them apart.

#### 6.7.1 Declaring a method

A method is declared **inside an `impl` block for its type** and takes `self`
first. A top-level `fn` taking `self` is an error, and so is an `impl` function
without one:

```buri
export struct Square {
    height: Int,
    width: Int,
}

impl Square {
    export fn area(self): Int {
        self.height * self.width
    }

    export fn scaled(self, factor: Int): Square {
        Square { height: self.height * factor, width: self.width * factor }
    }
}

// NOT a method
export fn combine(a: Square, b: Square): Square {
    Square { height: a.height + b.height, width: a.width + b.width }
}
```

An `impl` with a `for` clause declares trait conformance instead
(Section 5.12.2).

`self` takes no type; the `impl` head already gives it. Writing one is the
`typed-self` error.

An `impl` block may appear only in the module that declares its type. Neither it
nor a `derive` is ever `export`ed. Methods carry their own `export`, except
methods supplied to a trait, whose conformance travels with the type.

Generic parameters the self type mentions belong to the `impl`; the rest belong
to the method.

```buri
export struct Pair<T>(T, T);

impl<T> Pair<T> {
    export fn map<U>(self, f: fn(T) => U): Pair<U> {
        Pair(f(self.0), f(self.1))
    }
}
```

#### 6.7.2 Calling a method

`x.f(a, b)` passes `x` as `self`, then `a` and `b`. **The receiver comes first**,
then the context parameter if there is one. `core/list` declares `map` as
`fn map<B, C: Allocator>(self, ctx: C, f: fn(A) => B): [B]` in `impl<A> [A]`, so:

```buri wrap=body
# let xs = [1, 2, 3];
# let double = fn(x: Int): Int => x * 2;
let doubled = xs.map(ctx, double); // reads as: this list, in this world, mapped
```

That's the calling convention of Section 10.7.

**Methods need no import:**

```buri repo=cli/tests/example package=//lib/ledger
from "//lib/money" import { Cents }; // the type, not `add` or `isZero`

fn cancels(a: Cents, b: Cents): Bool {
    a.add(b).isZero() // both resolve with no further imports
}
```

If you never name the type, you need no import at all.

#### 6.7.3 Resolution

`x.f(...)` resolves in order:

1. If `x`'s type has a field named `f`, this is field access. A field of function
   type is called as `(x.f)(...)`.
2. If `x`'s type is concrete, `f` must be a method from an `impl` block in that
   type's **defining module**, inherent or supplied to a trait. Only inherent
   methods are subject to `export`.
3. If `x`'s type is a type parameter, `f` must be declared by one of its
   **bounds** (Section 5.10). A bare parameter with no bounds has no methods.

| Type | Defining module |
|---|---|
| a `struct` or `enum` you declared | the module declaring it |
| `[T]` | `core/list` |
| `Str` | `core/str` |
| `Char` | `core/character` |
| `Bool` | `core/bool` |
| every integer and float type | `core/number` |
| `Option<T>` | `core/option` |
| `Result<T, E>` | `core/result` |
| tuples, function types, `Template` | none — no methods |

Type aliases are transparent, so `Int` and `I64` have the same methods.

**Steps 2 and 3 exclude an effect's methods.** Perform an effect by passing the
context to a function: `io.println(ctx, t)`, not `ctx.println(t)`. Only the
standard library and the body of an `impl` that supplies an effect may use the
method form (Section 10.2). The `effect-method-call` error names the function to
call.

Each step is one table lookup by name and type: no candidate set, autoref,
autoderef, or coherence check. Because it needs the receiver's type, name
resolution consults inference.

If two bounds declare the same method name, the call is ambiguous; call the trait
method as a function instead: `Ordered.compare(x, y)`.

Consequences:

- **Methods aren't extensible.** `impl Str { ... }` in your module is an error;
  write a free function.
- **Methods aren't values.** Neither `sq.area` nor `area` is one; wrap
  `sq.area()` in a lambda to pass it on.
- **The receiver's type must be known.** Inside `fn f<T>(x: T)`, `x.anything()`
  is an error unless a bound on `T` declares it (Section 5.12).

### 6.8 `?` — error propagation

Postfix `?` unwraps a `Result` or `Option`, or returns the failure from the
enclosing function.

```buri
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };
# from "platform/effect" import { Allocator, IoError };
#
# enum ConfigError {
#     Unreadable(IoError),
#     Malformed,
# }
#
# struct Config {
#     port: Int,
# }
#
# fn parseConfig(text: Str): Result<Config, ConfigError> {
#     match (text.toInt()) {
#         .Some(port) => .Ok(Config { port }),
#         .None => .Err(.Malformed),
#     }
# }
#
fn loadPort<C: Allocator + FileSystemRead>(ctx: C, at: Path): Result<Int, ConfigError> {
    let text = fs.readText(ctx, at).mapErr(fn(e) => ConfigError.Unreadable(e))?;
    let cfg = parseConfig(text)?; // on .Err(e), loadPort answers .Err(e)
    .Ok(cfg.port)
}
```

- On `Result<T, E>`, the function must return `Result<_, E>`. Errors aren't
  converted automatically; use `result.mapErr`.
- On `Option<T>`, the function must return `Option<_>`.

`?` is the only early exit. There is no `return`.

To fall back instead, use `withDefault`: `cfg.port.withDefault(8080)`. Its
argument is always evaluated, so use a `match` when the default must only run if
needed.

### 6.9 Aborting

You can't write that a branch is impossible. `panic` and `unreachable` are
reserved (Section 3.4), `crash` is an ordinary identifier, and there is no bottom
type. Handle every case instead: `withDefault` or `match` an `Option`, and make
impossible states unrepresentable.

Division by zero, a shift at or beyond its type's width, and stack exhaustion
**abort**: the program prints a message on stderr and exits non-zero.

An abort needs no context and can happen anywhere, but nothing can observe or
catch one.

---
