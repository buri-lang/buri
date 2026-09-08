# Methods, and traits as interfaces

You declare a method **inside an `impl` block for its type**, with `self` as its
first parameter:

```buri
# struct Square {
#     height: Int,
#     width: Int,
# }

impl Square {
    export fn area(self): Int {
        self.height * self.width
    }
}
```

`x.f(a)` then looks `f` up among the methods of x's type, which live in that
type's **defining module** and nowhere else. No dispatch, no vtable, no hidden
receiver: a name resolved through a type instead of through scope, so a type's
operations travel with it.

```buri ignore why="names a module in another repository, so it cannot be compiled standalone; the same pattern is compiled in cli/tests/example"
from "//lib/square" import { Square };            // the type — not `area`, not `scaled`
sq.scaled(2).area()                      // both resolve with no further imports
```

Resolution stays one type, one module, one lookup.

A **trait is an interface**, and conformance is **nominal**: a type satisfies it
only where an `impl` or `derive` says so, never by accident of shape. `Ordered` is
one such interface, declared in the prelude as:

```buri sig
trait Ordered {
    fn compare(self, other: Self): Order;
}
```

and a type takes it on in one of two ways:

```buri
# struct Version(Int);

# derive Equal, Ordered, Show for Playlist; // generates them structurally
struct Playlist(Int);

impl Ordered for Version {
    // supplies the methods, checked against the trait
    fn compare(self, other: Version): Order {
        self.0.compare(other.0)
    }
}
```

The same keyword covers both jobs: `impl Type { ... }` declares what the type
can do on its own, and `impl Trait for Type { ... }` declares what it can do as
somebody else's interface.

Because a type has exactly one defining module and conformance is declared,
there is exactly one candidate per `(trait, type)`: coherence, orphan rules and
instance search are unrepresentable rather than restricted. Blanket impls,
associated types, `where` clauses, supertraits and trait objects are all
deliberately absent, because each turns resolution from a lookup into a
search.

Operators are trait methods, which is what makes newtypes usable:

```buri
derive Add, Subtract, Ordered, Show for Meters;
struct Meters(F64);

# fn demo(): Meters {
    let total = Meters(1.5) + Meters(2.0); // Meters
    let bad = Meters(1.5) + 2.0; // ERROR: expected `Meters`, found `Float`
#     total
# }
```

An operator implementation **cannot allocate or perform an effect**, because
`a + b` has no argument position for a context. You cannot write an expensive
`+` in this language.
