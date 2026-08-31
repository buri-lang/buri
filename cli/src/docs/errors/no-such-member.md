---
title: A namespace member is named by the module that exports it
message: "{path}" has no member `{name}`
note: the module exports {exports}
fix: correct the member name, or check `buri docs {path}`
---
# A namespace member is named by the module that exports it

```text
error: "core/fs" has no member `appendBytes` [no-such-member]
```

## What to do

Correct the member name. The diagnostic lists what the module exports, and
offers the nearest of them when the name is a near miss.

## Why

`fs` in `fs.appendBytes(...)` is a namespace, not a value: it stands for the
module its import named, and what may be written after the dot is exactly what
that module exports. So the question the compiler can answer is about the
member, and answering it about the base instead — "there is nothing named `fs`
in scope" — sends a reader hunting for a missing import when the import is the
one part that was right.

## A program that provokes it

```buri fail code=no-such-member
from "core/effect" import { Alloc, Fs };
from "core/fs" import * as fs;

export fn appendWal<C: Alloc + Fs>(ctx: C): Bool {
    fs.appendBytes(ctx, "wal")
}
```
