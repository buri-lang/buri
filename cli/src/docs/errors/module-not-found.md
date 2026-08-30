---
title: A module path names exactly one file
message: {problem}
fix: create the file the path names, or correct the path — a module path maps to exactly one file, with no search
---
# A module path names exactly one file

```text
error: "//cmd/discarded_result" names no file (cmd/discarded_result/lib.buri) [module-not-found]
```

## What to do

Create the file the path names, or correct the path.

## Why

A module path maps to exactly one file, with no search path and no fallback, so
there is never a question of which of two candidates a path meant.

## A program that provokes it

A module path resolves against the repository, so this one is compiled against
the worked monorepo in `cli/tests/example`, where `//lib/nope` does not exist.

```buri fail code=module-not-found repo=cli/tests/example
from "//lib/nope/lib.buri" import { Nope };

export fn main(): Result<(), Str> {
  .Ok(())
}
```
