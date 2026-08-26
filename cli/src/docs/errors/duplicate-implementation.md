---
title: There is one implementation per trait and type
message: '`{type}` already implements `{trait}`'
---

```buri fail code=duplicate-implementation
struct Point { export x: Int }

trait Measurable { fn size(self): Int }

impl Measurable for Point { fn size(self): Int { self.x } }

impl Measurable for Point { fn size(self): Int { self.x } }
```
