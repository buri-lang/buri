# A `test` lives in a test source

```text
error: a `test` declaration is legal only in a test source [test-outside-test-source]
```

## What to do

move it into a file listed in the target's `test.sources`

## Why

a module is a test source because a rule lists it in `test.sources`; that is the only thing that makes one

## A program that provokes it

```buri fail code=test-outside-test-source
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

test "a test in a binary source" {
  let n = 1;
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `test-outside-test-source` — so
this page cannot describe an error the compiler has stopped emitting.
