---
title: A derived trait holds for every field
message: `{type}` cannot derive `{trait}`: `{field}` has type `{field_type}`
note: a derived implementation is a fold over the type's components, and `{field_type}` does not satisfy `{trait}`
---
# A derived trait holds for every field

```text
error: `Outer` cannot derive `Equal`: `inner` has type `Inner` [underivable-field]
```

```buri fail code=underivable-field
# from "core/order" import { Equal };

struct Inner {
    export x: Int,
}

derive Equal for Outer;
struct Outer {
    export inner: Inner,
}
```

Make the component's type satisfy the trait first, with a `derive` in its own
module or an `impl`, or drop the trait from this `derive`.
