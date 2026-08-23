# A rest pattern comes last

```text
error: a rest pattern must come last [rest-pattern-not-last]
```

## What to do

move `..` to the end, as in `[first, ..rest]`; matching a prefix is what an array pattern does

## Why

`[first, ..rest]` is legal; `[..init, last]` is not

## A program that provokes it

```buri fail code=rest-pattern-not-last
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

fn lastOf(xs: [Int]): Int {
  match (xs) {
    [..init, last] => last,
    _ => 0,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${lastOf([1, 2, 3])}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `rest-pattern-not-last` — so
this page cannot describe an error the compiler has stopped emitting.
