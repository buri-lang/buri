---
title: An entry takes its platform's host
message: '`{entry}` takes no host'
note: the host is the platform's effects, one field each, and the program binds the ones it needs
---
# An entry takes its platform's host

```text
error: `main` takes no host [entry-missing-host]
fix: take the platform's host and bind its fields:
     export fn main(host: NativeHost): Result<(), Str> {
         run(context { Allocator: host.alloc, Stdout: host.stdout })
     }
```

## What to do

Import the host type from the platform and take it as the entry's one
parameter:

```buri
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "hello").mapErr(fn(_e) => "could not write")
}
```

The host types are `NativeHost`, `NodeHost` and `WebHost`, from `"native"`,
`"node"` and `"web"`. A field is named after the effect it implements:
`host.fs`, `host.env`, `host.clock`.

## Why

An entry used to build its context from `core/host`, now retired, which
exported names half the platforms lacked. A host type lists exactly what its platform offers, so the
type checker says which effects a program can bind.

## A program that provokes it

```buri fail code=entry-missing-host
export fn main(): Result<(), Str> {
    .Ok(())
}
```
