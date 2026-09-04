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

A namespace qualifies a type, a bound and an `impl` head the same way it
qualifies a function, so `list.Vector<Int>` and `<T: order.Comparable>` are
answered here too. The base is the answer only when no import bound it at all —
with no namespace of that name in the file, `fs.readText(...)` is
`unresolved-name` on `fs`.

## A program that provokes it

```buri fail code=no-such-member
from "core/effect" import { Alloc };
from "core/fs" import * as fs;
from "core/fs" import { FsWrite };
from "core/path" import * as path;

export fn appendWal<C: Alloc + FsWrite>(ctx: C): Bool {
    fs.appendBytes(ctx, path.of(ctx, "wal"))
}
```
