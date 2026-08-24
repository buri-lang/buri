# A module exports what it says it exports

```text
error: "core/list" does not export `notAThing` [no-such-export]
```

## What to do

add `export` to `notAThing`'s declaration in "core/list", or drop it from this list

## Why

a re-export may name only what its module path exports

## A program that provokes it

```buri fail code=no-such-export
from "core/list" export { notAThing };
```
