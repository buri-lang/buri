---
title: Every module path names a file
message: "{path}" names no file
note: a module is a file, so its path is that file's name — there is one spelling of a module and it is the one you would type into an editor
fix: name the file: `"{path}/lib.buri"` for a library's surface, `"{path}.buri"` for a module inside one
---
# Every module path names a file

```text
error: "//lib/money" names no file [import-path-without-a-file]
  = note: write "//lib/money/lib.buri"
```

## What to do

Add the file name the path already meant.

| what it names | write |
|---|---|
| a library's surface | `"//lib/money/lib.buri"` |
| a module inside a library | `"//lib/money/cents.buri"` |
| a testing surface | `"//lib/ledger/testing/lib.buri"` |
| a binary's entry point | `"//cmd/app/main.buri"` |
| a standard library module | `"core/list/lib.buri"`, `"ui/node/lib.buri"` |
| a schema | `"//proto/address.proto"` — already a file, unchanged |

Inside a repository the compiler works out which of those the old spelling
meant and says so in a note, and `buri lint --fix` and the editor's quick fix
will make the edit for you.

## Why

A module used to have two names: the file `lib/money/cents.buri` on disk and
the path `//lib/money/cents` in an import, with a rule connecting them that you
had to know — a library was its directory, a module inside one dropped its
extension, and a testing surface was a directory that meant a file two levels
down. Three spellings for three kinds of the same thing.

Now the path is the file. `//` is the repository root, `core/` and `ui/` are the
standard library, and everything after that is a path you could hand to any
other tool. Go-to-definition, a file rename, a grep for who imports this file,
and the error above all become the same question with one answer.

## A program that provokes it

```buri fail code=import-path-without-a-file
from "core/list" import { map };
```
