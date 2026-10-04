---
title: A type implements a trait once
message: '`{type}` already implements `{trait}`'
---

```buri fail code=duplicate-impl
struct Point { export x: Int }

trait Measurable { fn size(self): Int }

impl Measurable for Point { fn size(self): Int { self.x } }

impl Measurable for Point { fn size(self): Int { self.x } }
```
