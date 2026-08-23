# `if` is an expression, so it needs an `else`

```text
error: `if` requires an `else` branch [if-without-else]
```

## What to do

add `else { ... }`; an `if` is an expression, so it has a value either way

## Why

`if` is an expression, so both branches must produce a value of the same type

## A program that provokes it

```buri fail code=if-without-else
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

fn sign(n: Int): Int {
  let label = if (n > 0) { 1 };
  label
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${sign(1)}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `if-without-else` — so
this page cannot describe an error the compiler has stopped emitting.
