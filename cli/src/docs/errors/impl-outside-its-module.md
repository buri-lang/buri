---
title: An `impl` or a `derive` lives in its type's own module
message: "`{name}` is not declared in this module"
---
```buri fail code=impl-outside-its-module
# from "core/effect/lib.buri" import { Region };
# from "core/order/lib.buri" import { Ord, Order };
impl Ord for Region {
  fn compare(self, other: Region): Order {
    .Equal
  }
}
```
