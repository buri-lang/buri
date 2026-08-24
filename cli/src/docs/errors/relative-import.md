# Every module path is absolute

```text
error: "./helper" is a relative module path [relative-import]
```

## What to do

Write the absolute path: `"core/..."` for the standard library, `"//..."` for
this repository.

## Why

Every module path means the same module wherever it is written, so a file can
be moved between directories without its own imports changing — which is what
lets `buri gen` rewrite a build file without touching source.

## A program that provokes it

```buri fail code=relative-import
from "./helper" import { thing };
```
