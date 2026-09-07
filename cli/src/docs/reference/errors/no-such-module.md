---
title: A module path names a module that exists
message: there is no module "{path}"
fix: check the path; the standard library's modules are all {roots}
---
# A module path names a module that exists

```text
error: there is no module "core/lists" [no-such-module]
```

## What to do

Check the path. There are two kinds and no others: `"core/..."`, `"ui/..."` and
`"std/..."` for the standard library's three reserved roots, and `"//..."` for
this repository, from its root. A surface is named as a module —
`"core/list"`, `"//lib/money"` — and every other module by its file, extension
and all.

The error lands where the path is written rather than where the missing name is
later used.

## A program that provokes it

```buri fail code=no-such-module
from "core/lists" import * as lists;
```
