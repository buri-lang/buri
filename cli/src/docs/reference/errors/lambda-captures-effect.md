---
title: A lambda may not capture an effect
message: a lambda may not capture `{name}`, which carries an effect
note: a lambda that could close over authority would make the capture rule meaningless
fix: thread the context through a `*Ctx` combinator, which passes it in as a parameter instead: `paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p))`
---
# A lambda may not capture an effect

```text
error: a lambda may not capture `ctx`, which carries an effect [lambda-captures-effect]
```

## What to do

Thread the context through a `*Ctx` combinator, which passes it in as a
parameter instead: `paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p))`.

## Why

A closure's type is all a caller can see about it, so a lambda that closed over
a context would carry authority behind a type mentioning no effect.

## A program that provokes it

```buri fail code=lambda-captures-effect
# from "core/effect" import { Allocator };
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead, Path };

fn checkAll<C: Allocator + FileSystemRead>(ctx: C, paths: [Path]): [Bool] {
    paths.map(ctx, fn(p) => fs.exists(ctx, p))
}
```
