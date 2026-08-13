# The Buri Language Specification

**Version 0.3 (draft) · file extension `.buri`**

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

Version 0.3 is deliberately small: primitives, arrays, tuples, records, structs,
enums, functions, methods, and traits. Data and behaviour are declared
separately; there is no mutable state, no inheritance, and no dynamic dispatch. A
method is an ordinary function whose first parameter is `self`, and a trait is an
interface satisfied structurally — neither introduces a runtime mechanism.

There are also no loops. Iteration is recursion — guaranteed tail-call
eliminated — or a fold. A `for`/`while` sugar was drafted for this version and
cut; Section 15 records why.

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
export fn main<C: Alloc + Stdout>(ctx: C): Result<(), Str> {
  let shapes = [Shape.Circle(1.0), Shape.Rect { width: 2.0, height: 3.0 }];
  let total = shapes.map(ctx, area).sum();
  let _ = io.println(ctx, "total area: ${total}");
  .Ok(())
}
```

---

## 2. Notation and conformance

The normative grammar lives in [`grammar.ebnf`](./grammar.ebnf). Where this
document and that file disagree about syntax, the grammar file wins. Where this
document states a rule that the grammar cannot express (Section 14), that rule is
normative and is checked after parsing.

Terminology: *must* is a requirement on conforming programs and implementations;
*should* is a recommendation; *may* grants latitude.

---

## 3. Source text and lexical structure

### 3.1 Encoding

Source files are UTF-8. Identifiers are ASCII in v0.3. String and character
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

`as` `const` `crash` `ctx` `derive` `effect` `else` `enum` `export` `false`
`fn` `for` `from` `if` `impl` `import` `let` `match` `self` `Self` `struct`
`trait` `true` `type`

`for` appears only in `impl ... for ...` and `derive ... for ...`. `self` is
legal only as a method's first parameter and `ctx` only as the parameter after
it (Section 10.2); `Self` only inside a trait or `impl`.

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
effect and can stream directly to the sink.

To turn a `Template` into a `Str` you must allocate:

```buri
let greeting: Str = str.format(ctx, "Hello, ${name}!");
```

Hole expressions must have type `Int` (any width), `Float` (any width), `Bool`,
`Char`, or `Str`. There is no user-extensible display mechanism in v0.3; convert
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

Struct fields and enum variants carry their own `export`, so a type's name and
its representation are exported separately:

```buri
export struct UserId(Str);          // name public, contents private
export struct Meters(export F64);   // both public
```

A type with any unexported field or variant cannot be constructed, destructured,
or exhaustively matched outside its module.

`impl` and `derive` declarations are never exported: whether a type satisfies a
trait is a property of the type, visible wherever the type is.

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
fn ratio(hits: Int, total: Int): F64 { hits.toF64() / total.toF64() }
```

### 5.2 Unit

The unit type and its only value are both written `()`. Functions that exist
only for their effect return `()`.

```buri
fn log<C: Stdout>(ctx: C, msg: Str): () { ctx.println(msg) }
```

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
`Alloc` effect.

### 5.5 No records

There are no anonymous record types and no record literals. Every product type
is a `struct` with a declared name (Section 5.6), and every type in the language
is nominal — including trait conformance (Section 5.12).

Earlier drafts had structural records, mainly so that a context could be a bag of
effects. Effects are trait bounds now (Section 10), which removed the only
thing records were load-bearing for. Deleting them removed row polymorphism, row
unification from the type checker, and the ambiguity that had cost struct
literals their field shorthand — see Section 12.3.

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
let shorthand = User { id, name, email };     // field shorthand: `name: name`
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

```buri
let renamed = User { ..u, name: "new" };     // fine anywhere
let forged = User { id: ..., name: ..., passwordHash: ... };   // only in the
                                                               // declaring module
```

This is the only visibility mechanism. Earlier drafts also had an `opaque`
modifier that hid a type's whole representation; a struct with no exported
fields does exactly that, so `opaque` was removed as redundant.

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
library for no safety gain. Section 15 records this as a judgment call rather
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
there are no polymorphic function *values* in v0.3.

### 5.9 Type aliases

```buri
type UserId = Str;
type Handler<T> = fn(T) => Result<(), Str>;
```

Aliases are transparent: `type UserId = Str` makes `UserId` and `Str` the same
type. For a distinct type, use a tuple struct: `struct UserId(Str);`.

### 5.10 Generics

Type parameters are declared in angle brackets; row parameters are prefixed with
`..`.

```buri
fn identity<T>(x: T): T { x }
fn map<A, B, C: Alloc>(self: [A], ctx: C, f: fn(A) => B): [B] { ... }
fn tee<T, C: Stdout>(ctx: C, x: T): T { ... }
```

A parameter may carry one or more **bounds**, naming traits the argument type
must satisfy. Multiple bounds are joined with `+`:

```buri
fn largest<T: Ord>(xs: [T]): Option<T> { ... }
fn report<T: Ord + Show, C: Alloc>(ctx: C, xs: [T]): Str { ... }
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

**Equality is structural, never referential.** `a == b` compares values: fields
in declaration order for a struct, the variant plus its payload for an enum,
element-wise for arrays and tuples, recursively all the way down. Two separately
constructed values with equal contents are equal.

Referential equality is not merely unchosen — it is **not expressible**. Buri has
no references, so there is no identity to compare. It is also ruled out by
Section 8.1: the runtime may share a representation between two values, or copy
one, whenever that is faster, and the language guarantees you cannot tell. A
referential `==` would make that guarantee false, since the answer would depend
on what the optimizer decided.

`==` and `!=` are `Eq.eq`; `<` `<=` `>` `>=` are `Ord.compare` (Section 5.12.4).
Neither is compiler magic: a type has them because it derives or implements the
trait. Every primitive, and `[T]`, tuples, and records built from types that have
them, satisfy `Eq` and `Ord` already. Your own structs and enums opt in:

```buri
derive Eq, Ord for Version;

let same = Version { major: 1, minor: 2 } == Version { major: 1, minor: 2 };
// true — different values, equal contents
```

`Eq` is not defined for function types, `Template`, or opaque types from other
modules, so comparing those is a compile error.

Two consequences worth knowing:

- **A derived `Eq` inherits IEEE-754 float behaviour.** `NaN != NaN`, so a struct
  with an `F64` field holding `NaN` is not equal to itself. That is structural
  equality being honest about its components rather than papering over them, but
  it does mean derived `Eq` is not reflexive for every value. `Ord` on floats
  orders `-0.0` equal to `0.0` and reports `NaN` as unordered.
Referential equality was considered and rejected. `a === b` on ordinary values
has no stable answer: the runtime may share one representation between two equal
values or copy it, so the result would depend on the optimization level and on
the backend. Code that needs identity carries it as data (`struct NodeId(U64)`),
which is a value the compiler cannot invent or coalesce.

- **A hand-written `impl Eq` need not be structural.** Nothing checks that it is
  reflexive, symmetric, or transitive, so a case-insensitive `Str` wrapper is
  expressible — and so is a broken one. `derive` cannot be wrong in that way;
  hand-written implementations are a place to be deliberate.

### 5.12 Traits

A trait is an **interface**: a named set of method signatures that a type may
satisfy.

```buri
trait Ord {
  fn compare(self: Self, other: Self): Order;
}

trait Show {
  fn show<C: Alloc>(self: Self, ctx: C): Str;
}
```

`Self` stands for the implementing type and is legal only inside a trait or an
`impl`. Trait methods declare `self` first, exactly like any other method
(Section 6.7.1).

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

An `impl` block declares conformance and supplies the methods:

```buri
impl Ord for Version {
  fn compare(self: Version, other: Version): Order { ... }
}
```

The methods land in the type's ordinary method namespace, so `v.compare(other)`
resolves the same way any method does (Section 6.7.3). An `impl` introduces no
second namespace and no second resolution path.

An `impl` may appear only in the defining module of the type, and is never
exported — conformance is a property of the type, visible wherever the type is.
There is no way to implement a trait for someone else's type, which is the same
restriction that already applies to methods.

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
| 10 | `-` `!` `~` (prefix) | right |
| 11 | `.f` `.0` `(args)` `[i]` `?` `::<T>` `{ ... }` | left |

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

Numeric conversions are explicit, and they are **ordinary methods** rather than
operators:

```buri
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
prefers ergonomics to ceremony, and it is called out rather than hidden.

Earlier drafts used three cast operators (`as`, `as?`, `as%`). They were
operators because a *function* cannot be generic over its source type — but a
*method* can be resolved by its receiver's type, which is exactly the same
lookup. So the operators bought nothing that methods do not, and cost three
tokens, a precedence level, and a rule about which conversions the compiler would
accept. `as` now appears only in import specifiers.

There are a lot of these functions in `core/num` — one per source-and-target
pair. They are mechanical, greppable, and each says in its return type what it
can do.

`Char` and `U32` convert the same way: `c.toU32()` is exact, `n.toChar()` yields
`Result<Char, RangeError>`.

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
export fn map<A, B, C: Alloc>(self: [A], ctx: C, f: fn(A) => B): [B]

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

`x.f(...)` resolves in three steps, each a lookup rather than a search:

1. If `x`'s type has a field named `f`, this is field access. A field of function
   type is called as `(x.f)(...)`.
2. If `x`'s type is a concrete type, `f` must be an `export`ed method in that
   type's **defining module** whose `self` parameter has that type. Methods
   supplied by an `impl` live in the same namespace and are found here.
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

A crash is not an effect in the `ctx` sense: it can occur in a function with
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
an effect are never indistinguishable.

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

#### 8.3.1 How, on a target without native tail calls

Native backends can lower a tail call directly. JavaScript cannot — no engine but
JavaScriptCore implements proper tail calls — so the **compiler performs the
elimination itself** rather than relying on the host. Three cases, in increasing
cost:

| Shape | Transformation | Cost |
|---|---|---|
| A function tail-calls itself | rewrite to a loop with parameter rebinding | none |
| A statically known group of functions tail-call each other | merge the group into one function with a dispatch switch | one branch per bounce |
| A tail call through a value of function type | trampoline: return a thunk, drive it from a loop | one allocation per bounce |

The first two cover essentially all Buri code, and both are exact — the emitted
loop is what a hand-written loop would have been. They apply because Buri has no
dynamic dispatch: there are no trait objects and no virtual calls, so the call
graph of direct calls is fully known, and generic calls become direct after
monomorphization.

Only the third case costs anything, and it arises solely when a function *value*
is invoked in tail position. An implementation should apply the cheaper
transformation wherever the callee is statically known, and may specialize a
call site whose function value is known to avoid the trampoline entirely.

One consequence is observable: a crash inside a transformed group reports fewer
stack frames than the source suggests, because those frames no longer exist.
Implementations should preserve source positions through the transformation so
that the reported location is still correct.

### 8.4 Closures

Lambdas capture by value. Since values are immutable, capture is unobservable —
with one exception: **a lambda may not capture a effect-carrying value**
(Section 10.5).

---

## 9. Functions

```buri
export fn slugify(s: Str): Str { ... }

fn quadratic(a: F64, b: F64, c: F64): Option<(F64, F64)> { ... }

fn retry<T, C: Clock>(
  ctx: C,
  attempts: Int,
  action: fn(C) => Result<T, Str>,
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

## 10. Effects and purity

This is the part of Buri that is not TypeScript and not Rust.

### 10.1 The model

An **effect** is an interface declared with `effect` instead of `trait`. Its
methods are the operations it grants:

```buri
// core/cap
export effect Alloc {
  fn allocate(self: Self, bytes: Int): Region;
}

export effect Stdout {
  fn print(self: Self, text: Template): ();
  fn println(self: Self, text: Template): ();
}

export effect Fs {
  fn readFile(self: Self, path: Str): Result<Str, IoError>;
  fn writeFile(self: Self, path: Str, body: Str): Result<(), IoError>;
}
```

`core/cap` declares `Alloc`, `Fs`, `Net`, `Clock`, `Rand`, `Env`, `Stdin`,
`Stdout`, `Stderr`, and `Proc`. **Only platform modules may declare effects**;
`effect` in ordinary code is a compile error, so the set of things a Buri program
can do to the world is fixed by its platform rather than open-ended.

An effect is a trait in every other respect — same declaration shape, same
nominal conformance, same `impl`, same bounds. Two rules separate them:

- an effect's implementors are **effect-carrying**, and so may be passed only as
  `self` or `ctx` (Section 10.2);
- **no type may implement both an effect and a trait.** A type is either part of
  the world or part of your data, and the boundary is checked rather than
  assumed.

A function names the effects it needs as **bounds** on its context parameter:

```buri
fn loadConfig<C: Alloc + Fs>(ctx: C, path: Str): Result<Config, ConfigError> {
  let text = fs.readText(ctx, path)?;
  parse(ctx, text)
}
```

There is one constraint mechanism in the language. `<T: Ord + Show>` and
`<C: Alloc + Fs>` are the same feature: a list of interfaces a type parameter
must satisfy.

### 10.2 The `ctx` rule

**A effect-carrying parameter must be `self` or `ctx`** — never any other
name, never any other position, and at most one of each:

```buri
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>       // ok
fn render<C: Alloc>(self: Report, ctx: C): Str                            // ok
fn allocate(self: Self, bytes: Int): Region                               // ok
fn sneaky<C: Fs>(a: Int, handle: C): Bool                                 // ERROR
fn twoWorlds<A: Fs, B: Net>(ctx: A, other: B): {}                         // ERROR
```

A type is **effect-carrying** if it is a type variable with an effect
bound, or any type mentioning one — so a struct that stores a context is
effect-carrying too.

`self` has to be allowed because a effect's own methods take the
effect as their receiver (`fn allocate(self: Self, ...)`), and so do the
attenuation wrappers of Section 10.8. Outside those two places, effects arrive
through `ctx`.

The rule costs a little flexibility — a function cannot take two independent
contexts; bundle them into one type instead — and buys the property the chapter
rests on:

> **A function is effectful if and only if it has a `ctx` parameter, or a
> effect-carrying `self`.**

Both are fixed positions with fixed names, so you read the first two parameters
and stop. You never scan a signature.

### 10.3 Where effects come from

The platform. It provides one concrete type implementing the effects it
grants, and hands it to `main`:

```buri
export fn main<C: Alloc + Stdout + Fs>(ctx: C): Result<(), Str> { ... }
```

If `main` requests an effect the platform does not implement, the program does
not compile. A program that never names `Net` in `main`'s bounds cannot open a
socket anywhere in its transitive call graph — not in a dependency, not in a
build script, not by accident, because nothing anywhere can obtain a value
bounded by `Net`.

Note what is *not* claimed: a effect is an ordinary interface, so
anyone may write a type that satisfies it.

```buri
struct SilentOut {}
fn writeOut(self: SilentOut, text: Template): {} { {} }   // satisfies Stdout
```

That is not a forgery hole — a fake `Stdout` still cannot write anything. What is
unforgeable is the *platform's* implementation. The open interface is what makes
testing free (Section 10.8).

### 10.4 What "pure" means

> **Purity theorem.** If a function has no `ctx` parameter, no
> effect-carrying `self`, and captures no effect-carrying value, then any
> two evaluations on equal arguments produce
> equal results, perform no observable effect, and may be freely cached,
> reordered, or eliminated.

Top-level functions capture nothing but other top-level declarations, which are
themselves effect-free, so for a top-level `fn` the theorem reduces to: *is
there a `ctx` parameter?*

Two consequences worth naming:

- Purity is not a keyword and not an effect annotation. It is the absence of one
  argument, in a fixed position, with a fixed name.
- The check is shallow and local. You never read a function body, or its
  callees' bodies, to know whether it can touch the world.

### 10.5 Determinism versus effects

`Alloc` is a **resource** effect: it can fail (out of memory) and it costs
something, but it is not observable. Every other effect in `core/cap` is
**observable**.

A function is **deterministic** if its only effect bound is `Alloc`.
`list.map(ctx, f)` is deterministic: it needs to allocate, but it is
referentially transparent. `time.now(ctx)` is not.

Tracking allocation is why `[T]`-returning combinators take a context at all, and
it is what makes "does no I/O" and "does not allocate" separately expressible:

```buri
fn sum(self: [Int]): Int                                              // pure
fn map<A, B, C: Alloc>(self: [A], ctx: C, f: fn(A) => B): [B]         // deterministic
fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>   // effectful
```

Fixed-size construction — struct literals, tuples, enum payloads, array literals,
closures, `Template`s — never requires `Alloc`. Only results whose size depends
on runtime data do.

### 10.6 The capture rule

**A lambda may not capture a effect-carrying value.** Capabilities travel
through the `ctx` parameter only.

```buri
// ERROR: the lambda captures ctx
let texts = paths.map(ctx, fn(p) => fs.readText(ctx, p));

// Thread the context through a *Ctx combinator instead
let texts = paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p));
```

Without this rule, a value of type `fn(Str) => Str` could smuggle a file handle
past a signature with no `ctx` parameter, and the purity theorem would be false.
With it, a function type says everything about what its values can do.

The standard library provides `*Ctx` variants (`list.mapCtx`, `list.filterCtx`,
`result.andThenCtx`), and explicit recursion is always available when the
combinator does not fit. This is the sharpest trade-off in the language, and
Section 15 lists it as the first open question.

### 10.7 Calling convention

**receiver first, context second, everything else after** — which is now enforced
rather than merely conventional (Section 10.2):

```buri
export fn map<A, B, C: Alloc>(self: [A], ctx: C, f: fn(A) => B): [B]
export fn readText<C: Alloc + Fs>(ctx: C, path: Str): Result<Str, IoError>
```

```buri
xs.map(ctx, double)
lines.filter(ctx, isLong).sortBy(ctx, order.str)
```

### 10.8 Restricting what propagates

Two forms, giving different guarantees.

**Static confinement.** Bound the callee to fewer effects. It receives the
same value and cannot use, or pass on, anything its bounds do not name:

```buri
fn logOnly<C: Stdout>(ctx: C, msg: Str): {} {
  let _ = io.println(ctx, msg);
  // fs.readText(ctx, "/etc/passwd")     // ERROR: C is not bounded by Fs
  // dangerous(ctx)                      // ERROR: dangerous needs C: Fs
}

export fn main<C: Alloc + Stdout + Fs>(ctx: C): Result<(), Str> {
  let _ = logOnly(ctx, "starting");      // same value, confined by its bound
  .Ok(())
}
```

No copy and no ceremony. Confinement is transitive: `logOnly` cannot hand its
context to anything requiring more, because `C` is opaque at every call site
downstream.

**Attenuation.** Wrap the context in a type that satisfies fewer effects, so
the callee holds a value that genuinely lacks the rest:

```buri
// module: safe/readonly
export struct ReadOnly<C>(C);

export fn readOnly<C>(ctx: C): ReadOnly<C> { ReadOnly(ctx) }

// Forwards Alloc...
impl Alloc for ReadOnly<C: Alloc> {
  fn allocate(self: ReadOnly<C>, bytes: Int): Region { self.0.allocate(bytes) }
}

// ...and reading, but there is deliberately no `writeFile`, so ReadOnly<C>
// does not satisfy Fs no matter what C is.
export fn readFile<C: Fs>(self: ReadOnly<C>, path: Str): Result<Str, IoError> {
  self.0.readFile(path)
}
```

Static confinement is a fact about the type checker; attenuation is a fact about
the value, and survives anything that later escapes the type system. Use the
first by default and the second at trust boundaries.

Note that attenuation narrows the *context*, not one effect out of it. That
is what keeps the `ctx` rule satisfiable: there is still exactly one
effect-carrying parameter.

### 10.9 Testing

A pure function needs no harness. An effectful one is tested by passing a context
that satisfies the same effects and does something else — and because effects are
ordinary interfaces, writing one is writing a struct with methods. The call site
does not change, because there was never a global to stub.

```buri
struct FakeFs { export files: [(Str, Str)] }

fn readFile(self: FakeFs, path: Str): Result<Str, IoError> {
  match (self.files.find(fn(e) => e.0 == path)) {
    .Some(entry) => .Ok(entry.1),
    .None => .Err(.NotFound),
  }
}

// loadConfig<C: Alloc + Fs> accepts it with no changes anywhere.
```

## 11. Programs

A program is a module that exports `main`:

```buri
export fn main<C: Alloc + Stdout + Env>(ctx: C): Result<(), Str>
```

- `main` must take exactly one parameter, `ctx`, whose type is a parameter
  bounded by the effects the program needs.
- `main` must return `Result<(), Str>`.
- `.Ok(())` exits 0. `.Err(msg)` prints `msg` to stderr and exits 1.
- The bounds are the program's complete effect budget. The platform supplies one
  value satisfying them; if it cannot satisfy one, the program does not
  compile.

### 11.1 Standard library sketch

Non-normative in v0.3, but the examples assume it. The purity tier of each entry
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

**Deterministic — bounded by `<C: Alloc>`**

| Module | Functions |
|---|---|
| `core/list` | `map`, `mapCtx`, `filter`, `filterCtx`, `concat`, `push`, `reverse`, `sortBy`, `take`, `drop`, `zip`, `range` |
| `core/str` | `concat`, `join`, `split`, `splitAny`, `replace`, `repeat`, `toUpper`, `toLower`, `fromInt`, `format`, `chars` |

**Effectful — bounded by an effect**

| Module | Needs | Functions |
|---|---|---|
| `core/cap` | — | the effect declarations themselves |
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

**12.3 There are no records, so `{` at the start of an expression is always a
block.**
Structural records made `{ x }` ambiguous — a record with a shorthand field, or a
block whose result expression is `x` — and an earlier draft paid for that by
banning field shorthand in literals. Removing records (Section 5.5) removed the
ambiguity at its source: a bare `{` opens a block, and a `{` after a path opens a
struct literal. Nothing competes, so `Point { x, y }` shorthand works and the
grammar got smaller rather than more careful. *Cost:* every product type needs a
name.

**12.4 Type arguments in expressions use the turbofish `::<T>`.**
`f<a>(b)` is genuinely ambiguous with two comparisons. Types have no comparison
operators, so `<` is unambiguous inside a type; expressions get `::<`.
*Cost:* the turbofish is ugly. It is also rare.

**12.5 There is no cast operator.**
`as`, `as?`, and `as%` were three tokens and a precedence level doing work that
methods do with none: a conversion is resolved by its receiver's type, which is
the same lookup either way (Section 6.2.1). Removing them also let the fallible
conversions return `Result` instead of encoding failure in the choice of
operator. *Cost:* `core/num` has one method per source-and-target pair.

**12.6 There is no `<<` or `>>` token.**
Longest-match lexing would turn `Map<Str, [Int]>>` into a shift. Removing the
operators removes the problem, rather than papering over it with a token splitter
that makes the lexer position-dependent. *Cost:* `bits.shl(x, n)`.

**12.7 Enum variants in patterns must be qualified or dot-prefixed.**
Otherwise `None` is a binding or a variant depending on what is in scope, which
makes the parser depend on name resolution. A bare `IDENT` in a pattern is always
a binding; `IDENT(` and `IDENT{` are struct patterns; `A.B` is a path. Decided by
one token of lookahead. *Cost:* `.None` instead of `None`.

**12.8 Tuples have arity ≥ 2 and unit is `()`.**
`(e)` is grouping, full stop — no `(e,)` special case, no zero-tuple competing
with the empty record. *Cost:* none worth mentioning.

**12.9 Lambdas begin with `fn`.**
`(x) => ...` requires deciding whether `(x)` is a parenthesized expression or a
parameter list, which is a reduce/reduce conflict at `)`. A leading `fn` decides
it at token one. *Cost:* two characters; it also makes lambdas and function types
read the same.

**12.10 `else` is mandatory and branches are blocks.**
Kills the dangling-else ambiguity, and prevents `if (c) { a } else { b } + 1`
from having two parses. *Cost:* you must say what the other case is.

**12.11 Lambdas and `crash` are top-level-only expressions.**
Their bodies extend maximally to the right, so allowing them as operands would
make `2 * fn(x) => x + 1` ambiguous. Block-like expressions (`{}`, `if`, `match`)
*are* allowed as operands, because they are brace-terminated and self-delimiting.
*Cost:* parentheses around a lambda used as an operand.

**12.12 Match arms are comma-separated, always.**
Without a required separator, `A => x` followed by an arm starting `-1 =>` would
greedily parse as `x - 1`. *Cost:* a comma after `}`.

**12.13 Block-like expressions cannot head a postfix chain.**
`match (x) { ... }.field` is a parse error; parenthesize. This is what stops
`if (c) { a } else { b } { x: 1 }` from parsing two ways once struct literals are
a postfix form. *Cost:* occasional parentheses.

**12.14 Float literals must start with a digit.**
So `pair.0` lexes as three tokens. *Cost:* write `0.5`.

**12.15 Every declaration starts with a distinct keyword.**
`from` `export` `fn` `struct` `enum` `type` `const` `opaque`. Top-level parsing
is a switch on one token — and putting `from` first on an import means the module
path is known before the specifier list is parsed, which is what makes
completion inside the braces possible.

**12.16 Method calls reuse `.` rather than taking a token of their own.**
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

**12.17 `self` is a keyword in a fixed position, not a convention.**
A function is a method because its first parameter is literally written `self`,
so "is this a method?" is answered by the parser, not by comparing types against
a rule about argument order. `impl`, `derive`, and `trait` likewise each begin
with a distinct keyword, keeping top-level parsing a switch on one token.

Two lexical warts remain and are accepted:

- `t.0.1` lexes as `t` `.` `0.1`. Write `(t.0).1`.
- `Foo<Bar<Int>>= x` lexes `>` `>=`. Write a space: `Foo<Bar<Int>> = x`.

---

## 13. Compilation invariants

Section 12 explains why parsing is cheap. This section states the invariants that
make *checking* cheap, because they are the ones a future feature is most likely
to break quietly. A conforming implementation may rely on all of them, and any
proposed addition to the language should be measured against them.

**13.1 Parsing depends on nothing.** No production consults name resolution or
types (Section 12). Parsing is one pass, and files parse in parallel with no
coordination.

**13.2 Name resolution and type inference interleave, in a single traversal.**
This is the one place Buri gives something up. Resolving `x.f()` requires knowing
what `x` is, so the two cannot be separate passes.

What keeps it a single traversal rather than a fixpoint:

- Method resolution needs only the receiver's **head type constructor**, not its
  full type. `xs.first()` resolves in `core/list` whether `xs` is `[Int]` or
  `[T]` for an unresolved `T`.
- Type information flows **outside-in and left-to-right**. A lambda's parameter
  types come from the expected type at its call site, which is known before the
  body is visited.
- There is no overloading, so a name plus one type constructor selects exactly
  one definition.
- Conformance is nominal (Section 5.12.1), so a bound is a table lookup rather
  than a search that could need the rest of the program.

What would break it: overloading resolved by argument types, return-type-directed
dispatch, structural conformance, or any construct where a method call can appear
before its receiver's type constructor is determined.

**13.3 Function bodies check independently.** Top-level signatures are mandatory
(Section 9), so no inference crosses a function boundary. Bodies check in
parallel, and editing one body can never invalidate the check of another.

**13.4 A module's inter-module surface is exactly its exported declarations.**
Because conformance is declared rather than inferred from shape, adding or
removing a private function cannot change what any other module sees. Incremental
invalidation is therefore precise: a dependent is rechecked only when a
declaration it names actually changes.

**13.5 Monomorphization is a codegen concern, not a checking one.** A generic body
is checked once, polymorphically, with bounds verified at each call site. Checking
is O(code), not O(code × instantiations).

**13.6 Nothing in the checker requires a fixpoint.** No recursive trait solving
(no blanket impls, no associated types, no supertraits — Section 5.12.5), no
variance inference (no subtyping), no effect inference (effects are declared, not
deduced), and no cross-module exhaustiveness.

---

## 14. Static rules not expressed in the grammar

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
8. A lambda may not capture a effect-carrying value (Section 10.6).
9. Opaque types may not be constructed or destructured outside their defining
   module, and private fields may not be read, written, or matched outside it.
10. Numeric conversion methods are declared per source-and-target pair in
    `core/num` (Section 6.2.1); there is no cast operator.
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

Capabilities:

26. `ctx` may appear only as a function's first parameter, or the parameter
    immediately after `self`.
27. A effect-carrying parameter must be `self` or `ctx`, at most one of each
    (Section 10.2). A type is effect-carrying if it is a type variable with a
    effect bound, or any type mentioning one.
28. `effect` declarations may appear only in platform modules, and no type may
    implement both an effect and a trait.
29. `impl` and `derive` may not be exported, and may appear only in the defining
    module of the type they name.
30. Numeric literals, conversions, and comparisons are ordinary methods; there is
    no cast operator.
31. `main` has the signature required by Section 11.

## 15. Non-goals and open questions

**Not in v0.3, and not planned:** mutation, references, lifetimes, classes,
inheritance, dynamic dispatch, trait objects, `null`, exceptions, implicit
conversions (beyond `Str → Template`), overloading, macros, reflection.

**Not in v0.3:** loops, and the `|>` pipe operator. See below.

**Deferred to a later version:** blanket implementations; associated types;
`where` clauses; supertraits; implementing a trait for a foreign type; dict and
set literals; fixed-length array types; `async`; ranges; a module-level effect
summary in generated documentation.

Each item in that first deferred group is a step from "trait resolution is a
lookup" toward "trait resolution is a search," which is the entire compile-time
cost of a trait system. They are deferred together, deliberately.

### 15.1 Considered and cut: `for` and `while`

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
  with the presence of `with`, a special termination check for effect-free
  `while` conditions, plus a `Range` type, a `core/range` module, and two new
  operators with their own ambiguity argument. That is a lot of specification
  for zero new expressive power.
- **It made the capture rule inconsistent.** Exempting loop bodies is sound, but
  it means "can this construct see an effect?" stops having one answer. Better
  to keep the rule absolute and treat its cost as the open question it is.

If loops return, the case to beat is: they must earn their keep on something
other than familiarity, and the capture-rule exemption should be solved directly
rather than routed around.

### 15.2 Considered and cut: the `|>` pipe operator

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
   through a `*Ctx` combinator or hand-written recursion (Section 10.6). The alternative —
   encoding a captured-effect row in the function type, e.g.
   `fn(Str) => Str uses { fs: Fs }` — is more expressive but adds an effect system
   to a language whose selling point is not having one. This is the language's
   sharpest unresolved trade-off, and cutting loops (15.1) put the full cost back
   on it. Traits do not help: a trait method that needs an effect must declare
   the context in its signature, which is the honest encoding but not a
   convenient one.
2. *`Alloc` granularity.* Requiring `Alloc` for every size-dependent result is
   principled and noisy. Whether the noise is worth the guarantee is an empirical
   question.
3. *Indexing returns `Option`.* Correct, and occasionally miserable. A
   `list.getOr(default, i, xs)` helper and better pattern-matching over arrays may
   absorb most of the pain.
4. *Trampolining higher-order tail calls.* Section 8.3.1 specifies how tail-call
   elimination is achieved on a target without native support, and the first two
   cases are exact and free. The third — a tail call through a value of function
   type — costs an allocation per bounce, and how often that shape occurs in real
   Buri code is unknown. If it turns out to be common, the answer is probably
   call-site specialization rather than a language change.
5. *Methods are not extensible, and not available on type variables* (6.7.3).
   Resolving through the receiver's defining module is what makes them
   import-free and collision-free, and it is the same property that stops you
   adding an operation to `Str`. Calling a method on a bare `T` is what bounds are
   for (5.10); extending a foreign type is what free functions are for. Neither
   gap has a fix that preserves import-free, collision-free resolution.
6. *Whether the Section 13 invariants survive contact with real features.* They
   are now written down, which is the point — but every one of them is the kind
   of property that a reasonable-looking addition erodes. 13.2 is the fragile
   one.
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
