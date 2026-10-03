---
title: An entry has the signature its platform declares
message: '`{entry}` {requirement}'
---
# An entry has the signature its platform declares

```text
error: `main` declares no generic parameters [main-signature]
```

## What to do

Give the entry the signature its platform's `platform.buri` declares.

| Platform | The entry |
|---|---|
| `native` | `fn main(host: NativeHost): Result<(), Str>` |
| `node` | `fn main(host: NodeHost): Result<(), Str>` |
| `web` | `fn main(host: WebHost): Result<(), Str>` |
| `CLOUDFLARE_WORKER` | `fn <entry>(request: Request): Response` |

## Why

The platform calls the entry, so the platform fixes its signature. A program is
handed its host, runs, and reports how it went: `.Ok(())` exits 0, `.Err(msg)`
prints `msg` to stderr and exits 1. A worker is called once per request, so the
request is the argument and the response is the answer.

Nothing calls an entry with a type argument, so none declares generic
parameters. An `outputs` entry says which function enters and which platform
fixes its signature.

## A program that provokes it

```buri fail code=main-signature
from "native" import { NativeHost };

export fn main<T>(host: NativeHost): Result<(), Str> {
    .Ok(())
}
```
