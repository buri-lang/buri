---
title: An expression statement is legal only in a test
message: an expression statement is legal only in a test source
note: a block is `let`s followed by a result expression
fix: bind it: `let _ = ...;`, or make it the block's result expression
---
# An expression statement is legal only in a test

```text
error: an expression statement is legal only in a test source [expression-statement]
```

## What to do

Bind it — `let _ = ...;` — or make it the block's result expression.

## Why

A block is `let`s followed by a result expression. There is no third statement
form. A test source is the one exception, which is what lets `assert.eq(...)`
stand alone. There, any expression of type `()` may stand alone, terminated by
`;` — a `match` or an `if` whose branches all assert included.

Between this rule and `result-discarded`, a value can be thrown away in exactly
two places: bound to a `_`, or left standing. Both refuse a `Result`, which is
what makes must-use total rather than a convention. A statement whose type is
`Result` is therefore *both* errors at once, and the fix has to answer both.
`.ignore()` alone settles the type and leaves the statement standing. `let _ =
...;` alone binds a `Result` that may not be dropped. The edit is the two
together, which is what a program printing a line writes:

```buri role=entry
# from "core/effect" import { Alloc, Stdout };
# from "core/host" import * as host;
# from "core/io" import * as io;

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
    };
    let _ = io.println(ctx, "ready").ignore();
    .Ok(())
}
```

## A program that provokes it

```buri fail code=expression-statement wrap=body effects=Stdout,Alloc
io.println(ctx, "ready");
```
