---
title: Some traits are derived, never implemented
message: `{trait}` is derived, not implemented
note: '{reason}'
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

`core/json`'s `ToJson` and `FromJson`, and `core/flags`'s `Flags`, are always
derived.

One runtime walker encodes every derived type by its shape. So encoding a
`Date` directly would call a hand-written `impl ToJson for Date`, but encoding
an `Appointment` that holds a `Date` would walk straight past it. For a
different document, convert to another type first, through a function visible
at the call site.

`Flags`'s operations work on the word `derive Flags` packs the fields into, so
there is nothing for an `impl` to say.
