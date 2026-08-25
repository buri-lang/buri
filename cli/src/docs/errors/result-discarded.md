---
title: A `Result` may not be discarded
message: a `Result` may not be discarded
fix: consume it: `?` to propagate, `match` to handle both cases, `result.withDefault` to supply one — or, when you really mean to drop it, the explicit and greppable `result.ignore`
---
# A `Result` may not be discarded

```text
error: a `Result` may not be discarded [result-discarded]
```

## What to do

Consume it: `?` to propagate, `match` to handle both cases, `result.withDefault`
to supply one — or, when you really mean to drop it, the explicit and greppable
`result.ignore`.

## Why

There are no expression statements outside a test source, so `let _ =` is the
only way to discard a value at all. Closing that one hole is what makes
must-use total rather than a convention: `result.ignore` is then the single
spelling of a deliberate drop, and `buri lint` reports it as
`discarded-result`.

## A program that provokes it

```buri fail code=result-discarded
# from "core/effect" import { Alloc, Fs, Stdout };
# from "core/host" import * as host;
# from "core/fs" import * as fs;
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Fs: host.fs, Stdout: host.stdout };
  let _ = fs.readText(ctx, "config.toml");
  .Ok(())
}
```
