---
title: The defining module of `[T]` is `core/list`
message: the defining module of `[T]` is `core/list`
fix: write a free function over the array instead
---

```buri fail code=array-impl-outside-core-list
impl [Int] {
  fn total(self: [Int]): Int { 0 }
}
```
