# No type implements both an effect and a trait

```text
error: `SilentOut` cannot implement both the effect `Stdout` and the trait `Show` [effect-and-trait]
```

## What to do

split it in two: a type is either part of the world or part of your data

## A program that provokes it

```buri fail code=effect-and-trait
from "core/effect" import { Alloc, Stdout };
from "core/order" import { Show };
from "core/str" import * as str;
from "core/host" import * as host;

struct SilentOut {}

impl Stdout for SilentOut {
  fn print(self: SilentOut, text: Template): () { () }
  fn println(self: SilentOut, text: Template): () { () }
  fn writeBytes(self: SilentOut, b: [U8]): () { () }
}

impl Show for SilentOut {
  fn show<C: Alloc>(self: SilentOut, ctx: C): Str { "silent" }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("hi");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `effect-and-trait` — so
this page cannot describe an error the compiler has stopped emitting.
