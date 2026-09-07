---
title: No type implements both an effect and a trait
message: `{type}` cannot implement both the effect `{effect}` and the trait `{trait}`
fix: split it in two: a type is either part of the world or part of your data
---
# No type implements both an effect and a trait

```text
error: `SilentOut` cannot implement both the effect `Stdout` and the trait `Show` [effect-and-trait]
```

## What to do

Split it in two: a type is either part of the world or part of your data.

## Why

Keeping the two kinds of interface apart is what stops a type parameter bounded
by an ordinary trait from ever being a context type. That is why
`xs.any(fn(x) => x == needle)` is legal while a lambda capturing a context is
not.

## A program that provokes it

```buri fail code=effect-and-trait
# from "core/effect" import { Alloc, IoError, Stdout };
# from "core/order" import { Show };

struct SilentOut {}

impl Stdout for SilentOut {
    fn print(self, text: Template): Result<(), IoError> {
        .Ok(())
    }

    fn println(self, text: Template): Result<(), IoError> {
        .Ok(())
    }

    fn writeBytes(self, b: [U8]): Result<(), IoError> {
        .Ok(())
    }
}

impl Show for SilentOut {
    fn show<C: Alloc>(self, ctx: C): Str {
        "silent"
    }
}
```
