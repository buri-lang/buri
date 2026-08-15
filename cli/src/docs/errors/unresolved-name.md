# Every name resolves to a declaration

```text
error: there is nothing named `d` in scope [unresolved-name]
```

## What to do

correct the spelling, or declare it

## Why

did you mean `Add`?

## A program that provokes it

```buri fail code=unresolved-name
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let d = 5 as U8;
  let _ = ctx.println("${d}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `unresolved-name` — so
this page cannot describe an error the compiler has stopped emitting.
