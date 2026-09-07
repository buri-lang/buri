---
title: A bound is named once
message: `{effect}` is bound twice
note: a spread's binding is replaced by an explicit one, but two explicit bindings of one effect are a mistake
fix: delete one of the two bindings
---
# A bound is named once

```text
error: `Alloc` is bound twice [duplicate-bound]
```

## What to do

Delete one of the two bindings.

## Why

An explicit binding replaces a spread's binding: `context { ..Fixture(),
FsRead: fs().files([]) }` is how a test overrides a default. Two explicit
bindings have no such reading, so the later one does not silently win.

## A program that provokes it

```buri fail code=duplicate-bound
# from "core/effect" import { Alloc, Stdout };
# from "core/host" import * as host;
# from "core/io" import * as io;

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Alloc: host.alloc,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```
