---
title: An entry takes the host of the platform it is built for
message: '`{entry}` takes `{taken}`, and the {platform} platform hands its entry a `{wanted}`'
---
# An entry takes the host of the platform it is built for

```text
error: `main` takes `NodeHost`, and the native platform hands its entry a `NativeHost` [entry-host-mismatch]
```

## What to do

Take the host of the platform the output names:

```buri
from "native" import { NativeHost };

export fn main(host: NativeHost): Result<(), Str> {
    .Ok(())
}
```

A program built for two platforms gives each output an entry of its own, and
both delegate to one function that takes `ctx`:

```textproto schema=build
binary {
    outputs: [
        { platform: "native", variant: "linux-arm64" },
        { platform: "node", entries { main: "mainForNode" } },
    ]
}
```

## Why

Each platform offers different effects, so each declares its own host type. One
entry can't take two.

## A program that provokes it

```buri fail code=entry-host-mismatch platform=node
from "native" import { NativeHost };

export fn main(host: NativeHost): Result<(), Str> {
    .Ok(())
}
```
