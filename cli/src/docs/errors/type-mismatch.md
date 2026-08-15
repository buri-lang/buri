# There are no implicit conversions

```text
error: expected `I64`, found `I32` [type-mismatch]
```

## What to do

convert explicitly: `.toI64()`, which is exact for every `I32`

## Why

there is no implicit promotion of any kind

## A program that provokes it

```buri fail code=type-mismatch
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn widenByAccident(a: I32, b: I64): I64 {
  a + b
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${widenByAccident(1, 2)}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `type-mismatch` — so
this page cannot describe an error the compiler has stopped emitting.
