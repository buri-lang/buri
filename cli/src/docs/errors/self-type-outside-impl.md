# `Self` names the implementing type

```text
error: `Self` is legal only inside a `trait` or `impl` [self-type-outside-impl]
```

## What to do

name the type itself here

## Why

`Self` stands for the implementing type, and there is none here

## A program that provokes it

```buri fail code=self-type-outside-impl
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn identity(x: Int): Self { x }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `self-type-outside-impl` — so
this page cannot describe an error the compiler has stopped emitting.
