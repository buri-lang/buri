---
title: A renamed module is imported by its new name
message: '"{path}" is now "{now}"'
note: the old path was retired rather than kept beside the new one, so one module has one path and two imports of it are the same import
fix: write "{now}" in the import
---
# A renamed module is imported by its new name

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

Five abbreviations were replaced by the word: `core/char` is `core/character`,
`core/num` is `core/number`, `core/ordmap` is `core/orderedmap`, `core/ordset`
is `core/orderedset`, and `core/proc` is `core/process`. Four modules moved
when effects left `core/` and `ui/`: `core/effect` and `ui/effect` are
`platform/effect`, and `core/host/testing` and `ui/testing` are
`platform/effect/testing`.

`core/host` is retired rather than moved. Its values are an entry's host now:
the entry takes its platform's host type, such as `NodeHost`, and binds the
effects it needs from the host's fields. The effects themselves are declared
in `platform/effect`.

## Why

The old path is gone rather than kept as a second spelling. An alias would mean
one module with two names, and every reader of an import would have to know
they were the same — which is the thing a rename is meant to stop.

## A program that provokes it

```buri fail code=retired-module
from "core/char" import * as character;
```
