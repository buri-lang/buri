# A derive is a fold over the type's components

```text
error: `Outer` cannot derive `Eq`: `inner` has type `Inner` [underivable]
```

## What to do

make `Inner` satisfy `Eq` first — `derive Eq for Inner;` in its own module, or an `impl` — or drop `Eq` from this `derive`

## Why

a derived implementation is a fold over the type's components, and `Inner` does not satisfy `Eq`

## A program that provokes it

```buri fail code=underivable
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/order" import { Eq };

struct Inner { export x: Int }

struct Outer { export inner: Inner }

derive Eq for Outer;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = Outer { inner: Inner { x: 1 } };
  let _ = ctx.println("${a == a}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `underivable` — so
this page cannot describe an error the compiler has stopped emitting.
