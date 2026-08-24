# A module exports what it says it exports

```text
error: "core/list" does not export `notAThing` [no-such-export]
```

## What to do

Add `export` to the declaration in the module the path names, or drop the name
from this list.

## Why

A re-export may name only what its module path exports, so a library's surface
can never be wider than the modules it is built from.

## A program that provokes it

```buri fail code=no-such-export
from "core/list" export { notAThing };
```
