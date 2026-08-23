# Every module path is absolute

```text
error: "./helper" is a relative module path [relative-import]
```

## What to do

write the absolute path: `"core/..."` for the standard library, `"//..."` for this repository

## Why

every module path is absolute, so a path means the same module wherever it is written and a file can move without its imports changing

## A program that provokes it

```buri fail code=relative-import
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "./helper" import { thing };

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `relative-import` — so
this page cannot describe an error the compiler has stopped emitting.
