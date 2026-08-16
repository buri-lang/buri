# A module path names a module that exists

```text
error: there is no module "core/lists" [no-such-module]
```

## What to do

check the path; the standard library's modules are all `core/...`

## Why

There are two kinds of module path and no others: `"core/..."` for the standard
library, which ships with the toolchain, and `"//..."` for this repository,
from its root. A path that matches neither names nothing, and the error says so
where it is written rather than where the missing name is later used.

## A program that provokes it

```buri fail code=no-such-module
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/lists" import * as lists;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces
`no-such-module` — so this page cannot describe an error the compiler has
stopped emitting.
