---
title: A struct literal gives each field once
message: 'field `{field}` is given twice'
fix: delete one of the two
---

```buri fail code=duplicate-field-initializer
struct Point {
    export x: Int,
    export y: Int,
}

fn go(): Point {
    Point { x: 1, x: 2, y: 3 }
}
```
