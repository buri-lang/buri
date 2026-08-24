# Every name resolves to a declaration

```text
error: there is nothing named `duoble` in scope [unresolved-name]
```

## What to do

Correct the spelling, or declare it. The diagnostic offers the nearest name in
scope.

## Why

There is no prelude and no ambient scope: the names available in a module are
the ones it declares plus the ones its own imports name. That is what makes the
suggestion trustworthy — the set it is drawn from is exactly the set the file
put there.

## A program that provokes it

```buri fail code=unresolved-name
fn twice(n: Int): Int {
  duoble(n)
}
```
