# A lambda may not capture an effect

```text
error: a lambda may not capture `ctx`, which carries an effect [lambda-captures-effect]
```

## What to do

thread the context through a `*Ctx` combinator, which passes it in as a parameter instead: `paths.mapCtx(ctx, fn(c, p) => fs.readText(c, p))`

## Why

a lambda that could close over authority would make the capture rule meaningless

## A program that provokes it

```buri fail code=lambda-captures-effect
from "core/effect" import { Alloc, Fs, Stdout };
from "core/host" import * as host;
from "core/fs" import * as fs;
from "core/list" import * as list;

fn checkAll<C: Alloc + Fs>(ctx: C, paths: [Str]): [Bool] {
  paths.map(ctx, fn(p) => fs.exists(ctx, p))
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Fs: host.fs, Stdout: host.stdout };
  let found = checkAll(ctx, ["a"]).len();
  let _ = ctx.println("${found}");
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `lambda-captures-effect` — so
this page cannot describe an error the compiler has stopped emitting.
