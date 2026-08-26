---
title: An `impl` supplies each method once
message: '`{method}` is supplied twice'
fix: delete one of the two
---

```buri fail code=method-supplied-twice
struct Point { export x: Int }

trait Measurable { fn size(self: Self): Int; }

impl Measurable for Point {
  fn size(self: Point): Int { self.x }
  fn size(self: Point): Int { self.x }
}
```
