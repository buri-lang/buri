# A name is declared once

```text
error: variant `Yes` is declared twice [duplicate-declaration]
```

## What to do

rename one of them; `match` tells variants apart by name

## A program that provokes it

```buri fail code=duplicate-declaration
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

enum Choice { Yes, No, Yes }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `duplicate-declaration` — so
this page cannot describe an error the compiler has stopped emitting.
