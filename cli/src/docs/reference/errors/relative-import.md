---
title: Every module path is absolute
message: "{path}" is a relative module path
note: every module path is absolute, so a path means the same module wherever it is written and a file can move without its imports changing
fix: write the absolute path: `"core/..."` for the standard library, `"//..."` for this repository
---
# Every module path is absolute

```text
error: "./helper" is a relative module path [relative-import]
```

## What to do

Write the absolute path: `"core/..."` for the standard library, `"//..."` for
this repository.

## A program that provokes it

```buri fail code=relative-import
from "./helper" import { thing };
```
