---
title: Indexing is defined on arrays
message: '`{type}` cannot be indexed'
note: indexing is defined on `[T]`, and yields `Option<T>`
fix: index an array; for a tuple, write `.0`
---

```buri fail code=not-indexable
fn read(n: Int): Option<Int> {
    n[0]
}
```
