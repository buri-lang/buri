# Every type name resolves to a declaration

```text
error: there is no type `Widgett` [unresolved-type]
```

## What to do

Declare it, import it, or correct the spelling.

## Why

Types are nominal throughout. There is no structural fallback and no inference
from shape, so a misspelling cannot quietly become a different type that
happens to fit.

## A program that provokes it

```buri fail code=unresolved-type wrap=body
let n: Widgett = 1;
```
