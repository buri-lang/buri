---
title: Every type name resolves to a declaration
message: there is no type `{name}`
fix: declare it, import it, or correct the spelling
---
# Every type name resolves to a declaration

```text
error: there is no type `Widgett` [unresolved-type]
```

## What to do

Correct the spelling, or bring the type into scope — a module declares it or an
import names it, and there is no third way. Where there is a near miss the fix
names it, and a type the standard library renamed gets the answer instead:
`OrdMap` was renamed to `OrderedMap`.

## Why

Types are nominal throughout. There is no structural fallback and no inference
from shape, so a misspelling cannot quietly become a different type that happens
to fit.

## A program that provokes it

```buri fail code=unresolved-type wrap=body
let n: Widgett = 1;
```
