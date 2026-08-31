---
title: An `impl` or a `derive` lives in its type's own module
message: "`{name}` is not declared in this module"
---
```buri fail code=impl-outside-its-module
# from "core/effect" import { Region };
# from "core/order" import { Ord, Order };

impl Ord for Region {
    fn compare(self, other: Region): Order {
        .Equal
    }
}
```
