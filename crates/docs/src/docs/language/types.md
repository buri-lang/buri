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

Everyday code writes `Int` and `Float`; code that cares about size writes an
exact width.

```buri
type Int = I64; // the default integer

type Float = F64; // the default float

type Uint = U64;

type Byte = U8;
```

These are **aliases**: `Int` and `I64` are the same type and mix freely.
Diagnostics print whichever spelling the program used.

**Every integer type holds its whole range on every backend.** On JavaScript,
widths up to 32 bits are a `number` and wider ones a heap-allocated `BigInt`, so
hot code that doesn't need the range should use `I32`.

#### Literals are polymorphic until they are pinned

A numeric literal can be any integer type (or any float type, for a float
literal) until inference pins it. If nothing does, it's `Int` or `Float`.

```buri
# fn takesU16(n: U16): U16 {
#     n
# }
#
# fn demo(): U16 {
    let a = 5; // nothing constrains it -> Int
    let b: U8 = 5; // the annotation pins it -> U8, no conversion
    let c: F32 = 1.5; // -> F32
    let d = takesU16(5); // the parameter pins it -> U16
    let e: [U8] = [1, 2, 3]; // every element is a U8
#     d
# }
```

This applies to *literals only*: `a + b` still requires `a` and `b` to have the
same type.

**A literal that doesn't fit its type is a compile error:**

```buri wrap=body
let x: U8 = 300; // ERROR: 300 is not representable in `U8`
let y: I8 = -129; // ERROR: -129 is not representable in `I8`
let z: U32 = -1; // ERROR: -1 is not representable in `U32`
let w: U64 = 18_446_744_073_709_551_615; // fine
```

There are no literal suffixes like `5u8`. To change a value's type, use a
conversion method (Section 6.2.1).

#### Generic numeric code

Generic arithmetic uses the operator traits of Section 5.12: `Add`, `Subtract`,
`Multiply`, `Divide`, `Remainder`, `Negate`, `Ordered`.

```buri sig
fn total<N: Add>(zero: N, xs: [N]): N;

fn clamp<N: Ordered>(lo: N, hi: N, x: N): N;
```

No bound is privileged. Integer-specific operations are ordinary traits too:
`Bounded`, `Checked`, `Wrapping`, and `Saturating` (Section 6.2.2).

### 5.2 Unit

The unit type and its only value are both `()`. Functions called only for their
effect return it.

```buri
# from "core/io" import * as io;
# from "platform/effect" import { Stdout };

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

`0.1` lexes as a float, so nested access needs parentheses: `(t.0).1`.

### 5.4 Arrays

`[T]` is an immutable, densely packed sequence of `T`.

```buri wrap=body
let xs: [Int] = [1, 2, 3];
let n = xs.length(); // pure: no allocation
let maybe = xs[0]; // Option<Int>, not Int
```

**Indexing yields `Option<T>`**, so it can't go out of bounds.

An array literal needs no allocator. An operation whose result length depends on
runtime data, like `map`, `filter`, `concat`, `sort` or `range`, needs an
`Allocator`.

### 5.5 No records

There are no anonymous record types. Every product type is a named `struct`, and
every type, including trait conformance, is nominal. There is no row
polymorphism.

### 5.6 Structs

Two structs with identical fields are different types. Fields are
**module-private unless exported**:

```buri
# struct UserId(Str);
#
struct User {
    export id: UserId,
    export name: Str,
    passwordHash: Str, // private to this module
}

struct Meters(F64); // tuple struct

struct Pair<A, B>(A, B); // generic tuple struct

fn examples(id: UserId, name: Str, passwordHash: Str, hash: Str): F64 {
    let u = User { id: UserId("u1"), name: "Ada", passwordHash: hash };
    let shorthand = User { id, name, passwordHash }; // shorthand: `name: name`
    let u2 = User { ..u, name: "Ada L." };
    let d = Meters(9.8);
    let raw = d.0;
    raw
}
```

Tuple-struct fields are exported the same way:

```buri
struct Meters(export F64); // the F64 is readable as `m.0` anywhere

struct UserId(Str); // the Str is readable only in this module
```

A tuple-struct declaration ends with `;`; a record struct doesn't.

A literal may leave out a field declared `Option<...>`; it becomes `.None`.

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

The declared type decides, not the instantiation. A field declared `Maybe`,
where `type Maybe = Option<Str>`, may be left out. A field declared `T` in
`struct S<T>` may not, even in `S<Option<Int>>`. A left-out `Option<Option<T>>`
is the *outer* `.None`.

A spread fills every field the literal doesn't write, so `World { ..base }` takes
`hello` from `base` rather than resetting it.

The type name may be left out where the expected type is known:

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

The type comes from an annotated `let`, a call argument, a field value, a match
arm, or a function's result; it is never inferred from the fields. Without one,
or when the expected type is an enum, a primitive, or a generic struct with an
unsettled type argument, the literal is `untyped-struct-literal`.

Braces are a literal when `{` is followed by `..` or `name :`, and a block
otherwise (`design/grammar-rationale.md` 12.3). So `World { hi }` and `{}` need
their type name, while `{ hi: hi, hello }` doesn't.

Outside the declaring module you can't read, write or match a private field, so
you can't build such a struct from scratch. Functional update still works:
`User { ..u, name: "new" }` compiles anywhere, while a literal naming
`passwordHash` compiles only in the declaring module.

There is no `opaque` modifier: a struct with no exported fields already hides its
representation.

### 5.7 Enums

Enums are Rust-style sum types. Variants may be nullary, tuple-like, or
record-like, mixed freely.

```buri name=shape
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

An exported enum exports every variant and payload field; a private one exports
none. `export` on a variant is the `variant-export` error. To hide a
representation, use a struct with a private field.

Construct a variant with a qualified path or the dot form:

```buri wrap=body use=shape
let a = Shape.Circle(1.0);
let b: Shape = .Rect { width: 2.0, height: 1.0 };
let c: Shape = .Empty;
```

The dot form needs an expected type: a `let` annotation, a parameter type, the
function's return type, or a `match` scrutinee's type.

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

**A `Result` may not be discarded**, whether by a `_` anywhere in a `let`
pattern or as an expression statement (legal only in test sources,
`design/grammar-rationale.md` 12.2):

```buri role=test
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemWrite, Path };
# from "platform/effect" import { Allocator };
#
# fn save<C: Allocator + FileSystemWrite>(ctx: C, path: Path, body: Str): () {
    let _ = fs.writeText(ctx, path, body); // ERROR: a `Result` may not be discarded
    let (n, _) = (1, fs.writeText(ctx, path, body)); // ERROR: a `Result` may not be discarded
    fs.writeText(ctx, path, body); // ERROR: has type `Result<(), IoError>`, not `()`
#     ()
# }
```

Consume it instead:

```buri
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemWrite, Path };
# from "platform/effect" import { Allocator, IoError };
#
# type Written = Result<(), IoError>;
#
# fn save<C: Allocator + FileSystemWrite>(ctx: C, path: Path, body: Str): Written {
    let propagated = fs.writeText(ctx, path, body)?;
    let handled = match (fs.writeText(ctx, path, body)) {
        .Ok(()) => (),
        .Err(e) => (),
    };
    let defaulted = fs.writeText(ctx, path, body).withDefault(());
    let ignored = fs.writeText(ctx, path, body).ignore(); // explicitly, greppably
#     .Ok(())
# }
```

`ignore` is a method only, so it's greppable, and `buri lint` reports each one as
`ignored-result`.

The rule follows the type, so a pure function's `Result` is must-use too. So is
`io.println`'s `Result<(), IoError>`: pipes close and disks fill.

`Option` is **not** must-use (`design/non-goals.md`).

### 5.8 Function types

```buri
# struct Config(Int);
#
# enum ParseError {
#     Malformed,
# }
#
type Combine = fn(Int, Int) => Int;

type Thunk = fn() => ();

type Parser = fn(Str) => Result<Config, ParseError>;
```

The `fn` keyword keeps `(A, B)` unambiguously a tuple. Function types are
rank-1: there are no polymorphic function *values*.

### 5.9 Type aliases

```buri
type UserId = Str;

type Handler<T> = fn(T) => Result<(), Str>;
```

Aliases are transparent: `UserId` and `Str` are the same type. For a distinct
type, use a tuple struct: `struct UserId(Str);`.

Aliases export, import and re-export like any declaration (Section 4.2), and
expand in the declaring module, so `type Handle = LocalStruct` means the same
everywhere.

An alias that reaches itself, directly or through other aliases in any module, is
`circular-type-alias`. Two aliases reaching the same type by different routes
aren't a cycle. Write recursive types with a struct or enum.

### 5.10 Generics

Type parameters go in angle brackets:

```buri sig
# from "platform/effect" import { Allocator, Stdout };
#
fn identity<T>(x: T): T {
    x
}

fn map<A, B, C: Allocator>(ctx: C, xs: [A], f: fn(A) => B): [B];

fn tee<T, C: Stdout>(ctx: C, x: T): T;
```

**Bounds** name traits a type argument must satisfy, joined with `+`:

```buri sig
# from "platform/effect" import { Allocator };
#
fn largest<T: Ordered>(xs: [T]): Option<T>;

fn report<T: Ordered + Show, C: Allocator>(ctx: C, xs: [T]): Str;
```

A generic body may call only the bounds' methods, like `x.compare(y)`. Pass any
other operation as a function: `sortBy(xs, cmp)`.

Explicit type arguments go on the expression:

```buri
# from "core/list" import * as list;
#
# fn identity<T>(x: T): T {
#     x
# }
#
# fn demo(): [Int] {
    let f = identity<Int>;
    let e: [Int] = list.empty<Int>();
#     e
# }
```

### 5.11 Equality and ordering

**Equality is structural.** `a == b` compares struct fields in declaration
order, an enum's variant and payload, and arrays and tuples element-wise, all
the way down.

There are no references, so there is no referential equality; the runtime may
share or copy equal values freely (Section 8.1). Carry identity as data:
`struct NodeId(U64)`.

`==` and `!=` call `Equal.equal`; `<` `<=` `>` `>=` call `Ordered.compare`
(Section 5.12.4), except at a float. Primitives, and arrays and tuples of types that have them,
satisfy both. Your structs and enums opt in:

```buri
derive Equal, Ordered for Version;
# struct Version {
#     major: Int,
#     minor: Int,
# }

fn same(): Bool {
    // true: different values, equal contents
    Version { major: 1, minor: 2 } == Version { major: 1, minor: 2 }
}
```

Function types and `Template` have no `Equal`, so comparing them is a compile
error.

- **A derived `Equal` is an equivalence relation, as is float `==`.** Since
  `NaN == NaN` (Section 6.2), a struct holding a `NaN` equals any copy of itself.
- **A float's `compare` is a total order**, `-inf < … < -0.0 < 0.0 < … < inf <
  NaN`, so sorts and `OrderedMap` keys don't depend on input order. Every `NaN` is `.Equal` to every other, whatever its sign or payload. That's
  where it parts from IEEE-754's `totalOrder`, which orders `NaN`s by sign: the
  sign of a `NaN` that arithmetic makes depends on the CPU. A derived `Ordered`
  orders a float field the same way. `compare` and `==` disagree only at zero:
  `-0.0 == 0.0`, but `-0.0` sorts first. The operators `<` `<=` `>` `>=` stay
  IEEE-754's at a float, behind an `Ordered` bound too (Section 6.2).
- **A hand-written `impl Equal` need not be structural.** Nothing checks it's an
  equivalence, so a case-insensitive `Str` wrapper is possible, and so is a
  broken one.
- **`Str` orders by Unicode scalar value**, the unit `len` counts and `charAt`
  returns. That's UTF-8 byte order, not JavaScript's UTF-16 order, on every
  backend, and `sort`, `OrderedMap<Str, _>` and `core/order`'s `str` all use it.
  `Char` orders by scalar value. There is no locale-aware comparison.

### 5.12 Traits

A trait is an interface: a named set of method signatures.

```buri
# from "platform/effect" import { Allocator };

trait Ordered {
    fn compare(self, other: Self): Order;
}

trait Show {
    fn show<C: Allocator>(self, ctx: C): Str;
}
```

`Self` is the implementing type. Trait methods take an untyped `self` first, like
any method (Section 6.7.1).

An `effect` trait is an ordinary trait whose implementors fall under the `ctx`
rule of Section 10.2.

#### 5.12.1 Conformance is nominal

A type satisfies a trait only where an `impl` or `derive` says so, never by having
matching methods. Checking `T: Ordered` is one lookup keyed by `(trait, type)`: no
coherence pass, orphan rule, or instance search.

#### 5.12.2 `impl`

`impl Trait for Type` declares conformance and supplies the methods.

```buri
# struct Version {
#     major: Int,
#     minor: Int,
# }
#
impl Ordered for Version {
    fn compare(self, other: Version): Order {
        match (self.major.compare(other.major)) {
            .Equal => self.minor.compare(other.minor),
            unequal => unequal,
        }
    }
}
```

Its methods share a namespace with the type's own (Section 6.7.3), but are never
`export`ed. Like any `impl`, it lives in the type's defining module, so you can't
implement a trait for someone else's type.

A supplied method's signature, including its type parameters and their bounds,
must match the trait's with `Self` read as the implementing type. So `compare`
may write `Version` or `Self` for `other`.

#### 5.12.3 `derive` generates the implementation

```buri
derive Equal, Ordered, Show for Version;
# struct Version {
#     major: Int,
#     minor: Int,
# }
```

`derive` generates methods structurally, over fields and variants in declaration
order. It works for `Equal`, `Ordered`, `Show`, `Hash`, `ToJson`, `FromJson`, and
the operator traits, and fails if a field's type doesn't satisfy the trait.
Each field goes through its own type's implementation, so a field whose `Show`
is written by hand prints the way that `impl` says, inside any derived type.

`ToJson` and `FromJson` can *only* be derived; a hand-written one would apply to
the type alone but not where it's nested. `core/json` documents the JSON
mapping.

`derive Flags` stores a struct whose fields are all `Bool` as one unsigned word
with a bit per field: a `U8` up to 8 fields, then `U16`, `U32` and `U64`, and
at most 64. Fields still read as `x.read` and update as `Access { ..x, read:
true }`. The `Flags` trait in `core/flags` adds the set operations, and
`flags.none` and `flags.all` build the two ends:

```buri
# from "core/flags" import * as flags;
# from "core/flags" import { Flags };

derive Flags, Equal, Ordered, Show for Access;
struct Access {
    read: Bool,
    write: Bool,
}

# fn demo(): Bool {
    let editor = Access { ..flags.none<Access>(), write: true };
    editor.union(flags.all<Access>()).count() == 2
# }
```

`Equal`, `Ordered` and `Hash` compare the word, ordering as a plain struct's
derived `Ordered` does. `Show`, `ToJson` and `FromJson` read and write the plain
struct. `Flags` can only be derived.

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

That makes newtypes work:

```buri
derive Add, Subtract, Ordered, Show for Meters;
struct Meters(F64);

# fn demo(): Bool {
    let total = Meters(1.5) + Meters(2.0); // Meters
    let far = total > Meters(3.0); // Bool
    let bad = Meters(1.5) + 2.0; // ERROR: expected `Meters`, found `Float`
#     far
# }
```

**An operator can't allocate or perform an effect**, since `a + b` has nowhere to
pass a context. Matrix addition allocates, so it's `a.add(ctx, b)`, not
`a + b`.

#### 5.12.5 What traits deliberately lack

No blanket implementations, associated types, `where` clauses, supertraits, trait
objects, or dynamic dispatch. Each would turn resolution from a lookup into a
search. Generic bodies are typechecked once, with bounds verified at each call
site, then monomorphized.

---
