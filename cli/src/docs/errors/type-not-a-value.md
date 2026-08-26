---
title: A type's name is not a value
message: '`{name}` is a type, not a value'
fix: 'construct one, as in `Point {{ x: 1, y: 2 }}`, or name a value'
---

```buri fail code=type-not-a-value
struct Point { export x: Int, export y: Int }

fn go(): Int {
  let p = Point;
  1
}
```
