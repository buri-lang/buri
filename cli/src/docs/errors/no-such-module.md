# A module path names a module that exists

```text
error: there is no module "core/lists" [no-such-module]
```

## What to do

Check the path. There are two kinds and no others: `"core/..."` and `"ui/..."`
for the standard library's two reserved roots, and `"//..."` for this
repository, from its root.

## Why

A path matching neither names nothing, and the error says so where the path is
written rather than where the missing name is later used.

## A program that provokes it

```buri fail code=no-such-module
from "core/lists" import * as lists;
```
