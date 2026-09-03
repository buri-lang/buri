---
title: A literal gives every required field a value
message: '`{name}` is missing {fields}'
---

A field whose declared type is `Option<...>` is not required: leaving it out is
writing `.None` for it, so only the other fields are named here.

```buri fail code=missing-field-value
struct Point {
    export x: Int,
    export y: Int,
    export label: Option<Str>,
}

fn go(): Point {
    Point { x: 1 }
}
```
