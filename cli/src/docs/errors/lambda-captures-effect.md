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

A closure's type is the whole of what a caller can see about it. A lambda that
had closed over a context would carry authority behind a type mentioning no
effect, and the guarantee that a signature says what a function may do would
hold for every function except the ones written as lambdas.

## A program that provokes it

```buri fail code=lambda-captures-effect
# from "core/effect/lib.buri" import { Alloc, Fs };
# from "core/fs/lib.buri" import * as fs;
fn checkAll<C: Alloc + Fs>(ctx: C, paths: [Str]): [Bool] {
  paths.map(ctx, fn(p) => fs.exists(ctx, p))
}
```
