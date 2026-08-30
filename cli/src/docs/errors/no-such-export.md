---
title: A module exports what it says it exports
message: "{path}" does not export `{name}`
---
# A module exports what it says it exports

```text
error: "core/list" does not export `notAThing` [no-such-export]
```

## What to do

If the module declares the name and holds it back, add `export` to the
declaration there. If it declares no such name, the spelling is the mistake —
the diagnostic says which of the two it is, and offers the nearest exported
name.

## Why

A re-export may name only what its module path exports, so a library's surface
can never be wider than the modules it is built from.

## A program that provokes it

```buri fail code=no-such-export
from "core/list/lib.buri" export { notAThing };
```
