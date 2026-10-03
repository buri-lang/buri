---
title: An `impl` supplies only its trait's methods
message: '`{trait}` declares no method `{method}`'
---

```buri fail code=impl-unknown-method
struct Point {
    export x: Int,
}

trait Measurable {
    fn size(self): Int;
}

impl Measurable for Point {
    fn size(self): Int {
        self.x
    }

    fn extra(self): Int {
        self.x
    }
}
```
