# A `match` covers every case

```text
error: this `match` does not cover `.Empty` [match-not-exhaustive]
```

## What to do

add an arm for `.Empty`, or a `_` arm for everything left

## Why

every `match` must cover its scrutinee's type

## A program that provokes it

```buri fail code=match-not-exhaustive
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

enum Shape { Circle(Int), Square(Int), Empty }

fn describe(s: Shape): Int {
  match (s) {
    .Circle(r) => r,
    .Square(n) => n,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${describe(Shape.Empty)}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `match-not-exhaustive` — so
this page cannot describe an error the compiler has stopped emitting.
