---
title: A module that is not a surface is named by its file
message: "{path}" names no file
note: a surface is named as a module and everything else is a file, so this path is that file's name — the one you would type into an editor, extension and all
fix: 'write the file name: `"{path}.buri"`'
reproduction: none
---
# A module that is not a surface is named by its file

```text
error: "//lib/money/cents" names no file [import-path-without-a-file]
 --> lib/money/parse.buri:5:6
  |
5 | from "//lib/money/cents" import { Cents };
  |      ^^^^^^^^^^^^^^^^^^^
  |
  = a surface is named as a module and everything else is a file, so this path
    is that file's name — the one you would type into an editor, extension and
    all
  = fix: write "//lib/money/cents.buri"
```

## What to do

Add the file name the path already meant. The compiler works out which file
that is and offers the edit, so `buri lint --fix` and the editor's quick fix
will make it for you.

## Why

An import path names one of two things, spelled differently because they are
different:

| what | written | who may write it |
|---|---|---|
| a **surface** — a library's `lib.buri`, or its `testing/lib.buri` | `"//lib/money"`, `"//lib/money/testing"`, `"core/list"` | anyone the dependency and visibility rules allow, including the package's own suite |
| a **file** inside a package | `"//lib/money/cents.buri"`, `"//cmd/app/main.buri"` | only another file of that same package |

**Their shape cannot tell them apart**, which is why the compiler resolves the
fix rather than spelling it. `"//lib/money/testing"` and `"//lib/money/cents"`
differ by one segment; what decides them is whether `lib/money/testing/lib.buri`
or `lib/money/cents.buri` is on disk.

A binary's entry point is a file for the same reason: `//cmd/app` would be that
package's `lib.buri`, and a package with only a binary has none. So you write
`"//cmd/app/main.buri"`, from that binary's own test sources and nowhere else.

A path that leaves the package and names a file inside it is `internal-import`
instead.
