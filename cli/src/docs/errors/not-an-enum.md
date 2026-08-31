---
title: A `.Variant` form names an enum
message: '`{type}` is not an enum'
---

```buri fail code=not-an-enum
fn go(): [Int] {
    let xs: [Int] = .Some;
    xs
}
```
