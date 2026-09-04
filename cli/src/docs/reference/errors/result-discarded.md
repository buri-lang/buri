---
title: A `Result` may not be discarded
message: a `Result` may not be discarded
fix: consume it: `?` to propagate, `match` to handle both cases, `.withDefault(...)` to supply one — or, when you really mean to drop it, the explicit and greppable `.ignore()`
---
# A `Result` may not be discarded

```text
error: a `Result` may not be discarded [result-discarded]
```

## What to do

Consume it: `?` to propagate, `match` to handle both cases, `.withDefault(...)`
to supply one — or, when you really mean to drop it, the explicit and greppable
`.ignore()`.

## Why

A `Result` can be thrown away in two places and no third: bound to a `_` in a
`let`, or left standing as an expression statement. Both are this error, so
must-use is total rather than a convention — and `.ignore()` is then the single
spelling of a deliberate drop, which `buri lint` reports as `discarded-result`.

The `_` is looked for anywhere in the pattern rather than only at its head. A
`let (count, _) = (1, mayFail());` drops the failure exactly as thoroughly as a
`let _ =` does, and a rule that read only the head would be a rule with a
one-character way around it.

## A program that provokes it

```buri fail code=result-discarded
# from "core/effect" import { Alloc, Stdout };
# from "core/fs" import * as fs;
# from "core/fs" import { FsRead };
# from "core/host" import * as host;
# from "core/path" import * as path;

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        FsRead: host.fs,
        Stdout: host.stdout,
    };
    let _ = fs.readText(ctx, path.of(ctx, "config.toml"));
    .Ok(())
}
```
