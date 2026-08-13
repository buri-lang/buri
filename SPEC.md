# The Buri Language Specification

**Version 0.2 (draft) · file extension `.buri`**

---

## 1. Introduction

Buri is a strict, purely functional, statically typed language with TypeScript-shaped
syntax, Rust-shaped data declarations, and Roc-shaped ideas about platforms and
effects.

Three ideas define it:

1. **There is no mutation.** Every binding is final. There are no references, no
   borrowing, and no lifetimes. Values are values.
2. **Effects travel through arguments.** The ability to allocate, read a file, or
   open a socket is a *value* of an unforgeable type. A function that was not
   handed one cannot perform that effect. Purity is therefore a property you can
   read off a signature, not a property the compiler asks you to trust.
3. **The grammar is context-free and unambiguous.** Parsing never consults name
   resolution or types. Section 12 documents each design decision that pays for
   this, and what was given up to get it.

Version 0.2 is deliberately small: primitives, arrays, tuples, records, structs,
enums, functions, methods, and traits. Data and behaviour are declared
separately; there is no mutable state, no inheritance, and no dynamic dispatch. A
method is an ordinary function whose first parameter is `self`, and a trait is an
interface satisfied structurally — neither introduces a runtime mechanism.

There are also no loops. Iteration is recursion — guaranteed tail-call
eliminated — or a fold. A `for`/`while` sugar was drafted for this version and
cut; Section 14 records why.

### 1.1 A taste

```buri
from "core/io" import * as io;
from "core/list" import * as list;
from "core/cap" import { Alloc, Stdout };

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
fn area(self: Shape): Float {
  match (self) {
    .Circle(r) => 3.14159 * r * r,
    .Rect { width, height } => width * height,
    .Empty => 0.0,
  }
}

// Takes a context, so it may allocate and print.
export fn main(ctx: { alloc: Alloc, stdout: Stdout, .. }): Result<{}, Str> {
  let shapes = [Shape.Circle(1.0), Shape.Rect { width: 2.0, height: 3.0 }];
  let total = shapes.map(ctx, area).sum();
  let _ = io.println(ctx, "total area: ${total}");
  .Ok({})
}
```

---

## 2. Notation and conformance

The normative grammar lives in [`grammar.ebnf`](./grammar.ebnf). Where this
document and that file disagree about syntax, the grammar file wins. Where this
document states a rule that the grammar cannot express (Section 13), that rule is
normative and is checked after parsing.

Terminology: *must* is a requirement on conforming programs and implementations;
*should* is a recommendation; *may* grants latitude.

---

## 3. Source text and lexical structure

### 3.1 Encoding

Source files are UTF-8. Identifiers are ASCII in v0.2. String and character
literals may contain any Unicode scalar value.

### 3.2 Whitespace and comments

Buri is not newline-sensitive; there is no automatic semicolon insertion and no
offside rule. Whitespace separates tokens and is otherwise meaningless.

```buri
// line comment
/* block comment /* which nests */ still a comment */
/// doc comment; attaches to the declaration that follows
```

### 3.3 Identifiers and naming

`IDENT` is `[A-Za-z_][A-Za-z0-9_]*` minus keywords and reserved words.

Naming conventions — **not enforced by the grammar, by design**, since a parser
that depends on capitalization is a parser that depends on convention:

| Kind | Convention | Example |
|---|---|---|
| Types, structs, enums, variants | `UpperCamelCase` | `UserId`, `Some` |
| Functions, parameters, bindings | `lowerCamelCase` | `readConfig`, `ctx` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_RETRIES` |
| Modules | `lowercase` | `list`, `str` |

### 3.4 Keywords

`as` `as?` `as%` `const` `crash` `derive` `else` `enum` `export` `false` `fn`
`for` `from` `if` `impl` `import` `let` `match` `opaque` `self` `Self` `struct`
`trait` `true` `type`

`for` appears only in `impl ... for ...` and `derive ... for ...`. `self` is
legal only as a method's first parameter; `Self` only inside a trait or `impl`.

Reserved for future versions and rejected today: `async` `await` `break`
`continue` `do` `in` `is` `loop` `module` `mut` `pub` `return` `use` `when`
`where` `while` `with` `yield`.

### 3.5 Literals

```buri
42          1_000_000     0xFF     0o755     0b1010_0110      // INT
3.14        1.0e-9        6.02e23                             // FLOAT
"hello"     "tab\there"   "\u{1F600}"                         // STRING -> Str
'a'         '\n'          '\u{41}'                            // CHAR
true        false                                             // BOOL
"n = ${n}"                                                    // TEMPLATE
```

A float literal must begin with a digit. `.5` is not a literal; write `0.5`. This
is what lets `pair.0` lex as tuple access.

Underscores are permitted as digit separators anywhere after the first digit.

There are no literal suffixes. A numeric literal takes its type from context and
falls back to `Int` / `Float`; see Section 5.1.1.

### 3.6 String interpolation and `Template`

A string literal containing at least one `${ ... }` hole has type `Template`, not
`Str`. A `Template` is a fixed-size value: a statically known array of literal
fragments plus the evaluated holes. **Constructing a `Template` allocates
nothing**, which is why `io.println(ctx, "hi ${name}")` needs only the `stdout`
capability and can stream directly to the sink.

To turn a `Template` into a `Str` you must allocate:

```buri
let greeting: Str = str.format(ctx, "Hello, ${name}!");
```

Hole expressions must have type `Int` (any width), `Float` (any width), `Bool`,
`Char`, or `Str`. There is no user-extensible display mechanism in v0.2; convert
explicitly.

`Str` is implicitly widened to `Template` in argument position. This is the only
implicit conversion in the language, and it exists so that `io.println(ctx, "hi")`
and `io.println(ctx, "hi ${name}")` are both well-typed.

Escape `\$` to write a literal dollar sign before a brace.

---

## 4. Modules

A source file is a module. Its path relative to the package root is its name.

### 4.1 Imports

The module path comes **first**, before the specifier list:

```buri
from "core/list" import { map, filter };
from "core/list" import { map as listMap };
from "core/list" import * as list;
from "core/cap" import { Alloc, Fs, Stdout };
```

The ordering is chosen for tooling rather than for prose: by the time you open
the brace, the compiler already knows which module you mean, so an editor can
offer the module's exports as completions. With the path last, the specifier
list has to be typed blind and then retro-checked.

A namespace import **must** be named. `from "core/list" import *;` is not
derivable from the grammar — the only wildcard form is `* as <name>`. There is
consequently no way for an identifier to enter a module's scope without that
identifier, or the namespace holding it, being written in the importing file.
Every unqualified name in a module can be resolved by reading that module alone,
and adding an export to a library can never shadow or collide with a name in
code that imports it.

Import declarations are terminated with `;`. Circular imports are an error.

None of this applies to method calls. `sq.area()` resolves through the receiver's
type rather than through scope (Section 6.7.3), so a type's own operations are
available wherever a value of that type is, with no import and no possibility of
collision. Importing a type brings its methods with it.

### 4.2 Exports

A declaration is module-private unless prefixed with `export`.

```buri
fn helper(x: Int): Int { x * 2 }           // private
export fn double(x: Int): Int { helper(x) } // public
```

`opaque` exports the *name* of a type but not its representation: outside the
defining module the type cannot be constructed, destructured, or pattern-matched.
This is the mechanism behind capabilities (Section 10).

```buri
export opaque struct UserId(Str);
```

### 4.3 Order

Declarations are visible throughout their module regardless of order. Mutual
recursion between top-level functions requires no forward declarations.

---

## 5. Types

### 5.1 Primitives

| Type | Meaning |
|---|---|
| `Bool` | `true` / `false` |
| `I8` `I16` `I32` `I64` `I128` | signed two's-complement integers |
| `U8` `U16` `U32` `U64` `U128` | unsigned integers |
| `F32` `F64` | IEEE-754 binary32 / binary64 |
| `Char` | a Unicode scalar value |
| `Str` | an immutable UTF-8 string |
| `Template` | an interpolated string literal (Section 3.6) |

There is no `null` and no `undefined`. Absence is `Option<T>`.

### 5.1.1 Numbers

Two audiences have to be served at once. Most code wants to say "a number" and
move on. Some code — binary formats, checksums, graphics, FFI, anything with a
size on the wire — needs to name an exact width and have the compiler hold it to
that. Buri serves both with **one set of types and two names for the common
ones**:

```buri
type Int   = I64;      // the default integer
type Float = F64;      // the default float
type Uint  = U64;
type Byte  = U8;
```

These are **aliases, not distinct types**. `Int` and `I64` are the same type, so
a function declared with `Int` and one declared with `I64` interoperate with no
conversion. Diagnostics print whichever spelling the program used.

Everyday code writes `Int` and `Float` and never thinks about widths. Code that
cares writes `U8` or `I32` or `F32` and gets exactly that. There is no third
category and no numeric tower.

#### Literals are polymorphic until they are pinned

A numeric literal does not have a type on sight. It gets a fresh type variable
constrained to the integer types (for an integer literal) or the float types (for
a float literal); ordinary unification then decides. Only if nothing constrains
it does the default apply — `Int` for integer literals, `Float` for float
literals.

```buri
let a = 5;               // nothing constrains it -> Int
let b: U8 = 5;           // the annotation pins it -> U8, no conversion
let c: F32 = 1.5;        // -> F32
takesU16(5)              // the parameter pins it -> U16
let d = 5 as U8;         // the cast target pins it -> U8

let e: [U8] = [1, 2, 3]; // every element is a U8
```

This is defaulting on *literals only*. It is not overloading and not a numeric
tower: `a + b` still requires `a` and `b` to already have the same type.

Because a literal's type is known before it is checked, **a literal that does not
fit its type is a compile error**, not a runtime surprise:

```buri
let x: U8 = 300;         // ERROR: 300 is not representable in U8
let y: I8 = -129;        // ERROR
let z: U32 = -1;         // ERROR: U32 has no negative values
let w: U64 = 18_446_744_073_709_551_615;    // fine
```

There are no literal suffixes (`5u8`), because `as` already does that job with
one rule instead of two.

#### Generic numeric code

Arithmetic is available on a type parameter through the operator traits of
Section 5.12 — `Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`, `Ord` — each of which
is an ordinary interface with a method set:

```buri
fn total<N: Add>(zero: N, xs: [N]): N { ... }
fn clamp<N: Ord>(lo: N, hi: N, x: N): N { ... }
```

Earlier drafts had three compiler-privileged bounds named `Num`, `Integral`, and
`Floating`. They are gone. A blob bound named after what a type *is* was standing
in for a trait system that did not exist yet; now that traits do exist, bounds
name what a type *can do*, which is both more precise and one fewer mechanism.

The integer-specific operations follow the same rule — they are interfaces named
for what they provide, not for the representation behind them:

```buri
trait Bounded  { fn minValue(): Self; fn maxValue(): Self; }
trait Checked  { fn checkedAdd(self: Self, rhs: Self): Option<Self>; ... }
trait Wrapping { fn wrappingAdd(self: Self, rhs: Self): Self; ... }
```

Every built-in integer type satisfies `Bounded`, `Checked`, and `Wrapping`; the
float types satisfy `Bounded` but not the other two.

None of this affects ordinary code. `Int` and `F64` are concrete types, so a
function over them needs no bound, no trait, and no ceremony:

```buri
fn area(self: Square): Int { self.height * self.width }
fn ratio(hits: Int, total: Int): F64 { hits as% F64 / total as% F64 }
```

### 5.2 Unit

The unit type is the empty record `{}`, and its only value is written `{}`.
There is no zero-tuple. Functions that exist only for their effect return `{}`.

### 5.3 Tuples

Tuples have arity 2 or more. `(T)` is a parenthesized type, not a 1-tuple.

```buri
let pair: (Int, Str) = (1, "one");
let first = pair.0;
let (n, name) = pair;
```

Tuple element access is `.0`, `.1`, … . Because `0.1` lexes as a float, nested
access must be parenthesized: `(t.0).1`.

### 5.4 Arrays

`[T]` is an immutable, densely packed sequence of `T`.

```buri
let xs: [Int] = [1, 2, 3];
let n = list.len(xs);          // pure: no allocation
let maybe = xs[0];             // Option<Int>, not Int
```

**Indexing yields `Option<T>`.** There is no way to index out of bounds and no
way to panic by indexing. This is the single largest ergonomic tax the language
charges, and it is charged on purpose.

An array literal has a statically known length and is not, by itself, an
allocation the programmer must account for. Any operation whose result length
depends on runtime data (`map`, `filter`, `concat`, `sort`, `range`) requires an
`Alloc` capability.

### 5.5 Records

Records are structural. Field order is not significant.

```buri
type Config = { host: Str, port: Int };

let c: Config = { host: "localhost", port: 8080 };
let c2 = { ..c, port: 9090 };            // functional update
let h = c.host;
```

**Record literals do not support field shorthand.** Write `{ host: host }`, not
`{ host }`. See Section 12.3 for why. Record *patterns* do support shorthand.

#### 5.5.1 Row polymorphism

A record type may end in a row rest, meaning "at least these fields":

```buri
{ host: Str, .. }        // fresh anonymous row variable
{ host: Str, ..R }       // the row variable R, declared in <..R>
```

An open record type accepts any record that has at least the listed fields with
the listed types. Use `..R` when two positions must share the same unknown tail:

```buri
fn passThrough<..R>(ctx: { alloc: Alloc, ..R }): { alloc: Alloc, ..R } { ctx }
```

Buri uses row polymorphism, not subtyping. There is no `Any`, no top type, and no
variance to reason about.

### 5.6 Structs

Structs are nominal. Two structs with identical fields are different types.

Fields are **module-private unless exported**, the same rule that governs
declarations:

```buri
struct User {
  export id: UserId,
  export name: Str,
  passwordHash: Str,          // private to this module
}

struct Meters(F64);                        // tuple struct
struct Pair<A, B>(A, B);                   // generic tuple struct

let u = User { id: UserId("u1"), name: "Ada", email: .None };
let u2 = User { ..u, name: "Ada L." };
let d = Meters(9.8);
let raw = d.0;
```

Tuple-struct declarations are terminated with `;`; record-struct declarations are
not.

Outside the declaring module, a private field cannot be read, written in a
literal, or matched. A struct with any private field therefore cannot be
constructed from scratch elsewhere — but functional update still works, because
it never names the hidden fields:

```buri
let renamed = User { ..u, name: "new" };     // fine anywhere
let forged = User { id: ..., name: ..., passwordHash: ... };   // only in the
                                                               // declaring module
```

`opaque` (Section 4.2) remains the all-or-nothing form: it hides the
representation entirely, including the type's shape. Per-field privacy is the
finer-grained tool.

### 5.7 Enums

Enums are Rust-style sum types. Variants may be nullary, tuple-like, or
record-like, and may mix within one enum.

```buri
enum Shape {
  Empty,
  Circle(Float),
  Rect { width: Float, height: Float },
}

enum Tree<T> {
  Leaf,
  Node(Tree<T>, T, Tree<T>),               // recursive; boxed by the runtime
}
```

Constructing a variant uses a qualified path or the inferred-type dot form:

```buri
let a = Shape.Circle(1.0);
let b: Shape = .Rect { width: 2.0, height: 1.0 };
let c: Shape = .Empty;
```

The dot form requires that the expected type is known from context (a
`let` annotation, a parameter type, the enclosing function's return type, or a
`match` scrutinee's type). When it is not, use the qualified form.

The prelude defines:

```buri
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }
enum Order { Less, Equal, Greater }
```

#### 5.7.1 `Result` is must-use

**A value of type `Result<T, E>` may not be discarded.** Discarding means binding
it to `_`, or to any pattern that drops it without inspecting the `Err` case:

```buri
let _ = fs.writeText(ctx, path, body);          // ERROR: discarded Result
```

Since there are no expression statements (Section 12.2), `let _ =` is the only
place a value can be thrown away, so this is a one-line rule with no holes. The
legal ways to consume a `Result` are:

```buri
fs.writeText(ctx, path, body)?                   // propagate
match (fs.writeText(ctx, path, body)) { ... }    // handle
result.withDefault({}, fs.writeText(ctx, path, body))
result.ignore(fs.writeText(ctx, path, body))     // explicitly, greppably, ignore
```

`result.ignore(r): {}` exists so that "I considered this and do not care" is a
thing you *write*, rather than a thing that happens by not writing anything. A
reviewer can grep for it; `_` is unsearchable.

The rule is on the type, not on the call: a `Result` returned from a pure
function is just as must-use as one returned from an I/O call.

`Option` is **not** must-use. Ignoring an absent value is usually harmless, and
making it an error would put `option.ignore` in front of half the standard
library for no safety gain. Section 14 records this as a judgment call rather
than a principle.

Note also that `io.print` / `io.println` return `{}`, not `Result`. Stream errors
are reported by the platform at flush time and surface as `main`'s exit status;
threading an `IoError` through every print statement buys nothing that a
program can act on.

### 5.8 Function types

```buri
fn(Int, Int) => Int
fn() => {}
fn(Str) => Result<Config, ParseError>
```

Function types are written with the `fn` keyword for the same reason lambdas are:
it makes `(A, B)` unambiguously a tuple everywhere. Function types are rank-1;
there are no polymorphic function *values* in v0.2.

### 5.9 Type aliases

```buri
type UserId = Str;
type Handler<T> = fn(T) => Result<{}, Str>;
type IoCtx = { alloc: Alloc, stdout: Stdout, stderr: Stderr };
```

Aliases are transparent: `type UserId = Str` makes `UserId` and `Str` the same
type. For a distinct type, use a tuple struct: `struct UserId(Str);`.

### 5.10 Generics

Type parameters are declared in angle brackets; row parameters are prefixed with
`..`.

```buri
fn identity<T>(x: T): T { x }
fn map<A, B>(ctx: { alloc: Alloc, .. }, f: fn(A) => B, xs: [A]): [B] { ... }
fn tee<T, ..R>(ctx: { stdout: Stdout, ..R }, x: T): T { ... }
```

A parameter may carry one or more **bounds**, naming traits the argument type
must satisfy. Multiple bounds are joined with `+`:

```buri
fn largest<T: Ord>(xs: [T]): Option<T> { ... }
fn report<T: Ord + Show>(ctx: { alloc: Alloc, .. }, xs: [T]): Str { ... }
```

Inside such a function, the bound's methods are callable on the parameter —
`x.compare(y)`, `x.show(ctx)` — and nothing else is. There are no `where`
clauses, no associated types, and no blanket implementations, which is what keeps
bound checking a lookup rather than a search (Section 5.12).

Generic code that needs an operation no trait provides takes it as a function
argument, as it always has: `sortBy(xs, cmp)` rather than inventing a trait.

In *expression* position, explicit type arguments use the turbofish:

```buri
let f = identity::<Int>;
let e: [Int] = list.empty::<Int>();
```

### 5.11 Equality and ordering

`==` and `!=` are `Eq.eq`; `<` `<=` `>` `>=` are `Ord.compare` (Section 5.12.4).
Neither is compiler magic: a type has them because it derives or implements the
trait.

Every primitive, and `[T]`, tuples, and records built from types that have them,
satisfy `Eq` and `Ord` out of the box. Your own structs and enums opt in:

```buri
derive Eq, Ord for Version;
```

`Eq` is not defined for function types, `Template`, or opaque types from other
modules, so comparing those is a compile error. `Ord` on floats orders `-0.0`
equal to `0.0` and reports `NaN` as unordered.

### 5.12 Traits

A trait is an **interface**: a named set of method signatures that a type may
satisfy.

```buri
trait Ord {
  fn compare(self: Self, other: Self): Order;
}

trait Show {
  fn show(self: Self, ctx: { alloc: Alloc, .. }): Str;
}
```

`Self` stands for the implementing type and is legal only inside a trait or an
`impl`. Trait methods declare `self` first, exactly like any other method
(Section 6.7.1).

#### 5.12.1 Satisfaction is structural

A type satisfies a trait when its **defining module declares matching methods**.
Nothing needs to be written for satisfaction to hold:

```buri
// lib/version.buri
export struct Version { export major: Int, export minor: Int }

export fn compare(self: Version, other: Version): Order {
  match (self.major.compare(other.major)) {
    .Equal => self.minor.compare(other.minor),
    ord => ord,
  }
}
```

`Version` now satisfies `Ord`. Checking `T: Ord` is one lookup in one known
module, because a type has exactly one defining module and therefore exactly one
candidate. There is no coherence pass, no orphan rule, and no instance search —
they are not forbidden by discipline, they are unrepresentable.

This is also why trait methods and ordinary methods are the same mechanism
rather than two: both are found by looking in the receiver's defining module. A
trait is a *name for a predicate over what that module declares*.

#### 5.12.2 `impl` states the intent, and is checked

Structural satisfaction alone can happen by accident. An `impl` block declares
that satisfaction is deliberate and makes the compiler verify it:

```buri
impl Ord for Version {
  fn compare(self: Version, other: Version): Order { ... }
}
```

An `impl` block is defined as exactly equivalent to declaring those methods at
module level, plus an assertion that every signature the trait requires is
present and matches. It introduces no second namespace and no second resolution
path — `version.compare` and `v.compare(other)` mean the same thing either way.

An `impl` may only appear in the defining module of the type. There is no way to
implement a trait for someone else's type, which is the same restriction that
already applies to methods.

#### 5.12.3 `derive` generates the implementation

```buri
derive Eq, Ord, Show for Version;
```

`derive` generates the trait's methods structurally: struct fields in declaration
order, enum variants in declaration order, recursing into field types. It is a
fold over one type definition — no search, no instances to resolve.

Derivation is available for `Eq`, `Ord`, `Show`, `Hash`, and the operator traits.
A `derive` fails to compile if any field's type does not itself satisfy the trait.

#### 5.12.4 Operators are trait methods

| Operator | Trait method |
|---|---|
| `a + b` | `Add.add` |
| `a - b` | `Sub.sub` |
| `-a` | `Neg.neg` |
| `a * b` | `Mul.mul` |
| `a / b` | `Div.div` |
| `a % b` | `Rem.rem` |
| `a == b`, `a != b` | `Eq.eq` |
| `a < b`, `a <= b`, `a > b`, `a >= b` | `Ord.compare` |

This is what makes newtype wrappers work:

```buri
struct Meters(F64);
derive Add, Sub, Ord, Show for Meters;

let total = Meters(1.5) + Meters(2.0);     // Meters
let far = total > Meters(3.0);             // Bool
// let bad = Meters(1.5) + 2.0;            // ERROR: F64 is not Meters
```

`derive Add for Meters` provides `Meters + Meters` and nothing else, so the unit
safety the newtype exists for survives contact with arithmetic.

**An operator implementation cannot allocate or perform an effect.** There is no
argument position in `a + b` through which a context could be passed, so every
operator is structurally confined to bounded, pure computation over values that
already exist. You cannot write an expensive `+` in this language. That is why
operator traits are safe here in a way they are not in languages where `+` can be
an arbitrary method call — and it is also why `Matrix + Matrix` is not
expressible: matrix addition allocates, so it is `a.add(ctx, b)`, which says so.

#### 5.12.5 What traits deliberately lack

No blanket implementations, no associated types, no `where` clauses, no
supertraits, no trait objects, and no dynamic dispatch. Each of those is a step
from "resolution is a lookup" toward "resolution is a search," and the search is
the entire compile-time cost of a trait system. Generic code is monomorphized;
a generic body is typechecked once, polymorphically, with bounds verified at the
call site.

---

---

## 6. Expressions

Everything that produces a value is an expression, including `if`, `match`, and
blocks. There is no statement/expression split beyond `let`.

### 6.1 Precedence

Lowest to highest:

| Level | Operators | Associativity |
|---|---|---|
| 0 | `fn(...) => e`, `crash e` | top-level only (never a sub-operand) |
| 1 | `\|\|` | left |
| 2 | `??` | right |
| 3 | `&&` | left |
| 4 | `==` `!=` `<` `<=` `>` `>=` | **non-associative** |
| 5 | `\|` | left |
| 6 | `^` | left |
| 7 | `&` | left |
| 8 | `+` `-` | left |
| 9 | `*` `/` `%` | left |
| 10 | `as` `as?` `as%` | left |
| 11 | `-` `!` `~` (prefix) | right |
| 12 | `.f` `.0` `(args)` `[i]` `?` `::<T>` `{ ... }` | left |

Comparison is non-associative: `a < b < c` is a parse error, not a bug waiting to
happen.

Bitwise operators bind tighter than comparison (as in Rust), so `a & MASK == 0`
means `(a & MASK) == 0`.

There is no `<<` or `>>`. Use `bits.shl(x, n)` and `bits.shr(x, n)`. See
Section 12.5.

### 6.2 Arithmetic

`+ - * / %` desugar to the operator traits of Section 5.12.4. On the built-in
numeric types they are defined on two operands of the *same* type and produce
that type. **There is no implicit promotion of any kind** — not integer
promotion, not int-to-float, not narrow-to-wide. `a: I32 + b: I64` is an error,
and so is `1.0 + 1`.

Integer `/` truncates toward zero; `%` takes the sign of the dividend, so
`a == (a / b) * b + (a % b)` holds for every non-zero `b`.

Division by zero, and overflow of any signed or unsigned integer operation, is a
**crash** (Section 6.10). Overflow is not wrapping by default: silent wrapping is
a correctness bug in almost all code and a deliberate technique in a little of
it, so the little of it says so out loud (below).

Floating point follows IEEE-754. `==` on floats compares numerically with `-0.0`
equal to `0.0`, and `NaN != NaN`.

#### 6.2.1 Conversions

Conversions between numeric types are always explicit, and they are **operators
rather than library functions**. That is forced: without traits, a library
function cannot be generic over its *source* type, so `num.toU8` would need one
version per source and would be `14 × 14` functions. The compiler already knows
every numeric type, so conversion belongs to it.

There are three, distinguished by what they do when the value does not fit:

| Form | Result | When it does not fit |
|---|---|---|
| `x as T` | `T` | **compile error** — only provably lossless conversions are allowed |
| `x as? T` | `Option<T>` | `.None` |
| `x as% T` | `T` | wraps (integers) or rounds (floats) |

```buri
let a: I32 = 7;
let b = a as I64;               // always fine: I32 fits in I64
let c = a as U32;               // ERROR: a could be negative

let big: I64 = readCount(ctx);
let idx: Option<I32> = big as? I32;     // handle the overflow case
let lo: U8 = big as% U8;                // deliberately keep the low 8 bits

let ratio = (hits as F64) / (total as F64);
```

**`as` is total and lossless.** It compiles only when every value of the source
type is exactly representable in the target:

| From → To | Allowed |
|---|---|
| `IN` → `IM` | `M >= N` |
| `UN` → `UM` | `M >= N` |
| `UN` → `IM` | `M > N` (so `U8 → I16` yes, `U8 → I8` no, `U64 → I64` no) |
| `IN` → `UM` | never |
| `I8 I16 I32 U8 U16 U32` → `F64` | yes (53-bit mantissa) |
| `I8 I16 U8 U16` → `F32` | yes (24-bit mantissa) |
| `I64 U64 I128 U128` → `F64`, `I32 U32` → `F32` | no |
| `F32` → `F64` | yes |
| `F64` → `F32`, any float → any integer | never |
| `Char` → `U32` | yes |

The table is mechanical, and the error message when a conversion falls outside it
names the operator to reach for instead.

**`as?` is checked.** It works between any two numeric types and yields `.None`
exactly when the value is not representable — including float-to-integer when the
value has a fractional part, is `NaN`, or is out of range. `U32 as? Char` is
`.None` for surrogates and out-of-range scalars.

**`as%` is modular.** For integers it reduces modulo 2^N (two's-complement
truncation or sign extension) and is the operator for checksums, hashing, and
binary formats. For `F64 as% F32` it rounds to nearest-even. For float-to-integer
it truncates toward zero, mapping `NaN` to `0` and infinities to the target's
bounds; if you want a different rounding, call `math.floor`/`math.round` first
and then use `as?`.

`as` never changes a value. `as?` never lies about one. `as%` changes it in a way
that is written down at the call site. There is no fourth form and no default.

The most common place `as%` appears outside bit-twiddling is `Int → Float`:

```buri
let average = total as% F64 / list.len(xs) as% F64;
```

`I64 → F64` is lossy above 2^53, so it does not qualify for `as`. Requiring the
sigil on a conversion this ordinary is a real ergonomic cost, and it is the cost
of not having a category of cast that silently loses data. `as? F64` is available
when the loss actually matters.

#### 6.2.2 Checked and wrapping arithmetic

The default `+` crashes on overflow. The alternatives are trait methods, so they
are spelled out where they are used and are available on any type that derives
them:

```buri
trait Checked {
  fn checkedAdd(self: Self, rhs: Self): Option<Self>;
  fn checkedSub(self: Self, rhs: Self): Option<Self>;
  fn checkedMul(self: Self, rhs: Self): Option<Self>;
  fn checkedDiv(self: Self, rhs: Self): Option<Self>;
}

trait Wrapping {
  fn wrappingAdd(self: Self, rhs: Self): Self;
  fn wrappingSub(self: Self, rhs: Self): Self;
  fn wrappingMul(self: Self, rhs: Self): Self;
}

trait Bounded {
  fn minValue(): Self;
  fn maxValue(): Self;
}
```

```buri
let safe = a.checkedAdd(b) ?? 0;
let hash = seed.wrappingMul(31).wrappingAdd(byte);
let ceiling = num.maxValue::<U8>();
```

Every built-in integer type satisfies all three; the float types satisfy
`Bounded` only.

### 6.3 Blocks

```buri
Block ::= "{" LetStmt+ "}"
        | "{" LetStmt* Expr "}"
```

A block is zero or more `let` bindings followed by a result expression. A block
whose last item is a `let` has type `{}`.

```buri
let hypotenuse = {
  let a2 = a * a;
  let b2 = b * b;
  math.sqrt(a2 + b2)
};
```

`let` bindings are evaluated **strictly, in source order** (Section 8.2). Each
binding is in scope for the remainder of the block. Shadowing is permitted, both
in nested scopes and within a single block:

```buri
let name = str.trim(raw);
let name = str.toLower(ctx, name);   // legal; the earlier `name` is inaccessible
```

The pattern in a `let` must be irrefutable. Use `match` for anything else.

### 6.4 `if`

```buri
let label = if (n < 0) { "negative" } else if (n == 0) { "zero" } else { "positive" };
```

- The condition must be parenthesized and must have type `Bool`. There is no
  truthiness.
- Both branches are blocks, and `else` is **mandatory**. This eliminates the
  dangling-else ambiguity outright, and there is nothing sensible for a missing
  branch to produce in a language where `if` is an expression.
- Both branches must have the same type.

To produce a record from an `if`, the record literal needs its own braces:
`if (c) { { x: 1 } } else { { x: 2 } }`, or bind it first.

### 6.5 `match`

```buri
let describe = match (shape) {
  .Circle(r) if r > 100.0 => "huge circle",
  .Circle(_) => "circle",
  .Rect { width: w, height: h } => if (w == h) { "square" } else { "rect" },
  .Empty => "nothing",
};
```

- The scrutinee must be parenthesized.
- Arms are **comma-separated**, with an optional trailing comma. The comma is
  required even after a brace-terminated arm body.
- Arms are tried in order; the first matching arm wins.
- A guard (`if expr`) may follow a pattern. Guards do not contribute to
  exhaustiveness.
- The match must be **exhaustive**. A non-exhaustive match is a compile error
  that names a missing case.

### 6.6 Calls and lambdas

```buri
fn add(a: Int, b: Int): Int { a + b }

let inc = fn(x) => x + 1;
let addTyped = fn(a: Int, b: Int): Int => a + b;
let sum = list.fold(fn(acc, x) => acc + x, 0, xs);
```

Lambdas begin with `fn` so that `(x)` is never ambiguous with a parameter list.
Parameter types and the return type may be omitted when inferable.

A lambda body extends as far right as possible, so a lambda cannot appear as a
bare operand of a binary operator. `2 * fn(x) => x` is a parse error; write
`2 * (fn(x) => x)`.

Arguments are evaluated left to right before the call. Partial application is not
built in; write a lambda.

### 6.7 Method calls

```buri
user.name          // record / struct field
pair.0             // tuple element
xs[i]              // Option<T>
list.map           // module member
sq.area()          // method call
```

All five are the same production — `PostfixExpr "." IDENT` — and they are told
apart during name resolution, never during parsing.

#### 6.7.1 Declaring a method

A function is a method **if and only if its first parameter is written `self`**.
That is a declaration, not a convention about position or type:

```buri
export struct Square { height: Int, width: Int }

export fn area(self: Square): Int { self.height * self.width }
export fn scaled(self: Square, factor: Int): Square {
  Square { height: self.height * factor, width: self.width * factor }
}

export fn combine(a: Square, b: Square): Square { ... }   // NOT a method
```

`self` is a keyword and may appear only as the first parameter. A method is
still an ordinary function: `area(sq)` and `sq.area()` are the same call, and
`area` is exported, imported, and passed as a value like anything else.

#### 6.7.2 Calling a method

```buri
x.f(a, b)      ==  f(x, a, b)
x.f()          ==  f(x)
```

**The receiver comes first**, and a context parameter — when there is one —
comes second:

```buri
export fn map<A, B>(self: [A], ctx: { alloc: Alloc, .. }, f: fn(A) => B): [B]

xs.map(ctx, double)          // reads as: this list, in this world, mapped
```

So the standard library's calling convention (Section 10.6) is **receiver first,
context second, everything else after**. Free functions that have no receiver
keep the context first, as before.

**Methods need no import.** That is the point of the feature:

```buri
// main.buri
from "lib/square" import { Square };     // the type — not `area`, not `scaled`

fn describe(sq: Square): Int {
  sq.scaled(2).area()                    // both resolve with no further imports
}
```

You import the type; its methods come along. If the value arrives from elsewhere
and you never name its type, you need no import at all.

#### 6.7.3 Resolution

`x.f(...)` resolves in exactly two steps, with no search:

1. If `x`'s type has a field named `f`, this is field access. A field of function
   type is called as `(x.f)(...)`.
2. Otherwise `f` must be an `export`ed method in the **defining module** of `x`'s
   type whose `self` parameter has that type.

There is no candidate set, no trait scope, no coherence check, no autoref and no
autoderef — Buri has no references. One type, one module, one lookup. Method
resolution does need the receiver's type, so name resolution consults inference;
a single lookup is the version of that cost worth paying.

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
| tuples, anonymous records, function types, `Template` | none — no methods |

Type aliases are transparent, so `Int` and `I64` have the same methods.

Three consequences worth stating plainly:

- **Methods are not extensible.** You cannot add a method to `Str` from your own
  module. Write a free function and call it as one.
- **Methods are not values.** `sq.area` on its own is an error; write `sq.area()`,
  or import the module and use `square.area` for the function itself.
- **The receiver's type must be known.** Inside `fn f<T>(x: T)`, `x.anything()`
  is an error unless `T` carries a bound that declares the method (Section 5.12).

### 6.8 `?` — error propagation

Postfix `?` unwraps a `Result` or `Option`, returning early from the enclosing
function on the failure case.

```buri
fn loadPort(ctx: { alloc: Alloc, fs: Fs, .. }, path: Str): Result<Int, ConfigError> {
  let text = fs.readText(ctx, path)?;        // Err(e) => return Err(e)
  let cfg = parseConfig(text)?;
  .Ok(cfg.port)
}
```

- On `Result<T, E>`, the enclosing function must return `Result<_, E>`. There is
  no automatic error conversion in v0.2; map the error explicitly with
  `result.mapErr`.
- On `Option<T>`, the enclosing function must return `Option<_>`.

`?` is the only early exit in the language. There is no `return`.

Note that `??` is a single token, so `x??y` is coalescing, not try-then-coalesce.
Write `(x?) ?? y`.

### 6.9 `??` — default

```buri
let port = cfg.port ?? 8080;             // Option<Int> ?? Int  -> Int
let name = lookup(id) ?? "anonymous";
```

Defined for `Option<T> ?? T` and `Result<T, E> ?? T`. The right operand is
evaluated only when the left is `None` / `Err`. `??` is right-associative, so
`a ?? b ?? c` works.

### 6.10 `crash`

```buri
let x = match (parsed) {
  .Some(v) => v,
  .None => crash "unreachable: validated upstream",
};
```

`crash` takes a `Str` or `Template` and has the bottom type, so it unifies with
any expected type. It terminates the program with the message and a stack trace.

`crash` is for conditions the programmer asserts cannot happen. It is not error
handling; that is `Result`. Arithmetic overflow, division by zero, and stack
exhaustion also crash.

A crash is not an effect in the capability sense: it can occur in a function with
no context parameter. What a context-free function cannot do is *observe* or
*recover from* one. There is no catch.

---

## 7. Patterns

### 7.1 Forms

| Form | Example |
|---|---|
| Wildcard | `_` |
| Binding | `n` |
| Named subpattern | `whole @ .Circle(r)` |
| Literal | `0`, `-1`, `"yes"`, `'x'`, `true` |
| Qualified variant | `Option.Some(x)`, `Shape.Empty` |
| Inferred variant | `.Some(x)`, `.Empty` |
| Struct | `User { id, name: n }`, `User { id, .. }` |
| Tuple struct | `Meters(m)` |
| Record | `{ alloc, fs }`, `{ host, .. }` |
| Tuple | `(a, b)` |
| Array | `[]`, `[x]`, `[first, ..rest]` |
| Or | `.Circle(_) | .Empty` |

Record and struct patterns support field shorthand: `{ alloc, fs }` binds `alloc`
and `fs`. A `..` at the end ignores remaining fields; without it, a struct pattern
must mention every field.

Array rest patterns bind only at the end: `[first, ..rest]` is legal,
`[..init, last]` is not.

Or-patterns must bind the same names at the same types in every alternative.

### 7.2 Why variants must be qualified

A bare identifier pattern is **always** a binding. `None` as a pattern binds a
variable named `None`; it does not match the `None` variant. Write `.None` or
`Option.None`.

This is a real ergonomic cost, and it is what removes name resolution from the
parser: `Foo` versus `Foo(x)` versus `Foo { .. }` is decided by the token after
`Foo`, never by what `Foo` means. Section 12.6.

### 7.3 Exhaustiveness

Every `match` must cover its scrutinee's type. The checker reasons about enum
variants, `Bool`, tuples, structs, and array lengths. It does not attempt
exhaustiveness over integer or string ranges; those need a `_` arm.

Unreachable arms are a compile error, not a warning.

---

## 8. Evaluation

### 8.1 Immutability

Every binding is final. There is no assignment operator, no `mut`, no interior
mutability, no aliasing hazard, and therefore no borrow checker and no lifetimes.
"Modifying" a value produces a new one:

```buri
let u2 = User { ..u, name: "new" };
```

An implementation is expected to make this cheap through structural sharing and
opportunistic in-place update when a value is provably not shared. That is an
implementation strategy, not a language rule, and it is never observable.

### 8.2 Strictness and order

Buri is strict. Evaluation order is fully specified:

1. `let` bindings in a block are evaluated top to bottom, before the block's
   result expression.
2. Call arguments are evaluated left to right, then the function is applied.
3. Operands of binary operators are evaluated left to right, except for `&&`,
   `||`, and `??`, which short-circuit.
4. `if` evaluates its condition, then exactly one branch.
5. `match` evaluates its scrutinee, then tests arms in order, evaluating each
   guard only when its pattern matched.

This matters more than it usually would: because effects are performed by
ordinary function calls rather than by a monad, **specified evaluation order is
what makes effect sequencing meaningful.** An implementation may reorder or
eliminate work only where the result is indistinguishable, and calls that consume
a capability are never indistinguishable.

```buri
let _ = io.println(ctx, "first");
let _ = io.println(ctx, "second");    // guaranteed to print second
```

### 8.3 Recursion and tail calls

Recursion is the only looping construct. Implementations **must** eliminate tail
calls, including mutually recursive ones, so that tail-recursive functions run in
constant stack space. This is what makes `fold`, and every accumulator-passing
helper written on top of it, a real loop rather than a stack hazard.

Non-tail recursion that exhausts the stack crashes.

### 8.4 Closures

Lambdas capture by value. Since values are immutable, capture is unobservable —
with one exception: **a lambda may not capture a capability-carrying value**
(Section 10.5).

---

## 9. Functions

```buri
export fn slugify(s: Str): Str { ... }

fn quadratic(a: F64, b: F64, c: F64): Option<(F64, F64)> { ... }

fn retry<T, ..R>(
  ctx: { clock: Clock, ..R },
  attempts: Int,
  action: fn({ clock: Clock, ..R }) => Result<T, Str>,
): Result<T, Str> { ... }
```

- The return type annotation is **required** on every top-level `fn`. Local
  bindings and lambdas are inferred.
- Parameter types are required.
- Trailing commas are allowed in parameter and argument lists.
- Functions are first-class values and may be passed, returned, and stored.
- There is no overloading and no default arguments.

Type inference is Hindley–Milner extended with row polymorphism. Because
top-level signatures are mandatory, inference is local to a function body, and
type errors are reported against the signature you wrote rather than one the
compiler guessed.

---

## 10. Capabilities and purity

This is the part of Buri that is not TypeScript and not Rust.

### 10.1 The model

A **capability** is a value of an opaque, unforgeable type. `core/cap` exports
these primitive capability types:

| Type | Grants |
|---|---|
| `Alloc` | heap allocation |
| `Fs` | file system access |
| `Net` | network sockets and DNS |
| `Clock` | reading wall-clock and monotonic time |
| `Rand` | non-deterministic randomness |
| `Env` | environment variables and process arguments |
| `Stdin` `Stdout` `Stderr` | the standard streams |
| `Proc` | spawning subprocesses |

A **context** is an ordinary record whose fields are capabilities. Contexts are
not a special language construct — a context is just a record, and passing one is
just passing an argument. Conventionally it is the first parameter and is named
`ctx`.

```buri
fn logAndFetch(ctx: { alloc: Alloc, net: Net, stdout: Stdout, .. }, url: Str)
  : Result<Str, Str> { ... }
```

Because record types are row-polymorphic, a function declares the minimum it
needs and callers pass whatever richer context they hold:

```buri
fn caller(ctx: { alloc: Alloc, net: Net, stdout: Stdout, fs: Fs, clock: Clock }) : ... {
  logAndFetch(ctx, "https://example.com")     // fine: ctx has at least what's required
}
```

### 10.2 Where capabilities come from

Nowhere in user code. Capability types are `opaque`, so their constructors exist
only inside the platform module that defines them. The single entry point into a
program is `main`, and the platform hands `main` exactly the context its signature
asks for:

```buri
export fn main(ctx: { alloc: Alloc, stdout: Stdout, fs: Fs }): Result<{}, Str> { ... }
```

If the signature requests a capability the platform does not provide, the program
does not compile. A program that never mentions `Net` in `main`'s signature cannot
open a socket anywhere in its transitive call graph — not in a dependency, not in
a build script, not by accident.

### 10.3 What "pure" means

A type is **capability-carrying** if it is a primitive capability type, or a
struct/enum/record/tuple/array/function type that transitively mentions one.
Otherwise it is **capability-free**.

> **Purity theorem.** If every parameter type of a function `f` is
> capability-free, and `f` captures no capability-carrying value, then any two
> evaluations of `f(a)` on equal arguments produce equal results, perform no
> observable effect, and may be freely cached, reordered, or eliminated.

Top-level functions capture nothing but other top-level declarations, which are
themselves capability-free (capabilities cannot be constructed outside the
platform), so for a top-level `fn` the theorem reduces to a signature check you
can do with your eyes.

Two consequences worth naming:

- Purity is not a keyword and not an effect annotation. It is the *absence* of an
  argument. Nothing needs to be inferred, propagated, or polymorphic over.
- The check is shallow and local. You never have to read a function body, or its
  callees' bodies, to know whether it can touch the world.

### 10.4 Determinism versus effects

`Alloc` is a **resource** capability: it can fail (out of memory) and it costs
something, but it is not observable. Every other primitive capability is an
**effect** capability.

A function is **deterministic** if its parameters are capability-free except for
`Alloc`. `list.map(ctx, f, xs)` is deterministic: it needs to allocate, but it
is referentially transparent. `time.now(ctx)` is not.

Tracking allocation is the reason `[T]`-returning combinators take a context at
all, and it is what makes "this function does no I/O" and "this function does not
allocate" separately expressible:

```buri
fn sum(xs: [Int]): Int                                            // pure, no allocation
fn map<A, B>(ctx: { alloc: Alloc, .. }, f: fn(A) => B, xs: [A]): [B]   // deterministic, allocates
fn readText(ctx: { alloc: Alloc, fs: Fs, .. }, path: Str): Result<Str, IoError>  // effectful
```

Fixed-size construction — struct literals, tuples, enum payloads, array literals,
closures, `Template`s — never requires `Alloc`. Only results whose size depends on
runtime data do.

### 10.5 The capture rule

**A lambda may not capture a capability-carrying value.** Capabilities travel
through parameters only.

```buri
// ERROR: captures ctx
let f = fn(path) => fs.readText(ctx, path);

// OK: takes its context as an argument
let f = fn(c: { alloc: Alloc, fs: Fs }, path: Str) => fs.readText(c, path);
let results = list.mapCtx(ctx, f, paths);
```

Without this rule, a value of type `fn(Str) => Str` could smuggle a file handle
past a signature that looks pure, and the purity theorem would be false. With it,
a function type says everything about what its values can do.

The cost is that effectful higher-order code must thread context explicitly:

```buri
// ERROR: the lambda captures ctx
let texts = list.map(ctx, fn(p) => fs.readText(ctx, p), paths);

// Thread the context through a *Ctx combinator instead
let texts = list.mapCtx(ctx, fn(c, p) => fs.readText(c, p), paths);
```

The standard library provides `*Ctx` variants (`list.mapCtx`, `list.filterCtx`,
`result.andThenCtx`) for exactly this, and explicit recursion is always available
when the combinator does not fit. This is the sharpest trade-off in the language,
and Section 14 lists it as the first open question.

### 10.6 Calling convention

The standard library, and idiomatic Buri, follows:

**receiver first, context second, everything else after.**

```buri
export fn map<A, B>(self: [A], ctx: { alloc: Alloc, .. }, f: fn(A) => B): [B]
export fn readText(ctx: { alloc: Alloc, fs: Fs, .. }, path: Str): Result<Str, IoError>
```

A method leads with `self` because that is what method syntax requires
(Section 6.7.2), and the context follows immediately so that a signature reads
as "this value, in this world, does X." A free function has no receiver, so its
context comes first.

```buri
xs.map(ctx, double)
lines.filter(ctx, isLong).sortBy(ctx, order.str)
```

### 10.7 Attenuation

Because a capability is a value, narrowing authority is just a function. Wrap a
capability in an opaque struct and export only the operations you want to permit:

```buri
// module: safe/readonlyfs
export opaque struct ReadOnlyFs(Fs);

export fn restrict(fs: Fs): ReadOnlyFs { ReadOnlyFs(fs) }

export fn readText(ctx: { alloc: Alloc, .. }, rfs: ReadOnlyFs, path: Str)
  : Result<Str, IoError> {
  fs.readText({ alloc: ctx.alloc, fs: rfs.0 }, path)
}
```

Outside `safe/readonlyfs`, `ReadOnlyFs` cannot be destructured, so the wrapped
`Fs` cannot be recovered. A caller handed a `ReadOnlyFs` can read and nothing
else. The same shape gives you path-scoped file access, rate-limited network
access, or a seeded `Rand` for tests.

### 10.8 Testing

A pure function needs no harness. An effectful function is tested by passing a
context whose capabilities came from a test platform rather than the OS — the
call site does not change, because there was never a global to stub.

---

## 11. Programs

A program is a module that exports `main`:

```buri
export fn main(ctx: { alloc: Alloc, stdout: Stdout, env: Env, .. }): Result<{}, Str>
```

- `main` must take exactly one parameter, a record type.
- `main` must return `Result<{}, Str>`.
- `.Ok({})` exits 0. `.Err(msg)` prints `msg` to stderr and exits 1.
- Trailing `..` in `main`'s context type means "the platform may pass more"; a
  closed record means "grant exactly this and nothing else", which is the
  stricter and recommended form for security-sensitive programs.

### 11.1 Standard library sketch

Non-normative in v0.2, but the examples assume it. The purity tier of each entry
is the part that matters.

Two conventions run through the whole library: **receiver first, context second**
(Section 10.6), and **a name has one meaning**. Everything below that operates on
a value declares that value as `self`, so it is callable as a method —
`xs.map(ctx, f)`, `s.trim()`, `opt.withDefault(0)` — with no import. There is no overloading, so a pure variant and an
allocating variant of the same idea get different names (`splitOnce` returns two
slices and is pure; `split` returns `[Str]` and allocates).

**Pure — no context parameter at all**

| Module | Functions |
|---|---|
| `core/list` | `len`, `get`, `isEmpty`, `first`, `last`, `fold`, `foldResult`, `any`, `all`, `find`, `sum` |
| `core/str` | `len`, `isEmpty`, `slice`, `trim`, `charAt`, `startsWith`, `endsWith`, `contains`, `splitOnce`, `toInt`, `compare` |
| `core/option` | `map`, `andThen`, `withDefault`, `isSome` |
| `core/result` | `map`, `mapErr`, `andThen`, `withDefault`, `ignore`, `isOk` |
| `core/num` | `abs`, `min`, `max`, `signum`, `compare`, and the `Bounded` / `Checked` / `Wrapping` trait methods |
| `core/math` | `sqrt`, `pow`, `floor`, `ceil`, `round` |
| `core/bits` | `shl`, `shr`, `popCount` |
| `core/order` | `reverse`, and the `Ord`, `Eq`, `Show`, `Hash` trait declarations |
| `core/char` | `isDigit`, `isAlpha`, `isSpace`, `toLower`, `toUpper` |
| `core/bool` | `not`, `and`, `or`, `toStr` |

`str.trim`, `str.slice`, and `str.splitOnce` are pure because `Str` is immutable
and sliceable: they return views, not copies. `fold` is pure because it produces
one value rather than a new collection.

**Deterministic — requires `{ alloc: Alloc, .. }`**

| Module | Functions |
|---|---|
| `core/list` | `map`, `mapCtx`, `filter`, `filterCtx`, `concat`, `push`, `reverse`, `sortBy`, `take`, `drop`, `zip`, `range` |
| `core/str` | `concat`, `join`, `split`, `splitAny`, `replace`, `repeat`, `toUpper`, `toLower`, `fromInt`, `format`, `chars` |

**Effectful — requires an effect capability**

| Module | Needs | Functions |
|---|---|---|
| `core/cap` | — | the capability types themselves |
| `core/io` | `Stdout`/`Stderr`/`Stdin` | `print`, `println`, `eprintln`, `readLine` |
| `core/fs` | `Fs` | `readText`, `writeText`, `exists`, `listDir`, and the `IoError` type |
| `core/net/http` | `Net` | `get`, `post`, `Request`, `Response`, `errorText` |
| `core/time` | `Clock` | `now`, `since`, `sleepMs`, `Instant` |
| `core/random` | `Rand` | `int`, `float` |
| `core/env` | `Env` | `get`, `args` |

The `*Ctx` variants (`list.mapCtx`, `list.filterCtx`) take a callback of the form
`fn(Ctx, A) => B` and pass the context through, which is how effectful
higher-order code is written given the capture rule of Section 10.5.

---

## 12. Why the grammar is context-free and unambiguous

Each item below is a deliberate design decision, listed with what it costs.

**12.1 `if` and `match` subjects are parenthesized.**
`if (c) { ... }` closes the condition at `)`, so the `{` that follows is always a
block. This is the ambiguity that forces Rust to ban struct literals in condition
position. *Cost:* two characters, and it looks like TypeScript anyway.

**12.2 There are no expression statements.**
A block is `let`s followed by a result expression. Nothing can sit next to a
`{`-initial expression and compete with it. *Cost:* a call performed only for its
effect must be bound: `let _ = io.println(ctx, "hi");`. Arguably a feature.

**12.3 Record literals have no field shorthand.**
`{ x }` would be ambiguous: a record with a shorthand field, or a block whose
result expression is `x`. Requiring `{ x: x }` makes one token of lookahead
(`:` or not) decide it. *Cost:* mild verbosity at record construction sites.
Record *patterns* keep shorthand, because patterns are never blocks.

**12.4 Type arguments in expressions use the turbofish `::<T>`.**
`f<a>(b)` is genuinely ambiguous with two comparisons. Types have no comparison
operators, so `<` is unambiguous inside a type; expressions get `::<`.
*Cost:* the turbofish is ugly. It is also rare.

**12.5 There is no `<<` or `>>` token.**
Longest-match lexing would turn `Map<Str, [Int]>>` into a shift. Removing the
operators removes the problem, rather than papering over it with a token splitter
that makes the lexer position-dependent. *Cost:* `bits.shl(x, n)`.

**12.6 Enum variants in patterns must be qualified or dot-prefixed.**
Otherwise `None` is a binding or a variant depending on what is in scope, which
makes the parser depend on name resolution. A bare `IDENT` in a pattern is always
a binding; `IDENT(` and `IDENT{` are struct patterns; `A.B` is a path. Decided by
one token of lookahead. *Cost:* `.None` instead of `None`.

**12.7 Tuples have arity ≥ 2 and unit is `{}`.**
`(e)` is grouping, full stop — no `(e,)` special case, no zero-tuple competing
with the empty record. *Cost:* none worth mentioning.

**12.8 Lambdas begin with `fn`.**
`(x) => ...` requires deciding whether `(x)` is a parenthesized expression or a
parameter list, which is a reduce/reduce conflict at `)`. A leading `fn` decides
it at token one. *Cost:* two characters; it also makes lambdas and function types
read the same.

**12.9 `else` is mandatory and branches are blocks.**
Kills the dangling-else ambiguity, and prevents `if (c) { a } else { b } + 1`
from having two parses. *Cost:* you must say what the other case is.

**12.10 Lambdas and `crash` are top-level-only expressions.**
Their bodies extend maximally to the right, so allowing them as operands would
make `2 * fn(x) => x + 1` ambiguous. Block-like expressions (`{}`, `if`, `match`)
*are* allowed as operands, because they are brace-terminated and self-delimiting.
*Cost:* parentheses around a lambda used as an operand.

**12.11 Match arms are comma-separated, always.**
Without a required separator, `A => x` followed by an arm starting `-1 =>` would
greedily parse as `x - 1`. *Cost:* a comma after `}`.

**12.12 Block-like expressions cannot head a postfix chain.**
`match (x) { ... }.field` is a parse error; parenthesize. This is what stops
`if (c) { a } else { b } { x: 1 }` from parsing two ways once struct literals are
a postfix form. *Cost:* occasional parentheses.

**12.13 Float literals must start with a digit.**
So `pair.0` lexes as three tokens. *Cost:* write `0.5`.

**12.14 Every declaration starts with a distinct keyword.**
`from` `export` `fn` `struct` `enum` `type` `const` `opaque`. Top-level parsing
is a switch on one token — and putting `from` first on an import means the module
path is known before the specifier list is parsed, which is what makes
completion inside the braces possible.

**12.15 Method calls reuse `.` rather than taking a token of their own.**
`sq.area()` needs no new production: it is the existing `PostfixExpr "." IDENT`
followed by a call, and name resolution decides whether `area` is a field or a
method. The alternative, `sq:area()`, would visibly separate data from
computation — but `:` after an expression breaks the one-token lookahead that
tells `{ foo: bar }` (record literal) from `{ foo:bar() }` (block whose result is
a method call), which is the very disambiguation that cost record literals their
field shorthand in 12.3. Buying `:` back would mean moving record literals to
`{ x = 1 }`. *Cost:* `.` now carries four meanings — field, tuple index, module
member, method — all resolved after parsing, and a method may not share a name
with a field of the same type.

**12.16 `self` is a keyword in a fixed position, not a convention.**
A function is a method because its first parameter is literally written `self`,
so "is this a method?" is answered by the parser, not by comparing types against
a rule about argument order. `impl`, `derive`, and `trait` likewise each begin
with a distinct keyword, keeping top-level parsing a switch on one token.

Two lexical warts remain and are accepted:

- `t.0.1` lexes as `t` `.` `0.1`. Write `(t.0).1`.
- `Foo<Bar<Int>>= x` lexes `>` `>=`. Write a space: `Foo<Bar<Int>> = x`.

---

## 13. Static rules not expressed in the grammar

The grammar accepts a superset of well-formed programs. These are checked
afterward:

1. The head of a struct literal (`Expr { ... }`) must be a type path — optionally
   with a turbofish, or the inferred-type dot form `.Variant` — not an arbitrary
   expression. The grammar permits `f(x) { a: 1 }`; the checker does not.
2. `let` patterns must be irrefutable.
3. `match` must be exhaustive, and no arm may be unreachable.
4. Or-pattern alternatives must bind identical names at identical types.
5. Array rest patterns appear only in final position; at most one per pattern.
6. Record type field names, struct field names, enum variant names, and match-arm
   pattern bindings must each be unique within their scope.
7. `main` has the signature required by Section 11.
8. A lambda may not capture a capability-carrying value (Section 10.5).
9. Opaque types may not be constructed or destructured outside their defining
   module, and private fields may not be read, written, or matched outside it.
10. `as` is permitted only for the conversions in the table of Section 6.2.1;
    `as?` and `as%` are permitted between any two numeric types, plus
    `Char as U32` and `U32 as? Char`.
11. A numeric literal must be representable in the type it resolves to.
12. The dot form (`.Variant`) requires a known expected type.
13. `?` requires the enclosing function's return type to be a compatible `Result`
    or `Option`.
14. Recursive type definitions must be productive (a recursive enum must have at
    least one variant that does not recurse).
15. The head of a struct literal may not be a block-like expression; neither may
    the head of any postfix chain.
16. A value of type `Result<T, E>` may not be discarded by a `_` pattern
    (Section 5.7.1). Use `?`, `match`, `result.withDefault`, or the explicit
    `result.ignore`.

Methods and traits:

17. `self` may appear only as a function's first parameter. A function with a
    `self` parameter is a method on that parameter's type, which must be a type
    declared in the same module.
18. A method may not share a name with a field of its `self` type.
19. A method call `x.f(...)` requires the receiver's type to be known and to have
    a defining module (Section 6.7.3), or to be a type parameter whose bounds
    declare `f`.
20. A method is not a value: `x.f` must be immediately called.
21. `Self` is legal only inside a `trait` or `impl` body.
22. An `impl` may appear only in the defining module of its type, and must supply
    every method the trait declares, with matching signatures.
23. `derive` requires every field type (for a struct) or payload type (for an
    enum) to satisfy the derived trait.
24. A generic parameter's bounds must name declared traits. Inside the function,
    only the methods those traits declare are callable on that parameter.
25. Record *types* may not carry `export` on their fields — visibility is a
    property of nominal declarations, not of structural types.

## 14. Non-goals and open questions

**Not in v0.2, and not planned:** mutation, references, lifetimes, classes,
inheritance, dynamic dispatch, trait objects, `null`, exceptions, implicit
conversions (beyond `Str → Template`), overloading, macros, reflection.

**Not in v0.2:** loops, and the `|>` pipe operator. See below.

**Deferred to a later version:** blanket implementations; associated types;
`where` clauses; supertraits; implementing a trait for a foreign type; dict and
set literals; fixed-length array types; `async`; ranges; a module-level effect
summary in generated documentation; user-definable primitive capabilities.

Each item in that first deferred group is a step from "trait resolution is a
lookup" toward "trait resolution is a search," which is the entire compile-time
cost of a trait system. They are deferred together, deliberately.

### 14.1 Considered and cut: `for` and `while`

A `for`/`while` sugar was fully specified for this version and then removed. It
is recorded here because the reasoning constrains any future attempt.

The design was: `for (x in xs) with (acc = init) { body }` desugaring to a
tail-recursive local function, where the body evaluates to the next accumulator;
`while (cond) with (...)` likewise; plus a `Range` type and `a..b` / `a..=b`
operators so that counting loops would not have to allocate an array.

What it bought: familiar syntax for folds, and — the strongest argument — an
exemption from the capture rule of Section 10.5, since a loop body is inlined
control flow rather than a value of function type. Effectful iteration could be
written directly instead of through a `*Ctx` combinator.

What it cost, and why it lost:

- **Two ways to say one thing.** `for (x in xs) with (n = 0) { n + x }` and
  `list.fold(fn(n, x) => n + x, 0, xs)` are the same program. A small language
  that offers both has to teach both, and every codebase splits on which to use.
- **The sugar was not simple.** A `with` clause whose scope differs between
  `for` and `while`, an optional index binding, a body typing rule that changes
  with the presence of `with`, a special termination check for capability-free
  `while` conditions, plus a `Range` type, a `core/range` module, and two new
  operators with their own ambiguity argument. That is a lot of specification
  for zero new expressive power.
- **It made the capture rule inconsistent.** Exempting loop bodies is sound, but
  it means "can this construct see a capability?" stops having one answer. Better
  to keep the rule absolute and treat its cost as the open question it is.

If loops return, the case to beat is: they must earn their keep on something
other than familiarity, and the capture-rule exemption should be solved directly
rather than routed around.

### 14.2 Considered and cut: the `|>` pipe operator

`x |> f(a)` meant `f(a, x)`, and it is why the standard library originally put
its data *last*. Method syntax (Section 6.7) covers the case that mattered —
chaining operations that belong to a type — and covers it with resolution that
needs no import. What remained for `|>` was chaining a function that is not a
method of the receiver's type, which reads at least as well as a `let` sequence
in a language that already has no expression statements.

By the same standard that cut loops, it did not earn its keep. Removing it also
freed the argument convention: with `|>` gone, the receiver could move to the
front (Section 10.6), where it reads correctly for methods and for direct calls
alike.



**Open questions, honestly flagged:**

1. *The capture rule (10.5) is strict.* It buys a clean purity theorem at the cost
   of ergonomic effectful higher-order code: every effectful traversal goes
   through a `*Ctx` combinator or hand-written recursion. The alternative —
   encoding a captured-capability row in the function type, e.g.
   `fn(Str) => Str uses { fs: Fs }` — is more expressive but adds an effect system
   to a language whose selling point is not having one. This is the language's
   sharpest unresolved trade-off, and cutting loops (14.1) put the full cost back
   on it. Traits do not help: a trait method that needs a capability must declare
   the context in its signature, which is the honest encoding but not a
   convenient one.
2. *`Alloc` granularity.* Requiring `Alloc` for every size-dependent result is
   principled and noisy. Whether the noise is worth the guarantee is an empirical
   question.
3. *Indexing returns `Option`.* Correct, and occasionally miserable. A
   `list.getOr(default, i, xs)` helper and better pattern-matching over arrays may
   absorb most of the pain.
4. *Structural satisfaction can happen by accident.* A module that exports a
   matching `compare` satisfies `Ord` whether or not it meant to. `impl` exists
   to state intent, but nothing requires it. Go has lived with this; whether the
   `impl` opt-in should become mandatory is unsettled.
5. *Methods are not extensible, and not available on type variables* (6.7.3).
   Resolving through the receiver's defining module is what makes them
   import-free and collision-free, and it is the same property that stops you
   adding an operation to `Str`. Calling a method on a bare `T` is what bounds are
   for (5.10); extending a foreign type is what free functions are for. Neither
   gap has a fix that preserves import-free, collision-free resolution.
6. *Row polymorphism versus subtyping* for contexts. Rows were chosen for
   inference; subtyping would allow a nominal `Ctx` type with better error
   messages.
7. *Holding the line on 5.12.5.* Restricted traits are cheap precisely because
   resolution is a lookup. Every deferred feature — blanket impls, associated
   types, `where` chains, foreign impls — individually looks reasonable and
   collectively turns the lookup into a search. By then the compiler's
   architecture will assume constant-time resolution. The risk is not the cost of
   what was built; it is the difficulty of refusing the next request.
8. *`I64` on a JavaScript target.* `Int` is `I64` on every target, which is the
   right call for portability and the expensive one for the JS backend, where it
   means `BigInt` or a two-word representation for the type that ordinary code
   reaches for by default. The alternatives — making `Int` 32-bit, or making its
   width target-dependent — trade a real performance problem for a real
   correctness one. Unresolved, and the sharpest tension between the "runs fast"
   and "compiles to JavaScript" goals.
9. *Must-use is hard-coded to `Result` (5.7.1).* A general `@mustUse` marker on
   user types would be more honest than a compiler that knows one type by name,
   but it is the first piece of attribute syntax in a language with none, and
   `Result` covers the case that actually bites. Revisit if a second must-use
   type shows up in practice.
