# `ctx` comes first, or immediately after `self`

```text
error: `ctx` must come first, or immediately after `self` [ctx-not-first]
```

## What to do

Move `ctx` to that position.

## Why

The position is the whole of what makes effectfulness readable: a reader
answers "can this function touch the world?" from the first two parameters and
stops. A `ctx` in fourth place would make that a question about the whole
signature.

## A program that provokes it

```buri fail code=ctx-not-first
# from "core/effect" import { Stdout };
# from "core/io" import * as io;
fn shout<C: Stdout>(times: Int, ctx: C): () {
  io.println(ctx, "loud")
}
```
