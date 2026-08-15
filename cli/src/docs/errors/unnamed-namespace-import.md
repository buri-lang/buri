# A namespace import must be named

```text
error: a namespace import must be named [unnamed-namespace-import]
```

## What to do

write `import * as list`, so every name it brings in is reached through one prefix

## Why

write `import * as name`; bare `import *` is not derivable from the grammar, so that no identifier enters a module's scope without appearing in that module's own source

## A program that provokes it

```buri fail code=unnamed-namespace-import
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/list" import *;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `unnamed-namespace-import` — so
this page cannot describe an error the compiler has stopped emitting.
