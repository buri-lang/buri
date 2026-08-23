# Type arguments qualify a function, not a value

```text
error: explicit type arguments qualify a function or a call [type-args-on-a-value]
```

## What to do

Attach the type arguments to the function being called — `f<Str>(x)` — or, if
a comparison was meant, remember that comparisons do not chain: `a < b && b > c`.

## Why

`a < Int > (c)` parses as type arguments applied to `a` because the comparison
reading is not available — comparison operators are non-associative, so
`x < y > z` has no meaning as a comparison. Type arguments name *which*
instantiation of a generic function to use, so the thing to their left must be
a function.

## A program that provokes it

```buri fail code=type-args-on-a-value
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

fn f(a: Int, c: Int): Bool {
  a < Int > (c)
}

export fn main(): Result<(), Str> {
  context ctx {
    Alloc: host.alloc,
    Stdout: host.stdout,
  }
  let _ = ctx.println("${f(1, 2)}");
  .Ok(())
}
```
