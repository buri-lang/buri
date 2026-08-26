---
title: A type has one method of each name
message: '`{type}` already has a method `{name}`'
fix: rename one of them
---

```buri fail code=duplicate-method
struct Point { export x: Int }

impl Point {
  fn size(self: Point): Int { self.x }
  fn size(self: Point): Int { self.x }
}
```
