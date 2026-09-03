---
title: A derive is a fold over the type's components
message: `{type}` cannot derive `{trait}`: `{field}` has type `{field_type}`
note: a derived implementation is a fold over the type's components, and `{field_type}` does not satisfy `{trait}`
---
# A derive is a fold over the type's components

```text
error: `Outer` cannot derive `Eq`: `inner` has type `Inner` [underivable]
```

## What to do

Make `Inner` satisfy `Eq` first — `derive Eq for Inner;` in its own module, or
an `impl` — or drop `Eq` from this `derive`.

## Why

A derived implementation is exactly the fold: `Outer`'s `eq` is its fields'
`eq`. So a derive is only ever as available as the components it is built from,
and the diagnostic names the component rather than the type you wrote it on.

## A program that provokes it

```buri fail code=underivable
# from "core/order" import { Eq };

struct Inner {
    export x: Int,
}

derive Eq for Outer;
struct Outer {
    export inner: Inner,
}
```
