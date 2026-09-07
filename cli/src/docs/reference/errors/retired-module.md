---
title: A renamed standard library module is imported by its new name
message: '"{path}" is now "{now}"'
note: the abbreviation was renamed rather than kept beside the new name, so one module has one path and two imports of it are the same import
fix: write "{now}" in the import
---
# A renamed standard library module is imported by its new name

```text
error: "core/char" is now "core/character" [retired-module]
```

## What to do

Write the new path. Nothing else moves — the module has the same exports, and
the alias is its last segment as it is for every other one:

```buri
from "core/character" import * as character;

export fn hexDigit(n: Int): Char {
    character.fromDigit(n, 16).withDefault('0')
}
```

Two have been renamed so far: `core/char` is `core/character`, and `core/proc`
is `core/process`.

## Why

The old path is gone rather than kept as a second spelling. An alias would mean
one module with two names, and every reader of an import would have to know
they were the same — which is the thing a rename is meant to stop.

## A program that provokes it

```buri fail code=retired-module
from "core/char" import * as character;
```
