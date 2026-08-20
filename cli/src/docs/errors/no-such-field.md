# A field is named by the type that declares it

```text
error: `Rec` has no field `f1` [no-such-field]
```

## What to do

check the spelling, or name a field the type declares

## Why

A field belongs to the type it is written in. There is no structural typing and
no inheritance, so the fields a value has are exactly the ones its declaration
lists — which is also why the diagnostic can offer the nearest name it does have.

## A program that provokes it

```buri fail code=no-such-field
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

struct Rec { export f0: Int }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let r = Rec { f0: 1 };
  let _ = ctx.println("${r.f1}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `no-such-field` — so
this page cannot describe an error the compiler has stopped emitting.
