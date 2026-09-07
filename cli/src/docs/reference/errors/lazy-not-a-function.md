---
title: A lazily loaded chunk is built around a named function
message: '`load` takes the name of a function, and this is {got}'
label: nothing to move into a chunk
fix: 'name a function: `lazy.load(adminPage)`'
---
# A lazily loaded chunk is built around a named function

```text
error: `load` takes the name of a function, and this is Int [lazy-not-a-function]
```

## What to do

Write the function down somewhere and hand `load` its name.

```buri
from "core/effect" import { Stdout };
from "core/io" import * as io;
from "core/lazy" import * as lazy;

fn admin<C: Stdout>(ctx: C): () {
    io.println(ctx, "admin").ignore()
}

fn route<C: Stdout>(ctx: C): () {
    let page = lazy.load(admin);
    page(ctx)
}
```

## Why

`load` moves a function into a chunk of its own, so there has to be a function
to move. A lambda written at the call site is part of the body it sits in and
has nowhere to move from. A local holding a function value is a value, and the
compiler cannot see which body it will hold.

`load<F>(f: F): F` is generic, because what it answers is what it was handed. So
the type cannot refuse anything here, and this is the check that does.

## A program that provokes it

```buri fail code=lazy-not-a-function
# from "core/lazy" import * as lazy;

fn eager(): Int {
    let f = lazy.load(3);
    f
}
```
