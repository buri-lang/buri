---
title: Only a platform names the backends' production values
message: '"{path}" is importable only by a platform'
note: a production value is authority, and an entry receives it through its host rather than importing it
fix: take the platform's host as the entry's parameter, `export fn main(host: NativeHost)`, and bind its fields
---
# Only a platform names the backends' production values

```text
error: "platform/host" is importable only by a platform [host-import-outside-platform]
```

## What to do

Take the host as the entry's parameter and bind its fields:

```buri
from "core/io" import * as io;
from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "hello").mapErr(fn(_e) => "could not write")
}
```

A test binds test implementations from `platform/effect/testing` instead.

## A program that provokes it

```buri fail code=host-import-outside-platform
from "platform/host" import { HostStdout };
```
