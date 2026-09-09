---
title: Every name resolves to a declaration
message: there is nothing named `{name}` in scope
---
# Every name resolves to a declaration

```text
error: there is nothing named `duoble` in scope [unresolved-name]
```

## What to do

Correct the spelling, or declare it. Where there is a near miss the fix names
it: "if you meant `double`, use that; if not, a name is in scope only from this
module's own declarations and its imports". A name the standard library renamed
is not a guess and gets the answer instead — `sqrt` was renamed to `squareRoot`.

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
