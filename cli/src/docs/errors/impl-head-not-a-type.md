---
title: An `impl` names a declared type
message: an `impl` names a declared type
fix: name a struct or enum this module declares
---

```buri fail code=impl-head-not-a-type
trait Measurable {
    fn size(self): Int;
}

impl Measurable for [Int] {
    fn size(self): Int {
        0
    }
}
```
