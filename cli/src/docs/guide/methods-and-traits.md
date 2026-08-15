## Methods, and traits as interfaces

A method is declared **inside an `impl` block for its type**, and takes `self`
as its first parameter:

```buri
# struct Square { height: Int, width: Int }
impl Square {
  export fn area(self: Square): Int { self.height * self.width }
}
```

`x.f(a)` then looks `f` up among the methods of x's type, which live in that
type's **defining module** and nowhere else. No dispatch, no vtable, no hidden
receiver — a name resolved through a type instead of through scope. What it buys
is that a type's operations travel with it —

```buri ignore why="names a module in another repository, so it cannot be compiled standalone; the same pattern is compiled in cli/tests/example"
from "//lib/square" import { Square };   // the type — not `area`, not `scaled`
sq.scaled(2).area()                      // both resolve with no further imports
```

— and resolution stays one type, one module, one lookup: no candidate set, no
coherence check, no autoref, because there are no references.

A **trait is an interface**, and conformance is **nominal** — a type satisfies it
only where an `impl` or `derive` says so, never by accident of shape:

`Ord` is one such interface, declared in the prelude as:

```buri sig
trait Ord { fn compare(self: Self, other: Self): Order; }
```

and a type takes it on in one of two ways:

```buri
# struct Version(Int);
# struct Playlist(Int);
impl Ord for Version {               // supplies the methods, checked against the trait
  fn compare(self: Version, other: Version): Order { self.0.compare(other.0) }
}
derive Eq, Ord, Show for Playlist;   // generates them structurally
```

The same keyword covers both jobs: `impl Type { ... }` declares what the type
can do on its own, and `impl Trait for Type { ... }` declares what it can do as
somebody else's interface.

Because a type has exactly one defining module and conformance is declared, there
is exactly one candidate per `(trait, type)`. Coherence, orphan rules, and
instance search aren't restricted — they're unrepresentable. It also keeps a
module's public API from implicitly including *which traits its types happen to
satisfy*, which is what would otherwise coarsen incremental rebuilds. Blanket impls, associated types, `where` clauses, supertraits,
and trait objects are all deliberately absent: each turns resolution from a
lookup into a search, and the search is the entire compile-time cost of a trait
system.

Operators are trait methods, which is what makes newtypes usable:

```buri
struct Meters(F64);
derive Add, Sub, Ord, Show for Meters;

# fn demo(): Meters {
let total = Meters(1.5) + Meters(2.0);   // Meters
let bad   = Meters(1.5) + 2.0;           // ERROR: expected `Meters`, found `{float}`
# total
# }
```

And an operator implementation **cannot allocate or perform an effect** — `a + b`
has no argument position for a context. You cannot write an expensive `+` in this
language, which is why operator overloading is safe here in a way it isn't
elsewhere.
