---
title: A call passes the arguments the value's type declares
message: expected {expected} arguments, found {found}
fix: 'pass the following arguments: {type}'
---
# A call passes the arguments the value's type declares

## What to do

Line the call up against the type the fix prints. A value's type has parameters
and no names for them, so where the argument types say which position was left
out the fix says that position, and where the call lines up two ways, or more
than one position has nothing to fill it, it says none of them. A declaration
counted the same way says more, because it has names to say it with —
`wrong-argument-count` is that page.

### The position, where the types say which

```text
error: expected 3 arguments, found 2 [argument-count-mismatch]
   = fix: the second argument is missing: fn(Int, Str, Bool) => Int
```

### The type alone, where they do not

```text
error: expected 3 arguments, found 2 [argument-count-mismatch]
   = fix: pass the following arguments: fn(Int, Str, Bool) => Int
```

## A program that provokes it

```buri fail code=argument-count-mismatch
fn go(): Int {
    let f = fn(a: Int): Int => a;
    f(1, 2)
}
```
