# The grammar expected something else here

```text
error: expected a pattern, found `..` [unexpected-token]
```

## What to do

write a pattern: a binding, a literal, `.Variant`, or `_`

## A program that provokes it

```buri fail code=unexpected-token
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn middle(xs: [Int]): Int {
  match (xs) {
    [..a, m, ..b] => m,
    _ => 0,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${middle([1, 2, 3])}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `unexpected-token` — so
this page cannot describe an error the compiler has stopped emitting.
