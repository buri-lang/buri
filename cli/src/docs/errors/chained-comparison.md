# Comparison operators do not chain

```text
error: comparison operators are non-associative [chained-comparison]
```

## What to do

write `a < b && b < c` rather than `a < b < c`

## Why

write `a < b && b < c` rather than `a < b < c`

## A program that provokes it

```buri fail code=chained-comparison
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn between(a: Int, b: Int, c: Int): Bool {
  a < b < c
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${between(1, 2, 3)}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `chained-comparison` — so
this page cannot describe an error the compiler has stopped emitting.
