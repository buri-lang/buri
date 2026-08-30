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

Most code wants to say "a number" and move on. Code with a size on the wire —
binary formats, checksums, graphics, a foreign function interface — needs to
name an exact width and have the compiler hold it to that. Buri serves both with
**one set of types and two names for the common ones**:

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

**Every integer type holds its whole range on every backend.** An `I64` is an
`I64` whether the program runs natively or on JavaScript, so a nanosecond
timestamp keeps its last three digits either way. That costs something on
JavaScript, where a double holds every integer only up to 2^53: the widths up to
32 bits compile to a `number` and the widths at 64 bits and above compile to a
`BigInt`, which is a heap value rather than an immediate. Code on a hot path
that does not need the range can say `I32` and get the faster representation;
code that needs the range gets the right answer without asking.

#### Literals are polymorphic until they are pinned

A numeric literal does not have a type on sight. It gets a fresh type variable
constrained to the integer types (for an integer literal) or the float types (for
a float literal); ordinary unification then decides. Only if nothing constrains
it does the default apply — `Int` for integer literals, `Float` for float
literals.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let a = 5;               // nothing constrains it -> Int
let b: U8 = 5;           // the annotation pins it -> U8, no conversion
let c: F32 = 1.5;        // -> F32
takesU16(5)              // the parameter pins it -> U16

let e: [U8] = [1, 2, 3]; // every element is a U8
```

This is defaulting on *literals only*. It is not overloading and not a numeric
tower: `a + b` still requires `a` and `b` to already have the same type.

Because a literal's type is known before it is checked, **a literal that does not
fit its type is a compile error**, not a runtime surprise:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let x: U8 = 300;         // ERROR: 300 is not representable in U8
let y: I8 = -129;        // ERROR
let z: U32 = -1;         // ERROR: U32 has no negative values
let w: U64 = 18_446_744_073_709_551_615;    // fine
```

There are no literal suffixes (`5u8`). An annotation or the call site pins a
literal's type, and a conversion method changes a value's (Section 6.2.1), so a
third mechanism would say nothing the other two do not.

#### Generic numeric code

Arithmetic is available on a type parameter through the operator traits of
Section 5.12 — `Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`, `Ord` — each of which
is an ordinary interface with a method set:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
fn total<N: Add>(zero: N, xs: [N]): N { ... }
fn clamp<N: Ord>(lo: N, hi: N, x: N): N { ... }
```

Earlier drafts had three compiler-privileged bounds named `Num`, `Integral`, and
`Floating`. They are gone. A blob bound named after what a type *is* was standing
in for a trait system that did not exist yet; now that traits do exist, bounds
name what a type *can do*, which is both more precise and one fewer mechanism.

The integer-specific operations follow the same rule — they are interfaces named
for what they provide, not for the representation behind them: `Bounded`,
`Checked`, `Wrapping`, and `Saturating`, declared in Section 6.2.2. Every
built-in integer type satisfies all four; the float types satisfy `Bounded`
only.

None of this affects ordinary code: `Int` and `F64` are concrete types, so a
function over them needs no bound, no trait, and no ceremony.

### 5.2 Unit

The unit type and its only value are both written `()`. Functions that exist
only for their effect return `()`.

```buri
# from "core/effect" import { Stdout };
fn log<C: Stdout>(ctx: C, msg: Str): () { ctx.println(msg) }
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
let n = list.len(xs);          // pure: no allocation
let maybe = xs[0];             // Option<Int>, not Int
```

**Indexing yields `Option<T>`.** There is no way to index out of bounds and no
way to panic by indexing. This is the single largest ergonomic tax the language
charges, and it is charged on purpose.

An array literal has a statically known length and is not, by itself, an
allocation the programmer must account for. Any operation whose result length
depends on runtime data (`map`, `filter`, `concat`, `sort`, `range`) requires an
`Alloc` effect.

### 5.5 No records

There are no anonymous record types and no record literals. Every product type
is a `struct` with a declared name (Section 5.6), and every type in the language
is nominal — including trait conformance (Section 5.12).

Earlier drafts had structural records, mainly so that a context could be a bag of
effects. Effects are trait bounds now (Section 10), and deleting records took row
polymorphism, row unification, and the `{` ambiguity of Section 12.3 with them.

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
struct Meters(export F64);      // the F64 is readable as `m.0` anywhere
struct UserId(Str);             // the Str is readable only in this module
```

Tuple-struct declarations are terminated with `;`; record-struct declarations are
not.

Outside the declaring module, a private field cannot be read, written in a
literal, or matched. A struct with any private field therefore cannot be
constructed from scratch elsewhere — but functional update still works, because
it never names the hidden fields:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let renamed = User { ..u, name: "new" };     // fine anywhere
let forged = User { id: ..., name: ..., passwordHash: ... };   // only in the
                                                               // declaring module
```

This is the only visibility mechanism a struct has. Earlier drafts also had an
`opaque` modifier that hid a type's whole representation; a struct with no
exported fields does exactly that, so `opaque` was removed as redundant.

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

A variant writes no `export`. The enum is the unit of visibility: an exported
enum exports every one of its variants and every field of their payloads, and a
private one exports none. Writing `export` before a variant is the
`variant-export` error, which carries the edit that deletes it. A type whose
representation is meant to stay hidden is a struct with a private field, which
is the shape the standard library uses for `Scope` and `Event`.

Constructing a variant uses a qualified path or the inferred-type dot form:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
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

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let _ = fs.writeText(ctx, path, body);          // ERROR: discarded Result
```

Since there are no expression statements (Section 12.2), `let _ =` is the only
place a value can be thrown away, so this is a one-line rule with no holes. The
legal ways to consume a `Result` are:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
fs.writeText(ctx, path, body)?                   // propagate
match (fs.writeText(ctx, path, body)) { ... }    // handle
result.withDefault((), fs.writeText(ctx, path, body))
result.ignore(fs.writeText(ctx, path, body))     // explicitly, greppably, ignore
```

`result.ignore(r): ()` exists so that "I considered this and do not care" is a
thing you *write*, rather than a thing that happens by not writing anything. A
reviewer can grep for it; `_` is unsearchable.

The rule is on the type, not on the call: a `Result` returned from a pure
function is just as must-use as one returned from an I/O call.

`Option` is **not** must-use. Ignoring an absent value is usually harmless, and
making it an error would put `option.ignore` in front of half the standard
library for no safety gain. Section 15 records this as a judgment call rather
than a principle.

Note also that `io.print` / `io.println` return `()`, not `Result`. Stream errors
are reported by the platform at flush time and surface as `main`'s exit status;
threading an `IoError` through every print statement buys nothing that a
program can act on.

### 5.8 Function types

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
fn(Int, Int) => Int
fn() => ()
fn(Str) => Result<Config, ParseError>
```

Function types are written with the `fn` keyword for the same reason lambdas are:
it makes `(A, B)` unambiguously a tuple everywhere. Function types are rank-1;
there are no polymorphic function *values* in v0.3.

### 5.9 Type aliases

```buri
type UserId = Str;
type Handler<T> = fn(T) => Result<(), Str>;
```

Aliases are transparent: `type UserId = Str` makes `UserId` and `Str` the same
type. For a distinct type, use a tuple struct: `struct UserId(Str);`.

### 5.10 Generics

Type parameters are declared in angle brackets. There are no row parameters:
row polymorphism went away with the structural records of Section 5.5.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Stdout };
fn identity<T>(x: T): T { x }
fn map<A, B, C: Alloc>(self, ctx: C, f: fn(A) => B): [B] { ... }
fn tee<T, C: Stdout>(ctx: C, x: T): T { ... }
```

A parameter may carry one or more **bounds**, naming traits the argument type
must satisfy. Multiple bounds are joined with `+`:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc };
fn largest<T: Ord>(xs: [T]): Option<T> { ... }
fn report<T: Ord + Show, C: Alloc>(ctx: C, xs: [T]): Str { ... }
```

Inside such a function, the bound's methods are callable on the parameter —
`x.compare(y)`, `x.show(ctx)` — and nothing else is. What traits deliberately
lack (Section 5.12.5) is what keeps bound checking a lookup rather than a
search.

Generic code that needs an operation no trait provides takes it as a function
argument, as it always has: `sortBy(xs, cmp)` rather than inventing a trait.

In *expression* position, explicit type arguments are written on the
expression itself:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let f = identity<Int>;
let e: [Int] = list.empty<Int>();
```

### 5.11 Equality and ordering

**Equality is structural, never referential.** `a == b` compares values: fields
in declaration order for a struct, the variant plus its payload for an enum,
element-wise for arrays and tuples, recursively all the way down. Two separately
constructed values with equal contents are equal.

Referential equality is not merely unchosen — it is **not expressible**. Buri has
no references, so there is no identity to compare, and Section 8.1 rules it out
besides: the runtime may share a representation between two equal values or copy
one, whenever that is faster, so `a === b` would have an answer that depended on
the optimization level and on the backend. Code that needs identity carries it as
data (`struct NodeId(U64)`), which is a value the compiler cannot invent or
coalesce. What a backend may do — and the JavaScript one does — is answer `true`
early when the two operands are already known to be one value, because
reflexivity says the walk would reach `true` anyway. That is a shortcut to a
fixed answer rather than a second definition of equality, and it is sound only
because `==` is reflexive; it was not, before the rule below, and the two
backends disagreed at exactly `NaN` as a result.

`==` and `!=` are `Eq.eq`; `<` `<=` `>` `>=` are `Ord.compare` — the operator
table is Section 5.12.4. Neither is compiler magic: a type has them because it
derives or implements the trait. Every primitive, and `[T]` and tuples built from
types that have them, satisfy `Eq` and `Ord` already. Your own structs and enums
opt in:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
derive Eq, Ord for Version;

let same = Version { major: 1, minor: 2 } == Version { major: 1, minor: 2 };
// true — different values, equal contents
```

`Eq` is not defined for function types or `Template`, so comparing those is a
compile error.

Two consequences:

- **A derived `Eq` is an equivalence relation, and so is `==` on a float.**
  `NaN == NaN` (Section 6.2), so a struct with an `F64` field holding `NaN` is
  equal to itself *and* to a separately built copy of itself. Derived `Eq` is
  therefore reflexive, symmetric and transitive at every value there is, which is
  what the rest of the language assumes of it. `Ord` on floats is unchanged and
  still IEEE-754's: it orders `-0.0` equal to `0.0` and reports `NaN` as
  unordered, so `<` and `compare` do not agree with `==` at `NaN` — `==` is the
  one that was made total.

- **A hand-written `impl Eq` need not be structural.** Nothing checks that it is
  reflexive, symmetric, or transitive, so a case-insensitive `Str` wrapper is
  expressible — and so is a broken one. `derive` cannot be wrong in that way;
  hand-written implementations are a place to be deliberate.

### 5.12 Traits

A trait is an **interface**: a named set of method signatures that a type may
satisfy.

```buri
# from "core/effect" import { Alloc };
trait Ord {
  fn compare(self, other: Self): Order;
}

trait Show {
  fn show<C: Alloc>(self, ctx: C): Str;
}
```

`Self` stands for the implementing type and is legal only inside a trait or an
`impl`. A trait's methods declare `self` first and without a type, exactly like
any other method (Section 6.7.1); it is the implementing type that `self` is,
which is what `Self` names in the rest of the signature.

A trait declared `effect` additionally marks its implementors as
effect-carrying, which subjects them to the `ctx` rule of Section 10.2. That
modifier is the only difference between an effect and an ordinary interface.

#### 5.12.1 Conformance is nominal

A type satisfies a trait only where an `impl` or a `derive` says so. Declaring a
method that happens to match a trait's signature does not make the type conform;
nothing is inferred from shape.

Checking `T: Ord` is therefore a lookup in one table keyed by `(trait, type)`,
populated by the declarations in the type's own module. There is exactly one
candidate, so there is no coherence pass, no orphan rule, and no instance search
— they are not restricted, they are unrepresentable.

The whole type system is nominal, and this is the same rule applied to
conformance: an `impl` is a declaration, like a `struct`.

An earlier draft made conformance structural, Go-style. It was cheap to check but
it made a module's public API implicitly include *which traits its types happen
to satisfy* — so adding an unrelated exported function could make a type conform
at a distance, and removing one could break a caller three modules away. That is
a correctness hazard and, worse for the compile-time goal, it coarsens
incremental invalidation exactly where it needs to be fine.

#### 5.12.2 `impl`

`impl Trait for Type` declares conformance and supplies the methods:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
impl Ord for Version {
  fn compare(self, other: Version): Order { ... }
}
```

This is the same block that declares a type's own methods (Section 6.7.1), with
a `for` clause added. The methods land in the same namespace, so
`v.compare(other)` resolves the way any method does (Section 6.7.3): an `impl`
introduces no second namespace and no second resolution path, whichever form it
takes.

The two forms differ in one respect, and it is Section 6.7.1's: a method of the
type's own may be `export`ed, and a method supplied to a trait may not. An `impl`
in either form may appear only in the defining module of its type, so there is no
way to implement a trait for someone else's type.

#### 5.12.3 `derive` generates the implementation

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
derive Eq, Ord, Show for Version;
```

`derive` generates the trait's methods structurally: struct fields in declaration
order, enum variants in declaration order, recursing into field types. It is a
fold over one type definition — no search, no instances to resolve.

Derivation is available for `Eq`, `Ord`, `Show`, `Hash`, `ToJson`, `FromJson`,
and the operator traits. A `derive` fails to compile if any field's type does not
itself satisfy the trait.

`ToJson` and `FromJson` — `core/json`'s typed encoding — are *only* ever derived.
What a derived implementation stands for is the type's shape, so a hand-written
one would be obeyed where the type is encoded on its own and ignored where a type
holding it is; an `impl` of either is rejected rather than half-obeyed. The
mapping from Buri shapes onto JSON ones is stated in `core/json`.

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

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
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
