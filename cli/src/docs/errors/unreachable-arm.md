# Every arm must be reachable

```text
error: this arm is unreachable [unreachable-arm]
```

## What to do

delete it, or move it above the arm that subsumes it

## Why

the arms before it already cover everything it matches

## A program that provokes it

```buri fail code=unreachable-arm
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

fn describe(o: Option<Int>): Int {
  match (o) {
    anything => 1,
    .None => 0,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${describe(.None)}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `unreachable-arm` — so
this page cannot describe an error the compiler has stopped emitting.
