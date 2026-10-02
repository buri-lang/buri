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
from "platform/effect" import { Allocator, Stdout };
from "core/io" import * as io;
from "node" import { NodeHost };

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    io.println(ctx, "hello").mapErr(fn(_e) => "could not write")
}
```

A test binds test implementations from `platform/effect/testing` instead.

## Why

`platform/host` holds the structs a platform lists as its host type's fields.
Anything that could import them could mint authority, so only a platform's
`platform.buri` may. `core/host` is the old way in and is kept only for a
`CLOUDFLARE_WORKER` entry, whose `fetch` takes no host yet.

## A program that provokes it

```buri fail code=host-import-outside-platform
from "platform/host" import { HostStdout };
```
