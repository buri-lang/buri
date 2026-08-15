# A bound names a trait or an effect

```text
error: `Bogus` is not a trait or effect [not-a-trait]
```

## What to do

name a declared trait or effect, or declare `Bogus` as one

## Why

a bound names a declared trait; there are no where clauses

## A program that provokes it

```buri fail code=not-a-trait
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

fn measure<T: Bogus>(x: T): Int { 1 }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${measure(1)}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `not-a-trait` — so
this page cannot describe an error the compiler has stopped emitting.
