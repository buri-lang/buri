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

Everyday code writes `Int` and `Float`. Code with a size on the wire writes an
exact width. **One set of types, two names for the common ones**:

```buri
type Int = I64; // the default integer

type Float = F64; // the default float

type Uint = U64;

type Byte = U8;
```

These are **aliases, not distinct types**. `Int` and `I64` are the same type, so
a function declared with `Int` and one declared with `I64` interoperate with no
conversion. Diagnostics print whichever spelling the program used. There is no
third category and no numeric tower.

**Every integer type holds its whole range on every backend.** On JavaScript the
widths up to 32 bits compile to a `number` and the widths at 64 bits and above to
a `BigInt`, which is a heap value rather than an immediate. Code on a hot path
that does not need the range says `I32` and gets the faster representation.

#### Literals are polymorphic until they are pinned

A numeric literal has no type on sight. It gets a fresh type variable constrained
to the integer types, or to the float types for a float literal, and ordinary
unification decides from there. The default applies only when nothing constrains
it: `Int` for an integer literal, `Float` for a float literal.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let a = 5;               // nothing constrains it -> Int
let b: U8 = 5;           // the annotation pins it -> U8, no conversion
let c: F32 = 1.5;        // -> F32
takesU16(5)              // the parameter pins it -> U16

let e: [U8] = [1, 2, 3]; // every element is a U8
```

This is defaulting on *literals only*. It is not overloading and not a numeric
tower: `a + b` still requires `a` and `b` to already have the same type.

The compiler knows a literal's type before it checks it, so **a literal that does
not fit its type is a compile error**, not a runtime surprise:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let x: U8 = 300; // ERROR: 300 is not representable in U8

let y: I8 = -129; // ERROR

let z: U32 = -1; // ERROR: U32 has no negative values

let w: U64 = 18_446_744_073_709_551_615; // fine
```

There are no literal suffixes (`5u8`). An annotation or the call site pins a
literal's type, and a conversion method changes a value's (Section 6.2.1).

#### Generic numeric code

Arithmetic is available on a type parameter through the operator traits of
Section 5.12 — `Add`, `Subtract`, `Multiply`, `Divide`, `Remainder`, `Negate`, `Ordered` — each of which
is an ordinary interface with a method set:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
fn total<N: Add>(zero: N, xs: [N]): N { ... }
fn clamp<N: Ordered>(lo: N, hi: N, x: N): N { ... }
```

There are no compiler-privileged bounds. A bound names what a type *can do*, so
the integer-specific operations are interfaces too: `Bounded`, `Checked`,
`Wrapping`, and `Saturating`, declared in Section 6.2.2. Every built-in integer
type satisfies all four; the float types satisfy `Bounded` only.

### 5.2 Unit

The unit type and its only value are both written `()`. Functions that exist
only for their effect return `()`.

```buri
# from "core/effect" import { Stdout };
# from "core/io" import * as io;

fn log<C: Stdout>(ctx: C, msg: Str): () {
    io.println(ctx, msg).ignore()
}
```

### 5.3 Tuples

Tuples have arity 2 or more. `(T)` is a parenthesized type, not a 1-tuple.

```buri wrap=body
let pair: (Int, Str) = (1, "one");
let first = pair.0;
let (n, name) = pair;
```

Tuple element access is `.0`, `.1`, … . Because `0.1` lexes as a float, nested
access must be parenthesized: `(t.0).1`.

### 5.4 Arrays

`[T]` is an immutable, densely packed sequence of `T`.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let xs: [Int] = [1, 2, 3];
let n = list.length(xs);          // pure: no allocation
let maybe = xs[0];             // Option<Int>, not Int
```

**Indexing yields `Option<T>`.** There is no way to index out of bounds.

An array literal has a statically known length, so it is not by itself an
allocation you must account for. Any operation whose result length depends on
runtime data — `map`, `filter`, `concat`, `sort`, `range` — needs an `Allocator`
effect.

### 5.5 No records

There are no anonymous record types and no record literals. Every product type
is a `struct` with a declared name (Section 5.6), and every type in the language
is nominal — including trait conformance (Section 5.12). There is no row
polymorphism.

### 5.6 Structs

Structs are nominal. Two structs with identical fields are different types.

Fields are **module-private unless exported**, the same rule that governs
declarations:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
struct User {
  export id: UserId,
  export name: Str,
  passwordHash: Str,          // private to this module
}

struct Meters(F64);                        // tuple struct
struct Pair<A, B>(A, B);                   // generic tuple struct

let u = User { id: UserId("u1"), name: "Ada", passwordHash: hash };
let shorthand = User { id, name, passwordHash };   // shorthand: `name: name`
let u2 = User { ..u, name: "Ada L." };
let d = Meters(9.8);
let raw = d.0;
```

Tuple-struct fields carry the same `export` marker, in the same position:

```buri
struct Meters(export F64); // the F64 is readable as `m.0` anywhere

struct UserId(Str); // the Str is readable only in this module
```

Tuple-struct declarations are terminated with `;`; record-struct declarations are
not.

A literal gives every **required** field a value. A field whose declared type is
`Option<...>` is not required: leaving it out of a literal is writing `.None`
for it.

```buri
struct World {
    export hi: Str,
    export hello: Option<Str>,
}

fn plain(): World {
    World { hi: "hi" }
} // `hello` is `.None`

fn given(): World {
    World { hi: "hi", hello: .Some("hello") }
}
```

The declaration decides which fields you may leave out, not one instantiation of
it. Aliases are transparent (Section 5.9), so you may leave out a field declared
`Maybe` where `type Maybe = Option<Str>`. But you may never leave out a field
declared `T` in a `struct S<T>`, at any instantiation — `S<Option<Int>>` still
writes its `T`. A left-out `Option<Option<T>>` is the *outer* `.None`.

Elision fills only what neither an initializer nor a spread provides, and a
spread provides every field the literal does not write — so `World { ..base }`
takes `hello` from `base` rather than resetting it to `.None`.

You may leave the type name out where the surroundings already give it. The
braces then build whatever the compiler checks the expression against:

```buri
struct World {
    export hi: Str,
    export hello: Option<Str>,
}

fn takes(w: World): Str {
    w.hi
}

fn annotated(): World {
    let w: World = { hi: "hi", hello: .None };
    w
}

fn argument(): Str {
    takes({ hi: "hi" })
}

fn result(): World {
    { hi: "hi" }
}
```

The compiler **reads** the type from above and never solves for it. It reaches a
literal in a `let` with an annotation, an argument of a call, the value of a
field, a match arm, and a function's result. A literal with nothing above it to
name its type is `struct-literal-type`, and so is one whose expected type is an
enum, a primitive, or a generic struct with a type argument nothing has settled.

The parser reads the braces as a literal when a `..` or a `name :` follows the
`{`, and as a block otherwise (`design/grammar-rationale.md` 12.3). So a literal whose *first*
field is shorthand keeps its type name — `World { hi }`, `World { hi, hello }` —
while shorthand after a first keyed field does not: `{ hi: hi, hello }` is a
literal. `{}` keeps its type name too.

Outside the declaring module, you cannot read a private field, write it in a
literal, or match it, so you cannot build a struct with any private field from
scratch elsewhere. Functional update still works, because it never names the
hidden fields:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let renamed = User { ..u, name: "new" };     // fine anywhere
let forged = User { id: ..., name: ..., passwordHash: ... };   // only in the
                                                               // declaring module
```

This is the only visibility mechanism a struct has. There is no `opaque`
modifier: a struct with no exported fields already hides its whole
representation.

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
    Node(Tree<T>, T, Tree<T>), // recursive; boxed by the runtime
}
```

A variant writes no `export`. The enum is the unit of visibility: an exported
enum exports every one of its variants and every field of their payloads, and a
private one exports none. Writing `export` before a variant is the
`variant-export` error, which carries the edit that deletes it. A type whose
representation should stay hidden is a struct with a private field.

Constructing a variant uses a qualified path or the inferred-type dot form:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let a = Shape.Circle(1.0);
let b: Shape = .Rect { width: 2.0, height: 1.0 };
let c: Shape = .Empty;
```

The dot form needs the expected type from context: a `let` annotation, a
parameter type, the enclosing function's return type, or a `match` scrutinee's
type. Without one, use the qualified form.

The prelude defines:

```buri
enum Option<T> {
    Some(T),
    None,
}

enum Result<T, E> {
    Ok(T),
    Err(E),
}

enum Order {
    Less,
    Equal,
    Greater,
}
```

#### 5.7.1 `Result` is must-use

**A value of type `Result<T, E>` may not be discarded.** Discarding means binding
it to a `_` anywhere in a pattern, or leaving it standing as a statement:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let _ = fs.writeText(ctx, path, body);                // ERROR: discarded Result
let (n, _) = (1, fs.writeText(ctx, path, body));      // ERROR: the same one, hidden
fs.writeText(ctx, path, body);                        // ERROR: and so is this
```

There are two ways to throw a value away and no third: a `_` in a `let`'s
pattern, and an expression statement, which `design/grammar-rationale.md` 12.2 admits only in a test
source. This rule refuses a `Result` in both, and looks for the `_` anywhere in
the pattern rather than only at its head.

The legal ways to consume a `Result` are:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
fs.writeText(ctx, path, body)?                        // propagate
match (fs.writeText(ctx, path, body)) { ... }         // handle
fs.writeText(ctx, path, body).withDefault(())         // supply one
fs.writeText(ctx, path, body).ignore()                // explicitly, greppably, ignore
```

`ignore(self): ()` is a method on `Result` and has no free-function spelling. A
reviewer can grep for it, where `_` is unsearchable, and `buri lint` reports
every `ignore` as `discarded-result`.

The rule is on the type, not on the call: a `Result` from a pure function is
every bit as must-use as one from an I/O call. That includes `io.print` and
`io.println`, which answer `Result<(), IoError>` because a closed pipe, a full
disk and a revoked permission all happen to prints.

`Option` is **not** must-use (`design/non-goals.md`).

### 5.8 Function types

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
fn(Int, Int) => Int
fn() => ()
fn(Str) => Result<Config, ParseError>
```

Function types use the `fn` keyword for the same reason lambdas do: it makes
`(A, B)` unambiguously a tuple everywhere. Function types are rank-1; there are
no polymorphic function *values* in v0.3.

### 5.9 Type aliases

```buri
type UserId = Str;

type Handler<T> = fn(T) => Result<(), Str>;
```

Aliases are transparent: `type UserId = Str` makes `UserId` and `Str` the same
type. For a distinct type, use a tuple struct: `struct UserId(Str);`.

An alias may be exported, imported and re-exported like any other declaration
(Section 4.2). It expands in the module that declared it, so `type Handle =
LocalStruct` means the same thing wherever the name is read.

An alias whose body reaches itself is `circular-type-alias`, and the alias
resolves to the error type. That covers reaching itself directly, through other
aliases, in this module, or across a boundary an export carried it over. Two
aliases that reach the same type by different routes are not a cycle; only a walk
that returns to where it started is. Write a recursive *type* with a struct or an
enum.

### 5.10 Generics

You declare type parameters in angle brackets. There are no row parameters.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator, Stdout };
fn identity<T>(x: T): T { x }
fn map<A, B, C: Allocator>(self, ctx: C, f: fn(A) => B): [B] { ... }
fn tee<T, C: Stdout>(ctx: C, x: T): T { ... }
```

A parameter may carry one or more **bounds**, naming traits the argument type
must satisfy. Multiple bounds are joined with `+`:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Allocator };
fn largest<T: Ordered>(xs: [T]): Option<T> { ... }
fn report<T: Ordered + Show, C: Allocator>(ctx: C, xs: [T]): Str { ... }
```

Inside such a function you may call the bound's methods on the parameter —
`x.compare(y)`, `x.show(ctx)` — and nothing else. Generic code that needs an
operation no trait provides takes it as a function argument: `sortBy(xs, cmp)`.

In *expression* position, write explicit type arguments on the expression
itself:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let f = identity<Int>;
let e: [Int] = list.empty<Int>();
```

### 5.11 Equality and ordering

**Equality is structural, never referential.** `a == b` compares values: fields
in declaration order for a struct, the variant plus its payload for an enum,
element-wise for arrays and tuples, recursively all the way down. Two separately
constructed values with equal contents are equal.

Referential equality is **not expressible**. Buri has no references, so there is
no identity to compare, and the runtime may share a representation between two
equal values or copy one whenever that is faster (Section 8.1). Code that needs
identity carries it as data — `struct NodeId(U64)` — which is a value the
compiler cannot invent or coalesce.

`==` and `!=` are `Equal.equal`; `<` `<=` `>` `>=` are `Ordered.compare`. Section 5.12.4
has the operator table. Neither is compiler magic: a type has them because it
derives or implements the trait. Every primitive, and `[T]` and tuples built from
types that have them, satisfy `Equal` and `Ordered` already. Your own structs and enums
opt in:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
derive Equal, Ordered for Version;

let same = Version { major: 1, minor: 2 } == Version { major: 1, minor: 2 };
// true — different values, equal contents
```

`Equal` is not defined for function types or `Template`, so comparing those is a
compile error.

Three consequences:

- **A derived `Equal` is an equivalence relation, and so is `==` on a float.**
  `NaN == NaN` (Section 6.2), so a struct with an `F64` field holding `NaN` is
  equal to itself *and* to a separately built copy of itself. `Ordered` on floats is
  unchanged and still IEEE-754's: it orders `-0.0` equal to `0.0` and reports
  `NaN` as unordered, so `<` and `compare` disagree with `==` at `NaN`. `==` is
  the one made total.

- **A hand-written `impl Equal` need not be structural.** Nothing checks that it is
  reflexive, symmetric, or transitive, so a case-insensitive `Str` wrapper is
  expressible — and so is a broken one. `derive` cannot be wrong in that way.

- **`Ordered` on a `Str` is by Unicode scalar value.** That is the unit `len` counts
  and `charAt` hands back, and for a valid string it is byte-for-byte UTF-8
  order — not the UTF-16 code-unit order a JavaScript `<` gives. Both backends
  answer the scalar order, and `sort`, an `OrderedMap<Str, _>` and `core/order`'s
  `str` all carry it. `Ordered` on a `Char` is the scalar's integer order. The
  language has no locale-aware comparison.

### 5.12 Traits

A trait is an **interface**: a named set of method signatures that a type may
satisfy.

```buri
# from "core/effect" import { Allocator };

trait Ordered {
    fn compare(self, other: Self): Order;
}

trait Show {
    fn show<C: Allocator>(self, ctx: C): Str;
}
```

`Self` stands for the implementing type and is legal only inside a trait or an
`impl`. A trait's methods declare `self` first and without a type, exactly like
any other method (Section 6.7.1).

A trait declared `effect` also marks its implementors effect-carrying, which puts
them under the `ctx` rule of Section 10.2. That modifier is the only difference
between an effect and an ordinary interface.

#### 5.12.1 Conformance is nominal

A type satisfies a trait only where an `impl` or a `derive` says so. Declaring a
method that happens to match a trait's signature does not make the type conform.
The compiler infers nothing from shape.

Checking `T: Ordered` is therefore a lookup in one table keyed by `(trait, type)`.
There is exactly one candidate, so there is no coherence pass, no orphan rule,
and no instance search.

#### 5.12.2 `impl`

`impl Trait for Type` declares conformance and supplies the methods:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
impl Ordered for Version {
  fn compare(self, other: Version): Order { ... }
}
```

This is the same block that declares a type's own methods (Section 6.7.1), with
a `for` clause added. The methods land in the same namespace, so
`v.compare(other)` resolves the way any method does (Section 6.7.3).

The two forms differ in one respect: you may `export` a method of the type's own,
and you may not `export` a method supplied to a trait. An `impl` in either form
may appear only in its type's defining module, so nobody can implement a trait
for someone else's type.

A supplied method's signature is the trait's. Its parameters, its return type,
and its own type parameters — how many, and what each is bound by — are what the
trait declared, reading `Self` as the implementing type. So `compare` above may
write `Version` or `Self` for its second parameter.

#### 5.12.3 `derive` generates the implementation

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
derive Equal, Ordered, Show for Version;
```

`derive` generates the trait's methods structurally: struct fields in declaration
order, enum variants in declaration order, recursing into field types.

Derivation is available for `Equal`, `Ordered`, `Show`, `Hash`, `ToJson`, `FromJson`,
and the operator traits. A `derive` fails to compile if any field's type does not
itself satisfy the trait.

`ToJson` and `FromJson` — `core/json`'s typed encoding — are *only* ever derived.
The compiler rejects an `impl` of either, because a hand-written one would be
obeyed where the type is encoded on its own and ignored where a type holding it
is. `core/json` states the mapping from Buri shapes onto JSON ones.

#### 5.12.4 Operators are trait methods

| Operator | Trait method |
|---|---|
| `a + b` | `Add.add` |
| `a - b` | `Subtract.subtract` |
| `-a` | `Negate.negate` |
| `a * b` | `Multiply.multiply` |
| `a / b` | `Divide.divide` |
| `a % b` | `Remainder.remainder` |
| `a == b`, `a != b` | `Equal.equal` |
| `a < b`, `a <= b`, `a > b`, `a >= b` | `Ordered.compare` |

This is what makes newtype wrappers work:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
struct Meters(F64);
derive Add, Subtract, Ordered, Show for Meters;

let total = Meters(1.5) + Meters(2.0);     // Meters
let far = total > Meters(3.0);             // Bool
// let bad = Meters(1.5) + 2.0;            // ERROR: F64 is not Meters
```

`derive Add for Meters` provides `Meters + Meters` and nothing else, so the unit
safety the newtype exists for survives contact with arithmetic.

**An operator implementation cannot allocate or perform an effect.** `a + b` has
no argument position to pass a context through. That is why `Matrix + Matrix` is
not expressible: matrix addition allocates, so you write `a.add(ctx, b)`.

#### 5.12.5 What traits deliberately lack

No blanket implementations, no associated types, no `where` clauses, no
supertraits, no trait objects, and no dynamic dispatch. Each of those turns
resolution from a lookup into a search, which is the entire compile-time cost of
a trait system. The compiler monomorphizes generic code, and typechecks a generic
body once, polymorphically, verifying bounds at the call site.

---
