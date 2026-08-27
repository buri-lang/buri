---
title: An `impl` supplies the methods its trait declares
message: '`{trait}` declares no method `{method}`'
---

```buri fail code=not-a-trait-method
struct Point { export x: Int }

trait Measurable { fn size(self): Int; }

impl Measurable for Point {
  fn size(self): Int { self.x }
  fn extra(self): Int { self.x }
}
```
