# A `.Variant` needs a known expected type

```text
error: `.Some` needs a known expected type [unannotated-variant]
```

## What to do

write the qualified form, as in `Option.Some(...)`, or annotate what this value is being used as

## A program that provokes it

```buri fail code=unannotated-variant
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

fn mystery(): Int {
  let v = .Some(3);
  match (v) {
    .Some(n) => n,
    .None => 0,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${mystery()}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `unannotated-variant` — so
this page cannot describe an error the compiler has stopped emitting.
