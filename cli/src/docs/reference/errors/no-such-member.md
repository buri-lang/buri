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

`fs` in `fs.appendBytes(...)` is a namespace, not a value. It stands for the
module its import named, and what you may write after the dot is exactly what
that module exports.

A namespace qualifies a type, a bound and an `impl` head the same way it
qualifies a function, so `list.Vector<Int>` and `<T: order.Comparable>` are
answered here too. When no import bound the name at all, `fs.readText(...)` is
`unresolved-name` on `fs` instead.

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
