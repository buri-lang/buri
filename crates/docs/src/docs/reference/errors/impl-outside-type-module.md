---
title: An `impl` or `derive` lives in its type's module
message: "`{name}` is not declared in this module"
---
```buri fail code=impl-outside-type-module
# from "core/order" import { Order, Ordered };
# from "platform/effect" import { Region };

impl Ordered for Region {
    fn compare(self, other: Region): Order {
        .Equal
    }
}
```
