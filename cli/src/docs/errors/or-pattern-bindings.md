# Or-pattern alternatives bind the same names

```text
error: or-pattern alternatives must bind the same names [or-pattern-bindings]
```

## What to do

bind the same names in every alternative, or split this into separate arms

## Why

it binds `y`, which the others do not

## A program that provokes it

```buri fail code=or-pattern-bindings
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

enum Either { Left(Int), Right(Int) }

fn value(e: Either): Int {
  match (e) {
    .Left(x) | .Right(y) => x,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${value(Either.Left(1))}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `or-pattern-bindings` — so
this page cannot describe an error the compiler has stopped emitting.
