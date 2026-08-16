# Every type name resolves to a declaration

```text
error: there is no type `Widgett` [unresolved-type]
```

## What to do

declare it, import it, or correct the spelling

## Why

A type is nominal: it has a declaration, and a name that resolves to none is
not a type this program has. There is no structural fallback and no inference
from shape, so a misspelling cannot quietly become a different type.

## A program that provokes it

```buri fail code=unresolved-type
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let n: Widgett = 1;
  let _ = ctx.println("${n}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces
`unresolved-type` — so this page cannot describe an error the compiler has
stopped emitting.
