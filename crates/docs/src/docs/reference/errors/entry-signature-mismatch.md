---
title: An entry has the signature its platform declares
message: '`{entry}` must {requirement}'
---
# An entry has the signature its platform declares

```text
error: `main` must declare no generic parameters [entry-signature-mismatch]
```

## What to do

Give the entry the signature its platform's `platform.buri` declares.

| Platform | The entry |
|---|---|
| `native` | `fn main(host: NativeHost): Result<(), Str>` |
| `node` | `fn main(host: NodeHost): Result<(), Str>` |
| `web` | `fn main(host: WebHost): Result<(), Str>` |
| `//platform/<name>` | the signature its `platform.buri` declares |

## Why

The platform calls the entry, so the platform fixes its signature, and nothing
calls an entry with a type argument. A repository platform's entry without a
`js` file starts itself, so it takes only its host and answers
`Result<(), Str>`.

## A program that provokes it

```buri fail code=entry-signature-mismatch
from "native" import { NativeHost };

export fn main<T>(host: NativeHost): Result<(), Str> {
    .Ok(())
}
```
