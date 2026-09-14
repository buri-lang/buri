---
title: A call passes the arguments the value's type declares
message: expected {expected} arguments, found {found}
fix: 'pass the following arguments: {type}'
---
# A call passes the arguments the value's type declares

## A program that provokes it

```buri fail code=argument-count-mismatch
fn go(): Int {
    let f = fn(a: Int): Int => a;
    f(1, 2)
}
```
