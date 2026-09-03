---
title: A call passes exactly the arguments the function declares
message: '`{function}` takes {expected} arguments, but {given} were given'
fix: pass exactly {expected}
---

```buri fail code=wrong-argument-count
fn add(a: Int, b: Int): Int {
    a + b
}

fn go(): Int {
    add(1)
}
```
