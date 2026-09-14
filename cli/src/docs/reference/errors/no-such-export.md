---
title: A module exports what it says it exports
message: "{path}" does not export `{name}`
---
# A module exports what it says it exports

```text
error: "core/list" does not export `notAThing` [no-such-export]
```

## Why

A re-export may name only what its module path exports, so a library's surface
is never wider than the modules it is built from.

## A program that provokes it

```buri fail code=no-such-export
from "core/list" export { notAThing };
```
