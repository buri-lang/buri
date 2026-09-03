---
title: An effect-carrying parameter is `self` or `ctx`
message: `{name}` carries an effect, so it must be named `ctx`
---
# An effect-carrying parameter is `self` or `ctx`

```text
error: `handle` carries an effect, so it must be named `ctx` [effect-param-not-ctx]
```

## What to do

Rename `handle` to `ctx` and put it first, or drop the effect bound if this
parameter is ordinary data.

## Why

A function is effectful exactly when it has a `ctx` parameter or an
effect-carrying `self`. That biconditional is what lets a reader stop after the
first two parameters, and a third spelling would cost it.

## A program that provokes it

```buri fail code=effect-param-not-ctx
# from "core/effect" import { Fs };
# from "core/fs" import * as fs;

fn sneaky<C: Fs>(a: Int, handle: C): Bool {
    fs.exists(handle, "x")
}
```
