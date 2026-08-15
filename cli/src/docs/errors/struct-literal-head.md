# A struct literal is headed by a type

```text
error: the head of a struct literal must be a type [struct-literal-head]
```

## What to do

name the type, as in `Point { x: 1, y: 2 }`, or `.Variant { ... }` where the expected type is known

## Why

the grammar permits `f(x) { a: 1 }`; the checker does not

## A program that provokes it

```buri fail code=struct-literal-head
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

struct Holder { export a: Int }

fn identity(n: Int): Int { n }

fn build(): Int {
  let h = identity(1) { a: 1 };
  h.a
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${build()}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `struct-literal-head` — so
this page cannot describe an error the compiler has stopped emitting.
