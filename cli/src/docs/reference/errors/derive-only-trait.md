---
title: Some traits are derived, never implemented
message: `{trait}` is derived, not implemented
note: a derived encoder is a fold over the type's shape, and would encode a hand-written one structurally rather than calling it — so an `impl` would be obeyed at the top of a document and ignored inside it
fix: write `derive {trait} for {type};` instead
---
# Some traits are derived, never implemented

```text
error: `ToJson` is derived, not implemented [derive-only-trait]
```

```buri fail code=derive-only-trait
# from "core/json" import { Json, ToJson };
# from "platform/effect" import { Allocator };

struct Point {
    export x: Int,
    export y: Int,
}

impl ToJson for Point {
    fn toJson<C: Allocator>(self, ctx: C): Json {
        Json.Num(0.0)
    }
}
```

One runtime walker encodes every derived type by its shape. So encoding a
`Date` directly would call a hand-written `impl ToJson for Date`, but encoding
an `Appointment` that holds a `Date` would walk straight past it. One value,
two encodings.

For a different document, convert to another type first, through a function
visible at the call site. Only `core/json`'s `ToJson` and `FromJson` work this
way.
