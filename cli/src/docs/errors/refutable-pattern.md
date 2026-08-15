# A `let` pattern must match every value

```text
error: this pattern does not match every value of its type [refutable-pattern]
```

## What to do

use `match`, which makes you say what the other cases do

## Why

a `let` binds unconditionally, so its pattern has to be irrefutable

## A program that provokes it

```buri fail code=refutable-pattern
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn unwrap(o: Option<Int>): Int {
  let .Some(n) = o;
  n
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${unwrap(.Some(3))}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `refutable-pattern` — so
this page cannot describe an error the compiler has stopped emitting.
