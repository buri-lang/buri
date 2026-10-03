---
title: A call passes the arguments its callee declares
message: '{callee} takes {expected} arguments, but {given} were given'
fix: 'pass {callee} the following arguments: {signature}'
---
# A call passes the arguments its callee declares

```buri fail code=argument-count
fn add(a: Int, b: Int): Int {
    a + b
}

fn go(): Int {
    add(1)
}
```

```buri fail code=argument-count
fn go(): Int {
    let f = fn(a: Int): Int => a;
    f(1, 2)
}
```
