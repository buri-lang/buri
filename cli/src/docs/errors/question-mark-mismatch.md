# `?` propagates into a matching return type

```text
error: `?` on a `Result` needs a `Result` return type, not `I64` [question-mark-mismatch]
```

## What to do

return a `Result` from this function, or handle the error here with `match` or `??`

## A program that provokes it

```buri fail code=question-mark-mismatch
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn unwrap(r: Result<Int, Str>): Int {
  let n = r?;
  n
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${unwrap(.Ok(1))}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `question-mark-mismatch` — so
this page cannot describe an error the compiler has stopped emitting.
