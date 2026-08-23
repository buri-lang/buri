# A literal must fit the type it is pinned to

```text
error: 18_446_744_073_709_551_616 is not representable in `U64` [literal-out-of-range]
```

## What to do

write a value inside `U64`'s range, or annotate a wider type

## Why

`U64` holds 0 to 18446744073709551615

## A program that provokes it

```buri fail code=literal-out-of-range
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let w: U64 = 18_446_744_073_709_551_616;
  let _ = ctx.println("${w}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `literal-out-of-range` — so
this page cannot describe an error the compiler has stopped emitting.
