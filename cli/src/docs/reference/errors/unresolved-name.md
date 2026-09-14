---
title: Every name resolves to a declaration
message: there is nothing named `{name}` in scope
---
# Every name resolves to a declaration

```text
error: there is nothing named `duoble` in scope [unresolved-name]
```

## Why

There is no prelude and no ambient scope. A module's available names are the
ones it declares plus the ones its own imports name, which is what makes the
suggestion trustworthy.

## A program that provokes it

```buri fail code=unresolved-name
fn twice(n: Int): Int {
    duoble(n)
}
```

```buri fail code=unresolved-name
fn root(x: Float): Float {
    sqrt(x)
}
```
