# A `testing` module is reachable only from a test

```text
error: this is a test-only module [test-only-import]
```

## What to do

import it from a file listed in a target's `test.sources`, or drop the import

## Why

a path containing a `testing` segment may be imported only from a test source

//cmd/testing_import_in_program/main is not one

## A program that provokes it

```buri fail code=test-only-import
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/testing/assert" import * as assert;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `test-only-import` — so
this page cannot describe an error the compiler has stopped emitting.
