# A struct pattern mentions every field

```text
error: this pattern does not mention `y` [missing-field-pattern]
```

## What to do

match `y` too, or end the pattern with `..` to ignore the rest

## A program that provokes it

```buri fail code=missing-field-pattern
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

struct Point { export x: Int, export y: Int }

fn xOf(p: Point): Int {
  let Point { x } = p;
  x
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${xOf(Point { x: 1, y: 2 })}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `missing-field-pattern` — so
this page cannot describe an error the compiler has stopped emitting.
