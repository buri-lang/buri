# Some traits are derived, never implemented

```text
error: `ToJson` is derived, not implemented [derive-only-trait]
```

## What to do

write `derive ToJson for Point;` instead

## Why

`core/json`'s `ToJson` and `FromJson` are the only two traits in the language
that may not be written by hand, and the reason is that a derived one would
quietly disagree with a hand-written one.

What a derived implementation stands for is the type's *shape*: the compiler
ships a descriptor of every type — field names, variant shapes, element types —
and one walker in the runtime turns a value into JSON by reading it. That is
what makes `derive ToJson` cost no generated code per type. It is also what
makes a hand-written `impl ToJson for Date` a trap: `json.encode(ctx, date)`
would call it, and `json.encode(ctx, appointment)` — where `Appointment` holds a
`Date` and derives `ToJson` — would walk the descriptor straight past it and
write the struct's fields instead. The same value would have two encodings
depending on where it appeared.

So there is one encoding, and it is the derived one. A type that needs a
different document is a type you convert to first, which is a function you can
see at the call site.

## A program that provokes it

```buri fail code=derive-only-trait
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/json" import { Json, ToJson };
from "core/json" import * as json;

struct Point { export x: Int, export y: Int }

impl ToJson for Point {
  fn toJson<C: Alloc>(self: Point, ctx: C): Json {
    Json.Num(0.0)
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let p = Point { x: 1, y: 2 };
  let _ = ctx.println(json.stringify(ctx, json.encode(ctx, p)));
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces
`derive-only-trait` — so this page cannot describe an error the compiler has
stopped emitting.
