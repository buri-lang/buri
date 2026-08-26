---
title: A method called as a free function names one type
message: '`{name}` is ambiguous as a free function'
fix: call it on a receiver, which is what picks the one you mean
---

```buri fail code=ambiguous-free-function
fn size(a: Int): Int { a }

fn size(a: Str): Int { 0 }

fn go(): Int {
  let f = size;
  1
}
```
