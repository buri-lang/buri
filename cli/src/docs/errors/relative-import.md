# Every module path is absolute

```text
error: "./helper" is a relative module path [relative-import]
```

## What to do

write the absolute path: `"core/..."` for the standard library, `"//..."` for this repository

## Why

every module path is absolute, so a path means the same module wherever it is written and a file can move without its imports changing

## A program that provokes it

```buri fail code=relative-import
from "./helper" import { thing };
```
