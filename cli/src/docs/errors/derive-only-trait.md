# Some traits are derived, never implemented

```text
error: `ToJson` is derived, not implemented [derive-only-trait]
```

## What to do

Write `derive ToJson for Point;` instead.

## Why

A derived encoder stands for the type's *shape*, which one walker in the
runtime reads — so a hand-written `impl ToJson for Date` would be called by
`json.encode(ctx, date)` and walked straight past by
`json.encode(ctx, appointment)`, where `Appointment` holds a `Date` and derives
its own. One value, two encodings, depending on where it appeared. So there is
one encoding and it is the derived one; a type that needs a different document
is a type you convert to first, which is a function visible at the call site.

`core/json`'s `ToJson` and `FromJson` are the only two traits this applies to.

## A program that provokes it

```buri fail code=derive-only-trait
# from "core/effect" import { Alloc };
# from "core/json" import { Json, ToJson };
struct Point { export x: Int, export y: Int }

impl ToJson for Point {
  fn toJson<C: Alloc>(self: Point, ctx: C): Json {
    Json.Num(0.0)
  }
}
```
